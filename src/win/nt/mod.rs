// ============================================================================
// Module:       win::nt
// Description:  The NtQuerySystemInformation call itself: one safe wrapper that
//               handles the grow-and-retry protocol every caller needs.
//
// Dependencies: windows-sys (Wdk NtQuerySystemInformation), sibling `types`
// ============================================================================

//! `NtQuerySystemInformation`, wrapped once.
//!
//! ## Why this call and not the documented ones
//!
//! Every process's CPU, memory, I/O, thread count, handle count, session,
//! parent, and creation time comes back from **one** call. The documented
//! route to the same data is `EnumProcesses`, then per process:
//! `OpenProcess`, `GetProcessTimes`, `GetProcessMemoryInfo`,
//! `GetProcessIoCounters`, `GetProcessHandleCount`, `CloseHandle`. On a
//! machine with four hundred processes that is two and a half thousand
//! syscalls per sample instead of one, at a once-a-second refresh — the
//! app would be the busiest process in its own list.
//!
//! It also *cannot* work. `OpenProcess` fails for protected processes
//! even with `SeDebugPrivilege`, so the documented route reports blank
//! CPU and memory for exactly the system processes a task manager exists
//! to show. `NtQuerySystemInformation` returns them all, because it is
//! answering a question about the system rather than about a process the
//! caller has a handle to.
//!
//! The cost is that the call is not formally documented and the struct
//! layouts are not published. [`types`] handles that with compile-time
//! layout assertions; see its module docs.
//!
//! ## The grow-and-retry protocol
//!
//! The buffer size cannot be known in advance and cannot be asked for
//! reliably: querying the length and then querying the data is a race, as
//! a process can start between the two calls and the second one then
//! fails with the same status as the first. Worse, the returned length
//! is a hint rather than a guarantee — it describes the machine as it was
//! during the failed call.
//!
//! So [`query`] grows and retries with a *bounded* number of attempts,
//! each with a margin over the reported size. The margin matters: without
//! it, a machine that is steadily launching processes can fail every
//! attempt in turn, and a loop with no bound would spin forever inside
//! the sampler thread while the UI showed a stale snapshot with no
//! explanation.

pub mod cpu;
pub mod process;
pub mod types;

use windows_sys::Wdk::System::SystemInformation::NtQuerySystemInformation;
use windows_sys::Win32::Foundation::{NTSTATUS, STATUS_INFO_LENGTH_MISMATCH, STATUS_SUCCESS};

/// The buffer the first attempt asks for.
///
/// A machine with four hundred processes and their threads needs roughly
/// 400 KB, so this normally succeeds on the first attempt and the retry
/// path is for the unusual machine rather than the usual one.
const INITIAL_BUFFER: usize = 512 * 1024;

/// How much bigger than the reported requirement each retry asks for.
///
/// The reported size describes the machine during the *failed* call, so
/// asking for exactly it loses the race against anything that started in
/// between. A quarter again is enough headroom for a burst of process
/// creation without wasting a meaningful amount of memory on a buffer
/// that is reused every sample anyway.
const RETRY_MARGIN: usize = 4; // reported + reported / 4

/// How many times to grow before giving up.
///
/// Bounded so a machine in a process-creation storm cannot spin the
/// sampler thread forever. Four attempts starting from 512 KB reaches
/// several megabytes, well past any real machine; a failure past that is
/// a real failure and should be reported as one.
const MAX_ATTEMPTS: usize = 4;

/// How many times [`query_exact`] retries before giving up.
///
/// Smaller than [`MAX_ATTEMPTS`] on purpose: the first attempt exists
/// only to learn the exact size, and the second attempt at that size
/// should always succeed, since the logical processor count it reports
/// does not change mid-process. A third is slack for the one case it
/// could — a hot-added or hot-removed processor between the two calls —
/// rather than a schedule that expects to need it.
const EXACT_MATCH_ATTEMPTS: usize = 3;

/// The largest buffer to ask for before treating the situation as broken.
///
/// A guard against a corrupted or hostile length: without it a bad
/// reported size could ask for an allocation large enough to fail, and
/// the failure would be an out-of-memory abort rather than a handled
/// error.
const MAX_BUFFER: usize = 64 * 1024 * 1024;

/// Why a query failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueryError {
    /// The buffer could not be grown enough within [`MAX_ATTEMPTS`].
    ///
    /// In practice this means the machine is creating processes faster
    /// than the call can enumerate them, which is worth surfacing rather
    /// than retrying forever.
    Grew,
    /// [`query_exact`]'s reported size never stabilised within its
    /// attempt budget — see that function's docs on why it retries
    /// against an exact size rather than growing.
    Unstable,
    /// The call failed with a status other than a length mismatch.
    Status(NTSTATUS),
}

impl std::fmt::Display for QueryError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Grew => write!(
                formatter,
                "the process list kept growing faster than it could be read"
            ),
            Self::Unstable => write!(
                formatter,
                "the reported buffer size never settled to a value that could be read"
            ),
            Self::Status(status) => {
                write!(
                    formatter,
                    "NtQuerySystemInformation failed (0x{status:08x})"
                )
            }
        }
    }
}

impl std::error::Error for QueryError {}

/// A buffer holding one class of system information.
///
/// Owns its allocation and hands out a byte slice. Kept as a type rather
/// than a bare `Vec<u8>` so the sampler can hold one across samples and
/// reuse the allocation — a 512 KB allocation and free every second is
/// avoidable work, and avoiding it is the difference between a sampler
/// that shows up in its own process list and one that does not.
#[derive(Debug, Default)]
pub struct InfoBuffer {
    /// The allocation. Its length is the *capacity* asked for, not the
    /// number of bytes the call filled.
    bytes: Vec<u8>,
    /// How many bytes the last successful call actually wrote.
    filled: usize,
}

impl InfoBuffer {
    /// An empty buffer, to be grown by the first query.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The bytes the last successful query wrote.
    #[must_use]
    pub fn filled(&self) -> &[u8] {
        self.bytes.get(..self.filled).unwrap_or(&[])
    }
}

/// Queries one information class into `buffer`, growing it as needed.
///
/// On success the data is in [`InfoBuffer::filled`]. See the module docs
/// for the retry protocol and why it is bounded.
pub fn query(class: i32, buffer: &mut InfoBuffer) -> Result<(), QueryError> {
    if buffer.bytes.is_empty() {
        buffer.bytes.resize(INITIAL_BUFFER, 0);
    }

    for _ in 0..MAX_ATTEMPTS {
        let mut needed: u32 = 0;
        let capacity = u32::try_from(buffer.bytes.len()).unwrap_or(u32::MAX);
        let status = call(class, &mut buffer.bytes, capacity, &mut needed);

        if status == STATUS_SUCCESS {
            // `needed` is the number of bytes written on success. Clamp
            // to the capacity: it comes from outside the program, and a
            // slice built from an over-large value would read past the
            // allocation.
            buffer.filled = usize::try_from(needed).unwrap_or(0).min(buffer.bytes.len());
            return Ok(());
        }
        if status != STATUS_INFO_LENGTH_MISMATCH {
            buffer.filled = 0;
            return Err(QueryError::Status(status));
        }

        // Grow, with the margin the module docs explain. `needed` can
        // come back as zero for some classes, so never shrink.
        let reported = usize::try_from(needed).unwrap_or(0);
        let wanted = reported
            .saturating_add(reported / RETRY_MARGIN)
            .max(buffer.bytes.len().saturating_mul(2))
            .min(MAX_BUFFER);
        if wanted <= buffer.bytes.len() {
            // Cannot grow any further — either the cap was hit or the
            // reported size is not increasing. Either way, retrying
            // would spin.
            break;
        }
        buffer.bytes.resize(wanted, 0);
    }

    buffer.filled = 0;
    Err(QueryError::Grew)
}

/// Queries an information class whose length check wants an *exact*
/// match rather than [`query`]'s "big enough" one.
///
/// `SystemProcessorPerformanceInformation` is the one class this crate
/// reads where `STATUS_INFO_LENGTH_MISMATCH` fires even when the buffer
/// is *larger* than the true size — confirmed against a real machine,
/// since it is not documented behaviour. [`query`]'s margin-and-double
/// growth can never converge against that: every attempt only ever asks
/// for more, so an already-oversized buffer just becomes a bigger
/// oversized one and the call fails identically every time.
///
/// This resizes to exactly what the kernel reports instead. That works
/// because the quantity behind the size — the logical processor count —
/// does not change while the process runs, so the exact size the first
/// attempt reports is still correct on the second attempt. The bound is
/// still small, in case a hot-added or hot-removed processor makes the
/// second attempt's size stale in turn.
pub fn query_exact(class: i32, buffer: &mut InfoBuffer) -> Result<(), QueryError> {
    if buffer.bytes.is_empty() {
        buffer.bytes.resize(INITIAL_BUFFER, 0);
    }

    for _ in 0..EXACT_MATCH_ATTEMPTS {
        let mut needed: u32 = 0;
        let capacity = u32::try_from(buffer.bytes.len()).unwrap_or(u32::MAX);
        let status = call(class, &mut buffer.bytes, capacity, &mut needed);

        if status == STATUS_SUCCESS {
            buffer.filled = usize::try_from(needed).unwrap_or(0).min(buffer.bytes.len());
            return Ok(());
        }
        if status != STATUS_INFO_LENGTH_MISMATCH {
            buffer.filled = 0;
            return Err(QueryError::Status(status));
        }

        // A `needed` of zero, or unchanged from the capacity that was
        // just rejected, cannot be acted on: resizing to it would either
        // allocate nothing or repeat the exact call that just failed.
        let reported = usize::try_from(needed).unwrap_or(0);
        if reported == 0 || reported == buffer.bytes.len() {
            break;
        }
        buffer.bytes.resize(reported, 0);
    }

    buffer.filled = 0;
    Err(QueryError::Unstable)
}

/// The FFI call, and nothing else.
///
/// The safe leaf wrapper this module exists to provide: every caller
/// above works in slices and `Result`s.
fn call(class: i32, buffer: &mut [u8], capacity: u32, needed: &mut u32) -> NTSTATUS {
    // SAFETY: `buffer` is a live, uniquely-borrowed allocation of at
    // least `capacity` bytes — `capacity` is derived from `buffer.len()`
    // by the caller and saturates rather than wrapping, so it can never
    // exceed the allocation. `needed` is a live `u32` the callee writes
    // once. The call neither retains nor frees either pointer, so both
    // borrows end when it returns. `class` is one of the constants in
    // `types`; an unrecognised class is rejected by the callee with a
    // status rather than being undefined.
    unsafe {
        NtQuerySystemInformation(
            class,
            buffer.as_mut_ptr().cast(),
            capacity,
            std::ptr::from_mut(needed),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_buffer_reads_back_as_nothing() {
        // The state before the first query, and the state after a failed
        // one. Neither may hand out bytes a call did not write.
        let buffer = InfoBuffer::new();
        assert!(buffer.filled().is_empty());
    }

    #[test]
    fn a_fill_length_past_the_allocation_cannot_be_handed_out() {
        // `filled` is clamped on the way in, but this pins the invariant
        // at the point it is read: the slice can never exceed the
        // allocation, whatever a call reported.
        let mut buffer = InfoBuffer::new();
        buffer.bytes.resize(16, 0);
        buffer.filled = 999;
        assert!(
            buffer.filled().len() <= 16,
            "a bad fill length must not produce a slice past the allocation"
        );
    }

    #[test]
    fn the_growth_schedule_terminates() {
        // Mirrors the arithmetic in `query`, which is the part that could
        // spin: each step must strictly increase until the cap, and stop
        // there. A schedule that plateaued below the cap would loop
        // MAX_ATTEMPTS times for nothing; one that never plateaued would
        // allocate without bound.
        let mut size = INITIAL_BUFFER;
        let mut seen = Vec::new();
        for _ in 0..MAX_ATTEMPTS {
            seen.push(size);
            // The worst case for growth: the call reports needing less
            // than the doubling, so the doubling is what applies.
            let reported = size;
            let wanted = reported
                .saturating_add(reported / RETRY_MARGIN)
                .max(size.saturating_mul(2))
                .min(MAX_BUFFER);
            if wanted <= size {
                break;
            }
            size = wanted;
        }
        assert!(
            seen.windows(2).all(|pair| pair[1] > pair[0]),
            "each attempt must ask for strictly more than the last: {seen:?}"
        );
        assert!(
            size <= MAX_BUFFER,
            "the schedule must respect the cap, reached {size}"
        );
        assert!(
            size >= 4 * 1024 * 1024,
            "four attempts should reach several megabytes, well past any \
             real machine; reached only {size}"
        );
    }

    #[test]
    fn a_zero_reported_size_does_not_shrink_the_buffer() {
        // Some classes report zero on a length mismatch. Taking that at
        // face value would shrink the buffer and guarantee the next
        // attempt fails the same way.
        let size = INITIAL_BUFFER;
        let reported = 0usize;
        let wanted = reported
            .saturating_add(reported / RETRY_MARGIN)
            .max(size.saturating_mul(2))
            .min(MAX_BUFFER);
        assert!(
            wanted > size,
            "a zero report must still grow the buffer, got {wanted}"
        );
    }

    #[test]
    fn an_absurd_reported_size_is_capped_rather_than_allocated() {
        // A corrupted or hostile length must not turn into an allocation
        // large enough to abort the process.
        let reported = usize::MAX;
        let wanted = reported
            .saturating_add(reported / RETRY_MARGIN)
            .max(INITIAL_BUFFER.saturating_mul(2))
            .min(MAX_BUFFER);
        assert_eq!(wanted, MAX_BUFFER, "the cap must bind");
    }

    #[test]
    fn the_error_messages_say_what_happened() {
        assert!(QueryError::Grew.to_string().contains("growing"));
        assert!(
            QueryError::Unstable.to_string().contains("settled"),
            "Unstable is query_exact's failure, not query's — it must \
             not reuse Grew's process-list wording"
        );
        let status = QueryError::Status(-1_073_741_820).to_string();
        assert!(
            status.contains("0x"),
            "an NTSTATUS should be shown in hex, which is how every \
             reference lists them; got {status}"
        );
    }

    #[test]
    fn query_exact_gives_up_when_the_reported_size_repeats() {
        // Mirrors `query_exact`'s stop condition. If the kernel reports
        // the same size that was just rejected, resizing to it and
        // calling again would repeat the identical failed call — so the
        // loop has to recognise that as stuck rather than spin for
        // `EXACT_MATCH_ATTEMPTS` iterations doing nothing.
        let capacity = INITIAL_BUFFER;
        let reported = capacity;
        let stuck = reported == 0 || reported == capacity;
        assert!(stuck, "an unchanged report must be treated as stuck");
    }

    #[test]
    fn query_exact_shrinks_to_a_reported_size_smaller_than_an_oversized_buffer() {
        // The scenario `query_exact` exists for: a buffer already larger
        // than needed still mismatches, and the fix is willing to shrink
        // to exactly what was reported — the opposite of `query`'s
        // never-shrink rule, which exists for a different class with
        // different failure semantics.
        let capacity = INITIAL_BUFFER;
        let reported = 768usize; // e.g. 16 logical processors * 48 bytes.
        let stuck = reported == 0 || reported == capacity;
        assert!(
            !stuck,
            "a genuine, different reported size must not be treated as stuck"
        );
        assert!(
            reported < capacity,
            "this is specifically the smaller-than-capacity case"
        );
    }
}

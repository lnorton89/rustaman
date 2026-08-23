// ============================================================================
// Module:       win::nt::process
// Description:  Walks the process-information chain into safe Rust values,
//               bounds-checking every step against the buffer.
//
// Dependencies: super::types (the layouts), super::super::strings, windows-sys
// ============================================================================

//! Turning the process-information buffer into values.
//!
//! [`enumerate`] walks the chain [`super::query`] filled and yields one
//! [`RawProcess`] per process. Everything the kernel reports about a
//! process comes out of this one walk.
//!
//! ## The walk is the dangerous part
//!
//! The buffer is a chain, not an array: each entry states the byte offset
//! to the next, and the entry's own thread records sit between them. That
//! offset comes from outside the program, so every step has to be treated
//! as untrusted:
//!
//! - An offset of zero ends the chain. That is the documented
//!   terminator.
//! - An offset that would step *backwards or nowhere* is a loop. A chain
//!   with one is not merely malformed, it hangs the sampler thread — so
//!   the walk requires strict forward progress rather than assuming it.
//! - An offset that steps past the end of the buffer, or one that leaves
//!   less than a whole entry, ends the walk. Reading a partial entry
//!   would produce a process with plausible-looking numbers assembled
//!   from whatever followed the buffer.
//!
//! None of this is theoretical for a hand-declared layout: if [`super::types`]
//! were ever wrong about the entry size, the thread records would be
//! misread as an entry and the offsets would be garbage. The bounds
//! checks are what turn that from a crash or a silent wrong answer into
//! a short process list.
//!
//! ## Why the raw values are not the model's values
//!
//! [`RawProcess`] holds what the kernel said, in the kernel's units:
//! cumulative 100-nanosecond tick counts, not percentages. Turning a
//! cumulative counter into a rate needs the *previous* sample, which is
//! the engine's business ([`crate::engine::rates`]) and not something a
//! single call can answer. Keeping the conversion out of here is what
//! lets the rate arithmetic be tested without a Windows kernel.

use super::types::{
    SystemProcessInformation, SystemThreadInformation, PROCESS_INFORMATION_SIZE,
    SYSTEM_PROCESS_INFORMATION_CLASS, THREAD_INFORMATION_SIZE, THREAD_STATE_WAITING,
    WAIT_REASON_SUSPENDED,
};
use super::{query, InfoBuffer, QueryError};
use crate::win::strings;

/// One process, exactly as the kernel reported it.
///
/// Cumulative counters are left cumulative; see the module docs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RawProcess {
    /// The process id.
    pub pid: u32,
    /// The creating process's id, unvalidated.
    pub parent_pid: u32,
    /// Image file name, e.g. `chrome.exe`. Not a path.
    pub name: String,
    /// Creation time as a FILETIME — the other half of the identity.
    pub started_at: u64,
    /// Terminal-services session.
    pub session_id: u32,
    /// Threads in the process.
    pub threads: u32,
    /// Open kernel handles.
    pub handles: u32,
    /// Cumulative kernel + user CPU time, in 100ns ticks.
    pub cpu_ticks: u64,
    /// Working set, in bytes.
    pub working_set: u64,
    /// Private commit, in bytes.
    pub private_bytes: u64,
    /// Virtual address space, in bytes.
    pub virtual_bytes: u64,
    /// Private working set, in bytes — the resident pages this process
    /// does not share with any other.
    ///
    /// The honest answer to "how much memory is this costing the
    /// machine": the rest of its working set is DLLs and mapped files
    /// that every other process using them is also counted for, so
    /// summing plain working sets across a machine counts the same
    /// pages many times over.
    pub private_working_set: u64,
    /// The most working set this process has ever held.
    pub peak_working_set: u64,
    /// The most private commit it has ever held.
    pub peak_private_bytes: u64,
    /// Kernel paged pool charged to this process.
    pub paged_pool: u64,
    /// Kernel non-paged pool charged to this process.
    pub nonpaged_pool: u64,
    /// Page faults since it started — soft and hard together.
    pub page_faults: u64,
    /// The hard ones alone: faults that had to reach the disk. This is
    /// the number that means a process is *thrashing* rather than merely
    /// growing.
    pub hard_faults: u64,
    /// Cumulative bytes read.
    pub read_bytes: u64,
    /// Cumulative bytes written.
    pub write_bytes: u64,
    /// Cumulative bytes transferred by other operations.
    ///
    /// Deliberately kept apart from reads and writes: this counts control
    /// operations, which for some processes dwarfs their actual I/O and
    /// would make the Disk column nonsense if folded in.
    pub other_bytes: u64,
    /// Base scheduling priority.
    pub base_priority: i32,
    /// Whether every thread is suspended. See [`is_suspended`].
    pub suspended: bool,
}

/// Enumerates every process on the machine.
///
/// `buffer` is reused across calls; see [`InfoBuffer`].
pub fn enumerate(buffer: &mut InfoBuffer) -> Result<Vec<RawProcess>, QueryError> {
    query(SYSTEM_PROCESS_INFORMATION_CLASS, buffer)?;
    Ok(walk(buffer.filled()))
}

/// Walks a filled buffer into processes.
///
/// Separated from [`enumerate`] so the walk — the part with the
/// bounds-checking that actually matters — can be tested against
/// hand-built buffers on any platform. See the tests below.
#[must_use]
pub fn walk(bytes: &[u8]) -> Vec<RawProcess> {
    let mut found = Vec::new();
    let mut offset = 0usize;

    while let Some(entry) = read_entry(bytes, offset) {
        // `UniqueProcessId` is a HANDLE-shaped integer; the PID is its
        // low bits. Truncating rather than `try_into`: a PID is a u32 by
        // definition, and the upper bits of the handle are not part of
        // it.
        let pid = entry.UniqueProcessId as usize as u32;
        let parent_pid = entry.InheritedFromUniqueProcessId as usize as u32;

        // SAFETY: `entry.ImageName.Buffer` points into `bytes`, which is
        // borrowed for the whole of this function and outlives the call.
        // `read_entry` has already confirmed a whole entry lies within
        // `bytes`, so the `UNICODE_STRING` itself was read from valid
        // memory. A hostile buffer could still hold a bad pointer, but
        // this buffer was written by the kernel into an allocation this
        // process owns — it is not attacker-controlled.
        let name = unsafe { strings::from_unicode_string(&entry.ImageName) };

        found.push(RawProcess {
            pid,
            parent_pid,
            // PID 0's image name comes back empty. Naming it here rather
            // than at a call site because every consumer would otherwise
            // need the same special case, and a blank row in the process
            // list looks like a bug.
            name: if name.is_empty() && pid == crate::model::IDLE_PID {
                "System Idle Process".to_string()
            } else {
                name
            },
            started_at: entry.CreateTime.max(0) as u64,
            session_id: entry.SessionId,
            threads: entry.NumberOfThreads,
            handles: entry.HandleCount,
            // Kernel and user time are both cumulative and both
            // non-negative; `max(0)` guards a garbage read rather than a
            // real negative.
            cpu_ticks: (entry.KernelTime.max(0) as u64)
                .saturating_add(entry.UserTime.max(0) as u64),
            working_set: entry.WorkingSetSize as u64,
            private_bytes: entry.PagefileUsage as u64,
            virtual_bytes: entry.VirtualSize as u64,
            // Already in the buffer the enumeration walks, so the whole
            // memory view costs no extra syscall — see `gui::ui::memory`.
            // `WorkingSetPrivateSize` is signed in the kernel's own
            // declaration and is never negative in practice; a negative
            // reading is treated as absent rather than wrapped into an
            // enormous unsigned one.
            private_working_set: u64::try_from(entry.WorkingSetPrivateSize).unwrap_or(0),
            peak_working_set: entry.PeakWorkingSetSize as u64,
            peak_private_bytes: entry.PeakPagefileUsage as u64,
            paged_pool: entry.QuotaPagedPoolUsage as u64,
            nonpaged_pool: entry.QuotaNonPagedPoolUsage as u64,
            page_faults: u64::from(entry.PageFaultCount),
            hard_faults: u64::from(entry.HardFaultCount),
            read_bytes: entry.ReadTransferCount.max(0) as u64,
            write_bytes: entry.WriteTransferCount.max(0) as u64,
            other_bytes: entry.OtherTransferCount.max(0) as u64,
            base_priority: entry.BasePriority,
            suspended: is_suspended(bytes, offset, entry.NumberOfThreads),
        });

        let step = entry.NextEntryOffset as usize;
        // Strict forward progress or stop. A zero offset is the
        // documented terminator; anything that would not advance is a
        // loop, and a loop here hangs the sampler thread. See the module
        // docs.
        if step == 0 {
            break;
        }
        let Some(next) = offset.checked_add(step) else {
            break;
        };
        if next <= offset || next >= bytes.len() {
            break;
        }
        offset = next;
    }

    found
}

/// Reads the entry at `offset`, or `None` if a whole one does not fit.
///
/// The bounds check that makes the walk safe: a partial entry would be
/// assembled from whatever followed the buffer and would look like a real
/// process.
fn read_entry(bytes: &[u8], offset: usize) -> Option<SystemProcessInformation> {
    let end = offset.checked_add(PROCESS_INFORMATION_SIZE)?;
    let slice = bytes.get(offset..end)?;
    // Copied out rather than referenced: the buffer is a `[u8]` with no
    // alignment guarantee, and `SystemProcessInformation` needs 8-byte
    // alignment. Reading a misaligned reference would be undefined
    // behaviour even though x86 tolerates it in hardware.
    //
    // SAFETY: `slice` is exactly `size_of::<SystemProcessInformation>()`
    // bytes long (the `get` above guarantees it) and is valid for reads.
    // `read_unaligned` imposes no alignment requirement. Every field of
    // the struct is a plain integer, pointer, or `UNICODE_STRING`, none
    // of which has an invalid bit pattern, so any byte content is a
    // valid — if possibly meaningless — value.
    let entry =
        unsafe { std::ptr::read_unaligned(slice.as_ptr().cast::<SystemProcessInformation>()) };
    Some(entry)
}

/// Whether every one of a process's threads is suspended.
///
/// A process is "Suspended" in the list only when *all* of its threads
/// are, which is what the shell does to a store app it has put to sleep.
/// A process with one suspended thread and thirty running ones is not
/// suspended, it is a process with a suspended thread — reporting it as
/// suspended would be wrong in a way that matters, since the row would
/// claim a busy process is doing nothing.
///
/// The thread records follow their process entry, so this reads the array
/// starting one entry past `offset`. A process whose thread array does
/// not fit in the buffer is reported as not suspended: the safe answer,
/// since it is the one that does not claim a running process is idle.
fn is_suspended(bytes: &[u8], offset: usize, thread_count: u32) -> bool {
    if thread_count == 0 {
        // The two kernel pseudo-processes report no threads. Neither is
        // suspended, and neither can be.
        return false;
    }
    let Some(start) = offset.checked_add(PROCESS_INFORMATION_SIZE) else {
        return false;
    };
    let count = usize::try_from(thread_count).unwrap_or(0);

    for index in 0..count {
        let Some(base) = index
            .checked_mul(THREAD_INFORMATION_SIZE)
            .and_then(|shift| start.checked_add(shift))
        else {
            return false;
        };
        let Some(end) = base.checked_add(THREAD_INFORMATION_SIZE) else {
            return false;
        };
        let Some(slice) = bytes.get(base..end) else {
            // The array runs past the buffer. Report "not suspended"
            // rather than deciding from a partial read.
            return false;
        };
        // SAFETY: as `read_entry` — `slice` is exactly one thread record
        // long, valid for reads, and read without an alignment
        // requirement. Every field is a plain integer or pointer.
        let thread =
            unsafe { std::ptr::read_unaligned(slice.as_ptr().cast::<SystemThreadInformation>()) };
        let sleeping = thread.ThreadState == THREAD_STATE_WAITING
            && thread.WaitReason == WAIT_REASON_SUSPENDED;
        if !sleeping {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a buffer holding `entries` process records, each with the
    /// stated PID, thread count, and next-entry offset.
    ///
    /// Hand-built so the walk's bounds and loop handling can be tested on
    /// any platform — this is the part where a mistake is a hang or a
    /// wrong answer rather than a compile error.
    fn buffer(entries: &[(u32, u32, u32)]) -> Vec<u8> {
        let mut bytes = Vec::new();
        for (pid, threads, next) in entries {
            let mut entry = SystemProcessInformation {
                NextEntryOffset: *next,
                NumberOfThreads: *threads,
                WorkingSetPrivateSize: 0,
                HardFaultCount: 0,
                NumberOfThreadsHighWatermark: 0,
                CycleTime: 0,
                CreateTime: 132_000_000_000_000_000,
                UserTime: 10_000_000,
                KernelTime: 20_000_000,
                ImageName: windows_sys::Win32::Foundation::UNICODE_STRING {
                    Length: 0,
                    MaximumLength: 0,
                    Buffer: std::ptr::null_mut(),
                },
                BasePriority: 8,
                UniqueProcessId: (*pid as usize) as _,
                InheritedFromUniqueProcessId: 0usize as _,
                HandleCount: 42,
                SessionId: 1,
                UniqueProcessKey: 0,
                PeakVirtualSize: 0,
                VirtualSize: 4096,
                PageFaultCount: 0,
                PeakWorkingSetSize: 0,
                WorkingSetSize: 8192,
                QuotaPeakPagedPoolUsage: 0,
                QuotaPagedPoolUsage: 0,
                QuotaPeakNonPagedPoolUsage: 0,
                QuotaNonPagedPoolUsage: 0,
                PagefileUsage: 2048,
                PeakPagefileUsage: 0,
                PrivatePageCount: 0,
                ReadOperationCount: 0,
                WriteOperationCount: 0,
                OtherOperationCount: 0,
                ReadTransferCount: 100,
                WriteTransferCount: 200,
                OtherTransferCount: 300,
            };
            // Silence the unused-assignment warning on a field only some
            // tests care about while keeping the struct literal complete.
            entry.HardFaultCount = 0;

            let raw: [u8; PROCESS_INFORMATION_SIZE] =
                // SAFETY: `SystemProcessInformation` is `repr(C)` and
                // every field is a plain integer or pointer, so its
                // representation is a valid byte array of exactly this
                // size (asserted in `types`). This is a test helper
                // building the byte layout the kernel would write.
                unsafe { std::mem::transmute(entry) };
            bytes.extend_from_slice(&raw);
            for _ in 0..*threads {
                bytes.extend_from_slice(&[0u8; THREAD_INFORMATION_SIZE]);
            }
        }
        bytes
    }

    /// The byte step from one entry to the next, given a thread count.
    fn stride(threads: u32) -> u32 {
        u32::try_from(PROCESS_INFORMATION_SIZE + THREAD_INFORMATION_SIZE * threads as usize)
            .unwrap_or(0)
    }

    #[test]
    fn a_single_entry_chain_reads_its_one_process() {
        let bytes = buffer(&[(1234, 0, 0)]);
        let found = walk(&bytes);
        assert_eq!(found.len(), 1);
        let Some(process) = found.first() else {
            return;
        };
        assert_eq!(process.pid, 1234);
        assert_eq!(process.handles, 42);
        assert_eq!(process.working_set, 8192);
        assert_eq!(process.private_bytes, 2048);
        assert_eq!(
            process.cpu_ticks, 30_000_000,
            "CPU time is kernel plus user"
        );
        assert_eq!(process.read_bytes, 100);
        assert_eq!(process.write_bytes, 200);
        assert_eq!(
            process.other_bytes, 300,
            "control-operation bytes are kept apart from real I/O"
        );
    }

    #[test]
    fn a_chain_of_entries_is_walked_in_order() {
        let bytes = buffer(&[(100, 0, stride(0)), (200, 2, stride(2)), (300, 0, 0)]);
        let pids: Vec<u32> = walk(&bytes).into_iter().map(|p| p.pid).collect();
        assert_eq!(
            pids,
            vec![100, 200, 300],
            "the offset must step over each entry's thread records too"
        );
    }

    #[test]
    fn an_offset_that_does_not_advance_ends_the_walk() {
        // A self-referential offset. Without the forward-progress check
        // this loops forever inside the sampler thread and the UI shows a
        // stale snapshot with no explanation.
        let bytes = buffer(&[(100, 0, 0)]);
        let mut looping = bytes.clone();
        // Point the first entry at itself.
        if let Some(slot) = looping.get_mut(0..4) {
            slot.copy_from_slice(&0u32.to_le_bytes());
        }
        assert_eq!(walk(&looping).len(), 1, "a zero offset terminates");

        // And an offset that would land before where it started. The
        // field is unsigned, so "backwards" is expressed as an enormous
        // forward step that wraps — which `checked_add` catches.
        let mut wrapping = bytes.clone();
        if let Some(slot) = wrapping.get_mut(0..4) {
            slot.copy_from_slice(&u32::MAX.to_le_bytes());
        }
        assert_eq!(
            walk(&wrapping).len(),
            1,
            "an offset that overflows must end the walk, not wrap it"
        );
    }

    #[test]
    fn an_offset_past_the_buffer_ends_the_walk() {
        // Reading a partial entry would assemble a process out of
        // whatever followed the buffer, and it would look real.
        let mut bytes = buffer(&[(100, 0, 0)]);
        let past = u32::try_from(bytes.len()).unwrap_or(0);
        if let Some(slot) = bytes.get_mut(0..4) {
            slot.copy_from_slice(&past.to_le_bytes());
        }
        assert_eq!(walk(&bytes).len(), 1);
    }

    #[test]
    fn a_truncated_buffer_yields_only_the_whole_entries_in_it() {
        let full = buffer(&[(100, 0, stride(0)), (200, 0, 0)]);
        // Cut the second entry in half.
        let truncated = full
            .get(..PROCESS_INFORMATION_SIZE + 16)
            .map(<[u8]>::to_vec)
            .unwrap_or_default();
        assert_eq!(walk(&truncated).len(), 1, "half an entry is not a process");
    }

    #[test]
    fn an_empty_buffer_yields_no_processes() {
        assert!(walk(&[]).is_empty());
        assert!(walk(&[0u8; 8]).is_empty(), "less than one entry");
    }

    #[test]
    fn the_idle_process_is_given_the_name_the_kernel_omits() {
        // PID 0 comes back with an empty image name, and a blank row in
        // the process list looks like a bug.
        let bytes = buffer(&[(0, 0, 0)]);
        let found = walk(&bytes);
        assert_eq!(
            found.first().map(|p| p.name.as_str()),
            Some("System Idle Process")
        );
    }

    #[test]
    fn a_process_is_suspended_only_when_every_thread_is() {
        // One suspended thread among thirty running ones is not a
        // suspended process; reporting it as one would claim a busy
        // process is doing nothing.
        let mut bytes = buffer(&[(100, 2, 0)]);
        let first = PROCESS_INFORMATION_SIZE;
        let second = first + THREAD_INFORMATION_SIZE;

        let mark = |bytes: &mut Vec<u8>, base: usize, state: u32, reason: u32| {
            if let Some(slot) = bytes.get_mut(base + 0x44..base + 0x48) {
                slot.copy_from_slice(&state.to_le_bytes());
            }
            if let Some(slot) = bytes.get_mut(base + 0x48..base + 0x4c) {
                slot.copy_from_slice(&reason.to_le_bytes());
            }
        };

        mark(
            &mut bytes,
            first,
            THREAD_STATE_WAITING,
            WAIT_REASON_SUSPENDED,
        );
        mark(&mut bytes, second, 2, 0); // running
        assert!(
            !walk(&bytes).first().is_some_and(|p| p.suspended),
            "one suspended thread does not suspend the process"
        );

        mark(
            &mut bytes,
            second,
            THREAD_STATE_WAITING,
            WAIT_REASON_SUSPENDED,
        );
        assert!(
            walk(&bytes).first().is_some_and(|p| p.suspended),
            "with every thread suspended, the process is suspended"
        );
    }

    #[test]
    fn a_thread_waiting_for_something_other_than_a_suspend_is_not_suspended() {
        // The common case: a thread blocked on I/O is `Waiting`, but for
        // a different reason. Checking only the state would report most
        // of the machine as suspended.
        let mut bytes = buffer(&[(100, 1, 0)]);
        let base = PROCESS_INFORMATION_SIZE;
        if let Some(slot) = bytes.get_mut(base + 0x44..base + 0x48) {
            slot.copy_from_slice(&THREAD_STATE_WAITING.to_le_bytes());
        }
        if let Some(slot) = bytes.get_mut(base + 0x48..base + 0x4c) {
            slot.copy_from_slice(&1u32.to_le_bytes()); // some other reason
        }
        assert!(!walk(&bytes).first().is_some_and(|p| p.suspended));
    }

    #[test]
    fn a_thread_array_running_past_the_buffer_does_not_claim_suspension() {
        // Claiming a running process is idle is the wrong way to fail.
        let bytes = buffer(&[(100, 0, 0)]);
        assert!(
            !is_suspended(&bytes, 0, 50),
            "a thread count the buffer cannot back must not be trusted"
        );
    }

    #[test]
    fn a_process_with_no_threads_is_not_suspended() {
        let bytes = buffer(&[(4, 0, 0)]);
        assert!(!walk(&bytes).first().is_some_and(|p| p.suspended));
    }
}

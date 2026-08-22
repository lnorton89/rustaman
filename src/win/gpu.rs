// ============================================================================
// Module:       win::gpu
// Description:  GPU engine utilisation and per-process video memory, read from
//               the performance counters Task Manager uses.
//
// Dependencies: windows-sys (PDH); super::strings
// ============================================================================

//! GPU utilisation.
//!
//! ## There is no Win32 call for this
//!
//! Not one. GPU utilisation is not exposed through any kernel API — the
//! only public source is the **performance counters** the display driver
//! stack publishes, `\GPU Engine(*)\Utilization Percentage` and
//! `\GPU Process Memory(*)\Dedicated Usage`, read through PDH. That is
//! exactly where Task Manager gets its own GPU column, and it is why that
//! column did not exist before Windows 10 1709: the counters did not.
//!
//! ## The instance names carry the data
//!
//! PDH instance names for these counters are structured strings:
//!
//! ```text
//! pid_12345_luid_0x00000000_0x0000C4F1_phys_0_eng_0_engtype_3D
//! ```
//!
//! The PID, the adapter LUID, and the engine type all have to be parsed
//! back out of that name — there is no other way to attribute a counter
//! to a process or an adapter. [`parse_instance`] does it, and its tests
//! are the ones that matter in this module, because a driver that formats
//! the name differently is the failure mode this has to survive.
//!
//! ## Per-adapter utilisation is a maximum, not a sum
//!
//! A GPU has several engines — 3D, copy, video decode, video encode,
//! compute — and they run **in parallel**. Summing their utilisation
//! yields figures well over 100% for a machine that is merely playing a
//! video while it renders, which is not a rare case, it is the common
//! one. Task Manager reports the busiest engine, and so does this: see
//! [`crate::model::GpuSample::utilisation`].
//!
//! ## Failure here is not an error
//!
//! A machine whose driver does not publish these counters — an old GPU, a
//! VM with a basic display adapter, a server with no display driver at
//! all — simply reports no GPUs, and the Performance view omits the
//! section. Every function below returns an empty result rather than an
//! error for that reason.

use super::strings;
use std::collections::HashMap;
use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::System::Performance::{
    PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData, PdhGetFormattedCounterArrayW,
    PdhOpenQueryW, PDH_FMT_COUNTERVALUE, PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE,
};

/// The counter path for engine utilisation, across every instance.
const ENGINE_COUNTER: &str = r"\GPU Engine(*)\Utilization Percentage";

/// The counter path for per-process dedicated video memory.
const MEMORY_COUNTER: &str = r"\GPU Process Memory(*)\Dedicated Usage";

/// What one PDH instance name decodes to.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Instance {
    /// The owning process id, if the name carried one.
    pub pid: Option<u32>,
    /// The adapter's LUID as text, which is the only stable adapter
    /// identifier these counters offer.
    pub luid: String,
    /// The engine type, e.g. `3D`, `Copy`, `VideoDecode`.
    pub engine: String,
}

/// A live PDH query, closed on drop.
///
/// PDH requires the query handle to be closed, and the interesting path
/// is the one where a counter fails to add — an early return there leaks
/// a query handle every sample, which is exactly what the owning-wrapper
/// rule exists to prevent.
struct Query(HANDLE);

impl Query {
    /// Opens a PDH query against the local machine.
    fn open() -> Option<Self> {
        let mut handle: HANDLE = std::ptr::null_mut();
        // SAFETY: a null data-source pointer selects the local machine
        // and a zero user-data value is the documented "none". `handle`
        // is a live, uniquely-borrowed out-parameter the callee writes a
        // query handle into on success; this `Query` then owns it.
        let status = unsafe { PdhOpenQueryW(std::ptr::null(), 0, std::ptr::from_mut(&mut handle)) };
        (status == 0 && !handle.is_null()).then_some(Self(handle))
    }

    /// Adds a wildcard counter to the query.
    ///
    /// `PdhAddEnglishCounterW` rather than `PdhAddCounterW`: the latter
    /// takes the *localised* counter name, so `\GPU Engine(*)\Utilization
    /// Percentage` fails on a German or Japanese install where the
    /// counter is called something else. The English variant takes the
    /// invariant name on every locale, which is the only way a
    /// hard-coded path can work at all.
    fn add(&self, path: &str) -> Option<HANDLE> {
        let wide = strings::to_wide(path);
        let mut counter: HANDLE = std::ptr::null_mut();
        // SAFETY: `self.0` is a live query handle. `wide` is a live,
        // NUL-terminated UTF-16 buffer bound to a local that outlives the
        // call. `counter` is a live out-parameter. The counter handle is
        // owned by the query and is closed with it, so it needs no
        // wrapper of its own.
        let status = unsafe {
            PdhAddEnglishCounterW(self.0, wide.as_ptr(), 0, std::ptr::from_mut(&mut counter))
        };
        (status == 0 && !counter.is_null()).then_some(counter)
    }

    /// Collects one round of counter data.
    fn collect(&self) -> bool {
        // SAFETY: `self.0` is a live query handle; the call takes it by
        // value and retains nothing.
        let status = unsafe { PdhCollectQueryData(self.0) };
        status == 0
    }
}

// SAFETY: a PDH query handle has no thread affinity — it is an opaque
// kernel-side object addressed by handle, like the process handles in
// `super::handle` — and `Query` owns its handle exclusively, with no
// interior mutability and no `Clone`. Moving one to the sampler thread
// is what lets the GPU counters be read off the UI thread.
unsafe impl Send for Query {}

impl Drop for Query {
    fn drop(&mut self) {
        // SAFETY: `self.0` is a live query handle owned exclusively by
        // this value, and this is the one close.
        unsafe {
            let _ = PdhCloseQuery(self.0);
        }
    }
}

/// One reading of the GPU counters.
#[derive(Clone, Debug, Default)]
pub struct Reading {
    /// Utilisation per (adapter LUID, engine type), as a percentage.
    ///
    /// Summed across the processes sharing an engine, because that is
    /// what the adapter's own utilisation means.
    pub engines: HashMap<(String, String), f64>,
    /// Utilisation per process, as a percentage, taking the busiest
    /// engine that process is using.
    pub by_pid: HashMap<u32, f64>,
    /// Dedicated video memory per process, in bytes.
    pub memory_by_pid: HashMap<u32, u64>,
}

/// A PDH session held open across samples.
///
/// Kept alive rather than re-opened because these counters are *rate*
/// counters: PDH needs two collections to compute one value, so a query
/// opened and collected once always reads zero. Opening the query per
/// sample would therefore report a permanently idle GPU — which looks
/// exactly like a machine that is not using its GPU, and is the reason
/// this is a struct rather than a function.
pub struct Session {
    /// The query, and the two counters added to it.
    query: Query,
    /// The engine-utilisation counter, if it could be added.
    engine: Option<HANDLE>,
    /// The process-memory counter, if it could be added.
    memory: Option<HANDLE>,
    /// Whether a first collection has been made. See the struct docs.
    primed: bool,
}

// SAFETY: `Session` owns a `Query` (which is `Send`, see above) plus two
// counter handles that belong to that query and are closed with it. None
// has thread affinity, and `Session` is neither `Clone` nor `Sync`.
unsafe impl Send for Session {}

impl Session {
    /// Opens a session, or `None` if PDH or the counters are unavailable.
    ///
    /// A machine with no GPU counters is normal; see the module docs.
    #[must_use]
    pub fn open() -> Option<Self> {
        let query = Query::open()?;
        let engine = query.add(ENGINE_COUNTER);
        let memory = query.add(MEMORY_COUNTER);
        if engine.is_none() && memory.is_none() {
            // Neither counter exists: this machine has nothing to report.
            return None;
        }
        // The priming collection. Its values are discarded — a rate
        // counter needs two.
        let primed = query.collect();
        Some(Self {
            query,
            engine,
            memory,
            primed,
        })
    }

    /// Takes a reading.
    ///
    /// Returns an empty reading rather than an error when the collection
    /// fails, which happens transiently while a driver is restarting.
    #[must_use]
    pub fn read(&mut self) -> Reading {
        if !self.query.collect() {
            return Reading::default();
        }
        if !self.primed {
            // The open-time collection failed, so this one is the prime.
            self.primed = true;
            return Reading::default();
        }

        let mut reading = Reading::default();

        if let Some(counter) = self.engine {
            for (name, value) in formatted_array(counter) {
                let Some(instance) = parse_instance(&name) else {
                    continue;
                };
                // An adapter's utilisation for one engine type is the sum
                // over the processes using it; the *adapter's* figure is
                // then the maximum over engine types, which the caller
                // takes. See the module docs.
                *reading
                    .engines
                    .entry((instance.luid.clone(), instance.engine.clone()))
                    .or_insert(0.0) += value;
                if let Some(pid) = instance.pid {
                    let slot = reading.by_pid.entry(pid).or_insert(0.0);
                    // Per process, the busiest engine rather than the sum,
                    // for the same reason.
                    *slot = slot.max(value);
                }
            }
        }

        if let Some(counter) = self.memory {
            for (name, value) in formatted_array(counter) {
                let Some(instance) = parse_instance(&name) else {
                    continue;
                };
                let Some(pid) = instance.pid else {
                    continue;
                };
                // A process can hold memory on several adapters; the
                // total is what the column means.
                *reading.memory_by_pid.entry(pid).or_insert(0) +=
                    if value.is_finite() && value > 0.0 {
                        value as u64
                    } else {
                        0
                    };
            }
        }

        reading
    }
}

/// Reads every instance of a wildcard counter as `(instance name, value)`.
///
/// `PdhGetFormattedCounterArrayW` uses the ask-then-fetch protocol, and
/// returns a buffer holding an array of items **plus** the instance-name
/// strings those items point into — so the buffer has to stay alive while
/// the names are read, which is why the strings are copied out before it
/// is dropped.
fn formatted_array(counter: HANDLE) -> Vec<(String, f64)> {
    let mut size = 0u32;
    let mut count = 0u32;
    // SAFETY: a null buffer is the documented way to ask for the
    // required size. Both out-parameters are live, uniquely-borrowed
    // values. The call is expected to fail; only the size is used.
    let _ = unsafe {
        PdhGetFormattedCounterArrayW(
            counter,
            PDH_FMT_DOUBLE,
            std::ptr::from_mut(&mut size),
            std::ptr::from_mut(&mut count),
            std::ptr::null_mut(),
        )
    };
    let capacity = usize::try_from(size).unwrap_or(0);
    if capacity == 0 {
        return Vec::new();
    }

    let mut buffer = vec![0u8; capacity];
    // SAFETY: `buffer` is a live, uniquely-borrowed allocation of exactly
    // `size` bytes, which is what the size out-parameter states. The
    // callee writes the item array and the instance-name strings into it.
    let status = unsafe {
        PdhGetFormattedCounterArrayW(
            counter,
            PDH_FMT_DOUBLE,
            std::ptr::from_mut(&mut size),
            std::ptr::from_mut(&mut count),
            buffer.as_mut_ptr().cast(),
        )
    };
    if status != 0 {
        return Vec::new();
    }

    let items = usize::try_from(count).unwrap_or(0);
    let stride = std::mem::size_of::<PDH_FMT_COUNTERVALUE_ITEM_W>();
    let mut found = Vec::with_capacity(items);
    for index in 0..items {
        let Some(base) = index.checked_mul(stride) else {
            break;
        };
        let Some(end) = base.checked_add(stride) else {
            break;
        };
        // The item count comes from the call; a count the buffer cannot
        // back ends the walk rather than reading past it.
        let Some(slice) = buffer.get(base..end) else {
            break;
        };
        // SAFETY: `slice` is exactly one item long and valid for reads.
        // `read_unaligned` imposes no alignment requirement, which
        // matters because the backing buffer is a `Vec<u8>`.
        let item = unsafe {
            std::ptr::read_unaligned(slice.as_ptr().cast::<PDH_FMT_COUNTERVALUE_ITEM_W>())
        };
        if item.szName.is_null() {
            continue;
        }
        // SAFETY: `szName` points at a NUL-terminated UTF-16 string
        // inside `buffer`, which is alive for the rest of this function.
        // The string is copied out before the buffer is dropped.
        let name = unsafe { wide_from_pointer(item.szName) };
        // SAFETY: the value union was formatted as `PDH_FMT_DOUBLE`
        // above, so the `doubleValue` arm is the populated one.
        let value = unsafe { value_of(&item.FmtValue) };
        found.push((name, value));
    }
    found
}

/// Reads the double out of a formatted counter value.
///
/// # Safety
///
/// The value must have been formatted with `PDH_FMT_DOUBLE`, which is
/// what makes `doubleValue` the live arm of the union.
unsafe fn value_of(value: &PDH_FMT_COUNTERVALUE) -> f64 {
    // SAFETY: the caller guarantees the union was formatted as a double.
    let raw = unsafe { value.Anonymous.doubleValue };
    if raw.is_finite() && raw >= 0.0 {
        raw
    } else {
        0.0
    }
}

/// Reads a NUL-terminated wide string from a raw pointer.
///
/// # Safety
///
/// `pointer` must be non-null and point at a NUL-terminated UTF-16 string
/// alive for this call.
unsafe fn wide_from_pointer(pointer: *const u16) -> String {
    /// A guard against a missing terminator rather than a real limit —
    /// a PDH instance name is well under a hundred characters.
    const MAX_UNITS: usize = 512;
    let mut length = 0usize;
    while length < MAX_UNITS {
        // SAFETY: the caller guarantees the string is NUL-terminated, so
        // every unit up to the terminator is within the allocation.
        let unit = unsafe { *pointer.add(length) };
        if unit == 0 {
            break;
        }
        length += 1;
    }
    // SAFETY: `length` units were just confirmed readable.
    let slice = unsafe { std::slice::from_raw_parts(pointer, length) };
    String::from_utf16_lossy(slice)
}

/// Decodes a PDH instance name.
///
/// The names look like
/// `pid_12345_luid_0x00000000_0x0000C4F1_phys_0_eng_0_engtype_3D`, and
/// this is the only route from a counter back to a process or an adapter.
///
/// Parsed by scanning for the `pid_`, `luid_` and `engtype_` markers
/// rather than by splitting on `_` and indexing, because the fields are
/// not all present in every name — the `GPU Process Memory` counter's
/// instances have no `engtype_`, and a driver may add fields — and a
/// positional parse breaks on the first one that differs.
#[must_use]
pub fn parse_instance(name: &str) -> Option<Instance> {
    /// Reads the text following `marker` up to the next `_` that begins
    /// another marker, or the end.
    fn field<'a>(name: &'a str, marker: &str) -> Option<&'a str> {
        let start = name.find(marker)? + marker.len();
        name.get(start..)
    }

    let pid = field(name, "pid_")
        .and_then(|rest| rest.split('_').next())
        .and_then(|digits| digits.parse::<u32>().ok());

    // The LUID is two underscore-separated hex words, so it cannot be
    // taken as "up to the next underscore".
    let luid = field(name, "luid_")
        .map(|rest| rest.split('_').take(2).collect::<Vec<_>>().join("_"))
        .unwrap_or_default();

    // `engtype_` is last in the name, so everything after it is the type.
    let engine = field(name, "engtype_").unwrap_or("").to_string();

    // A name carrying none of the three is not one of these counters.
    if pid.is_none() && luid.is_empty() && engine.is_empty() {
        return None;
    }
    Some(Instance { pid, luid, engine })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fails the test for an instance name that should have decoded.
    ///
    /// A helper rather than an `assert!(false, ..)` at each site: the
    /// lints forbid `panic!` and an `assert!` on a constant reads as a
    /// mistake. This states the expectation as a real assertion.
    fn unreachable_instance(name: &str) {
        assert!(
            parse_instance(name).is_some(),
            "{name:?} should have decoded as a GPU instance"
        );
    }

    #[test]
    fn a_full_engine_instance_name_decodes() {
        let name = "pid_12345_luid_0x00000000_0x0000C4F1_phys_0_eng_0_engtype_3D";
        let Some(instance) = parse_instance(name) else {
            unreachable_instance(name);
            return;
        };
        assert_eq!(instance.pid, Some(12345));
        assert_eq!(
            instance.luid, "0x00000000_0x0000C4F1",
            "the LUID is two hex words and cannot be read as one field"
        );
        assert_eq!(instance.engine, "3D");
    }

    #[test]
    fn the_other_engine_types_decode() {
        for engine in ["3D", "Copy", "VideoDecode", "VideoEncode", "Compute_0"] {
            let name = format!("pid_1_luid_0x00000000_0x00001234_phys_0_eng_0_engtype_{engine}");
            assert_eq!(
                parse_instance(&name).map(|instance| instance.engine),
                Some(engine.to_string())
            );
        }
    }

    #[test]
    fn a_process_memory_instance_has_no_engine_and_still_decodes() {
        // The `GPU Process Memory` counter's instances carry no
        // `engtype_`, which is why the parse scans for markers rather
        // than splitting positionally.
        let name = "pid_4242_luid_0x00000000_0x0000C4F1_phys_0";
        let Some(instance) = parse_instance(name) else {
            unreachable_instance(name);
            return;
        };
        assert_eq!(instance.pid, Some(4242));
        assert_eq!(instance.luid, "0x00000000_0x0000C4F1");
        assert!(instance.engine.is_empty());
    }

    #[test]
    fn an_instance_name_with_no_recognised_fields_is_rejected() {
        // PDH returns a `_Total` instance for many counters, and a driver
        // can publish names in a shape this does not understand. Neither
        // may be attributed to a process.
        for name in ["_Total", "", "something else entirely"] {
            assert!(
                parse_instance(name).is_none(),
                "{name:?} should not decode as a GPU instance"
            );
        }
    }

    #[test]
    fn a_name_with_a_malformed_pid_still_yields_its_adapter() {
        // A partial decode is better than none: the adapter's own
        // utilisation is still usable even when the process cannot be
        // identified.
        let name = "pid_notanumber_luid_0x00000000_0x0000C4F1_engtype_3D";
        let Some(instance) = parse_instance(name) else {
            unreachable_instance(name);
            return;
        };
        assert_eq!(instance.pid, None);
        assert_eq!(instance.engine, "3D");
    }

    #[test]
    fn a_session_opens_or_declines_cleanly() {
        // A machine with no GPU counters — a VM with a basic display
        // adapter, or a server with no display driver — is a normal
        // configuration, so both outcomes are correct. What matters is
        // that neither panics and that a session that opens can be read.
        let Some(mut session) = Session::open() else {
            return;
        };
        let reading = session.read();
        for (pid, percent) in &reading.by_pid {
            assert!(
                percent.is_finite() && *percent >= 0.0,
                "PID {pid} reported an unusable utilisation of {percent}"
            );
        }
    }

    #[test]
    fn a_second_reading_is_what_carries_values() {
        // These are rate counters: PDH needs two collections to compute
        // one value, so a query opened and read once always reads zero.
        // Opening per sample would report a permanently idle GPU.
        let Some(mut session) = Session::open() else {
            return;
        };
        let _ = session.read();
        std::thread::sleep(std::time::Duration::from_millis(200));
        let second = session.read();
        // Cannot assert the GPU is busy — it may genuinely be idle — but
        // the reading must be well-formed.
        for value in second.by_pid.values() {
            assert!(value.is_finite());
        }
    }
}

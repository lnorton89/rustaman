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
    PdhOpenQueryW, PDH_CSTATUS_NEW_DATA, PDH_CSTATUS_VALID_DATA, PDH_FMT_COUNTERVALUE,
    PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE,
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
    /// The physical-engine group published by the driver.
    pub physical: u32,
    /// The exact engine number within the physical group.
    pub engine_index: u32,
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
        let status = open_local_query(&mut handle);
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
        let status = add_english_counter(self.0, &wide, &mut counter);
        (status == 0 && !counter.is_null()).then_some(counter)
    }

    /// Collects one round of counter data.
    fn collect(&self) -> bool {
        let status = collect_query_data(self.0);
        status == 0
    }
}

impl Drop for Query {
    fn drop(&mut self) {
        close_query(self.0);
    }
}

/// One reading of the GPU counters.
#[derive(Clone, Debug, Default)]
pub struct Reading {
    /// Utilisation per exact physical engine, as a percentage.
    ///
    /// Summed across the processes sharing an engine, because that is
    /// what the adapter's own utilisation means.
    pub engines: HashMap<(String, u32, u32, String), f64>,
    /// Utilisation per process, as a percentage, taking the busiest
    /// engine that process is using.
    pub by_pid: HashMap<u32, f64>,
    /// Dedicated video memory per process, in bytes.
    pub memory_by_pid: HashMap<u32, u64>,
    /// Dedicated video memory per adapter LUID, in bytes.
    pub memory_by_adapter: HashMap<String, u64>,
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
                // Sum processes only when they refer to the same exact
                // physical engine. Distinct engines with the same type
                // run in parallel and must remain distinct.
                *reading
                    .engines
                    .entry((
                        instance.luid.clone(),
                        instance.physical,
                        instance.engine_index,
                        instance.engine.clone(),
                    ))
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
                let bytes = if value.is_finite() && value > 0.0 {
                    value as u64
                } else {
                    0
                };
                *reading.memory_by_pid.entry(pid).or_insert(0) += bytes;
                *reading.memory_by_adapter.entry(instance.luid).or_insert(0) += bytes;
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
    query_formatted_array_size(counter, &mut size, &mut count);
    let capacity = usize::try_from(size).unwrap_or(0);
    if capacity == 0 {
        return Vec::new();
    }

    let mut buffer = vec![0u8; capacity];
    let status = read_formatted_array(counter, &mut size, &mut count, &mut buffer);
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
        let Some(item) = read_counter_item(slice) else {
            break;
        };
        if item.szName.is_null() {
            continue;
        }
        let Some(name) = wide_from_buffer(item.szName, &buffer) else {
            continue;
        };
        if let Some(value) = formatted_double(&item.FmtValue) {
            found.push((name, value));
        }
    }
    found
}

/// Reads a double produced by this module's `PDH_FMT_DOUBLE` query.
fn formatted_double(value: &PDH_FMT_COUNTERVALUE) -> Option<f64> {
    if value.CStatus != PDH_CSTATUS_VALID_DATA && value.CStatus != PDH_CSTATUS_NEW_DATA {
        return None;
    }
    // SAFETY: `read_formatted_array` always requests `PDH_FMT_DOUBLE`, so
    // this item's populated union arm is `doubleValue`.
    let raw = unsafe { value.Anonymous.doubleValue };
    if raw.is_finite() && raw >= 0.0 {
        Some(raw)
    } else {
        None
    }
}

/// Copies a NUL-terminated UTF-16 name only when the pointer and the
/// terminator are both inside the owning PDH buffer.
fn wide_from_buffer(pointer: *const u16, buffer: &[u8]) -> Option<String> {
    let start = pointer as usize;
    let buffer_start = buffer.as_ptr() as usize;
    let buffer_end = buffer_start.checked_add(buffer.len())?;
    if start < buffer_start
        || start >= buffer_end
        || !start.is_multiple_of(std::mem::align_of::<u16>())
    {
        return None;
    }
    let byte_offset = start.checked_sub(buffer_start)?;
    let bytes = buffer.get(byte_offset..)?;
    let units = bytes.len() / std::mem::size_of::<u16>();
    let wide = u16_slice_in_buffer(pointer, units, buffer);
    let length = wide.iter().position(|unit| *unit == 0)?;
    Some(String::from_utf16_lossy(&wide[..length]))
}

/// Opens a PDH query on the local computer.
fn open_local_query(handle: &mut HANDLE) -> u32 {
    // SAFETY: a null data-source selects the local computer; `handle` is a
    // live writable out-parameter and the API retains neither pointer.
    unsafe { PdhOpenQueryW(std::ptr::null(), 0, handle) }
}

/// Adds an invariant-English PDH counter to a live query.
fn add_english_counter(query: HANDLE, path: &[u16], counter: &mut HANDLE) -> u32 {
    // SAFETY: `query` is owned by `Query`; `path` is live and NUL-terminated;
    // `counter` is a live writable out-parameter, and nothing is retained.
    unsafe { PdhAddEnglishCounterW(query, path.as_ptr(), 0, counter) }
}

/// Collects one sample from a live PDH query.
fn collect_query_data(query: HANDLE) -> u32 {
    // SAFETY: `query` is a live handle owned by `Query`; the API retains nothing.
    unsafe { PdhCollectQueryData(query) }
}

/// Closes the exclusively-owned PDH query handle.
fn close_query(query: HANDLE) {
    // SAFETY: `Query` owns this live handle and Drop invokes this exactly once.
    let _ = unsafe { PdhCloseQuery(query) };
}

/// Asks PDH how many bytes and items a formatted counter array requires.
fn query_formatted_array_size(counter: HANDLE, size: &mut u32, count: &mut u32) {
    // SAFETY: a null buffer is PDH's documented size query; both references are
    // live writable out-parameters and the API retains none of them.
    let _ = unsafe {
        PdhGetFormattedCounterArrayW(counter, PDH_FMT_DOUBLE, size, count, std::ptr::null_mut())
    };
}

/// Fetches one formatted counter array into its exact byte buffer.
fn read_formatted_array(
    counter: HANDLE,
    size: &mut u32,
    count: &mut u32,
    buffer: &mut [u8],
) -> u32 {
    // SAFETY: `buffer` has the byte capacity requested by PDH; `size` and
    // `count` are live out-parameters. PDH writes only within that buffer.
    unsafe {
        PdhGetFormattedCounterArrayW(
            counter,
            PDH_FMT_DOUBLE,
            size,
            count,
            buffer.as_mut_ptr().cast(),
        )
    }
}

/// Reads one packed PDH item from the byte buffer without assuming alignment.
fn read_counter_item(bytes: &[u8]) -> Option<PDH_FMT_COUNTERVALUE_ITEM_W> {
    if bytes.len() < std::mem::size_of::<PDH_FMT_COUNTERVALUE_ITEM_W>() {
        return None;
    }
    // SAFETY: the caller gives exactly `size_of::<PDH_FMT_COUNTERVALUE_ITEM_W>()`
    // bytes from the PDH output buffer; `read_unaligned` accepts its byte alignment.
    Some(unsafe { std::ptr::read_unaligned(bytes.as_ptr().cast::<PDH_FMT_COUNTERVALUE_ITEM_W>()) })
}

/// Reinterprets the checked aligned tail of PDH's backing buffer as UTF-16.
fn u16_slice_in_buffer(pointer: *const u16, units: usize, _buffer: &[u8]) -> &[u16] {
    // SAFETY: `wide_from_buffer` proved `pointer` is aligned and that exactly
    // `units` complete u16s remain within its still-live backing byte buffer.
    unsafe { std::slice::from_raw_parts(pointer, units) }
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
    let physical = field(name, "phys_")
        .and_then(|rest| rest.split('_').next())
        .and_then(|digits| digits.parse::<u32>().ok())
        .unwrap_or(0);
    let engine_index = field(name, "eng_")
        .and_then(|rest| rest.split('_').next())
        .and_then(|digits| digits.parse::<u32>().ok())
        .unwrap_or(0);

    // A name carrying none of the three is not one of these counters.
    if pid.is_none() && luid.is_empty() && engine.is_empty() {
        return None;
    }
    Some(Instance {
        pid,
        luid,
        engine,
        physical,
        engine_index,
    })
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

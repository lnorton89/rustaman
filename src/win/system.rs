// ============================================================================
// Module:       win::system
// Description:  Static facts about the machine — CPU name, core counts, clock,
//               uptime — read once at startup rather than every sample.
//
// Dependencies: windows-sys (GetActiveProcessorCount, GetTickCount64, registry,
//               GetLogicalProcessorInformationEx); super::strings
// ============================================================================

//! The machine's own description.
//!
//! Everything here except [`uptime`] is fixed for the life of the boot,
//! so [`Facts::read`] is called once at startup and the result is carried
//! in the snapshot. Reading a registry key and walking the processor
//! topology once a second to learn a CPU model that cannot change would
//! be pure waste.
//!
//! ## Physical cores are not `dwNumberOfProcessors`
//!
//! `GetSystemInfo` reports *logical* processors, which on a machine with
//! hyper-threading is twice the physical core count and on a modern
//! hybrid CPU is neither a multiple nor a divisor of it. There is no
//! field that says "physical cores"; the only correct source is
//! `GetLogicalProcessorInformationEx(RelationProcessorCore)`, which
//! returns one variable-length record per physical core, and the answer
//! is the number of records.
//!
//! Getting this wrong is visible: "16 cores, 16 logical processors" on a
//! machine that has 8 and 16 is the sort of error that makes a reader
//! distrust every other number on the page.

use super::strings;
use crate::model::SystemInfo;
use windows_sys::Win32::System::Diagnostics::Debug::{
    SetThreadErrorMode, SEM_FAILCRITICALERRORS, THREAD_ERROR_MODE,
};
use windows_sys::Win32::System::SystemInformation::{
    GetLogicalProcessorInformationEx, GetTickCount64, RelationProcessorCore,
};
use windows_sys::Win32::System::Threading::{GetActiveProcessorCount, ALL_PROCESSOR_GROUPS};

/// Stops Windows putting a modal error dialog in front of this thread
/// when a device it touches is not ready.
///
/// **This is the fix for the app hanging on close**, and the reason is
/// not obvious from any of the call sites it protects.
///
/// By default, a call that reaches a device with no media — a card
/// reader with nothing in it, an optical drive with the tray open, a
/// USB slot whose stick was pulled — makes the system raise a *hard
/// error*: the "There is no disk in the drive. Please insert a disk into
/// drive E:" box. That dialog is modal and it **blocks the thread that
/// made the call until somebody dismisses it**. Raised from a background
/// thread it need not appear anywhere near the app's own window, so what
/// the user sees is not a dialog to answer. It is a task manager that
/// has stopped responding.
///
/// The sampler walks every drive letter on the machine once a second
/// (see [`super::disk::volumes`]), so it is exposed to this constantly,
/// and the exposure grows with how long the app has been open — a stick
/// pulled at any point in a long session arms it.
///
/// `SEM_FAILCRITICALERRORS` turns that dialog into an error return,
/// which every caller here already handles. Set per **thread** rather
/// than through `SetErrorMode`, which is process-wide and would reach
/// into whatever else is running.
///
/// Returns whether the mode was accepted; a machine where it was not is
/// one where the dialogs are still possible, which is worth a test
/// rather than a silent assumption.
pub fn suppress_device_error_dialogs() -> bool {
    let mut previous: THREAD_ERROR_MODE = 0;
    let ok = set_thread_error_mode(SEM_FAILCRITICALERRORS, &mut previous);
    ok != 0
}

/// The error mode currently in force on this thread.
///
/// Only exists so the test below can check that
/// [`suppress_device_error_dialogs`] actually took — reading it back is
/// the only way to tell.
#[cfg(test)]
fn thread_error_mode() -> THREAD_ERROR_MODE {
    let mut previous: THREAD_ERROR_MODE = 0;
    // Setting a temporary zero mode exposes the previous mode through the out-parameter.
    let _ = set_thread_error_mode(0, &mut previous);
    // Put it back, so reading the mode does not change it.
    let _ = restore_thread_error_mode(previous);
    previous
}

/// What the machine is, as far as it can be described without sampling.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Facts {
    /// The processor's marketing name.
    pub cpu_name: String,
    /// Physical cores, or zero if the topology could not be read.
    pub physical_cores: usize,
    /// Logical processors.
    pub logical_cores: usize,
    /// Nominal clock in MHz.
    ///
    /// Nominal, not current. The current frequency needs a performance
    /// counter that costs several milliseconds to read and changes many
    /// times a second on any machine with a modern governor — so the
    /// number would be both expensive and misleading.
    pub megahertz: u32,
    /// Machine, firmware, and Windows installation identity.
    pub info: SystemInfo,
}

impl Facts {
    /// Reads the machine's description. Call once.
    #[must_use]
    pub fn read() -> Self {
        Self {
            cpu_name: cpu_name().unwrap_or_default(),
            physical_cores: physical_cores().unwrap_or(0),
            logical_cores: usize::try_from(active_logical_processors()).unwrap_or(0),
            megahertz: nominal_megahertz().unwrap_or(0),
            info: system_info(),
        }
    }
}

/// Reads the machine and Windows identity values that remain stable for a run.
fn system_info() -> SystemInfo {
    const WINDOWS: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion";
    const BIOS: &str = r"HARDWARE\DESCRIPTION\System\BIOS";
    let os_build = registry_string(WINDOWS, "CurrentBuildNumber").unwrap_or_default();
    let product_name = registry_string(WINDOWS, "ProductName").unwrap_or_default();
    SystemInfo {
        computer_name: registry_string(
            r"SYSTEM\CurrentControlSet\Control\ComputerName\ActiveComputerName",
            "ComputerName",
        )
        .or_else(|| std::env::var("COMPUTERNAME").ok())
        .unwrap_or_default(),
        os_name: windows_product_name(product_name, &os_build),
        os_version: registry_string(WINDOWS, "DisplayVersion")
            .or_else(|| registry_string(WINDOWS, "ReleaseId"))
            .unwrap_or_default(),
        os_build,
        manufacturer: registry_string(BIOS, "SystemManufacturer").unwrap_or_default(),
        model: registry_string(BIOS, "SystemProductName").unwrap_or_default(),
        bios_vendor: registry_string(BIOS, "BIOSVendor").unwrap_or_default(),
        bios_version: registry_string(BIOS, "BIOSVersion").unwrap_or_default(),
    }
}

/// Windows 11 retained "Windows 10" in this compatibility registry value
/// on some installations. The build boundary is the stable distinction.
fn windows_product_name(mut name: String, build: &str) -> String {
    let build = build.parse::<u32>().unwrap_or(0);
    if build >= 22_000 && name.contains("Windows 10") {
        name = name.replacen("Windows 10", "Windows 11", 1);
    }
    name
}

/// Seconds since the machine booted.
///
/// `GetTickCount64` rather than `GetTickCount`: the 32-bit version wraps
/// after 49.7 days, and a machine that has been up longer than that would
/// report an uptime that resets — which is exactly the machine whose
/// uptime someone is checking.
#[must_use]
pub fn uptime() -> u64 {
    let milliseconds = tick_count_milliseconds();
    milliseconds / 1_000
}

/// The system-wide active logical-processor count across all groups.
fn active_logical_processors() -> u32 {
    active_processor_count()
}

/// The number of physical cores.
///
/// See the module docs on why `dwNumberOfProcessors` is not this.
///
/// The call uses the ask-then-fetch protocol, and its result is a
/// *variable-length* record chain rather than an array — each record
/// states its own size — so the walk steps by the stated size and checks
/// bounds at every step, the same discipline as [`super::nt::process`].
fn physical_cores() -> Option<usize> {
    let mut needed = 0u32;
    query_core_topology_size(&mut needed);
    let size = usize::try_from(needed).unwrap_or(0);
    if size == 0 {
        return None;
    }

    let mut buffer = vec![0u8; size];
    let ok = read_core_topology(&mut buffer, &mut needed);
    if ok == 0 {
        return None;
    }
    Some(count_core_records(&buffer))
}

/// Counts the records in a `RelationProcessorCore` buffer.
///
/// The records are `SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX`, which is
/// *variable-length* — its trailing group array extends past the struct —
/// so it cannot be read as a fixed-size value the way [`super::nt`]'s
/// records can, and this walks raw bytes instead.
///
/// Split out so the walk can be reasoned about on its own. Each record's first two fields are its relationship and its size;
/// the size is what the walk steps by, and a zero or oversized one ends
/// the walk rather than looping or reading past the buffer.
fn count_core_records(buffer: &[u8]) -> usize {
    /// Byte offset of the `Size` field: past the 4-byte `Relationship`.
    const SIZE_OFFSET: usize = 4;
    /// The smallest a record can be and still hold its own header.
    const MIN_RECORD: usize = 8;

    let mut count = 0usize;
    let mut offset = 0usize;
    while offset + MIN_RECORD <= buffer.len() {
        let Some(bytes) = buffer.get(offset + SIZE_OFFSET..offset + MIN_RECORD) else {
            break;
        };
        let Ok(word) = <[u8; 4]>::try_from(bytes) else {
            break;
        };
        let size = usize::try_from(u32::from_le_bytes(word)).unwrap_or(0);
        // A record smaller than its own header would not advance, and a
        // record claiming to be larger than the buffer cannot be read.
        // Either ends the walk; neither is allowed to loop.
        if size < MIN_RECORD || offset.saturating_add(size) > buffer.len() {
            break;
        }
        count += 1;
        offset += size;
    }
    count
}

/// The processor's marketing name, from the registry.
///
/// There is no Win32 call for this. `GetSystemInfo` reports an
/// architecture, not a model, and the CPUID brand string is not something
/// to be reading inline assembly for — so the registry value Windows
/// itself populates at boot is the source, and it is the same one every
/// other tool reads.
fn cpu_name() -> Option<String> {
    registry_string(
        r"HARDWARE\DESCRIPTION\System\CentralProcessor\0",
        "ProcessorNameString",
    )
    .map(|name| name.trim().to_string())
    .filter(|name| !name.is_empty())
}

/// The nominal clock, from the same registry key.
fn nominal_megahertz() -> Option<u32> {
    registry_dword(r"HARDWARE\DESCRIPTION\System\CentralProcessor\0", "~MHz")
}

/// Reads a `REG_SZ` value from `HKEY_LOCAL_MACHINE`.
fn registry_string(subkey: &str, value: &str) -> Option<String> {
    let key = strings::to_wide(subkey);
    let name = strings::to_wide(value);
    let mut buffer = vec![0u16; 512];
    let mut bytes = u32::try_from(buffer.len() * 2).unwrap_or(0);

    let status = read_registry_string(&key, &name, &mut buffer, &mut bytes);
    if status != 0 {
        return None;
    }
    // `bytes` comes back as a *byte* count; the buffer is u16s.
    let units = u32::try_from(usize::try_from(bytes).unwrap_or(0) / 2).unwrap_or(0);
    Some(strings::from_wide_nul(strings::reported_slice(
        &buffer, units,
    )))
}

/// Reads a `REG_DWORD` value from `HKEY_LOCAL_MACHINE`.
fn registry_dword(subkey: &str, value: &str) -> Option<u32> {
    let key = strings::to_wide(subkey);
    let name = strings::to_wide(value);
    let mut word = 0u32;
    let mut size = u32::try_from(std::mem::size_of::<u32>()).unwrap_or(4);

    let status = read_registry_dword(&key, &name, &mut word, &mut size);
    (status == 0).then_some(word)
}

/// Sets the current thread's error mode and reports the old mode.
fn set_thread_error_mode(mode: THREAD_ERROR_MODE, previous: &mut THREAD_ERROR_MODE) -> i32 {
    // SAFETY: `previous` is a live writable out-parameter; `mode` is a
    // documented by-value flag set and the API retains no pointer.
    unsafe { SetThreadErrorMode(mode, previous) }
}

/// Restores a thread error mode without requesting the previous value.
#[cfg(test)]
fn restore_thread_error_mode(mode: THREAD_ERROR_MODE) -> i32 {
    // SAFETY: the null previous-mode pointer is documented when its value is not needed.
    unsafe { SetThreadErrorMode(mode, std::ptr::null_mut()) }
}

/// Returns the monotonic Windows tick count in milliseconds.
fn tick_count_milliseconds() -> u64 {
    // SAFETY: this call takes no arguments and returns the kernel counter by value.
    unsafe { GetTickCount64() }
}

/// Returns active logical processors across every processor group.
fn active_processor_count() -> u32 {
    // SAFETY: `ALL_PROCESSOR_GROUPS` is the documented all-groups sentinel.
    unsafe { GetActiveProcessorCount(ALL_PROCESSOR_GROUPS) }
}

/// Executes the size phase of the variable-length core-topology query.
fn query_core_topology_size(needed: &mut u32) {
    // SAFETY: a null buffer with zero capacity is the documented size query;
    // `needed` is a live writable out-parameter and is not retained.
    let _ = unsafe {
        GetLogicalProcessorInformationEx(RelationProcessorCore, std::ptr::null_mut(), needed)
    };
}

/// Fills the caller-owned buffer with variable-length core-topology records.
fn read_core_topology(buffer: &mut [u8], needed: &mut u32) -> i32 {
    // SAFETY: `buffer` has the requested byte capacity and `needed` is a live
    // writable size parameter; the API writes only inside it and retains neither.
    unsafe {
        GetLogicalProcessorInformationEx(RelationProcessorCore, buffer.as_mut_ptr().cast(), needed)
    }
}

/// Reads a `REG_SZ` from the local-machine hive into a caller-owned UTF-16 buffer.
fn read_registry_string(key: &[u16], name: &[u16], buffer: &mut [u16], bytes: &mut u32) -> u32 {
    use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ};

    // SAFETY: `key` and `name` are live NUL-terminated paths; `buffer` and
    // `bytes` are live writable out-parameters and the predefined hive needs no close.
    unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            buffer.as_mut_ptr().cast(),
            bytes,
        )
    }
}

/// Reads a `REG_DWORD` from the local-machine hive into a caller-owned word.
fn read_registry_dword(key: &[u16], name: &[u16], word: &mut u32, size: &mut u32) -> u32 {
    use windows_sys::Win32::System::Registry::{
        RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD,
    };

    // SAFETY: `key` and `name` are live NUL-terminated paths; `word` and `size`
    // are live writable out-parameters and the predefined hive needs no close.
    unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            std::ptr::from_mut(word).cast(),
            size,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_windows_11_build_boundary_corrects_the_legacy_product_label() {
        assert_eq!(
            windows_product_name("Windows 10 Pro".into(), "26100"),
            "Windows 11 Pro"
        );
        assert_eq!(
            windows_product_name("Windows 10 Pro".into(), "19045"),
            "Windows 10 Pro"
        );
    }

    #[test]
    fn the_error_mode_actually_suppresses_the_not_ready_dialog() {
        // The one call standing between the sampler and a modal system
        // dialog raised on its own thread, which blocks it there until
        // somebody finds and dismisses it. The failure mode is the app
        // hanging, and nothing about the code that *causes* it — a
        // free-space query on a drive letter — looks dangerous, so the
        // guarantee is worth reading back rather than assuming.
        assert!(
            suppress_device_error_dialogs(),
            "the thread error mode was refused, so a drive with no media              can still put a dialog in front of this thread"
        );
        assert!(
            thread_error_mode() & SEM_FAILCRITICALERRORS != 0,
            "SEM_FAILCRITICALERRORS is not in force after setting it"
        );
    }

    #[test]
    #[ignore = "environment smoke test"]
    fn the_machine_describes_itself() {
        let facts = Facts::read();
        assert!(
            facts.logical_cores > 0,
            "a running machine has at least one logical processor"
        );
        assert!(
            !facts.cpu_name.is_empty(),
            "the CPU name should be readable from the registry"
        );
        assert!(
            facts.megahertz > 0,
            "the nominal clock should be readable from the registry"
        );
    }

    #[test]
    fn physical_cores_are_counted_separately_from_logical_ones() {
        // "16 cores, 16 logical processors" on a machine that has 8 and
        // 16 is the sort of error that makes a reader distrust every
        // other number on the page.
        let facts = Facts::read();
        assert!(
            facts.physical_cores > 0,
            "the processor topology should be readable"
        );
        assert!(
            facts.physical_cores <= facts.logical_cores,
            "physical cores ({}) cannot exceed logical processors ({})",
            facts.physical_cores,
            facts.logical_cores
        );
    }

    #[test]
    fn uptime_advances() {
        let first = uptime();
        std::thread::sleep(std::time::Duration::from_millis(1_100));
        assert!(
            uptime() >= first,
            "uptime must be monotonic; a 32-bit tick count would wrap \
             after 49.7 days and appear to reset"
        );
    }

    #[test]
    fn the_record_walk_stops_on_a_size_that_would_not_advance() {
        // A zero size loops forever; a size smaller than the header
        // cannot hold one. Neither may hang the startup path.
        let mut buffer = vec![0u8; 32];
        // Relationship 0, Size 0.
        assert_eq!(
            count_core_records(&buffer),
            0,
            "a zero-sized record must end the walk rather than loop"
        );

        if let Some(slot) = buffer.get_mut(4..8) {
            slot.copy_from_slice(&3u32.to_le_bytes());
        }
        assert_eq!(count_core_records(&buffer), 0, "under the header size");
    }

    #[test]
    fn the_record_walk_stops_at_a_size_past_the_buffer() {
        let mut buffer = vec![0u8; 32];
        if let Some(slot) = buffer.get_mut(4..8) {
            slot.copy_from_slice(&999u32.to_le_bytes());
        }
        assert_eq!(count_core_records(&buffer), 0);
    }

    #[test]
    fn the_record_walk_counts_well_formed_records() {
        // Three records of sixteen bytes each.
        let mut buffer = vec![0u8; 48];
        for index in 0..3 {
            let base = index * 16;
            if let Some(slot) = buffer.get_mut(base + 4..base + 8) {
                slot.copy_from_slice(&16u32.to_le_bytes());
            }
        }
        assert_eq!(count_core_records(&buffer), 3);
    }

    #[test]
    fn an_empty_buffer_yields_no_records() {
        assert_eq!(count_core_records(&[]), 0);
        assert_eq!(count_core_records(&[0u8; 4]), 0, "less than one header");
    }

    #[test]
    fn a_missing_registry_value_is_not_an_error() {
        assert!(registry_string(r"HARDWARE\DESCRIPTION\System", "NoSuchValue").is_none());
        assert!(registry_dword(r"SOFTWARE\NoSuchKeyAtAll", "Nothing").is_none());
    }

    #[test]
    fn a_registry_string_is_read_as_characters_not_bytes() {
        // `RegGetValueW` reports a byte count for a UTF-16 buffer.
        // Treating it as a character count reads twice as far as the
        // value runs, into whatever followed it.
        let Some(name) = cpu_name() else {
            return;
        };
        assert!(
            !name.contains('\u{0}'),
            "a doubled length would drag the terminator and trailing \
             buffer into the string: {name:?}"
        );
        assert!(
            name.len() < 200,
            "a CPU name should be a marketing string, not a buffer dump"
        );
    }
}

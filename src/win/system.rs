// ============================================================================
// Module:       win::system
// Description:  Static facts about the machine — CPU name, core counts, the
//               hybrid P/E split, clock, uptime — read once at startup rather
//               than every sample.
//
// Dependencies: windows-sys (GetActiveProcessorCount, GetTickCount64, registry,
//               GetLogicalProcessorInformationEx); super::strings;
//               crate::model for CoreKind and SystemInfo
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
    /// What kind of core each logical processor is, on a hybrid machine.
    ///
    /// Empty when the topology could not be read. See [`core_kinds`].
    pub core_kinds: Vec<crate::model::CoreKind>,
    /// Machine, firmware, and Windows installation identity.
    pub info: SystemInfo,
}

impl Facts {
    /// Reads the machine's description. Call once.
    #[must_use]
    pub fn read() -> Self {
        // One topology query answers both questions. It is the expensive
        // call in here — an ask-then-fetch pair over a buffer the kernel
        // fills with a record per core — and asking twice for two fields
        // out of the same records would be the waste, not the walk.
        let topology = core_topology();
        Self {
            cpu_name: cpu_name().unwrap_or_default(),
            physical_cores: topology
                .as_ref()
                .map_or(0, |buffer| count_core_records(buffer)),
            logical_cores: usize::try_from(active_logical_processors()).unwrap_or(0),
            megahertz: nominal_megahertz().unwrap_or(0),
            core_kinds: topology.as_deref().map(core_kinds).unwrap_or_default(),
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
        // The revision is a DWORD beside the build string, and it is the
        // half that moves: two machines on 26100 can be a year of
        // patches apart, and the number that says which is this one.
        build_revision: registry_dword(WINDOWS, "UBR")
            .map(|revision| revision.to_string())
            .unwrap_or_default(),
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

/// The raw `RelationProcessorCore` buffer — one record per physical
/// core.
///
/// Both the physical core count and the hybrid core kinds come out of
/// this, and see the module docs on why `dwNumberOfProcessors` is
/// neither.
///
/// The call uses the ask-then-fetch protocol, and its result is a
/// *variable-length* record chain rather than an array — each record
/// states its own size — so the walk steps by the stated size and checks
/// bounds at every step, the same discipline as [`super::nt::process`].
fn core_topology() -> Option<Vec<u8>> {
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
    Some(buffer)
}

/// Byte offset of a record's `Size` field: past the 4-byte
/// `Relationship`.
const SIZE_OFFSET: usize = 4;

/// The smallest a record can be and still hold its own header.
const MIN_RECORD: usize = 8;

/// Walks a `RelationProcessorCore` buffer, handing each record's own
/// bytes to `visit`.
///
/// The records are `SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX`, which is
/// *variable-length* — its trailing group array extends past the struct —
/// so it cannot be read as a fixed-size value the way [`super::nt`]'s
/// records can, and this walks raw bytes instead.
///
/// Split out so the walk can be reasoned about on its own, and so the
/// two things read out of this buffer — how many cores there are, and
/// what kind each one is — do not each carry their own copy of the
/// bounds reasoning. Each record's first two fields are its relationship
/// and its size; the size is what the walk steps by, and a zero or
/// oversized one ends the walk rather than looping or reading past the
/// buffer.
fn for_each_core_record(buffer: &[u8], mut visit: impl FnMut(&[u8])) {
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
        let Some(record) = buffer.get(offset..offset + size) else {
            break;
        };
        visit(record);
        offset += size;
    }
}

/// Counts the records in a `RelationProcessorCore` buffer.
fn count_core_records(buffer: &[u8]) -> usize {
    let mut count = 0usize;
    for_each_core_record(buffer, |_| count += 1);
    count
}

/// What kind of core each logical processor belongs to.
///
/// **The Windows 11 half of the topology.** Before Alder Lake and
/// Snapdragon X every core on a die was the same and this field was
/// always zero; on a hybrid machine `EfficiencyClass` is what separates
/// the eight cores that finish work from the sixteen that save battery,
/// and it is the only place Windows says so.
///
/// The classes are collapsed rather than reported raw: the highest class
/// the machine names is [`CoreKind::Performance`] and everything below
/// it is [`CoreKind::Efficient`]. Windows permits more than two — some
/// parts do have three, with a low-power island below the E-cores — but
/// the question a person has in front of a core grid is "is this one of
/// the fast ones", and a tile labelled `2` does not answer it.
///
/// ## Only the first processor group is mapped
///
/// [`super::nt::cpu::read`] gets its per-core times from
/// `SystemProcessorPerformanceInformation`, which reports the calling
/// thread's processor group and no other — at most 64 entries. So the
/// index space this has to line up with is group-relative, and mapping
/// any other group's affinity bits into it would attribute one group's
/// core kinds to another's tiles. A machine large enough to have a
/// second group is not a hybrid laptop.
fn core_kinds(buffer: &[u8]) -> Vec<crate::model::CoreKind> {
    use crate::model::CoreKind;

    let classes = efficiency_classes(buffer);
    let Some(top) = classes.iter().flatten().copied().max() else {
        // No record named a processor in this group: report nothing
        // rather than a vector of defaults, so a view can tell an
        // unreadable topology from a uniform one.
        return Vec::new();
    };
    if top == 0 {
        // Every core is the same, which is every machine before the
        // hybrid parts and most of them since.
        return vec![CoreKind::Uniform; classes.len()];
    }
    classes
        .into_iter()
        .map(|class| match class {
            Some(class) if class == top => CoreKind::Performance,
            Some(_) => CoreKind::Efficient,
            // A processor no core record claimed. Saying nothing about
            // it is right; guessing is not.
            None => CoreKind::Uniform,
        })
        .collect()
}

/// The raw efficiency class of each logical processor in the first
/// processor group, indexed by its number within that group.
///
/// The offsets are into `SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX`'s
/// `Processor` arm: `Relationship` and `Size` take the first eight
/// bytes, the union that follows is pointer-aligned, and inside it
/// `Flags` and `EfficiencyClass` are the first two bytes, twenty
/// reserved bytes and a `WORD GroupCount` follow, and the
/// `GROUP_AFFINITY` array begins after them. Every read is bounds
/// checked against the record the walk handed over, so a record shorter
/// than it claims yields nothing rather than reading into the next one.
fn efficiency_classes(buffer: &[u8]) -> Vec<Option<u8>> {
    /// `EfficiencyClass`, one byte past `Flags` at the head of the union.
    const CLASS_OFFSET: usize = 9;
    /// `GroupCount`, past `Flags`, `EfficiencyClass` and twenty reserved.
    const GROUP_COUNT_OFFSET: usize = 30;
    /// The first `GROUP_AFFINITY`, which the union's alignment puts here
    /// on both 32- and 64-bit.
    const GROUP_MASK_OFFSET: usize = 32;
    /// `GROUP_AFFINITY`: a pointer-wide `KAFFINITY`, a `WORD Group`, and
    /// three reserved words.
    const AFFINITY_SIZE: usize = std::mem::size_of::<usize>() + 8;
    /// Processors per group, which is what `KAFFINITY` can address.
    const GROUP_WIDTH: usize = 64;

    let mut classes: Vec<Option<u8>> = Vec::new();
    for_each_core_record(buffer, |record| {
        let Some(&class) = record.get(CLASS_OFFSET) else {
            return;
        };
        let Some(groups) = read_u16(record, GROUP_COUNT_OFFSET) else {
            return;
        };
        for group in 0..usize::from(groups) {
            let base = GROUP_MASK_OFFSET + group * AFFINITY_SIZE;
            let (Some(mask), Some(number)) = (
                read_usize(record, base),
                read_u16(record, base + std::mem::size_of::<usize>()),
            ) else {
                break;
            };
            // See the note on the first group in `core_kinds`.
            if number != 0 {
                continue;
            }
            for processor in 0..GROUP_WIDTH {
                if mask & (1usize << processor) == 0 {
                    continue;
                }
                if classes.len() <= processor {
                    classes.resize(processor + 1, None);
                }
                if let Some(slot) = classes.get_mut(processor) {
                    *slot = Some(class);
                }
            }
        }
    });
    classes
}

/// A little-endian `u16` at `offset`, or `None` past the end.
fn read_u16(record: &[u8], offset: usize) -> Option<u16> {
    let bytes = record.get(offset..offset.checked_add(2)?)?;
    Some(u16::from_le_bytes(<[u8; 2]>::try_from(bytes).ok()?))
}

/// A little-endian pointer-wide word at `offset`, or `None` past the end.
///
/// Pointer-wide because that is what `KAFFINITY` is: eight bytes on the
/// 64-bit builds this ships as, four on a 32-bit one, and reading the
/// wrong width would take the group number as part of the mask.
fn read_usize(record: &[u8], offset: usize) -> Option<usize> {
    let width = std::mem::size_of::<usize>();
    let bytes = record.get(offset..offset.checked_add(width)?)?;
    let mut word = [0u8; std::mem::size_of::<usize>()];
    word.copy_from_slice(bytes);
    Some(usize::from_le_bytes(word))
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

    /// One `RelationProcessorCore` record naming `processors` in group
    /// zero at `class`.
    ///
    /// Built here rather than taken from a machine because the machine
    /// this runs on has exactly one topology and the layout has to be
    /// right for the others too — a hybrid part, a uniform one, and a
    /// record that lies about its own length.
    fn core_record(class: u8, processors: &[usize]) -> Vec<u8> {
        // Header, union head, reserved, group count, one affinity.
        let size = 32 + std::mem::size_of::<usize>() + 8;
        let mut record = vec![0u8; size];
        if let Some(slot) = record.get_mut(4..8) {
            slot.copy_from_slice(&u32::try_from(size).unwrap_or(0).to_le_bytes());
        }
        // Relationship stays zero: `RelationProcessorCore`.
        if let Some(slot) = record.get_mut(9) {
            *slot = class;
        }
        if let Some(slot) = record.get_mut(30..32) {
            slot.copy_from_slice(&1u16.to_le_bytes());
        }
        let mask = processors
            .iter()
            .fold(0usize, |mask, processor| mask | (1usize << processor));
        if let Some(slot) = record.get_mut(32..32 + std::mem::size_of::<usize>()) {
            slot.copy_from_slice(&mask.to_le_bytes());
        }
        record
    }

    #[test]
    fn a_hybrid_topology_names_the_fast_cores_and_the_rest() {
        // Two performance cores with two threads each, then four
        // efficiency cores with one — an Alder Lake i5 in miniature.
        let mut buffer = Vec::new();
        buffer.extend(core_record(1, &[0, 1]));
        buffer.extend(core_record(1, &[2, 3]));
        for processor in 4..8 {
            buffer.extend(core_record(0, &[processor]));
        }

        let kinds = core_kinds(&buffer);
        assert_eq!(kinds.len(), 8, "eight logical processors were named");
        assert_eq!(
            &kinds[..4],
            &[crate::model::CoreKind::Performance; 4],
            "the highest efficiency class is the performance one"
        );
        assert_eq!(&kinds[4..], &[crate::model::CoreKind::Efficient; 4]);
        assert_eq!(count_core_records(&buffer), 6, "six physical cores");
    }

    #[test]
    fn a_uniform_machine_says_so_rather_than_inventing_a_split() {
        // Every core class zero, which is every machine before the
        // hybrid parts. Calling half of them "efficiency cores" because
        // they all tie for the top class would be a distinction the
        // machine does not have.
        let mut buffer = Vec::new();
        for core in 0..4 {
            buffer.extend(core_record(0, &[core * 2, core * 2 + 1]));
        }
        let kinds = core_kinds(&buffer);
        assert_eq!(kinds, vec![crate::model::CoreKind::Uniform; 8]);

        let sample = crate::model::CpuSample {
            core_kinds: kinds,
            ..crate::model::CpuSample::default()
        };
        assert_eq!(
            sample.hybrid_counts(),
            None,
            "a uniform machine has no performance/efficiency split to print"
        );
    }

    #[test]
    fn an_unreadable_topology_reports_nothing_rather_than_defaults() {
        // Empty, not a vector of `Uniform`: the Performance view has to
        // be able to tell "all the same" from "we could not ask".
        assert!(core_kinds(&[]).is_empty());
        assert!(core_kinds(&[0u8; 4]).is_empty(), "less than one header");
    }

    #[test]
    fn a_record_shorter_than_its_own_fields_yields_no_classes() {
        // The record's stated size is honoured by the walk, so a record
        // claiming sixteen bytes must not have its group array read out
        // of the record that follows it.
        let mut buffer = vec![0u8; 32];
        for index in 0..2 {
            let base = index * 16;
            if let Some(slot) = buffer.get_mut(base + 4..base + 8) {
                slot.copy_from_slice(&16u32.to_le_bytes());
            }
        }
        assert_eq!(count_core_records(&buffer), 2);
        assert!(
            core_kinds(&buffer).is_empty(),
            "a truncated record names no processors"
        );
    }

    #[test]
    fn a_second_processor_group_is_left_out_of_the_mapping() {
        // The per-core times only cover the calling thread's group, so
        // mapping another group's bits into the same index space would
        // attribute one group's core kinds to another's tiles.
        let mut record = core_record(1, &[0, 1]);
        if let Some(slot) = record.get_mut(32 + std::mem::size_of::<usize>()..) {
            if let Some(number) = slot.get_mut(0..2) {
                number.copy_from_slice(&1u16.to_le_bytes());
            }
        }
        assert!(core_kinds(&record).is_empty());
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

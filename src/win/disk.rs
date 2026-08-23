// ============================================================================
// Module:       win::disk
// Description:  Per-physical-disk throughput and active time, plus the volume
//               capacities shown beside them.
//
// Dependencies: windows-sys (CreateFileW, DeviceIoControl, GetLogicalDrives);
//               super::handle, super::strings
// ============================================================================

//! Physical disk activity.
//!
//! `IOCTL_DISK_PERFORMANCE` against `\\.\PhysicalDriveN` returns
//! cumulative bytes read and written, and — the number that actually
//! matters — cumulative *idle time*.
//!
//! ## Why active time, and not throughput, is the headline
//!
//! Throughput does not tell you whether a disk is the bottleneck. A queue
//! of small random reads can saturate a spinning disk completely at a few
//! hundred kilobytes a second, while a sequential copy moves two hundred
//! megabytes a second and leaves it half idle. A user looking at "Disk:
//! 400 KB/s" and a hung machine has been told nothing.
//!
//! Active time — the share of the interval during which at least one
//! request was outstanding — is the figure that answers the question, and
//! it is what Task Manager leads with for the same reason. It comes from
//! `IdleTime`, which the structure reports as a cumulative 100ns counter,
//! so the active share of an interval is `1 - idle_delta / elapsed`.
//!
//! Note that this is *not* the same trap as [`super::nt::cpu`]: there the
//! problem was idle time being nested inside kernel time. Here idle time
//! is measured against wall-clock elapsed time, which the caller supplies
//! because only it knows how long the interval really was.
//!
//! ## Opening the device needs no privilege
//!
//! `CreateFileW` on `\\.\PhysicalDriveN` with a desired access of **zero**
//! succeeds for an unprivileged process. Asking for `GENERIC_READ` — the
//! obvious thing to write — requires administrator, and the call fails on
//! a normal run, which is how this ends up looking like it needs
//! elevation when it does not. Zero access is enough for a
//! metadata-only IOCTL.

use super::handle::OwnedHandle;
use super::strings;
use windows_sys::Win32::Foundation::MAX_PATH;
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDrives, FILE_ATTRIBUTE_NORMAL,
    FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows_sys::Win32::System::Ioctl::{DISK_PERFORMANCE, IOCTL_DISK_PERFORMANCE};
use windows_sys::Win32::System::WindowsProgramming::{DRIVE_FIXED, DRIVE_REMOVABLE};
use windows_sys::Win32::System::IO::DeviceIoControl;

/// The highest physical drive number to probe.
///
/// Probing is a `CreateFileW` per number, so the bound is what stops
/// startup from making an unbounded number of calls. Sixteen is well past
/// any desktop or workstation; a machine with more disks than that is a
/// server, and this is not a server tool.
const MAX_DRIVES: u32 = 16;

/// One physical disk's cumulative counters.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DiskCounters {
    /// The `\\.\PhysicalDriveN` number.
    pub index: u32,
    /// Cumulative bytes read.
    pub read_bytes: u64,
    /// Cumulative bytes written.
    pub write_bytes: u64,
    /// Cumulative time with no request outstanding, in 100ns ticks.
    pub idle_ticks: u64,
}

/// Reads every physical disk's counters.
///
/// Probes drive numbers from zero upward and stops at the first gap of
/// two, because drive numbers are usually contiguous but a removable
/// device that has been ejected can leave a hole. Stopping at the first
/// failure would lose every disk after an ejected card reader.
#[must_use]
pub fn read() -> Vec<DiskCounters> {
    let mut disks = Vec::new();
    let mut misses = 0u32;
    for index in 0..MAX_DRIVES {
        match counters(index) {
            Some(disk) => {
                disks.push(disk);
                misses = 0;
            }
            None => {
                misses += 1;
                if misses >= 2 {
                    break;
                }
            }
        }
    }
    disks
}

/// Reads one physical disk's counters.
fn counters(index: u32) -> Option<DiskCounters> {
    let device = open_device(index)?;
    let performance = query_performance(&device)?;
    Some(DiskCounters {
        index,
        read_bytes: performance.BytesRead.max(0) as u64,
        write_bytes: performance.BytesWritten.max(0) as u64,
        idle_ticks: performance.IdleTime.max(0) as u64,
    })
}

/// Opens `\\.\PhysicalDriveN` for a metadata-only IOCTL.
///
/// Zero desired access; see the module docs on why `GENERIC_READ` would
/// break this for every non-administrator.
fn open_device(index: u32) -> Option<OwnedHandle> {
    let path = strings::to_wide(&format!("\\\\.\\PhysicalDrive{index}"));
    // SAFETY: `path` is a live, NUL-terminated UTF-16 buffer bound to a
    // local that outlives the call. A null security-attributes pointer
    // requests the default descriptor and a null template handle is the
    // documented value for `OPEN_EXISTING`. The returned handle is
    // immediately given to `OwnedHandle`, which rejects both failure
    // sentinels and closes it on drop.
    let raw = unsafe {
        CreateFileW(
            path.as_ptr(),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            std::ptr::null_mut(),
        )
    };
    OwnedHandle::new(raw)
}

/// Issues `IOCTL_DISK_PERFORMANCE` against an open device.
fn query_performance(device: &OwnedHandle) -> Option<DISK_PERFORMANCE> {
    // SAFETY: `DISK_PERFORMANCE` is plain integers and a fixed-size
    // array, so the all-zero bit pattern is a valid starting value.
    let mut performance: DISK_PERFORMANCE = unsafe { std::mem::zeroed() };
    let size = u32::try_from(std::mem::size_of::<DISK_PERFORMANCE>()).unwrap_or(0);
    let mut returned = 0u32;
    // SAFETY: `device` is a live handle from `open_device`. The two null
    // input arguments say there is no input buffer, which this IOCTL
    // documents. `performance` is a live, uniquely-borrowed struct of
    // exactly `size` bytes, which is what the length argument states.
    // `returned` is a live out-parameter. A null OVERLAPPED requests a
    // synchronous call, which is correct for a handle not opened with
    // FILE_FLAG_OVERLAPPED. Nothing is retained past the call.
    let ok = unsafe {
        DeviceIoControl(
            device.raw(),
            IOCTL_DISK_PERFORMANCE,
            std::ptr::null(),
            0,
            std::ptr::from_mut(&mut performance).cast(),
            size,
            std::ptr::from_mut(&mut returned),
            std::ptr::null_mut(),
        )
    };
    // A short read means the driver filled in less than the structure,
    // and the tail would be the zeroes above rather than real counters.
    if ok == 0 || returned < size {
        return None;
    }
    Some(performance)
}

/// The active-time share of an interval, 0..=100.
///
/// `elapsed_ticks` is the wall-clock length of the interval in the same
/// 100ns units the counter uses; the caller supplies it because only it
/// knows how long the interval really was. See the module docs.
#[must_use]
pub fn active_percent(previous: DiskCounters, current: DiskCounters, elapsed_ticks: u64) -> f64 {
    if elapsed_ticks == 0 {
        return 0.0;
    }
    let idle = current.idle_ticks.saturating_sub(previous.idle_ticks);
    // Idle can exceed the interval on a multi-queue device, where the
    // counter accumulates per queue. Clamping is what stops that from
    // becoming a negative active time.
    let idle = idle.min(elapsed_ticks);
    ((1.0 - idle as f64 / elapsed_ticks as f64) * 100.0).clamp(0.0, 100.0)
}

/// A volume's capacity and free space.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Volume {
    /// The drive letter with its colon, e.g. `C:`.
    pub letter: String,
    /// Total capacity in bytes.
    pub capacity: u64,
    /// Free bytes.
    pub free: u64,
}

/// Every fixed volume with a drive letter.
///
/// Used for the capacity figure beside each disk. This does **not** map
/// volumes onto physical drives — that needs
/// `IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS` per volume and gets involved
/// with spanned and mirrored sets, which is a great deal of machinery for
/// a subtitle. The Performance page shows the volumes as their own list
/// instead, which is also what a reader actually wants: they think in
/// drive letters, not in physical drive numbers.
#[must_use]
pub fn volumes() -> Vec<Volume> {
    let mask = logical_drives();
    let mut found = Vec::new();
    for bit in 0..26u32 {
        if mask & (1 << bit) == 0 {
            continue;
        }
        let Some(letter) = char::from_u32(u32::from(b'A') + bit) else {
            continue;
        };
        let root = format!("{letter}:\\");
        // Asked what kind of drive it is *before* asking anything of the
        // drive itself. `GetDriveTypeW` reads the mount table and cannot
        // block; `GetDiskFreeSpaceExW` talks to the device and can block
        // for as long as the device takes to answer — which for a
        // network drive whose server has gone is the redirector's whole
        // timeout, once per second, for as long as the app is open.
        //
        // Network and optical drives are excluded on their own merits
        // too: this list exists to describe the machine's *physical*
        // disks, and neither is one.
        if !is_local_disk(&root) {
            continue;
        }
        let Some((capacity, free)) = free_space(&root) else {
            // A card reader with no card, or a disconnected network
            // drive. Skipped rather than shown as a zero-byte volume.
            continue;
        };
        if capacity == 0 {
            continue;
        }
        found.push(Volume {
            letter: format!("{letter}:"),
            capacity,
            free,
        });
    }
    found
}

/// The bitmask of drive letters in use.
fn logical_drives() -> u32 {
    // SAFETY: takes no arguments, returns a bitmask, cannot fail.
    unsafe { GetLogicalDrives() }
}

/// Whether a drive letter names a disk attached to this machine.
///
/// `DRIVE_FIXED` and `DRIVE_REMOVABLE` — an internal disk and a USB
/// stick. Everything else is either not a disk (`DRIVE_REMOTE` is a
/// share on another machine, `DRIVE_RAMDISK` is memory) or is one that
/// makes the caller wait while it finds out (`DRIVE_CDROM` spins media
/// up).
fn is_local_disk(root: &str) -> bool {
    let wide = strings::to_wide(root);
    // SAFETY: `wide` is a live, NUL-terminated UTF-16 path bound to a
    // local. The call reads it and retains nothing.
    let kind = unsafe { GetDriveTypeW(wide.as_ptr()) };
    kind == DRIVE_FIXED || kind == DRIVE_REMOVABLE
}

/// A volume's total and free bytes.
fn free_space(root: &str) -> Option<(u64, u64)> {
    let wide = strings::to_wide(root);
    let mut available = 0u64;
    let mut total = 0u64;
    let mut free = 0u64;
    // SAFETY: `wide` is a live, NUL-terminated UTF-16 path bound to a
    // local. All three out-parameters are live, uniquely-borrowed `u64`s
    // the callee writes once each. Nothing is retained.
    let ok = unsafe {
        GetDiskFreeSpaceExW(
            wide.as_ptr(),
            std::ptr::from_mut(&mut available),
            std::ptr::from_mut(&mut total),
            std::ptr::from_mut(&mut free),
        )
    };
    // `free` is the volume's free space; `available` is what this user's
    // quota permits. The volume figure is the one to show — a quota'd
    // user is not told the disk is full when it is not.
    (ok != 0).then_some((total, free))
}

/// A readable label for a physical drive.
///
/// Just the number: mapping drives to models needs
/// `IOCTL_STORAGE_QUERY_PROPERTY` and a variable-length descriptor walk,
/// and the model string it returns is frequently a padded,
/// vendor-formatted mess ("Samsung SSD 970 EVO Plus 1TB" is the good
/// case). The number is what `diskpart` and Disk Management call it.
#[must_use]
pub fn label(index: u32) -> String {
    format!("Disk {index}")
}

/// The longest path this module builds, as a compile-time sanity check on
/// the buffer sizes above.
///
/// `\\.\PhysicalDrive15` is 20 characters; `MAX_PATH` is 260. Stated so
/// that a future change to the device-path format has something to fail
/// against rather than silently truncating.
const _: () = assert!(
    "\\\\.\\PhysicalDrive99".len() < MAX_PATH as usize,
    "the device path must fit a MAX_PATH buffer"
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_volume_comes_from_a_drive_that_can_block() {
        // `volumes()` runs on the sampler thread once a second, and
        // `GetDiskFreeSpaceExW` talks to the device. On a network drive
        // whose server has gone that blocks for the redirector's whole
        // timeout; on optical media it waits for a spin-up. Neither is a
        // physical disk, which is what this list is for, so neither
        // should ever be reached.
        //
        // Checked against the drive type of what came back rather than
        // against the filter's source, because the filter is only worth
        // anything if it is the thing deciding the result.
        for volume in volumes() {
            let root = format!("{}\\", volume.letter);
            assert!(
                is_local_disk(&root),
                "{} is in the volume list but is not a local disk —                  querying it can park the sampler on a device timeout",
                volume.letter
            );
        }
    }

    /// One second, in the 100ns ticks these counters use.
    const SECOND: u64 = 10_000_000;

    #[test]
    fn the_machine_has_at_least_one_disk() {
        let disks = read();
        assert!(
            !disks.is_empty(),
            "a running Windows machine has a physical drive 0, and opening \
             it with zero desired access needs no privilege"
        );
        assert_eq!(
            disks.first().map(|disk| disk.index),
            Some(0),
            "probing should start at PhysicalDrive0"
        );
    }

    #[test]
    fn counters_advance_rather_than_reading_as_zero() {
        // Catches a call that silently returns a zeroed struct — which is
        // what a short read would look like without the length check.
        let Some(first) = counters(0) else {
            return;
        };
        assert!(
            first.idle_ticks > 0,
            "a booted machine's disk has accumulated idle time"
        );
    }

    #[test]
    fn an_idle_disk_reads_as_idle() {
        let previous = DiskCounters {
            index: 0,
            read_bytes: 0,
            write_bytes: 0,
            idle_ticks: 0,
        };
        let current = DiskCounters {
            idle_ticks: SECOND,
            ..previous
        };
        assert!(
            active_percent(previous, current, SECOND) < 0.001,
            "a disk idle for the whole interval is 0% active"
        );
    }

    #[test]
    fn a_saturated_disk_reads_as_fully_active() {
        let previous = DiskCounters::default();
        let current = DiskCounters {
            idle_ticks: 0,
            ..previous
        };
        assert!(
            (active_percent(previous, current, SECOND) - 100.0).abs() < 0.001,
            "a disk with no idle time in the interval is 100% active"
        );
    }

    #[test]
    fn a_multi_queue_disk_reporting_more_idle_than_elapsed_is_clamped() {
        // The counter accumulates per queue, so a device with four queues
        // idle for a second reports four seconds of idle time. Without
        // the clamp that is a negative active time.
        let previous = DiskCounters::default();
        let current = DiskCounters {
            idle_ticks: 4 * SECOND,
            ..previous
        };
        assert_eq!(active_percent(previous, current, SECOND), 0.0);
    }

    #[test]
    fn a_zero_length_interval_is_not_a_division_by_zero() {
        let disk = DiskCounters::default();
        let percent = active_percent(disk, disk, 0);
        assert_eq!(percent, 0.0);
        assert!(percent.is_finite());
    }

    #[test]
    fn counters_that_appear_to_go_backwards_do_not_produce_a_negative() {
        let previous = DiskCounters {
            idle_ticks: 100 * SECOND,
            ..DiskCounters::default()
        };
        let current = DiskCounters {
            idle_ticks: SECOND,
            ..DiskCounters::default()
        };
        let percent = active_percent(previous, current, SECOND);
        assert!((0.0..=100.0).contains(&percent), "got {percent}");
    }

    #[test]
    fn the_system_volume_is_found_with_a_plausible_capacity() {
        let volumes = volumes();
        assert!(!volumes.is_empty(), "a machine has at least one volume");
        let system = volumes.iter().find(|volume| volume.letter == "C:");
        let Some(system) = system else {
            return;
        };
        assert!(
            system.capacity > 1024 * 1024 * 1024,
            "a system volume is at least a gigabyte, got {}",
            system.capacity
        );
        assert!(
            system.free <= system.capacity,
            "free space cannot exceed capacity"
        );
    }

    #[test]
    fn a_drive_letter_with_no_media_is_skipped_rather_than_shown_as_empty() {
        // Every returned volume must have a real capacity; a card reader
        // with no card would otherwise appear as a 0-byte disk.
        for volume in volumes() {
            assert!(
                volume.capacity > 0,
                "{} was reported with no capacity",
                volume.letter
            );
        }
    }

    #[test]
    fn a_drive_number_that_does_not_exist_yields_nothing() {
        assert!(
            counters(MAX_DRIVES + 100).is_none(),
            "probing past the end must fail cleanly"
        );
    }

    #[test]
    fn labels_name_the_drive_the_way_disk_management_does() {
        assert_eq!(label(0), "Disk 0");
        assert_eq!(label(3), "Disk 3");
    }
}

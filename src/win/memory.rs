// ============================================================================
// Module:       win::memory
// Description:  Physical and committed memory totals, and the kernel pool
//               figures the Performance view reports beside them.
//
// Dependencies: windows-sys (GlobalMemoryStatusEx, GetPerformanceInfo)
// ============================================================================

//! System memory.
//!
//! Two calls, because neither answers the whole question.
//! `GlobalMemoryStatusEx` gives installed and available physical memory —
//! the numbers behind "8.2/32.0 GB" — and `GetPerformanceInfo` gives the
//! commit charge, the commit limit, the file cache, and the two kernel
//! pools.
//!
//! The pool figures are the ones worth having and the reason this is not
//! just the first call. Non-paged pool that only ever climbs is a driver
//! leak, and it is the one leak that takes the whole machine down rather
//! than the process responsible — there is no process to end, and by the
//! time the machine is unresponsive there is no way to look. A task
//! manager that cannot show it cannot diagnose it.
//!
//! `GetPerformanceInfo` reports in *pages*, not bytes, and hands back the
//! page size to convert with. Forgetting that multiplication understates
//! the commit charge by a factor of 4096, which looks like a machine
//! using almost no memory rather than like a bug.

use crate::model::MemorySample;
use windows_sys::Win32::System::ProcessStatus::{GetPerformanceInfo, PERFORMANCE_INFORMATION};
use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};

/// The counts `GetPerformanceInfo` reports, which the caller pairs with a
/// process count and thread count for the status bar.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SystemCounts {
    /// Open kernel handles across the machine.
    pub handles: u32,
    /// Processes running.
    pub processes: u32,
    /// Threads across every process.
    pub threads: u32,
}

/// Reads the machine's memory state.
///
/// Returns the sample and the system-wide counts together, because they
/// come from one call and splitting them would mean making it twice.
///
/// Never fails: a call that does not answer leaves its fields zero rather
/// than taking the whole sample down. A missing memory readout is a gap
/// in one panel; a failed sample is a frozen app.
#[must_use]
pub fn read() -> (MemorySample, SystemCounts) {
    let mut sample = MemorySample::default();
    let mut counts = SystemCounts::default();

    if let Some(status) = global_status() {
        sample.total = status.ullTotalPhys;
        sample.available = status.ullAvailPhys;
    }

    if let Some(info) = performance_info() {
        // Everything here is in pages. See the module docs.
        let page = info.PageSize as u64;
        let bytes = |pages: usize| (pages as u64).saturating_mul(page);

        sample.committed = bytes(info.CommitTotal);
        sample.commit_limit = bytes(info.CommitLimit);
        sample.cached = bytes(info.SystemCache);
        sample.paged_pool = bytes(info.KernelPaged);
        sample.nonpaged_pool = bytes(info.KernelNonpaged);

        // `GlobalMemoryStatusEx` is the more direct source for these two,
        // but if it failed above they are still zero — and this call
        // reports them too, in pages.
        if sample.total == 0 {
            sample.total = bytes(info.PhysicalTotal);
            sample.available = bytes(info.PhysicalAvailable);
        }

        counts = SystemCounts {
            handles: info.HandleCount,
            processes: info.ProcessCount,
            threads: info.ThreadCount,
        };
    }

    (sample, counts)
}

/// `GlobalMemoryStatusEx`, wrapped.
///
/// The `dwLength` field must be set before the call — it is how the
/// function knows which version of the struct it was handed, and a zero
/// there makes it fail rather than fill anything in.
fn global_status() -> Option<MEMORYSTATUSEX> {
    let mut status = MEMORYSTATUSEX {
        dwLength: u32::try_from(std::mem::size_of::<MEMORYSTATUSEX>()).unwrap_or(0),
        ..MEMORYSTATUSEX::default()
    };
    let ok = read_global_memory_status(&mut status);
    (ok != 0).then_some(status)
}

/// `GetPerformanceInfo`, wrapped.
fn performance_info() -> Option<PERFORMANCE_INFORMATION> {
    let size = u32::try_from(std::mem::size_of::<PERFORMANCE_INFORMATION>()).unwrap_or(0);
    let mut info = PERFORMANCE_INFORMATION {
        cb: size,
        ..PERFORMANCE_INFORMATION::default()
    };
    let ok = read_performance_info(&mut info, size);
    (ok != 0).then_some(info)
}

/// Fills the caller-owned global-memory structure.
fn read_global_memory_status(status: &mut MEMORYSTATUSEX) -> i32 {
    // SAFETY: `status.dwLength` identifies its exact initialized struct size;
    // `status` is a live writable out-parameter and is not retained.
    unsafe { GlobalMemoryStatusEx(status) }
}

/// Fills the caller-owned performance counter structure.
fn read_performance_info(info: &mut PERFORMANCE_INFORMATION, size: u32) -> i32 {
    // SAFETY: `info.cb` and `size` state the structure's exact byte size;
    // `info` is a live writable out-parameter and is not retained.
    unsafe { GetPerformanceInfo(info, size) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reading_is_internally_consistent() {
        // Runs against the real machine, so it asserts relationships
        // rather than values.
        let (sample, counts) = read();
        assert!(sample.total > 0, "a machine has some memory installed");
        assert!(
            sample.available <= sample.total,
            "available ({}) cannot exceed installed ({})",
            sample.available,
            sample.total
        );
        assert!(
            sample.used() <= sample.total,
            "used memory cannot exceed installed"
        );
        let percent = sample.used_percent();
        assert!(
            (0.0..=100.0).contains(&percent),
            "memory load {percent} is out of range"
        );
        assert!(
            counts.processes > 0 && counts.threads >= counts.processes,
            "a running machine has processes, each with at least one \
             thread: {counts:?}"
        );
    }

    #[test]
    fn the_page_conversion_is_actually_applied() {
        // Forgetting the multiplication understates the commit charge by
        // a factor of 4096, which looks like a machine using almost no
        // memory rather than like a bug. A machine's commit charge is
        // always at least a few hundred megabytes.
        let (sample, _) = read();
        assert!(
            sample.committed > 64 * 1024 * 1024,
            "commit charge of {} bytes is implausibly small — the page \
             count was probably not converted to bytes",
            sample.committed
        );
        assert!(
            sample.commit_limit >= sample.committed,
            "the commit limit must be at least the commit charge"
        );
    }

    #[test]
    fn the_kernel_pools_are_reported() {
        // The reason `GetPerformanceInfo` is called at all. A non-paged
        // pool of zero means the call silently failed or the conversion
        // was dropped.
        let (sample, _) = read();
        assert!(
            sample.nonpaged_pool > 0,
            "a running kernel always has non-paged pool allocated"
        );
        assert!(sample.paged_pool > 0);
    }
}

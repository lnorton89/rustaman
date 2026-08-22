// ============================================================================
// Module:       win::nt::types
// Description:  The real ntdll struct layouts the public SDK redacts, with
//               compile-time size and offset assertions against them.
//
// Dependencies: windows-sys (UNICODE_STRING, HANDLE)
// ============================================================================

//! The `SYSTEM_*_INFORMATION` layouts, declared here rather than imported.
//!
//! ## Why these are not taken from `windows-sys`
//!
//! `windows-sys` does ship a `SYSTEM_PROCESS_INFORMATION`, and it is
//! useless for this app. It is generated from the public SDK header,
//! which publishes a *redacted* version of the struct — the fields
//! Microsoft does not document are replaced with `Reserved` arrays:
//!
//! ```text
//! pub struct SYSTEM_PROCESS_INFORMATION {   // windows-sys 0.61
//!     pub NextEntryOffset: u32,
//!     pub NumberOfThreads: u32,
//!     pub Reserved1: [u8; 48],              // <-- CreateTime, UserTime,
//!     pub ImageName: UNICODE_STRING,        //     KernelTime live here
//!     ...
//!     pub Reserved3: *mut c_void,           // <-- InheritedFromUniqueProcessId
//!     ...
//!     pub Reserved7: [i64; 6],              // <-- the six I/O counters
//! }
//! ```
//!
//! Every number this app is built on is inside one of those arrays.
//! `Reserved1` hides the CPU times the whole CPU column is computed from,
//! `Reserved3` hides the parent PID the process tree is built from, and
//! `Reserved7` hides the read/write counters behind the Disk column. A
//! struct that hides all three is not a smaller version of what is
//! needed; it is a different struct.
//!
//! So the real layout is declared here. This is what every process
//! explorer on Windows does, and the layout has been stable since
//! Windows 7 — but "stable" is a claim, not a guarantee, which is what
//! the assertions below are for.
//!
//! ## The assertions are the load-bearing part
//!
//! A struct declared by hand that is *wrong* does not fail to compile and
//! does not crash. It reads the neighbouring field: a CPU column that
//! silently shows handle counts, or an offset that walks the entry chain
//! into the middle of a string. So every struct here carries a
//! `const _: () = assert!(size_of::<T>() == ...)` for the x86-64 layout,
//! and the important fields carry offset assertions too. If a future
//! Windows or a future Rust changes the padding, this stops being a
//! compiling program.
//!
//! The layouts are 64-bit. `debug_assert`-style runtime checks would be
//! no use here — the numbers would already be wrong — so they are
//! compile-time.

use windows_sys::Win32::Foundation::{HANDLE, UNICODE_STRING};

/// `SystemProcessInformation`, the information class that returns every
/// process on the machine in one call.
///
/// Not exported by `windows-sys` (its `SYSTEM_INFORMATION_CLASS`
/// constants cover only the documented handful), so it is stated here.
pub const SYSTEM_PROCESS_INFORMATION_CLASS: i32 = 5;

/// `SystemProcessorPerformanceInformation`: per-logical-processor idle,
/// kernel, and user times.
pub const SYSTEM_PROCESSOR_PERFORMANCE_INFORMATION_CLASS: i32 = 8;

/// The real `SYSTEM_PROCESS_INFORMATION`.
///
/// Returned as a chain: each entry is followed by its threads, and
/// [`SystemProcessInformation::NextEntryOffset`] is the byte offset from
/// *this* entry to the next. A zero offset ends the chain.
///
/// Field names keep their Windows spelling. That is deliberate — this
/// struct is checked against `ntdll` documentation and against other
/// implementations, and renaming the fields to be idiomatic would make
/// that comparison harder for no gain. The `non_snake_case` lint is
/// suppressed for the struct rather than the module, so nothing else
/// picks up the exemption.
#[repr(C)]
#[derive(Clone, Copy)]
#[expect(
    non_snake_case,
    reason = "matches the ntdll layout it is checked against"
)]
pub struct SystemProcessInformation {
    /// Bytes from this entry to the next; zero at the end of the chain.
    pub NextEntryOffset: u32,
    /// Threads in this process, and the number of
    /// [`SystemThreadInformation`] entries following this one.
    pub NumberOfThreads: u32,
    /// Private working set, in bytes. Vista and later.
    pub WorkingSetPrivateSize: i64,
    /// Hard page faults since the process started. Windows 7 and later.
    pub HardFaultCount: u32,
    /// The most threads this process has ever had at once.
    pub NumberOfThreadsHighWatermark: u32,
    /// Total CPU cycles. Not used here — cycle counts are not comparable
    /// across cores of differing frequency, which on a modern
    /// heterogeneous CPU means they are not comparable at all.
    pub CycleTime: u64,
    /// Creation time, as a FILETIME. Half of this app's process identity.
    pub CreateTime: i64,
    /// Cumulative user-mode CPU time, in 100ns ticks.
    pub UserTime: i64,
    /// Cumulative kernel-mode CPU time, in 100ns ticks.
    pub KernelTime: i64,
    /// The image file name — *not* the full path, and a counted string
    /// with no guaranteed terminator. See [`super::super::strings`].
    pub ImageName: UNICODE_STRING,
    /// Base scheduling priority.
    pub BasePriority: i32,
    /// The process id, as a handle-shaped integer.
    pub UniqueProcessId: HANDLE,
    /// The creating process's id. Stale once the creator exits, which is
    /// why [`crate::model::tree`] validates it.
    pub InheritedFromUniqueProcessId: HANDLE,
    /// Open kernel handles.
    pub HandleCount: u32,
    /// Terminal-services session.
    pub SessionId: u32,
    /// An opaque key. Unused.
    pub UniqueProcessKey: usize,
    /// Peak virtual address space, in bytes.
    pub PeakVirtualSize: usize,
    /// Current virtual address space, in bytes.
    pub VirtualSize: usize,
    /// Page faults since the process started.
    pub PageFaultCount: u32,
    /// Peak working set, in bytes.
    pub PeakWorkingSetSize: usize,
    /// Current working set, in bytes — the "Memory" column.
    pub WorkingSetSize: usize,
    /// Peak paged pool quota.
    pub QuotaPeakPagedPoolUsage: usize,
    /// Paged pool quota in use.
    pub QuotaPagedPoolUsage: usize,
    /// Peak non-paged pool quota.
    pub QuotaPeakNonPagedPoolUsage: usize,
    /// Non-paged pool quota in use.
    pub QuotaNonPagedPoolUsage: usize,
    /// Private commit, in bytes — the "Private" column, and the honest
    /// one of the two memory figures.
    pub PagefileUsage: usize,
    /// Peak private commit.
    pub PeakPagefileUsage: usize,
    /// Private pages, in bytes.
    pub PrivatePageCount: usize,
    /// Read operations issued.
    pub ReadOperationCount: i64,
    /// Write operations issued.
    pub WriteOperationCount: i64,
    /// Other (control) operations issued.
    pub OtherOperationCount: i64,
    /// Bytes read — half of the "Disk" column.
    pub ReadTransferCount: i64,
    /// Bytes written — the other half.
    pub WriteTransferCount: i64,
    /// Bytes transferred by operations that are neither reads nor writes.
    pub OtherTransferCount: i64,
}

/// The 64-bit size of [`SystemProcessInformation`], as `ntdll` lays it
/// out.
///
/// Also the offset from an entry to its first thread record, which
/// [`super::process`] relies on.
pub const PROCESS_INFORMATION_SIZE: usize = 0x100;

// The layout assertions. See the module docs: a hand-declared struct
// that is wrong reads the neighbouring field rather than failing, so
// these are what make the declaration above trustworthy. Offsets are
// checked for the fields whose value would be silently plausible if it
// came from the wrong place — a CPU time that was really a handle count
// looks like a number either way.
const _: () = {
    assert!(
        core::mem::size_of::<SystemProcessInformation>() == PROCESS_INFORMATION_SIZE,
        "SYSTEM_PROCESS_INFORMATION is not the expected 256 bytes; the \
         thread records that follow each entry would be read at the wrong \
         offset"
    );
    assert!(core::mem::align_of::<SystemProcessInformation>() == 8);
    assert!(core::mem::offset_of!(SystemProcessInformation, CreateTime) == 0x20);
    assert!(core::mem::offset_of!(SystemProcessInformation, UserTime) == 0x28);
    assert!(core::mem::offset_of!(SystemProcessInformation, KernelTime) == 0x30);
    assert!(core::mem::offset_of!(SystemProcessInformation, ImageName) == 0x38);
    assert!(core::mem::offset_of!(SystemProcessInformation, UniqueProcessId) == 0x50);
    assert!(core::mem::offset_of!(SystemProcessInformation, InheritedFromUniqueProcessId) == 0x58);
    assert!(core::mem::offset_of!(SystemProcessInformation, HandleCount) == 0x60);
    assert!(core::mem::offset_of!(SystemProcessInformation, SessionId) == 0x64);
    assert!(core::mem::offset_of!(SystemProcessInformation, WorkingSetSize) == 0x90);
    assert!(core::mem::offset_of!(SystemProcessInformation, PagefileUsage) == 0xb8);
    assert!(core::mem::offset_of!(SystemProcessInformation, ReadTransferCount) == 0xe8);
    assert!(core::mem::offset_of!(SystemProcessInformation, WriteTransferCount) == 0xf0);
};

/// One thread of a process, as it follows the process entry.
///
/// Read only to answer one question — whether *every* thread is waiting
/// on a suspend, which is what makes a process "Suspended" rather than
/// merely idle. See [`super::process`].
#[repr(C)]
#[derive(Clone, Copy)]
#[expect(
    non_snake_case,
    reason = "matches the ntdll layout it is checked against"
)]
pub struct SystemThreadInformation {
    /// Cumulative kernel time, in 100ns ticks.
    pub KernelTime: i64,
    /// Cumulative user time, in 100ns ticks.
    pub UserTime: i64,
    /// Creation time, as a FILETIME.
    pub CreateTime: i64,
    /// Time spent in the current wait.
    pub WaitTime: u32,
    /// The thread's entry point.
    pub StartAddress: *mut core::ffi::c_void,
    /// The owning process id.
    pub UniqueProcess: HANDLE,
    /// This thread's id.
    pub UniqueThread: HANDLE,
    /// Dynamic priority.
    pub Priority: i32,
    /// Base priority.
    pub BasePriority: i32,
    /// Context switches since creation.
    pub ContextSwitches: u32,
    /// The `KTHREAD_STATE` this thread is in. 5 is Waiting.
    pub ThreadState: u32,
    /// Why it is waiting. 5 is `Suspended`.
    pub WaitReason: u32,
}

/// The 64-bit size of [`SystemThreadInformation`].
pub const THREAD_INFORMATION_SIZE: usize = 0x50;

const _: () = {
    assert!(
        core::mem::size_of::<SystemThreadInformation>() == THREAD_INFORMATION_SIZE,
        "SYSTEM_THREAD_INFORMATION is not the expected 80 bytes; walking \
         the thread array would drift out of alignment with it"
    );
    assert!(core::mem::offset_of!(SystemThreadInformation, ThreadState) == 0x44);
    assert!(core::mem::offset_of!(SystemThreadInformation, WaitReason) == 0x48);
};

/// `KTHREAD_STATE::Waiting`. A thread in any other state is running or
/// runnable, which settles the question immediately.
pub const THREAD_STATE_WAITING: u32 = 5;

/// `KWAIT_REASON::Suspended`. A waiting thread with this reason has been
/// suspended rather than merely blocked on I/O.
pub const WAIT_REASON_SUSPENDED: u32 = 5;

/// Per-logical-processor time totals.
///
/// The counterpart of [`SystemProcessInformation`] for the CPU graphs:
/// one entry per logical processor, each holding cumulative tick counts
/// that a delta between samples turns into a utilisation.
#[repr(C)]
#[derive(Clone, Copy, Default)]
#[expect(
    non_snake_case,
    reason = "matches the ntdll layout it is checked against"
)]
pub struct SystemProcessorPerformanceInformation {
    /// Cumulative idle time, in 100ns ticks.
    ///
    /// Note that `KernelTime` **includes** this — see
    /// [`super::cpu`], where getting that wrong reports an idle machine
    /// as fully busy.
    pub IdleTime: i64,
    /// Cumulative kernel time, idle included.
    pub KernelTime: i64,
    /// Cumulative user time.
    pub UserTime: i64,
    /// Cumulative DPC time. Part of `KernelTime`.
    pub DpcTime: i64,
    /// Cumulative interrupt time. Part of `KernelTime`.
    pub InterruptTime: i64,
    /// Interrupts serviced.
    pub InterruptCount: u32,
}

/// The 64-bit size of [`SystemProcessorPerformanceInformation`].
pub const PROCESSOR_PERFORMANCE_SIZE: usize = 0x30;

const _: () = {
    assert!(
        core::mem::size_of::<SystemProcessorPerformanceInformation>() == PROCESSOR_PERFORMANCE_SIZE,
        "SYSTEM_PROCESSOR_PERFORMANCE_INFORMATION is not the expected 48 \
         bytes; the per-core array would be read at the wrong stride and \
         every core's utilisation would be another core's"
    );
    assert!(core::mem::offset_of!(SystemProcessorPerformanceInformation, IdleTime) == 0x00);
    assert!(core::mem::offset_of!(SystemProcessorPerformanceInformation, KernelTime) == 0x08);
    assert!(core::mem::offset_of!(SystemProcessorPerformanceInformation, UserTime) == 0x10);
};

#[cfg(test)]
mod tests {
    use super::*;

    // The `const _: () = assert!(...)` blocks above are the real check
    // and they run at compile time, so a wrong layout is a build failure
    // rather than a test failure. These restate the two sizes the walking
    // code depends on so that a reader running the test suite sees them
    // asserted, and so that a future change that made the constants
    // disagree with the structs is caught twice.

    #[test]
    fn the_process_entry_is_the_size_the_walker_steps_by() {
        assert_eq!(
            core::mem::size_of::<SystemProcessInformation>(),
            PROCESS_INFORMATION_SIZE
        );
    }

    #[test]
    fn the_thread_entry_is_the_size_the_walker_steps_by() {
        assert_eq!(
            core::mem::size_of::<SystemThreadInformation>(),
            THREAD_INFORMATION_SIZE
        );
    }

    #[test]
    fn the_processor_entry_is_the_size_the_walker_steps_by() {
        assert_eq!(
            core::mem::size_of::<SystemProcessorPerformanceInformation>(),
            PROCESSOR_PERFORMANCE_SIZE
        );
    }
}

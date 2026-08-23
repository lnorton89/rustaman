// ============================================================================
// Module:       win::control
// Description:  The write side — ending, suspending, resuming, re-prioritising,
//               re-affinitising and throttling a process, plus the shell
//               actions.
//
// Dependencies: windows-sys (OpenProcess, TerminateProcess, ntdll suspend);
//               super::handle, super::strings
// ============================================================================

//! Acting on a process, rather than reading it.
//!
//! Everything here changes the state of the machine, so everything here
//! is written to fail *closed*: an operation that cannot be performed
//! returns an error naming what happened, and no operation is attempted
//! against a target that was not explicitly identified.
//!
//! ## The PID is checked against the creation time first
//!
//! Every function takes a [`ProcessKey`], not a PID. That is the single
//! most important thing in this module.
//!
//! A task manager's kill path has an inherent race: the user sees a row,
//! decides, and clicks — and somewhere in those seconds the process can
//! exit and Windows can hand its PID to something else. Acting on the PID
//! alone at that point terminates whatever now holds the number, which on
//! a busy machine is a real possibility and is silent when it happens:
//! the row disappears either way, and the user believes they killed what
//! they aimed at.
//!
//! So [`verify`] opens the process and compares its **creation time**
//! against the one the row carried. A mismatch means the target is gone
//! and the operation is refused. This costs one extra syscall per action
//! — an action a human initiates, at human speed — and it is the
//! difference between a tool that does what it is told and one that
//! occasionally destroys something else.
//!
//! ## The two pseudo-processes are refused
//!
//! PID 0 and PID 4 cannot be opened for termination and must not be
//! attempted. They are refused explicitly, with a message, rather than
//! being allowed to fail with a generic access error that reads like a
//! permissions problem the user could fix by elevating.

use super::handle::OwnedHandle;
use super::strings;
use crate::model::{Priority, ProcessKey};
use std::fmt;
use windows_sys::Win32::Foundation::{GetLastError, FILETIME};
use windows_sys::Win32::System::Threading::{
    GetPriorityClass, GetProcessInformation, GetProcessTimes, OpenProcess, SetPriorityClass,
    SetProcessAffinityMask, SetProcessInformation, TerminateProcess, ABOVE_NORMAL_PRIORITY_CLASS,
    BELOW_NORMAL_PRIORITY_CLASS, HIGH_PRIORITY_CLASS, IDLE_PRIORITY_CLASS, NORMAL_PRIORITY_CLASS,
    PROCESS_POWER_THROTTLING_CURRENT_VERSION, PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
    PROCESS_POWER_THROTTLING_STATE, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_INFORMATION,
    PROCESS_SET_LIMITED_INFORMATION, PROCESS_SUSPEND_RESUME, PROCESS_TERMINATE,
    REALTIME_PRIORITY_CLASS,
};

// The suspend and resume calls are undocumented ntdll exports with no
// Win32 equivalent. `windows-sys` does not declare them, so they are
// declared here — the same situation as the struct layouts in
// `super::nt::types`, and for the same reason: there is no documented
// alternative. `NtSuspendProcess` has been present and stable since
// Windows XP, and every process explorer on the platform calls it.
//
// The declared signature is the whole contract: one HANDLE in, an
// NTSTATUS out. Getting it wrong would be a stack mismatch rather than a
// compile error, which is why it is stated once here and wrapped
// immediately below.
#[link(name = "ntdll")]
unsafe extern "system" {
    /// Suspends every thread in a process. Returns an NTSTATUS.
    fn NtSuspendProcess(process: windows_sys::Win32::Foundation::HANDLE) -> i32;
    /// Resumes every thread in a process. Returns an NTSTATUS.
    fn NtResumeProcess(process: windows_sys::Win32::Foundation::HANDLE) -> i32;
}

/// Why an action could not be performed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionError {
    /// The process exited, or its PID now belongs to a different
    /// process. See the module docs.
    Gone,
    /// One of the kernel pseudo-processes, which cannot be acted on.
    Protected,
    /// The process could not be opened with the rights this action needs.
    ///
    /// Carries the Win32 error so the message can distinguish "access
    /// denied — try running elevated" from anything else.
    Denied(u32),
    /// The operation itself failed.
    Failed(u32),
}

impl fmt::Display for ActionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Gone => write!(
                formatter,
                "the process has already exited — nothing was changed"
            ),
            Self::Protected => write!(formatter, "this is a kernel process and cannot be changed"),
            Self::Denied(ERROR_ACCESS_DENIED) => write!(
                formatter,
                "access denied — this process is owned by another account \
                 or is protected. Running Rustaman as administrator may help."
            ),
            Self::Denied(code) => {
                write!(formatter, "the process could not be opened (error {code})")
            }
            Self::Failed(code) => write!(formatter, "the operation failed (error {code})"),
        }
    }
}

impl std::error::Error for ActionError {}

/// `ERROR_ACCESS_DENIED`, singled out because it is the one the user can
/// do something about.
const ERROR_ACCESS_DENIED: u32 = 5;

/// The result of an action.
pub type Action = Result<(), ActionError>;

/// Ends a process.
///
/// The exit code is 1 rather than 0: a process that was terminated did
/// not succeed, and anything watching it — a shell, a service manager, a
/// build tool — should be able to tell.
pub fn end(key: ProcessKey) -> Action {
    let process = verify(key, PROCESS_TERMINATE)?;
    let ok = terminate_verified_process(process.raw());
    if ok == 0 {
        return Err(ActionError::Failed(last_error()));
    }
    Ok(())
}

/// Suspends every thread in a process.
pub fn suspend(key: ProcessKey) -> Action {
    let process = verify(key, PROCESS_SUSPEND_RESUME)?;
    let status = suspend_process_handle(process.raw());
    status_to_action(status)
}

/// Resumes a suspended process.
pub fn resume(key: ProcessKey) -> Action {
    let process = verify(key, PROCESS_SUSPEND_RESUME)?;
    let status = resume_process_handle(process.raw());
    status_to_action(status)
}

/// Sets a process's scheduling priority class.
pub fn set_priority(key: ProcessKey, priority: Priority) -> Action {
    let process = verify(key, PROCESS_SET_INFORMATION)?;
    let class = match priority {
        Priority::Idle => IDLE_PRIORITY_CLASS,
        Priority::BelowNormal => BELOW_NORMAL_PRIORITY_CLASS,
        Priority::Normal => NORMAL_PRIORITY_CLASS,
        Priority::AboveNormal => ABOVE_NORMAL_PRIORITY_CLASS,
        Priority::High => HIGH_PRIORITY_CLASS,
        Priority::Realtime => REALTIME_PRIORITY_CLASS,
    };
    let ok = set_process_priority(process.raw(), class);
    if ok == 0 {
        return Err(ActionError::Failed(last_error()));
    }
    Ok(())
}

/// Reads a process's current priority class.
///
/// Returns `None` rather than a default when it cannot be read, so the
/// menu can show no selection instead of claiming the process is Normal
/// when that is merely the fallback.
#[must_use]
pub fn priority_of(key: ProcessKey) -> Option<Priority> {
    let process = verify(key, PROCESS_QUERY_LIMITED_INFORMATION).ok()?;
    let class = process_priority(process.raw());
    match class {
        IDLE_PRIORITY_CLASS => Some(Priority::Idle),
        BELOW_NORMAL_PRIORITY_CLASS => Some(Priority::BelowNormal),
        NORMAL_PRIORITY_CLASS => Some(Priority::Normal),
        ABOVE_NORMAL_PRIORITY_CLASS => Some(Priority::AboveNormal),
        HIGH_PRIORITY_CLASS => Some(Priority::High),
        REALTIME_PRIORITY_CLASS => Some(Priority::Realtime),
        // Zero is the documented failure return.
        _ => None,
    }
}

/// Reads whether a process is running under reduced quality of service
/// — Windows 11's "Efficiency mode".
///
/// `None` when the state could not be read, which is a normal answer
/// rather than a failure, and on most machines the *only* answer:
///
/// - **Windows 10 rejects the query.** `GetProcessInformation` with
///   `ProcessPowerThrottling` returns `ERROR_INVALID_PARAMETER` there —
///   the *setting* side has existed since 1709, but nothing could read
///   it back until 11. So on 10 every process reads as unknown, the
///   process list draws no marks, and the menu item that would toggle
///   it is not offered.
/// - **A protected process refuses to be opened** at all, on any build.
///
/// See [`set_efficiency`] for what the state actually is.
///
/// This is the one read in the module that runs against *many*
/// processes rather than one the user pointed at, so its cost is the
/// sampler's problem rather than a click's — see the sweep in
/// [`crate::engine::sampler`], which is why this is bounded to a slice
/// of the process list per pass rather than called for every row.
#[must_use]
pub fn efficiency_of(key: ProcessKey) -> Option<bool> {
    let process = verify(key, PROCESS_QUERY_LIMITED_INFORMATION).ok()?;
    let mut state = throttling_state(0, 0);
    let ok = read_power_throttling(process.raw(), &mut state);
    if ok == 0 {
        return None;
    }
    // Both halves have to say so. `ControlMask` is which policies the
    // process has an opinion about and `StateMask` is what that opinion
    // is, so a process that has opted *out* of throttling has the
    // execution-speed bit set in the first and clear in the second —
    // reading either alone reports it as throttled.
    Some(
        state.ControlMask & PROCESS_POWER_THROTTLING_EXECUTION_SPEED != 0
            && state.StateMask & PROCESS_POWER_THROTTLING_EXECUTION_SPEED != 0,
    )
}

/// Turns efficiency mode on or off for a process.
///
/// ## What this actually does, and why it is two calls
///
/// Efficiency mode is not one switch. Task Manager's checkbox sets two
/// things at once, and setting either alone gets a fraction of the
/// effect:
///
/// - **`PROCESS_POWER_THROTTLING_EXECUTION_SPEED`** is the EcoQoS
///   request. On a hybrid machine it tells the scheduler to keep the
///   process on the efficiency cores; on any machine it lets the power
///   manager clock it down. This is the part that saves the battery.
/// - **`IDLE_PRIORITY_CLASS`** is what stops the process competing with
///   the foreground for the cores it *is* given. This is the part the
///   user feels.
///
/// Turning it off clears the control bit entirely rather than setting
/// it and leaving the state clear. The difference matters: a clear
/// control mask means "the system decides", which is where a process
/// starts life, and an explicit opt-out would leave the process pinned
/// at full speed even when Windows would otherwise have throttled it —
/// a switch whose "off" is not the state before it was ever touched.
///
/// The priority goes back to `NORMAL_PRIORITY_CLASS`, which is not
/// necessarily where it came from — a process that launched itself
/// below normal is returned to normal by turning this off. Task Manager
/// does the same, and the alternative is remembering a per-process
/// value across a restart of this app for a case that does not arise.
pub fn set_efficiency(key: ProcessKey, on: bool) -> Action {
    // Both rights up front. Splitting them into two `verify` calls would
    // open the process twice and, worse, leave the throttling state
    // changed and the priority not when the second one failed.
    let process = verify(
        key,
        PROCESS_SET_INFORMATION | PROCESS_SET_LIMITED_INFORMATION,
    )?;

    let mut state = if on {
        throttling_state(
            PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
            PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
        )
    } else {
        throttling_state(0, 0)
    };
    let ok = write_power_throttling(process.raw(), &mut state);
    if ok == 0 {
        return Err(ActionError::Failed(last_error()));
    }

    let class = if on {
        IDLE_PRIORITY_CLASS
    } else {
        NORMAL_PRIORITY_CLASS
    };
    if set_process_priority(process.raw(), class) == 0 {
        return Err(ActionError::Failed(last_error()));
    }
    Ok(())
}

/// A `PROCESS_POWER_THROTTLING_STATE` with the current version stamped.
///
/// The version field is not decoration: the kernel reads the struct
/// according to it, and a zero there is rejected as an invalid
/// parameter.
fn throttling_state(control: u32, state: u32) -> PROCESS_POWER_THROTTLING_STATE {
    PROCESS_POWER_THROTTLING_STATE {
        Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
        ControlMask: control,
        StateMask: state,
    }
}

/// Restricts a process to a set of logical processors.
///
/// A mask of zero is refused rather than passed through: Windows treats
/// it as an invalid argument, but the error that comes back reads like a
/// permissions problem, and "no processors at all" is a request that
/// should be rejected on its own terms.
pub fn set_affinity(key: ProcessKey, mask: usize) -> Action {
    if mask == 0 {
        return Err(ActionError::Failed(ERROR_INVALID_PARAMETER));
    }
    let process = verify(key, PROCESS_SET_INFORMATION)?;
    let ok = set_process_affinity(process.raw(), mask);
    if ok == 0 {
        return Err(ActionError::Failed(last_error()));
    }
    Ok(())
}

/// `ERROR_INVALID_PARAMETER`.
const ERROR_INVALID_PARAMETER: u32 = 87;

/// Opens a process for `access`, confirming it is still the process the
/// caller meant.
///
/// The heart of this module; see the module docs on why the creation
/// time is compared.
fn verify(key: ProcessKey, access: u32) -> Result<OwnedHandle, ActionError> {
    if key.pid == crate::model::IDLE_PID || key.pid == crate::model::SYSTEM_PID {
        return Err(ActionError::Protected);
    }

    // The requested access, plus the right to read the creation time —
    // which is needed for the check and is granted far more freely than
    // the action rights.
    let combined = access | PROCESS_QUERY_LIMITED_INFORMATION;
    let raw = open_process_handle(combined, key.pid);
    let Some(process) = OwnedHandle::new(raw) else {
        let code = last_error();
        // `ERROR_INVALID_PARAMETER` from `OpenProcess` means there is no
        // such process — it exited between the snapshot and the click.
        if code == ERROR_INVALID_PARAMETER {
            return Err(ActionError::Gone);
        }
        return Err(ActionError::Denied(code));
    };

    match created_at(&process) {
        // The PID was recycled: this is a different process wearing the
        // number the row was showing. Refuse.
        Some(actual) if actual != key.started_at => Err(ActionError::Gone),
        Some(_) => Ok(process),
        // The creation time could not be read, so the identity cannot be
        // confirmed. Refuse rather than proceed: an unverifiable target
        // is exactly the case this check exists for.
        None => Err(ActionError::Gone),
    }
}

/// A process's creation time as a FILETIME.
fn created_at(process: &OwnedHandle) -> Option<u64> {
    let mut created = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let mut exited = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let mut kernel = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let mut user = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let ok = read_process_times(
        process.raw(),
        &mut created,
        &mut exited,
        &mut kernel,
        &mut user,
    );
    if ok == 0 {
        return None;
    }
    Some(filetime_to_u64(created))
}

/// Packs a `FILETIME` into the single integer the model compares.
#[must_use]
pub fn filetime_to_u64(time: FILETIME) -> u64 {
    (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime)
}

/// Turns an NTSTATUS into an action result.
fn status_to_action(status: i32) -> Action {
    if status == 0 {
        return Ok(());
    }
    // NTSTATUS values are not Win32 error codes; carrying the raw value
    // is more useful than a bad translation of it.
    Err(ActionError::Failed(status as u32))
}

/// `GetLastError`, wrapped.
fn last_error() -> u32 {
    read_last_error()
}

/// Opens Explorer with a file selected.
///
/// The path is passed as a separate argument rather than interpolated
/// into a command line, so a path containing a quote or a `&` cannot
/// become part of the command. `ShellExecuteW` takes the parameters as
/// one string, so the path is quoted — and a path containing a quote
/// character is refused rather than escaped, because Windows paths cannot
/// contain one and a path that appears to is not a path.
pub fn reveal_in_explorer(path: &std::path::Path) -> Action {
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    let text = path.to_string_lossy();
    if text.contains('"') {
        return Err(ActionError::Failed(ERROR_INVALID_PARAMETER));
    }
    let operation = strings::to_wide("open");
    let file = strings::to_wide("explorer.exe");
    let parameters = strings::to_wide(&format!("/select,\"{text}\""));

    let result = open_explorer_selection(&operation, &file, &parameters, SW_SHOWNORMAL);
    // Values of 32 or less are the documented error range.
    if (result as isize) <= 32 {
        return Err(ActionError::Failed(last_error()));
    }
    Ok(())
}

/// Terminates a process after [`verify`] confirmed its creation time.
fn terminate_verified_process(handle: windows_sys::Win32::Foundation::HANDLE) -> i32 {
    // SAFETY: `verify` opened this live handle with PROCESS_TERMINATE; both
    // arguments are by value and the API retains no state.
    unsafe { TerminateProcess(handle, 1) }
}

/// Invokes the required ntdll suspend export for a verified process handle.
fn suspend_process_handle(handle: windows_sys::Win32::Foundation::HANDLE) -> i32 {
    // SAFETY: the verified live handle is passed by value to the exact declared ntdll ABI.
    unsafe { NtSuspendProcess(handle) }
}

/// Invokes the required ntdll resume export for a verified process handle.
fn resume_process_handle(handle: windows_sys::Win32::Foundation::HANDLE) -> i32 {
    // SAFETY: the verified live handle is passed by value to the exact declared ntdll ABI.
    unsafe { NtResumeProcess(handle) }
}

/// Sets the documented priority class on a verified live process handle.
fn set_process_priority(handle: windows_sys::Win32::Foundation::HANDLE, class: u32) -> i32 {
    // SAFETY: `handle` is live and `class` is a documented by-value priority constant.
    unsafe { SetPriorityClass(handle, class) }
}

/// Reads the documented priority class from a verified live process handle.
fn process_priority(handle: windows_sys::Win32::Foundation::HANDLE) -> u32 {
    // SAFETY: `handle` is live; the API takes it by value and retains nothing.
    unsafe { GetPriorityClass(handle) }
}

/// Reads the power-throttling state of a verified live process handle
/// into caller-owned storage.
fn read_power_throttling(
    handle: windows_sys::Win32::Foundation::HANDLE,
    state: &mut PROCESS_POWER_THROTTLING_STATE,
) -> i32 {
    use windows_sys::Win32::System::Threading::ProcessPowerThrottling;

    let size = std::mem::size_of::<PROCESS_POWER_THROTTLING_STATE>();
    // SAFETY: `handle` is live and opened for query; `state` is a live writable
    // value of exactly the `size` bytes declared for `ProcessPowerThrottling`,
    // and the API writes it synchronously without retaining the pointer.
    unsafe {
        GetProcessInformation(
            handle,
            ProcessPowerThrottling,
            std::ptr::from_mut(state).cast(),
            u32::try_from(size).unwrap_or(0),
        )
    }
}

/// Applies a power-throttling state to a verified live process handle.
fn write_power_throttling(
    handle: windows_sys::Win32::Foundation::HANDLE,
    state: &mut PROCESS_POWER_THROTTLING_STATE,
) -> i32 {
    use windows_sys::Win32::System::Threading::ProcessPowerThrottling;

    let size = std::mem::size_of::<PROCESS_POWER_THROTTLING_STATE>();
    // SAFETY: `handle` is live and opened for set; `state` is a live value of
    // exactly the `size` bytes declared for `ProcessPowerThrottling` with its
    // version stamped, and the API reads it synchronously and retains nothing.
    unsafe {
        SetProcessInformation(
            handle,
            ProcessPowerThrottling,
            std::ptr::from_mut(state).cast(),
            u32::try_from(size).unwrap_or(0),
        )
    }
}

/// Applies a non-zero affinity mask to a verified live process handle.
fn set_process_affinity(handle: windows_sys::Win32::Foundation::HANDLE, mask: usize) -> i32 {
    // SAFETY: `handle` is live and `mask` is a validated non-zero by-value bit mask.
    unsafe { SetProcessAffinityMask(handle, mask) }
}

/// Opens a process handle for the caller to wrap in [`OwnedHandle`].
fn open_process_handle(access: u32, pid: u32) -> windows_sys::Win32::Foundation::HANDLE {
    // SAFETY: all arguments are by value; the returned handle is immediately owned or rejected.
    unsafe { OpenProcess(access, 0, pid) }
}

/// Writes all four process timestamps into distinct caller-owned values.
fn read_process_times(
    handle: windows_sys::Win32::Foundation::HANDLE,
    created: &mut FILETIME,
    exited: &mut FILETIME,
    kernel: &mut FILETIME,
    user: &mut FILETIME,
) -> i32 {
    // SAFETY: `handle` is live and each reference is a distinct live writable
    // FILETIME out-parameter; the API retains neither the handle nor pointers.
    unsafe { GetProcessTimes(handle, created, exited, kernel, user) }
}

/// Reads the caller thread's last Win32 error without changing it.
fn read_last_error() -> u32 {
    // SAFETY: this parameterless call returns thread-local state by value.
    unsafe { GetLastError() }
}

/// Opens Explorer with a caller-owned operation, executable, and selection parameters.
fn open_explorer_selection(
    operation: &[u16],
    file: &[u16],
    parameters: &[u16],
    show: i32,
) -> windows_sys::Win32::Foundation::HINSTANCE {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;

    // SAFETY: all slices are live NUL-terminated UTF-16 strings; null owner and
    // directory are documented defaults, and ShellExecute retains no pointer.
    unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            operation.as_ptr(),
            file.as_ptr(),
            parameters.as_ptr(),
            std::ptr::null(),
            show,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// This process's real key, read from the kernel rather than assumed.
    fn own_key() -> Option<ProcessKey> {
        let pid = std::process::id();
        let raw = open_process_handle(PROCESS_QUERY_LIMITED_INFORMATION, pid);
        let process = OwnedHandle::new(raw)?;
        Some(ProcessKey {
            pid,
            started_at: created_at(&process)?,
        })
    }

    #[test]
    fn a_stale_creation_time_refuses_the_action() {
        // The whole point of the module. A row whose process exited and
        // whose PID was reused must not end whatever now holds it.
        let Some(mut key) = own_key() else {
            return;
        };
        key.started_at = key.started_at.wrapping_add(1);
        assert_eq!(
            end(key),
            Err(ActionError::Gone),
            "a mismatched creation time must refuse, not terminate this \
             very test process"
        );
        assert_eq!(suspend(key), Err(ActionError::Gone));
        assert_eq!(set_priority(key, Priority::Idle), Err(ActionError::Gone));
        assert_eq!(set_affinity(key, 1), Err(ActionError::Gone));
    }

    #[test]
    fn the_kernel_pseudo_processes_are_refused_by_name() {
        // Refused explicitly rather than being allowed to fail with a
        // generic access error, which reads like something elevating
        // would fix.
        for pid in [crate::model::IDLE_PID, crate::model::SYSTEM_PID] {
            let key = ProcessKey { pid, started_at: 0 };
            assert_eq!(end(key), Err(ActionError::Protected), "PID {pid}");
            assert_eq!(suspend(key), Err(ActionError::Protected));
            assert_eq!(resume(key), Err(ActionError::Protected));
        }
    }

    #[test]
    fn a_pid_that_does_not_exist_is_reported_as_gone() {
        let key = ProcessKey {
            pid: 0xffff_fffe,
            started_at: 12345,
        };
        assert!(
            matches!(end(key), Err(ActionError::Gone | ActionError::Denied(_))),
            "a nonexistent PID must not be reported as a success"
        );
    }

    #[test]
    fn this_process_can_read_its_own_priority() {
        let Some(key) = own_key() else {
            return;
        };
        assert!(
            priority_of(key).is_some(),
            "a process can always query itself"
        );
    }

    #[test]
    fn this_process_can_read_its_own_efficiency_state() {
        // Not asserting *which* state: a machine can legitimately have
        // put this test process into efficiency mode. What matters is
        // that the query answers rather than reporting the unknown state
        // that would blank the column on every row.
        let Some(key) = own_key() else {
            return;
        };
        // Which answer is right depends on the machine, and that *is*
        // the assertion. Windows 10 rejects the query outright with
        // `ERROR_INVALID_PARAMETER` — the setting side has existed since
        // 1709, the reading side arrived with 11 — so the same call has
        // to come back `None` there and `Some` here, and a build that
        // got the struct layout or the version stamp wrong would come
        // back `None` on both.
        let windows_11 = crate::win::system::Facts::read().info.is_windows_11();
        assert_eq!(
            efficiency_of(key).is_some(),
            windows_11,
            "a process can always query itself on Windows 11, and no              process can query anything on Windows 10"
        );
    }

    #[test]
    fn efficiency_mode_round_trips_on_this_process() {
        // Turning it on and back off on the test process itself, which
        // is the only process a test may touch. The read is what proves
        // the two mask fields are being written the way the kernel reads
        // them — a wrong `Version` is accepted as an invalid parameter
        // and a wrong mask silently does nothing.
        let Some(key) = own_key() else {
            return;
        };
        let Some(original) = efficiency_of(key) else {
            return;
        };

        let applied = set_efficiency(key, true).is_ok();
        let observed = efficiency_of(key);
        // Put it back *before* asserting. An assertion that fires here
        // would otherwise leave the test runner itself throttled and at
        // idle priority for the rest of the suite.
        let _ = set_efficiency(key, original);

        if applied {
            assert_eq!(observed, Some(true), "the throttling state did not take");
            assert_eq!(
                efficiency_of(key),
                Some(original),
                "turning it off did not restore the state it started in"
            );
        }
    }

    #[test]
    fn a_stale_creation_time_refuses_to_throttle_a_reused_pid() {
        // Same rule as every other action here: efficiency mode is a
        // write, so it goes through `verify` and refuses a key whose
        // process has been replaced.
        let Some(mut key) = own_key() else {
            return;
        };
        key.started_at = key.started_at.wrapping_add(1);
        assert_eq!(set_efficiency(key, true), Err(ActionError::Gone));
        assert_eq!(efficiency_of(key), None);
    }

    #[test]
    fn an_empty_affinity_mask_is_refused_on_its_own_terms() {
        // "No processors at all" should be rejected as the nonsense it
        // is, rather than producing an error that reads like a
        // permissions problem.
        let Some(key) = own_key() else {
            return;
        };
        assert_eq!(
            set_affinity(key, 0),
            Err(ActionError::Failed(ERROR_INVALID_PARAMETER))
        );
    }

    #[test]
    fn a_path_containing_a_quote_is_refused_rather_than_escaped() {
        // Windows paths cannot contain a quote character, so a path that
        // appears to is not a path — and interpolating it into the
        // `/select,"..."` parameter would let it break out.
        let path = std::path::Path::new("C:\\a\" & calc.exe & \"b.txt");
        assert_eq!(
            reveal_in_explorer(path),
            Err(ActionError::Failed(ERROR_INVALID_PARAMETER))
        );
    }

    #[test]
    fn a_filetime_packs_the_way_the_model_compares_it() {
        let time = FILETIME {
            dwLowDateTime: 0x8765_4321,
            dwHighDateTime: 0x0000_01db,
        };
        assert_eq!(filetime_to_u64(time), 0x0000_01db_8765_4321);
    }

    #[test]
    fn the_access_denied_message_says_what_to_do_about_it() {
        let message = ActionError::Denied(ERROR_ACCESS_DENIED).to_string();
        assert!(
            message.contains("administrator"),
            "the one error the user can act on should say so: {message}"
        );
        let other = ActionError::Denied(999).to_string();
        assert!(
            !other.contains("administrator"),
            "an unrelated failure should not suggest elevating: {other}"
        );
    }

    #[test]
    fn every_error_reads_as_a_sentence() {
        for error in [
            ActionError::Gone,
            ActionError::Protected,
            ActionError::Denied(ERROR_ACCESS_DENIED),
            ActionError::Failed(5),
        ] {
            let message = error.to_string();
            assert!(!message.is_empty());
            assert!(
                message.chars().next().is_some_and(char::is_lowercase),
                "messages are appended to a sentence stem, so they start \
                 lowercase: {message}"
            );
        }
    }
}

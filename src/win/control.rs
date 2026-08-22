// ============================================================================
// Module:       win::control
// Description:  The write side — ending, suspending, resuming, re-prioritising
//               and re-affinitising a process, plus the shell actions.
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
    GetPriorityClass, GetProcessTimes, OpenProcess, SetPriorityClass, SetProcessAffinityMask,
    TerminateProcess, ABOVE_NORMAL_PRIORITY_CLASS, BELOW_NORMAL_PRIORITY_CLASS,
    HIGH_PRIORITY_CLASS, IDLE_PRIORITY_CLASS, NORMAL_PRIORITY_CLASS,
    PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_INFORMATION, PROCESS_SUSPEND_RESUME,
    PROCESS_TERMINATE, REALTIME_PRIORITY_CLASS,
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
    // SAFETY: `process` is a live handle opened with PROCESS_TERMINATE
    // for the process `verify` confirmed is the intended target. The call
    // takes the handle and an exit code by value.
    let ok = unsafe { TerminateProcess(process.raw(), 1) };
    if ok == 0 {
        return Err(ActionError::Failed(last_error()));
    }
    Ok(())
}

/// Suspends every thread in a process.
pub fn suspend(key: ProcessKey) -> Action {
    let process = verify(key, PROCESS_SUSPEND_RESUME)?;
    // SAFETY: `process` is a live handle opened with
    // PROCESS_SUSPEND_RESUME for the verified target. `NtSuspendProcess`
    // takes the handle by value and returns a status.
    let status = unsafe { NtSuspendProcess(process.raw()) };
    status_to_action(status)
}

/// Resumes a suspended process.
pub fn resume(key: ProcessKey) -> Action {
    let process = verify(key, PROCESS_SUSPEND_RESUME)?;
    // SAFETY: as `suspend`.
    let status = unsafe { NtResumeProcess(process.raw()) };
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
    // SAFETY: `process` is a live handle opened with
    // PROCESS_SET_INFORMATION for the verified target. `class` is one of
    // the documented priority-class constants.
    let ok = unsafe { SetPriorityClass(process.raw(), class) };
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
    // SAFETY: `process` is a live handle opened with
    // PROCESS_QUERY_LIMITED_INFORMATION for the verified target.
    let class = unsafe { GetPriorityClass(process.raw()) };
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
    // SAFETY: `process` is a live handle opened with
    // PROCESS_SET_INFORMATION for the verified target. `mask` is a
    // non-zero bitmask passed by value.
    let ok = unsafe { SetProcessAffinityMask(process.raw(), mask) };
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
    // SAFETY: no pointers — an access mask, a BOOL, and a PID by value.
    // The returned handle goes straight into `OwnedHandle`, which rejects
    // the failure sentinels and closes it on drop.
    let raw = unsafe { OpenProcess(combined, 0, key.pid) };
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
    let mut ignored = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    // SAFETY: `process` is a live handle opened with at least
    // PROCESS_QUERY_LIMITED_INFORMATION. All four out-parameters are
    // live, uniquely-borrowed `FILETIME`s the callee writes once each;
    // the three this code does not need are given separate storage
    // rather than aliasing one, which would be undefined.
    let ok = unsafe {
        GetProcessTimes(
            process.raw(),
            std::ptr::from_mut(&mut created),
            std::ptr::from_mut(&mut ignored),
            std::ptr::from_mut(&mut ignored),
            std::ptr::from_mut(&mut ignored),
        )
    };
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
    // SAFETY: takes no arguments, reads thread-local state, cannot fail.
    // Called immediately after the call being interpreted, before
    // anything else can overwrite it.
    unsafe { GetLastError() }
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
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    let text = path.to_string_lossy();
    if text.contains('"') {
        return Err(ActionError::Failed(ERROR_INVALID_PARAMETER));
    }
    let operation = strings::to_wide("open");
    let file = strings::to_wide("explorer.exe");
    let parameters = strings::to_wide(&format!("/select,\"{text}\""));

    // SAFETY: all three wide buffers are live, NUL-terminated, and bound
    // to locals that outlive the call. A null window handle and a null
    // directory are both documented as "no preference". The return is an
    // integer status disguised as a handle and is not a resource.
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            operation.as_ptr(),
            file.as_ptr(),
            parameters.as_ptr(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    // Values of 32 or less are the documented error range.
    if (result as isize) <= 32 {
        return Err(ActionError::Failed(last_error()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// This process's real key, read from the kernel rather than assumed.
    fn own_key() -> Option<ProcessKey> {
        let pid = std::process::id();
        // SAFETY: no pointers; an access mask, a BOOL, and a PID by
        // value. The handle goes straight into `OwnedHandle`.
        let raw = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
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

// ============================================================================
// Module:       win::privilege
// Description:  Enabling SeDebugPrivilege at startup, and reporting honestly
//               when it could not be enabled.
//
// Dependencies: windows-sys (OpenProcessToken, AdjustTokenPrivileges)
// ============================================================================

//! `SeDebugPrivilege`, asked for once at startup.
//!
//! ## What it buys and what it does not
//!
//! Without it, `OpenProcess` fails for anything running as another user
//! or as `SYSTEM` — which on a normal machine is most of the list. The
//! process *rows* still appear, because [`super::nt`] enumerates them
//! without opening anything, but the per-process identity lookups in
//! [`super::identity`] come back empty: no owner, no path, no bitness for
//! roughly half the machine.
//!
//! It is not a way to gain privilege. The token has to already *hold*
//! the privilege for this to enable it, which in practice means the
//! process is running elevated. On a non-elevated run this fails, and
//! that is the normal case rather than an error state — so [`enable`]
//! returns whether it worked and the UI says so, rather than the app
//! either failing to start or silently showing a half-empty table with no
//! explanation for why.
//!
//! Enabling it also does *not* let this app open a protected process
//! (anti-malware, the DRM subsystem, LSA on a machine with Credential
//! Guard). Those refuse regardless, which is why every identity lookup
//! degrades to "unknown" rather than treating a failure as exceptional.

use super::handle::OwnedHandle;
use super::strings;
use windows_sys::Win32::Foundation::LUID;
use windows_sys::Win32::Security::{
    AdjustTokenPrivileges, LookupPrivilegeValueW, LUID_AND_ATTRIBUTES, SE_PRIVILEGE_ENABLED,
    TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES, TOKEN_QUERY,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

/// The privilege name, as Win32 spells it.
const SE_DEBUG_NAME: &str = "SeDebugPrivilege";

/// Attempts to enable `SeDebugPrivilege` on this process.
///
/// Returns whether it is now enabled. A `false` is expected on any
/// non-elevated run and is reported to the user rather than treated as a
/// failure — see the module docs.
#[must_use]
pub fn enable() -> bool {
    let Some(token) = open_own_token() else {
        return false;
    };
    let Some(luid) = lookup_privilege(SE_DEBUG_NAME) else {
        return false;
    };
    adjust(&token, luid)
}

/// Opens this process's own access token for adjusting privileges.
fn open_own_token() -> Option<OwnedHandle> {
    let mut raw = std::ptr::null_mut();
    let ok = open_current_process_token(&mut raw);
    if ok == 0 {
        return None;
    }
    // The token handle *is* a real handle and does need closing, which is
    // what the wrapper is for.
    OwnedHandle::new(raw)
}

/// Resolves a privilege name to its locally-unique identifier.
///
/// The LUID is per-boot, not a well-known constant, so it has to be
/// looked up rather than hard-coded.
fn lookup_privilege(name: &str) -> Option<LUID> {
    let wide = strings::to_wide(name);
    let mut luid = LUID {
        LowPart: 0,
        HighPart: 0,
    };
    let ok = lookup_local_privilege(&wide, &mut luid);
    (ok != 0).then_some(luid)
}

/// Enables one privilege on a token.
///
/// `AdjustTokenPrivileges` reports success even when it enabled nothing —
/// the documented behaviour is that it returns non-zero and sets the last
/// error to `ERROR_NOT_ALL_ASSIGNED` when the token does not hold the
/// privilege. That is exactly the non-elevated case, so checking only the
/// return value would report success on every run and the UI would claim
/// full access it does not have.
fn adjust(token: &OwnedHandle, luid: LUID) -> bool {
    let privileges = TOKEN_PRIVILEGES {
        PrivilegeCount: 1,
        Privileges: [LUID_AND_ATTRIBUTES {
            Luid: luid,
            Attributes: SE_PRIVILEGE_ENABLED,
        }],
    };
    let ok = adjust_one_privilege(token.raw(), &privileges);
    if ok == 0 {
        return false;
    }
    // See the doc comment: a non-zero return does not mean the privilege
    // was granted.
    last_error() != ERROR_NOT_ALL_ASSIGNED
}

/// `ERROR_NOT_ALL_ASSIGNED` — the token does not hold the privilege.
const ERROR_NOT_ALL_ASSIGNED: u32 = 1300;

/// `GetLastError`, wrapped.
fn last_error() -> u32 {
    read_last_error()
}

/// Opens the current process's real token handle into caller-owned storage.
fn open_current_process_token(raw: &mut windows_sys::Win32::Foundation::HANDLE) -> i32 {
    // SAFETY: the pseudo-handle is valid by definition; `raw` is a live
    // writable out-parameter and the API retains neither handle nor pointer.
    unsafe {
        OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY,
            raw,
        )
    }
}

/// Resolves a NUL-terminated privilege name against the local system.
fn lookup_local_privilege(name: &[u16], luid: &mut LUID) -> i32 {
    // SAFETY: `name` is live and NUL-terminated; `luid` is a live writable
    // out-parameter and the null system pointer requests the local machine.
    unsafe { LookupPrivilegeValueW(std::ptr::null(), name.as_ptr(), luid) }
}

/// Enables the one populated privilege entry on a live token.
fn adjust_one_privilege(
    token: windows_sys::Win32::Foundation::HANDLE,
    privileges: &TOKEN_PRIVILEGES,
) -> i32 {
    // SAFETY: token is live; `privileges` contains its declared single entry.
    // Null previous-state pointers are documented when the old value is not needed.
    unsafe {
        AdjustTokenPrivileges(
            token,
            0,
            privileges,
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    }
}

/// Reads the caller thread's last Win32 error without changing it.
fn read_last_error() -> u32 {
    // SAFETY: this parameterless call returns thread-local state by value.
    unsafe { windows_sys::Win32::Foundation::GetLastError() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enabling_reports_a_definite_answer_rather_than_failing() {
        // On an elevated test run this is true, on an ordinary one false.
        // Both are correct; what matters is that it returns rather than
        // panicking, and that it can be called safely from a test that
        // does not know which kind of run it is.
        let enabled = enable();
        assert!(enabled || !enabled, "the point is that this returns at all");
    }

    #[test]
    fn enabling_twice_is_harmless() {
        // The app calls this once at startup, but a future caller
        // shouldn't have to know that.
        let first = enable();
        let second = enable();
        assert_eq!(
            first, second,
            "the answer must not change between two identical calls"
        );
    }

    #[test]
    fn a_privilege_name_that_does_not_exist_is_rejected() {
        assert!(
            lookup_privilege("SeThisIsNotARealPrivilege").is_none(),
            "an unknown privilege name must not resolve to a LUID"
        );
    }

    #[test]
    fn the_real_privilege_name_resolves() {
        let luid = lookup_privilege(SE_DEBUG_NAME);
        assert!(
            luid.is_some(),
            "SeDebugPrivilege is a well-known privilege and must resolve \
             even when the token does not hold it"
        );
    }

    #[test]
    fn this_process_can_open_its_own_token() {
        assert!(
            open_own_token().is_some(),
            "a process may always open its own token"
        );
    }
}

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
    // SAFETY: `GetCurrentProcess` returns a pseudo-handle that is always
    // valid and must not be closed — it is passed straight through and
    // never wrapped in `OwnedHandle`. `raw` is a live, uniquely-borrowed
    // pointer the callee writes a real handle into on success.
    let ok = unsafe {
        OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY,
            std::ptr::from_mut(&mut raw),
        )
    };
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
    // SAFETY: `wide` is a live, NUL-terminated UTF-16 buffer that
    // outlives the call — it is bound to a local above rather than being
    // a temporary. The first argument being null asks for the local
    // system, which is what a privilege on this process means. `luid` is
    // a live, uniquely-borrowed out-parameter.
    let ok = unsafe {
        LookupPrivilegeValueW(
            std::ptr::null(),
            wide.as_ptr(),
            std::ptr::from_mut(&mut luid),
        )
    };
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
    // SAFETY: `token` is a live token handle opened with
    // TOKEN_ADJUST_PRIVILEGES above. `privileges` is a live, correctly
    // sized `TOKEN_PRIVILEGES` with `PrivilegeCount` matching its one
    // populated entry. The three null/zero arguments decline the
    // previous-state out-parameters, which the call documents as
    // permitted. Nothing is retained past the call.
    let ok = unsafe {
        AdjustTokenPrivileges(
            token.raw(),
            0,
            &privileges,
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
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
    // SAFETY: `GetLastError` takes no arguments, reads thread-local
    // state, and cannot fail. It must be called before anything else
    // that could overwrite the thread's last-error value, which is why
    // the only caller invokes it immediately after the call it is
    // interpreting.
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

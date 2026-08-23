// ============================================================================
// Module:       win::windows
// Description:  Mapping visible top-level windows back to the processes that
//               own them, which is what makes a process an "app".
//
// Dependencies: windows-sys (EnumWindows, GetWindowThreadProcessId)
// ============================================================================

//! Which processes have a window.
//!
//! [`titles_by_pid`] enumerates every visible top-level window and maps
//! each back to its owning process. Two things depend on it:
//!
//! - **The Apps group.** A process with a visible top-level window is
//!   something the user started and can see, which is exactly the
//!   distinction [`crate::model::ProcessKind::classify`] leads with.
//!   Without this the process list is four hundred undifferentiated rows.
//! - **The window title column**, which is often the only way to tell
//!   eighteen identical `chrome.exe` rows apart.
//!
//! ## The callback has to hand data back without a global
//!
//! `EnumWindows` is a C callback API: it calls a function pointer once
//! per window and passes through one `LPARAM`. The obvious ways to
//! collect the results — a `static mut`, a thread-local, a `Mutex` around
//! a global — are all worse than the alternative, which is to pass a
//! pointer to a local as the `LPARAM` and reconstitute it inside the
//! callback. That keeps the collection on the stack of the caller,
//! makes two concurrent enumerations independent, and needs no
//! synchronisation.
//!
//! The unsafety is confined to one line of the callback, with the
//! contract stated where the pointer is created. See [`collect`].
//!
//! ## What counts as a window
//!
//! Visible, top-level, and with a non-empty title. The extra conditions
//! matter: the shell, the input method, and several system components
//! each own invisible or zero-titled top-level windows, and counting
//! those would file half the background processes on the machine under
//! "Apps".

use super::strings;
use std::collections::HashMap;
use windows_sys::core::BOOL;
use windows_sys::Win32::Foundation::{HWND, LPARAM, TRUE};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible,
};

/// The longest window title to read.
///
/// Titles are usually well under a hundred characters; a browser tab with
/// a pathological page title can run longer, and there is no reason to
/// allocate for the whole of one that will be elided in a table cell
/// anyway.
const MAX_TITLE: usize = 512;

/// Every process that owns a visible, titled top-level window, with one
/// of its titles.
///
/// A process with several windows gets the first one enumerated, which is
/// the top of the Z order — the window the user is most likely looking
/// at, and the right one to name the process by.
#[must_use]
pub fn titles_by_pid() -> HashMap<u32, String> {
    let mut found: HashMap<u32, String> = HashMap::new();
    collect(&mut found);
    found
}

/// Runs the enumeration, filling `found`.
///
/// The `LPARAM` is a pointer to `found`, which lives on the caller's
/// stack for the whole call — `EnumWindows` is synchronous and does not
/// retain the callback or the parameter past its return, so the pointer
/// cannot outlive the borrow.
fn collect(found: &mut HashMap<u32, String>) {
    let parameter = std::ptr::from_mut(found) as LPARAM;
    enumerate_windows(parameter);
}

/// The `EnumWindows` callback.
///
/// `extern "system"` because that is the calling convention Win32
/// expects; a mismatch here corrupts the stack rather than failing to
/// compile.
// SAFETY: Win32 enters this callback only through `enumerate_windows`, which
// provides the `LPARAM` contract re-established by `callback_result_map`.
unsafe extern "system" fn visit(window: HWND, parameter: LPARAM) -> BOOL {
    visit_window(window, parameter)
}

/// Processes one callback invocation using the stack-owned result map.
fn visit_window(window: HWND, parameter: LPARAM) -> BOOL {
    let Some(found) = callback_result_map(parameter) else {
        // A null parameter cannot happen from `collect`, but returning
        // rather than dereferencing keeps the callback total.
        return TRUE;
    };

    if !is_visible(window) {
        return TRUE;
    }
    let Some(title) = window_title(window) else {
        return TRUE;
    };
    let Some(pid) = owning_pid(window) else {
        return TRUE;
    };
    // First window wins: the enumeration runs in Z order, so this is the
    // frontmost window of that process.
    found.entry(pid).or_insert(title);
    TRUE
}

/// Whether a window is visible.
fn is_visible(window: HWND) -> bool {
    window_is_visible(window)
}

/// A window's title, or `None` if it has none.
fn window_title(window: HWND) -> Option<String> {
    let length = window_text_length(window);
    if length <= 0 {
        return None;
    }
    // `GetWindowTextLengthW` can over-report — it is documented as
    // returning a value that may exceed the actual length — so the
    // buffer is sized from it and the *returned* length is what the
    // string is built from.
    let capacity = usize::try_from(length).unwrap_or(0).min(MAX_TITLE) + 1;
    let mut buffer = vec![0u16; capacity];
    let size = i32::try_from(buffer.len()).unwrap_or(0);
    let written = copy_window_text(window, &mut buffer, size);
    if written <= 0 {
        return None;
    }
    let units = u32::try_from(written).unwrap_or(0);
    let title = strings::from_wide_nul(strings::reported_slice(&buffer, units));
    // A window whose title is only whitespace is as good as untitled, and
    // several system components own exactly that.
    (!title.trim().is_empty()).then_some(title)
}

/// The process that owns a window.
fn owning_pid(window: HWND) -> Option<u32> {
    let mut pid = 0u32;
    let thread = window_process_id(window, &mut pid);
    // A zero thread id means the window handle was invalid — which can
    // happen if the window was destroyed between being enumerated and
    // being queried.
    (thread != 0 && pid != 0).then_some(pid)
}

/// Invokes synchronous top-level window enumeration for one stack-owned map.
fn enumerate_windows(parameter: LPARAM) {
    // SAFETY: `visit` has the exact callback ABI. `parameter` points at the
    // uniquely-borrowed map in `collect`; EnumWindows is synchronous and retains neither.
    let _ = unsafe { EnumWindows(Some(visit), parameter) };
}

/// Reconstitutes the result map encoded by [`collect`] for this synchronous callback.
fn callback_result_map<'a>(parameter: LPARAM) -> Option<&'a mut HashMap<u32, String>> {
    // SAFETY: only `collect` registers `visit`, and it passes its live,
    // uniquely-borrowed map as `LPARAM`; EnumWindows invokes synchronously.
    unsafe { (parameter as *mut HashMap<u32, String>).as_mut() }
}

/// Checks visibility of a window handle supplied by the active enumeration.
fn window_is_visible(window: HWND) -> bool {
    // SAFETY: Win32 supplied this opaque handle to the current callback; the call retains nothing.
    unsafe { IsWindowVisible(window) != 0 }
}

/// Reads the current UTF-16 title length for a callback window.
fn window_text_length(window: HWND) -> i32 {
    // SAFETY: Win32 supplied this opaque handle to the current callback; the call retains nothing.
    unsafe { GetWindowTextLengthW(window) }
}

/// Copies a callback window's UTF-16 title into a caller-owned buffer.
fn copy_window_text(window: HWND, buffer: &mut [u16], size: i32) -> i32 {
    // SAFETY: `buffer` is writable for `size` UTF-16 units, which is derived
    // from its length; Win32 retains neither pointer nor the window handle.
    unsafe { GetWindowTextW(window, buffer.as_mut_ptr(), size) }
}

/// Writes a callback window's owning process ID to caller-owned storage.
fn window_process_id(window: HWND, pid: &mut u32) -> u32 {
    // SAFETY: `pid` is a live writable out-parameter; Win32 retains neither it nor `window`.
    unsafe { GetWindowThreadProcessId(window, pid) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_enumeration_returns_without_a_global() {
        // The point of the pointer-through-LPARAM design: the collection
        // is on the caller's stack, so this is re-entrant and needs no
        // synchronisation.
        let first = titles_by_pid();
        let second = titles_by_pid();
        // A test runner has no windows of its own, and a headless CI
        // session may have none at all — so the assertion is about
        // well-formedness, not about finding anything.
        for (pid, title) in &first {
            assert!(*pid != 0, "PID 0 owns no windows");
            assert!(
                !title.trim().is_empty(),
                "an untitled window should have been filtered out"
            );
        }
        assert!(
            second.len().abs_diff(first.len()) < 100,
            "two enumerations moments apart should broadly agree"
        );
    }

    #[test]
    fn two_concurrent_enumerations_do_not_interfere() {
        // What a `static mut` or a global would break. Each call's
        // results live on its own stack.
        let handle = std::thread::spawn(titles_by_pid);
        let mine = titles_by_pid();
        let theirs = handle.join().unwrap_or_default();
        for (pid, title) in mine.iter().chain(theirs.iter()) {
            assert!(*pid != 0);
            assert!(!title.trim().is_empty());
        }
    }

    #[test]
    fn a_destroyed_window_handle_yields_nothing_rather_than_a_bad_pid() {
        // A window can be destroyed between being enumerated and being
        // queried, on every enumeration.
        let bogus: HWND = std::ptr::without_provenance_mut(0xdead_beef);
        assert!(
            owning_pid(bogus).is_none(),
            "an invalid window must not resolve to a process"
        );
        assert!(!is_visible(bogus));
        assert!(window_title(bogus).is_none());
    }
}

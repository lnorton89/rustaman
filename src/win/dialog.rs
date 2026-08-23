// ============================================================================
// Module:       win::dialog
// Description:  Small native dialogs available before the app window exists.
//
// Dependencies: win::strings; windows-sys (MessageBoxW)
// ============================================================================

//! Native dialogs used before egui has a window in which to report an error.

use super::strings;
use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};

/// Shows an ownerless error dialog.
pub fn show_error(message: &str, caption: &str) {
    let message = strings::to_wide(message);
    let caption = strings::to_wide(caption);
    message_box_error(&message, &caption);
}

/// Calls `MessageBoxW` with buffers whose lifetimes are carried by slices.
fn message_box_error(message: &[u16], caption: &[u16]) {
    // SAFETY: both slices are live NUL-terminated UTF-16 buffers created
    // by `show_error` and outlive this synchronous call. A null owner is
    // the documented ownerless-dialog form used before a window exists.
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            message.as_ptr(),
            caption.as_ptr(),
            MB_OK | MB_ICONERROR,
        );
    }
}

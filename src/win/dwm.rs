// ============================================================================
// Module:       win::dwm
// Description:  Desktop Window Manager attributes, every one of them probed at
//               runtime rather than assumed present.
//
// Dependencies: windows-sys (DwmSetWindowAttribute)
// ============================================================================

//! Window-manager decoration, for the builds that have it.
//!
//! Only relevant when the app is running with the system's own title bar
//! rather than its custom chrome. With custom chrome the window is
//! undecorated and none of this applies — which is the default, and the
//! reason this module is small.
//!
//! ## Everything here is probed, nothing is assumed
//!
//! The DWM attributes are the clearest case in the app of a Windows
//! feature that is not present on every Windows:
//!
//! - **`DWMWA_USE_IMMERSIVE_DARK_MODE`** (attribute 20) makes the system
//!   title bar dark. Available from Windows 10 1809 (build 17763), which
//!   is this app's floor — but between 1809 and 1903 the attribute number
//!   was **19**, not 20, and passing the wrong one is silently accepted
//!   and does nothing. [`set_dark_titlebar`] therefore tries 20 and falls
//!   back to 19, which is the only way to cover both.
//! - **`DWMWA_WINDOW_CORNER_PREFERENCE`** (attribute 33) rounds the
//!   window corners. Windows **11** only. On Windows 10 — which is what
//!   this is being built against — the call returns a failure code and
//!   nothing happens, which is exactly the right outcome:
//!   [`set_rounded_corners`] returns `false` and the caller carries on.
//!
//! None of these failures is an error. A window with a light title bar on
//! an older build is a slightly worse-looking window; a window that
//! refused to open because a decoration attribute was unavailable would
//! be a broken app. So every function returns a `bool` that the caller is
//! free to ignore.

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute;

/// `DWMWA_USE_IMMERSIVE_DARK_MODE` as numbered from Windows 10 1903 on.
const DARK_MODE_CURRENT: u32 = 20;

/// The same attribute as numbered in Windows 10 1809 to 1903.
///
/// Passing the wrong number is silently accepted and does nothing, which
/// is why both are tried rather than one being chosen from a version
/// check — a version check would need `RtlGetVersion` and a manifest to
/// be told the truth, which is more machinery than trying twice.
const DARK_MODE_LEGACY: u32 = 19;

/// `DWMWA_WINDOW_CORNER_PREFERENCE`. Windows 11 only.
const CORNER_PREFERENCE: u32 = 33;

/// `DWMWCP_ROUND`: the standard rounded corner.
const CORNER_ROUND: u32 = 2;

/// `DWMWA_BORDER_COLOR`. Windows 11 (build 22000) only.
const BORDER_COLOR: u32 = 34;

/// Asks DWM to draw a window's title bar dark.
///
/// Returns whether the attribute was accepted. See the module docs on why
/// both attribute numbers are tried.
pub fn set_dark_titlebar(window: HWND, dark: bool) -> bool {
    let value: u32 = u32::from(dark);
    set_attribute(window, DARK_MODE_CURRENT, value)
        || set_attribute(window, DARK_MODE_LEGACY, value)
}

/// Asks DWM to round a window's corners.
///
/// Returns whether the attribute was accepted, which on Windows 10 is
/// always `false`. The caller ignores it.
pub fn set_rounded_corners(window: HWND) -> bool {
    set_attribute(window, CORNER_PREFERENCE, CORNER_ROUND)
}

/// Asks DWM to paint the window's outer border in a given colour.
///
/// **Windows 11 only**, and the reason it is worth asking for is the
/// undecorated window rather than the decorated one. With the app's own
/// title bar the window has no frame of its own, so on a dark desktop it
/// ends where its background stops and nothing separates it from the
/// window behind it. The one-pixel border DWM draws is what gives it an
/// edge, and left at the system default that edge is whatever the
/// user's accent colour is — the one part of the window the theme does
/// not reach.
///
/// The colour is passed as `COLORREF`, which is **`0x00BBGGRR`** and not
/// the RGB order every other colour in this app is written in. Getting
/// that backwards produces a plausible-looking border in the wrong hue,
/// which is exactly the kind of thing nobody notices in review.
///
/// This paints a border; it does not remove one. DWM reserves
/// `0xFFFFFFFE` to mean "no border at all", which no colour can reach —
/// so a caller that wants none needs its own function rather than a
/// clever argument to this one.
///
/// Returns whether the attribute was accepted, which on Windows 10 is
/// always `false`.
pub fn set_border_colour(window: HWND, red: u8, green: u8, blue: u8) -> bool {
    set_attribute(window, BORDER_COLOR, colorref(red, green, blue))
}

/// Packs a colour the way `COLORREF` wants it: blue high, red low.
#[must_use]
fn colorref(red: u8, green: u8, blue: u8) -> u32 {
    u32::from(red) | (u32::from(green) << 8) | (u32::from(blue) << 16)
}

/// Sets one DWM attribute to a `u32` value.
///
/// Every attribute this app uses takes a 4-byte value, so one wrapper
/// covers them all.
fn set_attribute(window: HWND, attribute: u32, value: u32) -> bool {
    if window.is_null() {
        return false;
    }
    let size = u32::try_from(std::mem::size_of::<u32>()).unwrap_or(4);
    let status = set_dwm_u32_attribute(window, attribute, &value, size);
    status == 0
}

/// Sets a DWM attribute from a caller-owned four-byte value.
fn set_dwm_u32_attribute(window: HWND, attribute: u32, value: &u32, size: u32) -> i32 {
    // SAFETY: `window` is non-null; `value` is live for exactly `size` bytes
    // and DWM reads it synchronously without retaining the pointer.
    unsafe { DwmSetWindowAttribute(window, attribute, std::ptr::from_ref(value).cast(), size) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_null_window_is_refused_rather_than_passed_to_dwm() {
        let null: HWND = std::ptr::null_mut();
        assert!(!set_dark_titlebar(null, true));
        assert!(!set_rounded_corners(null));
        assert!(!set_border_colour(null, 1, 2, 3));
    }

    #[test]
    fn a_border_colour_is_packed_blue_high_and_red_low() {
        // `COLORREF` is 0x00BBGGRR, which is the reverse of how every
        // other colour in this app is written. A swapped pair produces a
        // border in a plausible wrong hue rather than an error.
        assert_eq!(colorref(0x12, 0x34, 0x56), 0x0056_3412);
        assert_eq!(
            colorref(0xFF, 0x00, 0x00),
            0x0000_00FF,
            "red is the low byte"
        );
        assert_eq!(
            colorref(0x00, 0x00, 0xFF),
            0x00FF_0000,
            "blue is the high one"
        );
    }

    #[test]
    fn an_invalid_window_handle_fails_cleanly() {
        // The app sets these once at startup, and a window can in
        // principle have been destroyed by then.
        let bogus: HWND = std::ptr::without_provenance_mut(0xdead_beef);
        assert!(
            !set_dark_titlebar(bogus, true),
            "an invalid handle must report failure rather than crash"
        );
    }

    #[test]
    fn the_two_dark_mode_attribute_numbers_are_both_known() {
        // Between Windows 10 1809 and 1903 the attribute was 19; from
        // 1903 it is 20. Passing the wrong one is silently accepted and
        // does nothing, so both have to be tried.
        assert_eq!(DARK_MODE_CURRENT, 20);
        assert_eq!(DARK_MODE_LEGACY, 19);
        assert_ne!(DARK_MODE_CURRENT, DARK_MODE_LEGACY);
    }
}

// ============================================================================
// Module:       win::window
// Description:  Keeps the native window inside its monitor's usable area.
//
// Dependencies: windows-sys (monitor and window geometry)
// ============================================================================

//! Native window placement safeguards.
//!
//! `eframe` clamps a restored size against the *largest* attached monitor.
//! On a mixed-DPI or mixed-size setup that still permits a window restored on
//! a smaller monitor to begin outside that monitor. An undecorated window is
//! particularly painful in that state because its title bar and resize edges
//! can both be unreachable. This module uses the monitor's real Win32 work
//! area, including taskbar reservations, to keep the complete window visible.

use windows_sys::Win32::Foundation::{HWND, RECT};
use windows_sys::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetWindowRect, SetWindowPos, SWP_NOACTIVATE, SWP_NOZORDER,
};

/// Moves and, only when necessary, shrinks a native window into its current
/// monitor's usable work area.
///
/// `raw_window` is the numeric value of a live Win32 `HWND`. Invalid or stale
/// handles are refused by the queried APIs and leave the window untouched.
pub fn fit_to_work_area(raw_window: isize) -> bool {
    if raw_window == 0 {
        return false;
    }
    let window = raw_window as HWND;
    let Some(current) = window_rect(window) else {
        return false;
    };
    let Some(work) = monitor_work_area(window) else {
        return false;
    };
    let Some(fitted) = fitted_rect(current, work) else {
        return false;
    };
    if rect_coords(fitted) == rect_coords(current) {
        return true;
    }
    set_window_rect(window, fitted)
}

fn window_rect(window: HWND) -> Option<RECT> {
    let mut rect = RECT::default();
    let ok = read_window_rect(window, &mut rect);
    (ok != 0).then_some(rect)
}

fn monitor_work_area(window: HWND) -> Option<RECT> {
    let monitor = nearest_monitor(window);
    if monitor.is_null() {
        return None;
    }
    let mut info = MONITORINFO {
        cbSize: u32::try_from(std::mem::size_of::<MONITORINFO>()).ok()?,
        ..MONITORINFO::default()
    };
    let ok = read_monitor_info(monitor, &mut info);
    (ok != 0).then_some(info.rcWork)
}

fn set_window_rect(window: HWND, rect: RECT) -> bool {
    let width = rect.right - rect.left;
    let height = rect.bottom - rect.top;
    let ok = move_window_without_z_order(window, rect.left, rect.top, width, height);
    ok != 0
}

/// Reads one native window rectangle into caller-owned storage.
fn read_window_rect(window: HWND, rect: &mut RECT) -> i32 {
    // SAFETY: `rect` is a live writable `RECT`; Win32 writes it synchronously
    // and retains neither pointer. Invalid handles simply return zero.
    unsafe { GetWindowRect(window, rect) }
}

/// Locates the nearest monitor for the supplied opaque window handle.
fn nearest_monitor(window: HWND) -> windows_sys::Win32::Graphics::Gdi::HMONITOR {
    // SAFETY: `window` is used only as an opaque lookup key and no pointer is retained.
    unsafe { MonitorFromWindow(window, MONITOR_DEFAULTTONEAREST) }
}

/// Reads a monitor's work area into caller-owned monitor information.
fn read_monitor_info(
    monitor: windows_sys::Win32::Graphics::Gdi::HMONITOR,
    info: &mut MONITORINFO,
) -> i32 {
    // SAFETY: `info.cbSize` advertises its exact size and `info` is a live
    // writable out-parameter; Win32 fills it synchronously and retains nothing.
    unsafe { GetMonitorInfoW(monitor, info) }
}

/// Moves a valid window while preserving z-order and activation state.
fn move_window_without_z_order(window: HWND, left: i32, top: i32, width: i32, height: i32) -> i32 {
    // SAFETY: `window` was accepted by `GetWindowRect`; dimensions are positive
    // from `fitted_rect`, and the null insertion handle is ignored by `SWP_NOZORDER`.
    unsafe {
        SetWindowPos(
            window,
            std::ptr::null_mut(),
            left,
            top,
            width,
            height,
            SWP_NOACTIVATE | SWP_NOZORDER,
        )
    }
}

/// The contained rectangle that preserves the requested size whenever it can.
fn fitted_rect(window: RECT, work: RECT) -> Option<RECT> {
    let work_width = work.right.checked_sub(work.left)?;
    let work_height = work.bottom.checked_sub(work.top)?;
    let window_width = window.right.checked_sub(window.left)?;
    let window_height = window.bottom.checked_sub(window.top)?;
    if work_width <= 0 || work_height <= 0 || window_width <= 0 || window_height <= 0 {
        return None;
    }

    let width = window_width.min(work_width);
    let height = window_height.min(work_height);
    let left = window.left.clamp(work.left, work.right - width);
    let top = window.top.clamp(work.top, work.bottom - height);
    Some(RECT {
        left,
        top,
        right: left + width,
        bottom: top + height,
    })
}

fn rect_coords(rect: RECT) -> [i32; 4] {
    [rect.left, rect.top, rect.right, rect.bottom]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_oversized_and_offset_window_is_fitted_to_the_work_area() {
        let window = RECT {
            left: -83,
            top: -53,
            right: 1_097,
            bottom: 707,
        };
        let work = RECT {
            left: 0,
            top: 0,
            right: 1_121,
            bottom: 730,
        };
        assert_eq!(
            fitted_rect(window, work).map(rect_coords),
            Some(rect_coords(work))
        );
    }

    #[test]
    fn a_fitting_window_keeps_its_size_and_is_only_repositioned() {
        let window = RECT {
            left: 1_850,
            top: 950,
            right: 2_850,
            bottom: 1_650,
        };
        let work = RECT {
            left: 1_920,
            top: 0,
            right: 3_840,
            bottom: 1_040,
        };
        assert_eq!(
            fitted_rect(window, work).map(rect_coords),
            Some(rect_coords(RECT {
                left: 1_920,
                top: 340,
                right: 2_920,
                bottom: 1_040,
            }))
        );
    }
}

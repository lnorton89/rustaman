// ============================================================================
// Module:       gui
// Description:  The desktop front end's entry point: the window's options, and
//               the decisions made once at startup rather than per frame.
//
// Dependencies: eframe (the window and event loop), anyhow; crate::config
// ============================================================================

//! The desktop front end.
//!
//! [`run`] opens the window and hands control to eframe. Everything after
//! that is [`app::App`] and [`ui::draw`].
//!
//! ## eframe runs on wgpu, not glow
//!
//! The default glow/glutin path goes through WGL, which fails on machines
//! with a stale or hybrid OpenGL ICD — and hybrid graphics is exactly what
//! the laptops this app is most useful on have. wgpu picks D3D12 or Vulkan
//! from what the machine actually exposes, and on Windows 10 D3D12 is
//! always there.
//!
//! ## The window opens undecorated by default
//!
//! Because the Windows 10 system caption is a light grey bar no theme can
//! reach; see [`ui::chrome`]. The decision is made **here, at startup**,
//! and not per frame: `with_decorations` is a `ViewportBuilder` option,
//! and toggling it on a live window on Windows leaves the window without
//! its shadow and sometimes without its taskbar entry. So the Settings
//! switch takes effect on the next start, and says so.

pub mod app;
pub mod icons;
pub mod ui;

use crate::config::Config;
use anyhow::Result;

/// The window's initial size, in logical points.
///
/// Wide enough for the process table's eight columns without a horizontal
/// scrollbar, and tall enough for around thirty rows — the number that
/// makes scrolling feel like navigating rather than hunting.
///
/// **Public because it is a fact other code has to agree with, and did
/// not.** A layout test and the screenshot harness had each written
/// their own idea of the default window down — both as `1440.0`, which
/// is not this number and never was; it is a value out of a config
/// round-trip *test fixture*. So the test protected a budget the app
/// does not have, and every screenshot was taken 260 points wider than
/// the window anyone opens. Take the width from here.
pub const DEFAULT_SIZE: [f32; 2] = [1180.0, 760.0];

/// The smallest the window may be dragged to.
///
/// Below this the navigation rail and the table's first two columns stop
/// fitting side by side, and the window becomes a thing you can resize
/// into uselessness. It is deliberately not smaller: a window that can be
/// made unusable will be, by accident, once.
pub(crate) const MIN_SIZE: [f32; 2] = [780.0, 480.0];

// Relations between constants, checked when the crate is compiled.
const _: () = {
    assert!(
        DEFAULT_SIZE[0] >= 1_000.0,
        "a narrower default opens the process table with a horizontal \
         scrollbar over its eight columns"
    );
    assert!(
        DEFAULT_SIZE[1] >= 700.0,
        "a shorter default shows too few rows to scan"
    );
    assert!(
        MIN_SIZE[0] > ui::theme::NAV_WIDTH * 2.0,
        "at this width the navigation rail would be most of the window — \
         and a window that can be made unusable will be, by accident, once"
    );
    assert!(MIN_SIZE[0] < DEFAULT_SIZE[0] && MIN_SIZE[1] < DEFAULT_SIZE[1]);
};

/// Opens the window and runs until it closes.
pub fn run(config: Config) -> Result<()> {
    let custom_chrome = config.custom_chrome.unwrap_or(true);
    let size = config.window_size.unwrap_or(DEFAULT_SIZE);
    let always_on_top = config.always_on_top.unwrap_or(false);

    let mut viewport = eframe::egui::ViewportBuilder::default()
        .with_inner_size(size)
        .with_min_inner_size(MIN_SIZE)
        .with_title(crate::brand::NAME)
        .with_icon(icons::app_icon())
        .with_decorations(!custom_chrome);
    if always_on_top {
        viewport = viewport.with_window_level(eframe::egui::WindowLevel::AlwaysOnTop);
    }

    let options = eframe::NativeOptions {
        viewport,
        // See the module docs.
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };

    eframe::run_native(
        crate::brand::NAME,
        options,
        Box::new(move |cc| {
            let mut instance = app::App::new(config);
            // The one thing that has to happen after the window exists:
            // asking DWM to darken the system caption, for the users who
            // chose it. A refusal is expected on older builds and costs
            // the dark caption, nothing else. See `crate::win::dwm`.
            if !custom_chrome {
                darken_system_caption(cc, instance.theme.mode);
            }
            // The sampler reports whether it got debug privilege, which
            // the Settings view explains. Read here rather than in
            // `App::new` because it is the sampler thread that asks for
            // it, and asking twice would be a second, pointless
            // privilege adjustment.
            instance.elevated = crate::win::privilege::enable();
            Ok(Box::new(instance))
        }),
    )
    .map_err(|error| anyhow::anyhow!("the window could not be opened: {error}"))
}

/// Asks DWM to draw the system caption dark, for a dark theme.
///
/// Only relevant when the system caption is in use at all. Every failure
/// path is silent by design: a light caption under a dark app is a
/// slightly worse-looking window, and a window that refused to open
/// because a decoration attribute was unavailable would be a broken app.
fn darken_system_caption(cc: &eframe::CreationContext<'_>, mode: crate::theme::Mode) {
    // Re-exported through eframe's wgpu backend rather than depended on
    // directly: taking `raw-window-handle` as a dependency of our own
    // would pin a version that has to agree with the one winit and wgpu
    // already agreed on, and a mismatch there is a trait-not-implemented
    // error with no useful message.
    use eframe::wgpu::rwh::{HasWindowHandle, RawWindowHandle};

    let dark = mode == crate::theme::Mode::Dark;
    let Ok(handle) = cc.window_handle() else {
        return;
    };
    let RawWindowHandle::Win32(window) = handle.as_ref() else {
        return;
    };
    let hwnd = window.hwnd.get() as *mut core::ffi::c_void;
    let _ = crate::win::dwm::set_dark_titlebar(hwnd, dark);
}

#[cfg(test)]
mod tests {
    #[test]
    fn wgpu_is_the_renderer() {
        // The glow/glutin path goes through WGL, which fails on machines
        // with a hybrid or stale OpenGL ICD — which is exactly what the
        // laptops this app is most useful on have.
        let options = eframe::NativeOptions {
            renderer: eframe::Renderer::Wgpu,
            ..Default::default()
        };
        assert_eq!(options.renderer, eframe::Renderer::Wgpu);
    }
}

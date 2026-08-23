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
//! the laptops this app is most useful on have. wgpu talks to D3D12
//! instead, and on Windows 10 D3D12 is always there.
//!
//! ## And it runs on D3D12 specifically — see [`backends`]
//!
//! Letting wgpu choose between D3D12 and Vulkan re-opens the same wound
//! the renderer was picked to close, one vendor along.
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
pub mod font;
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

/// The typeface the window asks for when the config does not name one.
///
/// A *name*, not a font — see [`font`] on why the distinction is the
/// whole design. Nothing is bundled and nothing is downloaded: if this
/// family is installed on the machine the window is set in it, and if
/// it is not, the window opens in egui's bundled face exactly as it did
/// before. Both outcomes are normal and neither is reported.
pub const DEFAULT_FAMILY: &str = "Product Sans";

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

/// The graphics backends the window may be created on.
///
/// **D3D12, and not Vulkan.** eframe's default is
/// `Backends::PRIMARY | Backends::GL`, which on Windows means wgpu
/// enumerates the Vulkan ICD as well and may well pick it — and an
/// installable client driver is vendor code loaded into this process,
/// exactly like the WGL path the renderer was chosen to avoid. Choosing
/// wgpu bought the escape from a broken OpenGL ICD; leaving Vulkan in the
/// set hands the same job to a different vendor's driver and hopes.
///
/// It is not hypothetical. An Intel UHD 620 on the 30.0.101.1122 driver
/// takes an access violation inside `igvk64.dll` while the swapchain is
/// being created — before the first frame, so the app does not fail to
/// draw, it fails to *open*, with no window and no message. A GUI-subsystem
/// binary has no console, so the only thing the user sees is nothing
/// happening. The offscreen tests never caught it because a headless
/// target creates no surface, and the surface is where that driver dies.
///
/// D3D12 costs nothing to prefer here. [`crate::win`] puts the floor at
/// Windows 10 1809, every such machine has D3D12, and where the hardware
/// cannot the runtime falls back to WARP rather than to nothing.
///
/// `WGPU_BACKEND` still overrides, which is the escape hatch in the other
/// direction: a machine whose D3D12 path is the broken one can be started
/// on Vulkan without a rebuild.
fn backends() -> eframe::wgpu::Backends {
    eframe::wgpu::Backends::from_env().unwrap_or(eframe::wgpu::Backends::DX12)
}

/// Opens the window and runs until it closes.
pub fn run(config: Config) -> Result<()> {
    let custom_chrome = config.custom_chrome.unwrap_or(true);
    // Cloned out before `config` is moved into the creation closure.
    let family = config
        .font
        .clone()
        .unwrap_or_else(|| DEFAULT_FAMILY.to_owned());
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

    // Everything else about the wgpu setup is eframe's default; only the
    // backend set is ours. Mutated in place rather than rebuilt because
    // `WgpuSetupCreateNew` also carries the device descriptor and the
    // display handle eframe fills in for winit, and restating those here
    // would silently pin them to whatever they were at the time.
    let mut wgpu_options = eframe::WgpuConfiguration::default();
    if let eframe::egui_wgpu::WgpuSetup::CreateNew(setup) = &mut wgpu_options.wgpu_setup {
        setup.instance_descriptor.backends = backends();
    }

    let options = eframe::NativeOptions {
        viewport,
        // Rustaman persists its own size. eframe's separate native-window
        // persistence can restore a stale position from a detached monitor
        // before the app has a chance to fit it to the current work area.
        persist_window: false,
        // See the module docs.
        renderer: eframe::Renderer::Wgpu,
        wgpu_options,
        ..Default::default()
    };

    eframe::run_native(
        crate::brand::NAME,
        options,
        Box::new(move |cc| {
            // Before the app is built, so the first frame is already
            // in the right face — installing it later would draw one
            // frame in the bundled one and then reflow the whole
            // window, which reads as a fault on startup.
            font::install(&cc.egui_ctx, &family);
            let mut instance = app::App::new(config);
            instance.native_window = native_window_handle(cc);
            // The one thing that has to happen after the window exists:
            // asking DWM to darken the system caption, for the users who
            // chose it. A refusal is expected on older builds and costs
            // the dark caption, nothing else. See `crate::win::dwm`.
            if !custom_chrome {
                darken_system_caption(cc, instance.theme.mode);
            }
            Ok(Box::new(instance))
        }),
    )
    .map_err(|error| anyhow::anyhow!("the window could not be opened: {error}"))
}

/// The numeric Win32 handle used by the bounds safeguard after native moves
/// and resizes. Other backends never reach this Windows-only module.
fn native_window_handle(cc: &eframe::CreationContext<'_>) -> Option<isize> {
    use eframe::wgpu::rwh::{HasWindowHandle, RawWindowHandle};

    let handle = cc.window_handle().ok()?;
    let RawWindowHandle::Win32(window) = handle.as_ref() else {
        return None;
    };
    Some(window.hwnd.get())
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

    #[test]
    fn the_window_is_created_on_d3d12_alone() {
        // `WGPU_BACKEND` is a documented override, so a machine that has
        // set it has asked for something other than the default and the
        // default is not what is under test here.
        if std::env::var_os("WGPU_BACKEND").is_some() {
            return;
        }
        // Not a style preference. Leaving Vulkan in the set lets wgpu
        // pick an ICD that crashes the process during swapchain creation
        // on hardware this app is specifically meant to run on — see
        // `super::backends`.
        assert_eq!(
            super::backends(),
            eframe::wgpu::Backends::DX12,
            "the window must be created on D3D12 alone"
        );
    }
}

// ============================================================================
// Module:       gui::icons
// Description:  Painting the brand mark with egui, and rasterising it for the
//               window icon.
//
// Dependencies: egui; crate::brand (the geometry and its fixed colours)
// ============================================================================

//! The brand mark, drawn.
//!
//! Both functions here consume the one definition in [`crate::brand`] —
//! its bar geometry in unit coordinates and its five fixed colours — so
//! the mark in the title bar, the mark in the taskbar, and the mark in
//! `assets/brand/` cannot drift apart. See that module on why its colours
//! are the one documented exception to the no-literal-colours rule.
//!
//! ## The window icon is rasterised, not loaded
//!
//! [`app_icon`] draws the mark into a pixel buffer at startup rather than
//! decoding a PNG. That avoids shipping an image whose contents nothing
//! checks — an icon file that fell out of step with `brand.rs` would be
//! wrong in the taskbar and right in the title bar, and nothing in the
//! build would notice.
//!
//! It is a few hundred microseconds once, on a 64×64 buffer.

use super::ui::theme;
use crate::brand;
use egui::{CornerRadius, Rect, Sense, Ui, Vec2};

/// The window icon's edge, in pixels.
///
/// 64 is what Windows asks for at typical DPI for the taskbar and the
/// Alt-Tab switcher; it downsamples cleanly to the 32 and 16 the title
/// bar and the tray use. Larger costs startup time for sizes nothing
/// requests at runtime — the `.ico` embedded by `build.rs` carries those.
const ICON_SIZE: u32 = 64;

/// Paints the brand mark at the current cursor, `size` points tall.
///
/// No plate: the title bar draws the mark straight onto the window, so a
/// filled ground would show as a rectangle around it.
pub fn paint_brand(ui: &mut Ui, size: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    let painter = ui.painter();
    let radius = CornerRadius::same(scaled_radius(brand::BAR_RADIUS, size));

    for bar in brand::BARS {
        let width = size * brand::BAR_WIDTH;
        let height = size * bar.height;
        let bar_rect = Rect::from_min_size(
            rect.left_bottom() + Vec2::new(size * bar.x, -height),
            Vec2::new(width, height),
        );
        painter.rect_filled(bar_rect, radius, theme::rgb(bar.color));
    }
}

/// A radius in unit coordinates as an egui corner radius at `size`.
///
/// Clamped rather than cast: `CornerRadius` is `u8` in egui 0.36, and a
/// bare `as u8` on a large size would wrap a rounded corner into a
/// square-ish one at exactly the sizes where the rounding is visible.
fn scaled_radius(unit: f32, size: f32) -> u8 {
    (unit * size).round().clamp(0.0, 255.0) as u8
}

/// The window icon, rasterised from the same definition.
///
/// Includes the plate, because this composites over whatever the taskbar
/// or the switcher puts behind it.
#[must_use]
pub fn app_icon() -> egui::IconData {
    let size = ICON_SIZE as usize;
    let mut pixels = vec![0u8; size * size * 4];

    let edge = ICON_SIZE as f32;
    let plate_radius = brand::PLATE_RADIUS * edge;

    for y in 0..size {
        for x in 0..size {
            let point = (x as f32 + 0.5, y as f32 + 0.5);
            let Some(color) = sample(point, edge, plate_radius) else {
                continue;
            };
            let Some(slot) = pixels.get_mut((y * size + x) * 4..(y * size + x) * 4 + 4) else {
                continue;
            };
            slot.copy_from_slice(&[color.0, color.1, color.2, 255]);
        }
    }

    egui::IconData {
        rgba: pixels,
        width: ICON_SIZE,
        height: ICON_SIZE,
    }
}

/// The colour at one pixel of the icon, or `None` outside the plate.
///
/// Nearest-neighbour, no anti-aliasing. At 64px the plate's corners and
/// the bars' are the only curved edges, and Windows downsamples the icon
/// to the size it actually wants — which anti-aliases them on the way.
/// Doing it here as well would soften edges twice.
fn sample(point: (f32, f32), edge: f32, plate_radius: f32) -> Option<(u8, u8, u8)> {
    let (x, y) = point;
    if !inside_rounded(x, y, 0.0, 0.0, edge, edge, plate_radius) {
        return None;
    }

    // Bars are drawn over the plate, so they are tested last and win.
    // Iterated in reverse so a later bar — which is drawn on top — is
    // found first, though the geometry guarantees they do not overlap.
    for bar in brand::BARS.iter().rev() {
        let left = bar.x * edge;
        let width = brand::BAR_WIDTH * edge;
        let height = bar.height * edge;
        let top = edge - height;
        let radius = brand::BAR_RADIUS * edge;
        if inside_rounded(x, y, left, top, width, height, radius) {
            return Some((bar.color.r, bar.color.g, bar.color.b));
        }
    }
    Some((brand::PLATE.r, brand::PLATE.g, brand::PLATE.b))
}

/// Whether a point is inside a rounded rectangle.
fn inside_rounded(
    x: f32,
    y: f32,
    left: f32,
    top: f32,
    width: f32,
    height: f32,
    radius: f32,
) -> bool {
    if x < left || y < top || x > left + width || y > top + height {
        return false;
    }
    // A radius larger than half the shorter side would make the corner
    // arcs overlap and the test meaningless.
    let radius = radius.min(width / 2.0).min(height / 2.0).max(0.0);
    if radius <= 0.0 {
        return true;
    }

    // The centre of the nearest corner arc, clamped into the inner
    // rectangle: a point in the middle of the shape clamps to itself and
    // trivially passes the distance test.
    let cx = x.clamp(left + radius, left + width - radius);
    let cy = y.clamp(top + radius, top + height - radius);
    let (dx, dy) = (x - cx, y - cy);
    dx * dx + dy * dy <= radius * radius
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_icon_is_the_size_it_claims() {
        let icon = app_icon();
        assert_eq!(icon.width, ICON_SIZE);
        assert_eq!(icon.height, ICON_SIZE);
        assert_eq!(
            icon.rgba.len(),
            (ICON_SIZE * ICON_SIZE * 4) as usize,
            "four bytes per pixel"
        );
    }

    #[test]
    fn the_icons_centre_is_opaque_and_its_corners_are_not() {
        // The plate is rounded, so the extreme corners fall outside it.
        let icon = app_icon();
        let at = |x: u32, y: u32| -> u8 {
            let index = ((y * ICON_SIZE + x) * 4 + 3) as usize;
            icon.rgba.get(index).copied().unwrap_or(0)
        };
        assert_eq!(at(ICON_SIZE / 2, ICON_SIZE / 2), 255, "the centre is drawn");
        assert_eq!(at(0, 0), 0, "the corner is outside the rounded plate");
        assert_eq!(at(ICON_SIZE - 1, ICON_SIZE - 1), 0);
    }

    #[test]
    fn every_brand_colour_appears_in_the_icon() {
        // If a bar were drawn off the edge, or under the plate, this is
        // what would catch it — and it is exactly the failure that looks
        // fine in the title bar and wrong in the taskbar.
        let icon = app_icon();
        for (index, bar) in brand::BARS.iter().enumerate() {
            let found = icon.rgba.as_chunks::<4>().0.iter().any(|pixel| {
                pixel.first() == Some(&bar.color.r)
                    && pixel.get(1) == Some(&bar.color.g)
                    && pixel.get(2) == Some(&bar.color.b)
                    && pixel.get(3) == Some(&255)
            });
            assert!(found, "bar {index} does not appear in the rasterised icon");
        }
    }

    #[test]
    fn the_plate_appears_behind_the_bars() {
        let icon = app_icon();
        let found = icon.rgba.as_chunks::<4>().0.iter().any(|pixel| {
            pixel.first() == Some(&brand::PLATE.r)
                && pixel.get(1) == Some(&brand::PLATE.g)
                && pixel.get(2) == Some(&brand::PLATE.b)
        });
        assert!(found, "the plate should be visible around the bars");
    }

    #[test]
    fn a_point_in_the_middle_of_a_rounded_rect_is_inside_it() {
        assert!(inside_rounded(50.0, 50.0, 0.0, 0.0, 100.0, 100.0, 20.0));
    }

    #[test]
    fn a_rounded_rects_corner_is_outside_it() {
        assert!(
            !inside_rounded(0.5, 0.5, 0.0, 0.0, 100.0, 100.0, 20.0),
            "the extreme corner falls outside the arc"
        );
        assert!(
            inside_rounded(20.0, 20.0, 0.0, 0.0, 100.0, 100.0, 20.0),
            "but the arc's own centre is inside"
        );
    }

    #[test]
    fn a_zero_radius_is_a_plain_rectangle() {
        assert!(inside_rounded(0.5, 0.5, 0.0, 0.0, 100.0, 100.0, 0.0));
        assert!(!inside_rounded(-1.0, 50.0, 0.0, 0.0, 100.0, 100.0, 0.0));
    }

    #[test]
    fn an_oversized_radius_does_not_make_the_test_meaningless() {
        // A radius larger than half the shorter side would make the
        // corner arcs overlap.
        assert!(inside_rounded(50.0, 50.0, 0.0, 0.0, 100.0, 100.0, 500.0));
        assert!(!inside_rounded(1.0, 1.0, 0.0, 0.0, 100.0, 100.0, 500.0));
    }

    #[test]
    fn a_scaled_radius_cannot_wrap() {
        // `CornerRadius` is u8; a bare cast on a large size would wrap a
        // rounded corner into a square-ish one at exactly the sizes where
        // the rounding shows.
        assert_eq!(scaled_radius(0.035, 18.0), 1);
        assert_eq!(scaled_radius(0.035, 512.0), 18);
        assert_eq!(scaled_radius(1.0, 10_000.0), 255, "clamped, not wrapped");
        assert_eq!(scaled_radius(-1.0, 100.0), 0);
    }
}

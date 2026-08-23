// ============================================================================
// Module:       tray
// Description:  The notification-area icon's appearance — how many of the
//               mark's bars are lit for a load, what colour they take, and
//               the pixels that follow from both.
//
// Dependencies: crate::brand (the mark's geometry), crate::color::Rgb,
//               crate::theme::Palette. Deliberately free of Win32: this is
//               arithmetic and pixels, so it is testable off Windows.
// ============================================================================

//! The tray icon, as a picture rather than as a handle.
//!
//! The notification area gets the brand mark doing a second job. The five
//! bars keep the positions and the rising heights they have everywhere
//! else — at 16×16 that silhouette is the only thing left that says which
//! app this is — but instead of the fixed rainbow they light up in
//! sequence, like a signal-strength meter, and the lit ones take the
//! theme's heat colour for the current load.
//!
//! So it reads as the logo at a glance and as a gauge on a second look,
//! which is the whole point of putting it there.
//!
//! ## Why this module is not on the Windows side
//!
//! Everything here is arithmetic on a load and bytes in a buffer. None of
//! it needs a handle, and `src/win/` is where the unsafe lives — so
//! keeping the drawing out of it means the part with the interesting
//! decisions in it (how many bars, which colour, which pixels) is covered
//! by tests that run on every platform, including the CI job that never
//! compiles a line of Win32. `win::tray` is left with the handles.
//!
//! ## Why the colours come from the theme and the plate does not
//!
//! [`crate::brand`] is the documented exception to the no-literal-colours
//! rule, and its five bar colours are fixed because a logo that restyles
//! itself is not a logo. This icon is not the logo: it is a readout that
//! happens to be logo-shaped, and a readout that ignored the theme would
//! be the only part of the app that did. The bars therefore take
//! [`Palette::heat`] — the same success-through-warning-to-danger ramp the
//! process table's busy cells use — while the ground stays
//! [`brand::PLATE`], so the icon sits on the same plate as the taskbar
//! icon two inches away from it.

use crate::brand;
use crate::color::Rgb;
use crate::theme::Palette;

/// How many bars the mark has, and so how many steps the meter has.
///
/// Not a number of its own: it is [`brand::BARS`]' length, because a mark
/// that grew a sixth bar and a meter that still had five would disagree
/// in a way nothing would catch.
pub const BAR_COUNT: usize = brand::BARS.len();

/// How dim an unlit bar is, as a blend from the plate toward the theme's
/// muted text.
///
/// Not zero. A bar that vanished entirely would leave the icon a
/// different *shape* at every load, and the shape is what identifies the
/// app — at 16×16 a one-bar icon and a five-bar icon do not read as the
/// same program. A ghost keeps the silhouette and still loses clearly to
/// a lit bar.
const UNLIT_BLEND: f32 = 0.35;

/// What the icon shows: how many bars are lit, and in what colour.
///
/// Compared rather than recomputed. Rasterising is cheap but
/// `Shell_NotifyIcon` is a system call, and the load moves continuously
/// while this only changes in five steps — so the caller keeps the last
/// one and pushes a new icon when it actually differs. On an idle machine
/// that is approximately never.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Face {
    /// Bars lit, 1..=[`BAR_COUNT`].
    pub lit: usize,
    /// The colour those bars take.
    pub color: Rgb,
    /// The colour the rest take.
    pub unlit: Rgb,
}

/// The face for a load fraction, 0..=1, under a theme.
#[must_use]
pub fn face(load: f32, theme: &Palette) -> Face {
    Face {
        lit: lit_bars(load),
        color: theme.heat(load),
        unlit: brand::PLATE.lerp(theme.text_muted, UNLIT_BLEND),
    }
}

/// How many bars a load fraction lights, 1..=[`BAR_COUNT`].
///
/// Never zero, for the reason [`UNLIT_BLEND`] is not zero and one more:
/// an icon that went blank at idle would look like the app had stopped,
/// and "nothing is happening" is exactly when a monitor most needs to
/// still look alive.
///
/// Rounded up rather than down, so that any load at all lights the first
/// bar and only a genuinely pinned machine lights the last.
#[must_use]
pub fn lit_bars(load: f32) -> usize {
    // A NaN load reaches here from a rate divided by an interval, the
    // same path `Palette::heat` guards. `NaN as usize` is 0 in Rust, so
    // an unguarded NaN would take the clamp's floor rather than saying
    // "unknown" — which is the right answer, but by accident. State it.
    if load.is_nan() {
        return 1;
    }
    let steps = (load.clamp(0.0, 1.0) * BAR_COUNT as f32).ceil();
    // `as usize` saturates at the cast in Rust, and the clamp above
    // bounds the value anyway; the clamp here is what enforces the
    // documented 1..=BAR_COUNT rather than the cast.
    (steps as usize).clamp(1, BAR_COUNT)
}

/// The icon as RGBA bytes, `edge` pixels square, four bytes per pixel.
///
/// Row-major from the top left, which is what every caller of this in the
/// crate already expects — `win::tray` flips and swizzles it into the
/// bottom-up BGRA a DIB section wants, because that is a Windows detail
/// and belongs on the Windows side.
///
/// Nearest-neighbour and no anti-aliasing, matching [`crate::gui::icons`]:
/// the shell downsamples whatever it is given to the size it actually
/// wants and anti-aliases on the way, so doing it here as well softens
/// every edge twice.
#[must_use]
pub fn rasterise(face: Face, edge: usize) -> Vec<u8> {
    let mut pixels = vec![0u8; edge * edge * 4];
    let size = edge as f32;
    let plate_radius = brand::PLATE_RADIUS * size;

    for y in 0..edge {
        for x in 0..edge {
            let point = (x as f32 + 0.5, y as f32 + 0.5);
            let Some(color) = sample(face, point, size, plate_radius) else {
                continue;
            };
            let Some(slot) = pixels.get_mut((y * edge + x) * 4..(y * edge + x) * 4 + 4) else {
                continue;
            };
            slot.copy_from_slice(&[color.r, color.g, color.b, 255]);
        }
    }
    pixels
}

/// The colour at one pixel, or `None` outside the plate.
fn sample(face: Face, point: (f32, f32), edge: f32, plate_radius: f32) -> Option<Rgb> {
    let (x, y) = point;
    if !inside_rounded(x, y, 0.0, 0.0, edge, edge, plate_radius) {
        return None;
    }

    // Reversed so a later bar — drawn on top — is found first. The
    // geometry guarantees they do not overlap, but the ordering is what
    // makes that a fact about the mark rather than a fact about this
    // loop.
    for (index, bar) in brand::BARS.iter().enumerate().rev() {
        let (left, top, width, height) = brand::bar_rect(bar, edge);
        let radius = brand::BAR_RADIUS * edge;
        if inside_rounded(x, y, left, top, width, height, radius) {
            return Some(if index < face.lit {
                face.color
            } else {
                face.unlit
            });
        }
    }
    Some(brand::PLATE)
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
    let cx = x.clamp(left + radius, left + width - radius);
    let cy = y.clamp(top + radius, top + height - radius);
    let (dx, dy) = (x - cx, y - cy);
    dx * dx + dy * dy <= radius * radius
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Catalog;

    /// A palette to measure against. Any theme will do — these tests are
    /// about the meter's arithmetic, not about a particular theme's
    /// colours.
    fn palette() -> Palette {
        Catalog::load().get(None).clone()
    }

    #[test]
    fn an_idle_machine_still_lights_one_bar() {
        assert_eq!(lit_bars(0.0), 1, "a blank icon reads as a stopped app");
    }

    #[test]
    fn a_pinned_machine_lights_them_all() {
        assert_eq!(lit_bars(1.0), BAR_COUNT);
    }

    #[test]
    fn the_meter_never_leaves_its_range() {
        // Including the loads that should be impossible: this arrives as
        // a rate divided by an interval.
        for load in [-1.0, -0.0, 0.0, 0.5, 1.0, 2.0, f32::NAN, f32::INFINITY] {
            let lit = lit_bars(load);
            assert!(
                (1..=BAR_COUNT).contains(&lit),
                "load {load} lit {lit} bars, outside 1..={BAR_COUNT}"
            );
        }
    }

    #[test]
    fn the_meter_rises_with_the_load() {
        let mut previous = 0;
        for step in 0..=20 {
            let lit = lit_bars(step as f32 / 20.0);
            assert!(
                lit >= previous,
                "the meter fell from {previous} to {lit} as the load rose"
            );
            previous = lit;
        }
    }

    #[test]
    fn half_load_lights_the_middle_bar_and_no_more() {
        assert_eq!(lit_bars(0.5), 3, "five bars, half way, is three lit");
        assert_eq!(lit_bars(0.41), 3);
        assert_eq!(lit_bars(0.39), 2);
    }

    #[test]
    fn a_hot_face_is_not_a_cold_one() {
        let theme = palette();
        let cold = face(0.05, &theme);
        let hot = face(0.95, &theme);
        assert_ne!(cold.lit, hot.lit, "the bar count has to move");
        assert_ne!(cold.color, hot.color, "and so does the colour");
        assert_eq!(cold.unlit, hot.unlit, "the ghost does not depend on load");
    }

    #[test]
    fn the_face_only_changes_when_the_icon_would() {
        // The gate the caller relies on to keep Shell_NotifyIcon off a
        // frame where nothing moved.
        let theme = palette();
        assert_eq!(
            face(0.81, &theme),
            face(0.81, &theme),
            "the same load and theme must compare equal, or every frame \
             would push a new icon"
        );
    }

    #[test]
    fn the_icon_is_the_size_it_claims() {
        let pixels = rasterise(face(0.5, &palette()), 16);
        assert_eq!(pixels.len(), 16 * 16 * 4, "four bytes per pixel");
    }

    #[test]
    fn the_centre_is_drawn_and_the_corners_are_not() {
        let edge = 32;
        let pixels = rasterise(face(0.5, &palette()), edge);
        let alpha =
            |x: usize, y: usize| -> u8 { pixels.get((y * edge + x) * 4 + 3).copied().unwrap_or(0) };
        assert_eq!(alpha(edge / 2, edge / 2), 255, "the plate is drawn");
        assert_eq!(alpha(0, 0), 0, "the plate's corners are rounded off");
    }

    #[test]
    fn a_lit_bar_and_an_unlit_bar_are_both_present_at_mid_load() {
        // The meter is only legible if both states appear at once, so
        // count the distinct bar colours actually rasterised.
        let theme = palette();
        let shown = face(0.5, &theme);
        let edge = 64;
        let pixels = rasterise(shown, edge);
        let mut lit_seen = false;
        let mut unlit_seen = false;
        let (chunks, _) = pixels.as_chunks::<4>();
        for chunk in chunks {
            let [r, g, b, a] = *chunk;
            if a == 0 {
                continue;
            }
            let rgb = Rgb::new(r, g, b);
            lit_seen |= rgb == shown.color;
            unlit_seen |= rgb == shown.unlit;
        }
        assert!(lit_seen, "no lit bar was rasterised");
        assert!(unlit_seen, "no unlit bar was rasterised");
    }

    #[test]
    fn every_load_rasterises_without_panicking_at_the_size_the_shell_asks_for() {
        // 16 and 32 are what GetSystemMetrics(SM_CXSMICON) returns at
        // 100% and 200%; 24 is the 150% this was developed on.
        let theme = palette();
        for edge in [16, 20, 24, 32] {
            for step in 0..=10 {
                let pixels = rasterise(face(step as f32 / 10.0, &theme), edge);
                assert_eq!(pixels.len(), edge * edge * 4, "edge {edge}");
            }
        }
    }
}

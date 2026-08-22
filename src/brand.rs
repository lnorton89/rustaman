// ============================================================================
// Module:       brand
// Description:  The application mark's geometry and its fixed colours — the
//               one documented exception to the no-literal-colours rule.
//
// Dependencies: crate::color::Rgb. Deliberately free of egui and of the theme:
//               a logo that restyles itself is not a logo.
// ============================================================================

//! The application mark.
//!
//! Five bars of a bar chart, rising left to right, inside an implied
//! rounded square: a monitor's own icon, which is what this is. The
//! geometry is stated once, here, in unit coordinates, and three
//! different renderers consume it — `gui::icons::paint_brand` draws it
//! into the title bar with egui, `gui::icons::app_icon` rasterises it for
//! the window icon, and `examples/brand_assets.rs` writes the PNGs and
//! the `.ico` that `build.rs` embeds into the executable.
//!
//! ## Why the colours are literals
//!
//! Every other colour in this app comes from the active theme, and
//! `CLAUDE.md` makes that a rule with a test behind it. This module is
//! the single exception, and it is deliberate: a logo that restyles
//! itself under a dark theme is not a logo, it is a decoration. The mark
//! has to be the same five colours in the title bar, in the taskbar, in
//! Explorer's file listing, and on the GitHub page — none of which know
//! what theme is loaded, and two of which are not drawn by this app at
//! all.
//!
//! The colours are a rainbow, which is the same idea the theme ramps are
//! built on — but they are *these* five, fixed, rather than sampled from
//! whatever ramp is in force.

use crate::color::Rgb;

/// One bar of the mark.
#[derive(Clone, Copy, Debug)]
pub struct Bar {
    /// Left edge, as a fraction of the mark's width.
    pub x: f32,
    /// Height, as a fraction of the mark's height.
    pub height: f32,
    /// The bar's colour.
    pub color: Rgb,
}

/// How wide each bar is, as a fraction of the mark's width.
///
/// The layout is five bars of this width, a gap of [`BAR_GAP`] between
/// them, and [`MARK_MARGIN`] of clear space at each edge:
/// `5w + 4g + 2m = 1`. The margin is what keeps the mark from touching
/// the edge of a 16×16 icon and reading as a solid block.
pub const BAR_WIDTH: f32 = 0.14;

/// The gap between adjacent bars, as a fraction of the mark's width.
pub const BAR_GAP: f32 = 0.05;

/// Clear space at each edge of the mark, as a fraction of its width.
pub const MARK_MARGIN: f32 = 0.05;

/// The corner radius of each bar, as a fraction of the mark's width.
///
/// Small enough to survive rasterisation at 16px, where a larger radius
/// eats the whole bar.
pub const BAR_RADIUS: f32 = 0.035;

/// The mark: five bars, rising, in a fixed rainbow.
///
/// Rising rather than arbitrary because a bar chart that goes up reads as
/// "measurement" at any size, including the 16×16 where the colours are
/// all that is left of it.
pub const BARS: [Bar; 5] = [
    Bar {
        x: MARK_MARGIN,
        height: 0.30,
        color: Rgb::new(0x4c, 0xc9, 0xf0),
    },
    Bar {
        x: MARK_MARGIN + (BAR_WIDTH + BAR_GAP),
        height: 0.46,
        color: Rgb::new(0x4c, 0xe0, 0xb5),
    },
    Bar {
        x: MARK_MARGIN + 2.0 * (BAR_WIDTH + BAR_GAP),
        height: 0.62,
        color: Rgb::new(0xf7, 0xd0, 0x5c),
    },
    Bar {
        x: MARK_MARGIN + 3.0 * (BAR_WIDTH + BAR_GAP),
        height: 0.78,
        color: Rgb::new(0xff, 0x91, 0x4d),
    },
    Bar {
        x: MARK_MARGIN + 4.0 * (BAR_WIDTH + BAR_GAP),
        height: 0.94,
        color: Rgb::new(0xf7, 0x5c, 0x8d),
    },
];

/// The plate the bars sit on, for renderers that need a filled ground —
/// the `.ico` and the taskbar icon, which composite over whatever is
/// behind them.
///
/// The title bar does not use this: there the mark is drawn straight onto
/// the window, so a plate would be a visible rectangle around it.
pub const PLATE: Rgb = Rgb::new(0x11, 0x14, 0x1c);

/// The plate's corner radius, as a fraction of the mark's width.
pub const PLATE_RADIUS: f32 = 0.22;

/// The product name, as it appears in the title bar and the about page.
pub const NAME: &str = "Rustaman";

/// The one-line description, for the about page and the window title.
pub const TAGLINE: &str = "A modern Windows task manager";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_layout_fills_the_mark_exactly() {
        // `5w + 4g + 2m = 1`. Stated as an assertion because the three
        // constants have to agree, and nothing else would notice if a
        // later tweak to one of them left a lopsided margin.
        let total = 5.0 * BAR_WIDTH + 4.0 * BAR_GAP + 2.0 * MARK_MARGIN;
        assert!(
            (total - 1.0).abs() < 1e-5,
            "the bar layout covers {total} of the mark's width, not 1.0"
        );
    }

    #[test]
    fn every_bar_fits_inside_the_mark() {
        // A bar that runs off the edge is clipped by whatever is
        // rasterising it, which shows up as one bar of the logo being
        // narrower than the rest at icon sizes.
        for (index, bar) in BARS.iter().enumerate() {
            assert!(
                bar.x >= 0.0 && bar.x + BAR_WIDTH <= 1.0,
                "bar {index} spans {}..{} and does not fit",
                bar.x,
                bar.x + BAR_WIDTH
            );
            assert!(
                bar.height > 0.0 && bar.height <= 1.0,
                "bar {index} has an out-of-range height of {}",
                bar.height
            );
        }
    }

    #[test]
    fn the_bars_do_not_overlap_and_stay_in_order() {
        for pair in BARS.windows(2) {
            let (Some(left), Some(right)) = (pair.first(), pair.last()) else {
                continue;
            };
            assert!(
                left.x + BAR_WIDTH <= right.x,
                "bars at {} and {} overlap, so the gap between them closes \
                 and the mark reads as a solid block",
                left.x,
                right.x
            );
        }
    }

    #[test]
    fn the_bars_rise() {
        // What makes the mark read as "measurement" rather than as an
        // arbitrary pattern, at every size.
        for pair in BARS.windows(2) {
            let (Some(left), Some(right)) = (pair.first(), pair.last()) else {
                continue;
            };
            assert!(
                right.height > left.height,
                "the bar chart must rise: {} then {}",
                left.height,
                right.height
            );
        }
    }

    #[test]
    fn every_bar_is_visible_against_the_plate() {
        // The icon composites over whatever is behind it, so the plate is
        // the only background the mark controls.
        for (index, bar) in BARS.iter().enumerate() {
            let ratio = bar.color.contrast(PLATE);
            assert!(
                ratio >= 3.0,
                "bar {index} is {ratio:.2}:1 against the plate and would \
                 vanish in the taskbar"
            );
        }
    }

    #[test]
    fn adjacent_bars_are_tellable_apart() {
        for (index, pair) in BARS.windows(2).enumerate() {
            let (Some(left), Some(right)) = (pair.first(), pair.last()) else {
                continue;
            };
            // Widened: three u8 differences can sum past 255.
            let delta = u32::from(left.color.r.abs_diff(right.color.r))
                + u32::from(left.color.g.abs_diff(right.color.g))
                + u32::from(left.color.b.abs_diff(right.color.b));
            assert!(
                delta >= 60,
                "bars {index} and {} are too close in colour ({delta}) to \
                 read as separate at 16px",
                index + 1
            );
        }
    }
}

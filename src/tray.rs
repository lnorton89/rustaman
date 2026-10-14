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
//! The notification area and the taskbar button get the brand mark doing
//! a second job. The five bars keep the positions, the rising heights and
//! the colours they have everywhere else — at 16×16 that silhouette is the
//! only thing left that says which app this is — but they fill in
//! sequence with the load, like a level meter. At full load the icon *is*
//! the logo; below it, the bars past the load are a dim tint of their own
//! colours, so the rainbow outline is always there.
//!
//! So it reads as the logo at a glance and as a gauge on a second look,
//! which is the whole point of putting it there.
//!
//! ## Why the bars keep the mark's colours
//!
//! The first version lit them in the theme's heat colour, every lit bar
//! the same flat green-to-red, with the rest a grey slab. It read as a
//! generic signal-strength glyph that happened to sit beside a logo — and
//! the heat it was there to show was already in the mark: its bars run
//! cyan, green, yellow, orange, pink, cool to hot, left to right. Filling
//! them in order says "hotter" by the colour the meter reaches, and the
//! icon stays the brand's rather than becoming a second, competing mark.
//! That makes this, like [`crate::brand`], an exception to the
//! colours-come-from-the-theme rule, for the same reason: a logo that
//! restyles itself is not a logo.
//!
//! ## Why it is drawn to the pixel grid
//!
//! The shell is handed an icon at exactly the size it asked for, so
//! nothing downstream rescales it and nothing re-softens it. That makes
//! two things right here that would be wrong in an icon the shell
//! resamples. The bars are snapped to whole pixels — at 16×16 a bar is
//! two pixels wide, and an edge that fell between pixels would smear
//! every bar into its gap. And the curves are anti-aliased: the plate's
//! rounded corners and the bars' rounded ends are sampled sixteen times a
//! pixel, because drawn nearest-neighbour they are visible staircases at
//! every size.
//!
//! ## Why this module is not on the Windows side
//!
//! Everything here is arithmetic on a load and bytes in a buffer. None of
//! it needs a handle, and `src/win/` is where the unsafe lives — so
//! keeping the drawing out of it means the part with the interesting
//! decisions in it (how far the meter reaches, which pixels follow) is
//! covered by tests that run on every platform, including the CI job that
//! never compiles a line of Win32. `win::tray` is left with the handles.

use crate::brand;
use crate::color::Rgb;

/// How many bars the mark has.
///
/// Not a number of its own: it is [`brand::BARS`]' length, because a mark
/// that grew a sixth bar and a meter that still had five would disagree
/// in a way nothing would catch.
pub const BAR_COUNT: usize = brand::BARS.len();

/// How many steps each bar fills in.
///
/// Quarters: enough that the meter moves with the load rather than in
/// five jumps — 41% and 59% were the same picture — and few enough that
/// a fill boundary always lands on a whole pixel row at 16×16, where a
/// bar is at most eleven rows tall.
pub const STEPS_PER_BAR: usize = 4;

/// The meter's full scale, in steps.
const LEVELS: usize = BAR_COUNT * STEPS_PER_BAR;

/// How much of its own colour an unfilled bar keeps, blended into the
/// plate.
///
/// Not zero. A bar that vanished entirely would leave the icon a
/// different *shape* at every load, and the shape is what identifies the
/// app. A tint keeps the silhouette, and keeps it the mark's rainbow
/// rather than grey, while losing clearly to a filled bar.
const UNLIT_TINT: f32 = 0.28;

/// How much lighter a filled bar is at its top than at its foot.
///
/// A gradient you notice at 32px and cannot see at 16, which is the
/// right way round: it is depth for the sizes that have room for it.
const BAR_SHEEN: f32 = 0.2;

/// How much lighter the plate is at its top edge than at its bottom.
const PLATE_SHEEN: f32 = 0.07;

/// How much lighter the plate's one-pixel rim is than the plate, at the
/// top; it fades to half of this at the foot.
///
/// The plate is near-black, and so is a dark taskbar: without an edge of
/// its own the icon's outline dissolves into what is behind it and the
/// bars float. A rim lighter than both gives it a silhouette on any
/// ground, and a light one costs nothing on a light taskbar.
const RIM_SHEEN: f32 = 0.16;

/// The smallest icon that gets a rim. At 16×16 a pixel of rim is an
/// eighth of the room between the plate's edge and the bars, and the
/// bars need it more.
const RIM_FROM: f32 = 20.0;

/// Samples per pixel along each axis when anti-aliasing.
const SUPERSAMPLE: usize = 4;

/// What the icon shows: how far the meter reaches, in steps.
///
/// Compared rather than recomputed. Rasterising is cheap but pushing an
/// icon to the shell is a system call, and the load moves continuously
/// while this changes in [`LEVELS`] steps — so the caller keeps the last
/// one and pushes a new icon when it actually differs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Face {
    /// Steps filled, from 2 to [`BAR_COUNT`] × [`STEPS_PER_BAR`].
    pub level: usize,
}

impl Face {
    /// Bars with any fill at all, 1..=[`BAR_COUNT`].
    #[must_use]
    pub const fn lit(self) -> usize {
        self.level.div_ceil(STEPS_PER_BAR)
    }

    /// How much of bar `index` is filled, 0..=1.
    fn fill(self, index: usize) -> f32 {
        let before = index * STEPS_PER_BAR;
        let steps = self.level.saturating_sub(before).min(STEPS_PER_BAR);
        steps as f32 / STEPS_PER_BAR as f32
    }
}

/// The face for a load fraction, 0..=1.
#[must_use]
pub fn face(load: f32) -> Face {
    Face { level: level(load) }
}

/// How many steps a load fraction fills.
///
/// Never less than half of the first bar, for the reason [`UNLIT_TINT`]
/// is not zero and one more: an icon that went blank at idle would look
/// like the app had stopped, and "nothing is happening" is exactly when
/// a monitor most needs to still look alive.
///
/// Rounded up rather than down, so that any load at all moves the meter
/// and only a genuinely pinned machine fills the last bar.
fn level(load: f32) -> usize {
    /// Half a bar: visibly present, visibly not "one bar's worth".
    const FLOOR: usize = STEPS_PER_BAR / 2;
    // A NaN load reaches here from a rate divided by an interval, the
    // same path `Palette::heat` guards. `NaN as usize` is 0 in Rust, so
    // an unguarded NaN would take the clamp's floor rather than saying
    // "unknown" — which is the right answer, but by accident. State it.
    if load.is_nan() {
        return FLOOR;
    }
    let steps = (load.clamp(0.0, 1.0) * LEVELS as f32).ceil();
    // `as usize` saturates at the cast in Rust, and the clamp above
    // bounds the value anyway; the clamp here is what enforces the
    // documented range rather than the cast.
    (steps as usize).clamp(FLOOR, LEVELS)
}

/// How many bars a load fraction lights, 1..=[`BAR_COUNT`].
#[must_use]
pub fn lit_bars(load: f32) -> usize {
    face(load).lit()
}

/// One bar, snapped to whole pixels: `(left, top, width, height)`.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Slot {
    left: f32,
    top: f32,
    width: f32,
    height: f32,
}

/// The five bars at `edge` pixels, each on whole pixel boundaries.
///
/// [`brand::bar_rect`] is the mark's geometry in fractions, and at 16×16
/// its bar is 2.24 pixels wide. Rasterised as it stands, every bar edge
/// falls inside a pixel and anti-aliases into a half-tone column that
/// joins it to its neighbour. So the proportions are kept and the numbers
/// rounded: the width and gap to whole pixels, never below two and one,
/// and the bars shrunk — gap first — until the row fits inside a margin
/// of at least one pixel, then centred.
fn slots(edge: usize) -> [Slot; BAR_COUNT] {
    let size = edge as f32;
    let margin = (brand::MARK_MARGIN * size).round().max(1.0);
    let mut width = (brand::BAR_WIDTH * size).round().max(2.0);
    let mut gap = (brand::BAR_GAP * size).round().max(1.0);
    let room = size - 2.0 * margin;
    let span = |width: f32, gap: f32| BAR_COUNT as f32 * width + (BAR_COUNT - 1) as f32 * gap;
    while span(width, gap) > room && (gap > 1.0 || width > 1.0) {
        if gap > 1.0 {
            gap -= 1.0;
        } else {
            width -= 1.0;
        }
    }
    let first = ((size - span(width, gap)) / 2.0).floor();

    let baseline = (brand::BASELINE * size).round();
    let bottom = size - baseline;
    let usable = size - 2.0 * baseline;
    std::array::from_fn(|index| {
        let height = brand::BARS
            .get(index)
            .map_or(0.0, |bar| (bar.height * usable).round().max(1.0));
        Slot {
            left: first + index as f32 * (width + gap),
            top: bottom - height,
            width,
            height,
        }
    })
}

/// The icon as RGBA bytes, `edge` pixels square, four bytes per pixel.
///
/// Row-major from the top left, which is what every caller of this in the
/// crate already expects — `win::tray` flips and swizzles it into the
/// bottom-up BGRA a DIB section wants, because that is a Windows detail
/// and belongs on the Windows side.
///
/// Straight alpha, not premultiplied: the colour of a partly covered
/// pixel is the average of the samples that landed on the icon, and its
/// alpha is how many did. That is what a 32-bit icon's DIB wants.
#[must_use]
pub fn rasterise(face: Face, edge: usize) -> Vec<u8> {
    let mut pixels = vec![0u8; edge * edge * 4];
    let size = edge as f32;
    let plate_radius = brand::PLATE_RADIUS * size;
    let bar_radius = brand::BAR_RADIUS * size;
    let slots = slots(edge);
    let samples = (SUPERSAMPLE * SUPERSAMPLE) as f32;

    for y in 0..edge {
        for x in 0..edge {
            let mut sum = [0.0f32; 3];
            let mut covered = 0.0f32;
            for sy in 0..SUPERSAMPLE {
                for sx in 0..SUPERSAMPLE {
                    let point = (
                        x as f32 + (sx as f32 + 0.5) / SUPERSAMPLE as f32,
                        y as f32 + (sy as f32 + 0.5) / SUPERSAMPLE as f32,
                    );
                    let Some(color) = sample(face, point, size, plate_radius, bar_radius, &slots)
                    else {
                        continue;
                    };
                    sum[0] += f32::from(color.r);
                    sum[1] += f32::from(color.g);
                    sum[2] += f32::from(color.b);
                    covered += 1.0;
                }
            }
            if covered == 0.0 {
                continue;
            }
            let Some(slot) = pixels.get_mut((y * edge + x) * 4..(y * edge + x) * 4 + 4) else {
                continue;
            };
            let channel = |total: f32| (total / covered).round().clamp(0.0, 255.0) as u8;
            slot.copy_from_slice(&[
                channel(sum[0]),
                channel(sum[1]),
                channel(sum[2]),
                (covered / samples * 255.0).round() as u8,
            ]);
        }
    }
    pixels
}

/// The colour at one sample point, or `None` outside the plate.
fn sample(
    face: Face,
    point: (f32, f32),
    edge: f32,
    plate_radius: f32,
    bar_radius: f32,
    slots: &[Slot; BAR_COUNT],
) -> Option<Rgb> {
    /// White, for the sheen to lighten toward. The mark's own geometry
    /// constant rather than a theme colour: the plate is the brand's,
    /// not the theme's.
    const WHITE: Rgb = Rgb::new(0xff, 0xff, 0xff);

    let (x, y) = point;
    if !inside_rounded(x, y, 0.0, 0.0, edge, edge, plate_radius) {
        return None;
    }

    for (index, (bar, slot)) in brand::BARS.iter().zip(slots).enumerate() {
        if !inside_rounded(
            x,
            y,
            slot.left,
            slot.top,
            slot.width,
            slot.height,
            bar_radius,
        ) {
            continue;
        }
        // Filled from the foot up, to a whole pixel row: a fill line
        // inside a pixel would anti-alias into a third colour across the
        // bar.
        let filled = (slot.height * face.fill(index)).round();
        if y < slot.top + slot.height - filled {
            return Some(brand::PLATE.lerp(bar.color, UNLIT_TINT));
        }
        let rise = (y - slot.top) / slot.height;
        return Some(bar.color.lerp(WHITE, BAR_SHEEN * (1.0 - rise)));
    }
    let fall = 1.0 - y / edge;
    if edge >= RIM_FROM
        && !inside_rounded(
            x,
            y,
            1.0,
            1.0,
            edge - 2.0,
            edge - 2.0,
            (plate_radius - 1.0).max(0.0),
        )
    {
        return Some(brand::PLATE.lerp(WHITE, RIM_SHEEN * (0.5 + 0.5 * fall)));
    }
    Some(brand::PLATE.lerp(WHITE, PLATE_SHEEN * fall))
}

/// Whether a point falls inside a rounded rectangle.
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

    /// The pixel at `(x, y)` of an `edge`-square raster, as RGBA.
    fn pixel(pixels: &[u8], edge: usize, x: usize, y: usize) -> [u8; 4] {
        let at = (y * edge + x) * 4;
        pixels
            .get(at..at + 4)
            .and_then(|slot| <[u8; 4]>::try_from(slot).ok())
            .unwrap_or_default()
    }

    #[test]
    fn an_idle_machine_still_lights_part_of_a_bar() {
        assert_eq!(lit_bars(0.0), 1, "a blank icon reads as a stopped app");
        assert!(face(0.0).level > 0, "and that bar is not empty");
    }

    #[test]
    fn a_pinned_machine_fills_them_all() {
        assert_eq!(lit_bars(1.0), BAR_COUNT);
        assert_eq!(face(1.0).level, BAR_COUNT * STEPS_PER_BAR);
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
        for step in 0..=100 {
            let level = face(step as f32 / 100.0).level;
            assert!(
                level >= previous,
                "the meter fell from {previous} to {level} as the load rose"
            );
            previous = level;
        }
    }

    #[test]
    fn half_load_reaches_into_the_middle_bar_and_no_further() {
        assert_eq!(lit_bars(0.5), 3, "five bars, half way, is into the third");
        assert_eq!(lit_bars(0.41), 3);
        assert_eq!(lit_bars(0.39), 2);
    }

    #[test]
    fn the_meter_moves_between_whole_bars() {
        // The point of the partial fill: 41% and 59% were one picture
        // when the meter only had five steps.
        assert_ne!(face(0.41), face(0.59));
        assert_eq!(
            lit_bars(0.41),
            lit_bars(0.59),
            "the same bar, filled further"
        );
    }

    #[test]
    fn the_face_only_changes_when_the_icon_would() {
        // The gate the caller relies on to keep the shell off a frame
        // where nothing moved.
        assert_eq!(
            face(0.81),
            face(0.81),
            "the same load must compare equal, or every frame would push a \
             new icon"
        );
        assert_eq!(
            face(0.801),
            face(0.8049),
            "and loads inside one step must not tell the shell anything new"
        );
    }

    #[test]
    fn the_icon_is_the_size_it_claims() {
        let pixels = rasterise(face(0.5), 16);
        assert_eq!(pixels.len(), 16 * 16 * 4, "four bytes per pixel");
    }

    #[test]
    fn the_centre_is_drawn_and_the_corners_are_not() {
        let edge = 32;
        let pixels = rasterise(face(0.5), edge);
        assert_eq!(
            pixel(&pixels, edge, edge / 2, edge / 2)[3],
            255,
            "the plate is drawn"
        );
        assert_eq!(
            pixel(&pixels, edge, 0, 0)[3],
            0,
            "the plate's corners are rounded off"
        );
    }

    #[test]
    fn the_plates_corners_are_smoothed_rather_than_stepped() {
        // Anti-aliased: somewhere along the curve there is a pixel the
        // plate only partly covers. Nearest-neighbour, every pixel is
        // either all plate or none.
        let edge = 32;
        let pixels = rasterise(face(0.5), edge);
        let partial = (0..edge / 4)
            .flat_map(|y| (0..edge / 4).map(move |x| (x, y)))
            .any(|(x, y)| {
                let alpha = pixel(&pixels, edge, x, y)[3];
                alpha > 0 && alpha < 255
            });
        assert!(partial, "no partly covered pixel anywhere in the corner");
    }

    #[test]
    fn every_bar_sits_on_whole_pixels() {
        // At 16×16 a bar is two pixels wide; an edge between pixels
        // smears it into its gap.
        for edge in [16, 20, 24, 32, 40, 48, 64] {
            for slot in slots(edge) {
                for value in [slot.left, slot.top, slot.width, slot.height] {
                    assert_eq!(value, value.round(), "edge {edge}: {slot:?}");
                }
                assert!(slot.width >= 1.0, "edge {edge}: a bar vanished");
            }
        }
    }

    #[test]
    fn the_bars_fit_inside_the_plate_with_room_to_spare() {
        for edge in [16, 20, 24, 32, 40, 48, 64] {
            let row = slots(edge);
            let (Some(first), Some(last)) = (row.first(), row.last()) else {
                continue;
            };
            assert!(
                first.left >= 1.0,
                "edge {edge}: the first bar touches the edge"
            );
            assert!(
                last.left + last.width <= edge as f32 - 1.0,
                "edge {edge}: the last bar touches the edge"
            );
            for pair in row.windows(2) {
                if let [left, right] = pair {
                    assert!(
                        left.left + left.width < right.left,
                        "edge {edge}: two bars run together"
                    );
                }
            }
        }
    }

    #[test]
    fn a_full_icon_shows_every_bar_in_its_own_colour() {
        // At full load the icon is the logo: each bar's foot is the
        // mark's colour for that bar, not a shared heat colour.
        let edge = 64;
        let pixels = rasterise(face(1.0), edge);
        for (slot, bar) in slots(edge).iter().zip(brand::BARS.iter()) {
            let x = (slot.left + slot.width / 2.0) as usize;
            let y = (slot.top + slot.height - 2.0) as usize;
            let [r, g, b, _] = pixel(&pixels, edge, x, y);
            let foot = Rgb::new(r, g, b);
            assert!(
                (i32::from(r) - i32::from(bar.color.r)).abs() < 24
                    && (i32::from(g) - i32::from(bar.color.g)).abs() < 24
                    && (i32::from(b) - i32::from(bar.color.b)).abs() < 24,
                "a filled bar's foot is {foot:?}, not its mark colour {:?}",
                bar.color
            );
        }
    }

    #[test]
    fn an_unfilled_bar_is_dimmer_than_a_filled_one() {
        let edge = 64;
        let idle = rasterise(face(0.0), edge);
        let full = rasterise(face(1.0), edge);
        let Some(last) = slots(edge).last().copied() else {
            return;
        };
        let x = (last.left + last.width / 2.0) as usize;
        let y = (last.top + last.height / 2.0) as usize;
        let brightness = |p: [u8; 4]| u32::from(p[0]) + u32::from(p[1]) + u32::from(p[2]);
        assert!(
            brightness(pixel(&idle, edge, x, y)) * 2 < brightness(pixel(&full, edge, x, y)),
            "the last bar at idle has to read as off"
        );
    }

    #[test]
    fn every_load_rasterises_at_every_size_the_shell_asks_for() {
        // 16, 24 and 32 are what GetSystemMetrics(SM_CXSMICON) returns at
        // 100%, 150% and 200%. The taskbar button draws the same face at
        // SM_CXICON — 32 at 100%, then 40, 48 and 64 as the scaling rises.
        for edge in [16, 20, 24, 32, 40, 48, 64] {
            for step in 0..=20 {
                let pixels = rasterise(face(step as f32 / 20.0), edge);
                assert_eq!(pixels.len(), edge * edge * 4, "edge {edge}");
            }
        }
    }
}

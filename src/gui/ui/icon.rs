// ============================================================================
// Module:       gui::ui::icon
// Description:  Painting crate::icon's geometry with egui, and the three sizes
//               the app draws an icon at.
//
// Dependencies: egui (Painter, Shape); crate::icon; super::theme
// ============================================================================

//! The icon set, painted.
//!
//! [`crate::icon`] owns the shapes — a list of polylines on a 16x16 grid,
//! with no egui in it and tests that run anywhere. This module maps them
//! onto a rect and strokes them.
//!
//! ## Strokes, not fills
//!
//! Every icon is drawn as a stroked path rather than a filled one, and
//! the strokes are round-capped and round-joined. Two reasons:
//!
//! - A stroke takes the theme's colour as one argument and scales with
//!   the icon, so an icon at 12px in a table row and the same icon at
//!   17px in the nav rail carry the same visual weight without either
//!   being tuned by hand.
//! - At 16px a mitre joint on a chevron renders as a spike one pixel
//!   long. It reads as a rendering fault rather than as a design, and it
//!   is the kind of thing that is obvious in the shipped window and
//!   invisible in the source.

use super::theme;
use crate::icon::{Icon, GRID, WEIGHT};
use egui::{Color32, Painter, Pos2, Rect, Shape, Stroke, Vec2};

/// Paints `icon` inside `rect`, in `color`.
///
/// The icon is centred and drawn on the largest square that fits, so a
/// caller may hand over a non-square rect — a table cell, a nav row's
/// icon column — without the shape distorting.
pub fn paint(painter: &Painter, rect: Rect, icon: Icon, color: Color32) {
    paint_rotated(painter, rect, icon, color, 0.0);
}

/// [`paint`], turned `turns` of a full rotation clockwise about the
/// icon's centre.
///
/// The reason this exists rather than a second icon: a disclosure
/// chevron that *rotates* between closed and open reads as the same
/// object turning, where two chevrons swapped at the halfway point read
/// as one being replaced by another. That difference is most of what
/// separates a tree that feels like it is opening from one that feels
/// like it is redrawing, and it costs a rotation matrix.
pub fn paint_rotated(painter: &Painter, rect: Rect, icon: Icon, color: Color32, turns: f32) {
    let side = rect.width().min(rect.height());
    if side <= 0.0 {
        return;
    }
    let scale = side / GRID;
    let origin = rect.center() - Vec2::splat(side / 2.0);
    let stroke = Stroke::new((WEIGHT * scale).max(1.0), color);

    // Rotation happens in grid space, about the grid's own centre, so an
    // icon that is optically centred stays centred as it turns — which
    // `crate::icon`'s own tests are what guarantee.
    let turns = if turns.is_finite() { turns } else { 0.0 };
    let (sin, cos) = (turns * std::f32::consts::TAU).sin_cos();
    let half = GRID / 2.0;
    let place = |(x, y): (f32, f32)| {
        let (dx, dy) = (x - half, y - half);
        let (rx, ry) = (dx * cos - dy * sin + half, dx * sin + dy * cos + half);
        Pos2::new(origin.x + rx * scale, origin.y + ry * scale)
    };

    for path in icon.strokes() {
        let mut points: Vec<Pos2> = path.points.iter().copied().map(place).collect();
        if path.filled {
            // Filled shapes are convex here — the only one is a
            // rectangle — and `convex_polygon` is the cheap path for
            // that. It takes its own stroke, which is `NONE`: a filled
            // shape that is also outlined is a shape drawn half a stroke
            // larger than the grid says.
            painter.add(egui::Shape::convex_polygon(points, color, Stroke::NONE));
            continue;
        }
        if path.closed {
            if let Some(&first) = points.first() {
                points.push(first);
            }
        }
        // `line` rather than `line_segment` per pair: egui joins the
        // segments of one polyline, and a chevron drawn as two
        // independent segments has a notch at its apex where the two
        // round caps meet at slightly different angles.
        painter.add(Shape::line(points, stroke));
    }
}

/// Allocates a square of `size` in the current layout and paints `icon`
/// into it.
///
/// The common case: an icon that participates in a horizontal layout
/// rather than being placed at a computed rect.
pub fn show(ui: &mut egui::Ui, icon: Icon, size: f32, color: Color32) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), egui::Sense::hover());
    paint(ui.painter(), rect, icon, color);
    response
}

/// The size an icon is drawn at beside body text.
///
/// Slightly smaller than the text's own line height: an icon drawn at the
/// full line height optically outweighs the label next to it, because a
/// glyph's ink covers much less of its box than a stroked shape covers of
/// its own.
pub const INLINE: f32 = 15.0;

/// The size for a primary destination — a nav rail entry.
pub const NAV: f32 = 17.0;

/// The size for a disclosure arrow inside a dense table row.
///
/// Small enough that a column of three hundred of them reads as texture
/// rather than as three hundred pieces of punctuation demanding
/// attention.
pub const DISCLOSURE: f32 = 12.0;

// The icon sizes have to stay inside the row they are drawn in, and these
// are relations between constants rather than facts about a running
// program.
const _: () = {
    assert!(
        NAV >= INLINE,
        "a nav icon smaller than an inline one inverts the hierarchy"
    );
    assert!(
        DISCLOSURE < INLINE,
        "a disclosure arrow the size of an inline icon competes with the \
         row's own content"
    );
    assert!(
        INLINE < theme::ROW_HEIGHT,
        "an icon taller than the row it is drawn in is clipped by it"
    );
};

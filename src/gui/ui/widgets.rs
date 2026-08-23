// ============================================================================
// Module:       gui::ui::widgets
// Description:  The shared vocabulary every view is built from — nav items,
//               chips, sortable headers, stat tiles, meters, and the hover ramp.
//
// Dependencies: egui; super::theme
// ============================================================================

//! The widgets every view shares.
//!
//! Building each view out of raw `ui.label` and `ui.button` calls is what
//! produces an app where the Processes page's chips are two pixels taller
//! than the Services page's and nobody can say why. Everything with a
//! repeated shape is a function here.
//!
//! ## Every hover fades; nothing switches
//!
//! Hoverable surfaces route their highlight through [`hover_fill`], which
//! is one eased ramp over [`crate::motion::INSTANT`] for the whole
//! window. One control that snaps beside one that fades is most of what
//! reads as unfinished, and it is very hard to notice deliberately — you
//! only register that the app feels cheap.
//!
//! Note that the *first* observation of an egui animation returns its
//! target rather than its start, which is what stops a control that
//! appears already-selected from sliding into place on the frame it first
//! draws.
//!
//! ## Icons are geometry
//!
//! Nothing here sets an icon in a font. Every one is drawn from
//! [`crate::icon`], because the characters that would do the job —
//! chevrons, a gear, the window-control marks — are not in egui's bundled
//! fonts and rendered as empty boxes in the shipped window.
//!
//! ## Measure, then allocate, then paint
//!
//! An `egui::Frame` does not wrap: it measures itself against the space
//! left on the current line and *then* allocates what it measured, so
//! inside a `horizontal_wrapped` a frame that does not fit overflows the
//! row rather than moving to the next line. Every chip-shaped widget here
//! therefore measures its text, allocates that size exactly with
//! [`egui::Ui::allocate_exact_size`] — which is what a wrapped layout
//! wraps on — and paints into the rect it gets back.

use super::icon as icons;
use super::motion;
use super::theme::{self, PAD, RADIUS, SELECTION_BAR, SPACE_MD, SPACE_SM, SPACE_XS};
use crate::color::Rgb;
use crate::icon::Icon;
use crate::theme::Palette;
use egui::{
    Align, Align2, Color32, CornerRadius, FontId, Rect, Response, Sense, Stroke, StrokeKind,
    TextStyle, Ui, Vec2,
};
use egui_extras::TableRow;

/// How far a hover animation has progressed, 0..=1, eased.
///
/// A thin alias for [`motion::hover`], kept because "hover_t" is what the
/// call sites read as. The curve and the duration live in
/// [`crate::motion`] with every other animation in the app — this module
/// used to own its own copy of an ease-out cubic and its own
/// `HOVER_SECONDS`, which is exactly how an app ends up with two easing
/// curves that are nearly the same.
#[must_use]
pub fn hover_t(ui: &Ui, id: egui::Id, hovered: bool) -> f32 {
    motion::hover(ui, id, hovered)
}

/// The fill for a hoverable surface, part-way through its ramp.
#[must_use]
pub fn hover_fill(ui: &Ui, id: egui::Id, hovered: bool, rest: Rgb, lifted: Rgb) -> Color32 {
    let t = hover_t(ui, id, hovered);
    theme::rgb(rest.lerp(lifted, t))
}

/// A navigation-rail entry: icon, label, and an accent bar when active.
///
/// The bar rather than a filled background because the rail is narrow and
/// a full-width fill at this size reads as a text field rather than a
/// selected item.
pub fn nav_item(ui: &mut Ui, theme: &Palette, icon: Icon, label: &str, active: bool) -> Response {
    /// The rail entry's height. Taller than a table row — this is a
    /// primary destination, not a list item.
    const HEIGHT: f32 = 36.0;
    /// The accent bar's width when the entry is active.
    const BAR: f32 = 3.0;
    /// The icon column's width, so every label starts on one column
    /// whatever shape the icon beside it draws.
    const ICON_COLUMN: f32 = 22.0;

    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, HEIGHT), Sense::click());
    let painter = ui.painter();

    let fill = if active {
        theme::rgb(theme.selection)
    } else {
        hover_fill(ui, response.id, response.hovered(), theme.app, theme.hover)
    };
    painter.rect_filled(rect, CornerRadius::same(RADIUS), fill);

    if active {
        let bar = Rect::from_min_size(
            rect.left_top() + Vec2::new(0.0, SPACE_XS),
            Vec2::new(BAR, rect.height() - SPACE_XS * 2.0),
        );
        painter.rect_filled(bar, CornerRadius::same(2), theme::rgb(theme.accent));
    }

    let text_color = if active {
        theme::rgb(theme.text)
    } else {
        theme::rgb(theme.text_muted)
    };
    let icon_colour = if active {
        theme::rgb(theme.accent)
    } else {
        text_color
    };
    // Centred in a fixed icon column rather than laid out beside the
    // label, so every label in the rail starts on one x however wide the
    // icon beside it happens to draw.
    let icon_box = Rect::from_center_size(
        egui::pos2(rect.left() + SPACE_MD + ICON_COLUMN / 2.0, rect.center().y),
        Vec2::splat(icons::NAV),
    );
    icons::paint(painter, icon_box, icon, icon_colour);

    painter.text(
        rect.left_center() + Vec2::new(SPACE_MD + ICON_COLUMN + SPACE_XS, 0.0),
        Align2::LEFT_CENTER,
        label,
        TextStyle::Body.resolve(ui.style()),
        text_color,
    );

    response
}

/// A tree disclosure control: a chevron that turns as it opens.
///
/// Both the category headings and the process rows use this, so the two
/// cannot end up with arrows at different sizes pointing different ways —
/// which is what they were, because each drew its own.
///
/// It *rotates* rather than swapping between two chevrons. A control that
/// turns reads as the same object opening; two shapes exchanged at the
/// halfway point read as one being replaced by another, and the tree then
/// feels like it reloads on every click rather than expanding.
///
/// `id_source` must identify the row, not its position — see
/// [`super::motion`]. Keyed on a row index, collapsing one row makes
/// every arrow below it spin.
pub fn disclosure(
    ui: &mut Ui,
    theme: &Palette,
    open: bool,
    id_source: impl std::hash::Hash + std::fmt::Debug,
) -> Response {
    let id = ui.id().with("disclosure").with(id_source);
    let (rect, response) = ui.allocate_exact_size(
        Vec2::new(icons::DISCLOSURE + SPACE_XS, theme::ROW_HEIGHT),
        Sense::click(),
    );

    let turn = motion::toggle(ui.ctx(), id, open, motion::QUICK);
    // A quarter turn: right-pointing when closed, down-pointing when
    // open. Clockwise, because the content appears below.
    let colour = theme
        .text_faint
        .lerp(theme.text, hover_t(ui, response.id, response.hovered()));
    icons::paint_rotated(
        ui.painter(),
        Rect::from_center_size(rect.center(), Vec2::splat(icons::DISCLOSURE)),
        Icon::ChevronRight,
        theme::rgb(colour),
        turn * 0.25,
    );
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Lays `count` cards out in as many columns as the pane can fit.
///
/// The equivalent of a CSS `repeat(auto-fill, minmax(min, 1fr))` grid:
/// the column count comes from the width available rather than being
/// stated, so the same code gives one column on a narrow window and four
/// on a wide one, and the cards always fill the row.
///
/// This exists because the Performance view stacked one full-width card
/// per device. A machine with three disks got three cards, each about a
/// hundred points tall and seventeen hundred wide, holding three short
/// numbers — so the view was a column of mostly-empty bars with the
/// bottom two thirds of the window blank. Cards that hold a label and
/// three stats want roughly the width of those stats, and the answer to
/// "what do I do with the rest" is another card, not more whitespace.
///
/// `minimum` is the narrowest a card may be before the grid drops to
/// fewer columns. It is a property of the card's *content* — the widest
/// stat row it has to hold without wrapping — so each caller states its
/// own rather than sharing one.
pub fn card_grid(ui: &mut Ui, minimum: f32, count: usize, mut card: impl FnMut(&mut Ui, usize)) {
    if count == 0 {
        return;
    }
    let (columns, width) = card_grid_layout(ui.available_width(), minimum, count);

    // Restored inside each card below. Zeroing it on the row is what
    // makes the gap between cards the explicit `add_space` and nothing
    // else — `ui.horizontal_top` inserts its own `item_spacing.x`
    // between siblings as well, so the real gap was `SPACE_MD +
    // SPACE_SM` while the column arithmetic above assumed `SPACE_MD`,
    // and five cards wanted twenty-five points more than the row had.
    let item_spacing = ui.spacing().item_spacing;

    for start in (0..count).step_by(columns) {
        let end = (start + columns).min(count);
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            for index in start..end {
                // An explicit width and a *zero* desired height. A region
                // given the parent's remaining height claims all of it,
                // so one card in a scroll area would grow to fill the
                // pane and strand its own content at the top.
                ui.allocate_ui_with_layout(
                    Vec2::new(width, 0.0),
                    egui::Layout::top_down(Align::Min),
                    |ui| {
                        // A card's *contents* want the app's normal
                        // spacing back. Without this the zeroing above
                        // reaches every label inside every card, and a
                        // caption and its value are drawn touching:
                        // "active41%".
                        ui.spacing_mut().item_spacing = item_spacing;
                        card(ui, index);
                    },
                );
                if index + 1 < end {
                    ui.add_space(SPACE_MD);
                }
            }
        });
        if end < count {
            ui.add_space(SPACE_MD);
        }
    }
}

/// How many `minimum`-wide columns fit across `available` width, and how
/// wide each of those columns actually gets.
///
/// A free function rather than inline in [`card_grid`] so the arithmetic
/// can be checked directly — a live `Ui` can show a grid looks wrong, but
/// not *why*, and the two numbers here are exactly the ones that would
/// have to disagree for a card to overflow its row or a grid to divide by
/// zero.
fn card_grid_layout(available: f32, minimum: f32, count: usize) -> (usize, f32) {
    if count == 0 {
        return (0, 0.0);
    }
    // At least one column, so a pane narrower than a single card still
    // draws it (clipped) rather than dividing by zero and drawing
    // nothing.
    let columns =
        (((available + SPACE_MD) / (minimum + SPACE_MD)).floor() as usize).clamp(1, count);
    let gaps = SPACE_MD * (columns.saturating_sub(1)) as f32;
    let width = ((available - gaps) / columns as f32).max(1.0);
    (columns, width)
}

/// A small labelled pill — a status, a count, a category.
///
/// Measures, allocates exactly, then paints; see the module docs on why a
/// `Frame` would overflow a wrapped row instead.
pub fn chip(ui: &mut Ui, text: &str, fill: Rgb, text_color: Rgb) -> Response {
    let font = TextStyle::Small.resolve(ui.style());
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_string(), font, theme::rgb(text_color));
    let size = galley.size() + Vec2::new(SPACE_SM * 2.0, SPACE_XS * 1.5);
    let (rect, response) = ui.allocate_exact_size(size, Sense::hover());

    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(RADIUS), theme::rgb(fill));
    painter.galley(
        rect.center() - galley.size() / 2.0,
        galley,
        theme::rgb(text_color),
    );
    response
}

/// Lays a single line of text out, truncated with an ellipsis if it does
/// not fit `width`.
///
/// `Painter::text` neither wraps nor truncates — it draws the whole
/// string wherever it is told to, so a long value runs straight through
/// whatever is drawn beside it. Clipping the painter instead would hide
/// the overflow but leave a word cut mid-glyph, which reads as a
/// rendering fault rather than as elision.
pub fn truncated(
    ui: &Ui,
    text: &str,
    font: FontId,
    color: Rgb,
    width: f32,
) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::single_section(
        text.to_string(),
        egui::TextFormat::simple(font, theme::rgb(color)),
    );
    job.wrap = egui::text::TextWrapping::truncate_at_width(width.max(0.0));
    ui.painter().layout_job(job)
}

/// How wide [`status_chip`] will draw for this text.
///
/// So a caller can reserve the room before laying out whatever precedes
/// the chip — a label that consumed the whole cell first left the chip
/// to be clipped in half by the column.
#[must_use]
pub fn status_chip_width(ui: &Ui, text: &str) -> f32 {
    let font = TextStyle::Small.resolve(ui.style());
    ui.painter()
        .layout_no_wrap(text.to_string(), font, Color32::PLACEHOLDER)
        .size()
        .x
        + SPACE_SM * 2.0
}

/// A chip carrying a **status**, tinted with the colour of that status.
///
/// [`chip`] takes a flat fill, and every status chip in the app passed
/// it `theme.raised` — which is also the colour of a striped row. That
/// worked only for as long as a row's stripe was (wrongly) confined to
/// its first column: the moment a row filled edge to edge, every chip on
/// every second row lost its pill and became a coloured word floating in
/// the middle of a table. See [`Row`].
///
/// Tinting with the status's own colour fixes it in the direction the
/// design wanted anyway. A green pill for Running and an amber one for
/// Starting say what they mean before the word is read, and the tint
/// reads against both of the surfaces a row can be, because it is not
/// either of them.
pub fn status_chip(ui: &mut Ui, text: &str, color: Rgb) -> Response {
    /// How strongly the status colour tints the pill.
    ///
    /// Low: the word on top is drawn in the same hue at full strength,
    /// and a fill that approached it would leave the two the same
    /// colour. This is a wash behind the text, not a badge.
    const TINT: u8 = 38;

    let font = TextStyle::Small.resolve(ui.style());
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_string(), font, theme::rgb(color));
    let size = galley.size() + Vec2::new(SPACE_SM * 2.0, SPACE_XS * 1.5);
    let (rect, response) = ui.allocate_exact_size(size, Sense::hover());

    let painter = ui.painter();
    painter.rect_filled(
        rect,
        CornerRadius::same(RADIUS),
        theme::translucent(color, TINT),
    );
    painter.galley(
        rect.center() - galley.size() / 2.0,
        galley,
        theme::rgb(color),
    );
    response
}

/// A column heading for a column that cannot be sorted.
///
/// Drawn rather than composed so it matches [`sortable_header`]'s text
/// exactly — the same style, the same colour, the same baseline. A
/// heading that is a plain `ui.label` beside four drawn ones is a
/// heading sitting a pixel off the others in a slightly different grey,
/// which is the sort of difference nobody can name and everybody sees.
///
/// It reserves no arrow column: there is no arrow that could appear
/// there, so the space would be a permanent gap the sortable headings
/// do not have.
pub fn plain_header(ui: &mut Ui, theme: &Palette, label: &str) {
    let font = TextStyle::Small.resolve(ui.style());
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_string(), font, theme::rgb(theme.text_muted));
    let (rect, _) = ui.allocate_exact_size(
        galley.size() + Vec2::new(SPACE_SM, SPACE_XS),
        Sense::hover(),
    );
    ui.painter().galley(
        egui::pos2(rect.left(), rect.center().y - galley.size().y / 2.0),
        galley,
        theme::rgb(theme.text_muted),
    );
}

/// A column heading that can be clicked to sort.
///
/// `claims_width` is the trap this signature exists for. `egui_extras`
/// records the widest thing a column ever allocated and will not shrink a
/// `remainder()` column below it — so a header that allocates its whole
/// cell sets a floor the column can never come back under, and widening
/// then narrowing the window leaves the table with a scrollbar over space
/// it gave back. So the header *senses* across the whole cell but
/// *allocates* only what its text needs.
pub fn sortable_header(
    ui: &mut Ui,
    theme: &Palette,
    label: &str,
    sorted: Option<bool>,
    claims_width: bool,
    right_aligned: bool,
    lifted: bool,
) -> Response {
    let font = TextStyle::Small.resolve(ui.style());
    let color = if sorted.is_some() {
        theme.text
    } else {
        theme.text_muted
    };
    // A heading being dragged is dimmed towards the surface behind it, so
    // it reads as lifted out of the row rather than as a second copy of
    // itself sitting beside the ghost that follows the pointer.
    let color = if lifted {
        color.lerp(theme.panel, 0.7)
    } else {
        color
    };
    // The arrow occupies a reserved column whether or not this heading
    // is the sorted one, so a column does not jump sideways when the
    // sort moves onto it. It is drawn rather than set: the triangles
    // that would do this in one string are not in egui's bundled fonts
    // (see `crate::icon`).
    let arrow_column = icons::DISCLOSURE + SPACE_XS;
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_string(), font, theme::rgb(color));

    let cell = ui.available_rect_before_wrap();
    let wanted = if claims_width {
        Vec2::new(cell.width(), galley.size().y + SPACE_XS)
    } else {
        galley.size() + Vec2::new(SPACE_SM + arrow_column, SPACE_XS)
    };
    // `click_and_drag`, so the same control sorts on a click and reorders
    // on a drag. egui reports `clicked()` as false once the pointer has
    // moved far enough to count as a drag, so the two cannot both fire —
    // which is what lets a heading be its own drag handle rather than
    // needing a separate grip beside it.
    let (rect, response) = ui.allocate_exact_size(wanted, super::dnd::Lane::sense());

    // Sensed across the whole cell so the click target is the column
    // heading rather than only its glyphs, without the allocation that
    // would pin the column's minimum width.
    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);

    // The arrow sits on the outside of the label — past its right edge
    // in a left-aligned heading, before its left edge in a right-aligned
    // one — so it is always on the column's own margin rather than
    // between the heading and its neighbour.
    let text_width = galley.size().x;
    let (text_left, arrow_centre) = if right_aligned {
        (
            rect.right() - text_width,
            rect.right() - text_width - SPACE_XS - icons::DISCLOSURE / 2.0,
        )
    } else {
        (
            rect.left(),
            rect.left() + text_width + SPACE_XS + icons::DISCLOSURE / 2.0,
        )
    };
    ui.painter().galley(
        egui::pos2(text_left, rect.center().y - galley.size().y / 2.0),
        galley,
        theme::rgb(color),
    );

    if let Some(descending) = sorted {
        // The arrow rotates between the two directions rather than being
        // swapped, so a click on an already-sorted heading reads as
        // "this reversed" rather than as the heading being redrawn.
        let flip = motion::toggle(
            ui.ctx(),
            response.id.with("sort"),
            descending,
            motion::QUICK,
        );
        let icon = if flip > 0.5 {
            Icon::ArrowDown
        } else {
            Icon::ArrowUp
        };
        icons::paint(
            ui.painter(),
            Rect::from_center_size(
                egui::pos2(arrow_centre, rect.center().y),
                Vec2::splat(icons::DISCLOSURE),
            ),
            icon,
            theme::rgb(theme.accent),
        );
    }
    response
}

/// A horizontal rule under a section heading.
///
/// A painted line rather than `ui.separator()`, which allocates padding
/// of its own on top of the row spacing — so two panes that ruled off
/// their headings with it would end up with different gaps depending on
/// what preceded them.
pub fn section_rule(ui: &mut Ui, theme: &Palette) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 1.0), Sense::hover());
    ui.painter().hline(
        rect.x_range(),
        rect.center().y,
        Stroke::new(1.0, theme::rgb(theme.border)),
    );
}

/// A section heading with its rule.
pub fn section(ui: &mut Ui, theme: &Palette, title: &str) {
    ui.label(
        egui::RichText::new(title)
            .color(theme::rgb(theme.text))
            .size(15.0)
            .strong(),
    );
    ui.add_space(SPACE_XS);
    section_rule(ui, theme);
    ui.add_space(SPACE_SM);
}

/// A large readout: a value, its unit, and a caption.
///
/// The Performance page is mostly these. Drawn rather than composed from
/// labels so the baseline of the value and the position of the caption
/// are identical across every tile, whatever the value's width.
pub fn stat(ui: &mut Ui, theme: &Palette, caption: &str, value: &str, accent: Rgb) -> Response {
    /// The tile's height: enough for a 24px value and a small caption
    /// under it, with the module's own spacing between.
    const HEIGHT: f32 = 58.0;

    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, HEIGHT), Sense::hover());
    let painter = ui.painter();

    painter.text(
        rect.left_top() + Vec2::new(0.0, SPACE_XS),
        Align2::LEFT_TOP,
        caption,
        TextStyle::Small.resolve(ui.style()),
        theme::rgb(theme.text_muted),
    );
    painter.text(
        rect.left_bottom() + Vec2::new(0.0, -SPACE_XS),
        Align2::LEFT_BOTTOM,
        value,
        FontId::new(24.0, egui::FontFamily::Proportional),
        theme::rgb(accent),
    );
    response
}

/// A labelled fraction bar — memory in use, disk full, a core's load.
pub fn meter(ui: &mut Ui, theme: &Palette, fraction: f32, fill: Rgb) -> Response {
    /// The bar's height. Thin: it is a supporting indicator beside a
    /// number, not the number itself.
    const HEIGHT: f32 = 6.0;

    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, HEIGHT), Sense::hover());
    let radius = CornerRadius::same(3);

    ui.painter()
        .rect_filled(rect, radius, theme::rgb(theme.raised));
    // A non-finite fraction would produce a NaN width, which egui paints
    // as nothing — so a broken measurement would look like an empty bar
    // rather than a broken one.
    let target = if fraction.is_finite() {
        fraction.clamp(0.0, 1.0)
    } else {
        0.0
    };
    // The bar travels to its new level rather than jumping there.
    //
    // These are sampled once a second, so an un-animated bar spends that
    // second perfectly still and then teleports — which reads as the
    // display being broken rather than as the machine being busy. A bar
    // that slides carries the *direction* of the change as well as its
    // value, which no single frame of a static bar can.
    let fraction = motion::value(ui.ctx(), response.id.with("level"), target);
    if fraction > 0.0 {
        let filled = Rect::from_min_size(
            rect.min,
            Vec2::new((rect.width() * fraction).max(2.0), rect.height()),
        );
        ui.painter().rect_filled(filled, radius, theme::rgb(fill));
    }
    response
}

/// The rect a row's background covers for the cell being drawn.
///
/// The cell's own rect grown by half the item spacing on each side — the
/// same `gapless_rect` `egui_extras` uses — so adjacent cells' fills meet
/// and a filled row has no hairline of panel colour at every boundary.
fn gapless_cell(ui: &Ui) -> Rect {
    /// A half point of slop past the half-spacing.
    ///
    /// `egui_extras` rounds its own gapless rect to the pixel grid and
    /// this does not, so without the slop its rect sits outside ours and
    /// a line of its brighter hover fill survives along the row's top
    /// edge — which is exactly as much as a stray hairline needs.
    ///
    /// A whole point rather than the half the rounding can account for,
    /// because the rounding depends on the device scale and being a
    /// fraction short shows. The cost is that a row's fill laps a point
    /// over its neighbour's, which nothing can see: rows are painted in
    /// order, so the lap lands on a band boundary that moves by a point.
    const SLOP: f32 = 1.0;

    let spacing = ui.spacing().item_spacing;
    ui.max_rect().expand2(0.5 * spacing + Vec2::splat(SLOP))
}

/// The whole row's rect: for hit-testing, not for painting.
///
/// Taken from the cell's vertical extent and the table's horizontal one,
/// because a row is hovered when the pointer is anywhere along it and not
/// only over the cell being drawn at the time.
///
/// Deliberately *not* expanded the way [`gapless_cell`] is: two rows
/// whose hit rects overlapped by the spacing between them would both
/// report themselves hovered along the seam.
fn row_rect(ui: &Ui, viewport: Rect) -> Rect {
    let cell = ui.max_rect();
    Rect::from_min_max(
        egui::pos2(viewport.left(), cell.top()),
        egui::pos2(viewport.right(), cell.bottom()),
    )
}

/// The clip a row's fill is painted through.
///
/// The row's full width, and the *gapless* vertical extent — tall enough
/// to include the half-spacing overhang above and below. Clipped to the
/// bare row instead, that overhang is trimmed off ours while
/// `egui_extras`' own fill keeps it, and its brighter colour shows
/// through as a hairline along the top and bottom of every hovered row.
fn row_clip(ui: &Ui, viewport: Rect) -> Rect {
    let gapless = gapless_cell(ui);
    // Never *narrower* than the cell's own gapless rect, whatever the
    // viewport says. A table that takes its viewport from
    // `available_rect_before_wrap` gets one whose left edge is the first
    // cell's left edge — and `egui_extras`' own hover fill is expanded
    // half a spacing past that, so clipping ours to the viewport left
    // four points of the *control* colour showing down the left of every
    // hovered row in the Services list. Four points of grey, only while
    // hovered, only on two of the four tables.
    Rect::from_min_max(
        egui::pos2(viewport.left().min(gapless.left()), gapless.top()),
        egui::pos2(viewport.right().max(gapless.right()), gapless.bottom()),
    )
}

/// Fills a grouping row — a category heading, not a record.
///
/// Painted behind every cell, like [`row_background`], and for the same
/// reason. [`Row`] is what does the painting.
fn group_row_background(ui: &Ui, theme: &Palette, viewport: Rect) {
    let mut painter = ui.painter().clone();
    painter.set_clip_rect(row_clip(ui, viewport));
    painter.rect_filled(
        gapless_cell(ui),
        CornerRadius::ZERO,
        theme::rgb(theme.raised),
    );
}

/// Paints a row's background, its hover lift, and its selection bar.
///
/// One function for all three so that a row cannot be selected-looking in
/// one table and hover-looking in another. The accent bar is what makes a
/// selection unmistakable — the fill itself is a light tint, chosen so
/// that secondary text on a selected row still clears WCAG AA. See
/// [`crate::theme::Palette::derive`].
///
/// ## Painted from every cell, not once per row
///
/// `egui_extras` paints its own stripe, selection and hover **per cell**,
/// before running that cell's contents, and its hover fill comes from
/// `widgets.hovered.bg_fill` — which this app points at the *control*
/// colour, because the scrollbar handle reads it. A row fill painted once
/// from the first cell therefore lands underneath every one of those
/// except its own, and the row comes out in two colours with the seam at
/// the first column's edge. That is exactly how it looked.
///
/// Silencing `egui_extras` is not on offer: blanking that fill takes the
/// scrollbar handle with it. So this paints per cell as well, on top, in
/// the same gapless rect — the app's colours win by being painted last.
///
/// Which is why this is private and [`Row`] is not: three of the app's
/// four tables called it from their first cell only, and looked it.
///
/// ## It works `hovered` out itself
///
/// All four call sites passed `false`, so the app's own hover ramp had
/// never been drawn at all and the highlight people saw was
/// `egui_extras`'. A parameter that every call site gets wrong is not a
/// parameter.
fn row_background(
    ui: &Ui,
    theme: &Palette,
    viewport: Rect,
    id: egui::Id,
    selected: bool,
    striped: bool,
) {
    let row = row_rect(ui, viewport);
    // Asked of the context rather than of the `Ui`. `Ui::rect_contains_pointer`
    // intersects the rect with the ui's own clip rect first, and a table
    // cell's clip rect is the cell — so a row-wide rect came back clipped
    // to one cell, and only the cell actually under the pointer lit up
    // while the other seven stayed at rest.
    let hovered = ui.ctx().rect_contains_pointer(ui.layer_id(), row);

    let base = if striped { theme.raised } else { theme.panel };
    let fill = if selected {
        theme::rgb(theme.selection)
    } else {
        hover_fill(ui, id, hovered, base, theme.hover)
    };
    // Through a painter clipped to the row rather than the cell. A
    // column drawn with `.clip(true)` gives its cell ui a clip rect of
    // exactly the cell, which would trim the half-spacing overhang back
    // off again and leave the boundary showing — the very gap the
    // overhang exists to cover. `set_clip_rect` replaces the clip;
    // `with_clip_rect` intersects, which would keep the cell's.
    let mut painter = ui.painter().clone();
    painter.set_clip_rect(row_clip(ui, viewport));
    painter.rect_filled(gapless_cell(ui), CornerRadius::ZERO, fill);

    // The bar belongs to the row rather than to a cell, so only the
    // leading cell draws one — otherwise every column grows its own.
    if selected && ui.max_rect().left() <= viewport.left() + 0.5 {
        // It animates out from the leading edge rather than appearing at
        // full height, so moving the selection down a list reads as one
        // marker travelling rather than as a series of unrelated flashes.
        let grown = motion::toggle(ui.ctx(), id.with("selected"), true, motion::QUICK);
        let bar = Rect::from_center_size(
            egui::pos2(row.left() + SELECTION_BAR / 2.0, row.center().y),
            Vec2::new(SELECTION_BAR, row.height() * grown),
        );
        painter.rect_filled(bar, CornerRadius::ZERO, theme::rgb(theme.accent));
    }
}

/// Which fill a [`Row`] paints behind its cells.
#[derive(Clone, Copy)]
enum RowFill {
    /// A record: striped by position, lit under the pointer, and
    /// selectable.
    Record {
        id: egui::Id,
        selected: bool,
        striped: bool,
    },
    /// A category heading, which is none of those things.
    Group,
}

/// One row of a table, painting its own background behind every cell.
///
/// ## Why the row draws its cells rather than the view drawing them
///
/// A row's fill has to be painted from *every* cell — see
/// [`row_background`] for why `egui_extras` leaves no other option. A
/// view that writes its own `row.col(..)` calls therefore has to remember
/// to paint the fill in each one, and what actually happened is that
/// three of the four tables painted it in the first cell and stopped:
/// Details, Services and Startup all shipped with the stripe, the hover
/// lift and the selection tint confined to their Name column, so a
/// selected row read as a selected *name* with seven unrelated numbers
/// beside it. The process tree was the only one that got it right, and
/// only because it had been fixed by hand once already.
///
/// So the cell is the thing a view is handed, and the fill is not the
/// view's business. `row.cell(|ui| ..)` in place of `row.col(|ui| ..)` is
/// the whole difference at a call site.
///
/// Drop the trailing spacer column and the row stops one column short of
/// the window's edge, which is why [`Row::spacer`] exists and is named
/// for what it is rather than being a `cell` with an empty body.
pub struct Row<'a, 'b, 'r> {
    row: &'r mut TableRow<'a, 'b>,
    theme: &'r Palette,
    viewport: Rect,
    fill: RowFill,
}

impl<'a, 'b, 'r> Row<'a, 'b, 'r> {
    /// A record row — a process, a service, a startup entry.
    ///
    /// `id` keys the hover animation, and it must name the *thing* rather
    /// than the slot it is in: keyed by row index, re-sorting the table
    /// hands every row the animation state of whatever used to be there
    /// and the whole table flashes. See [`crate::motion`].
    pub fn record(
        row: &'r mut TableRow<'a, 'b>,
        theme: &'r Palette,
        viewport: Rect,
        id: egui::Id,
        selected: bool,
        striped: bool,
    ) -> Self {
        // `egui_extras`' own selection fill comes from
        // `visuals.selection.bg_fill`, which is the accent at full
        // strength — under the app's own tint that is a solid accent slab
        // with unreadable text on it. The app paints selection itself, so
        // the row is never selected as far as the table is concerned.
        row.set_selected(false);
        Self {
            row,
            theme,
            viewport,
            fill: RowFill::Record {
                id,
                selected,
                striped,
            },
        }
    }

    /// A grouping row — a category heading, not a record.
    pub fn group(row: &'r mut TableRow<'a, 'b>, theme: &'r Palette, viewport: Rect) -> Self {
        row.set_selected(false);
        Self {
            row,
            theme,
            viewport,
            fill: RowFill::Group,
        }
    }

    /// Adds a cell, with the row's background already painted behind it.
    pub fn cell(&mut self, contents: impl FnOnce(&mut Ui)) {
        // Copied out before the call: `TableRow::col` borrows the row
        // mutably, so the closure cannot read `self` while it runs.
        let (theme, viewport, fill) = (self.theme, self.viewport, self.fill);
        self.row.col(|ui| {
            match fill {
                RowFill::Record {
                    id,
                    selected,
                    striped,
                } => row_background(ui, theme, viewport, id, selected, striped),
                RowFill::Group => group_row_background(ui, theme, viewport),
            }
            contents(ui);
        });
    }

    /// Adds the trailing spacer cell — the one that absorbs the window's
    /// slack, and the one a row has to fill to reach the window's edge.
    pub fn spacer(&mut self) {
        self.cell(|_| {});
    }

    /// The row's response: the union of its cells, which is what a click
    /// anywhere along the row lands on.
    pub fn response(&self) -> Response {
        self.row.response()
    }
}

/// A cell tinted by how busy the thing it reports is.
///
/// The process table's CPU, memory, disk and GPU columns use this: a
/// glance down the column finds the heavy rows without reading any of
/// them, which is the entire reason the columns are sorted by default.
///
/// The tint is drawn at low alpha over whatever the row's background is,
/// rather than replacing it — so a selected busy row still reads as
/// selected.
pub fn heat_cell(ui: &Ui, theme: &Palette, rect: Rect, load: f32) {
    /// Below this the tint is not worth drawing: it would be a wash of
    /// almost-invisible colour across three hundred idle rows, which is
    /// noise rather than signal.
    const FLOOR: f32 = 0.03;
    /// The tint's opacity at full load.
    ///
    /// This was 56, which paints a solid slab of colour behind the
    /// number — a column of them reads as a set of coloured blocks that
    /// happen to contain digits, rather than as numbers with a weight.
    /// The gauge below carries the magnitude now, so the tint only has
    /// to make the cell feel warm.
    const MAX_ALPHA: f32 = 22.0;
    /// The height of the gauge along the cell's bottom edge.
    const GAUGE: f32 = 2.0;

    if !load.is_finite() || load < FLOOR {
        return;
    }
    let load = load.clamp(0.0, 1.0);
    // Square-rooted, as in `crate::color::heat`: a linear ramp leaves
    // everything under about 30% looking identical to idle, and that is
    // exactly the range where "this should not be doing anything" lives.
    let emphasis = load.sqrt();
    let color = theme.heat(load);

    let alpha = (emphasis * MAX_ALPHA).clamp(0.0, 255.0) as u8;
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::ZERO, theme::translucent(color, alpha));

    // A gauge along the bottom edge, proportional to the load.
    //
    // The reason this is better than tinting harder: a tint encodes
    // magnitude as *saturation*, which the eye compares badly between
    // two cells that are not adjacent and which collides with the
    // requirement that the number on top stay readable. A length is
    // compared accurately at a glance, down a whole column, and costs
    // the text nothing. It is also what makes a row's four metric cells
    // legible as a little profile of that process.
    let width = rect.width() * load;
    if width >= 1.0 {
        let gauge = Rect::from_min_size(
            egui::pos2(rect.left(), rect.bottom() - GAUGE),
            Vec2::new(width, GAUGE),
        );
        painter.rect_filled(gauge, CornerRadius::ZERO, theme::rgb(color));
    }
}

/// A right-aligned numeric cell in the tabular font.
///
/// Monospace so the digits line up: a proportional font gives every digit
/// a different width, so a column of live-updating numbers shimmers as
/// its digits change and the decimal points do not form a column.
pub fn number(ui: &mut Ui, theme: &Palette, text: &str, muted: bool) {
    ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
        ui.label(
            egui::RichText::new(text)
                .text_style(TextStyle::Monospace)
                .color(theme::rgb(if muted {
                    theme.text_muted
                } else {
                    theme.text
                })),
        );
    });
}

/// A borderless icon button, for the title bar and toolbars.
pub fn icon_button(ui: &mut Ui, theme: &Palette, icon: Icon, tooltip: &str) -> Response {
    /// The button's square size. Matches the title bar's own height less
    /// its padding, so the window controls fill the bar's full height and
    /// are hittable by throwing the pointer at the corner.
    const SIZE: f32 = 32.0;

    let (rect, response) = ui.allocate_exact_size(Vec2::new(SIZE, SIZE), Sense::click());
    let fill = hover_fill(ui, response.id, response.hovered(), theme.app, theme.hover);
    ui.painter()
        .rect_filled(rect, CornerRadius::same(RADIUS), fill);
    icons::paint(
        ui.painter(),
        Rect::from_center_size(rect.center(), Vec2::splat(icons::INLINE)),
        icon,
        theme::rgb(theme.text),
    );
    response.on_hover_text(tooltip)
}

/// The window's close button, which highlights in the danger colour.
///
/// Separate from [`icon_button`] because it is the one control in the app
/// whose hover colour is a warning rather than a lift — every window on
/// Windows does this, and one that did not would be the odd one out in a
/// way people notice without being able to say why.
pub fn close_button(ui: &mut Ui, theme: &Palette) -> Response {
    const SIZE: f32 = 32.0;
    let (rect, response) = ui.allocate_exact_size(Vec2::new(SIZE, SIZE), Sense::click());
    let fill = hover_fill(ui, response.id, response.hovered(), theme.app, theme.danger);
    ui.painter()
        .rect_filled(rect, CornerRadius::same(RADIUS), fill);
    // The mark flips to the on-accent colour as the danger fill comes
    // up, or it goes unreadable against it at the end of the ramp.
    let t = hover_t(ui, response.id, response.hovered());
    let mark = theme.text.lerp(theme.text_on_accent, t);
    icons::paint(
        ui.painter(),
        Rect::from_center_size(rect.center(), Vec2::splat(icons::INLINE)),
        Icon::Close,
        theme::rgb(mark),
    );
    response.on_hover_text("Close")
}

/// A primary (accent-filled) button.
pub fn primary_button(ui: &mut Ui, theme: &Palette, label: &str) -> Response {
    let font = TextStyle::Button.resolve(ui.style());
    let galley =
        ui.painter()
            .layout_no_wrap(label.to_string(), font, theme::rgb(theme.text_on_accent));
    let size = galley.size() + Vec2::new(SPACE_MD * 2.0, SPACE_SM);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());

    let fill = hover_fill(
        ui,
        response.id,
        response.hovered(),
        theme.accent,
        theme.accent_hover,
    );
    ui.painter()
        .rect_filled(rect, CornerRadius::same(RADIUS), fill);
    ui.painter().galley(
        rect.center() - galley.size() / 2.0,
        galley,
        theme::rgb(theme.text_on_accent),
    );
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A destructive button — the danger colour, for "End task".
pub fn danger_button(ui: &mut Ui, theme: &Palette, label: &str, enabled: bool) -> Response {
    let text_color = if enabled {
        theme.text_on_accent
    } else {
        theme.text_faint
    };
    let font = TextStyle::Button.resolve(ui.style());
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_string(), font, theme::rgb(text_color));
    let size = galley.size() + Vec2::new(SPACE_MD * 2.0, SPACE_SM);
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(size, sense);

    let fill = if enabled {
        hover_fill(
            ui,
            response.id,
            response.hovered(),
            theme.danger,
            theme.danger,
        )
    } else {
        theme::rgb(theme.raised)
    };
    ui.painter()
        .rect_filled(rect, CornerRadius::same(RADIUS), fill);
    ui.painter().rect_stroke(
        rect,
        CornerRadius::same(RADIUS),
        Stroke::new(1.0, theme::rgb(theme.border)),
        StrokeKind::Inside,
    );
    ui.painter().galley(
        rect.center() - galley.size() / 2.0,
        galley,
        theme::rgb(text_color),
    );
    response
}

/// An empty-state message, centred in the space available.
///
/// A view with nothing in it has to say so. A blank pane is
/// indistinguishable from one that failed to load, and the difference
/// matters most on the views — Services, Startup — where an empty result
/// can mean "access denied" rather than "nothing here".
pub fn empty_state(ui: &mut Ui, theme: &Palette, message: &str) {
    let rect = ui.available_rect_before_wrap();
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        message,
        TextStyle::Body.resolve(ui.style()),
        theme::rgb(theme.text_faint),
    );
    // Consume the space so a caller that draws after this does not
    // overlap the message.
    ui.allocate_space(rect.size());
}

/// A one-line note where a *section* has nothing to list.
///
/// [`empty_state`] takes the whole of the pane that is left, which is
/// right for the message a view shows when it has nothing at all: the
/// message belongs in the middle of the empty space, and there is
/// nothing after it to push down. Partway down a panel it is wrong, and
/// silently so — the Network panel's "every adapter is virtual" note
/// claimed the rest of the page and pushed the virtual-adapter list, the
/// only list such a machine has, off the bottom of it.
///
/// So this one is a line, and the panel goes on underneath.
pub fn empty_note(ui: &mut Ui, theme: &Palette, message: &str) {
    let height = ui.text_style_height(&TextStyle::Body) + SPACE_SM * 2.0;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), height), Sense::hover());
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        message,
        TextStyle::Body.resolve(ui.style()),
        theme::rgb(theme.text_faint),
    );
}

/// A key/value line for a details pane.
pub fn detail_row(ui: &mut Ui, theme: &Palette, key: &str, value: &str) {
    /// The label column's width, so every value starts on one column.
    const KEY_WIDTH: f32 = 104.0;

    // Painted rather than composed from two labels in a `ui.horizontal`.
    // That is what it was, and `ui.horizontal`'s centre cross-alignment
    // put the value's baseline a couple of points above the key's on
    // every row of the inspector — two columns of text that never quite
    // sat on the same line, in a pane made of nothing but rows of two
    // columns of text.
    let height = ui.text_style_height(&TextStyle::Small);
    let (rect, _) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), height + SPACE_XS),
        Sense::hover(),
    );
    let font = TextStyle::Small.resolve(ui.style());
    let top = egui::pos2(rect.left(), rect.top());

    ui.painter().galley(
        top,
        truncated(ui, key, font.clone(), theme.text_muted, KEY_WIDTH),
        theme::rgb(theme.text_muted),
    );
    // Truncated to what is left of the pane. The inspector shows a
    // window title, a full account name and a path, any of which is
    // longer than the pane is wide.
    ui.painter().galley(
        egui::pos2(rect.left() + KEY_WIDTH, rect.top()),
        truncated(ui, value, font, theme.text, rect.width() - KEY_WIDTH),
        theme::rgb(theme.text),
    );
    ui.add_space(SPACE_XS);
}

/// The inset a pane's content starts at.
///
/// Exposed so a view that lays out its own rects rather than using a
/// [`theme::card`] still lands on the same column as the ones that do.
#[must_use]
pub const fn pane_inset() -> f32 {
    PAD
}

#[cfg(test)]
mod tests {
    use super::card_grid_layout;
    use super::theme::SPACE_MD;

    #[test]
    fn no_cards_is_no_columns_rather_than_a_division_by_zero() {
        let (columns, width) = card_grid_layout(400.0, 340.0, 0);
        assert_eq!(columns, 0, "an empty grid has no columns to divide by");
        assert!((width - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn a_pane_narrower_than_one_card_still_gets_a_single_column() {
        // The comment this guards: a pane narrower than a single card
        // should still draw it, clipped, rather than the column count
        // rounding down to zero and drawing nothing at all.
        let (columns, width) = card_grid_layout(100.0, 340.0, 3);
        assert_eq!(columns, 1, "a too-narrow pane must still get one column");
        assert!(width > 0.0, "the single column must have positive width");
    }

    #[test]
    fn a_pane_with_no_room_at_all_still_gets_a_positive_width() {
        // `width` floors at 1.0 rather than 0.0 or negative — a zero or
        // negative width is not "small", it is a card that never
        // allocates and a grid that draws nothing where a clipped card
        // was wanted.
        let (columns, width) = card_grid_layout(0.0, 340.0, 3);
        assert_eq!(columns, 1);
        assert!((width - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn the_column_count_never_exceeds_the_card_count() {
        // Three cards across a stadium-wide pane should still be three
        // columns, not padded out to however many `minimum`-wide slots
        // fit — a fourth, empty column reads as a bug, not free space.
        let (columns, _) = card_grid_layout(4000.0, 340.0, 3);
        assert_eq!(columns, 3);
    }

    #[test]
    fn the_columns_and_their_gaps_never_overflow_the_available_width() {
        // The structural invariant `card_grid` depends on: whatever
        // width this hands back, `columns` of them plus the gaps between
        // them must fit in what was actually available, or every card in
        // the row overflows together. Starts at 1.0, not 0.0 — below
        // that the width floor in
        // `a_pane_with_no_room_at_all_still_gets_a_positive_width`
        // deliberately claims more than is available, on purpose.
        for available in [1.0, 150.0, 339.0, 340.0, 341.0, 900.0, 4000.0] {
            for count in [1_usize, 2, 3, 7] {
                let (columns, width) = card_grid_layout(available, 340.0, count);
                let gaps = SPACE_MD * (columns.saturating_sub(1)) as f32;
                let claimed = columns as f32 * width + gaps;
                assert!(
                    claimed <= available + 0.01,
                    "available={available}, count={count}: {columns} columns \
                     of {width} plus gaps is {claimed}, which overflows"
                );
            }
        }
    }

    /// The curve's own properties — that it starts at rest, ends
    /// lifted, decelerates rather than running linearly, never goes
    /// backwards, and clamps nonsense — are pinned in
    /// [`crate::motion`]'s tests, which run on every platform rather
    /// than only on the Windows CI job. This module used to carry its
    /// own copy of all four, against its own private `cubic_out`.
    ///
    /// What is worth checking *here* is the thing that could drift once
    /// they were merged: that the hover helper still reaches for the
    /// app's hover duration rather than acquiring one of its own.
    #[test]
    fn the_hover_ramp_uses_the_apps_own_hover_duration() {
        let source = include_str!("widgets.rs");
        assert!(
            source.contains("motion::hover(ui, id, hovered)"),
            "hover_t no longer delegates to motion::hover, so the hover \
             ramp has picked up a duration of its own"
        );
    }

    #[test]
    fn a_row_of_cells_tiles_its_background_with_no_gaps() -> anyhow::Result<()> {
        // A row's background is painted per cell, so the question is
        // whether the cells' fills *meet*. `egui_extras` puts
        // `item_spacing.x` between columns, and a fill of each cell's own
        // rect leaves a hairline of panel colour at every boundary — eight
        // filled cells reading as eight cells rather than one row. Hence
        // the gapless expansion, and hence this.
        //
        // The earlier version of this test asserted the opposite shape:
        // one call from the first cell painting the whole row. That was
        // right until `egui_extras`' own per-cell hover fill turned out to
        // land on top of it everywhere except the first column.
        use super::super::theme;
        use super::row_background;
        use egui::{Rect, Sense, Shape, Vec2};

        let theme = crate::theme::Catalog::load().get(None).clone();
        let ctx = egui::Context::default();
        theme::apply(&ctx, &theme);
        let window = Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(800.0, 200.0));
        let input = egui::RawInput {
            screen_rect: Some(window),
            ..Default::default()
        };

        /// Four columns of this width, laid out with the app's own
        /// spacing between them, as a table would.
        const CELL: f32 = 180.0;

        let mut viewport = None;
        let mut output = ctx.run_ui(input, |ui| {
            let row = ui.available_rect_before_wrap();
            viewport = Some(row);
            let spacing = ui.spacing().item_spacing.x;
            for column in 0..4 {
                let left = row.left() + column as f32 * (CELL + spacing);
                let cell = Rect::from_min_size(
                    egui::pos2(left, row.top()),
                    Vec2::new(CELL, theme::ROW_HEIGHT),
                );
                ui.scope_builder(egui::UiBuilder::new().max_rect(cell), |ui| {
                    ui.set_clip_rect(cell);
                    ui.allocate_exact_size(cell.size(), Sense::hover());
                    row_background(ui, &theme, row, egui::Id::new("row"), true, false);
                });
            }
        });
        output.textures_delta.clear();
        let viewport = viewport.ok_or_else(|| anyhow::anyhow!("nothing was drawn"))?;

        // Every filled span, clipped as it will actually be painted.
        let mut spans: Vec<(f32, f32)> = output
            .shapes
            .iter()
            .filter_map(|clipped| {
                if let Shape::Rect(rect) = &clipped.shape {
                    let painted = rect.rect.intersect(clipped.clip_rect);
                    (painted.width() > 1.0).then_some((painted.left(), painted.right()))
                } else {
                    None
                }
            })
            .collect();
        spans.sort_by(|a, b| a.0.total_cmp(&b.0));

        let mut reach = viewport.left();
        for (left, right) in &spans {
            if *left > reach + 0.01 {
                break;
            }
            reach = reach.max(*right);
        }
        let covered = reach - viewport.left();
        let wanted = 4.0f32.mul_add(CELL, 3.0 * 8.0);
        assert!(
            covered >= wanted - 0.5,
            "the row's fills cover an unbroken {covered} points from its              left edge, but four cells and the gaps between them span              {wanted} — a gap means the column boundaries show through"
        );
        Ok(())
    }
}

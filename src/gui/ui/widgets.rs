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
use egui_extras::{TableBody, TableRow};

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

/// The shared, inset keyboard-focus ring, visible on neutral and filled controls.
pub fn focus_ring(ui: &Ui, theme: &Palette, response: &Response) {
    if response.has_focus() {
        paint_focus_ring(
            ui.painter(),
            theme,
            response.rect,
            CornerRadius::same(RADIUS),
        );
    }
}

fn paint_focus_ring(painter: &egui::Painter, theme: &Palette, rect: Rect, radius: CornerRadius) {
    // A surface-colored gap keeps the accent ring visible even on an
    // accent-filled primary button. Both strokes stay inside its clip.
    painter.rect_stroke(
        rect.shrink(1.0),
        radius,
        Stroke::new(4.0, theme::rgb(theme.app)),
        StrokeKind::Inside,
    );
    painter.rect_stroke(
        rect.shrink(2.0),
        radius,
        Stroke::new(2.0, theme::rgb(theme.accent)),
        StrokeKind::Inside,
    );
}

/// Maximum height for a table's scrolling body inside the remaining pane.
///
/// `TableBuilder::header` advances the cursor by the header and the Ui's
/// vertical item spacing before starting its body. Reserving only the visible
/// header makes every table extend one spacing step over the pane's bottom.
#[must_use]
pub fn table_body_height(ui: &Ui, pane_height: f32) -> f32 {
    (pane_height - theme::HEADER_HEIGHT - ui.spacing().item_spacing.y).max(0.0)
}

/// A scrolling reading pane with shared scrollbar clearance and bottom padding.
///
/// Dense tables manage their own scrolling; graphs, cards, pickers and
/// inspectors use this container so their content has the same inset.
pub fn scroll_body(ui: &mut Ui, contents: impl FnOnce(&mut Ui)) -> Rect {
    scroll_body_output(ui, contents).inner
}

/// The same reading pane with its actual content and viewport measurements.
pub(super) fn scroll_body_output(
    ui: &mut Ui,
    contents: impl FnOnce(&mut Ui),
) -> egui::scroll_area::ScrollAreaOutput<Rect> {
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.set_max_width((ui.available_width() - PAD).max(0.0));
            contents(ui);
            theme::gap(ui, PAD);
            ui.min_rect()
        })
}

/// Where a selection bar sits, which follows from what it marks.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Bar {
    /// A table row, which runs from one edge of its pane to the other.
    ///
    /// Square and the row's full height. The row has no corners of its
    /// own to round against, and a bar shorter than the row reads as
    /// marking something *inside* the row rather than the row itself.
    Flush,
    /// A rounded card: a rail entry, a picker tile, an adapter row.
    ///
    /// Inset and rounded, because a square bar drawn flush inside a
    /// rounded card pokes its corners out through the card's own.
    Inset,
}

/// The bar's corner radius in the [`Bar::Inset`] case.
///
/// Deliberately much tighter than [`RADIUS`]: it is a three-point bar,
/// and at the card's own radius it would be a lozenge.
const BAR_RADIUS: u8 = 2;

impl Bar {
    /// How far the bar is held back from the top and bottom.
    fn inset(self) -> f32 {
        match self {
            Self::Flush => 0.0,
            Self::Inset => SPACE_XS,
        }
    }

    /// Its corner radius.
    fn radius(self) -> CornerRadius {
        match self {
            Self::Flush => CornerRadius::ZERO,
            Self::Inset => CornerRadius::same(BAR_RADIUS),
        }
    }
}

/// Paints the bar down the leading edge of a selected row or card.
///
/// Every one in the app comes through here, and that is the point. There
/// were five, and no two agreed: three points wide or two, `SELECTION_BAR`
/// or a literal, inset or flush, rounded or square, animated in one place
/// and appearing at full height in the other four. Nobody chose that —
/// each was written next to the thing it marked, and each was reasonable
/// on its own.
///
/// It animates out from the centre rather than appearing whole, so moving
/// a selection down a list reads as one marker travelling rather than as
/// a series of unrelated flashes. **Key `id` on the thing selected**, not
/// on its position: an id built from a row index animates the *slot*, so
/// re-sorting a table hands every row the animation state of whatever
/// used to sit there.
pub fn accent_bar(
    ui: &Ui,
    painter: &egui::Painter,
    id: egui::Id,
    rect: Rect,
    bar: Bar,
    color: Rgb,
) {
    let grown = motion::toggle(ui.ctx(), id.with("selected"), true, motion::QUICK);
    let height = (rect.height() - bar.inset() * 2.0).max(0.0) * grown;
    let painted = Rect::from_center_size(
        egui::pos2(rect.left() + SELECTION_BAR / 2.0, rect.center().y),
        Vec2::new(SELECTION_BAR, height),
    );
    painter.rect_filled(painted, bar.radius(), theme::rgb(color));
}

/// The fill behind a selectable row or card.
///
/// One place, so "selected" looks the same in the rail, the process
/// tables, the Performance picker and the adapter list. A view that
/// reached for `theme.selection` itself got the colour right and the
/// hover ramp underneath it subtly wrong, every time.
#[must_use]
pub fn selection_fill(
    ui: &Ui,
    theme: &Palette,
    id: egui::Id,
    selected: bool,
    hovered: bool,
    rest: Rgb,
) -> Color32 {
    if selected {
        theme::rgb(theme.selection)
    } else {
        hover_fill(ui, id, hovered, rest, theme.hover)
    }
}

/// Hover and pressed fills for a selectable card or destination.
pub fn selectable_surface(
    ui: &Ui,
    theme: &Palette,
    id: egui::Id,
    response: &Response,
    selected: bool,
    rest: Rgb,
) -> Color32 {
    if response.is_pointer_button_down_on() {
        theme::rgb(theme.selection)
    } else {
        selection_fill(ui, theme, id, selected, response.hovered(), rest)
    }
}

/// A navigation-rail entry: icon, label, and an accent bar when active.
///
/// The selected fill and leading bar identify the current destination;
/// a focus outline distinguishes keyboard focus from selection.
pub fn nav_item(ui: &mut Ui, theme: &Palette, icon: Icon, label: &str, active: bool) -> Response {
    nav_destination(ui, theme, icon, label, active, false)
}

/// A destination with the same target and label in expanded and icon-only rails.
pub fn nav_destination(
    ui: &mut Ui,
    theme: &Palette,
    icon: Icon,
    label: &str,
    active: bool,
    compact: bool,
) -> Response {
    /// The rail entry's height. Taller than a table row — this is a
    /// primary destination, not a list item.
    const HEIGHT: f32 = theme::CONTROL_HEIGHT;

    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, HEIGHT), Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::Role::Button, ui.is_enabled(), active, label)
    });
    let painter = ui.painter();

    // Keyed on the label rather than on `response.id`, which an
    // `allocate_exact_size` derives from the slot: the rail is a fixed
    // list today, and would hand one destination another's animation
    // the day a view is added in the middle.
    //
    // Salted from the enclosing scope rather than rooted at the label,
    // because the rail is drawn twice while the hover flyout is open —
    // once narrow in the panel, once full-width over it. A label-rooted
    // id is the same id in both, so the two would write one hover
    // animation in the same frame and the destination under the pointer
    // would be told it is not hovered by the copy that is hidden behind
    // the flyout.
    let id = ui.scope_id().with(("nav", label));
    let fill = selectable_surface(ui, theme, id, &response, active, theme.panel);
    painter.rect_filled(rect, CornerRadius::same(RADIUS), fill);

    if active {
        accent_bar(ui, painter, id, rect, Bar::Inset, theme.accent);
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
    // Centred in a leading square the height of the row, whatever the
    // rail's width. A collapsed rail's entry *is* that square, so the
    // icon lands on the same x collapsed, unfolded and pinned open, and
    // the hover reveal uncovers labels beside icons that do not move.
    //
    // It used to sit in a column of its own, inset from the entry's
    // edge — which put it seven points further right in the wide rail
    // than in the narrow one, so every icon stepped sideways as the rail
    // unfolded. A fixed column still exists; it is this square, and every
    // label starts on its right edge however wide the icon draws.
    let leading = Rect::from_min_size(rect.min, Vec2::splat(HEIGHT));
    let icon_box = Rect::from_center_size(leading.center(), Vec2::splat(icons::NAV));
    icons::paint(painter, icon_box, icon, icon_colour);

    if !compact {
        painter.text(
            egui::pos2(leading.right() + SPACE_XS, rect.center().y),
            Align2::LEFT_CENTER,
            label,
            TextStyle::Body.resolve(ui.style()),
            text_color,
        );
    }

    focus_ring(ui, theme, &response);
    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
    if compact {
        response.on_hover_text(label)
    } else {
        response
    }
}

/// A stable, visible rail toggle; the chevron points toward the resulting width.
///
/// Two flags, because the two differ in the hover flyout: `compact` is
/// how this rail is being *drawn*, and `collapsed` is the saved state a
/// click will flip. The flyout draws the full rail while the preference
/// is still collapsed, so there the control pins the rail open rather
/// than collapsing one that is already collapsed — and the chevron and
/// the label both follow the action, not the drawing, so the glyph
/// cannot promise the opposite of what the tooltip says.
pub fn nav_toggle(ui: &mut Ui, theme: &Palette, compact: bool, collapsed: bool) -> Response {
    let label = if collapsed {
        "Expand sidebar"
    } else {
        "Collapse sidebar"
    };
    let (rect, response) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), theme::CONTROL_HEIGHT),
        Sense::click(),
    );
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, ui.is_enabled(), label));
    ui.painter().rect_filled(
        rect,
        CornerRadius::same(RADIUS),
        selectable_surface(ui, theme, response.id, &response, false, theme.panel),
    );
    // In the same leading square as every destination's icon, for the
    // same reason. This one moved furthest of anything in the rail: it
    // sat centred in the narrow rail and against the far edge of the wide
    // one, so unfolding the rail sent it a hundred-odd points across.
    let leading = Rect::from_min_size(rect.min, Vec2::splat(theme::CONTROL_HEIGHT));
    icons::paint_rotated(
        ui.painter(),
        Rect::from_center_size(leading.center(), Vec2::splat(icons::INLINE)),
        Icon::ChevronRight,
        theme::rgb(theme.text_muted),
        if collapsed { 0.0 } else { 0.5 },
    );
    if !compact {
        ui.painter().text(
            egui::pos2(leading.right() + SPACE_XS, rect.center().y),
            Align2::LEFT_CENTER,
            "Navigation",
            TextStyle::Small.resolve(ui.style()),
            theme::rgb(theme.text_muted),
        );
    }
    focus_ring(ui, theme, &response);
    response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(label)
}

/// The rail's note that the session is not elevated.
///
/// One widget at both widths, laid out like a destination — its glyph in
/// the leading square, its words beside it — rather than an icon in the
/// narrow rail and a chip in the wide one. Two widgets meant the hover
/// reveal swapped one for the other mid-unfold, so the only thing at the
/// foot of the rail that was not a destination changed shape as you
/// looked at it.
///
/// Not interactive, and drawn without a destination's hover fill or
/// pointer cursor, so it does not offer a click it cannot answer.
pub fn access_notice(ui: &mut Ui, theme: &Palette, compact: bool) -> Response {
    const LABEL: &str = "Limited access";
    let (rect, response) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), theme::CONTROL_HEIGHT),
        Sense::hover(),
    );
    let leading = Rect::from_min_size(rect.min, Vec2::splat(theme::CONTROL_HEIGHT));
    icons::paint(
        ui.painter(),
        Rect::from_center_size(leading.center(), Vec2::splat(icons::INLINE)),
        Icon::Info,
        theme::rgb(theme.text_muted),
    );
    if !compact {
        ui.painter().text(
            egui::pos2(leading.right() + SPACE_XS, rect.center().y),
            Align2::LEFT_CENTER,
            LABEL,
            TextStyle::Small.resolve(ui.style()),
            theme::rgb(theme.text_muted),
        );
    }
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, LABEL));
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
    let id = ui.scope_id().with("disclosure").with(id_source);
    let (rect, response) = ui.allocate_exact_size(
        Vec2::new(icons::DISCLOSURE + SPACE_XS, theme::ROW_HEIGHT),
        Sense::click(),
    );
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::Role::DisclosureTriangle,
            ui.is_enabled(),
            open,
            "Expand group",
        )
    });

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
    focus_ring(ui, theme, &response);
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
            theme::gap(ui, SPACE_MD);
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

/// A cell's text, elided to the room the cell has left less `reserved`.
///
/// For the leading text of a row that carries something *after* it — a
/// chip, a mark. A `ui.label` takes the whole cell, elided or not, and
/// whatever is laid out after it lands past the cell's edge and is cut
/// mid-word: "All users" read "All user" beside any startup entry with a
/// long enough name. A truncated *name* still identifies its row, because
/// the beginning of a name is the part that does; a truncated chip
/// identifies nothing and reads as a rendering fault.
///
/// `reserved` is what the trailing things cost, and that is more than
/// their own widths: the layout puts `item_spacing.x` between any two
/// items it allocates, on top of whatever `add_space` the caller puts
/// there itself. The first reservation counted the `add_space` and not
/// the spacing, and the chip went over the edge by exactly that much.
pub fn elided_text(ui: &mut Ui, text: &str, color: Rgb, reserved: f32) -> Response {
    let width = (ui.available_width() - reserved).max(0.0);
    let font = TextStyle::Body.resolve(ui.style());
    let galley = truncated(ui, text, font, color, width);
    let (rect, response) = ui.allocate_exact_size(galley.size(), Sense::hover());
    ui.painter()
        .galley(rect.left_top(), galley, theme::rgb(color));
    response
}

/// How wide [`status_chip`] will draw for this text.
///
/// So a caller can reserve the room before laying out whatever precedes
/// the chip — a label that consumed the whole cell first left the chip
/// to be clipped in half by the column. See [`elided_text`], and note
/// the layout's own item spacing is not in this figure.
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
pub struct Heading<'a> {
    /// The column's name.
    pub label: &'a str,
    /// The glyph before it, if the column has earned one and can afford
    /// the width. See [`crate::model::sort::SortKey::mark`].
    pub mark: Option<Icon>,
    /// `Some(descending)` when this is the column being sorted on.
    pub sorted: Option<bool>,
    /// Whether the heading claims its cell's whole width.
    pub claims_width: bool,
    /// Whether the column's values are right-aligned, which the heading
    /// follows so a number sits under its own name.
    pub right_aligned: bool,
    /// Whether a drag is carrying this heading.
    pub lifted: bool,
    /// Matches the leading inset in the name cell, independently of column order.
    pub leading_inset: bool,
}

/// A column heading: its name, its sort arrow, and the click and drag
/// that sort and reorder it.
///
/// Takes a [`Heading`] rather than seven arguments. Four of them are
/// booleans, and four booleans in a row is a call nobody can read at the
/// call site — `false, index > 0, false` says nothing about which
/// question each answers.
pub fn sortable_header(ui: &mut Ui, theme: &Palette, heading: Heading<'_>) -> Response {
    let Heading {
        label,
        mark,
        sorted,
        claims_width,
        right_aligned,
        lifted,
        leading_inset,
    } = heading;
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

    // The mark is an enhancement and is dropped when the column cannot
    // afford it, which is the same rule the Startup toolbar's note
    // follows. That is not fastidiousness: these tables are already
    // wider than the pane at the smallest window the app allows — the
    // Details table wants about 838 points against 588 — and a mark on
    // every heading would add sixteen points a column and undo the work
    // that made them scroll instead of losing a column. An enhancement
    // that costs a column is not an enhancement.
    let mark_column = icons::DISCLOSURE + SPACE_XS;
    let needed = galley.size().x + SPACE_SM + arrow_column + mark_column;
    let mark = mark.filter(|_| needed <= cell.width());

    let inset = if leading_inset { SPACE_SM } else { 0.0 };
    let leading = if mark.is_some() { mark_column } else { 0.0 } + inset;
    let height = cell.height().min(theme::HEADER_HEIGHT);
    let wanted = if claims_width {
        Vec2::new(cell.width(), height)
    } else {
        Vec2::new(galley.size().x + SPACE_SM + arrow_column + leading, height)
    };
    // `click_and_drag`, so the same control sorts on a click and reorders
    // on a drag. egui reports `clicked()` as false once the pointer has
    // moved far enough to count as a drag, so the two cannot both fire —
    // which is what lets a heading be its own drag handle rather than
    // needing a separate grip beside it.
    let (_, allocation) = ui.allocate_exact_size(wanted, Sense::hover());
    let rect = Rect::from_min_size(cell.min, Vec2::new(cell.width(), height));
    let response = ui.interact(rect, allocation.id, super::dnd::Lane::sense());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, ui.is_enabled(), label));
    ui.painter().rect_filled(
        rect,
        CornerRadius::same(RADIUS),
        selectable_surface(ui, theme, response.id, &response, false, theme.app),
    );

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
            rect.left() + leading,
            rect.left() + leading + text_width + SPACE_XS + icons::DISCLOSURE / 2.0,
        )
    };

    // Ahead of the label in a left-aligned heading, behind it in a
    // right-aligned one — the same side the text starts from, so the
    // heading reads as one object rather than as a glyph and a word that
    // happen to share a cell.
    if let Some(mark) = mark {
        let centre = if right_aligned {
            rect.right() - text_width - SPACE_XS - arrow_column - mark_column / 2.0
        } else {
            rect.left() + mark_column / 2.0
        };
        let box_ = Rect::from_center_size(
            egui::pos2(centre, rect.center().y),
            Vec2::splat(icons::DISCLOSURE),
        );
        icons::paint(ui.painter(), box_, mark, theme::rgb(color));
    }
    ui.painter().galley(
        egui::pos2(text_left, rect.center().y - galley.size().y / 2.0),
        galley,
        theme::rgb(color),
    );

    if sorted.is_some() || response.hovered() || response.has_focus() {
        // The arrow rotates between the two directions rather than being
        // swapped, so a click on an already-sorted heading reads as
        // "this reversed" rather than as the heading being redrawn.
        let flip = motion::toggle(
            ui.ctx(),
            response.id.with("sort"),
            sorted.unwrap_or(false),
            motion::QUICK,
        );
        let icon = if sorted.is_none() {
            Icon::ChevronDown
        } else if flip > 0.5 {
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
            theme::rgb(if sorted.is_some() {
                theme.accent
            } else {
                theme.text_muted
            }),
        );
    }
    focus_ring(ui, theme, &response);
    response.on_hover_text(match sorted {
        Some(true) => "Sorted descending. Click to sort ascending.",
        Some(false) => "Sorted ascending. Click to sort descending.",
        None => "Click to sort this column.",
    })
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
            .font(theme::section_font(ui)),
    );
    theme::gap(ui, SPACE_SM);
    section_rule(ui, theme);
    theme::gap(ui, SPACE_MD);
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
        theme::display_font(ui),
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
    // The viewport, widened by the same half-spacing the cell rects are.
    //
    // The widening is needed: a table that takes its viewport from
    // `available_rect_before_wrap` gets one whose left edge is the first
    // cell's left edge, and `egui_extras`' own hover fill is expanded
    // half a spacing past that — so clipping ours to the viewport exactly
    // left four points of the *control* colour showing down the left of
    // every hovered row in the Services list. Four points of grey, only
    // while hovered, only on two of the four tables.
    //
    // But it is widened by a *bound*, not to whatever the cell happens
    // to be. Taking the union with the cell rect makes the clip as wide
    // as the widest cell, which is no clip at all once a table is wider
    // than its pane: the Details table's last columns then paint their
    // row backgrounds straight across the inspector beside it. This rect
    // is the only thing holding them, because the fill is painted
    // through `set_clip_rect`, which *replaces* the clip in force rather
    // than intersecting it.
    //
    // Horizontally that widening is the whole purpose. *Vertically*
    // there must be none of it, and this is the half that was missing:
    // the extent came from the cell, so a row only half inside the
    // scroll viewport — which is every list scrolled anywhere but the
    // top — was clipped to its own full height and painted straight
    // through the bottom of the table. `set_clip_rect` replaces the
    // clip in force, so the scroll area's own was simply gone. The
    // result is a list that visibly runs past its own last row and into
    // the status bar, and it only appears once there are enough rows to
    // scroll, which is why no fabricated scene showed it.
    //
    // So the vertical extent is intersected back with the table body's
    // clip — the scroll viewport — which `viewport` carries; see
    // [`RowViewport`].
    //
    // It is *not* taken from `ui.clip_rect()`, which is what this did
    // first. That is the body's clip only in a column without `.clip(true)`:
    // in one with it, `egui_extras` shrinks the cell ui's clip to the cell
    // itself, and intersecting with that trimmed the half-spacing
    // overhang back off. So every clipped column drew its rows with a
    // strip of the app's background between them, while the columns
    // that do not clip — Status, in the Startup and Services lists —
    // kept their overhang, and their bands ran past the gaps the rest of
    // the row left and out of line with them. One row, two shapes,
    // decided by a flag on the column.
    let bleed = 0.5 * ui.spacing().item_spacing.x + 1.0;
    Rect::from_min_max(
        egui::pos2(viewport.left() - bleed, gapless.top().max(viewport.top())),
        egui::pos2(
            viewport.right() + bleed,
            gapless.bottom().min(viewport.bottom()),
        ),
    )
}

/// The rect a table's rows may paint in: the pane's width, and the
/// visible height of the table's *body*.
///
/// Only obtainable from a [`TableBody`], because the vertical half is
/// that body's clip and nothing else carries it. A cell's own clip does
/// not — in a `.clip(true)` column it is the cell — and the pane the view
/// measured includes the header, so a row half scrolled under the
/// heading would paint its fill over the heading's text. See
/// [`row_clip`].
#[derive(Clone, Copy)]
pub struct RowViewport(Rect);

impl RowViewport {
    /// The rows' viewport, from the pane the view measured and the body
    /// the rows are about to be drawn into.
    ///
    /// `pane` supplies the horizontal extent — the region a row's fill
    /// reaches across, which inside a horizontally scrolling table is the
    /// visible pane rather than the content. See [`row_clip`].
    pub fn new(pane: Rect, body: &mut TableBody<'_>) -> Self {
        Self(Rect::from_x_y_ranges(
            pane.x_range(),
            body.ui_mut().clip_rect().y_range(),
        ))
    }
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
    accent: Option<Rgb>,
) {
    let row = row_rect(ui, viewport);
    // Asked of the context rather than of the `Ui`. `Ui::rect_contains_pointer`
    // intersects the rect with the ui's own clip rect first, and a table
    // cell's clip rect is the cell — so a row-wide rect came back clipped
    // to one cell, and only the cell actually under the pointer lit up
    // while the other seven stayed at rest.
    let hovered = ui.ctx().rect_contains_pointer(ui.layer_id(), row);

    let mut base = if striped { theme.raised } else { theme.panel };
    if let Some(accent) = accent {
        base = base.lerp(accent, 0.07);
    }
    let fill = selection_fill(ui, theme, id, selected, hovered, base);
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
    if ui.max_rect().left() <= viewport.left() + 0.5 {
        // Selection outranks the identity hue: a row that is both
        // selected and accented is being *pointed at*, and the hue is
        // then the less urgent of the two things the bar could say.
        if selected {
            accent_bar(ui, &painter, id, row, Bar::Flush, theme.accent);
        } else if let Some(accent) = accent {
            accent_bar(ui, &painter, id, row, Bar::Flush, accent);
        }
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
        accent: Option<Rgb>,
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
    painter: Option<egui::Painter>,
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
        viewport: RowViewport,
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
            viewport: viewport.0,
            fill: RowFill::Record {
                id,
                selected,
                striped,
                accent: None,
            },
            painter: None,
        }
    }

    /// A record row carrying the executable icon's representative colour.
    pub fn accented(
        row: &'r mut TableRow<'a, 'b>,
        theme: &'r Palette,
        viewport: RowViewport,
        id: egui::Id,
        selected: bool,
        striped: bool,
        accent: Option<Rgb>,
    ) -> Self {
        let mut record = Self::record(row, theme, viewport, id, selected, striped);
        record.fill = RowFill::Record {
            id,
            selected,
            striped,
            accent,
        };
        record
    }

    /// A grouping row — a category heading, not a record.
    pub fn group(row: &'r mut TableRow<'a, 'b>, theme: &'r Palette, viewport: RowViewport) -> Self {
        row.set_selected(false);
        Self {
            row,
            theme,
            viewport: viewport.0,
            fill: RowFill::Group,
            painter: None,
        }
    }

    /// Adds a cell, with the row's background already painted behind it.
    pub fn cell(&mut self, contents: impl FnOnce(&mut Ui)) {
        // Copied out before the call: `TableRow::col` borrows the row
        // mutably, so the closure cannot read `self` while it runs.
        let (theme, viewport, fill) = (self.theme, self.viewport, self.fill);
        let painter = &mut self.painter;
        self.row.col(|ui| {
            if painter.is_none() {
                let mut clipped = ui.painter().clone();
                clipped.set_clip_rect(row_clip(ui, viewport));
                *painter = Some(clipped);
            }
            match fill {
                RowFill::Record {
                    id,
                    selected,
                    striped,
                    accent,
                } => row_background(ui, theme, viewport, id, selected, striped, accent),
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
        let response = self.row.response();
        if response.has_focus() {
            if let Some(painter) = &self.painter {
                paint_focus_ring(
                    painter,
                    self.theme,
                    response.rect.intersect(self.viewport),
                    CornerRadius::ZERO,
                );
            }
        }
        response
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

/// Alignment follows the meaning of a column, including after reordering.
#[must_use]
pub fn numeric_column(key: crate::model::sort::SortKey) -> bool {
    use crate::model::sort::SortKey;
    match key {
        SortKey::Pid
        | SortKey::Cpu
        | SortKey::Memory
        | SortKey::Disk
        | SortKey::Network
        | SortKey::Gpu
        | SortKey::PrivateBytes
        | SortKey::Threads
        | SortKey::Handles
        | SortKey::CpuTime
        | SortKey::Session => true,
        SortKey::Name
        | SortKey::Status
        | SortKey::User
        | SortKey::Priority
        | SortKey::Architecture
        | SortKey::Path => false,
    }
}

/// A text cell shares the numeric cell's center baseline, with readable elision.
pub fn text_cell(ui: &mut Ui, theme: &Palette, text: &str, muted: bool) {
    let color = if muted { theme.text_muted } else { theme.text };
    elided_text(ui, text, color, 0.0).on_hover_text(text);
}

/// A borderless icon button, for the title bar and toolbars.
pub fn icon_button(ui: &mut Ui, theme: &Palette, icon: Icon, tooltip: &str) -> Response {
    /// The button's square size. Matches the title bar's own height less
    /// its padding.
    const SIZE: f32 = theme::CONTROL_HEIGHT;

    let (rect, response) = ui.allocate_exact_size(Vec2::new(SIZE, SIZE), Sense::click());
    // Not the right edge: the close button is the outermost control, and
    // stretching these to an edge they do not touch would have them
    // answering clicks over their neighbour.
    let response = ui.interact(
        caption_hit_area(ui, rect, false),
        response.id,
        Sense::click(),
    );
    response
        .widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, ui.is_enabled(), tooltip));
    let fill = selectable_surface(ui, theme, response.id, &response, false, theme.app);
    ui.painter()
        .rect_filled(rect, CornerRadius::same(RADIUS), fill);
    icons::paint(
        ui.painter(),
        Rect::from_center_size(rect.center(), Vec2::splat(icons::INLINE)),
        icon,
        theme::rgb(theme.text),
    );
    focus_ring(ui, theme, &response);
    response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(tooltip)
}

/// A caption control's hit area, closed up to any window edge it sits
/// against.
///
/// Fitts's law, and the corner is the whole of the point: a pointer
/// cannot travel past the edge of the screen, so on a maximised window
/// the top-right corner is a target of effectively infinite size — it is
/// hit by throwing the pointer at it, without aiming, which is how the
/// close button is used. Inset from that corner by even a few points, the
/// corner becomes dead space: the throw lands on nothing and the button
/// has to be aimed at like any other. Every system's own caption buttons
/// run to the edge, and that is why.
///
/// The title bar earns its padding elsewhere, so only the *hit area*
/// grows. The fill and the glyph stay where the layout put them, which
/// is what keeps the bar looking padded while behaving as though it is
/// not.
///
/// Which edges to close up is stated by the caller rather than guessed
/// from how far the control sits from them. A distance test looks
/// tidier and is wrong: the gap to the right edge is the title bar's
/// padding *plus* the layout's own item spacing, and the first attempt
/// at this measured twelve points where the real gap was twenty, so the
/// close button kept a dead corner while appearing to be fixed. The
/// caller knows which control is the outermost one; the control cannot.
///
/// Both of this module's caption controls sit in the title bar and
/// nowhere else, so the top edge is always theirs to take.
fn caption_hit_area(ui: &Ui, painted: Rect, to_right_edge: bool) -> Rect {
    // `content_rect`, the same rectangle `chrome::resize_handles`
    // measures its border against, so the two agree on where the window
    // ends.
    let window = ui.ctx().content_rect();
    let mut hit = painted;
    hit.min.y = window.top();
    if to_right_edge {
        hit.max.x = window.right();
    }
    hit
}

/// The window's close button, which highlights in the danger colour.
///
/// Separate from [`icon_button`] because it is the one control in the app
/// whose hover colour is a warning rather than a lift — every window on
/// Windows does this, and one that did not would be the odd one out in a
/// way people notice without being able to say why.
pub fn close_button(ui: &mut Ui, theme: &Palette) -> Response {
    const SIZE: f32 = theme::CONTROL_HEIGHT;
    let (rect, response) = ui.allocate_exact_size(Vec2::new(SIZE, SIZE), Sense::click());
    // The outermost control in the caption, so this one takes the
    // corner — which is the whole reason any of this is here.
    let response = ui.interact(
        caption_hit_area(ui, rect, true),
        response.id,
        Sense::click(),
    );
    response
        .widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, ui.is_enabled(), "Close"));
    let fill = hover_fill(ui, response.id, response.hovered(), theme.app, theme.danger);
    ui.painter()
        .rect_filled(rect, CornerRadius::same(RADIUS), fill);
    // The mark flips to the on-accent colour as the danger fill comes
    // up, or it goes unreadable against it at the end of the ramp.
    let t = hover_t(ui, response.id, response.hovered());
    let mark = theme.text.lerp(theme.text_on_danger, t);
    icons::paint(
        ui.painter(),
        Rect::from_center_size(rect.center(), Vec2::splat(icons::INLINE)),
        Icon::Close,
        theme::rgb(mark),
    );
    focus_ring(ui, theme, &response);
    response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text("Close")
}

#[derive(Clone, Copy)]
enum ButtonTone {
    Primary,
    Secondary,
    Danger,
    Selected,
}

/// One implementation owns action and choice controls in every view.
fn button(
    ui: &mut Ui,
    theme: &Palette,
    label: &str,
    tone: ButtonTone,
    enabled: bool,
    selected: Option<bool>,
) -> Response {
    let enabled = enabled && ui.is_enabled();
    let (rest, hover, pressed, ink, border) = if enabled {
        match tone {
            ButtonTone::Primary => (
                theme.accent,
                theme.accent_hover,
                theme.accent,
                theme.text_on_accent,
                theme.accent,
            ),
            ButtonTone::Secondary => (
                theme.raised,
                theme.hover,
                theme.selection,
                theme.text,
                theme.border,
            ),
            ButtonTone::Danger => (
                theme.danger,
                theme.danger_hover,
                theme.danger,
                theme.text_on_danger,
                theme.danger,
            ),
            ButtonTone::Selected => (
                theme.selection,
                theme.hover,
                theme.selection,
                theme.text,
                theme.accent,
            ),
        }
    } else {
        (
            theme.panel,
            theme.panel,
            theme.panel,
            theme.text_faint,
            theme.border,
        )
    };
    let font = TextStyle::Button.resolve(ui.style());
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_string(), font, theme::rgb(ink));
    let size = Vec2::new(galley.size().x + SPACE_MD * 2.0, theme::CONTROL_HEIGHT);
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(size, sense);
    response.widget_info(|| match selected {
        Some(selected) => egui::WidgetInfo::selected(egui::Role::Button, enabled, selected, label),
        None => egui::WidgetInfo::labeled(egui::Role::Button, enabled, label),
    });
    let fill = if response.is_pointer_button_down_on() {
        theme::rgb(pressed)
    } else {
        hover_fill(ui, response.id, response.hovered() && enabled, rest, hover)
    };
    ui.painter()
        .rect_filled(rect, CornerRadius::same(RADIUS), fill);
    ui.painter().rect_stroke(
        rect,
        CornerRadius::same(RADIUS),
        Stroke::new(
            1.0,
            theme::rgb(
                if response.hovered() && enabled && matches!(tone, ButtonTone::Secondary) {
                    theme.border_strong
                } else {
                    border
                },
            ),
        ),
        StrokeKind::Inside,
    );
    ui.painter()
        .galley(rect.center() - galley.size() / 2.0, galley, theme::rgb(ink));
    if enabled {
        focus_ring(ui, theme, &response);
        response.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        response
    }
}

/// The main, accent-filled action.
pub fn primary_button(ui: &mut Ui, theme: &Palette, label: &str) -> Response {
    button(ui, theme, label, ButtonTone::Primary, true, None)
}

/// A supporting action with a neutral surface and border.
pub fn secondary_button(ui: &mut Ui, theme: &Palette, label: &str) -> Response {
    button(ui, theme, label, ButtonTone::Secondary, true, None)
}

/// A destructive action, disabled when it cannot currently be performed.
pub fn danger_button(ui: &mut Ui, theme: &Palette, label: &str, enabled: bool) -> Response {
    button(ui, theme, label, ButtonTone::Danger, enabled, None)
}

/// A selected or unselected view option with full button interaction feedback.
pub fn choice_button(ui: &mut Ui, theme: &Palette, label: &str, selected: bool) -> Response {
    button(
        ui,
        theme,
        label,
        if selected {
            ButtonTone::Selected
        } else {
            ButtonTone::Secondary
        },
        true,
        Some(selected),
    )
}

/// Readable supporting prose with a bounded line length, distinct from metadata.
pub fn prose(ui: &mut Ui, text: impl Into<String>, color: Rgb) -> Response {
    const MEASURE: f32 = 600.0;
    ui.scope(|ui| {
        ui.set_max_width(ui.available_width().min(MEASURE));
        ui.label(
            egui::RichText::new(text)
                .color(theme::rgb(color))
                .text_style(TextStyle::Body),
        )
    })
    .inner
}

/// An empty-state message, centred in the space available.
///
/// A view with nothing in it has to say so. A blank pane is
/// indistinguishable from one that failed to load, and the difference
/// matters most on the views — Services, Startup — where an empty result
/// can mean "access denied" rather than "nothing here".
pub fn empty_state(ui: &mut Ui, theme: &Palette, message: &str) {
    let rect = ui.available_rect_before_wrap().intersect(ui.clip_rect());
    empty_state_in(ui, theme, rect, message);
    // Consume the space so a caller that draws after this does not
    // overlap the message.
    ui.allocate_space(rect.size());
}

/// A wrapped, centered message inside an already allocated chart or map.
pub fn empty_state_in(ui: &Ui, theme: &Palette, rect: Rect, message: &str) {
    let width = (rect.width() - PAD * 2.0).clamp(0.0, 480.0);
    let galley = ui.painter().layout(
        message.to_owned(),
        TextStyle::Body.resolve(ui.style()),
        theme::rgb(theme.text_muted),
        width,
    );
    ui.painter()
        .with_clip_rect(rect.intersect(ui.clip_rect()))
        .galley(
            rect.center() - galley.size() / 2.0,
            galley,
            theme::rgb(theme.text_muted),
        );
}

/// The same recovery path for every filtered table. Returns whether to clear search.
pub fn no_matches(ui: &mut Ui, theme: &Palette, items: &str) -> bool {
    let rect = ui.available_rect_before_wrap().intersect(ui.clip_rect());
    let width = (rect.width() - PAD * 2.0).clamp(0.0, 400.0);
    let block = Rect::from_center_size(rect.center(), Vec2::new(width, 112.0));
    ui.scope_builder(
        egui::UiBuilder::new()
            .max_rect(block)
            .layout(egui::Layout::top_down(Align::Center)),
        |ui| {
            ui.label(
                egui::RichText::new(format!("No matching {items}"))
                    .font(theme::section_font(ui))
                    .color(theme::rgb(theme.text)),
            );
            ui.add(
                egui::Label::new(
                    egui::RichText::new("Try another search or clear the current filter.")
                        .color(theme::rgb(theme.text_muted)),
                )
                .wrap(),
            );
            theme::gap(ui, SPACE_MD);
            secondary_button(ui, theme, "Clear filter").clicked()
        },
    )
    .inner
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
    let galley = ui.painter().layout(
        message.to_owned(),
        TextStyle::Body.resolve(ui.style()),
        theme::rgb(theme.text_muted),
        (ui.available_width() - PAD * 2.0).max(0.0),
    );
    let height = galley.size().y + SPACE_SM * 2.0;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), height), Sense::hover());
    ui.painter().galley(
        rect.center() - galley.size() / 2.0,
        galley,
        theme::rgb(theme.text_muted),
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
    #[test]
    fn numeric_headings_align_to_the_column_and_sort_from_its_empty_space() -> anyhow::Result<()> {
        let palette = crate::theme::Catalog::load().get(None).clone();
        let ctx = egui::Context::default();
        theme::apply(&ctx, &palette);
        let cell = egui::Rect::from_min_size(
            egui::pos2(20.0, 20.0),
            egui::vec2(300.0, theme::HEADER_HEIGHT),
        );
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            ui.scope_builder(egui::UiBuilder::new().max_rect(cell), |ui| {
                super::sortable_header(
                    ui,
                    &palette,
                    super::Heading {
                        label: "CPU",
                        mark: None,
                        sorted: Some(true),
                        claims_width: false,
                        right_aligned: true,
                        lifted: false,
                        leading_inset: false,
                    },
                );
            });
        });
        output.textures_delta.clear();
        let right = output
            .shapes
            .iter()
            .find_map(|clipped| {
                if let egui::Shape::Text(text) = &clipped.shape {
                    (text.galley.text() == "CPU").then_some(text.pos.x + text.galley.size().x)
                } else {
                    None
                }
            })
            .ok_or_else(|| anyhow::anyhow!("no CPU heading drawn"))?;
        assert!(
            (right - cell.right()).abs() < 0.1,
            "heading stops at {right}, column at {}",
            cell.right()
        );

        let mut harness = egui_kittest::Harness::new_ui_state(
            |ui, clicks: &mut usize| {
                ui.set_width(300.0);
                if super::sortable_header(
                    ui,
                    &palette,
                    super::Heading {
                        label: "CPU",
                        mark: None,
                        sorted: Some(true),
                        claims_width: false,
                        right_aligned: true,
                        lifted: false,
                        leading_inset: false,
                    },
                )
                .clicked()
                {
                    *clicks += 1;
                }
            },
            0,
        );
        harness.run();
        assert!(harness.get_by_label("CPU").rect().width() >= 299.0);
        harness.get_by_label("CPU").click();
        harness.run();
        assert_eq!(
            *harness.state(),
            1,
            "the middle of the column must sort even when the label is at the right edge"
        );
        Ok(())
    }

    #[test]
    fn a_no_results_state_offers_keyboard_recovery() {
        let mut harness = egui_kittest::Harness::new_ui_state(
            |ui, cleared: &mut bool| {
                let palette = crate::theme::Catalog::load().get(None).clone();
                *cleared |= super::no_matches(ui, &palette, "services");
            },
            false,
        );
        harness.run();
        harness.get_by_label("Clear filter").focus();
        harness.run();
        harness.key_press(egui::Key::Enter);
        harness.run();
        assert!(*harness.state());
    }

    use super::card_grid_layout;
    use super::table_body_height;
    use super::theme;
    use super::theme::SPACE_MD;
    use egui_kittest::kittest::Queryable;

    #[test]
    fn action_buttons_support_keyboard_activation_and_respect_disabled_state() {
        let mut harness = egui_kittest::Harness::new_ui_state(
            |ui, counts: &mut [usize; 3]| {
                let palette = crate::theme::Catalog::load().get(None).clone();
                theme::apply(ui.ctx(), &palette);
                if super::primary_button(ui, &palette, "Start").clicked() {
                    counts[0] += 1;
                }
                if super::secondary_button(ui, &palette, "Copy").clicked() {
                    counts[1] += 1;
                }
                if super::danger_button(ui, &palette, "End task", false).clicked() {
                    counts[2] += 1;
                }
            },
            [0; 3],
        );
        harness.run();
        harness.get_by_label("Start").focus();
        harness.run();
        harness.key_press(egui::Key::Enter);
        harness.run();
        assert_eq!(
            harness.state()[0],
            1,
            "Enter must activate the focused primary action"
        );
        harness.get_by_label("Copy").focus();
        harness.run();
        harness.key_press(egui::Key::Space);
        harness.run();
        assert_eq!(
            harness.state()[1],
            1,
            "Space must activate the focused secondary action"
        );
        harness.get_by_label("End task").click();
        harness.run();
        assert_eq!(
            harness.state()[2],
            0,
            "a disabled destructive action must never fire"
        );
    }

    #[test]
    fn choice_controls_are_keyboard_operable() {
        let mut harness = egui_kittest::Harness::new_ui_state(
            |ui, selected: &mut bool| {
                let palette = crate::theme::Catalog::load().get(None).clone();
                theme::apply(ui.ctx(), &palette);
                if super::choice_button(ui, &palette, "Tree", *selected).clicked() {
                    *selected = !*selected;
                }
            },
            false,
        );
        harness.run();
        harness.get_by_label("Tree").focus();
        harness.run();
        harness.key_press(egui::Key::Space);
        harness.run();
        assert!(*harness.state(), "Space must select the option");
        harness.key_press(egui::Key::Enter);
        harness.run();
        assert!(
            !*harness.state(),
            "Enter must toggle the selected option again"
        );
    }

    #[test]
    fn rendered_card_gutters_are_equal_in_both_axes() -> anyhow::Result<()> {
        for scale in [1.0, 1.25, 1.5] {
            let ctx = egui::Context::default();
            ctx.set_pixels_per_point(scale);
            let mut cards = Vec::new();
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(460.0, 300.0),
                    )),
                    ..Default::default()
                },
                |ui| {
                    ui.set_width(460.0);
                    ui.spacing_mut().item_spacing = egui::vec2(theme::SPACE_SM, theme::SPACE_XS);
                    super::card_grid(ui, 200.0, 4, |ui, _| {
                        cards.push(
                            ui.allocate_exact_size(
                                egui::vec2(ui.available_width(), 40.0),
                                egui::Sense::hover(),
                            )
                            .0,
                        );
                    });
                },
            );
            output.textures_delta.clear();
            let [first, second, third, _] = cards.as_slice() else {
                anyhow::bail!("the grid did not draw four cards");
            };
            let horizontal = second.left() - first.right();
            let vertical = third.top() - first.bottom();
            assert!(
                (horizontal - SPACE_MD).abs() < 0.1,
                "horizontal gutter is {horizontal} at {scale}x"
            );
            assert!(
                (vertical - horizontal).abs() < 0.1,
                "vertical gutter {vertical} differs from horizontal {horizontal} at {scale}x"
            );
        }
        Ok(())
    }

    #[test]
    fn a_real_table_header_and_body_end_inside_the_pane() -> anyhow::Result<()> {
        use egui::{Rect, Vec2};
        use egui_extras::{Column, TableBuilder};

        let ctx = egui::Context::default();
        let window = Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(780.0, 480.0));
        let mut bounds = None;
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(window),
                ..Default::default()
            },
            |ui| {
                let pane = ui.available_rect_before_wrap();
                let body_height = table_body_height(ui, pane.height());
                let body = TableBuilder::new(ui)
                    .vscroll(true)
                    .min_scrolled_height(0.0)
                    .max_scroll_height(body_height)
                    .auto_shrink([true, false])
                    .column(Column::remainder())
                    .header(theme::HEADER_HEIGHT, |mut header| {
                        header.col(|ui| {
                            ui.label("Name");
                        });
                    })
                    .body(|body| {
                        body.rows(theme::ROW_HEIGHT, 100, |mut row| {
                            row.col(|ui| {
                                ui.label("process.exe");
                            });
                        });
                    });
                bounds = Some((pane, body.inner_rect));
            },
        );
        output.textures_delta.clear();
        let (pane, body) = bounds.ok_or_else(|| anyhow::anyhow!("table did not draw"))?;
        assert!(
            body.bottom() <= pane.bottom() + 0.01,
            "table body ends at {}, past pane bottom {}",
            body.bottom(),
            pane.bottom()
        );
        Ok(())
    }

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
                    row_background(ui, &theme, row, egui::Id::unique("row"), true, false, None);
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

    #[test]
    fn a_clipped_and_an_unclipped_column_paint_the_same_rows() -> anyhow::Result<()> {
        // Every table here mixes the two: most columns are `.clip(true)`,
        // and a column of chips — Status, in the Startup and Services
        // lists — is not. `egui_extras` shrinks a clipped cell's ui clip
        // to the cell, and `row_clip` used to take its vertical bound from
        // that clip, so the clipped columns trimmed the half-spacing
        // overhang off every row while the Status column kept it. The
        // result was a strip of background between every row in all but
        // one column, and in that one, bands running past the strips and
        // out of line with them.
        //
        // A real `TableBuilder` rather than hand-placed cells, because
        // the clip in question is the one `egui_extras` sets up, and a
        // test that builds its own cells decides the answer for it.
        use super::super::theme;
        use super::{Row, RowViewport};
        use egui::{Rect, Shape, Vec2};
        use egui_extras::{Column, TableBuilder};

        /// Both columns' width.
        const CELL: f32 = 160.0;
        /// More than a row, its overhang and the slop on both edges —
        /// anything taller is not a row's fill.
        const TALLEST_FILL: f32 = theme::ROW_HEIGHT + 2.0 * theme::SPACE_MD;

        let palette = crate::theme::Catalog::load().get(None).clone();
        let ctx = egui::Context::default();
        theme::apply(&ctx, &palette);
        let window = Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(800.0, 400.0));
        let mut boundary = None;
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(window),
                ..Default::default()
            },
            |ui| {
                let pane = ui.available_rect_before_wrap();
                ui.set_clip_rect(pane);
                boundary = Some(pane.left() + CELL + 0.5 * ui.spacing().item_spacing.x);
                let body_height = table_body_height(ui, pane.height());
                TableBuilder::new(ui)
                    .vscroll(true)
                    .min_scrolled_height(0.0)
                    .max_scroll_height(body_height)
                    .auto_shrink([true, false])
                    .column(Column::initial(CELL).clip(true))
                    .column(Column::initial(CELL).clip(false))
                    .header(theme::HEADER_HEIGHT, |mut header| {
                        header.col(|_| {});
                        header.col(|_| {});
                    })
                    .body(|mut body| {
                        let viewport = RowViewport::new(pane, &mut body);
                        body.rows(theme::ROW_HEIGHT, 4, |mut row| {
                            let index = row.index();
                            let mut row = Row::record(
                                &mut row,
                                &palette,
                                viewport,
                                egui::Id::unique("row").with(index),
                                false,
                                index % 2 == 1,
                            );
                            row.cell(|_| {});
                            row.cell(|_| {});
                        });
                    });
            },
        );
        output.textures_delta.clear();
        let boundary = boundary.ok_or_else(|| anyhow::anyhow!("nothing was drawn"))?;

        // Each column's fills, as their vertical spans once clipped.
        let mut columns: [Vec<(f32, f32)>; 2] = [Vec::new(), Vec::new()];
        for clipped in &output.shapes {
            let Shape::Rect(rect) = &clipped.shape else {
                continue;
            };
            let painted = rect.rect.intersect(clipped.clip_rect);
            if painted.width() > 1.0 && painted.height() > 1.0 && rect.rect.height() < TALLEST_FILL
            {
                let column = usize::from(painted.center().x > boundary);
                if let Some(spans) = columns.get_mut(column) {
                    spans.push((painted.top(), painted.bottom()));
                }
            }
        }
        for spans in &mut columns {
            spans.sort_by(|a, b| a.0.total_cmp(&b.0));
        }
        let [clipped, unclipped] = columns;

        assert_eq!(
            clipped.len(),
            4,
            "expected one fill per row in the clipped column, found {clipped:?}"
        );
        assert_eq!(
            clipped, unclipped,
            "a row's fill spans {clipped:?} in a clipped column and {unclipped:?} in an \
             unclipped one — the bands do not line up across the row"
        );
        for pair in clipped.windows(2) {
            if let [above, below] = pair {
                assert!(
                    above.1 >= below.0,
                    "a row's fill ends at {} and the next begins at {} — the background \
                     shows between them",
                    above.1,
                    below.0
                );
            }
        }
        Ok(())
    }

    #[test]
    fn a_long_name_leaves_its_chip_whole() -> anyhow::Result<()> {
        // The Startup list's Name cell, as it is drawn: a name elided to
        // the room its chip leaves, then the chip. The chip is the part
        // that must survive intact — "All user" beside a long name reads
        // as a rendering fault, where an elided name reads as a long one.
        //
        // The reservation has to count the layout's item spacing as well
        // as the chip and the `add_space` before it: counting only those
        // two put the chip over the cell's edge by exactly the spacing.
        // In a real clipped `egui_extras` column, because the clip in
        // question is the one it sets up.
        use super::super::theme::{self, SPACE_SM, SPACE_XS};
        use super::{elided_text, status_chip, status_chip_width};
        use egui::{Rect, Shape, Vec2};
        use egui_extras::{Column, TableBuilder};

        /// The Startup list's Name column, as it opens.
        const CELL: f32 = 240.0;
        const CHIP: &str = "All users";
        const NAME: &str = "Microsoft OneDrive Personal Cloud Files Synchronisation Helper";

        let palette = crate::theme::Catalog::load().get(None).clone();
        let ctx = egui::Context::default();
        theme::apply(&ctx, &palette);
        let window = Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(800.0, 400.0));
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(window),
                ..Default::default()
            },
            |ui| {
                TableBuilder::new(ui)
                    .vscroll(false)
                    .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                    .column(Column::initial(CELL).clip(true))
                    .column(Column::remainder())
                    .header(theme::HEADER_HEIGHT, |mut header| {
                        header.col(|_| {});
                        header.col(|_| {});
                    })
                    .body(|body| {
                        body.rows(theme::ROW_HEIGHT, 1, |mut row| {
                            row.col(|ui| {
                                ui.add_space(SPACE_SM);
                                let reserved = status_chip_width(ui, CHIP)
                                    + ui.spacing().item_spacing.x
                                    + SPACE_XS;
                                elided_text(ui, NAME, palette.text, reserved);
                                ui.add_space(SPACE_XS);
                                status_chip(ui, CHIP, palette.text_muted);
                            });
                            row.col(|_| {});
                        });
                    });
            },
        );
        output.textures_delta.clear();

        // The chip is the one rounded fill. The name's galley still
        // reports the whole text it was asked for; what was actually laid
        // out is in its rows.
        let mut chip = None;
        let mut name = None;
        for clipped in &output.shapes {
            if let Shape::Rect(rect) = &clipped.shape {
                if rect.corner_radius != egui::CornerRadius::ZERO {
                    chip = Some((rect.rect, clipped.clip_rect));
                }
            } else if let Shape::Text(text) = &clipped.shape {
                if text.galley.text() == NAME {
                    name = Some(
                        text.galley
                            .rows
                            .iter()
                            .map(|row| row.row.text())
                            .collect::<String>(),
                    );
                }
            }
        }
        let (chip, clip) = chip.ok_or_else(|| anyhow::anyhow!("no chip was drawn"))?;
        let name = name.ok_or_else(|| anyhow::anyhow!("no name was drawn"))?;

        assert!(
            clip.contains_rect(chip),
            "the chip {chip:?} runs past its cell's clip {clip:?}, so it is cut mid-word"
        );
        assert!(
            name.ends_with('…'),
            "the name was not elided to make room: {name:?}"
        );
        Ok(())
    }
}

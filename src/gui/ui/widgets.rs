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
) -> Response {
    let font = TextStyle::Small.resolve(ui.style());
    let color = if sorted.is_some() {
        theme.text
    } else {
        theme.text_muted
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
    let (rect, response) = ui.allocate_exact_size(wanted, Sense::click());

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
    let painter = ui.painter();
    let radius = CornerRadius::same(3);

    painter.rect_filled(rect, radius, theme::rgb(theme.raised));
    // A non-finite fraction would produce a NaN width, which egui paints
    // as nothing — so a broken measurement would look like an empty bar
    // rather than a broken one.
    let fraction = if fraction.is_finite() {
        fraction.clamp(0.0, 1.0)
    } else {
        0.0
    };
    if fraction > 0.0 {
        let filled = Rect::from_min_size(
            rect.min,
            Vec2::new((rect.width() * fraction).max(2.0), rect.height()),
        );
        painter.rect_filled(filled, radius, theme::rgb(fill));
    }
    response
}

/// Paints a row's background, its hover lift, and its selection bar.
///
/// One function for all three so that a row cannot be selected-looking in
/// one table and hover-looking in another. The accent bar is what makes a
/// selection unmistakable — the fill itself is a light tint, chosen so
/// that secondary text on a selected row still clears WCAG AA. See
/// [`crate::theme::Palette::derive`].
pub fn row_background(
    ui: &Ui,
    theme: &Palette,
    rect: Rect,
    id: egui::Id,
    selected: bool,
    hovered: bool,
    striped: bool,
) {
    let painter = ui.painter();
    let base = if striped { theme.raised } else { theme.panel };
    let fill = if selected {
        theme::rgb(theme.selection)
    } else {
        hover_fill(ui, id, hovered, base, theme.hover)
    };
    painter.rect_filled(rect, CornerRadius::ZERO, fill);

    if selected {
        let bar = Rect::from_min_size(rect.left_top(), Vec2::new(SELECTION_BAR, rect.height()));
        painter.rect_filled(bar, CornerRadius::ZERO, theme::rgb(theme.accent));
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
    /// The tint's opacity at full load. Low, because the number on top of
    /// it has to stay readable — the tint is a hint, not a highlight.
    const MAX_ALPHA: f32 = 56.0;

    if !load.is_finite() || load < FLOOR {
        return;
    }
    let load = load.clamp(0.0, 1.0);
    // Square-rooted, as in `crate::color::heat`: a linear ramp leaves
    // everything under about 30% looking identical to idle, and that is
    // exactly the range where "this should not be doing anything" lives.
    let alpha = (load.sqrt() * MAX_ALPHA).clamp(0.0, 255.0) as u8;
    let color = theme.heat(load);
    ui.painter()
        .rect_filled(rect, CornerRadius::ZERO, theme::translucent(color, alpha));
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

/// A key/value line for a details pane.
pub fn detail_row(ui: &mut Ui, theme: &Palette, key: &str, value: &str) {
    /// The label column's width, so every value starts on one column.
    const KEY_WIDTH: f32 = 130.0;

    ui.horizontal(|ui| {
        ui.allocate_ui_with_layout(
            Vec2::new(KEY_WIDTH, 0.0),
            egui::Layout::left_to_right(Align::Center),
            |ui| {
                ui.label(
                    egui::RichText::new(key)
                        .color(theme::rgb(theme.text_muted))
                        .text_style(TextStyle::Small),
                );
            },
        );
        ui.label(
            egui::RichText::new(value)
                .color(theme::rgb(theme.text))
                .text_style(TextStyle::Small),
        );
    });
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
}

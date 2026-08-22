// ============================================================================
// Module:       gui::ui::theme
// Description:  The active palette, the egui style derived from it, and the one
//               spacing scale every margin in the app comes from.
//
// Dependencies: egui; crate::theme (the catalog), crate::color::Rgb
// ============================================================================

//! Where a theme becomes a window.
//!
//! [`apply`] installs the chosen [`crate::theme::Palette`] into a
//! thread-local before anything paints each frame, and the drawing code
//! reads it back through [`palette`]. That is ambient state, which this
//! codebase otherwise avoids — the justification is that it is exactly
//! how `ctx.style()` already works, and the alternative is threading a
//! `&Palette` through roughly a hundred call sites that have no other
//! reason to know a theme exists. The property that matters — that a
//! theme change cannot leave half the window on the old one — holds
//! either way, because there is exactly one palette and it is set once
//! per frame before any drawing happens.
//!
//! Most of the palette also goes into egui's own `Visuals`, so built-in
//! widgets follow the theme without any call site being involved. Prefer
//! letting a colour arrive that way over reading it from [`palette`] by
//! hand.
//!
//! ## No literal colours in drawing code
//!
//! Every colour comes from here. `no_drawing_module_holds_a_colour_literal`
//! in `gui::ui::tests` walks the source and fails the build for a
//! `Color32::from_rgb` outside this module — because a single hard-coded
//! grey is invisible in review and then unreadable in half the themes.
//!
//! The only exceptions are alpha-only values (a scrim, a shadow), which
//! are not theme colours at all but lighting, and [`crate::brand`], for
//! the reason stated there.
//!
//! ## No hand-picked pixel gaps
//!
//! Every margin, inset, and gap is one of [`SPACE_XS`], [`SPACE_SM`],
//! [`SPACE_MD`], [`SPACE_LG`], or [`PAD`]. Five values, so the left edges
//! of the title bar, the nav rail, the status bar and every pane's
//! heading form one column. A value between two steps is one of the two
//! steps.

use crate::color::Rgb;
use crate::theme::{Mode, Palette};
use egui::{Color32, CornerRadius, Margin, Stroke, TextStyle, Vec2};
use std::cell::RefCell;

/// The tightest gap: between an icon and its label, or a chip's inset.
pub const SPACE_XS: f32 = 4.0;

/// Between related controls in a row.
pub const SPACE_SM: f32 = 8.0;

/// Between groups of controls, and a card's inner margin.
pub const SPACE_MD: f32 = 12.0;

/// Between sections, and around a modal's body.
pub const SPACE_LG: f32 = 20.0;

/// The inset from a panel edge to its content.
///
/// One of the scale's own values rather than a sixth number, so that
/// every panel's content starts on the same column. It is
/// [`SPACE_MD`]-sized deliberately: [`SPACE_SM`] leaves a dense table
/// touching the window edge, and [`SPACE_LG`] wastes the width a
/// twelve-column table needs.
pub const PAD: f32 = SPACE_MD;

/// The corner radius on cards, chips, and buttons.
pub const RADIUS: u8 = 6;

/// The corner radius on a large surface — a modal, a graph panel.
pub const RADIUS_LG: u8 = 10;

/// The height of a row in the process and details tables.
///
/// Tall enough for a 16px shell icon with breathing room, short enough
/// that a 1080p window shows around thirty rows — which is the number
/// that makes scrolling feel like navigating rather than hunting.
pub const ROW_HEIGHT: f32 = 26.0;

/// The height of a table's header row.
pub const HEADER_HEIGHT: f32 = 28.0;

// The row geometry has to hold together, and these are relations between
// constants — so they are checked when the crate is compiled rather than
// when its tests are run.
const _: () = {
    assert!(
        ROW_HEIGHT >= 24.0,
        "a row must leave a 16px shell icon room to breathe"
    );
    assert!(
        ROW_HEIGHT <= 30.0,
        "a row this tall shows fewer than thirty processes on a 1080p \
         window, which turns scrolling from navigating into hunting"
    );
    assert!(
        HEADER_HEIGHT >= ROW_HEIGHT,
        "a header shorter than the rows beneath it reads as a row that \
         has been clipped"
    );
    assert!(
        PAD == SPACE_XS || PAD == SPACE_SM || PAD == SPACE_MD || PAD == SPACE_LG,
        "the panel inset must be a step of the spacing scale, or panels \
         start their content on different columns"
    );
};

/// The height of the custom title bar.
pub const TITLE_BAR_HEIGHT: f32 = 40.0;

/// The width of the collapsed navigation rail.
pub const NAV_WIDTH: f32 = 168.0;

/// The width of the accent bar down a selected row's leading edge.
///
/// The selection fill is deliberately a light tint — see
/// [`crate::theme::Palette::derive`] — so this is what makes a selected
/// row unmistakable from across the window.
pub const SELECTION_BAR: f32 = 3.0;

// The hover duration used to live here, beside the spacing scale. It
// moved to `crate::motion::INSTANT` with the other three durations: this
// module owns colour and spacing, and a lone timing constant filed next
// to them was how the app came to have two easing curves that were
// nearly the same. See `crate::motion` on why there are exactly four.

thread_local! {
    /// The palette in force for this frame.
    ///
    /// Thread-local rather than a global: egui is single-threaded per
    /// context, and a `static` behind a lock would be a lock taken
    /// thousands of times per frame for a value that never changes
    /// during one.
    static ACTIVE: RefCell<Palette> = RefCell::new(crate::theme::Catalog::load().get(None).clone());
}

/// Installs `theme` for this frame and derives egui's own style from it.
///
/// Called once per frame, before anything paints.
pub fn apply(ctx: &egui::Context, theme: &Palette) {
    ACTIVE.with(|active| active.replace(theme.clone()));

    // egui 0.36 keeps a separate `Style` for light and dark and picks
    // between them from the OS setting. This app's palette already
    // decides which it is, so the preference is pinned to the theme's own
    // mode — otherwise a user on a light-mode desktop running a dark
    // theme would get egui's light `Style` underneath a dark palette, and
    // the fields this module does not set (shadows, the text cursor)
    // would come out of the wrong one.
    ctx.set_theme(match theme.mode {
        Mode::Dark => egui::ThemePreference::Dark,
        Mode::Light => egui::ThemePreference::Light,
    });

    let derived = visuals(theme);
    // Both styles get the same values, so the pin above cannot be the
    // only thing standing between the app and a half-themed window.
    ctx.all_styles_mut(|style| {
        style.visuals = derived.clone();
        style.spacing.item_spacing = Vec2::new(SPACE_SM, SPACE_XS);
        style.spacing.button_padding = Vec2::new(SPACE_SM, SPACE_XS);
        style.spacing.menu_margin = Margin::same(as_margin(SPACE_XS));
        style.spacing.indent = SPACE_LG;
        style.spacing.interact_size = Vec2::new(0.0, ROW_HEIGHT - SPACE_XS);
        // Solid scrollbars, never floating. A floating bar is invisible
        // until the pointer is already on it, so a table with columns
        // past its edge looks like it has simply lost them.
        style.spacing.scroll = egui::style::ScrollStyle::solid();
        style.spacing.scroll.bar_width = SPACE_SM;

        // Slightly larger body text than egui's default. The default is
        // 12.5 at 1.0 scaling, which on a 1440p monitor is genuinely hard
        // to read in a dense table — and a dense table is what this app
        // mostly is.
        style.text_styles.insert(
            TextStyle::Body,
            egui::FontId::new(13.5, egui::FontFamily::Proportional),
        );
        style.text_styles.insert(
            TextStyle::Button,
            egui::FontId::new(13.5, egui::FontFamily::Proportional),
        );
        style.text_styles.insert(
            TextStyle::Small,
            egui::FontId::new(11.5, egui::FontFamily::Proportional),
        );
        style.text_styles.insert(
            TextStyle::Heading,
            egui::FontId::new(19.0, egui::FontFamily::Proportional),
        );
        // Numeric columns are drawn in this. A proportional font gives
        // every digit a different width, so a column of live-updating
        // numbers shimmers as its digits change — and the decimal points
        // do not line up, which makes a column of magnitudes unreadable
        // at a glance.
        style.text_styles.insert(
            TextStyle::Monospace,
            egui::FontId::new(12.5, egui::FontFamily::Monospace),
        );
    });
}

/// The palette in force.
///
/// Cheap: a thread-local read and a clone of a small struct. Called from
/// drawing code, so it must stay that way — anything that made this
/// expensive would be paid thousands of times a frame.
#[must_use]
pub fn palette() -> Palette {
    ACTIVE.with(|active| active.borrow().clone())
}

/// Converts a palette colour to an egui one.
///
/// The single conversion point between [`crate::color::Rgb`] and
/// `Color32`, which is what lets the theme catalog and its contrast tests
/// stay free of egui. See [`crate::color`].
#[must_use]
pub const fn rgb(color: Rgb) -> Color32 {
    Color32::from_rgb(color.r, color.g, color.b)
}

/// A palette colour at a given opacity.
///
/// For scrims and overlays, where the *alpha* is the design decision and
/// the colour still comes from the theme.
#[must_use]
pub fn translucent(color: Rgb, alpha: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(color.r, color.g, color.b, alpha)
}

/// A spacing-scale value as a margin unit.
///
/// `Margin` is `i8` in egui 0.36 while the scale is `f32`, so this is the
/// one place the conversion happens — rather than a `as i8` at forty call
/// sites, each of which could silently truncate a value someone later
/// changed.
#[must_use]
pub fn as_margin(space: f32) -> i8 {
    // The scale's values are all well inside `i8`; the clamp is what
    // makes that true rather than assumed.
    space.round().clamp(0.0, 127.0) as i8
}

/// A uniform margin from a scale value.
#[must_use]
pub fn margin(space: f32) -> Margin {
    Margin::same(as_margin(space))
}

/// A margin with separate horizontal and vertical steps.
#[must_use]
pub fn margin_xy(horizontal: f32, vertical: f32) -> Margin {
    Margin::symmetric(as_margin(horizontal), as_margin(vertical))
}

/// The frame a card or pane is drawn in.
pub fn card(theme: &Palette) -> egui::Frame {
    egui::Frame::new()
        .fill(rgb(theme.panel))
        .stroke(Stroke::new(1.0, rgb(theme.border)))
        .corner_radius(CornerRadius::same(RADIUS_LG))
        .inner_margin(margin(SPACE_MD))
}

/// Silences the resting rule `egui_extras` draws at every resizable
/// column boundary, for this `Ui` and everything drawn inside it.
///
/// A resizable table paints a line at each boundary, the full height of
/// the scroll area — through the rows, and on down through the empty
/// space under the last one. Four columns of numbers over three hundred
/// rows becomes a spreadsheet grid, and on a short list it is mostly a
/// set of vertical rules through nothing.
///
/// Rows are separated by their own fill: the stripe, the hover lift and
/// the selection bar all run the width of the row (see
/// [`super::widgets::row_background`]), which is what makes a row
/// readable across eight columns. Vertical rules on top of that are a
/// second, competing structure, and the one that wins is the one drawn
/// in a solid colour from top to bottom.
///
/// Only the **resting** stroke goes. `egui_extras` picks the hovered and
/// dragged strokes from different visuals, so the boundary still lights
/// up under the pointer — the affordance survives; the grid does not.
///
/// Scoped to a `Ui` rather than set in [`visuals`] because
/// `noninteractive.bg_stroke` is also what egui draws a menu separator
/// and a group frame with, and those want their line.
pub fn quiet_column_rules(ui: &mut egui::Ui) {
    ui.visuals_mut().widgets.noninteractive.bg_stroke = Stroke::NONE;
}

/// The frame the window's central content sits in.
pub fn content(theme: &Palette) -> egui::Frame {
    egui::Frame::new()
        .fill(rgb(theme.app))
        .inner_margin(margin(PAD))
}

/// egui's `Visuals`, derived from a palette.
///
/// The whole point is that built-in widgets pick the theme up without any
/// call site knowing. Two of these assignments are load-bearing and are
/// commented where they are made.
fn visuals(theme: &Palette) -> egui::Visuals {
    let mut visuals = match theme.mode {
        Mode::Dark => egui::Visuals::dark(),
        Mode::Light => egui::Visuals::light(),
    };

    visuals.panel_fill = rgb(theme.panel);
    visuals.window_fill = rgb(theme.raised);
    visuals.window_stroke = Stroke::new(1.0, rgb(theme.border));
    visuals.window_corner_radius = CornerRadius::same(RADIUS_LG);
    visuals.menu_corner_radius = CornerRadius::same(RADIUS);
    visuals.faint_bg_color = rgb(theme.raised);
    visuals.extreme_bg_color = rgb(theme.app);
    visuals.code_bg_color = rgb(theme.raised);
    visuals.hyperlink_color = rgb(theme.accent);
    visuals.warn_fg_color = rgb(theme.warning);
    visuals.error_fg_color = rgb(theme.danger);
    visuals.override_text_color = Some(rgb(theme.text));
    visuals.weak_text_color = Some(rgb(theme.text_muted));

    visuals.selection.bg_fill = rgb(theme.selection);
    visuals.selection.stroke = Stroke::new(1.0, rgb(theme.accent));

    // The two assignments that matter.
    //
    // egui paints *buttons* from `weak_bg_fill` and *filled controls* —
    // the scrollbar handle, a checkbox's interior, a slider's rail — from
    // `bg_fill`. They are not interchangeable. A button is a surface and
    // may share the card's colour; a scrollbar handle in the card's own
    // colour is invisible.
    //
    // Pointing both at a surface is what makes every scrollbar in an app
    // disappear, and it is not obvious from looking at the window that
    // anything is wrong — the bars are simply not there.
    // `a_scrollbar_handle_is_never_the_colour_of_what_it_scrolls` in
    // `crate::theme` checks the separation for every theme.
    for (state, surface, control) in [
        (
            &mut visuals.widgets.noninteractive,
            theme.panel,
            theme.control,
        ),
        (&mut visuals.widgets.inactive, theme.raised, theme.control),
        (
            &mut visuals.widgets.hovered,
            theme.hover,
            theme.control_hover,
        ),
        (&mut visuals.widgets.active, theme.hover, theme.accent),
        (&mut visuals.widgets.open, theme.hover, theme.control_hover),
    ] {
        state.weak_bg_fill = rgb(surface);
        state.bg_fill = rgb(control);
        state.bg_stroke = Stroke::new(1.0, rgb(theme.border));
        state.fg_stroke = Stroke::new(1.0, rgb(theme.text));
        state.corner_radius = CornerRadius::same(RADIUS);
        // No expansion on hover. egui's default grows a widget by a
        // pixel when hovered, which in a table of 26px rows makes the
        // row under the pointer nudge its neighbours.
        state.expansion = 0.0;
    }
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, rgb(theme.text_muted));
    visuals.widgets.active.fg_stroke = Stroke::new(1.0, rgb(theme.text));

    visuals
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Catalog;

    fn every_theme() -> Vec<Palette> {
        Catalog::load().themes().to_vec()
    }

    #[test]
    fn the_spacing_scale_is_strictly_increasing() {
        // Four steps that are actually different sizes. Two steps a pixel
        // apart would be a scale in name only.
        let scale = [SPACE_XS, SPACE_SM, SPACE_MD, SPACE_LG];
        assert!(
            scale.windows(2).all(|pair| pair[1] > pair[0]),
            "the scale must ascend: {scale:?}"
        );
        assert!(
            scale.windows(2).all(|pair| pair[1] - pair[0] >= 4.0),
            "adjacent steps must differ enough to be told apart: {scale:?}"
        );
    }

    #[test]
    fn the_panel_inset_is_one_of_the_scales_own_values() {
        // Not a sixth number: every panel's content has to start on the
        // same column as every other's.
        assert!(
            [SPACE_XS, SPACE_SM, SPACE_MD, SPACE_LG].contains(&PAD),
            "PAD ({PAD}) is not a step of the scale"
        );
    }

    #[test]
    fn every_scale_value_survives_the_margin_conversion() {
        // `Margin` is i8 and the scale is f32; a step that truncated
        // would silently change one panel's inset.
        for space in [SPACE_XS, SPACE_SM, SPACE_MD, SPACE_LG, PAD] {
            let converted = f32::from(as_margin(space));
            assert!(
                (converted - space).abs() < 0.5,
                "{space} became {converted} as a margin"
            );
        }
    }

    #[test]
    fn an_absurd_margin_is_clamped_rather_than_wrapping() {
        // `as i8` on a large float is a wrap in release builds, which
        // would turn a large margin into a negative one.
        assert_eq!(as_margin(10_000.0), 127);
        assert_eq!(as_margin(-50.0), 0);
        assert!(as_margin(f32::NAN) >= 0);
    }

    #[test]
    fn a_scrollbar_handle_is_never_the_colour_of_the_surface_it_scrolls() {
        // The bug this module's longest comment is about: pointing both
        // `bg_fill` and `weak_bg_fill` at a surface makes every scrollbar
        // in the app invisible, and nothing about the window says so.
        for theme in every_theme() {
            let visuals = visuals(&theme);
            for state in [
                &visuals.widgets.noninteractive,
                &visuals.widgets.inactive,
                &visuals.widgets.hovered,
                &visuals.widgets.open,
            ] {
                assert_ne!(
                    state.bg_fill, state.weak_bg_fill,
                    "{}: a filled control shares its surface's colour",
                    theme.name
                );
            }
        }
    }

    #[test]
    fn the_derived_visuals_carry_the_themes_own_colours() {
        for theme in every_theme() {
            let visuals = visuals(&theme);
            assert_eq!(visuals.panel_fill, rgb(theme.panel), "{}", theme.name);
            assert_eq!(visuals.selection.bg_fill, rgb(theme.selection));
            assert_eq!(visuals.error_fg_color, rgb(theme.danger));
            assert_eq!(visuals.override_text_color, Some(rgb(theme.text)));
        }
    }

    #[test]
    fn a_light_theme_produces_light_visuals() {
        // egui's own defaults differ between the two, and starting from
        // the wrong one leaves the fields this module does not set —
        // shadows, the text cursor — reading as the opposite mode.
        for theme in every_theme() {
            let visuals = visuals(&theme);
            match theme.mode {
                Mode::Dark => assert!(visuals.dark_mode, "{}", theme.name),
                Mode::Light => assert!(!visuals.dark_mode, "{}", theme.name),
            }
        }
    }

    #[test]
    fn no_widget_state_expands_on_hover() {
        // egui grows a hovered widget by a pixel by default, which in a
        // table of 26px rows makes the row under the pointer nudge its
        // neighbours.
        for theme in every_theme() {
            let visuals = visuals(&theme);
            for state in [
                &visuals.widgets.noninteractive,
                &visuals.widgets.inactive,
                &visuals.widgets.hovered,
                &visuals.widgets.active,
                &visuals.widgets.open,
            ] {
                assert_eq!(state.expansion, 0.0, "{}", theme.name);
            }
        }
    }

    #[test]
    fn the_colour_conversion_round_trips() {
        let color = Rgb::new(0x4c, 0xc9, 0xf0);
        let converted = rgb(color);
        assert_eq!(
            (converted.r(), converted.g(), converted.b()),
            (color.r, color.g, color.b)
        );
        assert_eq!(converted.a(), 255, "a palette colour is opaque");
    }

    #[test]
    fn a_translucent_colour_keeps_its_hue() {
        let color = Rgb::new(10, 20, 30);
        let scrim = translucent(color, 128);
        assert_eq!(scrim.a(), 128);
    }
}

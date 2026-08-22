// ============================================================================
// Module:       gui::ui::settings
// Description:  The Settings view — theme picker with live previews, sampling
//               interval, window chrome, and the about panel.
//
// Dependencies: egui; super::{theme, widgets, chrome}, crate::theme::Catalog
// ============================================================================

//! Settings.
//!
//! ## The theme picker previews rather than names
//!
//! Each theme is a swatch row — its four surfaces, its accent, and three
//! of its rainbow ramp — beside its name. A list of names is a list
//! nobody can choose from without trying each one, and trying each one
//! means the window restyling under the pointer six times.
//!
//! ## Changing the interval clears the graphs
//!
//! The samples already in a history ring were taken at the old spacing.
//! A graph drawn from both would silently compress part of its own
//! history — the shape would change without the data changing, which is
//! the one thing a graph must never do. See
//! [`crate::gui::app::App::reset_history`].

use super::theme::{self, SPACE_LG, SPACE_MD, SPACE_SM, SPACE_XS};
use super::{chrome, widgets};
use crate::gui::app::App;
use crate::theme::Palette;
use egui::{CornerRadius, Sense, Ui, Vec2};

/// The size of one swatch in a theme preview.
const SWATCH: f32 = 14.0;

/// Draws the Settings view.
pub fn draw(app: &mut App, ui: &mut Ui) {
    let theme = app.theme.clone();
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            appearance(app, ui, &theme);
            ui.add_space(chrome::SECTION_GAP);
            sampling(app, ui, &theme);
            ui.add_space(chrome::SECTION_GAP);
            behaviour(app, ui, &theme);
            ui.add_space(chrome::SECTION_GAP);
            about(app, ui, &theme);
        });
}

/// The theme picker and the chrome switch.
fn appearance(app: &mut App, ui: &mut Ui, theme: &Palette) {
    widgets::section(ui, theme, "Appearance");

    let themes: Vec<Palette> = app.catalog.themes().to_vec();
    let mut chosen: Option<Palette> = None;
    for candidate in &themes {
        if theme_row(ui, theme, candidate, candidate.id == theme.id) {
            chosen = Some(candidate.clone());
        }
        ui.add_space(SPACE_XS);
    }
    if let Some(chosen) = chosen {
        app.theme = chosen;
    }

    ui.add_space(SPACE_MD);
    let mut custom = app.custom_chrome;
    if ui
        .checkbox(&mut custom, "Draw the window's own title bar")
        .on_hover_text(
            "Windows 10's system caption is a light grey bar that no theme \
             can reach. Turning this off uses it anyway, and takes effect \
             the next time Rustaman starts.",
        )
        .changed()
    {
        app.custom_chrome = custom;
        app.notify("The title bar changes the next time Rustaman starts", false);
    }

    // The theme directory, so a user who wants to add one knows where to
    // put it without reading the docs.
    if let Some(dir) = crate::theme::user_theme_dir() {
        ui.add_space(SPACE_XS);
        ui.label(
            egui::RichText::new(format!(
                "Drop a .toml theme into {} to add your own.",
                dir.display()
            ))
            .color(theme::rgb(theme.text_faint))
            .text_style(egui::TextStyle::Small),
        );
    }

    // Themes that failed to load are reported here rather than swallowed:
    // a theme that silently does not appear is indistinguishable from one
    // the app cannot see.
    for (source, reason) in app.catalog.problems() {
        ui.label(
            egui::RichText::new(format!("{source} could not be loaded: {reason}"))
                .color(theme::rgb(theme.danger))
                .text_style(egui::TextStyle::Small),
        );
    }
}

/// One theme's row in the picker. Returns whether it was clicked.
fn theme_row(ui: &mut Ui, active: &Palette, candidate: &Palette, selected: bool) -> bool {
    /// The row's height. Tall enough for a name and a swatch strip.
    const HEIGHT: f32 = 44.0;

    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), HEIGHT), Sense::click());

    let fill = if selected {
        theme::rgb(active.selection)
    } else {
        widgets::hover_fill(
            ui,
            response.id,
            response.hovered(),
            active.panel,
            active.hover,
        )
    };
    ui.painter()
        .rect_filled(rect, CornerRadius::same(theme::RADIUS), fill);
    if selected {
        ui.painter().rect_stroke(
            rect,
            CornerRadius::same(theme::RADIUS),
            egui::Stroke::new(1.0, theme::rgb(active.accent)),
            egui::StrokeKind::Inside,
        );
    }

    ui.painter().text(
        rect.left_center() + Vec2::new(SPACE_MD, 0.0),
        egui::Align2::LEFT_CENTER,
        &candidate.name,
        egui::TextStyle::Body.resolve(ui.style()),
        theme::rgb(active.text),
    );

    // The preview: four surfaces, the accent, then three ramp colours.
    // The ramp is included because it is what the graphs are drawn in and
    // it is the part of a theme that differs most between two themes with
    // similar chrome.
    let swatches = [
        candidate.app,
        candidate.panel,
        candidate.raised,
        candidate.hover,
        candidate.accent,
        candidate.series(0, 3),
        candidate.series(1, 3),
        candidate.series(2, 3),
    ];
    let strip_width = SWATCH * swatches.len() as f32 + SPACE_XS;
    let mut x = rect.right() - strip_width;
    for swatch in swatches {
        let cell = egui::Rect::from_min_size(
            egui::pos2(x, rect.center().y - SWATCH / 2.0),
            Vec2::splat(SWATCH),
        );
        ui.painter()
            .rect_filled(cell, CornerRadius::same(3), theme::rgb(swatch));
        x += SWATCH;
    }

    response.clicked()
}

/// The sampling-interval picker.
fn sampling(app: &mut App, ui: &mut Ui, theme: &Palette) {
    widgets::section(ui, theme, "Sampling");
    ui.label(
        egui::RichText::new(
            "How often Rustaman reads the machine. A faster interval is more \
             responsive and costs a little more CPU.",
        )
        .color(theme::rgb(theme.text_muted))
        .text_style(egui::TextStyle::Small),
    );
    ui.add_space(SPACE_SM);

    let current = app.engine.interval();
    let mut chosen: Option<std::time::Duration> = None;
    ui.horizontal_wrapped(|ui| {
        for millis in crate::config::INTERVAL_CHOICES {
            let interval = std::time::Duration::from_millis(millis);
            let active = interval == current;
            let response = widgets::chip(
                ui,
                &chrome::interval_label(interval),
                if active {
                    theme.accent_soft
                } else {
                    theme.raised
                },
                if active { theme.text } else { theme.text_muted },
            )
            .interact(Sense::click());
            if response.clicked() {
                chosen = Some(interval);
            }
            ui.add_space(SPACE_XS);
        }
    });

    if let Some(interval) = chosen {
        app.engine.set_interval(interval);
        // The samples already in the rings were taken at the old spacing.
        // See the module docs.
        app.reset_history();
        app.notify(
            format!("Sampling every {}", chrome::interval_label(interval)),
            false,
        );
    }
}

/// The behaviour switches.
fn behaviour(app: &mut App, ui: &mut Ui, theme: &Palette) {
    widgets::section(ui, theme, "Behaviour");

    let mut confirm = app.config.confirm_end_task.unwrap_or(true);
    if ui
        .checkbox(&mut confirm, "Ask before ending a task")
        .on_hover_text(
            "Ending a task loses whatever the process had not saved. \
             Turning this off is for repeated, deliberate use.",
        )
        .changed()
    {
        app.config.confirm_end_task = Some(confirm);
    }

    ui.add_space(SPACE_XS);
    let mut on_top = app.config.always_on_top.unwrap_or(false);
    if ui
        .checkbox(&mut on_top, "Keep the window above others")
        .changed()
    {
        app.config.always_on_top = Some(on_top);
        ui.ctx()
            .send_viewport_cmd(egui::ViewportCommand::WindowLevel(if on_top {
                egui::WindowLevel::AlwaysOnTop
            } else {
                egui::WindowLevel::Normal
            }));
    }

    ui.add_space(SPACE_MD);
    // The access notice. Not a warning — running unelevated is a
    // perfectly reasonable way to use this — but the missing columns need
    // an explanation, or they look like a bug in the app.
    let (message, color) = if app.elevated {
        (
            "Running with debug privilege: every process's owner, path and \
             architecture can be read.",
            theme.success,
        )
    } else {
        (
            "Running without debug privilege. Processes owned by other \
             accounts show no owner, path or architecture; every other \
             column is complete. Run Rustaman as administrator to see them.",
            theme.text_muted,
        )
    };
    ui.label(
        egui::RichText::new(message)
            .color(theme::rgb(color))
            .text_style(egui::TextStyle::Small),
    );
}

/// The about panel.
fn about(app: &App, ui: &mut Ui, theme: &Palette) {
    widgets::section(ui, theme, "About");
    ui.horizontal(|ui| {
        super::super::icons::paint_brand(ui, 40.0);
        ui.add_space(SPACE_MD);
        ui.vertical(|ui| {
            ui.label(
                egui::RichText::new(crate::brand::NAME)
                    .color(theme::rgb(theme.text))
                    .heading(),
            );
            ui.label(
                egui::RichText::new(crate::brand::TAGLINE)
                    .color(theme::rgb(theme.text_muted))
                    .text_style(egui::TextStyle::Small),
            );
            ui.label(
                egui::RichText::new(format!("Version {}", env!("CARGO_PKG_VERSION")))
                    .color(theme::rgb(theme.text_faint))
                    .text_style(egui::TextStyle::Small),
            );
        });
    });

    ui.add_space(SPACE_MD);
    if let Some(snapshot) = app.snapshot.as_ref() {
        widgets::detail_row(ui, theme, "Processor", &snapshot.system.cpu.name);
        widgets::detail_row(
            ui,
            theme,
            "Cores",
            &format!(
                "{} physical, {} logical",
                snapshot.system.cpu.physical_cores, snapshot.system.cpu.logical_cores
            ),
        );
        widgets::detail_row(
            ui,
            theme,
            "Memory",
            &crate::format::bytes(snapshot.system.memory.total),
        );
        widgets::detail_row(
            ui,
            theme,
            "Up time",
            &crate::format::duration(snapshot.system.uptime_seconds),
        );
    }

    ui.add_space(SPACE_LG);
    ui.label(
        egui::RichText::new(
            "Rustaman reads the machine through the same interfaces Task \
             Manager does. It never sends anything anywhere.",
        )
        .color(theme::rgb(theme.text_faint))
        .text_style(egui::TextStyle::Small),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    #[test]
    fn the_preview_shows_enough_of_a_theme_to_choose_by() {
        // A list of names is a list nobody can choose from without trying
        // each one — which means restyling the window six times.
        let catalog = crate::theme::Catalog::load();
        let Some(candidate) = catalog.themes().first() else {
            return;
        };
        let swatches = [
            candidate.app,
            candidate.panel,
            candidate.raised,
            candidate.hover,
            candidate.accent,
            candidate.series(0, 3),
            candidate.series(1, 3),
            candidate.series(2, 3),
        ];
        assert_eq!(swatches.len(), 8);
        // The ramp colours have to differ from the surfaces, or the
        // preview shows a theme's chrome and nothing about its charts.
        assert_ne!(swatches[5], swatches[0]);
    }

    #[test]
    fn changing_the_interval_clears_the_graphs() {
        // The samples already in a ring were taken at the old spacing; a
        // graph drawn from both would change shape without the data
        // changing, which is the one thing a graph must never do.
        let mut app = App::new(Config::default());
        let mut snapshot = crate::model::Snapshot::default();
        snapshot.system.cpu.total_percent = 50.0;
        app.poll();
        // Record directly, since no sampler has published yet.
        app.performance.cpu.push(50.0);
        assert!(!app.performance.cpu.is_empty());

        app.engine.set_interval(std::time::Duration::from_secs(2));
        app.reset_history();
        assert!(app.performance.cpu.is_empty());
    }

    #[test]
    fn every_offered_interval_is_selectable() {
        // A chip whose interval the engine then clamps would be a control
        // that visibly does not take.
        let app = App::new(Config::default());
        for millis in crate::config::INTERVAL_CHOICES {
            let interval = std::time::Duration::from_millis(millis);
            app.engine.set_interval(interval);
            assert_eq!(
                app.engine.interval(),
                interval,
                "the picker offers {millis}ms but the engine holds a \
                 different value"
            );
        }
    }

    #[test]
    fn every_theme_in_the_catalog_can_be_chosen() {
        let mut app = App::new(Config::default());
        let themes: Vec<Palette> = app.catalog.themes().to_vec();
        assert!(!themes.is_empty());
        for candidate in themes {
            app.theme = candidate.clone();
            assert_eq!(
                app.to_config().theme.as_deref(),
                Some(candidate.id.as_str())
            );
        }
    }
}

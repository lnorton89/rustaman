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

use super::theme::{self, SPACE_LG, SPACE_MD, SPACE_SM};
use super::{chrome, widgets};
use crate::gui::app::App;
use crate::theme::Palette;
use egui::{CornerRadius, Sense, Ui, Vec2};

/// The size of one swatch in a theme preview.
const SWATCH: f32 = 14.0;

/// Draws the Settings view.
pub fn draw(app: &mut App, ui: &mut Ui) {
    let theme = app.theme.clone();
    widgets::scroll_body(ui, |ui| {
        appearance(app, ui, &theme);
        theme::gap(ui, chrome::SECTION_GAP);
        sampling(app, ui, &theme);
        theme::gap(ui, chrome::SECTION_GAP);
        behaviour(app, ui, &theme);
        theme::gap(ui, chrome::SECTION_GAP);
        about(app, ui, &theme);
    });
}

/// The theme picker and the chrome switch.
fn appearance(app: &mut App, ui: &mut Ui, theme: &Palette) {
    widgets::section(ui, theme, "Appearance");

    // A gallery, not a list. As full-width rows, a theme's name sat at
    // the left edge of the window and its swatches at the right — eleven
    // hundred points apart on the window the app opens at, which is
    // further than the eye pairs two things across. Eight of them also
    // cost four hundred points of vertical space to show eight words.
    //
    // A theme picker is a set of *previews* being compared with each
    // other, which is what a grid is for: the swatch strip sits under
    // its own name, and every strip is beside the next one.
    let themes: Vec<Palette> = app.catalog.themes().to_vec();
    let mut chosen: Option<Palette> = None;
    widgets::card_grid(ui, THEME_CARD_WIDTH, themes.len(), |ui, index| {
        let Some(candidate) = themes.get(index) else {
            return;
        };
        if theme_card(ui, theme, candidate, candidate.id == theme.id) {
            chosen = Some(candidate.clone());
        }
    });
    if let Some(chosen) = chosen {
        app.theme = chosen;
    }

    // The theme directory, under the themes it is about rather than
    // under the title-bar switch below them.
    if let Some(dir) = crate::theme::user_theme_dir() {
        theme::gap(ui, SPACE_SM);
        ui.label(
            egui::RichText::new(format!(
                "Drop a .toml theme into {} to add your own.",
                dir.display()
            ))
            .color(theme::rgb(theme.text_muted))
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

    theme::gap(ui, chrome::SECTION_GAP);
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
}

/// The narrowest a theme card may be before the grid drops a column.
///
/// Set by the widest thing a card holds: the swatch strip, which is
/// eight [`SWATCH`]-wide cells, plus the card's own inset. A name longer
/// than that truncates; a strip narrower than that stops being a
/// preview.
const THEME_CARD_WIDTH: f32 = 200.0;

/// One theme's card in the gallery. Returns whether it was clicked.
fn theme_card(ui: &mut Ui, active: &Palette, candidate: &Palette, selected: bool) -> bool {
    /// The card's height: a name, the module's own spacing, and a swatch
    /// strip under it.
    const HEIGHT: f32 = 56.0;

    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), HEIGHT), Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::Role::Button,
            ui.is_enabled(),
            selected,
            &candidate.name,
        )
    });
    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);

    // Keyed on the theme rather than on its position in the gallery, so
    // adding a theme does not hand its hover state to whichever card now
    // sits where another one did.
    let id = ui.scope_id().with(("theme", &candidate.id));
    let fill = widgets::selectable_surface(ui, active, id, &response, selected, active.panel);
    ui.painter()
        .rect_filled(rect, CornerRadius::same(theme::RADIUS), fill);
    ui.painter().rect_stroke(
        rect,
        CornerRadius::same(theme::RADIUS),
        egui::Stroke::new(
            1.0,
            theme::rgb(if selected {
                active.accent
            } else {
                active.border
            }),
        ),
        egui::StrokeKind::Inside,
    );

    let name = widgets::truncated(
        ui,
        &candidate.name,
        egui::TextStyle::Body.resolve(ui.style()),
        active.text,
        rect.width() - SPACE_MD * 2.0,
    );
    ui.painter().galley(
        rect.left_top() + Vec2::new(SPACE_MD, SPACE_SM),
        name,
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
    let mut x = rect.left() + SPACE_MD;
    for swatch in swatches {
        let cell = egui::Rect::from_min_size(
            egui::pos2(x, rect.bottom() - SPACE_SM - SWATCH),
            Vec2::splat(SWATCH),
        );
        ui.painter()
            .rect_filled(cell, CornerRadius::same(3), theme::rgb(swatch));
        x += SWATCH;
    }

    widgets::focus_ring(ui, active, &response);
    response.on_hover_text(&candidate.name).clicked()
}

/// The sampling-interval picker.
fn sampling(app: &mut App, ui: &mut Ui, theme: &Palette) {
    widgets::section(ui, theme, "Sampling");
    widgets::prose(
        ui,
        "How often Rustaman reads the machine. A faster interval is more \
         responsive and costs a little more CPU.",
        theme.text_muted,
    );
    theme::gap(ui, SPACE_SM);

    let current = app.engine.interval();
    let mut chosen: Option<std::time::Duration> = None;
    ui.horizontal_wrapped(|ui| {
        for millis in crate::config::INTERVAL_CHOICES {
            let interval = std::time::Duration::from_millis(millis);
            let active = interval == current;
            let response =
                widgets::choice_button(ui, theme, &chrome::interval_label(interval), active);
            if response.clicked() {
                chosen = Some(interval);
            }
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

    theme::gap(ui, SPACE_SM);
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

    theme::gap(ui, SPACE_MD);
    // The access notice. Not a warning — running unelevated is a
    // perfectly reasonable way to use this — but the missing columns need
    // an explanation, or they look like a bug in the app.
    let (message, color) = if app.elevated {
        (
            "Running with debug privilege. More processes' owner, path and \
             architecture can be read; protected processes may still refuse access.",
            theme.success,
        )
    } else {
        (
            "Running without debug privilege. Some processes may hide owner, \
             path or architecture. Running as administrator can improve access; \
             protected processes may still refuse it.",
            theme.text_muted,
        )
    };
    widgets::prose(ui, message, color);
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
                    .color(theme::rgb(theme.text_muted))
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
    widgets::prose(
        ui,
        "Rustaman reads the machine through the same interfaces Task \
             Manager does. It never sends anything anywhere.",
        theme.text_muted,
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
        //
        // *Every* ring, not the headline ones. The disk read and write
        // halves and the network receive half were left out of the reset
        // for as long as they existed, so the Disk and Network panels'
        // own graphs kept the old spacing under headline graphs that had
        // started afresh.
        let mut app = App::new(Config::default());
        // Recorded through the same path a snapshot takes, so a ring
        // added to `record_history` and not to `reset_history` fails
        // here rather than on a user's machine.
        let mut snapshot = crate::model::Snapshot::default();
        snapshot.system.cpu.total_percent = 50.0;
        snapshot.system.cpu.kernel_percent = 10.0;
        snapshot.system.cpu.per_core = vec![50.0, 50.0];
        snapshot.system.memory.total = 100;
        snapshot.system.memory.available = 50;
        snapshot.system.disks.push(crate::model::DiskSample {
            read_rate: 1_000.0,
            write_rate: 2_000.0,
            ..crate::model::DiskSample::default()
        });
        snapshot.system.adapters.push(crate::model::AdapterSample {
            luid: 7,
            hardware: true,
            receive_rate: 3_000.0,
            send_rate: 4_000.0,
            ..crate::model::AdapterSample::default()
        });
        snapshot.system.gpus.push(crate::model::GpuSample {
            utilisation: 25.0,
            ..crate::model::GpuSample::default()
        });
        app.record_history(&snapshot);

        let rings = |app: &App| -> Vec<(&'static str, usize)> {
            let performance = &app.performance;
            let mut rings = vec![
                ("cpu", performance.cpu.len()),
                ("cpu_kernel", performance.cpu_kernel.len()),
                ("memory", performance.memory.len()),
                ("disk", performance.disk.len()),
                ("disk_read", performance.disk_read.len()),
                ("disk_write", performance.disk_write.len()),
                ("network", performance.network.len()),
                ("network_send", performance.network_send.len()),
                ("network_receive", performance.network_receive.len()),
                ("gpu", performance.gpu.len()),
            ];
            rings.extend(performance.cores.iter().map(|core| ("core", core.len())));
            rings.extend(
                performance
                    .adapters
                    .values()
                    .map(|adapter| ("adapter", adapter.len())),
            );
            rings
        };
        for (name, length) in rings(&app) {
            assert_eq!(length, 1, "{name} did not record the snapshot");
        }

        app.engine.set_interval(std::time::Duration::from_secs(2));
        app.reset_history();
        for (name, length) in rings(&app) {
            assert_eq!(
                length, 0,
                "{name} still holds {length} sample(s) taken at the old interval"
            );
        }
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

// ============================================================================
// Module:       gui::ui::performance
// Description:  The Performance view — a resource picker down one side and the
//               chosen resource's graph and readouts filling the rest.
//
// Dependencies: egui; super::{theme, widgets, graph, chrome}, crate::gui::app
// ============================================================================

//! The Performance view.
//!
//! A list of resources on the left, each with its own live sparkline, and
//! the selected one drawn large on the right.
//!
//! ## Why a picker rather than every graph at once
//!
//! Six resources drawn at full size is six graphs at a hundred pixels
//! each on a normal window, which is a size at which none of them can be
//! read — and on a machine with three disks and two adapters it is
//! eleven. The picker's sparklines carry the "is anything happening"
//! signal for all of them at once, which is what a glance is for, and
//! the large graph carries the detail for the one being investigated.
//!
//! ## The readouts are beside the graph, not under it
//!
//! Numbers under a graph get pushed off the bottom of a short window, and
//! they are the part that is actually read — a graph shows a shape, and
//! the shape is only meaningful once you know the scale it is drawn at.

use super::theme::{self, SPACE_LG, SPACE_MD, SPACE_SM, SPACE_XS};
use super::{chrome, graph, widgets};
use crate::gui::app::{App, PerformanceFocus};
use crate::model::history::Series;
use crate::model::{Snapshot, SystemSample};
use crate::theme::Palette;
use egui::{Rect, Response, Sense, Ui, Vec2};

/// The width of the resource picker.
///
/// Wide enough for "Network" plus a sparkline and a value; anything
/// narrower makes the sparkline too small to read a shape from.
const PICKER_WIDTH: f32 = 200.0;

/// The height of a picker entry.
const PICKER_ROW: f32 = 52.0;

/// The least height the main graph may shrink to.
///
/// It was the height the graph always got, and on a tall window that
/// left the bottom third of the pane empty under a panel whose whole
/// content is one chart. It is a floor now: see [`graph_height`].
const GRAPH_MINIMUM: f32 = 220.0;

/// The height one row of the per-core grid wants.
///
/// Enough for a core's own miniature chart to show a shape rather than a
/// band of colour. The grid is given more than this when the window has
/// it to spare — see [`cpu`].
const CORE_ROW: f32 = 46.0;

/// The least height the whole core grid may shrink to.
///
/// A four-core machine gets one row, and one row of [`CORE_ROW`] is a
/// strip too short to read a shape out of at all.
const CORE_GRID_MINIMUM: f32 = 90.0;

// Relations between the heights above, checked when the crate is
// compiled. A `const` block rather than a test: clippy's
// `assertions_on_constants` fires on the test form.
const _: () = {
    assert!(
        CORE_GRID_MINIMUM > CORE_ROW,
        "a core grid floored below one row's height cannot show its         one row"
    );
    assert!(
        GRAPH_MINIMUM > CORE_GRID_MINIMUM,
        "the main graph is the panel's headline and the core grid is its         detail; a floor that inverts them inverts the panel"
    );
};

/// The keys the five panels measure themselves under.
///
/// One per panel rather than one for the view: the panels have very
/// different tails, and a shared key would have each of them asking for
/// a re-layout every time the picker moved to another.
const CPU: &str = "performance-below-cpu";
/// See [`CPU`].
const MEMORY: &str = "performance-below-memory";
/// See [`CPU`].
const DISK: &str = "performance-below-disk";
/// See [`CPU`].
const NETWORK: &str = "performance-below-network";
/// See [`CPU`].
const GPU: &str = "performance-below-gpu";

/// What a panel drew below its graph when it was last drawn, if it has
/// been drawn.
fn measured_below(ui: &Ui, panel: &'static str) -> Option<f32> {
    ui.data(|data| data.get_temp::<f32>(egui::Id::new(panel)))
}

/// The height a panel's main graph takes: the pane's own remainder, once
/// whatever the panel draws below the graph has been measured out of it.
///
/// ## Measured rather than reserved
///
/// The obvious way to write this is a constant per panel saying how much
/// room its readouts need. Three of the five panels do not have one: the
/// disk panel's tail is a card per physical disk, the GPU panel's is a
/// card per adapter with a line per engine, and the network panel's is a
/// row per adapter, of which this machine has twenty-one. A constant
/// that has to cover those either strands a band of empty pane under the
/// short ones or pushes the long ones into a scroll they did not need.
///
/// So the panel measures itself instead. It draws once with the graph at
/// its floor, [`record_below`] notes how far the content below the graph
/// actually reached, and — because that is a *different* answer than the
/// one this pass drew with — asks egui for another pass over the same
/// frame. The second pass gives the graph the remainder, and the panel
/// is laid out correctly before anything reaches the screen: egui runs
/// both passes inside one `Context::run` and only the last one is
/// painted, which is exactly what [`egui::Context::request_discard`] is
/// for.
///
/// The extra pass is not per frame. What sits under the graph is text
/// and cards, whose height changes when a disk is plugged in or a panel
/// is switched to and not otherwise, so the second pass costs one
/// re-layout of five widgets on those frames and nothing on the rest.
fn graph_height(ui: &Ui, panel: &'static str) -> f32 {
    let Some(below) = measured_below(ui, panel) else {
        // Nothing has been measured yet, so the graph draws at its floor
        // — today's layout — and `record_below` corrects it before the
        // frame is painted.
        return GRAPH_MINIMUM;
    };
    (ui.available_height() - below).max(GRAPH_MINIMUM)
}

/// Notes how much room the panel needs under its graph, and asks for
/// another pass if that is not what this one assumed.
///
/// Called with the bottom of the graph's own rect, at the point where
/// everything that has to fit *under* the graph has been drawn — which
/// for the CPU panel is before the core grid rather than at the end of
/// the panel, because the grid takes its own share of the remainder and
/// so must not be counted as part of what the remainder is spent on.
fn record_below(ui: &Ui, panel: &'static str, graph_bottom: f32) {
    /// How far the measurement may move before it is worth another pass.
    ///
    /// Not zero: text metrics land on fractional points, and a
    /// measurement that disagrees with itself by a hundredth of a point
    /// would ask for a second pass on every frame forever.
    const TOLERANCE: f32 = 0.5;

    /// A point held back from the graph, so that the content lands just
    /// inside the pane rather than exactly on its edge.
    ///
    /// Filling the pane to the last fractional point risks tipping the
    /// scroll area into showing a scrollbar, and a scrollbar narrows the
    /// pane, and a narrower pane rewraps the card grids into another row
    /// — which is a *taller* tail, which asks for another pass, which
    /// takes the scrollbar away again. A point of slack costs nothing
    /// visible and cannot oscillate.
    const SLACK: f32 = 1.0;

    // `chrome::SECTION_GAP` for the gap `detail` leaves after the last
    // thing a panel draws, which is as much a part of what the pane owes
    // the tail as the tail itself.
    let below = ui.min_rect().bottom() - graph_bottom + chrome::SECTION_GAP + SLACK;
    let settled = measured_below(ui, panel).is_some_and(|old| (old - below).abs() <= TOLERANCE);
    if !settled {
        ui.data_mut(|data| data.insert_temp(egui::Id::new(panel), below));
        ui.ctx()
            .request_discard("a Performance panel measured what sits under its graph");
    }
}

/// The narrowest a per-device card may be before the grid drops a column.
///
/// Set by the widest thing these cards hold: the disk card's three stat
/// columns, whose last one is a `"97.1 GB / 1.57 TB"` pair. Below about
/// this the pair wraps and the card's three columns stop lining up with
/// the card beside it, which is most of what makes a grid of cards read
/// as a grid rather than as a pile.
const DEVICE_CARD_WIDTH: f32 = 340.0;

/// Draws the Performance view.
pub fn draw(app: &mut App, ui: &mut Ui) {
    let theme = app.theme.clone();
    let Some(snapshot) = app.snapshot.clone() else {
        widgets::empty_state(ui, &theme, "Waiting for the first sample…");
        return;
    };

    picker_panel().show(ui, |ui| {
        picker(app, ui, &theme, &snapshot);
    });
    gutter_panel().show(ui, |_| {});
    detail(app, ui, &theme, &snapshot);
}

/// The picker's docked column.
///
/// A constructor rather than a chain written out at the call site,
/// because the tests below have to reproduce this layout exactly to
/// measure the seam it creates — and a replica is a thing that drifts.
/// Three copies of this chain had already been written by hand.
///
/// No separator line. The gutter beside it *is* the separation, and the
/// line drew a second boundary a few points from the panel's own edge —
/// two rules with a sliver between them, which reads as a doubled border
/// rather than as a gap.
///
/// Not resizable, because it cannot be: `exact_size` pins the width, so
/// egui's default `resizable: true` leaves an edge that takes a drag,
/// shows a resize cursor, and clamps straight back. See
/// `no_fixed_panel_offers_a_resize_handle_that_does_nothing`.
fn picker_panel() -> egui::Panel {
    egui::Panel::left("performance-picker")
        .exact_size(PICKER_WIDTH)
        .resizable(false)
        .show_separator_line(false)
        .frame(egui::Frame::new().inner_margin(theme::margin_xy(0.0, 0.0)))
}

/// The empty column between the picker and the detail pane.
///
/// A second docked panel, holding nothing, purely to reserve a
/// horizontal gutter — not `ui.add_space(SPACE_LG)`, which reads as the
/// right call and does nothing at all here. `add_space` advances the
/// cursor along the *current layout's* axis, and the layout `ui` carries
/// past a docked `Panel::left` is still the page's own top-down one; the
/// panel changes what area remains, not which direction spacing moves
/// in. So the two columns had no gap between them beyond how each
/// rounded off its own edge, however each of those was written — which
/// is what made it so easy to miss: both sides could be independently
/// correct and the seam still read as touching.
fn gutter_panel() -> egui::Panel {
    egui::Panel::left("performance-picker-gap")
        .exact_size(SPACE_LG)
        .resizable(false)
        .frame(egui::Frame::new())
        .show_separator_line(false)
}

/// The resource list down the left.
///
/// Returns the rows' own bounding rect — not used by [`draw`], which
/// discards it, but by the tests, which need the content's real extent
/// rather than the picker panel's: the panel is a fixed
/// [`PICKER_WIDTH`] regardless of what is drawn inside it, so measuring
/// *it* can never reveal whether the content left a margin before its
/// edge.
fn picker(app: &mut App, ui: &mut Ui, theme: &Palette, snapshot: &Snapshot) -> Rect {
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            // The picker panel's own frame carries zero margin — see its
            // construction in `draw` — so a row that fills exactly
            // `ui.available_width()` reaches the panel's true right edge
            // with nothing but the gap before the detail column for
            // separation. That gap is real, but it is the *only* thing
            // standing between the sparkline and the seam, which is the
            // same "one gap doing all the work" shape the detail column
            // had — trimmed by the same step, for the same reason: the
            // sparkline's own margin should not depend on what is drawn
            // on the other side of the seam.
            let width = (ui.available_width() - SPACE_MD).max(0.0);
            ui.set_max_width(width);
            let entries = [
                (
                    PerformanceFocus::Cpu,
                    "CPU",
                    crate::format::percent(snapshot.system.cpu.total_percent),
                ),
                (
                    PerformanceFocus::Memory,
                    "Memory",
                    format!(
                        "{} / {}",
                        crate::format::bytes(snapshot.system.memory.used()),
                        crate::format::bytes(snapshot.system.memory.total)
                    ),
                ),
                (
                    PerformanceFocus::Disk,
                    "Disk",
                    crate::format::rate(
                        snapshot
                            .system
                            .disks
                            .iter()
                            .map(crate::model::DiskSample::total_rate)
                            .sum(),
                    ),
                ),
                (
                    PerformanceFocus::Network,
                    "Network",
                    crate::format::rate(snapshot.system.network_rate()),
                ),
                (
                    PerformanceFocus::Gpu,
                    "GPU",
                    crate::format::percent(
                        snapshot
                            .system
                            .gpus
                            .iter()
                            .map(|gpu| gpu.utilisation)
                            .fold(0.0, f64::max),
                    ),
                ),
            ];

            for (index, (focus, label, value)) in entries.into_iter().enumerate() {
                // A machine with no GPU counters gets no GPU entry rather
                // than one permanently reading 0% — which would look like
                // a GPU that is never used.
                if focus == PerformanceFocus::Gpu && snapshot.system.gpus.is_empty() {
                    continue;
                }
                if picker_entry(app, ui, theme, focus, label, &value, index) {
                    app.performance.focus = focus;
                }
                ui.add_space(SPACE_XS);
            }
            ui.min_rect()
        })
        .inner
}

/// One entry of the resource picker. Returns whether it was clicked.
fn picker_entry(
    app: &App,
    ui: &mut Ui,
    theme: &Palette,
    focus: PerformanceFocus,
    label: &str,
    value: &str,
    index: usize,
) -> bool {
    let active = app.performance.focus == focus;
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), PICKER_ROW), Sense::click());

    let fill = if active {
        theme::rgb(theme.selection)
    } else {
        widgets::hover_fill(
            ui,
            response.id,
            response.hovered(),
            theme.panel,
            theme.hover,
        )
    };
    ui.painter()
        .rect_filled(rect, egui::CornerRadius::same(theme::RADIUS), fill);
    if active {
        let bar = Rect::from_min_size(
            rect.left_top() + Vec2::new(0.0, SPACE_XS),
            Vec2::new(3.0, rect.height() - SPACE_XS * 2.0),
        );
        ui.painter()
            .rect_filled(bar, egui::CornerRadius::same(2), theme::rgb(theme.accent));
    }

    // The sparkline fills the entry's right half, behind the value — so
    // the number is read against its own recent shape.
    let series = series_for(app, focus);
    let spark = Rect::from_min_size(
        rect.left_top() + Vec2::new(rect.width() * 0.45, SPACE_XS),
        Vec2::new(
            rect.width() * 0.55 - SPACE_SM,
            rect.height() - SPACE_XS * 2.0,
        ),
    );
    if let Some(series) = series {
        graph::sparkline(
            ui,
            spark,
            series,
            accent_for(theme, focus, index),
            floor_for(focus),
        );
    }

    ui.painter().text(
        rect.left_top() + Vec2::new(SPACE_MD, SPACE_SM),
        egui::Align2::LEFT_TOP,
        label,
        egui::TextStyle::Body.resolve(ui.style()),
        theme::rgb(if active { theme.text } else { theme.text_muted }),
    );
    // Truncated at the sparkline's left edge rather than drawn through
    // it. The Memory entry's value is a pair — "25.6 GB / 64.0 GB" —
    // which is wider than the 45% of the row the text column actually
    // has, so it used to run underneath the sparkline and the two
    // overprinted each other into something that read as neither.
    let value_width = (spark.left() - SPACE_SM - (rect.left() + SPACE_MD)).max(0.0);
    let galley = widgets::truncated(
        ui,
        value,
        egui::TextStyle::Small.resolve(ui.style()),
        theme.text_faint,
        value_width,
    );
    ui.painter().galley(
        rect.left_bottom() + Vec2::new(SPACE_MD, -SPACE_SM - galley.size().y),
        galley,
        theme::rgb(theme.text_faint),
    );

    response.clicked()
}

/// The history ring behind a resource.
fn series_for(app: &App, focus: PerformanceFocus) -> Option<&crate::model::history::Series> {
    let performance = &app.performance;
    Some(match focus {
        PerformanceFocus::Cpu => &performance.cpu,
        PerformanceFocus::Memory => &performance.memory,
        PerformanceFocus::Disk => &performance.disk,
        PerformanceFocus::Network => &performance.network,
        PerformanceFocus::Gpu => &performance.gpu,
    })
}

/// A resource's colour, from the theme's rainbow ramp.
///
/// By index rather than by a per-resource choice, so the five resources
/// are five evenly spaced hues of the active theme rather than five
/// fixed colours that clash with half the catalog.
fn accent_for(theme: &Palette, focus: PerformanceFocus, index: usize) -> crate::color::Rgb {
    let _ = focus;
    theme.series(index, 5)
}

/// A resource's axis floor: fixed for a percentage, free for a rate.
fn floor_for(focus: PerformanceFocus) -> f32 {
    match focus {
        PerformanceFocus::Cpu | PerformanceFocus::Memory | PerformanceFocus::Gpu => 100.0,
        PerformanceFocus::Disk | PerformanceFocus::Network => 0.0,
    }
}

/// The wall-clock span represented by the samples currently visible.
///
/// Histories can be only partly filled just after launch, so describing the
/// configured capacity would overstate what the graph actually contains.
fn history_window(series: &Series, interval: std::time::Duration) -> String {
    let seconds = (series.len() as f64 * interval.as_secs_f64()).round() as u64;
    crate::format::duration(seconds)
}

/// The sampler interval that produced the history currently on screen.
fn sample_interval(app: &App) -> std::time::Duration {
    app.snapshot
        .as_ref()
        .map_or_else(|| app.engine.interval(), |snapshot| snapshot.interval)
}

/// The selected resource, drawn large.
///
/// Returns the drawn content's own bounding rect — not used by [`draw`],
/// which discards it, but by the tests, which need to measure the seam
/// with the picker column against what `detail()` actually drew rather
/// than against the outer `Ui`'s own bounds, which would also include
/// the picker panel drawn into the same `Ui` earlier.
fn detail(app: &mut App, ui: &mut Ui, theme: &Palette, snapshot: &Snapshot) -> Rect {
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            // The graphs and the core grid below them fill exactly
            // `ui.available_width()` — no dense multi-column table has
            // to be squeezed into this view the way `PAD` is sized
            // around, so this trims a further step off the right edge
            // rather than letting a bordered chart end flush with it.
            let width = (ui.available_width() - SPACE_MD).max(0.0);
            ui.set_max_width(width);
            match app.performance.focus {
                PerformanceFocus::Cpu => cpu(app, ui, theme, &snapshot.system),
                PerformanceFocus::Memory => memory(app, ui, theme, &snapshot.system),
                PerformanceFocus::Disk => disk(app, ui, theme, &snapshot.system),
                PerformanceFocus::Network => network(app, ui, theme, &snapshot.system),
                PerformanceFocus::Gpu => gpu(app, ui, theme, &snapshot.system),
            }
            // Every panel's own last element (the core grid, the last
            // device card, the kernel-memory row) would otherwise end
            // flush with the scroll area's own lower edge — the same
            // gap a section leaves above itself, left below the last one
            // too, so the content never reads as clipped by the pane
            // rather than simply ending.
            ui.add_space(chrome::SECTION_GAP);
            ui.min_rect()
        })
        .inner
}

/// The CPU panel.
fn cpu(app: &App, ui: &mut Ui, theme: &Palette, system: &SystemSample) {
    let name = if system.cpu.name.is_empty() {
        "Processor".to_string()
    } else {
        system.cpu.name.clone()
    };
    widgets::section(ui, theme, &name);

    // The graph and the core grid split the pane between them, in the
    // proportion their own floors give them. A tall window makes both
    // bigger rather than making the chart enormous and leaving the grid
    // the size it is on a laptop — and because the split of a pane that
    // is exactly `GRAPH_MINIMUM + grid` tall hands each of them its own
    // floor, the same arithmetic covers the short window too.
    let grid_floor = core_grid_floor(app.performance.cores.len());
    let visuals = match measured_below(ui, CPU) {
        Some(below) => (ui.available_height() - below).max(GRAPH_MINIMUM + grid_floor),
        None => GRAPH_MINIMUM + grid_floor,
    };
    let height = visuals * GRAPH_MINIMUM / (GRAPH_MINIMUM + grid_floor);

    let (rect, graph_response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), height), Sense::hover());
    graph::banded(
        ui,
        theme,
        rect,
        &graph::Graph {
            series: &app.performance.cpu,
            color: theme.series(0, 5),
            floor: 100.0,
            unit: graph::Unit::Percent,
        },
        &graph::Graph {
            series: &app.performance.cpu_kernel,
            // The kernel band takes the danger colour rather than another
            // ramp hue: a machine spending most of its CPU in the kernel
            // is a machine with a driver or a filter problem, and that is
            // a status rather than a series.
            color: theme.danger,
            floor: 100.0,
            unit: graph::Unit::Percent,
        },
    );
    graph_response.on_hover_text(format!(
        "CPU history\nCurrent {}\nRecent average {}\nRecent peak {}\nKernel now {}",
        crate::format::percent(system.cpu.total_percent),
        crate::format::percent(f64::from(app.performance.cpu.mean())),
        crate::format::percent(f64::from(app.performance.cpu.max())),
        crate::format::percent(system.cpu.kernel_percent),
    ));
    ui.add_space(SPACE_SM);

    ui.horizontal(|ui| {
        graph::legend(
            ui,
            theme,
            theme.series(0, 5),
            "Total",
            &crate::format::percent(system.cpu.total_percent),
        );
        ui.add_space(SPACE_LG);
        graph::legend(
            ui,
            theme,
            theme.danger,
            "Kernel",
            &crate::format::percent(system.cpu.kernel_percent),
        );
    });
    ui.add_space(chrome::SECTION_GAP);

    ui.horizontal_top(|ui| {
        let width = stat_column_width(ui.available_width(), 4);
        stat_column(
            ui,
            theme,
            width,
            "Current load",
            &crate::format::percent(system.cpu.total_percent),
        );
        stat_column(
            ui,
            theme,
            width,
            "User mode",
            &crate::format::percent(
                (system.cpu.total_percent - system.cpu.kernel_percent).max(0.0),
            ),
        );
        stat_column(
            ui,
            theme,
            width,
            "Recent average",
            &crate::format::percent(f64::from(app.performance.cpu.mean())),
        );
        stat_column(
            ui,
            theme,
            width,
            "Recent peak",
            &crate::format::percent(f64::from(app.performance.cpu.max())),
        );
    });
    ui.add_space(SPACE_MD);

    ui.horizontal_top(|ui| {
        let width = stat_column_width(ui.available_width(), 4);
        stat_column(
            ui,
            theme,
            width,
            "Processes",
            &crate::format::count(system.process_count as u64),
        );
        stat_column(
            ui,
            theme,
            width,
            "Threads",
            &crate::format::count(system.thread_count),
        );
        stat_column(
            ui,
            theme,
            width,
            "Handles",
            &crate::format::count(system.handle_count),
        );
        stat_column(
            ui,
            theme,
            width,
            "Up time",
            &crate::format::duration(system.uptime_seconds),
        );
    });
    ui.add_space(chrome::SECTION_GAP);

    if !app.performance.cores.is_empty() {
        widgets::section(
            ui,
            theme,
            &format!(
                "{} logical processors · {} cores · {} MHz · {} history",
                system.cpu.logical_cores,
                system.cpu.physical_cores,
                system.cpu.megahertz,
                history_window(&app.performance.cpu, sample_interval(app)),
            ),
        );
        // Measured here rather than at the end of the panel: the grid
        // below takes its own share of the remainder, so it is not one
        // of the things the remainder has to be spent on.
        record_below(ui, CPU, rect.bottom());
        let (grid, _) = ui.allocate_exact_size(
            Vec2::new(ui.available_width(), visuals - height),
            Sense::hover(),
        );
        graph::core_grid(ui, theme, grid, &app.performance.cores);
    } else {
        record_below(ui, CPU, rect.bottom());
    }
}

/// The height the per-core grid falls back to when the pane has nothing
/// spare, and the share of a taller pane it takes.
///
/// Scaled to the core count so a 4-core machine does not get a grid of
/// four tall boxes and a 64-core one does not get a grid too short to
/// see anything in. `core_grid_layout` rather than a squareness formula
/// of its own — see that function's docs on why keeping a separate copy
/// here is the bug, not just duplication.
fn core_grid_floor(cores: usize) -> f32 {
    if cores == 0 {
        return 0.0;
    }
    let (_, rows) = graph::core_grid_layout(cores);
    (rows as f32 * CORE_ROW).max(CORE_GRID_MINIMUM)
}

/// The memory panel.
fn memory(app: &App, ui: &mut Ui, theme: &Palette, system: &SystemSample) {
    widgets::section(
        ui,
        theme,
        &format!(
            "Memory — {} installed",
            crate::format::bytes(system.memory.total)
        ),
    );

    let (rect, graph_response) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), graph_height(ui, MEMORY)),
        Sense::hover(),
    );
    graph::area(
        ui,
        theme,
        rect,
        &graph::Graph {
            series: &app.performance.memory,
            color: theme.series(1, 5),
            floor: 100.0,
            unit: graph::Unit::Percent,
        },
    );
    graph_response.on_hover_text(format!(
        "Memory load history\nCurrent {}\nRecent average {}\nRecent peak {}",
        crate::format::percent(system.memory.used_percent()),
        crate::format::percent(f64::from(app.performance.memory.mean())),
        crate::format::percent(f64::from(app.performance.memory.max())),
    ));
    ui.add_space(SPACE_MD);

    widgets::meter(
        ui,
        theme,
        (system.memory.used_percent() / 100.0) as f32,
        theme.series(1, 5),
    );
    ui.add_space(SPACE_MD);

    ui.horizontal_top(|ui| {
        let width = stat_column_width(ui.available_width(), 4);
        stat_column(
            ui,
            theme,
            width,
            "In use",
            &crate::format::bytes(system.memory.used()),
        );
        stat_column(
            ui,
            theme,
            width,
            "Available",
            &crate::format::bytes(system.memory.available),
        );
        stat_column(
            ui,
            theme,
            width,
            "Committed",
            &format!(
                "{} / {}",
                crate::format::bytes(system.memory.committed),
                crate::format::bytes(system.memory.commit_limit)
            ),
        );
        stat_column(
            ui,
            theme,
            width,
            "Cached",
            &crate::format::bytes(system.memory.cached),
        );
    });
    ui.add_space(SPACE_MD);
    ui.horizontal_top(|ui| {
        let width = stat_column_width(ui.available_width(), 4);
        stat_column(
            ui,
            theme,
            width,
            "Physical load",
            &crate::format::percent(system.memory.used_percent()),
        );
        stat_column(
            ui,
            theme,
            width,
            "Commit load",
            &crate::format::percent(crate::model::percent_of(
                system.memory.committed,
                system.memory.commit_limit,
            )),
        );
        stat_column(
            ui,
            theme,
            width,
            "Recent average",
            &crate::format::percent(f64::from(app.performance.memory.mean())),
        );
        stat_column(
            ui,
            theme,
            width,
            "Recent peak",
            &crate::format::percent(f64::from(app.performance.memory.max())),
        );
    });
    ui.add_space(chrome::SECTION_GAP);

    widgets::section(ui, theme, "Kernel memory");
    ui.horizontal_top(|ui| {
        let width = stat_column_width(ui.available_width(), 4);
        stat_column(
            ui,
            theme,
            width,
            "Paged pool",
            &crate::format::bytes(system.memory.paged_pool),
        );
        // The one number that diagnoses a driver leak — the leak that
        // takes the whole machine down rather than a process.
        stat_column(
            ui,
            theme,
            width,
            "Non-paged pool",
            &crate::format::bytes(system.memory.nonpaged_pool),
        );
    });
    record_below(ui, MEMORY, rect.bottom());
}

/// The disk panel.
fn disk(app: &App, ui: &mut Ui, theme: &Palette, system: &SystemSample) {
    widgets::section(ui, theme, "Disk");

    let (rect, graph_response) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), graph_height(ui, DISK)),
        Sense::hover(),
    );
    // Read and write drawn separately, on one axis. The combined line
    // this replaces answered "is the disk busy" and hid the only thing
    // worth knowing about a busy one, which is which way the traffic is
    // going: a machine paging is reading and a machine writing a backup
    // is writing, and summed they are the same picture.
    //
    // Two ramp hues rather than a hue and a status colour, because
    // neither direction is a state and neither is a share of the other —
    // see `graph::layered`. Read keeps the panel's own hue so the
    // picker's sparkline and the line it summarises are one colour.
    graph::layered(
        ui,
        theme,
        rect,
        &[
            graph::Graph {
                series: &app.performance.disk_read,
                color: theme.series(2, 5),
                floor: 0.0,
                unit: graph::Unit::Rate,
            },
            graph::Graph {
                series: &app.performance.disk_write,
                color: theme.series(4, 5),
                floor: 0.0,
                unit: graph::Unit::Rate,
            },
        ],
    );
    graph_response.on_hover_text(format!(
        "Disk throughput history\nCurrent {}\nRecent average {}\nRecent peak {}",
        crate::format::rate(app.performance.disk.latest().into()),
        crate::format::rate(app.performance.disk.mean().into()),
        crate::format::rate(app.performance.disk.max().into()),
    ));
    // The same gap the CPU panel leaves between a graph and its legend.
    ui.add_space(SPACE_SM);

    let (read, write) = system.disks.iter().fold((0.0, 0.0), |(read, write), disk| {
        (read + disk.read_rate, write + disk.write_rate)
    });
    ui.horizontal(|ui| {
        graph::legend(
            ui,
            theme,
            theme.series(2, 5),
            "Read",
            &crate::format::rate(read),
        );
        ui.add_space(SPACE_LG);
        graph::legend(
            ui,
            theme,
            theme.series(4, 5),
            "Write",
            &crate::format::rate(write),
        );
    });
    ui.add_space(chrome::SECTION_GAP);

    let busiest = system
        .disks
        .iter()
        .map(|disk| disk.active_percent)
        .fold(0.0f64, f64::max);
    ui.horizontal_top(|ui| {
        let width = stat_column_width(ui.available_width(), 4);
        stat_column(
            ui,
            theme,
            width,
            "Current",
            &crate::format::rate(read + write),
        );
        stat_column(
            ui,
            theme,
            width,
            "Recent average",
            &crate::format::rate(app.performance.disk.mean().into()),
        );
        stat_column(
            ui,
            theme,
            width,
            "Recent peak",
            &crate::format::rate(app.performance.disk.max().into()),
        );
        stat_column(
            ui,
            theme,
            width,
            "Busiest disk",
            &crate::format::percent(busiest),
        );
    });
    ui.add_space(chrome::SECTION_GAP);

    // `if`/`else` rather than an early return: the panel has to measure
    // itself on the way out of *either* branch, and a `return` above the
    // measurement is how a panel silently stops filling its pane.
    if system.disks.is_empty() {
        widgets::empty_state(ui, theme, "No physical disks reported");
        record_below(ui, DISK, rect.bottom());
        return;
    }
    // A grid rather than a stack. A machine with three disks used to get
    // three full-width cards holding three short numbers each, which on
    // a wide window is a column of near-empty bars with the bottom two
    // thirds of the view blank.
    //
    // Busiest first, same as the network adapters: the disk someone
    // opened this panel to check is the one doing something, and a
    // laptop's NVMe boot disk sorting ahead of an idle external drive is
    // more useful than whatever order Windows happened to enumerate them
    // in.
    let mut disks = system.disks.clone();
    sort_busiest_first(
        &mut disks,
        |disk| disk.active_percent,
        |disk| disk.name.as_str(),
    );
    widgets::card_grid(ui, DEVICE_CARD_WIDTH, disks.len(), |ui, index| {
        let Some(disk) = disks.get(index) else {
            return;
        };
        chrome::panel_card(ui, theme, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(&disk.name)
                        .color(theme::rgb(theme.text))
                        .strong(),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new(crate::format::percent(disk.active_percent))
                            .color(theme::rgb(theme.text))
                            .text_style(egui::TextStyle::Monospace),
                    );
                    ui.label(
                        egui::RichText::new("active")
                            .color(theme::rgb(theme.text_muted))
                            .text_style(egui::TextStyle::Small),
                    );
                });
            });
            ui.add_space(SPACE_XS);
            widgets::meter(
                ui,
                theme,
                (disk.active_percent / 100.0) as f32,
                theme.heat((disk.active_percent / 100.0) as f32),
            );
            ui.add_space(SPACE_SM);
            ui.horizontal_top(|ui| {
                let width = stat_column_width(ui.available_width(), 2);
                stat_column(
                    ui,
                    theme,
                    width,
                    "Read",
                    &crate::format::rate(disk.read_rate),
                );
                stat_column(
                    ui,
                    theme,
                    width,
                    "Write",
                    &crate::format::rate(disk.write_rate),
                );
            });
        });
    });
    if !system.volumes.is_empty() {
        ui.add_space(SPACE_MD);
        ui.horizontal_wrapped(|ui| {
            for volume in &system.volumes {
                let used = volume.capacity.saturating_sub(volume.free);
                widgets::chip(
                    ui,
                    &format!(
                        "{}  {} used · {} free",
                        volume.letter,
                        crate::format::bytes(used),
                        crate::format::bytes(volume.free)
                    ),
                    theme.raised,
                    theme.text_muted,
                );
            }
        });
    }
    record_below(ui, DISK, rect.bottom());
}

/// The network panel: one graph, its readouts, and the machine's
/// adapter inventory.
///
/// ## Why this is a list and the disk panel is a grid
///
/// A card is the right container for an item with rich, varied content —
/// a disk has a name, an active-time meter, and directional rates.
/// Volume capacity is a separate list because it is not attributable to
/// physical disks. An adapter has a
/// name and two rates, and every adapter has exactly the same fields, so
/// a card per adapter is a box drawn around one line of text. A machine
/// with twenty adapters got twenty of those, and the two that mattered
/// were somewhere in the middle of it.
///
/// A row per adapter puts the same information in a third of the height
/// and — because every row's name starts at the same x and every row's
/// number ends at the same x — makes the column scannable, which a grid
/// of cards is not.
///
/// ## The graph can be scoped to one adapter
///
/// Clicking a row graphs that adapter instead of the machine total,
/// which is the thing the old panel could not do at all: on a machine
/// with a VPN up, "is this traffic going over the tunnel or around it"
/// is the whole question, and a single summed graph cannot answer it.
fn network(app: &mut App, ui: &mut Ui, theme: &Palette, system: &SystemSample) {
    // Ordered once, here, by a rule that reads no live value — see
    // `model::adapter_order`. Everything below indexes into this.
    let mut adapters = system.adapters.clone();
    adapters.sort_by_key(crate::model::adapter_order);

    // The selection is re-resolved against this sample rather than
    // trusted: an adapter can be removed — a USB dongle unplugged, a VPN
    // client shut down — while its row is the selected one, and a
    // selection pointing at nothing would graph an empty series with a
    // heading naming an adapter that is gone.
    let selected = app
        .performance
        .network_selected
        .filter(|luid| adapters.iter().any(|adapter| adapter.luid == *luid));
    app.performance.network_selected = selected;
    let focused = selected.and_then(|luid| adapters.iter().find(|a| a.luid == luid));

    let graph = network_graph(app, ui, theme, system, focused);
    ui.add_space(chrome::SECTION_GAP);

    if adapters.is_empty() {
        widgets::empty_state(ui, theme, "This machine reports no network adapters");
    } else {
        adapter_list(app, ui, theme, &adapters, selected);
    }
    // The graph is drawn by `network_graph` and the list under it by
    // this function, so this is the only place that can see both.
    record_below(ui, NETWORK, graph.bottom());
}

/// The Network panel's graph, its legend, and the four readouts under it.
///
/// Scoped to `focused` if an adapter is selected and to the machine
/// otherwise, so the heading, the graph and every number below it are
/// describing the same thing — a panel where the heading says "Wi-Fi"
/// and the readouts are machine totals is worse than one that cannot
/// scope at all.
fn network_graph(
    app: &App,
    ui: &mut Ui,
    theme: &Palette,
    system: &SystemSample,
    focused: Option<&crate::model::AdapterSample>,
) -> Rect {
    // Receive keeps the panel's own hue, so the picker's sparkline and
    // the line it summarises are one colour, and send takes another from
    // the ramp. Send used to be `info` — a deliberate non-ramp colour,
    // because it was drawn as a *band* under the total and a ramp hue
    // would have claimed equal billing with the whole it was part of.
    // It is not a share of anything now: the two directions are drawn as
    // independent series on one axis, which is exactly the case where
    // equal billing is what they should have.
    let receive_color = theme.series(3, 5);
    let send_color = theme.series(1, 5);

    let heading = focused.map_or_else(
        || "Network · all adapters".to_string(),
        |adapter| format!("Network · {}", adapter.name),
    );
    widgets::section(ui, theme, &heading);

    let (rect, graph_response) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), graph_height(ui, NETWORK)),
        Sense::hover(),
    );
    let scoped = focused.and_then(|adapter| app.performance.adapters.get(&adapter.luid));
    let series = scoped.unwrap_or(&app.performance.network);
    match scoped {
        // A per-adapter ring holds that adapter's combined throughput
        // and nothing else, so there is no send band to draw under it.
        Some(series) => graph::area(
            ui,
            theme,
            rect,
            &graph::Graph {
                series,
                color: receive_color,
                floor: 0.0,
                unit: graph::Unit::Rate,
            },
        ),
        None => graph::layered(
            ui,
            theme,
            rect,
            &[
                graph::Graph {
                    series: &app.performance.network_receive,
                    color: receive_color,
                    floor: 0.0,
                    unit: graph::Unit::Rate,
                },
                graph::Graph {
                    series: &app.performance.network_send,
                    color: send_color,
                    floor: 0.0,
                    unit: graph::Unit::Rate,
                },
            ],
        ),
    }
    graph_response.on_hover_text(format!(
        "Network throughput history\nCurrent {}\nRecent average {}\nRecent peak {}",
        crate::format::rate(series.latest().into()),
        crate::format::rate(series.mean().into()),
        crate::format::rate(series.max().into()),
    ));
    // The same gap the CPU panel leaves between its graph and its
    // legend, because this is the same pairing.
    ui.add_space(SPACE_SM);

    let (receive, send) = focused.map_or_else(
        || {
            (
                system.network_rate() - system.network_send_rate(),
                system.network_send_rate(),
            )
        },
        |adapter| (adapter.receive_rate, adapter.send_rate),
    );
    ui.horizontal(|ui| {
        // A per-adapter ring holds that adapter's combined throughput
        // and nothing else, so a scoped graph is one line and its legend
        // says what that line is rather than naming a direction the
        // graph is not separating.
        let (label, value) = if scoped.is_some() {
            ("Total", receive + send)
        } else {
            ("Receive", receive)
        };
        graph::legend(ui, theme, receive_color, label, &crate::format::rate(value));
        if scoped.is_none() {
            ui.add_space(SPACE_LG);
            graph::legend(ui, theme, send_color, "Send", &crate::format::rate(send));
        }
    });
    ui.add_space(chrome::SECTION_GAP);

    // Cumulative counters, summed over whatever the graph is scoped to.
    // Not "since boot": Windows resets these when an adapter comes up,
    // so the caption says what they are rather than implying an epoch
    // they do not have.
    let (in_total, out_total) = focused.map_or_else(
        || {
            let hardware = system.adapters.iter().any(|adapter| adapter.hardware);
            system
                .adapters
                .iter()
                .filter(|adapter| adapter.hardware || !hardware)
                .fold((0u64, 0u64), |(rx, tx), adapter| {
                    (
                        rx.saturating_add(adapter.received_total),
                        tx.saturating_add(adapter.sent_total),
                    )
                })
        },
        |adapter| (adapter.received_total, adapter.sent_total),
    );

    ui.horizontal_top(|ui| {
        let width = stat_column_width(ui.available_width(), 4);
        stat_column(ui, theme, width, "Receive", &crate::format::rate(receive));
        stat_column(ui, theme, width, "Send", &crate::format::rate(send));
        stat_column(
            ui,
            theme,
            width,
            "Average",
            &crate::format::rate(f64::from(series.mean())),
        );
        // The peak over the window the graph draws, which is the context
        // that makes the current number mean something: 1.3 MB/s is
        // either busy or idle depending on what this link has been doing
        // for the last minute, and the graph shows the shape of that
        // without ever stating the figure.
        stat_column(
            ui,
            theme,
            width,
            "Peak",
            &crate::format::rate(f64::from(series.max())),
        );
    });
    ui.add_space(SPACE_MD);
    ui.horizontal_top(|ui| {
        let width = stat_column_width(ui.available_width(), 4);
        stat_column(
            ui,
            theme,
            width,
            "Total received",
            &crate::format::bytes(in_total),
        );
        stat_column(
            ui,
            theme,
            width,
            "Total sent",
            &crate::format::bytes(out_total),
        );
        if let Some(adapter) = focused {
            stat_column(
                ui,
                theme,
                width,
                "Link speed",
                &crate::format::link_speed(adapter.link_speed),
            );
            stat_column(ui, theme, width, "State", adapter.state.label());
        } else {
            let connected = system
                .adapters
                .iter()
                .filter(|adapter| adapter.state.is_online())
                .count();
            stat_column(ui, theme, width, "Connected", &connected.to_string());
            stat_column(
                ui,
                theme,
                width,
                "Adapters",
                &system.adapters.len().to_string(),
            );
        }
    });
    // Handed back so `network` can measure the list it draws below it
    // against the graph's own extent.
    rect
}

/// The adapter inventory: every adapter the machine has, hardware first.
///
/// The two groups are split on [`crate::model::AdapterSample::hardware`],
/// which is a fact about the adapter. The split used to be on whether
/// the adapter had carried a byte in the last second, which is a fact
/// about the *moment*, and the result was a list that reshuffled itself
/// once a second: an adapter with intermittent traffic crossed between
/// the visible grid and the collapsed drawer on every sample, so a row
/// could be gone by the time someone finished reaching for it. Nothing
/// in this list moves now unless the machine's hardware changes.
fn adapter_list(
    app: &mut App,
    ui: &mut Ui,
    theme: &Palette,
    adapters: &[crate::model::AdapterSample],
    selected: Option<u64>,
) {
    let connected = adapters
        .iter()
        .filter(|adapter| adapter.state.is_online())
        .count();
    widgets::section(
        ui,
        theme,
        &format!(
            "Adapters · {connected} of {} connected",
            crate::format::count(adapters.len() as u64)
        ),
    );

    let physical: Vec<&crate::model::AdapterSample> =
        adapters.iter().filter(|a| a.hardware).collect();
    let virtualised: Vec<&crate::model::AdapterSample> =
        adapters.iter().filter(|a| !a.hardware).collect();

    let mut clicked = None;
    if physical.is_empty() {
        // A note rather than an empty state: the virtual-adapter list
        // below is the only list this machine has, and an empty state
        // would take the pane it is drawn in.
        widgets::empty_note(ui, theme, "No hardware adapters — every adapter is virtual");
    } else {
        chrome::panel_card(ui, theme, |ui| {
            for (index, adapter) in physical.iter().enumerate() {
                if index > 0 {
                    widgets::section_rule(ui, theme);
                }
                if adapter_row(app, ui, theme, adapter, selected == Some(adapter.luid)) {
                    clicked = Some(adapter.luid);
                }
            }
        });
    }

    if !virtualised.is_empty() {
        ui.add_space(SPACE_MD);
        let expanded = app.performance.network_virtual_expanded;
        let mut toggled = false;
        chrome::panel_card(ui, theme, |ui| {
            let header = ui.horizontal(|ui| {
                widgets::disclosure(ui, theme, expanded, "network-virtual");
                ui.add_space(SPACE_XS);
                ui.label(
                    egui::RichText::new(format!(
                        "{} virtual adapter{}",
                        virtualised.len(),
                        if virtualised.len() == 1 { "" } else { "s" }
                    ))
                    .color(theme::rgb(theme.text_muted))
                    .text_style(egui::TextStyle::Small),
                );
                // The collapsed group still carries a signal: a VPN or a
                // virtual switch moving real traffic is exactly what
                // someone opens this panel to find, and a drawer that
                // says only how many things are inside it hides that.
                let traffic: f64 = virtualised.iter().map(|adapter| adapter.total_rate()).sum();
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new(crate::format::rate_or_dash(traffic))
                            .color(theme::rgb(theme.text_faint))
                            .text_style(egui::TextStyle::Small),
                    );
                });
            });
            // Sensed across the whole line rather than only the
            // chevron's own small hit box — see `widgets::sortable_header`
            // on why a control meant to be clicked casually should not
            // require aiming at it.
            let row = ui
                .interact(
                    header.response.rect,
                    ui.id().with("network-virtual-row"),
                    Sense::click(),
                )
                .on_hover_cursor(egui::CursorIcon::PointingHand);
            toggled = row.clicked();

            if expanded {
                for adapter in &virtualised {
                    widgets::section_rule(ui, theme);
                    if adapter_row(app, ui, theme, adapter, selected == Some(adapter.luid)) {
                        clicked = Some(adapter.luid);
                    }
                }
            }
        });
        if toggled {
            app.performance.network_virtual_expanded = !expanded;
        }
    }

    if let Some(luid) = clicked {
        // Clicking the row that is already graphed goes back to the
        // machine total, so the way out is the way in. Without it the
        // only route back would be a control that exists solely to
        // undo one click.
        app.performance.network_selected = if selected == Some(luid) {
            None
        } else {
            Some(luid)
        };
    }
}

/// The height of one adapter row.
///
/// Two lines of text — the name, and the kind and hardware under it —
/// plus the module's own spacing above and below. The old card was 108
/// points tall and carried the same information.
const ADAPTER_ROW: f32 = 46.0;

/// The width an adapter row's sparkline gets.
///
/// Shorter than the picker's, because this one only has to answer "has
/// this been doing anything" rather than carry a readable shape — the
/// readable shape is what clicking the row is for.
const ADAPTER_SPARK: f32 = 84.0;

/// The width reserved at an adapter row's right edge for its two
/// readouts: the rate, and the link speed under it.
const ADAPTER_READOUT: f32 = 96.0;

/// The narrowest the name column may get before the row drops its
/// sparkline to give the space back.
///
/// A name clipped to forty points is a name nobody can read, and a
/// sparkline is the least load-bearing thing in the row — it is the
/// first thing to go when the window is narrow.
const ADAPTER_NAME_MINIMUM: f32 = 140.0;

/// The radius of the status dot at an adapter row's left edge.
const STATUS_DOT: f32 = 4.0;

/// One adapter's row. Returns whether it was clicked.
fn adapter_row(
    app: &App,
    ui: &mut Ui,
    theme: &Palette,
    adapter: &crate::model::AdapterSample,
    selected: bool,
) -> bool {
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), ADAPTER_ROW), Sense::click());
    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);

    // Keyed on the adapter, not on the row's position in the list. An id
    // built from the loop index animates the *slot*, so an adapter
    // appearing above another one hands its hover state to whatever now
    // sits where it did.
    let id = ui.id().with(("adapter", adapter.luid));
    let fill = if selected {
        theme::rgb(theme.selection)
    } else {
        widgets::hover_fill(ui, id, response.hovered(), theme.raised, theme.hover)
    };
    ui.painter()
        .rect_filled(rect, egui::CornerRadius::same(theme::RADIUS), fill);
    if selected {
        let bar = Rect::from_min_size(
            rect.left_top() + Vec2::new(0.0, SPACE_XS),
            Vec2::new(theme::SELECTION_BAR, rect.height() - SPACE_XS * 2.0),
        );
        ui.painter()
            .rect_filled(bar, egui::CornerRadius::same(2), theme::rgb(theme.accent));
    }

    let online = adapter.state.is_online();
    ui.painter().circle_filled(
        rect.left_center() + Vec2::new(SPACE_MD + STATUS_DOT, 0.0),
        STATUS_DOT,
        theme::rgb(state_color(theme, adapter.state)),
    );

    let name_left = rect.left() + SPACE_MD + STATUS_DOT * 2.0 + SPACE_SM;
    let readout_right = rect.right() - SPACE_MD;
    let spark_right = readout_right - ADAPTER_READOUT - SPACE_MD;
    // The sparkline is drawn only if the name still has room to be read
    // after it: on a narrow window the name is the part that identifies
    // the row and the sparkline is decoration.
    let roomy = spark_right - ADAPTER_SPARK - SPACE_MD - name_left >= ADAPTER_NAME_MINIMUM;
    let name_width = if roomy {
        spark_right - ADAPTER_SPARK - SPACE_MD - name_left
    } else {
        (readout_right - ADAPTER_READOUT - SPACE_MD - name_left).max(0.0)
    };

    // Only for an adapter that could be carrying traffic. A sparkline
    // over a series of zeroes is a flat rule at the foot of the row,
    // which reads as a stray divider rather than as "nothing happened" —
    // and the row already says why nothing happened, in words, on the
    // right.
    if roomy && online {
        if let Some(series) = app.performance.adapters.get(&adapter.luid) {
            let spark = Rect::from_min_max(
                egui::pos2(spark_right - ADAPTER_SPARK, rect.top() + SPACE_SM),
                egui::pos2(spark_right, rect.bottom() - SPACE_SM),
            );
            graph::sparkline(ui, spark, series, theme.series(3, 5), 0.0);
        }
    }

    // An adapter that is not passing packets is drawn in the muted text
    // colour rather than removed or greyed to invisibility: it is still
    // a fact about the machine, and someone looking for the adapter that
    // *stopped* working needs to be able to find it.
    let name_color = if online { theme.text } else { theme.text_muted };
    ui.painter().galley(
        egui::pos2(name_left, rect.top() + SPACE_SM),
        widgets::truncated(
            ui,
            &adapter.name,
            egui::TextStyle::Body.resolve(ui.style()),
            name_color,
            name_width,
        ),
        theme::rgb(name_color),
    );

    // The description is what tells "Ethernet" and "Ethernet 2" apart,
    // and the kind is what says which of them is the Wi-Fi. Neither is
    // worth a line of its own.
    let subtitle = if adapter.description.is_empty() {
        adapter.kind.label().to_string()
    } else {
        format!("{} · {}", adapter.kind.label(), adapter.description)
    };
    ui.painter().galley(
        egui::pos2(name_left, rect.bottom() - SPACE_SM - small_height(ui)),
        widgets::truncated(
            ui,
            &subtitle,
            egui::TextStyle::Small.resolve(ui.style()),
            theme.text_faint,
            name_width,
        ),
        theme::rgb(theme.text_faint),
    );

    // The rate when there is one to show, and the reason there is not
    // when there is not. A row reading "0 B/s" for a disabled adapter
    // says the adapter is idle, which is a different and wrong answer.
    let (headline, headline_color) = if online {
        (crate::format::rate(adapter.total_rate()), theme.text)
    } else {
        (adapter.state.label().to_string(), theme.text_muted)
    };
    ui.painter().text(
        egui::pos2(readout_right, rect.top() + SPACE_SM),
        egui::Align2::RIGHT_TOP,
        headline,
        if online {
            egui::TextStyle::Monospace.resolve(ui.style())
        } else {
            egui::TextStyle::Small.resolve(ui.style())
        },
        theme::rgb(headline_color),
    );
    // Only when there is a speed to state. An adapter that is down
    // reports none, and the em dash the formatter returns would sit
    // directly under the word explaining why — two ways of saying
    // "nothing here", stacked.
    if adapter.link_speed > 0 {
        ui.painter().text(
            egui::pos2(readout_right, rect.bottom() - SPACE_SM),
            egui::Align2::RIGHT_BOTTOM,
            crate::format::link_speed(adapter.link_speed),
            egui::TextStyle::Small.resolve(ui.style()),
            theme::rgb(theme.text_faint),
        );
    }

    // Everything the row had to leave out, on hover. The row is a
    // summary by design — four facts, in a fixed place, scannable down a
    // column — and this is where the rest of them live rather than in a
    // fifth column nobody has room for.
    let response = response.on_hover_text(format!(
        "{}\n{}\n{} · {}\nLink speed {}\nReceive {} · Send {}\nTotal in {} · out {}",
        adapter.name,
        adapter.description,
        adapter.kind.label(),
        adapter.state.label(),
        crate::format::link_speed(adapter.link_speed),
        crate::format::rate(adapter.receive_rate),
        crate::format::rate(adapter.send_rate),
        crate::format::bytes(adapter.received_total),
        crate::format::bytes(adapter.sent_total),
    ));
    response.clicked()
}

/// The colour of an adapter's status dot.
///
/// Three readings, not six: working, not working but could be, and not
/// there. The state's own word is beside it — see
/// [`crate::model::AdapterState::label`] — so the colour does not have
/// to carry the whole distinction, which is what stops the row being
/// unreadable to someone who cannot separate the first two hues.
fn state_color(theme: &Palette, state: crate::model::AdapterState) -> crate::color::Rgb {
    match state {
        crate::model::AdapterState::Up => theme.success,
        crate::model::AdapterState::Dormant
        | crate::model::AdapterState::Disconnected
        | crate::model::AdapterState::LowerLayerDown => theme.warning,
        crate::model::AdapterState::Disabled | crate::model::AdapterState::NotPresent => {
            theme.text_faint
        }
    }
}

/// The height of one line of the small text style.
///
/// Read from the style rather than assumed, because a row that places
/// its second line by a guessed height puts it somewhere else the moment
/// the font scale changes.
fn small_height(ui: &Ui) -> f32 {
    ui.text_style_height(&egui::TextStyle::Small)
}

/// Sorts items busiest-first by a caller-supplied figure, breaking a tie
/// alphabetically by name.
///
/// Shared by every device grid in this view — network adapters, disks,
/// GPUs — because "descending, alphabetical on a tie" is the same rule
/// three times over. The tie-break is what stops two devices tied at
/// zero from reordering between one frame and the next depending on how
/// the kernel happened to enumerate them that sample; see
/// `model::sort::SortKey`'s own tie-break for the general case of this.
fn sort_busiest_first<T>(items: &mut [T], rate: impl Fn(&T) -> f64, name: impl Fn(&T) -> &str) {
    items.sort_by(|a, b| {
        rate(b)
            .partial_cmp(&rate(a))
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| name(a).cmp(name(b)))
    });
}

/// The GPU panel.
fn gpu(app: &App, ui: &mut Ui, theme: &Palette, system: &SystemSample) {
    widgets::section(ui, theme, "GPU");

    let (rect, graph_response) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), graph_height(ui, GPU)),
        Sense::hover(),
    );
    graph::area(
        ui,
        theme,
        rect,
        &graph::Graph {
            series: &app.performance.gpu,
            color: theme.series(4, 5),
            floor: 100.0,
            unit: graph::Unit::Percent,
        },
    );
    graph_response.on_hover_text(format!(
        "GPU utilisation history\nCurrent {}\nRecent average {}\nRecent peak {}",
        crate::format::percent(f64::from(app.performance.gpu.latest())),
        crate::format::percent(f64::from(app.performance.gpu.mean())),
        crate::format::percent(f64::from(app.performance.gpu.max())),
    ));
    ui.add_space(SPACE_MD);

    if system.gpus.is_empty() {
        widgets::empty_state(
            ui,
            theme,
            "This machine's display driver does not publish GPU counters",
        );
        record_below(ui, GPU, rect.bottom());
        return;
    }
    let dedicated: u64 = system.gpus.iter().map(|gpu| gpu.memory_used).sum();
    ui.horizontal_top(|ui| {
        let width = stat_column_width(ui.available_width(), 4);
        stat_column(
            ui,
            theme,
            width,
            "Current",
            &crate::format::percent(f64::from(app.performance.gpu.latest())),
        );
        stat_column(
            ui,
            theme,
            width,
            "Recent average",
            &crate::format::percent(f64::from(app.performance.gpu.mean())),
        );
        stat_column(
            ui,
            theme,
            width,
            "Recent peak",
            &crate::format::percent(f64::from(app.performance.gpu.max())),
        );
        stat_column(
            ui,
            theme,
            width,
            "Dedicated in use",
            &crate::format::bytes(dedicated),
        );
    });
    ui.add_space(chrome::SECTION_GAP);
    // Busiest first, same reasoning as the disk grid above: on a hybrid-
    // graphics laptop the integrated adapter usually sits idle and the
    // discrete one is doing the work, and the panel should lead with
    // whichever one that is rather than the enumeration order.
    let mut gpus = system.gpus.clone();
    sort_busiest_first(&mut gpus, |gpu| gpu.utilisation, |gpu| gpu.name.as_str());
    widgets::card_grid(ui, DEVICE_CARD_WIDTH, gpus.len(), |ui, card| {
        let Some(adapter) = gpus.get(card) else {
            return;
        };
        chrome::panel_card(ui, theme, |ui| {
            ui.label(
                egui::RichText::new(&adapter.name)
                    .color(theme::rgb(theme.text))
                    .strong(),
            );
            ui.add_space(SPACE_XS);
            // Per engine, because the busiest engine is the headline and
            // *which* engine is busy is the diagnosis: a machine at 100%
            // video-decode is playing a video, and one at 100% 3D is
            // rendering.
            for (index, (engine, value)) in adapter.engines.iter().enumerate() {
                graph::legend(
                    ui,
                    theme,
                    theme.series(index, adapter.engines.len().max(1)),
                    engine,
                    &crate::format::percent(*value),
                );
            }
            if adapter.memory_used > 0 {
                ui.add_space(SPACE_XS);
                stat_column(
                    ui,
                    theme,
                    stat_column_width(ui.available_width(), 1),
                    "Dedicated memory in use",
                    &crate::format::bytes(adapter.memory_used),
                );
            }
        });
    });
    record_below(ui, GPU, rect.bottom());
}

/// The width [`stat_column`] gives each of a row's columns.
///
/// `columns` is how many the *row* divides into, which is not always how
/// many it draws. The five Performance panels' own full-width rows all
/// pass four whatever they hold, because that is what lines a
/// three-column row up with the four-column one above it — the panel is
/// one grid and a row that filled its own width would put its second
/// caption where the row above put its third.
///
/// A **card's** internal row is a different grid. It has no neighbour
/// above to line up with and about a quarter of the width, so dividing
/// its three columns by four gave each of them 120 points for a pair
/// like `"97.1 GB / 932 GB"` that needs 150 — and the pair wrapped onto
/// a second line, which is the one thing the card's width was chosen to
/// prevent.
///
/// Callers must measure `available` **once**, before the row's first
/// [`stat_column`] call, and pass the same value to every column in that
/// row. Reading `ui.available_width()` freshly inside `stat_column`
/// itself was the bug this exists to prevent: `ui.horizontal`'s own
/// available width shrinks as each sibling is allocated, so a column
/// computed after its neighbours already claimed space is a share of
/// what was left rather than a share of the row — four calls in a row
/// then produce four different widths, shrinking geometrically instead
/// of matching.
#[must_use]
fn stat_column_width(available: f32, columns: usize) -> f32 {
    // `n` columns is `n - 1` gaps between them, and `theme::apply` sets
    // `ui.horizontal`'s own `item_spacing.x` to `SPACE_SM` — so claiming
    // a plain share for each, via `set_min_width` below, asks the row for
    // `n * share + (n - 1) * SPACE_SM`, which is more than the row
    // actually has. Left in, that overshoot is exactly what pushed the
    // CPU panel's own drawn content past the window's real edge two
    // levels up the call stack.
    let columns = columns.max(1);
    let gaps = SPACE_SM * (columns - 1) as f32;
    ((available - gaps) / columns as f32).max(0.0)
}

/// One readout in a row of them.
///
/// Each takes an equal share of the row — see [`stat_column_width`] on
/// why `width` is a parameter here rather than something this function
/// measures for itself.
fn stat_column(ui: &mut Ui, theme: &Palette, width: f32, caption: &str, value: &str) -> Response {
    ui.allocate_ui_with_layout(
        Vec2::new(width, 0.0),
        egui::Layout::top_down(egui::Align::Min),
        |ui| {
            // `allocate_ui_with_layout` only gives this child a *ceiling*
            // of `width` to wrap within — the space it actually claims
            // back from the parent's cursor is its content's own
            // measured size, which for two short lines of text is far
            // narrower than a quarter of the row. Without pinning the
            // minimum too, every column ends up exactly as wide as its
            // own caption or value and no wider, so "Processes" (the
            // longest caption in its row) reads as its own column while
            // "Threads"/"Handles"/"Up time" bunch up together — four
            // ransom-note widths rather than four equal ones.
            ui.set_min_width(width);
            ui.label(
                egui::RichText::new(caption)
                    .color(theme::rgb(theme.text_muted))
                    .text_style(egui::TextStyle::Small),
            );
            ui.label(
                egui::RichText::new(value)
                    .color(theme::rgb(theme.text))
                    .text_style(egui::TextStyle::Monospace),
            );
        },
    )
    .response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_window_describes_only_samples_that_exist() {
        let mut series = Series::new(240);
        for value in [1.0, 2.0, 3.0, 4.0] {
            series.push(value);
        }
        assert_eq!(
            history_window(&series, std::time::Duration::from_millis(500)),
            "0:00:02"
        );
    }

    #[test]
    fn a_full_width_rect_inside_detail_stays_off_the_window_edge() -> anyhow::Result<()> {
        // Exercises the exact mechanism `detail()` relies on — a
        // `ScrollArea` whose ui gets `set_max_width` before anything
        // fills it — rather than re-deriving the arithmetic by hand, so
        // a change to how that composes (a different scroll style, a
        // different order of calls) would fail this test too, not just
        // a formula.
        let window_width = 1024.0;
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(
                egui::Pos2::ZERO,
                Vec2::new(window_width, 768.0),
            )),
            ..Default::default()
        };
        let mut rect = None;
        let mut output = ctx.run_ui(input, |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    let width = (ui.available_width() - SPACE_MD).max(0.0);
                    ui.set_max_width(width);
                    let (r, _) = ui
                        .allocate_exact_size(Vec2::new(ui.available_width(), 50.0), Sense::hover());
                    rect = Some(r);
                });
        });
        output.textures_delta.clear();

        let rect = rect.ok_or_else(|| anyhow::anyhow!("the rect was never allocated"))?;
        let margin = window_width - rect.right();
        assert!(
            margin >= SPACE_MD,
            "a full-width rect inside the detail column left only {margin} \
             from the window edge, wanted at least {SPACE_MD}"
        );
        Ok(())
    }

    /// A machine with enough of everything that every panel has a tail
    /// worth measuring: sixteen cores, two disks, an adapter and a GPU.
    fn a_machine() -> Snapshot {
        let mut snapshot = Snapshot::default();
        snapshot.system.cpu.logical_cores = 16;
        snapshot.system.cpu.physical_cores = 8;
        snapshot.system.disks = vec![
            crate::model::DiskSample {
                name: "NVMe boot".to_string(),
                ..Default::default()
            },
            crate::model::DiskSample {
                name: "Archive".to_string(),
                ..Default::default()
            },
        ];
        snapshot.system.adapters = vec![crate::model::AdapterSample {
            name: "Ethernet".to_string(),
            ..Default::default()
        }];
        snapshot.system.gpus = vec![crate::model::GpuSample {
            name: "Radeon".to_string(),
            engines: vec![("3D".to_string(), 12.0)],
            ..Default::default()
        }];
        snapshot
    }

    #[test]
    fn every_panel_fills_the_pane_it_is_given() -> anyhow::Result<()> {
        // The bug this guards: every panel's graph was a flat 220 points
        // whatever the window, so on anything taller than a laptop
        // screen the CPU panel drew a chart, four numbers, a core grid,
        // and then a third of a pane of nothing — on a page whose entire
        // content is one chart, which is the one thing that could have
        // used the room.
        //
        // Stated as "the content reaches the bottom of the pane" rather
        // than as an expected height, because an expected height is the
        // arithmetic being tested written down twice. It also catches
        // the opposite failure: a panel that reserves too little
        // overshoots and pushes its own readouts into a scroll.
        //
        // A tall window on purpose. At the size the app opens at these
        // panels are already full, and a test that passes because there
        // was no spare room to misplace is not testing anything.
        let window = Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(1440.0, 1600.0));
        /// What the content may miss the pane's bottom edge by: the
        /// point [`record_below`] holds back, and the rounding either
        /// side of it.
        const TOLERANCE: f32 = 4.0;

        for focus in [
            PerformanceFocus::Cpu,
            PerformanceFocus::Memory,
            PerformanceFocus::Disk,
            PerformanceFocus::Network,
            PerformanceFocus::Gpu,
        ] {
            let snapshot = a_machine();
            let mut app = App::new(crate::config::Config::default());
            let theme = app.theme.clone();
            app.performance.focus = focus;
            app.performance.cores = (0..snapshot.system.cpu.logical_cores)
                .map(|_| crate::model::history::Series::new(60))
                .collect();

            let ctx = egui::Context::default();
            theme::apply(&ctx, &theme);
            let input = egui::RawInput {
                screen_rect: Some(window),
                ..Default::default()
            };
            let mut content = None;
            // `run_ui` runs the pass loop, which is what gives a panel
            // its second pass — the one it lays itself out on. A single
            // pass would measure the panel before it knew its own tail.
            let mut output = ctx.run_ui(input, |ui| {
                content = Some(detail(&mut app, ui, &theme, &snapshot));
            });
            output.textures_delta.clear();

            let content = content.ok_or_else(|| anyhow::anyhow!("{focus:?} drew nothing"))?;
            let empty = window.bottom() - content.bottom();
            assert!(
                empty <= TOLERANCE,
                "the {focus:?} panel left {empty} points of the pane empty                  below its content; a panel takes the pane it is given"
            );
            assert!(
                empty >= -TOLERANCE,
                "the {focus:?} panel overran its pane by {} points, which                  puts its own readouts into a scroll",
                -empty
            );
        }
        Ok(())
    }

    #[test]
    fn the_picker_leaves_a_real_margin_before_the_detail_column() -> anyhow::Result<()> {
        // The picker panel's own frame carries zero margin by design —
        // its rows are meant to fill it edge to edge on three sides —
        // but the fourth side, the seam with the detail column, needs
        // its own margin rather than leaning entirely on the gap
        // `draw()` adds after the panel.
        //
        // Calls the real `picker()` directly on a `Ui` sized to the real
        // `PICKER_WIDTH`, using its own return value for the content's
        // extent: a `Panel::left` built with `.exact_size(PICKER_WIDTH)`
        // is exactly that size regardless of what is drawn inside it, so
        // `min_rect()` read from *outside* the panel reports the panel's
        // fixed shape rather than its content's, and would pass even
        // with the bug this guards against.
        let window =
            Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(PICKER_WIDTH, PICKER_ROW * 6.0));
        let mut app = App::new(crate::config::Config::default());
        let theme = app.theme.clone();
        let snapshot = Snapshot::default();
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(window),
            ..Default::default()
        };
        let mut content_rect = None;
        let mut output = ctx.run_ui(input, |ui| {
            content_rect = Some(picker(&mut app, ui, &theme, &snapshot));
        });
        output.textures_delta.clear();

        let min_rect = content_rect.ok_or_else(|| anyhow::anyhow!("picker() drew nothing"))?;
        let margin = window.right() - min_rect.right();
        assert!(
            margin >= SPACE_MD,
            "the picker's own content left only {margin} from the panel's \
             right edge (the seam with the detail column), wanted at \
             least {SPACE_MD}"
        );
        Ok(())
    }

    #[test]
    fn the_gap_between_the_picker_and_the_detail_column_is_a_real_gap() -> anyhow::Result<()> {
        // The picker's own content and the detail column's own content
        // can each have correct internal margins and the seam between
        // them still read as touching, because that seam is a *third*,
        // separate gap — `draw()`'s own `add_space` between the two —
        // and nothing checks it just because the two columns either
        // side of it are individually fine.
        //
        // Replicates `draw()`'s own structure (the picker panel, the
        // gap, `detail()`) rather than calling `draw()` itself: `draw()`
        // returns nothing, and the picker panel and `detail()` both
        // draw into the same outer `Ui`, so a plain `ui.min_rect()`
        // after both have run would report their *union* — extending
        // back to the picker panel's own left edge — not the gap
        // between them specifically. Both functions returning their own
        // content rect (see their docs) is what makes that gap
        // measurable at all.
        let window = Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(1024.0, 768.0));
        let mut app = App::new(crate::config::Config::default());
        let theme = app.theme.clone();
        let snapshot = Snapshot::default();
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(window),
            ..Default::default()
        };

        let mut picker_rect = None;
        let mut detail_rect = None;
        let mut output = ctx.run_ui(input, |ui| {
            egui::CentralPanel::default()
                .frame(theme::content(&theme))
                .show(ui, |ui| {
                    picker_panel().show(ui, |ui| {
                        picker_rect = Some(picker(&mut app, ui, &theme, &snapshot));
                    });
                    gutter_panel().show(ui, |_| {});
                    detail_rect = Some(detail(&mut app, ui, &theme, &snapshot));
                });
        });
        output.textures_delta.clear();

        let picker_rect = picker_rect.ok_or_else(|| anyhow::anyhow!("picker() drew nothing"))?;
        let detail_rect = detail_rect.ok_or_else(|| anyhow::anyhow!("detail() drew nothing"))?;
        let gap = detail_rect.left() - picker_rect.right();
        assert!(
            gap >= SPACE_LG,
            "the seam between the picker and the detail column is only \
             {gap} wide, wanted at least {SPACE_LG}"
        );
        Ok(())
    }

    #[test]
    fn the_picker_detail_seam_draws_no_separator_line_at_all() -> anyhow::Result<()> {
        // Both panels that meet at this seam are `Panel::left`, and a
        // `Panel::left` draws a separator line by default. Two of them a
        // few points apart read as one doubled border hugging whichever
        // column is on the near side — not as a gap — so the spacer's
        // was turned off first.
        //
        // The remaining one was still wrong, and was reported as such:
        // a line the pointer could grab, on a panel pinned by
        // `exact_size` that could never move. The gutter between the two
        // columns *is* the separation; a rule drawn inside it is a
        // second boundary a few points from the first.
        //
        // A rect check cannot see any of this — neither panel's content
        // rect moves — so it is verified against what actually gets
        // painted at the seam.
        let window = Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(1024.0, 768.0));
        let mut app = App::new(crate::config::Config::default());
        let theme = app.theme.clone();
        let snapshot = Snapshot::default();
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(window),
            ..Default::default()
        };

        let mut picker_rect = None;
        let mut detail_rect = None;
        let mut output = ctx.run_ui(input, |ui| {
            egui::CentralPanel::default()
                .frame(theme::content(&theme))
                .show(ui, |ui| {
                    picker_panel().show(ui, |ui| {
                        picker_rect = Some(picker(&mut app, ui, &theme, &snapshot));
                    });
                    gutter_panel().show(ui, |_| {});
                    detail_rect = Some(detail(&mut app, ui, &theme, &snapshot));
                });
        });
        output.textures_delta.clear();

        let picker_rect = picker_rect.ok_or_else(|| anyhow::anyhow!("picker() drew nothing"))?;
        let detail_rect = detail_rect.ok_or_else(|| anyhow::anyhow!("detail() drew nothing"))?;

        let seam_lines = output
            .shapes
            .iter()
            .filter(|clipped| match &clipped.shape {
                egui::Shape::LineSegment { points, .. } => {
                    let is_vertical = (points[0].x - points[1].x).abs() < 0.01;
                    let in_seam = points[0].x >= picker_rect.right() - 1.0
                        && points[0].x <= detail_rect.left() + 1.0;
                    is_vertical && in_seam
                }
                egui::Shape::Noop
                | egui::Shape::Vec(_)
                | egui::Shape::Circle(_)
                | egui::Shape::Ellipse(_)
                | egui::Shape::Path(_)
                | egui::Shape::Rect(_)
                | egui::Shape::Text(_)
                | egui::Shape::Mesh(_)
                | egui::Shape::QuadraticBezier(_)
                | egui::Shape::CubicBezier(_)
                | egui::Shape::Callback(_) => false,
            })
            .count();

        assert_eq!(
            seam_lines, 0,
            "the picker/detail seam is a gutter, not a border: expected no              separator line drawn in it, found {seam_lines}"
        );
        Ok(())
    }

    #[test]
    fn no_performance_panel_lets_its_content_reach_the_window_edge() -> anyhow::Result<()> {
        // The general form of the regression above: every resource in
        // the picker draws through the same `detail()` wrapper, and any
        // of the five can develop the same "claims more width than it
        // was given" bug `stat_column` and `graph::legend` did — this
        // drives all five through the real entry point, through the
        // real `CentralPanel` frame, at a real window size, rather than
        // picking one panel by hand and trusting the rest by
        // resemblance.
        //
        // `min_rect()`, not the painted shapes: egui clips a shape to
        // its own rect on the way to the screen, so a shape-bounds check
        // would pass even on an overflow, because the overflowing part
        // is exactly what got clipped away — invisible, not out of
        // frame. `min_rect` is the *layout* claim, made before any
        // clipping, which is where a widget claiming too much width
        // actually originates.
        let window = Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(1024.0, 768.0));
        let theme = App::new(crate::config::Config::default()).theme;

        let snapshot = Snapshot {
            system: SystemSample {
                cpu: crate::model::CpuSample {
                    logical_cores: 16,
                    physical_cores: 8,
                    ..Default::default()
                },
                disks: vec![
                    crate::model::DiskSample {
                        name: "C:".to_string(),
                        ..Default::default()
                    },
                    crate::model::DiskSample {
                        name: "D:".to_string(),
                        ..Default::default()
                    },
                ],
                adapters: vec![crate::model::AdapterSample {
                    name: "Ethernet".to_string(),
                    receive_rate: 500.0,
                    link_speed: 1_000_000_000,
                    ..Default::default()
                }],
                gpus: vec![crate::model::GpuSample {
                    name: "Test GPU".to_string(),
                    utilisation: 10.0,
                    engines: vec![("3D".to_string(), 10.0)],
                    ..Default::default()
                }],
                ..Default::default()
            },
            ..Default::default()
        };

        for focus in [
            PerformanceFocus::Cpu,
            PerformanceFocus::Memory,
            PerformanceFocus::Disk,
            PerformanceFocus::Network,
            PerformanceFocus::Gpu,
        ] {
            let mut app = App::new(crate::config::Config::default());
            app.theme = theme.clone();
            app.performance.focus = focus;
            // Sixteen cores, so the CPU panel's core grid actually draws
            // — it is the element the original report was screenshotted
            // against, and the panel skips it entirely while this is
            // empty.
            app.performance.cores = vec![crate::model::history::Series::new(60); 16];

            let ctx = egui::Context::default();
            let input = egui::RawInput {
                screen_rect: Some(window),
                ..Default::default()
            };
            let mut min_rect = None;
            let mut output = ctx.run_ui(input, |ui| {
                egui::CentralPanel::default()
                    .frame(theme::content(&theme))
                    .show(ui, |ui| {
                        min_rect = Some(detail(&mut app, ui, &theme, &snapshot));
                    });
            });
            output.textures_delta.clear();

            let min_rect =
                min_rect.ok_or_else(|| anyhow::anyhow!("{focus:?} panel drew nothing"))?;
            let margin = window.right() - min_rect.right();
            assert!(
                margin >= SPACE_MD,
                "{focus:?} panel, drawn through the real CentralPanel \
                 frame, left only {margin} from the window's right edge, \
                 wanted at least {SPACE_MD}"
            );
        }
        Ok(())
    }

    #[test]
    fn the_picker_and_detail_column_still_fit_at_the_window_s_minimum_size() -> anyhow::Result<()> {
        // Every other layout test in this file uses a comfortable
        // 1024x768 window. The app can be dragged down to
        // `gui::MIN_SIZE` — 780x480 — and nothing here has ever been
        // measured at that size: the picker's fixed 200 points, the
        // gap's fixed 20, and the nav rail's fixed 168 are all width the
        // detail column does not get to negotiate away, and a window
        // this narrow is exactly where a margin that looks fine at
        // 1024 quietly turns negative.
        //
        // Drives the real nav rail and the real `CentralPanel` too, not
        // just the picker and detail column in isolation — the rail's
        // width is exactly the kind of thing that is correct on its own
        // and only breaks its neighbour once both are drawn together.
        let window = Rect::from_min_size(
            egui::Pos2::ZERO,
            Vec2::new(crate::gui::MIN_SIZE[0], crate::gui::MIN_SIZE[1]),
        );
        let theme = App::new(crate::config::Config::default()).theme;

        // A populated snapshot, not `Snapshot::default()`: an empty one
        // draws almost nothing in `detail()`, so the trailing-margin
        // check below would pass whether or not the trim that earns it
        // is even present. Sixteen cores is what makes the CPU panel's
        // core grid actually draw — the same reason
        // `no_performance_panel_lets_its_content_reach_the_window_edge`
        // above builds one.
        let snapshot = Snapshot {
            system: SystemSample {
                cpu: crate::model::CpuSample {
                    logical_cores: 16,
                    physical_cores: 8,
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        };

        let mut app = App::new(crate::config::Config::default());
        app.theme = theme.clone();
        app.performance.cores = vec![crate::model::history::Series::new(60); 16];

        let ctx = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(window),
            ..Default::default()
        };

        let mut picker_rect = None;
        let mut detail_rect = None;
        let mut output = ctx.run_ui(input, |ui| {
            chrome::nav_rail(&mut app, ui);
            egui::CentralPanel::default()
                .frame(theme::content(&theme))
                .show(ui, |ui| {
                    picker_panel().show(ui, |ui| {
                        picker_rect = Some(picker(&mut app, ui, &theme, &snapshot));
                    });
                    gutter_panel().show(ui, |_| {});
                    detail_rect = Some(detail(&mut app, ui, &theme, &snapshot));
                });
        });
        output.textures_delta.clear();

        let picker_rect = picker_rect.ok_or_else(|| anyhow::anyhow!("picker() drew nothing"))?;
        let detail_rect = detail_rect.ok_or_else(|| anyhow::anyhow!("detail() drew nothing"))?;

        let gap = detail_rect.left() - picker_rect.right();
        assert!(
            gap >= SPACE_LG,
            "at the window's minimum size the picker/detail seam is only \
             {gap} wide, wanted at least {SPACE_LG}"
        );

        let trailing_margin = window.right() - detail_rect.right();
        assert!(
            trailing_margin >= SPACE_MD,
            "at the window's minimum size the detail column left only \
             {trailing_margin} from the window's right edge, wanted at \
             least {SPACE_MD}"
        );
        Ok(())
    }

    #[test]
    fn stat_column_width_leaves_room_for_the_three_gaps_between_four_columns() {
        // A fixed unit, not a quarter of however many columns this
        // particular row has — see the function's own docs on why a
        // three-column row still divides by four. And not a bare
        // quarter either: `set_min_width` forces every column to claim
        // this width exactly, so if the four widths plus the three gaps
        // `ui.horizontal` inserts between them summed to more than
        // `available`, the row would overflow it — which is the
        // regression this guards.
        let available = 400.0;
        for columns in 1..=4usize {
            let width = stat_column_width(available, columns);
            let claimed = columns as f32 * width + (columns - 1) as f32 * SPACE_SM;
            assert!(
                claimed <= available,
                "{columns} columns of {width} plus their {SPACE_SM}-wide                  gaps is {claimed}, which overflows the {available} available"
            );
        }
        assert!((stat_column_width(0.0, 4) - 0.0).abs() < f32::EPSILON);
        assert!(
            stat_column_width(available, 3) > stat_column_width(available, 4),
            "a card's own three-column row must give each column more than              a panel's four-column row does"
        );
    }

    #[test]
    fn a_row_of_stat_columns_are_all_the_same_width() -> anyhow::Result<()> {
        // The regression this guards: `stat_column` used to give its
        // child ui only a *ceiling* to wrap text within, and the space
        // it actually claimed back from the row was its own content's
        // measured size — so a caption longer than its neighbours (like
        // "Processes" beside "Threads"/"Handles"/"Up time") produced a
        // wider column purely because the word is longer, not because
        // the layout intended it. `ui.set_min_width` inside
        // `stat_column` is what makes the allocated width the real one.
        let app = App::new(crate::config::Config::default());
        let theme = app.theme.clone();
        let ctx = egui::Context::default();
        let mut widths = Vec::new();
        let mut output = ctx.run_ui(Default::default(), |ui| {
            ui.horizontal_top(|ui| {
                let width = stat_column_width(ui.available_width(), 4);
                // Deliberately mismatched caption and value lengths —
                // the exact shape that exposed the bug.
                for (caption, value) in [
                    ("Processes", "581"),
                    ("Threads", "9,125"),
                    ("Handles", "226,047"),
                    ("Up time", "2d 1:57:44"),
                ] {
                    let response = stat_column(ui, &theme, width, caption, value);
                    widths.push(response.rect.width());
                }
            });
        });
        output.textures_delta.clear();

        assert_eq!(widths.len(), 4);
        for (index, width) in widths.iter().enumerate() {
            assert!(
                (width - widths[0]).abs() < 0.5,
                "column {index} is {width} wide but column 0 is {}; every \
                 column in a row must be the same width regardless of its \
                 own content, got {widths:?}",
                widths[0]
            );
        }
        Ok(())
    }

    #[test]
    fn a_percentage_resource_has_a_fixed_axis_and_a_rate_does_not() {
        // Two CPU graphs are only comparable at a glance if they share an
        // axis; a network graph has no natural ceiling to share.
        assert_eq!(floor_for(PerformanceFocus::Cpu), 100.0);
        assert_eq!(floor_for(PerformanceFocus::Memory), 100.0);
        assert_eq!(floor_for(PerformanceFocus::Gpu), 100.0);
        assert_eq!(floor_for(PerformanceFocus::Disk), 0.0);
        assert_eq!(floor_for(PerformanceFocus::Network), 0.0);
    }

    #[test]
    fn every_resource_has_a_history_ring_behind_it() {
        let app = App::new(crate::config::Config::default());
        for focus in [
            PerformanceFocus::Cpu,
            PerformanceFocus::Memory,
            PerformanceFocus::Disk,
            PerformanceFocus::Network,
            PerformanceFocus::Gpu,
        ] {
            assert!(
                series_for(&app, focus).is_some(),
                "{focus:?} has no series, so its sparkline would be blank"
            );
        }
    }

    #[test]
    fn sort_busiest_first_orders_descending_with_an_alphabetical_tie_break() {
        let mut items = vec![("c", 0.0), ("a", 5.0), ("b", 0.0)];
        sort_busiest_first(&mut items, |item| item.1, |item| item.0);
        assert_eq!(
            items.iter().map(|item| item.0).collect::<Vec<_>>(),
            vec!["a", "b", "c"],
            "the busy item leads, and the two tied at zero fall back to \
             alphabetical order rather than staying in their input order"
        );
    }

    #[test]
    fn an_adapter_row_never_reaches_past_the_space_it_was_given() {
        // The row is painted rather than laid out — a name, a subtitle,
        // a sparkline and two readouts placed by arithmetic off the
        // allocated rect — so nothing in egui stops one of them being
        // computed past the row's own right edge. The name and the
        // subtitle are the two that can, because their width is what is
        // left after everything else has taken its share.
        let name_left = SPACE_MD + STATUS_DOT * 2.0 + SPACE_SM;
        for width in [320.0f32, 480.0, 640.0, 1024.0, 1920.0] {
            let readout_right = width - SPACE_MD;
            let spark_right = readout_right - ADAPTER_READOUT - SPACE_MD;
            let roomy = spark_right - ADAPTER_SPARK - SPACE_MD - name_left >= ADAPTER_NAME_MINIMUM;
            let name_width = if roomy {
                spark_right - ADAPTER_SPARK - SPACE_MD - name_left
            } else {
                (readout_right - ADAPTER_READOUT - SPACE_MD - name_left).max(0.0)
            };
            assert!(
                name_width >= 0.0,
                "a {width}-wide row gave the name a negative column"
            );
            assert!(
                name_left + name_width <= readout_right,
                "at {width} wide the name column runs to {} but the \
                 readouts start at {readout_right}",
                name_left + name_width
            );
            if roomy {
                assert!(
                    name_width >= ADAPTER_NAME_MINIMUM,
                    "at {width} wide the row kept its sparkline but left \
                     the name only {name_width}"
                );
            }
        }
    }

    #[test]
    fn the_adapter_list_does_not_reorder_when_the_traffic_changes() {
        // The regression this exists for, and the one the screenshot
        // that started this was of: the list used to be sorted
        // busiest-first and split into "active" and "idle" groups at a
        // 1 B/s threshold, so an adapter with intermittent traffic
        // crossed between the two groups — and moved within its group —
        // on every one-second sample. Rows appeared, vanished and
        // swapped places under the pointer.
        //
        // Asserted by sorting the same adapters twice with completely
        // different rates: `model::adapter_order` reads no rate at all,
        // so the two orders must be identical.
        let make = |name: &str, hardware: bool, kind: crate::model::AdapterKind, rate: f64| {
            crate::model::AdapterSample {
                luid: name.len() as u64,
                name: name.to_string(),
                kind,
                hardware,
                receive_rate: rate,
                ..crate::model::AdapterSample::default()
            }
        };
        let quiet = vec![
            make(
                "vEthernet (WSL)",
                false,
                crate::model::AdapterKind::Virtual,
                0.0,
            ),
            make("Wi-Fi", true, crate::model::AdapterKind::WiFi, 0.0),
            make("Ethernet", true, crate::model::AdapterKind::Ethernet, 0.0),
        ];
        let busy = vec![
            make(
                "vEthernet (WSL)",
                false,
                crate::model::AdapterKind::Virtual,
                9_000_000.0,
            ),
            make("Wi-Fi", true, crate::model::AdapterKind::WiFi, 4_000.0),
            make("Ethernet", true, crate::model::AdapterKind::Ethernet, 1.0),
        ];

        let order = |mut adapters: Vec<crate::model::AdapterSample>| {
            adapters.sort_by_key(crate::model::adapter_order);
            adapters
                .into_iter()
                .map(|adapter| adapter.name)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            order(quiet),
            order(busy),
            "the adapter list's order changed because the traffic did"
        );
    }

    #[test]
    fn the_picker_is_wide_enough_for_a_readable_sparkline() {
        // The sparkline gets the right 55% less a gap; under about 80
        // points there is no shape to read.
        let spark = PICKER_WIDTH * 0.55 - SPACE_SM;
        assert!(
            spark >= 80.0,
            "a {spark}-point sparkline is too small to read a shape from"
        );
    }
}

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
//! ## Readouts explain the graph
//!
//! Every resource uses the same timeline, then named groups of live
//! readings and window statistics. The grid reflows at narrow widths.
//! GPU engine activity and dedicated memory sit alongside each other
//! when the pane has room, and stack within the same scrolling body otherwise.

use super::theme::{self, SPACE_LG, SPACE_MD, SPACE_SM, SPACE_XS};
use super::{chrome, dnd, graph, icon as icons, widgets};
use crate::gui::app::{App, PerformanceFocus};
use crate::icon::Icon;
use crate::model::history::Series;
use crate::model::{Snapshot, SystemSample};
use crate::theme::Palette;
use egui::{Rect, Response, Sense, Ui, Vec2};

/// The default resource-picker width. Its divider can widen it for long device names.
const PICKER_WIDTH: f32 = 248.0;

/// The height of a picker entry.
const PICKER_ROW: f32 = 112.0;

/// The least height the main graph may shrink to.
///
/// It was the height the graph always got, and on a tall window that
/// left the bottom third of the pane empty under a panel whose whole
/// content is one chart. It is a floor now: see [`graph_height`].
const GRAPH_MINIMUM: f32 = 180.0;

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
    ui.data(|data| data.get_temp::<f32>(egui::Id::unique(panel)))
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
    ///
    /// One point was not enough, and the reason is not rounding. A
    /// panel's tail is independent of its graph for only one of the
    /// five: the CPU panel derives its core grid's height *from* the
    /// graph's, and the card panels rewrap on a width that the graph
    /// height can change. So the measure-and-redraw fixed point does not
    /// land exactly, and four of the five settled three points *past*
    /// the pane's bottom edge — inside the plus-or-minus four the panel
    /// test allows, and precisely enough to give each of them a
    /// full-height scrollbar with nothing to scroll to.
    const SLACK: f32 = 4.0;

    // The shared scroll body also owns the final inset below the content.
    let below = ui.min_rect().bottom() - graph_bottom + theme::PAD + SLACK;
    let settled = measured_below(ui, panel).is_some_and(|old| (old - below).abs() <= TOLERANCE);
    if !settled {
        ui.data_mut(|data| data.insert_temp(egui::Id::unique(panel), below));
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

    let (main, secondary) = panes(app, ui, &theme);
    ui.scope_builder(
        egui::UiBuilder::new()
            .id_salt("performance-secondary")
            .max_rect(secondary),
        |ui| {
            picker(app, ui, &theme, &snapshot);
        },
    );
    ui.scope_builder(
        egui::UiBuilder::new()
            .id_salt("performance-main")
            .max_rect(main),
        |ui| {
            detail(app, ui, &theme, &snapshot);
        },
    );
}

/// The shared splitter keeps graphs usable while the picker is resized.
fn panes(app: &mut App, ui: &mut Ui, theme: &Palette) -> (Rect, Rect) {
    dnd::split_pane(
        ui,
        theme,
        "Resize resource picker",
        &mut app.config.performance_width,
        dnd::PaneSplit {
            default_width: PICKER_WIDTH,
            min_width: 224.0,
            max_width: 360.0,
            main_min_width: 280.0,
            secondary_on_right: false,
        },
    )
}

/// The resource list down the left.
///
/// Returns the rows' own bounding rect — not used by [`draw`], which
/// discards it, but by the tests, which need the content's real extent
/// rather than its resizable pane, so the tests measure the actual
/// clearance between a card and the next pane.
fn picker(app: &mut App, ui: &mut Ui, theme: &Palette, snapshot: &Snapshot) -> Rect {
    widgets::scroll_body(ui, |ui| {
        ui.spacing_mut().item_spacing.y = SPACE_SM;
        for (index, summary) in picker_summaries(&snapshot.system).into_iter().enumerate() {
            let focus = summary.focus;
            // A machine with no GPU counters gets no GPU entry rather
            // than one permanently reading 0% — which would look like
            // a GPU that is never used.
            if focus == PerformanceFocus::Gpu && snapshot.system.gpus.is_empty() {
                continue;
            }
            if picker_entry(app, ui, theme, &summary, index) {
                app.performance.focus = focus;
            }
        }
    })
}

/// The main reading and two supporting facts shown even when a resource is inactive.
struct PickerSummary {
    focus: PerformanceFocus,
    value: String,
    context: String,
    detail: String,
    hint: &'static str,
}

/// Only snapshot facts: the summary has the same scope as its headline and history.
///
/// The two supporting lines have different jobs, and keeping them to
/// those jobs is what makes five cards readable as a column: the first
/// says *what* the resource is, the second gives the figures that
/// qualify the headline. Both are truncated to a card that is narrower
/// than a sentence, so a line spending its room on something the line
/// above already implies, or on an identifier that means nothing once
/// cut, has spent it badly.
fn picker_summaries(system: &SystemSample) -> [PickerSummary; 5] {
    use crate::format::{bytes, percent, rate, DASH};
    let cpu = &system.cpu;
    let memory = &system.memory;
    let read = system.disks.iter().map(|disk| disk.read_rate).sum::<f64>();
    let write = system.disks.iter().map(|disk| disk.write_rate).sum::<f64>();
    let disk_count = system.disks.len();
    let active = system
        .disks
        .iter()
        .map(|disk| disk.active_percent)
        .fold(0.0, f64::max);
    let has_hardware = system.adapters.iter().any(|adapter| adapter.hardware);
    let connected = system
        .adapters
        .iter()
        .filter(|adapter| (adapter.hardware || !has_hardware) && adapter.state.is_online())
        .count();
    let send = system.network_send_rate();
    let receive = (system.network_rate() - send).max(0.0);
    let busiest_adapter = system
        .adapters
        .iter()
        .filter(|adapter| (adapter.hardware || !has_hardware) && adapter.state.is_online())
        .max_by(|a, b| (a.receive_rate + a.send_rate).total_cmp(&(b.receive_rate + b.send_rate)))
        .map(|adapter| adapter.name.trim())
        .filter(|name| !name.is_empty());
    let busiest_gpu = system
        .gpus
        .iter()
        .max_by(|a, b| a.utilisation.total_cmp(&b.utilisation));
    [
        PickerSummary {
            focus: PerformanceFocus::Cpu,
            value: percent(cpu.total_percent),
            context: if cpu.name.is_empty() { "Processor details unavailable".to_string() } else { cpu.name.clone() },
            detail: cpu_shape(cpu),
            hint: "Overall utilisation across all logical processors.",
        },
        PickerSummary {
            focus: PerformanceFocus::Memory,
            value: if memory.total == 0 { DASH.to_string() } else { percent(memory.used_percent()) },
            context: if memory.total == 0 { "Physical memory unavailable".to_string() } else { format!("{} used of {}", bytes(memory.used()), bytes(memory.total)) },
            // Not "available", which the line above already states:
            // `available` is `total - used` by definition, so a card
            // carrying both says one thing twice and leaves the reader
            // to notice. The commit charge is the independent fact —
            // it counts memory the system has *promised*, pagefile
            // included, and it is the figure that runs out first on a
            // machine about to start refusing allocations.
            detail: if memory.commit_limit > 0 { format!("Committed {} of {}", bytes(memory.committed), bytes(memory.commit_limit)) }
                else if memory.total > 0 { format!("{} available", bytes(memory.available)) }
                else { "Capacity not reported".to_string() },
            hint: "Physical memory in use against installed capacity, and the commit charge against the commit limit.",
        },
        PickerSummary {
            focus: PerformanceFocus::Disk,
            value: if disk_count == 0 { DASH.to_string() } else { rate(read + write) },
            // "busiest", because the percentage is one disk's active
            // time and not the set's: five idle disks beside one at 90%
            // read as a machine 90% busy otherwise, and the hint was
            // the only thing saying so.
            context: if disk_count == 0 { "No physical disks reported".to_string() } else {
                format!("{disk_count} disk{} · busiest {} active", if disk_count == 1 { "" } else { "s" }, percent(active)) },
            // Spelled out rather than "R"/"W": the card below writes
            // "Rx"/"Tx", and a reader who has to learn two abbreviation
            // schemes for two adjacent cards learns neither.
            detail: if disk_count == 0 { "Activity unavailable".to_string() } else { format!("Read {} · Write {}", rate(read), rate(write)) },
            hint: "Combined physical disk throughput. Active time is the busiest single disk.",
        },
        PickerSummary {
            focus: PerformanceFocus::Network,
            value: if system.adapters.is_empty() { DASH.to_string() } else { rate(system.network_rate()) },
            // Named where there is a name to use. Every other card's
            // first line identifies its hardware, and a bare count left
            // this one the only card that said nothing about what it
            // was measuring. The count stays leftmost so it survives
            // the truncation that a long adapter name causes.
            context: if system.adapters.is_empty() { "No adapters reported".to_string() }
                else if let Some(name) = busiest_adapter { format!("{connected} connected · {name}") }
                else { format!("{connected} connected adapter{}", if connected == 1 { "" } else { "s" }) },
            detail: if system.adapters.is_empty() { "Traffic unavailable".to_string() } else { format!("Rx {} · Tx {}", rate(receive), rate(send)) },
            hint: "Rx: received, Tx: sent. Hardware traffic is shown when hardware adapters exist; otherwise all adapters are included.",
        },
        PickerSummary {
            focus: PerformanceFocus::Gpu,
            value: busiest_gpu.map_or_else(|| DASH.to_string(), |gpu| percent(gpu.utilisation)),
            context: busiest_gpu.map_or_else(|| "GPU counters unavailable".to_string(), |gpu| {
                match (names_its_adapter(gpu), system.gpus.len()) {
                    (true, 1) => gpu.name.clone(),
                    (true, count) => format!("{count} adapters · {}", gpu.name),
                    (false, 1) => "Graphics adapter".to_string(),
                    (false, count) => format!("{count} display adapters"),
                }
            }),
            // Never the dead line "Dedicated memory not reported",
            // which is what this card showed on every machine whose
            // driver publishes engine counters but no capacity — a
            // whole line spent saying that a line could not be filled.
            // The busiest engine is real, always present where the
            // headline is, and the fact a reader wants next: a GPU at
            // 20% on its video decoder is doing something quite
            // different from one at 20% on its 3D engine.
            detail: busiest_gpu.map_or_else(|| "Engine counters unavailable".to_string(), |gpu| {
                if gpu.memory_total > 0 { format!("VRAM {} of {}", bytes(gpu.memory_used), bytes(gpu.memory_total)) }
                else { format!("Busiest engine: {}", busiest_engine(system)) }
            }),
            hint: "Utilisation of the busiest graphics adapter, with its dedicated video memory where the driver reports one.",
        },
    ]
}

/// The processor's shape: cores, logical processors, and nominal clock.
///
/// The line above already carries the marketing core count — every
/// desktop part is sold as an "8-Core Processor" — so this line earns
/// its place by what the name does *not* say: the thread count and the
/// clock. Each part is dropped rather than guessed when its source did
/// not report it, so the line shortens instead of claiming a zero.
fn cpu_shape(cpu: &crate::model::CpuSample) -> String {
    let cores = match (cpu.physical_cores, cpu.logical_cores) {
        (0, 0) => None,
        // Spelled out when it stands alone: "16 logical" reads as an
        // abbreviation of the pair, and without the pair it is just
        // elliptical.
        (0, logical) => Some(format!("{logical} logical processors")),
        (physical, 0) => Some(format!("{physical} cores")),
        (physical, logical) => Some(format!("{physical} cores · {logical} logical")),
    };
    let clock = (cpu.megahertz > 0).then(|| crate::format::clock(cpu.megahertz));
    match (cores, clock) {
        (Some(cores), Some(clock)) => format!("{cores} · {clock}"),
        (Some(cores), None) => cores,
        (None, Some(clock)) => clock,
        (None, None) => "Core count unavailable".to_string(),
    }
}

/// Whether a sample carries a real adapter description.
///
/// `win::gpu::describe` falls back to `GPU <luid>` when the registry
/// cannot be joined to the counters, which it deliberately leaves
/// unjoined whenever more than one adapter is installed — see there. In
/// the detail panel's heading that stand-in is honest, because the whole
/// string is readable. In a card this narrow it truncates to
/// `GPU 0x00000000_0x00…`, which identifies nothing, and spends the
/// card's one identity line doing it.
///
/// Tested by containment rather than by the `GPU ` prefix so this cannot
/// quietly stop matching if that wording changes: the stand-in is the
/// only name that can hold the adapter's own LUID, since a driver's
/// `DriverDesc` never does.
fn names_its_adapter(gpu: &crate::model::GpuSample) -> bool {
    let name = gpu.name.trim();
    !name.is_empty() && !name.contains(&gpu.luid)
}

/// The engine family doing the most work, across every adapter.
///
/// Same reading and same wording as the GPU panel's own "Busiest engine"
/// row, so the card and the panel it opens cannot disagree.
fn busiest_engine(system: &SystemSample) -> String {
    system
        .gpus
        .iter()
        .flat_map(|gpu| &gpu.engines)
        .filter(|(_, value)| *value > 0.0)
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map_or_else(
            || "Idle".to_string(),
            |(name, _)| engine_family(name).to_string(),
        )
}

/// One consistent resource card: identity, current reading, context, and recent history.
fn picker_entry(
    app: &App,
    ui: &mut Ui,
    theme: &Palette,
    summary: &PickerSummary,
    index: usize,
) -> bool {
    let focus = summary.focus;
    let (label, icon) = match focus {
        PerformanceFocus::Cpu => ("CPU", Icon::Cpu),
        PerformanceFocus::Memory => ("Memory", Icon::Memory),
        PerformanceFocus::Disk => ("Disk", Icon::Disk),
        PerformanceFocus::Network => ("Network", Icon::Network),
        PerformanceFocus::Gpu => ("GPU", Icon::Gpu),
    };
    let active = app.performance.focus == focus;
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), PICKER_ROW), Sense::click());
    response.widget_info(|| {
        let mut info = egui::WidgetInfo::selected(
            egui::Role::Button,
            ui.is_enabled(),
            active,
            format!("{label}: {}", summary.value),
        );
        info.hint_text = Some(format!("{}. {}", summary.context, summary.detail));
        info
    });
    let id = ui.scope_id().with(("picker", focus));
    let fill = widgets::selectable_surface(ui, theme, id, &response, active, theme.panel);
    ui.painter()
        .rect_filled(rect, egui::CornerRadius::same(theme::RADIUS), fill);
    if active {
        widgets::accent_bar(
            ui,
            ui.painter(),
            id,
            rect,
            widgets::Bar::Inset,
            theme.accent,
        );
    }
    let color = accent_for(theme, focus, index);
    let inner = rect.shrink(SPACE_MD);
    let title_font = egui::TextStyle::Body.resolve(ui.style());
    let metric_font = theme::metric_font(ui);
    let label_width = ui
        .painter()
        .layout_no_wrap(
            label.to_string(),
            title_font.clone(),
            theme::rgb(theme.text),
        )
        .size()
        .x;
    let metric = widgets::truncated(
        ui,
        &summary.value,
        metric_font,
        theme.text,
        (inner.width() - label_width - icons::INLINE - SPACE_SM * 2.0).max(0.0),
    );
    let title_width = (inner.width() - metric.size().x - icons::INLINE - SPACE_SM * 2.0).max(0.0);
    icons::paint(
        ui.painter(),
        Rect::from_min_size(inner.min + Vec2::new(0.0, 2.0), Vec2::splat(icons::INLINE)),
        icon,
        theme::rgb(color),
    );
    ui.painter().galley(
        inner.min + Vec2::new(icons::INLINE + SPACE_SM, 0.0),
        widgets::truncated(ui, label, title_font, theme.text, title_width),
        theme::rgb(theme.text),
    );
    ui.painter().galley(
        egui::pos2(inner.right() - metric.size().x, inner.top()),
        metric,
        theme::rgb(theme.text),
    );
    for (text, top) in [
        (&summary.context, SPACE_LG + SPACE_XS),
        (&summary.detail, SPACE_LG * 2.0),
    ] {
        let galley = widgets::truncated(
            ui,
            text,
            egui::TextStyle::Small.resolve(ui.style()),
            theme.text_muted,
            inner.width(),
        );
        ui.painter().galley(
            inner.min + Vec2::new(0.0, top),
            galley,
            theme::rgb(theme.text_muted),
        );
    }
    let spark = Rect::from_min_max(
        egui::pos2(inner.left(), inner.bottom() - SPACE_LG - SPACE_XS),
        inner.max,
    );
    if let Some(series) = series_for(app, focus) {
        graph::sparkline(ui, spark, series, color, floor_for(focus));
    }
    widgets::focus_ring(ui, theme, &response);
    let history = series_for(app, focus)
        .map(|series| history_window(series, sample_interval(app)))
        .unwrap_or_default();
    response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(format!(
            "{label}: {}\n{}\n{}\n{}\n{history} history",
            summary.value, summary.context, summary.detail, summary.hint
        ))
        .clicked()
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

/// The stable nominal interval for the chart's approximate time axis.
/// Individual rate values still use each snapshot's actual elapsed interval.
fn sample_interval(app: &App) -> std::time::Duration {
    app.engine.interval()
}

/// The selected resource, drawn large.
///
/// Returns the drawn content's own bounding rect — not used by [`draw`],
/// which discards it, but by the tests, which need to measure the seam
/// with the picker column against what `detail()` actually drew rather
/// than against the outer `Ui`'s own bounds, which would also include
/// the picker panel drawn into the same `Ui` earlier.
fn detail(app: &mut App, ui: &mut Ui, theme: &Palette, snapshot: &Snapshot) -> Rect {
    detail_scroll(app, ui, theme, snapshot).inner
}

fn detail_scroll(
    app: &mut App,
    ui: &mut Ui,
    theme: &Palette,
    snapshot: &Snapshot,
) -> egui::scroll_area::ScrollAreaOutput<Rect> {
    widgets::scroll_body_output(ui, |ui| match app.performance.focus {
        PerformanceFocus::Cpu => cpu(app, ui, theme, &snapshot.system),
        PerformanceFocus::Memory => memory(app, ui, theme, &snapshot.system),
        PerformanceFocus::Disk => disk(app, ui, theme, &snapshot.system),
        PerformanceFocus::Network => network(app, ui, theme, &snapshot.system),
        PerformanceFocus::Gpu => gpu(app, ui, theme, &snapshot.system),
    })
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

    let rect = graph::timeline(
        ui,
        theme,
        height,
        &graph::Timeline {
            label: "CPU utilisation history",
            interval: sample_interval(app),
            composition: graph::Composition::PartOfTotal,
            lines: &[
                graph::TimelineLine {
                    label: "Total",
                    graph: graph::Graph {
                        series: &app.performance.cpu,
                        color: theme.series(0, 5),
                        floor: 100.0,
                        unit: graph::Unit::Percent,
                    },
                },
                graph::TimelineLine {
                    label: "Kernel",
                    graph: graph::Graph {
                        series: &app.performance.cpu_kernel,
                        color: theme.danger,
                        floor: 100.0,
                        unit: graph::Unit::Percent,
                    },
                },
            ],
        },
    );

    stats_group(
        ui,
        theme,
        "Utilisation",
        &[
            ("Current", crate::format::percent(system.cpu.total_percent)),
            (
                "User mode",
                crate::format::percent(
                    (system.cpu.total_percent - system.cpu.kernel_percent).max(0.0),
                ),
            ),
            (
                "Window average",
                crate::format::percent(app.performance.cpu.mean().into()),
            ),
            (
                "Window peak",
                crate::format::percent(app.performance.cpu.max().into()),
            ),
        ],
    );
    stats_group(
        ui,
        theme,
        "System activity",
        &[
            (
                "Processes",
                crate::format::count(system.process_count as u64),
            ),
            ("Threads", crate::format::count(system.thread_count)),
            ("Handles", crate::format::count(system.handle_count)),
            ("Up time", crate::format::duration(system.uptime_seconds)),
        ],
    );

    if !app.performance.cores.is_empty() {
        theme::gap(ui, chrome::SECTION_GAP);
        widgets::section(ui, theme, "Logical processors");
        ui.add(
            egui::Label::new(
                egui::RichText::new(format!(
                    "{} logical processors · {} · nominal {} MHz · 0–100% per tile",
                    system.cpu.logical_cores,
                    core_summary(&system.cpu),
                    system.cpu.megahertz,
                ))
                .small()
                .color(theme::rgb(theme.text_muted)),
            )
            .wrap(),
        );
        theme::gap(ui, SPACE_SM);
        // Measured here rather than at the end of the panel: the grid
        // below takes its own share of the remainder, so it is not one
        // of the things the remainder has to be spent on.
        record_below(ui, CPU, rect.bottom());
        let (grid, _) = ui.allocate_exact_size(
            Vec2::new(ui.available_width(), visuals - height),
            Sense::hover(),
        );
        graph::core_grid(
            ui,
            theme,
            grid,
            &app.performance.cores,
            &system.cpu.core_kinds,
            sample_interval(app),
        );
    } else {
        record_below(ui, CPU, rect.bottom());
    }
}

/// How the heading describes the machine's cores.
///
/// A hybrid machine gets its split named — "8 performance + 16
/// efficiency cores" — because on those parts the plain count is the
/// least interesting thing about the topology: twenty-four cores of
/// which sixteen are small is a different machine from twenty-four of
/// which none are, and the per-core grid below reads as broken until
/// somebody knows which they are looking at.
///
/// Everything else gets the count it always got. See
/// [`crate::model::CpuSample::hybrid_counts`] on why a uniform machine
/// is not described as "16 performance + 0 efficiency".
fn core_summary(cpu: &crate::model::CpuSample) -> String {
    match cpu.hybrid_counts() {
        Some((performance, efficient)) => {
            format!("{performance} performance + {efficient} efficiency cores")
        }
        None => format!("{} cores", cpu.physical_cores),
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

    let height = graph_height(ui, MEMORY);
    let rect = graph::timeline(
        ui,
        theme,
        height,
        &graph::Timeline {
            label: "Physical memory load history",
            interval: sample_interval(app),
            composition: graph::Composition::Independent,
            lines: &[graph::TimelineLine {
                label: "In use",
                graph: graph::Graph {
                    series: &app.performance.memory,
                    color: theme.series(1, 5),
                    floor: 100.0,
                    unit: graph::Unit::Percent,
                },
            }],
        },
    );

    stats_group(
        ui,
        theme,
        "Physical memory",
        &[
            ("In use", crate::format::bytes(system.memory.used())),
            ("Available", crate::format::bytes(system.memory.available)),
            ("Cached", crate::format::bytes(system.memory.cached)),
            ("Installed", crate::format::bytes(system.memory.total)),
        ],
    );
    stats_group(
        ui,
        theme,
        "Load and history",
        &[
            (
                "Physical load",
                crate::format::percent(system.memory.used_percent()),
            ),
            (
                "Commit load",
                crate::format::percent(crate::model::percent_of(
                    system.memory.committed,
                    system.memory.commit_limit,
                )),
            ),
            (
                "Window average",
                crate::format::percent(app.performance.memory.mean().into()),
            ),
            (
                "Window peak",
                crate::format::percent(app.performance.memory.max().into()),
            ),
        ],
    );
    stats_group(
        ui,
        theme,
        "Virtual and kernel memory",
        &[
            ("Committed", crate::format::bytes(system.memory.committed)),
            (
                "Commit limit",
                crate::format::bytes(system.memory.commit_limit),
            ),
            ("Paged pool", crate::format::bytes(system.memory.paged_pool)),
            (
                "Non-paged pool",
                crate::format::bytes(system.memory.nonpaged_pool),
            ),
        ],
    );
    record_below(ui, MEMORY, rect.bottom());
}

/// The disk panel.
fn disk(app: &App, ui: &mut Ui, theme: &Palette, system: &SystemSample) {
    widgets::section(ui, theme, "Disk");

    let height = graph_height(ui, DISK);
    let rect = graph::timeline(
        ui,
        theme,
        height,
        &graph::Timeline {
            label: "Physical disk throughput history",
            interval: sample_interval(app),
            composition: graph::Composition::Independent,
            lines: &[
                graph::TimelineLine {
                    label: "Read",
                    graph: graph::Graph {
                        series: &app.performance.disk_read,
                        color: theme.series(2, 5),
                        floor: 0.0,
                        unit: graph::Unit::Rate,
                    },
                },
                graph::TimelineLine {
                    label: "Write",
                    graph: graph::Graph {
                        series: &app.performance.disk_write,
                        color: theme.series(4, 5),
                        floor: 0.0,
                        unit: graph::Unit::Rate,
                    },
                },
            ],
        },
    );
    let (read, write) = system.disks.iter().fold((0.0, 0.0), |(read, write), disk| {
        (read + disk.read_rate, write + disk.write_rate)
    });

    let busiest = system
        .disks
        .iter()
        .map(|disk| disk.active_percent)
        .fold(0.0f64, f64::max);
    stats_group(
        ui,
        theme,
        "Combined disk activity",
        &[
            ("Current total", crate::format::rate(read + write)),
            (
                "Window average",
                crate::format::rate(app.performance.disk.mean().into()),
            ),
            (
                "Window peak",
                crate::format::rate(app.performance.disk.max().into()),
            ),
            ("Busiest disk", crate::format::percent(busiest)),
        ],
    );
    theme::gap(ui, chrome::SECTION_GAP);
    widgets::section(ui, theme, "Physical disks");

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
            theme::gap(ui, SPACE_XS);
            widgets::meter(
                ui,
                theme,
                (disk.active_percent / 100.0) as f32,
                theme.heat((disk.active_percent / 100.0) as f32),
            );
            theme::gap(ui, SPACE_SM);
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
        theme::gap(ui, SPACE_MD);
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
    theme::gap(ui, chrome::SECTION_GAP);

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
        || {
            if system.adapters.iter().any(|adapter| adapter.hardware) {
                "Network · hardware adapters".to_string()
            } else {
                "Network · all adapters".to_string()
            }
        },
        |adapter| format!("Network · {}", adapter.name),
    );
    widgets::section(ui, theme, &heading);

    let scoped = focused.and_then(|adapter| app.performance.adapters.get(&adapter.luid));
    let empty = Series::new(app.performance.network.capacity());
    let series = if focused.is_some() {
        scoped.unwrap_or(&empty)
    } else {
        &app.performance.network
    };
    let lines = if focused.is_some() {
        vec![graph::TimelineLine {
            label: "Total",
            graph: graph::Graph {
                series,
                color: receive_color,
                floor: 0.0,
                unit: graph::Unit::Rate,
            },
        }]
    } else {
        vec![
            graph::TimelineLine {
                label: "Receive",
                graph: graph::Graph {
                    series: &app.performance.network_receive,
                    color: receive_color,
                    floor: 0.0,
                    unit: graph::Unit::Rate,
                },
            },
            graph::TimelineLine {
                label: "Send",
                graph: graph::Graph {
                    series: &app.performance.network_send,
                    color: send_color,
                    floor: 0.0,
                    unit: graph::Unit::Rate,
                },
            },
        ]
    };
    let height = graph_height(ui, NETWORK);
    let rect = graph::timeline(
        ui,
        theme,
        height,
        &graph::Timeline {
            label: "Network throughput history",
            interval: sample_interval(app),
            composition: graph::Composition::Independent,
            lines: &lines,
        },
    );
    let (receive, send) = focused.map_or_else(
        || {
            (
                (system.network_rate() - system.network_send_rate()).max(0.0),
                system.network_send_rate(),
            )
        },
        |adapter| (adapter.receive_rate, adapter.send_rate),
    );

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

    stats_group(
        ui,
        theme,
        "Traffic",
        &[
            ("Receiving", crate::format::rate(receive)),
            ("Sending", crate::format::rate(send)),
            ("Window average", crate::format::rate(series.mean().into())),
            ("Window peak", crate::format::rate(series.max().into())),
        ],
    );
    let mut counters = vec![
        ("Total received", crate::format::bytes(in_total)),
        ("Total sent", crate::format::bytes(out_total)),
    ];
    if let Some(adapter) = focused {
        counters.push(("Link speed", crate::format::link_speed(adapter.link_speed)));
        counters.push(("State", adapter.state.label().to_string()));
    } else {
        counters.push((
            "Connected adapters",
            system
                .adapters
                .iter()
                .filter(|adapter| adapter.state.is_online())
                .count()
                .to_string(),
        ));
        counters.push(("All adapters", system.adapters.len().to_string()));
    }
    stats_group(ui, theme, "Adapter counters", &counters);
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
        theme::gap(ui, SPACE_MD);
        let expanded = app.performance.network_virtual_expanded;
        let mut toggled = false;
        chrome::panel_card(ui, theme, |ui| {
            let header = ui.horizontal(|ui| {
                if widgets::disclosure(ui, theme, expanded, "network-virtual").clicked() {
                    toggled = true;
                }
                theme::gap(ui, SPACE_XS);
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
                            .color(theme::rgb(theme.text_muted))
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
                    ui.scope_id().with("network-virtual-row"),
                    Sense::click(),
                )
                .on_hover_cursor(egui::CursorIcon::PointingHand);
            row.widget_info(|| {
                egui::WidgetInfo::selected(
                    egui::Role::DisclosureTriangle,
                    ui.is_enabled(),
                    expanded,
                    "Virtual adapters",
                )
            });
            widgets::focus_ring(ui, theme, &row);
            toggled |= row.clicked();

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
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::Role::Button, ui.is_enabled(), selected, &adapter.name)
    });
    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);

    // Keyed on the adapter, not on the row's position in the list. An id
    // built from the loop index animates the *slot*, so an adapter
    // appearing above another one hands its hover state to whatever now
    // sits where it did.
    let id = ui.scope_id().with(("adapter", adapter.luid));
    let fill = widgets::selectable_surface(ui, theme, id, &response, selected, theme.raised);
    ui.painter()
        .rect_filled(rect, egui::CornerRadius::same(theme::RADIUS), fill);
    if selected {
        widgets::accent_bar(
            ui,
            ui.painter(),
            id,
            rect,
            widgets::Bar::Inset,
            theme.accent,
        );
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
            theme.text_muted,
            name_width,
        ),
        theme::rgb(theme.text_muted),
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
            theme::rgb(theme.text_muted),
        );
    }

    // Everything the row had to leave out, on hover. The row is a
    // summary by design — four facts, in a fixed place, scannable down a
    // column — and this is where the rest of them live rather than in a
    // fifth column nobody has room for.
    let response = response.on_hover_text_at_pointer(format!(
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
    widgets::focus_ring(ui, theme, &response);
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
            theme.text_muted
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
    // The adapter's name in the heading when the machine has one, the
    // bare label when it has several.
    //
    // The CPU panel already heads itself with the processor's name, and
    // this read as an oversight beside it. It is only done for a single
    // adapter because the heading describes the *panel*, and a panel
    // covering two cards cannot be headed with one of their names — the
    // per-adapter cards below carry the names in that case, which is
    // where they belong once there is more than one.
    //
    // Distinct adapters, not distinct LUIDs: a machine can present one
    // physical GPU under two LUIDs, and heading the panel "GPU" because
    // of that would be answering a question nobody asked.
    let mut names: Vec<&str> = system.gpus.iter().map(|gpu| gpu.name.as_str()).collect();
    names.sort_unstable();
    names.dedup();
    let heading_names_the_adapter = names.len() == 1;
    match names.as_slice() {
        [only] if !only.trim().is_empty() => widgets::section(ui, theme, only),
        _ => widgets::section(ui, theme, "GPU"),
    }

    let height = graph_height(ui, GPU);
    let rect = graph::timeline(
        ui,
        theme,
        height,
        &graph::Timeline {
            label: "GPU utilisation history",
            interval: sample_interval(app),
            composition: graph::Composition::Independent,
            lines: &[graph::TimelineLine {
                label: "Busiest engine",
                graph: graph::Graph {
                    series: &app.performance.gpu,
                    color: theme.series(4, 5),
                    floor: 100.0,
                    unit: graph::Unit::Percent,
                },
            }],
        },
    );

    if system.gpus.is_empty() {
        widgets::empty_state(
            ui,
            theme,
            "This machine's display driver does not publish GPU counters",
        );
        record_below(ui, GPU, rect.bottom());
        return;
    }
    let busiest = system
        .gpus
        .iter()
        .flat_map(|gpu| &gpu.engines)
        .max_by(|a, b| a.1.total_cmp(&b.1));
    let busy_type = busiest.filter(|(_, value)| *value > 0.0).map_or_else(
        || "Idle".to_string(),
        |(name, _)| engine_family(name).to_string(),
    );
    stats_group(
        ui,
        theme,
        "GPU activity",
        &[
            (
                "Current",
                crate::format::percent(app.performance.gpu.latest().into()),
            ),
            (
                "Window average",
                crate::format::percent(app.performance.gpu.mean().into()),
            ),
            (
                "Window peak",
                crate::format::percent(app.performance.gpu.max().into()),
            ),
            ("Busiest engine", busy_type),
        ],
    );
    let mut groups: std::collections::BTreeMap<&str, Vec<&crate::model::GpuSample>> =
        std::collections::BTreeMap::new();
    for adapter in &system.gpus {
        groups.entry(&adapter.name).or_default().push(adapter);
    }
    for (name, mut adapters) in groups {
        adapters.sort_by(|a, b| a.luid.cmp(&b.luid));
        theme::gap(ui, chrome::SECTION_GAP);
        ui.push_id(("gpu-model", name), |ui| {
            if !heading_names_the_adapter { widgets::section(ui, theme, name); }
            if adapters.len() > 1 {
                ui.add(egui::Label::new(egui::RichText::new(format!("{} reported adapters share this name. Activity shows the busiest engine; memory stays separate.", adapters.len())).small().color(theme::rgb(theme.text_muted))).wrap());
                theme::gap(ui, SPACE_MD);
            }
            if ui.available_width() >= 600.0 {
                let width = (ui.available_width() - theme::PANE_GUTTER) / 2.0;
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = theme::PANE_GUTTER;
                    ui.allocate_ui_with_layout(Vec2::new(width, 0.0), egui::Layout::top_down(egui::Align::Min), |ui| {
                        ui.set_width(width);
                        gpu_engines(ui, theme, &adapters);
                    });
                    ui.allocate_ui_with_layout(Vec2::new(width, 0.0), egui::Layout::top_down(egui::Align::Min), |ui| {
                        ui.set_width(width);
                        gpu_memory(ui, theme, &adapters);
                    });
                });
            } else {
                gpu_engines(ui, theme, &adapters);
                theme::gap(ui, chrome::SECTION_GAP);
                gpu_memory(ui, theme, &adapters);
            }
        });
    }
    record_below(ui, GPU, rect.bottom());
}

fn gpu_engines(ui: &mut Ui, theme: &Palette, adapters: &[&crate::model::GpuSample]) {
    widgets::section(ui, theme, "Engine activity");
    let groups = engine_groups(adapters);
    if groups.is_empty() {
        widgets::empty_note(ui, theme, "No per-engine counters reported");
    }
    for (family, value, count) in groups {
        engine_activity(ui, theme, family, value, count);
    }
    theme::gap(ui, SPACE_MD);
    let count: usize = adapters.iter().map(|adapter| adapter.engines.len()).sum();
    egui::CollapsingHeader::new(format!("Raw engine counters ({count})"))
        .id_salt("raw-engine-counters")
        .show(ui, |ui| {
            for (index, adapter) in adapters.iter().enumerate() {
                if adapters.len() > 1 {
                    ui.label(format!("Adapter {} · {}", index + 1, adapter.luid));
                }
                for (label, value) in &adapter.engines {
                    raw_engine(ui, theme, label, *value);
                }
            }
        });
}

fn gpu_memory(ui: &mut Ui, theme: &Palette, adapters: &[&crate::model::GpuSample]) {
    widgets::section(ui, theme, "Dedicated video memory");
    for (index, adapter) in adapters.iter().enumerate() {
        if index > 0 {
            theme::gap(ui, SPACE_MD);
        }
        if adapters.len() > 1 {
            ui.label(
                egui::RichText::new(format!("Reported adapter {}", index + 1))
                    .small()
                    .color(theme::rgb(theme.text_muted)),
            )
            .on_hover_text(&adapter.luid);
        }
        let memory = if adapter.memory_total > 0 {
            vec![
                ("In use", crate::format::bytes(adapter.memory_used)),
                ("Capacity", crate::format::bytes(adapter.memory_total)),
                (
                    "Available",
                    crate::format::bytes(adapter.memory_total.saturating_sub(adapter.memory_used)),
                ),
            ]
        } else {
            vec![
                ("In use", crate::format::bytes(adapter.memory_used)),
                ("Capacity", "Not reported".to_string()),
            ]
        };
        stats_grid(ui, theme, &memory);
        if adapter.memory_total > 0 {
            theme::gap(ui, SPACE_SM);
            widgets::meter(
                ui,
                theme,
                (adapter.memory_used as f32 / adapter.memory_total as f32).clamp(0.0, 1.0),
                theme.series(4, 5),
            );
        }
    }
}

/// Readouts grouped by the question they answer, with the same adaptive grid everywhere.
fn stats_group(ui: &mut Ui, theme: &Palette, title: &str, stats: &[(&str, String)]) {
    theme::gap(ui, chrome::SECTION_GAP);
    widgets::section(ui, theme, title);
    stats_grid(ui, theme, stats);
}

fn stats_grid(ui: &mut Ui, theme: &Palette, stats: &[(&str, String)]) {
    const MIN_WIDTH: f32 = 132.0;
    let columns =
        (((ui.available_width() + SPACE_SM) / (MIN_WIDTH + SPACE_SM)).floor() as usize).clamp(1, 4);
    let width = stat_column_width(ui.available_width(), columns);
    for (index, row) in stats.chunks(columns).enumerate() {
        if index > 0 {
            theme::gap(ui, SPACE_MD);
        }
        ui.horizontal_top(|ui| {
            // Nested section layouts may use a wider pane gutter. Readout
            // widths are calculated with the shared control spacing below.
            ui.spacing_mut().item_spacing.x = SPACE_SM;
            for (label, value) in row {
                stat_column(ui, theme, width, label, value);
            }
        });
    }
}

/// Driver engine numbers identify counters, not useful top-level tasks.
fn engine_family(name: &str) -> &'static str {
    let name = name
        .split('·')
        .next()
        .unwrap_or(name)
        .trim()
        .to_ascii_lowercase();
    if name.contains("decode") {
        "Video decode"
    } else if name.contains("encode") {
        "Video encode"
    } else if name.contains("3d") {
        "3D"
    } else if name.contains("compute") {
        "Compute"
    } else if name.contains("copy") {
        "Copy"
    } else if name.contains("video") {
        "Video processing"
    } else {
        "Other"
    }
}

fn engine_groups(adapters: &[&crate::model::GpuSample]) -> Vec<(&'static str, f64, usize)> {
    let mut groups: std::collections::BTreeMap<&str, (f64, usize)> =
        std::collections::BTreeMap::new();
    for adapter in adapters {
        for (name, value) in &adapter.engines {
            let group = groups.entry(engine_family(name)).or_default();
            if value.is_finite() {
                group.0 = group.0.max(*value).clamp(0.0, 100.0);
            }
            group.1 += 1;
        }
    }
    [
        "3D",
        "Compute",
        "Copy",
        "Video decode",
        "Video encode",
        "Video processing",
        "Other",
    ]
    .into_iter()
    .filter_map(|name| {
        groups
            .get(name)
            .map(|(value, count)| (name, *value, *count))
    })
    .collect()
}

fn engine_activity(ui: &mut Ui, theme: &Palette, family: &str, value: f64, counters: usize) {
    let (rect, response) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), theme::CONTROL_HEIGHT),
        Sense::hover(),
    );
    let label_width = 104.0f32.min(rect.width() / 2.0);
    let value_width = 64.0;
    ui.painter().text(
        rect.left_center(),
        egui::Align2::LEFT_CENTER,
        family,
        egui::TextStyle::Body.resolve(ui.style()),
        theme::rgb(theme.text),
    );
    ui.painter().text(
        rect.right_center(),
        egui::Align2::RIGHT_CENTER,
        crate::format::percent(value),
        egui::TextStyle::Monospace.resolve(ui.style()),
        theme::rgb(theme.text),
    );
    let bar = Rect::from_center_size(
        egui::pos2(
            (rect.left() + label_width + rect.right() - value_width) / 2.0,
            rect.center().y,
        ),
        Vec2::new(
            (rect.width() - label_width - value_width).max(0.0),
            SPACE_SM,
        ),
    );
    ui.painter()
        .rect_filled(bar, egui::CornerRadius::same(2), theme::rgb(theme.raised));
    if value > 0.0 {
        let fill = Rect::from_min_size(
            bar.min,
            Vec2::new(bar.width() * (value as f32 / 100.0), bar.height()),
        );
        ui.painter().rect_filled(
            fill,
            egui::CornerRadius::same(2),
            theme::rgb(theme.series(4, 5)),
        );
    }
    response.on_hover_text(format!("{family}: {}. Busiest of {counters} reported engine counters; parallel counters are not added.", crate::format::percent(value)));
}

fn raw_engine(ui: &mut Ui, theme: &Palette, label: &str, value: f64) {
    let (rect, response) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), theme::ROW_HEIGHT),
        Sense::hover(),
    );
    let value = ui.painter().layout_no_wrap(
        crate::format::percent(value),
        egui::TextStyle::Monospace.resolve(ui.style()),
        theme::rgb(theme.text),
    );
    let label_galley = widgets::truncated(
        ui,
        label,
        egui::TextStyle::Small.resolve(ui.style()),
        theme.text_muted,
        rect.width() - value.size().x - SPACE_SM,
    );
    ui.painter().galley(
        egui::pos2(rect.left(), rect.center().y - label_galley.size().y / 2.0),
        label_galley,
        theme::rgb(theme.text_muted),
    );
    ui.painter().galley(
        egui::pos2(
            rect.right() - value.size().x,
            rect.center().y - value.size().y / 2.0,
        ),
        value,
        theme::rgb(theme.text),
    );
    response.on_hover_text(label);
}

/// Equal column widths for one readout row, after its shared gaps.
/// Main statistics choose their column count from the pane width;
/// device-card rows pass their own count without shrinking later siblings.
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
                    .font(theme::metric_font(ui)),
            );
        },
    )
    .response
}

#[cfg(test)]
mod tests {
    #[test]
    fn nested_memory_readouts_do_not_inherit_the_pane_gutter() -> anyhow::Result<()> {
        let ctx = egui::Context::default();
        let app = App::new(crate::config::Config::default());
        theme::apply(&ctx, &app.theme);
        let mut drawn = None;
        let mut output = ctx.run_ui(Default::default(), |ui| {
            let pane = Rect::from_min_size(ui.cursor().min, Vec2::new(522.0, 400.0));
            ui.scope_builder(egui::UiBuilder::new().max_rect(pane), |ui| {
                ui.spacing_mut().item_spacing.x = theme::PANE_GUTTER;
                let adapter = crate::model::GpuSample {
                    memory_used: 1_460_000_000,
                    memory_total: 8 * 1024 * 1024 * 1024,
                    ..Default::default()
                };
                gpu_memory(ui, &app.theme, &[&adapter, &adapter]);
                drawn = Some(ui.min_rect());
            });
        });
        output.textures_delta.clear();
        let drawn = drawn.ok_or_else(|| anyhow::anyhow!("memory readouts drew nothing"))?;
        assert!(
            drawn.width() <= 522.5,
            "memory readouts expanded to {}",
            drawn.width()
        );
        Ok(())
    }

    #[test]
    fn short_network_inventory_only_scrolls_when_its_content_exceeds_the_pane() -> anyhow::Result<()>
    {
        use crate::model::{AdapterKind, AdapterSample, AdapterState};
        for size in [
            Vec2::new(1180.0, 800.0),
            Vec2::new(1480.0, 800.0),
            Vec2::new(1440.0, 900.0),
        ] {
            let ctx = egui::Context::default();
            crate::gui::font::install(&ctx, crate::gui::DEFAULT_FAMILY);
            let mut app = App::new(crate::config::Config::default());
            app.performance.focus = PerformanceFocus::Network;
            for _ in 0..240 {
                app.performance.network.push(4096.0);
                app.performance.network_receive.push(3584.0);
                app.performance.network_send.push(512.0);
            }
            let mut adapters = vec![
                AdapterSample {
                    name: "Ethernet".into(),
                    hardware: true,
                    kind: AdapterKind::Ethernet,
                    state: AdapterState::Disconnected,
                    ..Default::default()
                },
                AdapterSample {
                    name: "Wi-Fi".into(),
                    hardware: true,
                    kind: AdapterKind::WiFi,
                    state: AdapterState::Up,
                    receive_rate: 3584.0,
                    send_rate: 512.0,
                    ..Default::default()
                },
            ];
            adapters.extend((0..17).map(|index| AdapterSample {
                name: format!("Virtual {index}"),
                luid: index + 2,
                kind: AdapterKind::Virtual,
                ..Default::default()
            }));
            let snapshot = Snapshot {
                system: SystemSample {
                    adapters,
                    ..Default::default()
                },
                ..Default::default()
            };
            let theme = app.theme.clone();
            theme::apply(&ctx, &theme);
            let mut metrics = None;
            for _ in 0..4 {
                let mut output = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, size)),
                        ..Default::default()
                    },
                    |ui| {
                        let pane = Rect::from_min_max(
                            egui::pos2(344.0, 40.0),
                            egui::pos2(size.x - 12.0, size.y - 46.0),
                        );
                        ui.scope_builder(egui::UiBuilder::new().max_rect(pane), |ui| {
                            let output = detail_scroll(&mut app, ui, &theme, &snapshot);
                            metrics = Some((output.content_size.y, output.inner_rect.height()));
                        });
                    },
                );
                output.textures_delta.clear();
            }
            let (content, viewport) =
                metrics.ok_or_else(|| anyhow::anyhow!("no scroll metrics"))?;
            assert!(content <= viewport + 0.5, "at {size:?}, content {content} exceeds viewport {viewport} by {} despite a short inventory", content - viewport);

            app.performance.network_virtual_expanded = true;
            let mut expanded_content = 0.0;
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, size)),
                    ..Default::default()
                },
                |ui| {
                    let pane = Rect::from_min_max(
                        egui::pos2(344.0, 40.0),
                        egui::pos2(size.x - 12.0, size.y - 46.0),
                    );
                    ui.scope_builder(egui::UiBuilder::new().max_rect(pane), |ui| {
                        expanded_content = detail_scroll(&mut app, ui, &theme, &snapshot)
                            .content_size
                            .y;
                    });
                },
            );
            output.textures_delta.clear();
            assert!(
                expanded_content > viewport,
                "expanded adapter details must remain scrollable"
            );
        }
        Ok(())
    }
    #[test]
    fn gpu_engine_groups_collapse_driver_numbers_without_summing_parallel_engines() {
        let first = crate::model::GpuSample {
            engines: vec![
                ("3D · 0:0".into(), 35.0),
                ("3D · 0:1".into(), 40.0),
                ("Compute 0 · 0:6".into(), 70.0),
                ("VideoDecode · 0:11".into(), 12.0),
            ],
            ..Default::default()
        };
        let second = crate::model::GpuSample {
            engines: vec![
                ("High Priority 3D · 0:2".into(), 20.0),
                ("Compute 1 · 0:7".into(), 10.0),
                ("True Audio · 0:3".into(), 0.0),
            ],
            ..Default::default()
        };
        assert_eq!(
            engine_groups(&[&first, &second]),
            vec![
                ("3D", 40.0, 3),
                ("Compute", 70.0, 2),
                ("Video decode", 12.0, 1),
                ("Other", 0.0, 1),
            ]
        );
    }
    #[test]
    fn resource_summaries_distinguish_unavailable_data_from_zero_usage() {
        let summaries = picker_summaries(&SystemSample::default());
        for summary in summaries.iter().skip(1) {
            assert_eq!(
                summary.value,
                crate::format::DASH,
                "missing {:?} data must not look like zero activity",
                summary.focus
            );
            assert!(!summary.context.is_empty() && !summary.detail.is_empty());
        }
        let system = SystemSample {
            memory: crate::model::MemorySample {
                total: 8 * 1024 * 1024 * 1024,
                available: 8 * 1024 * 1024 * 1024,
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(
            picker_summaries(&system)[1].value,
            "0%",
            "a valid idle reading remains zero"
        );
    }

    #[test]
    fn network_card_details_use_the_same_adapters_as_the_headline() {
        use crate::model::{AdapterSample, AdapterState};
        let system = SystemSample {
            adapters: vec![
                AdapterSample {
                    hardware: true,
                    state: AdapterState::Up,
                    receive_rate: 2048.0,
                    send_rate: 1024.0,
                    ..Default::default()
                },
                AdapterSample {
                    hardware: false,
                    state: AdapterState::Up,
                    receive_rate: 100_000_000.0,
                    send_rate: 50_000_000.0,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let summaries = picker_summaries(&system);
        let network = &summaries[3];
        assert_eq!(network.value, crate::format::rate(3072.0));
        assert_eq!(network.context, "1 connected adapter");
        assert_eq!(
            network.detail,
            format!(
                "Rx {} · Tx {}",
                crate::format::rate(2048.0),
                crate::format::rate(1024.0)
            ),
            "virtual traffic must not be counted again in the supporting readings"
        );
        let guest = SystemSample {
            adapters: vec![system.adapters[1].clone()],
            ..Default::default()
        };
        assert_eq!(
            picker_summaries(&guest)[3].value,
            crate::format::rate(150_000_000.0),
            "virtual-only guests keep their real traffic"
        );
    }

    #[test]
    fn no_card_identifies_its_hardware_by_a_luid_it_cannot_finish_printing() {
        // `win::gpu::describe` leaves an adapter named `GPU <luid>`
        // whenever the registry cannot be joined to the counters, which
        // is every machine with more than one display adapter. That
        // stand-in is readable in the detail panel's heading and is not
        // readable here: the card truncated it to
        // `3 adapters · GPU 0x00000000_0x00…`, spending its one identity
        // line on nothing at all.
        let unnamed = |luid: &str| crate::model::GpuSample {
            luid: luid.to_string(),
            name: format!("GPU {luid}"),
            utilisation: 20.0,
            engines: vec![("VideoDecode · 0:3".to_string(), 20.0)],
            ..Default::default()
        };
        let system = SystemSample {
            gpus: vec![
                unnamed("luid_0x00000000_0x0000c4f1"),
                unnamed("luid_0x00000000_0x0000c502"),
                unnamed("luid_0x00000000_0x0000c513"),
            ],
            ..Default::default()
        };
        let gpu = &picker_summaries(&system)[4];
        assert_eq!(gpu.context, "3 display adapters");
        assert!(
            !gpu.context.contains("0x"),
            "a half-printed LUID identifies nothing: {}",
            gpu.context
        );
        assert_eq!(
            gpu.detail, "Busiest engine: Video decode",
            "a driver that publishes engines but no capacity must still \
             fill the second line with something true"
        );

        let named = SystemSample {
            gpus: vec![crate::model::GpuSample {
                luid: "luid_0x00000000_0x0000c4f1".to_string(),
                name: "NVIDIA GeForce RTX 4070".to_string(),
                memory_used: 3_221_225_472,
                memory_total: 12_884_901_888,
                ..Default::default()
            }],
            ..Default::default()
        };
        let gpu = &picker_summaries(&named)[4];
        assert_eq!(gpu.context, "NVIDIA GeForce RTX 4070");
        assert_eq!(
            gpu.detail,
            format!(
                "VRAM {} of {}",
                crate::format::bytes(3_221_225_472),
                crate::format::bytes(12_884_901_888)
            ),
            "a reported capacity is the more useful second line"
        );
    }

    #[test]
    fn a_supporting_line_never_restates_the_line_above_it() {
        // Available memory is `total - available` run backwards: a card
        // showing "21.9 GB used of 47.9 GB" above "26.0 GB available"
        // spends its second line on arithmetic the reader can do from
        // the first. The commit charge cannot be derived from either.
        let system = SystemSample {
            memory: crate::model::MemorySample {
                total: 48 * 1024 * 1024 * 1024,
                available: 26 * 1024 * 1024 * 1024,
                committed: 24 * 1024 * 1024 * 1024,
                commit_limit: 56 * 1024 * 1024 * 1024,
                ..Default::default()
            },
            cpu: crate::model::CpuSample {
                name: "AMD Ryzen 7 3700X 8-Core Processor".to_string(),
                physical_cores: 8,
                logical_cores: 16,
                megahertz: 3593,
                ..Default::default()
            },
            ..Default::default()
        };
        let summaries = picker_summaries(&system);
        assert_eq!(
            summaries[1].detail,
            format!(
                "Committed {} of {}",
                crate::format::bytes(24 * 1024 * 1024 * 1024),
                crate::format::bytes(56 * 1024 * 1024 * 1024)
            )
        );
        assert!(
            !summaries[1].detail.contains("available"),
            "the available figure is the first line restated"
        );
        // Likewise the processor: its name already says "8-Core", so the
        // line under it carries what the name cannot.
        assert_eq!(summaries[0].detail, "8 cores · 16 logical · 3.59 GHz");
    }

    #[test]
    fn a_processor_line_shortens_rather_than_reporting_a_zero_it_did_not_read() {
        let shape = |physical, logical, megahertz| {
            cpu_shape(&crate::model::CpuSample {
                physical_cores: physical,
                logical_cores: logical,
                megahertz,
                ..Default::default()
            })
        };
        assert_eq!(shape(8, 16, 3593), "8 cores · 16 logical · 3.59 GHz");
        assert_eq!(
            shape(8, 16, 0),
            "8 cores · 16 logical",
            "an unreadable clock drops out rather than reading 0 MHz"
        );
        assert_eq!(
            shape(0, 16, 3593),
            "16 logical processors · 3.59 GHz",
            "a thread count standing alone is spelled out"
        );
        assert_eq!(
            shape(0, 0, 3593),
            "3.59 GHz",
            "a clock alone still beats an empty line"
        );
        assert_eq!(shape(0, 0, 0), "Core count unavailable");
    }

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
        // Measure actual card content, not the enclosing pane's width:
        // the shared scroll body must retain clearance on its trailing edge.
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
        // Measure both panes through their real shared splitter. Independent
        // scope IDs keep scroll state separate while the gutter stays usable.
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
                    let (main, secondary) = panes(&mut app, ui, &theme);
                    ui.scope_builder(
                        egui::UiBuilder::new()
                            .id_salt("performance-secondary")
                            .max_rect(secondary),
                        |ui| {
                            picker_rect = Some(picker(&mut app, ui, &theme, &snapshot));
                        },
                    );
                    ui.scope_builder(
                        egui::UiBuilder::new()
                            .id_salt("performance-main")
                            .max_rect(main),
                        |ui| {
                            detail_rect = Some(detail(&mut app, ui, &theme, &snapshot));
                        },
                    );
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
    fn the_picker_detail_seam_has_no_full_height_rule_at_rest() -> anyhow::Result<()> {
        // A short grip advertises the real divider at rest; the full-height
        // line appears only during hover, drag, or keyboard focus.
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
                    let (main, secondary) = panes(&mut app, ui, &theme);
                    ui.scope_builder(
                        egui::UiBuilder::new()
                            .id_salt("performance-secondary")
                            .max_rect(secondary),
                        |ui| {
                            picker_rect = Some(picker(&mut app, ui, &theme, &snapshot));
                        },
                    );
                    ui.scope_builder(
                        egui::UiBuilder::new()
                            .id_salt("performance-main")
                            .max_rect(main),
                        |ui| {
                            detail_rect = Some(detail(&mut app, ui, &theme, &snapshot));
                        },
                    );
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
                | egui::Shape::Band(_)
                | egui::Shape::Callback(_) => false,
            })
            .count();

        assert_eq!(
            seam_lines, 0,
            "expected no full-height divider at rest, found {seam_lines}"
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
        // measured at that size: the picker's default width, the
        // shared pane gutter, and the expanded navigation rail are all width the
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
                    let (main, secondary) = panes(&mut app, ui, &theme);
                    ui.scope_builder(
                        egui::UiBuilder::new()
                            .id_salt("performance-secondary")
                            .max_rect(secondary),
                        |ui| {
                            picker_rect = Some(picker(&mut app, ui, &theme, &snapshot));
                        },
                    );
                    ui.scope_builder(
                        egui::UiBuilder::new()
                            .id_salt("performance-main")
                            .max_rect(main),
                        |ui| {
                            detail_rect = Some(detail(&mut app, ui, &theme, &snapshot));
                        },
                    );
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
        // The chart fills its own line inside the row's two insets and
        // the picker's trailing margin; under 80 points it loses its shape.
        let spark = PICKER_WIDTH - SPACE_MD * 3.0;
        assert!(
            spark >= 80.0,
            "a {spark}-point sparkline is too small to read a shape from"
        );
    }
}

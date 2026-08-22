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

/// The height the main graph gets.
const GRAPH_HEIGHT: f32 = 220.0;

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

    egui::Panel::left("performance-picker")
        .exact_size(PICKER_WIDTH)
        .frame(egui::Frame::new().inner_margin(theme::margin_xy(0.0, 0.0)))
        .show(ui, |ui| {
            picker(app, ui, &theme, &snapshot);
        });

    // A second, empty docked panel, purely to reserve a horizontal
    // gutter — not `ui.add_space(SPACE_LG)`, which reads as the right
    // call and does nothing at all here. `add_space` advances the
    // cursor along the *current layout's* axis, and the layout `ui`
    // carries past a docked `Panel::left` is still the page's own
    // top-down one; the panel changes what area remains, not which
    // direction spacing moves in. So the two columns had no gap between
    // them beyond how each rounded off its own edge, however each of
    // those was written — which is what made this so easy to miss: both
    // sides could be independently correct and the seam still read as
    // touching.
    egui::Panel::left("performance-picker-gap")
        .exact_size(SPACE_LG)
        .frame(egui::Frame::new())
        .show_separator_line(false)
        .show(ui, |_| {});
    detail(app, ui, &theme, &snapshot);
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
                    crate::format::rate(
                        snapshot
                            .system
                            .adapters
                            .iter()
                            .map(crate::model::AdapterSample::total_rate)
                            .sum(),
                    ),
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
    ui.painter().text(
        rect.left_bottom() + Vec2::new(SPACE_MD, -SPACE_SM),
        egui::Align2::LEFT_BOTTOM,
        value,
        egui::TextStyle::Small.resolve(ui.style()),
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

    let (rect, _) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), GRAPH_HEIGHT),
        Sense::hover(),
    );
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

    ui.horizontal(|ui| {
        let width = stat_column_width(ui.available_width());
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
                "{} logical processors · {} cores · {} MHz",
                system.cpu.logical_cores, system.cpu.physical_cores, system.cpu.megahertz
            ),
        );
        // Height scaled to the core count so a 4-core machine does not
        // get a grid of four tall boxes and a 64-core one does not get a
        // grid too short to see anything in. `core_grid_layout` rather
        // than a squareness formula of its own — see that function's
        // docs on why keeping a separate copy here is the bug, not just
        // duplication.
        let (_, rows) = graph::core_grid_layout(app.performance.cores.len());
        let height = (rows as f32 * 46.0).clamp(90.0, 320.0);
        let (rect, _) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), height), Sense::hover());
        graph::core_grid(ui, theme, rect, &app.performance.cores);
    }
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

    let (rect, _) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), GRAPH_HEIGHT),
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
    ui.add_space(SPACE_MD);

    widgets::meter(
        ui,
        theme,
        (system.memory.used_percent() / 100.0) as f32,
        theme.series(1, 5),
    );
    ui.add_space(SPACE_MD);

    ui.horizontal(|ui| {
        let width = stat_column_width(ui.available_width());
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
    ui.add_space(chrome::SECTION_GAP);

    widgets::section(ui, theme, "Kernel memory");
    ui.horizontal(|ui| {
        let width = stat_column_width(ui.available_width());
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
}

/// The disk panel.
fn disk(app: &App, ui: &mut Ui, theme: &Palette, system: &SystemSample) {
    widgets::section(ui, theme, "Disk");

    let (rect, _) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), GRAPH_HEIGHT),
        Sense::hover(),
    );
    graph::area(
        ui,
        theme,
        rect,
        &graph::Graph {
            series: &app.performance.disk,
            color: theme.series(2, 5),
            floor: 0.0,
            unit: graph::Unit::Rate,
        },
    );
    ui.add_space(SPACE_MD);

    if system.disks.is_empty() {
        widgets::empty_state(ui, theme, "No physical disks reported");
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
            ui.horizontal(|ui| {
                let width = stat_column_width(ui.available_width());
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
                stat_column(
                    ui,
                    theme,
                    width,
                    "Free",
                    &format!(
                        "{} / {}",
                        crate::format::bytes(disk.free),
                        crate::format::bytes(disk.capacity)
                    ),
                );
            });
        });
    });
}

/// The network panel.
fn network(app: &mut App, ui: &mut Ui, theme: &Palette, system: &SystemSample) {
    widgets::section(ui, theme, "Network");

    let (rect, _) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), GRAPH_HEIGHT),
        Sense::hover(),
    );
    graph::area(
        ui,
        theme,
        rect,
        &graph::Graph {
            series: &app.performance.network,
            color: theme.series(3, 5),
            floor: 0.0,
            unit: graph::Unit::Rate,
        },
    );
    ui.add_space(SPACE_MD);

    if system.adapters.is_empty() {
        widgets::empty_state(ui, theme, "No connected adapters");
        return;
    }

    // Busiest first, and split at the last one carrying any traffic. A
    // machine running Hyper-V, WSL or a VPN client reports a real,
    // throughput-less adapter for every virtual switch and filter driver
    // bound to the physical one — two dozen is routine — and a grid that
    // draws every one of them as a full card is a wall of near-identical
    // boxes with the two that matter lost somewhere in it.
    let (active, idle) = split_by_activity(system.adapters.clone());

    if active.is_empty() {
        widgets::empty_state(ui, theme, "No adapters are currently active");
    } else {
        widgets::card_grid(ui, DEVICE_CARD_WIDTH, active.len(), |ui, index| {
            let Some(adapter) = active.get(index) else {
                return;
            };
            chrome::panel_card(ui, theme, |ui| {
                ui.label(
                    egui::RichText::new(&adapter.name)
                        .color(theme::rgb(theme.text))
                        .strong(),
                );
                ui.add_space(SPACE_XS);
                ui.horizontal(|ui| {
                    let width = stat_column_width(ui.available_width());
                    stat_column(
                        ui,
                        theme,
                        width,
                        "Receive",
                        &crate::format::rate(adapter.receive_rate),
                    );
                    stat_column(
                        ui,
                        theme,
                        width,
                        "Send",
                        &crate::format::rate(adapter.send_rate),
                    );
                    stat_column(
                        ui,
                        theme,
                        width,
                        "Link speed",
                        // Link speed is in bits per second; the byte
                        // formatter would report a gigabit adapter as
                        // "119 MB/s", which is right and reads as wrong.
                        &format!("{} Mbps", adapter.link_speed / 1_000_000),
                    );
                });
            });
        });
    }

    if !idle.is_empty() {
        ui.add_space(SPACE_MD);
        idle_adapters(app, ui, theme, &idle);
    }
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

/// Sorts adapters busiest-first and splits them at the last one carrying
/// at least a byte a second of traffic — the same floor
/// [`crate::format::rate`] rounds to `"0 B/s"` at, so a card never reads
/// as active while showing two zeroes.
fn split_by_activity(
    mut adapters: Vec<crate::model::AdapterSample>,
) -> (
    Vec<crate::model::AdapterSample>,
    Vec<crate::model::AdapterSample>,
) {
    sort_busiest_first(
        &mut adapters,
        crate::model::AdapterSample::total_rate,
        |adapter| adapter.name.as_str(),
    );
    let split = adapters.partition_point(|adapter| adapter.total_rate() >= 1.0);
    let idle = adapters.split_off(split);
    (adapters, idle)
}

/// The idle adapters, collapsed behind a disclosure by default.
///
/// One line per adapter rather than a card: a card's three stat columns
/// exist to hold live numbers, and an idle adapter has none — a card
/// that draws a name and nothing else is the same box that started this,
/// just smaller.
fn idle_adapters(
    app: &mut App,
    ui: &mut Ui,
    theme: &Palette,
    idle: &[crate::model::AdapterSample],
) {
    let expanded = app.performance.network_idle_expanded;
    chrome::panel_card(ui, theme, |ui| {
        let header = ui.horizontal(|ui| {
            widgets::disclosure(ui, theme, expanded, "network-idle");
            ui.add_space(SPACE_XS);
            ui.label(
                egui::RichText::new(format!(
                    "{} inactive adapter{}",
                    idle.len(),
                    if idle.len() == 1 { "" } else { "s" }
                ))
                .color(theme::rgb(theme.text_muted))
                .text_style(egui::TextStyle::Small),
            );
        });
        // Sensed across the whole line rather than only the chevron's own
        // small hit box — see `widgets::sortable_header` on why a control
        // meant to be clicked casually should not require aiming at it.
        let row = ui
            .interact(
                header.response.rect,
                ui.id().with("network-idle-row"),
                Sense::click(),
            )
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        if row.clicked() {
            app.performance.network_idle_expanded = !expanded;
        }

        if expanded {
            ui.add_space(SPACE_XS);
            for adapter in idle {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(&adapter.name)
                            .color(theme::rgb(theme.text_muted))
                            .text_style(egui::TextStyle::Small),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            egui::RichText::new(format!("{} Mbps", adapter.link_speed / 1_000_000))
                                .color(theme::rgb(theme.text_faint))
                                .text_style(egui::TextStyle::Small),
                        );
                    });
                });
            }
        }
    });
}

/// The GPU panel.
fn gpu(app: &App, ui: &mut Ui, theme: &Palette, system: &SystemSample) {
    widgets::section(ui, theme, "GPU");

    let (rect, _) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), GRAPH_HEIGHT),
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
    ui.add_space(SPACE_MD);

    if system.gpus.is_empty() {
        widgets::empty_state(
            ui,
            theme,
            "This machine's display driver does not publish GPU counters",
        );
        return;
    }
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
                    stat_column_width(ui.available_width()),
                    "Dedicated memory in use",
                    &crate::format::bytes(adapter.memory_used),
                );
            }
        });
    });
}

/// The width [`stat_column`] gives each of a row's columns.
///
/// A fixed quarter of the row, not a quarter of however many columns
/// happen to be in it — that is what lines a three-column row up with a
/// four-column one above it, at the cost of a three-column row not
/// stretching to fill the last quarter.
///
/// Callers must measure `available` **once**, before the row's first
/// [`stat_column`] call, and pass the same value to every column in that
/// row. Reading `ui.available_width()` freshly inside `stat_column`
/// itself was the bug this exists to prevent: `ui.horizontal`'s own
/// available width shrinks as each sibling is allocated, so a column
/// computed after its neighbours already claimed space is a quarter of
/// what was left rather than a quarter of the row — four calls in a row
/// then produce four different widths, shrinking geometrically instead
/// of matching.
#[must_use]
fn stat_column_width(available: f32) -> f32 {
    // Four columns is three gaps between them, and `theme::apply` sets
    // `ui.horizontal`'s own `item_spacing.x` to `SPACE_SM` — so claiming
    // a plain quarter for each of the four, via `set_min_width` below,
    // asks the row for `4 * quarter + 3 * SPACE_SM`, which is
    // `3 * SPACE_SM` more than the row actually has. Left in, that
    // overshoot is exactly what pushed the CPU panel's own drawn content
    // past the window's real edge two levels up the call stack.
    ((available - 3.0 * SPACE_SM) / 4.0).max(0.0)
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
                    egui::Panel::left("performance-picker")
                        .exact_size(PICKER_WIDTH)
                        .frame(egui::Frame::new().inner_margin(theme::margin_xy(0.0, 0.0)))
                        .show(ui, |ui| {
                            picker_rect = Some(picker(&mut app, ui, &theme, &snapshot));
                        });
                    egui::Panel::left("performance-picker-gap")
                        .exact_size(SPACE_LG)
                        .frame(egui::Frame::new())
                        .show_separator_line(false)
                        .show(ui, |_| {});
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
    fn the_picker_detail_seam_draws_exactly_one_separator_line() -> anyhow::Result<()> {
        // The spacer panel that carves out the seam's gap is itself a
        // `Panel::left`, and a `Panel::left` draws its own separator
        // line by default — right next to the picker panel's own,
        // already-correct one. The two lines sit only a few pixels
        // apart, which reads as a single doubled border hugging
        // whichever column is on the near side, not as a gap. A rect
        // check can't see this: neither panel's content rect moves, so
        // the fix has to be verified against what actually gets
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
                    egui::Panel::left("performance-picker")
                        .exact_size(PICKER_WIDTH)
                        .frame(egui::Frame::new().inner_margin(theme::margin_xy(0.0, 0.0)))
                        .show(ui, |ui| {
                            picker_rect = Some(picker(&mut app, ui, &theme, &snapshot));
                        });
                    egui::Panel::left("performance-picker-gap")
                        .exact_size(SPACE_LG)
                        .frame(egui::Frame::new())
                        .show_separator_line(false)
                        .show(ui, |_| {});
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
            seam_lines, 1,
            "expected exactly one separator line at the picker/detail seam, found {seam_lines}"
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
                        capacity: 1,
                        ..Default::default()
                    },
                    crate::model::DiskSample {
                        name: "D:".to_string(),
                        capacity: 1,
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
                    egui::Panel::left("performance-picker")
                        .exact_size(PICKER_WIDTH)
                        .frame(egui::Frame::new().inner_margin(theme::margin_xy(0.0, 0.0)))
                        .show(ui, |ui| {
                            picker_rect = Some(picker(&mut app, ui, &theme, &snapshot));
                        });
                    egui::Panel::left("performance-picker-gap")
                        .exact_size(SPACE_LG)
                        .frame(egui::Frame::new())
                        .show_separator_line(false)
                        .show(ui, |_| {});
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
        let width = stat_column_width(available);
        let claimed = 4.0 * width + 3.0 * SPACE_SM;
        assert!(
            claimed <= available,
            "four columns of {width} plus three {SPACE_SM}-wide gaps is \
             {claimed}, which overflows the {available} available"
        );
        assert!((stat_column_width(0.0) - 0.0).abs() < f32::EPSILON);
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
            ui.horizontal(|ui| {
                let width = stat_column_width(ui.available_width());
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
    fn adapters_split_busiest_first_with_idle_ties_alphabetical() {
        let adapters = vec![
            crate::model::AdapterSample {
                name: "Idle B".to_string(),
                receive_rate: 0.0,
                send_rate: 0.0,
                link_speed: 1_000_000_000,
            },
            crate::model::AdapterSample {
                name: "Busy".to_string(),
                receive_rate: 500.0,
                send_rate: 200.0,
                link_speed: 1_000_000_000,
            },
            crate::model::AdapterSample {
                name: "Idle A".to_string(),
                receive_rate: 0.0,
                send_rate: 0.4,
                link_speed: 1_000_000_000,
            },
        ];
        let (active, idle) = split_by_activity(adapters);
        assert_eq!(
            active.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(),
            vec!["Busy"],
            "only the adapter clearing 1 B/s counts as active"
        );
        assert_eq!(
            idle.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(),
            vec!["Idle A", "Idle B"],
            "idle adapters tie at zero and should break alphabetically"
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

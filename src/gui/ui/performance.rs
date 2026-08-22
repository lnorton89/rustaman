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
use egui::{Rect, Sense, Ui, Vec2};

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

    ui.add_space(SPACE_MD);
    detail(app, ui, &theme, &snapshot);
}

/// The resource list down the left.
fn picker(app: &mut App, ui: &mut Ui, theme: &Palette, snapshot: &Snapshot) {
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
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
        });
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
fn detail(app: &mut App, ui: &mut Ui, theme: &Palette, snapshot: &Snapshot) {
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
        });
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
        stat_column(
            ui,
            theme,
            "Processes",
            &crate::format::count(system.process_count as u64),
        );
        stat_column(
            ui,
            theme,
            "Threads",
            &crate::format::count(system.thread_count),
        );
        stat_column(
            ui,
            theme,
            "Handles",
            &crate::format::count(system.handle_count),
        );
        stat_column(
            ui,
            theme,
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
        // grid too short to see anything in.
        let rows = (app.performance.cores.len() as f32).sqrt().ceil().max(1.0);
        let height = (rows * 46.0).clamp(90.0, 320.0);
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
        stat_column(
            ui,
            theme,
            "In use",
            &crate::format::bytes(system.memory.used()),
        );
        stat_column(
            ui,
            theme,
            "Available",
            &crate::format::bytes(system.memory.available),
        );
        stat_column(
            ui,
            theme,
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
            "Cached",
            &crate::format::bytes(system.memory.cached),
        );
    });
    ui.add_space(chrome::SECTION_GAP);

    widgets::section(ui, theme, "Kernel memory");
    ui.horizontal(|ui| {
        stat_column(
            ui,
            theme,
            "Paged pool",
            &crate::format::bytes(system.memory.paged_pool),
        );
        // The one number that diagnoses a driver leak — the leak that
        // takes the whole machine down rather than a process.
        stat_column(
            ui,
            theme,
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
                stat_column(ui, theme, "Read", &crate::format::rate(disk.read_rate));
                stat_column(ui, theme, "Write", &crate::format::rate(disk.write_rate));
                stat_column(
                    ui,
                    theme,
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
                    stat_column(
                        ui,
                        theme,
                        "Receive",
                        &crate::format::rate(adapter.receive_rate),
                    );
                    stat_column(ui, theme, "Send", &crate::format::rate(adapter.send_rate));
                    stat_column(
                        ui,
                        theme,
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
                    "Dedicated memory in use",
                    &crate::format::bytes(adapter.memory_used),
                );
            }
        });
    });
}

/// One readout in a row of them.
///
/// Each takes an equal share of the row, so a row of four lines up with a
/// row of three above it rather than each column being as wide as its own
/// value.
fn stat_column(ui: &mut Ui, theme: &Palette, caption: &str, value: &str) {
    let width = ui.available_width() / 4.0;
    ui.allocate_ui_with_layout(
        Vec2::new(width, 0.0),
        egui::Layout::top_down(egui::Align::Min),
        |ui| {
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
    );
}

#[cfg(test)]
mod tests {
    use super::*;

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

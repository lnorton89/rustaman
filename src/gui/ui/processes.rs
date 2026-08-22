// ============================================================================
// Module:       gui::ui::processes
// Description:  The process tree — the toolbar, the table, the row rendering,
//               and the context menu that acts on a row.
//
// Dependencies: egui, egui_extras (the table); super::{theme, widgets, chrome},
//               crate::gui::app
// ============================================================================

//! The process list.
//!
//! The view the app opens on, and the one most of the design decisions
//! elsewhere exist to serve.
//!
//! ## The table's columns
//!
//! Columns start at **stated widths**, never `Column::auto()`. An `auto`
//! column makes the table's first frame a sizing pass, which egui lays
//! out with an unbounded width — the content comes back as wide as the
//! window, and the table opens with a horizontal scrollbar over columns
//! nobody asked to be that wide.
//!
//! Exactly one column absorbs the slack, and it is the Name column, which
//! is the one that benefits from the room. It is a `remainder()`, and a
//! `remainder()` cannot also be resizable — dragging one gives it a
//! stored width, after which it absorbs nothing and the table stops
//! filling its pane.
//!
//! **No column ever disappears at a narrow width.** Hiding columns below
//! a breakpoint is the obvious way to handle a narrow window and it is
//! wrong: a column that vanishes cannot be scrolled to, resized, or even
//! known to exist, and the table reads as having lost them. Narrow
//! windows scroll horizontally instead.
//!
//! ## Rows are drawn, not composed
//!
//! Each row paints its own background, selection bar, heat tint, and
//! cells. Composing them from `ui.label` calls inside a `Frame` would put
//! the frame's own background over the heat tint and the selection bar,
//! and the row would lose both.

use super::theme::{self, HEADER_HEIGHT, PAD, ROW_HEIGHT, SPACE_MD, SPACE_SM, SPACE_XS};
use super::{chrome, widgets};
use crate::gui::app::actions::Action;
use crate::gui::app::{rows::RowKey, App};
use crate::model::sort::SortKey;
use crate::model::tree::{Entry, Totals};
use crate::model::{ProcessKey, ProcessKind, ProcessRow};
use crate::theme::Palette;
use egui::{Rect, Sense, Ui, Vec2};
use egui_extras::{Column, TableBuilder};

/// The columns, in order, with the width each starts at.
///
/// Stated rather than measured; see the module docs.
const COLUMNS: [(SortKey, f32); 8] = [
    (SortKey::Name, 0.0), // the remainder column; the width is ignored
    (SortKey::Pid, 64.0),
    (SortKey::Status, 82.0),
    (SortKey::Cpu, 62.0),
    (SortKey::Memory, 86.0),
    (SortKey::Disk, 92.0),
    (SortKey::Network, 62.0),
    (SortKey::Gpu, 58.0),
];

/// The least a resizable column may be dragged to.
///
/// Below this a heading is unreadable, and a column nobody can read is a
/// column that has effectively disappeared — which is the thing this view
/// does not do.
const MIN_COLUMN: f32 = 48.0;

/// How far each tree level indents.
const INDENT: f32 = 16.0;

// Relations between constants, checked when the crate is compiled.
const _: () = {
    assert!(
        MIN_COLUMN >= 40.0,
        "a column narrower than this cannot show its own heading, and a \
         column nobody can read has effectively disappeared — which is \
         the thing this view does not do"
    );
    assert!(
        INDENT >= 12.0,
        "an indent under 12 points does not read as nesting"
    );
    assert!(
        INDENT * 8.0 < NAME_MINIMUM,
        "eight levels of nesting is a realistic browser, and at this \
         indent it would fill the name column's minimum width"
    );
};

/// Draws the process view.
pub fn draw(app: &mut App, ui: &mut Ui) {
    let theme = app.theme.clone();
    toolbar(app, ui, &theme);
    ui.add_space(chrome::TOOLBAR_GAP);
    refresh_rows(app);

    if app.snapshot.is_none() {
        widgets::empty_state(ui, &theme, "Waiting for the first sample…");
        return;
    }
    if app.processes.rows.entries().is_empty() {
        let message = if app.is_filtering() {
            "No processes match that search"
        } else {
            "No processes"
        };
        widgets::empty_state(ui, &theme, message);
        return;
    }
    table(app, ui, &theme);
}

/// The row above the table: search, grouping, and the actions.
fn toolbar(app: &mut App, ui: &mut Ui, theme: &Palette) {
    ui.horizontal(|ui| {
        if chrome::search_box(
            ui,
            theme,
            &mut app.processes.search,
            "Search — try chrome, pid:4242, user:system",
        ) {
            // Reparsed only when the text changes; doing it every frame
            // would parse the query sixty times a second.
            app.processes.query = crate::model::filter::Query::parse(&app.processes.search);
        }

        chrome::toolbar_dot(ui, theme);

        let grouped = app.processes.grouped;
        if widgets::chip(
            ui,
            if grouped { "Tree" } else { "Flat" },
            if grouped {
                theme.accent_soft
            } else {
                theme.raised
            },
            theme.text,
        )
        .interact(Sense::click())
        .on_hover_text("Group into a process tree, or show one flat list")
        .clicked()
        {
            app.processes.grouped = !grouped;
        }

        if app.is_filtering() {
            let matched = app.processes.rows.matched();
            let total = app
                .snapshot
                .as_ref()
                .map_or(0, |snapshot| snapshot.processes.len());
            widgets::chip(
                ui,
                &format!("{matched} of {total}"),
                theme.raised,
                theme.text_muted,
            );
        }

        // The destructive action sits at the far right, away from the
        // controls that are clicked constantly. A button that ends a
        // process should not be adjacent to one that changes a sort.
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let selected = app.selected_row().map(ProcessRow::key);
            let can_act = app.selected_row().is_some_and(|row| !row.is_pseudo());
            if widgets::danger_button(ui, theme, "End task", can_act).clicked() {
                if let Some(key) = selected {
                    app.dispatch(Action::EndTask(key), ui);
                }
            }
        });
    });
}

/// Rebuilds the visible rows if anything they depend on has changed.
fn refresh_rows(app: &mut App) {
    let Some(snapshot) = app.snapshot.as_ref() else {
        return;
    };
    let key = RowKey::new(
        snapshot.sequence,
        app.processes.sort,
        app.processes.descending,
        app.processes.grouped,
        &app.processes.search,
        &app.processes.expanded,
        &app.processes.collapsed,
    );
    // The two sets are passed alongside the key because the key holds
    // sorted copies for comparison while the layout wants the sets
    // themselves for lookup.
    let expanded = app.processes.expanded.clone();
    let collapsed = app.processes.collapsed.clone();
    app.processes.rows.refresh(
        &snapshot.processes,
        &app.processes.query,
        key,
        &expanded,
        &collapsed,
    );
}

/// Draws the table.
fn table(app: &mut App, ui: &mut Ui, theme: &Palette) {
    let entries: Vec<Entry> = app.processes.rows.entries().to_vec();
    let Some(snapshot) = app.snapshot.clone() else {
        return;
    };

    let mut clicked: Option<ProcessKey> = None;
    let mut toggled: Option<ProcessKey> = None;
    let mut group_toggled: Option<ProcessKind> = None;
    let mut action: Option<Action> = None;
    let mut sort_clicked: Option<SortKey> = None;

    let mut builder = TableBuilder::new(ui)
        .striped(false)
        .resizable(true)
        .vscroll(true)
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
        .sense(Sense::click());

    for (index, (_, width)) in COLUMNS.iter().enumerate() {
        builder = if index == 0 {
            // The Name column absorbs the slack. A `remainder()` cannot
            // also be resizable — dragging one gives it a stored width,
            // after which it absorbs nothing and the table stops filling
            // its pane.
            builder.column(Column::remainder().at_least(200.0).clip(true))
        } else {
            builder.column(
                Column::initial(*width)
                    .at_least(MIN_COLUMN)
                    .resizable(true)
                    .clip(true),
            )
        };
    }

    builder
        .header(HEADER_HEIGHT, |mut header| {
            for (index, (key, _)) in COLUMNS.iter().enumerate() {
                header.col(|ui| {
                    let sorted = (app.processes.sort == *key).then_some(app.processes.descending);
                    // Only the Name column claims its cell's width; see
                    // `widgets::sortable_header` on why the others must
                    // not.
                    let claims = index == 0;
                    let right = index > 0;
                    if widgets::sortable_header(ui, theme, key.label(), sorted, claims, right)
                        .clicked()
                    {
                        sort_clicked = Some(*key);
                    }
                });
            }
        })
        .body(|body| {
            body.rows(ROW_HEIGHT, entries.len(), |mut row| {
                let index = row.index();
                let Some(entry) = entries.get(index) else {
                    return;
                };
                match entry {
                    Entry::Group {
                        kind,
                        totals,
                        collapsed,
                    } => {
                        let mut hit = false;
                        for column in 0..COLUMNS.len() {
                            row.col(|ui| {
                                if column == 0 {
                                    hit |= group_heading(ui, theme, *kind, totals, *collapsed);
                                } else {
                                    group_cell(ui, theme, column, totals);
                                }
                            });
                        }
                        if hit || row.response().clicked() {
                            group_toggled = Some(*kind);
                        }
                    }
                    Entry::Process {
                        index: row_index,
                        depth,
                        children,
                        expanded,
                        totals,
                    } => {
                        let Some(process) = snapshot.processes.get(*row_index) else {
                            return;
                        };
                        let key = process.key();
                        let selected = app.processes.selected == Some(key);
                        row.set_selected(false);

                        let mut disclosure = false;
                        for column in 0..COLUMNS.len() {
                            row.col(|ui| {
                                let cell = ui.max_rect();
                                if column == 0 {
                                    // The background is painted from the
                                    // first cell across the whole row,
                                    // before anything else, so the heat
                                    // tints and the text land on top of
                                    // it.
                                    let full = Rect::from_min_size(
                                        cell.min,
                                        Vec2::new(
                                            ui.available_width().max(cell.width()) + 4_000.0,
                                            cell.height(),
                                        ),
                                    );
                                    widgets::row_background(
                                        ui,
                                        theme,
                                        full,
                                        egui::Id::new("row").with(key),
                                        selected,
                                        false,
                                        index % 2 == 1,
                                    );
                                    disclosure =
                                        name_cell(ui, theme, process, *depth, *children, *expanded);
                                } else {
                                    metric_cell(ui, theme, column, process, totals, *expanded);
                                }
                            });
                        }

                        let response = row.response();
                        if disclosure {
                            toggled = Some(key);
                        } else if response.clicked() {
                            clicked = Some(key);
                        }
                        if response.double_clicked() && *children > 0 {
                            toggled = Some(key);
                        }
                        response.context_menu(|ui| {
                            if let Some(chosen) = context_menu(ui, theme, process) {
                                clicked = Some(key);
                                action = Some(chosen);
                            }
                        });
                    }
                }
            });
        });

    if let Some(key) = sort_clicked {
        toggle_sort(app, key);
    }
    if let Some(key) = clicked {
        app.processes.selected = Some(key);
    }
    if let Some(key) = toggled {
        if !app.processes.expanded.remove(&key) {
            app.processes.expanded.insert(key);
        }
    }
    if let Some(kind) = group_toggled {
        if !app.processes.collapsed.remove(&kind) {
            app.processes.collapsed.insert(kind);
        }
    }
    if let Some(action) = action {
        app.dispatch(action, ui);
    }
}

/// Applies a click on a column heading.
///
/// Clicking the *sorted* column flips its direction; clicking another
/// switches to it in that column's own natural direction. Switching to a
/// magnitude column in ascending order — which is what "keep the current
/// direction" would do after sorting a name — shows the three hundred
/// idle processes first, and reads as the sort having failed.
fn toggle_sort(app: &mut App, key: SortKey) {
    if app.processes.sort == key {
        app.processes.descending = !app.processes.descending;
    } else {
        app.processes.sort = key;
        app.processes.descending = key.defaults_descending();
    }
}

/// Draws a category heading. Returns whether its disclosure was clicked.
fn group_heading(
    ui: &mut Ui,
    theme: &Palette,
    kind: ProcessKind,
    totals: &Totals,
    collapsed: bool,
) -> bool {
    let rect = ui.max_rect();
    ui.painter()
        .rect_filled(rect, egui::CornerRadius::ZERO, theme::rgb(theme.raised));

    ui.horizontal_centered(|ui| {
        ui.add_space(SPACE_XS);
        let arrow = if collapsed { "▸" } else { "▾" };
        ui.label(
            egui::RichText::new(arrow)
                .color(theme::rgb(theme.text_muted))
                .text_style(egui::TextStyle::Small),
        );
        ui.add_space(SPACE_XS);
        ui.label(
            egui::RichText::new(kind.label())
                .color(theme::rgb(theme.text))
                .strong()
                .text_style(egui::TextStyle::Small),
        );
        ui.label(
            egui::RichText::new(format!("({})", totals.processes))
                .color(theme::rgb(theme.text_faint))
                .text_style(egui::TextStyle::Small),
        );
    });
    false
}

/// Draws a category heading's aggregate figure for one column.
fn group_cell(ui: &mut Ui, theme: &Palette, column: usize, totals: &Totals) {
    let rect = ui.max_rect();
    ui.painter()
        .rect_filled(rect, egui::CornerRadius::ZERO, theme::rgb(theme.raised));

    // Only the summable columns carry a category total. A PID or a
    // status has no meaningful aggregate, and showing one would be worse
    // than showing nothing.
    let text = match COLUMNS.get(column).map(|(key, _)| *key) {
        Some(SortKey::Cpu) => crate::format::percent_or_dash(totals.cpu_percent),
        Some(SortKey::Memory) => crate::format::bytes_or_dash(totals.working_set),
        Some(SortKey::Disk) => crate::format::rate_or_dash(totals.disk_rate),
        Some(SortKey::Gpu) => crate::format::percent_or_dash(totals.gpu_percent),
        Some(_) | None => String::new(),
    };
    if !text.is_empty() {
        widgets::number(ui, theme, &text, true);
    }
}

/// Draws the name cell. Returns whether the disclosure triangle was hit.
fn name_cell(
    ui: &mut Ui,
    theme: &Palette,
    process: &ProcessRow,
    depth: u16,
    children: usize,
    expanded: bool,
) -> bool {
    let mut hit = false;
    ui.horizontal_centered(|ui| {
        ui.add_space(SPACE_XS + f32::from(depth) * INDENT);

        if children > 0 {
            let arrow = if expanded { "▾" } else { "▸" };
            let (rect, response) =
                ui.allocate_exact_size(Vec2::new(INDENT, ROW_HEIGHT), Sense::click());
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                arrow,
                egui::TextStyle::Small.resolve(ui.style()),
                theme::rgb(theme.text_muted),
            );
            hit = response.clicked();
        } else {
            ui.add_space(INDENT);
        }

        // A per-process colour dot, keyed on the process rather than its
        // row index — so it does not change when the table is resorted,
        // which would read as the table having reloaded.
        let (dot, _) = ui.allocate_exact_size(Vec2::splat(8.0), Sense::hover());
        let hue = theme.series_for(u64::from(process.pid) ^ process.started_at);
        ui.painter()
            .circle_filled(dot.center(), 3.0, theme::rgb(hue));
        ui.add_space(SPACE_XS);

        ui.label(egui::RichText::new(process.display_name()).color(theme::rgb(theme.text)));

        if process.status == crate::model::ProcessStatus::Suspended {
            ui.add_space(SPACE_XS);
            widgets::chip(ui, "Suspended", theme.raised, theme.warning);
        }
        if process.elevated {
            ui.add_space(SPACE_XS);
            widgets::chip(ui, "Admin", theme.raised, theme.text_muted);
        }
    });
    hit
}

/// Draws one metric cell, with its heat tint.
fn metric_cell(
    ui: &mut Ui,
    theme: &Palette,
    column: usize,
    process: &ProcessRow,
    totals: &Totals,
    expanded: bool,
) {
    let rect = ui.max_rect();
    let Some((key, _)) = COLUMNS.get(column) else {
        return;
    };

    // A collapsed parent shows its subtree's total, or collapsing the
    // tree makes a busy process disappear — which is the single most
    // common thing a task manager is opened to find.
    let aggregated = !expanded && totals.processes > 1;

    let (text, load) = match key {
        SortKey::Pid => (process.pid.to_string(), 0.0),
        SortKey::Status => (process.status.label().to_string(), 0.0),
        SortKey::Cpu => {
            let value = if aggregated {
                totals.cpu_percent
            } else {
                process.cpu_percent
            };
            (
                crate::format::percent_or_dash(value),
                (value / 100.0) as f32,
            )
        }
        SortKey::Memory => {
            let value = if aggregated {
                totals.working_set
            } else {
                process.working_set
            };
            // Scaled against a gigabyte rather than against the machine's
            // memory: the tint is asking "is this a lot for a process",
            // and on a 128 GB machine every process would otherwise be
            // untinted.
            (
                crate::format::bytes_or_dash(value),
                (value as f32 / 1_073_741_824.0).min(1.0),
            )
        }
        SortKey::Disk => {
            let value = if aggregated {
                totals.disk_rate
            } else {
                process.disk_rate()
            };
            // Against 100 MB/s, which is a busy disk for one process.
            (
                crate::format::rate_or_dash(value),
                (value as f32 / 104_857_600.0).min(1.0),
            )
        }
        SortKey::Network => {
            let value = if aggregated {
                totals.connections
            } else {
                process.connections
            };
            (
                if value == 0 {
                    crate::format::DASH.to_string()
                } else {
                    value.to_string()
                },
                0.0,
            )
        }
        SortKey::Gpu => {
            let value = if aggregated {
                totals.gpu_percent
            } else {
                process.gpu_percent
            };
            (
                crate::format::percent_or_dash(value),
                (value / 100.0) as f32,
            )
        }
        // The name column is drawn by the caller, and the rest belong to
        // the Details view. Listed rather than wildcarded so that adding
        // a column without a branch here is a compile error rather than
        // a silently blank cell.
        SortKey::Name
        | SortKey::PrivateBytes
        | SortKey::User
        | SortKey::Threads
        | SortKey::Handles
        | SortKey::CpuTime
        | SortKey::Priority
        | SortKey::Architecture
        | SortKey::Session
        | SortKey::Path => (String::new(), 0.0),
    };

    if load > 0.0 {
        widgets::heat_cell(ui, theme, rect, load);
    }
    let muted = text == crate::format::DASH;
    widgets::number(ui, theme, &text, muted);
}

/// The right-click menu for a row. Returns the action chosen, if any.
fn context_menu(ui: &mut Ui, theme: &Palette, process: &ProcessRow) -> Option<Action> {
    let key = process.key();
    let mut chosen = None;
    // The pseudo-processes cannot be acted on at all; the menu says so
    // rather than offering items that will refuse.
    if process.is_pseudo() {
        ui.label(
            egui::RichText::new("Kernel process — cannot be changed")
                .color(theme::rgb(theme.text_muted))
                .text_style(egui::TextStyle::Small),
        );
        return None;
    }

    if ui.button("End task").clicked() {
        chosen = Some(Action::EndTask(key));
        ui.close();
    }
    if ui.button("End process tree").clicked() {
        chosen = Some(Action::EndTree(key));
        ui.close();
    }
    ui.separator();

    let suspended = process.status == crate::model::ProcessStatus::Suspended;
    let label = if suspended { "Resume" } else { "Suspend" };
    if ui.button(label).clicked() {
        chosen = Some(if suspended {
            Action::Resume(key)
        } else {
            Action::Suspend(key)
        });
        ui.close();
    }

    ui.menu_button("Set priority", |ui| {
        // Highest first: the menu opens downwards, so the classes people
        // actually reach for are nearest the pointer.
        for priority in crate::model::Priority::ALL.into_iter().rev() {
            let mut label = egui::RichText::new(priority.label());
            if priority.is_dangerous() {
                label = label.color(theme::rgb(theme.danger));
            }
            if ui.button(label).clicked() {
                chosen = Some(Action::SetPriority(key, priority));
                ui.close();
            }
        }
    });

    ui.separator();
    if ui.button("Open file location").clicked() {
        chosen = Some(Action::Reveal(key));
        ui.close();
    }
    if ui.button("Copy details").clicked() {
        chosen = Some(Action::Copy(key));
        ui.close();
    }

    chosen
}

/// The gap the view leaves at its edges.
pub const VIEW_INSET: f32 = PAD;

/// The gap between the toolbar's own controls.
pub const CONTROL_GAP: f32 = SPACE_SM;

/// The width the name column will not shrink below.
pub const NAME_MINIMUM: f32 = 200.0;

/// The gap between a chip and the control beside it.
pub const CHIP_GAP: f32 = SPACE_MD;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clicking_the_sorted_column_flips_it() {
        let mut app = App::new(crate::config::Config::default());
        app.processes.sort = SortKey::Cpu;
        app.processes.descending = true;
        toggle_sort(&mut app, SortKey::Cpu);
        assert!(!app.processes.descending);
        toggle_sort(&mut app, SortKey::Cpu);
        assert!(app.processes.descending);
    }

    #[test]
    fn switching_columns_uses_the_new_columns_own_direction() {
        // Keeping the previous direction is the obvious implementation
        // and it is wrong: switching from a name sort to CPU would show
        // the three hundred idle processes first, which reads as the sort
        // having failed.
        let mut app = App::new(crate::config::Config::default());
        app.processes.sort = SortKey::Name;
        app.processes.descending = false;

        toggle_sort(&mut app, SortKey::Cpu);
        assert_eq!(app.processes.sort, SortKey::Cpu);
        assert!(
            app.processes.descending,
            "a magnitude column opens descending"
        );

        toggle_sort(&mut app, SortKey::Name);
        assert!(!app.processes.descending, "a name column opens ascending");
    }

    #[test]
    fn every_column_is_sortable_and_named() {
        for (key, width) in COLUMNS {
            assert!(!key.label().is_empty());
            assert!(
                width >= MIN_COLUMN || key == SortKey::Name,
                "{} starts at {width}, below the minimum",
                key.label()
            );
        }
    }

    #[test]
    fn the_columns_are_distinct() {
        // A duplicate would make one heading sort by the other's column.
        let mut keys: Vec<SortKey> = COLUMNS.iter().map(|(key, _)| *key).collect();
        let count = keys.len();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), count, "two columns share a sort key");
    }

    #[test]
    fn exactly_one_column_absorbs_the_slack() {
        // A `remainder()` cannot also be resizable, and two of them would
        // fight. The first column is the one, and it is the Name column
        // because that is what benefits from the room.
        assert_eq!(
            COLUMNS.first().map(|(key, _)| *key),
            Some(SortKey::Name),
            "the absorbing column must be the first one"
        );
    }
}

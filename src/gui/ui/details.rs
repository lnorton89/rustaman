// ============================================================================
// Module:       gui::ui::details
// Description:  The Details view — one flat technical table, plus the inspector
//               pane for whichever row is selected.
//
// Dependencies: egui, egui_extras; super::{theme, widgets, chrome}
// ============================================================================

//! The Details view.
//!
//! One flat, fully sorted table with the columns the Processes view has
//! no room for, and an inspector pane beside it.
//!
//! ## Why this is a separate view rather than more columns
//!
//! The Processes view answers "what is making this machine slow", and it
//! is a tree because that question needs the browser's thirty renderers
//! collapsed under it. This view answers "tell me everything about this
//! one process", and a tree is in the way of that: the row you want is
//! nested under two others you do not care about.
//!
//! So they are two views over the same data with two different shapes,
//! rather than one view with a mode switch — a switch would need every
//! control in the toolbar to make sense in both modes, and half of them
//! do not.

use super::theme::{self, HEADER_HEIGHT, ROW_HEIGHT, SPACE_MD, SPACE_SM, SPACE_XS};
use super::{chrome, widgets};
use crate::gui::app::actions::Action;
use crate::gui::app::{rows::RowKey, App};
use crate::model::sort::SortKey;
use crate::model::tree::Entry;
use crate::model::{ProcessKey, ProcessRow};
use crate::theme::Palette;
use egui::{Sense, Ui};
use egui_extras::{Column, TableBuilder};

/// The columns, with the width each starts at.
const COLUMNS: [(SortKey, f32); 10] = [
    (SortKey::Name, 260.0),
    (SortKey::Pid, 64.0),
    (SortKey::Status, 82.0),
    (SortKey::User, 150.0),
    (SortKey::Cpu, 62.0),
    (SortKey::PrivateBytes, 88.0),
    (SortKey::Threads, 64.0),
    (SortKey::Handles, 72.0),
    (SortKey::Architecture, 58.0),
    (SortKey::Session, 62.0),
];

/// The width of the inspector pane.
const INSPECTOR_WIDTH: f32 = 300.0;

// `widgets::detail_row` gives the key column 130 points, so anything
// narrower than this leaves nothing for the value. A relation between
// constants, so checked when the crate is compiled.
const _: () = assert!(
    INSPECTOR_WIDTH >= 260.0,
    "the inspector is too narrow for a label and a value side by side"
);

/// Draws the Details view.
pub fn draw(app: &mut App, ui: &mut Ui) {
    let theme = app.theme.clone();
    toolbar(app, ui, &theme);
    ui.add_space(chrome::TOOLBAR_GAP);

    if app.snapshot.is_none() {
        widgets::empty_state(ui, &theme, "Waiting for the first sample…");
        return;
    }
    refresh_rows(app);

    // The inspector is drawn first so it claims its width before the
    // table takes the rest — a table that measured first would take the
    // whole pane and the inspector would have nothing left.
    egui::Panel::right("details-inspector")
        .exact_size(INSPECTOR_WIDTH)
        .frame(egui::Frame::new().inner_margin(theme::margin_xy(SPACE_MD, 0.0)))
        .show(ui, |ui| {
            inspector(app, ui, &theme);
        });

    if app.details.rows.entries().is_empty() {
        widgets::empty_state(ui, &theme, "No processes match that search");
        return;
    }
    table(app, ui, &theme);
}

/// The row above the table.
fn toolbar(app: &mut App, ui: &mut Ui, theme: &Palette) {
    ui.horizontal(|ui| {
        if chrome::search_box(ui, theme, &mut app.details.search, "Search all processes") {
            app.details.query = crate::model::filter::Query::parse(&app.details.search);
        }
        if app.is_filtering() {
            chrome::toolbar_dot(ui, theme);
            let matched = app.details.rows.matched();
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
    });
}

/// Rebuilds the visible rows if anything they depend on has changed.
fn refresh_rows(app: &mut App) {
    let Some(snapshot) = app.snapshot.as_ref() else {
        return;
    };
    let key = RowKey::new(
        snapshot.sequence,
        app.details.sort,
        app.details.descending,
        // Never grouped: this view is one flat list. See the module docs.
        false,
        &app.details.search,
        &std::collections::HashSet::new(),
        &std::collections::HashSet::new(),
    );
    app.details.rows.refresh(
        &snapshot.processes,
        &app.details.query,
        key,
        &std::collections::HashSet::new(),
        &std::collections::HashSet::new(),
    );
}

/// The table.
fn table(app: &mut App, ui: &mut Ui, theme: &Palette) {
    let entries: Vec<Entry> = app.details.rows.entries().to_vec();
    let Some(snapshot) = app.snapshot.clone() else {
        return;
    };
    let mut clicked: Option<ProcessKey> = None;
    let mut sort_clicked: Option<SortKey> = None;
    let mut action: Option<Action> = None;

    // Captured before the builder borrows the `Ui`; see
    // `widgets::row_background` on why a row needs it.
    let viewport = ui.available_rect_before_wrap();

    let mut builder = TableBuilder::new(ui)
        .resizable(true)
        .vscroll(true)
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
        .sense(Sense::click());

    for (index, (_, width)) in COLUMNS.iter().enumerate() {
        let least = if index == 0 { 180.0 } else { 48.0 };
        builder = builder.column(
            Column::initial(*width)
                .at_least(least)
                .resizable(true)
                .clip(true),
        );
    }
    // The trailing spacer that absorbs the window's slack; see
    // `super::processes` on why the absorbing column has to be one with
    // nothing in it.
    builder = builder.column(Column::remainder().clip(true));

    builder
        .header(HEADER_HEIGHT, |mut header| {
            for (index, (key, _)) in COLUMNS.iter().enumerate() {
                header.col(|ui| {
                    let sorted = (app.details.sort == *key).then_some(app.details.descending);
                    // `claims_width: false` for every column now that
                    // none of them is the remainder — see the header
                    // loop in `super::processes`.
                    if widgets::sortable_header(ui, theme, key.label(), sorted, false, index > 0)
                        .clicked()
                    {
                        sort_clicked = Some(*key);
                    }
                });
            }
            // The spacer's heading, which is empty. Skipping it leaves
            // the header one cell short of the body.
            header.col(|_| {});
        })
        .body(|body| {
            body.rows(ROW_HEIGHT, entries.len(), |mut row| {
                let position = row.index();
                let Some(Entry::Process { index, .. }) = entries.get(position) else {
                    return;
                };
                let Some(process) = snapshot.processes.get(*index) else {
                    return;
                };
                let key = process.key();
                let selected = app.details.selected == Some(key);

                // `..=COLUMNS.len()`: the trailing spacer is a real
                // column and has to be filled, or the row stops one
                // column short of the window's edge.
                for column in 0..=COLUMNS.len() {
                    row.col(|ui| match column {
                        0 => {
                            widgets::row_background(
                                ui,
                                theme,
                                viewport,
                                egui::Id::new("detail-row").with(key),
                                selected,
                                false,
                                position % 2 == 1,
                            );
                            ui.add_space(SPACE_SM);
                            ui.label(
                                egui::RichText::new(process.display_name())
                                    .color(theme::rgb(theme.text)),
                            );
                        }
                        _ if column < COLUMNS.len() => cell_text(ui, theme, column, process),
                        _ => {}
                    });
                }

                let response = row.response();
                if response.clicked() {
                    clicked = Some(key);
                }
                response.context_menu(|ui| {
                    if !process.is_pseudo() {
                        if ui.button("End task").clicked() {
                            action = Some(Action::EndTask(key));
                            ui.close();
                        }
                        if ui.button("Open file location").clicked() {
                            action = Some(Action::Reveal(key));
                            ui.close();
                        }
                        if ui.button("Copy details").clicked() {
                            action = Some(Action::Copy(key));
                            ui.close();
                        }
                    }
                });
            });
        });

    if let Some(key) = sort_clicked {
        if app.details.sort == key {
            app.details.descending = !app.details.descending;
        } else {
            app.details.sort = key;
            app.details.descending = key.defaults_descending();
        }
    }
    if let Some(key) = clicked {
        app.details.selected = Some(key);
    }
    if let Some(action) = action {
        app.dispatch(action, ui);
    }
}

/// One cell's text.
fn cell_text(ui: &mut Ui, theme: &Palette, column: usize, process: &ProcessRow) {
    let Some((key, _)) = COLUMNS.get(column) else {
        return;
    };
    let text = match key {
        SortKey::Pid => process.pid.to_string(),
        SortKey::Status => process.status.label().to_string(),
        SortKey::User => short_user(&process.user),
        SortKey::Cpu => crate::format::percent_or_dash(process.cpu_percent),
        SortKey::PrivateBytes => crate::format::bytes_or_dash(process.private_bytes),
        SortKey::Threads => process.thread_count.to_string(),
        SortKey::Handles => crate::format::count(u64::from(process.handle_count)),
        SortKey::Architecture => process.architecture.label().to_string(),
        SortKey::Session => process.session_id.to_string(),
        // The name column is drawn by the caller, and the rest are not
        // among this table's columns. Listed rather than wildcarded so
        // that adding a column without a branch here is a compile error
        // rather than a silently blank cell.
        SortKey::Name
        | SortKey::Memory
        | SortKey::Disk
        | SortKey::Network
        | SortKey::Gpu
        | SortKey::CpuTime
        | SortKey::Priority
        | SortKey::Path => String::new(),
    };
    let muted = text == crate::format::DASH;
    widgets::number(ui, theme, &text, muted);
}

/// An account name without its domain prefix.
///
/// `NT AUTHORITY\SYSTEM` in a 150-point column shows as `NT AUTHOR…`,
/// which identifies nothing. The bare account name is what distinguishes
/// one row from another; the full name is in the inspector.
#[must_use]
pub fn short_user(user: &str) -> String {
    if user.is_empty() {
        return crate::format::DASH.to_string();
    }
    user.rsplit('\\').next().unwrap_or(user).to_string()
}

/// The inspector pane for the selected row.
fn inspector(app: &mut App, ui: &mut Ui, theme: &Palette) {
    let Some(process) = app.selected_row().cloned() else {
        widgets::empty_state(ui, theme, "Select a process");
        return;
    };

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            widgets::section(ui, theme, process.display_name());

            widgets::detail_row(ui, theme, "Image", &process.name);
            widgets::detail_row(ui, theme, "PID", &process.pid.to_string());
            widgets::detail_row(ui, theme, "Parent PID", &process.parent_pid.to_string());
            widgets::detail_row(ui, theme, "Status", process.status.label());
            widgets::detail_row(
                ui,
                theme,
                "User",
                if process.user.is_empty() {
                    crate::format::DASH
                } else {
                    &process.user
                },
            );
            widgets::detail_row(ui, theme, "Session", &process.session_id.to_string());
            widgets::detail_row(
                ui,
                theme,
                "Elevated",
                if process.elevated { "Yes" } else { "No" },
            );
            widgets::detail_row(ui, theme, "Architecture", process.architecture.label());
            widgets::detail_row(ui, theme, "Priority", process.priority.label());
            if let Some(title) = process.window_title.as_deref() {
                widgets::detail_row(ui, theme, "Window", title);
            }

            ui.add_space(SPACE_XS);
            widgets::section(ui, theme, "Resources");
            widgets::detail_row(
                ui,
                theme,
                "CPU",
                &crate::format::percent(process.cpu_percent),
            );
            widgets::detail_row(
                ui,
                theme,
                "CPU time",
                &crate::format::cpu_time(process.cpu_time_ms),
            );
            widgets::detail_row(
                ui,
                theme,
                "Working set",
                &crate::format::bytes(process.working_set),
            );
            widgets::detail_row(
                ui,
                theme,
                "Private bytes",
                &crate::format::bytes(process.private_bytes),
            );
            widgets::detail_row(
                ui,
                theme,
                "Virtual size",
                &crate::format::bytes(process.virtual_bytes),
            );
            widgets::detail_row(ui, theme, "Threads", &process.thread_count.to_string());
            widgets::detail_row(
                ui,
                theme,
                "Handles",
                &crate::format::count(u64::from(process.handle_count)),
            );
            widgets::detail_row(
                ui,
                theme,
                "Disk read",
                &crate::format::bytes(process.io_read_bytes),
            );
            widgets::detail_row(
                ui,
                theme,
                "Disk written",
                &crate::format::bytes(process.io_write_bytes),
            );
            widgets::detail_row(ui, theme, "Connections", &process.connections.to_string());

            if let Some(path) = process.path.as_ref() {
                ui.add_space(SPACE_XS);
                widgets::section(ui, theme, "Image");
                // Wrapped rather than elided: a path is the one field
                // where the *end* is what identifies it, and an ellipsis
                // in a 300-point pane would hide exactly that.
                ui.label(
                    egui::RichText::new(path.to_string_lossy())
                        .color(theme::rgb(theme.text_muted))
                        .text_style(egui::TextStyle::Small),
                );
            }

            ui.add_space(SPACE_MD);
            ui.horizontal(|ui| {
                if widgets::primary_button(ui, theme, "Open location").clicked() {
                    app.dispatch(Action::Reveal(process.key()), ui);
                }
                if widgets::primary_button(ui, theme, "Copy").clicked() {
                    app.dispatch(Action::Copy(process.key()), ui);
                }
            });
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_domain_qualified_account_is_shortened_to_its_name() {
        // `NT AUTHORITY\SYSTEM` in a 150-point column shows as
        // `NT AUTHOR…`, which identifies nothing.
        assert_eq!(short_user("NT AUTHORITY\\SYSTEM"), "SYSTEM");
        assert_eq!(short_user("DESKTOP-A1B2C3\\alice"), "alice");
    }

    #[test]
    fn an_unqualified_account_is_left_alone() {
        assert_eq!(short_user("alice"), "alice");
    }

    #[test]
    fn an_unreadable_account_reads_as_absent() {
        assert_eq!(short_user(""), crate::format::DASH);
    }

    #[test]
    fn the_columns_are_distinct_and_named() {
        let mut keys: Vec<SortKey> = COLUMNS.iter().map(|(key, _)| *key).collect();
        let count = keys.len();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), count, "two columns share a sort key");
        for (key, _) in COLUMNS {
            assert!(!key.label().is_empty());
        }
    }

    #[test]
    fn this_view_shows_columns_the_process_list_has_no_room_for() {
        // The reason it exists as a separate view.
        let keys: Vec<SortKey> = COLUMNS.iter().map(|(key, _)| *key).collect();
        for extra in [
            SortKey::User,
            SortKey::Threads,
            SortKey::Handles,
            SortKey::Architecture,
            SortKey::Session,
        ] {
            assert!(keys.contains(&extra), "{} is missing", extra.label());
        }
    }
}

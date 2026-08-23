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
///
/// Sized so that all ten fit at the window size the app opens at with
/// the inspector closed, which is the state the view is usually in.
/// Opening the inspector takes about 300 points and the columns then do
/// not fit — that case scrolls, and `content_width` is where the two
/// are decided between.
///
/// This used to claim the columns fit *beside an open inspector* at a
/// 1440-point window. Both halves were wrong: the app opens at
/// [`crate::gui::DEFAULT_SIZE`], 1180 points, and 1440 came from a
/// config round-trip fixture. The test built on it passed because the
/// budget it computed was 260 points more generous than any window the
/// app actually opens.
const COLUMNS: [(SortKey, f32); 10] = [
    (SortKey::Name, 210.0),
    (SortKey::Pid, 56.0),
    // Sized for "Suspended", not for the em dash it shows on every
    // running process — see `ProcessStatus::column_label`. A column
    // sized for its common value clips the only value anyone is
    // scanning for.
    (SortKey::Status, 76.0),
    (SortKey::User, 104.0),
    (SortKey::Cpu, 56.0),
    (SortKey::PrivateBytes, 84.0),
    // These three are sized by their *headings* rather than their
    // values. A four-digit thread count needs about forty points and
    // the word "Threads" needs sixty-six with the sort arrow's reserved
    // column beside it, so a column sized for the numbers renders its
    // own heading as "Threa…".
    (SortKey::Threads, 66.0),
    (SortKey::Handles, 68.0),
    (SortKey::Architecture, 50.0),
    (SortKey::Session, 68.0),
];

/// Whether the columns fit the pane, and what to do about it.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Fit {
    /// They fit. The table scrolls itself vertically, which keeps its
    /// header pinned and puts its scrollbar against the pane's own
    /// edge. This is the state the view is in at the size the app opens
    /// at with the inspector closed, and it is worth keeping rather
    /// than paying the cost below unconditionally.
    Pane,
    /// They do not, so one scroll area owns both axes, with the content
    /// this wide.
    Scroll(f32),
}

/// Decides between the two, for a pane of `pane` points.
///
/// `bar` is the lane a vertical scrollbar takes, and it is subtracted
/// from the pane rather than added to what the columns want. Adding it
/// puts the content over the viewport by exactly that lane in the case
/// where the columns *fit*, which shows a horizontal scrollbar under a
/// table with nothing to scroll to.
fn fit(pane: f32, spacing: f32, bar: f32) -> Fit {
    let wanted: f32 =
        COLUMNS.iter().map(|(_, width)| width).sum::<f32>() + spacing * COLUMNS.len() as f32;
    if wanted <= pane - bar {
        Fit::Pane
    } else {
        Fit::Scroll(wanted)
    }
}

/// The width of the inspector pane.
const INSPECTOR_WIDTH: f32 = 280.0;

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

    // The pane is split by hand rather than with `egui::Panel::right`.
    //
    // A right panel reserves its width by moving the parent's *cursor*,
    // and in a top-down layout `available_rect_before_wrap` is derived
    // from the parent's `max_rect` — so the table measured the full pane,
    // laid its last columns out across the inspector, and painted its row
    // backgrounds over the inspector's own text. Splitting the rect here
    // and handing each half an explicit `max_rect` makes the reservation
    // a fact rather than a request, and it is the same arithmetic the
    // panel would have done.
    //
    // The inspector only exists when a row is selected. A permanently
    // reserved three-hundred-point pane reading "Select a process" costs
    // the table a quarter of its width to say nothing, on a view whose
    // whole job is breadth.
    let pane = ui.available_rect_before_wrap();
    let inspector_open = app.selected_row().is_some();
    let reserved = if inspector_open {
        INSPECTOR_WIDTH + SPACE_MD * 2.0
    } else {
        0.0
    };
    let table_rect = egui::Rect::from_min_max(
        pane.min,
        egui::pos2((pane.right() - reserved).max(pane.left()), pane.bottom()),
    );

    if inspector_open {
        let inspector_rect = egui::Rect::from_min_max(
            egui::pos2(pane.right() - INSPECTOR_WIDTH, pane.top()),
            pane.max,
        );
        ui.scope_builder(egui::UiBuilder::new().max_rect(inspector_rect), |ui| {
            inspector(app, ui, &theme);
        });
    }

    if app.details.rows.entries().is_empty() {
        widgets::empty_state(ui, &theme, "No processes match that search");
        return;
    }
    ui.scope_builder(egui::UiBuilder::new().max_rect(table_rect), |ui| {
        table(app, ui, &theme, table_rect);
    });
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
fn table(app: &mut App, ui: &mut Ui, theme: &Palette, pane: egui::Rect) {
    let entries = app.details.rows.shared_entries();
    let Some(snapshot) = app.snapshot.clone() else {
        return;
    };
    let mut clicked: Option<ProcessKey> = None;
    let mut sort_clicked: Option<SortKey> = None;
    let mut action: Option<Action> = None;

    // What the columns actually need, against what the pane has.
    //
    // Ten columns want about 838 points and the pane at the window size
    // the app opens at — with the inspector out — is about 592. The
    // previous answer was to clip, which did not drop the *overflow*, it
    // dropped four whole columns: Threads, Handles, Arch and Session
    // vanished with no scrollbar, no ellipsis and nothing to say they
    // existed. A table cannot answer "too narrow" by silently showing
    // less than it was asked for.
    //
    // So it scrolls, which is what this module's own docs have claimed
    // all along. Note this is not a case that can be designed away by
    // choosing better widths: `MIN_SIZE` lets the window down to 780
    // points, so *no* fixed column set fits every size the app allows.
    let fit = fit(
        pane.width(),
        ui.spacing().item_spacing.x,
        ui.spacing().scroll.allocated_width(),
    );

    // Both axes on *one* scroll area, rather than a horizontal one
    // wrapped around the table's own vertical scroll.
    //
    // Nesting them looks right and is not: a scrollbar is drawn at the
    // edge of the content it scrolls, so the table's vertical scrollbar
    // landed at the right-hand edge of 932 points of columns. In a pane
    // of 588 that is three hundred points off-screen, and the table
    // read as a list that simply ran off the bottom of the window with
    // no way to scroll it. The horizontal bar then sliced the last row
    // in half for good measure.
    //
    // One scroll area puts both bars at the edges of the *viewport*,
    // where they are visible and where they say how much more there is
    // in each direction.
    match fit {
        // Nothing overflows, so nothing is wrapped: the table scrolls
        // itself, its header stays pinned and its scrollbar sits
        // against the pane's edge, exactly as before any of this.
        Fit::Pane => table_body(
            app,
            ui,
            theme,
            pane,
            true,
            &entries,
            &snapshot,
            &mut clicked,
            &mut sort_clicked,
            &mut action,
        ),
        Fit::Scroll(content) => {
            // No hand-rolled clip on the *ui* here: the rows are
            // already held to the pane by `widgets::row_clip`, which
            // has to do it because a row's fill is painted through
            // `set_clip_rect` and that replaces the clip in force
            // rather than intersecting it. Clipping the ui as well
            // would have hidden the earlier fault rather than fixing
            // it — the rows would stop at the pane's edge while the
            // scrollbar that reaches them stayed off-screen.
            egui::ScrollArea::both()
                // Named, and it has to be.
                //
                // A `ScrollArea` keys its stored state — offsets, and
                // whether each bar is showing — on an id derived from
                // its parent `Ui`. The inspector beside this table has
                // a scroll area of its own, and the two collided: one
                // state entry, written twice a frame with different
                // values. egui requests a repaint whenever a bar's
                // visibility changes from what the state says, so the
                // two areas sat flipping that flag at each other and
                // the window never stopped repainting. The offscreen
                // harness is what caught it — it exceeded its step
                // limit with two `scroll_area.rs:1524` repaint causes
                // and nothing else in the list, which is one cause per
                // area rather than one per axis.
                //
                // Nothing about that is visible in a screenshot, and on
                // a real machine it shows only as a fan spinning up on
                // an idle process list.
                .id_salt("details-table")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    // `set_width`, not `set_min_width`: min alone
                    // leaves `available_width` at the viewport, and a
                    // *resizable* `egui_extras` table refits its
                    // columns to whatever that says — so the table
                    // would narrow itself back to the pane it is
                    // supposed to be scrolling past. Fixing the width
                    // at what the columns want is what makes the
                    // content a constant.
                    ui.set_width(content);
                    table_body(
                        app,
                        ui,
                        theme,
                        pane,
                        false,
                        &entries,
                        &snapshot,
                        &mut clicked,
                        &mut sort_clicked,
                        &mut action,
                    );
                });
        }
    }
}

/// The table proper, inside the horizontal scroll area.
///
/// Split out only so [`table`] can stay readable: the scroll wrapper and
/// the twelve-argument body do not belong in one function.
#[expect(
    clippy::too_many_arguments,
    reason = "the pieces one table body needs, threaded through the               scroll-area closure that owns the borrow of `ui`"
)]
fn table_body(
    app: &mut App,
    ui: &mut Ui,
    theme: &Palette,
    pane: egui::Rect,
    // Whether the table is drawn straight into the pane, rather than
    // inside a scroll area that already owns both axes.
    //
    // It governs the vertical scroll and the trailing spacer together,
    // because they are the same question: a table filling the pane
    // scrolls itself and has slack to absorb, and a table inside a
    // scroll area does neither. Two viewports in one is what put the
    // vertical scrollbar three hundred points off-screen; a spacer
    // absorbing slack that does not exist is what made its width
    // disagree with the scroll area's.
    fills_pane: bool,
    entries: &[Entry],
    snapshot: &crate::model::Snapshot,
    clicked: &mut Option<ProcessKey>,
    sort_clicked: &mut Option<SortKey>,
    action: &mut Option<Action>,
) {
    // The *visible* pane, deliberately, even though the content may be
    // wider than it.
    //
    // A row's background is painted through `Painter::set_clip_rect`,
    // which **replaces** the clip in force rather than intersecting it
    // — that is what lets one cell's fill widen across the whole row.
    // The cost is that this rect is the only thing standing between the
    // row and the rest of the window: a content-wide rect here escapes
    // the scroll area's own clip, and the table paints its rows straight
    // across the inspector. Which is the bug the horizontal scrolling
    // was added to fix, arriving back by another door.
    //
    // Scrolled right, the cells beyond this edge are clipped away, and
    // that is correct: they are off-screen.
    let viewport = egui::Rect::from_min_max(egui::pos2(pane.left(), ui.cursor().top()), pane.max);

    theme::quiet_column_rules(ui);
    let body_height = widgets::table_body_height(ui, viewport.height());
    let mut builder = TableBuilder::new(ui)
        .resizable(true)
        // The scroll area above owns both axes now. A table that also
        // scrolled itself would be a second viewport inside the first,
        // which is where the unreachable scrollbar came from.
        //
        // `TableBody::rows` still only draws what is visible: it takes
        // its range from the ui's clip rect, and inside a scroll area
        // that is the viewport either way. Four hundred processes still
        // cost a screenful of rows to draw.
        // `TableBody::rows` only draws what is visible either way: it
        // takes its range from the ui's clip rect, which is the
        // viewport whichever scroll area owns it. Four hundred
        // processes still cost a screenful of rows to draw.
        .vscroll(fills_pane)
        .min_scrolled_height(0.0)
        .max_scroll_height(body_height)
        .auto_shrink([true, false])
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
    if fills_pane {
        builder = builder.column(Column::remainder().clip(true));
    }

    builder
        .header(HEADER_HEIGHT, |mut header| {
            for (index, (key, _)) in COLUMNS.iter().enumerate() {
                header.col(|ui| {
                    let sorted = (app.details.sort == *key).then_some(app.details.descending);
                    // `claims_width: false` for every column now that
                    // none of them is the remainder — see the header
                    // loop in `super::processes`.
                    if widgets::sortable_header(
                        ui,
                        theme,
                        key.label(),
                        sorted,
                        false,
                        index > 0,
                        false,
                    )
                    .clicked()
                    {
                        *sort_clicked = Some(*key);
                    }
                });
            }
            // The spacer's heading, which is empty. Skipping it leaves
            // the header one cell short of the body.
            if fills_pane {
                header.col(|_| {});
            }
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

                let mut row = widgets::Row::record(
                    &mut row,
                    theme,
                    viewport,
                    egui::Id::new("detail-row").with(key),
                    selected,
                    position % 2 == 1,
                );
                for column in 0..COLUMNS.len() {
                    row.cell(|ui| {
                        if column == 0 {
                            ui.add_space(SPACE_SM);
                            ui.label(
                                egui::RichText::new(process.display_name())
                                    .color(theme::rgb(theme.text)),
                            );
                        } else {
                            cell_text(ui, theme, column, process);
                        }
                    });
                }
                // The trailing spacer is a real column and has to be
                // filled, or the row stops one column short of the
                // window's edge.
                if fills_pane {
                    row.spacer();
                }

                let response = row.response();
                if response.clicked() {
                    *clicked = Some(key);
                }
                response.context_menu(|ui| {
                    if !process.is_pseudo() {
                        if ui.button("End task").clicked() {
                            *action = Some(Action::EndTask(key));
                            ui.close();
                        }
                        if ui.button("Open file location").clicked() {
                            *action = Some(Action::Reveal(key));
                            ui.close();
                        }
                        if ui.button("Copy details").clicked() {
                            *action = Some(Action::Copy(key));
                            ui.close();
                        }
                    }
                });
            });
        });

    if let Some(key) = *sort_clicked {
        if app.details.sort == key {
            app.details.descending = !app.details.descending;
        } else {
            app.details.sort = key;
            app.details.descending = key.defaults_descending();
        }
    }
    if let Some(key) = *clicked {
        app.details.selected = Some(key);
    }
    if let Some(action) = action.take() {
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
        SortKey::Status => process.status.column_label().to_string(),
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

    /// The pane the table gets, for a window `window` points wide.
    ///
    /// Stated the way the window computes it rather than as one number,
    /// so a change to the nav rail or the content inset moves these
    /// tests with it.
    fn pane_for(window: f32, inspector: bool) -> f32 {
        let pane = window - theme::NAV_WIDTH - theme::PAD * 2.0;
        if inspector {
            pane - (INSPECTOR_WIDTH + SPACE_MD * 2.0)
        } else {
            pane
        }
    }

    /// `egui_extras` puts `item_spacing.x` between columns and reserves
    /// a lane for the vertical scrollbar.
    const SPACING: f32 = SPACE_SM;
    const SCROLLBAR: f32 = 12.0;

    #[test]
    fn the_columns_fit_the_window_the_app_opens_at() -> anyhow::Result<()> {
        // The common state of this view: default window, no inspector.
        // It has to come out `Pane`, and for two reasons rather than
        // one. A horizontal scrollbar under a table with nothing to
        // scroll to is the visible half; the invisible half is that
        // wrapping it would cost the pinned header and put its vertical
        // scrollbar at the edge of the content instead of the pane.
        let Some(window) = crate::gui::DEFAULT_SIZE.first() else {
            return Ok(());
        };
        let pane = pane_for(*window, false);
        assert_eq!(
            fit(pane, SPACING, SCROLLBAR),
            Fit::Pane,
            "the ten columns do not fit the {window}-point window with the              inspector closed, so the view opens wrapped in a scroll area              it does not need"
        );
        Ok(())
    }

    #[test]
    fn opening_the_inspector_scrolls_rather_than_dropping_columns() -> anyhow::Result<()> {
        // The bug the whole arrangement exists for. The columns want
        // about 838 points and the pane beside an open inspector has
        // about 592, and the original answer — a clip — did not shorten
        // the overflowing column, it removed Threads, Handles, Arch and
        // Session outright with nothing on screen to say so.
        let Some(window) = crate::gui::DEFAULT_SIZE.first() else {
            return Ok(());
        };
        let total: f32 = COLUMNS.iter().map(|(_, width)| width).sum();
        let Fit::Scroll(content) = fit(pane_for(*window, true), SPACING, SCROLLBAR) else {
            anyhow::bail!(
                "the columns fit beside an open inspector, so the scroll                  branch is dead code — if that is really true now, delete                  it rather than this test"
            );
        };
        assert!(
            content >= total,
            "the content is {content} but the columns alone want {total},              so the table is still being asked to draw in less room than it              needs and a column is cut off"
        );
        Ok(())
    }

    #[test]
    fn the_narrowest_window_the_app_allows_still_reaches_every_column() -> anyhow::Result<()> {
        // `MIN_SIZE` is well under what the columns want, which is why
        // no fixed set of widths can be the answer here: whatever they
        // are shrunk to, some allowed window is narrower.
        let total: f32 = COLUMNS.iter().map(|(_, width)| width).sum();
        let Fit::Scroll(content) = fit(pane_for(780.0, false), SPACING, SCROLLBAR) else {
            anyhow::bail!("the narrowest allowed window does not scroll");
        };
        assert!(
            content >= total,
            "the narrowest allowed window cannot reach all ten columns"
        );
        Ok(())
    }

    #[test]
    fn the_scrollbar_lane_comes_out_of_the_pane_rather_than_the_columns() {
        // The repaint loop, stated as arithmetic. Charging the vertical
        // scrollbar's lane to the *content* makes the content exceed
        // the viewport by exactly that lane in the case where the
        // columns fit — a scroll area whose content disagrees with its
        // own viewport every frame, which egui resolves by asking for
        // another frame, forever.
        //
        // So a pane with room for the columns and the lane is `Pane`,
        // and a pane with room for the columns but *not* the lane is
        // not silently treated as though it had both.
        let columns: f32 =
            COLUMNS.iter().map(|(_, width)| width).sum::<f32>() + SPACING * COLUMNS.len() as f32;
        assert_eq!(fit(columns + SCROLLBAR, SPACING, SCROLLBAR), Fit::Pane);
        assert_eq!(
            fit(columns, SPACING, SCROLLBAR),
            Fit::Scroll(columns),
            "a pane exactly as wide as the columns has no room left for              the scrollbar, and pretending otherwise is the loop"
        );
    }

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

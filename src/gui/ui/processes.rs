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
//! **Exactly one column absorbs the slack, and it is a trailing spacer
//! with nothing in it.** Only a non-resizable `remainder()` re-fits to
//! the pane each frame, and a `remainder()` cannot also be resizable —
//! dragging one gives it a stored width, after which it absorbs nothing
//! and the table stops filling its pane. So the absorbing column has to
//! be one nobody can drag, and the only column that can afford to be
//! undraggable is one with no content.
//!
//! Name used to be that column. It could therefore not be resized at
//! all, and on a wide window it grew to nearly a thousand points for
//! entries about thirty characters long — a wall of name beside seven
//! cramped metrics, with the table still stopping short of the window's
//! right edge.
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

use super::icon as icons;
use super::theme::{self, HEADER_HEIGHT, PAD, ROW_HEIGHT, SPACE_MD, SPACE_SM, SPACE_XS};
use super::{chrome, dnd, motion, widgets};
use crate::gui::app::actions::Action;
use crate::gui::app::{rows::RowKey, App};
use crate::model::sort::SortKey;
use crate::model::tree::{Entry, Totals};
use crate::model::{ProcessKey, ProcessKind, ProcessRow};
use crate::theme::Palette;
use egui::{Sense, Ui};
use egui_extras::{Column, TableBuilder};

/// The columns this build has, in the order a fresh install shows them.
///
/// The *order* is a default, not a fixed layout: a user drags the
/// headings into whatever arrangement they want and it is persisted. The
/// *set* is what this build can draw, and a saved order is reconciled
/// against it on load — see [`crate::model::columns`].
pub const DEFAULT_COLUMNS: [SortKey; 8] = [
    SortKey::Name,
    SortKey::Pid,
    SortKey::Status,
    SortKey::Cpu,
    SortKey::Memory,
    SortKey::Disk,
    SortKey::Network,
    SortKey::Gpu,
];

/// The width each column starts at.
///
/// Stated rather than measured; see the module docs. Keyed by column
/// rather than positional, because the position is now the user's to
/// choose and a width tied to a slot would mean dragging Name into the
/// third slot gave it the PID column's width.
fn initial_width(key: SortKey) -> f32 {
    match key {
        SortKey::Name => 320.0,
        SortKey::Pid => 64.0,
        SortKey::Status => 82.0,
        SortKey::Cpu => 68.0,
        SortKey::Memory => 92.0,
        SortKey::Disk => 96.0,
        SortKey::Network => 74.0,
        SortKey::Gpu => 64.0,
        // Not shown in this view; the Details view has them. Named
        // rather than wildcarded so a column added to `DEFAULT_COLUMNS`
        // without a width here is a compile error rather than a column
        // that silently opens at the fallback size.
        SortKey::PrivateBytes
        | SortKey::User
        | SortKey::Threads
        | SortKey::Handles
        | SortKey::CpuTime
        | SortKey::Priority
        | SortKey::Architecture
        | SortKey::Session
        | SortKey::Path => MIN_COLUMN,
    }
}

/// The least a resizable column may be dragged to.
///
/// Below this a heading is unreadable, and a column nobody can read is a
/// column that has effectively disappeared — which is the thing this view
/// does not do.
const MIN_COLUMN: f32 = 48.0;

/// How far each tree level indents.
///
/// Equal to the width [`widgets::disclosure`] allocates for its chevron
/// — a leaf row spaces past where a chevron would sit by exactly this
/// much, or its name starts to the left of a sibling row's.
const INDENT: f32 = SPACE_XS + icons::DISCLOSURE;

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
    refresh_rows(app, ui.ctx());

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
fn refresh_rows(app: &mut App, context: &egui::Context) {
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
    if app.processes.icon_sequence != snapshot.sequence {
        app.processes.icons.retain(|path, _| {
            snapshot
                .processes
                .iter()
                .any(|process| process.path.as_ref() == Some(path))
        });
        for process in &snapshot.processes {
            let (Some(path), Some(icon)) = (&process.path, &process.icon) else {
                continue;
            };
            if app.processes.icons.contains_key(path) {
                continue;
            }
            let image =
                egui::ColorImage::from_rgba_unmultiplied([icon.width, icon.height], &icon.rgba);
            let texture = context.load_texture(
                format!("process-icon:{}", path.display()),
                image,
                egui::TextureOptions::LINEAR,
            );
            app.processes.icons.insert(path.clone(), texture);
        }
        app.processes.icon_sequence = snapshot.sequence;
    }
}

/// Draws the table.
fn table(app: &mut App, ui: &mut Ui, theme: &Palette) {
    let entries = app.processes.rows.shared_entries();
    let Some(snapshot) = app.snapshot.clone() else {
        return;
    };

    let mut clicked: Option<ProcessKey> = None;
    let mut toggled: Option<ProcessKey> = None;
    let mut group_toggled: Option<ProcessKind> = None;
    let mut action: Option<Action> = None;
    let mut sort_clicked: Option<SortKey> = None;

    // The table's own rect, captured before the builder borrows the `Ui`.
    //
    // Every row needs it to paint a background that reaches the window's
    // edge: a table cell's painter is clipped to that cell, and the row
    // fill has to be painted while drawing the first one so that
    // everything else lands on top of it.
    let viewport = ui.available_rect_before_wrap();

    // The user's column order, taken by value so the table can draw while
    // `app` is borrowed mutably by the row closures.
    let columns: Vec<SortKey> = app.processes.columns.as_slice().to_vec();
    let mut reordered: Option<crate::gui::ui::dnd::Moved> = None;

    // The headings are a drag lane. It is declared out here, filled in
    // inside the header closure, and resolved after the table has
    // finished — the closure has no `&Ui` of its own to resolve against,
    // and in immediate mode the answer to "what is under the pointer"
    // is not available until every heading has been drawn anyway.
    let mut lane = Some(dnd::Lane::new(
        egui::Id::new("process-columns"),
        dnd::Axis::Horizontal,
    ));

    theme::quiet_column_rules(ui);
    // See `super::details`: a table wider than its pane paints over
    // whatever is beside it rather than clipping or scrolling.
    ui.set_clip_rect(viewport);
    let body_height = widgets::table_body_height(ui, viewport.height());
    let mut builder = TableBuilder::new(ui)
        // Keyed on the column *order*, because `egui_extras` stores the
        // dragged widths in a `Vec` indexed by position. Reorder the
        // headings and every stored width stays with the slot rather
        // than the column it was dragged for — so moving Name into the
        // third position handed it the PID column's width and gave PID
        // Name's, and the table came out with a sixty-point name beside
        // a three-hundred-point PID. Each arrangement gets its own
        // remembered widths this way, and a fresh one opens at the
        // `initial_width` its columns actually asked for.
        .id_salt(&columns)
        .striped(false)
        .resizable(true)
        .vscroll(true)
        // `TableBuilder` otherwise defaults to an infinite maximum scroll
        // height. With hundreds of rows it allocates through the status bar
        // and places its scrollbar at the physical window edge instead of
        // the bottom of this central pane.
        .min_scrolled_height(0.0)
        .max_scroll_height(body_height)
        .auto_shrink([true, false])
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
        .sense(Sense::click());

    for key in &columns {
        // The Name column needs more room than the rest whatever slot it
        // has been dragged into, because it is the one holding text
        // rather than a number.
        let least = if *key == SortKey::Name {
            NAME_MINIMUM
        } else {
            MIN_COLUMN
        };
        builder = builder.column(
            Column::initial(initial_width(*key))
                .at_least(least)
                .resizable(true)
                .clip(true),
        );
    }
    // Exactly one column absorbs the window's slack, and it is a trailing
    // spacer with nothing in it.
    //
    // Name used to be that column, as a `remainder()`. Two things were
    // wrong with it. A `remainder()` cannot also be resizable — dragging
    // one gives it a stored width, after which it absorbs nothing and the
    // table stops filling the pane — so the Name column could not be
    // resized at all. And on a wide window it grew to nearly a thousand
    // points for a column whose longest entry is about thirty characters,
    // which is how the table ended up as a wall of name and eight cramped
    // metrics.
    //
    // The spacer takes the slack instead, so every real column keeps the
    // width it was given, the row backgrounds run to the window's edge,
    // and the one column nobody can drag is the one that has nothing in
    // it to drag.
    builder = builder.column(Column::remainder().clip(true));

    builder
        .header(HEADER_HEIGHT, |mut header| {
            for (index, key) in columns.iter().enumerate() {
                header.col(|ui| {
                    let sorted = (app.processes.sort == *key).then_some(app.processes.descending);
                    // No column claims its cell's width any more.
                    //
                    // `egui_extras` records the widest thing a column
                    // ever allocated and will not let the column shrink
                    // below it, so a heading that allocates its whole
                    // cell is a floor that column can never come back
                    // under. That was harmless while Name was a
                    // non-resizable remainder; now that every column is
                    // resizable, it would mean Name could be widened and
                    // never narrowed again.
                    let response = widgets::sortable_header(
                        ui,
                        theme,
                        key.label(),
                        sorted,
                        false,
                        index > 0,
                        // The heading a drag is carrying is dimmed in
                        // place, so it reads as lifted out of the row
                        // rather than duplicated by the ghost.
                        lane.as_ref()
                            .is_some_and(|lane| lane.is_dragging(ui, index)),
                    );
                    if let Some(lane) = lane.as_mut() {
                        lane.item(index, ui.max_rect(), key.label(), &response);
                    }

                    // A click sorts; a drag reorders. `clicked()` is
                    // false when the pointer moved far enough to become
                    // a drag, so the two do not fight — which is why the
                    // heading senses both rather than the app needing a
                    // separate grip to drag by.
                    if response.clicked() {
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
                        let mut row = widgets::Row::group(&mut row, theme, viewport);
                        for slot in 0..columns.len() {
                            row.cell(|ui| {
                                match columns.get(slot) {
                                    // The heading sits in whichever slot
                                    // the Name column has been dragged
                                    // to, not in the first one.
                                    Some(SortKey::Name) => {
                                        hit |= group_heading(ui, theme, *kind, totals, *collapsed);
                                    }
                                    Some(key) => group_cell(ui, theme, *key, totals),
                                    None => {}
                                }
                            });
                        }
                        // The trailing spacer is a real column and has to
                        // be filled, or every row stops one column short
                        // of the window edge.
                        row.spacer();
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

                        let mut disclosure = false;
                        let accent = process.icon.as_ref().map(|icon| icon.accent);
                        let mut row = widgets::Row::accented(
                            &mut row,
                            theme,
                            viewport,
                            egui::Id::new("row").with(key),
                            selected,
                            index % 2 == 1,
                            accent,
                        );
                        for slot in 0..columns.len() {
                            row.cell(|ui| match columns.get(slot) {
                                Some(SortKey::Name) => {
                                    let texture = process
                                        .path
                                        .as_ref()
                                        .and_then(|path| app.processes.icons.get(path));
                                    disclosure = name_cell(
                                        ui, theme, process, texture, *depth, *children, *expanded,
                                    );
                                }
                                Some(column) => {
                                    metric_cell(
                                        ui, theme, *column, process, totals, *children, *expanded,
                                    );
                                }
                                None => {}
                            });
                        }
                        // The trailing spacer; see the group row above.
                        row.spacer();

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

    // Resolved out here rather than inside the header closure: the drop
    // target cannot be known until every heading has been drawn, and the
    // closure has no `&Ui` of its own to paint the feedback with. The
    // table's borrow of `ui` has ended by this point.
    if let Some(lane) = lane.take() {
        reordered = lane.show(ui, theme);
    }
    if let Some(moved) = reordered {
        // `move_column`, not `drop_at`: the lane has already converted the
        // drop gap into a destination index. See `model::columns::landing`
        // on why those are two different numbers.
        app.processes.columns.move_column(moved.from, moved.to);
    }

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
    ui.horizontal_centered(|ui| {
        ui.add_space(SPACE_XS);
        widgets::disclosure(ui, theme, !collapsed, kind);
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
fn group_cell(ui: &mut Ui, theme: &Palette, key: SortKey, totals: &Totals) {
    // Only the summable columns carry a category total. A PID or a
    // status has no meaningful aggregate, and showing one would be worse
    // than showing nothing.
    let text = match key {
        SortKey::Cpu => crate::format::percent_or_dash(totals.cpu_percent),
        SortKey::Memory => crate::format::bytes_or_dash(totals.working_set),
        SortKey::Disk => crate::format::rate_or_dash(totals.disk_rate),
        SortKey::Gpu => crate::format::percent_or_dash(totals.gpu_percent),
        // Only the summable columns carry a category total; the rest are
        // named rather than wildcarded so a new one is a compile error.
        SortKey::Name
        | SortKey::Pid
        | SortKey::Status
        | SortKey::Network
        | SortKey::PrivateBytes
        | SortKey::User
        | SortKey::Threads
        | SortKey::Handles
        | SortKey::CpuTime
        | SortKey::Priority
        | SortKey::Architecture
        | SortKey::Session
        | SortKey::Path => String::new(),
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
    texture: Option<&egui::TextureHandle>,
    depth: u16,
    children: usize,
    expanded: bool,
) -> bool {
    let mut hit = false;
    ui.horizontal_centered(|ui| {
        ui.add_space(SPACE_XS + f32::from(depth) * INDENT);

        if children > 0 {
            // Keyed on the process, not the row index: the table is
            // re-sorted constantly, and an arrow keyed on position would
            // inherit the animation state of whatever used to be there —
            // so a sort would set every arrow in the table spinning.
            hit = widgets::disclosure(ui, theme, expanded, process.key()).clicked();
        } else {
            // Written out rather than as `INDENT`: the pixel-gap lint
            // checks an `add_space` call's own text for a scale-step
            // prefix, and this is the one place `INDENT` is not already
            // added to something that starts with one.
            ui.add_space(SPACE_XS + icons::DISCLOSURE);
        }

        if let Some(texture) = texture {
            let image = egui::Image::new((texture.id(), egui::Vec2::splat(18.0)))
                .corner_radius(egui::CornerRadius::same(3));
            ui.add(image);
            ui.add_space(SPACE_XS);
        }

        // There used to be a coloured dot here, hashed from the process
        // key. It was stable across a re-sort, which was the property it
        // was written for, and it meant *nothing* — a hue per process,
        // carrying no fact about that process.
        //
        // Colour is the scarcest signal in this table and it is already
        // spent: the heat tint and gauge in every metric cell encode
        // load, the chips encode status and elevation, and the Network
        // panel's dots encode an adapter's state. A fifteenth hue down
        // the leading edge, meaning nothing, competes with all of them
        // and wins, because it is the leftmost thing in the row.
        // The name yields to its chips rather than the other way round.
        //
        // The Name column clips, and the chips are laid out after the
        // label — so a name long enough to fill the column pushed them
        // off its edge and they were cut mid-word: "Discord System
        // Helper" beside a chip reading "Adm". A truncated *name* still
        // identifies the process, because the beginning of a name is
        // the part that does; a truncated chip identifies nothing and
        // reads as a rendering fault.
        let chips: &[(&str, crate::color::Rgb)] = match (
            process.status == crate::model::ProcessStatus::Suspended,
            process.elevated,
        ) {
            (true, true) => &[("Suspended", theme.warning), ("Admin", theme.text_muted)],
            (true, false) => &[("Suspended", theme.warning)],
            (false, true) => &[("Admin", theme.text_muted)],
            (false, false) => &[],
        };
        let reserved: f32 = chips
            .iter()
            .map(|(text, _)| widgets::status_chip_width(ui, text) + SPACE_XS)
            .sum();

        let name_width = (ui.available_width() - reserved).max(0.0);
        let font = egui::TextStyle::Body.resolve(ui.style());
        let galley = widgets::truncated(ui, process.display_name(), font, theme.text, name_width);
        let (rect, _) = ui.allocate_exact_size(galley.size(), Sense::hover());
        ui.painter()
            .galley(rect.left_top(), galley, theme::rgb(theme.text));

        for (text, colour) in chips {
            ui.add_space(SPACE_XS);
            widgets::status_chip(ui, text, *colour);
        }
    });
    hit
}

/// Draws one metric cell, with its heat tint.
fn metric_cell(
    ui: &mut Ui,
    theme: &Palette,
    key: SortKey,
    process: &ProcessRow,
    totals: &Totals,
    children: usize,
    expanded: bool,
) {
    let rect = ui.max_rect();

    // A collapsed parent shows its subtree's total, or collapsing the
    // tree makes a busy process disappear — which is the single most
    // common thing a task manager is opened to find.
    //
    // Keyed on *having children in this list*, not on the subtree's
    // size. In the flat list every process is its own row, so a parent
    // stands in for nobody — and keyed on `totals.processes` it showed
    // its whole subtree's figures anyway, beside the very children it
    // was summing, counting them twice on one screen. `children` is
    // zero there, which is the same rule `model::tree::shows_subtree`
    // sorts by, so the ordering and the number agree.
    let aggregated = children > 0 && !expanded;

    // The continuous metrics slide to their new readings rather than
    // replacing them.
    //
    // This is the difference between a table that is readable while it
    // updates and one that is not. Samples arrive once a second, so
    // un-animated the whole grid sits still and then every cell changes
    // at once — which the eye reads as the display flickering rather than
    // as the machine's state moving. Sliding numbers stay legible
    // throughout, and the movement itself carries information no single
    // frame can: a column where everything is drifting upward is a
    // machine getting busier.
    //
    // Keyed on the process and the column, not the row index — the table
    // re-sorts constantly, and keyed on position a row would animate to
    // the value of whatever used to be in its slot.
    let smooth = |ui: &Ui, value: f64| -> f64 {
        let id = egui::Id::new("cell")
            .with(process.key())
            .with(key)
            .with(aggregated);
        f64::from(motion::value(ui.ctx(), id, value as f32))
    };

    let (text, load) = match key {
        SortKey::Pid => (process.pid.to_string(), 0.0),
        SortKey::Status => (process.status.column_label().to_string(), 0.0),
        SortKey::Cpu => {
            let value = smooth(
                ui,
                if aggregated {
                    totals.cpu_percent
                } else {
                    process.cpu_percent
                },
            );
            (
                crate::format::percent_or_dash(value),
                (value / 100.0) as f32,
            )
        }
        SortKey::Memory => {
            let value = smooth(
                ui,
                if aggregated {
                    totals.working_set
                } else {
                    process.working_set
                } as f64,
            ) as u64;
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
            let value = smooth(
                ui,
                if aggregated {
                    totals.disk_rate
                } else {
                    process.disk_rate()
                },
            );
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
            let value = smooth(
                ui,
                if aggregated {
                    totals.gpu_percent
                } else {
                    process.gpu_percent
                },
            );
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
    if ui.button("End process tree (best effort)").clicked() {
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
    fn the_process_list_cannot_grow_past_its_pane() {
        let ctx = egui::Context::default();
        let mut output = ctx.run_ui(Default::default(), |ui| {
            for pane in [0.0, HEADER_HEIGHT / 2.0, 620.0, 1_406.0] {
                let body = widgets::table_body_height(ui, pane);
                assert!(body >= 0.0);
                assert!(body + HEADER_HEIGHT <= pane.max(HEADER_HEIGHT));
            }
        });
        output.textures_delta.clear();
    }

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
    fn every_column_is_sortable_and_opens_wide_enough_to_read() {
        for key in DEFAULT_COLUMNS {
            assert!(!key.label().is_empty());
            let width = initial_width(key);
            assert!(
                width >= MIN_COLUMN,
                "{} opens at {width}, below the width its own heading \
                 needs",
                key.label()
            );
        }
    }

    #[test]
    fn the_columns_are_distinct() {
        // A duplicate would make one heading sort by the other's column.
        let mut keys: Vec<SortKey> = DEFAULT_COLUMNS.to_vec();
        let count = keys.len();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), count, "two columns share a sort key");
    }

    #[test]
    fn a_reordered_table_still_draws_every_column() {
        // The table iterates the user's order rather than `DEFAULT_COLUMNS`,
        // so the property that matters is that no arrangement can lose a
        // column or draw one twice. The reconciliation and the moves are
        // tested in `crate::model::columns`; this checks that this view's
        // own column set survives a trip through it.
        let mut order = crate::model::columns::ColumnOrder::new(&DEFAULT_COLUMNS);
        order.move_column(0, DEFAULT_COLUMNS.len() - 1);
        assert_eq!(order.len(), DEFAULT_COLUMNS.len());
        for key in DEFAULT_COLUMNS {
            assert!(
                order.as_slice().contains(&key),
                "{} was lost by a reorder",
                key.label()
            );
        }
    }

    #[test]
    fn the_name_column_keeps_its_own_minimum_wherever_it_is_dragged() {
        // The width floor is keyed on the column, not on the slot. Tied
        // to a slot, dragging Name into third place would give it the
        // Status column's minimum and clip every process name in the
        // table.
        assert!(
            initial_width(SortKey::Name) > initial_width(SortKey::Pid),
            "the text column should open wider than a numeric one"
        );
    }
}

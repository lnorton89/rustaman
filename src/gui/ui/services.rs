// ============================================================================
// Module:       gui::ui::services
// Description:  The Services and Startup views — both read on their own
//               schedule rather than by the sampler.
//
// Dependencies: egui, egui_extras; crate::win::{services, startup}
// ============================================================================

//! Services, and the programs that run at logon.
//!
//! Two views in one module because they share their whole shape: a list
//! read on demand, a search box, and a detail line. Neither is read by
//! the sampler — see [`crate::engine::sampler`] — because neither changes
//! on a one-second timescale, and enumerating four hundred services every
//! second to redraw a list nobody is looking at is exactly what makes a
//! monitoring tool a load of its own.
//!
//! ## Refreshed on entry and on demand
//!
//! Each view reads its list when it is first shown and then leaves it
//! alone until the user asks, or until an action changes it — starting a
//! service clears the timestamp, so the next frame re-reads and the row's
//! state updates. [`STALE_AFTER`] puts a ceiling on how out of date a
//! list left open can get.

use super::theme::{self, HEADER_HEIGHT, ROW_HEIGHT, SPACE_MD, SPACE_SM, SPACE_XS};
use super::{chrome, widgets};
use crate::gui::app::actions::Action;
use crate::gui::app::{service_label, App, ServiceSortKey, StartupSortKey};
use crate::theme::Palette;
use crate::win::services::{Service, ServiceState};
use crate::win::startup::StartupEntry;
use egui::{Sense, Ui};
use egui_extras::{Column, TableBuilder};
use std::time::{Duration, Instant};

/// How long a list left open may go without being re-read.
///
/// Ten seconds: long enough that switching between views does not
/// re-enumerate constantly, short enough that a service someone started
/// in another window shows up before they wonder why it has not.
const STALE_AFTER: Duration = Duration::from_secs(10);

/// Draws the Services view.
pub fn draw(app: &mut App, ui: &mut Ui) {
    let theme = app.theme.clone();
    refresh(app);

    if app.services.refreshed.is_none() {
        // The first read is in flight; showing the empty state here
        // instead would claim there are no services on the machine,
        // which is a different thing and one worth telling apart from
        // "still finding out" — see the module docs on why an empty
        // result is ambiguous enough to need its own wording already.
        widgets::empty_state(ui, &theme, "Reading services…");
        return;
    }

    ui.horizontal(|ui| {
        let _ = chrome::search_box(ui, &theme, &mut app.services.search, "Search services");
        chrome::toolbar_dot(ui, &theme);

        let running = app
            .services
            .services
            .iter()
            .filter(|service| service.state == ServiceState::Running)
            .count();
        widgets::chip(
            ui,
            &format!("{running} running of {}", app.services.services.len()),
            theme.raised,
            theme.text_muted,
        );

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let selected = app.services.selected.clone();
            let state = selected.as_ref().and_then(|name| {
                app.services
                    .services
                    .iter()
                    .find(|service| &service.name == name)
                    .map(|service| service.state)
            });

            // The available action depends on the state, and a control
            // that is present but does nothing is worse than one that is
            // absent — so Start and Stop are drawn only when they mean
            // something.
            if state.is_some_and(ServiceState::can_stop) {
                if widgets::danger_button(ui, &theme, "Stop", true).clicked() {
                    if let Some(name) = selected.clone() {
                        app.dispatch(Action::StopService(name), ui);
                    }
                }
            } else if state.is_some_and(ServiceState::can_start)
                && widgets::primary_button(ui, &theme, "Start").clicked()
            {
                if let Some(name) = selected.clone() {
                    app.dispatch(Action::StartService(name), ui);
                }
            }
        });
    });
    ui.add_space(chrome::TOOLBAR_GAP);

    if app.services.services.is_empty() {
        widgets::empty_state(
            ui,
            &theme,
            "No services could be read from the service control manager",
        );
        return;
    }
    table(app, ui, &theme);
}

/// Re-reads the service list if it is missing or stale.
///
/// `EnumServicesStatusExW` is a real syscall against the service control
/// manager, so it runs on a background thread rather than here — see
/// [`crate::gui::app::background`]. This only starts the read and drains
/// whichever one is already in flight; neither half blocks.
fn refresh(app: &mut App) {
    if let Some(pending) = &app.services.pending {
        if let Some(services) = pending.poll() {
            app.services.services = services;
            app.services.refreshed = Some(Instant::now());
            app.services.pending = None;
        }
    }

    let stale = app
        .services
        .refreshed
        .is_none_or(|at| at.elapsed() > STALE_AFTER);
    if stale && app.services.pending.is_none() {
        app.services.pending = Some(crate::gui::app::background::BackgroundRead::spawn(
            "rustaman-services",
            crate::win::services::enumerate,
        ));
    }
}

/// Rebuilds the filtered, sorted service list if the search text, the
/// sort, or the underlying list itself has changed since the last frame.
///
/// A few hundred services filtered by substring and sorted with a
/// case-folded comparator is not the process-tree-sized cost
/// `rows::Cache` exists to avoid, but it is still real work with no
/// reason to repeat sixty times a second when nothing that would change
/// its answer has.
fn refresh_visible_services(app: &mut App) {
    let key = (
        app.services.search.to_lowercase(),
        app.services.sort,
        app.services.descending,
        app.services.refreshed,
    );
    if app.services.visible_key.as_ref() == Some(&key) {
        return;
    }
    let mut visible: Vec<Service> = app
        .services
        .services
        .iter()
        .filter(|service| matches(service, &key.0))
        .cloned()
        .collect();
    visible.sort_by(|a, b| key.1.compare_directed(a, b, key.2));
    app.services.visible = visible;
    app.services.visible_key = Some(key);
}

/// The services table.
fn table(app: &mut App, ui: &mut Ui, theme: &Palette) {
    refresh_visible_services(app);
    let visible = app.services.visible.clone();

    if visible.is_empty() {
        widgets::empty_state(ui, theme, "No services match that search");
        return;
    }

    let mut clicked: Option<String> = None;
    let mut sort_clicked: Option<ServiceSortKey> = None;
    let mut action: Option<Action> = None;

    // PID to process name, built once for the whole table rather than
    // searched per row: the process list is four hundred entries and the
    // service list is three hundred, and a linear search per row inside
    // a draw call is the sixty-thousand-comparisons-a-frame shape this
    // codebase has a rule against.
    let hosts: std::collections::HashMap<u32, String> =
        app.snapshot
            .as_ref()
            .map_or_else(std::collections::HashMap::new, |snapshot| {
                snapshot
                    .processes
                    .iter()
                    .map(|process| (process.pid, process.name.clone()))
                    .collect()
            });

    // Captured before the builder borrows the `Ui`; see
    // `widgets::row_background` on why a row needs it.
    let viewport = ui.available_rect_before_wrap();

    theme::quiet_column_rules(ui);
    TableBuilder::new(ui)
        .resizable(true)
        .vscroll(true)
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
        .sense(Sense::click())
        .column(Column::remainder().at_least(220.0).clip(true))
        .column(
            Column::initial(180.0)
                .at_least(80.0)
                .resizable(true)
                .clip(true),
        )
        .column(Column::initial(90.0).at_least(70.0).resizable(true))
        .column(Column::initial(70.0).at_least(56.0).resizable(true))
        // The host process. Ten services share one `svchost.exe`, and
        // "which of the fifteen services inside this svchost is the one
        // burning the CPU" is the question this view exists beside the
        // process list to answer — see the module docs. The PID column
        // alone makes the reader carry a number across two views to find
        // out; naming the process closes it here.
        .column(
            Column::initial(200.0)
                .at_least(120.0)
                .resizable(true)
                .clip(true),
        )
        .header(HEADER_HEIGHT, |mut header| {
            for (index, key) in [
                ServiceSortKey::Name,
                ServiceSortKey::ShortName,
                ServiceSortKey::Status,
                ServiceSortKey::Pid,
            ]
            .into_iter()
            .enumerate()
            {
                header.col(|ui| {
                    let sorted = (app.services.sort == key).then_some(app.services.descending);
                    if widgets::sortable_header(
                        ui,
                        theme,
                        key.label(),
                        sorted,
                        index == 0,
                        index == 3,
                        false,
                    )
                    .clicked()
                    {
                        sort_clicked = Some(key);
                    }
                });
            }
            // Not sortable: it is the process's name, and the PID column
            // beside it already sorts services onto their shared host.
            header.col(|ui| widgets::plain_header(ui, theme, "Host process"));
        })
        .body(|body| {
            body.rows(ROW_HEIGHT, visible.len(), |mut row| {
                let position = row.index();
                let Some(service) = visible.get(position) else {
                    return;
                };
                let selected = app.services.selected.as_ref() == Some(&service.name);

                row.col(|ui| {
                    widgets::row_background(
                        ui,
                        theme,
                        viewport,
                        egui::Id::new("service-row").with(position),
                        selected,
                        false,
                        position % 2 == 1,
                    );
                    ui.add_space(SPACE_SM);
                    // The display name leads: it is what a person
                    // recognises. The short name is the next column,
                    // because it is what `sc` and `net` take.
                    ui.label(
                        egui::RichText::new(service_label(service)).color(theme::rgb(theme.text)),
                    );
                });
                row.col(|ui| {
                    ui.label(
                        egui::RichText::new(&service.name)
                            .color(theme::rgb(theme.text_muted))
                            .text_style(egui::TextStyle::Small),
                    );
                });
                row.col(|ui| {
                    let color = match service.state {
                        ServiceState::Running => theme.success,
                        ServiceState::Stopped => theme.text_muted,
                        ServiceState::Starting | ServiceState::Stopping => theme.warning,
                        ServiceState::Other => theme.text_muted,
                    };
                    widgets::status_chip(ui, service.state.label(), color);
                });
                row.col(|ui| {
                    let text = service
                        .pid
                        .map_or_else(|| crate::format::DASH.to_string(), |pid| pid.to_string());
                    widgets::number(ui, theme, &text, service.pid.is_none());
                });
                row.col(|ui| {
                    // A stopped service has no host, and neither has a
                    // running one whose host is outside this snapshot —
                    // a service can start between two samples. Both read
                    // as absence rather than as an empty cell.
                    let host = service
                        .pid
                        .and_then(|pid| hosts.get(&pid).cloned())
                        .unwrap_or_else(|| crate::format::DASH.to_string());
                    ui.add_space(SPACE_SM);
                    ui.label(
                        egui::RichText::new(host)
                            .color(theme::rgb(theme.text_muted))
                            .text_style(egui::TextStyle::Small),
                    );
                });

                let response = row.response();
                if response.clicked() {
                    clicked = Some(service.name.clone());
                }
                response.context_menu(|ui| {
                    if service.state.can_start() && ui.button("Start").clicked() {
                        action = Some(Action::StartService(service.name.clone()));
                        ui.close();
                    }
                    if service.state.can_stop() && ui.button("Stop").clicked() {
                        action = Some(Action::StopService(service.name.clone()));
                        ui.close();
                    }
                    // The link back to the process list, which is the
                    // main reason this view is worth having next to it.
                    if service.pid.is_some() && ui.button("Go to process").clicked() {
                        if let Some(pid) = service.pid {
                            select_process(app, pid);
                        }
                        ui.close();
                    }
                });
            });
        });

    if let Some(key) = sort_clicked {
        if app.services.sort == key {
            app.services.descending = !app.services.descending;
        } else {
            app.services.sort = key;
            app.services.descending = key.defaults_descending();
        }
    }
    if let Some(name) = clicked {
        app.services.selected = Some(name);
    }
    if let Some(action) = action {
        app.dispatch(action, ui);
    }
}

/// Whether a service matches the search text.
///
/// Both names are searched: a person looking for "print spooler" and one
/// looking for "spooler" are both looking for the same service, and only
/// one of those is the display name.
#[must_use]
pub fn matches(service: &Service, lowercase_query: &str) -> bool {
    if lowercase_query.is_empty() {
        return true;
    }
    service.name.to_lowercase().contains(lowercase_query)
        || service
            .display_name
            .to_lowercase()
            .contains(lowercase_query)
}

/// Whether a startup entry matches the search text.
///
/// Both the registered name and the command are searched: a person
/// looking for "onedrive" and one looking for the path it launches from
/// are both trying to find the same entry, and a machine can register
/// dozens of these across per-user and all-users locations with no other
/// way to narrow the list.
#[must_use]
pub fn startup_matches(entry: &StartupEntry, lowercase_query: &str) -> bool {
    if lowercase_query.is_empty() {
        return true;
    }
    entry.name.to_lowercase().contains(lowercase_query)
        || entry.command.to_lowercase().contains(lowercase_query)
}

/// Switches to the Processes view with a PID selected.
///
/// The link from "svchost.exe is at 40% CPU" to which of the fifteen
/// services inside it is responsible — and back again.
fn select_process(app: &mut App, pid: u32) {
    let Some(snapshot) = app.snapshot.as_ref() else {
        return;
    };
    let Some(row) = snapshot.processes.iter().find(|row| row.pid == pid) else {
        app.notify("That service's process is no longer running", true);
        return;
    };
    let key = row.key();
    app.processes.selected = Some(key);
    // Expanded from the root down, or the selected row is inside a
    // collapsed subtree and the jump appears to do nothing.
    let mut ancestors = Vec::new();
    let mut cursor = row.parent_pid;
    while cursor != 0 {
        let Some(parent) = snapshot.processes.iter().find(|row| row.pid == cursor) else {
            break;
        };
        if ancestors.contains(&parent.key()) {
            break;
        }
        ancestors.push(parent.key());
        cursor = parent.parent_pid;
    }
    app.processes.expanded.extend(ancestors);
    app.processes.collapsed.clear();
    app.view = crate::gui::app::View::Processes;
}

/// Draws the Startup view.
pub fn draw_startup(app: &mut App, ui: &mut Ui) {
    let theme = app.theme.clone();
    refresh_startup(app);

    if app.startup.refreshed.is_none() {
        widgets::empty_state(ui, &theme, "Reading startup entries…");
        return;
    }

    ui.horizontal(|ui| {
        let _ = chrome::search_box(
            ui,
            &theme,
            &mut app.startup.search,
            "Search startup entries",
        );
        chrome::toolbar_dot(ui, &theme);

        let enabled = app
            .startup
            .entries
            .iter()
            .filter(|entry| entry.enabled)
            .count();
        widgets::chip(
            ui,
            &format!("{enabled} enabled of {}", app.startup.entries.len()),
            theme.raised,
            theme.text_muted,
        );
        chrome::toolbar_dot(ui, &theme);
        ui.label(
            egui::RichText::new(
                "Rustaman reports what is registered to run at logon. \
                 Changing it is done in the location each entry names.",
            )
            .color(theme::rgb(theme.text_faint))
            .text_style(egui::TextStyle::Small),
        );
    });
    ui.add_space(chrome::TOOLBAR_GAP);

    if app.startup.entries.is_empty() {
        widgets::empty_state(ui, &theme, "Nothing is registered to run at logon");
        return;
    }
    startup_table(app, ui, &theme);
}

/// Re-reads the startup list if it is missing or stale.
///
/// The registry and startup-folder walk behind this is a real read
/// against disk, so it runs on a background thread — see
/// [`crate::gui::app::background`] and [`refresh`]'s matching comment.
fn refresh_startup(app: &mut App) {
    if let Some(pending) = &app.startup.pending {
        if let Some(entries) = pending.poll() {
            app.startup.entries = entries;
            app.startup.refreshed = Some(Instant::now());
            app.startup.pending = None;
        }
    }

    let stale = app
        .startup
        .refreshed
        .is_none_or(|at| at.elapsed() > STALE_AFTER);
    if stale && app.startup.pending.is_none() {
        app.startup.pending = Some(crate::gui::app::background::BackgroundRead::spawn(
            "rustaman-startup",
            crate::win::startup::enumerate,
        ));
    }
}

/// See [`refresh_visible_services`] — the same cache, for the startup
/// list.
fn refresh_visible_startup(app: &mut App) {
    let key = (
        app.startup.search.to_lowercase(),
        app.startup.sort,
        app.startup.descending,
        app.startup.refreshed,
    );
    if app.startup.visible_key.as_ref() == Some(&key) {
        return;
    }
    let mut visible: Vec<StartupEntry> = app
        .startup
        .entries
        .iter()
        .filter(|entry| startup_matches(entry, &key.0))
        .cloned()
        .collect();
    visible.sort_by(|a, b| key.1.compare_directed(a, b, key.2));
    app.startup.visible = visible;
    app.startup.visible_key = Some(key);
}

/// The startup table.
fn startup_table(app: &mut App, ui: &mut Ui, theme: &Palette) {
    refresh_visible_startup(app);
    let entries = app.startup.visible.clone();

    if entries.is_empty() {
        widgets::empty_state(ui, theme, "No startup entries match that search");
        return;
    }

    let mut clicked: Option<String> = None;
    let mut sort_clicked: Option<StartupSortKey> = None;
    let mut reveal: Option<std::path::PathBuf> = None;

    // Captured before the builder borrows the `Ui`; see
    // `widgets::row_background` on why a row needs it.
    let viewport = ui.available_rect_before_wrap();

    theme::quiet_column_rules(ui);
    TableBuilder::new(ui)
        .resizable(true)
        .vscroll(true)
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
        .sense(Sense::click())
        // Name is a stated width rather than the remainder. As the
        // remainder it absorbed every point of a 1440-wide window that
        // the other two columns did not want — seven hundred of them for
        // entries like "OneDrive" — while the one thing a person opens
        // this view to check, *what the entry actually runs*, was in a
        // hover tooltip. The command is a column now, and it is the
        // column the slack goes to, because a command line is the one
        // field here that can genuinely use six hundred points.
        .column(
            Column::initial(240.0)
                .at_least(140.0)
                .resizable(true)
                .clip(true),
        )
        .column(Column::initial(90.0).at_least(70.0).resizable(true))
        .column(
            Column::initial(150.0)
                .at_least(110.0)
                .resizable(true)
                .clip(true),
        )
        .column(Column::remainder().at_least(160.0).clip(true))
        .header(HEADER_HEIGHT, |mut header| {
            for (index, key) in [
                StartupSortKey::Name,
                StartupSortKey::Status,
                StartupSortKey::Location,
            ]
            .into_iter()
            .enumerate()
            {
                header.col(|ui| {
                    let sorted = (app.startup.sort == key).then_some(app.startup.descending);
                    if widgets::sortable_header(
                        ui,
                        theme,
                        key.label(),
                        sorted,
                        index == 0,
                        false,
                        false,
                    )
                    .clicked()
                    {
                        sort_clicked = Some(key);
                    }
                });
            }
            // Not sortable: sorting a list of command lines alphabetically
            // orders it by drive letter and then by `Program Files`,
            // which is not an order anyone wants a startup list in.
            header.col(|ui| widgets::plain_header(ui, theme, "Command"));
        })
        .body(|body| {
            body.rows(ROW_HEIGHT, entries.len(), |mut row| {
                let position = row.index();
                let Some(entry) = entries.get(position) else {
                    return;
                };
                let selected = app.startup.selected.as_ref() == Some(&entry.name);

                row.col(|ui| {
                    widgets::row_background(
                        ui,
                        theme,
                        viewport,
                        egui::Id::new("startup-row").with(position),
                        selected,
                        false,
                        position % 2 == 1,
                    );
                    ui.add_space(SPACE_SM);
                    ui.label(egui::RichText::new(&entry.name).color(theme::rgb(theme.text)));
                    if entry.all_users {
                        ui.add_space(SPACE_XS);
                        widgets::status_chip(ui, "All users", theme.text_muted);
                    }
                });
                row.col(|ui| {
                    let (label, color) = if entry.enabled {
                        ("Enabled", theme.success)
                    } else {
                        ("Disabled", theme.text_muted)
                    };
                    widgets::status_chip(ui, label, color);
                });
                row.col(|ui| {
                    ui.label(
                        egui::RichText::new(entry.location)
                            .color(theme::rgb(theme.text_muted))
                            .text_style(egui::TextStyle::Small),
                    );
                });
                row.col(|ui| {
                    // Monospace, because this is a path: a proportional
                    // font makes two entries under the same directory
                    // look like two unrelated strings, and the shared
                    // prefix is most of what a person is scanning for.
                    ui.label(
                        egui::RichText::new(&entry.command)
                            .color(theme::rgb(theme.text_faint))
                            .text_style(egui::TextStyle::Monospace),
                    );
                });

                let response = row.response();
                if response.clicked() {
                    clicked = Some(entry.name.clone());
                }
                response
                    .clone()
                    .on_hover_text(&entry.command)
                    .context_menu(|ui| {
                        if ui.button("Open file location").clicked() {
                            reveal = entry.executable();
                            ui.close();
                        }
                        if ui.button("Copy command").clicked() {
                            ui.ctx().copy_text(entry.command.clone());
                            ui.close();
                        }
                    });
            });
        });

    if let Some(key) = sort_clicked {
        if app.startup.sort == key {
            app.startup.descending = !app.startup.descending;
        } else {
            app.startup.sort = key;
            app.startup.descending = key.defaults_descending();
        }
    }
    if let Some(name) = clicked {
        app.startup.selected = Some(name);
    }
    if let Some(path) = reveal {
        let result = crate::win::control::reveal_in_explorer(&path);
        match result {
            Ok(()) => app.notify("Opened file location", false),
            Err(error) => app.notify(error.to_string(), true),
        }
    }
}

/// The gap the view leaves between its rows.
pub const ROW_GAP: f32 = SPACE_MD;

#[cfg(test)]
mod tests {
    use super::*;

    fn service(name: &str, display: &str, state: ServiceState) -> Service {
        Service {
            name: name.to_string(),
            display_name: display.to_string(),
            state,
            pid: None,
        }
    }

    #[test]
    fn a_search_matches_either_name() {
        // Someone looking for "print spooler" and someone looking for
        // "spooler" want the same service, and only one of those is the
        // display name.
        let spooler = service("Spooler", "Print Spooler", ServiceState::Running);
        assert!(matches(&spooler, "spooler"));
        assert!(matches(&spooler, "print"));
        assert!(!matches(&spooler, "defender"));
    }

    #[test]
    fn an_empty_search_matches_everything() {
        let spooler = service("Spooler", "Print Spooler", ServiceState::Running);
        assert!(matches(&spooler, ""));
    }

    fn startup_entry(name: &str, command: &str) -> StartupEntry {
        StartupEntry {
            name: name.to_string(),
            command: command.to_string(),
            location: "HKCU Run",
            all_users: false,
            enabled: true,
        }
    }

    #[test]
    fn a_startup_search_matches_the_name_or_the_command() {
        // A person searching "onedrive" and one searching the path it
        // launches from are both looking for the same entry.
        let onedrive = startup_entry("OneDrive", "C:\\OneDrive\\OneDrive.exe /background");
        assert!(startup_matches(&onedrive, "onedrive"));
        assert!(startup_matches(&onedrive, "background"));
        assert!(!startup_matches(&onedrive, "defender"));
    }

    #[test]
    fn an_empty_startup_search_matches_everything() {
        let onedrive = startup_entry("OneDrive", "C:\\OneDrive\\OneDrive.exe");
        assert!(startup_matches(&onedrive, ""));
    }

    #[test]
    fn a_stale_window_is_long_enough_to_avoid_churn_and_short_enough_to_notice() {
        // Long enough that switching views does not re-enumerate
        // constantly; short enough that a service started elsewhere shows
        // up before anyone wonders why it has not.
        assert!(STALE_AFTER >= Duration::from_secs(5));
        assert!(STALE_AFTER <= Duration::from_secs(30));
    }
}

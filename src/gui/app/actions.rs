// ============================================================================
// Module:       gui::app::actions
// Description:  What a click on End task, Suspend, or a priority menu actually
//               does, including which of them ask first.
//
// Dependencies: crate::win::control, crate::win::services; super::App
// ============================================================================

//! Acting on the selection.
//!
//! Every function here is the *whole* path from a click to a system call
//! and back to a status message. Nothing is dispatched through a queue or
//! a channel — an action a human initiated at human speed does not need
//! one, and a queue would make "did that work?" a question with no
//! immediate answer.
//!
//! ## What asks first, and why only those
//!
//! Three things ask for confirmation, and the rule is that a
//! confirmation exists where a mistaken click is **not recoverable**:
//!
//! - **End task** loses whatever the process had not saved.
//! - **End process tree** loses it for a subtree the user cannot see the
//!   whole of, so the count is shown.
//! - **Realtime priority** can make a machine stop answering the
//!   keyboard entirely, with no way back short of the power button. See
//!   [`crate::model::Priority::is_dangerous`].
//!
//! Suspend, resume, and every other priority class are immediately
//! reversible by the control that set them, so they act at once. A
//! confirmation on a reversible action is not caution, it is friction —
//! and an app that asks about everything trains people to click through
//! the one dialog that mattered.
//!
//! The End task confirmation can be turned off in Settings. That is a
//! real preference — someone killing a crashed process forty times in a
//! debugging session means it — and it is off *by choice*, never by
//! default.

use super::{App, Pending};
use crate::model::{Priority, ProcessKey, ProcessRow};
use crate::win::control;

/// Something the user asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// End the selected process.
    EndTask(ProcessKey),
    /// End it and everything under it.
    EndTree(ProcessKey),
    /// Suspend it.
    Suspend(ProcessKey),
    /// Resume it.
    Resume(ProcessKey),
    /// Set its priority class.
    SetPriority(ProcessKey, Priority),
    /// Open its folder in Explorer with the file selected.
    Reveal(ProcessKey),
    /// Copy its details to the clipboard.
    Copy(ProcessKey),
    /// Start a service.
    StartService(String),
    /// Stop a service.
    StopService(String),
}

impl App {
    /// Performs an action, asking first where the module docs say to.
    pub fn dispatch(&mut self, action: Action, ui: &egui::Ui) {
        match action {
            Action::EndTask(key) => self.ask_or_end(key),
            Action::EndTree(key) => self.ask_end_tree(key),
            Action::Suspend(key) => {
                let name = self.name_of(key);
                self.report(control::suspend(key), &format!("Suspended {name}"));
            }
            Action::Resume(key) => {
                let name = self.name_of(key);
                self.report(control::resume(key), &format!("Resumed {name}"));
            }
            Action::SetPriority(key, priority) => self.ask_or_set_priority(key, priority),
            Action::Reveal(key) => self.reveal(key),
            Action::Copy(key) => self.copy_details(key, ui),
            Action::StartService(name) => {
                let result = crate::win::services::start(&name);
                self.report(result, &format!("Starting {name}"));
                self.services.refreshed = None;
            }
            Action::StopService(name) => {
                let label = self.service_label(&name);
                self.pending = Some(Pending::StopService(name, label));
            }
        }
    }

    /// Answers a pending confirmation.
    pub fn resolve(&mut self, confirmed: bool) {
        let Some(pending) = self.pending.take() else {
            return;
        };
        if !confirmed {
            return;
        }
        match pending {
            Pending::EndTask(key, name) => {
                self.report(control::end(key), &format!("Ended {name}"));
            }
            Pending::EndTree(key, name, _) => self.end_tree(key, &name),
            Pending::Realtime(key, name) => {
                let result = control::set_priority(key, Priority::Realtime);
                self.report(result, &format!("{name} set to realtime priority"));
            }
            Pending::StopService(name, label) => {
                let result = crate::win::services::stop(&name);
                self.report(result, &format!("Stopping {label}"));
                self.services.refreshed = None;
            }
        }
    }

    /// Ends a process, asking first unless the user has turned that off.
    fn ask_or_end(&mut self, key: ProcessKey) {
        let name = self.name_of(key);
        if self.config.confirm_end_task.unwrap_or(true) {
            self.pending = Some(Pending::EndTask(key, name));
            return;
        }
        self.report(control::end(key), &format!("Ended {name}"));
    }

    /// Asks before ending a subtree, and says how large it is.
    ///
    /// The count is the point. "End chrome.exe and its 34 child
    /// processes?" is a different question from "End chrome.exe?", and
    /// the user cannot see the whole subtree — it is collapsed, which is
    /// usually why they reached for this.
    fn ask_end_tree(&mut self, key: ProcessKey) {
        let name = self.name_of(key);
        let count = self.subtree_size(key);
        self.pending = Some(Pending::EndTree(key, name, count));
    }

    /// Ends a process and everything beneath it.
    ///
    /// **Children first, parent last.** A parent ended first can have its
    /// children re-parented by the OS before they are reached, and a
    /// supervisor process that is ended first often restarts the very
    /// children this is trying to end — so the obvious order does the
    /// opposite of what was asked.
    ///
    /// The order comes from the forest's own depth: the subtree is
    /// collected, then walked deepest-first.
    fn end_tree(&mut self, key: ProcessKey, name: &str) {
        let Some(snapshot) = self.snapshot.as_ref() else {
            return;
        };
        let forest = self.active_rows().forest();
        let Some(index) = snapshot.processes.iter().position(|row| row.key() == key) else {
            self.notify("That process is no longer running", true);
            return;
        };

        // Deepest first. `subtree` returns indices; the forest knows each
        // one's depth.
        let mut branch = forest.subtree(index);
        branch.sort_by_key(|node| std::cmp::Reverse(forest.depth_of(*node)));

        let keys: Vec<ProcessKey> = branch
            .into_iter()
            .filter_map(|node| snapshot.processes.get(node).map(ProcessRow::key))
            .collect();

        let total = keys.len();
        let mut ended = 0usize;
        let mut last_error = None;
        for target in keys {
            match control::end(target) {
                Ok(()) => ended += 1,
                // A process that exited on its own between the snapshot
                // and the click is not a failure — it is the outcome that
                // was asked for.
                Err(control::ActionError::Gone) => ended += 1,
                Err(error) => last_error = Some(error),
            }
        }

        match last_error {
            None => self.notify(
                format!("Ended {name} and {} more", total.saturating_sub(1)),
                false,
            ),
            Some(error) => {
                self.notify(format!("Ended {ended} of {total} in {name}: {error}"), true)
            }
        }
    }

    /// Sets a priority class, asking first only for realtime.
    fn ask_or_set_priority(&mut self, key: ProcessKey, priority: Priority) {
        let name = self.name_of(key);
        if priority.is_dangerous() {
            self.pending = Some(Pending::Realtime(key, name));
            return;
        }
        let result = control::set_priority(key, priority);
        self.report(result, &format!("{name} set to {}", priority.label()));
    }

    /// Opens the selected process's folder with its executable selected.
    fn reveal(&mut self, key: ProcessKey) {
        let Some(path) = self.path_of(key) else {
            self.notify("That process's image path could not be read", true);
            return;
        };
        let result = control::reveal_in_explorer(&path);
        self.report(result, "Opened file location");
    }

    /// Copies the selected process's details as text.
    ///
    /// Tab-separated, so it pastes into a spreadsheet as columns and into
    /// a chat window as something readable. The header line is included
    /// because a bare row of numbers pasted into an issue is unreadable
    /// without one.
    fn copy_details(&mut self, key: ProcessKey, ui: &egui::Ui) {
        let Some(row) = self
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.processes.iter().find(|row| row.key() == key))
        else {
            self.notify("That process is no longer running", true);
            return;
        };

        let text = format!(
            "Name\tPID\tStatus\tCPU\tMemory\tPrivate\tThreads\tHandles\tUser\tPath\n\
             {}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            row.display_name(),
            row.pid,
            row.status.label(),
            crate::format::percent(row.cpu_percent),
            crate::format::bytes(row.working_set),
            crate::format::bytes(row.private_bytes),
            row.thread_count,
            row.handle_count,
            row.user,
            row.path
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned())
                .unwrap_or_default(),
        );
        ui.ctx().copy_text(text);
        self.notify("Copied to clipboard", false);
    }

    /// Turns an action's result into a status message.
    fn report(&mut self, result: control::Action, success: &str) {
        match result {
            Ok(()) => self.notify(success.to_string(), false),
            Err(error) => self.notify(error.to_string(), true),
        }
    }

    /// The display name of a process, for a message.
    ///
    /// Falls back to the PID rather than to nothing: a confirmation
    /// reading "End ?" is worse than one reading "End PID 4242".
    fn name_of(&self, key: ProcessKey) -> String {
        self.snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.processes.iter().find(|row| row.key() == key))
            .map(|row| row.display_name().to_string())
            .unwrap_or_else(|| format!("PID {}", key.pid))
    }

    /// A service's display name, for a message.
    fn service_label(&self, name: &str) -> String {
        self.services
            .services
            .iter()
            .find(|service| service.name == name)
            .map(|service| service.display_name.clone())
            .unwrap_or_else(|| name.to_string())
    }

    /// The image path of a process.
    fn path_of(&self, key: ProcessKey) -> Option<std::path::PathBuf> {
        self.snapshot
            .as_ref()?
            .processes
            .iter()
            .find(|row| row.key() == key)?
            .path
            .clone()
    }

    /// How many processes are in a subtree, including its root.
    fn subtree_size(&self, key: ProcessKey) -> usize {
        let Some(snapshot) = self.snapshot.as_ref() else {
            return 1;
        };
        let forest = self.active_rows().forest();
        snapshot
            .processes
            .iter()
            .position(|row| row.key() == key)
            .map_or(1, |index| forest.subtree(index).len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::model::Snapshot;

    fn app_with(rows: Vec<ProcessRow>) -> App {
        let mut app = App::new(Config::default());
        app.snapshot = Some(std::sync::Arc::new(Snapshot {
            sequence: 1,
            processes: rows,
            ..Snapshot::default()
        }));
        app
    }

    fn row(pid: u32, name: &str) -> ProcessRow {
        ProcessRow {
            pid,
            started_at: u64::from(pid) + 1,
            name: name.to_string(),
            ..ProcessRow::default()
        }
    }

    #[test]
    fn ending_a_task_asks_first_by_default() {
        // The confirmation exists because a mistaken click is not
        // recoverable: whatever the process had not saved is gone.
        let mut app = app_with(vec![row(100, "editor.exe")]);
        let key = ProcessKey {
            pid: 100,
            started_at: 101,
        };
        app.ask_or_end(key);
        assert!(
            matches!(app.pending, Some(Pending::EndTask(..))),
            "End task must ask before acting"
        );
    }

    #[test]
    fn the_confirmation_can_be_turned_off_but_never_is_by_default() {
        let mut app = app_with(vec![row(100, "editor.exe")]);
        assert!(
            app.config.confirm_end_task.unwrap_or(true),
            "the default must be to ask"
        );

        app.config.confirm_end_task = Some(false);
        // With confirmation off this attempts the kill immediately. The
        // target does not exist, so it reports a failure rather than
        // ending anything — which is what is being checked: that it did
        // not stop to ask.
        app.ask_or_end(ProcessKey {
            pid: 0xffff_fffe,
            started_at: 1,
        });
        assert!(app.pending.is_none(), "it should not have asked");
        assert!(app.toast.is_some(), "and should have reported the outcome");
    }

    #[test]
    fn realtime_priority_asks_and_the_others_do_not() {
        // Realtime can make a machine stop answering the keyboard, with
        // no way back short of the power button. Every other class is
        // reversible by the control that set it.
        let mut app = app_with(vec![row(100, "a.exe")]);
        let key = ProcessKey {
            pid: 100,
            started_at: 101,
        };

        app.ask_or_set_priority(key, Priority::Realtime);
        assert!(matches!(app.pending, Some(Pending::Realtime(..))));

        app.pending = None;
        for priority in [
            Priority::Idle,
            Priority::BelowNormal,
            Priority::Normal,
            Priority::AboveNormal,
            Priority::High,
        ] {
            app.ask_or_set_priority(key, priority);
            assert!(
                app.pending.is_none(),
                "{priority:?} is reversible and must not ask"
            );
        }
    }

    #[test]
    fn suspending_and_resuming_never_ask() {
        // Immediately reversible by the control that set them. An app
        // that asks about everything trains people to click through the
        // one dialog that mattered.
        let mut app = app_with(vec![row(100, "a.exe")]);
        let key = ProcessKey {
            pid: 0xffff_fffe,
            started_at: 1,
        };
        let ctx = egui::Context::default();
        let mut output = ctx.run_ui(Default::default(), |ui| {
            app.dispatch(Action::Suspend(key), ui);
            assert!(app.pending.is_none());
            app.dispatch(Action::Resume(key), ui);
            assert!(app.pending.is_none());
        });
        // Nothing in this test paints, so there is no renderer to hand
        // these to. epaint's own `Drop` panics on an unhandled delta as a
        // safety net against a real app that forgets to upload a new
        // texture — clearing it here is the documented way to say "this
        // one was never going anywhere" instead.
        output.textures_delta.clear();
    }

    #[test]
    fn declining_a_confirmation_does_nothing_at_all() {
        let mut app = app_with(vec![row(100, "a.exe")]);
        app.pending = Some(Pending::EndTask(
            ProcessKey {
                pid: 0xffff_fffe,
                started_at: 1,
            },
            "a.exe".to_string(),
        ));
        app.resolve(false);
        assert!(app.pending.is_none(), "the prompt closes");
        assert!(
            app.toast.is_none(),
            "and nothing was attempted, so nothing is reported"
        );
    }

    #[test]
    fn a_name_falls_back_to_the_pid_rather_than_to_nothing() {
        // A confirmation reading "End ?" is worse than one reading
        // "End PID 4242".
        let app = app_with(Vec::new());
        let name = app.name_of(ProcessKey {
            pid: 4242,
            started_at: 1,
        });
        assert_eq!(name, "PID 4242");
    }

    #[test]
    fn a_name_uses_the_processs_description_when_it_has_one() {
        let mut process = row(100, "chrome.exe");
        process.description = "Google Chrome".to_string();
        let app = app_with(vec![process]);
        assert_eq!(
            app.name_of(ProcessKey {
                pid: 100,
                started_at: 101
            }),
            "Google Chrome"
        );
    }

    #[test]
    fn revealing_a_process_with_no_readable_path_says_so() {
        let mut app = app_with(vec![row(100, "protected.exe")]);
        app.reveal(ProcessKey {
            pid: 100,
            started_at: 101,
        });
        assert!(
            app.toast.as_ref().is_some_and(|toast| toast.failed),
            "an unreadable path is reported rather than silently ignored"
        );
    }

    #[test]
    fn every_action_variant_is_dispatchable() {
        // A guard on the match in `dispatch`: a variant added later
        // without a branch would fail to compile, but one added with a
        // branch that does nothing would not — this at least exercises
        // each.
        let mut app = app_with(vec![row(100, "a.exe")]);
        let key = ProcessKey {
            pid: 0xffff_fffe,
            started_at: 1,
        };
        let ctx = egui::Context::default();
        let mut output = ctx.run_ui(Default::default(), |ui| {
            for action in [
                Action::EndTask(key),
                Action::EndTree(key),
                Action::Suspend(key),
                Action::Resume(key),
                Action::SetPriority(key, Priority::Normal),
                Action::Reveal(key),
                Action::Copy(key),
                Action::StartService("NoSuchService".to_string()),
                Action::StopService("NoSuchService".to_string()),
            ] {
                app.pending = None;
                app.dispatch(action, ui);
            }
        });
        // See the matching comment in `suspending_and_resuming_never_ask`.
        output.textures_delta.clear();
    }
}

// ============================================================================
// Module:       gui::ui::modal
// Description:  The one confirmation card, and the scrim behind it.
//
// Dependencies: egui; super::{theme, widgets}, crate::gui::app::Pending
// ============================================================================

//! Confirmations.
//!
//! **There is one modal, not several.** [`crate::gui::app::App::pending`]
//! selects which question a single card asks, and there is no
//! `egui::Window` anywhere in this app. A window per confirmation is how
//! an app ends up with six dialogs of six different widths, none of them
//! aligned, three of them movable for no reason, and one that can be left
//! open behind the main window.
//!
//! ## What a confirmation has to say
//!
//! Three things, and the third is the one usually missing:
//!
//! 1. **What will happen**, in the words of the action — "End task", not
//!    "Are you sure?".
//! 2. **What it will happen to**, named the same way the row named it. A
//!    dialog saying "End Google Chrome?" over a row labelled `chrome.exe`
//!    is how someone ends the wrong thing.
//! 3. **What it costs**, when that is not obvious. Ending a task loses
//!    unsaved work; ending a tree does it to a subtree the user cannot
//!    see the whole of, so the count is stated.
//!
//! ## Escape cancels, Enter confirms
//!
//! Both, because a dialog that can only be dismissed with the mouse is a
//! dialog that interrupts typing. Enter is bound to the *confirming*
//! button only where the action is the expected one — see
//! [`Confirmation::default_is_confirm`].

use super::motion;
use super::theme::{self, SPACE_LG, SPACE_MD, SPACE_SM};
use super::widgets;
use crate::gui::app::{App, Pending};
use egui::{Align2, CornerRadius, Key, Sense, Ui, Vec2};

/// The modal card's width.
///
/// Wide enough for a sentence naming a process without wrapping mid-name;
/// narrow enough that it reads as a dialog rather than as a page.
const CARD_WIDTH: f32 = 420.0;

/// The scrim's opacity over the window behind.
///
/// Enough to push the window back without hiding it — the process the
/// question is about is usually still visible behind, and being able to
/// check it is the point.
const SCRIM_ALPHA: u8 = 160;

/// What a pending confirmation asks.
pub struct Confirmation {
    /// The heading: what will happen.
    pub title: String,
    /// The body: what it will happen to, and what it costs.
    pub body: String,
    /// The confirming button's label.
    pub confirm: String,
    /// Whether the confirming action is destructive, which decides the
    /// button's colour.
    pub destructive: bool,
}

impl Confirmation {
    /// The question a pending action asks.
    #[must_use]
    pub fn of(pending: &Pending) -> Self {
        match pending {
            Pending::EndTask(_, name) => Self {
                title: format!("End {name}?"),
                body: "Unsaved work in this process will be lost.".to_string(),
                confirm: "End task".to_string(),
                destructive: true,
            },
            Pending::EndTree(_, name, count) => Self {
                title: format!("End {name} and everything it started?"),
                // The count is the whole point: the subtree is usually
                // collapsed, which is why the user reached for this, and
                // they cannot see how large it is.
                body: format!(
                    "{count} processes will be ended, children first. \
                     Unsaved work in any of them will be lost."
                ),
                confirm: "End tree".to_string(),
                destructive: true,
            },
            Pending::Realtime(_, name) => Self {
                title: format!("Set {name} to realtime priority?"),
                body: "Realtime schedules a process above most of the \
                       kernel's own threads, including the ones handling \
                       input. A busy process at this priority can stop the \
                       machine responding to the keyboard."
                    .to_string(),
                confirm: "Set realtime".to_string(),
                destructive: true,
            },
            Pending::StopService(_, label) => Self {
                title: format!("Stop {label}?"),
                body: "Anything depending on this service will stop working \
                       until it is started again."
                    .to_string(),
                confirm: "Stop service".to_string(),
                destructive: true,
            },
        }
    }

    /// Whether Enter should confirm rather than cancel.
    ///
    /// It should not, for any of these. Every confirmation in this app
    /// guards something irreversible, and binding Enter to the
    /// destructive button means a stray keystroke while the card is
    /// opening does the thing. Cancel is the safe default, and the
    /// confirming button is reached deliberately — with the mouse or with
    /// Tab.
    #[must_use]
    pub fn default_is_confirm(&self) -> bool {
        !self.destructive
    }
}

/// How far the card rises as it arrives.
///
/// Small, and shared with [`super::enter_view`]'s own `RISE`: this is the
/// one other surface that fades and rises into place rather than
/// switching, and a card that used a different distance or a different
/// curve would read as a second animation style living in the same app.
const RISE: f32 = 6.0;

/// Draws the modal, if one is pending. Returns the answer, if given.
pub fn draw(app: &mut App, ui: &mut Ui) -> Option<bool> {
    let pending = app.pending.as_ref()?;
    let question = Confirmation::of(pending);
    let theme = app.theme.clone();

    // Keyed on a fixed id rather than the question's own content: only
    // one modal is ever open at a time, and the point is that opening a
    // *different* confirmation while one is already up — Stop, then
    // immediately End task on the row behind it — does not restart the
    // fade from zero.
    let progress = motion::transition(ui.ctx(), egui::Id::new("modal-card"), true, motion::QUICK);

    let screen = ui.ctx().content_rect();
    // Painted on a foreground layer so it covers every panel, including
    // ones drawn after this call.
    let painter = ui.ctx().layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new("modal"),
    ));
    let scrim_alpha = (f32::from(SCRIM_ALPHA) * progress) as u8;
    painter.rect_filled(
        screen,
        CornerRadius::ZERO,
        theme::translucent(theme.app, scrim_alpha),
    );

    let mut answer = None;
    // An `Area` rather than a `Window`: no title bar, no move handle, no
    // close button, and it cannot be left open behind the main window.
    // See the module docs.
    egui::Area::new(egui::Id::new("modal-card"))
        .order(egui::Order::Foreground)
        .anchor(
            Align2::CENTER_CENTER,
            Vec2::new(0.0, RISE * (1.0 - progress)),
        )
        .show(ui.ctx(), |ui| {
            ui.set_opacity(progress);
            egui::Frame::new()
                .fill(theme::rgb(theme.raised))
                .stroke(egui::Stroke::new(1.0, theme::rgb(theme.border)))
                .corner_radius(CornerRadius::same(theme::RADIUS_LG))
                .inner_margin(theme::margin(SPACE_LG))
                .shadow(egui::epaint::Shadow {
                    offset: [0, 8],
                    blur: 24,
                    spread: 0,
                    // Alpha-only: a shadow is lighting, not a theme
                    // colour. See `gui::ui::theme`.
                    color: egui::Color32::from_black_alpha(96),
                })
                .show(ui, |ui| {
                    ui.set_width(CARD_WIDTH);
                    ui.label(
                        egui::RichText::new(&question.title)
                            .color(theme::rgb(theme.text))
                            .size(16.0)
                            .strong(),
                    );
                    ui.add_space(SPACE_SM);
                    ui.label(
                        egui::RichText::new(&question.body)
                            .color(theme::rgb(theme.text_muted))
                            .text_style(egui::TextStyle::Small),
                    );
                    ui.add_space(SPACE_LG);

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // The destructive button is on the right,
                        // where the eye ends up, but Cancel is
                        // adjacent to it rather than across the card
                        // — a gap between them is a gap the pointer
                        // travels through on the way to the wrong one.
                        let confirmed = if question.destructive {
                            widgets::danger_button(ui, &theme, &question.confirm, true)
                        } else {
                            widgets::primary_button(ui, &theme, &question.confirm)
                        };
                        if confirmed.clicked() {
                            answer = Some(true);
                        }
                        ui.add_space(SPACE_SM);
                        let cancel = ui.button("Cancel");
                        if cancel.clicked() {
                            answer = Some(false);
                        }
                    });
                });
        });

    // Escape always cancels. Enter confirms only where the action is not
    // destructive, which for now is never — see
    // `Confirmation::default_is_confirm`.
    ui.ctx().input(|input| {
        if input.key_pressed(Key::Escape) {
            answer = Some(false);
        }
        if input.key_pressed(Key::Enter) {
            answer = Some(question.default_is_confirm());
        }
    });

    // The scrim swallows clicks so a click that misses the card does not
    // reach the table behind it and change the selection the question is
    // about.
    let blocker = ui.interact(screen, egui::Id::new("modal-scrim"), Sense::click());
    let _ = blocker;

    answer
}

/// The gap between a modal's body and its buttons.
pub const BUTTON_GAP: f32 = SPACE_MD;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ProcessKey;

    fn key() -> ProcessKey {
        ProcessKey {
            pid: 4242,
            started_at: 1,
        }
    }

    #[test]
    fn every_confirmation_names_what_it_will_act_on() {
        // A dialog saying "End Google Chrome?" over a row labelled
        // `chrome.exe` is how someone ends the wrong thing — so the name
        // in the question is the one the row used.
        let cases = [
            Pending::EndTask(key(), "Google Chrome".to_string()),
            Pending::EndTree(key(), "Google Chrome".to_string(), 12),
            Pending::Realtime(key(), "Google Chrome".to_string()),
            Pending::StopService("Spooler".to_string(), "Print Spooler".to_string()),
        ];
        for pending in &cases {
            let question = Confirmation::of(pending);
            assert!(
                question.title.contains("Chrome") || question.title.contains("Spooler"),
                "the question does not name its target: {}",
                question.title
            );
            assert!(!question.body.is_empty(), "and must say what it costs");
            assert!(!question.confirm.is_empty());
        }
    }

    #[test]
    fn ending_a_tree_states_how_many_processes_that_is() {
        // The subtree is usually collapsed — which is why the user
        // reached for this — so they cannot see how large it is.
        let question = Confirmation::of(&Pending::EndTree(key(), "chrome.exe".to_string(), 35));
        assert!(
            question.body.contains("35"),
            "the count is the whole point: {}",
            question.body
        );
    }

    #[test]
    fn the_confirming_button_says_what_it_does() {
        // "OK" over a destructive action is how the action gets taken by
        // someone who was reading the heading.
        for pending in [
            Pending::EndTask(key(), "a".to_string()),
            Pending::EndTree(key(), "a".to_string(), 2),
            Pending::Realtime(key(), "a".to_string()),
            Pending::StopService("a".to_string(), "A".to_string()),
        ] {
            let question = Confirmation::of(&pending);
            let label = question.confirm.to_lowercase();
            assert!(
                label != "ok" && label != "yes",
                "the button should name the action, got {}",
                question.confirm
            );
        }
    }

    #[test]
    fn enter_never_confirms_a_destructive_action() {
        // Every confirmation in this app guards something irreversible.
        // Binding Enter to the destructive button means a stray keystroke
        // while the card is opening does the thing.
        for pending in [
            Pending::EndTask(key(), "a".to_string()),
            Pending::EndTree(key(), "a".to_string(), 2),
            Pending::Realtime(key(), "a".to_string()),
            Pending::StopService("a".to_string(), "A".to_string()),
        ] {
            let question = Confirmation::of(&pending);
            assert!(question.destructive);
            assert!(
                !question.default_is_confirm(),
                "Enter must not confirm: {}",
                question.title
            );
        }
    }

    #[test]
    fn the_realtime_warning_says_what_actually_happens() {
        // "Are you sure?" tells someone nothing they can decide with.
        let question = Confirmation::of(&Pending::Realtime(key(), "a.exe".to_string()));
        assert!(
            question.body.to_lowercase().contains("keyboard"),
            "the warning should name the consequence: {}",
            question.body
        );
    }
}

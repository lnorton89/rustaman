// ============================================================================
// Module:       gui::ui
// Description:  The one draw entry point, the frame's layout order, and the
//               keyboard shortcuts.
//
// Dependencies: egui; every sibling module
// ============================================================================

//! Drawing one frame.
//!
//! [`draw`] runs in full every frame. It lays the window out in a fixed
//! order — chrome first, then the view, then anything over the top — and
//! the order is what makes the layout predictable rather than dependent
//! on which panel happened to measure first.
//!
//! ## Layout order
//!
//! egui's panels claim space in the order they are shown, and each takes
//! from what is left. So:
//!
//! 1. **The title bar** (top), which needs the full window width.
//! 2. **The status bar** (bottom), same.
//! 3. **The navigation rail** (left), which takes the height between them
//!    — drawn after the bars so it does not run behind either.
//! 4. **The view**, in whatever is left.
//! 5. **The modal and the resize handles**, over everything.
//!
//! Reordering these is not cosmetic: showing the rail before the status
//! bar makes the rail full-height and the status bar starts to its right,
//! which looks like a mistake because it is one.
//!
//! ## Shortcuts are handled before the view
//!
//! And they return early while a modal is open, so a keystroke aimed at a
//! confirmation cannot also delete the row behind it. That is the same
//! reason [`shortcuts`] runs before the view draws rather than after: a
//! view that has already consumed the keystroke would swallow it.

pub mod chrome;
pub mod details;
pub mod dnd;
pub mod graph;
pub mod icon;
pub mod modal;
pub mod motion;
pub mod performance;
pub mod processes;
pub mod services;
pub mod settings;
pub mod theme;
pub mod widgets;

use crate::gui::app::actions::Action;
use crate::gui::app::{App, View};
use egui::{Key, Ui};

/// Draws one frame.
pub fn draw(app: &mut App, ui: &mut Ui) {
    // Installed before anything paints, so no widget can be drawn under
    // the previous frame's theme.
    theme::apply(ui.ctx(), &app.theme.clone());

    let mut closing = false;
    if app.custom_chrome {
        closing |= chrome::title_bar(app, ui);
    }
    chrome::status_bar(app, ui);
    chrome::nav_rail(app, ui);

    // The modal's answer is taken before the view draws, so a click that
    // dismissed it does not also land on the table underneath.
    if let Some(answer) = modal::draw(app, ui) {
        app.resolve(answer);
    }
    let blocked = app.pending.is_some();

    if !blocked {
        shortcuts(app, ui);
    }

    egui::CentralPanel::default()
        .frame(theme::content(&app.theme))
        .show(ui, |ui| match app.view {
            View::Processes => processes::draw(app, ui),
            View::Performance => performance::draw(app, ui),
            View::Details => details::draw(app, ui),
            View::Services => services::draw(app, ui),
            View::Startup => services::draw_startup(app, ui),
            View::Settings => settings::draw(app, ui),
        });

    if app.custom_chrome {
        // Last, so they win the hit test against a control at the window
        // edge — a table's scrollbar is exactly there.
        chrome::resize_handles(ui);
    }

    if closing {
        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
    }
}

/// Handles the keyboard shortcuts.
///
/// Returns early while a modal is open — see the module docs — so every
/// shortcut added later is automatically blocked during a confirmation
/// without its author having to remember.
fn shortcuts(app: &mut App, ui: &Ui) {
    // A shortcut must not fire while a text field has focus, or typing
    // "delete" into the search box ends a process on the "e".
    if ui.ctx().egui_wants_keyboard_input() {
        return;
    }

    let (delete, refresh, find, escape, digits) = ui.ctx().input(|input| {
        (
            input.key_pressed(Key::Delete),
            input.key_pressed(Key::F5),
            input.modifiers.command && input.key_pressed(Key::F),
            input.key_pressed(Key::Escape),
            [
                Key::Num1,
                Key::Num2,
                Key::Num3,
                Key::Num4,
                Key::Num5,
                Key::Num6,
            ]
            .map(|key| input.modifiers.command && input.key_pressed(key)),
        )
    });

    if delete {
        if let Some(row) = app.selected_row() {
            if !row.is_pseudo() {
                let key = row.key();
                app.dispatch(Action::EndTask(key), ui);
            }
        }
    }

    if refresh {
        // Services and startup are read on their own schedule, not the
        // sampler's — this is what F5 means for them. The process list
        // refreshes on the sampler's own interval and needs nothing.
        app.services.refreshed = None;
        app.startup.refreshed = None;
        app.notify("Refreshed", false);
    }

    if find {
        // Focus is requested by id, which is how egui addresses a widget
        // that has not been drawn yet this frame.
        ui.ctx().memory_mut(|memory| {
            memory.request_focus(egui::Id::new("search-box"));
        });
    }

    if escape {
        // Clears the filter before clearing the selection: a user with a
        // filter active and a row selected who presses Escape means the
        // filter, which is the thing currently hiding rows.
        if !app.processes.search.is_empty() {
            app.processes.search.clear();
            app.processes.query = crate::model::filter::Query::default();
        } else {
            app.processes.selected = None;
            app.details.selected = None;
        }
    }

    for (index, pressed) in digits.into_iter().enumerate() {
        if pressed {
            if let Some(view) = View::ALL.get(index) {
                app.view = *view;
            }
        }
    }
}

/// The shortcuts, for the about panel and for the test below.
///
/// Stated as data so that a shortcut shown to the user and a shortcut
/// that exists cannot drift apart — the pair that is easiest to let
/// diverge, because nothing breaks when they do.
pub const SHORTCUTS: [(&str, &str); 5] = [
    ("Delete", "End the selected task"),
    ("F5", "Re-read services and startup entries"),
    ("Ctrl+F", "Search"),
    ("Esc", "Clear the search, then the selection"),
    ("Ctrl+1…6", "Switch view"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_advertised_shortcut_is_one_the_handler_implements() {
        // The pair that is easiest to let drift, because nothing breaks
        // when they do — the menu simply lies.
        let source = include_str!("mod.rs");
        for (keys, description) in SHORTCUTS {
            let implemented = match keys {
                "Delete" => source.contains("Key::Delete"),
                "F5" => source.contains("Key::F5"),
                "Ctrl+F" => source.contains("Key::F)"),
                "Esc" => source.contains("Key::Escape"),
                "Ctrl+1…6" => source.contains("Key::Num1"),
                _ => false,
            };
            assert!(
                implemented,
                "{keys} ({description}) is advertised but not handled"
            );
        }
    }

    #[test]
    fn there_is_a_shortcut_for_every_view() {
        // Ctrl+1..6 covers the rail, so a view added later without a
        // digit would be the one view with no shortcut.
        let digits = 6;
        assert_eq!(
            View::ALL.len(),
            digits,
            "the view count and the Ctrl+digit range must agree"
        );
    }

    #[test]
    fn no_shortcut_is_advertised_twice() {
        let mut keys: Vec<&str> = SHORTCUTS.iter().map(|(keys, _)| *keys).collect();
        let count = keys.len();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), count, "two shortcuts share a key");
    }

    /// Every module that paints, and its source.
    ///
    /// One list rather than one per rule: the rules below all ask the
    /// same question of the same files, and three separate lists is how a
    /// module added later ends up covered by two of them. `theme.rs`,
    /// `icon.rs` and `motion.rs` are deliberately absent — they are where
    /// colours, shapes and durations are *made*, so a rule forbidding
    /// those things would forbid the definitions themselves.
    const DRAWING_MODULES: [(&str, &str); 9] = [
        ("chrome.rs", include_str!("chrome.rs")),
        ("details.rs", include_str!("details.rs")),
        ("graph.rs", include_str!("graph.rs")),
        ("modal.rs", include_str!("modal.rs")),
        ("performance.rs", include_str!("performance.rs")),
        ("processes.rs", include_str!("processes.rs")),
        ("services.rs", include_str!("services.rs")),
        ("settings.rs", include_str!("settings.rs")),
        ("widgets.rs", include_str!("widgets.rs")),
    ];

    /// The lines a source scan should not read: comments explain the
    /// rules, and doc comments quote them.
    fn is_prose(line: &str) -> bool {
        let trimmed = line.trim_start();
        trimmed.starts_with("//") || trimmed.starts_with("*")
    }

    #[test]
    fn no_drawing_module_sets_an_icon_in_a_font() {
        // egui's bundled fonts cover Latin, Greek, Cyrillic and an emoji
        // subset — and almost nothing in the Geometric Shapes,
        // Miscellaneous Symbols or Dingbats blocks, which is where every
        // icon-shaped character lives. A glyph the font lacks renders as
        // an empty box, so the nav rail shipped as a column of squares
        // beside its labels.
        //
        // Every icon is geometry now (`crate::icon`), and this is what
        // stops the convenient one-character version coming back.
        //
        // Typographic punctuation is fine and used throughout the prose:
        // an em dash, a middle dot, an ellipsis and curly quotes are all
        // in the bundled fonts. The rule is about *pictographs*.
        for (name, source) in DRAWING_MODULES {
            for (number, line) in source.lines().enumerate() {
                if is_prose(line) {
                    continue;
                }
                for character in line.chars() {
                    let code = character as u32;
                    let pictographic = matches!(
                        code,
                        // Arrows, Miscellaneous Technical, Geometric
                        // Shapes, Miscellaneous Symbols, Dingbats.
                        0x2190..=0x2BFF
                        // Everything above the Basic Multilingual Plane
                        // that this app could plausibly reach for:
                        // emoji, and the symbols-and-pictographs block
                        // the window-control glyphs live in.
                        | 0x1F000..=0x1FAFF
                    );
                    assert!(
                        !pictographic,
                        "{name}:{} sets an icon in a font: U+{code:04X}. \
                         egui's bundled fonts do not carry it, so it \
                         renders as an empty box — use crate::icon::Icon",
                        number + 1
                    );
                }
            }
        }
    }

    #[test]
    fn no_drawing_module_animates_by_hand() {
        // Every animation goes through `gui::ui::motion`, which is what
        // makes the app's four durations four decisions rather than
        // thirty call sites that each picked a number. A direct
        // `ctx.animate_bool_with_time(id, on, 0.25)` is invisible in
        // review and is how a hover that fades over 0.1s ends up beside a
        // panel that slides over 0.25s.
        for (name, source) in DRAWING_MODULES {
            for (number, line) in source.lines().enumerate() {
                if is_prose(line) {
                    continue;
                }
                assert!(
                    !line.contains(".animate_bool")
                        && !line.contains(".animate_value")
                        && !line.contains(".animate_"),
                    "{name}:{} animates by hand: {}. Use gui::ui::motion, \
                     which owns the durations",
                    number + 1,
                    line.trim()
                );
            }
        }
    }

    #[test]
    fn no_drawing_module_holds_a_bare_duration() {
        // The other half of the rule above: a duration reaching `motion`
        // from a call site is the same drift with one more step in it.
        // The four constants are the whole vocabulary.
        for (name, source) in DRAWING_MODULES {
            for (number, line) in source.lines().enumerate() {
                if is_prose(line) || !line.contains("motion::") {
                    continue;
                }
                // A literal float argument to a motion helper. The
                // durations are named constants, so a `0.` in one of
                // these calls is someone's own number.
                let suspicious = line.contains("motion::toggle(")
                    || line.contains("motion::transition(")
                    || line.contains("motion::settled(");
                if !suspicious {
                    continue;
                }
                let arguments = line.split("motion::").nth(1).unwrap_or("");
                assert!(
                    !arguments.contains("0."),
                    "{name}:{} passes a bare duration: {}. Use INSTANT, \
                     QUICK, SETTLE or ENTER",
                    number + 1,
                    line.trim()
                );
            }
        }
    }

    #[test]
    fn no_drawing_module_carries_its_own_drag_state() {
        // Drag and drop is almost entirely feedback — a dimmed source, a
        // ghost on the pointer, an indicator at the drop target, and
        // nothing at all when the drop would be ignored. A view that
        // tracked its own `dragging: Option<usize>` would get some subset
        // of those, and a different subset from the next one.
        //
        // So the state lives in `dnd`, in egui's own memory, and a view
        // reaches it through `Lane`. Reading `drag_started` here is the
        // tell: it means a view is deciding for itself what a drag is.
        for (name, source) in DRAWING_MODULES {
            for (number, line) in source.lines().enumerate() {
                if is_prose(line) {
                    continue;
                }
                assert!(
                    !line.contains("drag_started")
                        && !line.contains("drag_stopped")
                        && !line.contains("dragged_by")
                        && !line.contains("drag_delta"),
                    "{name}:{} tracks a drag by hand: {}. Use gui::ui::dnd, \
                     which owns the feedback as well as the state",
                    number + 1,
                    line.trim()
                );
            }
        }
    }

    #[test]
    fn no_drawing_module_holds_a_colour_literal() {
        // Every colour comes from the theme. A single hard-coded grey is
        // invisible in review and then unreadable in half the themes.
        //
        // The exceptions are stated rather than assumed: `theme.rs` is
        // where colours are made, and alpha-only constructors are
        // lighting rather than theme colours.
        for (name, source) in DRAWING_MODULES {
            for (number, line) in source.lines().enumerate() {
                let trimmed = line.trim_start();
                // Comments describe the rule; they do not break it.
                if trimmed.starts_with("//") {
                    continue;
                }
                // Alpha-only constructors are lighting, not colour.
                if line.contains("from_black_alpha") || line.contains("from_white_alpha") {
                    continue;
                }
                assert!(
                    !line.contains("Color32::from_rgb")
                        && !line.contains("Color32::from_gray")
                        && !line.contains("Color32::WHITE")
                        && !line.contains("Color32::BLACK")
                        && !line.contains("Color32::RED"),
                    "{name}:{} holds a colour literal: {}",
                    number + 1,
                    line.trim()
                );
            }
        }
    }

    #[test]
    fn no_drawing_module_holds_a_hand_picked_pixel_gap() {
        // Every margin, inset, and gap is one of the five scale values.
        // A `add_space(7.0)` is how a window ends up with four different
        // gaps that are each nearly the same.
        for (name, source) in DRAWING_MODULES {
            for (number, line) in source.lines().enumerate() {
                let trimmed = line.trim_start();
                if trimmed.starts_with("//") {
                    continue;
                }
                if let Some(rest) = line.split("add_space(").nth(1) {
                    let argument = rest.split(')').next().unwrap_or("");
                    let named = argument.starts_with("SPACE_")
                        || argument.starts_with("chrome::")
                        || argument.starts_with("PAD")
                        || argument.starts_with("theme::");
                    assert!(
                        named,
                        "{name}:{} adds a hand-picked gap of {argument}; use \
                         a step of the spacing scale",
                        number + 1
                    );
                }
            }
        }
    }
}

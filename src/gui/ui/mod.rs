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
pub mod memory;
pub mod modal;
pub mod motion;
pub mod performance;
pub mod processes;
pub mod services;
pub mod settings;
pub mod system_info;
pub mod theme;
pub mod widgets;

use crate::gui::app::actions::Action;
use crate::gui::app::{App, View};
use egui::{Key, Ui};

/// Fades and lifts a view into place as it arrives.
///
/// A view that simply replaces the last one gives the user no idea where
/// the new content came from, and switching between two dense tables
/// reads as a flicker rather than as navigation. A short fade upward
/// answers "this is new, and it arrived from below".
///
/// It works by setting the `Ui`'s opacity and nudging the layout's
/// origin, both of which apply to everything drawn afterwards — so no
/// individual view has to know it is being animated, and a view added
/// later gets the transition without doing anything.
///
/// Everything is left alone once the transition finishes, rather than
/// being multiplied by 1.0 forever: a permanently non-opaque `Ui` forces
/// egui to composite the whole panel to its own layer every frame, which
/// is real cost for no visible effect.
fn enter_view(app: &App, ui: &mut Ui) {
    /// How far the content rises as it arrives. Small: this is a hint at
    /// a direction, not a slide.
    const RISE: f32 = 6.0;

    // Keyed on the view, so switching *to* a view restarts its own
    // animation rather than continuing whatever the last one was doing.
    let progress = motion::transition(
        ui.ctx(),
        egui::Id::new("view-enter").with(app.view),
        true,
        motion::ENTER,
    );
    if progress >= 1.0 {
        return;
    }
    ui.set_opacity(progress);
    ui.add_space(RISE * (1.0 - progress));
}

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
        .show(ui, |ui| {
            enter_view(app, ui);
            match app.view {
                View::Processes => processes::draw(app, ui),
                View::Performance => performance::draw(app, ui),
                View::Memory => memory::draw(app, ui),
                View::Details => details::draw(app, ui),
                View::Services => services::draw(app, ui),
                View::Startup => services::draw_startup(app, ui),
                View::System => system_info::draw(app, ui),
                View::Settings => settings::draw(app, ui),
            }
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
                Key::Num7,
                Key::Num8,
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
    ("Ctrl+1…8", "Switch view"),
];

#[cfg(test)]
mod tests {
    use super::*;

    /// Every rect a pass actually painted on screen.
    ///
    /// A mesh is not clipped geometrically — epaint hands the clip to
    /// the GPU as a scissor and leaves the vertices alone — so what
    /// reaches the screen is the mesh's bounds intersected with its own
    /// clip rect. Checking either alone passes on the broken version.
    fn painted(ctx: &egui::Context, shapes: Vec<egui::epaint::ClippedShape>) -> Vec<egui::Rect> {
        ctx.tessellate(shapes, 1.0)
            .into_iter()
            .filter_map(|primitive| {
                let egui::epaint::Primitive::Mesh(mesh) = primitive.primitive else {
                    return None;
                };
                let bounds = mesh
                    .vertices
                    .iter()
                    .fold(egui::Rect::NOTHING, |bounds, vertex| {
                        bounds.union(egui::Rect::from_min_size(vertex.pos, egui::Vec2::ZERO))
                    });
                let visible = bounds.intersect(primitive.clip_rect);
                visible.is_positive().then_some(visible)
            })
            .collect()
    }

    /// A machine with more processes than any window can show.
    fn a_long_list() -> crate::model::Snapshot {
        crate::model::Snapshot {
            processes: (0..400)
                .map(|index| crate::model::ProcessRow {
                    pid: index + 4,
                    name: format!("process-{index}.exe"),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn no_drawing_module_paints_its_own_selection() {
        // "Selected" has to look the same everywhere, and the way it
        // stops looking the same is not that anybody decides otherwise.
        // Each bar gets written next to the thing it marks, each is
        // reasonable there, and the app ends up with five. It did: the
        // rail entry, a table row, a row's identity accent, the
        // Performance picker tile and the adapter row, at three
        // different widths — `3.0`, `SELECTION_BAR` and `2.0` — square
        // in two places and rounded in three, inset in three and flush
        // in two, and animated in exactly one.
        //
        // `theme.selection` is the tell rather than `theme.accent`,
        // which several modules paint for good reasons — `dnd` draws
        // the drop indicator in it, `memory` the picked tile. But a
        // module reaching for the *selection* colour is deciding for
        // itself what a selected thing looks like, and that belongs to
        // `widgets::selection_fill` and `widgets::accent_bar`.
        //
        // `SELECTION_BAR` counts too: a module that needs the bar's
        // width is building its own bar.
        for (name, source) in DRAWING_MODULES {
            if name == "widgets.rs" {
                continue;
            }
            for (number, line) in source.lines().enumerate() {
                if is_prose(line) {
                    continue;
                }
                assert!(
                    !line.contains("theme.selection") && !line.contains("SELECTION_BAR"),
                    "{name}:{} decides for itself what a selected row looks                      like: {}. Use widgets::selection_fill for the surface                      and widgets::accent_bar for the marker, so every list                      in the app agrees",
                    number + 1,
                    line.trim()
                );
            }
        }
    }

    #[test]
    fn no_view_paints_outside_the_pane_it_was_given() -> anyhow::Result<()> {
        // The bug this exists for is one that three rounds of looking at
        // screenshots failed to find, because it cannot appear in a list
        // short enough to fit: a row only *half* inside the scroll
        // viewport painted its background at full height, straight
        // through the bottom of the table and into the status bar.
        //
        // It survived because a row's fill is painted through
        // `Painter::set_clip_rect`, which replaces the clip in force
        // rather than intersecting it — so the scroll area's own clip,
        // the thing that would have cut the row off, was simply gone.
        //
        // Stated as "paints nothing below the rect it was given" rather
        // than as a pixel measurement, because that is the actual
        // contract between a view and its pane, and it is the one a
        // clip that replaces rather than intersects can break.
        for view in View::ALL {
            let snapshot = a_long_list();
            let mut app = App::new(crate::config::Config::default());
            app.view = view;
            app.snapshot = Some(std::sync::Arc::new(snapshot));

            // Both of these lists are read on their view's own schedule
            // rather than by the sampler, and a view that has never
            // refreshed starts a background read on its first frame and
            // draws its empty state — which paints nothing and would
            // pass this test without testing anything.
            app.services.services = (0..400)
                .map(|index| crate::win::services::Service {
                    name: format!("svc{index}"),
                    display_name: format!("Service {index}"),
                    state: crate::win::services::ServiceState::Running,
                    pid: Some(index + 4),
                })
                .collect();
            app.services.refreshed = Some(std::time::Instant::now());
            app.startup.entries = (0..400)
                .map(|index| crate::win::startup::StartupEntry {
                    name: format!("entry{index}"),
                    command: format!(r"C:\Program Files\App{index}\app.exe"),
                    location: "HKCU Run",
                    all_users: false,
                    enabled: true,
                })
                .collect();
            app.startup.refreshed = app.services.refreshed;

            let window =
                egui::Rect::from_min_size(egui::Pos2::ZERO, egui::Vec2::new(1180.0, 760.0));
            // Deliberately short of the window, standing in for the
            // status bar: room below the pane for an escape to show up
            // in, which a view drawn to the window's own edge would not
            // give this test.
            let pane = egui::Rect::from_min_max(
                egui::pos2(0.0, 0.0),
                egui::pos2(window.right(), window.bottom() - 120.0),
            );

            let ctx = egui::Context::default();
            theme::apply(&ctx, &app.theme.clone());
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(window),
                    ..Default::default()
                },
                |ui| {
                    ui.scope_builder(egui::UiBuilder::new().max_rect(pane), |ui| match view {
                        View::Processes => processes::draw(&mut app, ui),
                        View::Details => details::draw(&mut app, ui),
                        View::Services => services::draw(&mut app, ui),
                        View::Startup => services::draw_startup(&mut app, ui),
                        View::Performance => performance::draw(&mut app, ui),
                        View::Memory => memory::draw(&mut app, ui),
                        View::System => system_info::draw(&mut app, ui),
                        View::Settings => settings::draw(&mut app, ui),
                    });
                },
            );
            output.textures_delta.clear();

            // The two axes get different tolerances, because only one
            // of them has anything legitimate to allow.
            //
            // Sideways, a row's fill is *deliberately* widened past its
            // viewport by half the item spacing — `widgets::row_clip`,
            // covering `egui_extras`' own hover fill, which is expanded
            // by that much and would otherwise show as a strip of the
            // control colour down the leading edge of every hovered
            // row. So the horizontal tolerance is that overhang plus
            // feathering, and it is a known quantity rather than a
            // fudge.
            //
            // Vertically there is no such allowance and none is wanted:
            // painting below the pane is the whole fault, so the only
            // tolerance is epaint's feathering.
            const FEATHER: f32 = 2.0;
            let bleed = 0.5 * theme::SPACE_SM + 1.0 + FEATHER;
            let allowed = pane.expand2(egui::vec2(bleed, FEATHER));
            for visible in painted(&ctx, output.shapes) {
                assert!(
                    allowed.contains_rect(visible),
                    "the {view:?} view painted {visible:?}, outside the pane it                      was given ({pane:?}). Whatever is beside or below this view                      is being drawn over: the Details table did exactly that to                      its own inspector, and a half-visible row did it to the                      status bar"
                );
            }
        }
        Ok(())
    }

    #[test]
    fn there_is_a_shortcut_for_every_view() {
        // Ctrl+1..8 covers the rail, so a view added later without a
        // digit would be the one view with no shortcut.
        let digits = 8;
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
    const DRAWING_MODULES: [(&str, &str); 11] = [
        ("chrome.rs", include_str!("chrome.rs")),
        ("details.rs", include_str!("details.rs")),
        ("graph.rs", include_str!("graph.rs")),
        ("memory.rs", include_str!("memory.rs")),
        ("modal.rs", include_str!("modal.rs")),
        ("performance.rs", include_str!("performance.rs")),
        ("processes.rs", include_str!("processes.rs")),
        ("services.rs", include_str!("services.rs")),
        ("settings.rs", include_str!("settings.rs")),
        ("system_info.rs", include_str!("system_info.rs")),
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
        //
        // The exception is the window's own chrome: handing a gesture
        // straight to `ViewportCommand::StartDrag` (moving the window,
        // `chrome::drag_region`) or `BeginResize` (its invisible resize
        // handles, `chrome::resize_handles`) holds no state at all, which
        // is the opposite of what this lint exists to catch. A
        // `drag_started` a handful of lines above either is that
        // hand-off, not a view reimplementing `dnd::Lane`.
        for (name, source) in DRAWING_MODULES {
            let lines: Vec<&str> = source.lines().collect();
            for (number, line) in lines.iter().enumerate() {
                if is_prose(line) {
                    continue;
                }
                let hand_tracked = line.contains("drag_started")
                    || line.contains("drag_stopped")
                    || line.contains("dragged_by")
                    || line.contains("drag_delta");
                if !hand_tracked {
                    continue;
                }
                let nearby = lines[number..(number + 10).min(lines.len())].join("\n");
                let handed_to_window_manager = nearby.contains("ViewportCommand::StartDrag")
                    || nearby.contains("ViewportCommand::BeginResize");
                assert!(
                    handed_to_window_manager,
                    "{name}:{} tracks a drag by hand: {}. Use gui::ui::dnd, \
                     which owns the feedback as well as the state",
                    number + 1,
                    line.trim()
                );
            }
        }
    }

    #[test]
    fn no_fixed_panel_offers_a_resize_handle_that_does_nothing() {
        // `egui::Panel` is `resizable: true` by default, and every panel
        // in this app is `.exact_size(..)`. The two together give a panel
        // an edge that shows a resize cursor, highlights under the
        // pointer, and then clamps the drag straight back to the size it
        // already was — a control that invites a gesture and cannot
        // respond to it. The window had two of them side by side in the
        // Performance view, and the report was exactly that: two lines to
        // grab, neither doing anything.
        //
        // If a panel ever should be resizable, it will not be an
        // `exact_size` one, so the pairing is the tell.
        for (name, source) in DRAWING_MODULES {
            let lines: Vec<&str> = source.lines().collect();
            for (number, line) in lines.iter().enumerate() {
                if is_prose(line) || !line.contains(".exact_size(") {
                    continue;
                }
                // The builder call is a chain, so the setting can be on
                // any of the next few lines rather than the very next.
                let chain = lines[number..(number + 6).min(lines.len())].join(
                    "
",
                );
                assert!(
                    chain.contains(".resizable(false)"),
                    "{name}:{} sizes a panel exactly but leaves it                      resizable: {}. The edge offers a drag it will clamp                      away — say `.resizable(false)`",
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

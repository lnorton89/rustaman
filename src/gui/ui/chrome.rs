// ============================================================================
// Module:       gui::ui::chrome
// Description:  The window's own title bar, the navigation rail, and the status
//               bar — everything around the view rather than in it.
//
// Dependencies: egui (ViewportCommand for the window controls); super::theme,
//               super::widgets, super::super::app
// ============================================================================

//! The window's frame.
//!
//! ## Why the title bar is drawn rather than asked for
//!
//! Windows 10 gives an app a system caption that is a light grey bar with
//! the system's own fonts and buttons in it. Nothing an app does reaches
//! it: DWM's `DWMWA_USE_IMMERSIVE_DARK_MODE` (see [`crate::win::dwm`])
//! darkens it and nothing more, and it did not exist before 1809. A dark
//! app under one looks like a dark app wearing someone else's hat.
//!
//! So the window is opened undecorated and the bar is drawn here — which
//! also buys the room to put the search box and the process count *in*
//! it, rather than in a second row underneath.
//!
//! The cost is that everything the system caption did has to be
//! reimplemented, and the parts people notice when they are missing are:
//!
//! - **Dragging**, including snap. [`ViewportCommand::StartDrag`] is not
//!   a "move the window" command — it hands the drag to the window
//!   manager, which is what makes Aero Snap, snap layouts, and
//!   drag-to-another-monitor keep working. A hand-rolled "move by the
//!   pointer delta" loses all three, and loses them silently.
//! - **Double-click to maximise**, which is muscle memory.
//! - **Resizing**, since an undecorated window has no border to grab.
//!   [`resize_handles`] adds eight invisible ones.
//!
//! A user who would rather have the system bar can have it: the Settings
//! view has a switch, and [`crate::gui::run`] honours it at startup.

use super::theme::{
    self, NAV_WIDTH, PAD, SPACE_LG, SPACE_MD, SPACE_SM, SPACE_XS, TITLE_BAR_HEIGHT,
};
use super::widgets;
use crate::gui::app::{App, View, TOAST_SECONDS};
use crate::icon::Icon;
use crate::theme::Palette;
use egui::{Align, CornerRadius, Layout, Rect, ResizeDirection, Sense, Ui, Vec2, ViewportCommand};

/// How thick the invisible resize handles are.
///
/// Six points: wide enough to hit without aiming, narrow enough that it
/// does not steal clicks from a control near the edge. The system's own
/// border is about four, and an undecorated window has none at all — so
/// slightly more than the system's is the right answer, not less.
const RESIZE_GRIP: f32 = 6.0;

// Relations between constants, so checked when the crate is compiled.
const _: () = {
    assert!(
        RESIZE_GRIP >= 4.0,
        "an undecorated window has no border at all, so the grip must be \
         at least as wide as the system's own ~4 points"
    );
    assert!(
        RESIZE_GRIP <= 10.0,
        "a grip this wide steals clicks from controls at the window edge \
         — a table's scrollbar is exactly there"
    );
};

/// Draws the title bar and returns whether the window should close.
pub fn title_bar(app: &mut App, ui: &mut Ui) -> bool {
    let theme = app.theme.clone();
    let mut closing = false;

    egui::Panel::top("title-bar")
        .exact_size(TITLE_BAR_HEIGHT)
        .resizable(false)
        .frame(
            egui::Frame::new()
                .fill(theme::rgb(theme.app))
                .inner_margin(theme::margin_xy(SPACE_SM, SPACE_XS)),
        )
        .show(ui, |ui| {
            ui.horizontal_centered(|ui| {
                // The brand mark and name, at the left edge.
                super::super::icons::paint_brand(ui, 18.0);
                ui.add_space(SPACE_SM);
                ui.label(
                    egui::RichText::new(crate::brand::NAME)
                        .color(theme::rgb(theme.text))
                        .strong(),
                );

                // The window controls, at the right. Laid out
                // right-to-left so they sit against the edge whatever the
                // window's width, and drawn before the draggable region
                // below so that region gets what is left rather than
                // overlapping them.
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if widgets::close_button(ui, &theme).clicked() {
                        closing = true;
                    }
                    let (maximise, tooltip) = if app.maximised {
                        (Icon::WindowRestore, "Restore")
                    } else {
                        (Icon::WindowMaximise, "Maximise")
                    };
                    if widgets::icon_button(ui, &theme, maximise, tooltip).clicked() {
                        app.maximised = !app.maximised;
                        ui.ctx()
                            .send_viewport_cmd(ViewportCommand::Maximized(app.maximised));
                    }
                    if widgets::icon_button(ui, &theme, Icon::WindowMinimise, "Minimise").clicked()
                    {
                        ui.ctx().send_viewport_cmd(ViewportCommand::Minimized(true));
                    }

                    ui.add_space(SPACE_SM);
                    // The status readout, right-aligned beside the
                    // controls: it is the one number worth having on
                    // screen whatever view is open.
                    if let Some(snapshot) = app.snapshot.as_ref() {
                        ui.label(
                            egui::RichText::new(format!(
                                "{} processes · {} CPU · {} memory",
                                snapshot.system.process_count,
                                crate::format::percent(snapshot.system.cpu.total_percent),
                                crate::format::percent(snapshot.system.memory.used_percent()),
                            ))
                            .color(theme::rgb(theme.text_muted))
                            .text_style(egui::TextStyle::Small),
                        );
                    }

                    // Whatever width is left between the brand and the
                    // controls is the drag region.
                    drag_region(ui, app);
                });
            });
        });

    closing
}

/// Makes the empty part of the title bar drag the window.
///
/// See the module docs on why this is `StartDrag` rather than a
/// hand-rolled move.
fn drag_region(ui: &mut Ui, app: &mut App) {
    let space = ui.available_size();
    if space.x <= 0.0 {
        return;
    }
    let (rect, response) = ui.allocate_exact_size(space, Sense::click_and_drag());
    let _ = rect;

    if response.drag_started() {
        // Hands the drag to the window manager. This is what keeps Aero
        // Snap, snap layouts, and drag-to-another-monitor working — a
        // "move by the pointer delta" implementation loses all three, and
        // loses them silently.
        ui.ctx().send_viewport_cmd(ViewportCommand::StartDrag);
    }
    if response.double_clicked() {
        app.maximised = !app.maximised;
        ui.ctx()
            .send_viewport_cmd(ViewportCommand::Maximized(app.maximised));
    }
}

/// Adds the eight invisible resize handles an undecorated window needs.
///
/// Drawn last, over everything, so they win the hit test against a
/// control that happens to sit against the window edge — a table's
/// scrollbar is exactly there, and losing the resize to it would make the
/// window feel stuck.
pub fn resize_handles(ui: &mut Ui) {
    let rect = ui.ctx().content_rect();
    // Corners before edges: a corner is inside both edges' bands, and
    // whichever is registered first wins the hit test. Diagonal resize is
    // the one people aim for deliberately.
    let handles = [
        (
            Rect::from_min_size(rect.left_top(), Vec2::splat(RESIZE_GRIP)),
            ResizeDirection::NorthWest,
            egui::CursorIcon::ResizeNwSe,
        ),
        (
            Rect::from_min_size(
                rect.right_top() - Vec2::new(RESIZE_GRIP, 0.0),
                Vec2::splat(RESIZE_GRIP),
            ),
            ResizeDirection::NorthEast,
            egui::CursorIcon::ResizeNeSw,
        ),
        (
            Rect::from_min_size(
                rect.left_bottom() - Vec2::new(0.0, RESIZE_GRIP),
                Vec2::splat(RESIZE_GRIP),
            ),
            ResizeDirection::SouthWest,
            egui::CursorIcon::ResizeNeSw,
        ),
        (
            Rect::from_min_size(
                rect.right_bottom() - Vec2::splat(RESIZE_GRIP),
                Vec2::splat(RESIZE_GRIP),
            ),
            ResizeDirection::SouthEast,
            egui::CursorIcon::ResizeNwSe,
        ),
        (
            Rect::from_min_size(rect.left_top(), Vec2::new(rect.width(), RESIZE_GRIP)),
            ResizeDirection::North,
            egui::CursorIcon::ResizeVertical,
        ),
        (
            Rect::from_min_size(
                rect.left_bottom() - Vec2::new(0.0, RESIZE_GRIP),
                Vec2::new(rect.width(), RESIZE_GRIP),
            ),
            ResizeDirection::South,
            egui::CursorIcon::ResizeVertical,
        ),
        (
            Rect::from_min_size(rect.left_top(), Vec2::new(RESIZE_GRIP, rect.height())),
            ResizeDirection::West,
            egui::CursorIcon::ResizeHorizontal,
        ),
        (
            Rect::from_min_size(
                rect.right_top() - Vec2::new(RESIZE_GRIP, 0.0),
                Vec2::new(RESIZE_GRIP, rect.height()),
            ),
            ResizeDirection::East,
            egui::CursorIcon::ResizeHorizontal,
        ),
    ];

    for (index, (area, direction, cursor)) in handles.into_iter().enumerate() {
        let id = egui::Id::new("resize-handle").with(index);
        let response = ui.interact(area, id, Sense::drag());
        if response.hovered() || response.dragged() {
            ui.ctx().set_cursor_icon(cursor);
        }
        if response.drag_started() {
            ui.ctx()
                .send_viewport_cmd(ViewportCommand::BeginResize(direction));
        }
    }
}

/// Draws the navigation rail.
pub fn nav_rail(app: &mut App, ui: &mut Ui) {
    let theme = app.theme.clone();
    egui::Panel::left("nav-rail")
        .exact_size(NAV_WIDTH)
        .resizable(false)
        .frame(
            egui::Frame::new()
                .fill(theme::rgb(theme.app))
                .inner_margin(theme::margin(SPACE_SM)),
        )
        .show(ui, |ui| {
            for view in View::ALL {
                // Settings sits at the bottom, away from the views that
                // are switched between constantly — a destination reached
                // occasionally should not be one keystroke from the one
                // reached every time.
                if view == View::Settings {
                    continue;
                }
                if widgets::nav_item(ui, &theme, view.icon(), view.label(), app.view == view)
                    .clicked()
                {
                    app.view = view;
                }
                ui.add_space(SPACE_XS);
            }

            ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
                if widgets::nav_item(
                    ui,
                    &theme,
                    View::Settings.icon(),
                    View::Settings.label(),
                    app.view == View::Settings,
                )
                .clicked()
                {
                    app.view = View::Settings;
                }
                ui.add_space(SPACE_XS);

                // The elevation notice. Not a warning — running
                // unelevated is a perfectly reasonable way to use this —
                // but the missing columns need an explanation, or they
                // look like a bug in the app.
                if !app.elevated {
                    let response =
                        widgets::chip(ui, "Limited access", theme.raised, theme.text_muted);
                    response.on_hover_text(
                        "Some processes' owner, path and architecture cannot be \
                         read without administrator rights. Everything else is \
                         shown for every process.",
                    );
                }
            });
        });
}

/// Draws the status bar along the bottom.
pub fn status_bar(app: &mut App, ui: &mut Ui) {
    let theme = app.theme.clone();
    /// The bar's height. One line of small text with the standard
    /// vertical inset either side.
    const HEIGHT: f32 = 26.0;

    egui::Panel::bottom("status-bar")
        .exact_size(HEIGHT)
        .resizable(false)
        .frame(
            egui::Frame::new()
                .fill(theme::rgb(theme.panel))
                .inner_margin(theme::margin_xy(PAD, SPACE_XS)),
        )
        .show(ui, |ui| {
            ui.horizontal_centered(|ui| {
                // A toast takes the left of the bar while it is up, then
                // gives it back to the summary. A separate strip for
                // messages would be a strip that is empty almost always.
                if let Some(text) = active_toast(app) {
                    let color = if app.toast.as_ref().is_some_and(|toast| toast.failed) {
                        theme.danger
                    } else {
                        theme.success
                    };
                    ui.label(
                        egui::RichText::new(text)
                            .color(theme::rgb(color))
                            .text_style(egui::TextStyle::Small),
                    );
                } else if let Some(snapshot) = app.snapshot.as_ref() {
                    ui.label(
                        egui::RichText::new(format!(
                            "Up {} · {} threads · {} handles",
                            crate::format::duration(snapshot.system.uptime_seconds),
                            crate::format::count(snapshot.system.thread_count),
                            crate::format::count(snapshot.system.handle_count),
                        ))
                        .color(theme::rgb(theme.text_muted))
                        .text_style(egui::TextStyle::Small),
                    );
                }

                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    // A stalled sampler has to say so. Without this the
                    // window sits there with a frozen display looking
                    // perfectly live, which is the worst way for a
                    // monitoring tool to fail.
                    if app.engine.is_running() {
                        ui.label(
                            egui::RichText::new(format!(
                                "Updating every {}",
                                interval_label(app.engine.interval()),
                            ))
                            .color(theme::rgb(theme.text_faint))
                            .text_style(egui::TextStyle::Small),
                        );
                    } else {
                        ui.label(
                            egui::RichText::new("Sampling has stopped")
                                .color(theme::rgb(theme.danger))
                                .text_style(egui::TextStyle::Small),
                        );
                    }
                });
            });
        });
}

/// The toast's text, if one is still within its window.
fn active_toast(app: &App) -> Option<String> {
    let toast = app.toast.as_ref()?;
    (toast.raised.elapsed().as_secs_f32() < TOAST_SECONDS).then(|| toast.message.clone())
}

/// A sampling interval as a short label.
#[must_use]
pub fn interval_label(interval: std::time::Duration) -> String {
    let millis = interval.as_millis();
    if millis < 1_000 {
        return format!("{millis} ms");
    }
    let seconds = interval.as_secs_f32();
    if (seconds - seconds.round()).abs() < 0.05 {
        return format!("{} s", seconds.round() as i64);
    }
    format!("{seconds:.1} s")
}

/// Draws the search box that both the process list and Details share.
///
/// Returns whether the text changed, which the caller uses to reparse the
/// query — reparsing unconditionally would parse it once per frame rather
/// than once per keystroke.
pub fn search_box(ui: &mut Ui, theme: &Palette, text: &mut String, hint: &str) -> bool {
    /// The box's width. Wide enough for a full path fragment, narrow
    /// enough to leave the toolbar room for its buttons.
    const WIDTH: f32 = 260.0;

    let before = text.clone();
    let response = ui.add_sized(
        Vec2::new(WIDTH, 0.0),
        egui::TextEdit::singleline(text)
            .hint_text(hint)
            .background_color(theme::rgb(theme.raised))
            .margin(theme::margin_xy(SPACE_SM, SPACE_XS))
            .desired_width(WIDTH),
    );

    // Escape clears the box while it has focus, which is what every
    // search field does and what the hand reaches for.
    if response.has_focus() && ui.input(|input| input.key_pressed(egui::Key::Escape)) {
        text.clear();
    }
    *text != before
}

/// Draws a toolbar row's separator dot.
///
/// Between groups of controls in a toolbar, where a full rule would be
/// too heavy and a gap alone does not read as a grouping.
pub fn toolbar_dot(ui: &mut Ui, theme: &Palette) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(SPACE_MD, SPACE_MD), Sense::hover());
    ui.painter()
        .circle_filled(rect.center(), 2.0, theme::rgb(theme.border));
}

/// A card the width of the pane, for the Performance view's sections.
pub fn panel_card<R>(ui: &mut Ui, theme: &Palette, add: impl FnOnce(&mut Ui) -> R) -> R {
    let response = egui::Frame::new()
        .fill(theme::rgb(theme.panel))
        .stroke(egui::Stroke::new(1.0, theme::rgb(theme.border)))
        .corner_radius(CornerRadius::same(theme::RADIUS_LG))
        .inner_margin(theme::margin(SPACE_MD))
        .show(ui, add);
    ui.add_space(SPACE_SM);
    response.inner
}

/// The gap a view leaves between its toolbar and its content.
pub const TOOLBAR_GAP: f32 = SPACE_SM;

/// The gap a view leaves between major sections.
pub const SECTION_GAP: f32 = SPACE_LG;

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn interval_labels_read_the_way_a_person_would_say_them() {
        assert_eq!(interval_label(Duration::from_millis(250)), "250 ms");
        assert_eq!(interval_label(Duration::from_millis(500)), "500 ms");
        assert_eq!(interval_label(Duration::from_secs(1)), "1 s");
        assert_eq!(interval_label(Duration::from_secs(2)), "2 s");
        assert_eq!(interval_label(Duration::from_secs(10)), "10 s");
    }

    #[test]
    fn an_awkward_interval_still_labels_cleanly() {
        assert_eq!(interval_label(Duration::from_millis(1_500)), "1.5 s");
        assert_eq!(interval_label(Duration::ZERO), "0 ms");
    }

    #[test]
    fn every_offered_interval_has_a_readable_label() {
        // A settings control whose options render as "1.0000001 s" would
        // be a control nobody trusts.
        for millis in crate::config::INTERVAL_CHOICES {
            let label = interval_label(Duration::from_millis(millis));
            assert!(
                label.len() <= 7,
                "{millis}ms produced an unwieldy label: {label}"
            );
            assert!(label.ends_with('s'), "{label} should name a unit");
        }
    }
}

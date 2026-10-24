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
use super::{motion, widgets};
use crate::gui::app::{App, SamplerHealth, View, TOAST_SECONDS};
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

/// Corners need a larger target than straight edges.
///
/// Six-by-six made diagonal resizing technically present but practically
/// unreachable at normal pointer speeds and high-DPI scaling. Windows' own
/// non-client corner target is substantially larger for the same reason.
const RESIZE_CORNER: f32 = 18.0;

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
    assert!(RESIZE_CORNER > RESIZE_GRIP);
};

/// Draws the title bar and returns whether the window should close.
pub fn title_bar(app: &mut App, ui: &mut Ui) -> bool {
    let theme = app.theme.clone();
    let mut closing = false;

    // `show_separator_line(false)` below: `Panel` strokes a separator
    // at its inner edge by default, and every panel here is padded on
    // both sides — so the stroke lands as a pale hairline floating in a
    // gap, touching neither side, which is exactly what a grab handle
    // looks like. This panel is `resizable(false)`, so that is an
    // affordance for a gesture it cannot answer.
    egui::Panel::top("title-bar")
        .exact_size(TITLE_BAR_HEIGHT)
        .resizable(false)
        .show_separator_line(false)
        .frame(
            egui::Frame::new()
                .fill(theme::rgb(theme.app))
                .inner_margin(theme::margin_xy(PAD, SPACE_XS)),
        )
        .show(ui, |ui| {
            ui.horizontal_centered(|ui| {
                // The brand mark and name, at the left edge.
                super::super::icons::paint_brand(ui, 18.0);
                theme::gap(ui, SPACE_SM);
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

                    theme::gap(ui, SPACE_MD);
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
/// `maximised` suppresses them entirely, and that is not an optimisation.
/// A maximised window has no resizable edge — dragging one does nothing
/// — but these are a *foreground* layer, so they win the hit test over
/// whatever is beneath them. At the top right that is the close button,
/// and it is precisely when the window is maximised that the corner
/// matters most: it is the one place the pointer can be thrown without
/// aiming. Handles for a gesture the window cannot perform were taking
/// the corner from the control that can.
pub fn resize_handles(ui: &mut Ui, maximised: bool) {
    if maximised {
        return;
    }
    let rect = ui.ctx().content_rect();
    resize_handles_in(ui, rect);
}

fn resize_handles_in(ui: &mut Ui, rect: Rect) {
    for (index, (area, direction, cursor)) in resize_regions(rect).into_iter().enumerate() {
        let id = egui::Id::unique("resize-handle").with(index);
        // A foreground Area gives the native border its own interaction
        // layer. Registering the rectangles on the root Ui looked correct but
        // left them underneath panel widgets in egui's hit-test order, so a
        // corner could be stolen by the title bar or the table beneath it.
        egui::Area::new(id)
            .order(egui::Order::Foreground)
            .constrain(false)
            .fixed_pos(area.min)
            .show(ui.ctx(), |ui| {
                let (_, response) = ui.allocate_exact_size(area.size(), Sense::click_and_drag());
                // These eight rectangles are disjoint, so geometric hit
                // testing is both unambiguous and more reliable than asking
                // egui which widget owns the pointer. The latter can report a
                // title-bar/table widget even when this foreground border is
                // visibly under the pointer.
                let contains_pointer = ui.input(|input| {
                    input
                        .pointer
                        .interact_pos()
                        .is_some_and(|pos| response.rect.contains(pos))
                });
                if contains_pointer {
                    ui.ctx().set_cursor_icon(cursor);
                }
                // Hand the gesture to Windows on the press frame. Waiting for
                // egui's drag threshold misses the button-down event that the
                // native `drag_resize_window` call requires.
                if contains_pointer && ui.input(|input| input.pointer.primary_pressed()) {
                    ui.ctx()
                        .send_viewport_cmd(ViewportCommand::BeginResize(direction));
                }
            });
    }
}

/// The eight disjoint hit regions around an undecorated viewport.
///
/// Edge bands deliberately stop at the corner squares. Egui resolves
/// overlapping interactions by layer/order, so registering a full-width
/// horizontal edge over a corner lets that edge steal the pointer and turns
/// an intended diagonal resize into a vertical one.
fn resize_regions(rect: Rect) -> [(Rect, ResizeDirection, egui::CursorIcon); 8] {
    let horizontal = (rect.width() - RESIZE_CORNER * 2.0).max(0.0);
    let vertical = (rect.height() - RESIZE_CORNER * 2.0).max(0.0);
    [
        (
            Rect::from_min_size(rect.left_top(), Vec2::splat(RESIZE_CORNER)),
            ResizeDirection::NorthWest,
            egui::CursorIcon::ResizeNwSe,
        ),
        (
            Rect::from_min_size(
                rect.right_top() - Vec2::new(RESIZE_CORNER, 0.0),
                Vec2::splat(RESIZE_CORNER),
            ),
            ResizeDirection::NorthEast,
            egui::CursorIcon::ResizeNeSw,
        ),
        (
            Rect::from_min_size(
                rect.left_bottom() - Vec2::new(0.0, RESIZE_CORNER),
                Vec2::splat(RESIZE_CORNER),
            ),
            ResizeDirection::SouthWest,
            egui::CursorIcon::ResizeNeSw,
        ),
        (
            Rect::from_min_size(
                rect.right_bottom() - Vec2::splat(RESIZE_CORNER),
                Vec2::splat(RESIZE_CORNER),
            ),
            ResizeDirection::SouthEast,
            egui::CursorIcon::ResizeNwSe,
        ),
        (
            Rect::from_min_size(
                rect.left_top() + Vec2::new(RESIZE_CORNER, 0.0),
                Vec2::new(horizontal, RESIZE_GRIP),
            ),
            ResizeDirection::North,
            egui::CursorIcon::ResizeVertical,
        ),
        (
            Rect::from_min_size(
                rect.left_bottom() + Vec2::new(RESIZE_CORNER, -RESIZE_GRIP),
                Vec2::new(horizontal, RESIZE_GRIP),
            ),
            ResizeDirection::South,
            egui::CursorIcon::ResizeVertical,
        ),
        (
            Rect::from_min_size(
                rect.left_top() + Vec2::new(0.0, RESIZE_CORNER),
                Vec2::new(RESIZE_GRIP, vertical),
            ),
            ResizeDirection::West,
            egui::CursorIcon::ResizeHorizontal,
        ),
        (
            Rect::from_min_size(
                rect.right_top() + Vec2::new(-RESIZE_GRIP, RESIZE_CORNER),
                Vec2::new(RESIZE_GRIP, vertical),
            ),
            ResizeDirection::East,
            egui::CursorIcon::ResizeHorizontal,
        ),
    ]
}

/// Draws the navigation rail.
///
/// A collapsed rail reveals itself under the pointer. That reveal is an
/// *overlay* over the content rather than a wider panel, and the
/// difference is not cosmetic: every table in this app sizes its columns
/// against the pane it is handed (see `details::draw`), so a rail that
/// grew on hover would reflow the whole view — on every accidental pass
/// of the pointer, for a reveal nobody asked for. The overlay leaves the
/// content exactly where it is.
pub fn nav_rail(app: &mut App, ui: &mut Ui) {
    let theme = app.theme.clone();
    let compact = app.config.nav_collapsed.unwrap_or(false);
    if !compact {
        app.nav_flyout = false;
    }
    let width = if compact {
        theme::NAV_MINI_WIDTH
    } else {
        NAV_WIDTH
    };
    // While the flyout is up it *is* the rail, so the panel behind it
    // draws its surface and nothing else. Leaving the narrow copy drawn
    // under an opaque overlay puts every destination in the frame twice:
    // unreachable by pointer, still reachable by Tab, and read out twice
    // by a screen reader.
    //
    // Both surfaces are `theme.panel`, so the handover is invisible —
    // what fades in over an unchanged background is the icons and their
    // labels. The flag is last frame's answer, which is a frame of lag
    // on a reveal that takes a tenth of a second.
    let showing_flyout = compact && app.nav_flyout;
    // `show_separator_line(false)` below: `Panel` strokes a separator
    // at its inner edge by default, and every panel here is padded on
    // both sides — so the stroke lands as a pale hairline floating in a
    // gap, touching neither side, which is exactly what a grab handle
    // looks like. This panel is `resizable(false)`, so that is an
    // affordance for a gesture it cannot answer.
    let rail = egui::Panel::left("nav-rail")
        .exact_size(width)
        .resizable(false)
        .show_separator_line(false)
        .frame(
            egui::Frame::new()
                .fill(theme::rgb(theme.panel))
                .inner_margin(theme::margin(PAD)),
        )
        .show(ui, |ui| {
            if !showing_flyout {
                rail_body(app, ui, compact, compact);
            }
        })
        .response
        .rect;

    // Re-read, because the body above may have just answered a click on
    // the toggle. `compact` is this frame's *layout*, decided before the
    // click; `still_collapsed` is the preference as it now stands. On
    // the frame the rail is expanded the two disagree, and unfolding a
    // flyout over a rail that has just been pinned open puts a second
    // copy of every control on screen for that frame — including a
    // second toggle, on top of the one the pointer is resting on.
    let still_collapsed = app.config.nav_collapsed.unwrap_or(false);
    if compact && still_collapsed {
        nav_flyout(app, ui, rail);
    }
}

/// The rail's contents, drawn either in the panel or in the hover flyout.
///
/// One function for both so a destination cannot appear in the flyout
/// that the rail does not have, or answer a click differently there.
///
/// `compact` is how this copy is drawn; `collapsed` is the saved
/// preference. They differ in the flyout, which draws the full rail
/// while the preference is still collapsed — so its toggle pins the
/// rail open instead of collapsing one that is already collapsed.
fn rail_body(app: &mut App, ui: &mut Ui, compact: bool, collapsed: bool) {
    let theme = app.theme.clone();
    ui.spacing_mut().item_spacing.y = SPACE_XS;
    if widgets::nav_toggle(ui, &theme, compact, collapsed).clicked() {
        let now_collapsed = !collapsed;
        app.config.nav_collapsed = Some(now_collapsed);
        // The pointer is resting on the control that just narrowed the
        // rail. A flyout unfolding under it on the next frame would
        // undo the only visible result of the click.
        app.nav_flyout_shut = now_collapsed;
    }
    theme::gap(ui, SPACE_MD);
    for view in View::ALL {
        // Settings sits at the bottom, away from the views that
        // are switched between constantly — a destination reached
        // occasionally should not be one keystroke from the one
        // reached every time.
        if view == View::Settings {
            continue;
        }
        if view == View::Services {
            theme::gap(ui, SPACE_LG);
        }
        if widgets::nav_destination(
            ui,
            &theme,
            view.icon(),
            view.label(),
            app.view == view,
            compact,
        )
        .clicked()
        {
            app.view = view;
        }
    }

    ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
        if widgets::nav_destination(
            ui,
            &theme,
            View::Settings.icon(),
            View::Settings.label(),
            app.view == View::Settings,
            compact,
        )
        .clicked()
        {
            app.view = View::Settings;
        }

        // The elevation notice. Not a warning — running
        // unelevated is a perfectly reasonable way to use this —
        // but the missing columns need an explanation, or they
        // look like a bug in the app.
        if !app.elevated {
            let response = if compact {
                widgets::access_notice(ui, &theme)
            } else {
                widgets::chip(ui, "Limited access", theme.raised, theme.text_muted)
            };
            response.on_hover_text(
                "Some processes hide owner, path or architecture. \
                 Administrator rights improve access, but protected \
                 processes can still refuse it.",
            );
        }
    });
}

/// The collapsed rail's hover reveal: the full rail, over the content.
///
/// Held open by the pointer being anywhere over the flyout, not over the
/// narrow rail that spawned it. Testing the rail alone shuts the reveal
/// the instant the pointer reaches the labels it just opened, which is
/// the one place the reader was going.
fn nav_flyout(app: &mut App, ui: &mut Ui, rail: Rect) {
    // The region that keeps it open: the rail while shut, the whole
    // flyout once open. `app.nav_flyout` carries the answer from last
    // frame, which is a frame of lag on a pointer that needs dozens to
    // cross the rail.
    let reach = if app.nav_flyout {
        Rect::from_min_size(rail.min, Vec2::new(NAV_WIDTH, rail.height()))
    } else {
        rail
    };
    // Geometric, like the resize handles above and for the same reason:
    // this is a foreground layer over whatever the view put underneath,
    // and asking egui which widget owns the pointer answers about the
    // table below as readily as about this.
    let pointer = ui
        .ctx()
        .input(|input| input.pointer.hover_pos())
        .is_some_and(|pos| reach.contains(pos));
    if !pointer {
        app.nav_flyout_shut = false;
    }
    // Three things that are not a hover over the rail, because the
    // pointer test above is geometric and cannot see any of them:
    //
    // - A drag that began elsewhere. A column being resized or a row
    //   being reordered must not be interrupted by a rail unfolding
    //   over the table it is crossing.
    // - A confirmation awaiting an answer. Its scrim covers the rail,
    //   and this layer would unfold straight through it.
    // - The about panel, for the same reason.
    // - A pointer button that is already down. This one is not taste:
    //   a press and its release arrive on separate frames, and opening
    //   the flyout between them takes the rail's own body off screen
    //   mid-gesture — so the widget egui recorded the press against no
    //   longer exists on the release frame, and the click is silently
    //   lost. That is what broke `mini_navigation_…`: the toggle under
    //   the pointer stopped answering, because unfolding the rail
    //   removed it halfway through being clicked. A press that began on
    //   the narrow rail finishes on the narrow rail.
    let blocked = app.pending.is_some()
        || app.about
        || ui.ctx().dragged_id().is_some()
        || ui.ctx().input(|input| input.pointer.any_down());
    let open = pointer && !app.nav_flyout_shut && !blocked;
    app.nav_flyout = open;

    let progress = motion::transition(
        ui.ctx(),
        egui::Id::unique("nav-flyout"),
        open,
        motion::QUICK,
    );
    if progress <= 0.0 {
        return;
    }
    let theme = app.theme.clone();
    egui::Area::new(egui::Id::unique("nav-flyout-area"))
        .order(egui::Order::Foreground)
        .constrain(false)
        .fixed_pos(rail.min)
        .show(ui.ctx(), |ui| {
            // Faded rather than unrolled. A width that animates would
            // re-lay-out the rail's own contents every frame of the
            // reveal, and a label half-clipped mid-slide reads as a
            // rendering fault rather than as motion.
            ui.multiply_opacity(progress);
            egui::Frame::new()
                .fill(theme::rgb(theme.panel))
                .stroke(egui::Stroke::new(1.0, theme::rgb(theme.border)))
                .corner_radius(CornerRadius {
                    nw: 0,
                    sw: 0,
                    ne: theme::RADIUS,
                    se: theme::RADIUS,
                })
                .inner_margin(theme::margin(PAD))
                .shadow(egui::epaint::Shadow {
                    offset: [8, 0],
                    blur: 24,
                    spread: 0,
                    // Alpha-only: a shadow is lighting, not a theme
                    // colour. See `gui::ui::theme`.
                    color: egui::Color32::from_black_alpha(96),
                })
                .show(ui, |ui| {
                    ui.set_width(NAV_WIDTH - PAD * 2.0);
                    ui.set_height(rail.height() - PAD * 2.0);
                    // Drawn full-width, with the preference read live
                    // rather than assumed collapsed: the toggle inside
                    // must decide from the state as it stands, or a
                    // click here writes from a stale reading of it.
                    let collapsed = app.config.nav_collapsed.unwrap_or(false);
                    rail_body(app, ui, false, collapsed);
                });
        });
}

/// Gives every destination a consistent title and a short description.
pub fn view_heading(app: &App, ui: &mut Ui) {
    let description = match app.view {
        View::Processes => "Process tree and resource usage",
        View::Performance => "CPU, memory, disks, network and GPU",
        View::Memory => "Memory usage by process",
        View::Details => "All processes and technical details",
        View::Services => "Windows services",
        View::Startup => "Programs that run at sign-in",
        View::System => "Hardware and Windows information",
        View::Settings => "Appearance and app preferences",
    };
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(app.view.label()).heading().strong());
        theme::gap(ui, SPACE_MD);
        ui.add(
            egui::Label::new(
                egui::RichText::new(description).color(theme::rgb(app.theme.text_muted)),
            )
            .truncate(),
        );
    });
    theme::gap(ui, SPACE_MD);
}

/// Draws the status bar along the bottom.
pub fn status_bar(app: &mut App, ui: &mut Ui) {
    let theme = app.theme.clone();
    /// The bar's height. One line of small text with the standard
    /// vertical inset either side.
    const HEIGHT: f32 = 26.0;

    // `show_separator_line(false)` below: `Panel` strokes a separator
    // at its inner edge by default, and every panel here is padded on
    // both sides — so the stroke lands as a pale hairline floating in a
    // gap, touching neither side, which is exactly what a grab handle
    // looks like. This panel is `resizable(false)`, so that is an
    // affordance for a gesture it cannot answer.
    egui::Panel::bottom("status-bar")
        .exact_size(HEIGHT)
        .resizable(false)
        .show_separator_line(false)
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
                    if app.sampler_health() == SamplerHealth::Stopped {
                        ui.label(
                            egui::RichText::new("Sampling has stopped")
                                .color(theme::rgb(theme.danger))
                                .text_style(egui::TextStyle::Small),
                        );
                    } else if app.sampler_health() == SamplerHealth::Stale {
                        ui.label(
                            egui::RichText::new("Sample is stale")
                                .color(theme::rgb(theme.warning))
                                .text_style(egui::TextStyle::Small),
                        );
                    } else if app.sampler_health() == SamplerHealth::Starting {
                        ui.label(
                            egui::RichText::new("Starting sampler…")
                                .color(theme::rgb(theme.text_muted))
                                .text_style(egui::TextStyle::Small),
                        );
                    } else {
                        ui.label(
                            egui::RichText::new(format!(
                                "Updating every {}",
                                interval_label(app.engine.interval()),
                            ))
                            .color(theme::rgb(theme.text_muted))
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

/// The id every view's search box is drawn under, which is what Ctrl+F
/// focuses.
///
/// One id for all of them rather than one per view: only one view draws
/// at a time, so they never collide, and the shortcut does not have to
/// know which view is up. It has to be *stated*, because a `TextEdit`
/// given no id takes an automatic one — and `request_focus` on a name no
/// widget carries does not merely do nothing. egui holds the phantom id
/// as focused, `egui_wants_keyboard_input` turns true for it, and every
/// other shortcut is then swallowed until something else is clicked.
/// That was Ctrl+F for as long as the shortcut existed.
#[must_use]
pub fn search_box_id() -> egui::Id {
    egui::Id::unique("search-box")
}

/// A toolbar with a stable height and vertically centered controls.
pub fn toolbar(ui: &mut Ui, contents: impl FnOnce(&mut Ui)) {
    ui.allocate_ui_with_layout(
        Vec2::new(ui.available_width(), theme::CONTROL_HEIGHT),
        Layout::left_to_right(Align::Center),
        contents,
    );
}

/// Draws the search box that both the process list and Details share.
///
/// Returns whether the text changed, which the caller uses to reparse the
/// query — reparsing unconditionally would parse it once per frame rather
/// than once per keystroke.
pub fn search_box(ui: &mut Ui, theme: &Palette, text: &mut String, hint: &str) -> bool {
    /// Includes a leading search icon and a trailing shortcut or clear control.
    const WIDTH: f32 = 300.0;
    const HEIGHT: f32 = theme::CONTROL_HEIGHT;
    const TRAILING: f32 = 44.0;

    let before = text.clone();
    // Egui releases focus for Escape before drawing the frame. Include
    // the previous focus owner so the search still handles that key.
    let clear_with_escape = ui.memory(|memory| {
        memory.has_focus(search_box_id()) || memory.had_focus_last_frame(search_box_id())
    }) && ui.input(|input| input.key_pressed(egui::Key::Escape));
    let width = WIDTH.min(ui.available_width()).max(0.0);
    let (rect, shell) = ui.allocate_exact_size(Vec2::new(width, HEIGHT), Sense::hover());
    ui.painter().rect_filled(
        rect,
        CornerRadius::same(theme::RADIUS),
        theme::rgb(theme.raised),
    );
    super::icon::paint(
        ui.painter(),
        Rect::from_center_size(
            egui::pos2(
                rect.left() + SPACE_SM + super::icon::INLINE / 2.0,
                rect.center().y,
            ),
            Vec2::splat(super::icon::INLINE),
        ),
        Icon::Search,
        theme::rgb(theme.text_muted),
    );
    let input_rect = Rect::from_min_max(
        rect.left_top() + Vec2::new(SPACE_SM * 2.0 + super::icon::INLINE, SPACE_XS),
        rect.right_bottom() - Vec2::new(TRAILING + SPACE_SM, SPACE_XS),
    );
    // The shell already owns its place in the toolbar. A scoped layout
    // would advance the parent again from the narrower input rectangle,
    // allowing the next control to overlap the trailing button.
    let mut input_ui = ui.new_child(egui::UiBuilder::new().max_rect(input_rect));
    // `add_sized` makes the edit taller than one row, and egui places both
    // the typed text and the hint at the top of that rect unless told
    // otherwise (its default is `LEFT_TOP`), so center them vertically.
    let response = input_ui.add_sized(
        input_rect.size(),
        egui::TextEdit::singleline(text)
            .id(search_box_id())
            .align(egui::Align2::LEFT_CENTER)
            .hint_text(hint)
            .frame(egui::Frame::NONE)
            .margin(theme::margin_xy(0.0, 0.0))
            .desired_width(input_rect.width()),
    );

    let trailing = Rect::from_center_size(
        egui::pos2(rect.right() - SPACE_SM - TRAILING / 2.0, rect.center().y),
        Vec2::new(TRAILING, HEIGHT - SPACE_XS * 2.0),
    );
    if text.is_empty() {
        ui.painter().text(
            trailing.center(),
            egui::Align2::CENTER_CENTER,
            "Ctrl+F",
            egui::TextStyle::Small.resolve(ui.style()),
            theme::rgb(theme.text_muted),
        );
    } else {
        let clear = ui.interact(trailing, shell.id.with("clear"), Sense::click());
        clear.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "Clear search"));
        ui.painter().rect_filled(
            trailing,
            CornerRadius::same(theme::RADIUS),
            widgets::hover_fill(ui, clear.id, clear.hovered(), theme.raised, theme.hover),
        );
        super::icon::paint(
            ui.painter(),
            Rect::from_center_size(trailing.center(), Vec2::splat(super::icon::INLINE)),
            Icon::Close,
            theme::rgb(theme.text_muted),
        );
        widgets::focus_ring(ui, theme, &clear);
        if clear.on_hover_text("Clear search (Esc)").clicked() {
            text.clear();
            response.request_focus();
        }
    }
    ui.painter().rect_stroke(
        rect,
        CornerRadius::same(theme::RADIUS),
        egui::Stroke::new(
            1.0,
            theme::rgb(if response.has_focus() {
                theme.accent
            } else {
                theme.border
            }),
        ),
        egui::StrokeKind::Inside,
    );
    let mut field_focus = response.clone();
    field_focus.rect = rect;
    widgets::focus_ring(ui, theme, &field_focus);

    // Escape clears the box while it has focus, which is what every
    // search field does and what the hand reaches for.
    if clear_with_escape {
        text.clear();
        response.request_focus();
    }
    *text != before
}

/// Draws a toolbar row's separator dot.
///
/// Between groups of controls in a toolbar, where a full rule would be
/// too heavy and a gap alone does not read as a grouping.
pub fn toolbar_dot(ui: &mut Ui, theme: &Palette) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(SPACE_XS, SPACE_MD), Sense::hover());
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
    response.inner
}

/// The gap a view leaves between its toolbar and its content.
pub const TOOLBAR_GAP: f32 = SPACE_MD;

/// The gap a view leaves between major sections.
pub const SECTION_GAP: f32 = SPACE_LG;

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::kittest::Queryable;
    use std::time::Duration;

    #[test]
    fn mini_navigation_keeps_destinations_named_and_can_expand_again() {
        let mut harness = egui_kittest::Harness::builder()
            .with_size(egui::vec2(780.0, 480.0))
            .build_ui_state(
                |ui, app: &mut App| {
                    theme::apply(ui.ctx(), &app.theme.clone());
                    nav_rail(app, ui);
                },
                App::new(crate::config::Config::default()),
            );
        harness.run();
        harness.get_by_label("Collapse sidebar").click();
        harness.run();
        assert_eq!(harness.state().config.nav_collapsed, Some(true));
        let performance = harness.get_by_label("Performance");
        assert!((performance.rect().width() - theme::CONTROL_HEIGHT).abs() < 0.1);
        assert!(!performance
            .rect()
            .intersects(harness.get_by_label("Settings").rect()));
        performance.focus();
        harness.run();
        harness.key_press(egui::Key::Enter);
        harness.run();
        assert_eq!(harness.state().view, View::Performance);
        harness.get_by_label("Expand sidebar").click();
        harness.run();
        assert_eq!(
            harness.state().config.nav_collapsed,
            Some(false),
            "the rail's own toggle must expand it again (flyout open: {}, \
             held shut: {})",
            harness.state().nav_flyout,
            harness.state().nav_flyout_shut,
        );
        assert!(harness.get_by_label("Performance").rect().width() > 100.0);
    }

    #[test]
    fn the_caption_controls_reach_the_window_corner() {
        // Fitts's law: a pointer cannot travel past the edge of the
        // screen, so on a maximised window the top-right corner is a
        // target of effectively infinite size and the close button is
        // hit by throwing the pointer at it. The title bar's own padding
        // was insetting the controls out of that corner — 4 points below
        // the top edge and 12 short of the right — so the throw landed
        // on nothing.
        let window = egui::vec2(780.0, 480.0);
        let mut harness = egui_kittest::Harness::builder()
            .with_size(window)
            .build_ui_state(
                |ui, app: &mut App| {
                    theme::apply(ui.ctx(), &app.theme.clone());
                    let _ = title_bar(app, ui);
                },
                App::new(crate::config::Config::default()),
            );
        harness.run();

        let close = harness.get_by_label("Close").rect();
        assert!(
            close.top() <= 0.01,
            "the close button must reach the top edge of the window, got {close:?}"
        );
        assert!(
            close.right() >= window.x - 0.01,
            "and the right edge, got {close:?} in a window {window:?} wide"
        );

        // The neighbours reach the top too — the whole caption strip is
        // one unbroken band — but they are not at the right edge and
        // must not be stretched to it.
        for label in ["Maximise", "Minimise"] {
            let rect = harness.get_by_label(label).rect();
            assert!(
                rect.top() <= 0.01,
                "{label} must reach the top edge, got {rect:?}"
            );
            assert!(
                rect.right() < close.left() + 0.01,
                "{label} must stay left of Close rather than growing to the \
                 edge it does not touch, got {rect:?}"
            );
        }
    }

    /// A rail collapsed, with the pointer parked clear of it.
    fn collapsed_rail() -> egui_kittest::Harness<'static, App> {
        let mut harness = egui_kittest::Harness::builder()
            .with_size(egui::vec2(780.0, 480.0))
            .build_ui_state(
                |ui, app: &mut App| {
                    theme::apply(ui.ctx(), &app.theme.clone());
                    nav_rail(app, ui);
                },
                App::new(crate::config::Config {
                    nav_collapsed: Some(true),
                    ..crate::config::Config::default()
                }),
            );
        // Well to the right of the widest the rail can get, so the
        // flyout is shut before a test asks it to open.
        harness.hover_at(egui::pos2(600.0, 240.0));
        harness.run();
        harness
    }

    #[test]
    fn a_collapsed_rail_unfolds_under_the_pointer_without_being_un_collapsed() {
        let mut harness = collapsed_rail();
        assert!(
            harness
                .query_by_label("Performance")
                .is_none_or(|node| node.rect().width() < NAV_WIDTH / 2.0),
            "with the pointer away, the rail stays narrow"
        );

        harness.hover_at(egui::pos2(theme::NAV_MINI_WIDTH / 2.0, 240.0));
        harness.run();
        harness.run();
        let performance = harness.get_by_label("Performance");
        assert!(
            performance.rect().width() > NAV_WIDTH / 2.0,
            "hovering a collapsed rail should reveal the full-width \
             destinations, got {:?}",
            performance.rect()
        );
        assert_eq!(
            harness.state().config.nav_collapsed,
            Some(true),
            "a hover is where the pointer is, not a preference — the rail \
             must still be collapsed once the pointer leaves"
        );

        harness.hover_at(egui::pos2(600.0, 240.0));
        harness.run();
        harness.run();
        assert!(
            harness.get_by_label("Performance").rect().width() < NAV_WIDTH / 2.0,
            "and it folds back up when the pointer leaves"
        );
    }

    #[test]
    fn the_unfolded_rail_reaches_its_own_labels() {
        // The reveal is held open by the pointer being anywhere over the
        // flyout. Keyed to the narrow rail that spawned it, it would
        // shut the moment the pointer crossed onto the labels — which is
        // the one place the reader is going.
        let mut harness = collapsed_rail();
        harness.hover_at(egui::pos2(theme::NAV_MINI_WIDTH / 2.0, 240.0));
        harness.run();
        harness.run();
        let label_x = harness.get_by_label("Performance").rect().center().x;
        assert!(
            label_x > theme::NAV_MINI_WIDTH,
            "this test is pointless unless the label sits outside the \
             narrow rail, got {label_x}"
        );
        harness.hover_at(egui::pos2(label_x, 240.0));
        harness.run();
        harness.run();
        assert!(
            harness.get_by_label("Performance").rect().width() > NAV_WIDTH / 2.0,
            "the flyout must survive the pointer moving onto it"
        );
    }

    #[test]
    fn a_click_begun_on_the_narrow_rail_is_not_swallowed_by_the_reveal() {
        // A press and its release arrive on separate frames. The reveal
        // takes the narrow rail's own body off screen, so if it opens
        // between the two, the widget egui recorded the press against
        // has stopped existing by the release frame and the click is
        // lost — the control under the pointer simply stops answering.
        //
        // This is not hypothetical: it is what the reveal did to the
        // rail's own toggle the first time, and the symptom was a
        // sidebar that could be collapsed and then not expanded again.
        let mut harness = collapsed_rail();
        harness.get_by_label("Memory").click();
        harness.run();
        assert_eq!(
            harness.state().view,
            View::Memory,
            "a destination clicked on the narrow rail must still answer \
             while the pointer's arrival is opening the flyout over it"
        );
    }

    #[test]
    fn the_unfolded_rail_pins_open_rather_than_collapsing_what_is_collapsed() {
        let mut harness = collapsed_rail();
        harness.hover_at(egui::pos2(theme::NAV_MINI_WIDTH / 2.0, 240.0));
        harness.run();
        harness.run();
        // The control reads "Expand sidebar" in the flyout, because that
        // is what clicking it does: the rail is collapsed underneath,
        // and a second "Collapse" would be a no-op wearing the label of
        // the thing it cannot do.
        harness.get_by_label("Expand sidebar").click();
        harness.run();
        assert_eq!(harness.state().config.nav_collapsed, Some(false));
    }

    #[test]
    fn collapsing_the_rail_does_not_immediately_unfold_it_again() {
        // The pointer ends the click resting on the toggle, which is
        // inside the rail — so without holding the flyout shut until the
        // pointer leaves, the rail springs straight back to full width
        // and the click looks as though it did nothing.
        let mut harness = egui_kittest::Harness::builder()
            .with_size(egui::vec2(780.0, 480.0))
            .build_ui_state(
                |ui, app: &mut App| {
                    theme::apply(ui.ctx(), &app.theme.clone());
                    nav_rail(app, ui);
                },
                App::new(crate::config::Config::default()),
            );
        harness.run();
        harness.get_by_label("Collapse sidebar").click();
        harness.run();
        harness.run();
        assert_eq!(harness.state().config.nav_collapsed, Some(true));
        assert!(
            !harness.state().nav_flyout,
            "the rail must stay folded while the pointer is still on the \
             control that folded it"
        );
        assert!(
            harness.get_by_label("Performance").rect().width() < NAV_WIDTH / 2.0,
            "and it must look folded, too"
        );
    }

    #[test]
    fn clearing_search_reports_the_change_and_keeps_keyboard_focus() {
        let mut harness = egui_kittest::Harness::new_ui_state(
            |ui, state: &mut (String, bool)| {
                let theme = crate::theme::Catalog::load().get(None).clone();
                theme::apply(ui.ctx(), &theme);
                ui.horizontal(|ui| {
                    state.1 |= search_box(ui, &theme, &mut state.0, "Search processes");
                    let _ = ui.button("Tree");
                });
            },
            ("chrome".to_string(), false),
        );
        harness.run();
        let clear = harness.get_by_label("Clear search");
        assert!(
            !clear.rect().intersects(harness.get_by_label("Tree").rect()),
            "the trailing search control must not overlap the next toolbar control"
        );
        clear.click();
        harness.run();
        assert!(harness.state().0.is_empty(), "Clear must empty the query");
        assert!(
            harness.state().1,
            "the caller must know to reparse its filter"
        );
        assert!(
            harness
                .ctx
                .memory(|memory| memory.has_focus(search_box_id())),
            "clearing search must return typing focus to the field"
        );

        harness
            .input_mut()
            .events
            .push(egui::Event::Text("pid:4242".to_string()));
        harness.run();
        assert_eq!(
            harness.state().0,
            "pid:4242",
            "typing must continue after clearing"
        );
        harness.state_mut().1 = false;
        harness.key_press(egui::Key::Escape);
        harness.run();
        assert!(
            harness.state().0.is_empty(),
            "Escape must also clear the query"
        );
        assert!(
            harness.state().1,
            "Escape must notify the caller of the change"
        );
    }

    #[test]
    fn every_resize_grip_has_one_unambiguous_direction() {
        let window = Rect::from_min_size(egui::pos2(10.0, 20.0), Vec2::new(780.0, 620.0));
        let regions = resize_regions(window);
        let probes = [
            (
                window.left_top() + Vec2::splat(RESIZE_GRIP / 2.0),
                ResizeDirection::NorthWest,
            ),
            (
                window.right_top() + Vec2::new(-RESIZE_GRIP / 2.0, RESIZE_GRIP / 2.0),
                ResizeDirection::NorthEast,
            ),
            (
                window.left_bottom() + Vec2::new(RESIZE_GRIP / 2.0, -RESIZE_GRIP / 2.0),
                ResizeDirection::SouthWest,
            ),
            (
                window.right_bottom() - Vec2::splat(RESIZE_GRIP / 2.0),
                ResizeDirection::SouthEast,
            ),
            (
                egui::pos2(window.center().x, window.top() + 1.0),
                ResizeDirection::North,
            ),
            (
                egui::pos2(window.center().x, window.bottom() - 1.0),
                ResizeDirection::South,
            ),
            (
                egui::pos2(window.left() + 1.0, window.center().y),
                ResizeDirection::West,
            ),
            (
                egui::pos2(window.right() - 1.0, window.center().y),
                ResizeDirection::East,
            ),
        ];

        for (point, expected) in probes {
            let hits: Vec<ResizeDirection> = regions
                .iter()
                .filter(|(region, _, _)| region.contains(point))
                .map(|(_, direction, _)| *direction)
                .collect();
            assert_eq!(
                hits,
                vec![expected],
                "{point:?} must hit exactly one region"
            );
        }
    }

    #[test]
    fn diagonal_resize_targets_are_large_enough_to_hit() {
        let window = Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(780.0, 620.0));
        for (region, direction, _) in resize_regions(window).into_iter().take(4) {
            assert!(matches!(
                direction,
                ResizeDirection::NorthWest
                    | ResizeDirection::NorthEast
                    | ResizeDirection::SouthWest
                    | ResizeDirection::SouthEast
            ));
            assert_eq!(region.size(), Vec2::splat(RESIZE_CORNER));
            assert!(
                region.width() >= 16.0,
                "{direction:?} is still only a precision-sized target"
            );
        }
    }

    #[test]
    fn pressing_a_corner_immediately_requests_a_diagonal_native_resize() -> anyhow::Result<()> {
        let ctx = egui::Context::default();
        let window = Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(780.0, 620.0));
        let run = |events| {
            ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(window),
                    events,
                    focused: true,
                    ..Default::default()
                },
                |ui| resize_handles_in(ui, window),
            )
        };

        let start = window.right_bottom() - Vec2::splat(3.0);
        // Areas participate in hit testing from the frame after their first
        // layout, as they do in the live app after its first painted frame.
        let mut warmup = run(Vec::new());
        warmup.textures_delta.clear();
        let mut output = run(vec![
            egui::Event::PointerMoved(start),
            egui::Event::PointerButton {
                pos: start,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::default(),
            },
        ]);
        let commands = &output
            .viewport_output
            .get(&egui::ViewportId::ROOT)
            .ok_or_else(|| anyhow::anyhow!("egui produced no root viewport output"))?
            .commands;
        let requested =
            commands.contains(&ViewportCommand::BeginResize(ResizeDirection::SouthEast));
        let commands = format!("{commands:?}");
        output.textures_delta.clear();
        assert!(
            requested,
            "corner press produced {commands} instead of a diagonal resize"
        );
        Ok(())
    }

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

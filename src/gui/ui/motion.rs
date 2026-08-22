// ============================================================================
// Module:       gui::ui::motion
// Description:  The egui binding for crate::motion — the helpers that read and
//               advance an animation held in the Context.
//
// Dependencies: egui (the animation state lives in its Context); crate::motion
// ============================================================================

//! Animation, as egui holds it.
//!
//! [`crate::motion`] owns the curves, the four named durations, and
//! [`Tween`]; it has no egui in it and its tests run on any platform.
//! This module is the part that needs a `Context` — egui keeps a value
//! per [`egui::Id`] and advances it itself, which is what makes an
//! animation survive the immediate-mode redraw that has no memory of the
//! previous frame.
//!
//! **This is the only file in the app allowed to call `ctx.animate_*`.**
//! `no_drawing_module_animates_by_hand` in [`super`] fails the build for
//! a call anywhere else, because a call site that reaches past this
//! module is a call site that picked its own duration.
//!
//! ## Ids must name the thing, not its position
//!
//! Every helper here takes an [`egui::Id`], and it must be derived from
//! the *thing* being animated rather than from where it currently sits.
//! An id built from a loop index animates the position: delete the third
//! row of a list and every row below it inherits the animation state of
//! the row that used to be there, so the whole tail flashes. Derive ids
//! from a [`crate::model::ProcessKey`], a service name, a column — an
//! identity that survives the list being re-sorted.

pub use crate::motion::{ease_in_out, ease_out, spring, Tween, ENTER, INSTANT, QUICK, SETTLE};

use crate::motion::{ease_in_out as symmetric, ease_out as responsive};
use egui::{Context, Id, Ui};

/// How far a boolean animation has progressed, 0..=1, eased out.
///
/// The workhorse. `id` must be stable across frames for the same control
/// — an id derived from a loop index rather than from the thing being
/// drawn is how a list ends up animating the *position* rather than the
/// item, so that deleting a row makes every row below it flash.
#[must_use]
pub fn toggle(ctx: &Context, id: Id, on: bool, seconds: f32) -> f32 {
    responsive(ctx.animate_bool_with_time(id, on, seconds))
}

/// [`toggle`], for the common case of a hover.
#[must_use]
pub fn hover(ui: &Ui, id: Id, hovered: bool) -> f32 {
    toggle(ui.ctx(), id, hovered, INSTANT)
}

/// [`toggle`] with the symmetric curve, for a state change no pointer is
/// driving.
#[must_use]
pub fn transition(ctx: &Context, id: Id, on: bool, seconds: f32) -> f32 {
    symmetric(ctx.animate_bool_with_time(id, on, seconds))
}

/// A continuous value that chases its target over [`SETTLE`].
///
/// This is what stops a table of live numbers from flickering. See the
/// module docs.
///
/// Note that egui returns the *target* on the first observation of an id
/// rather than the start, which is deliberate and load-bearing: a row
/// scrolling into view shows its real value immediately instead of
/// animating up from zero, and a freshly opened window does not play a
/// three-hundred-row animation nobody asked for.
#[must_use]
pub fn value(ctx: &Context, id: Id, target: f32) -> f32 {
    settled(ctx, id, target, SETTLE)
}

/// [`value`] with an explicit duration.
#[must_use]
pub fn settled(ctx: &Context, id: Id, target: f32, seconds: f32) -> f32 {
    // A non-finite target would be stored and then returned forever,
    // because every later comparison against it is false — so the
    // animation would never converge and the control would be stuck.
    let target = if target.is_finite() { target } else { 0.0 };
    ctx.animate_value_with_time(id, target, seconds)
}

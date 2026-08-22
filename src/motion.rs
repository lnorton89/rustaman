// ============================================================================
// Module:       motion
// Description:  The easing curves, the four named durations, and the tween that
//               makes a value arrive rather than appear.
//
// Dependencies: none — deliberately. See the module docs.
// ============================================================================

//! Motion, in one place.
//!
//! ## Why a module rather than a call to `animate_bool` where it is needed
//!
//! egui already offers `Context::animate_bool_with_time`, and reaching for
//! it at each call site is the obvious thing to do. It is also how an app
//! ends up with a hover that fades over 0.1s beside a panel that slides
//! over 0.25s beside a value that snaps, with no one decision anywhere
//! that says what the app's motion is meant to feel like.
//!
//! So every animation goes through one place, and the durations are four
//! named constants rather than numbers at call sites.
//! `no_drawing_module_animates_by_hand` in [`crate::gui::ui`] fails the
//! build for a direct `ctx.animate_` call anywhere but the binding layer.
//!
//! ## Why the curves are here and not in `gui`
//!
//! Nothing in this file mentions egui, or a window, or Windows. An easing
//! curve is arithmetic and a tween is a number chasing another number, so
//! they live in the portable half of the crate where their tests run on
//! any machine — the same split as [`crate::theme`] (the palette and its
//! contrast rules) against `gui::ui::theme` (the palette as an egui
//! `Visuals`).
//!
//! That is not a stylistic preference. The properties worth pinning here
//! — that a curve never runs backwards, that a tween converges instead of
//! repainting forever, that a `NaN` cannot make a control vanish — are
//! exactly the ones a person cannot check by looking at the window, and
//! they are checked on every CI run rather than only on the Windows one.
//! [`crate::gui::ui::motion`] holds the part that does need a `Context`.
//!
//! ## The durations, and why there are only four
//!
//! Motion in an interface is not decoration; it is the answer to "what
//! just happened, and where did it come from". That gives each duration a
//! job, and four jobs is all this app has:
//!
//! - [`INSTANT`] — a hover, a press. The pointer is already there, so the
//!   ramp exists only to stop the change reading as a flicker. Anything
//!   longer feels like the control is lagging the pointer.
//! - [`QUICK`] — a selection moving, a row expanding, a chip appearing.
//!   Long enough to be seen and followed, short enough that a user
//!   clicking through five rows is never waiting on it.
//! - [`SETTLE`] — a number changing, a bar re-levelling, a graph taking a
//!   new sample. These are not responses to input at all: they are the
//!   machine's own state arriving, and they should read as something
//!   settling rather than something switching.
//! - [`ENTER`] — a whole view arriving. The one place a slightly longer
//!   move is right, because the thing that moved is the whole page and a
//!   fast one reads as a flash.
//!
//! A fifth duration would be one of these four with a different number,
//! which is exactly the drift this module exists to stop.
//!
//! ## The curves
//!
//! Nothing in this app moves linearly. A linear ramp decelerates nowhere,
//! and the eye is unusually good at noticing that — it reads as
//! mechanical, which is the specific quality that separates an interface
//! that feels made from one that feels generated.
//!
//! - [`ease_out`] for anything responding to input. It leaves immediately
//!   and settles gently, so the interface appears to react before the
//!   animation has finished.
//! - [`ease_in_out`] for anything moving between two resting states with
//!   no input driving it — a view transition, a panel.
//! - [`spring`] for a value that should feel physical: a meter finding
//!   its level, a drop indicator snapping to a slot. It overshoots
//!   slightly and comes back, which is what makes a moved object read as
//!   having mass.
//!
//! ## Numbers do not jump
//!
//! [`Tween`] carries a displayed value that chases its target. This
//! matters more here than it would in most apps: a task manager is
//! largely a grid of numbers that change once a second, and a grid of
//! numbers that *replace* themselves once a second is visually noisy in a
//! way that makes the whole window hard to read. The same numbers sliding
//! to their new values are legible while they move, and the movement
//! itself carries information — a column where everything is drifting up
//! is a machine getting busier, which no single frame can tell you.

/// A hover, a press — a response to a pointer that is already there.
pub const INSTANT: f32 = 0.12;

/// A selection, a disclosure, a chip arriving.
pub const QUICK: f32 = 0.18;

/// A value, a bar, a graph — the machine's own state arriving.
pub const SETTLE: f32 = 0.35;

/// A whole view arriving.
pub const ENTER: f32 = 0.22;

// The durations have to stay in their lanes, and these are relations
// between constants rather than facts about a running program.
const _: () = {
    assert!(
        INSTANT >= 0.06 && INSTANT <= 0.15,
        "under ~60ms a hover reads as a switch rather than a fade; over \
         ~150ms it reads as lag on a control the pointer has already left"
    );
    assert!(
        QUICK > INSTANT,
        "a selection that moves faster than a hover fades inverts the \
         hierarchy: the larger change should take the longer time"
    );
    assert!(
        SETTLE > QUICK,
        "the machine's own state should settle more slowly than the \
         user's own input resolves, or the app appears to be arguing \
         with the pointer"
    );
    assert!(
        ENTER <= SETTLE,
        "a view transition longer than a value settle makes navigation \
         feel heavier than the data it navigates"
    );
    assert!(
        ENTER <= 0.3,
        "past ~300ms a view transition stops reading as motion and \
         starts reading as a wait"
    );
};

/// An ease-out cubic: leaves fast, settles gently.
///
/// The default for anything responding to input, because the interface
/// appears to have reacted well before the animation finishes.
#[must_use]
pub fn ease_out(t: f32) -> f32 {
    let t = clamp01(t);
    1.0 - (1.0 - t).powi(3)
}

/// An ease-in-out cubic: gentle at both ends.
///
/// For a move between two resting states that no input is driving.
#[must_use]
pub fn ease_in_out(t: f32) -> f32 {
    let t = clamp01(t);
    if t < 0.5 {
        4.0 * t * t * t
    } else {
        1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
    }
}

/// A damped spring: overshoots slightly, then settles.
///
/// For a value that should read as having mass. The overshoot is small on
/// purpose — a bouncy interface is charming once and irritating by the
/// fiftieth time a user sees it, and this one is on screen continuously.
#[must_use]
pub fn spring(t: f32) -> f32 {
    /// How far past the target the value travels, as a fraction. About
    /// 4%: enough to read as a settle, small enough that a meter does
    /// not appear to report a number it never had.
    const OVERSHOOT: f32 = 1.7;

    let t = clamp01(t);
    if t >= 1.0 {
        return 1.0;
    }
    let t = t - 1.0;
    1.0 + (OVERSHOOT + 1.0) * t.powi(3) + OVERSHOOT * t.powi(2)
}

/// `t`, clamped, with a non-finite input treated as the start.
///
/// A `NaN` reaching a curve poisons the position it produces, which
/// poisons a rect, which egui then declines to draw — so one bad value
/// makes a control vanish rather than misplacing it. `f32::clamp`
/// propagates `NaN` rather than removing it, so this is explicit.
fn clamp01(t: f32) -> f32 {
    if t.is_nan() {
        0.0
    } else {
        t.clamp(0.0, 1.0)
    }
}

/// A value that chases its target, expressed as a struct.
///
/// Where [`value`] is right for something drawn from a rect and an id,
/// this is right for state an app struct owns and updates itself — the
/// scroll position of a panel, the width of a resizing column — because
/// it does not need an `egui::Context` to advance and can therefore be
/// tested without one.
#[derive(Clone, Copy, Debug)]
pub struct Tween {
    /// Where the value is now.
    current: f32,
    /// Where it is heading.
    target: f32,
    /// How long a full journey takes, in seconds.
    seconds: f32,
}

impl Tween {
    /// A tween already at `value`, so it does not animate into place on
    /// the frame it is created.
    #[must_use]
    pub fn new(value: f32, seconds: f32) -> Self {
        Self {
            current: value,
            target: value,
            seconds: seconds.max(f32::EPSILON),
        }
    }

    /// Points the tween at a new target, leaving it where it is.
    pub fn set(&mut self, target: f32) {
        if target.is_finite() {
            self.target = target;
        }
    }

    /// Jumps straight to `value`, cancelling any journey in progress.
    ///
    /// For a change that is not a change *of* the same thing — a
    /// different process selected into the same inspector, say. Animating
    /// between two unrelated values draws a path through numbers that
    /// were never true.
    pub fn snap(&mut self, value: f32) {
        if value.is_finite() {
            self.current = value;
            self.target = value;
        }
    }

    /// Advances by `delta` seconds and returns the new value.
    ///
    /// Frame-rate independent: the fraction covered is derived from the
    /// elapsed time rather than assumed per frame, so the same motion
    /// takes the same wall-clock time at 60Hz and at 144Hz. A fixed
    /// per-frame fraction is why some apps animate visibly faster on a
    /// high-refresh monitor.
    pub fn advance(&mut self, delta: f32) -> f32 {
        if !delta.is_finite() || delta <= 0.0 {
            return self.current;
        }
        let t = clamp01(delta / self.seconds);
        self.current += (self.target - self.current) * ease_out(t);
        // Snap the last fraction of a pixel. Without this the value
        // approaches its target asymptotically and never arrives, so egui
        // is asked to repaint forever for a change nobody can see.
        if (self.target - self.current).abs() < Self::EPSILON {
            self.current = self.target;
        }
        self.current
    }

    /// The value as it stands, without advancing it.
    #[must_use]
    pub fn get(self) -> f32 {
        self.current
    }

    /// Whether the tween has arrived.
    ///
    /// A frame that has no unfinished tween on it does not need to
    /// schedule a repaint, which is what lets an idle window drop to
    /// redrawing only when a sample arrives.
    #[must_use]
    pub fn finished(self) -> bool {
        (self.target - self.current).abs() < Self::EPSILON
    }

    /// Below this a difference is under a tenth of a pixel at any size
    /// this app draws, and chasing it costs a repaint per frame forever.
    const EPSILON: f32 = 0.01;
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Result;

    #[test]
    fn every_curve_runs_from_zero_to_one() -> Result<()> {
        for (name, curve) in [
            ("ease_out", ease_out as fn(f32) -> f32),
            ("ease_in_out", ease_in_out),
            ("spring", spring),
        ] {
            assert!(
                curve(0.0).abs() < 1e-5,
                "{name} starts at {}, not 0",
                curve(0.0)
            );
            assert!(
                (curve(1.0) - 1.0).abs() < 1e-5,
                "{name} ends at {}, not 1",
                curve(1.0)
            );
        }
        Ok(())
    }

    /// A curve that reverses would move a control backwards part-way
    /// through, which reads as a glitch rather than as easing. `spring`
    /// is exempt — overshooting is its whole job.
    #[test]
    fn the_eased_curves_never_move_backwards() -> Result<()> {
        for (name, curve) in [
            ("ease_out", ease_out as fn(f32) -> f32),
            ("ease_in_out", ease_in_out),
        ] {
            let mut previous = curve(0.0);
            for step in 1..=100 {
                let value = curve(step as f32 / 100.0);
                assert!(
                    value >= previous - 1e-6,
                    "{name} goes backwards at t={}: {value} after {previous}",
                    step as f32 / 100.0
                );
                previous = value;
            }
        }
        Ok(())
    }

    #[test]
    fn ease_out_leaves_faster_than_it_arrives() -> Result<()> {
        // The property that makes it read as responsive: more than half
        // the distance is covered in the first half of the time.
        assert!(
            ease_out(0.5) > 0.5,
            "ease_out is only {} of the way at half time, so it does not \
             ease out",
            ease_out(0.5)
        );
        Ok(())
    }

    #[test]
    fn spring_overshoots_and_comes_back() -> Result<()> {
        let peak = (0..=100)
            .map(|step| spring(step as f32 / 100.0))
            .fold(f32::MIN, f32::max);
        assert!(
            peak > 1.0,
            "spring never exceeds its target ({peak}), so it does not \
             read as having mass"
        );
        assert!(
            peak < 1.15,
            "spring overshoots by {:.0}%, which is charming once and \
             irritating by the fiftieth time",
            (peak - 1.0) * 100.0
        );
        Ok(())
    }

    #[test]
    fn a_curve_given_nonsense_starts_rather_than_vanishing() -> Result<()> {
        // A NaN reaching a curve poisons a position, which poisons a
        // rect, which egui declines to draw — so the control disappears
        // instead of being misplaced, which is much harder to diagnose.
        for (name, curve) in [
            ("ease_out", ease_out as fn(f32) -> f32),
            ("ease_in_out", ease_in_out),
            ("spring", spring),
        ] {
            assert!(curve(f32::NAN).is_finite(), "{name} passes NaN through");
            assert!(
                curve(f32::INFINITY).is_finite(),
                "{name} passes infinity through"
            );
        }
        Ok(())
    }

    #[test]
    fn a_tween_arrives_and_then_stops_asking_for_frames() -> Result<()> {
        let mut tween = Tween::new(0.0, QUICK);
        tween.set(100.0);
        assert!(!tween.finished(), "a tween with a new target has arrived");

        // Sixty frames at 60Hz is a full second — several times the
        // duration, so this is testing that it converges, not that it
        // converges quickly.
        for _ in 0..60 {
            tween.advance(1.0 / 60.0);
        }
        assert!(
            tween.finished(),
            "the tween is still at {} after a second, so the window would \
             repaint forever",
            tween.get()
        );
        assert!(
            (tween.get() - 100.0).abs() < 0.1,
            "the tween settled at {} rather than its target",
            tween.get()
        );
        Ok(())
    }

    #[test]
    fn a_tween_takes_the_same_time_at_any_refresh_rate() -> Result<()> {
        // A fixed per-frame fraction is why some apps animate visibly
        // faster on a 144Hz monitor. Two tweens, same wall-clock elapsed,
        // different frame counts.
        let mut sixty = Tween::new(0.0, SETTLE);
        let mut one_forty_four = Tween::new(0.0, SETTLE);
        sixty.set(1.0);
        one_forty_four.set(1.0);

        for _ in 0..30 {
            sixty.advance(1.0 / 60.0);
        }
        for _ in 0..72 {
            one_forty_four.advance(1.0 / 144.0);
        }

        let difference = (sixty.get() - one_forty_four.get()).abs();
        assert!(
            difference < 0.05,
            "after the same half second the 60Hz tween is at {} and the \
             144Hz one at {}",
            sixty.get(),
            one_forty_four.get()
        );
        Ok(())
    }

    #[test]
    fn a_tween_ignores_nonsense_rather_than_getting_stuck_on_it() -> Result<()> {
        let mut tween = Tween::new(5.0, QUICK);
        tween.set(f32::NAN);
        tween.advance(1.0 / 60.0);
        assert!(
            tween.get().is_finite(),
            "a NaN target poisoned the tween, so the control it drives \
             would stop being drawn at all"
        );

        // A backwards or zero delta happens for real: a frame where the
        // clock did not advance, or a paused window.
        let before = tween.get();
        tween.advance(-1.0);
        assert!(
            (tween.get() - before).abs() < f32::EPSILON,
            "a negative delta moved the tween"
        );
        Ok(())
    }

    #[test]
    fn snapping_cancels_the_journey_rather_than_redirecting_it() -> Result<()> {
        // A different process selected into the same inspector is not a
        // change *of* the same value: animating between them draws a path
        // through numbers that were never true.
        let mut tween = Tween::new(0.0, SETTLE);
        tween.set(1000.0);
        tween.advance(1.0 / 60.0);
        tween.snap(7.0);
        assert!(
            (tween.get() - 7.0).abs() < f32::EPSILON && tween.finished(),
            "snap left the tween at {} and still travelling",
            tween.get()
        );
        Ok(())
    }
}

// ============================================================================
// Module:       gui::ui::dnd
// Description:  Dragging an item to a new place in a list — the state, the drop
//               indicator, the ghost that follows the pointer.
//
// Dependencies: egui; super::{icon, motion, theme}
// ============================================================================

//! Drag and drop, once.
//!
//! One [`Lane`] handles every reorderable list in the app: the table
//! headers, and anything added later. Not because the code is long, but
//! because drag and drop is almost entirely *feedback*, and feedback done
//! twice is done differently twice.
//!
//! ## What a reorder has to tell the user, and when
//!
//! An implementation that only moves the item on release is technically a
//! reorder and feels broken, because between press and release the user
//! has no idea whether anything is happening. Four things have to be
//! visible, and this module is what makes them consistent:
//!
//! - **That a drag has started.** The source slot dims, so the item reads
//!   as lifted out rather than duplicated.
//! - **What is being dragged.** A ghost follows the pointer, carrying the
//!   item's own label. Without it the user is dragging an invisible
//!   thing and watching an indicator move.
//! - **Where it will land.** A single accent bar at the boundary it would
//!   drop into. It *slides* between boundaries rather than jumping, which
//!   is what makes the target read as one marker moving rather than as a
//!   series of unrelated flashes.
//! - **That releasing here does nothing.** Dropping outside the lane
//!   leaves everything where it was, and the indicator disappears while
//!   the pointer is outside so that is visible before the user commits.
//!
//! ## Immediate mode makes this awkward, and here is the shape that works
//!
//! There is no retained widget tree to ask "which item is under the
//! pointer", and the items are drawn inside a closure that has already
//! finished by the time the answer is needed. So a lane is filled in
//! during drawing and resolved afterwards:
//!
//! ```text
//! let mut lane = Lane::new(id, Axis::Horizontal);
//! for each item:
//!     lane.item(index, rect, label, &response);   // while drawing
//! if let Some(moved) = lane.show(ui, &theme) {    // after
//!     order.move_column(moved.from, moved.to);
//! }
//! ```
//!
//! The in-flight drag lives in egui's own memory rather than in `App`,
//! because it is interaction state with no meaning between frames and
//! nothing outside this module should be able to observe a half-finished
//! drag.
//!
//! ## The slot index is a boundary, not an item
//!
//! The pointer is resolved to the *gap* it is nearest, not the item it is
//! over. Resolving to an item gives a dead zone in the middle of each one
//! where the target does not change, and an off-by-one at the end of the
//! list that makes the last position unreachable — you can drop before
//! the final item but never after it.

use super::theme::{self, RADIUS, SPACE_SM, SPACE_XS};
use crate::theme::Palette;
use egui::{Align2, Id, Rect, Response, Sense, TextStyle, Ui, Vec2};

/// Which way a lane runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    /// A row of items — a table's headings.
    Horizontal,
    /// A column of items — a list.
    Vertical,
}

/// A completed reorder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Moved {
    /// Where the item was.
    pub from: usize,
    /// Where it was dropped.
    pub to: usize,
}

/// The drag in flight, as egui remembers it between frames.
#[derive(Clone, Debug)]
struct InFlight {
    /// Which item was picked up.
    from: usize,
    /// Its label, for the ghost. Carried rather than looked up because
    /// the caller's list may have re-sorted under us between frames.
    label: String,
}

/// One reorderable list, for one frame.
pub struct Lane {
    /// Identifies this lane across frames.
    id: Id,
    /// Which way it runs.
    axis: Axis,
    /// Each item's index and rect, in drawn order.
    items: Vec<(usize, Rect)>,
    /// The item a drag started on this frame, and its label.
    started: Option<(usize, String)>,
    /// Whether a drag ended this frame.
    released: bool,
}

impl Lane {
    /// Begins collecting a lane's items for this frame.
    #[must_use]
    pub fn new(id: Id, axis: Axis) -> Self {
        Self {
            id,
            axis,
            items: Vec::new(),
            started: None,
            released: false,
        }
    }

    /// The sense an item must be given for a lane to work.
    ///
    /// Exposed so a call site cannot pass `Sense::click()` and then spend
    /// an afternoon wondering why nothing drags.
    #[must_use]
    pub fn sense() -> Sense {
        Sense::click_and_drag()
    }

    /// Records one item as it is drawn.
    pub fn item(&mut self, index: usize, rect: Rect, label: &str, response: &Response) {
        self.items.push((index, rect));
        if response.drag_started() {
            self.started = Some((index, label.to_owned()));
        }
        if response.drag_stopped() {
            self.released = true;
        }
    }

    /// Whether `index` is the item currently being dragged.
    ///
    /// Call sites use it to dim the source slot. It reads the stored
    /// state rather than this frame's, so it is correct on every frame of
    /// the drag rather than only the one it started on.
    #[must_use]
    pub fn is_dragging(&self, ui: &Ui, index: usize) -> bool {
        self.in_flight(ui).is_some_and(|drag| drag.from == index)
    }

    /// Resolves the drag, paints the feedback, and reports a completed
    /// move.
    ///
    /// Call once, after every item has been recorded.
    pub fn show(self, ui: &Ui, theme: &Palette) -> Option<Moved> {
        // A drag that started this frame is stored first, so the ghost
        // and the indicator appear on the same frame the pointer went
        // down rather than one frame later — a single frame of nothing
        // reads as the control having missed the click.
        if let Some((from, label)) = self.started.clone() {
            ui.ctx().data_mut(|data| {
                data.insert_temp(self.id, InFlight { from, label });
            });
        }

        let drag = self.in_flight(ui)?;

        // Releasing anywhere ends the drag, whether or not it lands
        // somewhere useful. A drag that survived a release outside the
        // lane would leave a ghost stuck to the pointer.
        if self.released {
            ui.ctx().data_mut(|data| data.remove::<InFlight>(self.id));
        }

        let pointer = ui.ctx().pointer_interact_pos();
        let slot = pointer.and_then(|position| self.slot_for(position));

        // While the pointer is outside the lane there is no target, so
        // nothing is painted and a release does nothing — which is what
        // makes "this drop will be ignored" visible before the user
        // commits to it.
        let target = slot?;

        if self.released {
            let to = crate::model::columns::landing(drag.from, target);
            return (to != drag.from).then_some(Moved {
                from: drag.from,
                to,
            });
        }

        self.paint_indicator(ui, theme, target);
        if let Some(position) = pointer {
            self.paint_ghost(ui, theme, position, &drag.label);
        }
        None
    }

    /// The drag this lane has in flight, if any.
    fn in_flight(&self, ui: &Ui) -> Option<InFlight> {
        ui.ctx().data(|data| data.get_temp::<InFlight>(self.id))
    }

    /// The boundary index nearest `position`, or `None` if the pointer is
    /// outside the lane.
    ///
    /// Boundaries, not items — see the module docs on why resolving to an
    /// item makes the last position unreachable. For `n` items there are
    /// `n + 1` boundaries: before the first, between each pair, and after
    /// the last.
    fn slot_for(&self, position: egui::Pos2) -> Option<usize> {
        let bounds = self.bounds()?;
        if !bounds.expand(Self::CATCH).contains(position) {
            return None;
        }
        let along = match self.axis {
            Axis::Horizontal => position.x,
            Axis::Vertical => position.y,
        };
        // The first boundary the pointer has not yet passed the midpoint
        // of. Items are recorded in drawn order, so this is a scan rather
        // than a search — a table has a dozen columns.
        for (slot, (_, rect)) in self.items.iter().enumerate() {
            let middle = match self.axis {
                Axis::Horizontal => rect.center().x,
                Axis::Vertical => rect.center().y,
            };
            if along < middle {
                return Some(slot);
            }
        }
        Some(self.items.len())
    }

    /// The rect covering every item in the lane.
    fn bounds(&self) -> Option<Rect> {
        let mut items = self.items.iter();
        let (_, first) = items.next()?;
        Some(items.fold(*first, |bounds, (_, rect)| bounds.union(*rect)))
    }

    /// How far outside the lane a pointer still counts as over it.
    ///
    /// Without some slack the indicator flickers off whenever the pointer
    /// strays a pixel above a heading it is dragging along, which reads
    /// as the drag being dropped and picked up again.
    const CATCH: f32 = 12.0;

    /// Paints the accent bar at the boundary the item would drop into.
    fn paint_indicator(&self, ui: &Ui, theme: &Palette, target: usize) {
        /// The bar's thickness.
        const THICKNESS: f32 = 2.0;

        let Some(bounds) = self.bounds() else {
            return;
        };
        let edge = self.boundary_position(target);

        // The bar slides between boundaries rather than jumping, so the
        // target reads as one marker travelling along the lane. A spring
        // rather than an ease: it should feel like the marker is being
        // caught by each slot.
        let travelled = super::motion::settled(
            ui.ctx(),
            self.id.with("indicator"),
            edge,
            super::motion::QUICK,
        );

        let rect = match self.axis {
            Axis::Horizontal => Rect::from_min_max(
                egui::pos2(travelled - THICKNESS / 2.0, bounds.top()),
                egui::pos2(travelled + THICKNESS / 2.0, bounds.bottom()),
            ),
            Axis::Vertical => Rect::from_min_max(
                egui::pos2(bounds.left(), travelled - THICKNESS / 2.0),
                egui::pos2(bounds.right(), travelled + THICKNESS / 2.0),
            ),
        };
        ui.painter()
            .rect_filled(rect, egui::CornerRadius::same(1), theme::rgb(theme.accent));
    }

    /// Where boundary `target` sits, in screen coordinates.
    fn boundary_position(&self, target: usize) -> f32 {
        let leading = |rect: &Rect| match self.axis {
            Axis::Horizontal => rect.left(),
            Axis::Vertical => rect.top(),
        };
        let trailing = |rect: &Rect| match self.axis {
            Axis::Horizontal => rect.right(),
            Axis::Vertical => rect.bottom(),
        };
        match self.items.get(target) {
            Some((_, rect)) => leading(rect),
            // Past the last item: its trailing edge.
            None => self.items.last().map_or(0.0, |(_, rect)| trailing(rect)),
        }
    }

    /// Paints the label following the pointer.
    fn paint_ghost(&self, ui: &Ui, theme: &Palette, pointer: egui::Pos2, label: &str) {
        /// How far above the pointer the ghost floats.
        ///
        /// Offset rather than centred: a ghost under the pointer covers
        /// the drop indicator at exactly the moment the user is trying to
        /// read it.
        const LIFT: f32 = 18.0;

        let font = TextStyle::Small.resolve(ui.style());
        let galley =
            ui.painter()
                .layout_no_wrap(label.to_owned(), font, theme::rgb(theme.text_on_accent));
        let size = galley.size() + Vec2::new(SPACE_SM * 2.0, SPACE_XS * 2.0);
        let rect = Rect::from_center_size(pointer - Vec2::new(0.0, LIFT), size);

        // On the tooltip layer, so it is painted above everything already
        // drawn this frame rather than under the next widget.
        let painter = ui.ctx().layer_painter(egui::LayerId::new(
            egui::Order::Tooltip,
            self.id.with("ghost"),
        ));
        painter.rect_filled(
            rect,
            egui::CornerRadius::same(RADIUS),
            theme::rgb(theme.accent),
        );
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            label,
            TextStyle::Small.resolve(ui.style()),
            theme::rgb(theme.text_on_accent),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Result;

    /// A lane of five items, each 100 wide, starting at x = 0.
    fn lane() -> Lane {
        let mut lane = Lane::new(Id::new("test"), Axis::Horizontal);
        for index in 0..5 {
            let left = index as f32 * 100.0;
            lane.items.push((
                index,
                Rect::from_min_size(egui::pos2(left, 0.0), Vec2::new(100.0, 20.0)),
            ));
        }
        lane
    }

    #[test]
    fn the_pointer_resolves_to_the_nearest_boundary() -> Result<()> {
        let lane = lane();
        // Left of the first item's midpoint: drop before it.
        assert_eq!(lane.slot_for(egui::pos2(10.0, 10.0)), Some(0));
        // Just past the first midpoint: drop between the first and second.
        assert_eq!(lane.slot_for(egui::pos2(60.0, 10.0)), Some(1));
        // Past the last midpoint: drop after everything.
        assert_eq!(
            lane.slot_for(egui::pos2(490.0, 10.0)),
            Some(5),
            "the position after the last item must be reachable, or the \
             last slot in the list can never be dropped into"
        );
        Ok(())
    }

    #[test]
    fn a_pointer_outside_the_lane_has_no_target() -> Result<()> {
        let lane = lane();
        assert_eq!(
            lane.slot_for(egui::pos2(-200.0, 10.0)),
            None,
            "a drag well outside the lane should offer no drop target, so \
             that releasing there visibly does nothing"
        );
        assert_eq!(lane.slot_for(egui::pos2(250.0, 400.0)), None);
        Ok(())
    }

    #[test]
    fn a_pointer_just_outside_still_counts_as_over_the_lane() -> Result<()> {
        // Without slack the indicator flickers off whenever the pointer
        // strays a pixel above the heading it is dragging along, which
        // reads as the drag being dropped and picked up again.
        let lane = lane();
        assert!(
            lane.slot_for(egui::pos2(250.0, -6.0)).is_some(),
            "a pointer six points above the lane lost its drop target"
        );
        Ok(())
    }

    #[test]
    fn the_boundary_positions_run_along_the_lane_in_order() -> Result<()> {
        // A misordered boundary would send the drop indicator backwards
        // as the pointer moved forwards.
        let lane = lane();
        let mut previous = f32::MIN;
        for target in 0..=lane.items.len() {
            let position = lane.boundary_position(target);
            assert!(
                position > previous,
                "boundary {target} sits at {position}, behind the one \
                 before it at {previous}"
            );
            previous = position;
        }
        Ok(())
    }

    #[test]
    fn an_empty_lane_offers_no_target_rather_than_panicking() -> Result<()> {
        let lane = Lane::new(Id::new("empty"), Axis::Vertical);
        assert!(lane.bounds().is_none());
        assert_eq!(lane.slot_for(egui::pos2(0.0, 0.0)), None);
        Ok(())
    }
}

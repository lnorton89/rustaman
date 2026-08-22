// ============================================================================
// Module:       model::columns
// Description:  A user's column order, and reconciling a saved one against the
//               set of columns the running build actually has.
//
// Dependencies: serde (it is persisted); super::sort::SortKey
// ============================================================================

//! The order the columns are in.
//!
//! A [`ColumnOrder`] is a permutation of a table's columns, dragged into
//! place by the user and written to the config file. The interesting part
//! is not the permutation — it is what happens when a saved one meets a
//! build whose column set has changed.
//!
//! ## Reconciling is the whole job
//!
//! The naive version stores a list and reads it back. Then the next
//! release adds a GPU column, and every existing user's table silently
//! loses it: their saved order does not mention it, so it is not drawn,
//! and nothing anywhere reports a problem. The user's conclusion is that
//! the feature was not shipped.
//!
//! The mirror image is a column *removed*: a saved order still names it,
//! and a table that trusts its saved order draws a heading for a column
//! that no longer exists — or, worse, indexes an array with it.
//!
//! So a saved order is never used directly. [`ColumnOrder::reconcile`]
//! treats it as a *preference* to be applied to the real set:
//!
//! - Columns the build has and the saved order names keep the saved
//!   relative order.
//! - Columns the build has that the saved order does not mention are
//!   **appended**, so a new column appears rather than vanishing.
//! - Names in the saved order the build does not have are dropped.
//! - Duplicates are collapsed to their first occurrence.
//!
//! A hand-edited config is the other reason all four cases are handled
//! rather than assumed away: the file is text, people edit it, and a
//! typo should cost that line rather than the table.
//!
//! ## Why this is portable
//!
//! There is no egui here and no Windows. It is a permutation and a set
//! reconciliation, which means the four cases above are pinned by tests
//! that run on any machine — and they are exactly the cases that are
//! invisible until someone upgrades.

use super::sort::SortKey;
use serde::{Deserialize, Serialize};

/// The order a table's columns are drawn in.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ColumnOrder {
    /// The columns, in the order they are drawn.
    columns: Vec<SortKey>,
}

impl ColumnOrder {
    /// An order that is exactly `defaults`.
    #[must_use]
    pub fn new(defaults: &[SortKey]) -> Self {
        Self {
            columns: defaults.to_vec(),
        }
    }

    /// A saved order, applied to the columns this build actually has.
    ///
    /// See the module docs: the saved list is a preference, not an
    /// instruction. Anything `defaults` contains ends up in the result
    /// exactly once, and nothing else does.
    #[must_use]
    pub fn reconcile(saved: &[SortKey], defaults: &[SortKey]) -> Self {
        let mut columns = Vec::with_capacity(defaults.len());
        for key in saved {
            // `contains` on a slice this short beats building a set: a
            // table has a dozen columns, and the allocation would cost
            // more than the scan.
            if defaults.contains(key) && !columns.contains(key) {
                columns.push(*key);
            }
        }
        // Whatever the saved order did not mention, in the build's own
        // order. This is the case that makes a column added in a later
        // release show up for an existing user rather than silently
        // going missing.
        for key in defaults {
            if !columns.contains(key) {
                columns.push(*key);
            }
        }
        Self { columns }
    }

    /// The columns, in order.
    #[must_use]
    pub fn as_slice(&self) -> &[SortKey] {
        &self.columns
    }

    /// How many columns there are.
    #[must_use]
    pub fn len(&self) -> usize {
        self.columns.len()
    }

    /// Whether there are no columns at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.columns.is_empty()
    }

    /// The column at `index`, if there is one.
    #[must_use]
    pub fn get(&self, index: usize) -> Option<SortKey> {
        self.columns.get(index).copied()
    }

    /// Moves the column at `from` so that it sits at `to`.
    ///
    /// Out-of-range indices are ignored rather than clamped. Clamping
    /// looks tidier and is wrong: a drag that ended outside the table
    /// would silently move the column to whichever end was nearest,
    /// which is a reorder the user did not ask for and cannot undo.
    /// Doing nothing is the behaviour they expect from dropping
    /// something in the wrong place.
    pub fn move_column(&mut self, from: usize, to: usize) {
        if from >= self.columns.len() || to >= self.columns.len() || from == to {
            return;
        }
        let key = self.columns.remove(from);
        self.columns.insert(to, key);
    }

    /// Restores the build's own order.
    pub fn reset(&mut self, defaults: &[SortKey]) {
        self.columns = defaults.to_vec();
    }

    /// Whether this is still the build's own order.
    ///
    /// The Settings view uses it to decide whether a "reset the columns"
    /// control is worth offering — a control that is present but does
    /// nothing is worse than one that is absent.
    #[must_use]
    pub fn is_default(&self, defaults: &[SortKey]) -> bool {
        self.columns == defaults
    }
}

/// Where an item dragged from `from` lands when dropped at `boundary`.
///
/// A reorder interaction resolves the pointer to the *gap* it is nearest
/// rather than to the item it is over — resolving to an item leaves a
/// dead zone in the middle of each one, and makes the position after the
/// last item unreachable, so a column can be dropped before the final
/// column but never after it. For `n` items there are `n + 1` gaps.
///
/// Converting a gap back to an index is the off-by-one every reorder gets
/// wrong once, and it is worth stating why: gap indices count the list
/// *as it stands*, with the dragged item still in it. Removing that item
/// first shifts everything after it down by one, so a gap past the item's
/// own position lands one place earlier than its number suggests.
///
/// It lives here, rather than with the drag interaction in
/// [`crate::gui::ui::dnd`], because it is integer arithmetic with no egui
/// in it — which means it is checked on every platform rather than only
/// on the Windows CI job, and it is precisely the part where being wrong
/// produces a plausible-looking reorder that is off by one column.
#[must_use]
pub fn landing(from: usize, boundary: usize) -> usize {
    if boundary > from {
        boundary - 1
    } else {
        boundary
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Result;

    /// A plausible column set, in the order a build declares it.
    const DEFAULTS: [SortKey; 5] = [
        SortKey::Name,
        SortKey::Pid,
        SortKey::Cpu,
        SortKey::Memory,
        SortKey::Disk,
    ];

    #[test]
    fn a_saved_order_is_honoured() -> Result<()> {
        let saved = [
            SortKey::Cpu,
            SortKey::Name,
            SortKey::Disk,
            SortKey::Memory,
            SortKey::Pid,
        ];
        let order = ColumnOrder::reconcile(&saved, &DEFAULTS);
        assert_eq!(
            order.as_slice(),
            saved,
            "a saved order naming exactly the build's columns should be \
             used as it stands"
        );
        Ok(())
    }

    #[test]
    fn a_column_added_by_a_later_build_is_appended_rather_than_lost() -> Result<()> {
        // The case that matters. A user upgrades; their saved order was
        // written before the GPU column existed. Trusting the saved list
        // would draw four columns and the user would conclude the
        // feature was never shipped.
        let saved = [SortKey::Name, SortKey::Pid, SortKey::Cpu, SortKey::Memory];
        let order = ColumnOrder::reconcile(&saved, &DEFAULTS);
        assert_eq!(
            order.len(),
            DEFAULTS.len(),
            "reconciling dropped a column the build has: {:?}",
            order.as_slice()
        );
        assert_eq!(
            order.get(4),
            Some(SortKey::Disk),
            "the unmentioned column should be appended, not inserted \
             somewhere in the middle of an order the user arranged"
        );
        Ok(())
    }

    #[test]
    fn a_column_a_later_build_removed_is_dropped() -> Result<()> {
        // The mirror image: a saved order naming a column that no longer
        // exists. A table that trusted it would draw a heading for a
        // column with no data behind it.
        let saved = [
            SortKey::Name,
            SortKey::Path,
            SortKey::Pid,
            SortKey::Cpu,
            SortKey::Memory,
            SortKey::Disk,
        ];
        let order = ColumnOrder::reconcile(&saved, &DEFAULTS);
        assert!(
            !order.as_slice().contains(&SortKey::Path),
            "a column the build does not have survived reconciliation"
        );
        assert_eq!(order.len(), DEFAULTS.len());
        Ok(())
    }

    #[test]
    fn a_hand_edited_duplicate_does_not_draw_a_column_twice() -> Result<()> {
        // The config file is text and people edit it. A duplicated entry
        // would otherwise draw one column twice and push another off the
        // end.
        let saved = [
            SortKey::Cpu,
            SortKey::Cpu,
            SortKey::Name,
            SortKey::Cpu,
            SortKey::Pid,
        ];
        let order = ColumnOrder::reconcile(&saved, &DEFAULTS);
        assert_eq!(order.len(), DEFAULTS.len());
        for key in DEFAULTS {
            assert_eq!(
                order.as_slice().iter().filter(|&&k| k == key).count(),
                1,
                "{key:?} appears more than once in {:?}",
                order.as_slice()
            );
        }
        Ok(())
    }

    #[test]
    fn an_empty_or_nonsense_saved_order_falls_back_to_the_build_order() -> Result<()> {
        let order = ColumnOrder::reconcile(&[], &DEFAULTS);
        assert_eq!(
            order.as_slice(),
            DEFAULTS,
            "an empty saved order should leave the table looking like a \
             fresh install, not like a table with no columns"
        );

        let nonsense = [SortKey::Path, SortKey::Threads, SortKey::Handles];
        let order = ColumnOrder::reconcile(&nonsense, &DEFAULTS);
        assert_eq!(order.as_slice(), DEFAULTS);
        Ok(())
    }

    #[test]
    fn reconciling_is_idempotent() -> Result<()> {
        // Load, save, load again. If this were not stable the config file
        // would churn on every run, which is both a diff nobody asked for
        // and a sign the merge is losing information.
        let saved = [SortKey::Disk, SortKey::Name];
        let once = ColumnOrder::reconcile(&saved, &DEFAULTS);
        let twice = ColumnOrder::reconcile(once.as_slice(), &DEFAULTS);
        assert_eq!(once, twice, "reconciling twice gave a different answer");
        Ok(())
    }

    #[test]
    fn moving_a_column_forwards_and_backwards_lands_where_it_was_dropped() -> Result<()> {
        let mut order = ColumnOrder::new(&DEFAULTS);
        // Name to the end.
        order.move_column(0, 4);
        assert_eq!(
            order.as_slice(),
            [
                SortKey::Pid,
                SortKey::Cpu,
                SortKey::Memory,
                SortKey::Disk,
                SortKey::Name
            ]
        );
        // And back to the front.
        order.move_column(4, 0);
        assert_eq!(order.as_slice(), DEFAULTS);
        Ok(())
    }

    #[test]
    fn a_drop_outside_the_table_moves_nothing() -> Result<()> {
        // Clamping an out-of-range index looks tidier and is wrong: it
        // turns "I dropped this in the wrong place" into a reorder the
        // user did not ask for.
        let mut order = ColumnOrder::new(&DEFAULTS);
        order.move_column(0, 99);
        assert_eq!(
            order.as_slice(),
            DEFAULTS,
            "an out-of-range drop moved a column"
        );
        order.move_column(99, 0);
        assert_eq!(order.as_slice(), DEFAULTS);
        order.move_column(2, 2);
        assert_eq!(
            order.as_slice(),
            DEFAULTS,
            "a no-op move disturbed the order"
        );
        Ok(())
    }

    #[test]
    fn a_reordered_table_still_holds_every_column_exactly_once() -> Result<()> {
        // The invariant a reorder must never break, checked across every
        // move the interaction can produce rather than one example.
        for from in 0..DEFAULTS.len() {
            for to in 0..DEFAULTS.len() {
                let mut order = ColumnOrder::new(&DEFAULTS);
                order.move_column(from, to);
                assert_eq!(
                    order.len(),
                    DEFAULTS.len(),
                    "moving {from} to {to} changed the column count"
                );
                for key in DEFAULTS {
                    assert!(
                        order.as_slice().contains(&key),
                        "moving {from} to {to} lost {key:?}"
                    );
                }
            }
        }
        Ok(())
    }

    #[test]
    fn dropping_past_the_source_accounts_for_the_item_leaving_its_slot() -> Result<()> {
        // Item 0 dropped at the gap after item 2 (gap 3) ends up at index
        // 2, not 3 — removing it first shifts everything down by one.
        assert_eq!(landing(0, 3), 2);
        // Moving backwards, no shift happens.
        assert_eq!(landing(4, 1), 1);
        // Either gap adjacent to its own position is a no-op.
        assert_eq!(landing(2, 2), 2);
        assert_eq!(landing(2, 3), 2);
        Ok(())
    }

    #[test]
    fn every_drop_position_is_reachable_and_inside_the_list() -> Result<()> {
        // Across every source and every gap: the landing index must be
        // inside the list, and every position must be reachable from
        // some gap — or there is a place a column simply cannot be
        // dragged to, which is the kind of fault a person blames
        // themselves for.
        for from in 0..DEFAULTS.len() {
            let mut reachable = std::collections::HashSet::new();
            for boundary in 0..=DEFAULTS.len() {
                let to = landing(from, boundary);
                assert!(
                    to < DEFAULTS.len(),
                    "dropping item {from} at gap {boundary} lands at {to}, \
                     past the end of a {}-item list",
                    DEFAULTS.len()
                );
                reachable.insert(to);
            }
            assert_eq!(
                reachable.len(),
                DEFAULTS.len(),
                "item {from} can only reach {} of {} positions",
                reachable.len(),
                DEFAULTS.len()
            );
        }
        Ok(())
    }

    #[test]
    fn dropping_a_column_lands_it_where_the_pointer_was() -> Result<()> {
        // The end-to-end version: gap 3 is between Cpu and Memory, so
        // Name dropped there should sit between them.
        let mut order = ColumnOrder::new(&DEFAULTS);
        order.move_column(0, landing(0, 3));
        assert_eq!(
            order.as_slice(),
            [
                SortKey::Pid,
                SortKey::Cpu,
                SortKey::Name,
                SortKey::Memory,
                SortKey::Disk
            ],
            "a column dropped between two others did not land between them"
        );
        Ok(())
    }

    #[test]
    fn resetting_restores_the_build_order() -> Result<()> {
        let mut order = ColumnOrder::new(&DEFAULTS);
        order.move_column(0, 3);
        assert!(!order.is_default(&DEFAULTS));
        order.reset(&DEFAULTS);
        assert!(
            order.is_default(&DEFAULTS),
            "reset left the order at {:?}",
            order.as_slice()
        );
        Ok(())
    }
}

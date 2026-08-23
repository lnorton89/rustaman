// ============================================================================
// Module:       gui::app::rows
// Description:  The cached flattened row list, and the key that decides when it
//               has to be rebuilt.
//
// Dependencies: crate::model (tree, sort, filter)
// ============================================================================

//! Keeping tree-sized work out of the draw path.
//!
//! `ui::draw` runs in full every frame. Sorting four hundred processes,
//! filtering them, building the parent/child forest and flattening it is
//! O(n log n) work that the data justifies **once per sample** — once a
//! second by default — and never sixty times a second.
//!
//! [`Cache`] holds the flattened result and a [`RowKey`] describing the
//! state it was built from. [`Cache::refresh`] compares the key and
//! rebuilds only on a change.
//!
//! ## The key is the whole mechanism
//!
//! The cache is keyed off *observed state* rather than invalidated by
//! hand. Hand invalidation means every place that changes a sort order,
//! a filter, or an expansion has to remember to call something — and the
//! failure mode when one forgets is a table that silently stops
//! responding to a control, which looks like the control is broken.
//!
//! **If you add a field that affects which rows are shown or their order,
//! add it to [`RowKey`].** `a_row_key_covers_every_input_to_the_layout`
//! is the test that argues for it, but a test cannot know about a field
//! that does not exist yet — this comment is the real safeguard.

use crate::model::filter::Query;
use crate::model::sort::SortKey;
use crate::model::tree::{Entry, Forest, Layout};
use crate::model::{ProcessKey, ProcessKind, ProcessRow};
use std::collections::HashSet;
use std::sync::Arc;

/// Everything the flattened row list depends on.
///
/// Compared each frame; a change rebuilds. `PartialEq` is derived so a
/// new field is included in the comparison automatically once it is
/// added to the struct — the only step that cannot be automated is
/// remembering to add it.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct RowKey {
    /// The snapshot this was built from. Bumping this alone rebuilds
    /// every frame a new sample arrives, which is the common case.
    pub sequence: u64,
    /// The sorted column.
    pub sort: SortKey,
    /// The sort direction.
    pub descending: bool,
    /// Whether the tree and its category headings are drawn.
    pub grouped: bool,
    /// The search text, as typed. Compared as a string rather than as a
    /// parsed [`Query`] because the parse is cheap and a `Query` would
    /// need its own `Eq` over a `Vec` of terms.
    pub search: String,
    /// Which rows are expanded.
    pub expanded: Vec<ProcessKey>,
    /// Which categories are collapsed.
    pub collapsed: Vec<ProcessKind>,
}

impl RowKey {
    /// Builds a key from the pieces of state the layout reads.
    ///
    /// The two `HashSet`s are sorted into `Vec`s: a `HashSet`'s iteration
    /// order is not stable, so comparing two of them by their iteration
    /// would report a change on a frame where nothing changed and rebuild
    /// the whole list every frame — which is precisely the cost this
    /// cache exists to avoid, arrived at by accident.
    #[must_use]
    pub fn new(
        sequence: u64,
        sort: SortKey,
        descending: bool,
        grouped: bool,
        search: &str,
        expanded: &HashSet<ProcessKey>,
        collapsed: &HashSet<ProcessKind>,
    ) -> Self {
        let mut expanded: Vec<ProcessKey> = expanded.iter().copied().collect();
        expanded.sort_unstable();
        let mut collapsed: Vec<ProcessKind> = collapsed.iter().copied().collect();
        collapsed.sort_unstable();
        Self {
            sequence,
            sort,
            descending,
            grouped,
            search: search.to_string(),
            expanded,
            collapsed,
        }
    }
}

/// The flattened rows, and the state they were built from.
#[derive(Default)]
pub struct Cache {
    /// The rows to draw.
    entries: Arc<[Entry]>,
    /// The forest they were flattened from, kept so a click on a
    /// disclosure triangle or an "end process tree" can walk it without
    /// rebuilding.
    forest: Forest,
    /// The state the rows were built from, or `None` before the first
    /// build.
    key: Option<RowKey>,
    /// How many rows the filter matched, for the "N of M" readout. Zero
    /// when there is no filter.
    matched: usize,
}

impl Cache {
    /// Rebuilds the rows if anything they depend on has changed.
    ///
    /// Returns whether a rebuild happened, which the caller uses to know
    /// when a cached scroll position is no longer meaningful.
    pub fn refresh(
        &mut self,
        rows: &[ProcessRow],
        query: &Query,
        key: RowKey,
        expanded: &HashSet<ProcessKey>,
        collapsed: &HashSet<ProcessKind>,
    ) -> bool {
        if self.key.as_ref() == Some(&key) {
            return false;
        }

        let forest = Forest::build(rows);
        let visible = if query.is_empty() {
            None
        } else {
            Some(query.select(rows))
        };
        self.matched = visible.as_ref().map_or(0, HashSet::len);

        self.entries = forest
            .flatten(
                rows,
                Layout {
                    sort: key.sort,
                    descending: key.descending,
                    grouped: key.grouped,
                    expanded,
                    collapsed,
                    visible: visible.as_ref(),
                },
            )
            .into();
        self.forest = forest;
        self.key = Some(key);
        true
    }

    /// The rows to draw.
    #[must_use]
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// A cheap owned handle for draw closures that also mutate app state.
    #[must_use]
    pub fn shared_entries(&self) -> Arc<[Entry]> {
        Arc::clone(&self.entries)
    }

    /// The forest the rows were flattened from.
    #[must_use]
    pub fn forest(&self) -> &Forest {
        &self.forest
    }

    /// How many rows the filter matched.
    #[must_use]
    pub fn matched(&self) -> usize {
        self.matched
    }

    /// How many process rows — not headings — are being drawn.
    #[must_use]
    pub fn process_count(&self) -> usize {
        self.entries
            .iter()
            .filter(|entry| matches!(entry, Entry::Process { .. }))
            .count()
    }

    /// Discards the cached rows so the next refresh rebuilds.
    ///
    /// For the cases a key cannot express — a theme change that alters
    /// nothing about the rows, but also the moment the whole view is
    /// being reset.
    pub fn invalidate(&mut self) {
        self.key = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(pid: u32, parent: u32, started: u64) -> ProcessRow {
        ProcessRow {
            pid,
            parent_pid: parent,
            started_at: started,
            name: format!("p{pid}.exe"),
            ..ProcessRow::default()
        }
    }

    fn key(sequence: u64) -> RowKey {
        RowKey::new(
            sequence,
            SortKey::Cpu,
            true,
            true,
            "",
            &HashSet::new(),
            &HashSet::new(),
        )
    }

    #[test]
    fn an_unchanged_key_does_not_rebuild() {
        // The whole point: this work is justified once per sample, not
        // sixty times a second.
        let rows = vec![row(1, 0, 1), row(2, 1, 2)];
        let mut cache = Cache::default();
        assert!(cache.refresh(
            &rows,
            &Query::default(),
            key(1),
            &HashSet::new(),
            &HashSet::new()
        ));
        assert!(
            !cache.refresh(
                &rows,
                &Query::default(),
                key(1),
                &HashSet::new(),
                &HashSet::new()
            ),
            "a second frame with the same state must not rebuild"
        );
    }

    #[test]
    fn a_new_snapshot_rebuilds() {
        let rows = vec![row(1, 0, 1)];
        let mut cache = Cache::default();
        let _ = cache.refresh(
            &rows,
            &Query::default(),
            key(1),
            &HashSet::new(),
            &HashSet::new(),
        );
        assert!(cache.refresh(
            &rows,
            &Query::default(),
            key(2),
            &HashSet::new(),
            &HashSet::new()
        ));
    }

    #[test]
    fn a_row_key_covers_every_input_to_the_layout() {
        // Each of these must rebuild on its own, or the table silently
        // stops responding to the control that changes it — which looks
        // like the control is broken.
        let rows = vec![row(1, 0, 1), row(2, 1, 2)];
        let empty: HashSet<ProcessKey> = HashSet::new();
        let kinds: HashSet<ProcessKind> = HashSet::new();

        let variations: Vec<RowKey> = vec![
            RowKey::new(1, SortKey::Cpu, true, true, "", &empty, &kinds),
            // Sort column.
            RowKey::new(1, SortKey::Memory, true, true, "", &empty, &kinds),
            // Direction.
            RowKey::new(1, SortKey::Cpu, false, true, "", &empty, &kinds),
            // Grouping.
            RowKey::new(1, SortKey::Cpu, true, false, "", &empty, &kinds),
            // Filter.
            RowKey::new(1, SortKey::Cpu, true, true, "p1", &empty, &kinds),
            // Expansion.
            RowKey::new(
                1,
                SortKey::Cpu,
                true,
                true,
                "",
                &std::iter::once(rows[0].key()).collect(),
                &kinds,
            ),
            // Category collapse.
            RowKey::new(
                1,
                SortKey::Cpu,
                true,
                true,
                "",
                &empty,
                &std::iter::once(ProcessKind::Background).collect(),
            ),
        ];

        for (index, variation) in variations.iter().enumerate().skip(1) {
            let mut cache = Cache::default();
            let _ = cache.refresh(
                &rows,
                &Query::default(),
                variations[0].clone(),
                &empty,
                &kinds,
            );
            assert!(
                cache.refresh(&rows, &Query::default(), variation.clone(), &empty, &kinds),
                "variation {index} did not rebuild — a control that \
                 changes it would appear to do nothing"
            );
        }
    }

    #[test]
    fn a_hash_sets_iteration_order_does_not_cause_a_spurious_rebuild() {
        // A `HashSet`'s iteration order is not stable between
        // constructions. Comparing two keys built from equal sets must
        // still compare equal, or the cache rebuilds every frame — which
        // is exactly the cost it exists to avoid, arrived at by accident.
        let mut first = HashSet::new();
        let mut second = HashSet::new();
        for pid in 0..64u32 {
            first.insert(ProcessKey {
                pid,
                started_at: u64::from(pid),
            });
        }
        // Inserted in the opposite order.
        for pid in (0..64u32).rev() {
            second.insert(ProcessKey {
                pid,
                started_at: u64::from(pid),
            });
        }
        let empty: HashSet<ProcessKind> = HashSet::new();
        assert_eq!(
            RowKey::new(1, SortKey::Cpu, true, true, "", &first, &empty),
            RowKey::new(1, SortKey::Cpu, true, true, "", &second, &empty),
            "two equal expansion sets must produce equal keys"
        );
    }

    #[test]
    fn a_filter_records_how_many_rows_it_matched() {
        let rows = vec![row(1, 0, 1), row(2, 0, 2), row(3, 0, 3)];
        let mut cache = Cache::default();
        let query = Query::parse("p2");
        let filtered = RowKey::new(
            1,
            SortKey::Cpu,
            true,
            true,
            "p2",
            &HashSet::new(),
            &HashSet::new(),
        );
        let _ = cache.refresh(&rows, &query, filtered, &HashSet::new(), &HashSet::new());
        assert_eq!(cache.matched(), 1);
        assert_eq!(cache.process_count(), 1);
    }

    #[test]
    fn no_filter_reports_no_match_count() {
        // Zero means "not filtering", which is a different state from
        // "filtering and matching nothing" — the latter shows a count.
        let rows = vec![row(1, 0, 1)];
        let mut cache = Cache::default();
        let _ = cache.refresh(
            &rows,
            &Query::default(),
            key(1),
            &HashSet::new(),
            &HashSet::new(),
        );
        assert_eq!(cache.matched(), 0);
        assert_eq!(cache.process_count(), 1);
    }

    #[test]
    fn invalidating_forces_the_next_refresh_to_rebuild() {
        let rows = vec![row(1, 0, 1)];
        let mut cache = Cache::default();
        let _ = cache.refresh(
            &rows,
            &Query::default(),
            key(1),
            &HashSet::new(),
            &HashSet::new(),
        );
        cache.invalidate();
        assert!(cache.refresh(
            &rows,
            &Query::default(),
            key(1),
            &HashSet::new(),
            &HashSet::new()
        ));
    }

    #[test]
    fn the_forest_survives_for_the_actions_that_need_it() {
        // "End process tree" walks the forest the rows were flattened
        // from, rather than rebuilding one.
        let rows = vec![row(1, 0, 1), row(2, 1, 2), row(3, 2, 3)];
        let mut cache = Cache::default();
        let _ = cache.refresh(
            &rows,
            &Query::default(),
            key(1),
            &HashSet::new(),
            &HashSet::new(),
        );
        let mut branch = cache.forest().subtree(0);
        branch.sort_unstable();
        assert_eq!(branch, vec![0, 1, 2]);
    }
}

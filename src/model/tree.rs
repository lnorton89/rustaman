// ============================================================================
// Module:       model::tree
// Description:  Parent/child structure over a snapshot, subtree aggregation,
//               and the flattening that turns it into drawable rows.
//
// Dependencies: std collections; super::{ProcessRow, ProcessKind, ProcessKey}
//               and super::sort for sibling ordering.
// ============================================================================

//! The process tree, and the flat list of rows it is drawn as.
//!
//! ## Why the parent link needs checking
//!
//! Every process records the PID of whatever created it, and nothing
//! updates that field when the creator exits. So on any real machine a
//! large fraction of the parent PIDs point at processes that are gone —
//! and, because Windows recycles PIDs freely, a good number point at some
//! *unrelated* process that happens to hold that number now. Building a
//! tree from the raw field therefore invents relationships: an editor
//! started an hour ago becomes the "child" of a `conhost.exe` that
//! launched five seconds ago and merely inherited the number.
//!
//! Worse, it can produce a cycle. Two processes each holding the other's
//! recycled PID is unlikely but entirely possible, and a naive recursive
//! walk over that hangs the UI thread.
//!
//! [`Forest::build`] rejects a parent link unless the claimed parent
//! exists in this same snapshot *and* started strictly before the child.
//! The time test is what actually does the work: a real parent always
//! predates its child, so a recycled PID whose current holder is younger
//! than the process claiming it is provably not the parent. A rejected
//! link makes the row a root, which is right — its parent really is gone.
//!
//! The ordering is strict, so equal creation times reject too. Two
//! processes can share a FILETIME (the clock's granularity is coarser
//! than process creation), and treating one as the other's parent on a
//! tie is exactly how a two-node cycle would get in. Since a strict
//! ordering has no cycles, the forest provably has none — and
//! [`Forest::flatten`] is still iterative and still carries a guard,
//! because "provably" and "at three in the morning on someone else's
//! machine" are different claims.

use super::sort::SortKey;
use super::{ProcessKey, ProcessKind, ProcessRow};
use std::collections::{HashMap, HashSet};

/// Parent/child structure over the rows of one snapshot.
///
/// Indices are into the `rows` slice it was built from and are only
/// meaningful against that slice — a [`Forest`] must be rebuilt when the
/// snapshot is replaced.
#[derive(Clone, Debug, Default)]
pub struct Forest {
    /// `children[i]` holds the row indices whose parent is row `i`.
    children: Vec<Vec<usize>>,
    /// `parent[i]` is the row index of `i`'s accepted parent, if it has
    /// one. Kept rather than derived: the aggregation below needs it once
    /// per row, and scanning the child lists for it would turn a linear
    /// pass into a quadratic one.
    parent: Vec<Option<usize>>,
    /// Row indices with no accepted parent, which is where drawing starts.
    roots: Vec<usize>,
    /// `depth[i]` is how far row `i` sits from its root. Computed during
    /// the build, because the aggregation below needs to visit rows
    /// deepest-first and this is what it orders by.
    depth: Vec<u16>,
}

impl Forest {
    /// Builds the forest, rejecting parent links that fail the checks in
    /// the module docs.
    #[must_use]
    pub fn build(rows: &[ProcessRow]) -> Self {
        // PID to row index. A snapshot cannot contain two rows with one
        // PID, so the last writer winning is not a real case; using a map
        // rather than a scan is what keeps this O(n) instead of O(n²) on
        // the four hundred rows a normal machine has.
        let mut by_pid: HashMap<u32, usize> = HashMap::with_capacity(rows.len());
        for (index, row) in rows.iter().enumerate() {
            by_pid.insert(row.pid, index);
        }

        let mut children: Vec<Vec<usize>> = vec![Vec::new(); rows.len()];
        let mut parent_of: Vec<Option<usize>> = vec![None; rows.len()];
        let mut roots = Vec::new();

        for (index, row) in rows.iter().enumerate() {
            let accepted = by_pid
                .get(&row.parent_pid)
                .copied()
                // A process cannot be its own parent, however the PIDs
                // landed.
                .filter(|parent| *parent != index)
                .filter(|parent| {
                    // The check that does the work: a real parent
                    // predates its child. Strictly — see the module docs
                    // on why a tie is rejected too.
                    rows.get(*parent)
                        .is_some_and(|parent| parent.started_at < row.started_at)
                });
            match accepted {
                Some(parent) => {
                    parent_of[index] = Some(parent);
                    if let Some(slot) = children.get_mut(parent) {
                        slot.push(index);
                    }
                }
                None => roots.push(index),
            }
        }

        let depth = compute_depths(&parent_of);
        Self {
            children,
            parent: parent_of,
            roots,
            depth,
        }
    }

    /// The row indices with no parent in this snapshot.
    #[must_use]
    pub fn roots(&self) -> &[usize] {
        &self.roots
    }

    /// The row indices whose parent is `index`.
    #[must_use]
    pub fn children_of(&self, index: usize) -> &[usize] {
        self.children.get(index).map_or(&[], Vec::as_slice)
    }

    /// How far row `index` sits from its root.
    #[must_use]
    pub fn depth_of(&self, index: usize) -> u16 {
        self.depth.get(index).copied().unwrap_or(0)
    }

    /// Every row in the subtree rooted at `index`, including `index`.
    ///
    /// Iterative, and bounded by the number of rows: "end process tree"
    /// runs through this, and a walk that recursed would put a
    /// user-influenced depth on the call stack of the thread about to
    /// terminate things.
    #[must_use]
    pub fn subtree(&self, index: usize) -> Vec<usize> {
        let mut found = Vec::new();
        let mut seen = HashSet::new();
        let mut pending = vec![index];
        while let Some(current) = pending.pop() {
            if !seen.insert(current) {
                continue;
            }
            found.push(current);
            pending.extend_from_slice(self.children_of(current));
        }
        found
    }

    /// Aggregates each subtree's live counters into its root.
    ///
    /// A collapsed parent has to account for what its children are doing,
    /// or collapsing the tree makes a busy process disappear — which is
    /// the single most common thing a task manager is opened to find. The
    /// browser case makes it concrete: `chrome.exe` collapsed shows 0.1%
    /// CPU while thirty renderer children under it use 60% between them.
    ///
    /// Computed bottom-up over rows ordered by descending depth, so each
    /// parent is reached only once and only after every one of its
    /// children — no recursion, one pass, and no repeated traversal of a
    /// deep subtree.
    #[must_use]
    pub fn aggregate(&self, rows: &[ProcessRow]) -> Vec<Totals> {
        let mut totals: Vec<Totals> = rows.iter().map(Totals::of).collect();

        // Deepest first. A parent's depth is strictly less than its
        // children's, so this order guarantees every child has already
        // been folded in by the time its parent is visited.
        let mut order: Vec<usize> = (0..rows.len()).collect();
        order.sort_unstable_by_key(|index| std::cmp::Reverse(self.depth_of(*index)));

        for index in order {
            // By the time `index` is reached, every one of its children
            // has already folded its own subtree in — so this is the
            // complete subtree total, and folding it into the parent is
            // all that is left to do.
            let Some(parent) = self.parent_of(index) else {
                continue;
            };
            let carried = totals.get(index).copied().unwrap_or_default();
            if let Some(slot) = totals.get_mut(parent) {
                slot.add(&carried);
            }
        }
        totals
    }

    /// The accepted parent of `index`, if it has one.
    #[must_use]
    pub fn parent_of(&self, index: usize) -> Option<usize> {
        self.parent.get(index).copied().flatten()
    }
}

/// Assigns each row its distance from its root.
///
/// Roots are seeded at zero, then every remaining row walks up its parent
/// chain to the first ancestor whose depth is already known and fills the
/// path back down. Each row is therefore assigned exactly once, so the
/// total work is linear in the number of rows however deep the tree is —
/// and nothing recurses, which is the rule for every whole-tree walk in
/// this crate.
///
/// The `seen` guard makes a cycle terminate rather than hang. The build
/// rules make a cycle impossible; this is what makes relying on that safe.
fn compute_depths(parent_of: &[Option<usize>]) -> Vec<u16> {
    let mut depth: Vec<Option<u16>> = vec![None; parent_of.len()];
    for (index, parent) in parent_of.iter().enumerate() {
        if parent.is_none() {
            if let Some(slot) = depth.get_mut(index) {
                *slot = Some(0);
            }
        }
    }

    let mut chain: Vec<usize> = Vec::new();
    let mut seen: HashSet<usize> = HashSet::new();
    for start in 0..parent_of.len() {
        if depth.get(start).copied().flatten().is_some() {
            continue;
        }
        chain.clear();
        seen.clear();
        let mut cursor = start;
        let base = loop {
            if let Some(known) = depth.get(cursor).copied().flatten() {
                break known;
            }
            if !seen.insert(cursor) {
                // A cycle: root the rest of the chain here rather than
                // walking it forever.
                break 0;
            }
            chain.push(cursor);
            match parent_of.get(cursor).copied().flatten() {
                Some(parent) => cursor = parent,
                // Unreachable in practice — every parentless row was
                // seeded above — but taking this branch rather than
                // asserting keeps the function total.
                None => break 0,
            }
        };
        // `chain` holds the path deepest-first, so filling it in reverse
        // walks down from just below the known ancestor.
        for (step, index) in chain.iter().rev().enumerate() {
            if let Some(slot) = depth.get_mut(*index) {
                // Saturating so a pathological chain longer than 65,535
                // cannot wrap the counter back to zero and flatten the
                // indentation.
                let step = u16::try_from(step).unwrap_or(u16::MAX);
                *slot = Some(base.saturating_add(1).saturating_add(step));
            }
        }
    }

    depth.into_iter().map(|value| value.unwrap_or(0)).collect()
}

/// The live counters that aggregate up a subtree.
///
/// Only the ones that *can* be summed. Working set deliberately is not
/// here as a sum of physical pages — see [`Totals::add`].
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Totals {
    /// Summed share of total CPU.
    pub cpu_percent: f64,
    /// Summed working set. See [`Totals::add`] on what this over-counts.
    pub working_set: u64,
    /// Summed private commit.
    pub private_bytes: u64,
    /// Summed disk throughput.
    pub disk_rate: f64,
    /// Summed GPU utilisation.
    pub gpu_percent: f64,
    /// Summed open endpoints.
    pub connections: u32,
    /// How many rows are in this subtree, including its own.
    pub processes: u32,
}

impl Totals {
    /// One row's own counters, before any children are folded in.
    #[must_use]
    pub fn of(row: &ProcessRow) -> Self {
        Self {
            cpu_percent: row.cpu_percent,
            working_set: row.working_set,
            private_bytes: row.private_bytes,
            disk_rate: row.disk_rate(),
            gpu_percent: row.gpu_percent,
            connections: row.connections,
            processes: 1,
        }
    }

    /// Folds `other`'s totals into these.
    ///
    /// Summing working set over-counts: two processes sharing a mapped
    /// DLL each count those pages, so a browser's twenty children add up
    /// to more physical memory than they occupy. It is summed anyway,
    /// because every task manager does and because the alternative —
    /// leaving the column blank on a collapsed row — hides the thing the
    /// user is looking for. Private bytes is the honest column, which is
    /// exactly why it is shown beside it.
    ///
    /// Saturating throughout: these are display figures, and a total that
    /// wrapped would read as a machine with negative memory.
    pub fn add(&mut self, other: &Self) {
        self.cpu_percent += other.cpu_percent;
        self.working_set = self.working_set.saturating_add(other.working_set);
        self.private_bytes = self.private_bytes.saturating_add(other.private_bytes);
        self.disk_rate += other.disk_rate;
        self.gpu_percent += other.gpu_percent;
        self.connections = self.connections.saturating_add(other.connections);
        self.processes = self.processes.saturating_add(other.processes);
    }
}

/// One drawable line of the process list.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Entry {
    /// A category heading, with the aggregate of everything under it.
    Group {
        /// Which category.
        kind: ProcessKind,
        /// Rows in the category, including nested ones.
        totals: Totals,
        /// Whether the category is collapsed.
        collapsed: bool,
    },
    /// A process row.
    Process {
        /// Index into the snapshot's `rows` slice.
        index: usize,
        /// Indent level, relative to the top of its category.
        depth: u16,
        /// How many direct children it has, which is what decides whether
        /// it draws a disclosure triangle.
        children: usize,
        /// Whether its children are currently shown.
        expanded: bool,
        /// This row plus everything under it.
        totals: Totals,
    },
}

/// What [`Forest::flatten`] needs to know to lay the list out.
#[derive(Clone, Copy)]
pub struct Layout<'a> {
    /// The column being sorted on.
    pub sort: SortKey,
    /// Whether that sort is descending.
    pub descending: bool,
    /// Whether to draw the tree and the category headings at all. When
    /// false the result is one flat, fully sorted list — which is what
    /// the Details view wants, and what the process list falls back to
    /// when a user prefers it.
    pub grouped: bool,
    /// Which rows have their children shown.
    pub expanded: &'a HashSet<ProcessKey>,
    /// Which categories are collapsed.
    pub collapsed: &'a HashSet<ProcessKind>,
    /// The rows a filter has left visible, or `None` for no filter. A row
    /// not in this set is still drawn if one of its descendants is — see
    /// [`Forest::flatten`].
    pub visible: Option<&'a HashSet<usize>>,
}

impl Forest {
    /// Turns the forest into the flat list of lines to draw.
    ///
    /// Iterative, for the reason every walk in this crate is: the depth
    /// is user-influenced and this runs on the UI thread.
    ///
    /// When a filter is active, a row is drawn if it matches *or* if
    /// anything beneath it does. Dropping the non-matching ancestors
    /// would leave the matches floating at the top level with no
    /// indication of what they belong to — searching for `renderer` would
    /// show thirty identical rows and not which browser each came from —
    /// and re-rooting them would silently redraw the machine's process
    /// tree as something it is not.
    #[must_use]
    pub fn flatten(&self, rows: &[ProcessRow], layout: Layout<'_>) -> Vec<Entry> {
        let totals = self.aggregate(rows);
        let keep = layout.visible.map(|visible| self.retained(visible));

        if !layout.grouped {
            let mut indices: Vec<usize> = (0..rows.len())
                .filter(|index| layout.visible.is_none_or(|set| set.contains(index)))
                .collect();
            sort_indices(&mut indices, rows, layout.sort, layout.descending);
            return indices
                .into_iter()
                .map(|index| Entry::Process {
                    index,
                    depth: 0,
                    children: 0,
                    expanded: false,
                    totals: totals.get(index).copied().unwrap_or_default(),
                })
                .collect();
        }

        let mut entries = Vec::with_capacity(rows.len() + ProcessKind::ALL.len());
        for kind in ProcessKind::ALL {
            // A category's members are the *roots* that belong to it. A
            // child is drawn under its parent whatever its own category,
            // because a tree that re-parents rows by category is not the
            // machine's tree any more.
            let mut group_roots: Vec<usize> = self
                .roots
                .iter()
                .copied()
                .filter(|index| rows.get(*index).is_some_and(|row| row.kind == kind))
                .filter(|index| keep.as_ref().is_none_or(|set| set.contains(index)))
                .collect();
            if group_roots.is_empty() {
                continue;
            }
            sort_indices(&mut group_roots, rows, layout.sort, layout.descending);

            let mut group_totals = Totals::default();
            for index in &group_roots {
                if let Some(subtotal) = totals.get(*index) {
                    group_totals.add(subtotal);
                }
            }
            let collapsed = layout.collapsed.contains(&kind);
            entries.push(Entry::Group {
                kind,
                totals: group_totals,
                collapsed,
            });
            if collapsed {
                continue;
            }

            // Depth-first, iteratively, with the stack holding rows in
            // reverse so siblings come out in sorted order.
            let mut stack: Vec<(usize, u16)> = group_roots
                .iter()
                .rev()
                .map(|index| (*index, 0u16))
                .collect();
            let mut seen: HashSet<usize> = HashSet::new();
            while let Some((index, depth)) = stack.pop() {
                // The build rules make a cycle impossible; this is what
                // makes relying on that safe. See the module docs.
                if !seen.insert(index) {
                    continue;
                }
                let mut children: Vec<usize> = self
                    .children_of(index)
                    .iter()
                    .copied()
                    .filter(|child| keep.as_ref().is_none_or(|set| set.contains(child)))
                    .collect();
                let key = rows.get(index).map(ProcessRow::key).unwrap_or_default();
                // A filter expands everything it left standing: a match
                // buried three levels down that the user then has to
                // click three times to reach has not been found.
                let expanded = layout.visible.is_some() || layout.expanded.contains(&key);
                entries.push(Entry::Process {
                    index,
                    depth,
                    children: children.len(),
                    expanded,
                    totals: totals.get(index).copied().unwrap_or_default(),
                });
                if expanded && !children.is_empty() {
                    sort_indices(&mut children, rows, layout.sort, layout.descending);
                    stack.extend(
                        children
                            .into_iter()
                            .rev()
                            .map(|child| (child, depth.saturating_add(1))),
                    );
                }
            }
        }
        entries
    }

    /// The rows to keep when a filter is active: the matches, plus every
    /// ancestor of a match.
    ///
    /// Walks *up* from each match rather than down from each root, so the
    /// cost is proportional to the number of matches and their depth
    /// rather than to the size of the tree.
    fn retained(&self, matched: &HashSet<usize>) -> HashSet<usize> {
        let mut keep: HashSet<usize> = HashSet::with_capacity(matched.len() * 2);
        for &index in matched {
            let mut cursor = Some(index);
            while let Some(current) = cursor {
                // Already-kept means this ancestor chain has been walked
                // for an earlier match; everything above it is in too.
                if !keep.insert(current) {
                    break;
                }
                cursor = self.parent_of(current);
            }
        }
        keep
    }
}

/// Sorts row indices by the given column.
fn sort_indices(indices: &mut [usize], rows: &[ProcessRow], sort: SortKey, descending: bool) {
    indices.sort_unstable_by(|a, b| {
        let (Some(a), Some(b)) = (rows.get(*a), rows.get(*b)) else {
            return std::cmp::Ordering::Equal;
        };
        if descending {
            sort.compare(b, a)
        } else {
            sort.compare(a, b)
        }
    });
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

    #[test]
    fn a_real_parent_child_link_is_accepted() {
        let rows = vec![row(100, 0, 10), row(200, 100, 20)];
        let forest = Forest::build(&rows);
        assert_eq!(forest.roots(), &[0], "only the parent is a root");
        assert_eq!(forest.children_of(0), &[1]);
        assert_eq!(forest.depth_of(1), 1);
    }

    #[test]
    fn a_recycled_parent_pid_does_not_invent_a_relationship() {
        // The common case: an editor started an hour ago whose parent
        // exited, and a `conhost.exe` that launched five seconds ago and
        // inherited the number.
        let rows = vec![
            row(100, 0, 900),  // the new holder of PID 100, started late
            row(200, 100, 10), // started long before its claimed parent
        ];
        let forest = Forest::build(&rows);
        assert_eq!(
            forest.roots(),
            &[0, 1],
            "a claimed parent younger than its child is not the parent"
        );
        assert!(forest.children_of(0).is_empty());
    }

    #[test]
    fn a_parent_that_has_exited_makes_its_child_a_root() {
        let rows = vec![row(200, 999, 20)];
        let forest = Forest::build(&rows);
        assert_eq!(forest.roots(), &[0], "a missing parent means a root");
    }

    #[test]
    fn a_process_cannot_be_its_own_parent() {
        let rows = vec![row(100, 100, 10)];
        let forest = Forest::build(&rows);
        assert_eq!(forest.roots(), &[0]);
        assert!(forest.children_of(0).is_empty());
    }

    #[test]
    fn processes_sharing_a_creation_time_cannot_form_a_cycle() {
        // The FILETIME clock is coarser than process creation, so ties
        // happen. Accepting a link on a tie is how a two-node cycle would
        // get in — and a cycle hangs a recursive walk.
        let rows = vec![row(100, 200, 50), row(200, 100, 50)];
        let forest = Forest::build(&rows);
        assert_eq!(
            forest.roots().len(),
            2,
            "an equal creation time must reject the link in both directions"
        );
    }

    #[test]
    fn flattening_terminates_even_on_a_forest_with_a_cycle_in_it() {
        // The build rules make this unreachable. The guard is what makes
        // relying on that safe, so it is worth a test that constructs the
        // impossible state directly.
        let rows = vec![row(1, 0, 1), row(2, 1, 2)];
        let mut forest = Forest::build(&rows);
        forest.children[1] = vec![0]; // 0 -> 1 -> 0
        let expanded: HashSet<ProcessKey> = rows.iter().map(ProcessRow::key).collect();
        let collapsed = HashSet::new();
        let entries = forest.flatten(
            &rows,
            Layout {
                sort: SortKey::Pid,
                descending: false,
                grouped: true,
                expanded: &expanded,
                collapsed: &collapsed,
                visible: None,
            },
        );
        assert!(
            entries.len() <= rows.len() + ProcessKind::ALL.len(),
            "a cycle must not produce rows without end"
        );
    }

    #[test]
    fn a_deep_chain_does_not_put_its_depth_on_the_stack() {
        // Depth is user-influenced, and this runs on the UI thread.
        const DEPTH: u32 = 20_000;
        let rows: Vec<ProcessRow> = (0..DEPTH)
            .map(|i| row(i + 1, if i == 0 { 0 } else { i }, u64::from(i) + 1))
            .collect();
        let forest = Forest::build(&rows);
        assert_eq!(forest.roots().len(), 1);
        // `depth` saturates at u16::MAX rather than wrapping.
        assert!(forest.depth_of(rows.len() - 1) > 0);

        let expanded: HashSet<ProcessKey> = rows.iter().map(ProcessRow::key).collect();
        let collapsed = HashSet::new();
        let entries = forest.flatten(
            &rows,
            Layout {
                sort: SortKey::Pid,
                descending: false,
                grouped: true,
                expanded: &expanded,
                collapsed: &collapsed,
                visible: None,
            },
        );
        let drawn = entries
            .iter()
            .filter(|entry| matches!(entry, Entry::Process { .. }))
            .count();
        assert_eq!(drawn, rows.len(), "every row in the chain should be drawn");

        // And the aggregation, which is the other whole-tree walk.
        let totals = forest.aggregate(&rows);
        assert_eq!(
            totals[0].processes as usize,
            rows.len(),
            "the root's subtree should account for the whole chain"
        );
    }

    #[test]
    fn a_collapsed_parent_accounts_for_what_its_children_are_doing() {
        // The browser case: `chrome.exe` at 0.1% with thirty renderers
        // under it using 60% between them. Collapsing must not make that
        // disappear.
        let mut parent = row(100, 0, 10);
        parent.cpu_percent = 0.1;
        parent.working_set = 1_000;
        let mut child_a = row(200, 100, 20);
        child_a.cpu_percent = 30.0;
        child_a.working_set = 2_000;
        let mut child_b = row(300, 200, 30);
        child_b.cpu_percent = 30.0;
        child_b.working_set = 4_000;

        let rows = vec![parent, child_a, child_b];
        let forest = Forest::build(&rows);
        let totals = forest.aggregate(&rows);

        assert!(
            (totals[0].cpu_percent - 60.1).abs() < 1e-9,
            "the root should carry the whole subtree's CPU, got {}",
            totals[0].cpu_percent
        );
        assert_eq!(totals[0].working_set, 7_000);
        assert_eq!(totals[0].processes, 3);
        assert_eq!(
            totals[1].processes, 2,
            "an intermediate node carries itself and everything below it"
        );
        assert_eq!(totals[2].processes, 1, "a leaf carries only itself");
    }

    #[test]
    fn a_collapsed_row_hides_its_children_and_an_expanded_one_shows_them() {
        let rows = vec![row(100, 0, 10), row(200, 100, 20)];
        let forest = Forest::build(&rows);
        let collapsed_groups = HashSet::new();

        let no_expansion = HashSet::new();
        let entries = forest.flatten(
            &rows,
            Layout {
                sort: SortKey::Pid,
                descending: false,
                grouped: true,
                expanded: &no_expansion,
                collapsed: &collapsed_groups,
                visible: None,
            },
        );
        let drawn = entries
            .iter()
            .filter(|entry| matches!(entry, Entry::Process { .. }))
            .count();
        assert_eq!(drawn, 1, "a collapsed parent draws only itself");

        let expanded: HashSet<ProcessKey> = std::iter::once(rows[0].key()).collect();
        let entries = forest.flatten(
            &rows,
            Layout {
                sort: SortKey::Pid,
                descending: false,
                grouped: true,
                expanded: &expanded,
                collapsed: &collapsed_groups,
                visible: None,
            },
        );
        let drawn: Vec<&Entry> = entries
            .iter()
            .filter(|entry| matches!(entry, Entry::Process { .. }))
            .collect();
        assert_eq!(drawn.len(), 2);
        assert!(
            matches!(drawn[1], Entry::Process { depth: 1, .. }),
            "the child should be indented one level"
        );
    }

    #[test]
    fn a_collapsed_category_hides_every_row_in_it_but_keeps_its_heading() {
        let rows = vec![row(100, 0, 10)];
        let forest = Forest::build(&rows);
        let expanded = HashSet::new();
        let collapsed: HashSet<ProcessKind> = std::iter::once(ProcessKind::Background).collect();
        let entries = forest.flatten(
            &rows,
            Layout {
                sort: SortKey::Pid,
                descending: false,
                grouped: true,
                expanded: &expanded,
                collapsed: &collapsed,
                visible: None,
            },
        );
        assert_eq!(
            entries.len(),
            1,
            "the heading remains so it can be reopened"
        );
        assert!(matches!(
            entries[0],
            Entry::Group {
                collapsed: true,
                ..
            }
        ));
    }

    #[test]
    fn a_filter_keeps_the_ancestors_of_every_match() {
        // Searching for a renderer must not leave thirty identical rows
        // floating with no indication of which browser each belongs to.
        let rows = vec![row(100, 0, 10), row(200, 100, 20), row(300, 200, 30)];
        let forest = Forest::build(&rows);
        let matched: HashSet<usize> = std::iter::once(2).collect();
        let expanded = HashSet::new();
        let collapsed = HashSet::new();
        let entries = forest.flatten(
            &rows,
            Layout {
                sort: SortKey::Pid,
                descending: false,
                grouped: true,
                expanded: &expanded,
                collapsed: &collapsed,
                visible: Some(&matched),
            },
        );
        let drawn: Vec<usize> = entries
            .iter()
            .filter_map(|entry| match entry {
                Entry::Process { index, .. } => Some(*index),
                Entry::Group { .. } => None,
            })
            .collect();
        assert_eq!(
            drawn,
            vec![0, 1, 2],
            "the match's whole ancestor chain should be drawn, in order"
        );
    }

    #[test]
    fn a_filter_expands_what_it_leaves_standing() {
        // A match buried three levels down that then needs three clicks
        // to reach has not been found.
        let rows = vec![row(100, 0, 10), row(200, 100, 20)];
        let forest = Forest::build(&rows);
        let matched: HashSet<usize> = std::iter::once(1).collect();
        let expanded = HashSet::new(); // nothing manually expanded
        let collapsed = HashSet::new();
        let entries = forest.flatten(
            &rows,
            Layout {
                sort: SortKey::Pid,
                descending: false,
                grouped: true,
                expanded: &expanded,
                collapsed: &collapsed,
                visible: Some(&matched),
            },
        );
        let drawn = entries
            .iter()
            .filter(|entry| matches!(entry, Entry::Process { .. }))
            .count();
        assert_eq!(drawn, 2, "the parent must be drawn already open");
    }

    #[test]
    fn an_ungrouped_layout_is_one_flat_sorted_list_with_no_headings() {
        let rows = vec![row(300, 0, 10), row(100, 0, 20), row(200, 100, 30)];
        let forest = Forest::build(&rows);
        let expanded = HashSet::new();
        let collapsed = HashSet::new();
        let entries = forest.flatten(
            &rows,
            Layout {
                sort: SortKey::Pid,
                descending: false,
                grouped: false,
                expanded: &expanded,
                collapsed: &collapsed,
                visible: None,
            },
        );
        assert_eq!(entries.len(), 3, "every row, no headings");
        assert!(entries.iter().all(|entry| matches!(
            entry,
            Entry::Process {
                depth: 0,
                children: 0,
                ..
            }
        )));
        let pids: Vec<u32> = entries
            .iter()
            .filter_map(|entry| match entry {
                Entry::Process { index, .. } => rows.get(*index).map(|row| row.pid),
                Entry::Group { .. } => None,
            })
            .collect();
        assert_eq!(pids, vec![100, 200, 300], "flat means fully sorted");
    }

    #[test]
    fn a_child_stays_under_its_parent_whatever_category_it_is_in() {
        // Re-parenting rows by category would draw a tree that is not the
        // machine's.
        let mut parent = row(100, 0, 10);
        parent.kind = ProcessKind::App;
        let mut child = row(200, 100, 20);
        child.kind = ProcessKind::System;

        let rows = vec![parent, child];
        let forest = Forest::build(&rows);
        let expanded: HashSet<ProcessKey> = std::iter::once(rows[0].key()).collect();
        let collapsed = HashSet::new();
        let entries = forest.flatten(
            &rows,
            Layout {
                sort: SortKey::Pid,
                descending: false,
                grouped: true,
                expanded: &expanded,
                collapsed: &collapsed,
                visible: None,
            },
        );
        let groups = entries
            .iter()
            .filter(|entry| matches!(entry, Entry::Group { .. }))
            .count();
        assert_eq!(groups, 1, "the child does not open a second category");
        assert!(
            matches!(
                entries.last(),
                Some(Entry::Process {
                    index: 1,
                    depth: 1,
                    ..
                })
            ),
            "the system-owned child stays indented under its app parent"
        );
    }

    #[test]
    fn siblings_are_ordered_by_the_active_sort() {
        let mut first = row(100, 0, 10);
        first.cpu_percent = 1.0;
        let mut second = row(200, 0, 20);
        second.cpu_percent = 50.0;
        let rows = vec![first, second];
        let forest = Forest::build(&rows);
        let expanded = HashSet::new();
        let collapsed = HashSet::new();
        let entries = forest.flatten(
            &rows,
            Layout {
                sort: SortKey::Cpu,
                descending: true,
                grouped: true,
                expanded: &expanded,
                collapsed: &collapsed,
                visible: None,
            },
        );
        let first_drawn = entries.iter().find_map(|entry| match entry {
            Entry::Process { index, .. } => Some(*index),
            Entry::Group { .. } => None,
        });
        assert_eq!(first_drawn, Some(1), "the busiest root comes first");
    }

    #[test]
    fn subtree_returns_the_whole_branch_for_end_process_tree() {
        let rows = vec![
            row(100, 0, 10),
            row(200, 100, 20),
            row(300, 200, 30),
            row(400, 0, 40),
        ];
        let forest = Forest::build(&rows);
        let mut branch = forest.subtree(0);
        branch.sort_unstable();
        assert_eq!(
            branch,
            vec![0, 1, 2],
            "the unrelated root must not be swept into the kill"
        );
    }
}

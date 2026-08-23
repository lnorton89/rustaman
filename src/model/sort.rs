// ============================================================================
// Module:       model::sort
// Description:  The sortable columns, the comparison each one implies, and the
//               tie-break that keeps a one-second refresh from reshuffling.
//
// Dependencies: std; super::ProcessRow
// ============================================================================

//! Sorting the process list.
//!
//! ## The problem this module actually solves
//!
//! A process list refreshes every second, and most of what it sorts by is
//! a live number. Sort by CPU on an idle machine and three hundred rows
//! are all exactly zero — so their relative order is decided by whatever
//! the comparison does with a tie, and if that is "leave them where they
//! were" the answer depends on the order the kernel happened to return
//! them in, which changes every sample. The result is a table that
//! reshuffles itself once a second, and it is unusable: you cannot click
//! a row, because it will not be there when the click lands.
//!
//! So every comparison here ends in the same total tie-break — name, then
//! PID — and [`SortKey::compare`] is a *total order* over rows rather than
//! a comparison of one field. Two rows that tie on CPU are then ordered by
//! name, and two processes with the same name by PID, and no two rows can
//! share a PID. The order is therefore fully determined by the rows
//! themselves and not by the order they arrived in, which is what makes it
//! stable across samples.
//!
//! That is stronger than using a stable sort. A stable sort preserves the
//! *input* order for ties, and the input order here is the kernel's, which
//! is not stable at all.

use super::tree::Totals;
use super::ProcessRow;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;

/// A column the process and details tables can sort by.
///
/// Serialized into the config file by its lowercase name, so the stored
/// value stays readable and stays valid if the variants are ever
/// reordered. Renaming a variant would silently drop everyone's saved
/// sort back to the default — the same rule as a theme `id`.
#[derive(
    Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default, Serialize, Deserialize,
)]
#[serde(rename_all = "kebab-case")]
pub enum SortKey {
    /// The display name — description, or image name where there is none.
    #[default]
    Name,
    /// Process id.
    Pid,
    /// Running or suspended.
    Status,
    /// Share of total CPU.
    Cpu,
    /// Working set.
    Memory,
    /// Private commit.
    PrivateBytes,
    /// Combined disk throughput.
    Disk,
    /// Open network endpoints.
    Network,
    /// GPU engine utilisation.
    Gpu,
    /// Owning account.
    User,
    /// Thread count.
    Threads,
    /// Open handle count.
    Handles,
    /// Cumulative CPU time.
    CpuTime,
    /// Priority class.
    Priority,
    /// Image bitness.
    Architecture,
    /// Terminal-services session.
    Session,
    /// Full path to the executable.
    Path,
}

impl SortKey {
    /// The column heading.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Name => "Name",
            Self::Pid => "PID",
            Self::Status => "Status",
            Self::Cpu => "CPU",
            Self::Memory => "Memory",
            Self::PrivateBytes => "Private",
            Self::Disk => "Disk",
            Self::Network => "Network",
            Self::Gpu => "GPU",
            Self::User => "User",
            Self::Threads => "Threads",
            Self::Handles => "Handles",
            Self::CpuTime => "CPU time",
            Self::Priority => "Priority",
            Self::Architecture => "Arch",
            Self::Session => "Session",
            Self::Path => "Path",
        }
    }

    /// The mark this column's heading carries, if it has earned one.
    ///
    /// **Only the columns that name a resource.** The five that do are
    /// the five the Performance view has panels for, and using the same
    /// glyph in both places is the whole return on this: a person who
    /// has seen the Disk panel recognises the Disk column without
    /// reading it.
    ///
    /// Everything else is `None` on purpose. A mark against "PID" or
    /// "Session" would have to be invented rather than recognised, and
    /// an invented glyph is a second thing to learn in a column heading
    /// that already says the word — noise dressed as information, which
    /// is the same argument that keeps colour off the process rows.
    #[must_use]
    pub fn mark(self) -> Option<crate::icon::Icon> {
        match self {
            Self::Cpu | Self::CpuTime => Some(crate::icon::Icon::Cpu),
            Self::Memory | Self::PrivateBytes => Some(crate::icon::Icon::Memory),
            Self::Disk => Some(crate::icon::Icon::Disk),
            Self::Network => Some(crate::icon::Icon::Network),
            Self::Gpu => Some(crate::icon::Icon::Gpu),
            // Named rather than wildcarded, so a column added later is a
            // compile error here and somebody has to decide.
            Self::Name
            | Self::Pid
            | Self::Status
            | Self::User
            | Self::Threads
            | Self::Handles
            | Self::Priority
            | Self::Architecture
            | Self::Session
            | Self::Path => None,
        }
    }

    /// Which way this column sorts when it is first clicked.
    ///
    /// A magnitude — CPU, memory, disk — opens descending, because the
    /// only reason anyone clicks it is to find the biggest one. A name or
    /// a path opens ascending, because that is alphabetical order and
    /// anything else reads as broken. Getting this wrong costs a second
    /// click every single time, which is the sort of small friction that
    /// makes a tool feel worse than the one it replaced without anyone
    /// being able to say why.
    #[must_use]
    pub fn defaults_descending(self) -> bool {
        match self {
            Self::Name
            | Self::Pid
            | Self::Status
            | Self::User
            | Self::Architecture
            | Self::Session
            | Self::Path => false,
            Self::Cpu
            | Self::Memory
            | Self::PrivateBytes
            | Self::Disk
            | Self::Network
            | Self::Gpu
            | Self::Threads
            | Self::Handles
            | Self::CpuTime
            | Self::Priority => true,
        }
    }

    /// Compares two rows by this column, ascending, ending in the total
    /// tie-break described in the module docs.
    ///
    /// For a directed comparison use [`SortKey::compare_directed`] —
    /// reversing *this* reverses the tie-break too, which is not what a
    /// descending sort should do.
    #[must_use]
    pub fn compare(self, a: &ProcessRow, b: &ProcessRow) -> Ordering {
        self.primary(a, b).then_with(|| tie_break(a, b))
    }

    /// Compares two rows in the requested direction.
    ///
    /// The direction applies to the sorted column **only**; the tie-break
    /// stays ascending either way. That asymmetry is deliberate, and it
    /// is what the obvious implementation — reverse the whole comparator
    /// — gets wrong.
    ///
    /// Sort by CPU descending on an idle machine and almost every row
    /// ties at zero, so the tie-break decides the order of nearly the
    /// whole table. Reversing it too would list those three hundred rows
    /// in reverse alphabetical order, from `zoom.exe` upwards, which
    /// reads as the table being sorted by nothing at all. Ascending, the
    /// busy rows sort to the top and everything below them is an
    /// alphabetical list — which is what a reader expects and what makes
    /// a name findable while a magnitude column is active.
    #[must_use]
    pub fn compare_directed(self, a: &ProcessRow, b: &ProcessRow, descending: bool) -> Ordering {
        let primary = self.primary(a, b);
        let primary = if descending {
            primary.reverse()
        } else {
            primary
        };
        primary.then_with(|| tie_break(a, b))
    }

    /// Compares two rows by **the figure the table is showing for
    /// them**, which is not always their own.
    ///
    /// A collapsed parent shows its subtree's total — collapsing the
    /// tree must not make a busy process disappear — and the row is
    /// passed its `Totals` here when it does. Sorting on the row's own
    /// value while the cell displays the subtree's is the bug this
    /// exists to prevent, and it is a loud one: a CPU column sorted
    /// descending read 15%, 0.1%, 2.5% down the page, because the
    /// numbers on screen were never the numbers being ordered.
    ///
    /// Only the summable columns can differ. A PID or a status has no
    /// subtree meaning, so those compare the row either way.
    #[must_use]
    pub fn compare_visible(
        self,
        a: (&ProcessRow, Option<&Totals>),
        b: (&ProcessRow, Option<&Totals>),
        descending: bool,
    ) -> Ordering {
        let primary = self.primary_visible(a, b);
        let primary = if descending {
            primary.reverse()
        } else {
            primary
        };
        // The tie-break stays ascending and stays on the row's own
        // identity — see `compare_directed`.
        primary.then_with(|| tie_break(a.0, b.0))
    }

    /// [`SortKey::compare_visible`]'s primary key, with no tie-break.
    #[must_use]
    fn primary_visible(
        self,
        a: (&ProcessRow, Option<&Totals>),
        b: (&ProcessRow, Option<&Totals>),
    ) -> Ordering {
        match self {
            Self::Cpu => compare_number(
                a.1.map_or(a.0.cpu_percent, |t| t.cpu_percent),
                b.1.map_or(b.0.cpu_percent, |t| t.cpu_percent),
            ),
            Self::Memory => {
                a.1.map_or(a.0.working_set, |t| t.working_set)
                    .cmp(&b.1.map_or(b.0.working_set, |t| t.working_set))
            }
            Self::PrivateBytes => {
                a.1.map_or(a.0.private_bytes, |t| t.private_bytes)
                    .cmp(&b.1.map_or(b.0.private_bytes, |t| t.private_bytes))
            }
            Self::Disk => compare_number(
                a.1.map_or(a.0.disk_rate(), |t| t.disk_rate),
                b.1.map_or(b.0.disk_rate(), |t| t.disk_rate),
            ),
            Self::Network => {
                a.1.map_or(a.0.connections, |t| t.connections)
                    .cmp(&b.1.map_or(b.0.connections, |t| t.connections))
            }
            Self::Gpu => compare_number(
                a.1.map_or(a.0.gpu_percent, |t| t.gpu_percent),
                b.1.map_or(b.0.gpu_percent, |t| t.gpu_percent),
            ),
            // No subtree meaning. Named rather than wildcarded so a
            // column added later has to decide which of the two it is.
            Self::Name
            | Self::Pid
            | Self::Status
            | Self::User
            | Self::Threads
            | Self::Handles
            | Self::CpuTime
            | Self::Priority
            | Self::Architecture
            | Self::Session
            | Self::Path => self.primary(a.0, b.0),
        }
    }

    /// This column's own comparison, with no tie-break applied.
    #[must_use]
    fn primary(self, a: &ProcessRow, b: &ProcessRow) -> Ordering {
        match self {
            Self::Name => compare_text(a.display_name(), b.display_name()),
            Self::Pid => a.pid.cmp(&b.pid),
            Self::Status => a.status.cmp(&b.status),
            Self::Cpu => compare_number(a.cpu_percent, b.cpu_percent),
            Self::Memory => a.working_set.cmp(&b.working_set),
            Self::PrivateBytes => a.private_bytes.cmp(&b.private_bytes),
            Self::Disk => compare_number(a.disk_rate(), b.disk_rate()),
            Self::Network => a.connections.cmp(&b.connections),
            Self::Gpu => compare_number(a.gpu_percent, b.gpu_percent),
            Self::User => compare_text(&a.user, &b.user),
            Self::Threads => a.thread_count.cmp(&b.thread_count),
            Self::Handles => a.handle_count.cmp(&b.handle_count),
            Self::CpuTime => a.cpu_time_ms.cmp(&b.cpu_time_ms),
            Self::Priority => a.priority.cmp(&b.priority),
            Self::Architecture => a.architecture.cmp(&b.architecture),
            Self::Session => a.session_id.cmp(&b.session_id),
            Self::Path => compare_text(&path_text(a), &path_text(b)),
        }
    }

    /// Sorts `rows` in place by this column, in the requested direction.
    ///
    /// `sort_unstable_by` rather than `sort_by`: [`SortKey::compare_directed`]
    /// is already a total order, so stability buys nothing, and the
    /// unstable sort neither allocates nor does the extra work. The point
    /// is worth making explicitly because "use a stable sort so the list
    /// does not jump" is the obvious wrong answer here — see the module
    /// docs.
    pub fn sort(self, rows: &mut [ProcessRow], descending: bool) {
        rows.sort_unstable_by(|a, b| self.compare_directed(a, b, descending));
    }
}

/// The tie-break every comparison ends in: name, then PID.
///
/// PID last because it is the only field two rows in one snapshot can
/// never share, which is what makes the whole ordering total. See the
/// module docs.
fn tie_break(a: &ProcessRow, b: &ProcessRow) -> Ordering {
    compare_text(a.display_name(), b.display_name()).then_with(|| a.pid.cmp(&b.pid))
}

/// Case-insensitive comparison, falling back to the case-sensitive one so
/// that names differing only in case still order deterministically.
///
/// Case-insensitive because a list where `Chrome` sorts before `about` —
/// which is what byte order gives you — reads as broken. ASCII-only
/// folding: full Unicode case folding needs a table this crate has no
/// other use for, and process names that differ only in the case of a
/// non-ASCII letter do not occur in practice.
///
/// `pub(crate)` rather than private: the Services and Startup views sort
/// their own short lists by the same rule and would otherwise carry a
/// second copy of it.
pub(crate) fn compare_text(a: &str, b: &str) -> Ordering {
    let folded = a
        .chars()
        .map(|c| c.to_ascii_lowercase())
        .cmp(b.chars().map(|c| c.to_ascii_lowercase()));
    folded.then_with(|| a.cmp(b))
}

/// Compares two rates or percentages, ordering a `NaN` as smaller than
/// everything.
///
/// A `NaN` cannot reach here through [`crate::engine::rates`], which
/// guards its own divisions — but `partial_cmp` returning `None` is a
/// case the type system makes us handle, and the two available answers
/// are "treat it as the smallest value" or "declare the rows equal".
/// Equal is the wrong one: it would break transitivity, and
/// `sort_unstable_by` with an intransitive comparator does not merely
/// produce a strange order, it can read out of bounds. Ordering it as
/// smallest keeps the comparator a total order no matter what arrives.
fn compare_number(a: f64, b: f64) -> Ordering {
    match (a.is_nan(), b.is_nan()) {
        (true, true) => Ordering::Equal,
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
        (false, false) => a.partial_cmp(&b).unwrap_or(Ordering::Equal),
    }
}

/// A row's path as sortable text, with a missing path sorting first.
fn path_text(row: &ProcessRow) -> String {
    row.path
        .as_ref()
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ProcessStatus;

    fn row(name: &str, pid: u32, cpu: f64) -> ProcessRow {
        ProcessRow {
            pid,
            name: name.to_string(),
            cpu_percent: cpu,
            ..ProcessRow::default()
        }
    }

    #[test]
    fn rows_tied_on_the_sorted_column_still_have_one_determined_order() {
        // The reshuffling bug in one test: on an idle machine every row
        // ties at 0% CPU, and the order must not depend on the order the
        // kernel returned them in.
        let kernel_order_a = vec![
            row("b.exe", 2, 0.0),
            row("a.exe", 1, 0.0),
            row("c.exe", 3, 0.0),
        ];
        let kernel_order_b = vec![
            row("c.exe", 3, 0.0),
            row("b.exe", 2, 0.0),
            row("a.exe", 1, 0.0),
        ];

        let sorted = |mut rows: Vec<ProcessRow>| {
            SortKey::Cpu.sort(&mut rows, true);
            rows.iter().map(|r| r.pid).collect::<Vec<_>>()
        };
        assert_eq!(
            sorted(kernel_order_a),
            sorted(kernel_order_b),
            "two samples containing the same tied rows in different orders \
             must sort identically, or the table reshuffles every second"
        );
    }

    #[test]
    fn two_processes_sharing_a_name_are_ordered_by_pid() {
        // Eighteen `chrome.exe` rows all at 0% is the common case, and
        // the name tie-break alone does not separate them.
        let mut rows = vec![
            row("chrome.exe", 30, 0.0),
            row("chrome.exe", 10, 0.0),
            row("chrome.exe", 20, 0.0),
        ];
        SortKey::Memory.sort(&mut rows, true);
        assert_eq!(
            rows.iter().map(|r| r.pid).collect::<Vec<_>>(),
            vec![10, 20, 30],
            "identically named rows fall through to the PID, which is unique"
        );
    }

    #[test]
    fn a_descending_sort_reverses_the_column_but_not_the_tie_break() {
        let make = || {
            vec![
                row("a.exe", 1, 5.0),
                row("b.exe", 2, 50.0),
                row("c.exe", 3, 0.5),
            ]
        };
        let mut up = make();
        let mut down = make();
        SortKey::Cpu.sort(&mut up, false);
        SortKey::Cpu.sort(&mut down, true);
        let up_pids: Vec<u32> = up.iter().map(|r| r.pid).collect();
        let mut down_pids: Vec<u32> = down.iter().map(|r| r.pid).collect();
        down_pids.reverse();
        assert_eq!(
            up_pids, down_pids,
            "with no ties present, the two directions must be exact \
             reverses of one another"
        );
    }

    #[test]
    fn a_descending_magnitude_sort_leaves_its_ties_alphabetical() {
        // The case the whole `compare_directed` split exists for. On an
        // idle machine almost every row ties at 0% CPU, so the tie-break
        // orders nearly the whole table — and a reversed one would list
        // it from `zoom.exe` upwards, which reads as unsorted.
        let mut rows = vec![
            row("zoom.exe", 3, 0.0),
            row("atom.exe", 1, 0.0),
            row("busy.exe", 2, 90.0),
            row("mid.exe", 4, 0.0),
        ];
        SortKey::Cpu.sort(&mut rows, true);
        let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["busy.exe", "atom.exe", "mid.exe", "zoom.exe"],
            "the busy row sorts to the top and the ties below it stay \
             alphabetical"
        );
    }

    #[test]
    fn magnitudes_open_descending_and_names_open_ascending() {
        assert!(
            SortKey::Cpu.defaults_descending(),
            "clicking CPU is asking which process is busiest"
        );
        assert!(SortKey::Memory.defaults_descending());
        assert!(
            !SortKey::Name.defaults_descending(),
            "a name column that opens Z-to-A reads as broken"
        );
        assert!(!SortKey::Path.defaults_descending());
    }

    #[test]
    fn names_sort_case_insensitively() {
        let mut rows = vec![row("Zebra.exe", 1, 0.0), row("apple.exe", 2, 0.0)];
        SortKey::Name.sort(&mut rows, false);
        assert_eq!(
            rows[0].name, "apple.exe",
            "byte order would put every capitalised name before every \
             lowercase one, which reads as an unsorted list"
        );
    }

    #[test]
    fn names_differing_only_in_case_still_order_deterministically() {
        let a = row("Setup.exe", 1, 0.0);
        let b = row("setup.exe", 2, 0.0);
        assert_ne!(
            SortKey::Name.compare(&a, &b),
            Ordering::Equal,
            "the case-sensitive fallback must break the fold's tie before \
             the PID has to"
        );
    }

    #[test]
    fn a_nan_rate_cannot_break_the_comparator() {
        // An intransitive comparator does not merely give a strange
        // order — `sort_unstable_by` can read out of bounds. This checks
        // the ordering stays total with a NaN in it.
        let nan = row("nan.exe", 1, f64::NAN);
        let zero = row("zero.exe", 2, 0.0);
        let big = row("big.exe", 3, 99.0);

        assert_eq!(SortKey::Cpu.compare(&nan, &zero), Ordering::Less);
        assert_eq!(SortKey::Cpu.compare(&zero, &nan), Ordering::Greater);
        assert_eq!(SortKey::Cpu.compare(&nan, &big), Ordering::Less);
        // Transitivity across the three.
        assert_eq!(SortKey::Cpu.compare(&zero, &big), Ordering::Less);

        let mut rows = vec![big, nan, zero];
        SortKey::Cpu.sort(&mut rows, true);
        assert_eq!(
            rows.iter().map(|r| r.pid).collect::<Vec<_>>(),
            vec![3, 2, 1],
            "a NaN sorts as the smallest value rather than as equal to \
             everything"
        );
    }

    #[test]
    fn every_column_produces_a_total_order() {
        // Walked over every key so a column added later cannot quietly
        // skip the tie-break.
        let keys = [
            SortKey::Name,
            SortKey::Pid,
            SortKey::Status,
            SortKey::Cpu,
            SortKey::Memory,
            SortKey::PrivateBytes,
            SortKey::Disk,
            SortKey::Network,
            SortKey::Gpu,
            SortKey::User,
            SortKey::Threads,
            SortKey::Handles,
            SortKey::CpuTime,
            SortKey::Priority,
            SortKey::Architecture,
            SortKey::Session,
            SortKey::Path,
        ];
        // Rows that are identical except for their PID: every column
        // except PID itself ties on its own field, so this exercises the
        // tie-break for all of them.
        let rows: Vec<ProcessRow> = (1..=5)
            .map(|pid| ProcessRow {
                pid,
                name: "same.exe".to_string(),
                status: ProcessStatus::Running,
                ..ProcessRow::default()
            })
            .collect();
        for key in keys {
            for a in &rows {
                for b in &rows {
                    let forward = key.compare(a, b);
                    let backward = key.compare(b, a);
                    for descending in [false, true] {
                        assert_eq!(
                            key.compare_directed(a, b, descending),
                            key.compare_directed(b, a, descending).reverse(),
                            "{} is not antisymmetric when directed",
                            key.label()
                        );
                    }
                    assert_eq!(
                        forward,
                        backward.reverse(),
                        "{} is not antisymmetric for pids {} and {}",
                        key.label(),
                        a.pid,
                        b.pid
                    );
                    if a.pid != b.pid {
                        assert_ne!(
                            forward,
                            Ordering::Equal,
                            "{} left two distinct rows tied, so their order \
                             would depend on the kernel's",
                            key.label()
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn a_missing_path_sorts_before_every_real_one() {
        use std::path::PathBuf;
        let mut with = row("a.exe", 1, 0.0);
        with.path = Some(PathBuf::from("C:\\Windows\\a.exe"));
        let without = row("b.exe", 2, 0.0);
        assert_eq!(
            SortKey::Path.compare(&without, &with),
            Ordering::Less,
            "the protected processes whose path cannot be read should \
             gather at one end rather than scattering"
        );
    }
}

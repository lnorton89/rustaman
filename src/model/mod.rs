// ============================================================================
// Module:       model
// Description:  The shape of one sample: every process, every system counter,
//               and the identity rules that make two samples comparable.
//
// Dependencies: std only; sibling modules for history, sorting, filtering, and
//               the process tree. No Windows types cross into here.
// ============================================================================

//! What a sample of the machine looks like, as plain data.
//!
//! The sampler in [`crate::engine`] produces a [`Snapshot`] on an
//! interval and the GUI reads it. Nothing here calls Windows, and no
//! Windows type appears in any signature — [`crate::win`] converts at its
//! own boundary. That is what lets the sorting, the filtering, the tree
//! grouping, and the rate arithmetic below be tested on a machine that is
//! not Windows at all, which is most of the logic that can actually be
//! wrong.
//!
//! ## Identity, and why a PID is not it
//!
//! A PID is not a process. Windows reuses them aggressively — the
//! numbers come from a pool, and on a busy machine a PID can be freed and
//! handed out again within a single one-second sample. Every part of this
//! app that compares two samples therefore keys on [`ProcessKey`], which
//! is the PID *and* the creation time, and creation time is a 100ns
//! FILETIME so two processes cannot share one.
//!
//! Getting this wrong is not cosmetic. CPU percentage is a delta of
//! cumulative CPU time between samples; matching last sample's `chrome.exe`
//! against this sample's freshly-created *different* process at the same
//! PID subtracts unrelated numbers, and the result is a negative delta
//! that clamps to zero — or, if the new process happens to have used more
//! CPU already, a wildly overstated spike. Worse, a "kill" issued against
//! a row identified only by PID can land on whatever now holds that
//! number. Selection, expansion, the history graphs, and the kill path
//! all key on [`ProcessKey`] for that reason.

pub mod filter;
pub mod history;
pub mod sort;
pub mod tree;

use std::path::PathBuf;
use std::time::Duration;

/// One process, as of one sample.
///
/// Everything the process table can show about a row, already merged:
/// the per-sample counters the kernel reports plus the identity fields
/// the engine resolves once and caches (see [`crate::engine`]). The GUI
/// never has to ask a second question to draw a row.
#[derive(Clone, Debug, Default)]
pub struct ProcessRow {
    /// Process id. Not an identity on its own — see the module docs.
    pub pid: u32,
    /// The creating process's id, for the tree. Zero, or a PID that is
    /// not in this snapshot, means the parent has exited: the row is
    /// re-rooted rather than hidden. See [`tree`].
    pub parent_pid: u32,
    /// Creation time, as a Windows FILETIME (100ns ticks since 1601).
    /// The other half of [`ProcessKey`].
    pub started_at: u64,
    /// Image file name, e.g. `chrome.exe`.
    pub name: String,
    /// `FileDescription` from the executable's version resource, e.g.
    /// "Google Chrome". Empty when the binary carries no version
    /// resource, which is normal for a lot of system and developer
    /// tooling — the UI falls back to [`ProcessRow::name`] rather than
    /// showing a blank.
    pub description: String,
    /// Full path to the executable, when it could be read. `None` for
    /// protected processes and for the two pseudo-processes (see
    /// [`ProcessRow::is_pseudo`]).
    pub path: Option<PathBuf>,
    /// Owning account, `DOMAIN\user`. Empty when the token could not be
    /// opened, which happens for protected system processes even with
    /// `SeDebugPrivilege`.
    pub user: String,
    /// Terminal-services session. 0 is the services session; the
    /// interactive desktop is normally 1.
    pub session_id: u32,
    /// Which of the three groups the process list files this under.
    pub kind: ProcessKind,
    /// Whether the process runs with an elevated token.
    pub elevated: bool,
    /// 32- or 64-bit, as far as it could be determined.
    pub architecture: Architecture,
    /// The title of one visible top-level window owned by this process,
    /// if it has one. Also what makes it an [`ProcessKind::App`].
    pub window_title: Option<String>,
    /// Running, or suspended in whole.
    pub status: ProcessStatus,
    /// Share of the *whole machine's* CPU capacity, 0..=100 — so a
    /// process saturating one core of eight reads 12.5%, matching Task
    /// Manager. See [`crate::engine::rates`].
    pub cpu_percent: f64,
    /// Cumulative CPU time (kernel + user) in milliseconds.
    pub cpu_time_ms: u64,
    /// Physical memory currently mapped for this process, including
    /// shared pages. The number Task Manager's "Memory" column shows.
    pub working_set: u64,
    /// Private commit — memory this process cannot share with another.
    /// The more honest number of the two, and the one to sort by when
    /// hunting a leak.
    pub private_bytes: u64,
    /// Reserved plus committed address space. Large and mostly
    /// meaningless on 64-bit; kept because it is the one number that
    /// exposes a runaway reservation.
    pub virtual_bytes: u64,
    /// Threads in the process.
    pub thread_count: u32,
    /// Open kernel handles. A number that only ever climbs is the
    /// clearest handle-leak signal there is.
    pub handle_count: u32,
    /// Bytes per second read from disk over the last interval.
    pub disk_read_rate: f64,
    /// Bytes per second written to disk over the last interval.
    pub disk_write_rate: f64,
    /// Cumulative bytes read, since the process started.
    pub io_read_bytes: u64,
    /// Cumulative bytes written, since the process started.
    pub io_write_bytes: u64,
    /// Open TCP and UDP endpoints owned by this process.
    ///
    /// A count, not a throughput. Per-process network *bytes* is not
    /// available from any Win32 call — Task Manager gets it from an ETW
    /// kernel session, which costs far more than this app is willing to
    /// spend on one column. See `docs/WINDOWS_APIS.md`.
    pub connections: u32,
    /// Share of GPU engine time, 0..=100, summed across engines.
    pub gpu_percent: f64,
    /// Dedicated GPU memory attributed to this process.
    pub gpu_memory: u64,
    /// Scheduling priority class.
    pub priority: Priority,
}

impl ProcessRow {
    /// This row's stable identity across samples.
    #[must_use]
    pub fn key(&self) -> ProcessKey {
        ProcessKey {
            pid: self.pid,
            started_at: self.started_at,
        }
    }

    /// The name to show: the version resource's description when there is
    /// one, the image name otherwise.
    ///
    /// A single method rather than the decision being made at each call
    /// site, because the table, the tooltip, the kill confirmation, and
    /// the copied text all have to name the same process the same way —
    /// a confirmation that says "End Google Chrome?" for a row labelled
    /// `chrome.exe` is the kind of mismatch that makes someone kill the
    /// wrong thing.
    #[must_use]
    pub fn display_name(&self) -> &str {
        if self.description.is_empty() {
            &self.name
        } else {
            &self.description
        }
    }

    /// Combined disk throughput, which is what the single "Disk" column
    /// shows.
    #[must_use]
    pub fn disk_rate(&self) -> f64 {
        self.disk_read_rate + self.disk_write_rate
    }

    /// Whether this is one of the two kernel pseudo-processes.
    ///
    /// PID 0 (`System Idle Process`) and PID 4 (`System`) are not
    /// processes in the sense the rest of this app means. They cannot be
    /// opened, terminated, suspended, or have their priority changed, and
    /// the idle process's "CPU time" is the machine's idle time — showing
    /// it in the CPU column would report an idle machine as 100% busy.
    /// Every action path checks this, and [`crate::engine`] excludes the
    /// idle process's CPU from the totals.
    #[must_use]
    pub fn is_pseudo(&self) -> bool {
        self.pid == IDLE_PID || self.pid == SYSTEM_PID
    }
}

/// The PID of the System Idle Process. Its "CPU time" is the machine's
/// idle time; see [`ProcessRow::is_pseudo`].
pub const IDLE_PID: u32 = 0;

/// The PID of the System process, which hosts kernel-mode threads.
pub const SYSTEM_PID: u32 = 4;

/// A process's identity across samples: PID plus creation time.
///
/// See the module docs for why the PID alone will not do.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub struct ProcessKey {
    /// The process id.
    pub pid: u32,
    /// Creation time as a FILETIME, which disambiguates a reused PID.
    pub started_at: u64,
}

/// Which group the process list files a row under.
///
/// The same three-way split Task Manager uses, and for the same reason:
/// a flat list of four hundred rows buries the six the user came to
/// find. What differs here is that the rule is stated rather than
/// guessed — see [`ProcessKind::classify`].
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub enum ProcessKind {
    /// Owns at least one visible top-level window: something the user
    /// started and can see.
    App,
    /// Runs in the user's own session with no window.
    #[default]
    Background,
    /// Runs in session 0 or under a system account: the OS itself.
    System,
}

impl ProcessKind {
    /// Every kind, in the order the process list groups them.
    pub const ALL: [Self; 3] = [Self::App, Self::Background, Self::System];

    /// The heading this group gets.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::App => "Apps",
            Self::Background => "Background processes",
            Self::System => "Windows processes",
        }
    }

    /// Decides a process's group.
    ///
    /// Order matters. A window wins over everything: an elevated app
    /// running in session 0 is vanishingly rare, but an app *is* what the
    /// user is looking at, so a visible window settles it first. Session 0
    /// is next, because that is the services session and nothing the user
    /// launched lives there. The remaining test is the account, which is
    /// what catches a `SYSTEM`-owned helper running in the interactive
    /// session — those are the OS's, not the user's, whatever session
    /// they landed in.
    #[must_use]
    pub fn classify(has_window: bool, session_id: u32, user: &str) -> Self {
        if has_window {
            return Self::App;
        }
        if session_id == 0 {
            return Self::System;
        }
        if is_system_account(user) {
            return Self::System;
        }
        Self::Background
    }
}

/// Whether an account name is one of the built-in service identities.
///
/// Compared case-insensitively against the un-domain-qualified name,
/// because the domain part is localised — a German install reports
/// `NT-AUTORITÄT\SYSTEM` — while the account name itself is not.
fn is_system_account(user: &str) -> bool {
    let bare = user.rsplit('\\').next().unwrap_or(user);
    const SYSTEM_ACCOUNTS: [&str; 3] = ["SYSTEM", "LOCAL SERVICE", "NETWORK SERVICE"];
    SYSTEM_ACCOUNTS
        .iter()
        .any(|account| bare.eq_ignore_ascii_case(account))
}

/// Whether a process is running or suspended.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub enum ProcessStatus {
    /// At least one thread is not in a suspended wait.
    #[default]
    Running,
    /// Every thread is suspended — either by this app's Suspend action or,
    /// far more often, by the OS: a store app the shell has put to sleep
    /// to save power sits like this, and reporting it as "running" while
    /// it uses no CPU is the confusing part, not the suspension.
    Suspended,
}

impl ProcessStatus {
    /// The word shown in the Status column.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Running => "Running",
            Self::Suspended => "Suspended",
        }
    }
}

/// The bitness of a process's image.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub enum Architecture {
    /// Could not be determined — normally a protected process the app
    /// cannot open.
    #[default]
    Unknown,
    /// A 32-bit process, running under WOW64 on a 64-bit Windows.
    X86,
    /// A 64-bit process.
    X64,
    /// An ARM64 process.
    Arm64,
}

impl Architecture {
    /// The short label the Details view shows.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Unknown => crate::format::DASH,
            Self::X86 => "x86",
            Self::X64 => "x64",
            Self::Arm64 => "ARM64",
        }
    }
}

/// A scheduling priority class.
///
/// The six Windows exposes, in ascending order, so the enum's own
/// ordering is the priority ordering and the Details view's sort needs no
/// separate table. `Realtime` is included because Windows has it, but see
/// [`Priority::is_dangerous`].
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub enum Priority {
    /// Runs only when nothing else wants the CPU.
    Idle,
    /// Below normal.
    BelowNormal,
    /// The default for a process started from the shell.
    #[default]
    Normal,
    /// Above normal.
    AboveNormal,
    /// High.
    High,
    /// Above every ordinary thread on the machine, including most of the
    /// kernel's own. See [`Priority::is_dangerous`].
    Realtime,
}

impl Priority {
    /// Every class, lowest first — the order the priority menu lists them
    /// in, reversed for display so High is at the top where a mouse
    /// reaches it.
    pub const ALL: [Self; 6] = [
        Self::Idle,
        Self::BelowNormal,
        Self::Normal,
        Self::AboveNormal,
        Self::High,
        Self::Realtime,
    ];

    /// The label shown in the menu and the Details column.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Idle => "Low",
            Self::BelowNormal => "Below normal",
            Self::Normal => "Normal",
            Self::AboveNormal => "Above normal",
            Self::High => "High",
            Self::Realtime => "Realtime",
        }
    }

    /// Whether setting this class warrants a confirmation.
    ///
    /// `Realtime` schedules a process above most kernel threads,
    /// including the ones servicing input and paging. A busy loop at
    /// realtime priority can make a machine stop responding to the
    /// keyboard entirely, with no way back short of the power button —
    /// so the UI asks first. Nothing below it can do that.
    #[must_use]
    pub fn is_dangerous(self) -> bool {
        matches!(self, Self::Realtime)
    }
}

/// One complete sample of the machine.
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    /// A monotonically increasing count, so the UI can tell a fresh
    /// snapshot from the one it already drew without comparing the
    /// contents.
    pub sequence: u64,
    /// How long the interval this snapshot's rates were computed over
    /// actually was. Not the configured interval: a sampler thread that
    /// was descheduled took longer, and dividing by the nominal figure
    /// would overstate every rate on a loaded machine — precisely when
    /// the numbers matter most.
    pub interval: Duration,
    /// Every process on the machine, in no particular order. Sorting is
    /// the view's business; see [`sort`].
    pub processes: Vec<ProcessRow>,
    /// The system-wide counters.
    pub system: SystemSample,
}

/// The system-wide counters, as of one sample.
#[derive(Clone, Debug, Default)]
pub struct SystemSample {
    /// Processor utilisation and the machine's static CPU facts.
    pub cpu: CpuSample,
    /// Physical and committed memory.
    pub memory: MemorySample,
    /// One entry per physical disk.
    pub disks: Vec<DiskSample>,
    /// One entry per network adapter that is up and not a loopback.
    pub adapters: Vec<AdapterSample>,
    /// One entry per GPU adapter the performance counters expose.
    pub gpus: Vec<GpuSample>,
    /// Seconds since the machine booted.
    pub uptime_seconds: u64,
    /// Total processes in this snapshot, including the pseudo-processes.
    pub process_count: usize,
    /// Total threads across every process.
    pub thread_count: u64,
    /// Total open handles across every process.
    pub handle_count: u64,
}

/// Processor utilisation, and the static facts about the CPU.
#[derive(Clone, Debug, Default)]
pub struct CpuSample {
    /// Overall utilisation across the machine, 0..=100.
    pub total_percent: f64,
    /// Per-logical-processor utilisation, 0..=100 each. One entry per
    /// core, which is what the Performance page's core grid draws.
    pub per_core: Vec<f64>,
    /// The share of [`CpuSample::total_percent`] that was kernel-mode.
    /// Drawn as a darker band under the total, which is the fastest way
    /// to spot a driver or antivirus filter eating the machine.
    pub kernel_percent: f64,
    /// Marketing name, e.g. "AMD Ryzen 9 5950X 16-Core Processor".
    pub name: String,
    /// Physical cores, where it could be determined.
    pub physical_cores: usize,
    /// Logical processors — [`CpuSample::per_core`]'s length.
    pub logical_cores: usize,
    /// Nominal clock in MHz, from the registry. Nominal, not current:
    /// the current frequency needs a counter that costs a great deal
    /// more to read than it is worth.
    pub megahertz: u32,
}

/// Physical and committed memory.
#[derive(Clone, Debug, Default)]
pub struct MemorySample {
    /// Installed physical memory, in bytes.
    pub total: u64,
    /// Physical memory available for allocation without paging.
    pub available: u64,
    /// Committed bytes — memory the system has promised to back.
    pub committed: u64,
    /// The most that can be committed before allocations start failing.
    pub commit_limit: u64,
    /// The standby and modified lists: physical memory holding cached
    /// file data, reclaimable on demand.
    pub cached: u64,
    /// Kernel paged pool.
    pub paged_pool: u64,
    /// Kernel non-paged pool. A number that climbs without bound here is
    /// a driver leak, and it is the one leak that takes the machine down
    /// rather than just the process.
    pub nonpaged_pool: u64,
}

impl MemorySample {
    /// Physical memory in use, in bytes.
    #[must_use]
    pub fn used(&self) -> u64 {
        self.total.saturating_sub(self.available)
    }

    /// Physical memory in use, 0..=100.
    #[must_use]
    pub fn used_percent(&self) -> f64 {
        percent_of(self.used(), self.total)
    }
}

/// One physical disk's activity over the interval.
#[derive(Clone, Debug, Default)]
pub struct DiskSample {
    /// The physical drive number, as in `\\.\PhysicalDrive0`.
    pub index: u32,
    /// A readable name: the drive letters on it, or the model.
    pub name: String,
    /// Bytes per second read over the interval.
    pub read_rate: f64,
    /// Bytes per second written over the interval.
    pub write_rate: f64,
    /// Share of the interval the disk had at least one request in
    /// flight, 0..=100. This is Task Manager's "Active time", and it is
    /// the number that actually tells you whether the disk is the
    /// bottleneck — throughput does not, because a queue of small random
    /// reads can saturate a disk at a trivial byte rate.
    pub active_percent: f64,
    /// Total capacity in bytes, summed across the volumes on this disk.
    pub capacity: u64,
    /// Free space in bytes, summed across the same volumes.
    pub free: u64,
}

impl DiskSample {
    /// Combined throughput, for the single-column summary.
    #[must_use]
    pub fn total_rate(&self) -> f64 {
        self.read_rate + self.write_rate
    }
}

/// One network adapter's throughput over the interval.
#[derive(Clone, Debug, Default)]
pub struct AdapterSample {
    /// The adapter's friendly name, e.g. "Ethernet" or "Wi-Fi".
    pub name: String,
    /// Bytes per second received.
    pub receive_rate: f64,
    /// Bytes per second sent.
    pub send_rate: f64,
    /// Nominal link speed in bits per second, for the graph's scale.
    pub link_speed: u64,
}

impl AdapterSample {
    /// Combined throughput.
    #[must_use]
    pub fn total_rate(&self) -> f64 {
        self.receive_rate + self.send_rate
    }
}

/// One GPU adapter's utilisation over the interval.
#[derive(Clone, Debug, Default)]
pub struct GpuSample {
    /// The adapter's LUID as text, which is how the performance counters
    /// name it and the only stable identifier they offer.
    pub luid: String,
    /// A readable name, from the display adapter registry key.
    pub name: String,
    /// Busiest engine's utilisation, 0..=100.
    ///
    /// The *maximum* across engines rather than the sum, which is what
    /// Task Manager reports and the only figure that means anything: a
    /// GPU's 3D, copy, video-decode and compute engines run in parallel,
    /// so summing them yields percentages well over 100 for a machine
    /// that is merely playing a video while it renders.
    pub utilisation: f64,
    /// Per-engine utilisation, keyed by engine type name.
    pub engines: Vec<(String, f64)>,
    /// Dedicated video memory in use, in bytes.
    pub memory_used: u64,
    /// Dedicated video memory installed, in bytes.
    pub memory_total: u64,
}

/// A part over a whole as a percentage, with a zero whole yielding zero
/// rather than a `NaN` that would propagate into a graph and a label.
///
/// Free functions rather than a method because both integer counters and
/// already-floating rates need it.
#[must_use]
pub fn percent_of(part: u64, whole: u64) -> f64 {
    if whole == 0 {
        return 0.0;
    }
    (part as f64 / whole as f64) * 100.0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A row with just enough filled in to exercise one rule.
    fn row(pid: u32, started_at: u64) -> ProcessRow {
        ProcessRow {
            pid,
            started_at,
            name: "test.exe".to_string(),
            ..ProcessRow::default()
        }
    }

    #[test]
    fn a_reused_pid_is_a_different_process() {
        // The case the whole identity scheme exists for: same PID,
        // different creation time. If these compared equal, a CPU delta
        // would be taken between two unrelated processes.
        let first = row(4242, 132_000_000_000_000_000);
        let second = row(4242, 132_000_000_000_000_001);
        assert_ne!(
            first.key(),
            second.key(),
            "a PID reused within a sample must not be treated as the same \
             process"
        );
        assert_eq!(first.key(), row(4242, first.started_at).key());
    }

    #[test]
    fn display_name_prefers_the_version_resource_but_never_shows_a_blank() {
        let mut process = row(1, 1);
        assert_eq!(
            process.display_name(),
            "test.exe",
            "a binary with no version resource falls back to its file name"
        );
        process.description = "Test Application".to_string();
        assert_eq!(process.display_name(), "Test Application");
    }

    #[test]
    fn the_kernel_pseudo_processes_are_recognised() {
        assert!(row(IDLE_PID, 0).is_pseudo(), "the idle process");
        assert!(row(SYSTEM_PID, 0).is_pseudo(), "the System process");
        assert!(
            !row(1234, 0).is_pseudo(),
            "an ordinary process must not be excluded from actions"
        );
    }

    #[test]
    fn a_window_settles_the_group_before_anything_else_is_considered() {
        assert_eq!(
            ProcessKind::classify(true, 0, "NT AUTHORITY\\SYSTEM"),
            ProcessKind::App,
            "a visible window is what the user is looking at, whatever \
             session or account it runs under"
        );
    }

    #[test]
    fn session_zero_and_the_service_accounts_are_the_system_group() {
        assert_eq!(
            ProcessKind::classify(false, 0, "DESKTOP\\alice"),
            ProcessKind::System,
            "nothing a user launched lives in session 0"
        );
        assert_eq!(
            ProcessKind::classify(false, 1, "NT AUTHORITY\\SYSTEM"),
            ProcessKind::System,
            "a SYSTEM-owned helper in the interactive session is still the \
             OS's, not the user's"
        );
        assert_eq!(
            ProcessKind::classify(false, 1, "DESKTOP\\alice"),
            ProcessKind::Background,
            "a windowless process of the user's own is a background task"
        );
    }

    #[test]
    fn a_localised_domain_prefix_does_not_hide_a_service_account() {
        // A German install reports NT-AUTORITÄT\SYSTEM. The domain half
        // is localised; the account name is not, which is why only the
        // bare name is compared.
        for user in [
            "NT AUTHORITY\\SYSTEM",
            "NT-AUTORITÄT\\SYSTEM",
            "AUTORITE NT\\SYSTEM",
            "system",
            "NT AUTHORITY\\LOCAL SERVICE",
            "NT AUTHORITY\\NETWORK SERVICE",
        ] {
            assert_eq!(
                ProcessKind::classify(false, 1, user),
                ProcessKind::System,
                "{user} should be recognised as a service account"
            );
        }
    }

    #[test]
    fn an_account_merely_containing_system_is_not_a_service_account() {
        // "systemtest" and a user actually called "System Administrator"
        // must not be swept into the Windows group by a substring match.
        for user in ["DESKTOP\\systemtest", "DESKTOP\\subsystem", "DESKTOP\\sys"] {
            assert_eq!(
                ProcessKind::classify(false, 1, user),
                ProcessKind::Background,
                "{user} is an ordinary account"
            );
        }
    }

    #[test]
    fn priority_orders_by_actual_priority() {
        assert!(
            Priority::Idle < Priority::Normal && Priority::Normal < Priority::Realtime,
            "the enum ordering is the priority ordering, which is what the \
             Details view sorts on"
        );
        assert!(
            Priority::ALL.windows(2).all(|pair| pair[0] < pair[1]),
            "ALL must be in ascending order"
        );
    }

    #[test]
    fn only_realtime_warrants_a_confirmation() {
        assert!(Priority::Realtime.is_dangerous());
        assert!(
            Priority::ALL
                .iter()
                .filter(|priority| priority.is_dangerous())
                .count()
                == 1,
            "exactly one class can make a machine stop answering the keyboard"
        );
    }

    #[test]
    fn a_percentage_of_nothing_is_zero_rather_than_nan() {
        // A machine reporting zero installed memory is impossible, but a
        // NaN reaching a graph is not: it propagates through the max, the
        // scale, and every subsequent point.
        assert_eq!(percent_of(0, 0), 0.0);
        assert_eq!(percent_of(5, 0), 0.0);
        assert!((percent_of(1, 4) - 25.0).abs() < f64::EPSILON);
    }

    #[test]
    fn memory_used_cannot_go_negative() {
        // `available` can momentarily exceed `total` on a machine with
        // memory being hot-added, and an underflowing subtraction would
        // wrap to sixteen exabytes in use.
        let sample = MemorySample {
            total: 100,
            available: 120,
            ..MemorySample::default()
        };
        assert_eq!(sample.used(), 0, "an underflow must saturate, not wrap");
        assert_eq!(sample.used_percent(), 0.0);
    }
}

// ============================================================================
// Module:       engine::sampler
// Description:  The sampler loop itself — one pass over every counter, merged
//               with cached identity into a Snapshot.
//
// Dependencies: crate::win (every subsystem), crate::model, crossbeam-channel
// ============================================================================

//! One pass of the sampler.
//!
//! [`run`] is the thread body: read everything, merge it, publish, sleep,
//! repeat. [`Sampler`] holds the state that has to survive between passes
//! — the previous counters every rate is a difference against, the
//! identity cache, the reusable query buffers, and the PDH session.
//!
//! ## What is read every pass and what is not
//!
//! Every pass reads the things that change: the process table, per-core
//! CPU, memory, disk, network, GPU. Once per pass, not once per process —
//! the per-PID lookups ([`crate::win::net::connections_by_pid`], the GPU
//! counters, the window titles) each produce a whole map in one call, and
//! rows index into it.
//!
//! What is *not* read every pass:
//!
//! - **Identity** — owner, path, bitness, description — is resolved once
//!   per process and cached. See [`crate::win::identity`].
//! - **The machine's own description** — CPU model, core counts — is read
//!   once at startup. It cannot change.
//! - **Services and startup entries** are read on demand by their views,
//!   not by the sampler. Neither changes on a one-second timescale, and
//!   enumerating four hundred services every second to redraw a list
//!   nobody is looking at is exactly the sort of thing that makes a
//!   monitoring tool a load of its own.
//!
//! ## The interval is measured, not assumed
//!
//! Every rate divides by the time that *actually* passed, taken from an
//! [`Instant`] either side of the sleep — never by the configured
//! interval. A sampler thread that was descheduled, or a machine that
//! resumed from sleep, makes those two numbers differ by a lot, and using
//! the nominal figure would overstate every rate on a loaded machine —
//! precisely when the numbers matter most.

use crate::model::rates::{Counters, Rates};
use crate::model::{
    CpuSample, DiskSample, GpuSample, Priority, ProcessKey, ProcessKind, ProcessRow, ProcessStatus,
    Snapshot, SystemSample,
};
use crate::win;
use crossbeam_channel::Sender;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// The thread body.
///
/// Runs until the receiver disconnects or `stopping` is set.
pub fn run(sender: &Sender<Snapshot>, interval_ms: &AtomicU64, stopping: &AtomicBool) {
    // Asked for once, at startup. Without it, identity lookups come back
    // empty for roughly half the machine — see `win::privilege`.
    let elevated = win::privilege::enable();

    let mut sampler = Sampler::new(elevated);
    let mut last = Instant::now();

    while !stopping.load(Ordering::Relaxed) {
        // Sleep first, so the first published snapshot already has a
        // previous sample behind it and its rates are real.
        //
        // Slept in short slices rather than one long one so that a
        // ten-second interval does not make quitting take ten seconds —
        // the window would stay on screen, unresponsive to its own close
        // button, while the thread finished a sleep nobody wants.
        let interval = Duration::from_millis(interval_ms.load(Ordering::Relaxed));
        if !sleep_interruptibly(interval, stopping) {
            return;
        }

        let now = Instant::now();
        let elapsed = now.saturating_duration_since(last);
        last = now;

        let snapshot = sampler.sample(elapsed);
        if !super::publish(sender, snapshot) {
            // The window has closed.
            return;
        }
    }
}

/// How long to sleep at a stretch before re-checking the stop flag.
///
/// Short enough that quitting is immediate at any interval; long enough
/// that the check costs nothing. See the wake-up comment in [`run`].
const SLEEP_SLICE: Duration = Duration::from_millis(100);

/// Sleeps for `interval`, returning `false` if asked to stop partway.
fn sleep_interruptibly(interval: Duration, stopping: &AtomicBool) -> bool {
    let deadline = Instant::now() + interval;
    while Instant::now() < deadline {
        if stopping.load(Ordering::Relaxed) {
            return false;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        std::thread::sleep(remaining.min(SLEEP_SLICE));
    }
    !stopping.load(Ordering::Relaxed)
}

/// The state one pass needs from the last one.
pub struct Sampler {
    /// Whether `SeDebugPrivilege` was granted, which the UI reports.
    elevated: bool,
    /// The machine's static description, read once.
    facts: win::system::Facts,
    /// Reusable buffer for the process query, so a 512 KB allocation is
    /// not made and freed every second.
    process_buffer: win::nt::InfoBuffer,
    /// Reusable buffer for the per-core CPU query.
    cpu_buffer: win::nt::InfoBuffer,
    /// Previous per-process counters, for the rate arithmetic.
    rates: Rates,
    /// Previous per-core CPU times.
    previous_cores: Vec<win::nt::cpu::CoreTimes>,
    /// Previous per-disk counters, keyed by drive number.
    previous_disks: HashMap<u32, win::disk::DiskCounters>,
    /// Previous per-adapter counters, keyed by interface LUID.
    previous_adapters: HashMap<u64, win::net::AdapterCounters>,
    /// Resolved identities.
    identity: win::identity::Cache,
    /// The GPU counter session, held open across passes because these are
    /// rate counters and a fresh query always reads zero. `None` on a
    /// machine that does not publish them.
    gpu: Option<win::gpu::Session>,
    /// Monotonic snapshot counter.
    sequence: u64,
}

impl Sampler {
    /// Prepares a sampler.
    #[must_use]
    pub fn new(elevated: bool) -> Self {
        Self {
            elevated,
            facts: win::system::Facts::read(),
            process_buffer: win::nt::InfoBuffer::new(),
            cpu_buffer: win::nt::InfoBuffer::new(),
            rates: Rates::new(),
            previous_cores: Vec::new(),
            previous_disks: HashMap::new(),
            previous_adapters: HashMap::new(),
            identity: win::identity::Cache::new(),
            gpu: win::gpu::Session::open(),
            sequence: 0,
        }
    }

    /// Whether `SeDebugPrivilege` was granted.
    #[must_use]
    pub fn is_elevated(&self) -> bool {
        self.elevated
    }

    /// Takes one sample.
    ///
    /// `elapsed` is the measured time since the previous sample; see the
    /// module docs on why it is not the configured interval.
    pub fn sample(&mut self, elapsed: Duration) -> Snapshot {
        self.sequence = self.sequence.saturating_add(1);

        let cores = self.facts.logical_cores.max(1);
        let processes = self.sample_processes(elapsed, cores);
        let system = self.sample_system(elapsed, &processes);

        Snapshot {
            sequence: self.sequence,
            interval: elapsed,
            processes,
            system,
        }
    }

    /// Reads and merges every process.
    fn sample_processes(&mut self, elapsed: Duration, cores: usize) -> Vec<ProcessRow> {
        let Ok(raw) = win::nt::process::enumerate(&mut self.process_buffer) else {
            // A failed enumeration yields an empty list rather than a
            // stale one: showing a process table that is quietly frozen
            // is worse than showing an empty one, because nothing about
            // it says the numbers have stopped moving.
            return Vec::new();
        };

        // The per-PID maps, each produced by one call rather than one
        // call per row.
        let titles = win::windows::titles_by_pid();
        let connections = win::net::connections_by_pid();
        let gpu = self.gpu.as_mut().map(win::gpu::Session::read);

        let live: HashSet<ProcessKey> = raw
            .iter()
            .map(|process| ProcessKey {
                pid: process.pid,
                started_at: process.started_at,
            })
            .collect();

        let mut rows = Vec::with_capacity(raw.len());
        for process in raw {
            let key = ProcessKey {
                pid: process.pid,
                started_at: process.started_at,
            };
            let delta = self.rates.observe(
                key,
                Counters {
                    cpu_ticks: process.cpu_ticks,
                    read_bytes: process.read_bytes,
                    write_bytes: process.write_bytes,
                },
                elapsed,
                cores,
            );

            // The pseudo-processes are not opened for identity: PID 0 and
            // PID 4 refuse, and asking every sample would be a wasted
            // call each time.
            let identity =
                if key.pid == crate::model::IDLE_PID || key.pid == crate::model::SYSTEM_PID {
                    win::identity::Identity::default()
                } else {
                    self.identity.get(key)
                };

            let window_title = titles.get(&process.pid).cloned();
            let kind =
                ProcessKind::classify(window_title.is_some(), process.session_id, &identity.user);

            rows.push(ProcessRow {
                pid: process.pid,
                parent_pid: process.parent_pid,
                started_at: process.started_at,
                name: process.name,
                description: identity.description,
                path: identity.path,
                user: identity.user,
                session_id: process.session_id,
                kind,
                elevated: identity.elevated,
                architecture: identity.architecture,
                window_title,
                status: if process.suspended {
                    ProcessStatus::Suspended
                } else {
                    ProcessStatus::Running
                },
                // The idle process's "CPU time" is the machine's idle
                // time. Reporting it would show an idle machine's busiest
                // process at 100%, which is the opposite of the truth.
                cpu_percent: if key.pid == crate::model::IDLE_PID {
                    0.0
                } else {
                    delta.cpu_percent
                },
                // 100ns ticks to milliseconds.
                cpu_time_ms: process.cpu_ticks / 10_000,
                working_set: process.working_set,
                private_bytes: process.private_bytes,
                virtual_bytes: process.virtual_bytes,
                thread_count: process.threads,
                handle_count: process.handles,
                disk_read_rate: delta.read_rate,
                disk_write_rate: delta.write_rate,
                io_read_bytes: process.read_bytes,
                io_write_bytes: process.write_bytes,
                connections: connections.get(&process.pid).copied().unwrap_or(0),
                gpu_percent: gpu
                    .as_ref()
                    .and_then(|reading| reading.by_pid.get(&process.pid).copied())
                    .unwrap_or(0.0),
                gpu_memory: gpu
                    .as_ref()
                    .and_then(|reading| reading.memory_by_pid.get(&process.pid).copied())
                    .unwrap_or(0),
                priority: priority_from_base(process.base_priority),
            });
        }

        // Both caches are pruned to what is still running, or they are
        // slow leaks: a machine running a long build accumulates an entry
        // per compiler process ever started.
        self.rates.retain_live(&live);
        self.identity.retain_live(&live);

        rows
    }

    /// Reads the system-wide counters.
    fn sample_system(&mut self, elapsed: Duration, processes: &[ProcessRow]) -> SystemSample {
        let (memory, counts) = win::memory::read();

        SystemSample {
            cpu: self.sample_cpu(),
            memory,
            disks: self.sample_disks(elapsed),
            adapters: self.sample_adapters(elapsed),
            gpus: self.sample_gpus(),
            uptime_seconds: win::system::uptime(),
            // Preferring the enumeration's own count over
            // `GetPerformanceInfo`'s: the two are taken at different
            // instants, and a status bar that disagrees with the number
            // of rows on screen reads as a bug.
            process_count: processes.len(),
            thread_count: processes
                .iter()
                .map(|row| u64::from(row.thread_count))
                .sum(),
            handle_count: if counts.handles > 0 {
                u64::from(counts.handles)
            } else {
                processes
                    .iter()
                    .map(|row| u64::from(row.handle_count))
                    .sum()
            },
        }
    }

    /// Per-core and overall CPU utilisation.
    fn sample_cpu(&mut self) -> CpuSample {
        let current = win::nt::cpu::read(&mut self.cpu_buffer).unwrap_or_default();
        let overall = win::nt::cpu::overall(&self.previous_cores, &current);
        let per_core: Vec<f64> = win::nt::cpu::utilisation(&self.previous_cores, &current)
            .into_iter()
            .map(|utilisation| utilisation.busy)
            .collect();
        self.previous_cores = current;

        CpuSample {
            total_percent: overall.busy,
            per_core,
            kernel_percent: overall.kernel,
            name: self.facts.cpu_name.clone(),
            physical_cores: self.facts.physical_cores,
            logical_cores: self.facts.logical_cores,
            megahertz: self.facts.megahertz,
        }
    }

    /// Per-disk throughput and active time.
    fn sample_disks(&mut self, elapsed: Duration) -> Vec<DiskSample> {
        // The disk counters are in 100ns ticks, so the interval has to be
        // in the same units for the active-time ratio to mean anything.
        let elapsed_ticks = u64::try_from(elapsed.as_nanos() / 100).unwrap_or(0);
        let volumes = win::disk::volumes();
        // Volume capacities are not mapped onto physical drives — see
        // `win::disk::volumes` — so the totals are reported against the
        // machine rather than per disk.
        let capacity: u64 = volumes.iter().map(|volume| volume.capacity).sum();
        let free: u64 = volumes.iter().map(|volume| volume.free).sum();

        let current = win::disk::read();
        let mut samples = Vec::with_capacity(current.len());
        for disk in &current {
            let previous = self.previous_disks.get(&disk.index).copied();
            let sample = match previous {
                Some(previous) => DiskSample {
                    index: disk.index,
                    name: win::disk::label(disk.index),
                    read_rate: crate::model::rates::per_second(
                        crate::model::rates::advance(previous.read_bytes, disk.read_bytes),
                        elapsed,
                    ),
                    write_rate: crate::model::rates::per_second(
                        crate::model::rates::advance(previous.write_bytes, disk.write_bytes),
                        elapsed,
                    ),
                    active_percent: win::disk::active_percent(previous, *disk, elapsed_ticks),
                    capacity,
                    free,
                },
                // First sight of this disk: rates are zero, for the same
                // reason a process's first sample is.
                None => DiskSample {
                    index: disk.index,
                    name: win::disk::label(disk.index),
                    capacity,
                    free,
                    ..DiskSample::default()
                },
            };
            samples.push(sample);
        }

        self.previous_disks = current.into_iter().map(|disk| (disk.index, disk)).collect();
        samples
    }

    /// Per-adapter throughput.
    fn sample_adapters(&mut self, elapsed: Duration) -> Vec<crate::model::AdapterSample> {
        let current = win::net::adapters();
        let mut samples = Vec::with_capacity(current.len());
        for adapter in &current {
            // Keyed by LUID, which is what an adapter *is*. The name was
            // the key here before, and a name is a label someone can
            // change: rename a connection in Network Connections and the
            // next sample finds no previous entry, so the adapter reads
            // zero for one tick and its history ring starts again. The
            // interface index is no better — Windows documents it as
            // changing when an adapter is disabled and re-enabled. This
            // is the same rule as `ProcessKey`, one layer down.
            let previous = self.previous_adapters.get(&adapter.luid);
            let mut sample = crate::model::AdapterSample {
                luid: adapter.luid,
                name: adapter.name.clone(),
                description: adapter.description.clone(),
                kind: adapter.kind,
                state: adapter.state,
                hardware: adapter.hardware,
                received_total: adapter.received,
                sent_total: adapter.sent,
                link_speed: adapter.link_speed,
                ..crate::model::AdapterSample::default()
            };
            if let Some(previous) = previous {
                sample.receive_rate = crate::model::rates::per_second(
                    crate::model::rates::advance(previous.received, adapter.received),
                    elapsed,
                );
                sample.send_rate = crate::model::rates::per_second(
                    crate::model::rates::advance(previous.sent, adapter.sent),
                    elapsed,
                );
            }
            samples.push(sample);
        }

        self.previous_adapters = current
            .into_iter()
            .map(|adapter| (adapter.luid, adapter))
            .collect();
        samples
    }

    /// Per-adapter GPU utilisation.
    ///
    /// Built from the engine map the process pass already read, so the
    /// counters are not collected twice — PDH would return the same
    /// values for a second collection in the same interval anyway.
    fn sample_gpus(&mut self) -> Vec<GpuSample> {
        let Some(session) = self.gpu.as_mut() else {
            return Vec::new();
        };
        let reading = session.read();
        // Group the (luid, engine) pairs by adapter.
        let mut by_adapter: HashMap<String, Vec<(String, f64)>> = HashMap::new();
        for ((luid, engine), value) in reading.engines {
            by_adapter.entry(luid).or_default().push((engine, value));
        }

        let mut samples: Vec<GpuSample> = by_adapter
            .into_iter()
            .map(|(luid, mut engines)| {
                // The busiest engine, not the sum: engines run in
                // parallel, so a sum exceeds 100% for a machine merely
                // playing a video while it renders. See `win::gpu`.
                let utilisation = engines
                    .iter()
                    .map(|(_, value)| *value)
                    .fold(0.0f64, f64::max)
                    .clamp(0.0, 100.0);
                engines.sort_by(|a, b| a.0.cmp(&b.0));
                GpuSample {
                    name: format!("GPU {luid}"),
                    luid,
                    utilisation,
                    engines,
                    memory_used: reading.memory_by_pid.values().sum(),
                    memory_total: 0,
                }
            })
            .collect();
        // Sorted so the Performance page's list does not reorder itself
        // between samples — a `HashMap` iteration order is not stable.
        samples.sort_by(|a, b| a.luid.cmp(&b.luid));
        samples
    }
}

/// Maps a base priority number to the class it implies.
///
/// The process enumeration reports a base priority *number*, not a class.
/// Reading the class properly means opening the process, which is a
/// syscall per row per sample — so this maps the number, which is exact
/// for every class a user can set. The boundaries are the documented base
/// priorities of each class.
#[must_use]
pub fn priority_from_base(base: i32) -> Priority {
    match base {
        ..=4 => Priority::Idle,
        5..=6 => Priority::BelowNormal,
        7..=8 => Priority::Normal,
        9..=10 => Priority::AboveNormal,
        11..=15 => Priority::High,
        _ => Priority::Realtime,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Architecture;
    use anyhow::{anyhow, Result};

    #[test]
    fn base_priorities_map_to_the_classes_they_imply() {
        // The documented base priority of each class.
        assert_eq!(priority_from_base(4), Priority::Idle);
        assert_eq!(priority_from_base(6), Priority::BelowNormal);
        assert_eq!(priority_from_base(8), Priority::Normal);
        assert_eq!(priority_from_base(10), Priority::AboveNormal);
        assert_eq!(priority_from_base(13), Priority::High);
        assert_eq!(priority_from_base(24), Priority::Realtime);
    }

    #[test]
    fn every_base_priority_maps_to_something() {
        // The kernel reports base priorities outside the documented
        // ranges for some system threads; none may be unmapped.
        for base in -10..=40 {
            let priority = priority_from_base(base);
            assert!(
                Priority::ALL.contains(&priority),
                "base {base} mapped outside the known classes"
            );
        }
    }

    #[test]
    fn the_priority_mapping_is_monotonic() {
        // A higher base priority must never map to a lower class, or the
        // Details view's priority sort is nonsense.
        let mut previous = Priority::Idle;
        for base in 0..=32 {
            let priority = priority_from_base(base);
            assert!(
                priority >= previous,
                "base {base} mapped to {priority:?}, below the previous \
                 {previous:?}"
            );
            previous = priority;
        }
    }

    #[test]
    fn an_interrupted_sleep_returns_early() {
        // A ten-second interval must not make quitting take ten seconds:
        // the window would sit on screen, unresponsive to its own close
        // button, while the thread finished a sleep nobody wants.
        let stopping = AtomicBool::new(false);
        let started = Instant::now();
        std::thread::scope(|scope| {
            scope.spawn(|| {
                std::thread::sleep(Duration::from_millis(150));
                stopping.store(true, Ordering::Relaxed);
            });
            let completed = sleep_interruptibly(Duration::from_secs(10), &stopping);
            assert!(!completed, "an interrupted sleep reports that it stopped");
        });
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "the sleep should have been cut short, took {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn an_uninterrupted_sleep_runs_its_course() {
        let stopping = AtomicBool::new(false);
        let started = Instant::now();
        assert!(sleep_interruptibly(Duration::from_millis(250), &stopping));
        assert!(
            started.elapsed() >= Duration::from_millis(200),
            "the sleep should have lasted roughly its interval, took {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn a_sleep_that_is_already_stopping_returns_immediately() {
        let stopping = AtomicBool::new(true);
        let started = Instant::now();
        assert!(!sleep_interruptibly(Duration::from_secs(30), &stopping));
        assert!(started.elapsed() < Duration::from_millis(500));
    }

    #[test]
    fn two_samples_produce_real_rates_for_this_process() {
        // End-to-end against the real machine: the first sample has no
        // previous counters, the second does.
        let mut sampler = Sampler::new(false);
        let first = sampler.sample(Duration::from_millis(500));
        assert!(
            first.processes.len() > 10,
            "a running machine has more than ten processes"
        );
        assert!(
            first.processes.iter().all(|row| row.cpu_percent == 0.0),
            "every process's first sample must report no CPU, not its \
             cumulative total"
        );

        std::thread::sleep(Duration::from_millis(300));
        let second = sampler.sample(Duration::from_millis(300));
        assert_eq!(second.sequence, 2);
        for row in &second.processes {
            assert!(
                (0.0..=100.0).contains(&row.cpu_percent),
                "{} reported {}% CPU",
                row.name,
                row.cpu_percent
            );
            assert!(row.disk_read_rate.is_finite() && row.disk_read_rate >= 0.0);
        }
    }

    #[test]
    fn the_idle_process_never_reports_cpu() {
        // Its "CPU time" is the machine's idle time — reporting it would
        // show an idle machine's busiest process at 100%.
        let mut sampler = Sampler::new(false);
        let _ = sampler.sample(Duration::from_millis(200));
        std::thread::sleep(Duration::from_millis(300));
        let snapshot = sampler.sample(Duration::from_millis(300));
        let idle = snapshot
            .processes
            .iter()
            .find(|row| row.pid == crate::model::IDLE_PID);
        let Some(idle) = idle else {
            return;
        };
        assert_eq!(idle.cpu_percent, 0.0);
    }

    #[test]
    fn this_process_appears_in_its_own_snapshot_with_its_identity_filled_in() -> Result<()> {
        let mut sampler = Sampler::new(false);
        let snapshot = sampler.sample(Duration::from_millis(200));
        let own = snapshot
            .processes
            .iter()
            .find(|row| row.pid == std::process::id())
            .ok_or_else(|| anyhow!("the test process should be in its own snapshot"))?;
        assert!(!own.name.is_empty());
        assert!(
            own.path.is_some(),
            "a process can always read its own image path"
        );
        assert!(!own.user.is_empty(), "and its own token");
        assert_ne!(
            own.architecture,
            Architecture::Unknown,
            "a process can always read its own bitness"
        );
        Ok(())
    }

    #[test]
    fn the_system_totals_agree_with_the_rows_on_screen() {
        // A status bar that disagrees with the number of rows reads as a
        // bug, which is why the enumeration's own count is preferred over
        // GetPerformanceInfo's.
        let mut sampler = Sampler::new(false);
        let snapshot = sampler.sample(Duration::from_millis(200));
        assert_eq!(
            snapshot.system.process_count,
            snapshot.processes.len(),
            "the process count must be the number of rows"
        );
        let threads: u64 = snapshot
            .processes
            .iter()
            .map(|row| u64::from(row.thread_count))
            .sum();
        assert_eq!(snapshot.system.thread_count, threads);
    }

    #[test]
    fn the_caches_do_not_grow_without_bound() {
        // Both are pruned to the live processes each pass; without that
        // they are slow leaks.
        let mut sampler = Sampler::new(false);
        for _ in 0..3 {
            let snapshot = sampler.sample(Duration::from_millis(100));
            assert!(
                sampler.rates.len() <= snapshot.processes.len(),
                "the rate history holds {} entries for {} processes",
                sampler.rates.len(),
                snapshot.processes.len()
            );
        }
    }
}

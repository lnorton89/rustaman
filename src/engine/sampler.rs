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
    CpuSample, DiskSample, Efficiency, GpuSample, Priority, ProcessKey, ProcessKind, ProcessRow,
    ProcessStatus, Snapshot, SystemSample, VolumeSample,
};
use crate::win;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The thread body.
///
/// Runs until the receiver disconnects or `stopping` is set.
pub fn run(
    snapshots: &Mutex<Option<Arc<Snapshot>>>,
    interval_ms: &AtomicU64,
    stopping: &AtomicBool,
    elevated: bool,
) {
    // First, before anything touches a device. This thread walks every
    // drive letter on the machine once a second, and a drive that is not
    // ready would otherwise raise a modal system dialog *on this thread*
    // and block it there until dismissed — which is the app hanging.
    // See `win::system::suppress_device_error_dialogs`.
    win::system::suppress_device_error_dialogs();

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
        super::publish(snapshots, snapshot);
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
    /// The rolling read of per-process efficiency mode.
    efficiency: EfficiencySweep,
    /// Monotonic snapshot counter.
    sequence: u64,
}

/// A rolling, bounded read of every process's efficiency-mode state.
///
/// ## Why this is not just another field on the process query
///
/// Everything else a row shows arrives in the one
/// `NtQuerySystemInformation` buffer. Quality of service does not:
/// `GetProcessInformation` is per-process and needs a handle, so
/// answering the question for every row every second means an
/// `OpenProcess`/query/`CloseHandle` for four hundred processes — the
/// same shape as the `EnumProcesses` route `docs/WINDOWS_APIS.md`
/// rejects, for the same reason.
///
/// So it is swept instead. Each pass reads a fixed slice of the process
/// list and remembers the answers, and the slice moves on. The cost per
/// pass is a constant rather than a function of how many processes the
/// machine is running — which is the property that matters, because the
/// machines where this app has to stay cheap are exactly the ones with
/// thousands of processes.
///
/// What it costs is *freshness*: a process that another tool throttles
/// keeps its old state on screen until the sweep comes round again, a
/// few seconds later. That is an acceptable trade for a flag that
/// changes when a human clicks something, and the one case where a human
/// clicked something *here* does not wait for it — see the optimistic
/// overlay in `gui::app`.
struct EfficiencySweep {
    /// What was last read, per process.
    known: HashMap<ProcessKey, bool>,
    /// Where the next slice starts, as an index into the row list.
    cursor: usize,
}

/// How many processes one pass reads the QoS state of.
///
/// Sized so a full sweep of a busy machine completes in a handful of
/// seconds while the per-pass cost stays in the low hundreds of
/// microseconds. Three syscalls each — open, query, close — so this is
/// about two hundred, against the thousands the process enumeration
/// would cost if it were done the documented way.
const EFFICIENCY_SLICE: usize = 64;

impl EfficiencySweep {
    /// A sweep that has read nothing yet.
    fn new() -> Self {
        Self {
            known: HashMap::new(),
            cursor: 0,
        }
    }

    /// Reads the next slice and stamps every row with what is known.
    ///
    /// Rows outside the slice are answered from the cache, and rows the
    /// sweep has never reached are left [`Efficiency::Unknown`] — which
    /// the UI draws as nothing rather than as "off". See
    /// [`crate::model::Efficiency`].
    fn refresh(&mut self, rows: &mut [ProcessRow]) {
        if rows.is_empty() {
            self.known.clear();
            self.cursor = 0;
            return;
        }
        if self.cursor >= rows.len() {
            self.cursor = 0;
        }

        // The slice is taken by position in a list whose order can shift
        // between passes, so a given process is not guaranteed to be
        // read exactly once per full cycle. It is guaranteed to be read
        // *often*, which is what this is for; keying the cursor on a
        // process instead would mean the sweep restarting every time
        // that one process exited.
        for offset in 0..EFFICIENCY_SLICE.min(rows.len()) {
            let index = (self.cursor + offset) % rows.len();
            let Some(row) = rows.get(index) else {
                continue;
            };
            // The pseudo-processes cannot be opened at all, and asking
            // every sweep would be a refused syscall each time.
            if row.is_pseudo() {
                continue;
            }
            let key = row.key();
            // A process that refuses to be opened keeps whatever was
            // last known rather than flickering to unknown: the refusal
            // says something about this app's rights, not about the
            // process's state.
            if let Some(reduced) = win::control::efficiency_of(key) {
                self.known.insert(key, reduced);
            }
        }
        self.cursor = (self.cursor + EFFICIENCY_SLICE) % rows.len();

        for row in rows.iter_mut() {
            row.efficiency = match self.known.get(&row.key()) {
                Some(true) => Efficiency::Reduced,
                Some(false) => Efficiency::Standard,
                None => Efficiency::Unknown,
            };
        }
    }

    /// Drops processes that have exited, the same way the rate and
    /// identity caches do — without it this is a slow leak of one entry
    /// per process the machine has ever run.
    fn retain_live(&mut self, live: &HashSet<ProcessKey>) {
        self.known.retain(|key, _| live.contains(key));
    }
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
            efficiency: EfficiencySweep::new(),
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
        // PDH rate counters are collected exactly once per pass. Both the
        // process rows and adapter cards derive from this same reading.
        let gpu = self.gpu.as_mut().map(win::gpu::Session::read);
        let processes = self.sample_processes(elapsed, cores, gpu.as_ref());
        let system = self.sample_system(elapsed, &processes, gpu.as_ref());

        Snapshot {
            sequence: self.sequence,
            interval: elapsed,
            processes,
            system,
        }
    }

    /// Reads and merges every process.
    fn sample_processes(
        &mut self,
        elapsed: Duration,
        cores: usize,
        gpu: Option<&win::gpu::Reading>,
    ) -> Vec<ProcessRow> {
        let Ok(raw) = win::nt::process::enumerate(&mut self.process_buffer) else {
            // A failed enumeration yields an empty list rather than a
            // stale one: showing a process table that is quietly frozen
            // is worse than showing an empty one, because nothing about
            // it says the numbers have stopped moving.
            self.rates.reset();
            return Vec::new();
        };

        // The per-PID maps, each produced by one call rather than one
        // call per row.
        let titles = win::windows::titles_by_pid();
        let connections = win::net::connections_by_pid();

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
                    hard_faults: process.hard_faults,
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
                icon: identity.icon,
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
                private_working_set: process.private_working_set,
                peak_working_set: process.peak_working_set,
                peak_private_bytes: process.peak_private_bytes,
                paged_pool: process.paged_pool,
                nonpaged_pool: process.nonpaged_pool,
                page_faults: process.page_faults,
                hard_faults: process.hard_faults,
                hard_fault_rate: delta.hard_fault_rate,
                thread_count: process.threads,
                handle_count: process.handles,
                disk_read_rate: delta.read_rate,
                disk_write_rate: delta.write_rate,
                io_read_bytes: process.read_bytes,
                io_write_bytes: process.write_bytes,
                connections: connections.get(&process.pid).copied().unwrap_or(0),
                gpu_percent: gpu
                    .and_then(|reading| reading.by_pid.get(&process.pid).copied())
                    .unwrap_or(0.0),
                gpu_memory: gpu
                    .and_then(|reading| reading.memory_by_pid.get(&process.pid).copied())
                    .unwrap_or(0),
                priority: priority_from_base(process.base_priority),
                // Filled in by the sweep below, which reads a bounded
                // slice of the list rather than every row.
                efficiency: Efficiency::Unknown,
            });
        }

        // Both caches are pruned to what is still running, or they are
        // slow leaks: a machine running a long build accumulates an entry
        // per compiler process ever started.
        self.rates.retain_live(&live);
        self.identity.retain_live(&live);
        self.efficiency.retain_live(&live);

        // After the rows exist, because the sweep stamps them — and
        // after the prune, so a slice never spends its budget on
        // processes that have already gone.
        self.efficiency.refresh(&mut rows);

        rows
    }

    /// Reads the system-wide counters.
    fn sample_system(
        &mut self,
        elapsed: Duration,
        processes: &[ProcessRow],
        gpu: Option<&win::gpu::Reading>,
    ) -> SystemSample {
        let (memory, counts) = win::memory::read();
        let volumes = win::disk::volumes()
            .into_iter()
            .map(|volume| VolumeSample {
                letter: volume.letter,
                capacity: volume.capacity,
                free: volume.free,
            })
            .collect();

        SystemSample {
            info: self.facts.info.clone(),
            cpu: self.sample_cpu(),
            memory,
            disks: self.sample_disks(elapsed),
            volumes,
            adapters: self.sample_adapters(elapsed),
            gpus: Self::sample_gpus(gpu),
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
            core_kinds: self.facts.core_kinds.clone(),
        }
    }

    /// Per-disk throughput and active time.
    fn sample_disks(&mut self, elapsed: Duration) -> Vec<DiskSample> {
        // The disk counters are in 100ns ticks, so the interval has to be
        // in the same units for the active-time ratio to mean anything.
        let elapsed_ticks = u64::try_from(elapsed.as_nanos() / 100).unwrap_or(0);
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
                },
                // First sight of this disk: rates are zero, for the same
                // reason a process's first sample is.
                None => DiskSample {
                    index: disk.index,
                    name: win::disk::label(disk.index),
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
    fn sample_gpus(reading: Option<&win::gpu::Reading>) -> Vec<GpuSample> {
        let Some(reading) = reading else {
            return Vec::new();
        };
        // Read once for the whole pass rather than once per adapter:
        // this walks a registry key, and the adapter list does not
        // change between two adapters of the same sample.
        let registry = win::gpu::adapters();

        // Group exact physical engines by adapter.
        let mut by_adapter: HashMap<String, Vec<(String, f64)>> = HashMap::new();
        for ((luid, physical, index, engine), value) in &reading.engines {
            let label = format!("{engine} · {physical}:{index}");
            by_adapter
                .entry(luid.clone())
                .or_default()
                .push((label, *value));
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
                let memory_used = reading.memory_by_adapter.get(&luid).copied().unwrap_or(0);
                // The counters name an adapter by LUID and carry neither
                // a description nor a capacity; the registry has both.
                // See `win::gpu::describe` on why only a single adapter
                // is matched.
                let (name, memory_total) = win::gpu::describe(&luid, &registry);
                GpuSample {
                    name,
                    luid,
                    utilisation,
                    engines,
                    memory_used,
                    memory_total,
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

    /// A row with nothing but an identity, for the sweep's bookkeeping.
    fn row(pid: u32) -> ProcessRow {
        ProcessRow {
            pid,
            started_at: u64::from(pid) * 1_000,
            ..ProcessRow::default()
        }
    }

    #[test]
    fn the_efficiency_sweep_answers_from_its_cache_between_passes() {
        // The point of the sweep: a row the slice did not reach this
        // pass still shows what was last read, rather than blanking and
        // refilling once a second.
        let mut sweep = EfficiencySweep::new();
        let mut rows = vec![row(100), row(200)];
        sweep.known.insert(rows[0].key(), true);
        sweep.known.insert(rows[1].key(), false);

        sweep.refresh(&mut rows);
        assert_eq!(rows[0].efficiency, Efficiency::Reduced);
        assert_eq!(rows[1].efficiency, Efficiency::Standard);
    }

    #[test]
    fn a_process_the_sweep_has_never_reached_reads_as_unknown() {
        // Not as "off". The sampler reads a slice of the list per pass,
        // so a row that has only just appeared genuinely has no answer,
        // and drawing the mark's absence as a fact would be a claim.
        let mut sweep = EfficiencySweep::new();
        let mut rows = vec![ProcessRow {
            pid: crate::model::SYSTEM_PID,
            ..ProcessRow::default()
        }];
        sweep.refresh(&mut rows);
        assert_eq!(
            rows[0].efficiency,
            Efficiency::Unknown,
            "the pseudo-processes are never opened, so they are never known"
        );
    }

    #[test]
    fn the_efficiency_cache_does_not_outlive_the_processes_in_it() {
        // The same leak every cache in this file has to avoid: one entry
        // per process the machine has ever run.
        let mut sweep = EfficiencySweep::new();
        let alive = row(100);
        let dead = row(200);
        sweep.known.insert(alive.key(), true);
        sweep.known.insert(dead.key(), true);

        let live: HashSet<ProcessKey> = std::iter::once(alive.key()).collect();
        sweep.retain_live(&live);

        assert_eq!(sweep.known.len(), 1);
        assert!(sweep.known.contains_key(&alive.key()));
    }

    #[test]
    fn an_empty_process_list_resets_the_sweep_rather_than_dividing_by_it() {
        // A failed enumeration publishes an empty list, and the cursor
        // arithmetic is modulo the row count.
        let mut sweep = EfficiencySweep::new();
        sweep.known.insert(row(100).key(), true);
        sweep.cursor = 37;

        sweep.refresh(&mut []);

        assert_eq!(sweep.cursor, 0);
        assert!(sweep.known.is_empty());
    }

    #[test]
    fn equal_type_physical_gpu_engines_are_not_summed_together() {
        let mut reading = win::gpu::Reading::default();
        reading
            .engines
            .insert(("adapter-a".into(), 0, 0, "3D".into()), 35.0);
        reading
            .engines
            .insert(("adapter-a".into(), 0, 1, "3D".into()), 40.0);

        let samples = Sampler::sample_gpus(Some(&reading));
        assert_eq!(samples.len(), 1);
        assert_eq!(samples[0].utilisation, 40.0);
        assert_eq!(samples[0].engines.len(), 2);
    }

    #[test]
    fn gpu_memory_stays_with_its_adapter() {
        let mut reading = win::gpu::Reading::default();
        reading
            .engines
            .insert(("adapter-a".into(), 0, 0, "3D".into()), 1.0);
        reading
            .engines
            .insert(("adapter-b".into(), 0, 0, "3D".into()), 1.0);
        reading.memory_by_adapter.insert("adapter-a".into(), 1_000);
        reading.memory_by_adapter.insert("adapter-b".into(), 2_000);

        let samples = Sampler::sample_gpus(Some(&reading));
        assert_eq!(samples.len(), 2);
        assert_eq!(samples[0].luid, "adapter-a");
        assert_eq!(samples[0].memory_used, 1_000);
        assert_eq!(samples[1].luid, "adapter-b");
        assert_eq!(samples[1].memory_used, 2_000);
    }
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
    #[ignore = "environment smoke test"]
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

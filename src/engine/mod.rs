// ============================================================================
// Module:       engine
// Description:  The sampler thread: calls into win/ on an interval and
//               publishes Snapshots the UI thread reads without blocking.
//
// Dependencies: crossbeam-channel, crate::win, crate::model
// ============================================================================

//! The seam between Windows and the UI.
//!
//! A background thread samples the machine on an interval and publishes a
//! [`Snapshot`] over a channel. The UI thread takes the latest one and
//! draws it. **The UI thread makes no system call at all**, which is the
//! single design decision this module exists to enforce.
//!
//! ## Why the sampling cannot be on the UI thread
//!
//! A full sample is a process enumeration, a per-core CPU read, memory,
//! per-disk IOCTLs, the interface table, two connection tables, the GPU
//! counters, and — for any process seen for the first time — an identity
//! resolution that reads a version resource off disk. On a normal machine
//! that is a few milliseconds; on a machine under load, with a disk that
//! is the reason the user opened a task manager, the disk IOCTLs alone
//! can block for hundreds of milliseconds.
//!
//! Done on the UI thread, the window stops redrawing for exactly as long
//! as the machine is struggling. A task manager that freezes when the
//! machine is busy is worse than useless — it freezes precisely when it
//! is being looked at, and it looks like *it* is the thing that has
//! hung. That is the complaint about the one that ships with Windows, and
//! it is the reason for this architecture.
//!
//! ## The channel drops rather than queues
//!
//! The channel is bounded at [`QUEUE_DEPTH`] and the sampler uses a
//! non-blocking send. If the UI is not keeping up — minimised, on a
//! machine mid-stall — the sampler discards the snapshot and takes the
//! next one on schedule.
//!
//! An unbounded channel would queue them instead, and the UI would then
//! work through a backlog of stale snapshots showing the machine as it
//! was thirty seconds ago, catching up in a burst. A blocking send would
//! be worse: the sampler would stall to the UI's rate and the graphs would
//! silently change their time base.
//!
//! Dropping is the only option that keeps "the last snapshot is what the
//! machine looks like now" true, which is the property everything on
//! screen depends on.

pub mod sampler;

use crate::model::Snapshot;
use crossbeam_channel::{bounded, Receiver, Sender, TrySendError};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// How many snapshots may be in flight before the sampler starts dropping.
///
/// Two: one the UI is drawing and one waiting. A deeper queue only buys
/// the ability to fall further behind — see the module docs.
const QUEUE_DEPTH: usize = 2;

/// A handle to the running sampler.
///
/// Dropping it asks the thread to stop and waits for it, so the sampler
/// cannot outlive the window it feeds.
pub struct Engine {
    /// Where snapshots arrive.
    snapshots: Receiver<Snapshot>,
    /// The sampling interval, in milliseconds. Shared with the thread so
    /// the settings page can change it without restarting the sampler.
    interval_ms: Arc<AtomicU64>,
    /// Set to ask the thread to stop.
    stopping: Arc<std::sync::atomic::AtomicBool>,
    /// The thread, joined on drop.
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Engine {
    /// Starts sampling.
    ///
    /// The first snapshot arrives after one interval, because every rate
    /// in it is a difference between two samples and there is only one to
    /// start with. See [`crate::model::rates`] — the alternative is an
    /// opening frame where every process shows its cumulative total as if
    /// it accrued in one second.
    #[must_use]
    pub fn start(interval: Duration) -> Self {
        let (sender, snapshots) = bounded(QUEUE_DEPTH);
        let interval_ms = Arc::new(AtomicU64::new(millis(interval)));
        let stopping = Arc::new(std::sync::atomic::AtomicBool::new(false));

        let thread = std::thread::Builder::new()
            // Named so it is identifiable in this app's own process list,
            // which is a thing people will do.
            .name("rustaman-sampler".to_string())
            .spawn({
                let interval_ms = Arc::clone(&interval_ms);
                let stopping = Arc::clone(&stopping);
                move || sampler::run(&sender, &interval_ms, &stopping)
            })
            .ok();

        Self {
            snapshots,
            interval_ms,
            stopping,
            thread,
        }
    }

    /// The most recent snapshot, or `None` if none has arrived since the
    /// last call.
    ///
    /// Drains the channel and returns the **last** item rather than the
    /// first. If two snapshots arrived between frames — which happens
    /// whenever a frame takes longer than the interval — drawing the
    /// older one and leaving the newer queued would put the UI
    /// permanently one sample behind, and it would never catch up.
    #[must_use]
    pub fn latest(&self) -> Option<Snapshot> {
        let mut newest = None;
        while let Ok(snapshot) = self.snapshots.try_recv() {
            newest = Some(snapshot);
        }
        newest
    }

    /// Changes the sampling interval.
    ///
    /// Takes effect after the current sleep, so a change from ten seconds
    /// to one can take up to ten seconds to be seen. That is deliberate:
    /// interrupting the sleep would mean an out-of-schedule sample whose
    /// interval is neither the old nor the new one, and every rate in it
    /// would be computed against the wrong elapsed time.
    pub fn set_interval(&self, interval: Duration) {
        self.interval_ms.store(millis(interval), Ordering::Relaxed);
    }

    /// The interval currently in force.
    #[must_use]
    pub fn interval(&self) -> Duration {
        Duration::from_millis(self.interval_ms.load(Ordering::Relaxed))
    }

    /// Whether the sampler thread is still running.
    ///
    /// A thread that failed to spawn, or that has exited, means no more
    /// snapshots will arrive — which the UI reports rather than sitting
    /// on a frozen display looking live.
    #[must_use]
    pub fn is_running(&self) -> bool {
        self.thread.is_some() && !self.stopping.load(Ordering::Relaxed)
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            // Joined rather than detached. The sampler holds process
            // handles and a PDH query; letting it run on while the
            // process tears down means those are released during exit,
            // racing the runtime's own shutdown.
            let _ = thread.join();
        }
    }
}

/// A duration as whole milliseconds, clamped to the configured range.
///
/// Clamped here as well as in [`crate::config`] because this is the
/// boundary the value actually crosses into the thread — a caller that
/// built a `Duration` some other way should not be able to pin the
/// machine with it.
fn millis(interval: Duration) -> u64 {
    let raw = u64::try_from(interval.as_millis()).unwrap_or(crate::config::DEFAULT_INTERVAL_MS);
    raw.clamp(
        crate::config::MIN_INTERVAL_MS,
        crate::config::MAX_INTERVAL_MS,
    )
}

/// Sends a snapshot, dropping it if the UI is not keeping up.
///
/// Returns whether the receiver is still connected, which is how the
/// sampler learns the window has closed.
fn publish(sender: &Sender<Snapshot>, snapshot: Snapshot) -> bool {
    match sender.try_send(snapshot) {
        Ok(()) => true,
        // The UI is behind. Drop this one and take the next on schedule;
        // see the module docs.
        Err(TrySendError::Full(_)) => true,
        Err(TrySendError::Disconnected(_)) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::{anyhow, Result};

    #[test]
    fn the_interval_is_clamped_at_the_thread_boundary() {
        // Clamped here as well as in `config` because this is where the
        // value actually crosses into the sampler.
        assert_eq!(
            millis(Duration::from_millis(1)),
            crate::config::MIN_INTERVAL_MS
        );
        assert_eq!(
            millis(Duration::from_secs(86_400)),
            crate::config::MAX_INTERVAL_MS
        );
        assert_eq!(millis(Duration::ZERO), crate::config::MIN_INTERVAL_MS);
        assert_eq!(millis(Duration::from_millis(1_000)), 1_000);
    }

    #[test]
    fn a_full_queue_drops_rather_than_disconnecting() {
        // The property the whole channel design rests on: a UI that is
        // not keeping up must cost the sampler nothing.
        let (sender, receiver) = bounded::<Snapshot>(1);
        assert!(publish(&sender, Snapshot::default()));
        assert!(
            publish(&sender, Snapshot::default()),
            "a full queue is not an error — the snapshot is dropped and \
             the sampler carries on"
        );
        assert_eq!(receiver.len(), 1, "and the queue has not grown");
    }

    #[test]
    fn a_closed_receiver_tells_the_sampler_to_stop() {
        let (sender, receiver) = bounded::<Snapshot>(1);
        drop(receiver);
        assert!(
            !publish(&sender, Snapshot::default()),
            "a disconnected channel is how the sampler learns the window \
             has closed"
        );
    }

    #[test]
    fn latest_returns_the_newest_snapshot_and_drains_the_rest() {
        // Drawing the oldest and leaving the newest queued would put the
        // UI permanently one sample behind, with no way to catch up.
        let (sender, snapshots) = bounded::<Snapshot>(4);
        for sequence in 1..=3u64 {
            let snapshot = Snapshot {
                sequence,
                ..Snapshot::default()
            };
            assert!(publish(&sender, snapshot));
        }
        let engine = Engine {
            snapshots,
            interval_ms: Arc::new(AtomicU64::new(1_000)),
            stopping: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            thread: None,
        };
        assert_eq!(
            engine.latest().map(|snapshot| snapshot.sequence),
            Some(3),
            "the newest snapshot is the one that describes the machine now"
        );
        assert_eq!(
            engine.latest().map(|snapshot| snapshot.sequence),
            None,
            "and the queue is empty afterwards"
        );
    }

    #[test]
    fn an_engine_with_no_thread_reports_that_it_is_not_running() {
        // A thread that failed to spawn means no snapshots will ever
        // arrive; the UI says so rather than showing a frozen display
        // that looks live.
        let (_sender, snapshots) = bounded::<Snapshot>(1);
        let engine = Engine {
            snapshots,
            interval_ms: Arc::new(AtomicU64::new(1_000)),
            stopping: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            thread: None,
        };
        assert!(!engine.is_running());
    }

    #[test]
    fn a_real_engine_produces_snapshots_and_stops_cleanly() -> Result<()> {
        // The end-to-end path, at the fastest interval so the test is
        // short. Exercises the sampler thread against the real machine.
        let engine = Engine::start(Duration::from_millis(crate::config::MIN_INTERVAL_MS));
        assert!(engine.is_running());

        // The first snapshot arrives after one interval, and every rate
        // in it needs a second sample — so wait for two.
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let mut seen = None;
        while std::time::Instant::now() < deadline {
            if let Some(snapshot) = engine.latest() {
                if snapshot.sequence >= 2 {
                    seen = Some(snapshot);
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(50));
        }

        let snapshot = seen.ok_or_else(|| anyhow!("no snapshot arrived within ten seconds"))?;
        assert!(
            snapshot.processes.len() > 10,
            "a running machine has more than ten processes, got {}",
            snapshot.processes.len()
        );
        assert!(
            snapshot.system.cpu.logical_cores > 0,
            "the machine should report its core count"
        );
        assert!(
            !snapshot.system.cpu.per_core.is_empty(),
            "per-core utilisation should be populated by the second sample"
        );
        assert!(
            snapshot.system.memory.total > 0,
            "memory should be populated"
        );
        assert!(
            snapshot.interval > Duration::ZERO,
            "the measured interval must be positive, or every rate in the \
             snapshot was computed against nothing"
        );
        // Dropping joins the thread; this is what checks it does not hang.
        drop(engine);
        Ok(())
    }

    #[test]
    fn changing_the_interval_takes_effect_without_a_restart() {
        let engine = Engine::start(Duration::from_secs(1));
        assert_eq!(engine.interval(), Duration::from_secs(1));
        engine.set_interval(Duration::from_secs(2));
        assert_eq!(engine.interval(), Duration::from_secs(2));
        // And a nonsense value is still clamped.
        engine.set_interval(Duration::from_millis(1));
        assert_eq!(
            engine.interval(),
            Duration::from_millis(crate::config::MIN_INTERVAL_MS)
        );
    }
}

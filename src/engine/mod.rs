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
//! [`Snapshot`] into a latest-value mailbox. The UI takes the newest value
//! and draws it. Expensive periodic collection stays off the UI thread;
//! explicit user actions may still make small, bounded system calls.
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
//! ## The mailbox overwrites rather than queues
//!
//! There is one slot. If the UI is not keeping up — minimised, on a
//! machine mid-stall — publishing replaces the older unread snapshot.
//!
//! An unbounded channel would queue them instead, and the UI would then
//! work through a backlog of stale snapshots showing the machine as it
//! was thirty seconds ago, catching up in a burst. A blocking send would
//! be worse: the sampler would stall to the UI's rate and the graphs would
//! silently change their time base.
//!
//! Overwriting the old value keeps "the last snapshot is what the machine
//! looks like now" true without ever blocking the sampler.

pub mod sampler;

use crate::model::Snapshot;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// A handle to the running sampler.
///
/// Dropping it asks the thread to stop and waits for it, so the sampler
/// cannot outlive the window it feeds.
pub struct Engine {
    /// Where snapshots arrive.
    snapshots: Arc<Mutex<Option<Arc<Snapshot>>>>,
    /// The sampling interval, in milliseconds. Shared with the thread so
    /// the settings page can change it without restarting the sampler.
    interval_ms: Arc<AtomicU64>,
    /// Set to ask the thread to stop.
    stopping: Arc<std::sync::atomic::AtomicBool>,
    /// Set by the thread as it returns, so [`Engine::drop`] can tell
    /// "finished" from "wedged" without blocking to find out.
    finished: Arc<std::sync::atomic::AtomicBool>,
    /// The thread, joined on drop — but only once it says it is done.
    thread: Option<std::thread::JoinHandle<()>>,
}

/// How long [`Engine::drop`] waits for the sampler before giving up on
/// it.
///
/// Generous next to what a stop costs when the thread is healthy: it
/// checks the flag every [`sampler::SLEEP_SLICE`] and a sample is
/// milliseconds, so the normal path is over long before this.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(2);

/// How often the grace period re-checks.
const SHUTDOWN_POLL: Duration = Duration::from_millis(10);

impl Engine {
    /// Starts sampling.
    ///
    /// The first snapshot arrives after one interval, because every rate
    /// in it is a difference between two samples and there is only one to
    /// start with. See [`crate::model::rates`] — the alternative is an
    /// opening frame where every process shows its cumulative total as if
    /// it accrued in one second.
    #[must_use]
    pub fn start(interval: Duration, elevated: bool) -> Self {
        let snapshots = Arc::new(Mutex::new(None));
        let interval_ms = Arc::new(AtomicU64::new(millis(interval)));
        let stopping = Arc::new(std::sync::atomic::AtomicBool::new(false));

        let finished = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let thread = std::thread::Builder::new()
            // Named so it is identifiable in this app's own process list,
            // which is a thing people will do.
            .name("rustaman-sampler".to_string())
            .spawn({
                let interval_ms = Arc::clone(&interval_ms);
                let stopping = Arc::clone(&stopping);
                let finished = Arc::clone(&finished);
                let snapshots = Arc::clone(&snapshots);
                move || {
                    sampler::run(&snapshots, &interval_ms, &stopping, elevated);
                    finished.store(true, Ordering::Release);
                }
            })
            .ok();

        Self {
            snapshots,
            interval_ms,
            stopping,
            finished,
            thread,
        }
    }

    /// The most recent snapshot, or `None` if none has arrived since the
    /// last call.
    ///
    /// Takes the mailbox value. Publishing may have overwritten older
    /// unread samples, so this is always the freshest available value.
    #[must_use]
    pub fn latest(&self) -> Option<Arc<Snapshot>> {
        self.snapshots.lock().ok()?.take()
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
        self.thread.is_some()
            && !self.stopping.load(Ordering::Relaxed)
            && !self.finished.load(Ordering::Acquire)
    }
}

impl Drop for Engine {
    /// Asks the sampler to stop, waits a bounded time for it, and
    /// abandons it if it does not come back.
    ///
    /// This runs on the **UI thread**, while the window is closing. A
    /// plain `join()` here is a promise that the sampler always returns
    /// — and the sampler spends its life inside Win32 calls against
    /// every process on the machine, any one of which can take
    /// arbitrarily long on a sick box: an image on a disconnected
    /// network share, a wedged driver behind a performance counter, a
    /// process that will not open. If one of them stalls, an unbounded
    /// join does not make the app close slowly. It makes it stop
    /// responding, with no way out but the task manager the user was
    /// already running.
    ///
    /// So the wait is bounded. Inside the grace period this behaves
    /// exactly as before — the thread is joined, and its process handles
    /// and PDH query are released in order rather than during the
    /// runtime's own teardown, which is what the join was for. Past it,
    /// the handle is dropped without joining and the thread is left to
    /// the process exit that is moments away, which reclaims everything
    /// it held regardless.
    ///
    /// Note what this does *not* do: it does not fix whatever stalled
    /// the sampler. It converts "the window never closes" into "the
    /// window closes", which is the difference between a bug and a
    /// bug report.
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::Relaxed);
        let Some(thread) = self.thread.take() else {
            return;
        };

        let deadline = std::time::Instant::now() + SHUTDOWN_GRACE;
        while !self.finished.load(Ordering::Acquire) {
            if std::time::Instant::now() >= deadline {
                // Deliberately not joined. `drop`ping a `JoinHandle`
                // detaches the thread, which is the whole point here.
                drop(thread);
                return;
            }
            std::thread::sleep(SHUTDOWN_POLL);
        }
        let _ = thread.join();
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

/// Publishes a snapshot, replacing an older unread value.
fn publish(mailbox: &Mutex<Option<Arc<Snapshot>>>, snapshot: Snapshot) {
    if let Ok(mut slot) = mailbox.lock() {
        *slot = Some(Arc::new(snapshot));
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
    fn latest_value_overwrites_stale_unread_snapshots() {
        let snapshots = Arc::new(Mutex::new(None));
        for sequence in 1..=4u64 {
            publish(
                &snapshots,
                Snapshot {
                    sequence,
                    ..Snapshot::default()
                },
            );
        }
        let engine = Engine {
            snapshots: Arc::clone(&snapshots),
            interval_ms: Arc::new(AtomicU64::new(1_000)),
            stopping: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            finished: Arc::new(std::sync::atomic::AtomicBool::new(true)),
            thread: None,
        };
        assert_eq!(
            engine.latest().map(|snapshot| snapshot.sequence),
            Some(4),
            "the newest snapshot is the one that describes the machine now"
        );
        assert_eq!(
            engine.latest().map(|snapshot| snapshot.sequence),
            None,
            "and the mailbox is empty afterwards"
        );
    }

    #[test]
    fn dropping_an_engine_whose_sampler_is_wedged_still_returns() {
        // The property the whole grace period exists for. `Drop` runs on
        // the UI thread while the window is closing, so an unbounded
        // `join()` there turns any stall anywhere in the Windows layer
        // into an app that will not close — which is what was reported
        // after a long session.
        //
        // The stand-in thread never observes `stopping`, the way a
        // thread parked inside a Win32 call cannot. If `Drop` waits for
        // it, this test hangs rather than fails, so it is bounded from
        // the outside too: the assertion is on elapsed time, and the
        // whole thing is over in `SHUTDOWN_GRACE` plus a poll.
        let snapshots = Arc::new(Mutex::new(None));
        let stopping = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let wedged = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let thread = std::thread::Builder::new()
            .name("wedged-sampler".to_string())
            .spawn({
                let wedged = Arc::clone(&wedged);
                move || {
                    while !wedged.load(Ordering::Acquire) {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                }
            })
            .ok();

        let engine = Engine {
            snapshots,
            interval_ms: Arc::new(AtomicU64::new(1_000)),
            stopping,
            // Never set, because the thread never finishes.
            finished: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            thread,
        };

        let start = std::time::Instant::now();
        drop(engine);
        let waited = start.elapsed();

        // Released only now, so the thread really was still running for
        // the whole of the drop above rather than having quietly exited.
        wedged.store(true, Ordering::Release);

        assert!(
            waited < SHUTDOWN_GRACE * 2,
            "dropping an Engine whose sampler never returns took {waited:?},              which means the wait is not bounded — on the UI thread that is              a window that never closes"
        );
        assert!(
            waited >= SHUTDOWN_GRACE,
            "the drop returned in {waited:?}, before the grace period was              up — a healthy sampler would have been abandoned rather than              joined"
        );
    }

    #[test]
    fn an_engine_with_no_thread_reports_that_it_is_not_running() {
        // A thread that failed to spawn means no snapshots will ever
        // arrive; the UI says so rather than showing a frozen display
        // that looks live.
        let snapshots = Arc::new(Mutex::new(None));
        let engine = Engine {
            snapshots,
            interval_ms: Arc::new(AtomicU64::new(1_000)),
            stopping: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            finished: Arc::new(std::sync::atomic::AtomicBool::new(true)),
            thread: None,
        };
        assert!(!engine.is_running());
    }

    #[test]
    #[ignore = "environment smoke test"]
    fn a_real_engine_produces_snapshots_and_stops_cleanly() -> Result<()> {
        // The end-to-end path, at the fastest interval so the test is
        // short. Exercises the sampler thread against the real machine.
        let engine = Engine::start(Duration::from_millis(crate::config::MIN_INTERVAL_MS), false);
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
        let engine = Engine::start(Duration::from_secs(1), false);
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

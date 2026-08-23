// ============================================================================
// Module:       model::rates
// Description:  Turning cumulative counters into per-second rates, keyed so a
//               recycled PID cannot produce a delta against another process.
//
// Dependencies: std; super::ProcessKey
// ============================================================================

//! From counters to rates.
//!
//! Everything the kernel reports about a process is **cumulative since it
//! started**: CPU time in 100ns ticks, bytes read, bytes written. None of
//! those is what a task manager shows. The CPU column is a percentage
//! over the last interval, and the Disk column is bytes per second — both
//! of which are differences between two samples divided by the time
//! between them.
//!
//! This module is that arithmetic, and it is deliberately in the portable
//! core rather than in [`crate::engine`]: it is the calculation most
//! likely to be subtly wrong, and none of it needs a Windows kernel to
//! test.
//!
//! ## The four ways this goes wrong
//!
//! Each has a test named after it below.
//!
//! 1. **The first sample.** There is no previous counter, so the "delta"
//!    is the whole cumulative total — for a process that has been running
//!    since boot, hours of CPU time attributed to one second. Every
//!    process would open at a wildly overstated figure. The first
//!    observation of a process therefore reports **zero**, and the second
//!    reports its first real rate.
//!
//! 2. **A recycled PID.** Keyed on the PID alone, a new process
//!    inheriting a dead one's number gets a delta taken against the dead
//!    process's totals — which is either a large negative (clamped to
//!    zero, so the process looks idle when it is not) or a large positive
//!    spike. [`Rates`] keys on [`ProcessKey`], so a recycled PID is
//!    simply a process that has not been seen before, and rule 1 applies.
//!
//! 3. **A zero or backwards interval.** Two samples within one clock tick
//!    divide by zero; a system clock adjustment can make the elapsed time
//!    negative. Both yield zero rather than an infinity or a `NaN`, which
//!    would then poison the graph's scale and blank it entirely.
//!
//! 4. **Normalisation across cores.** A process saturating one core of
//!    eight is at 12.5% of the machine, not 100%. Dividing by the elapsed
//!    time alone gives the latter, and the column then reads over 100% on
//!    any multi-core machine — which is what Task Manager's own "CPU"
//!    column used to do before it was normalised, and why people learned
//!    to distrust it.

use super::ProcessKey;
use std::collections::{HashMap, HashSet};
use std::time::Duration;

/// One process's cumulative counters, as the kernel reports them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counters {
    /// Cumulative CPU time (kernel + user) in 100ns ticks.
    pub cpu_ticks: u64,
    /// Cumulative bytes read.
    pub read_bytes: u64,
    /// Cumulative bytes written.
    pub write_bytes: u64,
    /// Cumulative hard page faults.
    pub hard_faults: u64,
}

/// The rates one interval's worth of change works out to.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Delta {
    /// Share of the whole machine's CPU capacity, 0..=100.
    pub cpu_percent: f64,
    /// Bytes per second read.
    pub read_rate: f64,
    /// Bytes per second written.
    pub write_rate: f64,
    /// Hard page faults per second.
    pub hard_fault_rate: f64,
}

/// Remembers the previous sample's counters so the next one can be a rate.
#[derive(Debug, Default)]
pub struct Rates {
    /// Counters from the previous sample, keyed so a recycled PID cannot
    /// match. See the module docs.
    previous: HashMap<ProcessKey, Counters>,
}

impl Rates {
    /// An empty history.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Records `counters` for `key` and returns the rates since the last
    /// time this key was seen.
    ///
    /// `cores` is the number of logical processors, which is what the CPU
    /// percentage is normalised against — see rule 4 in the module docs.
    ///
    /// The first observation of a key returns zeroes; see rule 1.
    pub fn observe(
        &mut self,
        key: ProcessKey,
        counters: Counters,
        elapsed: Duration,
        cores: usize,
    ) -> Delta {
        let previous = self.previous.insert(key, counters);
        let Some(previous) = previous else {
            // First sight of this process. Reporting the cumulative total
            // as if it accrued in one interval would show a process that
            // has run since boot at an impossible figure.
            return Delta::default();
        };

        let seconds = elapsed.as_secs_f64();
        if !seconds.is_finite() || seconds <= 0.0 {
            // Two samples inside one clock tick, or a clock adjustment.
            return Delta::default();
        }

        // `saturating_sub` throughout: a counter that appears to go
        // backwards yields zero rather than wrapping to an enormous
        // positive. That happens after a resume, and on the sample where
        // a process's counters are read mid-update.
        let read = counters.read_bytes.saturating_sub(previous.read_bytes);
        let write = counters.write_bytes.saturating_sub(previous.write_bytes);
        let cpu_ticks = counters.cpu_ticks.saturating_sub(previous.cpu_ticks);
        let hard_faults = counters.hard_faults.saturating_sub(previous.hard_faults);

        Delta {
            cpu_percent: cpu_percent(cpu_ticks, elapsed, cores),
            read_rate: read as f64 / seconds,
            write_rate: write as f64 / seconds,
            hard_fault_rate: hard_faults as f64 / seconds,
        }
    }

    /// Drops history for processes that are no longer running.
    ///
    /// Without this the map is a slow leak with the same shape as the
    /// identity cache's: an app left open across a long build accumulates
    /// an entry per compiler process ever started.
    pub fn retain_live(&mut self, live: &HashSet<ProcessKey>) {
        self.previous.retain(|key, _| live.contains(key));
    }

    /// Discards every baseline after a failed source read.
    ///
    /// A later successful enumeration must start a new baseline; otherwise
    /// its counter delta spans the failure while its elapsed time does not.
    pub fn reset(&mut self) {
        self.previous.clear();
    }

    /// How many processes are being tracked.
    #[must_use]
    pub fn len(&self) -> usize {
        self.previous.len()
    }

    /// Whether nothing has been observed yet.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.previous.is_empty()
    }
}

/// A CPU-tick delta as a share of the whole machine, 0..=100.
///
/// The interval is converted to the same 100ns ticks the counter uses,
/// then multiplied by the core count — because in one second of wall
/// clock an eight-core machine has eight core-seconds to give out. See
/// rule 4 in the module docs.
#[must_use]
pub fn cpu_percent(ticks: u64, elapsed: Duration, cores: usize) -> f64 {
    if cores == 0 {
        return 0.0;
    }
    // 100ns ticks in the interval. `u128` because the nanosecond count of
    // a long interval times a large core count overflows a `u64`.
    let available = elapsed.as_nanos() / 100 * cores as u128;
    if available == 0 {
        return 0.0;
    }
    let share = u128::from(ticks) as f64 / available as f64;
    // Clamped: a process can be credited slightly more CPU than the
    // interval accounts for, because the counter and the clock are read
    // at different instants. A column reading 103% looks like a bug even
    // when the underlying measurement is sound.
    (share * 100.0).clamp(0.0, 100.0)
}

/// A byte delta as bytes per second.
///
/// Free-standing because the system-wide counters — disk, network — need
/// the same arithmetic without the per-process bookkeeping.
#[must_use]
pub fn per_second(delta: u64, elapsed: Duration) -> f64 {
    let seconds = elapsed.as_secs_f64();
    if !seconds.is_finite() || seconds <= 0.0 {
        return 0.0;
    }
    delta as f64 / seconds
}

/// The difference between two cumulative readings, saturating at zero.
///
/// A counter that appears to go backwards — across a resume, or when read
/// mid-update — yields zero rather than wrapping to sixteen exabytes.
#[must_use]
pub fn advance(previous: u64, current: u64) -> u64 {
    current.saturating_sub(previous)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One second.
    const SECOND: Duration = Duration::from_secs(1);

    /// One second, in the 100ns ticks the CPU counters use.
    const SECOND_TICKS: u64 = 10_000_000;

    fn key(pid: u32, started_at: u64) -> ProcessKey {
        ProcessKey { pid, started_at }
    }

    #[test]
    fn the_first_sample_of_a_process_reports_nothing() {
        // Rule 1. A process that has run since boot has hours of CPU
        // time; attributing that to one interval would open every row at
        // an impossible figure.
        let mut rates = Rates::new();
        let counters = Counters {
            cpu_ticks: 9_999 * SECOND_TICKS,
            read_bytes: 500_000_000,
            write_bytes: 400_000_000,
            hard_faults: 20,
        };
        let delta = rates.observe(key(100, 1), counters, SECOND, 8);
        assert_eq!(
            delta,
            Delta::default(),
            "a first observation must report zero, not the cumulative total"
        );
    }

    #[test]
    fn the_second_sample_reports_the_real_rate() {
        let mut rates = Rates::new();
        let first = Counters {
            cpu_ticks: 100 * SECOND_TICKS,
            read_bytes: 1_000,
            write_bytes: 2_000,
            hard_faults: 10,
        };
        let _ = rates.observe(key(100, 1), first, SECOND, 4);

        let second = Counters {
            // One core-second of CPU in one wall second, on four cores.
            cpu_ticks: 101 * SECOND_TICKS,
            read_bytes: 1_000 + 4_096,
            write_bytes: 2_000 + 8_192,
            hard_faults: 14,
        };
        let delta = rates.observe(key(100, 1), second, SECOND, 4);
        assert!(
            (delta.cpu_percent - 25.0).abs() < 0.01,
            "one of four cores is 25%, got {}",
            delta.cpu_percent
        );
        assert!((delta.read_rate - 4_096.0).abs() < 0.01);
        assert!((delta.write_rate - 8_192.0).abs() < 0.01);
        assert!((delta.hard_fault_rate - 4.0).abs() < 0.01);
    }

    #[test]
    fn a_recycled_pid_does_not_produce_a_delta_against_the_dead_process() {
        // Rule 2, and the reason the map is keyed on ProcessKey. Keyed on
        // the PID alone, the new process's small counters minus the dead
        // one's large ones would clamp to zero — so a busy new process
        // would read as idle — or, the other way round, spike.
        let mut rates = Rates::new();
        let old = Counters {
            cpu_ticks: 5_000 * SECOND_TICKS,
            ..Counters::default()
        };
        let _ = rates.observe(key(4242, 100), old, SECOND, 4);

        let new = Counters {
            cpu_ticks: SECOND_TICKS,
            ..Counters::default()
        };
        // Same PID, different creation time.
        let delta = rates.observe(key(4242, 200), new, SECOND, 4);
        assert_eq!(
            delta,
            Delta::default(),
            "a reused PID is a process that has not been seen before, so \
             its first sample reports zero"
        );
        assert_eq!(rates.len(), 2, "the two are tracked separately");
    }

    #[test]
    fn a_zero_length_interval_is_not_a_division_by_zero() {
        // Rule 3. A NaN or an infinity here poisons the graph's scale and
        // blanks it entirely.
        let mut rates = Rates::new();
        let counters = Counters::default();
        let _ = rates.observe(key(1, 1), counters, SECOND, 4);
        let delta = rates.observe(
            key(1, 1),
            Counters {
                cpu_ticks: 5_000,
                read_bytes: 100,
                write_bytes: 100,
                hard_faults: 1,
            },
            Duration::ZERO,
            4,
        );
        assert_eq!(delta, Delta::default());
        assert!(delta.cpu_percent.is_finite() && delta.read_rate.is_finite());
    }

    #[test]
    fn counters_that_go_backwards_yield_zero_rather_than_an_enormous_rate() {
        // Happens across a sleep/resume, and on the sample where a
        // process's counters are read mid-update. An unsaturated
        // subtraction wraps to sixteen exabytes per second.
        let mut rates = Rates::new();
        let high = Counters {
            cpu_ticks: 1_000 * SECOND_TICKS,
            read_bytes: 1_000_000,
            write_bytes: 1_000_000,
            hard_faults: 1_000,
        };
        let _ = rates.observe(key(1, 1), high, SECOND, 4);
        let low = Counters {
            cpu_ticks: 1,
            read_bytes: 1,
            write_bytes: 1,
            hard_faults: 1,
        };
        let delta = rates.observe(key(1, 1), low, SECOND, 4);
        assert_eq!(delta.cpu_percent, 0.0);
        assert_eq!(delta.read_rate, 0.0);
        assert_eq!(delta.write_rate, 0.0);
    }

    #[test]
    fn cpu_is_normalised_across_the_machines_cores() {
        // Rule 4. A process saturating one core of eight is at 12.5% of
        // the machine, not 100%.
        assert!(
            (cpu_percent(SECOND_TICKS, SECOND, 8) - 12.5).abs() < 0.001,
            "got {}",
            cpu_percent(SECOND_TICKS, SECOND, 8)
        );
        assert!(
            (cpu_percent(SECOND_TICKS, SECOND, 1) - 100.0).abs() < 0.001,
            "one core of one is the whole machine"
        );
        assert!(
            (cpu_percent(8 * SECOND_TICKS, SECOND, 8) - 100.0).abs() < 0.001,
            "eight core-seconds on eight cores is the whole machine"
        );
    }

    #[test]
    fn a_cpu_share_slightly_over_the_interval_is_clamped() {
        // The counter and the clock are read at different instants, so a
        // process can be credited marginally more CPU than the interval
        // accounts for. A column reading 103% looks like a bug even when
        // the measurement is sound.
        let percent = cpu_percent(9 * SECOND_TICKS, SECOND, 8);
        assert_eq!(percent, 100.0);
    }

    #[test]
    fn a_machine_with_no_cores_reported_does_not_divide_by_zero() {
        // `cores` comes from a Win32 call that can fail.
        assert_eq!(cpu_percent(SECOND_TICKS, SECOND, 0), 0.0);
        assert_eq!(cpu_percent(SECOND_TICKS, Duration::ZERO, 8), 0.0);
    }

    #[test]
    fn a_long_interval_on_a_many_core_machine_does_not_overflow() {
        // The available-ticks product is what overflows: a 60-second
        // interval on 128 logical processors is 7.68e10 ticks, and in
        // nanoseconds before the division it is far more. Computed in
        // u128 for that reason.
        let percent = cpu_percent(u64::MAX, Duration::from_secs(60), 128);
        assert!(
            percent.is_finite() && (0.0..=100.0).contains(&percent),
            "got {percent}"
        );
        let idle = cpu_percent(0, Duration::from_secs(3_600), 256);
        assert_eq!(idle, 0.0);
    }

    #[test]
    fn per_second_handles_the_degenerate_intervals() {
        assert!((per_second(1_000, SECOND) - 1_000.0).abs() < 0.001);
        assert!((per_second(1_000, Duration::from_millis(500)) - 2_000.0).abs() < 0.001);
        assert_eq!(per_second(1_000, Duration::ZERO), 0.0);
        assert_eq!(per_second(0, SECOND), 0.0);
    }

    #[test]
    fn advance_saturates_rather_than_wrapping() {
        assert_eq!(advance(100, 250), 150);
        assert_eq!(advance(250, 100), 0, "a backwards counter yields zero");
        assert_eq!(advance(u64::MAX, 0), 0);
    }

    #[test]
    fn history_is_pruned_to_the_live_processes() {
        // Without this the map is a slow leak with the same shape as the
        // identity cache's.
        let mut rates = Rates::new();
        for pid in 0..10u32 {
            let _ = rates.observe(key(pid, 1), Counters::default(), SECOND, 4);
        }
        assert_eq!(rates.len(), 10);

        let live: HashSet<ProcessKey> = (0..3u32).map(|pid| key(pid, 1)).collect();
        rates.retain_live(&live);
        assert_eq!(rates.len(), 3);
        assert!(!rates.is_empty());

        rates.retain_live(&HashSet::new());
        assert!(rates.is_empty());
    }

    #[test]
    fn a_pruned_process_that_returns_starts_from_zero_again() {
        // Rule 1 has to keep applying after a prune, or a process that
        // was dropped and re-observed spikes.
        let mut rates = Rates::new();
        let counters = Counters {
            cpu_ticks: 500 * SECOND_TICKS,
            ..Counters::default()
        };
        let _ = rates.observe(key(1, 1), counters, SECOND, 4);
        rates.retain_live(&HashSet::new());
        let delta = rates.observe(key(1, 1), counters, SECOND, 4);
        assert_eq!(delta, Delta::default());
    }
}

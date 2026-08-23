// ============================================================================
// Module:       win::nt::cpu
// Description:  Per-logical-processor time totals, and the delta arithmetic
//               that turns two samples of them into a utilisation.
//
// Dependencies: super::types, super::query
// ============================================================================

//! Processor time, per core.
//!
//! ## The trap in these numbers
//!
//! `SYSTEM_PROCESSOR_PERFORMANCE_INFORMATION` reports `IdleTime`,
//! `KernelTime`, and `UserTime` per logical processor — and **`KernelTime`
//! includes `IdleTime`**. That is not a quirk to note in passing; it is
//! the single thing this module exists to get right.
//!
//! The obvious formula, `busy = kernel + user`, reports an idle machine
//! as 100% busy: on an idle core essentially all of the elapsed time is
//! idle time, and idle time is counted inside `KernelTime`. The result
//! looks *almost* plausible — a busy machine also reads near 100% — which
//! is why it survives casual testing and then ships.
//!
//! The correct reading is that `KernelTime + UserTime` is the whole
//! elapsed time for that core, and the busy fraction is therefore
//! `1 - idle_delta / total_delta`. That is what [`Utilisation::between`]
//! computes, and `an_idle_core_reads_as_idle` is the test that pins it.
//!
//! ## Why deltas and not the totals
//!
//! Every counter here is cumulative since boot. The utilisation over the
//! last second is the change across that second divided by the elapsed
//! time — never the totals themselves, which would give the average since
//! boot and barely move.

use super::types::{
    SystemProcessorPerformanceInformation, PROCESSOR_PERFORMANCE_SIZE,
    SYSTEM_PROCESSOR_PERFORMANCE_INFORMATION_CLASS,
};
use super::{query_exact, InfoBuffer, QueryError};

/// One logical processor's cumulative times, in 100ns ticks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CoreTimes {
    /// Time the core spent idle.
    pub idle: u64,
    /// Time the core spent in kernel mode — **including** `idle`.
    pub kernel: u64,
    /// Time the core spent in user mode.
    pub user: u64,
}

impl CoreTimes {
    /// The core's whole elapsed time, as these counters account for it.
    ///
    /// `kernel + user`, and not `kernel + user + idle`: idle is already
    /// inside kernel. See the module docs.
    #[must_use]
    pub fn total(self) -> u64 {
        self.kernel.saturating_add(self.user)
    }
}

/// A busy fraction derived from two samples.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Utilisation {
    /// Busy share of the interval, 0..=100.
    pub busy: f64,
    /// The share of the interval spent in kernel mode, excluding idle,
    /// 0..=100.
    ///
    /// Drawn as a darker band under the total on the CPU graph, and the
    /// fastest way to spot a driver or an antivirus filter eating the
    /// machine — a red flag that looks identical to ordinary load in the
    /// total alone.
    pub kernel: f64,
}

impl Utilisation {
    /// The utilisation between two samples of one core.
    ///
    /// Returns zero for a non-advancing or backwards interval rather than
    /// dividing by zero or reporting a negative load. Counters can appear
    /// to go backwards across a sleep/resume cycle, and after one the
    /// honest answer for the interval that spanned it is "nothing
    /// measurable".
    #[must_use]
    pub fn between(previous: CoreTimes, current: CoreTimes) -> Self {
        let total = current.total().saturating_sub(previous.total());
        if total == 0 {
            return Self::default();
        }
        let idle = current.idle.saturating_sub(previous.idle).min(total);
        let kernel = current
            .kernel
            .saturating_sub(previous.kernel)
            // Kernel time that is not idle time. Saturating, because
            // across a resume the two deltas can disagree.
            .saturating_sub(idle)
            .min(total);

        let total = total as f64;
        Self {
            // `1 - idle/total`, not `(kernel + user)/total`. See the
            // module docs — the latter reports an idle machine as fully
            // busy.
            busy: ((1.0 - idle as f64 / total) * 100.0).clamp(0.0, 100.0),
            kernel: ((kernel as f64 / total) * 100.0).clamp(0.0, 100.0),
        }
    }
}

/// Reads every logical processor's cumulative times.
///
/// One entry per logical processor, in the order Windows numbers them.
///
/// Goes through [`query_exact`] rather than [`super::query`]: this class
/// mismatches even against an oversized buffer, so the generic
/// grow-and-retry protocol can never converge for it. See
/// `query_exact`'s docs.
pub fn read(buffer: &mut InfoBuffer) -> Result<Vec<CoreTimes>, QueryError> {
    query_exact(SYSTEM_PROCESSOR_PERFORMANCE_INFORMATION_CLASS, buffer)?;
    Ok(parse(buffer.filled()))
}

/// Parses a filled buffer into per-core times.
///
/// Split out so the array walk can be tested on any platform, the same
/// way [`super::process::walk`] is. Unlike the process chain this is a
/// flat array, so the only thing that can go wrong is a trailing partial
/// entry — which is dropped rather than read.
#[must_use]
pub fn parse(bytes: &[u8]) -> Vec<CoreTimes> {
    let count = bytes.len() / PROCESSOR_PERFORMANCE_SIZE;
    let mut cores = Vec::with_capacity(count);
    for index in 0..count {
        let Some(base) = index.checked_mul(PROCESSOR_PERFORMANCE_SIZE) else {
            break;
        };
        let Some(end) = base.checked_add(PROCESSOR_PERFORMANCE_SIZE) else {
            break;
        };
        let Some(slice) = bytes.get(base..end) else {
            break;
        };
        // SAFETY: `slice` is exactly one record long and valid for reads.
        // `read_unaligned` imposes no alignment requirement, which
        // matters because the backing buffer is a `[u8]`. Every field is
        // a plain integer with no invalid bit pattern.
        let Some(entry) = read_cpu_entry(slice) else {
            break;
        };
        cores.push(CoreTimes {
            idle: entry.IdleTime.max(0) as u64,
            kernel: entry.KernelTime.max(0) as u64,
            user: entry.UserTime.max(0) as u64,
        });
    }
    cores
}

fn read_cpu_entry(bytes: &[u8]) -> Option<SystemProcessorPerformanceInformation> {
    if bytes.len() < PROCESSOR_PERFORMANCE_SIZE {
        return None;
    }
    // SAFETY: caller sliced exactly one kernel-written entry; unaligned read avoids byte-buffer alignment assumptions.
    Some(unsafe {
        std::ptr::read_unaligned(
            bytes
                .as_ptr()
                .cast::<SystemProcessorPerformanceInformation>(),
        )
    })
}

/// The utilisation of every core between two samples.
///
/// Pairs the two lists positionally and stops at the shorter. The lengths
/// differ only if a processor was hot-added or removed between samples,
/// and in that case the pairing is meaningless past the shorter of the
/// two anyway.
#[must_use]
pub fn utilisation(previous: &[CoreTimes], current: &[CoreTimes]) -> Vec<Utilisation> {
    previous
        .iter()
        .zip(current.iter())
        .map(|(previous, current)| Utilisation::between(*previous, *current))
        .collect()
}

/// The whole machine's utilisation between two samples.
///
/// Computed by summing the raw tick counts across cores and taking one
/// ratio, rather than by averaging the per-core percentages. The two
/// differ whenever the cores' intervals are not identical — which is
/// normal, since a parked core accumulates no time at all — and the
/// summed form is the one that stays correct: a machine with one core
/// pinned and fifteen parked reads as 1/16th busy, not as 100%.
#[must_use]
pub fn overall(previous: &[CoreTimes], current: &[CoreTimes]) -> Utilisation {
    let fold = |cores: &[CoreTimes]| {
        cores.iter().fold(CoreTimes::default(), |mut sum, core| {
            sum.idle = sum.idle.saturating_add(core.idle);
            sum.kernel = sum.kernel.saturating_add(core.kernel);
            sum.user = sum.user.saturating_add(core.user);
            sum
        })
    };
    // Only the cores present in both samples, for the reason above.
    let paired = previous.len().min(current.len());
    Utilisation::between(
        fold(previous.get(..paired).unwrap_or(&[])),
        fold(current.get(..paired).unwrap_or(&[])),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Result;

    /// One second, in the 100ns ticks these counters use.
    const SECOND: u64 = 10_000_000;

    #[test]
    fn this_machine_reports_its_own_per_core_times() -> Result<()> {
        // The regression this guards against: `read` used to go through
        // `super::query`, whose grow-and-retry protocol assumes a bigger
        // buffer eventually succeeds. `SystemProcessorPerformanceInformation`
        // mismatches even against an oversized one, so that call failed on
        // every real machine, on every sample, and `sample_cpu` silently
        // swallowed the error — every process's per-core graph, and the
        // machine's own total CPU figure, read as permanently idle.
        let mut buffer = InfoBuffer::new();
        let cores = read(&mut buffer)?;
        assert!(
            !cores.is_empty(),
            "a running machine reports at least one logical processor"
        );
        Ok(())
    }

    #[test]
    fn an_idle_core_reads_as_idle() {
        // The bug this module exists to avoid. `kernel + user` over the
        // interval is ~100% on an idle core, because idle time is counted
        // inside kernel time — so the naive formula reports an idle
        // machine as fully busy, and looks plausible while doing it.
        let previous = CoreTimes {
            idle: 0,
            kernel: 0,
            user: 0,
        };
        let current = CoreTimes {
            idle: SECOND,
            kernel: SECOND,
            user: 0,
        };
        let utilisation = Utilisation::between(previous, current);
        assert!(
            utilisation.busy < 0.001,
            "an idle core must read as idle, got {}%",
            utilisation.busy
        );
    }

    #[test]
    fn a_saturated_core_reads_as_fully_busy() {
        let previous = CoreTimes::default();
        let current = CoreTimes {
            idle: 0,
            kernel: 0,
            user: SECOND,
        };
        let utilisation = Utilisation::between(previous, current);
        assert!(
            (utilisation.busy - 100.0).abs() < 0.001,
            "got {}%",
            utilisation.busy
        );
    }

    #[test]
    fn a_half_busy_core_reads_as_half_busy() {
        let previous = CoreTimes::default();
        let current = CoreTimes {
            idle: SECOND / 2,
            kernel: SECOND / 2,
            user: SECOND / 2,
        };
        let utilisation = Utilisation::between(previous, current);
        assert!(
            (utilisation.busy - 50.0).abs() < 0.001,
            "got {}%",
            utilisation.busy
        );
    }

    #[test]
    fn kernel_time_is_reported_without_the_idle_time_inside_it() {
        // A core spending half its time in the kernel and half idle must
        // report 50% kernel, not 100%.
        let previous = CoreTimes::default();
        let current = CoreTimes {
            idle: SECOND / 2,
            kernel: SECOND,
            user: 0,
        };
        let utilisation = Utilisation::between(previous, current);
        assert!(
            (utilisation.kernel - 50.0).abs() < 0.001,
            "kernel time must exclude idle, got {}%",
            utilisation.kernel
        );
        assert!(
            (utilisation.busy - 50.0).abs() < 0.001,
            "and the core is half busy, got {}%",
            utilisation.busy
        );
    }

    #[test]
    fn a_zero_length_interval_is_not_a_division_by_zero() {
        // Two samples taken within one clock tick.
        let same = CoreTimes {
            idle: 5,
            kernel: 10,
            user: 3,
        };
        let utilisation = Utilisation::between(same, same);
        assert_eq!(utilisation.busy, 0.0);
        assert_eq!(utilisation.kernel, 0.0);
        assert!(utilisation.busy.is_finite() && utilisation.kernel.is_finite());
    }

    #[test]
    fn counters_that_appear_to_go_backwards_do_not_produce_a_negative_load() {
        // Counters can appear to move backwards across a sleep/resume.
        // The honest answer for the interval that spanned it is "nothing
        // measurable", never a negative percentage.
        let previous = CoreTimes {
            idle: 100 * SECOND,
            kernel: 200 * SECOND,
            user: 50 * SECOND,
        };
        let current = CoreTimes {
            idle: SECOND,
            kernel: SECOND,
            user: 0,
        };
        let utilisation = Utilisation::between(previous, current);
        assert!(
            utilisation.busy >= 0.0 && utilisation.busy <= 100.0,
            "got {}%",
            utilisation.busy
        );
        assert!(utilisation.kernel >= 0.0 && utilisation.kernel <= 100.0);
    }

    #[test]
    fn an_idle_delta_larger_than_the_interval_is_clamped() {
        // Another resume artefact: idle advances further than the total.
        let previous = CoreTimes::default();
        let current = CoreTimes {
            idle: 10 * SECOND,
            kernel: SECOND,
            user: 0,
        };
        let utilisation = Utilisation::between(previous, current);
        assert_eq!(
            utilisation.busy, 0.0,
            "more idle than interval means idle, not a negative load"
        );
    }

    #[test]
    fn the_overall_figure_sums_ticks_rather_than_averaging_percentages() {
        // One core pinned, fifteen parked. Parked cores accumulate no
        // time at all, so averaging their percentages would report the
        // machine as 100% busy — the pinned core's figure, averaged with
        // fifteen zeroes that are really "no data".
        let mut previous = vec![CoreTimes::default(); 16];
        let mut current = vec![CoreTimes::default(); 16];
        if let (Some(before), Some(after)) = (previous.get_mut(0), current.get_mut(0)) {
            *before = CoreTimes::default();
            *after = CoreTimes {
                idle: 0,
                kernel: 0,
                user: SECOND,
            };
        }
        // The other fifteen advance without doing anything: idle.
        for index in 1..16 {
            if let Some(after) = current.get_mut(index) {
                *after = CoreTimes {
                    idle: SECOND,
                    kernel: SECOND,
                    user: 0,
                };
            }
        }
        let overall = overall(&previous, &current);
        assert!(
            (overall.busy - 6.25).abs() < 0.01,
            "one of sixteen cores busy is 6.25%, got {}%",
            overall.busy
        );
    }

    #[test]
    fn mismatched_core_counts_pair_only_what_both_samples_have() {
        // A processor hot-added between samples.
        let previous = vec![CoreTimes::default(); 2];
        let current = vec![
            CoreTimes {
                idle: SECOND,
                kernel: SECOND,
                user: 0,
            };
            4
        ];
        assert_eq!(utilisation(&previous, &current).len(), 2);
        assert!(overall(&previous, &current).busy.is_finite());
    }

    #[test]
    fn a_partial_trailing_record_is_dropped_rather_than_read() {
        let mut bytes = vec![0u8; PROCESSOR_PERFORMANCE_SIZE * 2 + 7];
        // Give the first core a recognisable idle time.
        if let Some(slot) = bytes.get_mut(0..8) {
            slot.copy_from_slice(&SECOND.to_le_bytes());
        }
        let cores = parse(&bytes);
        assert_eq!(cores.len(), 2, "the seven trailing bytes are not a core");
        assert_eq!(cores.first().map(|core| core.idle), Some(SECOND));
    }

    #[test]
    fn an_empty_buffer_yields_no_cores() {
        assert!(parse(&[]).is_empty());
        assert!(parse(&[0u8; 4]).is_empty());
    }

    #[test]
    fn total_does_not_double_count_idle() {
        let core = CoreTimes {
            idle: 3,
            kernel: 10,
            user: 5,
        };
        assert_eq!(
            core.total(),
            15,
            "kernel already contains idle, so the total is kernel + user"
        );
    }
}

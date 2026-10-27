// ============================================================================
// Module:       model::gpu
// Description:  Carrying the last GPU reading across a sample the performance
//               counters dropped, so the panel does not flicker out and back.
//
// Dependencies: std; super::GpuSample
// ============================================================================

//! Bridging a dropped GPU reading.
//!
//! Every other figure in a sample comes from a call that either answers
//! or fails outright. The GPU figures do not: they come from PDH, whose
//! `GPU Engine` counter publishes **one instance per process per engine**
//! — `pid_1234_luid_..._eng_0_engtype_3D` — so the instance set churns
//! every time anything on the machine opens or closes a GPU context.
//! Reading it is an ask-then-fetch pair, and a set that grows between the
//! two calls makes the fetch fail; `win::gpu` retries, and a reading that
//! still comes back empty is reported as "no adapters".
//!
//! That answer is indistinguishable, to everything downstream, from a
//! machine whose driver publishes no GPU counters at all — and the
//! Performance view treats the two the same, because it has to: an entry
//! permanently reading 0% on a machine with no counters would describe a
//! GPU that is never used. So one dropped reading removed the GPU entry
//! from the picker, dropped the four cards below it up by a row, and put
//! it all back on the next sample. That is the flicker this exists to
//! stop.
//!
//! ## Holding, rather than zeroing
//!
//! The other way to keep the entry on screen is to publish the adapters
//! with a utilisation of zero. That is worse, and not marginally: zero is
//! a *claim* — it says the GPU was idle for that interval — and it lands
//! in the history ring as a notch in the middle of a busy graph. Holding
//! the previous reading claims only that nothing newer is known, which is
//! true, and costs one tick of staleness in exchange.
//!
//! ## The per-process GPU columns are not held
//!
//! Only the adapters are. The process table's GPU and GPU-memory columns
//! come from the same dropped reading and go to zero for that tick, and
//! they stay that way on purpose: those figures are keyed by bare PID in
//! the counter instance names, and Windows recycles a PID between one
//! sample and the next — so republishing them would be attributing a
//! dead process's GPU use to whatever now holds its number, which is the
//! one thing [`super::ProcessKey`] exists to prevent. A zero in a column
//! for one tick is the cheaper wrong answer.
//!
//! ## Bounded, because a GPU can genuinely leave
//!
//! A driver uninstall, an external GPU unplugged, a counter provider that
//! stops publishing: in each the reading is empty and stays empty, and a
//! panel still showing last week's adapter is the failure mode this would
//! trade the flicker for. [`Hold`] therefore bridges `CONSECUTIVE_GAPS`
//! readings and then lets the GPU go.
//!
//! Counted in samples rather than seconds because every other history in
//! this app is — the graphs hold a fixed number of points and derive
//! their time axis from the configured interval, so a bound in samples is
//! the one that stays in proportion to what is on screen.
//!
//! This is in the portable core rather than in `engine` so the off-by-one
//! in it can be tested on any machine, which is the same reason
//! [`super::rates`] is here.

use super::GpuSample;

/// How many consecutive empty readings are bridged before the adapters
/// are let go.
///
/// Three, which at the default one-second interval also covers a display
/// driver reset — a TDR takes a second or two, and the counters are gone
/// for the duration. One would cover the observed flicker; three cover it
/// and the next most likely cause of an empty reading without letting a
/// GPU that has really gone linger.
const CONSECUTIVE_GAPS: u32 = 3;

/// The last GPU reading, and how many empty ones have followed it.
#[derive(Clone, Debug, Default)]
pub struct Hold {
    /// The most recent non-empty reading, or empty if there has never
    /// been one — a machine with no GPU counters, which must never be
    /// given an entry.
    last: Vec<GpuSample>,
    /// Consecutive empty readings bridged so far.
    gaps: u32,
}

impl Hold {
    /// The adapters to publish for this sample.
    ///
    /// A non-empty reading is published as it is and becomes the one that
    /// is held. An empty one is answered with the held reading, up to
    /// `CONSECUTIVE_GAPS` times in a row; after that the GPU is treated
    /// as gone and the empty reading is passed through.
    #[must_use]
    pub fn resolve(&mut self, fresh: Vec<GpuSample>) -> Vec<GpuSample> {
        if !fresh.is_empty() {
            self.last = fresh.clone();
            self.gaps = 0;
            return fresh;
        }
        if self.last.is_empty() {
            return fresh;
        }
        if self.gaps >= CONSECUTIVE_GAPS {
            self.last = Vec::new();
            return fresh;
        }
        self.gaps = self.gaps.saturating_add(1);
        self.last.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One adapter, named and reading `utilisation`.
    fn adapter(luid: &str, utilisation: f64) -> GpuSample {
        GpuSample {
            luid: luid.to_string(),
            name: "NVIDIA GeForce RTX 4070".to_string(),
            utilisation,
            engines: vec![("3D · 0:0".to_string(), utilisation)],
            memory_used: 1_024,
            memory_total: 12_884_901_888,
        }
    }

    /// The adapter LUIDs a reading carries, which is what identifies it.
    fn luids(samples: &[GpuSample]) -> Vec<&str> {
        samples.iter().map(|gpu| gpu.luid.as_str()).collect()
    }

    #[test]
    fn a_reading_that_arrived_is_published_unchanged() {
        let mut hold = Hold::default();
        let published = hold.resolve(vec![adapter("0x0_0x1", 42.0)]);
        assert_eq!(luids(&published), ["0x0_0x1"]);
        assert_eq!(
            published.first().map(|gpu| gpu.utilisation),
            Some(42.0),
            "a reading PDH answered must reach the snapshot as it is"
        );
    }

    #[test]
    fn one_dropped_reading_keeps_the_adapters_and_their_last_figure() {
        // The bug: a single empty reading took the GPU entry out of the
        // picker and let the four cards below it jump up a row.
        let mut hold = Hold::default();
        let _ = hold.resolve(vec![adapter("0x0_0x1", 8.4)]);
        let bridged = hold.resolve(Vec::new());
        assert_eq!(luids(&bridged), ["0x0_0x1"]);
        assert_eq!(
            bridged.first().map(|gpu| gpu.utilisation),
            Some(8.4),
            "the held figure is the last one known, not a zero that would \
             claim the GPU had gone idle"
        );
    }

    #[test]
    fn a_machine_that_has_never_reported_a_gpu_is_never_given_one() {
        // No GPU counters at all is a normal configuration — a VM with a
        // basic display adapter, a server with no display driver. It must
        // not acquire an adapter from this.
        let mut hold = Hold::default();
        for _ in 0..10 {
            assert!(
                hold.resolve(Vec::new()).is_empty(),
                "a machine with no GPU counters must report none"
            );
        }
    }

    #[test]
    fn a_gpu_that_stays_gone_is_let_go() {
        let mut hold = Hold::default();
        let _ = hold.resolve(vec![adapter("0x0_0x1", 8.4)]);
        for gap in 0..CONSECUTIVE_GAPS {
            assert!(
                !hold.resolve(Vec::new()).is_empty(),
                "gap {gap} is within the bridge and must still report the adapter"
            );
        }
        assert!(
            hold.resolve(Vec::new()).is_empty(),
            "a reading that stays empty past the bridge is the GPU really \
             going away, and a stale adapter is worse than none"
        );
        assert!(
            hold.resolve(Vec::new()).is_empty(),
            "and it stays gone rather than the bridge restarting"
        );
    }

    #[test]
    fn a_reading_that_arrives_restores_the_whole_bridge() {
        // Otherwise a machine dropping one reading in three would spend
        // its gaps and then flicker exactly as before.
        let mut hold = Hold::default();
        let _ = hold.resolve(vec![adapter("0x0_0x1", 8.4)]);
        for _ in 0..CONSECUTIVE_GAPS {
            let _ = hold.resolve(Vec::new());
        }
        let _ = hold.resolve(vec![adapter("0x0_0x1", 11.0)]);
        for gap in 0..CONSECUTIVE_GAPS {
            assert!(
                !hold.resolve(Vec::new()).is_empty(),
                "gap {gap} follows a fresh reading and must be bridged"
            );
        }
    }

    #[test]
    fn the_held_reading_is_replaced_rather_than_merged() {
        // An adapter that is gone from a fresh reading is gone. Holding
        // the union would accumulate every adapter the machine ever had.
        let mut hold = Hold::default();
        let _ = hold.resolve(vec![adapter("0x0_0x1", 8.4), adapter("0x0_0x2", 1.0)]);
        let _ = hold.resolve(vec![adapter("0x0_0x2", 2.0)]);
        let bridged = hold.resolve(Vec::new());
        assert_eq!(
            luids(&bridged),
            ["0x0_0x2"],
            "the bridge republishes the last reading, not every adapter seen"
        );
    }
}

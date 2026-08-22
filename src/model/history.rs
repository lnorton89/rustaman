// ============================================================================
// Module:       model::history
// Description:  The fixed-capacity ring every graph is drawn from, and the
//               scale maths that keeps a chart's axis from twitching.
//
// Dependencies: std only.
// ============================================================================

//! The rolling window of samples behind every graph.
//!
//! A [`Series`] is a fixed-capacity ring of `f32`. Fixed capacity is the
//! whole point: a task manager runs for days, samples once a second, and
//! a `Vec` that merely grows is a slow memory leak with a graph in front
//! of it. The ring is allocated once at its full size and never
//! reallocates, so a window open for a week costs exactly what one open
//! for a minute does.
//!
//! `f32` rather than `f64` because this is display data — it is about to
//! become a pixel coordinate — and halving it halves the footprint of the
//! per-core series on a machine with 64 logical processors.
//!
//! ## Why the scale is not just the maximum
//!
//! A graph auto-scaled to the maximum of its window rescales whenever
//! that maximum leaves the window, and every point jumps at once. On a
//! network graph — where the range between idle and a burst is four
//! orders of magnitude — this happens constantly and makes the chart
//! unreadable: you cannot tell a real change in traffic from the axis
//! moving under it. [`Series::scale`] handles that by quantising the
//! maximum to a "nice" round number and holding it, so the axis moves in
//! visible steps with a label that changed, rather than continuously.

/// A fixed-capacity ring of samples, oldest first when iterated.
#[derive(Clone, Debug)]
pub struct Series {
    /// The backing store, always exactly `capacity` long once filled.
    samples: Vec<f32>,
    /// Where the next sample goes.
    head: usize,
    /// How many samples have been pushed, capped at the capacity.
    filled: usize,
}

impl Series {
    /// Creates an empty series holding at most `capacity` samples.
    ///
    /// A capacity of zero is rounded up to one: a series that cannot hold
    /// a sample would make every accessor a special case, and no caller
    /// wants one.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        let capacity = capacity.max(1);
        Self {
            samples: vec![0.0; capacity],
            head: 0,
            filled: 0,
        }
    }

    /// Appends a sample, discarding the oldest once full.
    ///
    /// A non-finite value is stored as zero. A `NaN` in a series
    /// poisons everything downstream — the maximum, the scale, and then
    /// every point's y coordinate, so one bad sample blanks the whole
    /// graph rather than showing one bad point.
    pub fn push(&mut self, value: f32) {
        let value = if value.is_finite() { value } else { 0.0 };
        // `head` is always < capacity, which the modulo below maintains;
        // the `get_mut` keeps this total rather than resting on that.
        if let Some(slot) = self.samples.get_mut(self.head) {
            *slot = value;
        }
        self.head = (self.head + 1) % self.samples.len();
        self.filled = (self.filled + 1).min(self.samples.len());
    }

    /// How many samples are currently held.
    #[must_use]
    pub fn len(&self) -> usize {
        self.filled
    }

    /// Whether no sample has been pushed yet.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.filled == 0
    }

    /// The most that can be held.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.samples.len()
    }

    /// The samples, oldest first.
    ///
    /// An iterator rather than a slice because the ring wraps: the data
    /// is in two runs in memory and only one order makes sense to a
    /// reader. Returning it lazily avoids the allocation a joined `Vec`
    /// would cost on every frame of every graph.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = f32> + '_ {
        let capacity = self.samples.len();
        // The oldest sample sits `filled` slots behind the head, modulo
        // the capacity.
        let start = (self.head + capacity - self.filled) % capacity;
        (0..self.filled).map(move |offset| {
            self.samples
                .get((start + offset) % capacity)
                .copied()
                .unwrap_or(0.0)
        })
    }

    /// The most recent sample, or zero if there is none.
    ///
    /// Zero rather than `Option` because every caller is a readout that
    /// has to print something, and "no samples yet" lasts for exactly one
    /// frame at startup.
    #[must_use]
    pub fn latest(&self) -> f32 {
        if self.filled == 0 {
            return 0.0;
        }
        let capacity = self.samples.len();
        self.samples
            .get((self.head + capacity - 1) % capacity)
            .copied()
            .unwrap_or(0.0)
    }

    /// The largest sample currently held.
    #[must_use]
    pub fn max(&self) -> f32 {
        self.iter().fold(0.0f32, f32::max)
    }

    /// The mean of the samples currently held, or zero when empty.
    #[must_use]
    pub fn mean(&self) -> f32 {
        if self.filled == 0 {
            return 0.0;
        }
        let sum: f32 = self.iter().sum();
        sum / self.filled as f32
    }

    /// Discards every sample.
    ///
    /// Used when the sampling interval changes: the window's samples were
    /// taken seconds apart and the new ones will not be, so a graph
    /// drawn from both would compress part of its own history without
    /// saying so.
    pub fn clear(&mut self) {
        self.samples.iter_mut().for_each(|slot| *slot = 0.0);
        self.head = 0;
        self.filled = 0;
    }

    /// Resizes the window, keeping the most recent samples that still fit.
    ///
    /// Growing keeps everything; shrinking drops the oldest. Either way
    /// the ring is reallocated exactly once, here, rather than drifting
    /// towards the new size a push at a time.
    pub fn resize(&mut self, capacity: usize) {
        let capacity = capacity.max(1);
        if capacity == self.samples.len() {
            return;
        }
        let kept: Vec<f32> = self
            .iter()
            .skip(self.filled.saturating_sub(capacity))
            .collect();
        self.samples = vec![0.0; capacity];
        self.filled = 0;
        self.head = 0;
        for value in kept {
            self.push(value);
        }
    }

    /// A stable upper bound for a graph's y axis.
    ///
    /// `floor` is the smallest axis the caller will accept — 100 for a
    /// percentage graph, which then never rescales at all, or a byte rate
    /// for a network chart, which has no natural ceiling.
    ///
    /// The returned value is the series maximum rounded *up* to one of
    /// the 1/2/5-times-a-power-of-ten steps, with headroom. See the module
    /// docs for why this is not just the maximum: quantising means the
    /// axis holds still while traffic varies within a step, and moves in
    /// one visible jump — with a different label — when it does not.
    #[must_use]
    pub fn scale(&self, floor: f32) -> f32 {
        let peak = self.max();
        // A percentage graph passes a floor of 100 and means it: the axis
        // must be *exactly* 100, not the next nice step above 100 plus
        // headroom. Two CPU graphs side by side are only comparable at a
        // glance if they share an axis, and an axis of 200 on an idle
        // machine also wastes half the panel drawing empty space.
        //
        // Applying the headroom before this check is what got that wrong:
        // a 40% peak became `nice_ceiling(115)` — an axis of 200.
        if peak <= floor {
            return floor.max(MIN_SCALE);
        }
        // Past the floor, 15% headroom so a series touching the top of
        // its range is not drawn flush against the frame, where it reads
        // as clipped.
        nice_ceiling(peak * 1.15).max(floor)
    }
}

/// The smallest axis a graph may be given.
///
/// An axis of zero would divide every point's coordinate by zero. Reached
/// only by an empty series with no floor, which is the state every graph
/// is in for its first frame.
const MIN_SCALE: f32 = 1.0;

/// Rounds `value` up to the next 1, 2, or 5 times a power of ten.
///
/// The steps a human-readable axis is built from: 1, 2, 5, 10, 20, 50,
/// 100 and so on. Anything else produces axis labels like "3.7 MB/s",
/// which nobody can divide by eye to read a midpoint off the graph.
fn nice_ceiling(value: f32) -> f32 {
    if !value.is_finite() || value <= 0.0 {
        return 1.0;
    }
    let magnitude = 10f32.powf(value.log10().floor());
    // Guard the degenerate case where a subnormal input makes the
    // magnitude zero, which would divide by zero below.
    if magnitude <= 0.0 {
        return 1.0;
    }
    let normalised = value / magnitude;
    let step = if normalised <= 1.0 {
        1.0
    } else if normalised <= 2.0 {
        2.0
    } else if normalised <= 5.0 {
        5.0
    } else {
        10.0
    };
    step * magnitude
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_series_never_grows_past_its_capacity() {
        // The reason the ring exists: a task manager left open for a week
        // must not accumulate a week of samples.
        let mut series = Series::new(4);
        for value in 0..1_000 {
            series.push(value as f32);
        }
        assert_eq!(series.len(), 4, "the window is fixed");
        assert_eq!(series.capacity(), 4);
    }

    #[test]
    fn iteration_yields_the_window_oldest_first_across_the_wrap() {
        let mut series = Series::new(4);
        for value in 1..=6 {
            series.push(value as f32);
        }
        assert_eq!(
            series.iter().collect::<Vec<_>>(),
            vec![3.0, 4.0, 5.0, 6.0],
            "the two runs in memory must read back in time order"
        );
        assert_eq!(series.latest(), 6.0);
    }

    #[test]
    fn a_partly_filled_series_reads_back_without_its_unwritten_slots() {
        let mut series = Series::new(8);
        series.push(1.0);
        series.push(2.0);
        assert_eq!(
            series.iter().collect::<Vec<_>>(),
            vec![1.0, 2.0],
            "the zeroes the ring was allocated with are not samples, and a \
             graph that drew them would show a flat line back to the start \
             of time"
        );
        assert_eq!(series.len(), 2);
    }

    #[test]
    fn an_empty_series_answers_every_accessor_without_a_special_case() {
        let series = Series::new(16);
        assert!(series.is_empty());
        assert_eq!(series.len(), 0);
        assert_eq!(series.latest(), 0.0);
        assert_eq!(series.max(), 0.0);
        assert_eq!(series.mean(), 0.0);
        assert_eq!(series.iter().count(), 0);
        assert!(series.scale(100.0) >= 100.0, "the floor still applies");
    }

    #[test]
    fn a_zero_capacity_series_still_works() {
        let mut series = Series::new(0);
        series.push(5.0);
        assert_eq!(series.capacity(), 1, "capacity is rounded up to one");
        assert_eq!(series.latest(), 5.0);
    }

    #[test]
    fn a_non_finite_sample_is_stored_as_zero() {
        // One NaN would otherwise poison the maximum, the scale, and then
        // every point's coordinate — blanking the whole graph.
        let mut series = Series::new(4);
        series.push(10.0);
        series.push(f32::NAN);
        series.push(f32::INFINITY);
        assert!(
            series.max().is_finite(),
            "a single bad sample must not blank the graph"
        );
        assert_eq!(series.max(), 10.0);
        assert!(series.scale(0.0).is_finite());
    }

    #[test]
    fn the_mean_covers_only_the_samples_actually_held() {
        let mut series = Series::new(10);
        series.push(2.0);
        series.push(4.0);
        assert_eq!(
            series.mean(),
            3.0,
            "the eight unwritten slots must not drag the average to 0.6"
        );
    }

    #[test]
    fn a_percentage_graph_holds_a_fixed_axis() {
        // Floor 100 means the axis never moves, which is what makes two
        // CPU graphs side by side comparable at a glance.
        let mut series = Series::new(16);
        for value in [0.0, 3.0, 40.0, 12.0] {
            series.push(value);
        }
        assert_eq!(series.scale(100.0), 100.0);
    }

    #[test]
    fn an_unbounded_graph_quantises_its_axis_rather_than_tracking_the_peak() {
        // The network-graph case: the axis must hold still while traffic
        // varies within a step.
        let mut low = Series::new(16);
        low.push(1_100_000.0);
        let mut higher = Series::new(16);
        higher.push(1_400_000.0);
        assert_eq!(
            low.scale(0.0),
            higher.scale(0.0),
            "a 27% change in traffic within one axis step must not move the \
             axis, or a real change cannot be told from the scale shifting"
        );

        let mut much_higher = Series::new(16);
        much_higher.push(4_000_000.0);
        assert!(
            much_higher.scale(0.0) > low.scale(0.0),
            "a change that leaves the step must move the axis"
        );
    }

    #[test]
    fn the_axis_always_clears_the_data_with_headroom() {
        let mut series = Series::new(16);
        for peak in [0.3f32, 7.0, 999.0, 1_048_576.0, 3.5e9] {
            series.clear();
            series.push(peak);
            let scale = series.scale(0.0);
            assert!(
                scale >= peak,
                "the axis {scale} must not clip a peak of {peak}"
            );
            assert!(
                scale.is_finite() && scale > 0.0,
                "a degenerate axis of {scale} would divide every point by zero"
            );
        }
    }

    #[test]
    fn nice_ceilings_land_on_readable_steps() {
        // Axis labels a reader can halve by eye.
        assert_eq!(nice_ceiling(0.7), 1.0);
        assert_eq!(nice_ceiling(1.5), 2.0);
        assert_eq!(nice_ceiling(3.0), 5.0);
        assert_eq!(nice_ceiling(6.0), 10.0);
        assert_eq!(nice_ceiling(45.0), 50.0);
        assert_eq!(nice_ceiling(101.0), 200.0);
    }

    #[test]
    fn a_degenerate_ceiling_input_does_not_divide_by_zero() {
        for bad in [0.0, -5.0, f32::NAN, f32::INFINITY, f32::MIN_POSITIVE] {
            let ceiling = nice_ceiling(bad);
            assert!(
                ceiling.is_finite() && ceiling > 0.0,
                "{bad} produced an unusable axis of {ceiling}"
            );
        }
    }

    #[test]
    fn resizing_keeps_the_most_recent_samples() {
        let mut series = Series::new(8);
        for value in 1..=8 {
            series.push(value as f32);
        }
        series.resize(3);
        assert_eq!(
            series.iter().collect::<Vec<_>>(),
            vec![6.0, 7.0, 8.0],
            "shrinking drops the oldest, not the newest"
        );

        series.resize(6);
        assert_eq!(
            series.iter().collect::<Vec<_>>(),
            vec![6.0, 7.0, 8.0],
            "growing keeps what was there and does not invent samples"
        );
        assert_eq!(series.capacity(), 6);
    }

    #[test]
    fn clearing_leaves_nothing_behind_the_next_sample_can_read() {
        // Called when the interval changes: the old samples were taken at
        // a different spacing, so a graph drawn from both would silently
        // compress part of its own history.
        let mut series = Series::new(4);
        for value in 1..=4 {
            series.push(value as f32);
        }
        series.clear();
        assert!(series.is_empty());
        assert_eq!(series.max(), 0.0);
        series.push(9.0);
        assert_eq!(
            series.iter().collect::<Vec<_>>(),
            vec![9.0],
            "a cleared series starts from the new sample alone"
        );
    }
}

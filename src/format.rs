// ============================================================================
// Module:       format
// Description:  The number formatting every column and readout shares — byte
//               sizes, throughput rates, percentages, durations, timestamps.
//
// Dependencies: std only. Deliberately portable: this is the arithmetic the
//               non-Windows test job exercises.
// ============================================================================

//! Human-readable rendering of the numbers this app is mostly made of.
//!
//! One module rather than a helper per view, because a task manager's
//! whole job is showing the same quantity in several places at once and
//! the reader has to be able to tell that it *is* the same quantity.
//! Memory in the process table, memory in the tooltip, and memory in the
//! Performance page's readout all come through [`bytes`], so they cannot
//! disagree about whether 1 MB is 1,000,000 or 1,048,576 bytes. (It is
//! 1,048,576: Windows counts memory in binary units everywhere, and a
//! task manager that reported a different number from the one Explorer's
//! properties dialog shows would be wrong in the only way that matters.)
//!
//! Everything here takes an already-computed quantity and returns a
//! `String`. None of it decides *what* to measure — that is [`crate::model`]'s
//! job — and none of it allocates a formatter, caches, or holds state, so
//! calling it from a draw path is fine.
//!
//! ## Why it is significant figures and not decimal places
//!
//! Every function that formats a magnitude emits three significant
//! figures rather than a fixed number of decimal places. A fixed decimal
//! count means a column renders "9.9 MB", then "10.0 MB", then
//! "100.0 MB", growing a character each time it crosses a power of ten;
//! at a one-second refresh that reads as the table twitching. Three
//! significant figures bounds the variation to a single character
//! (`1.07`, `10.7`, `107`) and is more precision than anyone reads off a
//! process list anyway.
//!
//! It is deliberately *not* padded to a fixed character count. The
//! obvious trick — right-pad `107` into the same four-character field as
//! `1.07` — does not work, because the UI font is proportional and a
//! space is narrower than a digit, so the padded strings still do not
//! line up. What actually lines a numeric column up is the renderer, not
//! the formatter: `gui::ui::widgets` right-aligns these cells and draws
//! them in a monospace text style, which makes every digit the same width
//! and the decimal points fall into one column. A leading space here
//! would buy nothing and would follow the value into the clipboard.

/// Binary size units, ascending. The list ends at `TB` on purpose: a
/// process with a petabyte of anything is a bug in this program, and
/// showing `0.0 PB` would hide it.
const BYTE_UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];

/// The divisor between adjacent entries of [`BYTE_UNITS`].
///
/// Binary, not decimal, because that is what Windows reports and what
/// every other Windows tool displays. See the module docs.
const BYTE_STEP: f64 = 1024.0;

/// Formats a byte count for a table cell: `"0 B"`, `"418 MB"`, `"1.42 GB"`.
///
/// Bytes are shown whole — a fractional byte is not a thing — and every
/// larger unit gets three significant figures. See the module docs on why
/// the width is stable rather than the decimal count.
#[must_use]
pub fn bytes(value: u64) -> String {
    if value < BYTE_STEP as u64 {
        return format!("{value} B");
    }
    let (scaled, unit) = scale(value as f64, BYTE_STEP, &BYTE_UNITS);
    format!("{}{unit}", significant(scaled))
}

/// Formats a byte count, rendering zero as an em dash.
///
/// For columns where zero means "this process has never done any of
/// this" — disk bytes for a process that has not touched the disk — and
/// a screen full of `0 B` is noise that hides the rows that have a
/// number. The dash reads as absence, which is what it is.
#[must_use]
pub fn bytes_or_dash(value: u64) -> String {
    if value == 0 {
        return DASH.to_string();
    }
    bytes(value)
}

/// Formats a throughput in bytes per second: `"1.20 MB/s"`.
///
/// Takes an `f64` because a rate is a quotient of a byte delta by an
/// elapsed time that is never exactly the sample interval — rounding it
/// to an integer before formatting throws away the only precision a
/// sub-`KB/s` trickle has.
#[must_use]
pub fn rate(bytes_per_second: f64) -> String {
    if !bytes_per_second.is_finite() || bytes_per_second < 1.0 {
        return format!("0 {}/s", BYTE_UNITS[0]);
    }
    let (scaled, unit) = scale(bytes_per_second, BYTE_STEP, &BYTE_UNITS);
    format!("{}{unit}/s", significant(scaled))
}

/// Formats a throughput, rendering anything below 1 B/s as an em dash.
///
/// The counterpart of [`bytes_or_dash`] for the live rate columns, and
/// the reason those columns are readable at all: on an idle machine all
/// but a handful of the four hundred rows have no disk or network
/// activity, and `0 B/s` four hundred times is a wall of text with the
/// signal buried in it.
#[must_use]
pub fn rate_or_dash(bytes_per_second: f64) -> String {
    if !bytes_per_second.is_finite() || bytes_per_second < 1.0 {
        return DASH.to_string();
    }
    rate(bytes_per_second)
}

/// Formats a 0..=100 percentage the way the process table's CPU column
/// wants it: `"0%"`, `"3.4%"`, `"100%"`.
///
/// Below ten it keeps one decimal, because the difference between 0.1%
/// and 4% is the difference between a process that is idle and one that
/// is doing something. At and above ten it drops the decimal: nobody
/// reads the tenth of a percent on a core-saturating process, and the
/// extra glyph costs the column its stable width.
#[must_use]
pub fn percent(value: f64) -> String {
    if !value.is_finite() || value <= 0.0 {
        return "0%".to_string();
    }
    if value < 10.0 {
        return format!("{value:.1}%");
    }
    format!("{}%", value.round() as i64)
}

/// Formats a percentage, rendering "nothing at all" as an em dash.
///
/// The threshold is 0.05 rather than zero so that a value that would
/// round to `0.0%` shows as absent rather than as a decimal that is
/// always zero. See [`rate_or_dash`] for why the dash matters.
#[must_use]
pub fn percent_or_dash(value: f64) -> String {
    if !value.is_finite() || value < 0.05 {
        return DASH.to_string();
    }
    percent(value)
}

/// Formats a duration in seconds as `"H:MM:SS"`, or `"Dd H:MM:SS"` past a
/// day: process and system uptime.
///
/// Days are broken out rather than left to accumulate in the hours field
/// because "73:14:02" is a number nobody converts in their head, and the
/// most common reason to look at this column at all is to find the
/// process that has been up since the last reboot.
#[must_use]
pub fn duration(total_seconds: u64) -> String {
    let seconds = total_seconds % 60;
    let minutes = (total_seconds / 60) % 60;
    let hours = (total_seconds / 3600) % 24;
    let days = total_seconds / 86_400;
    if days > 0 {
        return format!("{days}d {hours}:{minutes:02}:{seconds:02}");
    }
    format!("{hours}:{minutes:02}:{seconds:02}")
}

/// Formats CPU time — the total a process has spent on a core — as
/// `"H:MM:SS.mmm"`.
///
/// Milliseconds are kept where [`duration`] drops them because this is a
/// cumulative total that starts at zero and, for most processes, stays
/// under a second for their whole life. Truncating it to whole seconds
/// would show a permanent `0:00:00` for the majority of the list.
#[must_use]
pub fn cpu_time(milliseconds: u64) -> String {
    let millis = milliseconds % 1_000;
    format!("{}.{millis:03}", duration(milliseconds / 1_000))
}

/// Groups an integer with thousands separators: `"1,048,576"`.
///
/// For counts rather than magnitudes — handles, threads, PIDs-as-totals —
/// where the exact number is the point and a rounded `1.05M` would be
/// useless.
#[must_use]
pub fn count(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    // Walk from the right so the group boundary falls where it should for
    // a length that is not a multiple of three.
    for (index, ch) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// Decimal bit-rate units, ascending — for a link speed, which is the
/// one quantity in this app that is not counted in binary units.
const BIT_UNITS: [&str; 4] = ["bps", "Kbps", "Mbps", "Gbps"];

/// The divisor between adjacent entries of [`BIT_UNITS`].
///
/// A thousand, not 1024. Network link speeds are decimal by every
/// convention that names them: a "gigabit" adapter negotiates
/// 1,000,000,000 bits per second, and dividing that by 1024 three times
/// gives "0.93 Gbps" for a link the adapter, the switch, the driver and
/// the box it came in all call 1 Gbps. This is the opposite call from
/// [`bytes`] and it is deliberate — the two quantities are counted
/// differently by the people who make them.
const BIT_STEP: f64 = 1000.0;

/// Formats a link speed in bits per second: `"1 Gbps"`, `"2.5 Gbps"`,
/// `"100 Mbps"`. Zero — which is what an adapter that is down reports —
/// renders as an em dash.
///
/// Not [`rate`]: that formats *bytes* per second in binary units, and a
/// gigabit adapter run through it reads "119 MB/s", which is arithmetically
/// right and reads as a mistake.
#[must_use]
pub fn link_speed(bits_per_second: u64) -> String {
    if bits_per_second == 0 {
        return DASH.to_string();
    }
    if bits_per_second < BIT_STEP as u64 {
        return format!("{bits_per_second} {}", BIT_UNITS[0]);
    }
    let (scaled, unit) = scale(bits_per_second as f64, BIT_STEP, &BIT_UNITS);
    // Whole speeds are written whole. Every common link speed is a round
    // number in its own unit — 100 Mbps, 1 Gbps, 2.5 Gbps, 10 Gbps — and
    // `significant` would render the first of those "100" and the second
    // "1.00", so a machine with both would show one padded to a
    // precision the other does not have.
    if (scaled.fract()).abs() < 0.05 {
        format!("{} {unit}", scaled.round())
    } else {
        format!("{}{unit}", significant(scaled))
    }
}

/// The em dash standing in for "no value here".
///
/// A `pub const` rather than a literal at each call site because the
/// tests assert against it, and because a column that used a hyphen
/// while its neighbour used an em dash would look like one of them was
/// showing data.
pub const DASH: &str = "—";

/// Divides `value` down through `units` until it is below `step`,
/// returning the scaled value and the unit it landed in.
///
/// The loop stops at the last unit rather than running off the end of
/// the array, which is what makes an absurd input render as a large
/// number of terabytes instead of panicking.
fn scale(value: f64, step: f64, units: &[&'static str]) -> (f64, &'static str) {
    let mut scaled = value;
    let mut index = 0usize;
    while scaled >= step && index + 1 < units.len() {
        scaled /= step;
        index += 1;
    }
    // `units` is never empty at any call site, and the loop leaves
    // `index` in range by construction; the fallback keeps this total
    // rather than relying on that reasoning holding forever.
    (scaled, units.get(index).copied().unwrap_or(""))
}

/// Renders `value` to three significant figures with a trailing space,
/// ready for a unit to be appended.
///
/// Unpadded: see the module docs on why a fixed character count is the
/// wrong tool for lining up a column in a proportional font.
fn significant(value: f64) -> String {
    if value < 10.0 {
        format!("{value:.2} ")
    } else if value < 100.0 {
        format!("{value:.1} ")
    } else {
        format!("{} ", value.round() as i64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_below_a_kilobyte_are_whole() {
        assert_eq!(bytes(0), "0 B", "zero should not gain a decimal");
        assert_eq!(bytes(1), "1 B", "a single byte is not 1.00 B");
        assert_eq!(bytes(1023), "1023 B", "the last byte before the step");
    }

    #[test]
    fn bytes_step_in_binary_units_not_decimal_ones() {
        assert_eq!(
            bytes(1024),
            "1.00 KB",
            "1024 bytes is one binary kilobyte, which is what Windows reports"
        );
        assert_eq!(
            bytes(1_048_576),
            "1.00 MB",
            "a binary megabyte, matching Explorer's properties dialog"
        );
        assert_eq!(
            bytes(1_000_000),
            "977 KB",
            "a decimal megabyte is not a megabyte here"
        );
    }

    #[test]
    fn a_magnitude_keeps_three_significant_figures_at_every_scale() {
        // The property that matters is bounded variation: a column whose
        // values cross a power of ten may not grow without limit. Three
        // significant figures caps it at one character; a fixed decimal
        // count would not.
        let widths: Vec<usize> = [
            1_100u64,
            11_000,
            110_000,
            1_100_000,
            11_000_000,
            110_000_000,
        ]
        .into_iter()
        .map(|value| bytes(value).chars().count())
        .collect();
        let lo = widths.iter().copied().min().unwrap_or(0);
        let hi = widths.iter().copied().max().unwrap_or(0);
        assert!(
            hi - lo <= 1,
            "byte renderings should vary by at most one character, got \
             {widths:?}"
        );
        assert!(
            widths.iter().all(|width| *width >= 6),
            "and none should collapse to nothing, got {widths:?}"
        );
    }

    #[test]
    fn an_absurd_size_saturates_at_the_largest_unit() {
        let rendered = bytes(u64::MAX);
        assert!(
            rendered.ends_with(" TB"),
            "the scale should stop at TB rather than running off the unit \
             table, got {rendered}"
        );
    }

    #[test]
    fn a_rate_below_one_byte_per_second_reads_as_nothing() {
        assert_eq!(rate(0.0), "0 B/s", "an explicit zero rate");
        assert_eq!(rate(0.4), "0 B/s", "a rate that rounds to nothing");
        assert_eq!(rate_or_dash(0.4), DASH, "the dashing variant hides it");
        assert_eq!(
            rate_or_dash(2048.0),
            "2.00 KB/s",
            "a real rate is shown by both variants alike"
        );
    }

    #[test]
    fn a_link_speed_is_decimal_and_written_the_way_the_box_writes_it() {
        // The regression: the Network panel used to print
        // `link_speed / 1_000_000` with "Mbps" glued on, so a 2.5GbE
        // adapter read "2500 Mbps" and a 100 Mbps one read "100 Mbps"
        // in the same column — and running the figure through `rate`
        // instead would have made a gigabit link read "119 MB/s".
        assert_eq!(link_speed(1_000_000_000), "1 Gbps");
        assert_eq!(link_speed(2_500_000_000), "2.50 Gbps");
        assert_eq!(link_speed(100_000_000), "100 Mbps");
        assert_eq!(link_speed(10_000_000_000), "10 Gbps");
        assert_eq!(
            link_speed(0),
            DASH,
            "an adapter that is down reports no speed, and a zero there \
             would read as a link running at nothing"
        );
    }

    #[test]
    fn a_non_finite_rate_does_not_reach_the_formatter() {
        // A rate is a division by an elapsed time, and a sampler that is
        // called twice within one clock tick divides by zero. That must
        // render as nothing, not as "NaN B/s" or "inf B/s".
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -1.0] {
            assert_eq!(rate(bad), "0 B/s", "{bad} should render as no rate");
            assert_eq!(rate_or_dash(bad), DASH, "{bad} should render as absent");
        }
    }

    #[test]
    fn percentages_keep_a_decimal_only_where_it_carries_information() {
        assert_eq!(percent(0.0), "0%", "zero is not 0.0%");
        assert_eq!(percent(3.42), "3.4%", "a small load keeps its decimal");
        assert_eq!(percent(42.6), "43%", "a large one does not need it");
        assert_eq!(percent(100.0), "100%", "a saturated core");
    }

    #[test]
    fn a_percentage_that_would_round_to_zero_reads_as_absent() {
        assert_eq!(
            percent_or_dash(0.01),
            DASH,
            "a value that would render as 0.0% should read as absent"
        );
        assert_eq!(
            percent_or_dash(0.06),
            "0.1%",
            "a value that survives rounding should still be shown"
        );
    }

    #[test]
    fn durations_break_out_days_rather_than_accumulating_hours() {
        assert_eq!(duration(0), "0:00:00", "a process that just started");
        assert_eq!(duration(59), "0:00:59", "under a minute");
        assert_eq!(duration(3_661), "1:01:01", "an hour, a minute, a second");
        assert_eq!(
            duration(263_642),
            "3d 1:14:02",
            "three days should not render as 73 hours"
        );
    }

    #[test]
    fn cpu_time_keeps_the_milliseconds_a_short_lived_process_only_has() {
        assert_eq!(
            cpu_time(15),
            "0:00:00.015",
            "a process with 15ms of CPU should not read as 0:00:00"
        );
        assert_eq!(cpu_time(3_661_500), "1:01:01.500", "hours and milliseconds");
    }

    #[test]
    fn counts_are_grouped_from_the_right() {
        assert_eq!(count(0), "0", "a single digit gains no separator");
        assert_eq!(count(999), "999", "the last ungrouped value");
        assert_eq!(count(1_000), "1,000", "the first grouped one");
        assert_eq!(
            count(1_048_576),
            "1,048,576",
            "grouping should not depend on the digit count being a multiple of three"
        );
    }
}

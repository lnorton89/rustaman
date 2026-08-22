// ============================================================================
// Module:       color
// Description:  The colour type the theme catalog is written in, the maths that
//               derives shades from it, and the WCAG contrast check.
//
// Dependencies: std only. Deliberately free of egui: the catalog is data, and
//               the contrast rules are testable without a window.
// ============================================================================

//! Colour as data, one layer below the drawing code.
//!
//! [`Rgb`] is what a theme file parses into and what
//! [`crate::theme::Palette`] is made of. It is not an egui type on
//! purpose: the theme catalog, the shade derivation, and the contrast
//! rules that reject an unreadable theme are all pure arithmetic, and
//! keeping them here means the test that walks every shipped theme and
//! checks it against WCAG runs on any machine rather than needing a GPU
//! and a window. `gui::ui::theme` converts to `Color32` at the boundary,
//! once.
//!
//! ## The rainbow
//!
//! The accent scheme this app is built around is a *ramp*, not a colour:
//! a theme states an accent and a hue span, and [`Ramp`] hands out evenly
//! spaced hues across it. Per-core CPU graphs, the series in a stacked
//! chart, and the category chips all index into the same ramp, so the
//! third core is the same colour in the graph as in the legend beside it
//! without either of them holding a literal.
//!
//! The span matters more than the colours. A full 360° rainbow puts a
//! muddy yellow-green next to a saturated red and reads as a clown suit;
//! it also collides with the semantic colours — a "healthy" green series
//! sitting beside the danger red is exactly the ambiguity a status colour
//! exists to avoid. So a theme states where its rainbow starts and how
//! far it runs, and the default runs from green-cyan through blue and
//! violet to magenta.
//!
//! ## Why the ramp is built in OKLCh and not HSL
//!
//! A ramp has one job beyond being colourful: no series may look more
//! important than its neighbours. Every series in a per-core CPU graph is
//! the same *kind* of thing, so if one of them is visually louder, the
//! chart is saying something that is not true.
//!
//! HSL cannot deliver that, and the failure is not subtle. Holding HSL's
//! `l` fixed and sweeping the hue produces colours whose actual perceived
//! lightness varies enormously — `hsl(60, 76%, 57%)` (yellow) and
//! `hsl(260, 76%, 57%)` (blue-violet) are nominally the same lightness,
//! and the first is roughly six times as luminous as the second. The
//! first draft of this module did exactly that, and
//! `a_ramp_holds_one_perceived_lightness` measured the spread across a
//! sixteen-series ramp at 0.10 to 0.62. In a chart that reads as the
//! green core being highlighted and the blue one being disabled.
//!
//! [`Oklch`] fixes it, because that is what the space is for: its `l` is
//! a model of perceived lightness, so holding it constant while sweeping
//! `h` gives colours that genuinely carry the same visual weight. The
//! cost is a gamut problem — not every (l, c, h) is a real sRGB colour,
//! and the saturated end of a wide sweep runs off the edge — which
//! [`Oklch::to_rgb`] handles by walking the chroma down until the colour
//! fits rather than clipping channels, since clipping changes the hue as
//! well as the intensity.

/// An opaque 24-bit colour.
///
/// Deliberately not a wrapper over an egui type: see the module docs.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Rgb {
    /// Red channel, 0..=255.
    pub r: u8,
    /// Green channel, 0..=255.
    pub g: u8,
    /// Blue channel, 0..=255.
    pub b: u8,
}

impl Rgb {
    /// Constructs a colour from its channels.
    #[must_use]
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// Parses `#rrggbb` or `#rgb`, case-insensitively, with the `#`
    /// optional.
    ///
    /// Returns `None` rather than a fallback colour for anything else.
    /// A theme file with a typo in it should be reported as a broken
    /// theme, not silently rendered in black — see
    /// [`crate::theme`] for what the loader does with the `None`.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let hex = text.trim().trim_start_matches('#');
        let nibble = |c: char| c.to_digit(16).map(|d| d as u8);
        let mut chars = hex.chars();
        match hex.len() {
            // `#rgb` shorthand: each nibble is doubled, so `#f0a` is
            // `#ff00aa` — the same rule CSS uses.
            3 => {
                let r = nibble(chars.next()?)?;
                let g = nibble(chars.next()?)?;
                let b = nibble(chars.next()?)?;
                Some(Self::new(r * 17, g * 17, b * 17))
            }
            6 => {
                let mut byte = || -> Option<u8> {
                    let hi = nibble(chars.next()?)?;
                    let lo = nibble(chars.next()?)?;
                    Some(hi * 16 + lo)
                };
                let r = byte()?;
                let g = byte()?;
                let b = byte()?;
                Some(Self::new(r, g, b))
            }
            _ => None,
        }
    }

    /// Renders back to `#rrggbb`, for the settings page's theme export.
    #[must_use]
    pub fn to_hex(self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }

    /// Mixes towards `other`, `t` clamped to 0..=1.
    ///
    /// Channel-wise in sRGB rather than in a linear or perceptual space.
    /// That is the wrong way to interpolate a gradient in general, but it
    /// is the right way here: every use is a *small* step between two
    /// already-close colours — a hover lift, a heat tint, a scrim — and
    /// sRGB blending is what the eye expects for those. The one place a
    /// long interpolation happens is the rainbow, and that runs through
    /// [`Ramp`] in HSL where it belongs.
    #[must_use]
    pub fn lerp(self, other: Self, t: f32) -> Self {
        // `f32::clamp` propagates a NaN rather than resolving it, and a
        // NaN reaching the cast below becomes 0 in every channel — so a
        // single bad blend factor silently paints black. Every caller
        // computes `t` from a measured quantity divided by another, so
        // this is a real path, not a theoretical one.
        let t = if t.is_nan() { 0.0 } else { t.clamp(0.0, 1.0) };
        let mix = |a: u8, b: u8| -> u8 {
            let a = f32::from(a);
            let b = f32::from(b);
            (a + (b - a) * t).round().clamp(0.0, 255.0) as u8
        };
        Self::new(
            mix(self.r, other.r),
            mix(self.g, other.g),
            mix(self.b, other.b),
        )
    }

    /// Relative luminance, per WCAG 2.1.
    ///
    /// The gamma-expanded, channel-weighted brightness a contrast ratio
    /// is computed from — not the `l` of [`Hsl`], which is a much cruder
    /// measure and would pass colour pairs a reader cannot actually
    /// separate.
    #[must_use]
    pub fn luminance(self) -> f32 {
        fn channel(value: u8) -> f32 {
            let v = f32::from(value) / 255.0;
            if v <= 0.040_45 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        }
        0.2126 * channel(self.r) + 0.7152 * channel(self.g) + 0.0722 * channel(self.b)
    }

    /// The WCAG contrast ratio between two colours, from 1.0 (identical)
    /// to 21.0 (black on white).
    ///
    /// This is what makes an unreadable theme a failing build rather than
    /// a subtly unreadable window: see the test in [`crate::theme`].
    #[must_use]
    pub fn contrast(self, other: Self) -> f32 {
        let a = self.luminance();
        let b = other.luminance();
        let (lighter, darker) = if a > b { (a, b) } else { (b, a) };
        (lighter + 0.05) / (darker + 0.05)
    }

    /// Whether this colour is dark enough that light text belongs on it.
    ///
    /// Used to pick the label colour for a chip whose background is a
    /// ramp colour — the label has to stay readable as the hue moves
    /// through yellow, which is far brighter than the violet at the other
    /// end of the same ramp.
    #[must_use]
    pub fn prefers_light_text(self) -> bool {
        // The crossover where black and white text score the same
        // contrast against a background is a luminance of about 0.179.
        self.luminance() < 0.179
    }
}

/// A colour in the Oklab perceptual space.
///
/// `l` is perceived lightness on 0..=1; `a` and `b` are the green–red and
/// blue–yellow opponent axes, roughly -0.4..=0.4 for real colours. See
/// the module docs for why the ramp lives here rather than in HSL.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Oklab {
    /// Perceived lightness, 0..=1.
    pub l: f32,
    /// Green–red opponent axis.
    pub a: f32,
    /// Blue–yellow opponent axis.
    pub b: f32,
}

impl Oklab {
    /// Converts from sRGB.
    #[must_use]
    pub fn from_rgb(rgb: Rgb) -> Self {
        let (r, g, b) = (to_linear(rgb.r), to_linear(rgb.g), to_linear(rgb.b));
        // The LMS cone response, then its cube root — the non-linearity
        // that makes the space uniform.
        let l = (0.412_221_47 * r + 0.536_332_54 * g + 0.051_445_995 * b).cbrt();
        let m = (0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b).cbrt();
        let s = (0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_5 * b).cbrt();
        Self {
            l: 0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s,
            a: 1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
            b: 0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
        }
    }

    /// Converts to linear-light RGB, **without** clamping.
    ///
    /// Unclamped on purpose: [`Oklch::to_rgb`] needs to know whether a
    /// colour is outside the sRGB gamut, and a clamped conversion has
    /// already thrown that information away.
    #[must_use]
    fn to_linear_rgb(self) -> [f32; 3] {
        let l = (self.l + 0.396_337_78 * self.a + 0.215_803_76 * self.b).powi(3);
        let m = (self.l - 0.105_561_346 * self.a - 0.063_854_17 * self.b).powi(3);
        let s = (self.l - 0.089_484_18 * self.a - 1.291_485_5 * self.b).powi(3);
        [
            4.076_741_7 * l - 3.307_711_6 * m + 0.230_969_94 * s,
            -1.268_438 * l + 2.609_757_4 * m - 0.341_319_38 * s,
            -0.004_196_086_3 * l - 0.703_418_6 * m + 1.707_614_7 * s,
        ]
    }
}

/// [`Oklab`] in cylindrical form: lightness, chroma, hue.
///
/// The form the ramp is expressed in, because sweeping a rainbow is
/// exactly "hold `l` and `c`, vary `h`".
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Oklch {
    /// Perceived lightness, 0..=1.
    pub l: f32,
    /// Chroma — distance from grey. Unbounded in principle; about 0.33 is
    /// the most sRGB can express, and only at a few hues.
    pub c: f32,
    /// Hue in degrees, 0..360.
    pub h: f32,
}

impl Oklch {
    /// Constructs a colour, wrapping the hue and clamping the rest.
    #[must_use]
    pub fn new(l: f32, c: f32, h: f32) -> Self {
        Self {
            l: l.clamp(0.0, 1.0),
            c: c.max(0.0),
            h: h.rem_euclid(360.0),
        }
    }

    /// Converts from sRGB.
    #[must_use]
    pub fn from_rgb(rgb: Rgb) -> Self {
        let lab = Oklab::from_rgb(rgb);
        Self {
            l: lab.l,
            c: (lab.a * lab.a + lab.b * lab.b).sqrt(),
            h: lab.b.atan2(lab.a).to_degrees().rem_euclid(360.0),
        }
    }

    /// The [`Oklab`] this is the polar form of.
    #[must_use]
    fn to_lab(self) -> Oklab {
        let radians = self.h.to_radians();
        Oklab {
            l: self.l,
            a: self.c * radians.cos(),
            b: self.c * radians.sin(),
        }
    }

    /// Converts to sRGB, reducing chroma until the colour fits the gamut.
    ///
    /// Not every `(l, c, h)` is a real sRGB colour: sRGB can express far
    /// more chroma in yellow and green than in blue, so a ramp that holds
    /// one chroma while sweeping the hue *will* leave the gamut partway
    /// round.
    ///
    /// The obvious fix — clamp each channel to 0..=1 — is wrong in a way
    /// that matters here. Clipping a negative channel shifts the hue and
    /// raises the lightness, so the out-of-gamut part of a ramp comes back
    /// desaturated *and* the wrong colour, and the "same lightness"
    /// property the ramp exists for is broken exactly where it is hardest
    /// to notice. Walking the chroma down instead keeps the hue and the
    /// lightness — the two things a chart legend depends on — and gives up
    /// only saturation, which nothing depends on.
    ///
    /// Bisection rather than an analytic gamut boundary: the boundary has
    /// no closed form, and twelve steps land within 0.0001 of it, which is
    /// far below one 8-bit step.
    #[must_use]
    pub fn to_rgb(self) -> Rgb {
        /// How close to the true gamut boundary to bisect. One 8-bit
        /// step is about 0.004 in linear light, so this is comfortably
        /// finer than anything that can be displayed.
        const TOLERANCE: f32 = 0.000_1;

        fn fits(linear: [f32; 3]) -> bool {
            linear
                .iter()
                .all(|channel| *channel >= -TOLERANCE && *channel <= 1.0 + TOLERANCE)
        }

        let full = self.to_lab().to_linear_rgb();
        let linear = if fits(full) {
            full
        } else {
            // A chroma of zero is grey, which is always in gamut for an
            // `l` in 0..=1 — so the low end of the bisection is known
            // good and the search cannot fail to terminate.
            let (mut low, mut high) = (0.0f32, self.c);
            let mut best = Self::new(self.l, 0.0, self.h).to_lab().to_linear_rgb();
            while high - low > TOLERANCE {
                let middle = f32::midpoint(low, high);
                let candidate = Self::new(self.l, middle, self.h).to_lab().to_linear_rgb();
                if fits(candidate) {
                    best = candidate;
                    low = middle;
                } else {
                    high = middle;
                }
            }
            best
        };

        Rgb::new(
            from_linear(linear[0]),
            from_linear(linear[1]),
            from_linear(linear[2]),
        )
    }
}

/// Expands one sRGB channel to linear light.
fn to_linear(value: u8) -> f32 {
    let v = f32::from(value) / 255.0;
    if v <= 0.040_45 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

/// Encodes one linear-light channel back to sRGB, clamping to the
/// displayable range.
fn from_linear(value: f32) -> u8 {
    if !value.is_finite() {
        return 0;
    }
    let v = value.clamp(0.0, 1.0);
    let encoded = if v <= 0.003_130_8 {
        v * 12.92
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    };
    (encoded * 255.0).round().clamp(0.0, 255.0) as u8
}

/// An evenly spaced band of hues at one perceived lightness: the rainbow
/// accent scheme.
///
/// Constructed from a theme's accent and hue span, then indexed. See the
/// module docs for why it is a span rather than a full circle, and why it
/// is built in OKLCh rather than HSL.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Ramp {
    /// Where the band starts, in degrees.
    start: f32,
    /// How far it runs, in degrees. May exceed 360 for a theme that
    /// really does want the whole circle and then some.
    span: f32,
    /// The chroma every colour in the band shares.
    chroma: f32,
    /// The perceived lightness every colour in the band shares. This is
    /// the property that makes the ramp usable as a chart palette.
    lightness: f32,
}

impl Ramp {
    /// Builds a ramp centred on `accent`, running `span` degrees.
    ///
    /// The lightness and chroma come from the accent rather than being
    /// stated separately, which is what keeps a ramp recognisably part of
    /// its theme: a muted theme gets a muted rainbow, and a light theme
    /// gets one dark enough to read against white. Only the hue varies
    /// across the band.
    #[must_use]
    pub fn around(accent: Rgb, span: f32) -> Self {
        let base = Oklch::from_rgb(accent);
        Self {
            start: base.h - span / 2.0,
            span,
            chroma: base.c.clamp(MIN_RAMP_CHROMA, MAX_RAMP_CHROMA),
            lightness: base.l,
        }
    }

    /// The colour at position `index` of `count`.
    ///
    /// `count` is the number of series being coloured — cores, adapters,
    /// disks — so the band is divided to fit rather than sampled at fixed
    /// stops. Eight cores get eight well-separated hues; sixty-four get
    /// sixty-four close ones, which is still the right answer, because at
    /// that count the graph is read as a texture rather than as
    /// individually identified lines.
    ///
    /// `count` of zero or one returns the middle of the band — the
    /// accent's own hue — rather than dividing by zero.
    #[must_use]
    pub fn at(self, index: usize, count: usize) -> Rgb {
        if count <= 1 {
            return self.hue(0.5);
        }
        // `count - 1` divisions, so the first colour sits at the start of
        // the band and the last at its end, rather than the band's end
        // being an unused stop.
        let t = index.min(count - 1) as f32 / (count - 1) as f32;
        self.hue(t)
    }

    /// The colour for a *stable* identity rather than a position: the
    /// same key always gets the same hue, across sorts and restarts.
    ///
    /// The process table's per-row accent uses this. It cannot use
    /// [`Ramp::at`], because that assigns by index and a row's index
    /// changes every time the sort order does — a process would change
    /// colour when you clicked a column header, which reads as the table
    /// having reloaded rather than resorted.
    #[must_use]
    pub fn for_key(self, key: u64) -> Rgb {
        // A cheap integer hash (splitmix64's finaliser) rather than the
        // raw key: adjacent PIDs are extremely common — a process and the
        // three it just spawned — and using the key directly would give
        // them near-identical hues.
        let mut x = key.wrapping_add(0x9e37_79b9_7f4a_7c15);
        x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        x ^= x >> 31;
        // Take the top 24 bits: the low ones of a finalised hash are no
        // worse, but the top ones are what the mixing was tuned for.
        // 2^24 is exactly representable in f32, so the division is exact.
        const SPREAD: f32 = (1u32 << 24) as f32;
        self.hue((x >> 40) as f32 / SPREAD)
    }

    /// The colour at fraction `t` along the band.
    #[must_use]
    fn hue(self, t: f32) -> Rgb {
        Oklch::new(self.lightness, self.chroma, self.start + self.span * t).to_rgb()
    }

    /// The perceived lightness every colour in this ramp shares.
    ///
    /// Exposed so the tests can assert the property directly rather than
    /// through a proxy that does not actually measure it — see the module
    /// docs on what went wrong when the proxy was WCAG luminance.
    #[must_use]
    pub fn lightness(self) -> f32 {
        self.lightness
    }
}

/// The least chroma a ramp is allowed, however grey its accent.
///
/// Below this the per-series colours in a chart stop being tellable
/// apart, which defeats the only reason the ramp exists. In OKLCh terms
/// 0.09 is a clearly-coloured but unsaturated tone — think a muted teal
/// rather than a neon one.
const MIN_RAMP_CHROMA: f32 = 0.09;

/// The most chroma a ramp will ask for.
///
/// A very saturated accent would otherwise push most of the band out of
/// the sRGB gamut, where [`Oklch::to_rgb`] pulls it back anyway — but
/// unevenly, since how much chroma survives depends on the hue. Capping
/// up front means the whole band is in gamut and therefore evenly
/// saturated, rather than being saturated where sRGB is generous (yellow,
/// green) and washed out where it is not (blue).
const MAX_RAMP_CHROMA: f32 = 0.16;

/// Blends `color` towards `toward` by the load fraction `t`, for the
/// heat tint behind a busy cell.
///
/// The process table tints a CPU or disk cell by how busy it is rather
/// than only printing a number, so a glance down the column finds the
/// heavy rows without reading any of them. The curve is deliberately
/// not linear: `t` is square-rooted, so the tint appears early and then
/// saturates. A linear ramp leaves everything below about 30% looking
/// identical to idle, which is exactly the range where "this process is
/// doing something when it should not be" lives.
#[must_use]
pub fn heat(color: Rgb, toward: Rgb, t: f32) -> Rgb {
    // `f32::clamp` propagates a NaN, and `NaN.sqrt()` is a NaN — which
    // `Rgb::lerp` now resolves to "no blend" rather than to black, but
    // resolving it here as well states the intent at the place the load
    // actually arrives from a division.
    if t.is_nan() {
        return color;
    }
    color.lerp(toward, t.clamp(0.0, 1.0).sqrt())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Summed absolute channel difference, widened so three u8 deltas
    /// cannot overflow the way `u8 + u8 + u8` does in a debug build.
    fn channel_distance(a: Rgb, b: Rgb) -> u32 {
        u32::from(a.r.abs_diff(b.r)) + u32::from(a.g.abs_diff(b.g)) + u32::from(a.b.abs_diff(b.b))
    }

    #[test]
    fn hex_parses_in_both_lengths_and_either_case() {
        assert_eq!(Rgb::parse("#1e1e1e"), Some(Rgb::new(30, 30, 30)));
        assert_eq!(Rgb::parse("1E1E1E"), Some(Rgb::new(30, 30, 30)));
        assert_eq!(
            Rgb::parse("#f0a"),
            Some(Rgb::new(255, 0, 170)),
            "the three-digit shorthand doubles each nibble, as CSS does"
        );
    }

    #[test]
    fn a_malformed_hex_code_is_rejected_rather_than_defaulted() {
        // A theme file with a typo should be reported, not silently
        // rendered in some fallback colour.
        for bad in [
            "",
            "#",
            "#12",
            "#12345",
            "#1234567",
            "#zzzzzz",
            "not a color",
        ] {
            assert_eq!(Rgb::parse(bad), None, "{bad:?} should not parse");
        }
    }

    #[test]
    fn hex_round_trips() {
        let color = Rgb::new(0x3c, 0x99, 0xe6);
        assert_eq!(color.to_hex(), "#3c99e6");
        assert_eq!(Rgb::parse(&color.to_hex()), Some(color));
    }

    #[test]
    fn contrast_matches_the_wcag_reference_points() {
        let black = Rgb::new(0, 0, 0);
        let white = Rgb::new(255, 255, 255);
        assert!(
            (black.contrast(white) - 21.0).abs() < 0.01,
            "black on white is the 21:1 reference, got {}",
            black.contrast(white)
        );
        assert!(
            (white.contrast(white) - 1.0).abs() < 0.001,
            "a colour against itself is 1:1"
        );
        assert!(
            (black.contrast(white) - white.contrast(black)).abs() < 0.001,
            "contrast is symmetric"
        );
    }

    #[test]
    fn text_colour_flips_across_the_luminance_crossover() {
        assert!(
            Rgb::new(20, 20, 24).prefers_light_text(),
            "a near-black chip needs light text"
        );
        assert!(
            !Rgb::new(240, 230, 120).prefers_light_text(),
            "a yellow chip needs dark text — this is the case a fixed \
             white label gets wrong"
        );
    }

    #[test]
    fn oklch_round_trips_within_a_rounding_step() {
        for original in [
            Rgb::new(0x4c, 0xc9, 0xf0),
            Rgb::new(0xf1, 0x4c, 0x4c),
            Rgb::new(0x89, 0xd1, 0x85),
            Rgb::new(0x18, 0x18, 0x18),
            Rgb::new(255, 255, 255),
            Rgb::new(0, 0, 0),
        ] {
            let round_tripped = Oklch::from_rgb(original).to_rgb();
            let delta = |a: u8, b: u8| u32::from(a).abs_diff(u32::from(b));
            assert!(
                delta(original.r, round_tripped.r) <= 1
                    && delta(original.g, round_tripped.g) <= 1
                    && delta(original.b, round_tripped.b) <= 1,
                "{original:?} round-tripped to {round_tripped:?}"
            );
        }
    }

    #[test]
    fn a_grey_survives_the_round_trip_without_gaining_a_hue() {
        let grey = Rgb::new(0x2a, 0x2a, 0x2a);
        let polar = Oklch::from_rgb(grey);
        assert!(
            polar.c < 0.005,
            "a neutral must have no chroma, got {}",
            polar.c
        );
        assert_eq!(polar.to_rgb(), grey, "a neutral must not pick up a tint");
    }

    #[test]
    fn an_out_of_gamut_colour_keeps_its_hue_and_lightness() {
        // The whole reason `to_rgb` walks the chroma down instead of
        // clipping channels. A chroma of 0.3 in blue is well outside
        // sRGB; clipping would shift the hue and raise the lightness,
        // breaking the one property the ramp exists for.
        let asked = Oklch::new(0.55, 0.30, 264.0);
        let got = Oklch::from_rgb(asked.to_rgb());

        assert!(
            (got.l - asked.l).abs() < 0.02,
            "lightness drifted from {} to {}",
            asked.l,
            got.l
        );
        let hue_delta = (got.h - asked.h).abs().min(360.0 - (got.h - asked.h).abs());
        assert!(
            hue_delta < 3.0,
            "hue drifted from {} to {} ({hue_delta} degrees)",
            asked.h,
            got.h
        );
        assert!(
            got.c < asked.c,
            "the colour should have given up chroma, not kept it"
        );
    }

    #[test]
    fn a_degenerate_oklch_still_produces_a_colour() {
        // Nothing here may divide by zero or emit a NaN into a channel.
        for (l, c, h) in [
            (0.0, 0.0, 0.0),
            (1.0, 0.0, 0.0),
            (0.5, 0.0, 720.0),
            (0.5, 5.0, -90.0),
            (0.5, 0.1, f32::NAN),
        ] {
            let rgb = Oklch::new(l, c, h).to_rgb();
            let _ = rgb;
        }
    }

    #[test]
    fn a_ramp_spreads_its_series_across_the_band() {
        let ramp = Ramp::around(Rgb::new(0x3c, 0x99, 0xe6), 200.0);
        let colors: Vec<Rgb> = (0..8).map(|i| ramp.at(i, 8)).collect();
        // Every series must be distinguishable from every other, or the
        // legend is decoration. 8 hues over 200 degrees is 28 apart.
        for (i, a) in colors.iter().enumerate() {
            for (j, b) in colors.iter().enumerate().skip(i + 1) {
                // Widened to u32: three u8 differences can sum past 255,
                // which overflows in a debug build.
                let separated = channel_distance(*a, *b);
                assert!(
                    separated > 20,
                    "series {i} ({a:?}) and {j} ({b:?}) are too close to tell apart"
                );
            }
        }
    }

    #[test]
    fn a_ramp_holds_one_perceived_lightness_so_no_series_shouts() {
        // The property the whole OKLCh detour buys. The HSL version of
        // this ramp measured a perceived-lightness spread of 0.10 to
        // 0.62 across sixteen series, which reads as the green core
        // being highlighted and the blue one being disabled.
        let ramp = Ramp::around(Rgb::new(0x4c, 0xc9, 0xf0), 280.0);
        let lightnesses: Vec<f32> = (0..16).map(|i| Oklch::from_rgb(ramp.at(i, 16)).l).collect();
        let lo = lightnesses.iter().copied().fold(f32::MAX, f32::min);
        let hi = lightnesses.iter().copied().fold(f32::MIN, f32::max);
        assert!(
            hi - lo < 0.02,
            "perceived lightness spread {lo}..{hi} — every series in a \
             chart must carry the same visual weight"
        );
        assert!(
            (ramp.lightness() - lo).abs() < 0.02,
            "the ramp should report the lightness it actually produces"
        );
    }

    #[test]
    fn a_ramp_stays_inside_the_srgb_gamut_all_the_way_round() {
        // A band that leaves the gamut comes back desaturated only where
        // it left — so the chart is vivid at one end and washed out at
        // the other. Capping the chroma up front is what prevents that.
        let ramp = Ramp::around(Rgb::new(0x4c, 0xc9, 0xf0), 280.0);
        let chromas: Vec<f32> = (0..16).map(|i| Oklch::from_rgb(ramp.at(i, 16)).c).collect();
        let lo = chromas.iter().copied().fold(f32::MAX, f32::min);
        let hi = chromas.iter().copied().fold(f32::MIN, f32::max);
        assert!(
            hi - lo < 0.02,
            "chroma spread {lo}..{hi} — the band is more saturated at one \
             end than the other"
        );
    }

    #[test]
    fn a_single_series_gets_the_accent_rather_than_a_division_by_zero() {
        let accent = Rgb::new(0x3c, 0x99, 0xe6);
        let ramp = Ramp::around(accent, 200.0);
        let one = ramp.at(0, 1);
        let none = ramp.at(0, 0);
        assert_eq!(one, none, "both degenerate counts take the same path");
        let hue_delta = (Oklch::from_rgb(one).h - Oklch::from_rgb(accent).h).abs();
        assert!(
            hue_delta < 1.0,
            "a lone series should be the theme's own accent, got a hue \
             {hue_delta} degrees away"
        );
    }

    #[test]
    fn an_index_past_the_end_clamps_rather_than_wrapping() {
        let ramp = Ramp::around(Rgb::new(0x3c, 0x99, 0xe6), 200.0);
        assert_eq!(
            ramp.at(99, 4),
            ramp.at(3, 4),
            "an out-of-range series should land on the last colour, not \
             wrap around to the first and collide with it"
        );
    }

    #[test]
    fn a_grey_accent_still_yields_a_usable_ramp() {
        // A theme is allowed a desaturated accent; it is not allowed an
        // invisible chart legend.
        let ramp = Ramp::around(Rgb::new(0x80, 0x80, 0x80), 200.0);
        let first = ramp.at(0, 6);
        let last = ramp.at(5, 6);
        assert!(
            channel_distance(first, last) > 40,
            "a grey accent should still produce separable series, got \
             {first:?} and {last:?}"
        );
    }

    #[test]
    fn a_stable_key_keeps_its_colour_regardless_of_position() {
        let ramp = Ramp::around(Rgb::new(0x3c, 0x99, 0xe6), 220.0);
        assert_eq!(
            ramp.for_key(4242),
            ramp.for_key(4242),
            "the same key must always give the same colour, or a process \
             changes colour when the table is resorted"
        );
    }

    #[test]
    fn adjacent_keys_do_not_get_adjacent_hues() {
        // A process and the three it just spawned have consecutive PIDs.
        // Using the key directly would give them near-identical colours,
        // which is the case the hash exists for.
        let ramp = Ramp::around(Rgb::new(0x3c, 0x99, 0xe6), 220.0);
        let a = ramp.for_key(5000);
        let b = ramp.for_key(5001);
        assert!(
            channel_distance(a, b) > 10,
            "consecutive keys collided: {a:?} and {b:?}"
        );
    }

    #[test]
    fn heat_appears_early_and_then_saturates() {
        let cold = Rgb::new(0x20, 0x20, 0x20);
        let hot = Rgb::new(0xff, 0x00, 0x00);
        let low = heat(cold, hot, 0.1);
        let mid = heat(cold, hot, 0.5);
        assert!(
            low.r > cold.r + 20,
            "a 10% load must be visible, not indistinguishable from idle — \
             got {low:?} against {cold:?}"
        );
        assert!(
            f32::from(mid.r) > f32::from(low.r) * 1.5,
            "the tint should still deepen with load"
        );
        assert_eq!(
            heat(cold, hot, 0.0),
            cold,
            "no load leaves the colour alone"
        );
        assert_eq!(heat(cold, hot, 1.0), hot, "full load reaches the target");
    }

    #[test]
    fn heat_clamps_a_load_that_overshoots() {
        // A process can be reported at over 100% of one core for a sample
        // when the elapsed time is estimated slightly short.
        let cold = Rgb::new(0x20, 0x20, 0x20);
        let hot = Rgb::new(0xff, 0x00, 0x00);
        assert_eq!(heat(cold, hot, 4.0), hot, "an overshoot must clamp");
        assert_eq!(heat(cold, hot, -1.0), cold, "so must an undershoot");
    }
}

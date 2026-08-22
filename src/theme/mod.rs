// ============================================================================
// Module:       theme
// Description:  The theme catalog: what a theme file states, what is derived
//               from it, and the readability rules a theme has to pass.
//
// Dependencies: serde + toml (the on-disk format), dirs (the user theme
//               directory), crate::color for the maths and the contrast check.
// ============================================================================

//! Themes, as data.
//!
//! A theme states **thirteen** colours and one number. Everything else a
//! window needs — the selection fill, the muted accent behind a chip, the
//! scrollbar handle, the grid line, the text colour that goes on top of
//! the accent, the whole rainbow series ramp — is derived from those in
//! [`Palette::derive`].
//!
//! That ratio is the entire design. A theme format where every colour is
//! stated is a format where a contributor adding a theme has to get forty
//! decisions right, and the fortieth — the colour of the scrollbar handle
//! against the card it scrolls — is one nobody thinks about until it is
//! invisible. Deriving them means a new theme is thirteen colours and
//! comes out internally consistent, and it means a fix to how selection
//! is derived fixes every theme at once instead of needing forty edits.
//!
//! ## Adding a theme
//!
//! Append a `[[theme]]` block to `assets/themes.toml`, or drop a `.toml`
//! file with the same shape into `%APPDATA%\rustaman\themes\`. The built-in
//! catalog is compiled in with `include_str!`; the user directory is
//! loaded on top of it at startup, so a theme can be added, and iterated
//! on, without rebuilding.
//!
//! `id` is what gets persisted in the config file. **Never rename one** —
//! a rename silently drops everyone using it back to the default.
//!
//! ## The readability rules are enforced
//!
//! `every_theme_is_readable` walks the whole built-in catalog and checks
//! each theme for layer separation and WCAG contrast. A theme that fails
//! is a failing build, not a subtly unreadable window. This is worth the
//! friction: a theme is submitted by someone looking at it on their own
//! monitor, and "the secondary text is fine on mine" is not a claim
//! anyone can check in review.

use crate::color::{Ramp, Rgb};
use serde::Deserialize;
use std::path::PathBuf;

/// The built-in catalog, compiled into the binary.
const BUILT_IN: &str = include_str!("../../assets/themes.toml");

/// Whether a theme's surfaces get lighter or darker as they come forward.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Dark theme: a surface reads as lifted by getting lighter.
    #[default]
    Dark,
    /// Light theme: a pale surface reads as lifted by getting *darker*,
    /// which is why this is a stated property rather than something
    /// [`Palette::derive`] guesses from the background's luminance.
    Light,
}

/// What a theme file states.
///
/// Thirteen colours and a number; see the module docs on why that is the
/// whole format.
#[derive(Clone, Debug, Deserialize)]
pub struct ThemeSpec {
    /// Stable identifier, persisted in the config. Never rename one.
    pub id: String,
    /// The name shown in the theme picker.
    pub name: String,
    /// Which way the surface ramp runs.
    #[serde(default)]
    pub mode: Mode,

    /// Behind everything. The window's own background.
    pub app: String,
    /// A panel sitting on the app background.
    pub panel: String,
    /// A card or row sitting on a panel.
    pub raised: String,
    /// The interactive lift above `raised`.
    pub hover: String,
    /// Rules and outlines.
    pub border: String,

    /// The theme's primary accent, and the centre of its rainbow.
    pub accent: String,
    /// Primary text.
    pub text: String,
    /// Secondary text: column headings, units, inactive labels.
    pub text_muted: String,

    /// Errors, and destructive actions.
    pub danger: String,
    /// Warnings, and the middle of the heat scale.
    pub warning: String,
    /// Success, and the cool end of the heat scale.
    pub success: String,
    /// Informational callouts. Optional: defaults to the accent, which is
    /// right for most themes and is the field most likely to be forgotten.
    #[serde(default)]
    pub info: Option<String>,
    /// The scrollbar handle, checkbox interior, and slider rail.
    ///
    /// Optional, and derived when absent — but statable, because it is
    /// the one derived colour a theme may genuinely need to override:
    /// see [`Palette::control`].
    #[serde(default)]
    pub control: Option<String>,

    /// How many degrees of hue the rainbow ramp spans, centred on the
    /// accent. Defaults to [`DEFAULT_RAMP_SPAN`].
    #[serde(default)]
    pub rainbow_span: Option<f32>,
}

/// The default width of a theme's rainbow band, in degrees.
///
/// Wide enough that sixteen cores get sixteen tellable-apart colours,
/// narrow enough that the band does not wrap all the way round into the
/// semantic red and green. See [`crate::color`].
pub const DEFAULT_RAMP_SPAN: f32 = 260.0;

/// A theme's full set of colours, derived and ready to draw with.
///
/// Every colour the app paints comes from here. See the "no literal
/// colours" rule in `CLAUDE.md`.
#[derive(Clone, Debug)]
pub struct Palette {
    /// This theme's stable id.
    pub id: String,
    /// This theme's display name.
    pub name: String,
    /// Dark or light.
    pub mode: Mode,

    /// The window background.
    pub app: Rgb,
    /// A panel on the app background.
    pub panel: Rgb,
    /// A card or row on a panel.
    pub raised: Rgb,
    /// The interactive lift above `raised`.
    pub hover: Rgb,
    /// Rules and outlines.
    pub border: Rgb,
    /// A heavier rule, for a focused or active edge.
    pub border_strong: Rgb,

    /// Primary text.
    pub text: Rgb,
    /// Secondary text.
    pub text_muted: Rgb,
    /// Tertiary text: watermarks, disabled labels, the empty-state line.
    pub text_faint: Rgb,
    /// Text that sits on top of [`Palette::accent`].
    ///
    /// Chosen for contrast rather than fixed, because an accent can be a
    /// pale cyan in one theme and a deep violet in another, and white
    /// text is unreadable on the first.
    pub text_on_accent: Rgb,

    /// The primary accent.
    pub accent: Rgb,
    /// The accent lifted, for a hovered accent surface.
    pub accent_hover: Rgb,
    /// The accent knocked back into the panel, for a fill that should
    /// read as tinted rather than as coloured.
    pub accent_soft: Rgb,

    /// The selected-row fill.
    pub selection: Rgb,
    /// Text on a selected row.
    pub selection_text: Rgb,

    /// The scrollbar handle, checkbox interior, and slider rail.
    ///
    /// Deliberately *not* one of the surface colours. egui paints
    /// buttons from `weak_bg_fill` and filled controls from `bg_fill`; a
    /// scrollbar handle in the same colour as the card it scrolls is
    /// invisible, so this is separated from the surfaces by construction
    /// and checked by a test. See `CLAUDE.md`.
    pub control: Rgb,
    /// The control colour, hovered.
    pub control_hover: Rgb,

    /// Errors and destructive actions.
    pub danger: Rgb,
    /// Warnings.
    pub warning: Rgb,
    /// Success.
    pub success: Rgb,
    /// Informational callouts.
    pub info: Rgb,

    /// Graph gridlines.
    pub grid: Rgb,

    /// The rainbow band this theme's multi-series charts are drawn from.
    ramp: Ramp,
}

impl Palette {
    /// Derives a full palette from what a theme file stated.
    ///
    /// Returns `None` if any stated colour fails to parse — a theme with
    /// a typo in it is reported as broken rather than rendered with a
    /// black hole where one colour should be. See
    /// [`Catalog::load`] for what happens to the `None`.
    #[must_use]
    pub fn derive(spec: &ThemeSpec) -> Option<Self> {
        let app = Rgb::parse(&spec.app)?;
        let panel = Rgb::parse(&spec.panel)?;
        let raised = Rgb::parse(&spec.raised)?;
        let hover = Rgb::parse(&spec.hover)?;
        let border = Rgb::parse(&spec.border)?;
        let accent = Rgb::parse(&spec.accent)?;
        let text = Rgb::parse(&spec.text)?;
        let text_muted = Rgb::parse(&spec.text_muted)?;
        let danger = Rgb::parse(&spec.danger)?;
        let warning = Rgb::parse(&spec.warning)?;
        let success = Rgb::parse(&spec.success)?;
        let info = match &spec.info {
            Some(text) => Rgb::parse(text)?,
            None => accent,
        };

        // "Forward" means towards the viewer, which is lighter in a dark
        // theme and darker in a light one. Every derivation below is
        // written in terms of this rather than of black and white, which
        // is what lets one set of rules serve both modes.
        let forward = match spec.mode {
            Mode::Dark => Rgb::new(255, 255, 255),
            Mode::Light => Rgb::new(0, 0, 0),
        };

        let control = match &spec.control {
            Some(text) => Rgb::parse(text)?,
            // Derived by pulling the lifted surface a third of the way
            // towards the text colour. Towards *text* rather than towards
            // `forward`: in a light theme, forward is black, and a black
            // scrollbar handle on a white card is far heavier than the
            // rest of the chrome. Text is the theme's own idea of "as far
            // from the background as things go".
            None => hover.lerp(text, 0.34),
        };

        Some(Self {
            id: spec.id.clone(),
            name: spec.name.clone(),
            mode: spec.mode,
            app,
            panel,
            raised,
            hover,
            border,
            border_strong: border.lerp(text, 0.28),
            text,
            text_muted,
            // Faint text is pulled back towards the panel rather than
            // made more transparent, so it composites identically
            // whatever surface it lands on.
            text_faint: text_muted.lerp(panel, 0.42),
            text_on_accent: if accent.prefers_light_text() {
                Rgb::new(255, 255, 255)
            } else {
                Rgb::new(16, 16, 20)
            },
            accent,
            accent_hover: accent.lerp(forward, 0.18),
            accent_soft: accent.lerp(panel, 0.74),
            // Selection is a *tint*, not the accent itself. Two reasons,
            // and the second is the one that fixes the number.
            //
            // A fully-saturated selected row makes a table with several
            // rows selected look like an error state. And a selected row
            // still shows its muted columns — the units, the user, the
            // greyed-out counters — so the fill has to leave secondary
            // text clearing WCAG AA on top of it. At a heavier blend it
            // does not: the first draft of this used 38% accent and every
            // theme in the catalog failed that check by around 1.5:1.
            //
            // The trade is that a tint this light is not, by itself,
            // unmistakable. That is why a selected row also gets a solid
            // accent bar down its leading edge (`gui::ui::widgets`) —
            // which reads as selection at a glance from across the
            // window, and costs the text nothing.
            selection: accent.lerp(panel, 0.80),
            selection_text: text,
            control,
            control_hover: control.lerp(text, 0.28),
            danger,
            warning,
            success,
            info,
            // Gridlines sit between the border and the panel: visible
            // enough to read a value off, faint enough not to compete
            // with the series drawn over them.
            grid: border.lerp(panel, 0.45),
            ramp: Ramp::around(accent, spec.rainbow_span.unwrap_or(DEFAULT_RAMP_SPAN)),
        })
    }

    /// The colour for series `index` of `count` in a multi-series chart.
    #[must_use]
    pub fn series(&self, index: usize, count: usize) -> Rgb {
        self.ramp.at(index, count)
    }

    /// The colour for a stable identity — a process, an adapter — that
    /// must not change when the list is resorted.
    #[must_use]
    pub fn series_for(&self, key: u64) -> Rgb {
        self.ramp.for_key(key)
    }

    /// The heat colour for a load fraction, 0..=1.
    ///
    /// Success through warning to danger, which is the one colour scale
    /// every reader already knows. Piecewise rather than a single
    /// interpolation so the midpoint really is the theme's warning
    /// colour: blending green directly to red passes through a muddy
    /// brown that reads as neither.
    #[must_use]
    pub fn heat(&self, load: f32) -> Rgb {
        // `f32::clamp` propagates a NaN rather than resolving it, and
        // `NaN < 0.5` is false — so an unguarded NaN takes the *hot*
        // branch and paints a cell amber for a load that could not be
        // measured. Load arrives here as a rate divided by an interval,
        // so this is a real path.
        if load.is_nan() {
            return self.success;
        }
        let load = load.clamp(0.0, 1.0);
        if load < 0.5 {
            self.success.lerp(self.warning, load * 2.0)
        } else {
            self.warning.lerp(self.danger, (load - 0.5) * 2.0)
        }
    }

    /// The four surfaces, back to front. Used by the layer-separation
    /// check and by the settings page's theme preview.
    #[must_use]
    pub fn surfaces(&self) -> [Rgb; 4] {
        [self.app, self.panel, self.raised, self.hover]
    }
}

/// Every theme available to the app.
#[derive(Clone, Debug)]
pub struct Catalog {
    /// The themes, in catalog order. Never empty — see [`Catalog::load`].
    themes: Vec<Palette>,
    /// Theme files that were found but could not be used, as
    /// `(source, reason)`. Surfaced in the settings page rather than
    /// swallowed: a theme that silently does not appear is
    /// indistinguishable from one the app cannot see.
    problems: Vec<(String, String)>,
}

/// The `[[theme]]` array a catalog file holds.
#[derive(Deserialize)]
struct CatalogFile {
    /// One entry per theme.
    #[serde(default)]
    theme: Vec<ThemeSpec>,
}

impl Catalog {
    /// Loads the built-in catalog, then any user themes on top of it.
    ///
    /// A user theme whose `id` matches a built-in one **replaces** it,
    /// rather than being added beside it. That is what makes the user
    /// directory useful for tweaking a shipped theme — copy it, change
    /// two colours, keep the id — and it avoids a picker with two
    /// identically named entries and no way to tell which is which.
    #[must_use]
    pub fn load() -> Self {
        let mut catalog = Self {
            themes: Vec::new(),
            problems: Vec::new(),
        };
        catalog.absorb("built-in", BUILT_IN);

        // A malformed *built-in* catalog is a build error caught by the
        // tests below, so reaching here with nothing loaded means the
        // binary was tampered with. Carry on to the user directory and
        // let the fallback at the end handle it.
        if let Some(dir) = user_theme_dir() {
            let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
                .into_iter()
                .flatten()
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| path.extension().is_some_and(|ext| ext == "toml"))
                .collect();
            // Sorted so a directory listing's arbitrary order cannot make
            // the catalog differ between runs.
            files.sort();
            for path in files {
                let label = path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.to_string_lossy().into_owned());
                match std::fs::read_to_string(&path) {
                    Ok(text) => catalog.absorb(&label, &text),
                    Err(error) => catalog.problems.push((label, error.to_string())),
                }
            }
        }

        if catalog.themes.is_empty() {
            catalog.themes.push(Self::emergency());
        }
        catalog
    }

    /// Parses one catalog file and merges its themes in.
    fn absorb(&mut self, source: &str, text: &str) {
        let parsed: CatalogFile = match toml::from_str(text) {
            Ok(parsed) => parsed,
            Err(error) => {
                self.problems.push((source.to_string(), error.to_string()));
                return;
            }
        };
        for spec in parsed.theme {
            let id = spec.id.clone();
            let Some(palette) = Palette::derive(&spec) else {
                self.problems.push((
                    format!("{source}: {id}"),
                    "one of its colours is not a valid hex code".to_string(),
                ));
                continue;
            };
            match self.themes.iter().position(|theme| theme.id == id) {
                Some(existing) => {
                    if let Some(slot) = self.themes.get_mut(existing) {
                        *slot = palette;
                    }
                }
                None => self.themes.push(palette),
            }
        }
    }

    /// Every loaded theme, in catalog order.
    #[must_use]
    pub fn themes(&self) -> &[Palette] {
        &self.themes
    }

    /// Theme sources that could not be loaded, as `(source, reason)`.
    #[must_use]
    pub fn problems(&self) -> &[(String, String)] {
        &self.problems
    }

    /// The theme with this id, or the default when there is no such id.
    ///
    /// Falling back rather than failing is deliberate: themes come and
    /// go, and a preference naming one that has been removed should cost
    /// the theme, not make the config unreadable.
    #[must_use]
    pub fn get(&self, id: Option<&str>) -> &Palette {
        id.and_then(|id| self.themes.iter().find(|theme| theme.id == id))
            .or_else(|| self.themes.first())
            // `themes` is never empty (see `load`), so this is
            // unreachable; returning the leaked default keeps the
            // function total without an `unwrap`.
            .unwrap_or_else(|| default_palette())
    }

    /// The palette used when the catalog could not be loaded at all.
    ///
    /// Not a nice theme, and it is not meant to be: it exists so that a
    /// tampered-with binary opens a readable window saying so, rather
    /// than a black rectangle.
    fn emergency() -> Palette {
        default_palette().clone()
    }
}

/// The last-resort palette, built in code rather than parsed.
///
/// A `OnceLock` rather than a `const` because [`Palette`] holds `String`s
/// and a derived [`Ramp`], neither of which can be constructed in a
/// constant. Built at most once, and on almost every run not at all.
fn default_palette() -> &'static Palette {
    use std::sync::OnceLock;
    static FALLBACK: OnceLock<Palette> = OnceLock::new();
    FALLBACK.get_or_init(|| {
        let spec = ThemeSpec {
            id: "fallback".to_string(),
            name: "Fallback".to_string(),
            mode: Mode::Dark,
            app: "#0b0d12".to_string(),
            panel: "#12151d".to_string(),
            raised: "#1b1f2a".to_string(),
            hover: "#252a38".to_string(),
            border: "#333a4d".to_string(),
            accent: "#4cc9f0".to_string(),
            text: "#e8ecf4".to_string(),
            text_muted: "#9aa5bc".to_string(),
            danger: "#ff5c7a".to_string(),
            warning: "#ffb347".to_string(),
            success: "#4ade80".to_string(),
            info: None,
            control: None,
            rainbow_span: None,
        };
        // Every colour above is a literal this function controls, so the
        // derivation cannot fail; the fallback keeps it total anyway.
        Palette::derive(&spec).unwrap_or_else(|| Palette {
            id: "fallback".to_string(),
            name: "Fallback".to_string(),
            mode: Mode::Dark,
            app: Rgb::new(11, 13, 18),
            panel: Rgb::new(18, 21, 29),
            raised: Rgb::new(27, 31, 42),
            hover: Rgb::new(37, 42, 56),
            border: Rgb::new(51, 58, 77),
            border_strong: Rgb::new(90, 98, 120),
            text: Rgb::new(232, 236, 244),
            text_muted: Rgb::new(154, 165, 188),
            text_faint: Rgb::new(110, 120, 140),
            text_on_accent: Rgb::new(16, 16, 20),
            accent: Rgb::new(76, 201, 240),
            accent_hover: Rgb::new(110, 214, 244),
            accent_soft: Rgb::new(40, 68, 86),
            selection: Rgb::new(40, 89, 112),
            selection_text: Rgb::new(232, 236, 244),
            control: Rgb::new(90, 99, 120),
            control_hover: Rgb::new(130, 140, 160),
            danger: Rgb::new(255, 92, 122),
            warning: Rgb::new(255, 179, 71),
            success: Rgb::new(74, 222, 128),
            info: Rgb::new(76, 201, 240),
            grid: Rgb::new(38, 44, 60),
            ramp: Ramp::around(Rgb::new(76, 201, 240), DEFAULT_RAMP_SPAN),
        })
    })
}

/// Where user theme files live: `%APPDATA%\rustaman\themes\`.
#[must_use]
pub fn user_theme_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("rustaman").join("themes"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::{anyhow, Result};

    /// The contrast a body of text must clear against the surface it sits
    /// on. WCAG AAA for normal-size text — stricter than the AA most
    /// design systems settle for, because this app's densest screen is a
    /// table of four-hundred rows of 12px text and that is exactly the
    /// case AA is too loose for.
    const TEXT_CONTRAST: f32 = 7.0;

    /// The contrast secondary text and coloured indicators must clear.
    /// WCAG AA, which is the right bar for text that is deliberately
    /// de-emphasised and for a colour whose job is to be noticed rather
    /// than read.
    const SECONDARY_CONTRAST: f32 = 4.5;

    /// The least two adjacent surfaces may differ and still be tellable
    /// apart. Expressed as a contrast ratio, so it means the same thing
    /// at both ends of the lightness range — an absolute channel
    /// difference does not, which is how a dark theme's `app` and `panel`
    /// can differ by eight units and be clearly distinct while a light
    /// theme's differ by eight and are not.
    const LAYER_SEPARATION: f32 = 1.06;

    fn catalog() -> Catalog {
        // The built-in catalog only. A developer's own user themes must
        // not be able to make the build fail — or, worse, pass.
        let mut catalog = Catalog {
            themes: Vec::new(),
            problems: Vec::new(),
        };
        catalog.absorb("built-in", BUILT_IN);
        catalog
    }

    #[test]
    fn the_built_in_catalog_parses() {
        let catalog = catalog();
        assert!(
            catalog.problems().is_empty(),
            "assets/themes.toml did not load cleanly: {:?}",
            catalog.problems()
        );
        assert!(
            catalog.themes().len() >= 4,
            "the catalog should ship more than a token theme, found {}",
            catalog.themes().len()
        );
    }

    #[test]
    fn theme_ids_are_unique() {
        // A duplicate id would silently replace the earlier theme, which
        // looks like a theme simply going missing.
        let catalog = catalog();
        let mut ids: Vec<&str> = catalog.themes().iter().map(|t| t.id.as_str()).collect();
        let count = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), count, "two themes share an id");
    }

    #[test]
    fn every_theme_is_readable() {
        // The check that makes an unreadable theme a failing build. See
        // the module docs.
        for theme in catalog().themes() {
            let name = &theme.name;

            // Primary and secondary text, against every surface either
            // can land on. Checking only against `panel` would miss a
            // theme whose `hover` is much lighter than its panel, where
            // the text goes unreadable exactly when a row is hovered.
            for (label, surface) in [
                ("app", theme.app),
                ("panel", theme.panel),
                ("raised", theme.raised),
                ("hover", theme.hover),
                ("selection", theme.selection),
            ] {
                let primary = theme.text.contrast(surface);
                assert!(
                    primary >= TEXT_CONTRAST,
                    "{name}: primary text on {label} is {primary:.2}:1, \
                     below the {TEXT_CONTRAST}:1 this app's 12px tables need"
                );
                let secondary = theme.text_muted.contrast(surface);
                assert!(
                    secondary >= SECONDARY_CONTRAST,
                    "{name}: secondary text on {label} is {secondary:.2}:1, \
                     below WCAG AA's {SECONDARY_CONTRAST}:1"
                );
            }

            // The accent and the status colours have to be *noticeable*
            // against the panel, not readable as body text.
            for (label, color) in [
                ("accent", theme.accent),
                ("danger", theme.danger),
                ("warning", theme.warning),
                ("success", theme.success),
                ("info", theme.info),
            ] {
                let ratio = color.contrast(theme.panel);
                assert!(
                    ratio >= 3.0,
                    "{name}: {label} is {ratio:.2}:1 against the panel, \
                     which is not enough to be seen"
                );
            }

            // Text placed *on* the accent — a primary button's label.
            let on_accent = theme.text_on_accent.contrast(theme.accent);
            assert!(
                on_accent >= SECONDARY_CONTRAST,
                "{name}: text on the accent is {on_accent:.2}:1 — this is \
                 the case a hard-coded white label gets wrong on a pale \
                 accent"
            );

            // Text on a selected row.
            let on_selection = theme.selection_text.contrast(theme.selection);
            assert!(
                on_selection >= SECONDARY_CONTRAST,
                "{name}: text on a selected row is {on_selection:.2}:1"
            );
        }
    }

    #[test]
    fn every_themes_surfaces_are_tellable_apart() {
        for theme in catalog().themes() {
            let surfaces = theme.surfaces();
            let names = ["app", "panel", "raised", "hover"];
            for index in 0..surfaces.len() - 1 {
                let (Some(back), Some(front)) = (surfaces.get(index), surfaces.get(index + 1))
                else {
                    continue;
                };
                let ratio = back.contrast(*front);
                assert!(
                    ratio >= LAYER_SEPARATION,
                    "{}: {} and {} differ by only {ratio:.3}:1, so a card \
                     on a panel has no visible edge",
                    theme.name,
                    names[index],
                    names[index + 1]
                );
            }
        }
    }

    #[test]
    fn a_surface_ramp_runs_the_direction_its_mode_claims() {
        // A dark theme whose `hover` is darker than its `raised` reads as
        // a control being pressed in when it is merely hovered.
        for theme in catalog().themes() {
            let app = theme.app.luminance();
            let hover = theme.hover.luminance();
            match theme.mode {
                Mode::Dark => assert!(
                    hover > app,
                    "{}: a dark theme's surfaces must get lighter as they \
                     come forward",
                    theme.name
                ),
                Mode::Light => assert!(
                    hover < app,
                    "{}: a light theme's surfaces must get darker as they \
                     come forward — a pale surface reads as lifted by \
                     getting darker, not lighter",
                    theme.name
                ),
            }
        }
    }

    #[test]
    fn a_scrollbar_handle_is_never_the_colour_of_what_it_scrolls() {
        // The bug this exists for: pointing egui's `bg_fill` at a surface
        // colour makes every scrollbar in the app invisible. See
        // CLAUDE.md.
        for theme in catalog().themes() {
            for (label, surface) in [
                ("app", theme.app),
                ("panel", theme.panel),
                ("raised", theme.raised),
                ("hover", theme.hover),
            ] {
                let ratio = theme.control.contrast(surface);
                assert!(
                    ratio >= 1.25,
                    "{}: the control colour is {ratio:.3}:1 against {label}, \
                     so a scrollbar over it would be invisible",
                    theme.name
                );
            }
        }
    }

    #[test]
    fn every_theme_yields_a_usable_series_ramp() {
        for theme in catalog().themes() {
            // The realistic worst case is a modern many-core desktop.
            let count = 16;
            let colors: Vec<Rgb> = (0..count).map(|i| theme.series(i, count)).collect();
            for (index, color) in colors.iter().enumerate() {
                let ratio = color.contrast(theme.panel);
                assert!(
                    ratio >= 2.0,
                    "{}: series {index} is {ratio:.2}:1 against the panel \
                     and would be invisible in the chart",
                    theme.name
                );
            }
            // Neighbouring series must be separable, or a sixteen-core
            // graph is a single smear.
            for pair in colors.windows(2) {
                let (Some(a), Some(b)) = (pair.first(), pair.last()) else {
                    continue;
                };
                let delta = a.r.abs_diff(b.r) + a.g.abs_diff(b.g) + a.b.abs_diff(b.b);
                assert!(
                    delta >= 8,
                    "{}: adjacent series {a:?} and {b:?} are indistinguishable",
                    theme.name
                );
            }
        }
    }

    #[test]
    fn the_heat_scale_passes_through_the_themes_own_warning_colour() {
        // Blending success straight to danger passes through a muddy
        // brown; the piecewise scale is what avoids it.
        for theme in catalog().themes() {
            assert_eq!(theme.heat(0.0), theme.success, "{}: cold", theme.name);
            assert_eq!(theme.heat(0.5), theme.warning, "{}: midpoint", theme.name);
            assert_eq!(theme.heat(1.0), theme.danger, "{}: hot", theme.name);
        }
    }

    #[test]
    fn the_heat_scale_clamps_an_overshooting_load() {
        let theme = default_palette();
        assert_eq!(theme.heat(9.0), theme.danger);
        assert_eq!(theme.heat(-1.0), theme.success);
        assert_eq!(
            theme.heat(f32::NAN),
            theme.success,
            "a NaN must not paint black"
        );
    }

    #[test]
    fn a_theme_with_a_bad_colour_is_reported_rather_than_rendered() {
        let mut catalog = Catalog {
            themes: Vec::new(),
            problems: Vec::new(),
        };
        catalog.absorb(
            "test",
            r##"
            [[theme]]
            id = "broken"
            name = "Broken"
            app = "#zzzzzz"
            panel = "#12151d"
            raised = "#1b1f2a"
            hover = "#252a38"
            border = "#333a4d"
            accent = "#4cc9f0"
            text = "#e8ecf4"
            text_muted = "#9aa5bc"
            danger = "#ff5c7a"
            warning = "#ffb347"
            success = "#4ade80"
            "##,
        );
        assert!(catalog.themes().is_empty(), "a broken theme must not load");
        assert_eq!(catalog.problems().len(), 1, "and must be reported");
    }

    #[test]
    fn a_user_theme_replaces_the_built_in_it_shares_an_id_with() -> Result<()> {
        // What makes the user directory useful for tweaking a shipped
        // theme, and what stops the picker showing two identically named
        // entries.
        let mut catalog = catalog();
        let before = catalog.themes().len();
        let existing = catalog
            .themes()
            .first()
            .map(|theme| theme.id.clone())
            .ok_or_else(|| anyhow!("the built-in catalog is empty"))?;
        catalog.absorb(
            "user.toml",
            &format!(
                r##"
                [[theme]]
                id = "{existing}"
                name = "Mine"
                app = "#000000"
                panel = "#101010"
                raised = "#1c1c1c"
                hover = "#2a2a2a"
                border = "#3a3a3a"
                accent = "#4cc9f0"
                text = "#ffffff"
                text_muted = "#a0a0a0"
                danger = "#ff5c7a"
                warning = "#ffb347"
                success = "#4ade80"
                "##
            ),
        );
        assert_eq!(
            catalog.themes().len(),
            before,
            "a matching id replaces rather than appends"
        );
        assert_eq!(
            catalog.get(Some(&existing)).name,
            "Mine",
            "and the replacement is what the id now resolves to"
        );
        Ok(())
    }

    #[test]
    fn an_unknown_theme_id_falls_back_rather_than_failing() {
        // Themes come and go; a preference naming a removed one should
        // cost the theme, not make the config unreadable.
        let catalog = catalog();
        let fallback = catalog.get(Some("no-such-theme"));
        let default = catalog.get(None);
        assert_eq!(
            fallback.id, default.id,
            "an unknown id resolves to the first theme in the catalog"
        );
    }

    #[test]
    fn the_optional_fields_default_the_way_the_docs_claim() -> Result<()> {
        let mut catalog = Catalog {
            themes: Vec::new(),
            problems: Vec::new(),
        };
        catalog.absorb(
            "test",
            r##"
            [[theme]]
            id = "minimal"
            name = "Minimal"
            app = "#0b0d12"
            panel = "#12151d"
            raised = "#1b1f2a"
            hover = "#252a38"
            border = "#333a4d"
            accent = "#4cc9f0"
            text = "#e8ecf4"
            text_muted = "#9aa5bc"
            danger = "#ff5c7a"
            warning = "#ffb347"
            success = "#4ade80"
            "##,
        );
        let theme = catalog.themes().first().ok_or_else(|| {
            anyhow!(
                "the minimal theme should have loaded: {:?}",
                catalog.problems()
            )
        })?;
        assert_eq!(theme.mode, Mode::Dark, "mode defaults to dark");
        assert_eq!(theme.info, theme.accent, "info defaults to the accent");
        assert_ne!(
            theme.control, theme.raised,
            "a derived control colour must not land on a surface"
        );
        Ok(())
    }

    #[test]
    fn the_fallback_palette_is_itself_readable() {
        // It only ever appears when everything else has failed, which is
        // precisely when a readable window matters most.
        let theme = default_palette();
        assert!(theme.text.contrast(theme.panel) >= TEXT_CONTRAST);
        assert!(theme.text_muted.contrast(theme.panel) >= SECONDARY_CONTRAST);
        assert!(theme.control.contrast(theme.panel) >= 1.25);
    }
}

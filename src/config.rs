// ============================================================================
// Module:       config
// Description:  Persisted preferences, loaded once at startup and saved on a
//               normal quit, parsed forgivingly field by field.
//
// Dependencies: serde + toml (the on-disk format), dirs (the location);
//               crate::model::sort::SortKey
// ============================================================================

//! The preferences that survive a restart.
//!
//! Which theme, which view was open, how often to sample, how the table
//! was sorted, where the window was. All of it is convenience state: a
//! missing, partial, or unreadable config file means "use the defaults",
//! never a hard failure. Nothing here can hide a problem the user needs
//! to know about, which is what separates it from, say, a scan error.
//!
//! ## Parsed one field at a time
//!
//! [`parse`] pulls each field out of a `toml::Table` individually rather
//! than deserializing the whole struct in one shot. The difference shows
//! up the first time a field's type changes between releases, or someone
//! hand-edits the file and mistypes a value: whole-struct
//! deserialization fails on the first bad field and returns `Err`, and
//! the caller — having nothing else to do with it — falls back to
//! `Default::default()`. So one mistyped sort name silently throws away
//! the theme, the window position, the interval, and every other
//! preference along with it.
//!
//! Per-field, a bad value costs that one preference. The user loses their
//! sort order and keeps everything else, and the next save writes a valid
//! file again.
//!
//! A file that is not TOML at all still yields the defaults. There is
//! nothing in it to salvage.

use crate::model::sort::SortKey;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Everything the app remembers between runs.
///
/// Every field is `Option`, and `None` means "the app decides". That is
/// not the same as a stated default: it lets a default change in a later
/// release and reach users who never touched the setting, while still
/// respecting the choice of anyone who did.
#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq)]
pub struct Config {
    /// A theme `id` from the catalog. An id that no longer exists falls
    /// back to the default rather than failing to load.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theme: Option<String>,
    /// Which view was open, by its stable name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view: Option<String>,
    /// How often to sample, in milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interval_ms: Option<u64>,
    /// The process table's sort column.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort: Option<SortKey>,
    /// Whether that sort is descending.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort_descending: Option<bool>,
    /// Whether the process list groups into a tree with category
    /// headings, or shows one flat list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grouped: Option<bool>,
    /// Whether the window draws its own title bar. See
    /// [`crate::gui`] — on Windows 10 this is the difference between a
    /// modern-looking window and the system's grey caption.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_chrome: Option<bool>,
    /// Whether the window stays above others.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub always_on_top: Option<bool>,
    /// Whether ending a task asks first.
    ///
    /// Defaults to on. Turning it off is a real preference — someone
    /// killing a crashed process forty times in a debugging session
    /// means it — but it is off *by choice*, and never by default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirm_end_task: Option<bool>,
    /// The window's size in logical points, as `[width, height]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_size: Option<[f32; 2]>,
    /// Which optional columns are shown, by their stable names.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub columns: Option<Vec<String>>,
}

impl Config {
    /// The sampling interval, clamped to something a machine survives.
    ///
    /// A config file is a text file, and a hand-edited `interval_ms = 1`
    /// would have the sampler enumerate every process a thousand times a
    /// second — which costs more CPU than everything it is measuring and
    /// makes the app the busiest process in its own list. The floor is
    /// [`MIN_INTERVAL_MS`]; the ceiling stops a typo'd `interval_ms =
    /// 100000000` from looking like the app has frozen.
    #[must_use]
    pub fn interval(&self) -> std::time::Duration {
        let millis = self
            .interval_ms
            .unwrap_or(DEFAULT_INTERVAL_MS)
            .clamp(MIN_INTERVAL_MS, MAX_INTERVAL_MS);
        std::time::Duration::from_millis(millis)
    }
}

/// The default sampling interval: one sample a second, as every task
/// manager has done since NT 4.
pub const DEFAULT_INTERVAL_MS: u64 = 1_000;

/// The fastest the sampler may be asked to run.
///
/// A full process enumeration plus the counter reads costs a few
/// milliseconds on a normal machine; four times a second is responsive
/// without the app registering in its own CPU column, which is the line
/// this floor is drawn at.
pub const MIN_INTERVAL_MS: u64 = 250;

/// The slowest the sampler may be asked to run: one minute.
pub const MAX_INTERVAL_MS: u64 = 60_000;

/// The intervals the settings page offers.
pub const INTERVAL_CHOICES: [u64; 6] = [250, 500, 1_000, 2_000, 5_000, 10_000];

/// Where the config file lives: `%APPDATA%\rustaman\config.toml`.
#[must_use]
pub fn config_path() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("rustaman").join("config.toml"))
}

/// Loads the config, or the defaults if there is nothing readable.
#[must_use]
pub fn load() -> Config {
    config_path()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .map(|text| parse(&text))
        .unwrap_or_default()
}

/// Parses a config, forgiving per field.
///
/// See the module docs on why this is not one `toml::from_str`.
#[must_use]
pub fn parse(text: &str) -> Config {
    let Ok(table) = text.parse::<toml::Table>() else {
        return Config::default();
    };
    /// Reads one field, yielding `None` for anything that does not
    /// deserialize rather than failing the whole parse.
    fn field<T: serde::de::DeserializeOwned>(table: &toml::Table, key: &str) -> Option<T> {
        table
            .get(key)
            .cloned()
            .and_then(|value| value.try_into().ok())
    }
    Config {
        theme: field(&table, "theme"),
        view: field(&table, "view"),
        interval_ms: field(&table, "interval_ms"),
        sort: field(&table, "sort"),
        sort_descending: field(&table, "sort_descending"),
        grouped: field(&table, "grouped"),
        custom_chrome: field(&table, "custom_chrome"),
        always_on_top: field(&table, "always_on_top"),
        confirm_end_task: field(&table, "confirm_end_task"),
        window_size: field(&table, "window_size"),
        columns: field(&table, "columns"),
    }
}

/// Writes the config to the platform location, atomically.
///
/// Written to a temporary file beside the target and then renamed, so a
/// crash or a power cut during the write leaves the previous config
/// intact rather than a half-written one that parses to nothing. The
/// rename is the only step that can be observed, and it either happened
/// or it did not.
///
/// Failures are returned rather than swallowed. The app saves on exit,
/// and "your preferences were not saved" is the caller's to surface — a
/// `()` return would make saying so impossible, and every failed save
/// would look identical to a successful one.
pub fn save(config: &Config) -> std::io::Result<()> {
    let Some(path) = config_path() else {
        return Err(std::io::Error::other(
            "no configuration directory could be determined",
        ));
    };
    let Some(parent) = path.parent() else {
        return Err(std::io::Error::other(format!(
            "{} has no parent directory",
            path.display()
        )));
    };
    std::fs::create_dir_all(parent)?;

    let text = toml::to_string_pretty(config).map_err(std::io::Error::other)?;
    let temporary = path.with_extension("toml.tmp");
    std::fs::write(&temporary, text)?;
    // `rename` over an existing file is atomic on Windows for a
    // same-volume move, which this is by construction.
    match std::fs::rename(&temporary, &path) {
        Ok(()) => Ok(()),
        Err(error) => {
            // Do not leave the temporary behind to be mistaken for a
            // config later; the rename failing is already the error
            // being reported.
            let _ = std::fs::remove_file(&temporary);
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Result;

    #[test]
    fn an_absent_config_yields_the_defaults() {
        let config = parse("");
        assert_eq!(config, Config::default());
        assert_eq!(config.interval().as_millis() as u64, DEFAULT_INTERVAL_MS);
    }

    #[test]
    fn a_file_that_is_not_toml_at_all_yields_the_defaults() {
        assert_eq!(parse("}{ not toml <<<"), Config::default());
        assert_eq!(parse("\u{0}\u{1}\u{2}"), Config::default());
    }

    #[test]
    fn one_bad_field_costs_only_that_field() {
        // The whole reason for the per-field parse. `sort` is a string
        // that is not a valid variant, and `interval_ms` is a string
        // where a number belongs — neither may take the theme down with
        // them.
        let config = parse(
            r#"
            theme = "nebula"
            sort = "not-a-column"
            interval_ms = "fast"
            always_on_top = true
            "#,
        );
        assert_eq!(
            config.theme.as_deref(),
            Some("nebula"),
            "a valid field before the bad ones must survive"
        );
        assert_eq!(
            config.always_on_top,
            Some(true),
            "a valid field after the bad ones must survive too"
        );
        assert_eq!(config.sort, None, "the bad field itself is dropped");
        assert_eq!(config.interval_ms, None);
    }

    #[test]
    fn a_config_round_trips() -> Result<()> {
        let original = Config {
            theme: Some("midnight".to_string()),
            view: Some("performance".to_string()),
            interval_ms: Some(2_000),
            sort: Some(SortKey::Memory),
            sort_descending: Some(true),
            grouped: Some(false),
            custom_chrome: Some(true),
            always_on_top: Some(false),
            confirm_end_task: Some(true),
            window_size: Some([1440.0, 900.0]),
            columns: Some(vec!["cpu".to_string(), "memory".to_string()]),
        };
        let text = toml::to_string_pretty(&original)?;
        assert_eq!(
            parse(&text),
            original,
            "what was written must read back identically"
        );
        Ok(())
    }

    #[test]
    fn an_unset_field_is_not_written_out() -> Result<()> {
        // `skip_serializing_if` keeps the file to what the user actually
        // chose, so a default that changes in a later release reaches
        // anyone who never touched that setting.
        let text = toml::to_string_pretty(&Config::default())?;
        assert!(
            text.trim().is_empty(),
            "an untouched config should write nothing, got {text:?}"
        );
        Ok(())
    }

    #[test]
    fn a_hand_edited_interval_cannot_pin_the_machine() {
        // A config file is a text file. `interval_ms = 1` would have the
        // sampler enumerate every process a thousand times a second.
        let fast = Config {
            interval_ms: Some(1),
            ..Config::default()
        };
        assert_eq!(
            fast.interval().as_millis() as u64,
            MIN_INTERVAL_MS,
            "an absurdly fast interval must be clamped, not honoured"
        );

        let slow = Config {
            interval_ms: Some(100_000_000),
            ..Config::default()
        };
        assert_eq!(
            slow.interval().as_millis() as u64,
            MAX_INTERVAL_MS,
            "an absurdly slow one must be clamped too, or the app looks \
             frozen"
        );

        let zero = Config {
            interval_ms: Some(0),
            ..Config::default()
        };
        assert_eq!(zero.interval().as_millis() as u64, MIN_INTERVAL_MS);
    }

    #[test]
    fn every_offered_interval_survives_the_clamp() {
        // A choice in the settings page that the clamp then changes would
        // be a control that does not do what it says.
        for millis in INTERVAL_CHOICES {
            let config = Config {
                interval_ms: Some(millis),
                ..Config::default()
            };
            assert_eq!(
                config.interval().as_millis() as u64,
                millis,
                "the settings page offers {millis}ms, which the clamp then \
                 changes"
            );
        }
    }

    #[test]
    fn the_default_interval_is_one_of_the_offered_choices() {
        assert!(
            INTERVAL_CHOICES.contains(&DEFAULT_INTERVAL_MS),
            "the default must be selectable in the UI, or the control opens \
             showing none of its own options chosen"
        );
    }
}

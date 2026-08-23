// ============================================================================
// Module:       win::startup
// Description:  Programs registered to run at logon — native and 32-bit
//               Run/RunOnce views plus both Startup folders and enabled state.
//
// Dependencies: windows-sys (registry); super::handle::OwnedKey, super::strings
// ============================================================================

//! What runs at logon.
//!
//! ## Supported locations
//!
//! This covers the canonical per-user and machine Run/RunOnce keys in
//! their documented registry views, plus both Startup folders.
//!
//! | Location | Scope |
//! |---|---|
//! | `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` | this user |
//! | `HKLM\Software\Microsoft\Windows\CurrentVersion\Run` | all users |
//! | `HKCU\...\RunOnce` | this user, once |
//! | `HKLM\...\RunOnce` | all users, once |
//! | 32-bit HKLM `Run` / `RunOnce` registry view | all users, 32-bit |
//! | The user's Startup folder | this user |
//! | The common Startup folder | all users |
//!
//! Alternate views are requested with `KEY_WOW64_*KEY`; the reserved
//! physical `WOW6432Node` path is never addressed directly.
//!
//! ## Enabled state lives somewhere else entirely
//!
//! Disabling a startup item in Task Manager does **not** remove its `Run`
//! entry. It writes a binary blob to a parallel key,
//! `...\Explorer\StartupApproved\Run`, whose first byte is a flag: an even
//! value means enabled, an odd one means disabled. An item with no entry
//! there has never been touched and is enabled.
//!
//! So a startup list that reads only the `Run` keys reports disabled items
//! as enabled — which is worse than not showing the column, because it
//! actively contradicts what the user did.
//!
//! ## This module is read-only
//!
//! It reports what is registered and whether it is enabled. It does not
//! write. Toggling an item means writing that `StartupApproved` blob, and
//! removing one means deleting another program's registry value — both
//! are destructive changes to state this app does not own, and neither is
//! something to add without a confirmation flow designed around it. The
//! UI offers "open file location" and "copy path" instead, which is
//! enough to act on an item deliberately. Recorded in
//! `docs/WINDOWS_APIS.md` as a deliberate limit rather than an oversight.

use super::handle::OwnedKey;
use super::strings;
use std::path::PathBuf;
use windows_sys::Win32::System::Environment::ExpandEnvironmentStringsW;
use windows_sys::Win32::System::Registry::{
    RegEnumValueW, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE,
    KEY_READ, KEY_WOW64_32KEY, KEY_WOW64_64KEY,
};

/// One program registered to run at logon.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StartupEntry {
    /// The registered name, which is what the `StartupApproved` key is
    /// also keyed by.
    pub name: String,
    /// The command line, as registered.
    pub command: String,
    /// Where it came from, for the "Location" column.
    pub location: &'static str,
    /// Whether it applies to every user.
    pub all_users: bool,
    /// Whether it is currently enabled. See the module docs.
    pub enabled: bool,
}

impl StartupEntry {
    /// The executable path, extracted from the command line.
    ///
    /// A registered command is a command *line*, not a path: it may be
    /// quoted, and it may carry arguments. `"C:\Program Files\App\a.exe"
    /// --minimized` has to yield the path without the quotes and without
    /// the flag, or "open file location" opens nothing.
    #[must_use]
    pub fn executable(&self) -> Option<PathBuf> {
        let text = self.command.trim();
        if text.is_empty() {
            return None;
        }
        let path = if let Some(rest) = text.strip_prefix('"') {
            // Quoted: everything up to the closing quote, which is the
            // only reliable way to handle a path containing spaces.
            rest.split('"').next().unwrap_or(rest)
        } else {
            executable_prefix(text).unwrap_or_else(|| text.split(' ').next().unwrap_or(text))
        };
        let path = path.trim();
        (!path.is_empty()).then(|| PathBuf::from(path))
    }
}

/// One place startup entries are registered.
struct Location {
    /// Which hive.
    hive: HKEY,
    /// The subkey path.
    subkey: &'static str,
    /// The `StartupApproved` subkey that records enabled state, if this
    /// location has one.
    approved: Option<&'static str>,
    /// The label shown in the Location column.
    label: &'static str,
    /// Whether this location applies to every user.
    all_users: bool,
    /// Native or alternate registry view.
    view: u32,
}

/// Every registry location checked. See the module docs for the table.
fn locations() -> Vec<Location> {
    vec![
        Location {
            hive: HKEY_CURRENT_USER,
            subkey: r"Software\Microsoft\Windows\CurrentVersion\Run",
            approved: Some(
                r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run",
            ),
            label: "Registry (current user)",
            all_users: false,
            view: KEY_WOW64_64KEY,
        },
        Location {
            hive: HKEY_LOCAL_MACHINE,
            subkey: r"Software\Microsoft\Windows\CurrentVersion\Run",
            approved: Some(
                r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run",
            ),
            label: "Registry (all users)",
            all_users: true,
            view: KEY_WOW64_64KEY,
        },
        Location {
            hive: HKEY_LOCAL_MACHINE,
            subkey: r"Software\Microsoft\Windows\CurrentVersion\Run",
            approved: Some(
                r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run32",
            ),
            label: "Registry (all users, 32-bit)",
            all_users: true,
            view: KEY_WOW64_32KEY,
        },
        Location {
            hive: HKEY_CURRENT_USER,
            subkey: r"Software\Microsoft\Windows\CurrentVersion\RunOnce",
            approved: None,
            label: "Registry (run once)",
            all_users: false,
            view: KEY_WOW64_64KEY,
        },
        Location {
            hive: HKEY_LOCAL_MACHINE,
            subkey: r"Software\Microsoft\Windows\CurrentVersion\RunOnce",
            approved: None,
            label: "Registry (all users, run once)",
            all_users: true,
            view: KEY_WOW64_64KEY,
        },
        Location {
            hive: HKEY_LOCAL_MACHINE,
            subkey: r"Software\Microsoft\Windows\CurrentVersion\RunOnce",
            approved: None,
            label: "Registry (all users, 32-bit, run once)",
            all_users: true,
            view: KEY_WOW64_32KEY,
        },
    ]
}

/// Everything registered to run at logon.
#[must_use]
pub fn enumerate() -> Vec<StartupEntry> {
    let mut found = Vec::new();
    for location in locations() {
        let Some(key) = open(location.hive, location.subkey, location.view) else {
            continue;
        };
        let approved = location
            .approved
            .and_then(|subkey| open(location.hive, subkey, location.view));
        for (name, command) in values(&key) {
            let enabled = approved.as_ref().is_none_or(|key| is_approved(key, &name));
            found.push(StartupEntry {
                name,
                command,
                location: location.label,
                all_users: location.all_users,
                enabled,
            });
        }
    }
    found.extend(folder_entries());
    // Sorted by name so the list has a stable order between runs; the
    // registry's own enumeration order is not defined.
    found.sort_by_key(|entry| entry.name.to_lowercase());
    found
}

/// The two Startup folders' contents.
///
/// Shortcut files whose enablement is recorded in
/// `StartupApproved\StartupFolder`.
fn folder_entries() -> Vec<StartupEntry> {
    let mut found = Vec::new();
    let folders = [
        (
            dirs::config_dir()
                .map(|dir| dir.join(r"Microsoft\Windows\Start Menu\Programs\Startup")),
            "Startup folder (current user)",
            false,
        ),
        (
            std::env::var_os("ProgramData").map(|dir| {
                PathBuf::from(dir).join(r"Microsoft\Windows\Start Menu\Programs\Startup")
            }),
            "Startup folder (all users)",
            true,
        ),
    ];

    let approved = open(
        HKEY_CURRENT_USER,
        r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\StartupFolder",
        KEY_WOW64_64KEY,
    );
    for (path, label, all_users) in folders {
        let Some(path) = path else {
            continue;
        };
        let Ok(entries) = std::fs::read_dir(&path) else {
            continue;
        };
        for entry in entries.flatten() {
            let file = entry.path();
            // `desktop.ini` is folder metadata, not a startup item.
            if file
                .file_name()
                .is_some_and(|name| name.eq_ignore_ascii_case("desktop.ini"))
            {
                continue;
            }
            let name = file
                .file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
                .unwrap_or_default();
            if name.is_empty() {
                continue;
            }
            found.push(StartupEntry {
                name,
                command: file.to_string_lossy().into_owned(),
                location: label,
                all_users,
                enabled: approved.as_ref().is_none_or(|key| {
                    let file_name = file
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    is_approved(key, &file_name)
                }),
            });
        }
    }
    found
}

/// Opens a registry key for reading.
fn open(hive: HKEY, subkey: &str, view: u32) -> Option<OwnedKey> {
    let wide = strings::to_wide(subkey);
    let mut key: HKEY = std::ptr::null_mut();
    // SAFETY: `hive` is a predefined key constant. `wide` is a live,
    // NUL-terminated UTF-16 buffer bound to a local that outlives the
    // call. `key` is a live out-parameter the callee writes an open key
    // into on success; `OwnedKey` then owns and closes it.
    let status = open_registry_key(hive, &wide, view, &mut key);
    if status != 0 {
        return None;
    }
    OwnedKey::new(key)
}

/// Every `(name, value)` pair under a key, as strings.
///
/// Enumerated by index until the call reports there are no more, which is
/// the documented protocol. The index is bounded so a key that somehow
/// never reports the end cannot spin — the registry does not do that, but
/// this runs on a path the UI waits for.
fn values(key: &OwnedKey) -> Vec<(String, String)> {
    /// `ERROR_NO_MORE_ITEMS`, the documented end of the enumeration.
    const NO_MORE: u32 = 259;
    /// A bound on the enumeration. A `Run` key with more entries than
    /// this is not a startup configuration, it is a problem of its own.
    const MAX_VALUES: u32 = 4096;

    let mut found = Vec::new();
    for index in 0..MAX_VALUES {
        let mut name = vec![0u16; 512];
        let mut name_length = u32::try_from(name.len()).unwrap_or(0);
        let mut data = vec![0u8; 4096];
        let mut data_length = u32::try_from(data.len()).unwrap_or(0);
        let mut kind = 0u32;

        let status = enumerate_registry_value(
            key,
            index,
            &mut name,
            &mut name_length,
            &mut kind,
            &mut data,
            &mut data_length,
        );
        if status == NO_MORE {
            break;
        }
        if status != 0 {
            // A value too large for the buffers, or a transient error.
            // Skipped rather than ending the enumeration, so one
            // oversized entry does not hide every entry after it.
            continue;
        }

        let value_name = strings::from_wide_nul(strings::reported_slice(&name, name_length));
        if value_name.is_empty() {
            continue;
        }
        // `REG_SZ` and `REG_EXPAND_SZ` are the two a command line is
        // registered as; anything else is not one.
        const REG_SZ: u32 = 1;
        const REG_EXPAND_SZ: u32 = 2;
        if kind != REG_SZ && kind != REG_EXPAND_SZ {
            continue;
        }
        let mut command = string_from_bytes(&data, data_length);
        if kind == REG_EXPAND_SZ {
            command = expand_environment(&command).unwrap_or(command);
        }
        if command.is_empty() {
            continue;
        }
        found.push((value_name, command));
    }
    found
}

/// Reads one indexed registry value into caller-owned buffers.
fn enumerate_registry_value(
    key: &OwnedKey,
    index: u32,
    name: &mut [u16],
    name_length: &mut u32,
    kind: &mut u32,
    data: &mut [u8],
    data_length: &mut u32,
) -> u32 {
    // SAFETY: `key` is live with `KEY_READ`; both buffers and all length
    // and type out-parameters are uniquely borrowed for this call. The
    // null reserved argument is required by the API.
    unsafe {
        RegEnumValueW(
            key.raw(),
            index,
            name.as_mut_ptr(),
            std::ptr::from_mut(name_length),
            std::ptr::null_mut(),
            std::ptr::from_mut(kind),
            data.as_mut_ptr(),
            std::ptr::from_mut(data_length),
        )
    }
}

/// Expands `%NAME%` references from a `REG_EXPAND_SZ` command.
fn expand_environment(text: &str) -> Option<String> {
    let source = strings::to_wide(text);
    // SAFETY: null output asks for the required UTF-16 length.
    let needed = expanded_length(&source);
    if needed == 0 {
        return None;
    }
    let mut output = vec![0u16; usize::try_from(needed).ok()?];
    // SAFETY: `output` has the exact capacity returned above and both
    // strings remain live for the call.
    let written = expand_environment_into(&source, &mut output, needed);
    (written > 0 && written <= needed).then(|| strings::from_wide_nul(&output))
}

/// Finds a conventional executable suffix in an unquoted command.
fn executable_prefix(text: &str) -> Option<&str> {
    let lower = text.to_ascii_lowercase();
    [".exe", ".com", ".bat", ".cmd"]
        .into_iter()
        .filter_map(|suffix| lower.find(suffix).map(|index| index + suffix.len()))
        .min()
        .and_then(|end| text.get(..end))
}

/// Decodes a `REG_SZ` value's bytes as UTF-16.
fn string_from_bytes(data: &[u8], length: u32) -> String {
    let bytes = usize::try_from(length).unwrap_or(0).min(data.len());
    // A registry string is UTF-16, so the byte count is twice the unit
    // count. An odd byte count means a truncated final unit, which is
    // dropped rather than half-read.
    let units = bytes / 2;
    let mut wide = Vec::with_capacity(units);
    for index in 0..units {
        let Some(pair) = data.get(index * 2..index * 2 + 2) else {
            break;
        };
        let Ok(word) = <[u8; 2]>::try_from(pair) else {
            break;
        };
        wide.push(u16::from_le_bytes(word));
    }
    strings::from_wide_nul(&wide)
}

/// Whether an entry is enabled, per the `StartupApproved` key.
///
/// The value is a binary blob whose **first byte** is the flag: even
/// means enabled, odd means disabled. An entry with no value there has
/// never been toggled and is enabled. See the module docs.
fn is_approved(key: &OwnedKey, name: &str) -> bool {
    let wide = strings::to_wide(name);
    let mut data = vec![0u8; 64];
    let mut length = u32::try_from(data.len()).unwrap_or(0);
    let mut kind = 0u32;

    // SAFETY: `key` is a live registry key opened with KEY_READ. `wide`
    // is a live, NUL-terminated UTF-16 name bound to a local. `data` is
    // a live, uniquely-borrowed buffer of `length` bytes, which is what
    // the length out-parameter states. The null reserved argument is
    // documented as required.
    let status = query_registry_value(key.raw(), &wide, &mut kind, &mut data, &mut length);
    if status != 0 || length == 0 {
        // No entry: never toggled, therefore enabled.
        return true;
    }
    data.first().is_none_or(|flag| flag % 2 == 0)
}

fn open_registry_key(hive: HKEY, name: &[u16], view: u32, key: &mut HKEY) -> u32 {
    // SAFETY: hive is predefined, name is NUL-terminated, and key is a live output pointer.
    unsafe { RegOpenKeyExW(hive, name.as_ptr(), 0, KEY_READ | view, key) }
}
fn expanded_length(source: &[u16]) -> u32 {
    // SAFETY: source is live and NUL-terminated; null output is the documented size query.
    unsafe { ExpandEnvironmentStringsW(source.as_ptr(), std::ptr::null_mut(), 0) }
}
fn expand_environment_into(source: &[u16], output: &mut [u16], needed: u32) -> u32 {
    // SAFETY: source is NUL-terminated and output has the queried capacity; neither is retained.
    unsafe { ExpandEnvironmentStringsW(source.as_ptr(), output.as_mut_ptr(), needed) }
}
fn query_registry_value(
    key: HKEY,
    name: &[u16],
    kind: &mut u32,
    data: &mut [u8],
    length: &mut u32,
) -> u32 {
    // SAFETY: key is live; name is NUL-terminated and outputs are live writable buffers.
    unsafe {
        RegQueryValueExW(
            key,
            name.as_ptr(),
            std::ptr::null_mut(),
            kind,
            data.as_mut_ptr(),
            length,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_startup_list_reads_without_elevation() {
        // Every location here is readable by a normal user; the HKLM keys
        // are world-readable.
        let entries = enumerate();
        for entry in &entries {
            assert!(!entry.name.is_empty(), "an entry must be named");
            assert!(!entry.command.is_empty(), "an entry must have a command");
            assert!(!entry.location.is_empty());
        }
    }

    #[test]
    fn entries_are_ordered_stably() {
        // The registry's enumeration order is not defined, so the list
        // would otherwise reshuffle between runs.
        let first: Vec<String> = enumerate().into_iter().map(|e| e.name).collect();
        let second: Vec<String> = enumerate().into_iter().map(|e| e.name).collect();
        assert_eq!(first, second);
    }

    #[test]
    fn a_quoted_command_line_yields_the_path_without_its_arguments() {
        // "open file location" opens nothing if the flag is left on.
        let entry = StartupEntry {
            command: r#""C:\Program Files\App\a.exe" --minimized --quiet"#.to_string(),
            ..StartupEntry::default()
        };
        assert_eq!(
            entry.executable(),
            Some(PathBuf::from(r"C:\Program Files\App\a.exe"))
        );
    }

    #[test]
    fn an_unquoted_command_line_is_split_the_way_windows_splits_it() {
        // Genuinely ambiguous, and matching Windows' own resolution is
        // the correct answer rather than a compromise.
        let entry = StartupEntry {
            command: r"C:\Tools\thing.exe -run".to_string(),
            ..StartupEntry::default()
        };
        assert_eq!(
            entry.executable(),
            Some(PathBuf::from(r"C:\Tools\thing.exe"))
        );
    }

    #[test]
    fn a_bare_path_with_no_arguments_survives() {
        let entry = StartupEntry {
            command: r"C:\Tools\thing.exe".to_string(),
            ..StartupEntry::default()
        };
        assert_eq!(
            entry.executable(),
            Some(PathBuf::from(r"C:\Tools\thing.exe"))
        );
    }

    #[test]
    fn an_empty_or_whitespace_command_has_no_executable() {
        for command in ["", "   ", "\"\""] {
            let entry = StartupEntry {
                command: command.to_string(),
                ..StartupEntry::default()
            };
            assert_eq!(entry.executable(), None, "{command:?}");
        }
    }

    #[test]
    fn every_registry_location_is_checked() {
        // The WOW6432Node key is the one most often forgotten, and on a
        // 64-bit machine it is where every 32-bit installer writes.
        let all = locations();
        assert!(
            all.iter().any(|l| l.view == KEY_WOW64_32KEY),
            "the 32-bit registry view must be among the locations checked"
        );
        assert!(all.iter().any(|l| l.subkey.ends_with("RunOnce")));
        assert!(all.iter().any(|l| l.hive == HKEY_CURRENT_USER));
        assert!(all.iter().any(|l| l.hive == HKEY_LOCAL_MACHINE));
    }

    #[test]
    fn a_registry_string_is_decoded_from_bytes_not_units() {
        // The byte/unit confusion again: a registry value's length is in
        // bytes and its content is UTF-16.
        let text = "C:\\a.exe";
        let mut bytes = Vec::new();
        for unit in text.encode_utf16().chain(std::iter::once(0)) {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        let length = u32::try_from(bytes.len()).unwrap_or(0);
        assert_eq!(string_from_bytes(&bytes, length), text);
    }

    #[test]
    fn an_odd_byte_count_drops_the_truncated_unit_rather_than_half_reading_it() {
        let bytes = [0x41u8, 0x00, 0x42, 0x00, 0x43];
        assert_eq!(
            string_from_bytes(&bytes, 5),
            "AB",
            "the trailing half-unit is not a character"
        );
    }

    #[test]
    fn a_reported_length_past_the_buffer_is_clamped() {
        let bytes = [0x41u8, 0x00];
        assert_eq!(string_from_bytes(&bytes, 9_999), "A");
    }

    #[test]
    fn an_untouched_entry_reads_as_enabled() {
        // An item with no `StartupApproved` value has never been toggled.
        // Reporting it as disabled would blank the column for most of a
        // typical machine's list.
        let Some(key) = open(
            HKEY_CURRENT_USER,
            r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run",
            KEY_WOW64_64KEY,
        ) else {
            return;
        };
        assert!(
            is_approved(&key, "RustamanNoSuchStartupEntry"),
            "an entry with no approval record is enabled"
        );
    }

    #[test]
    fn a_key_that_does_not_exist_opens_to_nothing() {
        assert!(open(
            HKEY_CURRENT_USER,
            r"Software\NoSuchKeyRustaman",
            KEY_WOW64_64KEY
        )
        .is_none());
    }
}

// ============================================================================
// Module:       win::identity
// Description:  The per-process facts that never change — owner, image path,
//               bitness, elevation, description — resolved once and cached.
//
// Dependencies: windows-sys (OpenProcess, tokens, version info); super::handle,
//               super::strings
// ============================================================================

//! The facts about a process that are fixed for its lifetime.
//!
//! Owner, image path, bitness, elevation, and the executable's
//! `FileDescription`. None of them can change while a process runs — a
//! process cannot be re-parented onto a different binary, change user, or
//! become 32-bit — so all of them are resolved **once** per process and
//! cached against its [`crate::model::ProcessKey`].
//!
//! ## Why the caching is not an optimisation
//!
//! It is what makes the app usable. Resolving one process's identity
//! costs an `OpenProcess`, two or three queries, a token open, a SID
//! lookup, and — for the description — mapping and parsing the
//! executable's version resource. That is on the order of a millisecond,
//! dominated by the version resource, which reads from disk the first
//! time.
//!
//! Doing that for four hundred processes every second would cost several
//! hundred milliseconds per sample and a burst of disk reads, on a
//! one-second interval. The app would be the heaviest thing in its own
//! process list, and the numbers it reported would be measuring itself.
//!
//! Cached, it costs that once per process — so a steady-state sample does
//! no identity work at all, and only newly-started processes pay.
//!
//! ## Failure is normal here
//!
//! Every lookup below returns a default rather than an error when it
//! cannot answer. A protected process (anti-malware, LSA under Credential
//! Guard) refuses `OpenProcess` even with `SeDebugPrivilege`, and a
//! process running as another user refuses without it. That is not an
//! exceptional condition, it is most of the machine on a normal run — so
//! an unknown owner is an empty string and an unknown bitness is
//! [`Architecture::Unknown`], and the row still appears with everything
//! the kernel *did* report.

use super::handle::{OwnedHandle, OwnedLocalMemory};
use super::strings;
use crate::model::{Architecture, ProcessIcon, ProcessKey};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use windows_sys::Win32::Foundation::MAX_PATH;
use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;
use windows_sys::Win32::Security::{
    GetTokenInformation, TokenElevation, TokenUser, TOKEN_ELEVATION, TOKEN_QUERY, TOKEN_USER,
};
use windows_sys::Win32::Storage::FileSystem::{
    GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW,
};
use windows_sys::Win32::System::SystemInformation::{
    IMAGE_FILE_MACHINE_AMD64, IMAGE_FILE_MACHINE_ARM64, IMAGE_FILE_MACHINE_I386,
    IMAGE_FILE_MACHINE_UNKNOWN,
};
use windows_sys::Win32::System::Threading::{
    IsWow64Process2, OpenProcess, OpenProcessToken, QueryFullProcessImageNameW,
    PROCESS_QUERY_LIMITED_INFORMATION,
};

/// Everything resolved about one process, once.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Identity {
    /// Full path to the executable, if it could be read.
    pub path: Option<PathBuf>,
    /// `FileDescription` from the version resource, or empty.
    pub description: String,
    /// Owning account as `DOMAIN\user`, or empty.
    pub user: String,
    /// Whether the process runs with an elevated token.
    pub elevated: bool,
    /// Image bitness.
    pub architecture: Architecture,
    /// Explorer's small executable icon.
    pub icon: Option<Arc<ProcessIcon>>,
}

/// Resolved identities, keyed so a recycled PID cannot inherit one.
///
/// Keyed on [`ProcessKey`] rather than PID for the reason spelled out in
/// [`crate::model`]: Windows recycles PIDs freely, and a cache keyed on
/// the number alone would hand a new process the previous holder's name,
/// path, and owner. The row would look completely plausible and be
/// entirely wrong — and "open file location" would open the wrong
/// binary.
#[derive(Debug, Default)]
pub struct Cache {
    /// The resolved identities.
    entries: HashMap<ProcessKey, Identity>,
    /// Descriptions by image path, shared across every process running
    /// the same binary.
    ///
    /// A second layer, because the version resource is the expensive part
    /// and eighteen `chrome.exe` processes share one. Keyed by path
    /// rather than by process, so a browser's twentieth renderer costs
    /// nothing.
    descriptions: HashMap<FileKey, String>,
    /// Shell icons shared by every process using an image path.
    icons: HashMap<FileKey, Arc<ProcessIcon>>,
}

/// A path plus metadata that changes when an updater replaces the file.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct FileKey {
    path: PathBuf,
    bytes: u64,
    modified_nanos: u128,
}

impl FileKey {
    fn read(path: &Path) -> Self {
        let metadata = std::fs::metadata(path).ok();
        let bytes = metadata.as_ref().map_or(0, std::fs::Metadata::len);
        let modified_nanos = metadata
            .and_then(|value| value.modified().ok())
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |duration| duration.as_nanos());
        Self {
            path: path.to_path_buf(),
            bytes,
            modified_nanos,
        }
    }
}

impl Cache {
    /// An empty cache.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The identity for `key`, resolving and caching it on first use.
    pub fn get(&mut self, key: ProcessKey) -> Identity {
        if let Some(cached) = self.entries.get(&key) {
            return cached.clone();
        }
        let mut identity = resolve(key.pid);
        if let Some(path) = identity.path.clone() {
            identity.description = self.description_for(&path);
            identity.icon = self.icon_for(&path);
        }
        self.entries.insert(key, identity.clone());
        identity
    }

    fn icon_for(&mut self, path: &Path) -> Option<Arc<ProcessIcon>> {
        let key = FileKey::read(path);
        if let Some(cached) = self.icons.get(&key) {
            return Some(Arc::clone(cached));
        }
        let icon = Arc::new(super::app_icon::extract(path)?);
        if self.icons.len() >= 4096 {
            self.icons.clear();
        }
        self.icons.insert(key, Arc::clone(&icon));
        Some(icon)
    }

    /// The `FileDescription` for an image path, cached across processes.
    fn description_for(&mut self, path: &Path) -> String {
        let key = FileKey::read(path);
        if let Some(cached) = self.descriptions.get(&key) {
            return cached.clone();
        }
        let description = file_description(path).unwrap_or_default();
        if self.descriptions.len() >= 4096 {
            self.descriptions.clear();
        }
        self.descriptions.insert(key, description.clone());
        description
    }

    /// Drops entries for processes that are no longer running.
    ///
    /// Without this the cache is a slow leak: a machine that starts and
    /// stops a build's worth of compiler processes accumulates an entry
    /// each, and an app left open for a week accumulates tens of
    /// thousands. Called by the sampler with the keys in the current
    /// snapshot.
    ///
    /// The description and icon caches are keyed by path plus file
    /// metadata, so replacing an executable cannot retain its old label or
    /// art. They use a hard cap rather than live-process pruning: a compiler
    /// that runs a thousand times should still pay for its metadata once.
    pub fn retain_live(&mut self, live: &std::collections::HashSet<ProcessKey>) {
        self.entries.retain(|key, _| live.contains(key));
    }

    /// How many process identities are cached. For the diagnostics panel.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether nothing has been resolved yet.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Resolves one process's identity from scratch.
///
/// Every step degrades independently: a process whose token cannot be
/// opened still reports its path if the path could be read.
#[must_use]
pub fn resolve(pid: u32) -> Identity {
    let Some(process) = open(pid) else {
        return Identity::default();
    };
    Identity {
        path: image_path(&process),
        // Filled in by the cache, which shares it across processes with
        // the same image.
        description: String::new(),
        user: token_user(&process).unwrap_or_default(),
        elevated: token_elevated(&process).unwrap_or(false),
        architecture: architecture(&process),
        icon: None,
    }
}

/// Opens a process for querying.
///
/// `PROCESS_QUERY_LIMITED_INFORMATION` rather than
/// `PROCESS_QUERY_INFORMATION`: the limited right is enough for every
/// query here and is granted in cases the full right is not, so asking
/// for more would lose identity for processes this can otherwise read.
fn open(pid: u32) -> Option<OwnedHandle> {
    let raw = open_query_process(pid);
    OwnedHandle::new(raw)
}

/// The full path to a process's executable.
fn image_path(process: &OwnedHandle) -> Option<PathBuf> {
    // `MAX_PATH` is not actually the maximum — a path can reach ~32767
    // characters with long-path support — so this starts generously
    // rather than at 260 and retrying. One allocation per new process is
    // not worth a retry loop.
    let mut buffer = vec![0u16; 32 * 1024];
    let mut length = u32::try_from(buffer.len()).unwrap_or(MAX_PATH);
    let ok = read_process_image_path(process.raw(), &mut buffer, &mut length);
    if ok == 0 {
        return None;
    }
    let text = strings::from_wide_nul(strings::reported_slice(&buffer, length));
    (!text.is_empty()).then(|| PathBuf::from(text))
}

/// The account a process runs as, as `DOMAIN\user`.
///
/// Resolved through the SID rather than by any shorter route, because
/// there is no shorter route: a token carries a SID, and turning one into
/// a name is `LookupAccountSidW`.
fn token_user(process: &OwnedHandle) -> Option<String> {
    let token = open_token(process)?;
    let buffer = token_information(&token, TokenUser)?;
    // The buffer starts with a `TOKEN_USER`, whose first field points at
    // a SID stored later in the same buffer.
    let user = read_token_user(&buffer)?;
    if user.User.Sid.is_null() {
        return None;
    }
    let name = account_name(user.User.Sid);
    // Keep `buffer` alive across the call above; dropping it earlier
    // would dangle the SID pointer.
    drop(buffer);
    name
}

/// Turns the live `TOKEN_USER` SID into `DOMAIN\user`, falling back to text.
fn account_name(sid: *mut core::ffi::c_void) -> Option<String> {
    let mut name = vec![0u16; 256];
    let mut domain = vec![0u16; 256];
    let mut name_length = u32::try_from(name.len()).unwrap_or(0);
    let mut domain_length = u32::try_from(domain.len()).unwrap_or(0);
    let mut kind = 0i32;

    let ok = lookup_account_sid(
        sid,
        &mut name,
        &mut name_length,
        &mut domain,
        &mut domain_length,
        &mut kind,
    );

    if ok != 0 {
        let account = strings::from_wide_nul(strings::reported_slice(&name, name_length));
        let authority = strings::from_wide_nul(strings::reported_slice(&domain, domain_length));
        if account.is_empty() {
            return None;
        }
        if authority.is_empty() {
            return Some(account);
        }
        return Some(format!("{authority}\\{account}"));
    }

    // A SID with no account behind it — a deleted user, or an app
    // container identity — still identifies the process, and showing the
    // SID beats showing nothing.
    //
    let mut raw = std::ptr::null_mut();
    let ok = sid_to_string(sid, &mut raw);
    if ok == 0 {
        return None;
    }
    let owned = OwnedLocalMemory::new(raw.cast())?;
    let text = wide_from_pointer(owned.raw().cast(), &owned);
    Some(text)
}

/// Copies the NUL-terminated UTF-16 string owned by `OwnedLocalMemory`.
fn wide_from_pointer(pointer: *const u16, owned: &OwnedLocalMemory) -> String {
    if pointer.is_null() {
        return String::new();
    }
    let mut length = 0usize;
    // A SID string is at most ~200 characters; the cap is a guard against
    // a missing terminator rather than a real limit.
    const MAX_UNITS: usize = 1024;
    while length < MAX_UNITS {
        let unit = read_wide_unit(pointer, length);
        if unit == 0 {
            break;
        }
        length += 1;
    }
    let slice = wide_slice(pointer, length, owned);
    String::from_utf16_lossy(slice)
}

/// Whether a process's token is elevated.
fn token_elevated(process: &OwnedHandle) -> Option<bool> {
    let token = open_token(process)?;
    let buffer = token_information(&token, TokenElevation)?;
    let elevation = read_token_elevation(&buffer)?;
    Some(elevation.TokenIsElevated != 0)
}

/// Opens a process's access token for querying.
fn open_token(process: &OwnedHandle) -> Option<OwnedHandle> {
    let mut raw = std::ptr::null_mut();
    let ok = open_query_token(process.raw(), &mut raw);
    if ok == 0 {
        return None;
    }
    OwnedHandle::new(raw)
}

/// Reads one class of token information into a byte buffer.
///
/// `GetTokenInformation` uses the ask-then-fetch protocol: the first call
/// fails and reports the size needed, the second fills it in. Unlike the
/// process enumeration in [`super::nt`] this is not a race — a token's
/// user and elevation do not change size between two calls — so the
/// simple two-call form is correct here.
fn token_information(token: &OwnedHandle, class: i32) -> Option<Vec<u8>> {
    let mut needed = 0u32;
    query_token_information_size(token.raw(), class, &mut needed);
    let size = usize::try_from(needed).unwrap_or(0);
    if size == 0 {
        return None;
    }
    let mut buffer = vec![0u8; size];
    let ok = read_token_information(token.raw(), class, &mut buffer, needed, &mut needed);
    (ok != 0).then_some(buffer)
}

/// A process's image bitness.
///
/// `IsWow64Process2` rather than the older `IsWow64Process`: the old call
/// answers "is this running under WOW64", which is a different question
/// and returns false for *both* a native 64-bit process and a native
/// 32-bit process on 32-bit Windows. The newer call reports the actual
/// image machine, which is what the column claims to show, and it has
/// been available since Windows 10 1511 — comfortably below this app's
/// floor.
fn architecture(process: &OwnedHandle) -> Architecture {
    let mut image = IMAGE_FILE_MACHINE_UNKNOWN;
    let mut native = IMAGE_FILE_MACHINE_UNKNOWN;
    let ok = read_process_machines(process.raw(), &mut image, &mut native);
    if ok == 0 {
        return Architecture::Unknown;
    }
    // `image` is `IMAGE_FILE_MACHINE_UNKNOWN` when the process is *not*
    // running under WOW64 — that is, when it is native. So the native
    // machine is the answer in that case, and the emulated one otherwise.
    let machine = if image == IMAGE_FILE_MACHINE_UNKNOWN {
        native
    } else {
        image
    };
    match machine {
        IMAGE_FILE_MACHINE_I386 => Architecture::X86,
        IMAGE_FILE_MACHINE_AMD64 => Architecture::X64,
        IMAGE_FILE_MACHINE_ARM64 => Architecture::Arm64,
        _ => Architecture::Unknown,
    }
}

/// The `FileDescription` string from an executable's version resource.
///
/// This is what turns `chrome.exe` into "Google Chrome" and
/// `svchost.exe` into "Host Process for Windows Services" — the single
/// biggest difference between a process list that can be read and one
/// that has to be decoded.
///
/// Returns `None` for a binary with no version resource, which is normal
/// for a great deal of developer and system tooling.
#[must_use]
pub fn file_description(path: &Path) -> Option<String> {
    let wide = strings::to_wide(&path.to_string_lossy());
    let mut ignored = 0u32;
    let size = version_info_size(&wide, &mut ignored);
    let size_usize = usize::try_from(size).unwrap_or(0);
    if size_usize == 0 {
        return None;
    }

    let mut block = vec![0u8; size_usize];
    let ok = read_version_info(&wide, size, &mut block);
    if ok == 0 {
        return None;
    }

    // The version resource is keyed by language and codepage, and the
    // key naming the translation has to be read first: hard-coding
    // 040904b0 (US English, Unicode) works on an English install and
    // returns nothing on a German one, which would blank the column for
    // everyone outside the anglosphere.
    let (language, codepage) = translation(&block)?;
    let key = format!("\\StringFileInfo\\{language:04x}{codepage:04x}\\FileDescription");
    let text = query_string(&block, &key)?;
    (!text.is_empty()).then_some(text)
}

/// The first (language, codepage) pair a version resource declares.
fn translation(block: &[u8]) -> Option<(u16, u16)> {
    let key = strings::to_wide("\\VarFileInfo\\Translation");
    let mut pointer: *mut core::ffi::c_void = std::ptr::null_mut();
    let mut length = 0u32;
    let ok = query_version_value(block, &key, &mut pointer, &mut length);
    // Each translation entry is two u16s; one is enough.
    if ok == 0 || pointer.is_null() || length < 4 {
        return None;
    }
    read_translation_pair(pointer, block)
}

/// Reads one string value out of a version-info block.
///
/// Note that `VerQueryValueW` returns a pointer *into* `block` rather
/// than allocating: there is nothing to free, and the buffer has to
/// outlive every pointer taken from it. That is why `block` is borrowed
/// for the whole call and the string is copied out before returning.
fn query_string(block: &[u8], key: &str) -> Option<String> {
    let wide_key = strings::to_wide(key);
    let mut pointer: *mut core::ffi::c_void = std::ptr::null_mut();
    let mut length = 0u32;
    let ok = query_version_value(block, &wide_key, &mut pointer, &mut length);
    if ok == 0 || pointer.is_null() || length == 0 {
        return None;
    }
    let units = usize::try_from(length).unwrap_or(0);
    let slice = version_string_slice(pointer.cast(), units, block)?;
    Some(strings::from_wide_nul(slice))
}

fn open_query_process(pid: u32) -> windows_sys::Win32::Foundation::HANDLE {
    // SAFETY: all arguments are by value and the returned handle is immediately owned or rejected.
    unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) }
}
fn read_process_image_path(
    handle: windows_sys::Win32::Foundation::HANDLE,
    buffer: &mut [u16],
    length: &mut u32,
) -> i32 {
    // SAFETY: handle is live; buffer and length are live writable out-parameters and are not retained.
    unsafe {
        QueryFullProcessImageNameW(
            handle,
            windows_sys::Win32::System::Threading::PROCESS_NAME_WIN32,
            buffer.as_mut_ptr(),
            length,
        )
    }
}
fn read_token_user(buffer: &[u8]) -> Option<TOKEN_USER> {
    if buffer.len() < std::mem::size_of::<TOKEN_USER>() {
        return None;
    }
    // SAFETY: caller checked this buffer holds a TOKEN_USER from TokenUser; unaligned reads are supported.
    Some(unsafe { std::ptr::read_unaligned(buffer.as_ptr().cast::<TOKEN_USER>()) })
}
fn lookup_account_sid(
    sid: *mut core::ffi::c_void,
    name: &mut [u16],
    name_length: &mut u32,
    domain: &mut [u16],
    domain_length: &mut u32,
    kind: &mut i32,
) -> i32 {
    use windows_sys::Win32::Security::LookupAccountSidW;
    // SAFETY: sid points into the live token buffer; all slices and lengths are live out-parameters and no pointer is retained.
    unsafe {
        LookupAccountSidW(
            std::ptr::null(),
            sid,
            name.as_mut_ptr(),
            name_length,
            domain.as_mut_ptr(),
            domain_length,
            kind,
        )
    }
}
fn sid_to_string(sid: *mut core::ffi::c_void, raw: &mut *mut u16) -> i32 {
    // SAFETY: sid points into the live token buffer and raw is a live output pointer for LocalAlloc-owned text.
    unsafe { ConvertSidToStringSidW(sid, raw) }
}
fn read_wide_unit(pointer: *const u16, index: usize) -> u16 {
    // SAFETY: caller owns the LocalAlloc SID string and bounds its scan to its guaranteed terminator.
    unsafe { *pointer.add(index) }
}
fn wide_slice(pointer: *const u16, length: usize, _owned: &OwnedLocalMemory) -> &[u16] {
    // SAFETY: caller scanned these initialized units while the owned LocalAlloc allocation remains live.
    unsafe { std::slice::from_raw_parts(pointer, length) }
}
fn read_token_elevation(buffer: &[u8]) -> Option<TOKEN_ELEVATION> {
    if buffer.len() < std::mem::size_of::<TOKEN_ELEVATION>() {
        return None;
    }
    // SAFETY: caller checked this buffer holds TOKEN_ELEVATION from TokenElevation; unaligned reads are supported.
    Some(unsafe { std::ptr::read_unaligned(buffer.as_ptr().cast::<TOKEN_ELEVATION>()) })
}
fn open_query_token(
    process: windows_sys::Win32::Foundation::HANDLE,
    raw: &mut windows_sys::Win32::Foundation::HANDLE,
) -> i32 {
    // SAFETY: process is live and raw is a live writable out-parameter not retained by Win32.
    unsafe { OpenProcessToken(process, TOKEN_QUERY, raw) }
}
fn query_token_information_size(
    token: windows_sys::Win32::Foundation::HANDLE,
    class: i32,
    needed: &mut u32,
) {
    // SAFETY: null buffer is the documented size query; needed is a live writable out-parameter.
    let _ = unsafe { GetTokenInformation(token, class, std::ptr::null_mut(), 0, needed) };
}
fn read_token_information(
    token: windows_sys::Win32::Foundation::HANDLE,
    class: i32,
    buffer: &mut [u8],
    size: u32,
    needed: &mut u32,
) -> i32 {
    // SAFETY: buffer is allocated for size bytes; needed is live output storage and no pointer is retained.
    unsafe { GetTokenInformation(token, class, buffer.as_mut_ptr().cast(), size, needed) }
}
fn read_process_machines(
    handle: windows_sys::Win32::Foundation::HANDLE,
    image: &mut u16,
    native: &mut u16,
) -> i32 {
    // SAFETY: handle is live and both machine outputs are distinct writable out-parameters.
    unsafe { IsWow64Process2(handle, image, native) }
}
fn version_info_size(path: &[u16], ignored: &mut u32) -> u32 {
    // SAFETY: path is live and NUL-terminated; ignored is a live compatibility out-parameter.
    unsafe { GetFileVersionInfoSizeW(path.as_ptr(), ignored) }
}
fn read_version_info(path: &[u16], size: u32, block: &mut [u8]) -> i32 {
    // SAFETY: path is NUL-terminated and block has exactly the reported size; neither is retained.
    unsafe { GetFileVersionInfoW(path.as_ptr(), 0, size, block.as_mut_ptr().cast()) }
}
fn query_version_value(
    block: &[u8],
    key: &[u16],
    pointer: &mut *mut core::ffi::c_void,
    length: &mut u32,
) -> i32 {
    // SAFETY: block is a live version resource, key is NUL-terminated, and outputs are live; result points only into block.
    unsafe { VerQueryValueW(block.as_ptr().cast(), key.as_ptr(), pointer, length) }
}
fn read_translation_pair(pointer: *const core::ffi::c_void, block: &[u8]) -> Option<(u16, u16)> {
    if !region_is_inside(pointer.cast(), 4, std::mem::align_of::<u16>(), block) {
        return None;
    }
    // SAFETY: `VerQueryValueW` reported at least four readable bytes at
    // this pointer and the range check above proved they remain inside
    // the live version-info block. Unaligned access is deliberate.
    let [language, codepage] = unsafe { std::ptr::read_unaligned(pointer.cast::<[u16; 2]>()) };
    Some((language, codepage))
}
fn version_string_slice(pointer: *const u16, units: usize, block: &[u8]) -> Option<&[u16]> {
    let bytes = units.checked_mul(std::mem::size_of::<u16>())?;
    if !region_is_inside(pointer.cast(), bytes, std::mem::align_of::<u16>(), block) {
        return None;
    }
    // SAFETY: the checked byte range is aligned for u16 and lies wholly
    // inside the live version-info block borrowed for the returned slice.
    Some(unsafe { std::slice::from_raw_parts(pointer, units) })
}

/// Whether a raw byte range is aligned and wholly inside its live owner.
fn region_is_inside(pointer: *const u8, length: usize, alignment: usize, owner: &[u8]) -> bool {
    let start = pointer as usize;
    let owner_start = owner.as_ptr() as usize;
    let Some(end) = start.checked_add(length) else {
        return false;
    };
    let Some(owner_end) = owner_start.checked_add(owner.len()) else {
        return false;
    };
    start >= owner_start && end <= owner_end && start.is_multiple_of(alignment)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// This process's own key, which is always resolvable.
    fn own_key() -> ProcessKey {
        ProcessKey {
            pid: std::process::id(),
            started_at: 1,
        }
    }

    #[test]
    fn this_process_can_resolve_its_own_identity() {
        let identity = resolve(std::process::id());
        let path = identity.path.unwrap_or_default();
        assert!(
            path.to_string_lossy().to_lowercase().contains(".exe"),
            "the test binary's own path should have been read, got {path:?}"
        );
        assert!(
            !identity.user.is_empty(),
            "a process can always read its own token"
        );
        assert_ne!(
            identity.architecture,
            Architecture::Unknown,
            "IsWow64Process2 should report this process's own bitness"
        );
    }

    #[test]
    fn a_pid_that_does_not_exist_yields_a_blank_identity_rather_than_failing() {
        // Not an error path: a process can exit between being enumerated
        // and being resolved, on every sample.
        let identity = resolve(0xffff_fffe);
        assert_eq!(identity, Identity::default());
    }

    #[test]
    fn the_idle_process_cannot_be_opened_and_that_is_fine() {
        let identity = resolve(crate::model::IDLE_PID);
        assert!(identity.path.is_none(), "PID 0 has no image");
    }

    #[test]
    fn a_resolved_identity_is_cached_rather_than_resolved_again() {
        let mut cache = Cache::new();
        assert!(cache.is_empty());
        let first = cache.get(own_key());
        assert_eq!(cache.len(), 1);
        let second = cache.get(own_key());
        assert_eq!(first, second, "the cached value must be the same value");
        assert_eq!(cache.len(), 1, "and must not have been resolved twice");
    }

    #[test]
    fn a_recycled_pid_does_not_inherit_the_previous_holders_identity() {
        // The reason the cache is keyed on (pid, start time). Keyed on
        // the PID alone, the second process here would be handed the
        // first one's name, path and owner — and "open file location"
        // would open the wrong binary.
        let mut cache = Cache::new();
        let first = ProcessKey {
            pid: 4242,
            started_at: 100,
        };
        let second = ProcessKey {
            pid: 4242,
            started_at: 200,
        };
        let _ = cache.get(first);
        let _ = cache.get(second);
        assert_eq!(
            cache.len(),
            2,
            "the same PID at two creation times is two processes"
        );
    }

    #[test]
    fn pruning_drops_dead_processes_and_keeps_live_ones() {
        // Without this the cache is a slow leak: an app left open for a
        // week accumulates an entry per process ever started.
        let mut cache = Cache::new();
        let live = own_key();
        let dead = ProcessKey {
            pid: 999_999,
            started_at: 1,
        };
        let _ = cache.get(live);
        let _ = cache.get(dead);
        assert_eq!(cache.len(), 2);

        let mut keep = std::collections::HashSet::new();
        keep.insert(live);
        cache.retain_live(&keep);
        assert_eq!(cache.len(), 1, "the dead process should have been dropped");
    }

    #[test]
    fn a_system_binary_has_a_readable_description() {
        // The thing that turns `chrome.exe` into "Google Chrome". Tested
        // against a binary every Windows machine has.
        let path = Path::new("C:\\Windows\\System32\\notepad.exe");
        if !path.exists() {
            return;
        }
        let description = file_description(path).unwrap_or_default();
        assert!(
            !description.is_empty(),
            "notepad.exe carries a version resource with a FileDescription"
        );
    }

    #[test]
    fn a_file_with_no_version_resource_is_not_an_error() {
        // Normal for a great deal of developer tooling.
        let path = std::env::temp_dir().join("rustaman-no-version-resource.txt");
        if std::fs::write(&path, b"not a PE file").is_err() {
            return;
        }
        assert!(file_description(&path).is_none());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_path_that_does_not_exist_is_not_an_error() {
        assert!(file_description(Path::new("C:\\nope\\nothing.exe")).is_none());
    }
}

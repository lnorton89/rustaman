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
use crate::model::{Architecture, ProcessKey};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
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
    descriptions: HashMap<PathBuf, String>,
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
        }
        self.entries.insert(key, identity.clone());
        identity
    }

    /// The `FileDescription` for an image path, cached across processes.
    fn description_for(&mut self, path: &Path) -> String {
        if let Some(cached) = self.descriptions.get(path) {
            return cached.clone();
        }
        let description = file_description(path).unwrap_or_default();
        self.descriptions
            .insert(path.to_path_buf(), description.clone());
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
    /// The description cache is deliberately **not** pruned. It is keyed
    /// by path, bounded by the number of distinct executables the machine
    /// has run, and re-reading a version resource is the expensive thing
    /// this whole module exists to avoid — a compiler that runs a
    /// thousand times should pay for its description once, not a thousand
    /// times.
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
    }
}

/// Opens a process for querying.
///
/// `PROCESS_QUERY_LIMITED_INFORMATION` rather than
/// `PROCESS_QUERY_INFORMATION`: the limited right is enough for every
/// query here and is granted in cases the full right is not, so asking
/// for more would lose identity for processes this can otherwise read.
fn open(pid: u32) -> Option<OwnedHandle> {
    // SAFETY: no pointers are involved — the call takes an access mask, a
    // BOOL, and a PID by value, and returns a handle or null. The
    // returned handle is immediately given to `OwnedHandle`, which
    // rejects null and closes it on drop.
    let raw = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
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
    // SAFETY: `process` is a live handle opened with
    // PROCESS_QUERY_LIMITED_INFORMATION, which this call requires.
    // `buffer` is a live, uniquely-borrowed allocation of at least
    // `length` u16s — `length` is derived from `buffer.len()`. `length`
    // is also a live out-parameter the callee overwrites with the
    // characters written. Nothing is retained.
    let ok = unsafe {
        QueryFullProcessImageNameW(
            process.raw(),
            windows_sys::Win32::System::Threading::PROCESS_NAME_WIN32,
            buffer.as_mut_ptr(),
            std::ptr::from_mut(&mut length),
        )
    };
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
    if buffer.len() < std::mem::size_of::<TOKEN_USER>() {
        return None;
    }
    // SAFETY: the buffer is at least `size_of::<TOKEN_USER>()` bytes
    // (checked above) and was filled by `GetTokenInformation` for the
    // `TokenUser` class, which documents that layout. `read_unaligned`
    // imposes no alignment requirement, which matters because the buffer
    // is a `Vec<u8>`. The `TOKEN_USER` is copied out, but its `Sid`
    // pointer still points into `buffer`, which is alive for the rest of
    // this function — that is why the SID is consumed below rather than
    // returned.
    let user = unsafe { std::ptr::read_unaligned(buffer.as_ptr().cast::<TOKEN_USER>()) };
    if user.User.Sid.is_null() {
        return None;
    }
    // SAFETY: `user.User.Sid` points into `buffer`, which is still alive
    // here, and was written by the kernel as a valid SID.
    let name = unsafe { account_name(user.User.Sid) };
    // Keep `buffer` alive across the call above; dropping it earlier
    // would dangle the SID pointer.
    drop(buffer);
    name
}

/// Turns a SID into `DOMAIN\user`, falling back to its string form.
///
/// # Safety
///
/// `sid` must point at a valid SID that stays alive for this call.
unsafe fn account_name(sid: *mut core::ffi::c_void) -> Option<String> {
    use windows_sys::Win32::Security::LookupAccountSidW;

    let mut name = vec![0u16; 256];
    let mut domain = vec![0u16; 256];
    let mut name_length = u32::try_from(name.len()).unwrap_or(0);
    let mut domain_length = u32::try_from(domain.len()).unwrap_or(0);
    let mut kind = 0i32;

    // SAFETY: the caller guarantees `sid` is a valid, live SID. `name`
    // and `domain` are live, uniquely-borrowed buffers whose lengths are
    // passed alongside them and which the callee will not exceed; the
    // two length variables are live out-parameters. A null first
    // argument asks for the local system. Nothing is retained past the
    // call.
    let ok = unsafe {
        LookupAccountSidW(
            std::ptr::null(),
            sid,
            name.as_mut_ptr(),
            std::ptr::from_mut(&mut name_length),
            domain.as_mut_ptr(),
            std::ptr::from_mut(&mut domain_length),
            std::ptr::from_mut(&mut kind),
        )
    };

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
    // SAFETY: as above for `sid`. The out-parameter receives a
    // `LocalAlloc`-owned string, which `OwnedLocalMemory` takes over.
    let mut raw = std::ptr::null_mut();
    let ok = unsafe { ConvertSidToStringSidW(sid, std::ptr::from_mut(&mut raw)) };
    if ok == 0 {
        return None;
    }
    let owned = OwnedLocalMemory::new(raw.cast())?;
    // SAFETY: `raw` is a live, NUL-terminated UTF-16 string owned by
    // `owned` for the rest of this scope. The length is bounded by
    // scanning for the terminator the call guarantees.
    let text = unsafe { wide_from_pointer(owned.raw().cast()) };
    Some(text)
}

/// Reads a NUL-terminated wide string from a raw pointer.
///
/// # Safety
///
/// `pointer` must be non-null and point at a NUL-terminated UTF-16 string
/// that stays alive for this call.
unsafe fn wide_from_pointer(pointer: *const u16) -> String {
    if pointer.is_null() {
        return String::new();
    }
    let mut length = 0usize;
    // A SID string is at most ~200 characters; the cap is a guard against
    // a missing terminator rather than a real limit.
    const MAX_UNITS: usize = 1024;
    while length < MAX_UNITS {
        // SAFETY: the caller guarantees the string is NUL-terminated, so
        // every unit up to and including the terminator is within the
        // allocation. The cap stops the scan if it is not.
        let unit = unsafe { *pointer.add(length) };
        if unit == 0 {
            break;
        }
        length += 1;
    }
    // SAFETY: `length` units were just confirmed readable by the scan
    // above, and the buffer is alive for this call.
    let slice = unsafe { std::slice::from_raw_parts(pointer, length) };
    String::from_utf16_lossy(slice)
}

/// Whether a process's token is elevated.
fn token_elevated(process: &OwnedHandle) -> Option<bool> {
    let token = open_token(process)?;
    let buffer = token_information(&token, TokenElevation)?;
    if buffer.len() < std::mem::size_of::<TOKEN_ELEVATION>() {
        return None;
    }
    // SAFETY: the buffer holds at least a `TOKEN_ELEVATION` (checked
    // above) written by `GetTokenInformation` for that class.
    // `read_unaligned` imposes no alignment requirement on the `Vec<u8>`.
    let elevation = unsafe { std::ptr::read_unaligned(buffer.as_ptr().cast::<TOKEN_ELEVATION>()) };
    Some(elevation.TokenIsElevated != 0)
}

/// Opens a process's access token for querying.
fn open_token(process: &OwnedHandle) -> Option<OwnedHandle> {
    let mut raw = std::ptr::null_mut();
    // SAFETY: `process` is a live process handle. `raw` is a live,
    // uniquely-borrowed out-parameter the callee writes a real handle
    // into on success; `OwnedHandle` then owns and closes it.
    let ok = unsafe { OpenProcessToken(process.raw(), TOKEN_QUERY, std::ptr::from_mut(&mut raw)) };
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
    // SAFETY: `token` is a live token handle opened with TOKEN_QUERY. A
    // null buffer with a zero length is the documented way to ask for
    // the required size; the call fails and writes it to `needed`, which
    // is a live out-parameter.
    let _ = unsafe {
        GetTokenInformation(
            token.raw(),
            class,
            std::ptr::null_mut(),
            0,
            std::ptr::from_mut(&mut needed),
        )
    };
    let size = usize::try_from(needed).unwrap_or(0);
    if size == 0 {
        return None;
    }
    let mut buffer = vec![0u8; size];
    // SAFETY: `buffer` is a live, uniquely-borrowed allocation of exactly
    // `needed` bytes, which is what the length argument says. The callee
    // writes only within it and does not retain the pointer.
    let ok = unsafe {
        GetTokenInformation(
            token.raw(),
            class,
            buffer.as_mut_ptr().cast(),
            needed,
            std::ptr::from_mut(&mut needed),
        )
    };
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
    // SAFETY: `process` is a live handle opened with
    // PROCESS_QUERY_LIMITED_INFORMATION, which this call accepts. Both
    // out-parameters are live, uniquely-borrowed values the callee
    // writes once.
    let ok = unsafe {
        IsWow64Process2(
            process.raw(),
            std::ptr::from_mut(&mut image),
            std::ptr::from_mut(&mut native),
        )
    };
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
    // SAFETY: `wide` is a live, NUL-terminated UTF-16 path bound to a
    // local that outlives the call. `ignored` is a live out-parameter the
    // call writes a zero into (it exists only for compatibility).
    let size = unsafe { GetFileVersionInfoSizeW(wide.as_ptr(), std::ptr::from_mut(&mut ignored)) };
    let size_usize = usize::try_from(size).unwrap_or(0);
    if size_usize == 0 {
        return None;
    }

    let mut block = vec![0u8; size_usize];
    // SAFETY: `wide` is as above. `block` is a live, uniquely-borrowed
    // allocation of exactly `size` bytes, which is what the length
    // argument states and what the call just reported it needs.
    let ok = unsafe { GetFileVersionInfoW(wide.as_ptr(), 0, size, block.as_mut_ptr().cast()) };
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
    // SAFETY: `block` is a live version-info block filled by
    // `GetFileVersionInfoW`. `key` is a live NUL-terminated buffer bound
    // to a local. Both out-parameters are live. On success the call
    // points `pointer` *into* `block`, which is borrowed for the whole
    // of this function — so the read below cannot dangle.
    let ok = unsafe {
        VerQueryValueW(
            block.as_ptr().cast(),
            key.as_ptr(),
            std::ptr::from_mut(&mut pointer),
            std::ptr::from_mut(&mut length),
        )
    };
    // Each translation entry is two u16s; one is enough.
    if ok == 0 || pointer.is_null() || length < 4 {
        return None;
    }
    // SAFETY: the call reported at least 4 bytes at `pointer`, which
    // points into the live `block`. Two `u16`s are read without an
    // alignment requirement.
    let pair = unsafe {
        let language = std::ptr::read_unaligned(pointer.cast::<u16>());
        let codepage = std::ptr::read_unaligned(pointer.cast::<u16>().add(1));
        (language, codepage)
    };
    Some(pair)
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
    // SAFETY: as `translation` — `block` and `wide_key` are both live for
    // the call, the out-parameters are live, and on success `pointer`
    // points into `block`.
    let ok = unsafe {
        VerQueryValueW(
            block.as_ptr().cast(),
            wide_key.as_ptr(),
            std::ptr::from_mut(&mut pointer),
            std::ptr::from_mut(&mut length),
        )
    };
    if ok == 0 || pointer.is_null() || length == 0 {
        return None;
    }
    let units = usize::try_from(length).unwrap_or(0);
    // SAFETY: the call reported `length` UTF-16 units at `pointer`,
    // which points into the live `block`. The slice is consumed before
    // this function returns.
    let slice = unsafe { std::slice::from_raw_parts(pointer.cast::<u16>(), units) };
    Some(strings::from_wide_nul(slice))
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

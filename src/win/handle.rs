// ============================================================================
// Module:       win::handle
// Description:  Owning wrappers for the Win32 resources that need releasing,
//               so an early return cannot leak one.
//
// Dependencies: windows-sys (CloseHandle, LocalFree, RegCloseKey)
// ============================================================================

//! RAII for the handles this crate opens.
//!
//! The rule from `CLAUDE.md`: anything with a matching close or free call
//! gets an owning wrapper with a `Drop` impl. In a task manager that is
//! not a stylistic preference. The sampler opens a handle per process per
//! interval, so a leak on an error path — the path that runs for every
//! protected process, every sample, on every machine — exhausts the
//! handle table of the machine being monitored within minutes. The app
//! would show a handle leak in its own process list and be the cause of
//! it.
//!
//! Each wrapper here holds a handle that is **known non-null and not
//! `INVALID_HANDLE_VALUE`**, because the constructor rejects those. That
//! is what lets the rest of the crate treat one as simply "an open
//! process" with no further checking, and what makes the `Drop` impls
//! unconditionally correct.

use windows_sys::Win32::Foundation::{
    CloseHandle, LocalFree, HANDLE, HLOCAL, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::System::Registry::{RegCloseKey, HKEY};

/// A kernel handle that will be closed on drop.
///
/// Constructed only through [`OwnedHandle::new`], which rejects the two
/// failure sentinels — so the contained handle is always one
/// `CloseHandle` is correct for.
#[derive(Debug)]
pub struct OwnedHandle(HANDLE);

impl OwnedHandle {
    /// Takes ownership of a handle returned by a Win32 call.
    ///
    /// Returns `None` for the two values Win32 uses to mean failure,
    /// which are not the same value and are not interchangeable:
    /// `OpenProcess` returns null on failure while `CreateFileW` returns
    /// `INVALID_HANDLE_VALUE` (which is -1). Checking only one of them is
    /// the classic way to end up calling `CloseHandle(-1)`.
    #[must_use]
    pub fn new(handle: HANDLE) -> Option<Self> {
        if handle.is_null() || handle == INVALID_HANDLE_VALUE {
            return None;
        }
        Some(Self(handle))
    }

    /// The raw handle, for passing to another call.
    ///
    /// Borrowed rather than copied out: the returned value is only valid
    /// while `self` is alive, and taking `&self` is what ties it to that.
    #[must_use]
    pub fn raw(&self) -> HANDLE {
        self.0
    }
}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        close_kernel_handle(self.0);
    }
}

/// Closes one owned kernel handle.
fn close_kernel_handle(handle: HANDLE) {
    // SAFETY: `OwnedHandle::new` rejected both failure sentinels and the
    // non-Copy owner calls this once, so `handle` is a live kernel handle.
    let _ = unsafe { CloseHandle(handle) };
}

/// A block allocated by a Win32 call that documents `LocalFree` as its
/// release function.
///
/// `ConvertSidToStringSidW` is the one this crate uses. The pattern is
/// worth a type rather than a `let _ = LocalFree(..)` at the call site,
/// because the interesting path — the SID converts but the string then
/// fails to marshal — is exactly the one an early return skips.
#[derive(Debug)]
pub struct OwnedLocalMemory(HLOCAL);

impl OwnedLocalMemory {
    /// Takes ownership of a `LocalAlloc`-family allocation.
    ///
    /// Returns `None` for null, which is how every such call reports
    /// failure.
    #[must_use]
    pub fn new(pointer: HLOCAL) -> Option<Self> {
        if pointer.is_null() {
            return None;
        }
        Some(Self(pointer))
    }

    /// The raw pointer, valid while `self` is alive.
    #[must_use]
    pub fn raw(&self) -> HLOCAL {
        self.0
    }
}

impl Drop for OwnedLocalMemory {
    fn drop(&mut self) {
        free_local_memory(self.0);
    }
}

/// Frees a Win32 allocation whose documented release function is `LocalFree`.
fn free_local_memory(memory: HLOCAL) {
    // SAFETY: the constructor accepted only a non-null pointer returned by a
    // LocalAlloc-family API; this uniquely-owned wrapper releases it once.
    let _ = unsafe { LocalFree(memory) };
}

/// An open registry key that will be closed on drop.
///
/// Used by [`super::startup`], which walks four `Run` keys under two
/// hives and opens a subkey per entry — enough nesting that a hand-placed
/// `RegCloseKey` per early return would be missed at least once.
#[derive(Debug)]
pub struct OwnedKey(HKEY);

impl OwnedKey {
    /// Takes ownership of a key handle returned by `RegOpenKeyExW`.
    #[must_use]
    pub fn new(key: HKEY) -> Option<Self> {
        if key.is_null() {
            return None;
        }
        Some(Self(key))
    }

    /// The raw key, valid while `self` is alive.
    #[must_use]
    pub fn raw(&self) -> HKEY {
        self.0
    }
}

impl Drop for OwnedKey {
    fn drop(&mut self) {
        close_registry_key(self.0);
    }
}

/// Closes one uniquely-owned open registry key.
fn close_registry_key(key: HKEY) {
    // SAFETY: the constructor accepted only a non-null `RegOpenKeyExW` result
    // and this non-Copy wrapper owns it exclusively, so this is the one close.
    let _ = unsafe { RegCloseKey(key) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_two_failure_sentinels_are_both_rejected() {
        // They are different values and are not interchangeable:
        // `OpenProcess` fails with null, `CreateFileW` with -1. Checking
        // only one is how `CloseHandle(-1)` gets called.
        assert!(
            OwnedHandle::new(std::ptr::null_mut()).is_none(),
            "a null handle must not be wrapped"
        );
        assert!(
            OwnedHandle::new(INVALID_HANDLE_VALUE).is_none(),
            "INVALID_HANDLE_VALUE must not be wrapped"
        );
    }

    #[test]
    fn a_null_allocation_or_key_is_rejected() {
        assert!(OwnedLocalMemory::new(std::ptr::null_mut()).is_none());
        assert!(OwnedKey::new(std::ptr::null_mut()).is_none());
    }
}

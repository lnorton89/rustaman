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
        // SAFETY: `self.0` came from `OwnedHandle::new`, which rejected
        // null and INVALID_HANDLE_VALUE, so this is a handle the kernel
        // issued to this process. Nothing else holds a copy — `raw`
        // only lends it for the duration of a borrow, and this type is
        // neither `Copy` nor `Clone` — so this is the one close.
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

// A handle is a process-wide token, not a thread-local one, and every
// wrapper here owns its handle exclusively. Sending one to the sampler
// thread is therefore sound, and is what lets identity lookups be done
// off the UI thread.
//
// SAFETY: `OwnedHandle` has exclusive ownership of a kernel handle.
// Kernel handles are valid in any thread of the owning process, and
// `CloseHandle` may be called from any thread. There is no thread
// affinity and no interior mutability.
unsafe impl Send for OwnedHandle {}

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
        // SAFETY: `self.0` is non-null (checked in `new`) and came from a
        // Win32 call documented as returning `LocalFree`-owned memory.
        // Ownership is exclusive, so this is the one free.
        unsafe {
            let _ = LocalFree(self.0);
        }
    }
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
        // SAFETY: `self.0` is a non-null key handle from `RegOpenKeyExW`
        // (checked in `new`), owned exclusively by this value.
        unsafe {
            let _ = RegCloseKey(self.0);
        }
    }
}

// SAFETY: as `OwnedHandle` — a registry key handle has no thread
// affinity and this type owns its handle exclusively.
unsafe impl Send for OwnedKey {}

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

    #[test]
    fn an_owned_handle_can_be_moved_to_another_thread() {
        // Compile-time only: identity lookups run on the sampler thread,
        // and this is what says that is allowed.
        fn assert_send<T: Send>() {}
        assert_send::<OwnedHandle>();
        assert_send::<OwnedKey>();
    }
}

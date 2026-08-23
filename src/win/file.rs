// ============================================================================
// Module:       win::file
// Description:  Windows file operations whose semantics std does not expose.
//
// Dependencies: windows-sys (MoveFileExW)
// ============================================================================

//! File-system operations that need Windows-specific guarantees.

use std::os::windows::ffi::OsStrExt as _;
use std::path::Path;
use windows_sys::Win32::Storage::FileSystem::{
    MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
};

/// Atomically replaces `target` with `source` on the same volume.
pub fn replace(source: &Path, target: &Path) -> std::io::Result<()> {
    let source: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let target: Vec<u16> = target.as_os_str().encode_wide().chain(Some(0)).collect();
    if move_file_replace(&source, &target) {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

/// Invokes the one Win32 operation needed by [`replace`].
fn move_file_replace(source: &[u16], target: &[u16]) -> bool {
    // SAFETY: both slices are live NUL-terminated paths and outlive this
    // synchronous call. The flags request replacement and write-through.
    unsafe {
        MoveFileExW(
            source.as_ptr(),
            target.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        ) != 0
    }
}

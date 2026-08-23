// ============================================================================
// Module:       win::app_icon
// Description:  Shell executable icons converted to owned RGBA pixels.
//
// Dependencies: crate::color, crate::model, win::strings, Windows Shell/GDI
// ============================================================================

//! Reads the same small icon Explorer associates with an executable.
//!
//! Extraction runs in the sampler's identity cache, once per image path.
//! The GUI only uploads the owned pixels to its texture atlas.

use super::strings;
use crate::color::Rgb;
use crate::model::ProcessIcon;
use std::path::Path;
use windows_sys::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, SelectObject, BITMAPINFO,
    BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS,
};
use windows_sys::Win32::UI::Shell::{SHGetFileInfoW, SHFILEINFOW, SHGFI_ICON, SHGFI_SMALLICON};
use windows_sys::Win32::UI::WindowsAndMessaging::{DestroyIcon, DrawIconEx, DI_NORMAL};

/// Pixel edge used in the dense process table.
const EDGE: usize = 20;

/// Extracts an executable's shell icon as RGBA pixels.
#[must_use]
pub fn extract(path: &Path) -> Option<ProcessIcon> {
    let wide = strings::to_wide(&path.to_string_lossy());
    let mut info = SHFILEINFOW::default();
    let size = u32::try_from(std::mem::size_of::<SHFILEINFOW>()).ok()?;
    let found = shell_file_icon(&wide, &mut info, size);
    if found == 0 || info.hIcon.is_null() {
        return None;
    }

    let pixels = draw(info.hIcon);
    destroy_icon(info.hIcon);
    let mut rgba = pixels?;
    // A top-down 32-bit DIB is BGRA. Convert in-place to RGBA.
    for pixel in rgba.as_chunks_mut::<4>().0 {
        pixel.swap(0, 2);
    }
    let accent = dominant(&rgba);
    Some(ProcessIcon {
        width: EDGE,
        height: EDGE,
        rgba,
        accent,
    })
}

/// Draws an HICON into an owned top-down 32-bit DIB.
fn draw(icon: windows_sys::Win32::UI::WindowsAndMessaging::HICON) -> Option<Vec<u8>> {
    let edge = i32::try_from(EDGE).ok()?;
    let header = BITMAPINFOHEADER {
        biSize: u32::try_from(std::mem::size_of::<BITMAPINFOHEADER>()).ok()?,
        biWidth: edge,
        // Negative height makes the DIB top-down.
        biHeight: -edge,
        biPlanes: 1,
        biBitCount: 32,
        biCompression: BI_RGB,
        ..BITMAPINFOHEADER::default()
    };
    let mut bitmap_info = BITMAPINFO {
        bmiHeader: header,
        ..BITMAPINFO::default()
    };
    let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
    let bitmap = create_icon_bitmap(&mut bitmap_info, &mut bits);
    if bitmap.is_null() || bits.is_null() {
        return None;
    }
    let dc = create_memory_dc();
    if dc.is_null() {
        delete_gdi_object(bitmap);
        return None;
    }
    let old = select_gdi_object(dc, bitmap);
    let painted = paint_icon(dc, icon, edge);
    let length = EDGE.checked_mul(EDGE)?.checked_mul(4)?;
    let pixels = if painted != 0 {
        Some(copy_pixels(bits, length))
    } else {
        None
    };
    select_gdi_object(dc, old);
    delete_gdi_object(bitmap);
    delete_memory_dc(dc);
    pixels
}

/// Calls the Shell icon lookup with buffers owned by the caller.
fn shell_file_icon(path: &[u16], info: &mut SHFILEINFOW, size: u32) -> usize {
    // SAFETY: `path` is NUL-terminated and live for the call; `info` is a
    // writable out-parameter of exactly `size` bytes. The shell retains neither.
    unsafe { SHGetFileInfoW(path.as_ptr(), 0, info, size, SHGFI_ICON | SHGFI_SMALLICON) }
}

/// Releases the HICON that `SHGetFileInfoW` transferred to this module.
fn destroy_icon(icon: windows_sys::Win32::UI::WindowsAndMessaging::HICON) {
    // SAFETY: the caller passes the non-null icon handle returned by the Shell.
    let _ = unsafe { DestroyIcon(icon) };
}

/// Allocates the top-down DIB that receives the rendered icon pixels.
fn create_icon_bitmap(
    info: &mut BITMAPINFO,
    bits: &mut *mut core::ffi::c_void,
) -> windows_sys::Win32::Graphics::Gdi::HBITMAP {
    // SAFETY: a null source DC is valid; `info` and `bits` are live writable
    // out-parameters. The returned bitmap, if any, is owned by the caller.
    unsafe {
        CreateDIBSection(
            std::ptr::null_mut(),
            info,
            DIB_RGB_COLORS,
            bits,
            std::ptr::null_mut(),
            0,
        )
    }
}

/// Creates an empty memory device context owned by this module.
fn create_memory_dc() -> windows_sys::Win32::Graphics::Gdi::HDC {
    // SAFETY: a null compatible-DC source requests a memory DC.
    unsafe { CreateCompatibleDC(std::ptr::null_mut()) }
}

/// Selects a GDI object into a live device context, returning the old object.
fn select_gdi_object(
    dc: windows_sys::Win32::Graphics::Gdi::HDC,
    object: windows_sys::Win32::Graphics::Gdi::HGDIOBJ,
) -> windows_sys::Win32::Graphics::Gdi::HGDIOBJ {
    // SAFETY: the caller owns both live handles and restores the returned object.
    unsafe { SelectObject(dc, object) }
}

/// Draws a live icon into the selected square DIB.
fn paint_icon(
    dc: windows_sys::Win32::Graphics::Gdi::HDC,
    icon: windows_sys::Win32::UI::WindowsAndMessaging::HICON,
    edge: i32,
) -> i32 {
    // SAFETY: `dc` owns a selected `edge`-square bitmap; `icon` is live and no handle is retained.
    unsafe {
        DrawIconEx(
            dc,
            0,
            0,
            icon,
            edge,
            edge,
            0,
            std::ptr::null_mut(),
            DI_NORMAL,
        )
    }
}

/// Copies the initialized DIB bytes before its backing bitmap is released.
fn copy_pixels(bits: *const core::ffi::c_void, length: usize) -> Vec<u8> {
    // SAFETY: the successful `CreateDIBSection` allocated exactly `length`
    // bytes for this 32-bit `EDGE`-square bitmap, which remains alive here.
    let source = unsafe { std::slice::from_raw_parts(bits.cast::<u8>(), length) };
    source.to_vec()
}

/// Deletes a GDI bitmap once no device context has it selected.
fn delete_gdi_object(object: windows_sys::Win32::Graphics::Gdi::HGDIOBJ) {
    // SAFETY: the caller owns this live object and has restored any prior selection.
    let _ = unsafe { DeleteObject(object) };
}

/// Deletes the memory DC created by [`create_memory_dc`].
fn delete_memory_dc(dc: windows_sys::Win32::Graphics::Gdi::HDC) {
    // SAFETY: the caller owns this live memory DC and has restored its selection.
    let _ = unsafe { DeleteDC(dc) };
}

/// A weighted average of visible, chromatic pixels.
fn dominant(rgba: &[u8]) -> Rgb {
    let mut totals = [0u64; 3];
    let mut weight = 0u64;
    for pixel in rgba.as_chunks::<4>().0 {
        let alpha = u64::from(pixel[3]);
        let spread = u64::from(pixel[0].max(pixel[1]).max(pixel[2]))
            .saturating_sub(u64::from(pixel[0].min(pixel[1]).min(pixel[2])));
        let contribution = alpha.saturating_mul(spread.max(24));
        for channel in 0..3 {
            totals[channel] = totals[channel]
                .saturating_add(u64::from(pixel[channel]).saturating_mul(contribution));
        }
        weight = weight.saturating_add(contribution);
    }
    if weight == 0 {
        return Rgb::new(120, 140, 170);
    }
    Rgb::new(
        (totals[0] / weight).min(255) as u8,
        (totals[1] / weight).min(255) as u8,
        (totals[2] / weight).min(255) as u8,
    )
}

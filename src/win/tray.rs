// ============================================================================
// Module:       win::tray
// Description:  The notification-area icon: its hidden message window, the
//               HICON built from the pixels crate::tray rasterises, and the
//               clicks that come back.
//
// Dependencies: windows-sys (Shell notification icons, GDI bitmaps, menus,
//               window messages); win::strings; crate::tray for the picture
// ============================================================================

//! The tray icon's handles.
//!
//! [`crate::tray`] decides what the icon looks like and hands over RGBA
//! bytes; everything here is the Win32 apparatus that gets those bytes
//! into the notification area and the user's clicks back out.
//!
//! ## Why there is a window nobody can see
//!
//! `Shell_NotifyIcon` has no callback of its own. It delivers every click
//! as a window message, so an app that wants a tray icon needs a window
//! to receive them — and the app's real window will not do: it belongs to
//! `winit`, whose window procedure is not ours to extend.
//!
//! So this creates its own, with `HWND_MESSAGE` as its parent. That makes
//! it *message-only*: never painted, never in the taskbar, never in
//! Alt-Tab, and cheap. It exists to own a window procedure.
//!
//! It is created on the thread that runs the event loop, which is what
//! makes the whole thing work: window messages are delivered to the
//! thread that owns the window, and `winit` is already pumping that
//! thread's queue. No extra thread, no channel, no synchronisation —
//! `winit`'s own message pump dispatches to [`window_proc`] as a side
//! effect of running the app.
//!
//! ## What the icon costs to update
//!
//! Every update builds a fresh `HICON` — there is no "repaint this icon"
//! call, only "here is a different one". That is a DIB section, a mask,
//! `CreateIconIndirect`, and a `Shell_NotifyIcon`, and the old icon has
//! to outlive the call that replaces it, which is why [`Tray`] holds it
//! until the new one is installed.
//!
//! None of that is expensive, but none of it should happen sixty times a
//! second either. The gate is [`crate::tray::Face`]: the caller compares
//! the face it would draw against the one it last drew, and only calls
//! [`Tray::show`] when the picture actually differs. On an idle machine
//! that is approximately never, and a busy one still only changes in five
//! steps.

use super::strings;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    CreateBitmap, CreateDIBSection, DeleteObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
    DIB_RGB_COLORS, HBITMAP, HGDIOBJ,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY,
    NOTIFYICONDATAW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreateIconIndirect, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyIcon,
    DestroyMenu, DestroyWindow, GetCursorPos, GetForegroundWindow, IsIconic, PostMessageW,
    RegisterClassW, SetForegroundWindow, ShowWindow, TrackPopupMenu, HICON, HMENU, HWND_MESSAGE,
    ICONINFO, MF_SEPARATOR, MF_STRING, SW_MINIMIZE, SW_RESTORE, TPM_RETURNCMD, TPM_RIGHTBUTTON,
    WM_APP, WM_CLOSE, WM_LBUTTONDBLCLK, WM_LBUTTONUP, WM_NULL, WM_RBUTTONUP, WNDCLASSW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSMICON};

/// The window class the message-only window registers under.
const CLASS_NAME: &str = "RustamanTrayHost";

/// The message the shell sends this window for every click on the icon.
///
/// `WM_APP` and above are reserved for an application's own use, which is
/// exactly what this is — the value only has to be unique within this
/// window's procedure.
const TRAY_MESSAGE: u32 = WM_APP + 1;

/// Identifies the icon within this window. One icon, so any constant.
const ICON_ID: u32 = 1;

/// "Open Rustaman" in the right-click menu.
const MENU_OPEN: usize = 1;

/// "Exit" in the right-click menu.
const MENU_EXIT: usize = 2;

// The app's real window, for the click handler to act on.
//
// A thread-local rather than a field, because the window procedure is
// called by Windows with nothing of ours in hand but the message. Both
// windows live on the event-loop thread, so this is read on the same
// thread that wrote it and needs no synchronisation — and unlike
// `GWLP_USERDATA` it needs no pointer round-trip to get at.
thread_local! {
    static OWNER: std::cell::Cell<isize> = const { std::cell::Cell::new(0) };
}

/// An `HICON` this process owns, destroyed when it goes out of scope.
struct OwnedIcon(HICON);

impl Drop for OwnedIcon {
    fn drop(&mut self) {
        destroy_icon(self.0);
    }
}

/// A GDI bitmap, deleted when it goes out of scope.
struct OwnedBitmap(HGDIOBJ);

impl Drop for OwnedBitmap {
    fn drop(&mut self) {
        delete_gdi_object(self.0);
    }
}

/// A popup menu, destroyed when it goes out of scope.
///
/// `TrackPopupMenu` does not consume the menu, and an early return
/// between creating one and tracking it would otherwise leak a kernel
/// object per right-click.
struct OwnedMenu(HMENU);

impl Drop for OwnedMenu {
    fn drop(&mut self) {
        destroy_menu(self.0);
    }
}

/// The notification-area icon, removed when it goes out of scope.
pub struct Tray {
    /// The message-only window that receives the shell's clicks.
    window: HWND,
    /// The icon currently installed, held because `Shell_NotifyIcon` does
    /// not copy it — destroying it while the shell is drawing it leaves a
    /// blank square in the tray.
    icon: Option<OwnedIcon>,
    /// Whether `NIM_ADD` has succeeded, which decides whether the next
    /// update adds or modifies.
    added: bool,
}

impl Tray {
    /// Creates the message-only window and prepares an empty icon.
    ///
    /// `owner` is the numeric value of the app's real `HWND`, which the
    /// click handler shows, hides and closes. Returns `None` if the
    /// window could not be created, which costs the tray icon and
    /// nothing else — every caller treats the tray as optional, because
    /// an app that refused to start over a notification icon would be a
    /// worse app than one without one.
    #[must_use]
    pub fn create(owner: isize) -> Option<Self> {
        set_owner(owner);
        let class = strings::to_wide(CLASS_NAME);
        let instance = module_handle();
        // A second registration of the same class fails, which is fine
        // and expected if a previous Tray was dropped: the class stays
        // registered for the life of the process.
        register_class(&class, instance);
        let window = create_message_window(&class, instance);
        if window.is_null() {
            return None;
        }
        Some(Self {
            window,
            icon: None,
            added: false,
        })
    }

    /// Installs `pixels` as the icon, with `tooltip` on hover.
    ///
    /// `pixels` is `edge`-square RGBA from [`crate::tray::rasterise`].
    /// Returns whether the shell accepted it; a refusal is not worth
    /// reporting anywhere, because the failure mode is a missing icon and
    /// the user can see that.
    pub fn show(&mut self, pixels: &[u8], edge: u32, tooltip: &str) -> bool {
        let Some(icon) = icon_from_rgba(pixels, edge) else {
            return false;
        };
        let mut data = notify_icon_data(self.window, tooltip);
        data.hIcon = icon.0;
        let message = if self.added { NIM_MODIFY } else { NIM_ADD };
        if !notify_icon(message, &mut data) {
            return false;
        }
        self.added = true;
        // Replaces the previous icon only once the shell has taken the
        // new one, so the handle it was drawing stays alive until it is
        // no longer drawing it.
        self.icon = Some(icon);
        true
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        if self.added {
            let mut data = notify_icon_data(self.window, "");
            notify_icon(NIM_DELETE, &mut data);
        }
        destroy_window(self.window);
    }
}

/// The window procedure Windows calls for the message-only window.
///
/// `extern "system"` because that is the calling convention Win32
/// expects; a mismatch here corrupts the stack rather than failing to
/// compile.
// SAFETY: Windows calls this only for the window registered with it, and every
// argument is the message it is dispatching. The body performs no unsafe
// operation of its own — it forwards to safe code immediately.
unsafe extern "system" fn window_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    dispatch(window, message, wparam, lparam)
}

/// One message, handled in safe code.
fn dispatch(window: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if message != TRAY_MESSAGE {
        return default_window_proc(window, message, wparam, lparam);
    }
    // For a tray notification the mouse message is in the low word of
    // the lparam, not in the wparam — which holds the icon's id.
    let click = (lparam as u32) & 0xFFFF;
    match click {
        WM_LBUTTONUP | WM_LBUTTONDBLCLK => toggle_owner(),
        WM_RBUTTONUP => show_menu(window),
        // Every other mouse message over the icon — moves, middle
        // clicks, the button-down halves — is deliberately ignored
        // rather than matched, because acting on a button-down and again
        // on its button-up fires twice for one click.
        _ => {}
    }
    0
}

/// Shows, raises or minimises the app's window.
///
/// Minimised or behind something goes to the front; already in front
/// goes down. That is what a monitor's tray icon is for — it is the
/// control you reach for *because* the window is not where you can see
/// it.
fn toggle_owner() {
    let owner = owner_window();
    if owner.is_null() {
        return;
    }
    if is_iconic(owner) {
        show_window(owner, SW_RESTORE);
        set_foreground(owner);
    } else if foreground_window() == owner {
        show_window(owner, SW_MINIMIZE);
    } else {
        set_foreground(owner);
    }
}

/// Puts the right-click menu on screen and acts on the choice.
fn show_menu(window: HWND) {
    let Some(menu) = popup_menu() else {
        return;
    };
    let open = strings::to_wide("Open Rustaman");
    let exit = strings::to_wide("Exit");
    append_item(menu.0, MF_STRING, MENU_OPEN, &open);
    append_item(menu.0, MF_SEPARATOR, 0, &[0]);
    append_item(menu.0, MF_STRING, MENU_EXIT, &exit);

    let Some(point) = cursor_position() else {
        return;
    };
    // Documented requirement: without this the menu does not dismiss
    // when the user clicks away from it, because the foreground window
    // is somebody else's and the menu never loses activation.
    set_foreground(window);
    let choice = track_menu(menu.0, point, window);
    // The other half of the same workaround.
    post_message(window, WM_NULL, 0, 0);

    match choice as usize {
        MENU_OPEN => open_owner(),
        MENU_EXIT => close_owner(),
        // Zero is "dismissed without choosing", which is most of the
        // time and means exactly nothing should happen.
        _ => {}
    }
}

/// Restores and raises the app's window, whatever state it is in.
fn open_owner() {
    let owner = owner_window();
    if owner.is_null() {
        return;
    }
    if is_iconic(owner) {
        show_window(owner, SW_RESTORE);
    }
    set_foreground(owner);
}

/// Asks the app's window to close, the same way its own close button
/// does.
///
/// `WM_CLOSE` rather than anything more direct: that is the path that
/// runs `eframe`'s shutdown and saves the config on the way out. Ending
/// the process here instead would lose every setting changed since
/// launch.
fn close_owner() {
    let owner = owner_window();
    if owner.is_null() {
        return;
    }
    post_message(owner, WM_CLOSE, 0, 0);
}

/// The app's window handle, as recorded at creation.
fn owner_window() -> HWND {
    OWNER.with(|owner| owner.get()) as HWND
}

/// Records the app's window handle for the click handler.
fn set_owner(owner: isize) {
    OWNER.with(|cell| cell.set(owner));
}

/// A filled `NOTIFYICONDATAW` for this window's one icon.
fn notify_icon_data(window: HWND, tooltip: &str) -> NOTIFYICONDATAW {
    let mut data = zeroed_notify_icon();
    data.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
    data.hWnd = window;
    data.uID = ICON_ID;
    data.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
    data.uCallbackMessage = TRAY_MESSAGE;
    write_tooltip(&mut data.szTip, tooltip);
    data
}

/// Copies `text` into a fixed tooltip buffer, truncated and
/// NUL-terminated.
///
/// Truncation is by UTF-16 unit rather than by character. The tooltip is
/// ASCII in every language this ships in, and a split surrogate at the
/// end of a 128-unit buffer costs one replacement glyph in a hover
/// tooltip — which is not worth carrying a grapheme library for.
fn write_tooltip(buffer: &mut [u16; 128], text: &str) {
    let wide = strings::to_wide(text);
    let room = buffer.len() - 1;
    for (slot, value) in buffer.iter_mut().zip(wide.iter().take(room)) {
        *slot = *value;
    }
    // `to_wide` NUL-terminates, so a string that fits brings its own
    // terminator; one that does not gets this.
    if let Some(last) = buffer.get_mut(room) {
        *last = 0;
    }
}

/// An `HICON` built from `edge`-square RGBA pixels.
///
/// The colour bitmap is a top-down 32-bit DIB section, which is the only
/// form that carries an alpha channel through `CreateIconIndirect` —
/// without it the plate's rounded corners come back as black squares.
/// The mask is required even so, and is all zeroes: with a 32-bit colour
/// bitmap the alpha channel does the masking and an all-clear mask means
/// "consult it".
fn icon_from_rgba(pixels: &[u8], edge: u32) -> Option<OwnedIcon> {
    let length = (edge as usize).checked_mul(edge as usize)?.checked_mul(4)?;
    if pixels.len() < length {
        return None;
    }
    let mut bits = std::ptr::null_mut();
    let mut info = bitmap_info(edge);
    let color = OwnedBitmap(create_dib_section(&mut info, &mut bits).cast());
    if color.0.is_null() || bits.is_null() {
        return None;
    }
    write_bgra(bits, pixels, length);

    let mask_bits = vec![0u8; mask_length(edge)];
    let mask = OwnedBitmap(create_mask_bitmap(edge, &mask_bits).cast());
    if mask.0.is_null() {
        return None;
    }

    let icon = create_icon(color.0.cast(), mask.0.cast());
    if icon.is_null() {
        return None;
    }
    // The bitmaps are copied into the icon; both are dropped here.
    Some(OwnedIcon(icon))
}

/// Bytes in a monochrome mask bitmap of `edge` square.
///
/// Rows of a GDI bitmap are padded to a two-byte boundary, which at one
/// bit per pixel is what decides the row's length.
fn mask_length(edge: u32) -> usize {
    let row = ((edge as usize).div_ceil(16)) * 2;
    row * edge as usize
}

/// A `BITMAPINFO` describing a top-down 32-bit image of `edge` square.
///
/// The negative height is what makes it top-down, matching the row order
/// [`crate::tray::rasterise`] produces. A positive height would put the
/// icon on its head.
fn bitmap_info(edge: u32) -> BITMAPINFO {
    let mut info: BITMAPINFO = zeroed_bitmap_info();
    info.bmiHeader = BITMAPINFOHEADER {
        biSize: size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: edge as i32,
        biHeight: -(edge as i32),
        biPlanes: 1,
        biBitCount: 32,
        biCompression: BI_RGB,
        biSizeImage: 0,
        biXPelsPerMeter: 0,
        biYPelsPerMeter: 0,
        biClrUsed: 0,
        biClrImportant: 0,
    };
    info
}

/// Writes RGBA pixels into a DIB section's BGRA storage.
///
/// The swizzle is the whole reason this is not a `copy_from_slice`:
/// [`crate::tray`] produces RGBA because that is what the rest of the
/// crate speaks, and a DIB section stores blue first.
fn write_bgra(bits: *mut core::ffi::c_void, pixels: &[u8], length: usize) {
    let destination = bgra_slice(bits, length);
    let (out, _) = destination.as_chunks_mut::<4>();
    let (source, _) = pixels.as_chunks::<4>();
    for (slot, pixel) in out.iter_mut().zip(source) {
        let [r, g, b, a] = *pixel;
        *slot = [b, g, r, a];
    }
}

/// The DIB section's storage as a slice.
fn bgra_slice<'a>(bits: *mut core::ffi::c_void, length: usize) -> &'a mut [u8] {
    // SAFETY: `CreateDIBSection` returned this pointer and allocated exactly
    // `length` bytes for it — the caller computed `length` from the same edge
    // the bitmap was created with. The slice is used and dropped before the
    // bitmap is deleted.
    unsafe { std::slice::from_raw_parts_mut(bits.cast::<u8>(), length) }
}

/// A zeroed `NOTIFYICONDATAW`.
fn zeroed_notify_icon() -> NOTIFYICONDATAW {
    // SAFETY: every field is an integer, a handle, or an array of them, and a
    // union of the same — all-zero is a valid value for the struct, and is the
    // documented starting point before setting `cbSize` and the flags.
    unsafe { std::mem::zeroed() }
}

/// A zeroed `BITMAPINFO`.
fn zeroed_bitmap_info() -> BITMAPINFO {
    // SAFETY: a header of integers followed by a colour table, for which
    // all-zero is valid and is what a 32-bit BI_RGB image wants — it has no
    // palette.
    unsafe { std::mem::zeroed() }
}

/// The module handle this process was loaded from.
fn module_handle() -> windows_sys::Win32::Foundation::HMODULE {
    // SAFETY: a null name asks for the handle of the executable itself, which
    // is always loaded and needs no release.
    unsafe { GetModuleHandleW(std::ptr::null()) }
}

/// Registers the window class, returning whether it was accepted.
fn register_class(class: &[u16], instance: windows_sys::Win32::Foundation::HMODULE) -> u16 {
    let mut description: WNDCLASSW = zeroed_class();
    description.lpfnWndProc = Some(window_proc);
    description.hInstance = instance;
    description.lpszClassName = class.as_ptr();
    // SAFETY: `description` is live and fully initialised, and `class` outlives
    // the call and stays valid for as long as the class is registered — it is
    // owned by the caller across `create_message_window`.
    unsafe { RegisterClassW(&description) }
}

/// A zeroed `WNDCLASSW`.
fn zeroed_class() -> WNDCLASSW {
    // SAFETY: pointers and integers throughout; all-zero means "no icon, no
    // cursor, no background, no menu", which is what a message-only window
    // wants.
    unsafe { std::mem::zeroed() }
}

/// Creates the message-only window.
fn create_message_window(class: &[u16], instance: windows_sys::Win32::Foundation::HMODULE) -> HWND {
    // SAFETY: `class` is a live NUL-terminated name for a class registered on
    // this thread. `HWND_MESSAGE` as the parent is the documented request for a
    // message-only window, which is why every geometry argument is zero.
    unsafe {
        CreateWindowExW(
            0,
            class.as_ptr(),
            std::ptr::null(),
            0,
            0,
            0,
            0,
            0,
            HWND_MESSAGE,
            std::ptr::null_mut(),
            instance,
            std::ptr::null(),
        )
    }
}

/// Destroys a window this process created.
fn destroy_window(window: HWND) {
    // SAFETY: the caller owns this live window and it was created on this
    // thread, which is where `DestroyWindow` must be called from.
    unsafe { DestroyWindow(window) };
}

/// The default handling for a message this window does not want.
fn default_window_proc(window: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // SAFETY: these are the arguments Windows just passed to the window
    // procedure, forwarded unchanged, which is exactly the documented contract.
    unsafe { DefWindowProcW(window, message, wparam, lparam) }
}

/// Adds, modifies or removes the notification icon.
fn notify_icon(message: u32, data: &mut NOTIFYICONDATAW) -> bool {
    // SAFETY: `data` is a live, fully initialised structure whose `cbSize`
    // states its own size, which is how the shell decides which version of the
    // layout it was given.
    unsafe { Shell_NotifyIconW(message, data) != 0 }
}

/// Creates a top-down 32-bit DIB section, handing back its storage.
fn create_dib_section(info: &mut BITMAPINFO, bits: &mut *mut core::ffi::c_void) -> HBITMAP {
    // SAFETY: a null DC is valid and asks for a device-independent bitmap.
    // `info` and `bits` are live writable out-parameters, and no file mapping is
    // supplied, so the last two arguments are null and zero.
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

/// Creates the monochrome mask bitmap from an all-clear buffer.
fn create_mask_bitmap(edge: u32, bits: &[u8]) -> HBITMAP {
    // SAFETY: `bits` is live for the call and `mask_length` sized it for a
    // one-bit-per-pixel bitmap of exactly these dimensions, which is what the
    // plane and bit-count arguments declare.
    unsafe {
        CreateBitmap(
            edge as i32,
            edge as i32,
            1,
            1,
            bits.as_ptr().cast::<core::ffi::c_void>(),
        )
    }
}

/// Builds an icon from a colour bitmap and its mask.
fn create_icon(color: HBITMAP, mask: HBITMAP) -> HICON {
    let info = ICONINFO {
        fIcon: 1,
        xHotspot: 0,
        yHotspot: 0,
        hbmMask: mask,
        hbmColor: color,
    };
    // SAFETY: `info` is live and both bitmaps are live and of the same
    // dimensions. `CreateIconIndirect` copies them, so the caller remains their
    // owner and frees them afterwards.
    unsafe { CreateIconIndirect(&info) }
}

/// Releases an icon this process created.
fn destroy_icon(icon: HICON) {
    // SAFETY: the caller owns this live icon, created by `CreateIconIndirect`
    // and no longer installed.
    unsafe { DestroyIcon(icon) };
}

/// Deletes a GDI object this process created.
fn delete_gdi_object(object: HGDIOBJ) {
    // SAFETY: the caller owns this live object and it is not selected into any
    // device context.
    unsafe { DeleteObject(object) };
}

/// An empty popup menu.
fn popup_menu() -> Option<OwnedMenu> {
    // SAFETY: takes no arguments and returns a new menu or null.
    let menu = unsafe { CreatePopupMenu() };
    (!menu.is_null()).then_some(OwnedMenu(menu))
}

/// Appends one item to a menu.
fn append_item(menu: HMENU, flags: u32, id: usize, text: &[u16]) {
    // SAFETY: `menu` is live and owned by the caller; `text` is a live
    // NUL-terminated string for the duration of the call, which is all
    // `AppendMenuW` requires — it copies the label.
    unsafe { AppendMenuW(menu, flags, id, text.as_ptr()) };
}

/// Destroys a menu this process created.
fn destroy_menu(menu: HMENU) {
    // SAFETY: the caller owns this live menu and it is no longer displayed.
    unsafe { DestroyMenu(menu) };
}

/// Displays the menu and returns the chosen command, or zero.
fn track_menu(menu: HMENU, at: POINT, window: HWND) -> i32 {
    // SAFETY: `menu` and `window` are live and owned by this thread.
    // `TPM_RETURNCMD` makes this return the command rather than posting it,
    // which is why no `WM_COMMAND` handling is needed; the final argument is
    // optional and null.
    unsafe {
        TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON,
            at.x,
            at.y,
            0,
            window,
            std::ptr::null(),
        )
    }
}

/// The pointer's position in screen coordinates.
fn cursor_position() -> Option<POINT> {
    let mut point = POINT { x: 0, y: 0 };
    // SAFETY: `point` is a live writable out-parameter.
    let ok = unsafe { GetCursorPos(&mut point) };
    (ok != 0).then_some(point)
}

/// Whether a window is minimised.
fn is_iconic(window: HWND) -> bool {
    // SAFETY: a stale or invalid handle is refused by the call, which then
    // reports false.
    unsafe { IsIconic(window) != 0 }
}

/// The window currently in the foreground, which may be another app's.
fn foreground_window() -> HWND {
    // SAFETY: takes no arguments and returns a handle or null.
    unsafe { GetForegroundWindow() }
}

/// Brings a window to the front.
fn set_foreground(window: HWND) {
    // SAFETY: a stale or invalid handle is refused by the call. Windows may
    // also decline the request outright under its foreground-activation rules,
    // which is a refusal rather than an error.
    unsafe { SetForegroundWindow(window) };
}

/// Shows, restores or minimises a window.
fn show_window(window: HWND, command: i32) {
    // SAFETY: a stale or invalid handle is refused by the call.
    unsafe { ShowWindow(window, command) };
}

/// Posts a message to a window without waiting for it to be handled.
fn post_message(window: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) {
    // SAFETY: a stale or invalid handle is refused by the call. Posting rather
    // than sending is what keeps this off the caller's stack — `close_owner`
    // must not block inside a menu.
    unsafe { PostMessageW(window, message, wparam, lparam) };
}

/// The edge, in pixels, the shell wants a notification icon to be.
///
/// Asked rather than assumed: it is 16 at 100% scaling, 24 at 150% and
/// 32 at 200%, and handing the shell an icon of the wrong size makes it
/// scale one — which on a 16-pixel mark is the difference between five
/// legible bars and a smear.
#[must_use]
pub fn icon_edge() -> u32 {
    let edge = small_icon_metric();
    // A metric of zero means the call failed. 16 is the historical
    // default and is never wrong enough to be worth failing over.
    if edge <= 0 {
        return 16;
    }
    edge as u32
}

/// `GetSystemMetrics(SM_CXSMICON)`.
fn small_icon_metric() -> i32 {
    // SAFETY: the argument is one of the documented metric indices and the
    // call only reads a system value.
    unsafe { GetSystemMetrics(SM_CXSMICON) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mask_row_is_padded_to_two_bytes() {
        // One bit per pixel, rows padded to a 16-bit boundary.
        assert_eq!(mask_length(16), 2 * 16, "16 px is exactly one padded row");
        assert_eq!(mask_length(32), 4 * 32);
        assert_eq!(mask_length(24), 4 * 24, "24 px pads up to four bytes");
    }

    #[test]
    fn the_bitmap_header_is_top_down_and_32_bit() {
        let info = bitmap_info(16);
        assert_eq!(info.bmiHeader.biWidth, 16);
        assert_eq!(
            info.bmiHeader.biHeight, -16,
            "a positive height would stand the icon on its head"
        );
        assert_eq!(info.bmiHeader.biBitCount, 32, "an icon needs its alpha");
    }

    #[test]
    fn a_tooltip_that_fits_is_written_whole_and_terminated() {
        let mut buffer = [0xFFFFu16; 128];
        write_tooltip(&mut buffer, "CPU 23%");
        let text: Vec<u16> = buffer
            .iter()
            .copied()
            .take_while(|unit| *unit != 0)
            .collect();
        assert_eq!(String::from_utf16_lossy(&text), "CPU 23%");
    }

    #[test]
    fn a_tooltip_that_does_not_fit_is_truncated_and_still_terminated() {
        let mut buffer = [0xFFFFu16; 128];
        write_tooltip(&mut buffer, &"x".repeat(400));
        assert_eq!(
            buffer.last().copied(),
            Some(0),
            "an unterminated tooltip is read past its buffer by the shell"
        );
        let text: Vec<u16> = buffer
            .iter()
            .copied()
            .take_while(|unit| *unit != 0)
            .collect();
        assert_eq!(text.len(), 127, "127 units and the terminator");
    }

    #[test]
    fn the_icon_refuses_pixels_that_are_too_few_for_its_edge() {
        // Guards the slice the DIB section is written through: an edge
        // that disagreed with the buffer would write past it.
        assert!(
            icon_from_rgba(&[0u8; 16 * 16 * 4], 32).is_none(),
            "a 32px icon cannot be built from 16px of pixels"
        );
    }

    #[test]
    fn an_icon_is_built_and_released_without_leaking_its_bitmaps() {
        let pixels = crate::tray::rasterise(
            crate::tray::face(0.5, crate::theme::Catalog::load().get(None)),
            16,
        );
        let before = crate::win::app_icon::gdi_objects();
        for _ in 0..64 {
            let icon = icon_from_rgba(&pixels, 16);
            assert!(icon.is_some(), "the icon should build from valid pixels");
        }
        let after = crate::win::app_icon::gdi_objects();
        assert!(
            after <= before + 8,
            "GDI objects grew from {before} to {after} over 64 icons"
        );
    }
}

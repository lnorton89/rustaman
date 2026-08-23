// ============================================================================
// Module:       win::strings
// Description:  Marshalling between Rust strings and the UTF-16 Win32 speaks,
//               including the two buffer conventions its calls use.
//
// Dependencies: windows-sys (UNICODE_STRING), std::ffi::OsString
// ============================================================================

//! Rust strings in, UTF-16 out, and back.
//!
//! ## The two conventions, and why they need separate functions
//!
//! Win32 returns wide strings in two shapes and confusing them is a
//! reliable source of either a truncated string or a read past the end of
//! a buffer:
//!
//! - **NUL-terminated**, with the length reported separately or not at
//!   all — `QueryFullProcessImageNameW`, `GetWindowTextW`, registry
//!   values. [`from_wide_nul`] stops at the first NUL.
//! - **Counted**, with an explicit length and *no* guaranteed
//!   terminator — the `UNICODE_STRING` that `NtQuerySystemInformation`
//!   returns each process's image name in. [`from_unicode_string_in`]
//!   uses the length, validates it against its owner, and never looks for
//!   a NUL.
//!
//! Reading a counted string as if it were terminated walks off the end of
//! the process-information buffer into the next entry; reading a
//! terminated one by a stale length returns whatever the buffer held
//! before. Both are quiet failures, which is why they are two functions
//! with two names rather than one with a flag.
//!
//! ## Lossy on the way out
//!
//! A path or a process name that is not valid UTF-16 still has to be
//! shown — the user needs to see the row, and refusing to display a
//! process because its name has an unpaired surrogate in it would hide
//! exactly the sort of thing worth noticing. So the conversions here are
//! lossy, and a malformed sequence becomes U+FFFD rather than an error.

use windows_sys::Win32::Foundation::UNICODE_STRING;

/// Converts a Rust string to a NUL-terminated UTF-16 buffer for passing
/// *into* Win32.
///
/// The returned `Vec` must outlive the call it is passed to; every caller
/// here binds it to a local first, which is what stops the classic
/// `as_ptr()` on a temporary that is dropped before the call.
#[must_use]
pub fn to_wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Reads a NUL-terminated wide string out of a buffer Win32 filled.
///
/// `buffer` is the whole buffer, not the used part: this stops at the
/// first NUL, so an over-long buffer is fine and a call that reported no
/// length is still handled. A buffer with no NUL in it at all is read to
/// its end rather than past it.
#[must_use]
pub fn from_wide_nul(buffer: &[u16]) -> String {
    let end = buffer
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(buffer.len());
    String::from_utf16_lossy(buffer.get(..end).unwrap_or(&[]))
}

/// Reads a counted string only when it lies wholly inside `owner`.
#[must_use]
pub fn from_unicode_string_in(value: &UNICODE_STRING, owner: &[u8]) -> Option<String> {
    if value.Buffer.is_null() || value.Length == 0 {
        return Some(String::new());
    }
    if !value.Length.is_multiple_of(2) {
        return None;
    }
    let start = value.Buffer as usize;
    let owner_start = owner.as_ptr() as usize;
    let owner_end = owner_start.checked_add(owner.len())?;
    let string_end = start.checked_add(usize::from(value.Length))?;
    if start < owner_start
        || string_end > owner_end
        || !start.is_multiple_of(std::mem::align_of::<u16>())
    {
        return None;
    }
    let units = usize::from(value.Length / 2);
    // SAFETY: pointer alignment and the complete byte range were checked
    // against the live owning allocation above.
    let slice = unsafe { std::slice::from_raw_parts(value.Buffer, units) };
    Some(String::from_utf16_lossy(slice))
}

/// Trims a buffer to a length a Win32 call reported, guarding against a
/// length that exceeds the buffer.
///
/// Several calls report a character count through an out-parameter, and a
/// caller that indexes with it directly trusts a number that came from
/// outside the program. This clamps instead, so a driver or a shim
/// reporting nonsense costs a truncated string rather than a panic.
#[must_use]
pub fn reported_slice(buffer: &[u16], reported: u32) -> &[u16] {
    let length = usize::try_from(reported).unwrap_or(0).min(buffer.len());
    buffer.get(..length).unwrap_or(&[])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_round_trip_preserves_the_text() {
        for text in [
            "",
            "chrome.exe",
            "C:\\Program Files\\App\\a.exe",
            "日本語",
            "emoji 🦀",
        ] {
            let wide = to_wide(text);
            assert_eq!(
                from_wide_nul(&wide),
                text,
                "{text:?} did not survive the round trip"
            );
        }
    }

    #[test]
    fn the_terminator_is_not_part_of_the_string() {
        let wide = to_wide("ab");
        assert_eq!(wide.len(), 3, "two units and a NUL");
        assert_eq!(wide.last(), Some(&0));
        assert_eq!(from_wide_nul(&wide), "ab");
    }

    #[test]
    fn an_oversized_buffer_stops_at_the_first_nul() {
        // The common shape: a MAX_PATH buffer with a short path in it and
        // whatever the stack held after that.
        let mut buffer = vec![0u16; 260];
        for (slot, unit) in buffer.iter_mut().zip("C:\\a.exe".encode_utf16()) {
            *slot = unit;
        }
        // Garbage past the terminator, as a real call would leave.
        if let Some(tail) = buffer.get_mut(20..) {
            tail.iter_mut().for_each(|unit| *unit = u16::from(b'X'));
        }
        assert_eq!(from_wide_nul(&buffer), "C:\\a.exe");
    }

    #[test]
    fn a_buffer_with_no_terminator_is_read_to_its_end_not_past_it() {
        let buffer: Vec<u16> = "abc".encode_utf16().collect();
        assert_eq!(from_wide_nul(&buffer), "abc");
    }

    #[test]
    fn an_empty_buffer_yields_an_empty_string() {
        assert_eq!(from_wide_nul(&[]), "");
        assert_eq!(from_wide_nul(&[0]), "");
    }

    #[test]
    fn malformed_utf16_is_replaced_rather_than_rejected() {
        // An unpaired surrogate. A process whose name contains one still
        // has to appear in the list — hiding it would conceal exactly the
        // sort of thing worth noticing.
        let buffer = [0xd800u16, u16::from(b'a'), 0];
        let text = from_wide_nul(&buffer);
        assert!(
            text.contains('\u{fffd}'),
            "a lone surrogate should become the replacement character, \
             got {text:?}"
        );
        assert!(text.ends_with('a'), "and the rest should survive");
    }

    #[test]
    fn a_reported_length_past_the_buffer_is_clamped() {
        // The length comes from outside the program; trusting it would
        // turn a misbehaving shim into a panic.
        let buffer = [1u16, 2, 3];
        assert_eq!(reported_slice(&buffer, 2), &[1, 2]);
        assert_eq!(reported_slice(&buffer, 99), &[1, 2, 3]);
        assert_eq!(reported_slice(&buffer, 0), &[] as &[u16]);
    }

    #[test]
    fn a_counted_string_uses_its_length_and_ignores_any_nul() {
        // The `UNICODE_STRING` case: `Length` is in bytes, and there is
        // no guaranteed terminator. Reading it as terminated would run
        // into the next entry of the process-information buffer.
        #[repr(C, align(2))]
        struct AlignedBytes([u8; 40]);
        let mut backing = AlignedBytes([0; 40]);
        for (index, unit) in "chrome.exe\0garbage".encode_utf16().enumerate() {
            let offset = index * 2;
            backing.0[offset..offset + 2].copy_from_slice(&unit.to_ne_bytes());
        }
        let value = UNICODE_STRING {
            Length: 20, // ten characters, in bytes
            MaximumLength: 40,
            Buffer: backing.0.as_mut_ptr().cast(),
        };
        let text = from_unicode_string_in(&value, &backing.0);
        assert_eq!(
            text.as_deref(),
            Some("chrome.exe"),
            "the byte length must be halved to a unit count, or this \
             reads twice as far as the string runs"
        );
    }

    #[test]
    fn an_empty_or_null_counted_string_is_handled() {
        let empty = UNICODE_STRING {
            Length: 0,
            MaximumLength: 0,
            Buffer: std::ptr::null_mut(),
        };
        assert_eq!(from_unicode_string_in(&empty, &[]).as_deref(), Some(""));
    }

    #[test]
    fn a_counted_string_outside_its_owner_is_rejected() {
        let owner = vec![0u8; 16];
        let outside: Vec<u16> = "elsewhere".encode_utf16().collect();
        let value = UNICODE_STRING {
            Length: u16::try_from(outside.len() * 2).unwrap_or(u16::MAX),
            MaximumLength: u16::try_from(outside.len() * 2).unwrap_or(u16::MAX),
            Buffer: outside.as_ptr().cast_mut(),
        };
        assert_eq!(from_unicode_string_in(&value, &owner), None);
    }

    #[test]
    fn a_counted_string_crossing_its_owners_end_is_rejected() {
        #[repr(C, align(2))]
        struct AlignedBytes([u8; 8]);
        let mut owner = AlignedBytes([0; 8]);
        let value = UNICODE_STRING {
            Length: 4,
            MaximumLength: 4,
            Buffer: owner.0.as_mut_ptr().wrapping_add(6).cast(),
        };
        assert_eq!(from_unicode_string_in(&value, &owner.0), None);
    }
}

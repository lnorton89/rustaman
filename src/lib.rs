// ============================================================================
// Module:       rustaman (library crate root)
// Description:  Module graph: the portable core, the Win32 layer it is fed by,
//               and the desktop front end built on top of both.
//
// Dependencies: None; declarations only. See the individual modules.
// ============================================================================

//! A modern Windows task manager.
//!
//! The crate is in three layers, and the split is what keeps it testable.
//!
//! [`win`] is the only place that talks to Win32. Every call is wrapped in
//! a safe leaf function that takes Rust arguments and returns `Option` or
//! `Result` of a safe type, so nothing above it contains the word
//! `unsafe`. It is `cfg(windows)`, and so is everything that depends on
//! it — [`engine`] and [`gui`].
//!
//! The portable core is the rest: [`model`] is the shape of a sample,
//! [`theme`] is the palette catalog, and [`format`], [`color`],
//! [`config`], and [`brand`] are the maths and data around them. None of
//! it names a Windows type, which means `cargo test` exercises the
//! sorting, the rate arithmetic, the theme contrast checks, and the
//! config parsing on any machine — including the ones CI runs the
//! non-Windows job on. Only the ~20% that genuinely needs a Windows
//! kernel to answer is untestable off-platform, rather than all of it.
//!
//! [`engine`] is the seam: a sampler thread calls into [`win`] on an
//! interval and publishes [`model::Snapshot`]s that [`gui`] reads. The
//! UI thread never makes a system call, which is why a machine with
//! 400 processes and a busy disk does not drop frames.
//!
//! See `docs/ARCHITECTURE.md` for the module map, `docs/WINDOWS_APIS.md`
//! for which API answers which question and what it costs, and
//! `docs/PERFORMANCE.md` before touching anything on a per-frame path.

// This is a Windows task manager: the whole point of it is APIs that no
// other platform has. Saying so here gives anyone who tries a one-line
// answer, instead of several hundred linker errors about `ntdll`.
//
// It is a `cfg` rather than a hard `compile_error!` at the crate root
// because the portable core below genuinely does build and test
// anywhere, and that is deliberate — see the module docs above.
#[cfg(not(windows))]
const _: () = {
    // Intentionally empty: the binary carries the diagnostic (see
    // `src/main.rs`), and failing the library build here would take the
    // portable tests down with it.
};

pub mod brand;
pub mod color;
pub mod config;
pub mod format;
pub mod icon;
pub mod model;
pub mod motion;
pub mod theme;

#[cfg(windows)]
pub mod engine;
#[cfg(windows)]
pub mod gui;
#[cfg(windows)]
pub mod win;

#[cfg(test)]
mod header_check;

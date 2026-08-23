// ============================================================================
// Module:       win
// Description:  The only part of the crate that talks to Win32, wrapped so that
//               nothing above it contains the word `unsafe`.
//
// Dependencies: windows-sys; sibling modules per subsystem. No egui, no model
//               types beyond what the samplers return.
// ============================================================================

//! Everything that talks to Windows.
//!
//! ## The discipline
//!
//! Every FFI call in this crate lives inside a *safe leaf wrapper*: one
//! named function that takes ordinary Rust arguments, contains a single
//! `unsafe` block holding **the call and nothing else**, and returns an
//! `Option` or `Result` of a safe type. Arithmetic, error handling, and
//! string marshalling go outside the block — if it can be safe code, it
//! is.
//!
//! Three rules make that hold rather than merely being a preference:
//!
//! - **Every `unsafe` block carries a `// SAFETY:` comment** stating the
//!   argument-validity reasoning it actually depends on — not "this is
//!   fine", but which pointer points at what, for how long, and who
//!   guarantees the length.
//! - **Anything with a matching close or free call gets an owning
//!   wrapper with a `Drop` impl**, so an early return cannot leak it. See
//!   [`handle::OwnedHandle`] and [`handle::OwnedLocalMemory`]. A leaked
//!   process handle is not a cosmetic problem in this app: the sampler
//!   opens one per process per interval, so a leak on an error path
//!   exhausts the handle table of the machine being monitored within
//!   minutes.
//! - **No caller of one of these needs `unsafe` itself**, and no code
//!   above this module contains any.
//!
//! ## Where the data comes from
//!
//! `docs/WINDOWS_APIS.md` has the full table — which API answers which
//! question, what it costs, and what it cannot tell you. The short
//! version:
//!
//! - [`nt`] is the backbone. One `NtQuerySystemInformation` call returns
//!   every process on the machine with its CPU times, memory, and I/O
//!   counters already filled in. The documented alternatives —
//!   `EnumProcesses` plus `OpenProcess` plus four more calls per process
//!   — cost hundreds of syscalls per sample and cannot see processes the
//!   caller may not open.
//! - [`identity`] fills in the parts that call does *not* carry — owner,
//!   elevation, bitness, image path — once per process rather than once
//!   per sample, because none of them change over a process's life.
//! - [`nt::cpu`], [`memory`], [`disk`], [`net`], [`gpu`] are the
//!   system-wide counters behind the Performance view, and [`system`]
//!   the static facts beside them.
//! - [`control`] is the write side: end, suspend, resume, priority,
//!   affinity.
//!
//! ## Windows 10 is the floor
//!
//! This targets Windows 10 1809 (build 17763) and later. Anything newer
//! than that is *probed*, never assumed: [`dwm::set_dark_titlebar`] asks
//! DWM and accepts a refusal, and the GPU counters in [`gpu`] simply
//! report nothing on a machine whose driver does not publish them. A
//! task manager that refuses to start, or that starts and shows an empty
//! window, because one optional counter is missing would be worse than
//! one that quietly omits a column.

pub mod app_icon;
pub mod control;
pub mod dialog;
pub mod disk;
pub mod dwm;
pub mod file;
pub mod gpu;
pub mod handle;
pub mod identity;
pub mod memory;
pub mod net;
pub mod nt;
pub mod privilege;
pub mod services;
pub mod startup;
pub mod strings;
pub mod system;
pub mod window;
pub mod windows;

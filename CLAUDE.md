# rustaman — working notes for agents

A Windows task manager in Rust: process, service, startup and performance
management in one native app, on egui/eframe over a hand-wrapped Win32
layer.

Read [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) for the module map,
[`docs/WINDOWS_APIS.md`](docs/WINDOWS_APIS.md) before touching anything
that talks to Windows, and [`docs/PERFORMANCE.md`](docs/PERFORMANCE.md)
before touching anything on a per-frame path. Those three are where the
non-obvious constraints live; this file is the short version.

For the human-facing contribution guide — how to build and test, the
rules CI enforces, and what a pull request needs — see
[`CONTRIBUTING.md`](CONTRIBUTING.md).

## Commands

```bash
cargo test
cargo clippy --all-targets --all-features -- -D warnings
cargo fmt --all -- --check
cargo run --release                             # the app
cargo run --example brand_assets                # regenerate assets/brand/
cargo run --example changelog                   # regenerate CHANGELOG.md
cargo run --example changelog -- --check        # what CI runs
cargo run --example screenshot -- --list        # see the UI, without a window
```

All of these are clean on `main` and enforced by CI
(`.github/workflows/ci.yml`). A warning is a build failure there.

## Half this crate does not exist on your machine unless it is Windows

This is the single most important thing to know before you believe a
green test run.

`src/win/`, `src/engine/`, and `src/gui/` are `cfg(windows)` and compile
to **nothing** on Linux or macOS — as do `eframe`, `egui` and
`egui_extras`, which are scoped to `[target.'cfg(windows)'.dependencies]`
in `Cargo.toml`. A clean `cargo test` on Linux has exercised the model,
the theming, the colour space, the formatting, the rate arithmetic and
the config parser, and has not compiled a single line of the Win32 layer
or the window.

What to do instead, off Windows:

```bash
cargo clippy --all-targets --all-features --target x86_64-pc-windows-msvc -- -D warnings
```

`rustup target add x86_64-pc-windows-msvc` first. This type-checks the
whole Windows half without linking, which catches a broken
`unsafe extern` block, a wrong struct layout, or an egui API that moved.
It cannot run a test, and it cannot tell you the window looks right.
**Anything visual, and anything that reads a real machine, needs a
Windows run and a screenshot.** CI runs both halves; see the `windows`
and `portable` jobs.

### Getting that screenshot without driving the app

```bash
cargo run --example screenshot -- --list
cargo run --example screenshot -- --scene network
cargo run --example screenshot -- --all
cargo run --example screenshot -- --scene live-network
```

`examples/screenshot.rs` runs the real `gui::ui::draw` against an
offscreen wgpu target and writes a PNG to `target/screenshots/`. No
window, no event loop, no clicking through the app to find the panel —
which is what makes it usable over a remote session and from a terminal.

The point is not only convenience. A **scene** is a machine as well as a
view: the fabricated one has twenty-one network adapters of which three
are hardware and one is unplugged, sixteen cores, and two disks of very
different sizes. Those are the configurations the layout has to survive,
and the machine you are sitting at has exactly one of them, permanently.
`--size` re-renders any scene at another window size, which is how the
responsive behaviour gets checked at all.

The `live-*` scenes drive the real sampler instead, which is the other
half: a fabricated snapshot cannot tell you what `src/win/` decided to
report about this machine. Use one to check a change to the Windows
layer; use the fabricated ones to check the drawing.

## Commit only what you changed

**This repository may be worked on by more than one agent at a time.**
Assume the working tree holds someone else's half-finished work, and
check `git status` before staging anything.

`git commit` commits the whole index, not just the paths you passed to
`git add`. Use `git commit -- path/to/file` so a staged change you did not
make cannot ride along.

So never `git add -A`, never `git add .`, and never `git commit -a`.
Stage the files you actually changed, by name. Where a file you touched
also carries someone else's in-flight edits, stage only your own hunks.

**Line endings are `.gitattributes`' problem, not yours.** Every text
file is stored and checked out as LF, including on Windows. Do not convert
endings by hand and do not "restore CRLF" after editing — git normalises
on the way in, so nothing you do to a file's endings can reach a commit.

---

## Rules this codebase actually enforces

### Every source file opens with the header banner

A ruled block naming `Module:`, `Description:`, and `Dependencies:`, then
a blank line, then the module's `//!` docs.
`every_source_file_carries_a_module_header` in `src/header_check.rs`
walks `src/`, `tests/`, and `examples/` and fails the build for a file
missing one — so a new file needs its header in the same commit that adds
it.

`Module:` is checked against the path the file actually sits at
(`src/gui/ui/theme.rs` → `gui::ui::theme`, `src/win/mod.rs` → `win`,
`src/lib.rs` → `rustaman (library crate root)`), which is what catches a
header copied off a neighbour and never re-read. The rest is checked
structurally: the fields must be present and filled in.

The banner is plain `//`, not `//!`, and is deliberately *not* rustdoc.
It is the orientation a person gets on opening the file cold; the `//!`
block below it is the documentation, and that is where the reasoning
belongs. Do not restate one in the other, and keep `Dependencies:` to
what a reader needs to know is in play — not a transcription of every
`use`.

### No `unwrap`, `expect`, or `panic!` anywhere — including tests

Denied by lint in `Cargo.toml`, so a violation is a build failure. In
library code use `let ... else`, `?`, or an explicit fallback. In tests,
return `anyhow::Result<()>` and use `?` for anything fallible, with
`assert!` / `assert_eq!` carrying a message for the actual assertions.

There is no crate-wide exemption and there should not be one: a blanket
`cfg_attr(test, allow(..))` in `lib.rs` covers *all* `#[cfg(test)]`
items, including library code that merely happens to be test-gated.

This matters more here than in most crates. `panic = "abort"` in release
means a panic on the sampler thread takes the process down while it holds
an open handle to every process on the box.

### Every `unsafe` block gets a safe leaf wrapper

The whole Win32 surface is written by hand, and this is the discipline
that keeps it reviewable:

- One named function per FFI call, taking safe Rust arguments and
  returning `Option`/`Result` of a safe type.
- The `unsafe` block contains **the call and nothing else**. Arithmetic,
  error handling, and string marshalling go outside it — if it can be
  safe code, it is.
- A `// SAFETY:` comment on every block, stating the argument-validity
  reasoning it actually depends on. "This is fine" is not that.
- Anything with a matching close/free/destroy call gets an owning wrapper
  with a `Drop` impl, so an early return cannot leak it. `win::handle`
  holds the shared ones — `OwnedHandle`, `OwnedKey`, `OwnedLocalMemory` —
  and a module with a resource nobody else needs keeps its own private
  one: `gpu::Query` (`PdhCloseQuery`), `services::ServiceHandle`
  (`CloseServiceHandle`), `net::IfTable` (`FreeMibTable`).
- Prefer `MaybeUninit` over `mem::zeroed` for out-parameters, and only
  `assume_init` on the path where the callee reported success.

No caller of one of these should need `unsafe` itself, and no test body
should contain any.

This is mandatory for agents as well as contributors. Before adding an
unsafe operation, prove a safe standard-library operation, checked byte
encoding, `Default`, or an existing owner cannot do the job. Required
unsafe stays under `src/win/`; one private safe function owns exactly one
unsafe operation. Public `unsafe fn`, unsafe test fixtures, and unsafe in
`main`, `config`, `engine`, or `gui` are prohibited. ABI-required
`unsafe extern` declarations/callbacks are the narrow exception, and
their logic must immediately delegate to safe code.

The policy is executable: `unsafe_op_in_unsafe_fn` and Clippy's
`undocumented_unsafe_blocks` / `missing_safety_doc` are denied, while
`src/unsafe_check.rs` rejects unsafe outside `src/win`, unsafe in tests,
public unsafe functions, and functions containing multiple unsafe
operations. Do not suppress those checks; reshape the boundary.

### A hand-declared kernel struct is pinned by compile-time assertions

`src/win/nt/types.rs` declares `SystemProcessInformation` and friends by
hand, because `windows-sys` ships the SDK's redacted version. Every such
struct carries:

```rust
const _: () = {
    assert!(core::mem::size_of::<SystemProcessInformation>() == 0x100, "...");
    assert!(core::mem::offset_of!(SystemProcessInformation, CreateTime) == 0x20, "...");
};
```

A layout that disagrees with what the kernel writes does not crash. It
produces plausible wrong numbers, in one column, on some machines. The
assertions are what turn that into a build failure. They go in a
`const _: () = { ... }` block rather than a test — clippy's
`assertions_on_constants` fires on the test form, and a test that cannot
fail at runtime should not be a test.

### A process is `(pid, creation_time)`, never a bare PID

Windows reuses PIDs, and on a busy machine it reuses them between one
sample and the next. `ProcessKey` is the identity used by the sort keys,
the selection, the expansion set, the tree edges, the rate history, and
every action.

Two rules follow, and both are load-bearing:

- **A rate is only computed between two samples with the same
  `ProcessKey`.** Otherwise a reused PID subtracts one process's
  cumulative counters from another's, and a fresh process shows a CPU
  spike it never had.
- **Every action in `win::control` calls `verify()` before acting** —
  re-reading the creation time from the opened handle and comparing it
  against the key it was asked for. Without it, a kill lands on whatever
  took the PID between the click and the call. Never add an action that
  takes a `u32` pid.

### The UI thread makes no system call

Not "few". None. Everything drawn comes from a `Snapshot` the sampler
thread already built. This is the whole reason the window stays alive
when the machine does not, and it is the one rule in this file whose
violation is invisible until the exact moment it matters. Details in
[`docs/PERFORMANCE.md`](docs/PERFORMANCE.md).

If a draw path needs a fact the snapshot does not carry, add it to the
snapshot in `engine/sampler.rs`. Do not fetch it where it is needed.

### Never recompute something process-count-sized inside a draw call

The GUI is immediate mode: `gui::ui::draw` runs in full every frame.
Derived data is cached on the app state and keyed off observed state
rather than invalidated by hand. **If you add a field that affects which
rows are drawn or in what order, add it to `RowKey`** in
`src/gui/app/rows.rs`.

The two `Vec`s in `RowKey` are sorted `Vec`s built from `HashSet`s on
purpose: a `HashSet`'s iteration order is not stable, so comparing two by
iteration reports a change on a frame where nothing changed.

Every table uses `TableBody::rows` — the virtualised body — so drawing
costs what is on screen, not what is in the list. That is what makes the
per-row text formatting affordable; do not undo it.

### The channel between the sampler and the UI is bounded, and drops

A full queue drops the frame. An unbounded channel would let a UI thread
that fell behind accumulate a backlog and then show a five-second-old
machine while working through it. A dropped sample costs one tick of
graph history; a queued one costs correctness.

### No literal colour in drawing code

Every colour comes from `palette()` (`src/gui/ui/theme.rs`), which returns
the active theme's `Palette`. The catalog is data —
`assets/themes.toml` — and **a theme states thirteen colours** while
`Palette::derive` computes the rest, so adding a *derived* colour means
editing one function rather than every entry in the file.

`no_drawing_module_holds_a_colour_literal` in `src/gui/ui/mod.rs` scans
the drawing modules and fails the build for one.

Three tests guard the catalog itself, and a theme that fails any of them
is a failing build:

- `every_theme_is_readable` — WCAG AAA (7:1) for primary text, AA (4.5:1)
  for secondary, against every surface the text can land on.
- `a_scrollbar_handle_is_never_the_colour_of_what_it_scrolls`.
- `a_surface_ramp_runs_the_direction_its_mode_claims` — a `mode = "dark"`
  theme whose surfaces get darker as they rise is a theme with its layers
  inverted.

**The brand mark is the exception, and the only one.** `src/brand.rs`
holds the mark's geometry and its five colours as literals, because a logo
that restyles itself under a dark theme is not a logo. `gui::icons`
paints it and rasterises it, and `assets/brand/` is the same drawing at
larger sizes — regenerate those with `cargo run --example brand_assets`
rather than editing the PNGs.

### The rainbow ramp is OKLCh, and that is not a preference

`src/color.rs`. The per-core graphs, the chart series and the category
chips all index one ramp, and the point of the ramp is that **no series
looks more important than its neighbours**.

An HSL ramp does not do that. Sixteen series evenly spaced around HSL
gives a perceptual lightness spread of roughly 0.10 to 0.62 — the green
core reads as highlighted and the blue one reads as disabled, and no
amount of tweaking saturation fixes it because the problem is the colour
space. In OKLCh the same sixteen sit within 0.02 of each other.

`Oklch::to_rgb` walks chroma down by bisection to find the closest
in-gamut colour at the same hue and lightness, rather than clipping
channels — clipping shifts the hue, which is how you get a "red" that has
gone orange only in the themes with a bright accent.

### Icons are geometry; nothing is set in a font

`src/icon.rs` holds twenty-two icons as polylines on a shared 16×16 grid;
`gui/ui/icon.rs` strokes them in the theme's colour.

This is not a stylistic choice. The app originally used Unicode
characters — `U+25B8` for a disclosure arrow, `U+2699` for settings, the
three window-control codepoints — and **every one of them shipped as an
empty box.** egui bundles Ubuntu Sans, Hack and an emoji subset, which
cover Latin, Greek, Cyrillic and a few hundred emoji and almost nothing
in Geometric Shapes, Miscellaneous Symbols or Dingbats.

`no_drawing_module_sets_an_icon_in_a_font` scans for pictographic
codepoints and fails the build. Typographic punctuation (an em dash, a
middle dot, an ellipsis) is explicitly fine — the rule is about
pictographs.

Three tests guard the set itself, and all three catch things that are
invisible in review: every icon stays inside its grid, is optically
centred, and reaches far enough across the grid to look like a member of
the same set. The third one rejected the first drag grip.

### Every animation goes through `motion`, and there are four durations

`src/motion.rs` owns the curves and the durations; `gui/ui/motion.rs` is
the egui binding and is **the only file allowed to call `ctx.animate_*`**.
`no_drawing_module_animates_by_hand` fails the build for a call anywhere
else, and `no_drawing_module_holds_a_bare_duration` for a number passed
to one of the helpers.

The four are `INSTANT` (a hover), `QUICK` (a selection, a disclosure),
`SETTLE` (the machine's own state arriving) and `ENTER` (a whole view).
Each has a job; a fifth would be one of these with a different number,
which is the drift the module exists to stop. Their relationships are
compile-time assertions.

**Key every animation on the thing, not its position.** An id built from
a loop index animates the *slot*: re-sort the table and every row
inherits the animation state of whatever used to be there, so the whole
table flashes. Derive ids from a `ProcessKey`, a service name, a column.

### Drag and drop lives in `dnd`, feedback included

One `Lane` serves every reorderable list. Not because the code is long,
but because a reorder is almost entirely *feedback* — a dimmed source, a
ghost on the pointer, an indicator at the drop gap, and nothing at all
when the drop would be ignored — and a view tracking its own
`dragging: Option<usize>` implements some subset of those and the next
view a different subset. `no_drawing_module_carries_its_own_drag_state`
fails the build for a view that reads `drag_started` itself.

The pointer resolves to the **gap** it is nearest, never the item it is
over: resolving to an item leaves a dead zone in the middle of each one
and makes the position after the last item unreachable. Converting a gap
back to an index is `model::columns::landing`, which is portable so the
off-by-one is tested everywhere rather than only on the Windows CI job.

### No hand-picked pixel gaps

Every margin, inset and `add_space` is one of `SPACE_XS` / `SPACE_SM` /
`SPACE_MD` / `SPACE_LG` in `src/gui/ui/theme.rs`, and `PAD` — the inset
from a panel edge to its content — is one of them too. Four values, so
the left edges of the title bar, the sidebar, every view heading and every
table form one column.
`no_drawing_module_holds_a_hand_picked_pixel_gap` fails the build for a
bare number. If you want a value between two steps, the answer is one of
the two steps.

### A table row is one row, and one function paints it

Every table draws through `widgets::row_background` — the stripe, the
hover lift and the selection bar together, across the **whole** row.

Two things make that harder than it sounds, and both have already been
got wrong once.

A table cell's painter is clipped to that cell, so the fill has to be
painted from the *first* cell and widened. `Painter::with_clip_rect`
**intersects** with the clip already in force, so it undoes the widening
on the next line; `set_clip_rect` replaces it. Every table in the app
drew its stripes across one column for as long as that was wrong, under
a comment explaining that it had been fixed.
`a_row_background_fills_the_row_and_not_just_its_first_cell` checks the
painted shape against its own clip rect, because a bounds-only check
passes on the broken version.

And a resizable `egui_extras` table paints a rule at every column
boundary, the full height of the scroll area and on down through the
empty space below the last row. `theme::quiet_column_rules` silences the
resting stroke for a table's own `Ui` and leaves the hovered and dragged
ones alone, so the resize affordance survives and the grid does not. Call
it before building any table.

**A colour that encodes nothing is worse than no colour.** The metric
cells spend colour on load, the chips on status, the graphs on series
identity, and the Network panel's dots on an adapter's state. The
process rows used to carry a hue hashed from the process key, at the
leading edge of the row, meaning nothing at all — and it won, because it
was leftmost.

### An `egui_extras` column keeps its width whatever the pane can afford

Only a `remainder()` shrinks, and a table that overruns its pane neither
clips nor scrolls: it paints over whatever is beside it. The Details
table drew its last three columns across the inspector, and over the
inspector's own empty-state message, for exactly this reason.

So a table's stated widths have to fit the window the app opens at
(`config::Config::default`, 1440 points) *including* whatever else is on
screen beside it, and `every_details_column_fits_the_default_window`
holds the widest table to it.

`egui::Panel::right` is not a fix for that: it reserves its width by
moving the parent's cursor, and in a top-down layout
`available_rect_before_wrap` is derived from the parent's `max_rect`, so
a table measuring itself never sees the reservation. `details::draw`
splits the pane by hand and hands each half an explicit `max_rect`.

### `bg_fill` and `weak_bg_fill` are not interchangeable

egui paints buttons from `weak_bg_fill` and filled controls — scrollbar
handle, checkbox interior, slider rail — from `bg_fill`. A button is a
surface and may share the card's colour; a scrollbar handle in the card's
own colour is invisible. `gui/ui/theme.rs` therefore points
`weak_bg_fill` at the surfaces and `bg_fill` at `Palette::control`.
Setting both to `raised` is what makes every scrollbar in the app
disappear.

### The custom title bar drags with `ViewportCommand::StartDrag`

Handing the drag to the window manager is what preserves Aero Snap. A
hand-rolled move loop that repositions the window on each mouse event
works perfectly and silently loses snap-to-half, snap layouts, and
maximise-on-drag-to-top — a regression nobody reports as a bug, they just
stop using the app.

### Constants live with what they govern, and there is no `constants.rs`

Three tiers, in order of preference — reach for the narrowest that works:

- **Function-local `const`**, when one function uses it.
- **Module-private `const`** at the top of the file, when several
  functions in one module share it.
- **`pub` in the module that owns the concept**, when two modules
  genuinely need it — `theme` owns the spacing scale, `color` owns the
  ramp, `brand` owns the mark.

A shared `constants.rs` is the thing to avoid, and the reason is not
style. It is a file every change touches, it files unrelated numbers next
to each other by the accident of both being numbers, and it separates a
value from the reasoning that justifies it — which here is usually a
paragraph, not a line. Duplication across modules is the signal to
promote one.

### `wildcard_enum_match_arm` is denied, and that is deliberate

A `match` on a column, a view, a sort key or a process kind must list
every variant. It is more typing, and it is the thing that catches a new
column rendering blank in one view because a `_ => {}` swallowed it.
Where a wildcard is genuinely right, name the variants anyway.

### `allow` is denied; use `expect`

`#[expect(lint, reason = "...")]` fails the build when the lint stops
firing, so a suppression cannot outlive the thing it was suppressing.
Every one carries a `reason`.

### `CHANGELOG.md` is generated — never hand-edit it

`examples/changelog.rs` rebuilds it from `git tag` plus
conventional-commit subjects, and CI runs `--check` against the
*released* sections, so an edit to one is a build failure. `Unreleased`
is exempt by design: it changes with every merge, and a squash rewrites
the hashes it pins. Prose for a release belongs in the GitHub release
body.

[`CONTRIBUTING.md`](CONTRIBUTING.md) has the release ordering — the
changelog section is written **before** the tag
(`cargo run --example changelog -- --release vX.Y.Z`), so the tag names a
tree whose changelog is already finished.

**Never amend or rebase a commit after tagging it.** The tag survives
pointing at the discarded copy, on no branch at all, and every release
range (`<previous>..<tag>`) computed from it is then wrong. The changelog
`--check` fails on any version tag no branch can reach.

---

## Things that are deliberate, not oversights

- **`NtQuerySystemInformation` rather than `EnumProcesses`.** The
  documented route is ~2,500 syscalls a sample and cannot see protected
  processes. See [`docs/WINDOWS_APIS.md`](docs/WINDOWS_APIS.md).
- **Total CPU is `1 - idle/total`, not `(kernel + user)/total`.**
  `KernelTime` already includes `IdleTime`; the second form reads 100% on
  an idle machine.
- **A parent link is rejected unless the parent started strictly before
  the child.** Windows does not clear `InheritedFromUniqueProcessId` when
  a parent exits, so a new process landing on a dead parent's PID would
  otherwise adopt its orphans.
- **Descending sort reverses the primary key only.** `compare` is a total
  order; `compare_directed` applies the direction to the primary key and
  leaves ties breaking alphabetically ascending. Reversing the whole
  comparison lists the idle processes backwards, which reads as a bug.
- **The filter drops terms with no alphanumeric characters, and
  half-typed field prefixes.** Otherwise typing `pid:` blanks the list on
  the way to `pid:4242`.
- **No per-process network throughput.** No Win32 call provides it; Task
  Manager reads an ETW kernel session. Endpoint counts instead.
- **Startup entries are read-only.** Toggling one means writing another
  program's registry state.
- **IPv4 only in the connection tables.** The IPv6 pair roughly doubles
  the module to change a count from "some" to "slightly more some".
- **eframe runs on wgpu, not glow.** The glutin WGL path is the one that
  fails on hybrid-graphics laptops — exactly the machines this app is
  most useful on.
- **The floor is Windows 10 1809.** Anything newer is probed at runtime,
  so a missing entry point costs a column rather than the app.

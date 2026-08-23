# Architecture

Three layers, and the rule that separates them is the whole design:

```
  win/            Windows tells us things.      cfg(windows), unsafe, no state
  engine/         A thread asks, repeatedly.    cfg(windows), owns the sampling clock
  model/          What we make of the answers.  portable, pure, tested everywhere
  gui/            What the user sees.           cfg(windows), draws; dispatches user actions
```

Data flows down that list and never back up. `win` does not know a window
exists. `model` does not know Windows exists. `gui` does not know how a
number was obtained, and — the part that matters most — **cannot obtain
one itself**: periodic monitoring never runs on the paint thread, so a
stalled sampling read cannot stall the window. Explicit user actions may
perform bounded calls synchronously. That is the
single biggest difference from the Task Manager that ships with Windows,
which queries on the thread that draws.

The seam is a snapshot. `engine::Sampler` runs on its own thread, calls
into `win`, builds a `model::Snapshot`, and sends it through a **bounded,
single-slot, overwrite-oldest** mailbox. The window takes the newest one and
draws it. If the UI is busy the sampler drops frames rather than
queueing them, because a task manager showing a five-second-old backlog
is worse than one showing the present.

## The portable half

`model`, `theme`, `format`, `color`, `treemap`, and `config` have no
`windows_sys` in them and compile on any platform. This is deliberate and it is
enforced: CI runs their tests on Linux (`.github/workflows/ci.yml`, the
`portable` job) and a `use windows_sys::` that leaks into one of them
fails there.

The point is not portability as a feature — this app runs on Windows and
nowhere else. The point is that the sorting, the filtering, the tree
building, the rate arithmetic, the colour space and the contrast checks
are all *pure functions over data*, and pure functions over data can be
tested exhaustively without a machine to observe. Roughly four fifths of
the test suite lives here.

`treemap` is the newest of them and the clearest example of the split:
the Memory view's map is a *layout*, so the squarified algorithm that
produces it — the part with the arithmetic that can be wrong — lives
here and is checked on Linux, while `gui/ui/memory.rs` only paints the
rectangles it hands back. One of its tests measures the algorithm
against the naive alternative rather than against itself: squarifying
has to beat slice-and-dice by a factor of four on the worst aspect
ratio, or it is not earning the extra sixty lines.

## `src/win/` — the Windows layer

One module per interface, each exposing safe functions over raw FFI.
Nothing here holds state between calls except a handle it owns.

| Module | What it answers |
|---|---|
| `nt/` | `NtQuerySystemInformation`: the process table, CPU times, memory, I/O |
| `identity.rs` | Owner, session, elevation, integrity — via the process token |
| `memory.rs` | Working set, commit, and the system-wide memory picture |
| `disk.rs` | Per-physical-disk active time and throughput (`IOCTL_DISK_PERFORMANCE`) |
| `net.rs` | Adapter counters (`GetIfTable2`) and per-PID endpoints (`GetExtendedTcpTable`) |
| `gpu.rs` | PDH GPU engine counters |
| `services.rs` | The SCM: every Win32 service, its state, and its hosting PID |
| `startup.rs` | Canonical Run/RunOnce registry views and both Startup folders |
| `system.rs` | Uptime, logical processors, the machine's own description |
| `windows.rs` | Top-level window enumeration, for telling an app from a background task |
| `control.rs` | End, end tree, suspend, resume, set priority |
| `privilege.rs` | `SeDebugPrivilege`, enabled once at startup if available |
| `dwm.rs` | The dark title bar, probed rather than assumed |
| `handle.rs` | The owning `Drop` wrappers everything above returns |
| `strings.rs` | UTF-16 in, `String` out, and the `UNICODE_STRING` marshalling |

[`WINDOWS_APIS.md`](WINDOWS_APIS.md) covers *why* each of these is the
call it is, what it costs, and what none of them can tell you.

### `nt/` is the one to read first

Everything else in the list above is a supplement. `nt::process::sample`
is a single `NtQuerySystemInformation(SystemProcessInformation)` call
that returns a variable-length chain of records covering every process on
the machine — name, PID, parent, create time, thread count, handle count,
kernel and user CPU time, every memory counter, and cumulative I/O — in
one syscall.

The documented alternative (`EnumProcesses`, then `OpenProcess` +
`GetProcessTimes` + `GetProcessMemoryInfo` + `GetProcessIoCounters` per
process) is roughly 2,500 syscalls per sample on an ordinary desktop, and
it cannot see protected processes at all, so they appear in the list with
blank columns.

The cost of using it is that `windows-sys` ships a *redacted*
`SYSTEM_PROCESS_INFORMATION` — the public SDK does not document the real
one — so `nt/types.rs` declares the layout by hand and pins it with
compile-time assertions:

```rust
const _: () = {
    assert!(core::mem::size_of::<SystemProcessInformation>() == 0x100);
    assert!(core::mem::offset_of!(SystemProcessInformation, CreateTime) == 0x20);
    assert!(core::mem::offset_of!(SystemProcessInformation, ReadTransferCount) == 0xe8);
};
```

A struct that disagrees with what the kernel writes is not a bug that
shows up as a compile error or a crash. It shows up as plausible wrong
numbers. The assertions are what make it a build failure instead.

## `src/engine/` — the sampler

`Sampler::spawn` starts the thread; `Sampler::latest` drains the channel
and returns the newest `Snapshot`, or `None` if nothing new arrived. The
window calls `latest` once a frame.

The interval is sliced: the thread sleeps in 100 ms pieces and checks its
stop flag between them, so closing the window does not wait out a
two-second interval. Services and startup entries are re-read on demand
(`F5`) rather than every tick — the SCM enumeration is expensive and the
answer changes on a human timescale.

## `src/model/` — what the numbers mean

Pure, portable, and where the arithmetic that is easy to get wrong lives.

- **`rates.rs`** turns two cumulative counters and two timestamps into a
  rate. Four documented failure modes, each with a test: a counter that
  went backwards (the process restarted under a reused PID), a zero or
  negative interval, a first observation with nothing to subtract from,
  and an interval so long the answer is meaningless.
- **`tree.rs`** builds the parent/child forest. A parent link is rejected
  unless the claimed parent started *strictly before* the child, because
  Windows does not clear `InheritedFromUniqueProcessId` when a parent
  exits — so a new process landing on the dead parent's PID would
  otherwise adopt its orphans.
- **`sort.rs`** has two comparisons on purpose. `compare` is a total
  order; `compare_directed` applies the direction to the *primary key
  only*, so ties still break alphabetically ascending. Reversing the
  whole comparison put the idle processes in reverse alphabetical order,
  which reads as a bug every time.
- **`filter.rs`** parses the search box. Terms with no alphanumeric
  characters are dropped, and so is a half-typed known field prefix — so
  typing `pid:` blanks nothing while you are still on your way to
  `pid:4242`.
- **`history.rs`** holds the ring buffers behind the graphs and picks
  their vertical scale.

### `ProcessKey`, everywhere

A PID identifies a process only until it exits. Windows reuses PIDs
aggressively, and on a busy machine the reuse can happen between one
sample and the next.

So nothing in this codebase identifies a process by PID alone.
`ProcessKey` is `(pid, creation_time)`, and it is what the sort keys, the
expansion set, the selection, the tree edges, the rate history, and every
action are keyed by. Two consequences worth knowing:

- A CPU rate is only computed between two samples with the same
  `ProcessKey`. A reused PID starts a fresh history rather than
  subtracting one process's counters from another's.
- Every action in `win::control` takes a `ProcessKey` and calls
  `verify()` — re-reading the creation time from the opened handle —
  before it does anything. A kill cannot land on a process that took over
  the PID between the click and the call.

## `src/gui/` — the window

egui/eframe on wgpu. Immediate mode: the whole window is rebuilt every
frame, which is why [`PERFORMANCE.md`](PERFORMANCE.md) exists and should
be read before touching a draw path.

```
gui/app/      state: the snapshot, the selection, the caches
gui/ui/       drawing: one module per view, plus chrome, widgets, theme
gui/icons.rs  the brand mark and the shell icon cache
```

`gui/app/rows.rs` holds the row cache. Layout is *not* recomputed every
frame; it is recomputed when `RowKey` changes, and `RowKey` is built from
the observed state that affects layout — the snapshot sequence, the sort
column and direction, whether grouping is on, the search text, the
expanded set, the collapsed categories. Adding a field that affects which
rows are drawn means adding it to `RowKey`. This is the same discipline
as hand invalidation, except that forgetting it makes the UI stale rather
than making it wrong in a way nobody notices for a month.

`gui/app/background.rs` holds `BackgroundRead`, the sampler's lighter
sibling for state that a view reads on demand rather than every sample —
Services and Startup's own lists. It spawns a one-shot thread and hands
back a receiver the view polls without blocking, so the win-layer call
behind it still never runs on the paint thread even though it is not
part of the snapshot. See [`PERFORMANCE.md`](PERFORMANCE.md) for why that
distinction exists rather than putting everything through the sampler.

### Colour goes one way

`theme::Palette` is portable and knows nothing about egui.
`gui/ui/theme.rs` is the only file that converts one to `egui::Color32`,
and `palette()` is the only way drawing code gets a colour. A test scans
the drawing modules for colour literals and fails the build on one. The
brand mark in `src/brand.rs` is the single exception, because a logo that
restyles itself under a dark theme is not a logo.

The same rule holds for spacing: four steps (`SPACE_XS`, `SPACE_SM`,
`SPACE_MD`, `SPACE_LG`), and a test that fails the build for a hand-picked
pixel gap. Four values are what make the title bar, the sidebar, every
view heading and every table share one column of left edges.

## Where a change usually goes

| You want to… | Start in |
|---|---|
| Add a column | `model/sort.rs` for the key, then the view in `gui/ui/` |
| Read something new from Windows, every sample | a new module in `win/`, then `engine/sampler.rs` |
| Read something new from Windows, on demand instead | a new module in `win/`, then `gui/app/background.rs` |
| Change what a number means | `model/rates.rs` or `model/history.rs` |
| Add a theme | `assets/themes.toml` — thirteen colours, nothing else |
| Add a *derived* colour | `theme::Palette::derive`, once, not every theme |
| Add an action | `win/control.rs`, keyed by `ProcessKey`, then `gui/app/actions.rs` |
| Change spacing | the scale in `gui/ui/theme.rs`, never at a call site |

## Where constants live

Three tiers, and reach for the narrowest that works:

- **Function-local `const`** when one function uses it. Most of the GUI's
  geometry is this, declared next to the code that has to agree with it.
- **Module-private `const`** at the top of the file when several functions
  in one module share it.
- **`pub` in the module that owns the concept** when two modules genuinely
  need it — `theme` owns the spacing scale, `color` owns the ramp,
  `brand` owns the mark.

There is deliberately no `constants.rs`. It would be a file every change
touches, it files unrelated numbers next to each other by the accident of
both being numbers, and it separates a value from the reasoning that
justifies it — which here is usually a paragraph, not a line.
Duplication across modules is the signal to promote one, not to copy it
again.

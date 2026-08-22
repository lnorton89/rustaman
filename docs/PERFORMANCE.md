# Working on a per-frame path

Read this before changing anything that runs while the window is open.

The premise of this app is that it stays responsive when the machine does
not. That is not a quality bar you reach by being careful; it is a
consequence of two structural rules, and everything below is a
consequence of those two.

> **1. The UI thread makes no system call.**
> **2. The UI is immediate mode, so every frame rebuilds the window.**

---

## 1. The UI thread makes no system call

Not "few". None. The window draws from a `Snapshot` that a background
thread already built.

This is the rule that the Task Manager shipping with Windows does not
have, and it is why that one freezes exactly when you opened it to find
out why the machine is frozen: it queries on the thread that paints, so a
process hung in a kernel wait, a disk that has stopped answering, or an
SCM call that is taking its time all block the window.

Concretely, in a draw path you may not:

- call anything in `crate::win`
- open a handle, a key, or a file
- read the clock for anything that affects what is drawn (the snapshot
  carries its own timestamp; two calls to `Instant::now` in one frame
  will eventually disagree with each other and produce a rate of
  infinity)
- allocate anything sized by the process count that could have been
  computed once

If you need something the snapshot does not carry, the answer is to add
it to the snapshot — in `engine/sampler.rs`, on the sampler thread —
rather than to fetch it where it is needed.

### Read-on-demand state gets its own background read, not a place in the snapshot

Services and startup entries are the exception the rule above does not
cover: they are read when their view is shown and every ten seconds
after, not every sample, because enumerating four hundred services a
second to redraw a list nobody is looking at is its own kind of load.
That means they cannot go through the sampler's snapshot — putting them
there would sample them continuously, which is exactly what "read on
demand" exists to avoid — but the underlying calls
(`EnumServicesStatusExW`, a registry and startup-folder walk) are just as
real and just as capable of blocking as anything the sampler reads.

`gui::app::background::BackgroundRead` is the sampler's lighter sibling
for exactly this: it spawns the read on its own thread once and hands
back a receiver the view polls without blocking, rather than a
persistent thread with a channel it drains on an interval. There is
nothing to join on drop — the thread exits the moment it sends, whether
or not anyone is still listening. `gui/ui/services.rs`'s `refresh` and
`refresh_startup` are the call sites; a third view with the same shape
of "read on demand, not every sample" state should reach for this rather
than either calling `crate::win` inline (the bug this section exists to
prevent) or inventing a second sampler.

### The channel drops, and that is the feature

`engine` sends snapshots over a bounded channel with a non-blocking send.
When the queue is full the sampler **drops the frame** and carries on.
`Sampler::latest` drains whatever arrived and returns only the newest.

An unbounded channel would be worse in exactly the situation this app
exists for: a UI thread that fell behind would accumulate a backlog, and
the window would then show a five-second-old machine while working
through it. A dropped sample costs one tick of graph history. A queued
one costs correctness.

The sleep is sliced into 100 ms pieces with a stop-flag check between
them, so closing the window does not wait out the sampling interval.

### Two caches, both pruned

The sampler holds an identity cache (`ProcessKey` → owner, path,
bitness, elevation, description) and a description cache (path →
`FileDescription`). Both are keyed on things that do not change for the
life of a process, so this is one token open per process per lifetime
rather than one per sample.

Both are pruned each tick against what is still running. A cache keyed by
`ProcessKey` on a machine that starts and stops processes all day grows
without bound otherwise, and
`the_caches_do_not_grow_without_bound` in `engine/sampler.rs` is the test
that says so.

---

## 2. The UI is immediate mode

`gui::ui::draw` runs in full, every frame, at whatever rate the compositor
asks for. There is no retained widget tree, so nothing is "only computed
when it changes" unless you make it so.

### Never recompute something process-count-sized in a draw call

Sorting 400 processes, building the tree, applying the filter, and
laying out the rows is not free at 60 Hz — and on a build server or a
Citrix host with several thousand processes it is the difference between
a window and a slideshow.

Derived data is cached on the app state and **keyed off observed state**
rather than invalidated by hand:

```rust
// gui/app/rows.rs
pub struct RowKey {
    pub sequence: u64,          // the snapshot this was built from
    pub sort: SortKey,
    pub descending: bool,
    pub grouped: bool,
    pub search: String,
    pub expanded: Vec<ProcessKey>,
    pub collapsed: Vec<ProcessKind>,
}
```

Each frame builds a `RowKey` and compares it to the cached one. Equal
means the cached rows are still correct; different means rebuild. **If
you add a field that affects which rows are drawn, or in what order, add
it to `RowKey`.**

This is the same discipline as hand invalidation with one important
difference: forgetting a hand invalidation makes the UI *wrong*, and
forgetting a `RowKey` field makes it *stale* — which is visible the
moment you use the feature you just added, rather than a month later in
one configuration.

Note the two `Vec`s. They are sorted `Vec`s built from `HashSet`s on
purpose: a `HashSet`'s iteration order is not stable, so comparing two by
iteration reports a change on a frame where nothing changed, and the
cache rebuilds every frame while appearing to work.

### The snapshot sequence is what makes it cheap

`sequence` increments once per sample — about once a second — so the
common case is: the key differs once a second and matches for the sixty
frames in between. Sorting once a second is nothing. Sorting sixty times
a second is the whole budget.

### Text formatting is in the draw path

`format::bytes`, `format::rate`, `format::percent` and the rest each
return a `String`, and they are called while drawing rather than cached
on the row. That is affordable for exactly one reason: the tables are
virtualised, so a frame formats the thirty-odd rows on screen and not the
four hundred in the list.

It stops being affordable the moment something makes the table visit
every row — see the note below. If that ever has to happen, the
formatted text moves into the cached row first.

---

## 3. Things that are still O(processes), and when they run

For the avoidance of doubt, here is what walks every process and where it
runs:

| Work | Where | How often |
|---|---|---|
| `NtQuerySystemInformation` and the record walk | sampler thread | once a sample |
| Rate computation against the previous sample | sampler thread | once a sample |
| Identity lookup for processes not in the cache | sampler thread | once per process, ever |
| Tree building | sampler thread | once a sample |
| Filter, sort, flatten to rows | UI thread | once per `RowKey` change |
| Drawing | UI thread | every frame, **but only the visible rows** |

That last row is the one worth stating explicitly. Every table in the app
— processes, details, services, startup — is built with
`TableBody::rows`, which is `egui_extras`' virtualised body: it invokes
the row closure only for the rows the viewport actually shows. Drawing is
therefore proportional to rows on screen, not to rows in the list, and
that is what makes the per-row formatting above acceptable.

Swapping one for `TableBody::row` in a loop, or measuring a column
against every entry to size it, turns a constant-cost path into a linear
one. The symptom is a window that is fine on your machine and unusable on
a build server, which is the worst kind of regression to have: it does
not reproduce where it was written.

---

## 4. Motion costs frames, and that is the trade

Cells, meters and graphs animate to their new values over
`motion::SETTLE` (0.35s) rather than jumping. An egui animation requests
a repaint while it is running, so with a one-second sampling interval the
window is now redrawing for roughly a third of each second instead of
once per second.

That is a deliberate trade, and it is worth being explicit about which
way it goes. A table of four hundred live numbers that *replace*
themselves once a second is close to unreadable — the eye registers the
whole grid changing at once as flicker, not as information. Sliding
values stay legible while they move and carry the direction of the change
as well as its magnitude.

The cost is bounded by the same thing that bounds everything else here:
only visible rows are drawn, so it is thirty rows' worth of interpolation
per frame, not four hundred. Two rules keep it that way:

- **Animate in the draw path, never in the sampler.** The sampler
  publishes real readings. Smoothing is a property of the display.
- **Key every animation on the thing, not its position.** An id built
  from a row index makes a re-sort animate every row to the value of
  whatever used to be in its slot — which is both wrong and the most
  expensive thing the table could possibly do.

When the machine is idle and nothing is changing, every animation
converges and the window goes back to redrawing once per sample.
`motion::Tween` snaps its last fraction of a pixel for exactly this
reason: an asymptotic approach never arrives, so the window would ask for
frames forever over a difference nobody can see.

## 5. The graphs

`model::history` holds fixed-size ring buffers. A ring buffer, not a
`Vec` with a `drain(..1)`: the graphs keep a couple of minutes of history
per series, per core, and shifting a `Vec` sixty times a second across a
few dozen series is measurable for no reason.

Vertical scale is recomputed from the buffer's peak, with headroom, and
with a floor — check the floor *before* applying the headroom, or a
series pinned at 100 comes back as 200 and the graph reads as half
height.

Per-core graphs index the OKLCh ramp in `color.rs`. The ramp is computed
once, not per frame; `Oklch::to_rgb` walks chroma down by bisection to
find the closest in-gamut colour that keeps the hue and lightness, which
is a handful of iterations and absolutely not something to do while
drawing.

---

## 6. If you are about to measure something

Two things that will mislead you:

- **A debug build is not the app.** Release is `opt-level = 3`, LTO, one
  codegen unit and `panic = "abort"`. The record walk in particular is
  several times slower without optimisation, which makes the sampler look
  like the problem when it is not.
- **A quiet machine is not the machine.** Roughly everything here is
  fast at 300 processes. The failure modes live at 3,000, and at "one
  process is hung and its token will not open". A VM with a few hundred
  spawned sleepers is the cheap way to find out which of your assumptions
  was about the machine you happened to be on.

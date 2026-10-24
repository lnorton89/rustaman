# Working on the UI without Windows

Half this crate is `cfg(windows)` and compiles to nothing on Linux, so a
clean `cargo test` there has not touched the Win32 layer, the sampler, or
a single line of the window. That is true, and it is the reason this file
exists — because the conclusion usually drawn from it, that anything
visual has to wait for someone with a Windows machine to look at it, is
not.

A Linux container can build this crate for Windows and run it, GUI
included. `scripts/cloud-ui.sh` is the whole thing:

```bash
scripts/cloud-ui.sh setup          # once per machine
scripts/cloud-ui.sh test           # all 581 tests, the GUI ones included
scripts/cloud-ui.sh test chrome::  # or one module, in about a second
scripts/cloud-ui.sh shot cpu       # a PNG of the real Performance panel
scripts/cloud-ui.sh shot --all
```

## How it works

**The GNU target, not MSVC.** `rustup target add x86_64-pc-windows-msvc`
installs a standard library and no linker, and there is no MSVC linker to
be had on Linux. `x86_64-pc-windows-gnu` links with mingw-w64, which apt
has. Both targets compile the same source — `windows-sys` supports each —
so this runs the code the `windows` CI job builds rather than a port of
it. **No `cfg` was added and no module was moved to make this work.**

**Wine runs the test binary.** `CARGO_TARGET_..._RUNNER=wine` is all
cargo needs; `cargo test --target x86_64-pc-windows-gnu` then behaves
like any other test run.

**lavapipe gives wgpu an adapter.** `mesa-vulkan-drivers` is a software
Vulkan driver, so a machine with no GPU still renders. The app pins wgpu
to D3D12, which Wine does not implement, and `WGPU_BACKEND=vulkan` is the
override that pin deliberately leaves open.

**Xvfb, but only for pixels.** Wine's Vulkan driver wants a display
connection even to enumerate an adapter for an offscreen render, so
anything that rasterises runs under a virtual one. The tests do not need
it: `egui_kittest` builds the shape list and the accessibility tree on
the CPU and only asks for a device when something calls `render()`.

## What it proves, and what it does not

It is worth being exact about this, because the tempting mistake is to
treat a green run here as a green run on Windows.

**It does prove** everything decided by layout, geometry, state, and
interaction — which, going by the bugs actually found this way, is most
of what goes wrong in this UI:

- where a widget lands, how wide it is, what it overlaps, whether a table
  fits the pane it was given;
- what a control is labelled and what it answers to, through the
  accessibility tree;
- whether a click, a hover, a drag or a keystroke does what it should,
  across the several frames a real gesture takes;
- what a graph's geometry does at the edges of its scale;
- and every portable thing the Linux job already covered.

**It does not prove** anything that depends on the machine being real:

- **A driver that falls over.** The reason wgpu is pinned to D3D12 is an
  Intel UHD 620 that takes an access violation inside `igvk64.dll`
  during swapchain creation, so the app fails to *open*. lavapipe is not
  that driver, and headless rendering creates no swapchain. A real
  window on real hardware remains the only test for this class.
- **What the machine reports about itself.** Three `win::` tests fail
  under Wine — `win::dwm::an_invalid_window_handle_fails_cleanly`,
  `win::identity::a_system_binary_has_a_readable_description`, and
  `win::memory::the_kernel_pools_are_reported` — because Wine has no
  DWM, its system binaries carry no version resources, and it reports no
  kernel pools. Those are Wine's limits, not regressions. Everything else
  passes.
- **Typography.** The screenshot example asks for the face the window
  asks for, and a machine without it renders in egui's bundled one, so
  every string is a different width. A scene's *metrics* are
  machine-dependent by design. Read a scene for layout, not for kerning.
- **Anything about the shell.** The tray icon, Explorer integration and
  the properties dialog are real Win32 and belong on a real machine.

So: use this to find and fix the UI bugs it can find, which is most of
them, and say plainly in a pull request that it is what you used. It
replaces guessing. It does not replace the Windows job, and it does not
replace someone opening the app on the sort of laptop this app is for.

## Speed

Running one module's tests takes about a second against roughly seven
minutes for a CI round trip, which is the difference between iterating
on a UI bug and taking three guesses at it. Both the rail's lost-click
bug and the close button's dead corner were pushed as "fixed", twice,
and were still broken; both were caught here in the minute after the
loop existed.

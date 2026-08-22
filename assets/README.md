# Assets

Three kinds of file, with different rules about editing them.

## `themes.toml` — edit freely

The built-in theme catalog, compiled into the binary and shipped
alongside it in the release archive. A theme is **thirteen colours**;
everything else — the selection fill, the scrollbar handle, the grid
lines, the text that goes on the accent, the whole series ramp — is
derived by `theme::Palette::derive`.

That is deliberate: adding a *derived* colour means editing one function
rather than every entry in this file, and it is what stops a new theme
from shipping a scrollbar the same colour as the card it scrolls.

Every theme here is checked as a test — WCAG AAA contrast for primary
text, AA for secondary, layer separation, and that a `mode = "dark"`
theme's surfaces actually get lighter as they rise. A theme that fails
any of those is a failing build, not a warning.

A user's own themes go in `%APPDATA%\rustaman\themes\`; giving one the
same `id` as a built-in replaces it. See the README for the format.

## `brand/` — generated, do not edit

The mark at six PNG sizes, the multi-size `.ico` that `build.rs` compiles
into the executable, and the wordmark. All of it comes from the single
definition in [`../src/brand.rs`](../src/brand.rs):

```sh
cargo run --example brand_assets
```

The release workflow regenerates these and fails on a diff, so a
hand-edited icon does not ship. An icon that has fallen out of step with
`brand.rs` is wrong in Explorer and right in the title bar, and nothing
else in the build would notice.

## `rustaman.manifest` — edit with care

The side-by-side manifest `build.rs` embeds. Every element in it corrects
something Windows otherwise gets wrong for an unmanifested application —
per-monitor DPI awareness, the UTF-8 code page, the supported-OS list
that stops `GetVersionEx` reporting Windows 8 forever, and
`asInvoker` so the app does not prompt for elevation on every launch. The
file carries a comment on each block saying what it buys.

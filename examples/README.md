# Examples

Two binaries that write into the source tree. That is the whole reason
they are `examples/` rather than `#[test]`s: `cargo test` should never
mutate the repository, so the parts of this project that *are* generated —
the brand art and the changelog — live behind `cargo run --example`
instead. Both are still compiled by `cargo clippy --all-targets`, so they
cannot rot silently the way a script nobody runs would.

## `brand_assets`

Regenerates the PNGs and the `.ico` in `assets/brand/` from the single
mark definition in [`src/brand.rs`](../src/brand.rs):

```sh
cargo run --example brand_assets
```

Every size is the same geometry the title-bar mark and the window icon
are drawn from, so the README art, the taskbar icon and the mark in the
app are one drawing by construction rather than by anyone remembering to
re-export one when the other changes. The `.ico` is the one that matters
most: `build.rs` compiles it into the executable, and it is what
Explorer, the taskbar and Alt-Tab read — a `.exe` with no icon resource
shows the generic blank page everywhere it is not running.

Regenerate the PNGs rather than editing them. The release workflow runs
this and fails on a diff, so a hand-edited icon does not ship.

## `changelog`

Regenerates `CHANGELOG.md` from the git history, grouping
conventional-commit subjects under the release tag that shipped them:

```sh
cargo run --example changelog
```

Check the committed file against the history without rewriting it — this
is what CI runs:

```sh
cargo run --example changelog -- --check
```

Or write the section for a release that is *about to be tagged*, so the
tag can be placed on a commit whose changelog is already final:

```sh
cargo run --example changelog -- --release v0.2.0
```

`CHANGELOG.md` is generated, never hand-edited — released sections are
rewritten on every run. See [`CONTRIBUTING.md`](../CONTRIBUTING.md) for
the release ordering, and the module docs in `changelog.rs` for what is
worth knowing before changing how it groups things.

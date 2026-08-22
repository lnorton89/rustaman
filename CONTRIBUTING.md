# Contributing to Rustaman

Thanks for considering a contribution. Rustaman is a Windows task manager
in Rust — an egui/eframe window over a hand-wrapped Win32 layer, with a
sampler thread in between.

Before you start, read [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) for
the module map. It is short, and the layering rule it describes (data
flows `win` → `engine` → `model` → `gui` and never back) is the one thing
that would be expensive to unpick later. Then
[`docs/WINDOWS_APIS.md`](docs/WINDOWS_APIS.md) if you are touching
anything that talks to Windows, and
[`docs/PERFORMANCE.md`](docs/PERFORMANCE.md) if you are touching anything
that runs while the window is open.

The [README](README.md) covers installation and use; this file is about
contributing.

## Reporting bugs and requesting features

Open an issue on the [issue tracker](https://github.com/lnorton89/rustaman/issues).
Security vulnerabilities are *not* reported there — see
[`SECURITY.md`](SECURITY.md) for the private channel.

For a bug report, include your Windows build (`winver` gives it), how you
installed the binary, whether you were running as administrator, and what
you did. A screenshot is usually the fastest way to convey a layout or
theming problem.

Before opening a feature request, it is worth checking two things: the
"what it deliberately does not do" section of the README, and the last
section of [`docs/WINDOWS_APIS.md`](docs/WINDOWS_APIS.md), which lists the
questions no Windows API can actually answer. Per-process network
throughput is the usual one.

## Building and running

Windows 10 1809 or later, 64-bit, and a Rust toolchain — the version is
pinned in `rust-toolchain.toml`, so `rustup` fetches the right one
automatically.

```powershell
cargo run                 # debug: keeps a console for panics and println!
cargo run --release       # what a user gets: no console, LTO, panic = abort
```

## Before you submit

Three commands, all clean:

```powershell
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
```

CI runs all three on Windows and on Linux, plus `cargo deny check` and
the changelog check. A warning is a build failure there.

### The part that catches people out

**Roughly four fifths of this crate does not compile on Linux or macOS.**
`src/win/`, `src/engine/` and `src/gui/` are `cfg(windows)`, and so are
`eframe`, `egui` and `egui_extras` — they are scoped to
`[target.'cfg(windows)'.dependencies]` in `Cargo.toml` precisely so that
the portable half can be tested anywhere.

That means a green `cargo test` on Linux has exercised the model, the
theming, the colour space, the formatting, the rate arithmetic and the
config parser, and has not compiled a single line of the Windows layer or
the window. If you are working off Windows:

```bash
rustup target add x86_64-pc-windows-msvc
cargo clippy --all-targets --all-features --target x86_64-pc-windows-msvc -- -D warnings
```

That type-checks the whole Windows half without linking anything, which
catches a broken `unsafe extern` block, a struct-layout assertion, or an
egui API that moved. It cannot run a test and it cannot tell you the
window looks right. **Anything visual, and anything that reads a real
machine, needs a Windows run and a screenshot in the pull request.**

CI is arranged the same way: a `windows` job that is the authority, and a
`portable` job on Linux whose value is in its *failure* — it says a
module that was supposed to have no Windows in it now does.

## Rules the codebase actually enforces

These are enforced by lints and build-time checks, so a violation fails
CI rather than getting caught in review. [`CLAUDE.md`](CLAUDE.md) has the
full list with the reasoning; the short version:

- **Every source file opens with a module header banner** — a ruled block
  naming `Module:`, `Description:`, and `Dependencies:`, then the `//!`
  docs. `src/header_check.rs` walks `src/`, `tests/` and `examples/` and
  fails the build for a file missing one, checking `Module:` against the
  path the file actually sits at.
- **No `unwrap`, `expect`, or `panic!` anywhere — including tests.** In
  library code use `let ... else`, `?`, or an explicit fallback. Tests
  return `anyhow::Result<()>` and use `?`.
- **Every `unsafe` block is a safe leaf wrapper.** One named function per
  FFI call taking safe Rust arguments; the `unsafe` block contains the
  call and nothing else; a `// SAFETY:` comment stating the reasoning it
  depends on; an owning `Drop` wrapper wherever there is a matching
  close/free/destroy. Callers and test bodies never need `unsafe`.
- **A hand-declared kernel struct is pinned by `const _: () = assert!()`
  on its size and field offsets.** A layout that disagrees with the
  kernel produces plausible wrong numbers rather than crashing, so this
  is the only thing that turns it into a build failure.
- **A process is `(pid, creation_time)`, never a bare PID.** Windows
  reuses PIDs between samples. Every action re-verifies the creation time
  before acting, so a kill cannot land on a process that took over the
  PID.
- **The UI thread makes no system call.** Everything drawn comes from a
  snapshot the sampler thread already built. This is why the window stays
  alive when the machine does not.
- **Nothing process-count-sized is recomputed in a draw call.** Derived
  data is cached and keyed off observed state (`RowKey`), not invalidated
  by hand. Tables use the virtualised `TableBody::rows`.
- **No literal colour in drawing code.** Everything comes from
  `palette()`; the one exception is the brand mark in `src/brand.rs`.
  Every theme is checked for WCAG contrast as a test, so a theme that
  fails is a failing build.
- **Spacing comes from the shared scale** — `SPACE_XS` / `SPACE_SM` /
  `SPACE_MD` / `SPACE_LG`. There is no literal spacing in `src/gui`; if
  you want a value between two steps, the answer is one of the two steps.
- **Constants live with what they govern**, and there is deliberately no
  `constants.rs`.
- **`match` arms list every variant** (`wildcard_enum_match_arm` is
  denied), and suppressions are `#[expect(..., reason = "...")]` rather
  than `#[allow]` so they cannot outlive what they suppressed.

## Generated files

Two things in this repo are generated and must not be hand-edited. Both
are `cargo run --example` rather than tests, because `cargo test` should
never mutate the repository — and both are still compiled by
`cargo clippy --all-targets`, so they cannot rot the way a script nobody
runs would.

```powershell
cargo run --example brand_assets    # assets/brand/ — the PNGs and the .ico
cargo run --example changelog       # CHANGELOG.md
```

`assets/brand/` comes from the single mark definition in `src/brand.rs`,
which is also what the window icon and the title-bar mark are drawn from.
The release workflow regenerates the assets and fails on a diff, so a
hand-edited icon does not ship.

## Changelog

`CHANGELOG.md` is rebuilt from the git history: every entry is a
conventional-commit subject, grouped under the release tag that shipped
it. That is the other reason the `fix:` / `feat:` / `refactor:` prefixes
matter — a subject that does not parse still appears, but in a catch-all
section rather than the right one.

CI runs the check form:

```powershell
cargo run --example changelog -- --check
```

It compares the **released** sections only. `Unreleased` is exempt on
purpose: it moves with every merge, and a squash or rebase rewrites the
very hashes it pins, so gating pull requests on it would fail all of them
for no signal.

### Cutting a release, in this order

1. On a release branch off `main`: bump `version` in `Cargo.toml` and
   commit.
2. Write the release's section **before** any tag exists:
   `cargo run --example changelog -- --release v0.2.0`, and commit it.
   The generator refuses a version that does not match `Cargo.toml`,
   already has a tag, or is not newer than the latest release.
3. Open the pull request and let the checks pass — the changelog check
   recognises a leading section whose tag does not exist yet and
   validates it as the release being cut, rather than calling it drift.
4. Merge (a merge commit, never a squash — the changelog pins individual
   commit hashes), then tag the merge commit, annotated:
   `git tag -a v0.2.0 -m "rustaman v0.2.0"`, and push the tag. That push
   triggers `.github/workflows/release.yml`.

The point of writing the section first is that the tag then names a tree
whose changelog is already finished — the source archive GitHub serves
for that tag is correct, permanently, and a tag is not editable after the
fact.

Two mechanics make the pre-tag section possible: commits touching only
`CHANGELOG.md` are excluded from every section (the changelog commit
cannot list itself, because its hash does not exist while the file is
being generated), and release dates are UTC with `--check` comparing
headings date-insensitively, so a tag landing across midnight cannot fail
its own check.

**Never amend or rebase a commit after tagging it.** The tag keeps
pointing at the copy you discarded, which then sits on no branch at all —
and since each release is computed as `<previous>..<tag>`, every range
from that point on is wrong. The changelog check fails on any version tag
no branch can reach, so it cannot go unnoticed.

Pushing a `v*` tag re-triggers the release workflow, which ends in
`gh release upload --clobber` — so re-pointing a tag on an already
*published* release rebuilds its assets and changes their checksums. If
that is not what you want, cancel the run.

## Dependency updates

Dependabot opens version-update PRs weekly for both crates and GitHub
Actions (`.github/dependabot.yml`). CI and `cargo deny check` run against
every one, so the check marks are the first thing to read.

One thing about that config is deliberate: **only patch bumps are
grouped.** Dependabot classifies an update by its literal version
segments, so egui `0.36 → 0.37` reaches it as a *minor* — while under
cargo that is exactly the breaking one, and almost everything here is
still `0.x`. Anything above patch therefore arrives as its own PR by
design. If you are batching upgrades by hand, batch them the same way.

Nothing is ignored, and nothing should be without a note saying what
would clear it — the same standard `deny.toml` applies to an ignored
advisory. `rust-toolchain.toml` and the `dtolnay/rust-toolchain` action
pin are outside what Dependabot can see; those move by hand, in their own
commit, and `tests/toolchain_pin.rs` fails the build if they disagree.

## Commit hygiene

- Keep each commit focused on one change, with a message that says *why*
  (`fix:`, `feat:`, `refactor:`, `docs:`, `test:`, `build:`, `ci:`
  prefixes match the existing history and feed the changelog).
- Stage only the files you actually changed, by name — not `git add -A`.
  `git commit` commits the whole index, so use `git commit -- path/to/file`
  when the tree holds work that is not yours.
- Line endings are handled by `.gitattributes`; every text file is stored
  and checked out as LF, including on Windows. Don't convert endings by
  hand — git normalises on the way in.

## Opening a pull request

Against `main`, with a title that summarises the change and a body that
says what and why. The template asks which platform you tested on; please
answer it honestly, because a clean Linux run genuinely does not cover
most of this crate. UI changes want a screenshot.

Small, focused PRs are easier to review and land faster than large ones.

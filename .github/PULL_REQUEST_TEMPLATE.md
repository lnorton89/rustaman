## Summary

What this change does and why. One or two sentences — the *why* matters more
than the *what*. If this fixes an issue, reference it here (e.g. `Closes #12`).

## Test plan

What you ran, and on what. `cargo test`, `cargo clippy --all-targets
--all-features -- -D warnings`, and `cargo fmt --all -- --check` must be clean
before this is mergeable — a warning is a build failure in CI.

Say whether you ran it **on Windows**. The portable half of this crate (model,
theme, format, colour, config, rates) builds and tests anywhere, and CI runs it
on Linux for exactly that reason — but everything in `src/win/`, `src/engine/`,
and `src/gui/` is `cfg(windows)` and compiles to nothing elsewhere, so a clean
run on Linux proves considerably less than it looks. A change that touches a
Win32 call, a sampler path, or anything drawn needs a Windows run and, for a
visible change, a screenshot.

## Checklist

- [ ] `cargo fmt --all -- --check` passes
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` passes
- [ ] `cargo test --all-features` passes
- [ ] No new `unwrap` / `expect` / `panic!` — not even in tests
- [ ] New files carry the module header banner (`Module:` / `Description:` /
      `Dependencies:`) and the matching `mod` declaration
- [ ] Every new `unsafe` block is inside a safe leaf wrapper, contains only the
      FFI call, and carries a `// SAFETY:` comment
- [ ] Any action on a process is addressed by `ProcessKey`, not by a bare PID
- [ ] No literal colour and no hand-picked pixel gap in drawing code
- [ ] UI changes include a screenshot
- [ ] Commit message uses the `fix:` / `feat:` / `refactor:` / `docs:` /
      `test:` / `build:` prefixes

Full guidelines, including how to build and what CI enforces, are in
[`CONTRIBUTING.md`](CONTRIBUTING.md).

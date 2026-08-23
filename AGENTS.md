# Rustaman agent instructions

Read `docs/ARCHITECTURE.md` before changing module boundaries and
`docs/WINDOWS_APIS.md` before changing Windows integration code. Preserve
unrelated dirty-worktree changes; never revert another contributor's edits.

## Unsafe Rust is a leaf boundary, without exceptions

- First try to remove the unsafe operation. Prefer safe standard-library
  APIs, `Default`, checked slices, explicit byte encoding, and RAII owners.
- Required unsafe code belongs only under `src/win/`. Portable, engine,
  GUI, binary, example, and test code must call safe wrappers.
- Put each required FFI call, union read, unaligned read, raw-pointer read,
  or release call in its own small, named, private **safe** function. That
  leaf contains one unsafe operation and nothing else unsafe.
- Marshal strings, check lengths/ranges/alignment, perform arithmetic, and
  handle errors in safe code before or after the leaf operation.
- Every unsafe operation needs an immediately relevant `// SAFETY:` proof
  describing pointer validity, lifetime, size/alignment, initialization,
  ownership, and call count as applicable.
- Do not expose `unsafe fn`. The only admissible non-safe declarations are
  ABI-required `unsafe extern` blocks/callbacks and a genuinely necessary
  unsafe trait implementation; document why the ABI/trait requires it and
  keep all callback logic in a safe function.
- Any resource with a matching close/free/destroy API gets one non-`Copy`
  owning type whose `Drop` calls a safe leaf release wrapper exactly once.
- Never add unsafe to a test to fabricate input. Build bytes explicitly or
  test through the safe owner/wrapper.
- Never silence an unsafe lint. `Cargo.toml` and `src/unsafe_check.rs`
  enforce this policy; run the full Windows Clippy and test gates after a
  change.

Before handoff, run:

```powershell
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
```

`CLAUDE.md` and `CONTRIBUTING.md` contain the remaining project rules.

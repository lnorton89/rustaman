# Integration tests

What `cargo test` runs *after* the unit tests inside `src/`. They compile
as separate crates against the library, so they cover seams a unit test
cannot reach. They follow the same house rules as everything else: no
`unwrap`, `expect`, or `panic!`, `anyhow::Result<()>` from each test, and
assertions carry a message.

Most of this project's tests are *not* here. The model, the theming, the
colour space and the rate arithmetic are pure functions over data, so
they are tested in place, next to what they test — that is where the
bulk of the suite lives, and it is the part that runs on any platform.

## `toolchain_pin.rs`

Checks that `rust-toolchain.toml` and both CI workflows name the same
Rust version, and that no workflow has gone back to a floating
`@stable`. Three places, one fact — which is the shape that drifts.

It is not hypothetical. A workflow on `@stable` moves to each new Rust
release on its own while a local checkout stays where it is, and the
symptom is a lint firing in CI that cannot fire locally: a red build on a
morning nobody changed anything.

This test reads the repo's own config files rather than running a binary,
so it costs nothing and runs everywhere.

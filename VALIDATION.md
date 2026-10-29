# Validation

## Current voice-chat branch

The protocol framing implementation and its Rust unit tests have been added, but
they have **not been compiled or run in this environment** because Rust/Cargo is
not installed here. The CI workflow should validate formatting, Clippy, Cargo
tests, and the native-mixin fixture after this branch is pushed.

Pending checks for this revision:

- `cargo fmt --all --check`
- `cargo clippy --workspace --all-targets --locked --offline -- -D warnings`
- `cargo test --workspace --locked --offline`
- `python3 tools/test_template.py`
- `python3 tools/test_stage.py --horizon ../horizon`
- Native Horizon `check` and `test` using the pinned Horizon checkout.

## Scope of validation

The tiny native-mixin fixture tests that the injected Rust runtime compiles and
that the original method body/result are preserved. It does not provide
microphone capture, voice audio playback, an authenticated transport, a server
relay, or multiplayer end-to-end evidence.

The template's earlier validation results applied to the inherited example
mod before voice protocol changes. Do not treat those historical results as
validation of this branch or as proof that voice chat works.

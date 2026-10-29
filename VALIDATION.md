# Validation

## Automated checks

GitHub Actions passed for the voice-chat foundation on 2026-10-29:
[workflow run 40889070675](https://github.com/Knaaaaaf/horizon-voice-chat/actions/runs/40889070675),
associated with branch commit 209c5c66fe5c0aa0ceb2fecf44256cbfd66fc84f.

- `cargo fmt --all --check` — passed.
- `cargo clippy --workspace --all-targets --locked --offline -- -D warnings` — passed.
- `cargo test --workspace --locked --offline` — passed.
- `python3 tools/test_template.py` — passed; the actual mixin bundle compiled in
  the tiny fixture, the original method/result stayed intact, and source-audit
  plus deterministic-staging checks passed.

These checks ran on a GitHub-hosted Linux runner with Rust 1.95.0. They verify
the standalone packet/proximity helpers and the native-mixin staging fixture;
they are not a gameplay or audio integration test.

## Still unverified

- Full build/check/test against the complete pinned Horizon source export.
- Microphone capture, audio playback, codec integration, real UDP transport,
  authenticated sessions, server relay, and proximity filtering in multiplayer.
- Compatibility on Windows/macOS or with a live Minecraft server.

The pinned Horizon mod API still lacks a mod-facing microphone capture
interface and a client/server networking channel for mod halves. The code in
this repository is a tested protocol/geometry foundation, **not a functional
voice-chat mod yet**.

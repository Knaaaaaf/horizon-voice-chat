# Validation

## Latest code validation

GitHub Actions run 40964713173 passed for code commit
73037fce593fe1f9e11ca8b2e3bc9c6305522eb3:
https://github.com/Knaaaaaf/horizon-voice-chat/actions/runs/40964713173

- `cargo fetch --locked` — passed.
- `cargo fmt --all --check` — passed.
- `cargo clippy --workspace --all-targets --locked --offline -- -D warnings` — passed.
- `cargo test --workspace --locked --offline` — passed.
- `python3 tools/test_template.py` — passed.

## Scope of checks

The standalone Rust tests cover packet framing, sequence guards, proximity
selection, jitter-buffer ordering/loss skips, and token-bucket refill. The
native-mixin fixture checks that the runtime can be staged into the pinned
source-mixin path. Even a fully green fixture is not a gameplay/audio test.

## Still unverified

- Full native Horizon source export and gameplay build/test.
- Microphone capture, audio playback, codec integration, real UDP transport,
  authenticated sessions, server relay, and multiplayer proximity audio.
- Compatibility on Windows/macOS or with a live Minecraft server.

The pinned Horizon mod API does not provide a mod-facing microphone interface
or a client/server channel for mod halves. This remains a tested protocol and
geometry foundation, **not a functional voice-chat mod yet**.

# Horizon Voice Chat

A native Rust voice-chat port for Horizon, inspired by
[Simple Voice Chat](https://github.com/henkelmax/simple-voice-chat).

> **Status: early scaffold — not a working voice-chat mod yet.** This repository
> currently starts from the Horizon native-mixin template. It does not capture
> microphone audio, play voice audio, transmit packets, or connect clients to a
> voice server. See [VOICE_CHAT_PORT.md](docs/VOICE_CHAT_PORT.md) for the
> implementation plan and current API blockers.

## Target features

- Proximity voice with configurable distance and spatial audio.
- Push-to-talk, mute, and clear microphone/connection status.
- A dedicated voice transport with versioned packets and server-side validation.
- Configurable server address/port and sensible connection timeouts.
- Explicit privacy controls: microphone capture only when the player enables it.

## Important architecture note

The upstream Simple Voice Chat project is a Java mod with its own client/server
integration. Horizon is a Rust rewrite, so its Java mod cannot be loaded directly.
A real port needs supported Horizon hooks for client audio capture/playback,
networking, player identity/positions, and server-side voice handling. The
current template's block-break hook is only a starter example; it is not voice
chat functionality.

## Build the current scaffold

This repository follows the native `horizon-mixins` source-mixin path and builds
a custom Horizon executable; it is not an ordinary `.jar` or sandboxed `.hmod`
mod. Requirements and pinned source revisions are documented below and in
[compatibility.json](compatibility.json).

```sh
cargo fetch --locked
cargo test --workspace --locked --offline
cargo fmt --all --check
python3 tools/test_template.py
```

For the full native build workflow, pinned dependencies, and validation
requirements, see the original template instructions below in the repository
history or consult [VOICE_CHAT_PORT.md](docs/VOICE_CHAT_PORT.md) before attempting
to run it. Do not treat a successful template build as proof that voice chat
works.

## Reference

- [Simple Voice Chat](https://github.com/henkelmax/simple-voice-chat)
- [Horizon](https://github.com/horizon-rust/horizon)
- [Horizon Mixins](https://github.com/horizon-rust/horizon-mixins)

License: MIT (see [LICENSE](LICENSE)).

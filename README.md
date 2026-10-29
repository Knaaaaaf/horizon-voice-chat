# Horizon Voice Chat

A native Rust voice-chat port for Horizon, inspired by
[Simple Voice Chat](https://github.com/henkelmax/simple-voice-chat).

> **Status: protocol foundation only — not a working voice-chat mod yet.**
> This branch includes a bounded packet codec, jitter buffer, proximity/rate-limit
> helpers, and an optional cpal microphone-capture backend. Capture is not wired
> to an in-game control or transport yet; it does not play or transmit voice.

## Implemented

- Versioned packet envelope with a 14-byte header and 1200-byte datagram cap.
- Validation for magic, version, packet kind, reserved flags and payload size.
- Monotonic per-session sequence guard with duplicate/stale rejection and
  u32 wraparound handling.
- Pure 3D proximity filtering that excludes the speaker and rejects invalid
  coordinates/ranges.
- Bounded reorder buffer for late/out-of-order frames, with explicit missing-frame
  skipping controlled by a future playout scheduler.
- Deterministic token-bucket rate limiter to prepare per-session relay limits.
- Optional cpal input capture, started only when explicitly invoked, with
  non-blocking 10 ms mono PCM16 frame assembly and bounded queue backpressure.
- Unit tests for protocol, proximity, jitter-buffer, rate-limit, and PCM framing.

See [the protocol specification](docs/protocol-v1.md) for exact fields and
known security/compatibility limits.

## Planned features

- Proximity voice with configurable distance and spatial audio.
- Push-to-talk, mute, and clear microphone/connection status.
- Authenticated client/server transport and server-side relay.
- Wire capture to an explicit push-to-talk/mute control, transport, playback, and a supported codec.
- Explicit privacy controls: microphone capture only after the player enables it.

## Important architecture note

Horizon is a Rust rewrite with its own mod API. The Java jar from Simple Voice
Chat cannot be loaded directly. The pinned Horizon mod API still lacks a
mod-facing client/server networking channel and microphone capture interface.
Those integration points must be implemented upstream or made available before
a working port can be completed. See [the port roadmap](docs/VOICE_CHAT_PORT.md).

## Native scaffold

This repository follows the horizon-mixins source-mixin path and builds a
custom Horizon executable; it is not an ordinary jar or sandboxed hmod mod.
The inherited block-break hook is a temporary smoke test for native module
injection, not a voice feature. Requirements and pinned source revisions are
documented in compatibility.json.

## Build and validate

Run cargo fetch --locked, cargo test --workspace --locked --offline,
cargo fmt --all --check, and python3 tools/test_template.py on a Linux
development machine with the pinned horizon-mixins checkout available.

GitHub Actions passed formatting, Clippy, Rust tests and the native-mixin
fixture for this foundation. Those checks do not prove that audio or voice
networking works; full native/gameplay and microphone tests remain pending.

## Reference

- [Simple Voice Chat](https://github.com/henkelmax/simple-voice-chat)
- [Horizon](https://github.com/horizon-rust/horizon)
- [Horizon Mixins](https://github.com/horizon-rust/horizon-mixins)

License: MIT (see LICENSE).

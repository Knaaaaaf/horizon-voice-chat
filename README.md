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
  non-blocking ~10 ms mono PCM16 frame assembly and bounded queue backpressure.
- AudioFrame PCM16 payload codec with sample-rate and frame-length validation.
- Unit tests for protocol, proximity, jitter-buffer, rate-limit, and PCM framing.

Audio frames are currently uncompressed PCM16, so bandwidth use is high and the
capture backend is not wired to an in-game control or transport. See [the
protocol specification](docs/protocol-v1.md) for exact fields and known limits.

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

## In-game GUI companion

The `ui-companion/` directory is a separate sandboxed Horizon `.hmod`
component built against the exact pinned Horizon SDK revision. It adds:
- A V-key menu, rebindable in Options > Mods.
- In-game controls for microphone enabled, mute, push-to-talk mode and a
  proximity-range preference.
- A compact top-right microphone status indicator.
- Persistent settings through the mod storage capability.

Build it with the Horizon authoring tool from the companion directory:
`cd ui-companion && cargo horizon-mod build`. The resulting `.hmod` is
installed through Horizon's normal mod folder/manager. This UI component is
separate from the native source-mixin executable.

**Important limitation:** these controls are currently UI/state only. The
pinned Horizon 0.1 sandbox API has no microphone-capture or mod-owned
client/server networking capability, so toggling the microphone here does not
yet start the native cpal capture stream or send voice. The UI is an actual
Horizon screen/settings page, but end-to-end voice chat still needs a host
bridge and server relay.

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

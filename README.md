# Horizon Voice Chat

A native Rust voice-chat port for Horizon, inspired by
[Simple Voice Chat](https://github.com/henkelmax/simple-voice-chat).

> **Status: native capture is wired to the GUI; network voice is not connected yet.**
> The native mixin polls the companion's own Horizon storage files, honors explicit
> mic enable/mute and PTT state, and starts/stops the optional cpal capture stream.
> Captured frames are drained locally; there is still no authenticated transport,
> server relay, codec compression, or playback.

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
- Optional cpal input capture controlled by the GUI companion via the native
  game-tick bridge; microphone capture is disabled by default and stops on mute/off.
- Bounded storage bridge for mic-enabled, mute, PTT mode/key-down, proximity range,
  and a native backend status message read by the in-game GUI.
- AudioFrame PCM16 payload codec with sample-rate and frame-length validation.
- Unit tests for protocol, proximity, jitter-buffer, rate-limit, and PCM framing.

Audio frames are currently uncompressed PCM16. The native backend captures and
drains frames locally but does not transmit them. See [the protocol specification](docs/protocol-v1.md)
for exact fields and known limits.

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

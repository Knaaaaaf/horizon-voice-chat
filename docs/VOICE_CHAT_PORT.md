# Voice chat port: design and implementation status

## Status

The repository now contains a first, testable protocol primitive: a bounded
14-byte packet header codec and a per-session sequence guard in the runtime.
Tests cover round trips, size limits, malformed headers and sequence
replay/staleness. The tests are committed but have not yet been run in this
environment because Rust/Cargo is not installed here.

This is still **not a working voice-chat mod**. It has no microphone capture,
Opus pipeline, network socket, authentication/encryption, audio playback,
spatial sound integration, or server relay. The inherited block-break hook is
only a native-mixin staging smoke test.

## Horizon API blockers

The repository's pinned Horizon revision is
028f60881678a9a0e3e198f693a82858fa475c76. Its own
[mod API documentation](https://github.com/horizon-rust/horizon/blob/028f60881678a9a0e3e198f693a82858fa475c76/docs/api/mods.md)
lists client APIs for game state, chat, HUD and storage, and experimental server
mods. It explicitly lists a client/server channel for a mod's two halves as work
that is still needed. It exposes no mod-facing microphone capture interface.
A Java Simple Voice Chat jar cannot be loaded by Horizon.

These gaps must be solved or supported by upstream APIs before a playable
end-to-end port can be claimed. Avoid guessing API names or using text-rewrite
patches around the Rust source.

## Intended architecture

1. **Client audio adapter** — request microphone access only after explicit
   user input; capture frames; support push-to-talk and mute; stop capture when
   disconnected or disabled.
2. **Codec and frame pipeline** — pick an available, compatible codec; bound
   frame sizes and queues; handle packet loss without unbounded buffering.
3. **Authenticated transport** — negotiate protocol and codec; bind packets to
   a verified player/session identity; apply timeouts and replay protection.
4. **Server relay** — validate packet sizes, rate-limit senders, and relay only
   to eligible nearby players using server-authoritative positions.
5. **Spatial playback** — derive distance and direction from game state; apply
   attenuation and stop playback when a player leaves range.
6. **Settings and diagnostics** — server address/port, push-to-talk binding,
   mute controls, connection state, and non-sensitive error logs.

## Implemented packet framing

See [protocol-v1.md](protocol-v1.md) for the experimental packet layout and its
limits. The header codec is deliberately independent of any socket, audio
backend or Horizon game hook, so it can be unit-tested in isolation. It is not
Simple Voice Chat wire-compatible and must not be treated as secure networking.

## Suggested milestones

- [x] M1a — add bounded packet framing, sequence guarding, and pure 3D proximity selection with unit tests.
- [ ] M1b — run Rust tests/Clippy and verify the native staging fixture.
- [ ] M2 — confirm supported client microphone/playback and mod-network APIs.
- [ ] M3 — establish a versioned, authenticated two-client relay on a test server.
- [ ] M4 — add microphone capture, push-to-talk, codec, and playback.
- [ ] M5 — add proximity filtering and spatial attenuation.
- [ ] M6 — test disconnects, malformed packets, rate limits, mute/privacy, and
      supported operating systems.

A complete port also needs a server component; a client-only implementation
cannot make players hear one another on a remote server.

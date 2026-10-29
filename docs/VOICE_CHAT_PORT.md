# Voice chat port: design and implementation status

## Status

The repository now has three pure Rust building blocks:
- A bounded 14-byte packet framing codec with strict header/length validation.
- A per-session sequence guard that rejects duplicates and stale sequence numbers.
- A bounded jitter/reorder buffer plus 3D proximity recipient selection.

The tests cover serialization boundaries, malformed input, sequence wraparound,
out-of-order delivery, buffer bounds, missing-frame skips, and invalid positions.
GitHub Actions has passed formatting, Clippy, unit tests, and the native-mixin
fixture for the prior foundation commit; this latest jitter-buffer change is
waiting for its own CI run.

This is still **not a working voice-chat mod**. It has no microphone capture,
Opus pipeline, network socket, authentication/encryption, audio playback,
spatial sound integration, or server relay. The inherited block-break hook is
only a native-mixin staging smoke test.

## Horizon API blockers

The repository's pinned Horizon revision is
028f60881678a9a0e3e198f693a82858fa475c76. Its [mod API documentation](https://github.com/horizon-rust/horizon/blob/028f60881678a9a0e3e198f693a82858fa475c76/docs/api/mods.md)
describes sandboxed WebAssembly mods with game/chat/HUD/storage APIs and
experimental server mods. It does not expose a mod-facing microphone capture
interface or a client/server networking channel for a mod's two halves. A Java
Simple Voice Chat jar cannot be loaded by Horizon.

Those integration points need upstream support or a separate, well-defined
native host extension before a playable end-to-end port can be completed. Avoid
guessing API names or patching source with string-based rewrites.

## Intended architecture

1. **Client audio adapter** — request microphone access only after explicit
   user input; capture frames; support push-to-talk and mute; stop capture when
   disconnected or disabled.
2. **Codec and frame pipeline** — select a supported codec; bound frame sizes
   and queues; handle packet loss without unbounded buffering.
3. **Authenticated transport** — negotiate protocol and codec; bind packets to
   verified session identity; apply timeouts and replay protection.
4. **Server relay** — validate sizes, rate-limit senders, and relay only to
   eligible nearby players using server-authoritative positions.
5. **Spatial playback** — derive distance/direction from game state; attenuate
   audio and stop playback when a player leaves range.
6. **Settings and diagnostics** — server address/port, push-to-talk binding,
   mute controls, connection state, and non-sensitive error logs.

## Implemented core

See [protocol-v1.md](protocol-v1.md) for the experimental wire framing. The
jitter buffer is deliberately clock-free: a future playout scheduler decides
when a missing frame has waited long enough and calls `skip_missing`. The
proximity helper is a pure geometry function; it does not authenticate players
or send/relay any packets. None of these components is a secure transport.

## Milestones

- [x] M1a — bounded packet framing and sequence guard.
- [x] M1b — pure 3D proximity selection.
- [x] M1c — bounded reordering buffer and explicit missing-frame skip.
- [x] CI — formatting, Clippy, Rust unit tests and native-mixin fixture passed
      for the foundation revision.
- [ ] M2 — confirm supported client microphone/playback and mod-network APIs.
- [ ] M3 — establish a versioned, authenticated client/server relay.
- [ ] M4 — add microphone capture, push-to-talk, codec, and playback.
- [ ] M5 — wire proximity filtering and spatial attenuation into live multiplayer.
- [ ] M6 — test disconnects, malformed packets, rate limits, mute/privacy, and
      supported operating systems.

A complete port needs a server component; a client-only implementation cannot
make players hear one another on a remote server.

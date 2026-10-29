# Voice chat port: design and implementation status

## Status

This is a planning/scaffolding stage, not an implemented voice-chat system.
The repository currently contains the native-mixin example's block-break hook.
There is no microphone capture, audio codec pipeline, voice packet transport,
spatial playback, or voice server integration yet.

The Horizon project has documented the main prerequisites for a proximity voice
mod: server-side mod support, mod networking, and an audio interface exposed to
mods. These must be verified against the exact Horizon revision before coding
against them. Do not substitute guessed APIs or unsafe source-text rewriting.

## Intended architecture

1. **Client audio adapter** — request microphone access only after explicit
   user input; capture frames; support push-to-talk and mute; stop capture when
   disconnected or disabled.
2. **Codec and frame pipeline** — encode voice frames using a codec supported by
   the available Horizon build; bound frame sizes and queues; handle packet loss
   without unbounded buffering.
3. **Versioned transport** — define a protocol version, frame type, sequence
   number, bounded payload length, and authenticated player/session identity.
   Never trust a client-provided player position or identity.
4. **Server relay** — validate membership and packet sizes, rate-limit senders,
   and relay only to eligible nearby players. Keep voice transport separate from
   Minecraft's ordinary chat messages.
5. **Spatial playback** — derive distance and direction from authoritative game
   state; apply attenuation and stop playback when a player leaves range.
6. **Settings and diagnostics** — server address/port, push-to-talk binding,
   mute controls, connection state, and non-sensitive error logs.

## Compatibility gates

Before the first end-to-end prototype, confirm all of the following against a
pinned Horizon revision:

- A supported client audio capture and playback API is available to mods.
- A supported way to send/receive mod packets exists on the client.
- The server can load compatible mod logic and relay voice data.
- The client can access authoritative player identity, positions, and lifecycle.
- The chosen codec and its licensing/build requirements are compatible with the
  target platforms.

If any gate is missing, the next step is upstream API work or a clearly labelled
prototype, not claiming a functional voice-chat port.

## Suggested milestones

- [ ] M1 — verify audio, networking, and server mod APIs for a pinned revision.
- [ ] M2 — implement and unit-test a versioned, bounded voice-frame format.
- [ ] M3 — establish a two-client server relay with no audio playback yet.
- [ ] M4 — add microphone capture, push-to-talk, codec, and playback.
- [ ] M5 — add proximity filtering and spatial attenuation.
- [ ] M6 — test disconnects, malformed packets, rate limits, mute/privacy, and
      supported operating systems.

## Reference implementation

[Simple Voice Chat](https://github.com/henkelmax/simple-voice-chat) is a
reference for user-facing features and protocol behavior, not a drop-in Rust
dependency. Its Java/Fabric/Forge integration cannot be loaded by Horizon.

# Horizon Voice Chat

A native Rust voice-chat prototype for Horizon, inspired by
[Simple Voice Chat](https://github.com/henkelmax/simple-voice-chat). It is not
wire-compatible with the Java mod.

> **Status: experimental HTTPS room relay and native PCM playback implemented; not yet a drop-in Simple Voice Chat replacement.**
> The native mixin connects the in-game GUI to cpal capture, and can send/receive
> bounded PCM16 frames through the optional authenticated HTTPS relay. The
> server currently broadcasts within an allowlisted room; the proximity slider
> does not yet filter recipients by server-authoritative world positions.

## Implemented

- Bounded HVC1 packet framing: 14-byte header, maximum 1200-byte packet.
- PCM16 mono frames of approximately 10 ms, validated sample rate/frame size.
- Sequence/replay checks, bounded jitter/reorder structures and rate-limit helpers.
- GUI companion: V opens the settings page; the Push-to-talk key defaults to C
  and can be rebound in Options > Mods.
- Explicit microphone opt-in, mute, PTT or basic energy-gate voice activation,
  persistent settings and a native backend status indicator.
- Native \`Game::tick\` hook reads the companion's official Horizon per-mod
  storage files and starts/stops the optional cpal capture.
- Authenticated client enrollment with unique bearer tokens, TLS-only transport,
  bounded requests/responses, per-client rate limits and an in-memory room relay.
- Native speaker output using cpal, bounded playback queue and time-slot mixing.
- CI checks for the WASM UI guest, native audio-enabled Horizon build, mixin
  fixture and relay binary-protocol tests.

## Important limits

This is an experimental prototype, not a verified release. PCM is uncompressed
(no Opus), so a 48 kHz mono stream is about 96 kB/s of raw samples per speaker
before HTTPS framing. The relay is **room-wide**, not proximity-aware: all
clients with credentials for the same room can hear each other. The distance
setting is saved, but live server-authoritative position filtering/spatial
attenuation is not connected yet. Basic voice activation is an RMS threshold,
not a robust speech detector. Audio device operation still needs real
multiplayer/platform testing.

The stock Horizon sandbox does not expose microphone capture or mod-owned
sockets to \`.hmod\` code. The local native source-mixin supplies capture and
networking; installing the GUI companion alone is not enough. The Java
Simple Voice Chat jar cannot be loaded directly.

## Build and test

The native mixin follows the pinned \`horizon-mixins\` source-mixin path. See
[\`compatibility.json\`](compatibility.json) for exact revisions. Build/check the
native audio-enabled Horizon tree with:

\`\`\`sh
python3 tools/horizon.py check --horizon ../horizon --audio --target-dir target/horizon-audio-cargo
\`\`\`

Build the standalone Horizon GUI companion from \`ui-companion/\`:

\`\`\`sh
cd ui-companion
cargo horizon-mod build
\`\`\`

Install the resulting \`.hmod\` using Horizon's normal mod manager/folder. The
companion requires the native source-mixin executable for actual microphone
capture, transport and playback.

## Configure a relay

See [\`docs/RELAY.md\`](docs/RELAY.md) for TLS certificate requirements,
client-token allow-list setup and the required client environment variables.
The server is a separate process. Without relay configuration, microphone
capture remains local-only. Mic defaults to off; enable it in the V menu and
hold C while speaking when Push-to-talk is selected.

## Design notes

- [Protocol framing](docs/protocol-v1.md): the internal packet envelope and PCM format.
- [Native bridge](docs/NATIVE_BRIDGE.md): GUI storage, native capture and playback flow.
- [Port roadmap](docs/VOICE_CHAT_PORT.md): remaining proximity, codec and integration gaps.

License: MIT.

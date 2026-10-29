# Native GUI/audio bridge

The UI companion remains a sandboxed Horizon `.hmod`; it cannot import a
microphone or socket API that the pinned Horizon 0.1 SDK does not offer. The
native mixin supplies a narrow control plane by reading only the companion's
own Horizon storage files.

## Data flow

- The GUI stores `mic_enabled`, `muted`, `ptt`, `ptt_down`, and `distance`
  using Horizon's regular per-mod storage API.
- The injected `Game::tick` hook polls those files every 10 game ticks.
- With microphone enabled and not muted, an audio-enabled Horizon build starts
  `cpal` capture and drains bounded mono PCM16 frames without blocking the game
  tick. Turning the mic off or muting drops the stream.
- The native backend writes `backend_status` into the same per-mod data folder.
  The GUI reads that value through its normal Horizon storage API and updates the
  in-game status indicator.
- The microphone defaults to disabled. The UI resets PTT-held state on startup
  and when its screen closes to avoid a stuck transmit gate.

This is a local integration only. PCM frames are currently discarded after the
capture queue is drained. No packets leave the client; PTT gating is not yet
connected to a network sender; distance is persisted for the future relay but
does not yet filter live recipients. The status deliberately says the relay is
offline.

## Storage path

The host SDK stores data in
`<horizon-mods>/.horizon-mod-data/horizon_voice_chat_ui/`. The native helper
searches the current working directory and executable directory, including a few
parent directories, for a `horizon-mods` folder. If profile discovery does not
match the launcher setup, set one of these environment variables before starting
the game:

- `HORIZON_VOICE_CHAT_MODS_DIR`: absolute path to the directory containing
  Horizon `.hmod` files.
- `HORIZON_VOICE_CHAT_SETTINGS_DIR`: absolute path to the exact
  `.horizon-mod-data/horizon_voice_chat_ui` directory; this takes precedence.

The bridge reads only these known preference keys and writes only the
`backend_status` text file. It does not read or persist microphone samples.
Use a build with Horizon's existing `audio` feature enabled to compile the
`cpal` capture path; without it, the UI reports that native audio was not built.

## Still required for multiplayer

A real multiplayer implementation still needs an authenticated, encrypted
transport/session handshake, server-side validation and proximity-based relay,
a codec suitable for voice bandwidth, and client playback with jitter buffering.
The Horizon sandbox does not expose client/server sockets; this must stay in the
native layer or be added as an explicit upstream host capability. Do not
expose a public UDP relay without authentication, encryption, and strict packet
and rate limits.

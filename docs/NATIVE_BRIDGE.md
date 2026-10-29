# Native GUI/audio bridge

The UI companion is a sandboxed Horizon \`.hmod\`; it cannot directly capture
microphone audio or open sockets. The native source-mixin connects its official
per-mod storage files to the cpal microphone, optional HTTPS relay and speaker
output. Installing the GUI companion by itself only provides the settings UI.

## Data flow

- The GUI stores \`mic_enabled\`, \`muted\`, \`ptt\`, \`ptt_down\`, and \`distance\`
  using Horizon's regular per-mod storage API.
- The injected \`Game::tick\` hook reads the small preference files each game
  tick, so quick push-to-talk press/release events are less likely to be missed.
  It retries profile-directory discovery twice per second if necessary.
- The microphone defaults to disabled. When enabled and not muted, a native
  build with the existing \`audio\` feature starts \`cpal\` capture and drains
  bounded ~10 ms mono PCM16 frames. Turning the microphone off or muting drops
  the input stream. The capture callback uses a bounded non-blocking queue.
- When relay environment configuration is present, the native layer validates
  each frame, applies the PTT or simple RMS-level gate, batches frames onto a
  bounded background queue, and sends them to the configured HTTPS relay.
- Incoming room frames are validated and grouped by server time slot, passed
  through a short bounded playout delay, summed/clipped, and queued to cpal
  output. The speaker queue is bounded to avoid unbounded latency.
- The native backend writes \`backend_status\` in the same per-mod data folder.
  The GUI reads it with the regular Horizon storage API and displays connection,
  capture and error state.

Voice activation is a simple RMS level threshold, not a robust speech detector.
The relay and audio output run on a separate worker thread; the game tick does
not perform synchronous HTTP requests.

## Storage path

Horizon stores the companion's data in
\`<horizon-mods>/.horizon-mod-data/horizon_voice_chat_ui/\`. The native helper
searches the current working directory and executable directory, including
several parents, for the \`horizon-mods\` folder. If this does not match a custom
launcher/profile, set one of these variables before starting the game:

- \`HORIZON_VOICE_CHAT_MODS_DIR\`: absolute path to the directory containing
  Horizon \`.hmod\` files.
- \`HORIZON_VOICE_CHAT_SETTINGS_DIR\`: absolute path to the exact
  \`.horizon-mod-data/horizon_voice_chat_ui\` data directory; this wins over the
  directory search.

The native bridge reads only its known preference keys and writes only
\`backend_status\` to the companion folder. Microphone samples are not written
to those files.

## Enable multiplayer relay

Follow [RELAY.md](RELAY.md) to configure the TLS server allow-list and set the
client's \`HORIZON_VOICE_CHAT_RELAY_URL\`, \`HORIZON_VOICE_CHAT_RELAY_TOKEN\`,
\`HORIZON_VOICE_CHAT_CLIENT_ID\` and \`HORIZON_VOICE_CHAT_ROOM\` before launching
Horizon. If none of the relay variables are set, no relay worker starts and
captured frames are discarded locally. If only part of the configuration is
set, the native status reports the missing/invalid settings.

The Rust client requires an HTTPS URL ending exactly in \`/v1/frames\`, a token
of at least 32 URL-safe characters, and simple room/client IDs. TLS certificate
validation is enabled; redirects and plaintext HTTP are not allowed.

## Current boundary

The relay broadcasts to an authorized room rather than filtering by actual
Minecraft/Horizon proximity. The GUI distance setting is persisted but does not
yet affect live recipient selection. The server does not yet query authoritative
player positions or session identity from the game server. PCM is uncompressed
and not wire-compatible with Simple Voice Chat's Java implementation. Real
multiplayer, audio-device and cross-platform testing remains necessary.

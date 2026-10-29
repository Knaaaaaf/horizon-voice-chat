# HTTPS room relay setup

This service is an experimental relay for the native Horizon Voice Chat mixin.
It uses HTTPS only and an explicit client/token/room allow-list. It is not a
Minecraft server plugin and does not discover Minecraft player positions, so
it currently relays between all authorized clients in the same room.

## Requirements

- Python 3.10 or newer (the server uses only the Python standard library).
- A valid TLS certificate and private key for the hostname clients will use.
- A separate, unique bearer token for each enrolled client. Keep the client
  allow-list and private key secret.
- The native Horizon executable built with the \`audio\` feature enabled.
  The sandboxed \`.hmod\` UI alone cannot make network connections or capture audio.

## Start the server

Create a JSON allow-list. Each client gets one unique token and may join only
the room IDs assigned to it:

\`\`\`json
{
  "alice": {
    "token": "REPLACE_WITH_A_UNIQUE_32_PLUS_CHARACTER_URL_SAFE_TOKEN",
    "rooms": ["survival"]
  },
  "bob": {
    "token": "REPLACE_WITH_A_DIFFERENT_UNIQUE_TOKEN",
    "rooms": ["survival"]
  }
}
\`\`\`

Generate a cryptographically random token with:

\`\`\`sh
python3 -c 'import secrets; print(secrets.token_urlsafe(36))'
\`\`\`

Generate a different token for each client. Store this allow-list in a protected
environment file or secret manager; do not commit real tokens or private keys.

Set the server environment and launch it:

\`\`\`sh
export HVC_TLS_CERT=/absolute/path/fullchain.pem
export HVC_TLS_KEY=/absolute/path/privkey.pem
export HVC_RELAY_CLIENTS='{"alice":{"token":"TOKEN_FOR_ALICE","rooms":["survival"]},"bob":{"token":"TOKEN_FOR_BOB","rooms":["survival"]}}'
export HVC_BIND=0.0.0.0
export HVC_PORT=8443
python3 tools/relay/server.py
\`\`\`

The service refuses to start without a TLS certificate/key and a valid client
allow-list. The TLS server enforces TLS 1.2 or newer. Put it behind a firewall
and only expose the configured TLS port. Use a real certificate or a certificate
trusted by every client. The Rust client verifies TLS certificates and does not
permit redirects or plaintext HTTP.

## Configure each game client

Set these variables **before launching the native Horizon executable**. Restart
the game after changing them.

\`\`\`sh
export HORIZON_VOICE_CHAT_RELAY_URL=https://voice.example.com:8443/v1/frames
export HORIZON_VOICE_CHAT_RELAY_TOKEN='THE_ENROLLED_CLIENT_TOKEN'
export HORIZON_VOICE_CHAT_CLIENT_ID=alice
export HORIZON_VOICE_CHAT_ROOM=survival
\`\`\`

Use the same client ID and exact token listed on the server. The room must be
allowed for that client. Each player must have a distinct ID/token. Keep the
token private: anyone who obtains it may send audio as that enrolled client
until the token is revoked on the server.

Start Horizon with the native mixin build that includes \`--audio\`, load the
separate \`horizon_voice_chat_ui.hmod\`, press V, enable the microphone and save.
When Push-to-talk is selected, hold C (the default, rebindable in Options >
Mods). The UI's backend text should change from “Relay not configured” to a
relay connection status when the client can connect.

When no relay variables are set, native capture stays local-only and no HTTP
relay worker is started. If only part of the relay configuration is set, the
GUI status reports the missing configuration or validation error. A speaker
output device is only opened when relay configuration is present.

## Wire and safety properties

- The client sends validated HVC1 PCM16 frames inside bounded HTTPS request
  bodies; this is not UDP and it is not compatible with the Java Simple Voice
  Chat protocol.
- The server verifies bearer tokens with constant-time comparison, checks
  allow-listed rooms, validates packet/header/frame sizes, rejects stale or
  duplicate sequence numbers, and applies per-client rate and session limits.
- Room history is memory-only, bounded by frame count and a short retention
  window; voice audio is not written to disk by the relay code.
- The network transport is encrypted in transit by TLS. The server operator
  necessarily handles decrypted audio while relaying it, so host the server
  only where you trust its operator.
- This is room-based broadcast, **not proximity chat**. The in-game distance
  preference is persisted but does not yet limit recipients. Do not use a room
  with people you do not intend to hear each other.
- PCM16 is uncompressed: at 48 kHz mono, raw samples are approximately 96 kB/s
  per active speaker, before HTTPS framing. Opus and bandwidth measurements are
  still future work.
- Audio output, microphone hardware and latency have not been verified on all
  supported operating systems. Voice activation is a simple RMS level threshold,
  not a robust speech detector.

The relay is an experimental building block, not an audited public service.
Do not expose it to an untrusted network without strong client enrollment,
TLS, firewalling, monitoring and explicit consent from everyone in the room.

## Tests

Run the server framing and authentication helper tests locally:

\`\`\`sh
python3 tools/relay/test_server.py
\`\`\`

The fixture and native-audio GitHub Actions jobs test the injected native build;
the UI job checks the companion's WASM guest. Passing CI does not substitute for
a live two-client test against a trusted TLS endpoint and actual audio devices.

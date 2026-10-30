# Voice chat port: implementation status and remaining work

## Implemented prototype

- Bounded HVC1 packet framing and PCM16 payload validation.
- Native GUI-to-audio bridge through Horizon's official per-mod storage files.
- Explicit microphone enable/mute, PTT (default C) and a basic RMS-gated voice
  activation option; microphone capture defaults to off.
- Optional cpal capture, bounded PCM queues, HTTPS relay worker and native
  cpal speaker output.
- Python TLS relay with unique per-client bearer credentials, room allow-lists,
  bounded packet validation, sequence replay checks, rate limits, and a short
  in-memory room history.
- Relay request/response wire envelopes, server time slots, bounded mixing and
  a short playout delay.
- CI covers the GUI WASM guest, native audio-enabled Horizon build, native mixin
  fixture, and relay protocol helpers.

This is an **experimental room voice prototype**, not a release-ready Simple
Voice Chat port. A live two-client session with actual microphones, speaker
devices and a trusted TLS host has not yet been demonstrated by CI.

## Important limitations

1. **No proximity relay yet.** The distance value from the GUI is persisted but
   not used for server recipient filtering. The server does not yet obtain
   authoritative positions or session membership from Horizon's multiplayer
   server, so every authorized client in one room can hear other active clients
   in that room.
2. **PCM only.** There is no Opus encoder/decoder. At 48 kHz mono, raw PCM16 is
   approximately 96 kB/s per active speaker before HTTPS framing.
3. **Prototype voice activation.** The optional mode is a simple RMS threshold,
   not a proper noise-resistant voice activity detector.
4. **Session and game integration.** Relay identities and rooms are configured
   by the relay operator. The Horizon game-server identity is not yet bound to
   these client credentials.
5. **Platform validation.** Local CI compiles the native audio-enabled source
   and the WASM companion, but it cannot validate actual audio devices, latency,
   disconnects, feedback/echo, or remote play on every supported operating system.
6. **Not wire compatible.** The HVC1 frame format is an internal prototype, not
   Simple Voice Chat's Java protocol.

## Architecture and current data path

1. The sandboxed UI companion saves mic/mute/PTT/distance settings in its official
   Horizon storage folder.
2. The injected native \`Game::tick\` hook reads those small preference files and
   explicitly starts/stops cpal capture.
3. Captured 10 ms PCM16 frames are queued with bounded backpressure. Only frames
   permitted by mute/PTT or the simple level gate are passed to the relay sender.
4. The native relay worker sends bounded HVC1 batches to the configured HTTPS
   endpoint using a unique bearer token and TLS certificate validation.
5. The Python relay verifies the client token, room membership, frame sizes and
   sequence progression; it stores frames in a bounded, short-lived in-memory
   room buffer.
6. Clients poll the relay, validate returned frames, mix frames assigned to the
   same server time slot, wait a short bounded delay, then enqueue output samples
   to cpal.

The relay worker performs network requests off the game tick. The GUI alone is
not sufficient: the native-mixin build with audio enabled is required for
microphone, network and speaker support.

See [NATIVE_BRIDGE.md](NATIVE_BRIDGE.md) and [RELAY.md](RELAY.md) for setup
and data flow. See [protocol-v1.md](protocol-v1.md) for the binary framing.

## Next milestones

- [x] Native mic/mute/PTT bridge with safe defaults.
- [x] TLS-authenticated HTTPS relay with per-client room authorization.
- [x] Initial remote PCM playback and bounded time-slot mixing.
- [ ] Live end-to-end test with two clients and speaker/mic hardware.
- [ ] Integrate the game server's authenticated player sessions and positions,
      then filter recipients by 3D proximity rather than room broadcast.
- [ ] Add Opus, bandwidth benchmarks, loss concealment and a more robust jitter
      buffer / playout scheduler.
- [ ] Add server-side disconnect cleanup, key rotation/revocation procedures,
      operational metrics and deployment tests.
- [ ] Test privacy/mute/hold-to-talk, feedback, disconnects, network loss and
      supported operating systems.

Do not deploy the relay as a public service before reviewing authentication,
room isolation, rate limits, TLS operations, monitoring and the intended privacy
model. Audio is decrypted at the relay server, so its operator is trusted.

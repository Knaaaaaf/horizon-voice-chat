# Voice packet framing v1

## Status and scope

This internal experimental format is used by the native Horizon voice-chat
prototype. It is not compatible with Simple Voice Chat's Java protocol.

The HVC1 packet itself has no authentication fields. It must only be sent inside
the configured authenticated HTTPS relay connection; do not send raw HVC1 packets
to an arbitrary UDP socket or an untrusted network. Client identity and access
control are supplied by the relay's TLS channel and unique bearer token.

## HVC1 audio packet layout

All integer fields are little-endian. The fixed header is 14 bytes.

| Offset | Size | Field | Rule |
| --- | ---: | --- | --- |
| 0 | 4 | Magic | ASCII HVC1 |
| 4 | 1 | Version | 1 |
| 5 | 1 | Kind | 1 Hello, 2 AudioFrame, 3 KeepAlive, 4 Disconnect |
| 6 | 2 | Flags | Reserved; must be zero |
| 8 | 4 | Sequence | Per-direction unsigned sequence number |
| 12 | 2 | Payload length | Payload byte count, excluding the header |
| 14 | variable | Payload | Opaque bytes |

Packets are capped at 1200 bytes, yielding a maximum payload of 1186 bytes.
The receiver rejects a short header, an oversized packet, wrong magic, unknown
version/kind, nonzero reserved flags, or a payload length that does not match
the remaining bytes.

## AudioFrame payload v1

AudioFrame payloads use a six-byte PCM header followed by mono signed PCM16
samples. All fields are little-endian.

| Offset | Size | Field | Rule |
| --- | ---: | --- | --- |
| 0 | 4 | Sample rate | 8000–48000 samples/second |
| 4 | 2 | Sample count | Must equal ceil(sample_rate / 100), approximately 10 ms |
| 6 | variable | PCM16 samples | One signed 16-bit little-endian sample per output frame |

At 48 kHz, a frame contains 480 samples (960 bytes); with the six-byte PCM
header and 14-byte HVC1 header, a packet is 980 bytes. This is uncompressed
PCM, not Opus, so bandwidth use is high.

## HTTPS relay envelopes

Client uploads use a separate body envelope so one HTTPS request can carry
several HVC1 frames:

- \`HVB1\` (4 bytes)
- frame count (u16 little-endian, 1–32)
- repeated: packet length (u16 little-endian), then the complete HVC1 packet

The server validates the whole upload before inserting any frames. It checks
packet kind, exact PCM length/sample rate, packet bounds and strictly forward
u32 sequence numbers per authenticated client session. Duplicate, stale or
ambiguous half-range jumps are rejected.

Server downloads use the response envelope:

- \`HVR1\` (4 bytes)
- next scan cursor (u64 little-endian)
- current server time slot in approximately 10 ms units (u64 little-endian)
- frame count (u16 little-endian, at most 256)
- repeated: frame time slot (u64), sender-ID byte length (u8), ASCII sender ID,
  packet length (u16), complete HVC1 packet

The client validates response bounds and frames, ignores its own frames, adds
remote samples sharing a time slot, applies a bounded playout delay, and queues
mono PCM to the default output device. The server queue is in-memory and
bounded by frame count/time. This is still a room-wide relay: it does not apply
Minecraft world position or distance filtering.

## Transport and security boundary

The native client only connects to a configured HTTPS endpoint ending in
\`/v1/frames\`. TLS certificate validation is enabled, redirects are disabled,
and each enrolled client must present a unique bearer token authorized for a
room. The Python relay requires a TLS certificate/key and an explicit allow-list,
and applies body/packet limits, sequence checks, per-client upload/download
rate limits, room caps and stale session cleanup. Voice is encrypted in transit
by TLS but decoded at the relay server, whose operator must be trusted.

This is a prototype, not a reviewed production protocol. It has no Opus codec,
voice privacy beyond TLS and room permissions, Minecraft server identity binding,
server-authoritative proximity, or end-to-end encryption. Do not expose a relay
to untrusted traffic without review and monitoring.

## Tests

The Rust unit tests cover framing/PCM boundaries, sequence wrap, jitter/reorder
behavior, proximity helpers, rate limiting and relay envelope validation. The
Python relay tests cover batched PCM frames, invalid payloads, sequence replay,
credential validation and response layout. Live two-client tests with actual
devices and a trusted TLS endpoint are still required.

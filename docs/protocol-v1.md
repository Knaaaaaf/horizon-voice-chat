# Voice packet framing v1

## Status and scope

Implemented in the voice-chat runtime as a standard-library-only codec, a
3D proximity selection helper, a bounded jitter/reorder buffer, and a token
bucket rate limiter, all with unit tests. This is an internal, experimental wire format for the Horizon port;
it is **not compatible with Simple Voice Chat's Java protocol**.

This document defines packet framing only. No UDP socket, microphone capture,
Opus encoder/decoder, playback, authentication, encryption, or in-game/server
integration is implemented yet. Do not expose this format to an untrusted
network as a secure protocol.

## Datagram layout

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

Datagrams are capped at 1200 bytes, yielding a maximum payload of 1186 bytes.
A receiver must reject a short header, oversized datagram, wrong magic, unknown
version/kind, nonzero reserved flags, oversized payload length, or a payload
length that does not exactly match the datagram.

## Sequence handling

Each sender/session needs its own SequenceGuard. It accepts the first packet
and then only forward sequence movement using wrapping u32 serial arithmetic.
Duplicates, older values, and ambiguous jumps of half the sequence space are
rejected. This first version drops late/reordered frames instead of buffering
them; a future jitter buffer may improve audio quality.

The guard is **not authentication** and does not stop a spoofing sender who can
forge a fresh sequence. The separate jitter buffer allows in-window reordering
and lets the caller explicitly skip a missing sequence after its playout
deadline; it intentionally has no clock or automatic timeout. A real transport
needs authenticated session identity, replay protection tied to that session,
and server-authoritative player membership and positions. The separate token
bucket helper can enforce a caller-defined token budget only after a trusted
session identity has been established; it is not a network limiter by itself.

## Payloads

Payloads are presently opaque. The AudioFrame kind does not imply that Opus is
already wired in, or that a payload can be decoded. A later protocol revision
must specify codec, sample rate, frame duration, negotiation, key rotation,
handshake challenges, and failure behavior before this codec is used for live
voice.

## Tests

The Rust tests cover round-trip serialization, exact maximum packet size,
oversized payload rejection, malformed header/length rejection, sequence
handling, proximity boundary/invalid-input handling, jitter-buffer ordering and
loss skips, and token-bucket capacity/refill behavior. GitHub Actions has passed
formatting, Clippy, Cargo tests, and the native-mixin fixture for the latest
token-bucket revision.

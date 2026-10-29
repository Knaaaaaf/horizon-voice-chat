// This source is both the standalone Rust library and the injected voice_chat module.
// The block-break hook is only a template smoke-test hook; it is not voice functionality.

pub mod voice_protocol {
    /// Protocol marker for Horizon Voice Chat's private wire format.
    pub const MAGIC: [u8; 4] = *b"HVC1";
    pub const VERSION: u8 = 1;
    pub const HEADER_LEN: usize = 14;
    /// Conservative datagram cap. The current protocol uses UDP-style framing,
    /// but no socket or network transport is wired up yet.
    pub const MAX_DATAGRAM_BYTES: usize = 1200;
    pub const MAX_PAYLOAD_BYTES: usize = MAX_DATAGRAM_BYTES - HEADER_LEN;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    #[repr(u8)]
    pub enum PacketKind {
        Hello = 1,
        AudioFrame = 2,
        KeepAlive = 3,
        Disconnect = 4,
    }

    impl PacketKind {
        fn from_wire(value: u8) -> Result<Self, CodecError> {
            match value {
                1 => Ok(Self::Hello),
                2 => Ok(Self::AudioFrame),
                3 => Ok(Self::KeepAlive),
                4 => Ok(Self::Disconnect),
                other => Err(CodecError::UnknownKind(other)),
            }
        }
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum CodecError {
        TooShort,
        TooLarge,
        BadMagic,
        UnsupportedVersion(u8),
        UnknownKind(u8),
        ReservedFlags,
        LengthMismatch,
        PayloadTooLarge,
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct Packet {
        pub kind: PacketKind,
        pub sequence: u32,
        /// Opaque bytes. Audio encoding, authentication and transport are not
        /// implemented by this framing layer.
        pub payload: Vec<u8>,
    }

    impl Packet {
        /// Encode a packet using the v1 fixed header and a bounded payload.
        pub fn encode(&self) -> Result<Vec<u8>, CodecError> {
            if self.payload.len() > MAX_PAYLOAD_BYTES {
                return Err(CodecError::PayloadTooLarge);
            }

            let mut bytes = Vec::with_capacity(HEADER_LEN + self.payload.len());
            bytes.extend_from_slice(&MAGIC);
            bytes.push(VERSION);
            bytes.push(self.kind as u8);
            bytes.extend_from_slice(&0u16.to_le_bytes()); // reserved flags
            bytes.extend_from_slice(&self.sequence.to_le_bytes());
            bytes.extend_from_slice(&(self.payload.len() as u16).to_le_bytes());
            bytes.extend_from_slice(&self.payload);
            Ok(bytes)
        }

        /// Decode a complete datagram. Reject malformed or oversized input
        /// before allocating the owned payload.
        pub fn decode(bytes: &[u8]) -> Result<Self, CodecError> {
            if bytes.len() < HEADER_LEN {
                return Err(CodecError::TooShort);
            }
            if bytes.len() > MAX_DATAGRAM_BYTES {
                return Err(CodecError::TooLarge);
            }
            if &bytes[..4] != MAGIC.as_slice() {
                return Err(CodecError::BadMagic);
            }
            if bytes[4] != VERSION {
                return Err(CodecError::UnsupportedVersion(bytes[4]));
            }

            let kind = PacketKind::from_wire(bytes[5])?;
            let flags = u16::from_le_bytes([bytes[6], bytes[7]]);
            if flags != 0 {
                return Err(CodecError::ReservedFlags);
            }

            let sequence = u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]);
            let payload_len = u16::from_le_bytes([bytes[12], bytes[13]]) as usize;
            if payload_len > MAX_PAYLOAD_BYTES {
                return Err(CodecError::PayloadTooLarge);
            }
            if bytes.len() != HEADER_LEN + payload_len {
                return Err(CodecError::LengthMismatch);
            }

            Ok(Self {
                kind,
                sequence,
                payload: bytes[HEADER_LEN..].to_vec(),
            })
        }
    }

    /// Monotonic per-direction sequence guard. It rejects duplicates and older
    /// packets; late/reordered packets are intentionally dropped by this simple
    /// first version rather than buffered. Create one guard per sender/session.
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct SequenceGuard {
        latest: Option<u32>,
    }

    impl SequenceGuard {
        pub fn accept(&mut self, sequence: u32) -> bool {
            let Some(latest) = self.latest else {
                self.latest = Some(sequence);
                return true;
            };

            // Serial-number arithmetic: accepts forward movement across u32 wrap,
            // but rejects duplicates, stale values and ambiguous half-range jumps.
            let delta = sequence.wrapping_sub(latest);
            if delta == 0 || delta >= 0x8000_0000 {
                return false;
            }
            self.latest = Some(sequence);
            true
        }

        pub fn reset(&mut self) {
            self.latest = None;
        }
    }
}

pub mod rate_limiter {
    /// A small token bucket for per-session relay rate limiting.
    ///
    /// Time is supplied by the caller so tests are deterministic. This is not
    /// authentication; the caller must key buckets by verified session IDs.
    pub struct TokenBucket {
        capacity_milli: u64,
        tokens_milli: u64,
        refill_tokens_per_second: u32,
        last_ms: u64,
    }

    impl TokenBucket {
        /// Returns None for a zero capacity or a zero refill rate.
        pub fn new(
            capacity_tokens: u32,
            refill_tokens_per_second: u32,
            now_ms: u64,
        ) -> Option<Self> {
            if capacity_tokens == 0 || refill_tokens_per_second == 0 {
                return None;
            }
            let capacity_milli = u64::from(capacity_tokens) * 1000;
            Some(Self {
                capacity_milli,
                tokens_milli: capacity_milli,
                refill_tokens_per_second,
                last_ms: now_ms,
            })
        }

        /// Consume whole tokens, refilling from elapsed milliseconds first.
        /// Invalid zero-cost or over-capacity requests are rejected.
        pub fn allow(&mut self, now_ms: u64, cost_tokens: u32) -> bool {
            if cost_tokens == 0 {
                return false;
            }

            if now_ms >= self.last_ms {
                let elapsed_ms = now_ms - self.last_ms;
                let refill = elapsed_ms.saturating_mul(u64::from(self.refill_tokens_per_second));
                self.tokens_milli = self
                    .tokens_milli
                    .saturating_add(refill)
                    .min(self.capacity_milli);
                self.last_ms = now_ms;
            }

            let cost_milli = u64::from(cost_tokens) * 1000;
            if cost_milli > self.capacity_milli || self.tokens_milli < cost_milli {
                return false;
            }
            self.tokens_milli -= cost_milli;
            true
        }

        pub fn available_milli_tokens(&self) -> u64 {
            self.tokens_milli
        }
    }
}

pub mod jitter_buffer {
    use std::collections::BTreeMap;

    use super::voice_protocol::Packet;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum PushError {
        ZeroCapacity,
        TooFarAhead,
        Stale,
        Duplicate,
        Full,
    }

    /// A bounded, sequence-ordered packet buffer for a future audio playout loop.
    ///
    /// The caller chooses the first expected sequence and decides when a missing
    /// packet has waited long enough; this type deliberately has no hidden clock.
    pub struct JitterBuffer {
        next_sequence: u32,
        capacity: usize,
        pending: BTreeMap<u32, Packet>,
    }

    impl JitterBuffer {
        pub fn new(first_sequence: u32, capacity: usize) -> Result<Self, PushError> {
            if capacity == 0 {
                return Err(PushError::ZeroCapacity);
            }
            Ok(Self {
                next_sequence: first_sequence,
                capacity,
                pending: BTreeMap::new(),
            })
        }

        pub fn push(&mut self, packet: Packet) -> Result<(), PushError> {
            let distance = packet.sequence.wrapping_sub(self.next_sequence);
            if distance >= 0x8000_0000 {
                return Err(PushError::Stale);
            }
            if distance >= self.capacity as u32 {
                return Err(PushError::TooFarAhead);
            }
            if self.pending.contains_key(&packet.sequence) {
                return Err(PushError::Duplicate);
            }
            if self.pending.len() >= self.capacity {
                return Err(PushError::Full);
            }
            self.pending.insert(packet.sequence, packet);
            Ok(())
        }

        /// Returns only the next expected packet. Missing frames do not advance
        /// the cursor automatically; the playout scheduler must choose when to
        /// call skip_missing after its deadline.
        pub fn pop_next(&mut self) -> Option<Packet> {
            let packet = self.pending.remove(&self.next_sequence)?;
            self.next_sequence = self.next_sequence.wrapping_add(1);
            Some(packet)
        }

        /// Explicitly abandon missing sequence numbers until target_sequence.
        /// This should only be called by a playout scheduler after its deadline.
        pub fn skip_missing(&mut self, target_sequence: u32) -> bool {
            let distance = target_sequence.wrapping_sub(self.next_sequence);
            if distance == 0 || distance >= 0x8000_0000 {
                return false;
            }
            self.next_sequence = target_sequence;
            true
        }

        pub fn len(&self) -> usize {
            self.pending.len()
        }

        pub fn is_empty(&self) -> bool {
            self.pending.is_empty()
        }

        pub fn next_sequence(&self) -> u32 {
            self.next_sequence
        }
    }
}

pub mod proximity {
    /// True when a listener is within the speaker's 3D voice radius.
    ///
    /// Invalid coordinates, non-finite ranges, negative ranges and arithmetic
    /// overflow are rejected rather than accidentally broadening the audience.
    pub fn within_range(speaker: [f64; 3], listener: [f64; 3], range: f64) -> bool {
        if !range.is_finite()
            || range < 0.0
            || speaker.iter().any(|value| !value.is_finite())
            || listener.iter().any(|value| !value.is_finite())
        {
            return false;
        }

        let dx = speaker[0] - listener[0];
        let dy = speaker[1] - listener[1];
        let dz = speaker[2] - listener[2];
        let distance_squared = dx * dx + dy * dy + dz * dz;
        let range_squared = range * range;

        distance_squared.is_finite()
            && range_squared.is_finite()
            && distance_squared <= range_squared
    }

    /// Select listeners in range, preserving candidate order and excluding the
    /// speaker. Callers must provide authenticated identities and authoritative
    /// positions; this pure helper does not authenticate or relay any packets.
    pub fn eligible_recipients<'a>(
        speaker_id: &str,
        speaker_position: [f64; 3],
        candidates: &'a [(&'a str, [f64; 3])],
        range: f64,
    ) -> Vec<&'a str> {
        candidates
            .iter()
            .filter_map(|(id, position)| {
                (*id != speaker_id && within_range(speaker_position, *position, range))
                    .then_some(*id)
            })
            .collect()
    }
}

// Temporary native hook used only to prove that the template still stages.
// It does not capture or transmit voice audio.
pub fn block_break_message(pos: [i32; 3]) -> String {
    format!(
        "[horizon-voice-chat/dev-hook] Block break at {}, {}, {}",
        pos[0], pos[1], pos[2]
    )
}

pub fn before_break(pos: [i32; 3]) {
    eprintln!("{}", block_break_message(pos));
}

#[cfg(test)]
mod tests {
    use super::jitter_buffer::{JitterBuffer, PushError};
    use super::proximity::{eligible_recipients, within_range};
    use super::rate_limiter::TokenBucket;
    use super::voice_protocol::{
        CodecError, HEADER_LEN, MAX_DATAGRAM_BYTES, MAX_PAYLOAD_BYTES, Packet, PacketKind,
        SequenceGuard,
    };
    use super::*;

    #[test]
    fn message_preserves_signed_block_coordinates() {
        assert_eq!(
            block_break_message([-3, 64, 12]),
            "[horizon-voice-chat/dev-hook] Block break at -3, 64, 12"
        );
    }

    #[test]
    fn packet_round_trip_preserves_fields() {
        let packet = Packet {
            kind: PacketKind::AudioFrame,
            sequence: 42,
            payload: vec![0, 1, 2, 253, 254, 255],
        };
        let encoded = packet.encode().unwrap();
        assert_eq!(Packet::decode(&encoded).unwrap(), packet);
    }

    #[test]
    fn encoder_accepts_maximum_datagram_and_rejects_one_byte_more() {
        let packet = Packet {
            kind: PacketKind::AudioFrame,
            sequence: 7,
            payload: vec![0xA5; MAX_PAYLOAD_BYTES],
        };
        let encoded = packet.encode().unwrap();
        assert_eq!(encoded.len(), MAX_DATAGRAM_BYTES);
        assert_eq!(Packet::decode(&encoded).unwrap(), packet);

        let oversized = Packet {
            payload: vec![0xA5; MAX_PAYLOAD_BYTES + 1],
            ..packet
        };
        assert_eq!(oversized.encode(), Err(CodecError::PayloadTooLarge));
    }

    #[test]
    fn decoder_rejects_invalid_headers_and_lengths() {
        let packet = Packet {
            kind: PacketKind::Hello,
            sequence: 1,
            payload: Vec::new(),
        };
        let valid = packet.encode().unwrap();

        assert_eq!(
            Packet::decode(&valid[..HEADER_LEN - 1]),
            Err(CodecError::TooShort)
        );

        let mut bad_magic = valid.clone();
        bad_magic[0] ^= 1;
        assert_eq!(Packet::decode(&bad_magic), Err(CodecError::BadMagic));

        let mut bad_version = valid.clone();
        bad_version[4] = 99;
        assert_eq!(
            Packet::decode(&bad_version),
            Err(CodecError::UnsupportedVersion(99))
        );

        let mut bad_kind = valid.clone();
        bad_kind[5] = 99;
        assert_eq!(Packet::decode(&bad_kind), Err(CodecError::UnknownKind(99)));

        let mut bad_flags = valid.clone();
        bad_flags[6] = 1;
        assert_eq!(Packet::decode(&bad_flags), Err(CodecError::ReservedFlags));

        let mut bad_length = valid.clone();
        bad_length[12] = 1;
        assert_eq!(Packet::decode(&bad_length), Err(CodecError::LengthMismatch));

        let oversized = vec![0; MAX_DATAGRAM_BYTES + 1];
        assert_eq!(Packet::decode(&oversized), Err(CodecError::TooLarge));
    }

    #[test]
    fn sequence_guard_rejects_replays_and_supports_wraparound() {
        let mut guard = SequenceGuard::default();
        assert!(guard.accept(u32::MAX));
        assert!(guard.accept(0));
        assert!(!guard.accept(0));
        assert!(!guard.accept(u32::MAX));
        assert!(guard.accept(2));
        assert!(!guard.accept(1));
        guard.reset();
        assert!(guard.accept(1));
    }

    #[test]
    fn proximity_uses_three_dimensional_distance_and_rejects_invalid_values() {
        assert!(within_range([0.0, 0.0, 0.0], [3.0, 4.0, 0.0], 5.0));
        assert!(!within_range([0.0, 0.0, 0.0], [5.1, 0.0, 0.0], 5.0));
        assert!(!within_range([f64::NAN, 0.0, 0.0], [0.0, 0.0, 0.0], 5.0));
        assert!(!within_range([0.0, 0.0, 0.0], [0.0, 0.0, 0.0], f64::NAN));
        assert!(!within_range([0.0, 0.0, 0.0], [0.0, 0.0, 0.0], -1.0));
    }

    #[test]
    fn recipient_filter_excludes_speaker_and_out_of_range_players() {
        let candidates = [
            ("speaker", [0.0, 0.0, 0.0]),
            ("near", [2.0, 0.0, 0.0]),
            ("edge", [0.0, 3.0, 4.0]),
            ("far", [6.0, 0.0, 0.0]),
            ("invalid", [f64::NAN, 0.0, 0.0]),
        ];
        assert_eq!(
            eligible_recipients("speaker", [0.0, 0.0, 0.0], &candidates, 5.0),
            vec!["near", "edge"]
        );
    }

    fn audio_packet(sequence: u32) -> Packet {
        Packet {
            kind: PacketKind::AudioFrame,
            sequence,
            payload: vec![sequence as u8],
        }
    }

    #[test]
    fn jitter_buffer_reorders_frames_and_handles_sequence_wraparound() {
        let mut buffer = JitterBuffer::new(u32::MAX, 4).unwrap();
        buffer.push(audio_packet(0)).unwrap();
        buffer.push(audio_packet(u32::MAX)).unwrap();
        assert_eq!(buffer.pop_next().unwrap().sequence, u32::MAX);
        assert_eq!(buffer.pop_next().unwrap().sequence, 0);
        assert!(buffer.is_empty());
    }

    #[test]
    fn jitter_buffer_is_bounded_and_rejects_stale_duplicate_and_distant_frames() {
        let mut buffer = JitterBuffer::new(10, 2).unwrap();
        assert_eq!(buffer.push(audio_packet(12)), Err(PushError::TooFarAhead));
        buffer.push(audio_packet(11)).unwrap();
        assert_eq!(buffer.push(audio_packet(11)), Err(PushError::Duplicate));
        assert_eq!(buffer.push(audio_packet(9)), Err(PushError::Stale));
        buffer.push(audio_packet(10)).unwrap();
        assert_eq!(buffer.push(audio_packet(10)), Err(PushError::Duplicate));
        assert_eq!(buffer.len(), 2);
        assert_eq!(buffer.pop_next().unwrap().sequence, 10);
        assert_eq!(buffer.pop_next().unwrap().sequence, 11);
        assert_eq!(buffer.pop_next(), None);
    }

    #[test]
    fn jitter_buffer_requires_nonzero_capacity_and_can_skip_lost_frames() {
        assert!(matches!(
            JitterBuffer::new(1, 0),
            Err(PushError::ZeroCapacity)
        ));
        let mut buffer = JitterBuffer::new(5, 4).unwrap();
        buffer.push(audio_packet(7)).unwrap();
        assert!(buffer.skip_missing(7));
        assert_eq!(buffer.pop_next().unwrap().sequence, 7);
        assert!(!buffer.skip_missing(7));
        assert!(!buffer.skip_missing(6));
    }

    #[test]
    fn token_bucket_enforces_capacity_and_refills_over_time() {
        let mut bucket = TokenBucket::new(3, 2, 1_000).unwrap();
        assert!(bucket.allow(1_000, 1));
        assert!(bucket.allow(1_000, 1));
        assert!(bucket.allow(1_000, 1));
        assert!(!bucket.allow(1_000, 1));
        assert!(bucket.allow(1_500, 1));
        assert!(!bucket.allow(1_500, 1));
        assert!(bucket.allow(2_000, 1));
    }

    #[test]
    fn token_bucket_rejects_invalid_costs_and_does_not_refill_on_clock_rewind() {
        assert!(TokenBucket::new(0, 1, 0).is_none());
        assert!(TokenBucket::new(1, 0, 0).is_none());
        let mut bucket = TokenBucket::new(2, 1, 100).unwrap();
        assert!(!bucket.allow(100, 0));
        assert!(!bucket.allow(100, 3));
        assert!(bucket.allow(100, 1));
        assert!(bucket.allow(100, 1));
        assert!(!bucket.allow(50, 1));
        assert_eq!(bucket.available_milli_tokens(), 0);
        assert!(bucket.allow(1_100, 1));
    }
}

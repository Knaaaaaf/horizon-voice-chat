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
}

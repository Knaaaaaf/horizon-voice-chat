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

pub mod pcm_capture {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::mpsc::{SyncSender, TrySendError};

    /// Converts interleaved device samples into bounded mono PCM frames.
    /// Frame duration is approximately 10 ms at the device's sample rate.
    pub struct PcmFrameAssembler {
        channels: usize,
        frame_samples: usize,
        pending: Vec<i16>,
        dropped_frames: Arc<AtomicU64>,
    }

    impl PcmFrameAssembler {
        pub fn new(
            channels: usize,
            sample_rate: u32,
            dropped_frames: Arc<AtomicU64>,
        ) -> Option<Self> {
            if channels == 0 || !(8_000..=48_000).contains(&sample_rate) {
                return None;
            }
            let frame_samples = (sample_rate as usize).div_ceil(100);
            Some(Self {
                channels,
                frame_samples,
                pending: Vec::with_capacity(frame_samples),
                dropped_frames,
            })
        }

        /// Downmixes each interleaved channel frame to mono and emits complete
        /// PCM16 frames without ever blocking the device callback.
        pub fn push<T: Copy>(
            &mut self,
            interleaved: &[T],
            convert: impl Fn(T) -> i16,
            sender: &SyncSender<Vec<i16>>,
        ) {
            for channels in interleaved.chunks_exact(self.channels) {
                let sum: i32 = channels
                    .iter()
                    .map(|sample| i32::from(convert(*sample)))
                    .sum();
                let mono = (sum / self.channels as i32)
                    .clamp(i32::from(i16::MIN), i32::from(i16::MAX))
                    as i16;
                self.pending.push(mono);

                if self.pending.len() == self.frame_samples {
                    let frame = std::mem::replace(
                        &mut self.pending,
                        Vec::with_capacity(self.frame_samples),
                    );
                    if let Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) =
                        sender.try_send(frame)
                    {
                        self.dropped_frames.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::sync::mpsc::sync_channel;

        #[test]
        fn assembles_ten_millisecond_mono_frames_and_downmixes_stereo() {
            let (sender, receiver) = sync_channel(4);
            let dropped = Arc::new(AtomicU64::new(0));
            let mut assembler = PcmFrameAssembler::new(2, 8_000, dropped.clone()).unwrap();
            let interleaved: Vec<i16> = (0..160).flat_map(|_| [1_000_i16, 3_000_i16]).collect();
            assembler.push(&interleaved, |sample| sample, &sender);
            let frame = receiver.try_recv().unwrap();
            assert_eq!(frame.len(), 80);
            assert!(frame.iter().all(|sample| *sample == 2_000));
            assert_eq!(dropped.load(Ordering::Relaxed), 0);
        }

        #[test]
        fn bounded_channel_drops_frames_instead_of_blocking_capture() {
            let (sender, _receiver) = sync_channel(0);
            let dropped = Arc::new(AtomicU64::new(0));
            let mut assembler = PcmFrameAssembler::new(1, 8_000, dropped.clone()).unwrap();
            assembler.push(&[42_i16; 80], |sample| sample, &sender);
            assert_eq!(dropped.load(Ordering::Relaxed), 1);
        }

        #[test]
        fn rejects_unsupported_channels_and_sample_rates() {
            let dropped = Arc::new(AtomicU64::new(0));
            assert!(PcmFrameAssembler::new(0, 48_000, dropped.clone()).is_none());
            assert!(PcmFrameAssembler::new(1, 96_000, dropped.clone()).is_none());
            assert!(PcmFrameAssembler::new(1, 4_000, dropped).is_none());
        }
    }
}

#[cfg(feature = "audio")]
pub mod microphone {
    use super::pcm_capture::PcmFrameAssembler;
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use cpal::{Device, SampleFormat, SizedSample, Stream, StreamConfig};
    use std::sync::Arc;
    use std::sync::atomic::AtomicU64;
    use std::sync::mpsc::SyncSender;
    use std::time::Duration;

    #[derive(Debug)]
    pub enum CaptureError {
        NoInputDevice,
        DefaultConfig(String),
        UnsupportedSampleRate(u32),
        UnsupportedSampleFormat,
        BuildStream(String),
        StartStream(String),
    }

    /// Live microphone capture. Construct this only after explicit user opt-in;
    /// dropping it stops the OS input stream.
    pub struct MicrophoneCapture {
        _stream: Stream,
        sample_rate: u32,
        channels: u16,
        dropped_frames: Arc<AtomicU64>,
    }

    impl MicrophoneCapture {
        pub fn start(sender: SyncSender<Vec<i16>>) -> Result<Self, CaptureError> {
            let host = cpal::default_host();
            let device = host
                .default_input_device()
                .ok_or(CaptureError::NoInputDevice)?;
            let supported = device
                .default_input_config()
                .map_err(|error| CaptureError::DefaultConfig(error.to_string()))?;
            let sample_rate = supported.sample_rate().0;
            let channels = supported.channels();
            if !(8_000..=48_000).contains(&sample_rate) {
                return Err(CaptureError::UnsupportedSampleRate(sample_rate));
            }
            if channels == 0 {
                return Err(CaptureError::DefaultConfig(
                    "input has zero channels".into(),
                ));
            }

            let config: StreamConfig = supported.clone().into();
            let dropped_frames = Arc::new(AtomicU64::new(0));
            let stream = match supported.sample_format() {
                SampleFormat::F32 => build_stream::<f32, _>(
                    &device,
                    &config,
                    sender,
                    Arc::clone(&dropped_frames),
                    |sample| (sample.clamp(-1.0, 1.0) * f32::from(i16::MAX)) as i16,
                )?,
                SampleFormat::I16 => build_stream::<i16, _>(
                    &device,
                    &config,
                    sender,
                    Arc::clone(&dropped_frames),
                    |sample| sample,
                )?,
                SampleFormat::U16 => build_stream::<u16, _>(
                    &device,
                    &config,
                    sender,
                    Arc::clone(&dropped_frames),
                    |sample| (i32::from(sample) - 32_768) as i16,
                )?,
                _ => return Err(CaptureError::UnsupportedSampleFormat),
            };
            stream
                .play()
                .map_err(|error| CaptureError::StartStream(error.to_string()))?;

            Ok(Self {
                _stream: stream,
                sample_rate,
                channels,
                dropped_frames,
            })
        }

        pub fn sample_rate(&self) -> u32 {
            self.sample_rate
        }

        pub fn channels(&self) -> u16 {
            self.channels
        }

        pub fn dropped_frames(&self) -> u64 {
            self.dropped_frames
                .load(std::sync::atomic::Ordering::Relaxed)
        }
    }

    fn build_stream<T, F>(
        device: &Device,
        config: &StreamConfig,
        sender: SyncSender<Vec<i16>>,
        dropped: Arc<AtomicU64>,
        convert: F,
    ) -> Result<Stream, CaptureError>
    where
        T: SizedSample + Copy,
        F: Fn(T) -> i16 + Send + 'static,
    {
        let channels = usize::from(config.channels);
        let sample_rate = config.sample_rate.0;
        let mut assembler = PcmFrameAssembler::new(channels, sample_rate, dropped)
            .ok_or(CaptureError::UnsupportedSampleRate(sample_rate))?;
        device
            .build_input_stream(
                config,
                move |data: &[T], _| assembler.push(data, &convert, &sender),
                |error| eprintln!("[horizon-voice-chat] microphone stream: {error}"),
                Some(Duration::from_millis(100)),
            )
            .map_err(|error| CaptureError::BuildStream(error.to_string()))
    }
}

pub mod pcm_codec {
    use super::voice_protocol::{MAX_PAYLOAD_BYTES, Packet, PacketKind};

    pub const PCM_HEADER_LEN: usize = 6;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum PcmFrameError {
        PayloadTooShort,
        PayloadTooLarge,
        InvalidSampleRate,
        InvalidFrameLength,
        LengthMismatch,
        WrongPacketKind,
    }

    /// Uncompressed mono PCM16 audio frame. Frames represent approximately 10 ms.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct PcmFrame {
        pub sample_rate: u32,
        pub samples: Vec<i16>,
    }

    impl PcmFrame {
        pub fn encode_payload(&self) -> Result<Vec<u8>, PcmFrameError> {
            validate_sample_rate(self.sample_rate)?;
            let expected = (self.sample_rate as usize).div_ceil(100);
            if self.samples.len() != expected {
                return Err(PcmFrameError::InvalidFrameLength);
            }
            let payload_len = PCM_HEADER_LEN + self.samples.len() * 2;
            if payload_len > MAX_PAYLOAD_BYTES {
                return Err(PcmFrameError::PayloadTooLarge);
            }

            let mut payload = Vec::with_capacity(payload_len);
            payload.extend_from_slice(&self.sample_rate.to_le_bytes());
            payload.extend_from_slice(&(self.samples.len() as u16).to_le_bytes());
            for sample in &self.samples {
                payload.extend_from_slice(&sample.to_le_bytes());
            }
            Ok(payload)
        }

        pub fn decode_payload(payload: &[u8]) -> Result<Self, PcmFrameError> {
            if payload.len() < PCM_HEADER_LEN {
                return Err(PcmFrameError::PayloadTooShort);
            }
            if payload.len() > MAX_PAYLOAD_BYTES {
                return Err(PcmFrameError::PayloadTooLarge);
            }

            let sample_rate = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
            validate_sample_rate(sample_rate)?;
            let sample_count = u16::from_le_bytes([payload[4], payload[5]]) as usize;
            let expected = (sample_rate as usize).div_ceil(100);
            if sample_count != expected {
                return Err(PcmFrameError::InvalidFrameLength);
            }
            if payload.len() != PCM_HEADER_LEN + sample_count * 2 {
                return Err(PcmFrameError::LengthMismatch);
            }

            let samples = payload[PCM_HEADER_LEN..]
                .chunks_exact(2)
                .map(|bytes| i16::from_le_bytes([bytes[0], bytes[1]]))
                .collect();
            Ok(Self {
                sample_rate,
                samples,
            })
        }

        pub fn to_packet(&self, sequence: u32) -> Result<Packet, PcmFrameError> {
            Ok(Packet {
                kind: PacketKind::AudioFrame,
                sequence,
                payload: self.encode_payload()?,
            })
        }

        pub fn from_packet(packet: &Packet) -> Result<Self, PcmFrameError> {
            if packet.kind != PacketKind::AudioFrame {
                return Err(PcmFrameError::WrongPacketKind);
            }
            Self::decode_payload(&packet.payload)
        }
    }

    fn validate_sample_rate(sample_rate: u32) -> Result<(), PcmFrameError> {
        if (8_000..=48_000).contains(&sample_rate) {
            Ok(())
        } else {
            Err(PcmFrameError::InvalidSampleRate)
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


/// Polls the GUI companion's persisted settings and controls the native microphone.
///
/// Horizon's current WASM mod API intentionally has no direct microphone or
/// socket capability. The native mixin therefore bridges only the companion's
/// own storage folder. The folder is discovered beside the installed
/// `horizon-mods` folder, or can be set explicitly with
/// `HORIZON_VOICE_CHAT_SETTINGS_DIR` / `HORIZON_VOICE_CHAT_MODS_DIR`.
pub mod native_bridge {
    use std::fs;
    use std::path::{Path, PathBuf};

    const UI_MOD_ID: &str = "horizon_voice_chat_ui";
    const STORAGE_DIR: &str = ".horizon-mod-data";

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct Settings {
        pub microphone_enabled: bool,
        pub muted: bool,
        pub push_to_talk: bool,
        pub ptt_down: bool,
        /// Actual proximity distance in blocks, not the companion's list index.
        pub proximity_blocks: u32,
    }

    impl Default for Settings {
        fn default() -> Self {
            Self {
                microphone_enabled: false,
                muted: false,
                push_to_talk: true,
                ptt_down: false,
                proximity_blocks: 16,
            }
        }
    }

    fn read_text(dir: &Path, key: &str) -> Option<String> {
        fs::read_to_string(dir.join(key)).ok()
    }

    fn read_bool(dir: &Path, key: &str, default: bool) -> bool {
        read_text(dir, key)
            .map(|value| value.trim().eq_ignore_ascii_case("true"))
            .unwrap_or(default)
    }

    pub fn read_settings_at(dir: &Path) -> Settings {
        let distance_index = read_text(dir, "distance")
            .and_then(|value| value.trim().parse::<usize>().ok())
            .filter(|index| *index < 4)
            .unwrap_or(1);

        Settings {
            microphone_enabled: read_bool(dir, "mic_enabled", false),
            muted: read_bool(dir, "muted", false),
            push_to_talk: read_bool(dir, "ptt", true),
            // A held key must never survive a game restart or a missed key-up.
            ptt_down: read_bool(dir, "ptt_down", false),
            proximity_blocks: [8, 16, 32, 48][distance_index],
        }
    }

    /// Microphone frames may leave the client only when this gate is open.
    /// The network sender is intentionally not connected yet.
    pub fn transmit_allowed(settings: &Settings) -> bool {
        settings.microphone_enabled
            && !settings.muted
            && (!settings.push_to_talk || settings.ptt_down)
    }

    fn mods_dir_from_env() -> Option<PathBuf> {
        std::env::var_os("HORIZON_VOICE_CHAT_MODS_DIR").map(PathBuf::from)
    }

    fn settings_dir_from_env() -> Option<PathBuf> {
        std::env::var_os("HORIZON_VOICE_CHAT_SETTINGS_DIR").map(PathBuf::from)
    }

    fn candidate_roots() -> Vec<PathBuf> {
        let mut roots = Vec::new();
        if let Ok(cwd) = std::env::current_dir() {
            roots.extend(cwd.ancestors().take(5).map(Path::to_path_buf));
        }
        if let Ok(exe) = std::env::current_exe() {
            if let Some(parent) = exe.parent() {
                roots.extend(parent.ancestors().take(5).map(Path::to_path_buf));
            }
        }
        roots.sort();
        roots.dedup();
        roots
    }

    /// Finds Horizon's documented `horizon-mods/.horizon-mod-data/<mod-id>`
    /// folder. The exact settings path environment variable takes precedence.
    pub fn settings_directory() -> Option<PathBuf> {
        if let Some(path) = settings_dir_from_env() {
            return Some(path);
        }
        if let Some(path) = mods_dir_from_env() {
            return Some(path.join(STORAGE_DIR).join(UI_MOD_ID));
        }
        for root in candidate_roots() {
            if root.file_name().is_some_and(|name| name == "horizon-mods") && root.is_dir() {
                return Some(root.join(STORAGE_DIR).join(UI_MOD_ID));
            }
            let mods = root.join("horizon-mods");
            if mods.is_dir() {
                return Some(mods.join(STORAGE_DIR).join(UI_MOD_ID));
            }
        }
        None
    }

    fn publish_status(dir: &Path, status: &str) {
        let path = dir.join("backend_status");
        if fs::read_to_string(&path).ok().as_deref() == Some(status) {
            return;
        }
        if fs::create_dir_all(dir).is_ok() {
            let _ = fs::write(path, status.as_bytes());
        }
    }

    #[cfg(feature = "audio")]
    struct AudioRuntime {
        capture: Option<super::microphone::MicrophoneCapture>,
        receiver: Option<std::sync::mpsc::Receiver<Vec<i16>>>,
        attempted: bool,
        last_error: Option<String>,
    }

    #[cfg(feature = "audio")]
    impl Default for AudioRuntime {
        fn default() -> Self {
            Self {
                capture: None,
                receiver: None,
                attempted: false,
                last_error: None,
            }
        }
    }

    #[cfg(feature = "audio")]
    impl AudioRuntime {
        fn update(&mut self, settings: &Settings) -> String {
            let requested = settings.microphone_enabled && !settings.muted;
            if !requested {
                self.capture = None;
                self.receiver = None;
                self.attempted = false;
                self.last_error = None;
                return if settings.microphone_enabled && settings.muted {
                    "Muted — native microphone stopped".into()
                } else {
                    "Microphone off".into()
                };
            }

            // Only attempt to open the OS device once per enable cycle. A
            // failure remains visible until the user disables/enables the mic.
            if self.capture.is_none() && !self.attempted {
                self.attempted = true;
                let (sender, receiver) = std::sync::mpsc::sync_channel(16);
                match super::microphone::MicrophoneCapture::start(sender) {
                    Ok(capture) => {
                        self.capture = Some(capture);
                        self.receiver = Some(receiver);
                        self.last_error = None;
                    }
                    Err(error) => self.last_error = Some(format!("{error:?}")),
                }
            }

            if let Some(receiver) = &self.receiver {
                // The local capture is real, but no network sender is wired up.
                // Drain bounded PCM frames so the device callback never backs up.
                while receiver.try_recv().is_ok() {}
            }

            if self.capture.is_none() {
                return format!(
                    "Microphone error — {}",
                    self.last_error.as_deref().unwrap_or("capture did not start")
                );
            }
            if settings.push_to_talk {
                if transmit_allowed(settings) {
                    "Capturing locally — PTT held; server relay offline".into()
                } else {
                    "Capturing locally — PTT released; server relay offline".into()
                }
            } else {
                "Capturing locally — server relay offline".into()
            }
        }
    }

    #[derive(Default)]
    struct Runtime {
        ticks: u64,
        settings_dir: Option<PathBuf>,
        settings: Settings,
        #[cfg(feature = "audio")]
        audio: AudioRuntime,
    }

    impl Runtime {
        fn tick(&mut self) {
            self.ticks = self.ticks.wrapping_add(1);

            // 20 game ticks/s: poll the files twice per second, not on every
            // frame, while still discovering the profile directory on startup.
            if self.settings_dir.is_none() || self.ticks % 10 == 1 {
                if let Some(dir) = settings_directory() {
                    self.settings = read_settings_at(&dir);
                    self.settings_dir = Some(dir);
                }
            }

            let status = self.backend_status();
            if let Some(dir) = &self.settings_dir {
                publish_status(dir, &status);
            }
        }

        fn backend_status(&mut self) -> String {
            #[cfg(feature = "audio")]
            {
                self.audio.update(&self.settings)
            }
            #[cfg(not(feature = "audio"))]
            {
                if self.settings.microphone_enabled && !self.settings.muted {
                    "Audio backend not built — rebuild Horizon with --audio".into()
                } else if self.settings.microphone_enabled && self.settings.muted {
                    "Muted — microphone disabled".into()
                } else {
                    "Microphone off".into()
                }
            }
        }
    }

    std::thread_local! {
        static RUNTIME: std::cell::RefCell<Runtime> =
            std::cell::RefCell::new(Runtime::default());
    }

    /// Called on the game thread by the native Game::tick mixin hook.
    pub fn tick() {
        RUNTIME.with(|runtime| runtime.borrow_mut().tick());
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::time::{SystemTime, UNIX_EPOCH};

        fn temp_dir() -> PathBuf {
            std::env::temp_dir().join(format!(
                "horizon-voice-bridge-{}-{}",
                std::process::id(),
                SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
            ))
        }

        #[test]
        fn defaults_are_privacy_safe_and_distance_is_bounded() {
            let dir = temp_dir();
            fs::create_dir_all(&dir).unwrap();
            let settings = read_settings_at(&dir);
            assert!(!settings.microphone_enabled);
            assert!(!settings.muted);
            assert!(settings.push_to_talk);
            assert!(!settings.ptt_down);
            assert_eq!(settings.proximity_blocks, 16);
            fs::remove_dir_all(dir).unwrap();
        }

        #[test]
        fn parses_companion_storage_and_ignores_bad_values() {
            let dir = temp_dir();
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("mic_enabled"), b"true").unwrap();
            fs::write(dir.join("muted"), b"false").unwrap();
            fs::write(dir.join("ptt"), b"false").unwrap();
            fs::write(dir.join("ptt_down"), b"true").unwrap();
            fs::write(dir.join("distance"), b"3").unwrap();

            let settings = read_settings_at(&dir);
            assert!(settings.microphone_enabled);
            assert!(!settings.muted);
            assert!(!settings.push_to_talk);
            assert!(settings.ptt_down);
            assert_eq!(settings.proximity_blocks, 48);

            fs::write(dir.join("distance"), b"900").unwrap();
            assert_eq!(read_settings_at(&dir).proximity_blocks, 16);
            fs::remove_dir_all(dir).unwrap();
        }

        #[test]
        fn push_to_talk_and_mute_gate_transmission() {
            let mut settings = Settings {
                microphone_enabled: true,
                muted: false,
                push_to_talk: true,
                ptt_down: false,
                proximity_blocks: 16,
            };
            assert!(!transmit_allowed(&settings));
            settings.ptt_down = true;
            assert!(transmit_allowed(&settings));
            settings.muted = true;
            assert!(!transmit_allowed(&settings));
            settings.muted = false;
            settings.push_to_talk = false;
            settings.ptt_down = false;
            assert!(transmit_allowed(&settings));
            settings.microphone_enabled = false;
            assert!(!transmit_allowed(&settings));
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
    use super::jitter_buffer::{JitterBuffer, PushError};
    use super::pcm_capture::PcmFrameAssembler;
    use super::pcm_codec::{PcmFrame, PcmFrameError};
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

    #[test]
    fn pcm_frame_assembler_builds_mono_frames_from_interleaved_samples() {
        use std::sync::Arc;
        use std::sync::atomic::AtomicU64;
        use std::sync::mpsc::sync_channel;

        let (sender, receiver) = sync_channel(2);
        let dropped = Arc::new(AtomicU64::new(0));
        let mut assembler = PcmFrameAssembler::new(2, 8_000, dropped).unwrap();
        let input: Vec<i16> = (0..160).flat_map(|_| [1_000_i16, 3_000_i16]).collect();
        assembler.push(&input, |sample| sample, &sender);
        let frame = receiver.try_recv().unwrap();
        assert_eq!(frame.len(), 80);
        assert!(frame.iter().all(|sample| *sample == 2_000));
    }

    #[test]
    fn pcm_frame_payload_round_trips_and_fits_voice_datagram() {
        let frame = PcmFrame {
            sample_rate: 48_000,
            samples: vec![123; 480],
        };
        let packet = frame.to_packet(17).unwrap();
        let wire = packet.encode().unwrap();
        assert!(wire.len() < super::voice_protocol::MAX_DATAGRAM_BYTES);
        let decoded_packet = Packet::decode(&wire).unwrap();
        assert_eq!(PcmFrame::from_packet(&decoded_packet).unwrap(), frame);
    }

    #[test]
    fn pcm_frame_payload_rejects_wrong_lengths_rates_and_packet_kinds() {
        let frame = PcmFrame {
            sample_rate: 16_000,
            samples: vec![0; 160],
        };
        let mut payload = frame.encode_payload().unwrap();
        payload[4] = 159;
        assert_eq!(
            PcmFrame::decode_payload(&payload),
            Err(PcmFrameError::InvalidFrameLength)
        );

        assert_eq!(
            PcmFrame::decode_payload(&[0; 5]),
            Err(PcmFrameError::PayloadTooShort)
        );

        let invalid_rate = PcmFrame {
            sample_rate: 96_000,
            samples: vec![0; 960],
        };
        assert_eq!(
            invalid_rate.encode_payload(),
            Err(PcmFrameError::InvalidSampleRate)
        );

        let hello = Packet {
            kind: PacketKind::Hello,
            sequence: 1,
            payload: frame.encode_payload().unwrap(),
        };
        assert_eq!(
            PcmFrame::from_packet(&hello),
            Err(PcmFrameError::WrongPacketKind)
        );
    }
}

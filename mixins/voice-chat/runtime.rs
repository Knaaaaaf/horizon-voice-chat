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

#[cfg(feature = "audio")]
pub mod pcm_playback {
    use std::{
        collections::VecDeque,
        sync::{Arc, Mutex},
        time::Duration,
    };

    use cpal::{
        Device, SampleFormat, SizedSample, Stream, StreamConfig,
        traits::{DeviceTrait, HostTrait, StreamTrait},
    };

    const MAX_QUEUE_SECONDS: usize = 2;

    /// Mono output samples in device sample rate. Writers are the relay worker;
    /// the real-time device callback only drains this bounded queue.
    pub struct PlaybackQueue {
        sample_rate: u32,
        max_samples: usize,
        samples: VecDeque<i16>,
    }

    impl PlaybackQueue {
        fn new(sample_rate: u32) -> Self {
            Self {
                sample_rate,
                max_samples: sample_rate as usize * MAX_QUEUE_SECONDS,
                samples: VecDeque::with_capacity(sample_rate as usize / 2),
            }
        }

        /// Resample a mono frame and append it without allowing unbounded latency.
        pub fn enqueue(&mut self, samples: &[i16], source_rate: u32) {
            if samples.is_empty() || !(8_000..=48_000).contains(&source_rate) {
                return;
            }
            let output_len = (samples.len() * self.sample_rate as usize)
                .div_ceil(source_rate as usize);
            let mut converted = Vec::with_capacity(output_len);
            for index in 0..output_len {
                let source_index = index * source_rate as usize / self.sample_rate as usize;
                converted.push(samples[source_index.min(samples.len() - 1)]);
            }
            if converted.len() > self.max_samples {
                converted.drain(..converted.len() - self.max_samples);
            }
            let overflow = self.samples.len() + converted.len();
            if overflow > self.max_samples {
                let discard = overflow - self.max_samples;
                self.samples.drain(..discard.min(self.samples.len()));
            }
            self.samples.extend(converted);
        }
    }

    pub struct AudioPlayback {
        _stream: Stream,
        queue: Arc<Mutex<PlaybackQueue>>,
    }

    impl AudioPlayback {
        pub fn start() -> Result<Self, String> {
            let host = cpal::default_host();
            let device = host
                .default_output_device()
                .ok_or_else(|| "no default audio output device".to_string())?;
            let supported = device
                .default_output_config()
                .map_err(|error| format!("default output config: {error}"))?;
            let sample_rate = supported.sample_rate().0;
            if !(8_000..=192_000).contains(&sample_rate) || supported.channels() == 0 {
                return Err(format!("unsupported output config: {sample_rate} Hz"));
            }
            let config: StreamConfig = supported.clone().into();
            let queue = Arc::new(Mutex::new(PlaybackQueue::new(sample_rate)));
            let stream = match supported.sample_format() {
                SampleFormat::F32 => build_stream::<f32, _>(
                    &device, &config, Arc::clone(&queue),
                    |sample| sample as f32 / 32768.0,
                )?,
                SampleFormat::I16 => build_stream::<i16, _>(
                    &device, &config, Arc::clone(&queue), |sample| sample,
                )?,
                SampleFormat::U16 => build_stream::<u16, _>(
                    &device, &config, Arc::clone(&queue),
                    |sample| (i32::from(sample) + 32_768) as u16,
                )?,
                _ => return Err("unsupported audio output sample format".into()),
            };
            stream.play().map_err(|error| format!("start output stream: {error}"))?;
            Ok(Self { _stream: stream, queue })
        }

        pub fn queue(&self) -> Arc<Mutex<PlaybackQueue>> {
            Arc::clone(&self.queue)
        }
    }

    fn build_stream<T, F>(
        device: &Device,
        config: &StreamConfig,
        queue: Arc<Mutex<PlaybackQueue>>,
        convert: F,
    ) -> Result<Stream, String>
    where
        T: SizedSample + Copy,
        F: Fn(i16) -> T + Send + 'static,
    {
        let channels = usize::from(config.channels).max(1);
        device
            .build_output_stream(
                config,
                move |data: &mut [T], _| {
                    let mut samples = match queue.lock() {
                        Ok(queue) => queue,
                        Err(poisoned) => poisoned.into_inner(),
                    };
                    for frame in data.chunks_mut(channels) {
                        let sample = samples.samples.pop_front().unwrap_or(0);
                        for channel in frame {
                            *channel = convert(sample);
                        }
                    }
                },
                |error| eprintln!("[horizon-voice-chat] speaker stream: {error}"),
                Some(Duration::from_millis(100)),
            )
            .map_err(|error| format!("build output stream: {error}"))
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
        if let Ok(exe) = std::env::current_exe()
            && let Some(parent) = exe.parent()
        {
            roots.extend(parent.ancestors().take(5).map(Path::to_path_buf));
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
    mod relay_client {
        use std::{
            collections::BTreeMap,
            io::Read,
            sync::{
                Arc, Mutex,
                mpsc::{self, Receiver, SyncSender, TrySendError},
            },
            thread,
            time::{Duration, Instant, SystemTime, UNIX_EPOCH},
        };

        use super::super::{
            pcm_codec::PcmFrame,
            pcm_playback::PlaybackQueue,
            voice_protocol::{MAX_DATAGRAM_BYTES, Packet, PacketKind},
        };

        const MAX_UPLOAD_PACKETS: usize = 32;
        const MAX_RESPONSE_BYTES: usize = 512 * 1024;
        const MAX_RESPONSE_PACKETS: usize = 256;
        const PLAYOUT_DELAY_SLOTS: u64 = 8;
        const MIX_RATE: u32 = 48_000;

        struct Config {
            endpoint: String,
            token: String,
            room: String,
            client_id: String,
            session_id: String,
        }

        struct RemoteFrame {
            slot: u64,
            sample_rate: u32,
            samples: Vec<i16>,
        }

        fn safe_id(value: &str) -> bool {
            !value.is_empty()
                && value.len() <= 64
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
        }

        impl Config {
            fn from_env() -> Result<Option<Self>, String> {
                let endpoint = std::env::var("HORIZON_VOICE_CHAT_RELAY_URL").ok();
                let token = std::env::var("HORIZON_VOICE_CHAT_RELAY_TOKEN").ok();
                let room = std::env::var("HORIZON_VOICE_CHAT_ROOM").ok();
                let client_id = std::env::var("HORIZON_VOICE_CHAT_CLIENT_ID").ok();

                if endpoint.is_none() && token.is_none() && room.is_none() && client_id.is_none() {
                    return Ok(None);
                }

                let endpoint = endpoint.ok_or_else(|| {
                    "Relay not configured: missing HORIZON_VOICE_CHAT_RELAY_URL".to_string()
                })?;
                let token = token.ok_or_else(|| {
                    "Relay not configured: missing HORIZON_VOICE_CHAT_RELAY_TOKEN".to_string()
                })?;
                let room = room.ok_or_else(|| {
                    "Relay not configured: missing HORIZON_VOICE_CHAT_ROOM".to_string()
                })?;
                let client_id = client_id.ok_or_else(|| {
                    "Relay not configured: missing HORIZON_VOICE_CHAT_CLIENT_ID".to_string()
                })?;

                // ureq validates the HTTPS certificate by default. Never allow
                // credentials or voice payloads to travel over plain HTTP.
                if !endpoint.starts_with("https://")
                    || !endpoint.ends_with("/v1/frames")
                    || endpoint.contains('@')
                    || endpoint.contains('#')
                    || endpoint.contains('?')
                {
                    return Err("Relay URL must be an HTTPS endpoint ending in /v1/frames".into());
                }
                if token.len() < 32
                    || token
                        .bytes()
                        .any(|byte| !(byte.is_ascii_alphanumeric() || b"-_".contains(&byte)))
                {
                    return Err("Relay token must be at least 32 URL-safe characters".into());
                }
                if !safe_id(&room) || !safe_id(&client_id) {
                    return Err(
                        "Relay room/client ID must use 1–64 ASCII letters, digits, '.', '_' or '-'"
                            .into(),
                    );
                }

                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos();
                let session_id = format!("s-{}-{now:x}", std::process::id());
                Ok(Some(Self {
                    endpoint,
                    token,
                    room,
                    client_id,
                    session_id,
                }))
            }
        }

        pub(super) struct RelayWorker {
            sender: SyncSender<Vec<Vec<u8>>>,
            statuses: Receiver<String>,
        }

        impl RelayWorker {
            pub(super) fn configuration_requested() -> bool {
                [
                    "HORIZON_VOICE_CHAT_RELAY_URL",
                    "HORIZON_VOICE_CHAT_RELAY_TOKEN",
                    "HORIZON_VOICE_CHAT_ROOM",
                    "HORIZON_VOICE_CHAT_CLIENT_ID",
                ]
                .iter()
                .any(|key| std::env::var_os(key).is_some())
            }

            pub(super) fn start(
                playback: Option<Arc<Mutex<PlaybackQueue>>>,
            ) -> Result<Option<Self>, String> {
                let Some(config) = Config::from_env()? else {
                    return Ok(None);
                };
                let (sender, outbound) = mpsc::sync_channel::<Vec<Vec<u8>>>(8);
                let (status_sender, statuses) = mpsc::channel::<String>();
                thread::Builder::new()
                    .name("horizon-voice-relay".into())
                    .spawn(move || run(config, outbound, status_sender, playback))
                    .map_err(|_| "Could not start relay worker thread".to_string())?;
                Ok(Some(Self { sender, statuses }))
            }

            pub(super) fn send(&self, frames: Vec<Vec<u8>>) -> Result<(), ()> {
                match self.sender.try_send(frames) {
                    Ok(()) => Ok(()),
                    Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => Err(()),
                }
            }

            pub(super) fn latest_status(&self) -> Option<String> {
                self.statuses.try_iter().last()
            }
        }

        fn upload_body(frames: &[Vec<u8>]) -> Option<Vec<u8>> {
            if frames.is_empty() || frames.len() > MAX_UPLOAD_PACKETS {
                return None;
            }
            let mut body = Vec::with_capacity(
                6 + frames.iter().map(|frame| frame.len() + 2).sum::<usize>(),
            );
            body.extend_from_slice(b"HVB1");
            body.extend_from_slice(&(frames.len() as u16).to_le_bytes());
            for frame in frames {
                if frame.len() > MAX_DATAGRAM_BYTES || frame.len() > u16::MAX as usize {
                    return None;
                }
                let packet = Packet::decode(frame).ok()?;
                if packet.kind != PacketKind::AudioFrame || PcmFrame::from_packet(&packet).is_err() {
                    return None;
                }
                body.extend_from_slice(&(frame.len() as u16).to_le_bytes());
                body.extend_from_slice(frame);
            }
            Some(body)
        }

        /// Response wire layout: HVR1, next cursor (u64), server time-slot (u64),
        /// count (u16), then slot (u64), sender-id and packet for each record.
        fn parse_response(bytes: &[u8], own_id: &str) -> Option<(u64, u64, Vec<RemoteFrame>)> {
            if bytes.len() < 22 || bytes.len() > MAX_RESPONSE_BYTES || &bytes[..4] != b"HVR1" {
                return None;
            }
            let cursor = u64::from_le_bytes(bytes[4..12].try_into().ok()?);
            let server_slot = u64::from_le_bytes(bytes[12..20].try_into().ok()?);
            let count = u16::from_le_bytes([bytes[20], bytes[21]]) as usize;
            if count > MAX_RESPONSE_PACKETS {
                return None;
            }
            let mut at = 22usize;
            let mut remote = Vec::with_capacity(count);
            for _ in 0..count {
                if at + 9 > bytes.len() {
                    return None;
                }
                let slot = u64::from_le_bytes(bytes[at..at + 8].try_into().ok()?);
                at += 8;
                let sender_len = bytes[at] as usize;
                at += 1;
                if sender_len == 0 || sender_len > 64 || at + sender_len + 2 > bytes.len() {
                    return None;
                }
                let sender = std::str::from_utf8(&bytes[at..at + sender_len]).ok()?;
                if !safe_id(sender) {
                    return None;
                }
                at += sender_len;
                let packet_len = u16::from_le_bytes([bytes[at], bytes[at + 1]]) as usize;
                at += 2;
                if packet_len > MAX_DATAGRAM_BYTES || at + packet_len > bytes.len() {
                    return None;
                }
                let packet = Packet::decode(&bytes[at..at + packet_len]).ok()?;
                let frame = PcmFrame::from_packet(&packet).ok()?;
                if packet.kind != PacketKind::AudioFrame {
                    return None;
                }
                if sender != own_id {
                    remote.push(RemoteFrame {
                        slot,
                        sample_rate: frame.sample_rate,
                        samples: frame.samples,
                    });
                }
                at += packet_len;
            }
            (at == bytes.len()).then_some((cursor, server_slot, remote))
        }

        fn report(status: &mpsc::Sender<String>, previous: &mut String, next: String) {
            if *previous != next {
                *previous = next.clone();
                let _ = status.send(next);
            }
        }

        fn mix_incoming(pending: &mut BTreeMap<u64, Vec<i32>>, frames: Vec<RemoteFrame>, now_slot: u64) {
            for frame in frames {
                // Frames outside the jitter window are too late to play in sync.
                if frame.slot.saturating_add(PLAYOUT_DELAY_SLOTS) <= now_slot
                    || !(8_000..=48_000).contains(&frame.sample_rate)
                {
                    continue;
                }
                let mixed = pending.entry(frame.slot).or_insert_with(|| vec![0; 480]);
                for (index, accumulated) in mixed.iter_mut().enumerate() {
                    let source_index = index * frame.sample_rate as usize / MIX_RATE as usize;
                    let sample = frame.samples[source_index.min(frame.samples.len() - 1)];
                    *accumulated =
                        (*accumulated + i32::from(sample)).clamp(i16::MIN as i32, i16::MAX as i32);
                }
            }
            while pending.len() > 512 {
                pending.pop_first();
            }
        }

        fn flush_ready(
            pending: &mut BTreeMap<u64, Vec<i32>>,
            playback: &Option<Arc<Mutex<PlaybackQueue>>>,
            now_slot: u64,
        ) {
            let ready: Vec<u64> = pending
                .keys()
                .copied()
                .take_while(|slot| slot.saturating_add(PLAYOUT_DELAY_SLOTS) <= now_slot)
                .collect();
            for slot in ready {
                let Some(mixed) = pending.remove(&slot) else {
                    continue;
                };
                if let Some(playback) = playback {
                    let samples: Vec<i16> = mixed
                        .into_iter()
                        .map(|sample| sample.clamp(i16::MIN as i32, i16::MAX as i32) as i16)
                        .collect();
                    let mut queue = match playback.lock() {
                        Ok(queue) => queue,
                        Err(poisoned) => poisoned.into_inner(),
                    };
                    queue.enqueue(&samples, MIX_RATE);
                }
            }
        }

        fn run(
            config: Config,
            outbound: Receiver<Vec<Vec<u8>>>,
            status: mpsc::Sender<String>,
            playback: Option<Arc<Mutex<PlaybackQueue>>>,
        ) {
            // All HTTPS requests live on this worker thread, never on Game::tick.
            let agent = ureq::AgentBuilder::new()
                .timeout(Duration::from_millis(700))
                .redirects(0)
                .build();
            let mut last_status = String::new();
            let mut cursor = 0u64;
            let mut sent_frames = 0u64;
            let mut received_frames = 0u64;
            let mut connected = false;
            let mut last_report = Instant::now() - Duration::from_secs(2);
            let mut pending = BTreeMap::<u64, Vec<i32>>::new();
            report(&status, &mut last_status, "Relay connecting over HTTPS".into());

            loop {
                let mut frames = Vec::new();
                if let Ok(batch) = outbound.recv_timeout(Duration::from_millis(35)) {
                    frames.extend(batch);
                    while frames.len() < MAX_UPLOAD_PACKETS {
                        match outbound.try_recv() {
                            Ok(batch) => {
                                let remaining = MAX_UPLOAD_PACKETS - frames.len();
                                frames.extend(batch.into_iter().take(remaining));
                            }
                            Err(_) => break,
                        }
                    }
                }

                if !frames.is_empty() {
                    let body = upload_body(&frames);
                    let result = body.as_ref().ok_or(()).and_then(|body| {
                        agent
                            .post(&config.endpoint)
                            .set("Authorization", &format!("Bearer {}", config.token))
                            .set("X-HVC-Client", &config.client_id)
                            .set("X-HVC-Session", &config.session_id)
                            .set("X-HVC-Room", &config.room)
                            .set("Content-Type", "application/octet-stream")
                            .send_bytes(body)
                            .map(|_| ())
                            .map_err(|_| ())
                    });
                    if result.is_ok() {
                        sent_frames = sent_frames.saturating_add(frames.len() as u64);
                        connected = true;
                    } else {
                        connected = false;
                    }
                }

                let response = agent
                    .get(&config.endpoint)
                    .set("Authorization", &format!("Bearer {}", config.token))
                    .set("X-HVC-Client", &config.client_id)
                    .set("X-HVC-Session", &config.session_id)
                    .set("X-HVC-Room", &config.room)
                    .set("X-HVC-Cursor", &cursor.to_string())
                    .call();

                if let Ok(response) = response {
                    let mut body = Vec::new();
                    let read_ok = response
                        .into_reader()
                        .take((MAX_RESPONSE_BYTES + 1) as u64)
                        .read_to_end(&mut body)
                        .is_ok();
                    if read_ok
                        && let Some((next_cursor, server_slot, incoming)) =
                            parse_response(&body, &config.client_id)
                    {
                        cursor = next_cursor;
                        received_frames = received_frames.saturating_add(incoming.len() as u64);
                        mix_incoming(&mut pending, incoming, server_slot);
                        flush_ready(&mut pending, &playback, server_slot);
                        connected = true;
                    } else {
                        connected = false;
                    }
                } else {
                    connected = false;
                }

                if last_report.elapsed() >= Duration::from_secs(1) {
                    let playback_state = if playback.is_some() {
                        "playback active"
                    } else {
                        "playback unavailable"
                    };
                    let state = if connected {
                        format!(
                            "Relay connected — sent {sent_frames}, received {received_frames} remote frames; {playback_state}"
                        )
                    } else {
                        "Relay offline — check HTTPS endpoint, certificate and client token".into()
                    };
                    report(&status, &mut last_status, state);
                    last_report = Instant::now();
                }
                thread::sleep(Duration::from_millis(15));
            }
        }

        #[cfg(test)]
        mod tests {
            use super::*;

            #[test]
            fn relay_ids_reject_header_injection() {
                assert!(!safe_id("../room"));
                assert!(!safe_id("room\\nAuthorization"));
                assert!(!safe_id(""));
                assert!(!safe_id(&"x".repeat(65)));
                assert!(safe_id("survival-1"));
            }

            #[test]
            fn relay_uploads_are_bounded_and_validate_audio_packets() {
                let frame = PcmFrame {
                    sample_rate: 8_000,
                    samples: vec![0; 80],
                };
                let packet = frame.to_packet(7).unwrap().encode().unwrap();
                let body = upload_body(std::slice::from_ref(&packet)).unwrap();
                assert_eq!(&body[..4], b"HVB1");
                assert_eq!(u16::from_le_bytes([body[4], body[5]]), 1);
                assert_eq!(
                    u16::from_le_bytes([body[6], body[7]]) as usize,
                    packet.len()
                );
                assert!(upload_body(&[]).is_none());
                assert!(upload_body(&[vec![0; MAX_DATAGRAM_BYTES + 1]]).is_none());
            }

            #[test]
            fn relay_response_rejects_invalid_headers_and_trailing_data() {
                assert!(parse_response(b"nope", "client").is_none());
                let mut empty = Vec::from(&b"HVR1"[..]);
                empty.extend_from_slice(&0u64.to_le_bytes());
                empty.extend_from_slice(&0u64.to_le_bytes());
                empty.extend_from_slice(&0u16.to_le_bytes());
                assert!(parse_response(&empty, "client").is_some());
                empty.push(1);
                assert!(parse_response(&empty, "client").is_none());
            }
        }
    }

    #[cfg(feature = "audio")]
    struct AudioRuntime {
        capture: Option<super::microphone::MicrophoneCapture>,
        receiver: Option<std::sync::mpsc::Receiver<Vec<i16>>>,
        attempted: bool,
        last_error: Option<String>,
        relay: Option<relay_client::RelayWorker>,
        _playback: Option<super::pcm_playback::AudioPlayback>,
        relay_attempted: bool,
        relay_status: String,
        sequence: u32,
    }

    #[cfg(feature = "audio")]
    impl Default for AudioRuntime {
        fn default() -> Self {
            Self {
                capture: None,
                receiver: None,
                attempted: false,
                last_error: None,
                relay: None,
                _playback: None,
                relay_attempted: false,
                relay_status: "Relay not configured — local capture only".into(),
                sequence: 0,
            }
        }
    }

    #[cfg(feature = "audio")]
    impl AudioRuntime {
        fn update(&mut self, settings: &Settings) -> String {
            if !self.relay_attempted {
                self.relay_attempted = true;
                let mut playback_queue = None;
                if relay_client::RelayWorker::configuration_requested() {
                    match super::pcm_playback::AudioPlayback::start() {
                        Ok(playback) => {
                            playback_queue = Some(playback.queue());
                            self._playback = Some(playback);
                        }
                        Err(_) => {}
                    }
                }
                match relay_client::RelayWorker::start(playback_queue) {
                    Ok(Some(worker)) => {
                        self.relay = Some(worker);
                        self.relay_status = "Relay connecting over HTTPS".into();
                    }
                    Ok(None) => {
                        self.relay_status = "Relay not configured — local capture only".into();
                    }
                    Err(error) => self.relay_status = error,
                }
            }
            if let Some(relay) = &self.relay
                && let Some(status) = relay.latest_status()
            {
                self.relay_status = status;
            }

            let requested = settings.microphone_enabled && !settings.muted;
            if !requested {
                self.capture = None;
                self.receiver = None;
                self.attempted = false;
                self.last_error = None;
                return if settings.microphone_enabled && settings.muted {
                    format!("Muted — native microphone stopped; {}", self.relay_status)
                } else {
                    format!("Microphone off; {}", self.relay_status)
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

            let mut captured = Vec::new();
            if let Some(receiver) = &self.receiver {
                // Bound work on the game thread by the microphone channel's
                // fixed capacity. The callback itself never blocks on sending.
                while let Ok(samples) = receiver.try_recv() {
                    captured.push(samples);
                }
            }

            if let Some(capture) = &self.capture {
                let sample_rate = capture.sample_rate();
                let mut encoded = Vec::with_capacity(captured.len());
                for samples in captured {
                    // Push-to-talk is a hard gate. The optional voice-activation
                    // mode uses a conservative frame-energy threshold; it is
                    // a basic level gate, not a noise-robust speech detector.
                    let gate_open = transmit_allowed(settings)
                        && (settings.push_to_talk || frame_has_voice_activity(&samples));
                    if !gate_open {
                        continue;
                    }
                    let frame = super::pcm_codec::PcmFrame {
                        sample_rate,
                        samples,
                    };
                    if let Ok(packet) = frame.to_packet(self.sequence) {
                        self.sequence = self.sequence.wrapping_add(1);
                        if let Ok(bytes) = packet.encode() {
                            encoded.push(bytes);
                        }
                    }
                }

                if !encoded.is_empty() {
                    if let Some(relay) = &self.relay
                        && relay.send(encoded).is_err()
                    {
                        self.relay_status =
                            "Relay transmit queue busy — outgoing frames dropped".into();
                    }
                }
            }

            if self.capture.is_none() {
                return format!(
                    "Microphone error — {}; {}",
                    self.last_error
                        .as_deref()
                        .unwrap_or("capture did not start"),
                    self.relay_status
                );
            }
            if settings.push_to_talk {
                if settings.ptt_down {
                    format!("Capturing locally — PTT held; {}", self.relay_status)
                } else {
                    format!("Capturing locally — PTT released; {}", self.relay_status)
                }
            } else {
                format!("Capturing locally — level-gated voice activation; {}", self.relay_status)
            }
        }
    }

    #[cfg(feature = "audio")]
    fn frame_has_voice_activity(samples: &[i16]) -> bool {
        if samples.is_empty() {
            return false;
        }
        let power = samples.iter().map(|sample| {
            let sample = i64::from(*sample);
            (sample * sample) as u128
        }).sum::<u128>();
        // RMS > 600/32768 (roughly -35 dBFS). This intentionally simple gate
        // is only a fallback; a real voice activity detector is still needed.
        power / samples.len() as u128 > 600_u128 * 600_u128
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

            // 20 game ticks/s: poll twice a second. Retry profile discovery
            // at the same cadence when the launcher uses an unusual directory.
            if (self.ticks == 1 || self.ticks % 10 == 1)
                && let Some(dir) = settings_directory()
            {
                self.settings = read_settings_at(&dir);
                self.settings_dir = Some(dir);
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
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
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

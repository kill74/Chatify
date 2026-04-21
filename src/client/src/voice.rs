//! Voice session management for client.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::{mpsc as std_mpsc, Arc, Mutex};
use std::thread;

use base64::{engine::general_purpose, Engine as _};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, Stream, StreamConfig};
use rodio::buffer::SamplesBuffer;
use rodio::{OutputStream, Sink};

use clifford::error::{ChatifyError, ChatifyResult};
use clifford::voice::AudioProcessor;

macro_rules! eprintln {
    ($($arg:tt)*) => {{
        crate::ui::emit_output_line(format!($($arg)*), true);
    }};
}

const SPEAKING_RMS_THRESHOLD: f32 = 0.010;
const SPEAKING_SIGNAL_COOLDOWN_MS: u64 = 150;
const SPEAKING_SIGNAL_SILENCE_MS: u64 = 450;

const JITTER_BUFFER_TARGET_FRAMES: usize = 3;
const JITTER_BUFFER_MAX_FRAMES: usize = 24;
const JITTER_BUFFER_GAP_RESET_THRESHOLD: u64 = 12;
const VOICE_FRAME_MAGIC: [u8; 4] = *b"CVR1";
const VOICE_FRAME_HEADER_LEN: usize = 10;
const MAX_VOICE_FRAME_SAMPLES: usize = 32_768;
const MAX_VOICE_CHANNELS: u16 = 8;
const MIN_VOICE_SAMPLE_RATE: u32 = 8_000;
const MAX_VOICE_SAMPLE_RATE: u32 = 192_000;

pub struct VoiceFrame {
    pub sample_rate: u32,
    pub channels: u16,
    pub samples: Vec<i16>,
}

pub struct VoicePlaybackPacket {
    pub source: String,
    pub seq: Option<u64>,
    pub capture_ts_ms: Option<u64>,
    pub frame: VoiceFrame,
}

#[derive(Default)]
struct PerSourceJitterBuffer {
    pending: BTreeMap<u64, VoiceFrame>,
    next_seq: Option<u64>,
    started: bool,
}

impl PerSourceJitterBuffer {
    fn push(&mut self, seq: Option<u64>, frame: VoiceFrame) -> Vec<VoiceFrame> {
        let Some(seq) = seq else {
            return vec![frame];
        };

        if self.pending.contains_key(&seq) {
            return Vec::new();
        }

        self.pending.insert(seq, frame);
        if self.next_seq.is_none() {
            self.next_seq = self.pending.keys().next().copied();
        }

        if !self.started && self.pending.len() >= JITTER_BUFFER_TARGET_FRAMES {
            self.started = true;
        }

        let mut ready = Vec::new();

        if self.started {
            self.drain_contiguous(&mut ready);
        }

        if self.pending.len() > JITTER_BUFFER_MAX_FRAMES {
            if let Some((&first_seq, _)) = self.pending.iter().next() {
                self.next_seq = Some(first_seq);
                self.started = true;
                self.drain_contiguous(&mut ready);
            }
        }

        if self.started {
            if let (Some(expected), Some((&first_seq, _))) =
                (self.next_seq, self.pending.iter().next())
            {
                if first_seq > expected
                    && first_seq.saturating_sub(expected) > JITTER_BUFFER_GAP_RESET_THRESHOLD
                {
                    self.next_seq = Some(first_seq);
                    self.drain_contiguous(&mut ready);
                }
            }
        }

        ready
    }

    fn drain_contiguous(&mut self, out: &mut Vec<VoiceFrame>) {
        while let Some(expected) = self.next_seq {
            if let Some(frame) = self.pending.remove(&expected) {
                out.push(frame);
                self.next_seq = Some(expected.wrapping_add(1));
            } else {
                break;
            }
        }
    }
}

pub struct VoiceSession {
    pub room: String,
    pub event_tx: std_mpsc::Sender<VoiceEvent>,
    pub task: thread::JoinHandle<()>,
}

pub enum VoiceEvent {
    Captured(VoiceFrame),
    Playback(VoiceFrame),
    PlaybackPacket(VoicePlaybackPacket),
    #[allow(dead_code)]
    MuteState(bool),
    #[allow(dead_code)]
    SpeakingState(bool),
    Stop,
}

pub fn start_voice_session(
    room: String,
    ws_tx: tokio::sync::mpsc::UnboundedSender<String>,
) -> ChatifyResult<VoiceSession> {
    let (event_tx, event_rx) = std_mpsc::channel::<VoiceEvent>();
    let (ready_tx, ready_rx) = std_mpsc::channel::<ChatifyResult<()>>();
    let ws_tx_task = ws_tx;
    let room_task = room.clone();
    let event_tx_task = event_tx.clone();

    let task = thread::spawn(move || {
        let host = cpal::default_host();
        let input_device = match host.default_input_device() {
            Some(d) => d,
            None => {
                let _ = ready_tx.send(Err(ChatifyError::Audio(
                    "no default input device available".to_string(),
                )));
                return;
            }
        };

        let input_supported = match input_device.default_input_config() {
            Ok(cfg) => cfg,
            Err(e) => {
                let _ = ready_tx.send(Err(ChatifyError::Audio(format!(
                    "failed to fetch default input config: {}",
                    e
                ))));
                return;
            }
        };

        let format = input_supported.sample_format();
        let input_config: StreamConfig = input_supported.into();
        let sample_rate = input_config.sample_rate.0;
        let channels = input_config.channels;
        let chunk_samples = 320usize
            .saturating_mul(channels as usize)
            .max(channels as usize);

        let (_output_stream, output_handle) = match OutputStream::try_default() {
            Ok(v) => v,
            Err(e) => {
                let _ = ready_tx.send(Err(ChatifyError::Audio(format!(
                    "audio output init failed: {}",
                    e
                ))));
                return;
            }
        };

        let sink = match Sink::try_new(&output_handle) {
            Ok(s) => s,
            Err(e) => {
                let _ = ready_tx.send(Err(ChatifyError::Audio(format!(
                    "audio output sink init failed: {}",
                    e
                ))));
                return;
            }
        };

        let mut audio_processor = AudioProcessor::new(sample_rate, channels.into());
        let mut voice_sequence: u64 = 0;
        let mut speaking_state = false;
        let mut last_voice_activity_ms = 0u64;
        let mut last_speaking_emit_ms = 0u64;
        let mut playback_jitter: HashMap<String, PerSourceJitterBuffer> = HashMap::new();

        let pending = Arc::new(Mutex::new(VecDeque::<i16>::new()));

        let result_stream = match format {
            SampleFormat::I16 => build_input_stream(
                &input_device,
                &input_config,
                pending.clone(),
                event_tx.clone(),
                chunk_samples,
            ),
            SampleFormat::U16 => build_input_stream_u16(
                &input_device,
                &input_config,
                pending.clone(),
                event_tx.clone(),
                chunk_samples,
            ),
            SampleFormat::F32 => build_input_stream_f32(
                &input_device,
                &input_config,
                pending.clone(),
                event_tx.clone(),
                chunk_samples,
            ),
        };

        let input_stream = match result_stream {
            Ok(s) => s,
            Err(e) => {
                let _ = ready_tx.send(Err(e));
                return;
            }
        };

        if let Err(e) = input_stream.play() {
            let _ = ready_tx.send(Err(ChatifyError::Audio(format!(
                "failed to start microphone stream: {}",
                e
            ))));
            return;
        }

        let _ = ready_tx.send(Ok(()));

        while let Ok(event) = event_rx.recv() {
            match event {
                VoiceEvent::Captured(frame) => {
                    let processed = audio_processor.process_capture(&frame.samples);
                    let voiced = is_voiced_frame(&processed);
                    let payload = encode_voice_frame(&VoiceFrame {
                        sample_rate: frame.sample_rate,
                        channels: frame.channels,
                        samples: processed,
                    });
                    let capture_ts_ms = now_ms();
                    let seq = voice_sequence;
                    voice_sequence = voice_sequence.wrapping_add(1);
                    if voiced {
                        last_voice_activity_ms = capture_ts_ms;
                    }

                    let desired_speaking = voiced
                        || (speaking_state
                            && capture_ts_ms.saturating_sub(last_voice_activity_ms)
                                <= SPEAKING_SIGNAL_SILENCE_MS);

                    if desired_speaking != speaking_state
                        && (last_speaking_emit_ms == 0
                            || capture_ts_ms.saturating_sub(last_speaking_emit_ms)
                                >= SPEAKING_SIGNAL_COOLDOWN_MS)
                    {
                        speaking_state = desired_speaking;
                        last_speaking_emit_ms = capture_ts_ms;
                        let _ = ws_tx_task.send(
                            serde_json::json!({
                                "t": "vspeaking",
                                "speaking": speaking_state
                            })
                            .to_string(),
                        );
                    }

                    let _ = ws_tx_task.send(
                        serde_json::json!({
                            "t": "vdata",
                            "r": room_task.clone(),
                            "a": payload,
                            "seq": seq,
                            "capture_ts_ms": capture_ts_ms,
                        })
                        .to_string(),
                    );
                }
                VoiceEvent::PlaybackPacket(packet) => {
                    let source = if packet.source.trim().is_empty() {
                        "unknown".to_string()
                    } else {
                        packet.source
                    };

                    let frames = playback_jitter
                        .entry(source)
                        .or_default()
                        .push(packet.seq, packet.frame);

                    for frame in frames {
                        let processed = audio_processor.process_playback(&frame.samples);
                        sink.append(SamplesBuffer::new(
                            frame.channels,
                            frame.sample_rate,
                            processed,
                        ));
                    }
                }
                VoiceEvent::Playback(frame) => {
                    let processed = audio_processor.process_playback(&frame.samples);
                    sink.append(SamplesBuffer::new(
                        frame.channels,
                        frame.sample_rate,
                        processed,
                    ));
                }
                VoiceEvent::MuteState(_) | VoiceEvent::SpeakingState(_) => {}
                VoiceEvent::Stop => {
                    if speaking_state {
                        let _ = ws_tx_task.send(
                            serde_json::json!({
                                "t": "vspeaking",
                                "speaking": false
                            })
                            .to_string(),
                        );
                    }
                    break;
                }
            }
        }
        sink.stop();
    });

    match ready_rx.recv() {
        Ok(Ok(())) => Ok(VoiceSession {
            room,
            event_tx: event_tx_task,
            task,
        }),
        Ok(Err(e)) => {
            let _ = task.join();
            Err(e)
        }
        Err(_) => {
            let _ = task.join();
            Err(ChatifyError::Audio(
                "voice runtime failed to initialize".to_string(),
            ))
        }
    }
}

fn build_input_stream(
    device: &cpal::Device,
    config: &StreamConfig,
    pending: Arc<Mutex<VecDeque<i16>>>,
    tx: std_mpsc::Sender<VoiceEvent>,
    chunk_samples: usize,
) -> ChatifyResult<Stream> {
    let pending_clone = pending.clone();
    let tx_clone = tx.clone();
    let sample_rate = config.sample_rate.0;
    let channels = config.channels;

    device
        .build_input_stream(
            config,
            move |data: &[i16], _: &cpal::InputCallbackInfo| {
                push_pcm_to_chunks(
                    &pending_clone,
                    data,
                    chunk_samples,
                    sample_rate,
                    channels,
                    &tx_clone,
                );
            },
            |err| {
                eprintln!("voice input error: {}", err);
            },
        )
        .map_err(|e| ChatifyError::Audio(format!("failed to build input stream: {}", e)))
}

fn build_input_stream_u16(
    device: &cpal::Device,
    config: &StreamConfig,
    pending: Arc<Mutex<VecDeque<i16>>>,
    tx: std_mpsc::Sender<VoiceEvent>,
    chunk_samples: usize,
) -> ChatifyResult<Stream> {
    let pending_clone = pending.clone();
    let tx_clone = tx.clone();
    let sample_rate = config.sample_rate.0;
    let channels = config.channels;

    device
        .build_input_stream(
            config,
            move |data: &[u16], _: &cpal::InputCallbackInfo| {
                let converted: Vec<i16> = data.iter().map(|v| u16_to_i16(*v)).collect();
                push_pcm_to_chunks(
                    &pending_clone,
                    &converted,
                    chunk_samples,
                    sample_rate,
                    channels,
                    &tx_clone,
                );
            },
            |err| {
                eprintln!("voice input error: {}", err);
            },
        )
        .map_err(|e| ChatifyError::Audio(format!("failed to build u16 input stream: {}", e)))
}

fn build_input_stream_f32(
    device: &cpal::Device,
    config: &StreamConfig,
    pending: Arc<Mutex<VecDeque<i16>>>,
    tx: std_mpsc::Sender<VoiceEvent>,
    chunk_samples: usize,
) -> ChatifyResult<Stream> {
    let pending_clone = pending.clone();
    let tx_clone = tx.clone();
    let sample_rate = config.sample_rate.0;
    let channels = config.channels;

    device
        .build_input_stream(
            config,
            move |data: &[f32], _: &cpal::InputCallbackInfo| {
                let converted: Vec<i16> = data.iter().map(|v| f32_to_i16(*v)).collect();
                push_pcm_to_chunks(
                    &pending_clone,
                    &converted,
                    chunk_samples,
                    sample_rate,
                    channels,
                    &tx_clone,
                );
            },
            |err| {
                eprintln!("voice input error: {}", err);
            },
        )
        .map_err(|e| ChatifyError::Audio(format!("failed to build f32 input stream: {}", e)))
}

fn u16_to_i16(sample: u16) -> i16 {
    (sample as i32 - 32768).clamp(i16::MIN as i32, i16::MAX as i32) as i16
}

fn f32_to_i16(sample: f32) -> i16 {
    let scaled = (sample.clamp(-1.0, 1.0) * i16::MAX as f32).round();
    scaled.clamp(i16::MIN as f32, i16::MAX as f32) as i16
}

fn push_pcm_to_chunks(
    pending: &Arc<Mutex<VecDeque<i16>>>,
    pcm: &[i16],
    chunk_samples: usize,
    sample_rate: u32,
    channels: u16,
    tx: &std_mpsc::Sender<VoiceEvent>,
) {
    if chunk_samples == 0 {
        return;
    }
    if let Ok(mut q) = pending.lock() {
        for sample in pcm {
            q.push_back(*sample);
        }
        while q.len() >= chunk_samples {
            let mut chunk = Vec::with_capacity(chunk_samples);
            for _ in 0..chunk_samples {
                if let Some(v) = q.pop_front() {
                    chunk.push(v);
                }
            }
            let _ = tx.send(VoiceEvent::Captured(VoiceFrame {
                sample_rate,
                channels,
                samples: chunk,
            }));
        }
    }
}

pub fn stop_voice_session(session: VoiceSession) {
    let room = session.room.clone();
    let _ = session.event_tx.send(VoiceEvent::Stop);
    let _ = session.task.join();
    let _ = room;
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn is_voiced_frame(samples: &[i16]) -> bool {
    if samples.is_empty() {
        return false;
    }

    let sum_sq: f32 = samples
        .iter()
        .map(|sample| {
            let normalized = *sample as f32 / i16::MAX as f32;
            normalized * normalized
        })
        .sum();

    let rms = (sum_sq / samples.len() as f32).sqrt();
    rms >= SPEAKING_RMS_THRESHOLD
}

pub fn encode_voice_frame(frame: &VoiceFrame) -> String {
    let compressed = AudioProcessor::encode_pcm_rle(&frame.samples);

    let mut out = Vec::with_capacity(VOICE_FRAME_HEADER_LEN + compressed.len());
    out.extend_from_slice(&VOICE_FRAME_MAGIC);
    out.extend_from_slice(&frame.sample_rate.to_le_bytes());
    out.extend_from_slice(&frame.channels.to_le_bytes());
    out.extend_from_slice(&compressed);
    general_purpose::STANDARD.encode(out)
}

pub fn decode_voice_frame(payload_b64: &str) -> Option<VoiceFrame> {
    let raw = general_purpose::STANDARD.decode(payload_b64).ok()?;
    if raw.starts_with(&VOICE_FRAME_MAGIC) {
        return decode_voice_frame_v1(&raw);
    }

    decode_voice_frame_legacy_raw(&raw)
}

fn decode_voice_frame_v1(raw: &[u8]) -> Option<VoiceFrame> {
    if raw.len() < VOICE_FRAME_HEADER_LEN {
        return None;
    }
    let sample_rate = u32::from_le_bytes([raw[4], raw[5], raw[6], raw[7]]);
    let channels = u16::from_le_bytes([raw[8], raw[9]]);
    if !is_voice_header_supported(sample_rate, channels) {
        return None;
    }

    let samples = AudioProcessor::decode_pcm_rle_checked(
        &raw[VOICE_FRAME_HEADER_LEN..],
        MAX_VOICE_FRAME_SAMPLES,
    )?;

    Some(VoiceFrame {
        sample_rate,
        channels,
        samples,
    })
}

fn decode_voice_frame_legacy_raw(raw: &[u8]) -> Option<VoiceFrame> {
    if raw.len() < 6 {
        return None;
    }
    let sample_rate = u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]);
    let channels = u16::from_le_bytes([raw[4], raw[5]]);
    if !is_voice_header_supported(sample_rate, channels) {
        return None;
    }
    let samples_raw = &raw[6..];
    if !samples_raw.len().is_multiple_of(2) {
        return None;
    }
    let sample_count = samples_raw.len() / 2;
    if sample_count > MAX_VOICE_FRAME_SAMPLES {
        return None;
    }
    let mut samples = Vec::with_capacity(samples_raw.len() / 2);
    for chunk in samples_raw.chunks_exact(2) {
        samples.push(i16::from_le_bytes([chunk[0], chunk[1]]));
    }
    Some(VoiceFrame {
        sample_rate,
        channels,
        samples,
    })
}

fn is_voice_header_supported(sample_rate: u32, channels: u16) -> bool {
    if channels == 0 || channels > MAX_VOICE_CHANNELS {
        return false;
    }
    (MIN_VOICE_SAMPLE_RATE..=MAX_VOICE_SAMPLE_RATE).contains(&sample_rate)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_decode_voice_frame_roundtrip_v1() {
        let frame = VoiceFrame {
            sample_rate: 48_000,
            channels: 1,
            samples: vec![255, 255, 255, 255, -257, -257, 1024, -1024],
        };
        let encoded = encode_voice_frame(&frame);
        let decoded = decode_voice_frame(&encoded).expect("frame should decode");

        assert_eq!(decoded.sample_rate, frame.sample_rate);
        assert_eq!(decoded.channels, frame.channels);
        assert_eq!(decoded.samples, frame.samples);
    }

    #[test]
    fn decode_voice_frame_supports_legacy_raw_layout() {
        let sample_rate = 44_100u32;
        let channels = 2u16;
        let samples = vec![10i16, -10i16, 327, -327];

        let mut raw = Vec::new();
        raw.extend_from_slice(&sample_rate.to_le_bytes());
        raw.extend_from_slice(&channels.to_le_bytes());
        for sample in &samples {
            raw.extend_from_slice(&sample.to_le_bytes());
        }
        let payload = general_purpose::STANDARD.encode(raw);

        let decoded = decode_voice_frame(&payload).expect("legacy frame should decode");
        assert_eq!(decoded.sample_rate, sample_rate);
        assert_eq!(decoded.channels, channels);
        assert_eq!(decoded.samples, samples);
    }

    #[test]
    fn sample_format_conversions_clamp_and_center_correctly() {
        assert_eq!(u16_to_i16(0), -32768);
        assert_eq!(u16_to_i16(32768), 0);
        assert_eq!(u16_to_i16(u16::MAX), 32767);

        assert_eq!(f32_to_i16(-1.0), -32767);
        assert_eq!(f32_to_i16(0.0), 0);
        assert_eq!(f32_to_i16(1.0), 32767);
        assert_eq!(f32_to_i16(2.0), 32767);
        assert_eq!(f32_to_i16(-2.0), -32767);
    }

    #[test]
    fn decode_voice_frame_rejects_malformed_v1_rle_payload() {
        let mut raw = Vec::new();
        raw.extend_from_slice(&VOICE_FRAME_MAGIC);
        raw.extend_from_slice(&48_000u32.to_le_bytes());
        raw.extend_from_slice(&1u16.to_le_bytes());
        raw.push(1); // OP_RUN
        raw.extend_from_slice(&10u16.to_le_bytes());
        raw.push(0x34); // truncated i16 sample payload

        let payload = general_purpose::STANDARD.encode(raw);
        assert!(decode_voice_frame(&payload).is_none());
    }

    #[test]
    fn decode_voice_frame_rejects_v1_decompression_bomb() {
        let mut raw = Vec::new();
        raw.extend_from_slice(&VOICE_FRAME_MAGIC);
        raw.extend_from_slice(&48_000u32.to_le_bytes());
        raw.extend_from_slice(&1u16.to_le_bytes());
        raw.push(1); // OP_RUN
        raw.extend_from_slice(&(MAX_VOICE_FRAME_SAMPLES as u16 + 1).to_le_bytes());
        raw.extend_from_slice(&123i16.to_le_bytes());

        let payload = general_purpose::STANDARD.encode(raw);
        assert!(decode_voice_frame(&payload).is_none());
    }

    #[test]
    fn decode_voice_frame_rejects_unsupported_header_values() {
        let mut invalid_rate = Vec::new();
        invalid_rate.extend_from_slice(&VOICE_FRAME_MAGIC);
        invalid_rate.extend_from_slice(&1000u32.to_le_bytes());
        invalid_rate.extend_from_slice(&1u16.to_le_bytes());
        invalid_rate.push(0); // OP_LITERAL
        invalid_rate.extend_from_slice(&1u16.to_le_bytes());
        invalid_rate.extend_from_slice(&42i16.to_le_bytes());

        let payload = general_purpose::STANDARD.encode(invalid_rate);
        assert!(decode_voice_frame(&payload).is_none());

        let mut invalid_channels = Vec::new();
        invalid_channels.extend_from_slice(&VOICE_FRAME_MAGIC);
        invalid_channels.extend_from_slice(&48_000u32.to_le_bytes());
        invalid_channels.extend_from_slice(&99u16.to_le_bytes());
        invalid_channels.push(0); // OP_LITERAL
        invalid_channels.extend_from_slice(&1u16.to_le_bytes());
        invalid_channels.extend_from_slice(&42i16.to_le_bytes());

        let payload = general_purpose::STANDARD.encode(invalid_channels);
        assert!(decode_voice_frame(&payload).is_none());
    }
}

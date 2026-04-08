//! Voice session management for client.

use std::collections::VecDeque;
use std::sync::{mpsc as std_mpsc, Arc, Mutex};
use std::thread;

use base64::{engine::general_purpose, Engine as _};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, Stream, StreamConfig};
use rodio::buffer::SamplesBuffer;
use rodio::{OutputStream, Sink};

use clifford::error::{ChatifyError, ChatifyResult};
use clifford::voice::AudioProcessor;

pub struct VoiceFrame {
    pub sample_rate: u32,
    pub channels: u16,
    pub samples: Vec<i16>,
}

pub struct VoiceSession {
    pub room: String,
    pub event_tx: std_mpsc::Sender<VoiceEvent>,
    pub task: thread::JoinHandle<()>,
}

pub enum VoiceEvent {
    Captured(VoiceFrame),
    Playback(VoiceFrame),
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
            _ => {
                let _ = ready_tx.send(Err(ChatifyError::Audio(
                    "unsupported sample format".to_string(),
                )));
                return;
            }
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
                    let encoded = audio_processor.encode(&processed);
                    let payload = general_purpose::STANDARD.encode(&encoded);
                    let _ = ws_tx_task.send(
                        serde_json::json!({
                            "t": "vdata",
                            "r": room_task.clone(),
                            "a": payload
                        })
                        .to_string(),
                    );
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
                VoiceEvent::Stop => break,
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
                let converted: Vec<i16> = data
                    .iter()
                    .map(|v| (*v as i32 - 32768).clamp(i16::MIN as i32, i16::MAX as i32) as i16)
                    .collect();
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

pub fn encode_voice_frame(frame: &VoiceFrame) -> String {
    let mut out = Vec::with_capacity(6 + frame.samples.len() * 2);
    out.extend_from_slice(&frame.sample_rate.to_le_bytes());
    out.extend_from_slice(&frame.channels.to_le_bytes());
    for s in &frame.samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    general_purpose::STANDARD.encode(out)
}

pub fn decode_voice_frame(payload_b64: &str) -> Option<VoiceFrame> {
    let raw = general_purpose::STANDARD.decode(payload_b64).ok()?;
    if raw.len() < 6 {
        return None;
    }
    let sample_rate = u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]);
    let channels = u16::from_le_bytes([raw[4], raw[5]]);
    if channels == 0 {
        return None;
    }
    let compressed_data = &raw[6..];

    let mut samples = Vec::with_capacity(compressed_data.len() * 2);
    let mut i = 0;
    let mut is_compressed = false;

    while i < compressed_data.len() {
        if i + 1 < compressed_data.len()
            && compressed_data[i] == 0xFF
            && compressed_data[i + 1] == 0x00
        {
            is_compressed = true;
            if i + 5 < compressed_data.len() {
                let length =
                    u16::from_le_bytes([compressed_data[i + 2], compressed_data[i + 3]]) as usize;
                let sample = i16::from_le_bytes([compressed_data[i + 4], compressed_data[i + 5]]);
                for _ in 0..length {
                    samples.push(sample);
                }
                i += 6;
            } else {
                break;
            }
        } else if i + 1 < compressed_data.len() {
            samples.push(i16::from_le_bytes([
                compressed_data[i],
                compressed_data[i + 1],
            ]));
            i += 2;
        } else {
            break;
        }
    }

    if is_compressed {
        Some(VoiceFrame {
            sample_rate,
            channels,
            samples,
        })
    } else {
        decode_voice_frame_legacy(payload_b64)
    }
}

fn decode_voice_frame_legacy(payload_b64: &str) -> Option<VoiceFrame> {
    let raw = general_purpose::STANDARD.decode(payload_b64).ok()?;
    if raw.len() < 6 {
        return None;
    }
    let sample_rate = u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]);
    let channels = u16::from_le_bytes([raw[4], raw[5]]);
    if channels == 0 {
        return None;
    }
    let samples_raw = &raw[6..];
    if samples_raw.len() % 2 != 0 {
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

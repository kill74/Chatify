use serde::{Deserialize, Serialize};

pub mod capture;
pub mod encode;
pub mod webrtc;
pub mod display;

pub use capture::{CaptureManager, SourceInfo};
pub use encode::{Encoder, EncoderConfig, QualityPreset};
pub use webrtc::WebRTCManager;
pub use display::TerminalDisplay;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScreenShareOptions {
    pub source_id: Option<String>,
    pub quality: QualityPreset,
    pub audio: bool,
    pub audio_device: Option<String>,
}

impl Default for ScreenShareOptions {
    fn default() -> Self {
        Self {
            source_id: None,
            quality: QualityPreset::Medium,
            audio: false,
            audio_device: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ScreenShareState {
    #[default]
    Idle,
    Starting,
    Sharing,
    Stopping,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScreenShareSession {
    pub id: String,
    pub sharer: String,
    pub room: String,
    pub state: ScreenShareState,
    pub options: ScreenShareOptions,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub viewer_count: usize,
    pub started_at: Option<u64>,
}

impl ScreenShareSession {
    pub fn new(sharer: String, room: String, options: ScreenShareOptions) -> Self {
        let (width, height, fps) = options.quality.resolution();
        Self {
            id: format!("ss_{}", rand::random::<u64>()),
            sharer,
            room,
            state: ScreenShareState::Idle,
            options,
            width,
            height,
            fps,
            viewer_count: 0,
            started_at: None,
        }
    }
}

pub struct ScreenShareManager {
    session: Option<ScreenShareSession>,
    capture_manager: Option<capture::CaptureManager>,
    encoder: Option<encode::Encoder>,
    webrtc_manager: Option<webrtc::WebRTCManager>,
}

impl Default for ScreenShareManager {
    fn default() -> Self {
        Self::new()
    }
}

impl ScreenShareManager {
    pub fn new() -> Self {
        Self {
            session: None,
            capture_manager: None,
            encoder: None,
            webrtc_manager: None,
        }
    }

    pub fn state(&self) -> ScreenShareState {
        self.session.as_ref().map(|s| s.state).unwrap_or(ScreenShareState::Idle)
    }

    pub fn session(&self) -> Option<&ScreenShareSession> {
        self.session.as_ref()
    }

    pub async fn start(
        &mut self,
        sharer: String,
        room: String,
        options: ScreenShareOptions,
    ) -> Result<(), String> {
        if self.session.is_some() {
            return Err("Screen share already active".to_string());
        }

        let mut session = ScreenShareSession::new(sharer.clone(), room.clone(), options.clone());
        session.state = ScreenShareState::Starting;

        let mut capture_manager = capture::CaptureManager::new()
            .map_err(|e| format!("Failed to initialize capture: {}", e))?;

        let sources = capture_manager.list_sources()
            .map_err(|e| format!("Failed to list sources: {}", e))?;

        if sources.is_empty() {
            return Err("No capture sources available".to_string());
        }

        let source = if let Some(ref source_id) = options.source_id {
            sources.iter().find(|s| s.id == *source_id).cloned()
        } else {
            sources.into_iter().next()
        }.ok_or_else(|| "Selected source not found".to_string())?;

        capture_manager.start(&source.id)
            .map_err(|e| format!("Failed to start capture: {}", e))?;

        let encoder_config = EncoderConfig {
            width: session.width,
            height: session.height,
            fps: session.fps,
            bitrate: options.quality.bitrate(),
        };

        let encoder = encode::Encoder::new(encoder_config)
            .map_err(|e| format!("Failed to initialize encoder: {}", e))?;

        let webrtc_manager = webrtc::WebRTCManager::new();

        session.state = ScreenShareState::Sharing;
        session.started_at = Some(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0)
        );

        self.session = Some(session);
        self.capture_manager = Some(capture_manager);
        self.encoder = Some(encoder);
        self.webrtc_manager = Some(webrtc_manager);

        Ok(())
    }

    pub async fn stop(&mut self) -> Result<(), String> {
        if let Some(mut capture) = self.capture_manager.take() {
            capture.stop();
        }
        self.encoder = None;
        self.webrtc_manager = None;
        self.session = None;
        Ok(())
    }

    pub fn get_viewer_offer(&self) -> Option<String> {
        self.webrtc_manager.as_ref()?.get_local_offer()
    }

    pub async fn handle_viewer_offer(&mut self, offer: String) -> Result<String, String> {
        self.webrtc_manager.as_mut()
            .ok_or_else(|| "No active screen share session".to_string())?
            .handle_offer(offer)
            .await
    }

    pub async fn add_viewer_ice(&mut self, candidate: String, sdp_mid: Option<String>) -> Result<(), String> {
        self.webrtc_manager.as_mut()
            .ok_or_else(|| "No active screen share session".to_string())?
            .add_remote_ice(candidate, sdp_mid)
            .await
    }

    pub fn encode_frame(&mut self, rgba_data: &[u8], width: u32, height: u32) -> Result<Vec<u8>, String> {
        self.encoder.as_mut()
            .ok_or_else(|| "Encoder not initialized".to_string())?
            .encode_frame(rgba_data, width, height)
    }

    pub fn get_encoded_frame(&mut self) -> Option<Vec<u8>> {
        self.encoder.as_mut()?.get_encoded_frame()
    }
}

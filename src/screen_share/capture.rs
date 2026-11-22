use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceInfo {
    pub id: String,
    pub name: String,
    pub source_type: SourceType,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceType {
    Screen,
    Window,
}

pub struct CaptureManager {
    is_running: bool,
    frame_buffer: Vec<u8>,
    current_source: Option<String>,
}

impl CaptureManager {
    pub fn new() -> Result<Self, String> {
        Ok(Self {
            is_running: false,
            frame_buffer: Vec::new(),
            current_source: None,
        })
    }

    pub fn list_sources(&self) -> Result<Vec<SourceInfo>, String> {
        Ok(vec![
            SourceInfo {
                id: "screen:0".into(),
                name: "Primary Display".into(),
                source_type: SourceType::Screen,
                width: 1920,
                height: 1080,
            },
            SourceInfo {
                id: "screen:1".into(),
                name: "Display 2".into(),
                source_type: SourceType::Screen,
                width: 1920,
                height: 1080,
            },
            SourceInfo {
                id: "window:0".into(),
                name: "Browser".into(),
                source_type: SourceType::Window,
                width: 1280,
                height: 720,
            },
            SourceInfo {
                id: "window:1".into(),
                name: "Terminal".into(),
                source_type: SourceType::Window,
                width: 1024,
                height: 768,
            },
        ])
    }

    pub fn start(&mut self, source_id: &str) -> Result<(), String> {
        self.current_source = Some(source_id.to_string());
        self.is_running = true;
        Ok(())
    }

    pub fn capture_frame(&mut self, width: u32, height: u32) -> Result<Vec<u8>, String> {
        if !self.is_running {
            return Err("Capture not started".to_string());
        }
        // Compute in u64 and cap before converting to usize: `width * height`
        // overflows u32 for adversarial dimensions and would panic in debug
        // or wrap into a bogus (potentially huge) allocation in release.
        const MAX_FRAME_BYTES: u64 = 3840 * 2160 * 4; // 4K RGBA
        let frame_size = u64::from(width)
            .checked_mul(u64::from(height))
            .and_then(|pixels| pixels.checked_mul(4))
            .filter(|bytes| *bytes <= MAX_FRAME_BYTES)
            .ok_or_else(|| "frame dimensions exceed supported limit".to_string())?;
        let frame_size = usize::try_from(frame_size)
            .map_err(|_| "frame dimensions exceed supported limit".to_string())?;
        let buffer: Vec<u8> = (0..frame_size).map(|i| ((i / 4) % 256) as u8).collect();
        self.frame_buffer = buffer.clone();
        Ok(buffer)
    }

    pub fn stop(&mut self) {
        self.current_source = None;
        self.is_running = false;
        self.frame_buffer.clear();
    }

    pub fn is_running(&self) -> bool {
        self.is_running
    }
}

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
        let frame_size = (width * height * 4) as usize;
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

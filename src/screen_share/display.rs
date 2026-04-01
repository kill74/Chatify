use serde::{Deserialize, Serialize};
use std::io::Write;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub enum DisplayMode {
    #[default]
    Inline,
    Window,
    Save,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DisplayConfig {
    pub mode: DisplayMode,
    pub width: u32,
    pub height: u32,
}

impl Default for DisplayConfig {
    fn default() -> Self {
        Self {
            mode: DisplayMode::Inline,
            width: 1280,
            height: 720,
        }
    }
}

pub struct TerminalDisplay {
    config: DisplayConfig,
    frame_count: u64,
}

impl TerminalDisplay {
    pub fn new() -> Result<Self, String> {
        Ok(Self {
            config: DisplayConfig::default(),
            frame_count: 0,
        })
    }

    pub fn render_frame(&mut self, data: &[u8], width: u32, height: u32) -> Result<(), String> {
        self.frame_count += 1;

        match self.config.mode {
            DisplayMode::Inline => {
                let encoded =
                    base64::Engine::encode(&base64::engine::general_purpose::STANDARD, data);
                println!(
                    "\x1b]1337;File=inline=1;width={}px;height={}px:{}\x07",
                    width, height, encoded
                );
            }
            DisplayMode::Window | DisplayMode::Save => {
                let path = std::env::temp_dir().join(format!("screen_{}.bin", self.frame_count));
                std::fs::write(&path, data).map_err(|e| e.to_string())?;
                println!("\x1b]1337;File=file://{};\x07", path.display());
            }
        }

        std::io::stdout().flush().map_err(|e| e.to_string())
    }

    pub fn set_mode(&mut self, mode: DisplayMode) {
        self.config.mode = mode;
    }
}

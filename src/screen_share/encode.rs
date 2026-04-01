use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum QualityPreset {
    Low,
    #[default]
    Medium,
    High,
}

impl QualityPreset {
    pub fn resolution(&self) -> (u32, u32, u32) {
        match self {
            Self::Low => (854, 480, 15),
            Self::Medium => (1280, 720, 24),
            Self::High => (1920, 1080, 30),
        }
    }

    pub fn bitrate(&self) -> u32 {
        match self {
            Self::Low => 1_000_000,
            Self::Medium => 2_500_000,
            Self::High => 5_000_000,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EncoderConfig {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub bitrate: u32,
}

impl Default for EncoderConfig {
    fn default() -> Self {
        Self {
            width: 1280,
            height: 720,
            fps: 24,
            bitrate: 2_500_000,
        }
    }
}

impl EncoderConfig {
    pub fn new(quality: QualityPreset) -> Self {
        let (w, h, f) = quality.resolution();
        Self {
            width: w,
            height: h,
            fps: f,
            bitrate: quality.bitrate(),
        }
    }
}

pub struct Encoder {
    config: EncoderConfig,
    frame_count: u64,
}

impl Encoder {
    pub fn new(config: EncoderConfig) -> Result<Self, String> {
        log::info!(
            "Encoder: {}x{} @ {}fps",
            config.width,
            config.height,
            config.fps
        );
        Ok(Self {
            config,
            frame_count: 0,
        })
    }

    pub fn encode_frame(
        &mut self,
        data: &[u8],
        width: u32,
        height: u32,
    ) -> Result<Vec<u8>, String> {
        if width != self.config.width || height != self.config.height {
            return Err(format!(
                "{}x{} != {}x{}",
                width, height, self.config.width, self.config.height
            ));
        }
        self.frame_count += 1;
        Ok(data.to_vec())
    }

    pub fn get_encoded_frame(&mut self) -> Option<Vec<u8>> {
        None
    }
}

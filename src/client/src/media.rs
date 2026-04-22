//! Media timeline helpers for inline previews and safe terminal fallbacks.

use std::collections::BTreeMap;
use std::env;

use image::imageops::FilterType;

const MAX_INLINE_IMAGE_BYTES: u64 = 2 * 1024 * 1024;
const MAX_PREVIEW_WIDTH_CELLS: u32 = 16;
const MAX_PREVIEW_HEIGHT_ROWS: u32 = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaKind {
    Image,
    Audio,
    Video,
    File,
}

impl MediaKind {
    pub fn from_wire(value: Option<&str>) -> Self {
        match value.unwrap_or("file").trim().to_ascii_lowercase().as_str() {
            "image" => Self::Image,
            "audio" => Self::Audio,
            "video" => Self::Video,
            _ => Self::File,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Image => "image",
            Self::Audio => "audio note",
            Self::Video => "video",
            Self::File => "file",
        }
    }

    pub fn wire_name(self) -> &'static str {
        match self {
            Self::Image => "image",
            Self::Audio => "audio",
            Self::Video => "video",
            Self::File => "file",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RgbColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StyledFragment {
    pub text: String,
    pub fg: Option<RgbColor>,
    pub bg: Option<RgbColor>,
    pub bold: bool,
    pub dim: bool,
}

pub type StyledLine = Vec<StyledFragment>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaRenderStatus {
    MetadataOnly,
    Pending,
    Complete,
    Disabled,
    Unsupported,
    TooLarge,
    DecodeFailed,
}

#[derive(Clone, Debug)]
pub struct TimelineMedia {
    pub file_id: String,
    pub filename: String,
    pub media_kind: MediaKind,
    pub mime: Option<String>,
    pub size: u64,
    pub duration_ms: Option<u64>,
    pub received_bytes: u64,
    pub local_path: Option<String>,
    pub preview: Vec<StyledLine>,
    pub render_status: MediaRenderStatus,
}

impl TimelineMedia {
    pub fn summary_line(&self) -> String {
        let mut parts = vec![format_byte_size(self.size)];
        if let Some(duration_ms) = self.duration_ms {
            parts.push(format_duration(duration_ms));
        }
        if let Some(mime) = self.mime.as_deref() {
            parts.push(mime.to_string());
        }

        format!(
            "[{}] {} ({})",
            self.media_kind.label(),
            self.filename,
            parts.join(", ")
        )
    }

    pub fn progress_label(&self) -> Option<String> {
        if self.size == 0 || self.received_bytes >= self.size {
            return None;
        }

        let percent = ((self.received_bytes as f64 / self.size as f64) * 100.0)
            .clamp(0.0, 100.0)
            .round() as u8;
        Some(format!(
            "receiving {} of {} ({}%)",
            format_byte_size(self.received_bytes),
            format_byte_size(self.size),
            percent
        ))
    }
}

#[derive(Clone, Debug)]
pub enum TimelinePayload {
    Media(TimelineMedia),
}

#[derive(Clone, Debug)]
pub struct PendingMediaTransfer {
    pub timeline_id: String,
    pub scope: String,
    pub sender: String,
    pub announced_at: f64,
    pub media: TimelineMedia,
    chunks: BTreeMap<u64, Vec<u8>>,
}

impl PendingMediaTransfer {
    pub fn new(
        timeline_id: String,
        scope: String,
        sender: String,
        announced_at: f64,
        media: TimelineMedia,
    ) -> Self {
        Self {
            timeline_id,
            scope,
            sender,
            announced_at,
            media,
            chunks: BTreeMap::new(),
        }
    }

    pub fn insert_chunk(&mut self, index: u64, bytes: Vec<u8>) {
        if let Some(previous) = self.chunks.insert(index, bytes) {
            self.media.received_bytes = self
                .media
                .received_bytes
                .saturating_sub(previous.len() as u64);
        }

        if let Some(chunk) = self.chunks.get(&index) {
            self.media.received_bytes =
                self.media.received_bytes.saturating_add(chunk.len() as u64);
        }

        if self.media.render_status != MediaRenderStatus::Disabled {
            self.media.render_status = MediaRenderStatus::Pending;
        }
    }

    pub fn is_complete(&self) -> bool {
        self.media.size > 0 && self.media.received_bytes >= self.media.size
    }

    pub fn into_bytes(self) -> Vec<u8> {
        let mut combined = Vec::new();
        for chunk in self.chunks.into_values() {
            combined.extend(chunk);
        }
        combined
    }
}

pub fn media_timeline_id(scope: &str, file_id: &str) -> String {
    format!("media:{}:{}", scope, file_id)
}

pub fn terminal_supports_inline_media() -> bool {
    if env::var_os("CHATIFY_FORCE_INLINE_MEDIA").is_some() {
        return true;
    }

    if env::var_os("NO_COLOR").is_some() || env::var("CLICOLOR").ok().as_deref() == Some("0") {
        return false;
    }

    let term = env::var("TERM")
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    if term.is_empty() || term == "dumb" {
        return false;
    }

    if cfg!(windows)
        && (env::var_os("WT_SESSION").is_some()
            || env::var_os("ANSICON").is_some()
            || env::var("ConEmuANSI")
                .map(|value| value.eq_ignore_ascii_case("on"))
                .unwrap_or(false)
            || term.contains("xterm")
            || term.contains("msys")
            || term.contains("cygwin"))
    {
        return true;
    }

    env::var_os("COLORTERM").is_some()
        || term.contains("256color")
        || term.contains("kitty")
        || term.contains("wezterm")
        || term.contains("tmux")
        || term.contains("screen")
        || term.contains("xterm")
        || term.contains("rxvt")
        || term.contains("linux")
}

pub fn guess_mime_from_path(path: &std::path::Path, media_kind: MediaKind) -> Option<String> {
    let extension = path.extension()?.to_str()?.trim().to_ascii_lowercase();
    let mime = match extension.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "bmp" => "image/bmp",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "mp4" => "video/mp4",
        "mov" => "video/quicktime",
        "mkv" => "video/x-matroska",
        "webm" => "video/webm",
        "avi" => "video/x-msvideo",
        "ogg" | "oga" => "audio/ogg",
        "opus" => "audio/ogg",
        "wav" => "audio/wav",
        "mp3" => "audio/mpeg",
        "m4a" => "audio/mp4",
        "flac" => "audio/flac",
        _ => {
            return match media_kind {
                MediaKind::Image => Some("image/*".to_string()),
                MediaKind::Audio => Some("audio/*".to_string()),
                MediaKind::Video => Some("video/*".to_string()),
                MediaKind::File => None,
            }
        }
    };

    Some(mime.to_string())
}

pub fn render_message_lines(
    content: &str,
    payload: Option<&TimelinePayload>,
    media_enabled: bool,
) -> Vec<StyledLine> {
    let mut lines = vec![plain_line(content.to_string())];
    if let Some(TimelinePayload::Media(media)) = payload {
        lines.extend(render_media_detail_lines(media, media_enabled));
    }
    lines
}

pub fn render_message_plain_lines(
    content: &str,
    payload: Option<&TimelinePayload>,
    media_enabled: bool,
) -> Vec<String> {
    render_message_lines(content, payload, media_enabled)
        .iter()
        .map(styled_line_to_ansi)
        .collect()
}

pub fn build_inline_image_preview(bytes: &[u8]) -> Result<Vec<StyledLine>, MediaRenderStatus> {
    if bytes.len() as u64 > MAX_INLINE_IMAGE_BYTES {
        return Err(MediaRenderStatus::TooLarge);
    }

    if !terminal_supports_inline_media() {
        return Err(MediaRenderStatus::Unsupported);
    }

    let image = image::load_from_memory(bytes).map_err(|_| MediaRenderStatus::DecodeFailed)?;
    let rgba = image.to_rgba8();
    let (width, height) = rgba.dimensions();
    if width == 0 || height == 0 {
        return Err(MediaRenderStatus::DecodeFailed);
    }

    let max_height_pixels = MAX_PREVIEW_HEIGHT_ROWS * 2;
    let scale = f32::min(
        MAX_PREVIEW_WIDTH_CELLS as f32 / width as f32,
        max_height_pixels as f32 / height as f32,
    )
    .min(1.0);
    let preview_width = ((width as f32 * scale).round().max(1.0)) as u32;
    let preview_height = ((height as f32 * scale).round().max(1.0)) as u32;

    let resized =
        image::imageops::resize(&rgba, preview_width, preview_height, FilterType::Triangle);
    let mut lines = Vec::new();
    let mut y = 0u32;
    while y < preview_height {
        let mut line = Vec::new();
        for x in 0..preview_width {
            let top = composite_pixel(*resized.get_pixel(x, y));
            let bottom = if y + 1 < preview_height {
                composite_pixel(*resized.get_pixel(x, y + 1))
            } else {
                RgbColor { r: 0, g: 0, b: 0 }
            };
            line.push(StyledFragment {
                text: "▀".to_string(),
                fg: Some(top),
                bg: Some(bottom),
                bold: false,
                dim: false,
            });
        }
        lines.push(line);
        y += 2;
    }

    Ok(lines)
}

pub fn format_byte_size(size: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = 1024.0 * 1024.0;
    const GIB: f64 = 1024.0 * 1024.0 * 1024.0;

    let size_f = size as f64;
    if size_f >= GIB {
        format!("{:.2} GiB", size_f / GIB)
    } else if size_f >= MIB {
        format!("{:.2} MiB", size_f / MIB)
    } else if size_f >= KIB {
        format!("{:.2} KiB", size_f / KIB)
    } else {
        format!("{} B", size)
    }
}

pub fn format_duration(duration_ms: u64) -> String {
    let total_seconds = duration_ms / 1000;
    let hours = total_seconds / 3600;
    let minutes = (total_seconds % 3600) / 60;
    let seconds = total_seconds % 60;
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

fn render_media_detail_lines(media: &TimelineMedia, media_enabled: bool) -> Vec<StyledLine> {
    let mut lines = Vec::new();

    if !media_enabled || media.render_status == MediaRenderStatus::Disabled {
        lines.push(dim_line("media rendering disabled by client settings"));
        return lines;
    }

    match media.media_kind {
        MediaKind::Image => {
            if !media.preview.is_empty() {
                lines.extend(media.preview.clone());
            } else {
                match media.render_status {
                    MediaRenderStatus::Pending => {
                        if let Some(progress) = media.progress_label() {
                            lines.push(dim_line(progress));
                        }
                    }
                    MediaRenderStatus::Unsupported => {
                        lines.push(dim_line("inline preview unavailable in this terminal"));
                    }
                    MediaRenderStatus::TooLarge => {
                        lines.push(dim_line(format!(
                            "inline preview skipped above {}",
                            format_byte_size(MAX_INLINE_IMAGE_BYTES)
                        )));
                    }
                    MediaRenderStatus::DecodeFailed => {
                        lines.push(dim_line("inline preview unavailable for this image"));
                    }
                    MediaRenderStatus::MetadataOnly
                    | MediaRenderStatus::Complete
                    | MediaRenderStatus::Disabled => {}
                }
            }
        }
        MediaKind::Audio | MediaKind::Video | MediaKind::File => {
            if let Some(progress) = media.progress_label() {
                lines.push(dim_line(progress));
            }
        }
    }

    if let Some(path) = media.local_path.as_deref() {
        lines.push(dim_line(format!("saved: {}", path)));
    }

    lines
}

fn styled_line_to_ansi(line: &StyledLine) -> String {
    let mut rendered = String::new();
    for fragment in line {
        rendered.push_str("\x1b[0m");
        if fragment.bold {
            rendered.push_str("\x1b[1m");
        }
        if fragment.dim {
            rendered.push_str("\x1b[2m");
        }
        if let Some(color) = fragment.fg {
            rendered.push_str(&format!("\x1b[38;2;{};{};{}m", color.r, color.g, color.b));
        }
        if let Some(color) = fragment.bg {
            rendered.push_str(&format!("\x1b[48;2;{};{};{}m", color.r, color.g, color.b));
        }
        rendered.push_str(&fragment.text);
    }
    rendered.push_str("\x1b[0m");
    rendered
}

fn plain_line(text: String) -> StyledLine {
    vec![StyledFragment {
        text,
        fg: None,
        bg: None,
        bold: false,
        dim: false,
    }]
}

fn dim_line(text: impl Into<String>) -> StyledLine {
    vec![StyledFragment {
        text: text.into(),
        fg: None,
        bg: None,
        bold: false,
        dim: true,
    }]
}

fn composite_pixel(pixel: image::Rgba<u8>) -> RgbColor {
    let alpha = pixel[3] as f32 / 255.0;
    RgbColor {
        r: (pixel[0] as f32 * alpha).round() as u8,
        g: (pixel[1] as f32 * alpha).round() as u8,
        b: (pixel[2] as f32 * alpha).round() as u8,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        build_inline_image_preview, format_byte_size, format_duration, render_message_plain_lines,
        terminal_supports_inline_media, MediaKind, MediaRenderStatus, StyledFragment,
        TimelineMedia, TimelinePayload,
    };

    #[test]
    fn format_byte_size_uses_human_units() {
        assert_eq!(format_byte_size(512), "512 B");
        assert_eq!(format_byte_size(2048), "2.00 KiB");
    }

    #[test]
    fn format_duration_uses_compact_clock_format() {
        assert_eq!(format_duration(42_000), "0:42");
        assert_eq!(format_duration(3_723_000), "1:02:03");
    }

    #[test]
    fn render_message_plain_lines_reports_disabled_media_predictably() {
        let payload = TimelinePayload::Media(TimelineMedia {
            file_id: "file-1".to_string(),
            filename: "voice-note.ogg".to_string(),
            media_kind: MediaKind::Audio,
            mime: Some("audio/ogg".to_string()),
            size: 11,
            duration_ms: Some(5_000),
            received_bytes: 0,
            local_path: None,
            preview: Vec::new(),
            render_status: MediaRenderStatus::Disabled,
        });

        let lines =
            render_message_plain_lines("[audio note] voice-note.ogg", Some(&payload), false);
        assert_eq!(lines.len(), 2);
        assert!(lines[1].contains("disabled"));
    }

    #[test]
    fn tiny_png_generates_preview_when_forced() {
        std::env::set_var("CHATIFY_FORCE_INLINE_MEDIA", "1");
        let mut cursor = std::io::Cursor::new(Vec::new());
        let image = image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            1,
            1,
            image::Rgba([255, 0, 0, 255]),
        ));
        image
            .write_to(&mut cursor, image::ImageOutputFormat::Png)
            .expect("encode 1x1 png");
        let png = cursor.into_inner();

        let preview = build_inline_image_preview(&png).expect("build inline preview");
        assert!(!preview.is_empty());

        std::env::remove_var("CHATIFY_FORCE_INLINE_MEDIA");
        let _ = terminal_supports_inline_media();
    }

    #[test]
    fn oversized_image_skips_preview() {
        let preview = build_inline_image_preview(&vec![0u8; (2 * 1024 * 1024 + 1) as usize]);
        assert_eq!(preview.err(), Some(MediaRenderStatus::TooLarge));
    }

    #[test]
    fn plain_line_output_keeps_ansi_reset() {
        let payload = TimelinePayload::Media(TimelineMedia {
            file_id: "img-1".to_string(),
            filename: "preview.png".to_string(),
            media_kind: MediaKind::Image,
            mime: Some("image/png".to_string()),
            size: 64,
            duration_ms: None,
            received_bytes: 64,
            local_path: None,
            preview: vec![vec![StyledFragment {
                text: "▀".to_string(),
                fg: Some(super::RgbColor { r: 255, g: 0, b: 0 }),
                bg: Some(super::RgbColor { r: 0, g: 0, b: 0 }),
                bold: false,
                dim: false,
            }]],
            render_status: MediaRenderStatus::Complete,
        });
        let lines = render_message_plain_lines("[image] preview.png", Some(&payload), true);
        assert!(lines[1].contains("\u{1b}[38;2;255;0;0m"));
        assert!(lines[1].ends_with("\u{1b}[0m"));
    }
}

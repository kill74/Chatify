//! Server CLI arguments and protocol constants.

use clap::{Parser, ValueEnum};

#[derive(Copy, Clone, Debug, Eq, PartialEq, ValueEnum)]
pub enum DbDurabilityMode {
    Balanced,
    MaxSafety,
}

#[derive(Parser)]
#[command(name = "clifford-server")]
pub struct Args {
    #[arg(long, default_value = "0.0.0.0")]
    pub host: String,

    #[arg(long, default_value_t = 8765)]
    pub port: u16,

    #[arg(long)]
    pub log: bool,

    #[arg(long, default_value = "chatify.db")]
    pub db: String,

    #[arg(long, value_enum, default_value_t = DbDurabilityMode::MaxSafety)]
    pub db_durability: DbDurabilityMode,

    #[arg(long)]
    pub db_key: Option<String>,

    #[arg(long)]
    pub tls: bool,

    #[arg(long, default_value = "cert.pem")]
    pub tls_cert: String,

    #[arg(long, default_value = "key.pem")]
    pub tls_key: String,

    #[arg(long, default_value_t = 8080)]
    pub health_port: u16,

    #[arg(long, default_value_t = true)]
    pub metrics_enabled: bool,

    #[arg(long, default_value_t = 30)]
    pub shutdown_timeout_secs: u64,

    #[arg(long)]
    pub enable_hot_reload: bool,

    #[arg(long)]
    pub shutdown_endpoint: bool,

    #[arg(long, default_value_t = 60)]
    pub max_msgs_per_minute: u32,

    #[arg(long)]
    pub enable_user_rate_limit: bool,

    #[arg(long, default_value_t = false)]
    pub enable_self_registration: bool,
}

// Protocol constants
pub const HISTORY_CAP: usize = 50;
pub const MAX_BYTES: usize = 16_000;
pub const MAX_AUTH_BYTES: usize = 4_096;
pub const MAX_HANDSHAKE_HEADER_SIZE: usize = 8192;
pub const MAX_HANDSHAKE_HEADERS: usize = 64;
pub const CURRENT_SCHEMA_VERSION: i64 = 6;
pub const DEFAULT_HISTORY_LIMIT: usize = 50;
pub const DEFAULT_SEARCH_LIMIT: usize = 30;
pub const DEFAULT_REWIND_SECONDS: u64 = 3600;
pub const DEFAULT_REWIND_LIMIT: usize = 100;
pub const DM_CHANNEL_PREFIX: &str = "__dm__";
pub const PROTOCOL_VERSION: u64 = 1;
pub const MAX_USERNAME_LEN: usize = 32;
pub const MAX_PASSWORD_FIELD_LEN: usize = 256;
pub const MAX_PUBLIC_KEY_FIELD_LEN: usize = 256;
pub const MAX_CLOCK_SKEW_SECS: f64 = 300.0;
pub const MAX_NONCE_LEN: usize = 64;
pub const NONCE_CACHE_CAP: usize = 256;
pub const NONCE_CLEANUP_INTERVAL_SECS: u64 = 60;
pub const NONCE_MAX_AGE_SECS: f64 = MAX_CLOCK_SKEW_SECS * 2.0;
pub const MAX_CONNECTIONS_PER_IP: usize = 5;
pub const AUTH_RATE_LIMIT_SECS: f64 = 0.5;
pub const MAX_FILE_SIZE: u64 = 100 * 1024 * 1024;
pub const MAX_FILE_NAME_LEN: usize = 256;
pub const MAX_FILE_ID_LEN: usize = 128;
pub const MAX_MEDIA_MIME_LEN: usize = 128;
pub const MAX_STATUS_TEXT_LEN: usize = 128;
pub const MAX_STATUS_EMOJI_LEN: usize = 16;
pub const CHANNEL_BUFFER_SIZE: usize = 1000;
pub const CHANNEL_HISTORY_LIMIT: usize = 1000;
pub const MAX_MSG_ID_LEN: usize = 64;

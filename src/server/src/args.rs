//! Command-line arguments for Chatify server.

use clap::{Parser, ValueEnum};

/// SQLite durability profile.
#[derive(Copy, Clone, Debug, Eq, PartialEq, ValueEnum)]
pub enum DbDurabilityMode {
    /// Throughput-friendly durability profile.
    /// Uses WAL and `synchronous=NORMAL`.
    Balanced,
    /// Crash-resilient durability profile.
    /// Uses WAL and `synchronous=FULL`.
    MaxSafety,
}

impl DbDurabilityMode {
    /// Returns the SQLite PRAGMA statements for this mode.
    pub fn db_pragmas(self) -> &'static str {
        match self {
            DbDurabilityMode::Balanced => {
                "
                PRAGMA journal_mode = WAL;
                PRAGMA synchronous = NORMAL;
                PRAGMA wal_autocheckpoint = 4096;
                PRAGMA cache_size = -16384;
                PRAGMA temp_store = MEMORY;
                PRAGMA foreign_keys = ON;
                PRAGMA mmap_size = 1073741824;
                PRAGMA page_size = 4096;
                PRAGMA journal_size_limit = 1073741824;
                "
            }
            DbDurabilityMode::MaxSafety => {
                "
                PRAGMA journal_mode = WAL;
                PRAGMA synchronous = FULL;
                PRAGMA wal_autocheckpoint = 1024;
                PRAGMA cache_size = -8192;
                PRAGMA temp_store = MEMORY;
                PRAGMA foreign_keys = ON;
                PRAGMA mmap_size = 536870912;
                PRAGMA page_size = 4096;
                PRAGMA journal_size_limit = 536870912;
                "
            }
        }
    }

    /// Returns a human-readable label for this mode.
    pub fn label(self) -> &'static str {
        match self {
            DbDurabilityMode::Balanced => "balanced (WAL + synchronous=NORMAL)",
            DbDurabilityMode::MaxSafety => "max-safety (WAL + synchronous=FULL)",
        }
    }

    /// Returns the WAL checkpoint PRAGMA for this mode.
    pub fn startup_checkpoint_pragma(self) -> &'static str {
        match self {
            DbDurabilityMode::Balanced => "PRAGMA wal_checkpoint(PASSIVE);",
            DbDurabilityMode::MaxSafety => "PRAGMA wal_checkpoint(FULL);",
        }
    }
}

/// Command-line arguments for `chatify-server`.
#[derive(Parser)]
#[command(name = "chatify-server")]
pub struct Args {
    /// IP address the server will bind to.
    #[arg(long, default_value = "0.0.0.0")]
    pub host: String,

    /// TCP port to listen on.
    #[arg(long, default_value_t = 8765)]
    pub port: u16,

    /// Enable structured logging via `env_logger`.
    #[arg(long)]
    pub log: bool,

    /// Path to the SQLite database file.
    #[arg(long, default_value = "chatify.db")]
    pub db: String,

    /// SQLite durability profile.
    #[arg(long, value_enum, default_value_t = DbDurabilityMode::MaxSafety)]
    pub db_durability: DbDurabilityMode,

    /// Hex-encoded encryption key for the SQLite database.
    #[arg(long)]
    pub db_key: Option<String>,

    /// Enable TLS for WebSocket connections (wss://).
    #[arg(long)]
    pub tls: bool,

    /// Path to the TLS certificate file.
    #[arg(long, default_value = "cert.pem")]
    pub tls_cert: String,

    /// Path to the TLS private key file.
    #[arg(long, default_value = "key.pem")]
    pub tls_key: String,

    /// TCP port for health check and metrics endpoints.
    #[arg(long, default_value_t = 8080)]
    pub health_port: u16,

    /// Enable Prometheus metrics endpoint.
    #[arg(long, default_value_t = true)]
    pub metrics_enabled: bool,

    /// Maximum time in seconds to wait for connections to drain during shutdown.
    #[arg(long, default_value_t = 30)]
    pub shutdown_timeout_secs: u64,

    /// Enable handling SIGHUP for config hot-reload.
    #[arg(long)]
    pub enable_hot_reload: bool,

    /// Enable shutdown HTTP endpoint for orchestration.
    #[arg(long)]
    pub shutdown_endpoint: bool,

    /// Maximum messages per user per minute (0 = unlimited).
    #[arg(long, default_value_t = 60)]
    pub max_msgs_per_minute: u32,

    /// Enable per-user rate limiting.
    #[arg(long)]
    pub enable_user_rate_limit: bool,

    /// Enable self-registration via WebSocket.
    #[arg(long)]
    pub enable_self_registration: bool,

    /// Database connection pool size (0 falls back to the server default).
    #[arg(long, default_value_t = 8)]
    pub db_pool_size: u32,

    /// Per-connection outbound queue capacity (0 falls back to default).
    #[arg(long, default_value_t = 1024)]
    pub outbound_queue_capacity: usize,

    /// Consecutive dropped outbound messages before disconnecting a slow client.
    #[arg(long, default_value_t = 64)]
    pub slow_client_drop_burst: usize,

    /// Retention window in days for persisted media.
    #[arg(long, default_value_t = 30)]
    pub media_retention_days: u32,

    /// Maximum retained media payload volume in GiB before oldest completed transfers are pruned.
    #[arg(long, default_value_t = 20.0)]
    pub media_max_total_size_gb: f64,

    /// Optional directory for spilling large media chunks out of SQLite.
    /// Defaults to `<db>.media` for file-backed databases.
    #[arg(long)]
    pub media_spill_dir: Option<String>,

    /// File/chunk size threshold in KiB before media is stored in the spill directory.
    #[arg(long, default_value_t = 1024)]
    pub media_spill_threshold_kib: u64,

    /// Interval in seconds between periodic media retention maintenance runs.
    #[arg(long, default_value_t = 600)]
    pub media_prune_interval_secs: u64,

    /// Disable periodic media retention pruning.
    #[arg(long)]
    pub disable_media_retention: bool,

    /// Register a new user (admin operation).
    #[arg(long, requires = "user_password")]
    pub register_user: Option<String>,

    /// Password for --register-user operation.
    #[arg(long)]
    pub user_password: Option<String>,

    /// Make the registered user an admin.
    #[arg(long)]
    pub make_admin: bool,

    /// Enable 2FA for a user (admin operation).
    #[arg(long)]
    pub enable_2fa_for: Option<String>,

    /// Disable 2FA for a user (admin operation).
    #[arg(long)]
    pub disable_2fa_for: Option<String>,

    /// Run a full SQLite integrity check and exit (admin operation).
    #[arg(long)]
    pub db_integrity_check: bool,

    /// Create a consistent SQLite backup snapshot at the target path and exit (admin operation).
    #[arg(long)]
    pub db_backup_to: Option<String>,

    /// Restore a SQLite backup snapshot from the given path into `--db` and exit (admin operation).
    #[arg(long)]
    pub db_restore_from: Option<String>,

    /// Internal mode: run a built-in plugin worker process.
    #[arg(long, hide = true)]
    pub chatify_plugin_worker: Option<String>,

    /// Internal mode: plugin operation.
    #[arg(long, hide = true, default_value = "manifest")]
    pub chatify_plugin_op: String,

    /// Internal mode: slash command identifier.
    #[arg(long, hide = true)]
    pub chatify_plugin_command: Option<String>,
}

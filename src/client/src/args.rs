//! Client CLI arguments.

use clap::Parser;

#[derive(Parser, Debug)]
#[command(name = "chatify-client")]
#[command(version = "1.0")]
#[command(about = "WebSocket chat client with encryption")]
pub struct Args {
    #[arg(long)]
    pub host: Option<String>,

    #[arg(long)]
    pub port: Option<u16>,

    #[arg(long)]
    pub tls: bool,

    #[arg(long)]
    pub log: bool,

    #[arg(long)]
    pub no_markdown: bool,

    #[arg(long)]
    pub no_media: bool,

    #[arg(long)]
    pub no_animation: bool,

    #[arg(long)]
    pub no_reconnect: bool,
}

impl Args {
    pub fn merge_with_config(&self, config: &chatify::config::Config) -> ClientConfig {
        let host = self
            .host
            .as_deref()
            .unwrap_or(&config.connection.default_host)
            .to_string();
        let port = self.port.unwrap_or(config.connection.default_port);
        let tls = self.tls || config.connection.use_tls;
        let auto_reconnect = !self.no_reconnect && config.connection.auto_reconnect;
        let markdown_enabled = !self.no_markdown && config.ui.enable_markdown;
        let media_enabled = !self.no_media && config.ui.enable_media;
        let animations_enabled = !self.no_animation && !config.ui.disable_animations;

        ClientConfig {
            host: host.to_string(),
            port,
            tls,
            auto_reconnect,
            log_enabled: self.log,
            markdown_enabled,
            media_enabled,
            animations_enabled,
        }
    }
}

#[derive(Clone)]
pub struct ClientConfig {
    pub host: String,
    pub port: u16,
    pub tls: bool,
    pub auto_reconnect: bool,
    pub log_enabled: bool,
    pub markdown_enabled: bool,
    pub media_enabled: bool,
    pub animations_enabled: bool,
}

impl ClientConfig {
    pub fn uri(&self) -> String {
        let scheme = if self.tls { "wss" } else { "ws" };
        format!("{}://{}:{}", scheme, self.host, self.port)
    }
}

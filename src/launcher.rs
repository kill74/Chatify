use std::env;
use std::ffi::OsStr;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use chatify::config::{Config, ServerProfileConfig};

const DEFAULT_HOST: &str = "127.0.0.1";
const LAN_HOST: &str = "0.0.0.0";
const DEFAULT_PORT: u16 = 8765;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LauncherMode {
    StartHere,
    HostForOthers,
    JoinServer,
    ServerOnly,
}

impl LauncherMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::StartHere => "start_here",
            Self::HostForOthers => "host_for_others",
            Self::JoinServer => "join_server",
            Self::ServerOnly => "server_only",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::StartHere => "Start here",
            Self::HostForOthers => "Host for others",
            Self::JoinServer => "Join a server",
            Self::ServerOnly => "Server only",
        }
    }

    fn from_str(value: &str) -> Option<Self> {
        match value {
            "start_here" => Some(Self::StartHere),
            "host_for_others" => Some(Self::HostForOthers),
            "join_server" => Some(Self::JoinServer),
            "server_only" => Some(Self::ServerOnly),
            _ => None,
        }
    }
}

fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("Chatify launcher error: {err}");
            pause_before_exit();
            std::process::ExitCode::FAILURE
        }
    }
}

fn run() -> io::Result<()> {
    let app_dir = resolve_app_dir()?;
    let server_exe = binary_path(&app_dir, "chatify-server");
    let client_exe = binary_path(&app_dir, "chatify-client");
    let mut config = Config::load();
    ensure_launcher_defaults(&mut config)?;

    if !config.launcher.first_run_complete {
        run_first_setup(&mut config)?;
    }

    loop {
        print_menu(&app_dir, &config);
        let choice = prompt("Choose", reconnect_prompt_default(&config))?;
        let choice = normalize_menu_choice(&choice, &config);

        match choice.as_str() {
            "r" => launch_last_mode(&server_exe, &client_exe, &mut config)?,
            "1" => quick_start_local(&server_exe, &client_exe, &mut config)?,
            "2" => host_for_lan(&server_exe, &client_exe, &mut config)?,
            "3" => join_server(&client_exe, &mut config)?,
            "4" => start_server_only(&server_exe, &mut config)?,
            "5" => manage_profiles(&mut config)?,
            "6" => open_readme(&app_dir)?,
            "q" | "quit" | "exit" => return Ok(()),
            _ => {
                println!("Please choose 1, 2, 3, 4, 5, 6, R, or Q.");
                pause_before_exit();
            }
        }
    }
}

fn run_first_setup(config: &mut Config) -> io::Result<()> {
    println!();
    println!("Welcome to Chatify.");
    println!("Let's save your usual launch choice so next time Enter reconnects.");
    println!();
    println!("1) Start here - local server + local client");
    println!("2) Host for others - LAN server + local client");
    println!("3) Join a server - client only");
    println!("4) Server only");
    println!();

    let mode = loop {
        let choice = prompt("Best default mode", "1")?;
        match choice.trim() {
            "" | "1" => break LauncherMode::StartHere,
            "2" => break LauncherMode::HostForOthers,
            "3" => break LauncherMode::JoinServer,
            "4" => break LauncherMode::ServerOnly,
            _ => println!("Choose 1, 2, 3, or 4."),
        }
    };

    let (host, port) = match mode {
        LauncherMode::StartHere => (DEFAULT_HOST.to_string(), prompt_port(DEFAULT_PORT)?),
        LauncherMode::HostForOthers => (LAN_HOST.to_string(), prompt_port(DEFAULT_PORT)?),
        LauncherMode::JoinServer => {
            let profile = prompt_profile(DEFAULT_HOST, DEFAULT_PORT)?;
            upsert_profile(&mut config.launcher.profiles, profile.clone());
            (profile.host, profile.port)
        }
        LauncherMode::ServerOnly => (prompt_host(LAN_HOST)?, prompt_port(DEFAULT_PORT)?),
    };

    record_last_launch(config, mode, &host, port)?;
    config.launcher.first_run_complete = true;
    save_config(config)?;

    println!();
    println!("Saved. Next time, press Enter in the launcher to use this setup.");
    pause_before_exit();
    Ok(())
}

fn print_menu(app_dir: &Path, config: &Config) {
    println!();
    println!("==============================");
    println!("         Chatify");
    println!("==============================");
    println!("Open one thing, then choose how you want to chat.");
    println!();
    if let Some(mode) = LauncherMode::from_str(&config.launcher.last_mode) {
        println!(
            "R) Reconnect: {} at {}:{}",
            mode.label(),
            config.launcher.last_host,
            config.launcher.last_port
        );
        println!();
    }
    println!("1) Start here");
    println!("   Server + client on this PC. Best first click.");
    println!("2) Host for others");
    println!("   Server + client, reachable from your LAN.");
    println!("3) Join a server");
    println!("   Choose a saved server or enter a new host/IP.");
    println!("4) Server only");
    println!("   Useful for a small always-on host machine.");
    println!("5) Manage servers");
    println!("   Add or remove saved server profiles.");
    println!("6) Open README");
    println!("Q) Quit");
    println!();
    println!("App folder: {}", app_dir.display());
    println!("Data folder: {}", chatify_data_dir().display());
    println!("Config: {}", config_path_label());
    println!();
}

fn reconnect_prompt_default(config: &Config) -> &'static str {
    if LauncherMode::from_str(&config.launcher.last_mode).is_some() {
        "R"
    } else {
        ""
    }
}

fn normalize_menu_choice(raw: &str, config: &Config) -> String {
    let choice = raw.trim().to_ascii_lowercase();
    if choice.is_empty() && LauncherMode::from_str(&config.launcher.last_mode).is_some() {
        "r".to_string()
    } else {
        choice
    }
}

fn launch_last_mode(server_exe: &Path, client_exe: &Path, config: &mut Config) -> io::Result<()> {
    let Some(mode) = LauncherMode::from_str(&config.launcher.last_mode) else {
        println!("No saved launch mode yet.");
        pause_before_exit();
        return Ok(());
    };

    let host = config.launcher.last_host.clone();
    let port = config.launcher.last_port;
    launch_mode(server_exe, client_exe, config, mode, host, port)
}

fn launch_mode(
    server_exe: &Path,
    client_exe: &Path,
    config: &mut Config,
    mode: LauncherMode,
    host: String,
    port: u16,
) -> io::Result<()> {
    match mode {
        LauncherMode::StartHere => {
            start_server_window(server_exe, DEFAULT_HOST, port)?;
            std::thread::sleep(std::time::Duration::from_millis(900));
            start_client_window(client_exe, DEFAULT_HOST, port)?;
            record_last_launch(config, mode, DEFAULT_HOST, port)?;
            println!("Chatify is starting in two windows.");
            println!("Use any username and password the first time on this local server.");
            pause_before_exit();
            Ok(())
        }
        LauncherMode::HostForOthers => {
            start_server_window(server_exe, LAN_HOST, port)?;
            std::thread::sleep(std::time::Duration::from_millis(900));
            start_client_window(client_exe, DEFAULT_HOST, port)?;
            record_last_launch(config, mode, LAN_HOST, port)?;
            println!("Chatify is hosting on port {port}.");
            println!("People on your network can join with this computer's IP and port {port}.");
            println!("Windows Firewall may ask for permission the first time.");
            pause_before_exit();
            Ok(())
        }
        LauncherMode::JoinServer => {
            record_last_launch(config, mode, &host, port)?;
            println!("Opening Chatify client for {host}:{port}...");
            run_client_foreground(client_exe, &host, port)
        }
        LauncherMode::ServerOnly => {
            start_server_window(server_exe, &host, port)?;
            record_last_launch(config, mode, &host, port)?;
            println!("Chatify server is starting on {host}:{port}.");
            println!("New usernames can register on this server.");
            pause_before_exit();
            Ok(())
        }
    }
}

fn quick_start_local(server_exe: &Path, client_exe: &Path, config: &mut Config) -> io::Result<()> {
    let port = prompt_port(config.launcher.last_port)?;
    launch_mode(
        server_exe,
        client_exe,
        config,
        LauncherMode::StartHere,
        DEFAULT_HOST.to_string(),
        port,
    )
}

fn host_for_lan(server_exe: &Path, client_exe: &Path, config: &mut Config) -> io::Result<()> {
    let port = prompt_port(config.launcher.last_port)?;
    launch_mode(
        server_exe,
        client_exe,
        config,
        LauncherMode::HostForOthers,
        LAN_HOST.to_string(),
        port,
    )
}

fn join_server(client_exe: &Path, config: &mut Config) -> io::Result<()> {
    let profile = choose_profile_or_manual(config)?;
    upsert_profile(&mut config.launcher.profiles, profile.clone());
    launch_mode(
        Path::new(""),
        client_exe,
        config,
        LauncherMode::JoinServer,
        profile.host,
        profile.port,
    )
}

fn start_server_only(server_exe: &Path, config: &mut Config) -> io::Result<()> {
    let bind_host = prompt_host(if config.launcher.last_host.is_empty() {
        LAN_HOST
    } else {
        &config.launcher.last_host
    })?;
    let port = prompt_port(config.launcher.last_port)?;
    launch_mode(
        server_exe,
        Path::new(""),
        config,
        LauncherMode::ServerOnly,
        bind_host,
        port,
    )
}

fn choose_profile_or_manual(config: &mut Config) -> io::Result<ServerProfileConfig> {
    ensure_profiles(&mut config.launcher.profiles);
    println!();
    println!("Saved servers:");
    for (index, profile) in config.launcher.profiles.iter().enumerate() {
        println!(
            "{}) {}  {}:{}",
            index + 1,
            profile.name,
            profile.host,
            profile.port
        );
    }
    println!("A) Add a new server");
    println!("M) Manual one-time host");
    println!();

    loop {
        let choice = prompt("Server", "1")?;
        let normalized = choice.trim().to_ascii_lowercase();
        if normalized.is_empty() {
            return Ok(config.launcher.profiles[0].clone());
        }
        if normalized == "a" {
            let default_host = config.launcher.last_host.as_str();
            let default_host = if default_host.is_empty() {
                DEFAULT_HOST
            } else {
                default_host
            };
            return prompt_profile(default_host, config.launcher.last_port);
        }
        if normalized == "m" {
            let host = prompt_host(DEFAULT_HOST)?;
            let port = prompt_port(DEFAULT_PORT)?;
            return Ok(ServerProfileConfig {
                name: format!("{}:{}", host, port),
                host,
                port,
            });
        }
        if let Ok(index) = normalized.parse::<usize>() {
            if let Some(profile) = config.launcher.profiles.get(index.saturating_sub(1)) {
                return Ok(profile.clone());
            }
        }
        println!("Choose a saved server number, A, or M.");
    }
}

fn manage_profiles(config: &mut Config) -> io::Result<()> {
    loop {
        ensure_profiles(&mut config.launcher.profiles);
        println!();
        println!("Saved servers:");
        for (index, profile) in config.launcher.profiles.iter().enumerate() {
            println!(
                "{}) {}  {}:{}",
                index + 1,
                profile.name,
                profile.host,
                profile.port
            );
        }
        println!();
        println!("A) Add server");
        println!("D) Delete server");
        println!("Q) Back");
        println!();

        match prompt("Choose", "Q")?.trim().to_ascii_lowercase().as_str() {
            "" | "q" => return Ok(()),
            "a" => {
                let profile = prompt_profile(DEFAULT_HOST, DEFAULT_PORT)?;
                upsert_profile(&mut config.launcher.profiles, profile);
                save_config(config)?;
                println!("Saved.");
            }
            "d" => {
                let raw = prompt("Delete number", "")?;
                let Some(index) = raw
                    .trim()
                    .parse::<usize>()
                    .ok()
                    .and_then(|value| value.checked_sub(1))
                else {
                    println!("Enter a server number.");
                    continue;
                };
                if index >= config.launcher.profiles.len() {
                    println!("No server at that number.");
                    continue;
                }
                let removed = config.launcher.profiles.remove(index);
                ensure_profiles(&mut config.launcher.profiles);
                save_config(config)?;
                println!("Removed {}.", removed.name);
            }
            _ => println!("Choose A, D, or Q."),
        }
    }
}

fn prompt_profile(default_host: &str, default_port: u16) -> io::Result<ServerProfileConfig> {
    let host = prompt_host(default_host)?;
    let port = prompt_port(default_port)?;
    let fallback_name = format!("{}:{}", host, port);
    let name = prompt("Profile name", &fallback_name)?;
    let name = clean_profile_name(&name).unwrap_or(fallback_name);
    Ok(ServerProfileConfig { name, host, port })
}

fn clean_profile_name(raw: &str) -> Option<String> {
    let name = raw.trim();
    if name.is_empty() {
        None
    } else {
        Some(name.chars().take(40).collect())
    }
}

fn ensure_profiles(profiles: &mut Vec<ServerProfileConfig>) {
    if profiles.is_empty() {
        profiles.push(ServerProfileConfig {
            name: "Local".to_string(),
            host: DEFAULT_HOST.to_string(),
            port: DEFAULT_PORT,
        });
    }
}

fn upsert_profile(profiles: &mut Vec<ServerProfileConfig>, profile: ServerProfileConfig) {
    ensure_profiles(profiles);
    if let Some(existing) = profiles
        .iter_mut()
        .find(|existing| existing.name.eq_ignore_ascii_case(&profile.name))
    {
        *existing = profile;
    } else {
        profiles.push(profile);
    }
    profiles.sort_by_key(|profile| profile.name.to_ascii_lowercase());
}

fn record_last_launch(
    config: &mut Config,
    mode: LauncherMode,
    host: &str,
    port: u16,
) -> io::Result<()> {
    config.launcher.last_mode = mode.as_str().to_string();
    config.launcher.last_host = host.to_string();
    config.launcher.last_port = port;
    config.launcher.first_run_complete = true;
    save_config(config)
}

fn save_config(config: &Config) -> io::Result<()> {
    config.save().map_err(io::Error::other)
}

fn ensure_launcher_defaults(config: &mut Config) -> io::Result<()> {
    if config.launcher.last_host.trim().is_empty() {
        config.launcher.last_host = DEFAULT_HOST.to_string();
    }
    if config.launcher.last_port == 0 {
        config.launcher.last_port = DEFAULT_PORT;
    }
    if config.launcher.profiles.is_empty() {
        ensure_profiles(&mut config.launcher.profiles);
    }
    Ok(())
}

fn start_server_window(server_exe: &Path, host: &str, port: u16) -> io::Result<()> {
    ensure_binary_exists(server_exe)?;

    let data_dir = chatify_data_dir();
    std::fs::create_dir_all(&data_dir)?;
    let db_path = data_dir.join("chatify.db");

    let args = vec![
        "--host".to_string(),
        host.to_string(),
        "--port".to_string(),
        port.to_string(),
        "--db".to_string(),
        db_path.to_string_lossy().to_string(),
        "--enable-self-registration".to_string(),
    ];

    start_in_new_window("Chatify Server", server_exe, &args)
}

fn start_client_window(client_exe: &Path, host: &str, port: u16) -> io::Result<()> {
    ensure_binary_exists(client_exe)?;
    let args = vec![
        "--host".to_string(),
        host.to_string(),
        "--port".to_string(),
        port.to_string(),
    ];

    start_in_new_window("Chatify Client", client_exe, &args)
}

fn run_client_foreground(client_exe: &Path, host: &str, port: u16) -> io::Result<()> {
    ensure_binary_exists(client_exe)?;
    let status = Command::new(client_exe)
        .arg("--host")
        .arg(host)
        .arg("--port")
        .arg(port.to_string())
        .status()?;

    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "chatify-client exited with {status}"
        )))
    }
}

#[cfg(windows)]
fn start_in_new_window(title: &str, exe: &Path, args: &[String]) -> io::Result<()> {
    let mut command_line = quote_cmd_arg(exe.as_os_str());
    for arg in args {
        command_line.push(' ');
        command_line.push_str(&quote_cmd_arg(arg));
    }

    Command::new("cmd")
        .arg("/C")
        .arg("start")
        .arg(title)
        .arg("cmd")
        .arg("/K")
        .arg(command_line)
        .spawn()?;

    Ok(())
}

#[cfg(not(windows))]
fn start_in_new_window(_title: &str, exe: &Path, args: &[String]) -> io::Result<()> {
    Command::new(exe).args(args).spawn()?;
    Ok(())
}

fn open_readme(app_dir: &Path) -> io::Result<()> {
    let packaged_readme = app_dir.join("README.txt");
    let repo_readme = app_dir.join("README.md");
    let readme = if packaged_readme.exists() {
        packaged_readme
    } else {
        repo_readme
    };

    if !readme.exists() {
        println!("README was not found in {}.", app_dir.display());
        pause_before_exit();
        return Ok(());
    }

    #[cfg(windows)]
    {
        Command::new("cmd")
            .arg("/C")
            .arg("start")
            .arg("")
            .arg(readme)
            .spawn()?;
    }

    #[cfg(not(windows))]
    {
        println!("{}", readme.display());
    }

    Ok(())
}

fn resolve_app_dir() -> io::Result<PathBuf> {
    let exe = env::current_exe()?;
    Ok(exe
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from(".")))
}

fn binary_path(app_dir: &Path, stem: &str) -> PathBuf {
    #[cfg(windows)]
    {
        app_dir.join(format!("{stem}.exe"))
    }

    #[cfg(not(windows))]
    {
        app_dir.join(stem)
    }
}

fn ensure_binary_exists(path: &Path) -> io::Result<()> {
    if path.exists() {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("required binary not found: {}", path.display()),
        ))
    }
}

fn chatify_data_dir() -> PathBuf {
    env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
        .join("Chatify")
}

fn config_path_label() -> String {
    Config::config_path()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "(unknown)".to_string())
}

fn prompt_host(default: &str) -> io::Result<String> {
    loop {
        let raw = prompt("Host/IP", default)?;
        if let Some(host) = clean_host_or_default(&raw, default) {
            return Ok(host);
        }
        println!("Host/IP cannot contain spaces.");
    }
}

fn prompt_port(default: u16) -> io::Result<u16> {
    loop {
        let raw = prompt("Port", &default.to_string())?;
        if let Some(port) = parse_port_or_default(&raw, default) {
            return Ok(port);
        }
        println!("Port must be between 1 and 65535.");
    }
}

fn prompt(label: &str, default: &str) -> io::Result<String> {
    if default.is_empty() {
        print!("{label}: ");
    } else {
        print!("{label} [{default}]: ");
    }
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    Ok(input.trim().to_string())
}

fn pause_before_exit() {
    print!("Press Enter to continue...");
    let _ = io::stdout().flush();
    let mut input = String::new();
    let _ = io::stdin().read_line(&mut input);
}

fn clean_host_or_default(raw: &str, default: &str) -> Option<String> {
    let value = raw.trim();
    let host = if value.is_empty() { default } else { value };
    if host.chars().any(char::is_whitespace) {
        None
    } else {
        Some(host.to_string())
    }
}

fn parse_port_or_default(raw: &str, default: u16) -> Option<u16> {
    let value = raw.trim();
    if value.is_empty() {
        return Some(default);
    }

    value.parse::<u16>().ok().filter(|port| *port > 0)
}

fn quote_cmd_arg(value: impl AsRef<OsStr>) -> String {
    let value = value.as_ref().to_string_lossy();
    if value.is_empty() {
        return "\"\"".to_string();
    }

    let needs_quotes = value.chars().any(|ch| {
        ch.is_whitespace() || matches!(ch, '"' | '&' | '(' | ')' | '^' | '|' | '<' | '>')
    });

    if needs_quotes {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        clean_host_or_default, clean_profile_name, normalize_menu_choice, parse_port_or_default,
        quote_cmd_arg, upsert_profile, LauncherMode,
    };
    use chatify::config::{Config, ServerProfileConfig};

    #[test]
    fn parse_port_accepts_default_and_valid_values() {
        assert_eq!(parse_port_or_default("", 8765), Some(8765));
        assert_eq!(parse_port_or_default("9000", 8765), Some(9000));
    }

    #[test]
    fn parse_port_rejects_invalid_values() {
        assert_eq!(parse_port_or_default("0", 8765), None);
        assert_eq!(parse_port_or_default("70000", 8765), None);
        assert_eq!(parse_port_or_default("abc", 8765), None);
    }

    #[test]
    fn clean_host_uses_default_and_rejects_spaces() {
        assert_eq!(
            clean_host_or_default("", "127.0.0.1"),
            Some("127.0.0.1".to_string())
        );
        assert_eq!(
            clean_host_or_default("chatify.local", "127.0.0.1"),
            Some("chatify.local".to_string())
        );
        assert_eq!(clean_host_or_default("bad host", "127.0.0.1"), None);
    }

    #[test]
    fn quote_cmd_arg_only_quotes_when_needed() {
        assert_eq!(quote_cmd_arg("chatify-client.exe"), "chatify-client.exe");
        assert_eq!(
            quote_cmd_arg(r#"C:\Program Files\Chatify\chatify.exe"#),
            r#""C:\Program Files\Chatify\chatify.exe""#
        );
    }

    #[test]
    fn launcher_mode_roundtrips_persisted_names() {
        assert_eq!(
            LauncherMode::from_str(LauncherMode::StartHere.as_str()),
            Some(LauncherMode::StartHere)
        );
        assert_eq!(
            LauncherMode::from_str(LauncherMode::JoinServer.as_str()),
            Some(LauncherMode::JoinServer)
        );
        assert_eq!(LauncherMode::from_str("unknown"), None);
    }

    #[test]
    fn blank_choice_reconnects_when_last_mode_exists() {
        let mut config = Config::default();
        config.launcher.last_mode = LauncherMode::JoinServer.as_str().to_string();
        assert_eq!(normalize_menu_choice("", &config), "r");

        config.launcher.last_mode.clear();
        assert_eq!(normalize_menu_choice("", &config), "");
    }

    #[test]
    fn profiles_upsert_by_case_insensitive_name() {
        let mut profiles = Vec::new();
        upsert_profile(
            &mut profiles,
            ServerProfileConfig {
                name: "Home".to_string(),
                host: "10.0.0.2".to_string(),
                port: 8765,
            },
        );
        upsert_profile(
            &mut profiles,
            ServerProfileConfig {
                name: "home".to_string(),
                host: "10.0.0.3".to_string(),
                port: 9000,
            },
        );

        let homes: Vec<_> = profiles
            .iter()
            .filter(|profile| profile.name.eq_ignore_ascii_case("home"))
            .collect();
        assert_eq!(homes.len(), 1);
        assert_eq!(homes[0].host, "10.0.0.3");
        assert_eq!(homes[0].port, 9000);
    }

    #[test]
    fn clean_profile_name_limits_empty_and_long_names() {
        assert_eq!(clean_profile_name("   "), None);
        assert_eq!(clean_profile_name(" Home "), Some("Home".to_string()));
        assert_eq!(clean_profile_name(&"x".repeat(80)).unwrap().len(), 40);
    }
}

use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

pub const PLUGIN_API_VERSION: &str = "1";
pub const DEFAULT_BUILTIN_PLUGINS: &[&str] = &["poll", "standup", "deploy-notifier"];
const MAX_COMMAND_NAME_LEN: usize = 32;
const MAX_PLUGIN_ID_LEN: usize = 64;
const MAX_PLUGIN_COMMANDS: usize = 32;
const MAX_PLUGIN_DESCRIPTION_LEN: usize = 160;
const MAX_PLUGIN_SPEC_LEN: usize = 1024;
const MAX_PLUGIN_MESSAGES_PER_RESPONSE: usize = 16;
const MAX_PLUGIN_MESSAGE_LEN: usize = 1024;
const MAX_PLUGIN_REPLACEMENT_LEN: usize = 4096;
const MAX_PLUGIN_PAYLOAD_BYTES: usize = 16 * 1024;
const MAX_PLUGIN_STDOUT_BYTES: usize = 64 * 1024;
const MAX_PLUGIN_STDERR_BYTES: usize = 16 * 1024;
const PLUGIN_PROCESS_TIMEOUT: Duration = Duration::from_secs(5);
const PROCESS_TERMINATION_TIMEOUT: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlashCommandRegistration {
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    pub api_version: String,
    pub name: String,
    #[serde(default)]
    pub commands: Vec<SlashCommandRegistration>,
    #[serde(default)]
    pub message_hook: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginMessageTarget {
    Channel,
    Sender,
}

impl PluginMessageTarget {
    fn from_wire(value: &str) -> Self {
        if value.eq_ignore_ascii_case("sender") {
            Self::Sender
        } else {
            Self::Channel
        }
    }
}

#[derive(Debug, Clone)]
pub struct PluginMessage {
    pub plugin: String,
    pub target: PluginMessageTarget,
    pub text: String,
}

#[derive(Debug, Clone, Default)]
pub struct SlashExecutionResult {
    pub messages: Vec<PluginMessage>,
}

#[derive(Debug, Clone, Default)]
pub struct MessageHookResult {
    pub blocked: bool,
    pub replacement: Option<String>,
    pub messages: Vec<PluginMessage>,
}

#[derive(Debug, Clone)]
enum PluginSpec {
    Builtin(String),
    External(PathBuf),
}

impl PluginSpec {
    fn source_label(&self) -> String {
        match self {
            PluginSpec::Builtin(name) => format!("builtin:{}", name),
            PluginSpec::External(path) => path.to_string_lossy().to_string(),
        }
    }
}

#[derive(Debug, Clone)]
struct PluginRecord {
    id: String,
    enabled: bool,
    manifest: PluginManifest,
    spec: PluginSpec,
}

pub struct PluginRuntime {
    server_executable: PathBuf,
    plugins: DashMap<String, PluginRecord>,
    slash_registry: DashMap<String, String>,
}

impl PluginRuntime {
    pub fn new(server_executable: PathBuf) -> Self {
        Self {
            server_executable,
            plugins: DashMap::new(),
            slash_registry: DashMap::new(),
        }
    }

    pub fn install_plugin(&self, spec: &str) -> Result<PluginManifest, String> {
        let parsed_spec = self.parse_plugin_spec(spec)?;
        let mut manifest = self.fetch_manifest(&parsed_spec)?;
        self.validate_manifest(&manifest)?;

        for cmd in &mut manifest.commands {
            cmd.name = normalize_command_name(&cmd.name)
                .ok_or_else(|| "plugin command name cannot be empty".to_string())?;
            if cmd.description.trim().is_empty() {
                cmd.description = format!("{} command", cmd.name);
            }
            validate_command_description(&cmd.name, &cmd.description)?;
        }

        let raw_plugin_id = if manifest.name.trim().is_empty() {
            default_plugin_id_from_spec(&parsed_spec)
        } else {
            manifest.name.trim().to_string()
        };

        let plugin_id = normalize_plugin_id(&raw_plugin_id).ok_or_else(|| {
            format!(
                "plugin id '{}' is invalid (allowed: [a-z0-9._-], max {} chars)",
                raw_plugin_id, MAX_PLUGIN_ID_LEN
            )
        })?;

        // Keep manifest identity canonical in runtime outputs.
        manifest.name = plugin_id.clone();

        if self.plugins.contains_key(&plugin_id) {
            return Err(format!("plugin '{}' is already installed", plugin_id));
        }

        for cmd in &manifest.commands {
            if let Some(existing) = self.slash_registry.get(&cmd.name) {
                if existing.value() != &plugin_id {
                    return Err(format!(
                        "command '/{}' is already registered by plugin '{}'",
                        cmd.name,
                        existing.value()
                    ));
                }
            }
        }

        let record = PluginRecord {
            id: plugin_id.clone(),
            enabled: true,
            manifest: manifest.clone(),
            spec: parsed_spec,
        };

        self.plugins.insert(plugin_id, record);
        self.rebuild_slash_registry();
        Ok(manifest)
    }

    pub fn disable_plugin(&self, plugin_id: &str) -> Result<(), String> {
        let id = normalize_plugin_id(plugin_id).ok_or_else(|| {
            format!(
                "plugin id '{}' is invalid (allowed: [a-z0-9._-], max {} chars)",
                plugin_id, MAX_PLUGIN_ID_LEN
            )
        })?;
        let Some(mut entry) = self.plugins.get_mut(&id) else {
            return Err(format!("plugin '{}' is not installed", plugin_id));
        };
        entry.enabled = false;
        drop(entry);
        self.rebuild_slash_registry();
        Ok(())
    }

    pub fn list_plugins_json(&self) -> Value {
        let mut rows: Vec<Value> = self
            .plugins
            .iter()
            .map(|entry| {
                let p = entry.value();
                let commands: Vec<Value> = p
                    .manifest
                    .commands
                    .iter()
                    .map(|cmd| {
                        json!({
                            "name": cmd.name,
                            "description": cmd.description
                        })
                    })
                    .collect();
                json!({
                    "id": p.id,
                    "enabled": p.enabled,
                    "api_version": p.manifest.api_version,
                    "message_hook": p.manifest.message_hook,
                    "commands": commands,
                    "source": p.spec.source_label(),
                })
            })
            .collect();

        rows.sort_by(|a, b| {
            let lhs = a
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_ascii_lowercase();
            let rhs = b
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_ascii_lowercase();
            lhs.cmp(&rhs)
        });

        Value::Array(rows)
    }

    pub fn execute_slash(
        &self,
        channel: &str,
        user: &str,
        command: &str,
        args: &[String],
    ) -> Result<SlashExecutionResult, String> {
        let cmd = normalize_command_name(command)
            .ok_or_else(|| "slash command name is required".to_string())?;

        let plugin_id = self
            .slash_registry
            .get(&cmd)
            .map(|entry| entry.value().clone())
            .ok_or_else(|| format!("unknown slash command '/{}'", cmd))?;

        let plugin = self
            .plugins
            .get(&plugin_id)
            .map(|entry| entry.value().clone())
            .ok_or_else(|| format!("plugin '{}' is not installed", plugin_id))?;

        if !plugin.enabled {
            return Err(format!("plugin '{}' is disabled", plugin.id));
        }

        let payload = json!({
            "api_version": PLUGIN_API_VERSION,
            "channel": channel,
            "user": user,
            "command": cmd,
            "args": args,
        });

        let response =
            self.invoke_plugin_process(&plugin.spec, "slash", Some(&cmd), Some(&payload))?;
        self.validate_response_api_version(&response)?;
        if let Some(err) = parse_error_field(&response)? {
            return Err(format!("plugin '{}' error: {}", plugin.id, err));
        }

        Ok(SlashExecutionResult {
            messages: parse_messages(&plugin.id, &response)?,
        })
    }

    pub fn apply_message_hooks(
        &self,
        channel: &str,
        user: &str,
        content: &str,
    ) -> Result<MessageHookResult, String> {
        let mut plugins: Vec<PluginRecord> = self
            .plugins
            .iter()
            .filter_map(|entry| {
                let p = entry.value();
                if p.enabled && p.manifest.message_hook {
                    Some(p.clone())
                } else {
                    None
                }
            })
            .collect();

        plugins.sort_by(|a, b| a.id.cmp(&b.id));

        let mut result = MessageHookResult::default();
        let mut current_content = content.to_string();

        for plugin in plugins {
            let payload = json!({
                "api_version": PLUGIN_API_VERSION,
                "channel": channel,
                "user": user,
                "content": current_content,
            });
            let response =
                self.invoke_plugin_process(&plugin.spec, "message_hook", None, Some(&payload))?;
            self.validate_response_api_version(&response)?;

            if let Some(err) = parse_error_field(&response)? {
                return Err(format!("plugin '{}' error: {}", plugin.id, err));
            }

            if parse_optional_bool_field(&response, "blocked")?.unwrap_or(false) {
                result.blocked = true;
            }

            if let Some(new_content) = parse_optional_string_field(&response, "replacement")? {
                validate_text_field(
                    "plugin response field 'replacement'",
                    new_content,
                    MAX_PLUGIN_REPLACEMENT_LEN,
                )?;
                current_content = new_content.to_string();
                result.replacement = Some(current_content.clone());
            }

            result
                .messages
                .extend(parse_messages(&plugin.id, &response)?);
            if result.blocked {
                break;
            }
        }

        Ok(result)
    }

    fn parse_plugin_spec(&self, spec: &str) -> Result<PluginSpec, String> {
        let trimmed = spec.trim();
        if trimmed.is_empty() {
            return Err("plugin spec cannot be empty".to_string());
        }
        if trimmed.len() > MAX_PLUGIN_SPEC_LEN {
            return Err(format!(
                "plugin spec is too long (max {} chars)",
                MAX_PLUGIN_SPEC_LEN
            ));
        }

        let lowercase = trimmed.to_ascii_lowercase();
        if DEFAULT_BUILTIN_PLUGINS.contains(&lowercase.as_str()) {
            return Ok(PluginSpec::Builtin(lowercase));
        }

        let external = PathBuf::from(trimmed)
            .canonicalize()
            .map_err(|err| format!("failed to resolve plugin path '{}': {}", trimmed, err))?;

        Ok(PluginSpec::External(external))
    }

    fn fetch_manifest(&self, spec: &PluginSpec) -> Result<PluginManifest, String> {
        let response = self.invoke_plugin_process(spec, "manifest", None, None)?;
        self.validate_response_api_version(&response)?;
        let manifest: PluginManifest = serde_json::from_value(response)
            .map_err(|err| format!("invalid plugin manifest response: {}", err))?;
        Ok(manifest)
    }

    fn validate_manifest(&self, manifest: &PluginManifest) -> Result<(), String> {
        if manifest.api_version != PLUGIN_API_VERSION {
            return Err(format!(
                "unsupported plugin API version '{}', expected '{}'",
                manifest.api_version, PLUGIN_API_VERSION
            ));
        }

        if !manifest.name.trim().is_empty() && normalize_plugin_id(manifest.name.trim()).is_none() {
            return Err(format!(
                "plugin name '{}' is invalid (allowed: [a-z0-9._-], max {} chars)",
                manifest.name.trim(),
                MAX_PLUGIN_ID_LEN
            ));
        }

        if manifest.commands.len() > MAX_PLUGIN_COMMANDS {
            return Err(format!(
                "plugin declares too many commands (max {})",
                MAX_PLUGIN_COMMANDS
            ));
        }

        let mut seen_commands = BTreeSet::new();
        for cmd in &manifest.commands {
            let normalized = normalize_command_name(&cmd.name)
                .ok_or_else(|| "plugin command name cannot be empty".to_string())?;

            if !seen_commands.insert(normalized.clone()) {
                return Err(format!("duplicate plugin command '/{}'", normalized));
            }

            validate_command_description(&normalized, &cmd.description)?;
        }

        Ok(())
    }

    fn validate_response_api_version(&self, response: &Value) -> Result<(), String> {
        if !response.is_object() {
            return Err("plugin response must be a JSON object".to_string());
        }

        let api_version = response
            .get("api_version")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "plugin response missing api_version".to_string())?;

        if api_version != PLUGIN_API_VERSION {
            return Err(format!(
                "plugin response api_version '{}' is unsupported (expected '{}')",
                api_version, PLUGIN_API_VERSION
            ));
        }

        Ok(())
    }

    fn rebuild_slash_registry(&self) {
        self.slash_registry.clear();

        let mut entries: Vec<(String, String)> = self
            .plugins
            .iter()
            .filter_map(|entry| {
                let plugin = entry.value();
                if !plugin.enabled {
                    return None;
                }
                Some(
                    plugin
                        .manifest
                        .commands
                        .iter()
                        .map(|cmd| (cmd.name.clone(), plugin.id.clone()))
                        .collect::<Vec<(String, String)>>(),
                )
            })
            .flatten()
            .collect();

        entries.sort();
        for (command, plugin_id) in entries {
            self.slash_registry.insert(command, plugin_id);
        }
    }

    fn invoke_plugin_process(
        &self,
        spec: &PluginSpec,
        op: &str,
        command: Option<&str>,
        payload: Option<&Value>,
    ) -> Result<Value, String> {
        if op != "manifest" && op != "slash" && op != "message_hook" {
            return Err(format!("unsupported plugin operation '{}'", op));
        }

        let payload_text = if let Some(payload_value) = payload {
            let text = payload_value.to_string();
            if text.len() > MAX_PLUGIN_PAYLOAD_BYTES {
                return Err(format!(
                    "plugin payload too large (max {} bytes)",
                    MAX_PLUGIN_PAYLOAD_BYTES
                ));
            }
            Some(text)
        } else {
            None
        };

        let mut process = match spec {
            PluginSpec::Builtin(plugin_name) => {
                let mut cmd = Command::new(&self.server_executable);
                cmd.arg("--chatify-plugin-worker").arg(plugin_name);
                cmd
            }
            PluginSpec::External(path) => {
                if !path.exists() {
                    return Err(format!(
                        "plugin executable not found: {}",
                        path.to_string_lossy()
                    ));
                }
                if !path.is_file() {
                    return Err(format!(
                        "plugin executable path is not a file: {}",
                        path.to_string_lossy()
                    ));
                }
                Command::new(path)
            }
        };

        process
            .arg("--chatify-plugin-op")
            .arg(op)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        if payload_text.is_some() {
            process.stdin(Stdio::piped());
        } else {
            process.stdin(Stdio::null());
        }

        if let Some(command_name) = command {
            process.arg("--chatify-plugin-command").arg(command_name);
        }

        let mut child = process
            .spawn()
            .map_err(|err| format!("failed to spawn plugin process: {}", err))?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "failed to capture plugin stdout".to_string())?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| "failed to capture plugin stderr".to_string())?;

        let stdout_reader =
            thread::spawn(move || read_limited_from_pipe(stdout, MAX_PLUGIN_STDOUT_BYTES));
        let stderr_reader =
            thread::spawn(move || read_limited_from_pipe(stderr, MAX_PLUGIN_STDERR_BYTES));

        if let Some(input) = payload_text {
            if let Some(mut stdin) = child.stdin.take() {
                if let Err(err) = stdin.write_all(input.as_bytes()) {
                    let termination_error = terminate_plugin_process(&mut child).err();
                    if termination_error.is_none() {
                        let _ = join_stream_reader(stdout_reader, "stdout");
                        let _ = join_stream_reader(stderr_reader, "stderr");
                    }

                    let termination_note = termination_error
                        .map(|e| format!("; termination warning: {}", e))
                        .unwrap_or_default();
                    return Err(format!(
                        "failed to write plugin payload: {}{}",
                        err, termination_note
                    ));
                }
            }
        }

        let deadline = Instant::now() + PLUGIN_PROCESS_TIMEOUT;
        let status = loop {
            if let Some(status) = child
                .try_wait()
                .map_err(|err| format!("failed to poll plugin process status: {}", err))?
            {
                break status;
            }

            if Instant::now() >= deadline {
                let termination_error = terminate_plugin_process(&mut child).err();
                let (stderr_bytes, stderr_truncated) = if termination_error.is_none() {
                    let _ = join_stream_reader(stdout_reader, "stdout");
                    join_stream_reader(stderr_reader, "stderr").unwrap_or_default()
                } else {
                    drop(stdout_reader);
                    drop(stderr_reader);
                    (Vec::new(), false)
                };
                let stderr_message = String::from_utf8_lossy(&stderr_bytes).trim().to_string();
                let stderr_preview = if stderr_message.is_empty() {
                    "<no stderr>".to_string()
                } else {
                    truncate_for_error(&stderr_message, 240)
                };
                let suffix = if stderr_truncated {
                    " [stderr truncated]"
                } else {
                    ""
                };
                let termination_note = termination_error
                    .map(|e| format!("; termination warning: {}", e))
                    .unwrap_or_default();

                return Err(format!(
                    "plugin process timed out after {}ms (op={}): {}{}{}",
                    PLUGIN_PROCESS_TIMEOUT.as_millis(),
                    op,
                    stderr_preview,
                    suffix,
                    termination_note
                ));
            }

            thread::sleep(Duration::from_millis(10));
        };

        let (stdout_bytes, stdout_truncated) = join_stream_reader(stdout_reader, "stdout")?;
        let (stderr_bytes, stderr_truncated) = join_stream_reader(stderr_reader, "stderr")?;

        if stdout_truncated {
            return Err(format!(
                "plugin process stdout exceeded {} bytes (op={})",
                MAX_PLUGIN_STDOUT_BYTES, op
            ));
        }

        if !status.success() {
            let stderr = String::from_utf8_lossy(&stderr_bytes).trim().to_string();
            let stderr_preview = if stderr.is_empty() {
                "<no stderr>".to_string()
            } else {
                truncate_for_error(&stderr, 240)
            };
            let suffix = if stderr_truncated {
                " [stderr truncated]"
            } else {
                ""
            };
            return Err(format!(
                "plugin process failed (op={}): {}{}",
                op, stderr_preview, suffix
            ));
        }

        let stdout = String::from_utf8_lossy(&stdout_bytes).trim().to_string();
        if stdout.is_empty() {
            return Err(format!("plugin process returned empty output (op={})", op));
        }

        serde_json::from_str::<Value>(&stdout).map_err(|err| {
            format!(
                "plugin process returned invalid JSON: {} (output='{}')",
                err,
                truncate_for_error(&stdout, 240)
            )
        })
    }
}

fn read_limited_from_pipe(
    mut reader: impl Read,
    max_bytes: usize,
) -> Result<(Vec<u8>, bool), String> {
    let mut out = Vec::new();
    let mut buf = [0_u8; 4096];
    let mut truncated = false;

    loop {
        let read = reader
            .read(&mut buf)
            .map_err(|err| format!("stream read failed: {}", err))?;
        if read == 0 {
            break;
        }

        if out.len() < max_bytes {
            let remaining = max_bytes - out.len();
            let keep = remaining.min(read);
            out.extend_from_slice(&buf[..keep]);
            if keep < read {
                truncated = true;
            }
        } else {
            truncated = true;
        }
    }

    Ok((out, truncated))
}

fn join_stream_reader(
    handle: thread::JoinHandle<Result<(Vec<u8>, bool), String>>,
    stream_name: &str,
) -> Result<(Vec<u8>, bool), String> {
    match handle.join() {
        Ok(result) => {
            result.map_err(|err| format!("failed reading plugin {}: {}", stream_name, err))
        }
        Err(_) => Err(format!("plugin {} reader thread panicked", stream_name)),
    }
}

fn terminate_plugin_process(child: &mut Child) -> Result<(), String> {
    if child
        .try_wait()
        .map_err(|err| format!("failed to poll plugin process status: {}", err))?
        .is_some()
    {
        return Ok(());
    }

    kill_process_tree(child)?;

    let deadline = Instant::now() + PROCESS_TERMINATION_TIMEOUT;
    loop {
        if child
            .try_wait()
            .map_err(|err| format!("failed to poll plugin process status: {}", err))?
            .is_some()
        {
            return Ok(());
        }

        if Instant::now() >= deadline {
            return Err(format!(
                "plugin process did not terminate within {}ms",
                PROCESS_TERMINATION_TIMEOUT.as_millis()
            ));
        }

        thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(windows)]
fn kill_process_tree(child: &mut Child) -> Result<(), String> {
    let status = Command::new("taskkill")
        .arg("/PID")
        .arg(child.id().to_string())
        .arg("/T")
        .arg("/F")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|err| format!("failed to execute taskkill: {}", err))?;

    if status.success() {
        Ok(())
    } else {
        child
            .kill()
            .map_err(|err| format!("failed to terminate plugin process: {}", err))
    }
}

#[cfg(not(windows))]
fn kill_process_tree(child: &mut Child) -> Result<(), String> {
    child
        .kill()
        .map_err(|err| format!("failed to terminate plugin process: {}", err))
}

pub fn run_builtin_plugin_worker(
    plugin_name: &str,
    op: &str,
    command: Option<&str>,
) -> Result<(), String> {
    let response = match op {
        "manifest" => builtin_manifest(plugin_name),
        "slash" => {
            let payload = read_payload_from_stdin()?;
            builtin_slash(plugin_name, command.unwrap_or(""), &payload)
        }
        "message_hook" => {
            let payload = read_payload_from_stdin()?;
            builtin_message_hook(plugin_name, &payload)
        }
        _ => plugin_error(format!(
            "unsupported plugin op '{}': expected manifest|slash|message_hook",
            op
        )),
    };

    println!("{}", response);
    Ok(())
}

fn read_payload_from_stdin() -> Result<Value, String> {
    let mut reader = std::io::stdin()
        .lock()
        .take((MAX_PLUGIN_PAYLOAD_BYTES + 1) as u64);
    let mut bytes = Vec::new();
    reader
        .read_to_end(&mut bytes)
        .map_err(|err| format!("failed to read plugin payload: {}", err))?;

    if bytes.len() > MAX_PLUGIN_PAYLOAD_BYTES {
        return Err(format!(
            "plugin payload exceeds limit (max {} bytes)",
            MAX_PLUGIN_PAYLOAD_BYTES
        ));
    }

    let buf = String::from_utf8(bytes)
        .map_err(|_| "plugin payload must be valid UTF-8 JSON".to_string())?;

    if buf.trim().is_empty() {
        Ok(Value::Null)
    } else {
        serde_json::from_str(&buf).map_err(|err| format!("invalid plugin payload JSON: {}", err))
    }
}

fn builtin_manifest(plugin_name: &str) -> Value {
    match plugin_name {
        "poll" => json!({
            "api_version": PLUGIN_API_VERSION,
            "name": "poll",
            "message_hook": false,
            "commands": [
                {
                    "name": "poll",
                    "description": "Create a quick poll: /poll <question> <option1> <option2> [optionN]"
                }
            ]
        }),
        "standup" => json!({
            "api_version": PLUGIN_API_VERSION,
            "name": "standup",
            "message_hook": true,
            "commands": [
                {
                    "name": "standup",
                    "description": "Post a standup update template: /standup [update ...]"
                }
            ]
        }),
        "deploy-notifier" => json!({
            "api_version": PLUGIN_API_VERSION,
            "name": "deploy-notifier",
            "message_hook": true,
            "commands": [
                {
                    "name": "deploy",
                    "description": "Broadcast deployment status: /deploy <service> <status> [details ...]"
                }
            ]
        }),
        other => plugin_error(format!("unknown builtin plugin '{}'", other)),
    }
}

fn builtin_slash(plugin_name: &str, command: &str, payload: &Value) -> Value {
    let args = payload
        .get("args")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str())
                .map(|s| s.to_string())
                .collect::<Vec<String>>()
        })
        .unwrap_or_default();
    let user = payload
        .get("user")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");

    match plugin_name {
        "poll" => {
            if normalize_command_name(command).as_deref() != Some("poll") {
                return plugin_error("poll plugin received unsupported slash command");
            }
            if args.len() < 3 {
                return json!({
                    "api_version": PLUGIN_API_VERSION,
                    "messages": [
                        {
                            "target": "sender",
                            "text": "Usage: /poll <question> <option1> <option2> [optionN]"
                        }
                    ]
                });
            }

            let question = &args[0];
            let options = &args[1..];
            let mut lines = vec![format!("Poll by {}: {}", user, question)];
            for (idx, option) in options.iter().enumerate() {
                lines.push(format!("{}. {}", idx + 1, option));
            }
            lines.push("Reply with the option number to vote.".to_string());

            json!({
                "api_version": PLUGIN_API_VERSION,
                "messages": [
                    {
                        "target": "channel",
                        "text": lines.join("\\n")
                    }
                ]
            })
        }
        "standup" => {
            if normalize_command_name(command).as_deref() != Some("standup") {
                return plugin_error("standup plugin received unsupported slash command");
            }
            if args.is_empty() {
                return json!({
                    "api_version": PLUGIN_API_VERSION,
                    "messages": [
                        {
                            "target": "sender",
                            "text": "Standup template:\\nYesterday: ...\\nToday: ...\\nBlockers: ..."
                        }
                    ]
                });
            }

            json!({
                "api_version": PLUGIN_API_VERSION,
                "messages": [
                    {
                        "target": "channel",
                        "text": format!("Standup update from {}: {}", user, args.join(" "))
                    }
                ]
            })
        }
        "deploy-notifier" => {
            if normalize_command_name(command).as_deref() != Some("deploy") {
                return plugin_error("deploy-notifier plugin received unsupported slash command");
            }
            if args.len() < 2 {
                return json!({
                    "api_version": PLUGIN_API_VERSION,
                    "messages": [
                        {
                            "target": "sender",
                            "text": "Usage: /deploy <service> <status> [details ...]"
                        }
                    ]
                });
            }

            let service = &args[0];
            let status = args[1].to_ascii_lowercase();
            let details = if args.len() > 2 {
                Some(args[2..].join(" "))
            } else {
                None
            };
            let icon = deploy_status_icon(&status);
            let detail_suffix = details
                .as_deref()
                .map(|v| format!(" - {}", v))
                .unwrap_or_default();

            json!({
                "api_version": PLUGIN_API_VERSION,
                "messages": [
                    {
                        "target": "channel",
                        "text": format!("{} Deploy update from {}: {} => {}{}", icon, user, service, status, detail_suffix)
                    }
                ]
            })
        }
        other => plugin_error(format!("unknown builtin plugin '{}'", other)),
    }
}

fn builtin_message_hook(plugin_name: &str, payload: &Value) -> Value {
    let content = payload
        .get("content")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let user = payload
        .get("user")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");

    match plugin_name {
        "poll" => json!({
            "api_version": PLUGIN_API_VERSION,
            "blocked": false,
            "messages": []
        }),
        "standup" => {
            let lower = content.to_ascii_lowercase();
            if lower.contains("#standup") {
                json!({
                    "api_version": PLUGIN_API_VERSION,
                    "blocked": false,
                    "messages": [
                        {
                            "target": "channel",
                            "text": format!("Standup helper for {}: Yesterday / Today / Blockers", user)
                        }
                    ]
                })
            } else {
                json!({
                    "api_version": PLUGIN_API_VERSION,
                    "blocked": false,
                    "messages": []
                })
            }
        }
        "deploy-notifier" => {
            let lower = content.to_ascii_lowercase();
            if let Some(rest) = lower.strip_prefix("deploy:") {
                let mut tokens = rest.split_whitespace();
                let service = tokens.next().unwrap_or("");
                let status = tokens.next().unwrap_or("");
                let details = tokens.collect::<Vec<&str>>().join(" ");
                if !service.is_empty() && !status.is_empty() {
                    let icon = deploy_status_icon(status);
                    let detail_suffix = if details.is_empty() {
                        String::new()
                    } else {
                        format!(" - {}", details)
                    };
                    return json!({
                        "api_version": PLUGIN_API_VERSION,
                        "blocked": true,
                        "messages": [
                            {
                                "target": "channel",
                                "text": format!("{} Deploy update from {}: {} => {}{}", icon, user, service, status, detail_suffix)
                            }
                        ]
                    });
                }
            }

            json!({
                "api_version": PLUGIN_API_VERSION,
                "blocked": false,
                "messages": []
            })
        }
        other => plugin_error(format!("unknown builtin plugin '{}'", other)),
    }
}

fn parse_messages(plugin_id: &str, payload: &Value) -> Result<Vec<PluginMessage>, String> {
    let Some(messages_value) = payload.get("messages") else {
        return Ok(Vec::new());
    };

    let messages = messages_value
        .as_array()
        .ok_or_else(|| "plugin response field 'messages' must be an array".to_string())?;

    let mut out = Vec::new();
    for (idx, msg) in messages
        .iter()
        .take(MAX_PLUGIN_MESSAGES_PER_RESPONSE)
        .enumerate()
    {
        let obj = msg
            .as_object()
            .ok_or_else(|| format!("plugin response messages[{}] must be an object", idx))?;

        let text = obj
            .get("text")
            .ok_or_else(|| format!("plugin response messages[{}].text is required", idx))?
            .as_str()
            .ok_or_else(|| format!("plugin response messages[{}].text must be a string", idx))?
            .trim();

        if text.is_empty() {
            continue;
        }

        validate_text_field(
            &format!("plugin response messages[{}].text", idx),
            text,
            MAX_PLUGIN_MESSAGE_LEN,
        )?;

        let target = match obj.get("target") {
            Some(Value::String(target)) => PluginMessageTarget::from_wire(target),
            Some(_) => {
                return Err(format!(
                    "plugin response messages[{}].target must be a string",
                    idx
                ));
            }
            None => PluginMessageTarget::Channel,
        };

        out.push(PluginMessage {
            plugin: plugin_id.to_string(),
            target,
            text: text.to_string(),
        });
    }

    Ok(out)
}

fn parse_error_field(payload: &Value) -> Result<Option<&str>, String> {
    match payload.get("error") {
        Some(Value::String(value)) => Ok(Some(value.as_str())),
        Some(_) => Err("plugin response field 'error' must be a string".to_string()),
        None => Ok(None),
    }
}

fn parse_optional_bool_field(payload: &Value, key: &str) -> Result<Option<bool>, String> {
    match payload.get(key) {
        Some(Value::Bool(value)) => Ok(Some(*value)),
        Some(_) => Err(format!("plugin response field '{}' must be a boolean", key)),
        None => Ok(None),
    }
}

fn parse_optional_string_field<'a>(
    payload: &'a Value,
    key: &str,
) -> Result<Option<&'a str>, String> {
    match payload.get(key) {
        Some(Value::String(value)) => Ok(Some(value.as_str())),
        Some(_) => Err(format!("plugin response field '{}' must be a string", key)),
        None => Ok(None),
    }
}

fn normalize_command_name(name: &str) -> Option<String> {
    let trimmed = name.trim().trim_start_matches('/').trim();
    if trimmed.is_empty() || trimmed.len() > MAX_COMMAND_NAME_LEN {
        None
    } else {
        let lowered = trimmed.to_ascii_lowercase();
        if lowered
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            Some(lowered)
        } else {
            None
        }
    }
}

fn normalize_plugin_id(name: &str) -> Option<String> {
    let lowered = name.trim().to_ascii_lowercase();
    if lowered.is_empty() || lowered.len() > MAX_PLUGIN_ID_LEN {
        return None;
    }

    if lowered
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
    {
        Some(lowered)
    } else {
        None
    }
}

fn validate_command_description(command_name: &str, description: &str) -> Result<(), String> {
    validate_text_field(
        &format!("plugin command description for '/{}'", command_name),
        description,
        MAX_PLUGIN_DESCRIPTION_LEN,
    )
}

fn validate_text_field(field_name: &str, text: &str, max_chars: usize) -> Result<(), String> {
    if text.chars().count() > max_chars {
        return Err(format!(
            "{} is too long (max {} chars)",
            field_name, max_chars
        ));
    }

    if text
        .chars()
        .any(|c| c.is_control() && c != '\n' && c != '\t')
    {
        return Err(format!(
            "{} contains forbidden control characters",
            field_name
        ));
    }

    Ok(())
}

fn truncate_for_error(input: &str, max: usize) -> String {
    if input.len() <= max {
        return input.to_string();
    }
    let mut out = input.chars().take(max).collect::<String>();
    out.push_str("...");
    out
}

fn default_plugin_id_from_spec(spec: &PluginSpec) -> String {
    match spec {
        PluginSpec::Builtin(name) => name.clone(),
        PluginSpec::External(path) => path
            .file_stem()
            .and_then(|v| v.to_str())
            .map(|v| v.to_ascii_lowercase())
            .unwrap_or_else(|| "external-plugin".to_string()),
    }
}

fn deploy_status_icon(status: &str) -> &'static str {
    match status {
        "success" | "succeeded" | "ok" => "[OK]",
        "failed" | "error" => "[FAIL]",
        "rollback" | "rolledback" => "[ROLLBACK]",
        "start" | "started" | "progress" | "in-progress" => "[DEPLOY]",
        _ => "[INFO]",
    }
}

fn plugin_error(msg: impl AsRef<str>) -> Value {
    json!({
        "api_version": PLUGIN_API_VERSION,
        "error": msg.as_ref(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_command_name_accepts_valid_tokens() {
        assert_eq!(
            normalize_command_name("/deploy"),
            Some("deploy".to_string())
        );
        assert_eq!(
            normalize_command_name(" standup "),
            Some("standup".to_string())
        );
        assert_eq!(
            normalize_command_name("poll_v2"),
            Some("poll_v2".to_string())
        );
        assert_eq!(
            normalize_command_name("poll-v2"),
            Some("poll-v2".to_string())
        );
    }

    #[test]
    fn normalize_command_name_rejects_invalid_tokens() {
        assert_eq!(normalize_command_name(""), None);
        assert_eq!(normalize_command_name("/"), None);
        assert_eq!(normalize_command_name("bad command"), None);
        assert_eq!(normalize_command_name("bad$cmd"), None);

        let oversized = "a".repeat(MAX_COMMAND_NAME_LEN + 1);
        assert_eq!(normalize_command_name(&oversized), None);
    }

    #[test]
    fn normalize_plugin_id_rejects_invalid_tokens() {
        assert_eq!(
            normalize_plugin_id("deploy.notifier"),
            Some("deploy.notifier".to_string())
        );
        assert_eq!(normalize_plugin_id("deploy notifier"), None);
        assert_eq!(normalize_plugin_id(""), None);

        let oversized = "a".repeat(MAX_PLUGIN_ID_LEN + 1);
        assert_eq!(normalize_plugin_id(&oversized), None);
    }

    #[test]
    fn builtin_manifests_are_api_v1() {
        for plugin in DEFAULT_BUILTIN_PLUGINS {
            let manifest = builtin_manifest(plugin);
            assert_eq!(
                manifest.get("api_version").and_then(|v| v.as_str()),
                Some(PLUGIN_API_VERSION)
            );
        }
    }

    #[test]
    fn deploy_notifier_hook_blocks_deploy_prefix() {
        let payload = json!({
            "api_version": PLUGIN_API_VERSION,
            "user": "alice",
            "content": "deploy: api success blue-green"
        });

        let response = builtin_message_hook("deploy-notifier", &payload);
        assert_eq!(
            response.get("blocked").and_then(|v| v.as_bool()),
            Some(true)
        );
        assert!(response
            .get("messages")
            .and_then(|v| v.as_array())
            .map(|v| !v.is_empty())
            .unwrap_or(false));
    }

    #[test]
    fn parse_messages_enforces_count_limit() {
        let payload = json!({
            "messages": (0..(MAX_PLUGIN_MESSAGES_PER_RESPONSE + 8))
                .map(|_| json!({"target": "channel", "text": "hello"}))
                .collect::<Vec<Value>>()
        });

        let parsed = parse_messages("demo", &payload).expect("messages should parse");
        assert_eq!(parsed.len(), MAX_PLUGIN_MESSAGES_PER_RESPONSE);
    }

    #[test]
    fn parse_messages_rejects_oversized_text() {
        let long = "x".repeat(MAX_PLUGIN_MESSAGE_LEN + 1);
        let payload = json!({
            "messages": [{"target": "channel", "text": long}]
        });

        assert!(parse_messages("demo", &payload).is_err());
    }

    #[test]
    fn parse_messages_rejects_non_array_payload() {
        let payload = json!({ "messages": "not-an-array" });
        assert!(parse_messages("demo", &payload).is_err());
    }

    #[test]
    fn parse_error_field_rejects_non_string() {
        let payload = json!({ "error": true });
        assert!(parse_error_field(&payload).is_err());
    }

    #[test]
    fn validate_text_field_rejects_forbidden_controls() {
        assert!(validate_text_field("field", "ok\nline", 20).is_ok());
        assert!(validate_text_field("field", "bad\u{0007}", 20).is_err());
    }

    #[test]
    fn validate_manifest_rejects_duplicate_commands() {
        let runtime = PluginRuntime::new(PathBuf::from("chatify-server"));
        let manifest = PluginManifest {
            api_version: PLUGIN_API_VERSION.to_string(),
            name: "safe-plugin".to_string(),
            message_hook: false,
            commands: vec![
                SlashCommandRegistration {
                    name: "deploy".to_string(),
                    description: "first".to_string(),
                },
                SlashCommandRegistration {
                    name: "/deploy".to_string(),
                    description: "second".to_string(),
                },
            ],
        };

        assert!(runtime.validate_manifest(&manifest).is_err());
    }
}

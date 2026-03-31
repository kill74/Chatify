# Chatify

<div align="center">

[![CI](https://github.com/kill74/Chatify/actions/workflows/ci.yml/badge.svg)](https://github.com/kill74/Chatify/actions/workflows/ci.yml)
[![Windows Release Package](https://github.com/kill74/Chatify/actions/workflows/windows-release-package.yml/badge.svg)](https://github.com/kill74/Chatify/actions/workflows/windows-release-package.yml)
[![Release Security Report](https://github.com/kill74/Chatify/actions/workflows/release-security-report.yml/badge.svg)](https://github.com/kill74/Chatify/actions/workflows/release-security-report.yml)
[![Release](https://img.shields.io/github/v/release/kill74/Chatify)](https://github.com/kill74/Chatify/releases)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)

**Terminal-first, self-hosted chat — built in Rust.**

Real-time WebSocket server · Live terminal dashboard · SQLite persistence · Optional Discord bridge

[Quick Start](#quick-start) · [Commands](#client-commands) · [Configuration](#configuration) · [Security](#security-posture) · [Discord Bridge](#discord-bridge-optional)

</div>

---

## What Is Chatify?

Chatify is a self-hosted chat system designed for developers and local teams who want full control over their infrastructure. It ships three binaries: a WebSocket server, a terminal dashboard client, and an optional Discord bridge — all written in Rust with minimal dependencies and predictable runtime behavior.

```text
// CHATIFY // [ONLINE:3] [CHANNELS:2] [EVENTS:18] [UNREAD:1] [TYPING:0] [THEME:retro-grid]
[ROOM:#general] [VOICE:OFF] [TRUST:T2/U1/C0] [STATUS:Online] [CLIENT:CID-ABCD-1234-EF90]

┌GLOBAL_FEED───────────────────────────────────────────────┐  ┌PROFILE──────────────────────────┐
│ [14:31] alice  #general                                  │  │ alice [CID-ABCD-1234-EF90]      │
│   > testing media upload                                 │  │ status:  Online                 │
│ [14:32] [VIDEO] alice shared 'demo.mp4' (12.40 MiB)     │  │ channel: #general               │
│        saved: .../Chatify/media/alice-...-demo.mp4       │  │ voice:   OFF                    │
│ [14:33] IMG alice inline image                           │  └QUICK ACTIONS────────────────────┘
│ @@@@%%%###**++==--::..                                   │  ┌LIVE ROSTER──────────────────────┐
│                                                          │  │ alice · bob · carol             │
└──────────────────────────────────────────────────────────┘  └─────────────────────────────────┘
```

---

## Features

| Category | Capabilities |
|---|---|
| **Messaging** | Multi-channel chat, direct messages, live typing indicators, unread tracking, message editing |
| **Media** | Image & video transfer via chunked protocol, ASCII image preview in terminal, 100 MB cap |
| **Persistence** | SQLite append-only event store, `/history`, `/search`, `/rewind` |
| **Security** | TLS (rustls), PBKDF2 credential hashing, TOTP 2FA + backup codes, replay protection, rate limiting |
| **Trust** | Explicit peer fingerprint workflow, key-change detection, encrypted DM payloads |
| **Discord Bridge** | Bidirectional relay, loop prevention, multi-channel mapping, health telemetry |
| **Releases** | Windows ZIP + installer artifacts, SHA256 checksums, per-release security report |

---

## Quick Start

### Option A — Windows Installer

1. Download `chatify-setup-<version>.exe` from [Releases](https://github.com/kill74/Chatify/releases).
2. Run the installer and open **Chatify Launcher** from the Start Menu.
3. Choose a mode:
   - `1` **Host** — starts the server and a local client on this machine.
   - `2` **Join** — prompts for a remote host/IP and port.

### Option B — Build from Source

**Prerequisites:** Rust toolchain (`cargo`)

```bash
# Build all release binaries
cargo build --release

# Start the server
./target/release/clicord-server --host 0.0.0.0 --port 8765

# Start the client (separate terminal)
./target/release/clicord-client --host 127.0.0.1 --port 8765
```

> **Windows (PowerShell):** replace `./` with `.\` and append `.exe` to binary names.

**Development mode (auto-rebuild):**

```bash
# Terminal 1
cargo run --bin clicord-server

# Terminal 2
cargo run --bin clicord-client -- --host 127.0.0.1 --port 8765
```

**Windows dev helpers (with automatic artifact cleanup):**

```powershell
.\run-server.ps1
.\run-client.ps1 -ProgramArgs @('--host','127.0.0.1','--port','8765')

# Advanced: full control over cleanup budget and retention
.\scripts\run-with-auto-clean.ps1 -Mode server
.\scripts\run-with-auto-clean.ps1 -Mode client -ProgramArgs @('--host','127.0.0.1','--port','8765')
.\scripts\run-with-auto-clean.ps1 -CleanupOnly -MaxAgeDays 2 -MaxTargetSizeGB 3
```

> Rust build artifacts can accumulate quickly. Reclaim space with `cargo clean` or `cargo clean --profile dev`.

---

## Configuration

### Server — `clicord-server`

| Flag | Default | Description |
|---|---|---|
| `--host` | `0.0.0.0` | Bind address |
| `--port` | `8765` | Bind port |
| `--db` | `chatify.db` | SQLite database path |
| `--db-key` | *(auto)* | Hex-encoded 32-byte encryption key (64 hex chars) |
| `--tls` | `false` | Enable TLS (`wss://`) |
| `--tls-cert` | `cert.pem` | TLS certificate path (PEM) |
| `--tls-key` | `key.pem` | TLS private key path (PEM) |
| `--log` | `false` | Enable structured logging |

**DB key resolution order:** `--db-key` flag → `<db>.key` file (auto-generated on first run) → no encryption for `:memory:`.

> **Secret hygiene:** never commit `<db>.key`, `cert.pem`, or `key.pem`. Rotate immediately if exposed.

### Client — `clicord-client`

| Flag | Default | Description |
|---|---|---|
| `--host` | `127.0.0.1` | Server host |
| `--port` | `8765` | Server port |
| `--tls` | `false` | Use `wss://` |
| `--log` | `false` | Enable debug logging |

---

## Client Commands

| Command | Description |
|---|---|
| `/commands [filter]` | Show command palette, optionally filtered |
| `/help [command]` | General help or detailed help for a specific command |
| `/join <channel>` | Join or create a channel |
| `/switch <channel>` | Alias for `/join` |
| `/dm <user> <message>` | Send a direct message |
| `/typing [on\|off] [scope]` | Broadcast typing state to `#channel` or `dm:user` |
| `/image <path>` | Upload an image to the current channel |
| `/video <path>` | Upload a video to the current channel |
| `/me [action]` | Show your profile or send an action-style message |
| `/users` | List online users |
| `/channels` | List available channels |
| `/voice [room]` | Toggle voice in a room |
| `/history [channel] [window]` | Load persisted history for a channel or DM scope |
| `/search <query>` | Search persisted events in the current channel |
| `/replay <timestamp>` | Reconstruct state from an absolute timestamp |
| `/rewind <time> [n]` | Replay events from a relative time window (`15m`, `2h`) |
| `/fingerprint [user]` | Show trust state and key fingerprint(s) |
| `/trust <user> <fingerprint>` | Mark a peer fingerprint as trusted |
| `/edit [#N] <new text>` | Edit your last (or Nth most recent) message |
| `/clear` | Clear terminal output |
| `/quit`, `/exit`, `/q` | Disconnect and exit |

### Media Transfer

Images and videos are sent over a `file_meta` + `file_chunk` chunked protocol. Transfers are capped at **100 MB**.

```text
/image "C:/Users/you/Pictures/screenshot.png"
/video "C:/Users/you/Videos/demo.mp4"
```

Received media is saved locally:
- **Windows:** `%APPDATA%/Chatify/media`
- **Linux/macOS:** `$HOME/.chatify/media`

Image transfers render an **ASCII preview** directly in the terminal feed. Video transfers display a metadata card with sender, filename, size, and saved path.

---

## Persistence & Event Store

Chatify uses an **append-only SQLite event store** — events are inserted, never mutated or deleted.

- Schema is versioned (`schema_meta`) and migrated sequentially on server startup.
- Channel history is indexed on `(channel, ts DESC)`.
- DM history uses route-aware indexes on `(event_type, sender, target, ts DESC)`.
- Encryption at rest covers `payload` and `search_text` columns.
- Contract tests verify history survives restarts and that `/history` and `/search` stay responsive under a 100k-event local dataset.

---

## Identity & Trust

Chatify uses an **explicit peer trust workflow** rather than silent key acceptance:

1. Run `/fingerprint <user>` to inspect a peer's current key material out-of-band.
2. After manual verification, run `/trust <user> <fingerprint>` to mark the key as trusted.

Trust states: `unknown` → `trusted` → `changed` (blocks DM encryption if a peer's key rotates without re-verification). Fingerprints are persisted in a local trust store on the client machine.

---

## Security Posture

**Implemented:**
- TLS support via `rustls`
- PBKDF2 credential hashing (salted, per-user)
- TOTP-based 2FA with backup codes
- Session token lifecycle management
- Replay protection (nonce + timestamp window validation)
- Automatic nonce cache eviction for ghost connections
- Rate limiting on connections and auth attempts
- Input validation and payload size limits
- SQLite parameterized queries with schema no-downgrade policy
- Encryption at rest for message payload and search text

**Known limitations:**
- No certificate pinning on the client
- Session tokens are not persisted across server restarts
- Encrypted search uses linear scan

See [docs/SECURITY_NOTES.md](docs/SECURITY_NOTES.md) for full details and threat model scope.

---

## Windows Packaging

Build a Windows ZIP + installer (requires [Inno Setup 6](https://jrsoftware.org/isinfo.php)):

```powershell
# Build ZIP + installer
.\build-windows-package.ps1

# Build ZIP only (no installer)
.\build-windows-package.ps1 -SkipInstaller

# Specify custom ISCC path
.\build-windows-package.ps1 -IsccPath "C:\Path\To\ISCC.exe"
```

**Generated artifacts:**

| File | Description |
|---|---|
| `dist/chatify-windows-x64.zip` | Binary package |
| `dist/chatify-windows-x64.zip.sha256` | SHA256 checksum |
| `dist/chatify-setup-<version>.exe` | Installer (when available) |
| `dist/chatify-setup-<version>.exe.sha256` | Installer checksum |
| `dist/chatify-windows-x64/chatify-launcher.cmd` | Launch helper |

The [windows-release-package workflow](.github/workflows/windows-release-package.yml) publishes ZIP + installer + checksums automatically on release. A structured security report (`.json` + `.md`) is also attached per release tag via the [release-security-report workflow](.github/workflows/release-security-report.yml).

---

## Discord Bridge (Optional)

The Discord bridge is opt-in behind the `discord-bridge` Cargo feature flag, keeping default builds lean.

```bash
# Build with bridge support
cargo build --release --features discord-bridge

# Run the bridge
cargo run --features discord-bridge --bin discord_bot
```

**Required environment variables:**

```bash
DISCORD_TOKEN=<your-discord-bot-token>
CHATIFY_PASSWORD=<server-password>
```

**Common settings:**

| Variable | Default | Description |
|---|---|---|
| `CHATIFY_HOST` | `127.0.0.1` | Chatify server host |
| `CHATIFY_PORT` | `8765` | Chatify server port |
| `CHATIFY_CHANNEL` | `general` | Default relay channel |
| `CHATIFY_BOT_USERNAME` | `DiscordBot` | Bridge username on Chatify |
| `CHATIFY_WS_SCHEME` | `ws` | `ws` or `wss` |
| `CHATIFY_RECONNECT_BASE_SECS` | `1` | Reconnect backoff base |
| `CHATIFY_RECONNECT_MAX_SECS` | `30` | Reconnect backoff ceiling |
| `CHATIFY_PING_SECS` | `20` | Keepalive interval (`0` to disable) |
| `CHATIFY_HEALTH_LOG_SECS` | `30` | Health telemetry log interval |
| `CHATIFY_DISCORD_CHANNEL_MAP` | — | Inline channel map: `discordId:chatifyChannel,...` |
| `CHATIFY_DISCORD_CHANNEL_MAP_FILE` | `bridge-channel-map.json` | Path to channel map file |
| `CHATIFY_LOG` | — | Set to `1` to enable bridge logs |

**Channel map — inline:**

```bash
CHATIFY_DISCORD_CHANNEL_MAP=123456789012345678:general,987654321098765432:ops
```

**Channel map — file (`bridge-channel-map.json`):**

```json
{
  "routes": [
    { "discord_channel_id": "123456789012345678", "chatify_channel": "general" },
    { "discord_channel_id": "987654321098765432", "chatify_channel": "ops" }
  ]
}
```

Route sources are merged in precedence order: file routes first, then `CHATIFY_DISCORD_CHANNEL_MAP` (overrides matching Discord channel IDs).

**Bridge features:**
- Bidirectional message relay with loop prevention via `relay.markers` metadata
- Discord attachment URL and reply context preserved across the bridge
- Discord outbound relay disables parsed mentions (`@everyone`, `@here`, role/user pings)
- `/bridge status` from Discord or the Rust client reports uptime, route count, and health counters

---

## Development Workflow

```bash
# Type-check all binaries
cargo check --bins

# Format check
cargo fmt --all --check

# Lint (strict)
cargo clippy --all-targets --all-features --locked -- -D warnings

# Core contract tests
cargo test --locked --test message_contracts auth_contract_returns_expected_fields
cargo test --locked --test message_contracts compatibility_contract_client_bootstrap_flow_stays_stable
cargo test --locked --test message_contracts protocol_contract_advertises_backward_compatible_version
cargo test --locked --test message_contracts file_contract_relays_media_metadata_and_chunks

# Full workspace tests
cargo test --workspace --all-targets --locked

# Feature-gated compile checks
cargo check --features discord-bridge --bin discord_bot --locked
cargo check --features bridge-client --bin clicord-client --locked
```

---

## Project Docs

- [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)
- [docs/BENCHMARKS.md](docs/BENCHMARKS.md)
- [docs/ENGINEERING_CASE_STUDY.md](docs/ENGINEERING_CASE_STUDY.md)
- [docs/SECURITY_NOTES.md](docs/SECURITY_NOTES.md)
- [docs/UNIQUE_ROADMAP.md](docs/UNIQUE_ROADMAP.md)
- [CHANGELOG.md](CHANGELOG.md)

---

## Contributing

Contributions are welcome. See [CONTRIBUTING.md](CONTRIBUTING.md) for setup, quality gates, and the PR checklist.

## License

MIT — see [LICENSE](LICENSE).

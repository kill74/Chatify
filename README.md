# Chatify

[![CI](https://github.com/kill74/Chatify/actions/workflows/ci.yml/badge.svg)](https://github.com/kill74/Chatify/actions/workflows/ci.yml)
[![Windows Release Package](https://github.com/kill74/Chatify/actions/workflows/windows-release-package.yml/badge.svg)](https://github.com/kill74/Chatify/actions/workflows/windows-release-package.yml)
[![Release](https://img.shields.io/github/v/release/kill74/Chatify)](https://github.com/kill74/Chatify/releases)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)

Terminal-first, self-hosted chat built with Rust.

Chatify provides a lightweight WebSocket server, a fast terminal client, and an optional Discord bridge. The project is designed for local teams, controlled environments, and developers who want clear behavior, low runtime overhead, and readable code paths.

## Why This Project Exists

Most chat projects optimize for UI layers first and protocol clarity second.
Chatify intentionally does the opposite:

- Protocol and message contracts are explicit and testable.
- Runtime behavior is easy to trace in server/client code paths.
- Feature growth is controlled by quality gates and release discipline.

This project is intended to demonstrate engineering judgment, not just feature output.

## Recruiter Snapshot

- Language and stack: Rust, WebSocket transport, SQLite persistence.
- Delivery model: command-line binaries for server and client, optional Discord bridge.
- Reliability posture: CI with formatting, lint, tests, and feature-specific regressions.
- Release posture: automated Windows release packaging with checksums.
- Documentation posture: architecture, roadmap, security notes, benchmark methodology, and case study.

## Results Snapshot

This section is intentionally short so reviewers can validate outcomes quickly.

- Latest release: `v0.1.0`
- CI posture: passing checks required for merge on `main`
- Delivery artifacts: Windows ZIP package with SHA256 checksum on published releases
- Performance baseline: benchmark process documented in [docs/BENCHMARKS.md](docs/BENCHMARKS.md); measured values to be published in next benchmark pass
- Test posture: protocol contracts, bridge reconnection regressions, and full workspace test suite in CI

See supporting evidence:

- [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)
- [docs/UNIQUE_ROADMAP.md](docs/UNIQUE_ROADMAP.md)
- [docs/SECURITY_NOTES.md](docs/SECURITY_NOTES.md)
- [docs/BENCHMARKS.md](docs/BENCHMARKS.md)
- [docs/ENGINEERING_CASE_STUDY.md](docs/ENGINEERING_CASE_STUDY.md)

## Table of Contents

- [Core Capabilities](#core-capabilities)
- [Why This Project Exists](#why-this-project-exists)
- [Recruiter Snapshot](#recruiter-snapshot)
- [Results Snapshot](#results-snapshot)
- [Project Status](#project-status)
- [Quick Start](#quick-start)
- [Shipping Binaries](#shipping-binaries)
- [Configuration](#configuration)
- [Client Command Reference](#client-command-reference)
- [Persistence Model](#persistence-model)
- [Discord Bridge (Optional)](#discord-bridge-optional)
- [Architecture](#architecture)
- [Development Workflow](#development-workflow)
- [Troubleshooting](#troubleshooting)
- [Security Posture](#security-posture)
- [Recruiter Ready Checklist](#recruiter-ready-checklist)
- [Changelog](#changelog)
- [Roadmap](#roadmap)
- [Contributing](#contributing)
- [License](#license)

## Core Capabilities

- Multi-channel chat and direct messages
- Presence/status updates and channel/user discovery
- SQLite-backed event persistence with `history`, `search`, and `rewind`
- Terminal-first command workflow with low setup friction
- Optional Discord bridge behind a Cargo feature flag

## Project Status

Chatify is actively maintained and still evolving.

- Core server/client flows are stable for local and controlled deployments.
- Auth/protocol hardening and bridge reliability are actively improving.
- Some commands are intentionally incomplete (for example, `/edit`).

## Quick Start

### 1. Build release binaries

```bash
cargo build --release
```

### 2. Start server

Windows (PowerShell):

```powershell
.\target\release\clicord-server.exe --host 0.0.0.0 --port 8765
```

Linux/macOS:

```bash
./target/release/clicord-server --host 0.0.0.0 --port 8765
```

### 3. Start client

Windows (PowerShell):

```powershell
.\target\release\clicord-client.exe --host 127.0.0.1 --port 8765
```

Linux/macOS:

```bash
./target/release/clicord-client --host 127.0.0.1 --port 8765
```

### Development mode

Terminal 1:

```bash
cargo run --bin clicord-server
```

Terminal 2:

```bash
cargo run --bin clicord-client -- --host 127.0.0.1 --port 8765
```

## Shipping Binaries

Configured binaries:

- `clicord-server`
- `clicord-client`
- `discord_bot` (feature-gated)

### Windows executable package

Build a shareable ZIP with launchers:

```powershell
.\build-windows-package.ps1
```

Generated artifacts:

- `dist/chatify-windows-x64.zip`
- `dist/chatify-windows-x64.zip.sha256`
- `dist/chatify-windows-x64/start-chatify.bat`
- `dist/chatify-windows-x64/start-server.bat`
- `dist/chatify-windows-x64/start-client.bat`

Optional checksum verification (PowerShell):

```powershell
$actual = (Get-FileHash .\dist\chatify-windows-x64.zip -Algorithm SHA256).Hash.ToLower()
$expected = (Get-Content .\dist\chatify-windows-x64.zip.sha256).Split(' ')[0].ToLower()
$actual -eq $expected
```

Release automation:

- On published releases, [.github/workflows/windows-release-package.yml](.github/workflows/windows-release-package.yml) builds and uploads the ZIP plus SHA256 file.

## Configuration

### Server (`clicord-server`)

| Flag     | Default      | Description          |
| -------- | ------------ | -------------------- |
| `--host` | `0.0.0.0`    | Bind address         |
| `--port` | `8765`       | Bind port            |
| `--db`   | `chatify.db` | SQLite database path |
| `--log`  | `false`      | Enable logging       |

### Client (`clicord-client`)

| Flag     | Default     | Description          |
| -------- | ----------- | -------------------- |
| `--host` | `127.0.0.1` | Server host          |
| `--port` | `8765`      | Server port          |
| `--tls`  | `false`     | Use `wss://`         |
| `--log`  | `false`     | Enable debug logging |

## Client Command Reference

| Command                | Description                                             |
| ---------------------- | ------------------------------------------------------- |
| `/join <channel>`      | Join or create a channel                                |
| `/dm <user> <message>` | Send direct message                                     |
| `/me <action>`         | Send action-style message                               |
| `/users`               | List online users                                       |
| `/channels`            | List channels                                           |
| `/voice [room]`        | Toggle voice in room                                    |
| `/history [limit]`     | Load persisted channel history                          |
| `/search <query>`      | Search persisted events in current channel              |
| `/rewind <time> [n]`   | Replay events from a time window (example: `15m`, `2h`) |
| `/edit <text>`         | Placeholder command                                     |
| `/clear`               | Clear terminal output                                   |
| `/help`                | Show command help                                       |
| `/quit`, `/exit`, `/q` | Disconnect and exit                                     |

## Persistence Model

Chatify persists events in SQLite using server flag `--db` (default: `chatify.db`).

Persisted/replayed flows:

- Channel messages and system events
- Channel search via `/search`
- History replay via `/history`
- Time-window replay via `/rewind`

Schema metadata and migrations run as part of server startup.

## Discord Bridge (Optional)

The bridge is opt-in to keep default builds lean.

Run the bridge:

```bash
cargo run --features discord-bridge --bin discord_bot
```

Bridge environment variables:

- `DISCORD_TOKEN` (required)
- `CHATIFY_PASSWORD` (required)
- `CHATIFY_HOST` (default: `127.0.0.1`)
- `CHATIFY_PORT` (default: `8765`)
- `CHATIFY_CHANNEL` (default: `general`)
- `CHATIFY_BOT_USERNAME` (default: `DiscordBot`)
- `CHATIFY_WS_SCHEME` (`ws` or `wss`, default: `ws`)
- `CHATIFY_AUTH_TIMEOUT_SECS` (default: `15`)
- `CHATIFY_RECONNECT_BASE_SECS` (default: `1`)
- `CHATIFY_RECONNECT_MAX_SECS` (default: `30`)
- `CHATIFY_HEALTH_LOG_SECS` (health log interval, default: `30`)
- `CHATIFY_BRIDGE_INSTANCE_ID` (optional stable bridge source id)
- `CHATIFY_DISCORD_CHANNEL_MAP` (optional map: `discordChannelId:chatifyChannel,...`)
- `CHATIFY_LOG` (set to `1` to enable logging)

Example channel map:

```text
CHATIFY_DISCORD_CHANNEL_MAP=123456789012345678:general,987654321098765432:ops
```

## Architecture

```text
.
├── Cargo.toml
├── src/
│   ├── main.rs        # server
│   ├── client.rs      # terminal client
│   ├── crypto.rs      # crypto helpers
│   ├── discord_bot.rs # optional bridge binary source
│   └── lib.rs
├── tests/
└── docs/
```

Detailed documents:

- [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)
- [docs/UNIQUE_ROADMAP.md](docs/UNIQUE_ROADMAP.md)

## Development Workflow

Recommended quality checks:

```bash
cargo check --bins
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all
cargo test --features discord-bridge --bin discord_bot bridge_supervisor_reconnects_after_disconnect
cargo test --features discord-bridge --bin discord_bot parse_channel_map_filters_and_normalizes_entries
cargo test --features discord-bridge --bin discord_bot self_source_filter_matches_only_non_empty_identical_source
cargo test --features discord-bridge --bin discord_bot self_source_filter_ignores_non_string_src_values
```

Auto-format:

```bash
cargo fmt
```

## Troubleshooting

### Connection refused

- Confirm server process is running.
- Confirm host and port match server configuration.
- Confirm local/network firewall rules allow the port.

### Auth or handshake errors

- Confirm server and client binaries are from compatible commits.
- Rebuild all binaries after protocol changes.

### Command appears inactive

- `/edit` is a placeholder and not complete yet.

### Decryption or message-format confusion

- Treat current crypto flow as experimental.
- Verify both peers run compatible runtime/protocol expectations.

## Security Posture

Chatify includes encryption helpers and protocol checks, but it is not yet a fully production-hardened secure messaging platform.

Known limitations include:

- Minimal authentication model
- Ongoing protocol hardening
- Pending full security review

Use in development, learning, or controlled environments unless you complete independent threat modeling and hardening.

Security design details and scope boundaries:

- [docs/SECURITY_NOTES.md](docs/SECURITY_NOTES.md)

## Recruiter Ready Checklist

This list is intentionally concrete so reviewers can verify engineering maturity quickly.

- [x] CI checks for format, lint, tests, and bridge feature compile path.
- [x] Release automation for Windows binary package with SHA256 artifact.
- [x] Architecture and roadmap docs linked from README.
- [x] Security scope and limitations documented.
- [x] Benchmark methodology and reporting template documented.
- [x] Engineering case study with trade-off analysis documented.

## Changelog

- [CHANGELOG.md](CHANGELOG.md)

## Roadmap

Near-term priorities:

- Complete `/edit` end-to-end behavior
- Continue auth and protocol hardening
- Expand integration and contract-level test coverage
- Improve bridge operational readiness and observability

## Contributing

Contributions are welcome.

By submitting a contribution, you agree that your code is licensed under the MIT License.

See [CONTRIBUTING.md](CONTRIBUTING.md) for setup, quality gates, and PR checklist.

## License

MIT. See [LICENSE](LICENSE).

# Chatify

Terminal-first, self-hosted chat built with Rust.

Chatify provides a lightweight WebSocket server and a terminal client focused on fast startup, low overhead, and practical team workflows.

## Table of Contents

- [Highlights](#highlights)
- [Project Status](#project-status)
- [Quick Start](#quick-start)
- [Binaries](#binaries)
- [Configuration](#configuration)
- [Client Commands](#client-commands)
- [Persistence and History](#persistence-and-history)
- [Discord Bridge (Optional)](#discord-bridge-optional)
- [Architecture](#architecture)
- [Development](#development)
- [Troubleshooting](#troubleshooting)
- [Security Notes](#security-notes)
- [Roadmap](#roadmap)
- [Contributing](#contributing)
- [License](#license)

## Highlights

- Multi-channel chat and direct messages
- Presence/status updates and user/channel listing
- SQLite-backed event persistence (`history`, `search`, `rewind`)
- Terminal-friendly command workflow
- Optional Discord bridge behind a Cargo feature flag

## Project Status

Chatify is actively evolving and should be considered experimental.

- Core chat flow is stable for local and controlled environments.
- Runtime validation and baseline hardening are in place.
- Some features are intentionally incomplete (for example, `/edit`).

## Quick Start

### 1. Build

```bash
cargo build --release
```

### 2. Run server

Windows (PowerShell):

```powershell
.\target\release\clicord-server.exe --host 0.0.0.0 --port 8765
```

Linux/macOS:

```bash
./target/release/clicord-server --host 0.0.0.0 --port 8765
```

### 3. Run client

Windows (PowerShell):

```powershell
.\target\release\clicord-client.exe --host 127.0.0.1 --port 8765
```

Linux/macOS:

```bash
./target/release/clicord-client --host 127.0.0.1 --port 8765
```

### Dev mode (no release build)

Terminal 1:

```bash
cargo run --bin clicord-server
```

Terminal 2:

```bash
cargo run --bin clicord-client -- --host 127.0.0.1 --port 8765
```

## Binaries

Configured Cargo binaries:

- `clicord-server`
- `clicord-client`
- `discord_bot` (feature-gated)

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

## Client Commands

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

## Persistence and History

Chatify persists events in SQLite using the server `--db` path (default: `chatify.db`).

Supported persisted/replayed flows include:

- Channel messages and system events
- Search within current channel (`/search`)
- History replay with cap (`/history`)
- Time-window rewind (`/rewind`)

Schema metadata and migrations are built into the server startup path.

## Discord Bridge (Optional)

The bridge is opt-in to keep default builds lean.

Run with feature:

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
- `CHATIFY_LOG` (set to `1` to enable logging)

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

Detailed docs:

- [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)
- [docs/UNIQUE_ROADMAP.md](docs/UNIQUE_ROADMAP.md)

## Development

Quality checks:

```bash
cargo check --bins
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all
```

Auto-format:

```bash
cargo fmt
```

## Troubleshooting

### Connection refused

- Verify server is running.
- Verify host and port values match.
- Verify firewall rules on the target machine.

### Auth or handshake problems

- Ensure server and client are built from compatible commits.
- Rebuild binaries after protocol changes.

### Commands appear inactive

- `/edit` is currently a placeholder.

### Crypto/decryption confusion

- Treat current crypto paths as experimental.
- Validate that both peers share the same runtime expectations.

## Security Notes

Encryption helpers exist, but the project is not production-grade end-to-end secure yet.

Current gaps include:

- Minimal authentication model
- Ongoing protocol hardening

Use Chatify for development, learning, and controlled environments unless you complete a dedicated security review and hardening pass.

## Roadmap

Near-term priorities:

- Complete `/edit` end-to-end behavior
- Continue auth and protocol hardening
- Expand integration and contract-level testing
- Improve bridge operational readiness

## Contributing

Contributions are welcome.

See [CONTRIBUTING.md](CONTRIBUTING.md) for setup, quality gates, and PR checklist.

## License

GPL v3. See [LICENSE](LICENSE).

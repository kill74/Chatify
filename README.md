# Chatify

[![CI](https://github.com/kill74/Chatify/actions/workflows/ci.yml/badge.svg)](https://github.com/kill74/Chatify/actions/workflows/ci.yml)
[![Windows Release Package](https://github.com/kill74/Chatify/actions/workflows/windows-release-package.yml/badge.svg)](https://github.com/kill74/Chatify/actions/workflows/windows-release-package.yml)
[![Release Security Report](https://github.com/kill74/Chatify/actions/workflows/release-security-report.yml/badge.svg)](https://github.com/kill74/Chatify/actions/workflows/release-security-report.yml)
[![Release](https://img.shields.io/github/v/release/kill74/Chatify)](https://github.com/kill74/Chatify/releases)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)

Terminal-first, self-hosted chat built with Rust, with a real-time dashboard UX in the CLI.

Chatify ships three binaries:

- `clicord-server`: WebSocket chat server with SQLite persistence.
- `clicord-client`: terminal dashboard client for channels, DM, typing, trust verification, media transfer, history, search, and rewind.
- `discord_bot` (optional feature): Discord <-> Chatify bridge.

## Highlights

- Multi-channel chat + direct messages
- Live typing indicators and unread activity tracking
- In-terminal dashboard UI with grouped feed and operational side panels
- Image/video transfer in channels with in-app rendering metadata and image ASCII preview
- SQLite persistence (`history`, `search`, `rewind`)
- Append-only event store with versioned schema migrations
- Explicit peer trust workflow (`/fingerprint`, `/trust`) with key-change detection
- Credential hardening (PBKDF2), rate limiting, 2FA (TOTP + backup codes)
- Replay protection (nonce + timestamp validation)
- Optional Discord bridge with mapped bidirectional relay, loop prevention, and health telemetry
- Windows release artifacts with SHA256 checksums (ZIP + installer)
- Structured security test report attached per release tag (`.json` + `.md`)

## Quick Start

### Option A: Windows installer (.exe)

1. Download `chatify-setup-<version>.exe` from [Releases](https://github.com/kill74/Chatify/releases).
2. Install Chatify.
3. Open **Chatify Launcher** from Start Menu.
4. Choose one mode:
   - `1` Host on this machine: starts server + local client.
   - `2` Join existing server: prompts for host/IP and port.

### Option B: Build from source

Build release binaries:

```bash
cargo build --release
```

Start server:

```powershell
.\target\release\clicord-server.exe --host 0.0.0.0 --port 8765
```

Start client:

```powershell
.\target\release\clicord-client.exe --host 127.0.0.1 --port 8765
```

Linux/macOS equivalents:

```bash
./target/release/clicord-server --host 0.0.0.0 --port 8765
./target/release/clicord-client --host 127.0.0.1 --port 8765
```

Development mode:

```bash
cargo run --bin clicord-server
cargo run --bin clicord-client -- --host 127.0.0.1 --port 8765
```

## Windows Packaging (Maintainers)

Build ZIP + installer (if Inno Setup 6 is available):

```powershell
.\build-windows-package.ps1
```

Build ZIP only:

```powershell
.\build-windows-package.ps1 -SkipInstaller
```

If `ISCC.exe` is not in PATH, pass it explicitly:

```powershell
.\build-windows-package.ps1 -IsccPath "C:\Path\To\ISCC.exe"
```

Generated artifacts:

- `dist/chatify-windows-x64.zip`
- `dist/chatify-windows-x64.zip.sha256`
- `dist/chatify-setup-<version>.exe` (when installer is generated)
- `dist/chatify-setup-<version>.exe.sha256` (when installer is generated)
- `dist/chatify-windows-x64/chatify-launcher.cmd`

Release pipeline:

- [windows-release-package workflow](.github/workflows/windows-release-package.yml) publishes ZIP + installer + checksums on release.
- [release-security-report workflow](.github/workflows/release-security-report.yml) publishes a structured security report per release tag.

## Event Store Design

- Write path is append-only: events are inserted, never updated/deleted.
- SQLite schema is versioned (`schema_meta`) and upgraded sequentially on startup.
- History lookups use channel + timestamp indexes (`channel`, `ts DESC`).
- DM lookups use route-aware indexes (`event_type`, `sender`, `target`, `ts DESC`).
- Contract tests verify history survives restart and that history/search remain responsive on a 100k local-event dataset.

## Configuration

### Server (`clicord-server`)

| Flag         | Default      | Description                            |
| ------------ | ------------ | -------------------------------------- |
| `--host`     | `0.0.0.0`    | Bind address                           |
| `--port`     | `8765`       | Bind port                              |
| `--db`       | `chatify.db` | SQLite database path                   |
| `--db-key`   | (auto)       | Hex-encoded 32-byte key (64 hex chars) |
| `--tls`      | `false`      | Enable TLS (`wss://`)                  |
| `--tls-cert` | `cert.pem`   | TLS certificate path (PEM)             |
| `--tls-key`  | `key.pem`    | TLS private key path (PEM)             |
| `--log`      | `false`      | Enable logging                         |

DB key resolution order:

1. `--db-key`
2. `<db>.key` auto-generated on first run
3. No encryption for `:memory:`

Secret hygiene:

- Never commit `<db>.key`.
- Rotate keys immediately if exposed.
- Keep runtime secrets out of VCS (`*.db.key`, `cert.pem`, `key.pem`).

### Client (`clicord-client`)

| Flag     | Default     | Description          |
| -------- | ----------- | -------------------- |
| `--host` | `127.0.0.1` | Server host          |
| `--port` | `8765`      | Server port          |
| `--tls`  | `false`     | Use `wss://`         |
| `--log`  | `false`     | Enable debug logging |

## Client Commands

| Command                       | Description                                             |
| ----------------------------- | ------------------------------------------------------- |
| `/commands [filter]`          | Show command palette (optionally filtered)              |
| `/help [command]`             | Show general help or detailed help for one command      |
| `/join <channel>`             | Join or create a channel                                |
| `/switch <channel>`           | Alias for `/join` to switch active channel              |
| `/dm <user> <message>`        | Send direct message                                     |
| `/typing [on\|off] [scope]`   | Broadcast typing state to `#channel` or `dm:user`       |
| `/image <path>`               | Upload an image to the current channel                  |
| `/video <path>`               | Upload a video to the current channel                   |
| `/me [action]`                | Show profile or send action-style message               |
| `/users`                      | List online users                                       |
| `/channels`                   | List channels                                           |
| `/voice [room]`               | Toggle voice in room                                    |
| `/history [channel] [window]` | Load persisted history for a channel or DM scope        |
| `/search <query>`             | Search persisted events in current channel              |
| `/replay <timestamp>`         | Reconstruct state from an absolute timestamp            |
| `/rewind <time> [n]`          | Replay events from a time window (example: `15m`, `2h`) |
| `/fingerprint [user]`         | Show trust state and key fingerprint(s)                 |
| `/trust <user> <fingerprint>` | Mark a peer key fingerprint as trusted                  |
| `/clear`                      | Clear terminal output                                   |
| `/help`                       | Alias for `/help [command]`                             |
| `/quit`, `/exit`, `/q`        | Disconnect and exit                                     |

Media transfer behavior:

- Images and videos are sent through `file_meta` + `file_chunk` protocol events.
- Transfer size is capped at 100 MB per file.
- Media is uploaded in chunked frames to keep protocol behavior stable under load.
- Received media is stored locally in:
  - Windows: `%APPDATA%/Chatify/media`
  - Linux/macOS: `$HOME/.chatify/media`
- Image transfers render an ASCII preview directly in the terminal feed.
- Video transfers show metadata cards in the feed and the saved local path.

Media quick usage:

```text
/image "C:/Users/you/Pictures/screenshot.png"
/video "C:/Users/you/Videos/demo.mp4"
```

Receiver-side behavior:

- The receiver gets a media card in the feed with sender, filename, size, and saved local path.
- Image transfers additionally show an ASCII preview in the feed itself.

## CLI Dashboard UX

The client UI is a live dashboard rendered directly in the terminal with:

- Header chips (`ONLINE`, `CHANNELS`, `UNREAD`, `TYPING`, `THEME`, `CLIENT`)
- Grouped global feed on the left (message streams + media cards/previews)
- Operational panels on the right (`PROFILE`, `QUICK ACTIONS`, `LIVE ROSTER`, `CHANNEL DOCK`)
- Compact mode fallback when terminal size is small

Example dashboard layout:

```text
// CHATIFY // [ONLINE:3] [CHANNELS:2] [EVENTS:18] [UNREAD:1] [TYPING:0] [THEME:retro-grid]
[ROOM:#general] [VOICE:OFF] [TRUST:T2/U1/C0] [STATUS:Online] [LIVE:alice@#general] [CLIENT:CID-ABCD-1234-EF90]

┌GLOBAL_FEED──────────────────────────────────────────────┐   ┌PROFILE───────────────────────────┐
│[14:31] alice  #general                                  │   │alice [CID-ABCD-1234-EF90]       │
│  > oi time, testando upload de mídia                    │   │status: Online                    │
│[14:32] [VIDEO] alice shared 'demo.mp4' (12.40 MiB)     │   │channel: #general                 │
│      saved: .../Chatify/media/alice-...-demo.mp4       │   │voice: OFF                        │
│[14:33] IMG alice inline image                           │   │typing: none                      │
│@@@@%%%###**++==--::..                                   │   └QUICK ACTIONS────────────────────┘
└──────────────────────────────────────────────────────────┘
```

Notes:

- `/edit` is currently a placeholder command.

## Identity and Trust UX

- Trust states are explicit: `unknown`, `trusted`, `changed`.
- Peer fingerprints are persisted in a local trust store on the client machine.
- Key rotation is never silent: if a peer key changes, state becomes `changed` and DM encryption/decryption is blocked until you explicitly trust the new fingerprint.
- Recommended flow: run `/fingerprint <user>` to verify current key material out of band.
- After manual verification, run `/trust <user> <fingerprint>`.

## Discord Bridge (Optional)

Bridge builds are pinned for stability (`serenity = =0.11.7`) and remain optional behind the `discord-bridge` feature flag.

Run bridge binary:

```bash
cargo run --features discord-bridge --bin discord_bot
```

Required environment variables:

- `DISCORD_TOKEN`
- `CHATIFY_PASSWORD`

Common bridge settings:

- `CHATIFY_HOST` (`127.0.0.1`)
- `CHATIFY_PORT` (`8765`)
- `CHATIFY_CHANNEL` (`general`)
- `CHATIFY_BOT_USERNAME` (`DiscordBot`)
- `CHATIFY_WS_SCHEME` (`ws` or `wss`)
- `CHATIFY_AUTH_TIMEOUT_SECS` (`15`)
- `CHATIFY_RECONNECT_BASE_SECS` (`1`)
- `CHATIFY_RECONNECT_MAX_SECS` (`30`)
- `CHATIFY_RECONNECT_JITTER_PCT` (`20`)
- `CHATIFY_RECONNECT_WARN_THRESHOLD` (`5`)
- `CHATIFY_PING_SECS` (`20`, use `0` to disable)
- `CHATIFY_HEALTH_LOG_SECS` (`30`)
- `CHATIFY_BRIDGE_INSTANCE_ID` (optional)
- `CHATIFY_DISCORD_CHANNEL_MAP_FILE` (optional path, default `bridge-channel-map.json` when present)
- `CHATIFY_DISCORD_CHANNEL_MAP` (optional `discordChannelId:chatifyChannel,...`)
- `CHATIFY_LOG=1` enables bridge logs

Route sources are merged in this precedence order:

1. File routes from `CHATIFY_DISCORD_CHANNEL_MAP_FILE` (or `bridge-channel-map.json` by default)
2. Environment routes from `CHATIFY_DISCORD_CHANNEL_MAP` (override file entries with same Discord channel id)

Channel map example:

```text
CHATIFY_DISCORD_CHANNEL_MAP=123456789012345678:general,987654321098765432:ops
```

Route file example (`bridge-channel-map.json`):

```json
{
  "routes": [
    {
      "discord_channel_id": "123456789012345678",
      "chatify_channel": "general"
    },
    { "discord_channel_id": "987654321098765432", "chatify_channel": "ops" }
  ]
}
```

The repository includes a safe template at [bridge-channel-map.json](bridge-channel-map.json) with empty routes; fill it with your real Discord channel IDs before enabling bidirectional relay.

Bridge health command:

- Send `/bridge status` in Discord and the bot replies with current bridge health counters.

Loop prevention:

- Discord ingress frames include `src` and `relay.markers` metadata.
- Relay to Discord is skipped when the destination marker already exists, preventing normal-operation loops.

Attachments and replies:

- Discord -> Chatify relay preserves attachment URL/metadata and basic reply context.
- Chatify -> Discord relay consumes preserved metadata and emits attachment/reply context in bridged messages.

Bridge hardening:

- Channel relay keys now follow the same derivation contract as the Rust client (avoids decrypt mismatch between client and bridge).
- Discord outbound relay disables parsed mentions (`@everyone`, `@here`, role/user pings) to avoid cross-platform mention amplification.

## Security Posture

Implemented:

- TLS support with rustls
- Credential hashing (PBKDF2, salted, per-user)
- Rate limiting (connections and auth attempts)
- 2FA with TOTP + backup codes
- Session token lifecycle
- Replay protection (nonce + timestamp window)
- Input validation and size limits
- SQLite parameterized queries + schema no-downgrade policy
- Encryption at rest for `payload` and `search_text`

Known limitations:

- No certificate pinning on client
- Session tokens are not persisted across server restarts
- Encrypted search uses linear scan

For details, see [docs/SECURITY_NOTES.md](docs/SECURITY_NOTES.md).

## Development Workflow

Recommended checks:

```bash
cargo check --bins
cargo fmt --all --check
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo test --locked --test message_contracts auth_contract_returns_expected_fields
cargo test --locked --test message_contracts compatibility_contract_client_bootstrap_flow_stays_stable
cargo test --locked --test message_contracts protocol_contract_advertises_backward_compatible_version
cargo test --locked --test message_contracts file_contract_relays_media_metadata_and_chunks
cargo test --workspace --all-targets --locked
cargo check --features discord-bridge --bin discord_bot --locked
cargo test --features discord-bridge --bin discord_bot
```

## Project Docs

- [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)
- [docs/BENCHMARKS.md](docs/BENCHMARKS.md)
- [docs/ENGINEERING_CASE_STUDY.md](docs/ENGINEERING_CASE_STUDY.md)
- [docs/UNIQUE_ROADMAP.md](docs/UNIQUE_ROADMAP.md)
- [CHANGELOG.md](CHANGELOG.md)

## Contributing

Contributions are welcome. See [CONTRIBUTING.md](CONTRIBUTING.md).

## License

MIT. See [LICENSE](LICENSE).

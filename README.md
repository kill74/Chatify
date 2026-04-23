# Chatify

[![CI](https://github.com/kill74/Chatify/actions/workflows/ci.yml/badge.svg)](https://github.com/kill74/Chatify/actions/workflows/ci.yml)
[![CodeQL](https://github.com/kill74/Chatify/actions/workflows/codeql.yml/badge.svg)](https://github.com/kill74/Chatify/actions/workflows/codeql.yml)
[![Windows Release Package](https://github.com/kill74/Chatify/actions/workflows/windows-release-package.yml/badge.svg)](https://github.com/kill74/Chatify/actions/workflows/windows-release-package.yml)
[![Release Security Report](https://github.com/kill74/Chatify/actions/workflows/release-security-report.yml/badge.svg)](https://github.com/kill74/Chatify/actions/workflows/release-security-report.yml)
[![Release](https://img.shields.io/github/v/release/kill74/Chatify)](https://github.com/kill74/Chatify/releases)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)

Self-hosted, terminal-native chat server written in Rust. Ships a WebSocket server, a terminal dashboard client, and an optional Discord relay bridge. Designed for controlled deployments where operational transparency and protocol correctness matter more than UI polish.

## 30-Second Engineering Signal

![Chatify terminal dashboard](docs/assets/chatify-dashboard.png)

**Quick demo:** [Run the two-terminal local walkthrough](docs/DEMO.md), or skim the recorded terminal flow below.

![Chatify local terminal demo](docs/assets/chatify-demo.gif)

| Signal | Evidence |
| ------ | -------- |
| Async Rust system | WebSocket server, terminal client, and feature-gated Discord bridge |
| Protocol correctness | [Contract tests](tests/message_contracts.rs) for auth, bootstrap compatibility, versioning, and media transfer |
| Persistence discipline | Append-only SQLite event store with schema no-downgrade policy |
| Performance evidence | [100k-event history/search baseline](docs/BENCHMARKS.md) backed by a contract test |
| Engineering judgment | [Case study](docs/ENGINEERING_CASE_STUDY.md) and [v0.3.0 release notes](https://github.com/kill74/Chatify/releases/tag/v0.3.0) show tradeoffs and shipped increments |
| Operational maturity | [CI gates](.github/workflows/ci.yml), [CodeQL scanning](.github/workflows/codeql.yml), [Windows packaging](.github/workflows/windows-release-package.yml), checksums, and [release security reports](.github/workflows/release-security-report.yml) |
| Honest security posture | [Security entry point](SECURITY.md), documented limits, and no unaudited production-security overclaiming |

---

## Table of Contents

- [30-Second Engineering Signal](#30-second-engineering-signal)
- [Design Rationale](#design-rationale)
- [System Overview](#system-overview)
- [Quick Start](#quick-start)
- [Configuration Reference](#configuration-reference)
- [Client Commands](#client-commands)
- [Persistence Model](#persistence-model)
- [Trust & Identity Model](#trust--identity-model)
- [Security Posture](#security-posture)
- [Discord Bridge](#discord-bridge)
- [Windows Packaging](#windows-packaging)
- [Development & CI](#development--ci)
- [Known Limitations](#known-limitations)
- [Project Docs](#project-docs)
- [Contributing](#contributing)

---

## Design Rationale

Most self-hosted chat systems are web-first and treat the protocol as a second-class concern. Chatify takes the opposite approach: the message contract is the specification, and the UI is a thin layer on top.

**Key decisions and their tradeoffs:**

**Append-only SQLite event store.** All state is derived from an ordered event log. This simplifies correctness reasoning — no partial updates, no in-place mutation — and makes history replay fully deterministic. The tradeoff is that search requires a linear scan over encrypted payloads, which is acceptable at single-tenant scale but not suitable for high-volume deployments without an external index.

**Chunked binary protocol over WebSocket.** Media transfer uses `file_meta` + `file_chunk` framing rather than HTTP multipart. This keeps the transport uniform: a single WebSocket connection handles all message types, eliminating the need for a separate file server or a second authenticated channel. The tradeoff is that the server holds chunk reassembly state in memory per active transfer, and the 100 MB cap is enforced at the application layer with no backpressure to the sender.

**Explicit peer trust rather than TOFU.** Key fingerprints require out-of-band verification and an explicit `/trust` confirmation. Key rotation transitions trust state to `changed` and blocks DM encryption until re-verified. This is intentionally stricter than Trust On First Use and prevents silent MITM via key rotation at the cost of operational friction on legitimate rotations.

**Discord bridge behind a Cargo feature flag.** Keeping the bridge opt-in (`--features discord-bridge`) avoids pulling Discord SDK dependencies into the default build surface and keeps the default binary footprint minimal.

---

## System Overview

```text
┌─────────────────────────────────────────────────────────────┐
│                        chatify-server                        │
│                                                              │
│  WebSocket listener → Auth / rate-limit → Message router     │
│         ↓                                                    │
│  Event store (SQLite, append-only, encrypted payload col)    │
│         ↓                                                    │
│  Broadcast fanout → connected client sessions                │
└────────────────────────┬─────────────────────────────────────┘
                         │ ws:// or wss://
          ┌──────────────┴──────────────┐
          │                             │
  chatify-client                  discord_bot
   (terminal dashboard)        (optional bridge)
                                        │
                                 Discord API (serenity)
```

| Binary           | Purpose                                                  |
| ---------------- | -------------------------------------------------------- |
| `chatify-server` | WebSocket server, event persistence, auth, rate limiting |
| `chatify-client` | Terminal dashboard: channels, DMs, media, search, trust  |
| `discord_bot`    | Discord ↔ Chatify relay bridge (feature-gated)           |

---

## Quick Start

### Windows — Installer

1. Download `chatify-setup-<version>.exe` from [Releases](https://github.com/kill74/Chatify/releases).
2. Run the installer. Open **Chatify Launcher** from the Start Menu.
3. Select a mode:
   - `1` — Host on this machine (starts server + local client).
   - `2` — Join an existing server (prompts for host and port).

### From Source

Requires the Rust stable toolchain.

```bash
cargo build --release

# Server — binds all interfaces by default
./target/release/chatify-server --host 0.0.0.0 --port 8765

# Client — separate terminal
./target/release/chatify-client --host 127.0.0.1 --port 8765
```

> **Windows:** substitute `.\` for `./` and append `.exe` to binary names.

**Dev mode (incremental rebuild):**

```bash
# Terminal 1
cargo run -p chatify-server --bin chatify-server

# Terminal 2
cargo run -p chatify-client --bin chatify-client -- --host 127.0.0.1 --port 8765
```

**Windows dev scripts with automatic artifact cleanup:**

```powershell
.\run-server.ps1
.\run-client.ps1 -ProgramArgs @('--host','127.0.0.1','--port','8765')

# Full control over cleanup budget and retention window
.\scripts\run-with-auto-clean.ps1 -Mode server
.\scripts\run-with-auto-clean.ps1 -Mode client -ProgramArgs @('--host','127.0.0.1','--port','8765')
.\scripts\run-with-auto-clean.ps1 -CleanupOnly -MaxAgeDays 2 -MaxTargetSizeGB 3
```

> Lean dev/test profile settings in `Cargo.toml` reduce artifact growth, but `cargo clean` remains the reliable escape hatch after extended build cycles.

---

## Configuration Reference

### Server — `chatify-server`

- `--host` (default: `0.0.0.0`): Bind address.
- `--port` (default: `8765`): Bind port.
- `--db` (default: `chatify.db`): SQLite database path.
- `--db-durability` (default: `max-safety`): SQLite durability profile; `balanced` is faster and `max-safety` improves crash durability.
- `--db-key` (default: auto): 32-byte encryption key (hex, 64 chars). See resolution order below.
- `--tls` (default: `false`): Enable TLS (`wss://`); requires `--tls-cert` and `--tls-key`.
- `--tls-cert` (default: `cert.pem`): PEM certificate path.
- `--tls-key` (default: `key.pem`): PEM private key path.
- `--log` (default: `false`): Structured logging to stderr.
- `--media-retention-days` (default: `30`): Maximum media age in days before pruning.
- `--media-max-total-size-gb` (default: `20.0`): Retention budget in GiB; oldest completed media is pruned first when exceeded.
- `--media-prune-interval-secs` (default: `600`): Interval for periodic retention maintenance.
- `--disable-media-retention` (default: `false`): Disables periodic media pruning.

**DB key resolution order:**

1. `--db-key` CLI flag
2. `<db>.key` file - auto-generated only when creating a new DB; existing DBs require the original key file (or `--db-key`)
3. No encryption when `--db :memory:`

> Never commit `*.db.key`, `cert.pem`, or `key.pem`. Rotate immediately on exposure. Keep runtime secrets out of shell history and version control.

### Client — `chatify-client`

| Flag             | Default     | Notes                                              |
| ---------------- | ----------- | -------------------------------------------------- |
| `--host`         | `127.0.0.1` | Server host                                        |
| `--port`         | `8765`      | Server port                                        |
| `--tls`          | `false`     | Connect via `wss://`                               |
| `--log`          | `false`     | Debug logging to stderr                            |
| `--no-markdown`  | `false`     | Disable markdown rendering in the terminal feed    |
| `--no-media`     | `false`     | Disable media features (voice and inline media UI) |
| `--no-animation` | `false`     | Disable terminal animations                        |
| `--no-reconnect` | `false`     | Disable automatic reconnect attempts               |

`chatify-client` also respects persisted defaults from `config.toml` (`connection.*`, `ui.*`, `session.*`) when CLI flags are omitted.

---

## Client Commands

The terminal client is command-driven. The full command reference lives in [docs/COMMANDS.md](docs/COMMANDS.md); the high-signal workflows are:

- Timeline: `/history`, `/search`, `/replay`
- Trust: `/fingerprint`, `/trust`, `/trust-audit`, `/trust-export`
- Operations: `/metrics`, `/db-profile`, `/doctor`
- Rich messages: `/image`, `/video`, `/audio`, `/react`

---

## Persistence Model

The event store is **append-only**. Events are inserted once; they are never updated or deleted. All readable state — channel feeds, DM history, search results — is derived from this log.

**Schema management:** versioned via a `schema_meta` table. Migrations run sequentially on server startup. A no-downgrade policy is enforced: a server will refuse to start against a database created by a newer schema version.

**Indexes:**

- Channel history: `(channel, ts DESC)`
- DM history: `(event_type, sender, target, ts DESC)`
- High-volume channel/event scans: `(channel, event_type, ts DESC)` and `(event_type, channel, ts DESC)`
- Sender activity scans: `(sender, ts DESC)`

**Media durability:**

- `file_meta` and `file_chunk` transfers are persisted in dedicated tables:
  - `media_objects` (metadata, transfer status, byte counters)
  - `media_chunks` (chunk payloads keyed by media object + chunk index)
- Chunk payloads are encrypted at rest when DB encryption is enabled.

**Encryption at rest:** event `payload` and `search_text` columns are encrypted with the key resolved at startup. Media chunk payloads are also encrypted at rest when encryption is enabled. `/search` over encrypted rows still requires decryption in a bounded linear scan.

**CI-verified guarantees:**

- Event history survives server restart (store durability)
- `/history` and `/search` complete within acceptable timeout on a 100k-event local dataset

---

## Trust & Identity Model

Chatify uses **explicit fingerprint verification** rather than TOFU.

**Verification flow:**

1. Run `/fingerprint <user>` to retrieve the peer's current key fingerprint.
2. Verify the fingerprint out-of-band (voice call, secure side-channel, etc.).
3. Run `/trust <user> <fingerprint>` to record it in the local trust store.

**Trust states:**

| State     | Meaning                                  | DM encryption             |
| --------- | ---------------------------------------- | ------------------------- |
| `unknown` | No fingerprint on record                 | Blocked                   |
| `trusted` | Fingerprint verified and stored locally  | Allowed                   |
| `changed` | Peer key rotated since last verification | Blocked until re-verified |

Key changes are never silent. A rotation transitions the peer to `changed` and immediately blocks encrypted DM traffic in both directions. The operator must re-run the full verification flow. This prevents MITM via silent key rotation but adds friction to any legitimate rotation — a deliberate tradeoff.

---

## Security Posture

**Implemented controls:**

| Control                   | Implementation                                                                |
| ------------------------- | ----------------------------------------------------------------------------- |
| Transport encryption      | TLS via `rustls`                                                              |
| Credential storage        | PBKDF2, salted, per-user                                                      |
| Two-factor authentication | TOTP + backup codes                                                           |
| Session management        | Token lifecycle with expiry; tokens do not survive server restart             |
| Replay protection         | Nonce + timestamp window; periodic nonce cache eviction for ghost connections |
| Rate limiting             | Per-IP limits on connection establishment and auth attempts                   |
| Input validation          | Payload size limits enforced; parameterized queries throughout                |
| Encryption at rest        | `payload` and `search_text` columns encrypted with the server DB key          |
| Schema integrity          | No-downgrade policy enforced at startup                                       |

**Known gaps — do not deploy in adversarial environments without addressing these:**

- No certificate pinning. A valid CA-signed cert is sufficient for a successful TLS handshake. A compromised CA is not detected.
- Session tokens are ephemeral. All clients must re-authenticate after a server restart.
- Encrypted search is a linear scan. Timing side-channels are possible on large corpora.
- No RBAC and no centralized admin audit trail.
- No independent third-party security audit.

See [docs/SECURITY_NOTES.md](docs/SECURITY_NOTES.md) for the full threat model and scope boundaries.

---

## Discord Bridge

The bridge is compiled only when `--features discord-bridge` is passed. It is absent from the default server and client binaries.

```bash
cargo build --release --features discord-bridge
cargo run --features discord-bridge --bin discord_bot
```

**Required environment variables:**

```bash
DISCORD_TOKEN=<bot-token>
CHATIFY_PASSWORD=<server-password>
```

**Full configuration:**

| Variable                           | Default                   | Description                                                      |
| ---------------------------------- | ------------------------- | ---------------------------------------------------------------- |
| `CHATIFY_HOST`                     | `127.0.0.1`               | Chatify server host                                              |
| `CHATIFY_PORT`                     | `8765`                    | Chatify server port                                              |
| `CHATIFY_CHANNEL`                  | `general`                 | Default relay channel                                            |
| `CHATIFY_BOT_USERNAME`             | `DiscordBot`              | Bridge identity on Chatify                                       |
| `CHATIFY_WS_SCHEME`                | `ws`                      | `ws` or `wss`                                                    |
| `CHATIFY_AUTH_TIMEOUT_SECS`        | `15`                      | Auth handshake timeout                                           |
| `CHATIFY_RECONNECT_BASE_SECS`      | `1`                       | Exponential backoff base (seconds)                               |
| `CHATIFY_RECONNECT_MAX_SECS`       | `30`                      | Exponential backoff ceiling (seconds)                            |
| `CHATIFY_RECONNECT_JITTER_PCT`     | `20`                      | Jitter applied to reconnect interval                             |
| `CHATIFY_RECONNECT_WARN_THRESHOLD` | `5`                       | Consecutive reconnects before log warning                        |
| `CHATIFY_PING_SECS`                | `20`                      | Keepalive interval (`0` to disable)                              |
| `CHATIFY_HEALTH_LOG_SECS`          | `30`                      | Health telemetry log interval                                    |
| `CHATIFY_BRIDGE_INSTANCE_ID`       | —                         | Optional stable source identifier for multi-instance deployments |
| `CHATIFY_DISCORD_CHANNEL_MAP`      | —                         | Inline route map: `discordId:chatifyChannel,...`                 |
| `CHATIFY_DISCORD_CHANNEL_MAP_FILE` | `bridge-channel-map.json` | Path to route map file                                           |
| `CHATIFY_LOG`                      | —                         | Set to `1` to enable bridge logging                              |

**Route map — inline:**

```bash
CHATIFY_DISCORD_CHANNEL_MAP=123456789012345678:general,987654321098765432:ops
```

**Route map — file (`bridge-channel-map.json`):**

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

Merge precedence: file routes load first; `CHATIFY_DISCORD_CHANNEL_MAP` entries override matching Discord channel IDs.

**Operational notes:**

- **Loop prevention:** relay frames carry `src` and `relay.markers`. A frame is not re-relayed to Discord if the destination marker already appears in `relay.markers`. This handles the common loop on normal operation; it is not a guarantee against all loop topologies in complex multi-bridge configurations.
- **Mention safety:** Discord outbound relay sets `allowed_mentions` to suppress `@everyone`, `@here`, role, and user pings.
- **Attachment and reply preservation:** Discord → Chatify relay includes attachment URL metadata and reply context. Chatify → Discord relay reconstructs and emits that context on the Discord side.
- **Health observability:** run `/bridge status` from Discord or from the Rust client (requires `bridge-client` feature) to see connected bridge instances, their uptime, route count, and health counters.
- **Dependency pinning:** `serenity` is pinned at `=0.11.7` for build reproducibility.

---

## Windows Packaging

Requires [Inno Setup 6](https://jrsoftware.org/isinfo.php) for the installer target.

```powershell
# ZIP + installer
.\build-windows-package.ps1

# ZIP only
.\build-windows-package.ps1 -SkipInstaller

# Explicit ISCC path
.\build-windows-package.ps1 -IsccPath "C:\Tools\InnoSetup\ISCC.exe"
```

**Output artifacts:**

| Artifact                                        | Description                              |
| ----------------------------------------------- | ---------------------------------------- |
| `dist/chatify-windows-x64.zip`                  | Portable binary package                  |
| `dist/chatify-windows-x64.zip.sha256`           | SHA256 checksum                          |
| `dist/chatify-setup-<version>.exe`              | Installer (when Inno Setup is available) |
| `dist/chatify-setup-<version>.exe.sha256`       | Installer checksum                       |
| `dist/chatify-windows-x64/chatify-launcher.cmd` | Interactive launch helper                |

**Checksum verification (PowerShell):**

```powershell
$actual   = (Get-FileHash .\dist\chatify-windows-x64.zip -Algorithm SHA256).Hash.ToLower()
$expected = (Get-Content .\dist\chatify-windows-x64.zip.sha256).Split(' ')[0].ToLower()
if ($actual -eq $expected) { "OK" } else { "MISMATCH — do not execute" }
```

The [windows-release-package](.github/workflows/windows-release-package.yml) workflow builds and uploads ZIP, installer, and checksums on every published release. A structured security report (`.json` + `.md`) is attached per release tag via the [release-security-report](.github/workflows/release-security-report.yml) workflow.

---

## Development & CI

**Required checks — all must pass before merge:**

```bash
cargo check --workspace --bins --locked
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --all-targets --locked
```

**Protocol contract tests — run independently:**

```bash
cargo test --locked --test message_contracts auth_contract_returns_expected_fields
cargo test --locked --test message_contracts compatibility_contract_client_bootstrap_flow_stays_stable
cargo test --locked --test message_contracts protocol_contract_advertises_backward_compatible_version
cargo test --locked --test message_contracts file_contract_relays_media_metadata_and_chunks
```

**Feature-gated compile checks:**

```bash
cargo check --features discord-bridge --bin discord_bot --locked
cargo check -p chatify-client --features bridge-client --locked
```

All checks above are enforced in CI on every push to `main`. A failing check blocks merge.

---

## Known Limitations

Tracked explicitly rather than omitted:

- **No certificate pinning.** The client does not pin the server certificate. A valid CA-signed cert is sufficient for a successful TLS handshake.
- **Ephemeral session tokens.** All clients must re-authenticate after a server restart. There is no token persistence or refresh mechanism.
- **Linear search.** `/search` decrypts and scans all stored events. Performance degrades linearly with event count. Not suitable for large corpora without architectural changes.
- **Single-node only.** The server is single-process with no shared state backend. Horizontal scaling is not supported.
- **Minimal auth model.** No RBAC and no centralized admin audit trail.
- **No independent security audit.**

---

## Project Docs

- [docs/COMMANDS.md](docs/COMMANDS.md) - full terminal client command reference
- [docs/DEMO.md](docs/DEMO.md) - two-terminal local demo walkthrough
- [docs/RECRUITER_REVIEW_GUIDE.md](docs/RECRUITER_REVIEW_GUIDE.md) - 5-minute reviewer path and proof commands
- [examples/plugins/README.md](examples/plugins/README.md) - tiny v1 plugin reference implementation
- [SECURITY.md](SECURITY.md) - vulnerability reporting and supported security scope
- [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) — component design, message flow, data model
- [docs/BENCHMARKS.md](docs/BENCHMARKS.md) — benchmark methodology and baseline results
- [docs/ENGINEERING_CASE_STUDY.md](docs/ENGINEERING_CASE_STUDY.md) — design decisions and tradeoff analysis
- [docs/SECURITY_NOTES.md](docs/SECURITY_NOTES.md) — threat model, controls, scope boundaries
- [docs/UNIQUE_ROADMAP.md](docs/UNIQUE_ROADMAP.md) — near and long-term direction
- [CHANGELOG.md](CHANGELOG.md)

---

## Contributing

Read [CONTRIBUTING.md](CONTRIBUTING.md) before opening a PR. It covers dev setup, quality gates, and the PR checklist. All submissions must pass CI. By contributing, you agree your code is licensed under MIT.

## License

MIT — see [LICENSE](LICENSE).

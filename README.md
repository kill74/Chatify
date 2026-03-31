# Chatify

Terminal-first group chat built in Rust.

Chatify is a self-hosted WebSocket chat system with a Rust server and client.
It focuses on fast setup, lightweight runtime behavior, and a clean CLI workflow.

## Status

This project is actively evolving and should currently be treated as experimental.

- Core chat flow works.
- Runtime hardening and input validation are in place.
- Some features are placeholders and documented below.

## What It Does Today

- Multi-channel messaging
- Direct messages
- Custom user status messages
- Live roster with online/idle state detection
- Automatic idle timeout (5 minutes)
- Join/channel info/user listing commands
- In-memory channel history relay

## Important Security Note

Encryption helpers exist in the codebase, but the current implementation is not production-grade end-to-end security.

Reasons:

- Authentication is minimal.
- Protocol hardening is still in progress.

Use this project for development, learning, and controlled environments unless you complete a dedicated security hardening pass.

## Binaries

Current binaries configured in Cargo:

- clicord-server
- clicord-client

Discord bridge code exists in the repository, but is disabled as a build target.

## Quick Start

### Build release binaries

```bash
cargo build --release
```

### Windows (PowerShell)

Server:

```powershell
.\target\release\clicord-server.exe --host 0.0.0.0 --port 8765
```

Client:

```powershell
.\target\release\clicord-client.exe --host 127.0.0.1 --port 8765
```

### Linux/macOS

Server:

```bash
./target/release/clicord-server --host 0.0.0.0 --port 8765
```

Client:

```bash
./target/release/clicord-client --host 127.0.0.1 --port 8765
```

### Local development run

Terminal 1:

```bash
cargo run --bin clicord-server
```

Terminal 2:

```bash
cargo run --bin clicord-client -- --host 127.0.0.1 --port 8765
```

## CLI Commands (Current)

| Command              | Description                         |
| -------------------- | ----------------------------------- |
| /join <channel>      | Join or create a channel            |
| /dm <user> <message> | Send a direct message               |
| /me <action>         | Send action-style message           |
| /users               | List online users with live status  |
| /status <message>    | Set custom status (e.g., "AFK")     |
| /channels            | List channels                       |
| /voice [room]        | Toggle voice in room                |
| /edit <text>         | Placeholder command                 |
| /clear               | Clear terminal output               |
| /help                | Show command help                   |
| /quit, /exit, /q     | Disconnect and exit                 |

## UI Example (Terminal)

This is an example of how Chatify looks to a user in a normal terminal session.

Server window:

```text
$ cargo run --bin clicord-server
📡 Chatify running on ws://0.0.0.0:8765
🔒 Encryption: None (testing) | 🛡️  IP Privacy: On
⏹️  Press Ctrl+C to stop
```

Client window (user Alice):

```text
$ cargo run --bin clicord-client -- --host 127.0.0.1 --port 8765
username: alice
password:
Connected to server

/join dev
→ #dev

Hello team, build passed on my side.

/status In a meeting

/users
━━━ LIVE ROSTER ━━━
Online (2):
  🟢 alice (In a meeting)
  🟢 bob
Away (1):
  💤 carol

/dm bob Can you review the auth patch?

/voice
Voice started in #dev

/voice
Voice stopped

/quit
```

## Status Management

Chatify includes intelligent presence tracking:

### Custom Status
Set a custom status message visible to all users:
```bash
/status In a meeting
/status AFK - back in 10 min
/status Working on the database migration
```

### Live Roster
The `/users` command shows an enhanced roster with:
- 🟢 **Online** - Active users
- 💤 **Away** - Users idle for 5+ minutes
- Custom status messages displayed alongside usernames

### Automatic Idle Detection
- Users are automatically marked as "away" after 5 minutes of inactivity
- Any message or command resets the idle timer
- Idle detection runs in the background on the server

## Feature Maturity

### Stable for local use

- Channel chat flow
- DM flow
- Basic command routing
- Join/leave and system messaging

### Present but incomplete

- Edit command behavior
- Full protocol-level security hardening

## Known Limitations

- The current protocol is functional but not yet hardened for hostile public environments.
- Security hardening is still required before production use.
- There is no persistent storage layer yet; state is memory-first.
- Test coverage is light and mostly build/lint-driven; integration coverage is planned.
- The Discord bridge source exists, but the binary target is disabled due to dependency conflicts.

## Architecture

```text
.
├── Cargo.toml
├── src/
│   ├── main.rs        # server
│   ├── client.rs      # client
│   ├── crypto.rs      # crypto helpers
│   ├── lib.rs
│   └── discord_bot.rs # bridge source (disabled binary)
├── discord-bot/
└── README.md
```

## Development

Build and checks:

```bash
cargo check --bins
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all
```

Formatting:

```bash
cargo fmt
```

## Troubleshooting

### Connection refused

- Verify server is running.
- Verify host and port match.
- Verify local firewall rules.

### Auth or handshake issues

- Ensure server/client are from the same commit range.
- Rebuild both binaries after changes.

### Messages not decrypting as expected

- Confirm both sides use the same runtime assumptions and channel context.
- Treat current crypto paths as experimental.

### Command appears inactive

- /edit is currently a placeholder.

## Contributing

PRs are welcome.

Recommended process:

1. Create a focused branch from main.
2. Keep changes scoped to one concern.
3. Run check, clippy, and tests before opening PR.
4. Document behavior changes in PR description.

## Changelog and Release Notes

This repository currently uses git history and PR descriptions as the source of truth.

Suggested release notes format:

```text
## vX.Y.Z - YYYY-MM-DD

### Added
- ...

### Changed
- ...

### Fixed
- ...

### Security
- ...
```

If release cadence becomes regular, add a dedicated CHANGELOG.md using this format.

## Roadmap (Near Term)

- Complete edit path end-to-end
- Strengthen auth and protocol validation
- Replace/complete stubbed cryptographic flows
- Add integration tests for client/server message contracts

## License

GPL v3. See LICENSE.

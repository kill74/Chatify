# AGENTS.md

## Required Checks (in order)

```bash
cargo check --workspace --bins --locked && cargo fmt --all --check && cargo clippy --all-targets --all-features --locked -- -D warnings && cargo test --workspace --all-targets --locked

# Protocol contract tests (separate from workspace tests)
cargo test --locked --test message_contracts auth_contract_returns_expected_fields
cargo test --locked --test message_contracts compatibility_contract_client_bootstrap_flow_stays_stable
cargo test --locked --test message_contracts protocol_contract_advertises_backward_compatible_version
cargo test --locked --test message_contracts file_contract_relays_media_metadata_and_chunks

# Feature-gated builds
cargo check --features discord-bridge --bin discord_bot --locked
cargo check --features bridge-client --locked
```

**Always use `--locked`** — CI enforces this for reproducible builds.

## Project Structure

| Path                       | Purpose                                                |
| -------------------------- | ------------------------------------------------------ |
| `src/server/`              | WebSocket server, event store, auth — workspace member |
| `src/client/`              | Terminal dashboard — workspace member                  |
| `src/discord_bot.rs`       | Discord bridge — separate binary, feature-gated        |
| `tests/message_contracts/` | Protocol contract tests — not in workspace test suite  |

**Binaries:** `chatify-server`, `chatify-client`, `discord_bot` (feature-gated)

## Critical Conventions

- **DB key resolution**: CLI flag `--db-key` > `<db>.key` file > auto-generated (never created if `--db :memory:`)
- **Never commit**: `*.db.key`, `cert.pem`, `key.pem`, `*.db`
- **Windows dev**: Use `run-server.ps1` / `run-client.ps1` — they handle artifact cleanup automatically
- **Release builds**: `cargo build --release` applies LTO, strip, and opt-level 3

## Protocol-Specific Notes

- **Search is O(n)** — decrypts all events in linear scan. Don't assume performance scales.
- **Media transfer**: Chunked over WebSocket (`file_meta` + `file_chunk`), not HTTP. 100 MB cap enforced client-side.
- **Trust model**: Explicit fingerprint verification, not TOFU. Key rotation blocks DMs until re-verified.
- **Event store**: Append-only SQLite. Schema has no-downgrade policy — newer server refuses older DB.
- **Session tokens**: Ephemeral — don't survive server restart. All clients re-auth after restart.

## Common Pitfalls

1. Forgetting `--features discord-bridge` when building the bot — it won't compile
2. Running server without `--db-key` or `<db>.key` — server generates one and saves it; if DB exists without key file, startup fails
3. Enabling TLS without cert/key — connection drops silently or fails upgrade
4. Expecting search to perform on large datasets — it's a linear scan over encrypted data
5. Assuming DB migrations are backward-compatible — they're not; server refuses older schema

## Reference

- Full docs: [README.md](README.md)
- Contributing: [CONTRIBUTING.md](CONTRIBUTING.md)
- Architecture: [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)
- Security notes: [docs/SECURITY_NOTES.md](docs/SECURITY_NOTES.md)

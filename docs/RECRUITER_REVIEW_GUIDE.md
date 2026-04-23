# Recruiter Review Guide

This guide is for a five-minute technical skim of Chatify. It points reviewers toward evidence of backend/systems engineering rather than asking them to read the whole repository.

## First 60 Seconds

1. Start with [README.md](../README.md) and the dashboard screenshot.
2. Check the CI, release packaging, and release security report badges.
3. Open [docs/ENGINEERING_CASE_STUDY.md](ENGINEERING_CASE_STUDY.md) for the design tradeoffs.

What to notice:

- Async Rust service and terminal client.
- WebSocket protocol with contract tests.
- Append-only SQLite persistence.
- Explicit trust model and scoped security claims.

## Proof Commands

The main quality gate is:

```bash
cargo check --workspace --bins --locked
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --all-targets --locked
```

Protocol contracts are called out separately because they protect the client/server message shape:

```bash
cargo test --locked --test message_contracts auth_contract_returns_expected_fields
cargo test --locked --test message_contracts compatibility_contract_client_bootstrap_flow_stays_stable
cargo test --locked --test message_contracts protocol_contract_advertises_backward_compatible_version
cargo test --locked --test message_contracts file_contract_relays_media_metadata_and_chunks
```

## Demo Path

For a local walkthrough, use [docs/DEMO.md](DEMO.md):

```powershell
.\scripts\demo-local.ps1
```

The demo uses a temporary database/key and prints exact client commands for a two-terminal review.

## Engineering Signals

- `tests/message_contracts.rs`: protocol compatibility, auth, media, history/search, schema, and replay coverage.
- `.github/workflows/ci.yml`: locked workspace checks, contract gates, and feature-gated builds.
- `.github/workflows/windows-release-package.yml`: packaged release artifacts with checksum validation.
- `docs/BENCHMARKS.md`: reproducible 100k-event history/search latency baseline.
- `SECURITY.md` and `docs/SECURITY_NOTES.md`: honest scope boundaries instead of inflated security claims.

## Good Interview Questions

- Why use append-only SQLite instead of mutable row state?
- Why keep media on the WebSocket protocol instead of using HTTP uploads?
- What breaks if protocol contracts are not tested separately?
- What would need to change before this could serve a large multi-tenant deployment?
- How would you harden the trust model before hostile-network production use?

# Changelog

<!-- markdownlint-disable MD024 -->

All notable changes to this project will be documented in this file.

This format is based on Keep a Changelog and the project follows Semantic Versioning.

## [Unreleased]

### Planned

- Add reproducible benchmark runs and publish measured results for each tagged release.
- Continue protocol hardening and trust-model improvements.

## [0.4.0] - 2026-11-22

### Added

- **Real end-to-end encryption in the terminal client.** Channel messages are encrypted with ChaCha20-Poly1305 using keys derived from the client password hash; DMs use X25519 ECDH keys derived from peers' authenticated public keys. Ciphertext rides in `c`, plaintext for the server search index in `p` (matching the Discord bridge contract). Live messages, history, search, replay, and reply previews are decrypted with a legacy-plaintext fallback.
- **Protocol v2 negotiation.** Clients request `proto_v: 2`; the server advertises `v`, `min_v`, `max_v`, and a `features` capability list in `ok.proto`. v1 clients remain fully supported and negotiation falls back to v1 for unknown versions.
- **`/crypto` command** shows the active algorithm, local fingerprint, and cached channel/DM key counts.
- **Health endpoint reliability counters** — real uptime plus lifetime totals for connections, messages, DMs, reconnects, and last-event age under `GET /health`.
- **Optional token-protected shutdown endpoint** and loopback-by-default health bind for safer remote deployments.
- **New contract coverage** for v2 negotiation, unknown-version clamping, DM authorization (self/unknown recipient), and third-party DM history isolation.

### Changed

- DM key caching tracks the source public key and invalidates/re-derives on key rotation, so a rotated peer key no longer leaves stale encrypted state.
- Decryption now reports whether a message was actually encrypted instead of always claiming it was.
- Reconnect backoff resets after a healthy session; per-connection voice/screen-share/typing/transfer state is cleared on disconnect.
- Client maps are bounded (typing, reactions, unread, drafts, command history) with flood tests; typing TTLs use local time.
- Plugin external paths are re-validated immediately before spawn to close a symlink-swap TOCTOU window; builtin workers use a sanitized environment.
- Terminal output is sanitized across plugin inventory, bridge status, and other wire-derived fields to block OSC 52 clipboard writes and escape injection.
- Discord bridge refuses plaintext `ws://` to remote hosts unless explicitly opted in; its token/password copies are zeroized on drop.
- TOTP verification honors configurable digits/step and rejects unsupported algorithm/config values.
- WebRTC signalling stub now validates SDP/ICE format and size and is documented as signalling-only.
- Fixed mojibake in user-facing system messages (join/leave/voice).

### Fixed

- Server session revocation can no longer be bypassed by omitting the session token; tokens are bound to their connection.
- Idle channel/voice/screen rooms and stale metadata are pruned; read paths no longer create voice relay rooms.
- Media validation rejects zero/oversized files, sanitizes file ids, bounds chunk sizes, and preserves chunks on duplicate announcements.
- Timer callbacks can no longer double-fire; memory arena arithmetic is overflow-checked; screen capture uses bounded u64 arithmetic.
- Plugin processes are killed as a process group; notification threads are capped in flight.
- Screen-share capture stops on encoder initialization failure; voice threads are joined on `/voice off` and disconnect.
- Build breakage from incompatible `sha2` version resolved; `sha2` pinned to 0.10 with deprecated `thread_rng()` calls removed.

### Security

- Session tokens bound to connections with an authoritative active-session check.
- Terminal escape sanitization (`sanitize_terminal`) blocks OSC 52/8 and bare control characters from untrusted wire content.
- Server-side DM recipient validation and DB-scoped DM queries prevent cross-user history leakage.
- Secrets in the Discord bridge are zeroized; transport policy refuses credential-hash leakage over plaintext remote connections.

## [0.3.0] - 2026-04-16

### Added

- Message reactions with server-side aggregation and client-side rendering in the modular terminal client.
- New message identifier field `msg_id` in `msg` events to support reaction targeting and replay-safe correlation.
- New `reaction_sync` protocol event for reconnect/bootstrap reaction hydration.
- New modular client commands: `/react`, `/recent`, and `/sync` for reaction workflows.

### Changed

- Hardened reaction payload validation on the server for `msg_id` and `emoji` fields.
- Optimized reaction synchronization to query only reaction events instead of full channel history.
- Improved client-side dedup logic for message IDs and reaction events to avoid double counting.

### Tests

- Added contract coverage for reaction broadcast/sync aggregation flow.
- Added contract coverage for invalid reaction payload rejection (`reaction requires valid msg_id`).

## [0.1.0] - 2026-03-23

Initial public release of Chatify as a terminal-first, self-hosted Rust chat platform.

### Added

- Rust server binary for WebSocket-based chat transport.
- Rust terminal client binary with command-driven workflow.
- Core chat features: channels, direct messages, presence, and command discovery.
- SQLite-backed persistence for timeline durability and replay workflows.
- History and discovery commands including history, search, and rewind flows.
- Crypto utility layer for password hashing, key derivation, key management helpers, and authenticated encryption primitives.
- Optional Discord bridge binary behind the discord-bridge feature flag.
- Windows packaging script that produces distributable ZIP artifacts and SHA256 checksums.

### Changed

- Authentication and protocol validation paths were tightened with nonce and timestamp validation to reduce replay risk.
- Server and client internals were refactored for improved readability and maintainability.
- Event and schema handling were refined to support safer persistence evolution.

### Security

- Added replay-protection improvements through nonce and timestamp checks in authentication flows.
- Reintroduced modern key-exchange support and aligned cryptographic utilities with current implementation needs.

### Fixed

- Multiple CI and runtime reliability issues were resolved during stabilization.
- Bridge reconnect and source-loop filtering regressions are covered by targeted tests.

### Documentation

- Expanded architecture and roadmap documentation.
- Added recruiter-facing engineering documentation: security notes, benchmark methodology, and engineering case study.
- Improved README with clearer project positioning, quality posture, and release context.

### CI and Release

- Added CI quality gates for formatting, linting, tests, and feature-specific checks.
- Added release automation for Windows package artifacts on published GitHub releases.

[Unreleased]: https://github.com/kill74/Chatify/compare/v0.4.0...HEAD
[0.4.0]: https://github.com/kill74/Chatify/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/kill74/Chatify/compare/v0.1.0...v0.3.0
[0.1.0]: https://github.com/kill74/Chatify/releases/tag/v0.1.0

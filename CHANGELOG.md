# Changelog

All notable changes to this project will be documented in this file.

This format is based on Keep a Changelog and the project follows Semantic Versioning.

## [Unreleased]

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

### Planned

- Add reproducible benchmark runs and publish measured results for each tagged release.
- Continue protocol hardening and trust-model improvements.

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

[Unreleased]: https://github.com/kill74/Chatify/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/kill74/Chatify/releases/tag/v0.1.0

# Architecture

## Overview

Chatify is a terminal-first, self-hosted chat system with a Rust server and client.

Core components:

1. Server binary: src/main.rs
2. Client binary: src/client.rs
3. Shared crypto library: src/crypto.rs
4. Optional Discord bridge: src/discord_bot.rs (feature-gated)

## Server Model

The server maintains in-memory runtime state plus a persistent event store:

1. Channels and broadcast buses are kept in DashMap.
2. Channel history is retained in bounded in-memory queues.
3. Durable events are written to SQLite for history/search/rewind.

Event flow:

1. WebSocket payload received.
2. Payload validated and dispatched by event type.
3. In-memory channel state updated.
4. Durable event persisted when relevant.
5. Broadcast forwarded to subscribed clients.

## Persistence and Schema

SQLite stores chat events and schema metadata:

1. schema_meta table tracks schema_version.
2. events table stores normalized event payloads.
3. Migration path upgrades old schema versions.
4. Newer unsupported schema versions are not downgraded.

## Client Model

Client responsibilities:

1. Authentication and connection lifecycle.
2. Command parsing and outbound payload generation.
3. Inbound event dispatch and local timeline rendering.
4. Local key derivation and message encryption/decryption.

Client code is organized around small event handlers and shared queue helpers to keep the main loop readable.

## Crypto Boundaries

Shared crypto helpers provide:

1. Channel key derivation from password + channel salt.
2. DM key derivation through X25519 Diffie-Hellman.
3. ChaCha20-Poly1305 encryption and decryption.
4. Password hashing and keypair/public key helpers.

## Optional Discord Bridge

The Discord bridge is feature-gated behind discord-bridge.

Build behavior:

1. Default build excludes bridge dependencies and binary.
2. Enabling discord-bridge compiles and exposes discord_bot binary.

## Testing Strategy

Current integration coverage focuses on protocol contracts:

1. Auth and user payload shape.
2. Message roundtrip behavior.
3. Voice payload forwarding.
4. Durable history/search/rewind behavior.
5. Schema migration and version handling.

## CI Expectations

Recommended CI checks:

1. cargo fmt --check
2. cargo clippy --all-targets --all-features -- -D warnings
3. cargo test --all
4. cargo check --features discord-bridge --bin discord_bot

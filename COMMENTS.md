# Chatify Codebase Comments & Documentation

This document summarizes the comprehensive code comments added throughout the Chatify project to aid developers in understanding the architecture, design decisions, and critical constraints.

## Documentation Strategy

Comments follow a senior-engineer-to-junior-engineer style, explaining:
- **What** the code does (straightforward functionality)
- **Why** it works that way (design decisions and constraints)
- **How** it fits into the system (relationships to other modules)
- References to **AGENTS.md** for architectural constraints

## Documented Areas

### Server Core (`src/server/src/`)

#### `main.rs`
- **Comprehensive module-level docs** explaining the WebSocket server architecture
- System components: auth, protocol validation, state management
- Configuration options and deployment guidance
- Async/await patterns and graceful shutdown flow

#### `db.rs`
- **Enhanced module documentation** covering:
  - Append-only event store design (audit trails)
  - Schema versioning and migration strategy
  - WAL mode for durability
  - O(n) search performance characteristics
- **Inline comments** on key methods:
  - `get_connection()`: WAL pragmas and concurrency notes
  - `record_db_observation()`: Metrics collection for performance monitoring
  - `store_event()`: Payload encryption expectations

#### `auth.rs`
- Already well-documented with validation flow explanations
- Type-driven auth model (prevents validation bypass)

#### `protocol.rs`
- Comprehensive constants documentation
- Message types and versioning strategy

#### `state.rs`
- Lock-free concurrent state management via DashMap
- Session token ephemeralness (per AGENTS.md)
- Connection tracking and rate limiting

### Client Core (`src/client/src/`)

#### `main.rs`
- **Full module documentation** of the terminal UI architecture
- Actor-like decoupling of UI, state, and handlers
- Session management and authentication flow
- Reference to ephemeral token constraints

#### `handlers.rs`
- **Protocol event dispatch documentation**
- Encryption key verification and trust model
- Voice audio relay and decoding
- Plugin hook invocation

#### `state.rs`
- **Exhaustive field-by-field documentation** of ClientState
- Key constraints:
  - Ephemeral sessions (AGENTS.md)
  - Explicit key trust (no TOFU)
  - O(n) search performance
- Bounded collections and deduplication strategy
- Trust store and fingerprint verification

### Shared Utilities (`src/`)

#### `crypto.rs`
- Channel key derivation and X25519 ECDH
- ChaCha20Poly1305 encryption/decryption with nonce management
- PBKDF2 password hashing (client vs server)
- Already well-commented

#### `config.rs`
- Configuration structure and defaults
- Cross-platform file paths
- Persistent session state
- Already well-documented

#### `error.rs`
- Granular error variants for protocol distinction
- Auth vs validation vs operational failures
- Already well-documented

#### `performance.rs`
- **New module docs** explaining optimization layers:
  - Static JSON caching for error responses (allocation savings)
  - LRU caches for derived keys (unbounded memory protection)
  - Ring buffers for message history (O(1) operations)
  - Why these matter for high throughput

#### `metrics.rs`
- **Comprehensive metrics documentation**
- PrometheusMetrics struct with field-by-field explanations
- Operations monitored: auth, connections, messages, DB, latency
- Per-field purpose and thresholds

### Voice Subsystem (`src/voice/`)

#### `mod.rs`
- **Module-level documentation** of voice architecture
- VoiceRoom and VoiceMemberState structures
- Per-field explanations of mute, deafen, speaking state
- Design note on client-side state signaling

#### `relay.rs`
- **Broadcast relay pattern** documented
- VoiceBroadcast enum variants with semantics
- Member info snapshots and state change events
- Implementation notes on lock-free relay design

### UI & Formatting (`src/ui/`)

#### `ansi.rs`
- ANSI escape code stripping for terminal geometry
- Already well-documented

## Key Constraints Referenced

Comments reference important architectural constraints from AGENTS.md:

1. **Ephemeral session tokens** (AGENTS.md)
   - Don't survive server restarts
   - Clients must re-authenticate after crashes
   - Affects key re-verification flows

2. **Explicit key trust** (AGENTS.md)
   - No TOFU (Trust On First Use)
   - Fingerprint verification required
   - KeyChangeWarning events trigger manual verification

3. **O(n) search performance** (AGENTS.md)
   - Server decrypts all events linearly
   - Full-text search has no index
   - Clients advised to use pagination

4. **Database schema versioning** (AGENTS.md)
   - No-downgrade policy
   - Newer servers refuse older schemas
   - Migration-based upgrades only

5. **Media over WebSocket** (AGENTS.md)
   - Files chunked over WS, not HTTP
   - 100 MB cap enforced client-side
   - Allows message stream multiplexing

## File Summary

| File | Status | Key Additions |
|------|--------|-----------------|
| `src/server/src/main.rs` | ✅ Enhanced | Module docs, architecture overview, macro explanations |
| `src/server/src/db.rs` | ✅ Enhanced | Design notes, method inline comments |
| `src/client/src/main.rs` | ✅ Enhanced | Full module docs, actor pattern explanation |
| `src/client/src/handlers.rs` | ✅ Enhanced | Trust model, protocol dispatch documentation |
| `src/client/src/state.rs` | ✅ Enhanced | 100+ field-level documentation |
| `src/voice/mod.rs` | ✅ Enhanced | Voice architecture and state explanations |
| `src/voice/relay.rs` | ✅ Enhanced | Broadcast relay pattern and implementation |
| `src/metrics.rs` | ✅ Enhanced | Metric purposes and field documentation |
| `src/performance.rs` | ✅ Enhanced | Optimization strategy and rationale |
| `src/lib.rs` | ✓ Reviewed | Already well-documented |
| `src/crypto.rs` | ✓ Reviewed | Already well-documented |
| `src/config.rs` | ✓ Reviewed | Already well-documented |
| `src/error.rs` | ✓ Reviewed | Already well-documented |
| `src/totp.rs` | ✓ Reviewed | Already well-documented |
| `src/discord_bot.rs` | ✓ Reviewed | Already well-documented |
| Other modules | ✓ Reviewed | Already appropriately documented |

## Comment Style Guidelines Used

- **Module-level (`//!`)**: Architecture, responsibilities, design patterns, constraints
- **Type/function docs (`///`)**: Purpose, parameters, return values, edge cases
- **Inline comments (`//`)**: Non-obvious logic, async/await patterns, cryptographic details
- **Conversational tone**: "This allows X because..." rather than "X is done here"
- **Constraint references**: "See AGENTS.md" for system-level requirements
- **No comments on obvious code**: Simple loops, straightforward assignments stay uncommented

## Future Maintenance

When modifying code:
1. Update module docs if responsibilities change
2. Add inline comments for non-obvious logic
3. Reference constraints (session tokens, key trust, search performance) when relevant
4. Keep doc comments in sync with implementation changes
5. Use these comments as onboarding material for new contributors

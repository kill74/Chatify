# Chatify Uniqueness Roadmap

This plan is designed to make Chatify feel meaningfully different from typical chat apps.

## Product Direction

Build the best terminal-first, self-hosted team chat for developer workflows.

## Phase 1: Durable Timeline and Search (2-3 weeks)

Goal: move from memory-first chat to a queryable event stream.

Deliverables:
- Add SQLite persistence for message events (chat, DM, system, joins, edits).
- Add command: `/history [channel] [window]` (examples: `#general 24h`, `dm:alice 7d`).
- Add command: `/search <query>` for full-text event search.
- Add command: `/replay <timestamp>` to reconstruct state from events.

Implementation notes:
- Keep write path append-only.
- Use channel + ts indexes.
- Define versioned schema migration flow.

Exit criteria:
- Restarting server preserves history.
- History and search latency stays low on 100k local events.

## Phase 2: Identity and Trust UX (2 weeks)

Goal: practical trust model without heavy enterprise complexity.

Deliverables:
- Persist public key fingerprints per user.
- Add command: `/fingerprint [user]`.
- Add command: `/trust <user> <fingerprint>`.
- Show key-change warnings on reconnect.

Implementation notes:
- Use explicit trust states: `unknown`, `trusted`, `changed`.
- Block silent key rotation.

Exit criteria:
- User can verify and trust peers from CLI.
- Key changes are obvious and auditable.

## Phase 3: Discord Bridge Re-Enable (2-3 weeks)

Goal: support mixed communities without forcing migration.

Deliverables:
- Re-enable bridge target with pinned dependency strategy.
- Add channel map config file for bridge routes.
- Add loop prevention using source IDs and relay markers.
- Add bridge health command: `/bridge status`.

Implementation notes:
- Keep bridge optional behind feature flag.
- Add structured logs for bridge events.

Exit criteria:
- Bidirectional message relay works in mapped channels.
- No relay loops under normal operation.

## Phase 4: Terminal-Native Rich Messages (2 weeks)

Goal: richer experience while preserving terminal identity.

Deliverables:
- ANSI image preview in client for supported payloads.
- Better code block rendering with language hint labels.
- Audio note payload support (short voice clips in channel timeline).

Implementation notes:
- Make media rendering optional (`--no-media`).
- Keep graceful fallback for simple terminals.

Exit criteria:
- Rich payloads are readable in default terminal without UI breakage.

## Phase 5: Plugin Runtime (3-4 weeks)

Goal: make Chatify extensible and community-driven.

Deliverables:
- Plugin API for slash command registration.
- Message hook API for moderation/automation.
- Ship 3 built-in plugins: poll, standup, deploy notifier.

Implementation notes:
- Start with process-isolated plugin execution.
- Freeze API v1 before publishing examples.

Exit criteria:
- Plugins can be installed, listed, and disabled without restart.

## Baseline Quality Gates (Apply to every phase)

- Contract tests for message schema changes.
- Integration tests for server/client compatibility.
- Clippy and rustfmt clean in CI.
- Backward compatibility check for protocol version.

## Release Cadence

- Ship every 2 weeks.
- Keep one feature flag per major roadmap item until stable.
- Promote to default only after one stable cycle.

## Suggested Milestones

1. v0.2.0: persistence + search + history
2. v0.3.0: identity trust model
3. v0.4.0: discord bridge alpha
4. v0.5.0: rich terminal payloads
5. v0.6.0: plugin runtime beta

# Chatify Uniqueness Roadmap

Last updated: 2026-04-21

This document is the single source of truth for product execution and release readiness.

## Product North Star

Build the best terminal-first, self-hosted team chat for developer workflows.

Chatify should feel differentiated by default in three ways:

- Fast and reliable event timeline for engineering teams.
- Practical trust model with clear key integrity signals.
- Extensible automation model that works in terminal-first environments.

## Planning Principles

- Bias toward user-visible outcomes, not internal refactors.
- Keep protocol compatibility stable unless explicitly versioned.
- Ship behind flags first, then promote after one stable cycle.
- Require measurable exit criteria for every phase.

## Operating Model

- Planning horizon: rolling 2-week delivery cycles.
- Scope control: one major risk per cycle, one clear user-facing milestone.
- Quality bar: no feature is considered "done" without contract/integration coverage.
- Promotion policy: feature-default changes require one full stable cycle.

## Status Legend

- Complete: Feature set is shipped and validated.
- In progress: Core capability exists, but user-facing gaps remain.
- Foundation only: Data model or scaffolding exists, but flows are incomplete.
- Planned: Not started.
- At risk: Progress exists, but delivery confidence is low without intervention.

## Current Portfolio Status

| Phase                                 | Status                  | Confidence | Reality Check                                                                     | Next Decision                                  |
| ------------------------------------- | ----------------------- | ---------- | --------------------------------------------------------------------------------- | ---------------------------------------------- |
| Phase 1 Durable Timeline and Search   | Complete                | High       | Queryable event stream is available server-side and exposed in default client CLI | Keep latency and compatibility guardrails      |
| Phase 2 Identity and Trust UX         | Complete                | High       | CLI trust workflow, key-change protection, and audit export are now shipped       | Monitor UX friction and tune defaults          |
| Phase 3 Discord Bridge Re-Enable      | In progress (alpha)     | Medium     | Bridge runtime, route mapping, and status visibility exist                        | Stabilize and decide default-off vs default-on |
| Phase 4 Terminal-Native Rich Messages | In progress             | Medium     | Markdown and ANSI are good; media UX is incomplete                                | Ship one media milestone cleanly               |
| Phase 5 Plugin Runtime                | In progress (ecosystem enablement) | High       | API v1, built-ins, runtime management, client plugin UX, and a reference example are shipped | Widen third-party validation |

## Program Health Metrics

Track these as release-level gates, not vanity metrics:

- P95 history/search query latency at 100k local events.
- P95 plugin execution latency for built-ins and representative external plugins.
- Bridge reconnect recovery time and loop-prevention incident count.
- Trust workflow completion rate (`fingerprint -> trust`) for active users.
- Regression rate per release (contract/integration failures after merge).

## Phase 1 Durable Timeline and Search

Goal: move from memory-first chat to a queryable event stream.

Status: Complete.

Completed:

- SQLite-backed persistence for timeline events is in place.
- Server-side history, search, and replay events are implemented.
- Contract coverage includes restart durability, DM scope, replay behavior, and high-volume latency checks.
- Default client CLI now exposes `/history`, `/search`, and `/replay`.
- CLI help and command grammar are aligned with protocol scopes (`#channel`, `dm:user`) and optional limits.

Remaining:

- None for core Phase 1 capability. Continue performance and compatibility monitoring.

Exit Criteria:

- Restart preserves timeline and query correctness.
- History and search remain low-latency at 100k local events.
- Default CLI exposes history, search, and replay workflows without manual JSON frames.
- Query paths remain protocol-compatible with existing clients.

## Phase 2 Identity and Trust UX

Goal: practical trust model without heavy enterprise complexity.

Status: Complete.

Completed:

- Client-side trust model scaffolding exists (`TrustStore`, peer fingerprint metadata, audit entry structures).
- Default client now exposes `/fingerprint`, `/trust`, and `/trust-audit`.
- Key-change warnings are emitted on reconnect/key-directory refresh and incoming key observations.
- Trust store and trust audit data are persisted per user/server profile across restarts.
- DM send path now enforces trust policy and blocks on untrusted or changed keys by default.
- `/trust-export` produces deterministic trust-audit JSON suitable for incident attachments.

Remaining:

- None for core Phase 2 capability. Continue monitoring trust UX and policy ergonomics.

Exit Criteria:

- Users can verify and trust peers entirely from CLI.
- Key changes are visible, explicit, and auditable.
- Silent key rotation is blocked by default.
- Trust events are present in audit output with deterministic formatting.

## Phase 3 Discord Bridge Re-Enable

Goal: support mixed communities without forcing migration.

Status: In progress (alpha).

Completed:

- Bridge runtime is active with reconnect handling.
- Route mapping support exists via `bridge-channel-map.json`.
- Bridge health and status signaling are present (`/bridge status` in bridge context).
- Loop-prevention markers and bridge metadata preservation are covered by tests.

Remaining:

- Complete stabilization pass under load and reconnect churn.
- Finalize feature-flag policy and promotion criteria to default behavior.

Exit Criteria:

- Bidirectional relay works reliably across mapped channels.
- No relay loops in normal operation.
- Health/status telemetry is actionable in production.
- Bridge remains optional and safe to disable without data-path side effects.

## Phase 4 Terminal-Native Rich Messages

Goal: richer experience while preserving terminal identity.

Status: In progress.

Completed:

- Markdown rendering exists for terminal output.
- ANSI styling and utility pipeline are established.
- Code block rendering path exists.

Remaining:

- Ship ANSI image preview with graceful fallback.
- Add audio note timeline payload support (short clips).
- Keep media optional and controllable (`--no-media`).

Exit Criteria:

- Rich payloads render safely in common terminals.
- Fallback behavior is predictable when media support is unavailable.
- Default terminal UX remains stable and readable.
- Media rendering can be disabled globally without breaking timeline rendering.

## Phase 5 Plugin Runtime

Goal: make Chatify extensible and community-driven.

Status: In progress (hardening cycle).

Completed:

- Plugin API v1 is implemented.
- Slash command registration and message hook execution are functional.
- Built-in plugins shipped: poll, standup, deploy-notifier.
- Runtime management supports install, list, and disable without restart (protocol-level).
- Default client now exposes `/plugin list`, `/plugin install`, and `/plugin disable` for operator workflows.
- Hardening delivered: strict response parsing, bounded I/O, process timeout, stronger termination behavior.
- A tiny external reference plugin is available under `examples/plugins/`.

Remaining:

- Broaden API v1 examples beyond the Windows-friendly reference plugin.
- Add wider ecosystem validation for third-party plugin ergonomics.

Exit Criteria:

- Plugins can be installed, listed, and disabled from normal operator workflow.
- API v1 contract is frozen and documented with examples.
- Plugin worker reliability and timeout behavior are stable in CI and local smoke tests.
- Plugin failure isolation does not degrade core chat availability.

## Next 2-Week Execution Plan

Cycle goal: complete plugin operator follow-through and de-risk the next promotion decisions.

| Workstream              | Deliverable                                                                  | Acceptance Criteria                                                     |
| ----------------------- | ---------------------------------------------------------------------------- | ----------------------------------------------------------------------- |
| Query UX follow-through | Publish usage examples for `/history`, `/search`, and `/replay`              | Operator docs match implemented CLI grammar and protocol scope behavior |
| Plugin UX               | Broaden API v1 examples and reference plugin templates                       | Third-party authors can follow documented install and command flows     |
| Trust follow-through    | Publish operator guidance for `/fingerprint`, `/trust`, and `/trust-export`  | Docs match implemented trust UX and incident-export workflow            |
| Bridge hardening        | Execute targeted reconnect/loop-prevention stress pass                       | No loop regressions in scripted stress test scenarios                   |
| Rich media increment    | Ship one media milestone (`image preview` or `audio notes`)                  | Feature works with explicit fallback and can be disabled globally       |

## Active Risks and Mitigations

| Risk                                        | Impact                                            | Mitigation                                        | Owner  |
| ------------------------------------------- | ------------------------------------------------- | ------------------------------------------------- | ------ |
| Roadmap drift vs shipped reality            | Teams prioritize stale work or miss shipped wins  | Re-baseline roadmap when operator UX lands        | PM/Client |
| Bridge instability under reconnect churn    | Reliability and trust impact in mixed communities | Keep default-off until stress criteria are met    | Bridge |
| Plugin ecosystem drift from API v1 contract | Third-party breakage and support burden           | Freeze docs/examples with conformance checks      | Server |
| Media scope creep                           | Rich-message work can sprawl past terminal UX bar | Ship one milestone with explicit fallback only    | Client |

## Quality Gates (All Phases)

- Contract tests for message schema changes.
- Integration tests for server/client compatibility.
- Clippy and rustfmt clean in CI.
- Backward compatibility check for protocol version.
- Plugin worker smoke tests and timeout/path safety checks.
- Feature flag coverage for all non-default capabilities.

## Release Cadence

- Ship every 2 weeks.
- Keep one feature flag per major roadmap item until stable.
- Promote to default only after one full stable cycle.

## Re-Baselined Milestones

1. v0.6.0: plugin runtime beta plus client command parity (`/plugin list`, `/plugin install`, `/plugin disable`)
2. v0.7.0: bridge stabilization cycle complete and default-policy decision
3. v0.8.0: first rich terminal media milestone (image preview or audio notes)
4. v0.9.0: plugin docs/examples and ecosystem conformance pass
5. v1.0.0 candidate: all phases at "Complete" with one stable-cycle validation

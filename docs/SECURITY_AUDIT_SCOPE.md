# Security Audit Scope

This file is the handoff scope for an independent Chatify security review.
It is not a self-audit certification.

## Review Targets

1. WebSocket authentication, auth-v2 challenge handling, lockout, 2FA, and session validation.
2. Client transport policy, TLS usage, and future certificate/SPKI pinning.
3. DM and channel encryption boundaries, trust-store behavior, fingerprint verification, and key-change handling.
4. SQLite event-store encryption, key-file resolution, backup/restore, and schema migration refusal behavior.
5. Plugin runtime isolation, trusted plugin root enforcement, environment scrubbing, payload limits, and timeout/termination behavior.
6. Discord bridge feature-gated build and loop-prevention behavior.

## Explicit Questions

1. Should auth-v2 be replaced with OPAQUE, SRP, or another audited PAKE for this deployment model?
2. Is the current X25519 DM design acceptable for controlled deployments, or should the next release require Signal-style Double Ratchet or MLS?
3. Are plugin trust roots and process isolation sufficient, or should production builds disable external plugins unless an OS sandbox profile is configured?
4. Are audit logs sufficient for incident response, or should hash-chained/tamper-evident audit records be required before release?

## Evidence To Provide

1. Required CI command output from `AGENTS.md`.
2. Release security report artifacts.
3. Protocol contract test results.
4. Threat-model updates in `docs/SECURITY_NOTES.md`.

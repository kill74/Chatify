# Security Notes

## Scope

This document defines the current security boundaries of Chatify and the hardening plan.
It is intentionally explicit to avoid overstating guarantees.

## Security Goals

1. Protect message confidentiality during transport and at rest where supported by protocol design.
2. Reduce the chance of silent protocol misuse or key confusion.
3. Keep trust and identity behavior inspectable from client and server logs.

## Current Controls

1. Crypto helper module for key derivation and authenticated encryption flows.
2. Protocol-level validation paths in server and client message handling.
3. CI checks that prevent unchecked changes from bypassing lint and tests.
4. Optional bridge isolation through feature flags so default builds stay minimal.
5. Structured security test report is generated per release tag and attached to release assets (`.json` + `.md`).
6. Server-side credential storage uses Argon2id PHC hashes for new password writes while retaining legacy PBKDF2 verification for existing rows.
7. Session bearer tokens are stored only as SHA-256 digests in memory and expire by absolute and idle TTL.
8. The client refuses plaintext `ws://` connections to non-loopback hosts unless explicitly started with the insecure-development override.
9. All key-producing and security-sensitive functions are annotated `#[must_use]` so the compiler warns on accidental discard of key material.
10. CI runs `cargo audit` on every push to detect known dependency vulnerabilities.
11. `BotState` in the Discord bridge zeroizes all credentials (`auth_password`, `channel_secret`, `priv_key`, cached keys) on drop.
12. PBKDF2 iterations are set to 600,000, matching OWASP 2023 recommendations, for client-side hashing and legacy verification.
13. Encryption plaintext limit is 100 MB, matching the documented client-side cap.
14. `AuthInfo` in the library crate omits `Debug` to prevent accidental logging of password hashes.

## Known Limits

1. Authentication model is still under active hardening; auth-v2 is a challenge/response flow, not a full PAKE.
2. Full independent security review has not been completed.
3. Production threat model is not fully closed for hostile network environments.
4. Certificate pinning and first-class trust-on-transport UX are not complete.
5. DM encryption does not yet provide a full Signal-style Double Ratchet or MLS group security model.
6. Plugin workers are restricted to trusted plugin roots and run with a scrubbed environment, but not yet OS-sandboxed with a least-privilege profile.
7. Security claims should be treated as controlled-environment level unless additional hardening is applied.

## Threat Model (Current)

### In Scope

1. Protocol misuse from malformed payloads.
2. Reliability risks from reconnect and relay loops in bridge scenarios.
3. State consistency risks between in-memory channels and durable event persistence.

### Out of Scope (For Now)

1. Nation-state adversary model.
2. Formal cryptographic proofs for protocol composition.
3. Full key lifecycle governance and compliance controls.

## Hardening Backlog

1. Replace auth-v2 credential verification with an audited PAKE/verifier design such as OPAQUE or SRP.
2. Add certificate/SPKI pinning for `wss://` and store server pins per profile with rotation UX.
3. Move DMs to an audited forward-secret protocol: Signal-style X3DH + Double Ratchet for one-to-one messaging, or MLS for groups.
4. Add stronger identity trust workflow: device identity keys, signed prekeys, QR/safety-number verification, and key-transparency style audit evidence.
5. Require fresh 2FA or recent re-auth for all admin and sensitive actions, including plugin install, 2FA disable, password change, DB backup/restore, bridge setup, and role changes.
6. Add signed plugin manifests and OS-level job/AppContainer restrictions where available.
7. Add replay and tamper-resistance tests for sensitive message flows.
8. Add adversarial integration tests for malformed and reordered payloads.

## Release Security Report Artifact

Each published release tag generates a machine-readable and human-readable security report:

1. `chatify-security-report-<tag>.json` includes check metadata (tag, commit, timestamp, run URL), required/optional results, and dependency-audit metrics.
2. `chatify-security-report-<tag>.md` includes an executive summary and per-check status table with log references.

## Release Gate Recommendations

Before each minor release:

1. Run all CI quality gates.
2. Run targeted protocol and bridge regression tests.
3. Update this file with any newly discovered limitation.
4. Reject release if a high-severity security issue is unresolved.

## Disclosure and Reporting

For security findings, open a private report path first, then publish a sanitized postmortem once fixed.

Suggested disclosure template:

1. Impact summary.
2. Affected versions.
3. Reproduction steps.
4. Mitigation and fix details.
5. Follow-up prevention action.

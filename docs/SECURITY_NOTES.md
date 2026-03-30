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

## Known Limits

1. Authentication model is still minimal and under active hardening.
2. Full independent security review has not been completed.
3. Production threat model is not fully closed for hostile network environments.
4. Security claims should be treated as controlled-environment level unless additional hardening is applied.

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

1. Add stronger identity trust workflow and explicit fingerprint verification UX.
2. Add replay and tamper-resistance tests for sensitive message flows.
3. Add adversarial integration tests for malformed and reordered payloads.

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

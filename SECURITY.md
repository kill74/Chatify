# Security Policy

Chatify is a personal/open-source systems project with a deliberately scoped security posture. It includes practical hardening, protocol tests, and release security reports, but it has not had an independent third-party security audit.

## Supported Versions

Security fixes target the latest tagged release and `main`.

| Version | Supported |
| ------- | --------- |
| `0.3.x` | Yes       |
| `<0.3`  | No        |

## Reporting a Vulnerability

Please avoid opening public proof-of-concept issues for exploitable vulnerabilities before a fix is available. Use a private contact path from the maintainer profile, or open a minimal public issue that asks for a private security channel without disclosing exploit details.

Include:

- Impact summary
- Affected version or commit
- Reproduction steps
- Relevant logs or packet/message examples
- Suggested mitigation, if known

## Scope

The current threat model, known limits, controls, and release-gate guidance live in [docs/SECURITY_NOTES.md](docs/SECURITY_NOTES.md). In short: Chatify is designed for controlled/self-hosted deployments, uses explicit peer fingerprint verification rather than TOFU, and documents known gaps such as no certificate pinning, no RBAC, and no independent audit.

## Release Security Reports

Tagged releases publish machine-readable and human-readable security reports through the [Release Security Report workflow](.github/workflows/release-security-report.yml). These reports are evidence of release checks, not a certification or audit.

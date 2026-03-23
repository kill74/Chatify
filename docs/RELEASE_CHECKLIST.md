# Release Checklist (v0.1.0)

Use this checklist to cut a clean, repeatable release.

## 1. Verify Working Tree

```bash
git status --short
```

Expected: only intentional release-doc changes.

## 2. Run Quality Gates

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all
cargo check --features discord-bridge --bin discord_bot
```

## 3. Build Release Binaries

```bash
cargo build --release
```

## 4. Build Windows Package

```powershell
.\build-windows-package.ps1
```

Expected artifacts:

- dist/chatify-windows-x64.zip
- dist/chatify-windows-x64.zip.sha256

## 5. Commit Release Docs

```bash
git add README.md CHANGELOG.md docs/SECURITY_NOTES.md docs/BENCHMARKS.md docs/ENGINEERING_CASE_STUDY.md docs/RELEASE_CHECKLIST.md
git commit -m "docs(release): prepare recruiter-facing v0.1.0 package"
```

## 6. Create Annotated Tag

```bash
git tag -a v0.1.0 -m "v0.1.0"
```

## 7. Push Commit and Tag

```bash
git push origin main
git push origin v0.1.0
```

## 8. Publish GitHub Release

1. Open the v0.1.0 tag in GitHub Releases.
2. Use the CHANGELOG v0.1.0 section as release notes.
3. Publish release.
4. Verify windows-release-package workflow completes.
5. Verify attached assets include ZIP and SHA256.

## 9. Post-Release Verification

1. Clone fresh and run server/client quick start.
2. Verify release links in README and CHANGELOG.
3. Save benchmark numbers to docs/BENCHMARKS.md after first measured run.

## 10. Vulnerability Triage and Remediation

Use this process when GitHub reports security alerts for the default branch.

### 10.1 Inspect Current Advisories

1. Open the Security tab in GitHub and list all Dependabot alerts by severity.
2. Classify each alert by exploitability in this project context: reachable, conditionally reachable, or not reachable.

### 10.2 Verify Affected Dependency Path

```bash
cargo tree -i <crate_name>
```

Confirm whether the vulnerable crate is direct or transitive.

### 10.3 Apply Safe Updates

```bash
cargo update -p <crate_name>
```

If a major version upgrade is required, update `Cargo.toml` intentionally and document the compatibility impact.

### 10.4 Re-Run Quality Gates

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all
cargo check --features discord-bridge --bin discord_bot
```

### 10.5 Record and Communicate

1. Add a brief summary of fixes in CHANGELOG under Unreleased.
2. If risk is high, publish a patch release and call out security impact in release notes.

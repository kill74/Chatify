use std::fs;
use std::path::Path;

fn read_script(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("scripts")
        .join(name);
    fs::read_to_string(&path).unwrap_or_else(|err| panic!("failed to read {path:?}: {err}"))
}

fn assert_contains_in_order(haystack: &str, needles: &[&str]) {
    let mut offset = 0;
    for needle in needles {
        let Some(index) = haystack[offset..].find(needle) else {
            panic!("expected to find `{needle}` after byte offset {offset}");
        };
        offset += index + needle.len();
    }
}

#[test]
fn local_ci_script_keeps_required_gates_in_ci_order() {
    let script = read_script("ci-local.ps1");

    assert_contains_in_order(
        &script,
        &[
            "assert-release-targets.ps1",
            r#""cargo" @("check", "--workspace", "--bins", "--locked")"#,
            r#""cargo" @("fmt", "--all", "--check")"#,
            r#""cargo" @("clippy", "--workspace", "--all-targets", "--all-features", "--locked", "--", "-D", "warnings")"#,
            r#""cargo" @("test", "--workspace", "--all-targets", "--locked")"#,
            r#""cargo" @("test", "--locked", "--test", "message_contracts", "auth_contract_returns_expected_fields")"#,
            r#""cargo" @("test", "--locked", "--test", "message_contracts", "compatibility_contract_client_bootstrap_flow_stays_stable")"#,
            r#""cargo" @("test", "--locked", "--test", "message_contracts", "protocol_contract_advertises_backward_compatible_version")"#,
            r#""cargo" @("test", "--locked", "--test", "message_contracts", "file_contract_relays_media_metadata_and_chunks")"#,
            r#""cargo" @("check", "--features", "discord-bridge", "--bin", "discord_bot", "--locked")"#,
            r#""cargo" @("check", "-p", "chatify-client", "--features", "bridge-client", "--locked")"#,
        ],
    );
}

#[test]
fn release_readiness_requires_clean_tracked_worktree_by_default() {
    let script = read_script("release-readiness.ps1");

    assert!(script.contains("[switch]$AllowDirty"));
    assert!(script.contains("git status --porcelain --untracked-files=no"));
    assert!(script.contains("Re-run with -AllowDirty"));
    assert!(script.contains("ci-local.ps1"));
    assert!(script.contains("ConvertTo-Json -Depth 8"));
}

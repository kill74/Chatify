# Contributing

Thanks for contributing to Chatify.

## Development Setup

1. Install stable Rust toolchain.
2. Clone the repository.
3. Build binaries:

   cargo check --workspace --bins --locked

## Quality Gates

Run all checks before opening a pull request:

1. cargo fmt --all --check
2. cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
3. cargo test --locked --test message_contracts auth_contract_returns_expected_fields
4. cargo test --locked --test message_contracts compatibility_contract_client_bootstrap_flow_stays_stable
5. cargo test --locked --test message_contracts protocol_contract_advertises_backward_compatible_version
6. cargo test --workspace --all-targets --locked

Optional feature check for Discord bridge:

1. cargo check --features discord-bridge --bin discord_bot --locked
2. cargo check -p clifford-client --features bridge-client --locked

## Branch and Commit Guidance

1. Create a focused branch from main.
2. Keep each commit scoped to one concern.
3. Use clear commit messages in imperative style.
4. Avoid mixing refactors and behavior changes in one commit.

## Pull Request Checklist

1. Explain what changed and why.
2. Include testing evidence.
3. Call out protocol or schema changes explicitly.
4. Update README and docs when behavior changes.

## Style Expectations

1. Prefer small functions with clear names.
2. Avoid duplicated logic; extract helpers.
3. Keep runtime-critical paths explicit and easy to trace.
4. Preserve backward compatibility unless a breaking change is intentional.

@echo off
setlocal enabledelayedexpansion

cd /d "C:\Users\Guilherme Sales\Documents\Projects\Chatify"

echo.
echo ========== TEST EXECUTION STARTED ==========
echo.

set "PASS=0"
set "FAIL=0"

REM CMD 1
echo [1/10] cargo check --workspace --bins --locked
cargo check --workspace --bins --locked
if !errorlevel! equ 0 (
  echo [1] PASS
  set /a PASS+=1
) else (
  echo [1] FAIL
  set /a FAIL+=1
)
echo.

REM CMD 2
echo [2/10] cargo fmt --all --check
cargo fmt --all --check
if !errorlevel! equ 0 (
  echo [2] PASS
  set /a PASS+=1
) else (
  echo [2] FAIL
  set /a FAIL+=1
)
echo.

REM CMD 3
echo [3/10] cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
if !errorlevel! equ 0 (
  echo [3] PASS
  set /a PASS+=1
) else (
  echo [3] FAIL
  set /a FAIL+=1
)
echo.

REM CMD 4
echo [4/10] cargo test --workspace --all-targets --locked
cargo test --workspace --all-targets --locked
if !errorlevel! equ 0 (
  echo [4] PASS
  set /a PASS+=1
) else (
  echo [4] FAIL
  set /a FAIL+=1
)
echo.

REM CMD 5
echo [5/10] cargo test --locked --test message_contracts auth_contract_returns_expected_fields
cargo test --locked --test message_contracts auth_contract_returns_expected_fields
if !errorlevel! equ 0 (
  echo [5] PASS
  set /a PASS+=1
) else (
  echo [5] FAIL
  set /a FAIL+=1
)
echo.

REM CMD 6
echo [6/10] cargo test --locked --test message_contracts compatibility_contract_client_bootstrap_flow_stays_stable
cargo test --locked --test message_contracts compatibility_contract_client_bootstrap_flow_stays_stable
if !errorlevel! equ 0 (
  echo [6] PASS
  set /a PASS+=1
) else (
  echo [6] FAIL
  set /a FAIL+=1
)
echo.

REM CMD 7
echo [7/10] cargo test --locked --test message_contracts protocol_contract_advertises_backward_compatible_version
cargo test --locked --test message_contracts protocol_contract_advertises_backward_compatible_version
if !errorlevel! equ 0 (
  echo [7] PASS
  set /a PASS+=1
) else (
  echo [7] FAIL
  set /a FAIL+=1
)
echo.

REM CMD 8
echo [8/10] cargo test --locked --test message_contracts file_contract_relays_media_metadata_and_chunks
cargo test --locked --test message_contracts file_contract_relays_media_metadata_and_chunks
if !errorlevel! equ 0 (
  echo [8] PASS
  set /a PASS+=1
) else (
  echo [8] FAIL
  set /a FAIL+=1
)
echo.

REM CMD 9
echo [9/10] cargo check --features discord-bridge --bin discord_bot --locked
cargo check --features discord-bridge --bin discord_bot --locked
if !errorlevel! equ 0 (
  echo [9] PASS
  set /a PASS+=1
) else (
  echo [9] FAIL
  set /a FAIL+=1
)
echo.

REM CMD 10
echo [10/10] cargo check -p chatify-client --features bridge-client --locked
cargo check -p chatify-client --features bridge-client --locked
if !errorlevel! equ 0 (
  echo [10] PASS
  set /a PASS+=1
) else (
  echo [10] FAIL
  set /a FAIL+=1
)
echo.

echo.
echo ========== SUMMARY ==========
echo PASS: !PASS!
echo FAIL: !FAIL!
echo =============================

endlocal

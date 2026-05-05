$results = @()

# Set working directory
cd "C:\Users\Guilherme Sales\Documents\Projects\Chatify"

# Command 1: cargo check --workspace --bins --locked
Write-Host "===== Command 1: cargo check --workspace --bins --locked =====" -ForegroundColor Cyan
$output = cargo check --workspace --bins --locked 2>&1
$exitCode = $LASTEXITCODE
Write-Host "Exit Code: $exitCode"
$results += @{
    number = 1
    command = "cargo check --workspace --bins --locked"
    exit_code = $exitCode
    status = @("FAIL","PASS")[$exitCode -eq 0]
    diagnostic = @("Build check failed","")[$exitCode -eq 0]
    stderr_snippet = @(($output | Select-Object -Last 15 | Out-String).Trim(),"")[$exitCode -eq 0]
}

# Command 2: cargo fmt --all --check
Write-Host "`n===== Command 2: cargo fmt --all --check =====" -ForegroundColor Cyan
$output = cargo fmt --all --check 2>&1
$exitCode = $LASTEXITCODE
Write-Host "Exit Code: $exitCode"
$results += @{
    number = 2
    command = "cargo fmt --all --check"
    exit_code = $exitCode
    status = @("FAIL","PASS")[$exitCode -eq 0]
    diagnostic = @("Format check failed","")[$exitCode -eq 0]
    stderr_snippet = @(($output | Select-Object -Last 15 | Out-String).Trim(),"")[$exitCode -eq 0]
}

# Command 3: cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
Write-Host "`n===== Command 3: cargo clippy --workspace --all-targets --all-features --locked -- -D warnings =====" -ForegroundColor Cyan
$output = cargo clippy --workspace --all-targets --all-features --locked -- -D warnings 2>&1
$exitCode = $LASTEXITCODE
Write-Host "Exit Code: $exitCode"
$results += @{
    number = 3
    command = "cargo clippy --workspace --all-targets --all-features --locked -- -D warnings"
    exit_code = $exitCode
    status = @("FAIL","PASS")[$exitCode -eq 0]
    diagnostic = @("Clippy warnings detected","")[$exitCode -eq 0]
    stderr_snippet = @(($output | Select-Object -Last 15 | Out-String).Trim(),"")[$exitCode -eq 0]
}

# Command 4: cargo test --workspace --all-targets --locked
Write-Host "`n===== Command 4: cargo test --workspace --all-targets --locked =====" -ForegroundColor Cyan
$output = cargo test --workspace --all-targets --locked 2>&1
$exitCode = $LASTEXITCODE
Write-Host "Exit Code: $exitCode"
$results += @{
    number = 4
    command = "cargo test --workspace --all-targets --locked"
    exit_code = $exitCode
    status = @("FAIL","PASS")[$exitCode -eq 0]
    diagnostic = @("Tests failed","")[$exitCode -eq 0]
    stderr_snippet = @(($output | Select-Object -Last 15 | Out-String).Trim(),"")[$exitCode -eq 0]
}

# Command 5: cargo test --locked --test message_contracts auth_contract_returns_expected_fields
Write-Host "`n===== Command 5: cargo test --locked --test message_contracts auth_contract_returns_expected_fields =====" -ForegroundColor Cyan
$output = cargo test --locked --test message_contracts auth_contract_returns_expected_fields 2>&1
$exitCode = $LASTEXITCODE
Write-Host "Exit Code: $exitCode"
$results += @{
    number = 5
    command = "cargo test --locked --test message_contracts auth_contract_returns_expected_fields"
    exit_code = $exitCode
    status = @("FAIL","PASS")[$exitCode -eq 0]
    diagnostic = @("Test failed","")[$exitCode -eq 0]
    stderr_snippet = @(($output | Select-Object -Last 15 | Out-String).Trim(),"")[$exitCode -eq 0]
}

# Command 6: cargo test --locked --test message_contracts compatibility_contract_client_bootstrap_flow_stays_stable
Write-Host "`n===== Command 6: cargo test --locked --test message_contracts compatibility_contract_client_bootstrap_flow_stays_stable =====" -ForegroundColor Cyan
$output = cargo test --locked --test message_contracts compatibility_contract_client_bootstrap_flow_stays_stable 2>&1
$exitCode = $LASTEXITCODE
Write-Host "Exit Code: $exitCode"
$results += @{
    number = 6
    command = "cargo test --locked --test message_contracts compatibility_contract_client_bootstrap_flow_stays_stable"
    exit_code = $exitCode
    status = @("FAIL","PASS")[$exitCode -eq 0]
    diagnostic = @("Test failed","")[$exitCode -eq 0]
    stderr_snippet = @(($output | Select-Object -Last 15 | Out-String).Trim(),"")[$exitCode -eq 0]
}

# Command 7: cargo test --locked --test message_contracts protocol_contract_advertises_backward_compatible_version
Write-Host "`n===== Command 7: cargo test --locked --test message_contracts protocol_contract_advertises_backward_compatible_version =====" -ForegroundColor Cyan
$output = cargo test --locked --test message_contracts protocol_contract_advertises_backward_compatible_version 2>&1
$exitCode = $LASTEXITCODE
Write-Host "Exit Code: $exitCode"
$results += @{
    number = 7
    command = "cargo test --locked --test message_contracts protocol_contract_advertises_backward_compatible_version"
    exit_code = $exitCode
    status = @("FAIL","PASS")[$exitCode -eq 0]
    diagnostic = @("Test failed","")[$exitCode -eq 0]
    stderr_snippet = @(($output | Select-Object -Last 15 | Out-String).Trim(),"")[$exitCode -eq 0]
}

# Command 8: cargo test --locked --test message_contracts file_contract_relays_media_metadata_and_chunks
Write-Host "`n===== Command 8: cargo test --locked --test message_contracts file_contract_relays_media_metadata_and_chunks =====" -ForegroundColor Cyan
$output = cargo test --locked --test message_contracts file_contract_relays_media_metadata_and_chunks 2>&1
$exitCode = $LASTEXITCODE
Write-Host "Exit Code: $exitCode"
$results += @{
    number = 8
    command = "cargo test --locked --test message_contracts file_contract_relays_media_metadata_and_chunks"
    exit_code = $exitCode
    status = @("FAIL","PASS")[$exitCode -eq 0]
    diagnostic = @("Test failed","")[$exitCode -eq 0]
    stderr_snippet = @(($output | Select-Object -Last 15 | Out-String).Trim(),"")[$exitCode -eq 0]
}

# Command 9: cargo check --features discord-bridge --bin discord_bot --locked
Write-Host "`n===== Command 9: cargo check --features discord-bridge --bin discord_bot --locked =====" -ForegroundColor Cyan
$output = cargo check --features discord-bridge --bin discord_bot --locked 2>&1
$exitCode = $LASTEXITCODE
Write-Host "Exit Code: $exitCode"
$results += @{
    number = 9
    command = "cargo check --features discord-bridge --bin discord_bot --locked"
    exit_code = $exitCode
    status = @("FAIL","PASS")[$exitCode -eq 0]
    diagnostic = @("Build check failed","")[$exitCode -eq 0]
    stderr_snippet = @(($output | Select-Object -Last 15 | Out-String).Trim(),"")[$exitCode -eq 0]
}

# Command 10: cargo check -p chatify-client --features bridge-client --locked
Write-Host "`n===== Command 10: cargo check -p chatify-client --features bridge-client --locked =====" -ForegroundColor Cyan
$output = cargo check -p chatify-client --features bridge-client --locked 2>&1
$exitCode = $LASTEXITCODE
Write-Host "Exit Code: $exitCode"
$results += @{
    number = 10
    command = "cargo check -p chatify-client --features bridge-client --locked"
    exit_code = $exitCode
    status = @("FAIL","PASS")[$exitCode -eq 0]
    diagnostic = @("Build check failed","")[$exitCode -eq 0]
    stderr_snippet = @(($output | Select-Object -Last 15 | Out-String).Trim(),"")[$exitCode -eq 0]
}

Write-Host "`n===== FINAL JSON RESULTS =====" -ForegroundColor Green
$results | ConvertTo-Json

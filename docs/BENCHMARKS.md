# Benchmarks

## Purpose

This document defines how Chatify performance is measured and reported.
Use this methodology to produce reproducible numbers instead of anecdotal claims.

## Metrics

Track at least the following per test run:

1. End-to-end message latency: p50, p95, p99.
2. Throughput: messages per second sustained.
3. Server memory footprint under load.
4. CPU usage for server and one representative client.
5. Reconnect recovery time (bridge and client).

## Test Scenarios

### Scenario A: Local Baseline

1. One server on localhost.
2. One sender client, one receiver client.
3. 1,000 messages fixed payload size.

### Scenario B: Concurrency

1. One server.
2. 10 to 50 concurrent clients.
3. Mixed broadcast and direct messages.

### Scenario C: Degraded Network

1. Inject latency and packet loss at OS/network layer.
2. Validate latency tail behavior and reconnect stability.

## Environment Recording

For every benchmark report, record:

1. Git commit SHA.
2. Rust version.
3. OS and kernel build.
4. CPU model and memory.
5. Benchmark command and client counts.

## Recommended Command Baseline

Build release binaries before measuring:

```bash
cargo build --release
```

Run server:

```bash
./target/release/clicord-server --host 127.0.0.1 --port 8765
```

Run client workloads from separate terminals or benchmark harnesses.

## Reporting Template

Use this table format in PRs and release notes.

| Commit | Scenario | Clients | p50 Latency (ms) | p95 Latency (ms) | p99 Latency (ms) | Throughput (msg/s) | Notes          |
| :----- | :------- | ------: | ---------------: | ---------------: | ---------------: | -----------------: | :------------- |
| sha123 | A        |       2 |               12 |               25 |               44 |               1800 | Baseline local |

## Regression Policy

Treat these as release blockers:

1. p95 latency regression greater than 20 percent without accepted trade-off.
2. Throughput regression greater than 15 percent without accepted trade-off.
3. Reconnect instability introduced in bridge regression paths.

## Next Step

Add an automated benchmark harness and publish trend data across tagged releases.

# Local Demo

This demo is meant for a quick two-terminal review. It starts Chatify with a temporary SQLite database and a generated DB key, so it does not touch local project data or runtime secrets.

## Start The Server

From the repository root:

```powershell
.\scripts\demo-local.ps1
```

Leave this terminal open. The script prints the exact client command with the selected port and keeps the server running until you press `Ctrl+C`.

For a throwaway free port:

```powershell
.\scripts\demo-local.ps1 -Port 0
```

For release-mode binaries:

```powershell
.\scripts\demo-local.ps1 -Release
```

## Connect A Client

Open a second terminal and run the client command printed by the demo script. It will look like this:

```powershell
cargo run -p chatify-client --bin chatify-client -- --host 127.0.0.1 --port 8765
```

Open a third terminal with the same command if you want to see multi-client broadcast behavior.

## Walkthrough Commands

Use these in the client:

```text
/join general
hello from the local demo
/history #general 20
/search #general demo limit=10
/fingerprint bob
/doctor
```

What this shows:

- WebSocket server/client flow works locally.
- Timeline events are persisted and queryable.
- Search uses the protocol path documented in the contract tests.
- Trust and diagnostics commands are visible from the terminal UX.

## Dry Run

Use `-DryRun` to verify the generated commands without launching a server:

```powershell
.\scripts\demo-local.ps1 -Port 0 -DryRun
```

## Cleanup

The script stores demo files under the system temp directory and removes them when the server exits. If the terminal is force-closed, delete any stale `chatify-demo-*` directory from the temp folder.

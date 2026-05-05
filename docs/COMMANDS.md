# Client Commands

| Command                                                           | Description                                                        |
| ----------------------------------------------------------------- | ------------------------------------------------------------------ |
| `/commands [filter]`                                              | List commands, optionally filtered by keyword                      |
| `/help [command]`                                                 | Show command help (general or per-command detail)                  |
| `/join <channel>`                                                 | Join or switch to a channel                                        |
| `/switch <channel>`                                               | Alias for `/join`                                                  |
| `/leave [channel]`                                                | Leave a channel (defaults to current channel)                      |
| `/part [channel]`                                                 | Alias for `/leave`                                                 |
| `/history [ch\|dm:user] [limit]`                                  | Load channel or DM history                                         |
| `/search [#ch\|dm:user] <query> [limit=N]`                        | Search timeline events                                             |
| `/replay <from_ts> [#ch\|dm:user] [limit=N]`                      | Replay events from a unix timestamp                                |
| `/users`                                                          | Refresh online users and key directory                             |
| `/metrics`                                                        | Show runtime counters plus DB pool and DB latency summaries        |
| `/db-profile` or `/dbprofile`                                     | Show focused DB latency profile and alerts                         |
| `/doctor [--json]`                                                | Run connection diagnostics (DNS, TCP, WebSocket, auth readiness)   |
| `/typing [on\|off] [#ch\|dm:user]`                                | Broadcast ephemeral typing state                                   |
| `/voice <on\|off\|mute\|unmute\|deafen\|undeafen\|status> [room]` | Control voice session and signaling                                |
| `/screen <start\|stop\|status> [room]`                            | Control screen-share signaling                                     |
| `/notify [target] [on\|off]`                                      | Manage notification settings, exports, diagnostics, and probes     |
| `/plugin [list\|install <plugin>\|disable <plugin>]`              | List, install, or disable server-side plugins                      |
| `/bridge status`                                                  | Show connected bridge instances and route health (`bridge-client`) |
| `/dm <user> <message>`                                            | Send encrypted direct message (trust-verified)                     |
| `/fingerprint [user]`                                             | Show peer key fingerprint(s) and trust status                      |
| `/trust <user> <fingerprint>`                                     | Mark a peer fingerprint as trusted after out-of-band verification  |
| `/trust-audit [n]`                                                | Show recent trust audit entries                                    |
| `/trust-export [path]`                                            | Export deterministic trust audit JSON                              |
| `/recent [n]`                                                     | Show recent message IDs for quick reaction targeting               |
| `/reply <msg_id\|#index> <message>`                               | Reply to a channel message by stable `msg_id` or recent index      |
| `/react <msg_id\|#index> <emoji>`                                 | React to a message by stable `msg_id` or recent index              |
| `/sync`                                                           | Request reaction sync for the active channel                       |
| `/image "<path>"`                                                 | Send an image file to the active channel                           |
| `/video "<path>"`                                                 | Send a video file to the active channel                            |
| `/audio "<path>"`                                                 | Send a short audio note to the active channel                      |
| `/quit` `/exit` `/q`                                              | Disconnect and exit                                                |

Any non-command text is sent as a channel message to the active scope.

## TUI Shortcuts

| Shortcut       | Description                                      |
| -------------- | ------------------------------------------------ |
| `Ctrl+K`       | Open searchable actions for common tasks         |
| `Ctrl+,`       | Open settings toggles                            |
| `Enter`        | Send the composer text or run selected action    |
| `Tab`          | Complete the current `@mention` suggestion       |
| `PageUp/Down`  | Scroll the timeline                              |
| `Alt+Up/Down`  | Switch rooms quickly                             |
| `Ctrl+C`       | Quit                                             |

## Click-First Call and Media Controls

The TUI exposes common call and media actions as clickable controls, so users
do not need to memorize slash commands for the main flows.

- Click `Call` / `Join call` to enter voice for the active room. From a DM, it
  uses the same default as `/voice on` and joins `general`.
- While in voice, click `Leave`, `Mic`/`Mute`, and `Sound`/`Deafen` to control
  the call.
- Click `Share` / `Stop share` to start or stop screen-share signaling.
- Click `Image`, `Video`, or `Audio` in the Now panel to prefill the matching
  upload command with the cursor inside the quoted path.
- Received audio notes keep their inline `Play` button.

Slash commands remain available as a fallback and for scripting.

## Click-First Chat Controls

Daily chat actions are also clickable in the TUI:

- Click a room or DM in `Chats` to switch there without typing `/join`.
- Click a person in the `Now` panel to open a DM draft.
- Click `Reply` or `React` beside a channel message to prefill the exact
  message ID.
- Click `Search` to prefill search for the active conversation.
- Click `Users` to refresh presence and key directory.
- Click `Settings` or press `Ctrl+,` to toggle media, markdown, notifications,
  sound, reconnect, and animations. Changes are saved to `config.toml`.

The TUI adapts to terminal width: narrow windows prioritize the chat list,
timeline, and composer; wider windows show a compact contextual panel for media,
mentions, people, voice state, and recent activity.

The visual layout is intentionally quiet: status, chat actions, call controls,
media actions, people, and recent activity are grouped in predictable sections
so the conversation stays easy to scan.

### Mentions and Notifications

- Mention users with `@username` inside message text.
- The client highlights mentions addressed to the current user.
- Desktop notifications are controlled by `notifications.*` config flags:
  - `notifications.on_mention` for mention alerts.
  - `notifications.on_dm` for incoming DM alerts.
  - `notifications.on_all_messages` for broad message alerts.
- `/notify` lets you inspect and toggle these settings from the client runtime.
- Valid `/notify` targets: `enabled`, `dm`, `mention`, `all`, `sound`.
- Use `/notify <target>` to inspect a single notification setting.
- Use `/notify reset` to restore notification settings to defaults.
- Use `/notify export [--redact] [path|stdout]` to write a settings snapshot, optionally masking profile identifiers.
- Use `/notify doctor [--json]` to print a quick diagnostics report in text or JSON format.
- Use `/notify test [sound] [info|warning|critical] [message]` to trigger a one-time desktop notification probe.
- Use `/doctor [--json]` for connection diagnostics (DNS, TCP, WebSocket, auth readiness).

### Reactions and Message IDs

- Every channel message now includes a stable `msg_id` in the protocol payload.
- Replies are sent on channel messages with `reply_to` and may include compact quoted context.
- Reactions are sent as `reaction` events and aggregated per `(msg_id, emoji)`.
- Clients can bootstrap reaction state with `reaction_sync` after join/reconnect.
- For terminal UX, `/reply #1 thanks` targets the most recent visible message ID.
- For terminal UX, `/react #1 +1` targets the most recent visible message ID.

### Runtime and Database Profiling

- `/metrics` returns runtime traffic counters, cache hit-rate, DB pool stats, DB top operations (p50/p95/p99/avg), and DB alerts.
- `/db-profile` (or `/dbprofile`) returns a DB-focused view with top operations, pool pressure, and latency alerts.
- Current default DB latency budget thresholds are `warning_p95=50ms`, `critical_p95=200ms`, with `min_samples=5`.

### Connection Diagnostics and Recovery

- Automatic reconnect is enabled by default and uses bounded exponential backoff (`1s` to `30s`).
- Use `--no-reconnect` to disable reconnect retries for deterministic failure behavior.
- Use `/doctor` to run an on-demand diagnostics pass (auth readiness, DNS, TCP reachability, WebSocket handshake).
- Use `/doctor --json` for structured diagnostics output suitable for scripts and support bundles.
- Outbound messages are buffered while reconnecting; the queue is bounded to avoid unbounded memory growth.

### Media Transfer

Media transfer uses `file_meta` + `file_chunk` framing over the existing WebSocket connection — no separate HTTP endpoint and no second auth context.

```text
/image "/path/to/screenshot.png"
/video "/path/to/demo.mp4"
/audio "/path/to/voice-note.ogg"
```

Received files are written to:

- **Windows:** `%APPDATA%\Chatify\media\`
- **Linux/macOS:** `$HOME/.chatify/media/`

Image transfers render an ASCII preview inline in the terminal feed. Audio notes show an inline `Play` button in the TUI after the file is received; pending notes show `Receiving...`, and missing local files show `Unavailable`. Video transfers produce a metadata card (sender, filename, size, local path). The 100 MB cap is enforced at the application layer on the sender side.

The client exposes `/image`, `/video`, and `/audio` directly. Audio playback is click-to-play in the TUI, not a slash command. Each upload is chunked over WebSocket and bounded by the 100 MB sender-side cap.


## Related Docs

- [README.md](../README.md) - project overview and quick start
- [ARCHITECTURE.md](ARCHITECTURE.md) - component design and protocol flow

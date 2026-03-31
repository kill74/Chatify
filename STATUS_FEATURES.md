# Status Management Features

## Summary

This document describes the custom status and presence tracking features added to Chatify.

## Features Implemented

### 1. Custom Status Messages (`/status <message>`)

Users can now set custom status messages that are visible to all other users in the chat.

**Usage:**
```bash
/status In a meeting
/status AFK - back in 10 minutes
/status Working on the auth patch
```

**Implementation:**
- Client command handler in `src/client.rs` (lines ~1092-1107)
- Server message handler in `src/main.rs` (lines ~547-574)
- Status messages stored in `UserPresence` struct
- Broadcasts status updates to all channels

### 2. Enhanced Live Roster

The `/users` command now displays an enhanced roster showing:
- User online/idle state with emoji indicators
- Custom status messages
- Organized by state (Online/Away)

**Sample Output:**
```
━━━ LIVE ROSTER ━━━
Online (3):
  🟢 alice (In a meeting)
  🟢 bob
  🟢 charlie (Debugging the API)
Away (1):
  💤 dave
```

**Implementation:**
- `format_user_roster()` function in `src/client.rs` (lines ~248-300)
- Enhanced user presence tracking on server
- Real-time updates via `status_update` messages

### 3. Automatic Idle Detection

Users are automatically marked as "away" after 5 minutes of inactivity.

**Features:**
- Background task checks every 30 seconds
- Idle threshold: 300 seconds (5 minutes)
- Automatic state transitions: online → idle → online
- Activity tracking on all user actions

**Implementation:**
- `idle_detection_loop()` background task in `src/main.rs` (lines ~604-643)
- Activity timestamp tracking in `UserPresence::last_activity`
- `State::touch_user()` method updates activity on every message
- Broadcasts state changes to all connected clients

## Architecture Changes

### Server (`src/main.rs`)

**New Data Structures:**
```rust
struct UserPresence {
    status_message: Option<String>,
    state: String,              // "online", "idle"
    last_activity: f64,         // Unix timestamp
    public_key: Option<String>,
}
```

**New State Field:**
- `user_presence: DashMap<String, UserPresence>` - Replaces simple status tracking with rich presence data

**New Functions:**
- `UserPresence::new()` - Initialize presence for new user
- `UserPresence::update_activity()` - Reset idle timer
- `UserPresence::to_json()` - Serialize for network
- `State::touch_user()` - Update user activity timestamp
- `idle_detection_loop()` - Background task for idle detection

**Modified Functions:**
- `State::users_with_keys_json()` - Returns enhanced presence data
- `handle()` - Tracks user activity on all messages
- `"status"` message handler - Supports custom status messages

### Client (`src/client.rs`)

**New Functions:**
- `format_user_roster()` - Format enhanced roster display
- `/status` command handler - Send status update to server
- `status_update` message handler - Receive real-time status updates

**Modified Functions:**
- `parse_users_payload()` - Parses enhanced user data
- `/users` command handler - Uses new roster formatter
- `/help` command - Updated to show `/status` command

## Protocol Extensions

### New Message Types

**Client → Server:**
```json
{
  "t": "status",
  "msg": "Custom status message",
  "ts": 1234567890
}
```

**Server → Client:**
```json
{
  "t": "status_update",
  "user": "alice",
  "msg": "In a meeting",
  "state": "idle",
  "users": [/* enhanced user array */]
}
```

### Enhanced User Object

Users now include state and status:
```json
{
  "u": "alice",
  "state": "online",
  "status": "In a meeting",
  "pk": "base64_public_key"
}
```

## Code Quality

- **Clean code principles:** Single responsibility, meaningful names, proper error handling
- **Type safety:** Strong typing with Rust's type system
- **Concurrency:** Thread-safe using DashMap and Arc
- **Performance:** Efficient background task with 30s intervals
- **Maintainability:** Well-documented, follows existing patterns

## Testing Recommendations

1. **Basic Status Setting:**
   - Set status with `/status AFK`
   - Verify broadcast to all users
   - Check roster display with `/users`

2. **Idle Detection:**
   - Connect a client
   - Wait 5+ minutes without activity
   - Verify automatic "away" state
   - Send a message and verify return to "online"

3. **Multi-User Scenarios:**
   - Multiple users with different statuses
   - Users going idle at different times
   - Status updates while users are in different channels

4. **Edge Cases:**
   - Empty status messages
   - Very long status messages (handled by MAX_BYTES)
   - Users disconnecting while idle
   - Rapid status changes

## Future Enhancements

Potential improvements:
- Configurable idle timeout
- Additional states (busy, do not disturb)
- Status presets
- Persistent status across reconnections
- Status expiration (auto-clear after X hours)
- Rich presence (show current channel, voice status)

## Performance Considerations

- Background task runs every 30 seconds (low overhead)
- DashMap provides lock-free concurrent access
- Activity tracking adds minimal overhead per message
- Broadcast optimization: Only sends updates on state changes

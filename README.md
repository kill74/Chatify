# chatify

```
        __          __  _ ____
   _____/ /_  ____ _/ /_(_) __/_  __
  / ___/ __ \/ __ `/ __/ / /_/ / / /
 /__/ / / / /_/ / /_/ / __/ /_/ /
 \___/_/ /_/\__,_/\__/_/_/  \__, /
                           /____/
```

A Discord-like chat app that runs entirely in your terminal. Self-hosted on your own machine — no accounts, no cloud, no IP leaks, no telemetry. End-to-end encrypted. Rust server and client.

---

## Table of Contents

- [Features](#features)
- [Requirements](#requirements)
- [Installation](#installation)
- [Running the Server](#running-the-server)
- [Connecting as a Client](#connecting-as-a-client)
- [Commands](#commands)
- [How Encryption Works](#how-encryption-works)
- [Privacy Guarantees](#privacy-guarantees)
- [Voice Chat](#voice-chat)
- [Sending Images](#sending-images)
- [Performance](#performance)
- [Advanced: Tor Hidden Service](#advanced-tor-hidden-service)
- [Advanced: TLS Transport](#advanced-tls-transport)
- [Building from Source](#building-from-source)
- [File Structure](#file-structure)
- [Troubleshooting](#troubleshooting)
- [License](#license)
- [Logging and Testing](#logging-and-testing)

---

## Features

- **Multi-channel text chat** — create any channel on the fly with `/join`
- **End-to-end encryption** — server only ever relays ciphertext, never plaintext
- **Encrypted direct messages** — DMs use a DH-derived key the server cannot read
- **Image sharing** — send any image and it auto-converts to ASCII art before sending
- **Real-time voice chat** — per-channel voice rooms with VAD (silence detection)
- **Zero IP leaks** — server never forwards client IPs to other clients
- **No disk writes** — all history in RAM as ciphertext, nothing persisted to disk
- **No accounts** — shared password + username you pick at login, that's it
- **Self-hosted** — your machine, your data, your rules
- **Emotes** — `/me` action messages
- **Colour-coded usernames** — deterministic per-user colours so you can tell people apart
- **Channel history** — last 50 messages cached per channel (still encrypted) so late joiners get context

---

## Requirements

**Server and Client** (everyone):
- Rust toolchain (to build from source) — any OS that supports Rust
- Optional: For voice chat, system audio libraries (see below)

---

## Installation

### 1. Install Rust

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source ~/.cargo/env
```

### 2. (Optional) Install voice chat dependencies

Voice is entirely optional — all other features work without it.

**Ubuntu / Debian:**
```bash
sudo apt install portaudio19-dev
```

**macOS:**
```bash
brew install portaudio
```

**Windows:**
No additional system packages needed; the Rust crates will work with the default audio system.

---

## Building from Source

```bash
git clone <repository-url>
cd chatify
cargo build --release
# binaries will be at:
#   target/release/clicord-server
#   target/release/clicord-client
```

`Cargo.lock` is included so you get the exact same dependency versions.

---

## Running the Server

One person in your group runs this. Everyone else connects to their IP.

```bash
./target/release/clicord-server
```

You'll be prompted to set a password. Share it with your group over a secure channel (Signal, in person, etc.). It's never written to disk — memory only, gone when the server stops.

**Options:**

```bash
./target/release/clicord-server --host 0.0.0.0 --port 8765    # default: all interfaces
./target/release/clicord-server --host 127.0.0.1              # localhost only (for Tor)
./target/release/clicord-server --port 9000                   # custom port
```

**Example output:**

```
server password:
chatify running on ws://0.0.0.0:8765
encryption: chacha20-poly1305 + x25519 | ip privacy: on
ctrl+c to stop

[19:04:01] + alice
[19:04:03] + bob
```

The server logs joins and leaves only. No messages, no IPs, nothing else.

> **Internet connections:** If people are connecting from outside your network, forward TCP 8765 on your router to your machine's local IP. Run `curl ifconfig.me` to find your public IP and share that with your group.

---

## Connecting as a Client

```bash
./target/release/clicord-client
```

Enter your username and the server password. If the username is taken, the server appends a number (`alice_1`, etc.).

**Remote server:**

```bash
./target/release/clicord-client --host 192.168.1.50              # LAN
./target/release/clicord-client --host 12.34.56.78               # internet
./target/release/clicord-client --host 12.34.56.78 --port 9000   # custom port
./target/release/clicord-client --host example.com --tls         # with TLS
```

**After connecting:**

```
chatify  logged in as alice  │  #general  │  3 online
type /help for commands
──────────────────────────────────────────────────────
── history ────────────────────────────────────────────
19:01  bob  hey, you there?
19:02  carol  yeah it works
──────────────────────────────────────────────────────
[alice@#general]›
```

Type to chat. Use `/` to run commands.

---

## Commands

| Command | Description |
|---|---|
| `/join <channel>` | Switch to a channel. Creates it if it doesn't exist. Lowercase, no spaces. |
| `/dm <user> <message>` | Send an encrypted direct message. Only you and the recipient can read it. |
| `/image <path>` | Send an image file — auto-converts to ASCII art. Supports JPEG, PNG, GIF, BMP, WEBP. |
| `/me <action>` | Emote. Shows as `* alice does something`. |
| `/users` | List everyone currently online. |
| `/channels` | List all channels on the server. |
| `/voice` | Toggle voice chat for your current channel. Run again to leave. |
| `/clear` | Clear the terminal. |
| `/help` | Show command reference. |
| `/edit <new_text>` | Edit your last sent message in the current channel. |
| `/quit` | Disconnect and exit. Also: `/exit`, `/q`. |

**Examples:**
```
/join off-topic
/dm bob hey check your dms
/image ~/Downloads/meme.jpg
/image "./file with spaces.png"
/me is grabbing coffee brb
/voice
```

---

## How Encryption Works

Everything is encrypted on your machine before it hits the network. The server is a blind relay — it routes ciphertext and nothing else.

### Channel Messages

```
password + channel name
        │
        ▼
  PBKDF2-SHA256
  120,000 iterations
  salt: "chatify:<channel>"
        │
        ▼
   32-byte symmetric key
        │
        ▼
   ChaCha20-Poly1305
   + random 12-byte nonce (per message)
        │
        ▼
   base64(nonce + ciphertext)
        │
        ▼
   server relays  ──►  all channel members decrypt locally
```

Key points:
- 120k PBKDF2 iterations makes brute-forcing from captured traffic expensive
- Each channel gets a unique key even with the same password (different salt per channel name)
- ChaCha20-Poly1305 is authenticated encryption — tampered messages fail to decrypt and are dropped
- Fresh nonce per message means identical plaintexts produce different ciphertext

### Direct Messages

DMs use **X25519 Elliptic Curve Diffie-Hellman** key exchange:

```
alice: X25519 keypair (priv_a, pub_a)
bob:   X25519 keypair (priv_b, pub_b)

pub_a ──► server broadcasts ◄── pub_b   (server only sees public keys)

alice: DH(priv_a, pub_b) → SHA-256 → key
bob:   DH(priv_b, pub_a) → SHA-256 → same key   ← DH math guarantees this

DM: ChaCha20-Poly1305(key) → server relays ciphertext → bob decrypts
```

The server has both public keys but cannot compute the shared secret — that's the fundamental guarantee of Diffie-Hellman. Only the two endpoints can derive it.

Keypairs are regenerated every session for session-level forward secrecy.

### What the Server Can and Cannot See

| Data | Server can read it? |
|------|---------------------|
| Usernames | ✅ Yes — routing requires it |
| Channel names | ✅ Yes — routing requires it |
| Message content | ❌ No — ciphertext only |
| DM content | ❌ No — different key, cannot derive |
| Your IP address | ❌ Never forwarded to other clients |
| Voice audio | ❌ No — encrypted with channel key |
| Images | ❌ No — encrypted with channel key |

---

## Privacy Guarantees

**No IP leaks.**
The server never includes remote addresses in any relayed payload. Other users only ever see your username.

**No disk writes.**
Chat history is a `VecDeque` in RAM, stored as ciphertext. No database, no log files, no persistence layer of any kind. When the server stops, everything is gone.

**No telemetry.**
The server binary makes zero outbound connections after startup. No analytics, no pinging an update server, no phoning home. Verify with `strace -e network ./target/release/clicord-server` or `ss -tp` if you want to be sure.

**No accounts.**
No email, no phone number, no registration. Username + shared password — that's the entire auth model.

**Fully auditable.**
All source code is included. The server and client are Rust programs. Read every line yourself.

---

## Voice Chat

```
/voice    # join voice in current channel
/voice    # run again to leave
```

Voice is channel-scoped. Each channel has its own voice room. You can be in `#general` voice while reading `#off-topic` text.

**How it works under the hood:**
- Mic captured at **16kHz mono**, 960-sample chunks (~60ms each)
- **VAD (voice activity detection):** chunks below RMS 350 are silently dropped — no bandwidth wasted on silence or background hum
- Audio encrypted with the channel key before sending
- Server relays to all other voice room members
- Playback queue holds up to 40 chunks (~2.4s) to absorb network jitter
- Capture and playback run in separate background threads so the chat UI stays responsive

**Bandwidth:** ~128–256 kbps when talking, near zero when silent.

---

## Sending Images

```
/image /absolute/path/to/photo.jpg
/image ~/Desktop/meme.png
/image "./folder with spaces/image.gif"
```

**Supported formats:** JPEG, PNG, GIF, BMP, WEBP, and anything else `image` crate can open.

**Conversion pipeline:**
1. Alpha transparency composited over a black background
2. Resized to **70 characters wide** using Lanczos resampling
3. Height calculated with a **0.45 correction factor** for the typical 2:1 height-to-width ratio of monospace terminal characters
4. Converted to greyscale
5. Each pixel's brightness mapped to one of 70 characters — `$@B%8&WM#*...` (darkest) through to ` ` (lightest)
6. The ASCII string is encrypted and sent as a regular channel message

Displays inline framed with box-drawing characters. Looks best at 100+ terminal columns wide.

---

## Performance

| Metric | chatify (Rust) |
|--------|----------------|
| Binary size | ~3-5 MB stripped (server and client similar) |
| RAM at idle | ~2-5 MB |
| Startup time | <10 ms |
| Message relay latency | <1 ms (LAN) |
| GC pauses | None |

**Why it's fast:**

Each channel has a `tokio::sync::broadcast::Sender<String>`. When a message arrives, sending it to all subscribers is a single atomic operation — no loops, no per-message locking. `DashMap` provides lock-free concurrent reads for user and channel lookups. Tokio schedules thousands of WebSocket connections as lightweight async tasks sharing a thread pool sized to your CPU. No OS thread per connection, no GC, no interpreter.

---

## Advanced: Tor Hidden Service

Run as a Tor hidden service so neither side learns the other's real IP. The server won't know who's connecting, and clients won't know where the server is.

**1. Bind to localhost only:**
```bash
./target/release/clicord-server --host 127.0.0.1 --port 8765
```

**2. Add to `/etc/tor/torrc`:**
```
HiddenServiceDir /var/lib/tor/chatify/
HiddenServicePort 8765 127.0.0.1:8765
```

**3. Restart Tor:**
```bash
sudo systemctl restart tor
```

**4. Get your .onion address:**
```bash
sudo cat /var/lib/tor/chatify/hostname
# → ab3cde4fgh567ijk.onion
```

**5. Clients connect with torsocks:**
```bash
torsocks ./target/release/clicord-client --host ab3cde4fgh567ijk.onion --port 8765
```

Combined with chatify's E2E encryption, this is about as private as CLI chat gets.

---

## Advanced: TLS Transport

chatify's E2E encryption protects message content regardless of the transport. TLS adds a second layer — it hides connection metadata (who's talking to whom) from anyone watching the wire.

**Generate a self-signed cert (for LAN / private use):**
```bash
openssl req -x509 -newkey rsa:4096 -keyout key.pem -out cert.pem \
  -days 365 -nodes -subj "/CN=chatify"
```

**Start server with TLS:**
```bash
./target/release/clicord-server --tls --cert cert.pem --key key.pem
```

**Clients connect with `--tls`:**
```bash
./target/release/clicord-client --host yourserver.com --tls
```

For a browser-trusted cert, use Let's Encrypt with a real domain name.

---

## File Structure

```
chatify/
├── src/
│   ├── main.rs           Rust server source
│   ├── client.rs         Rust client source
│   ├── crypto.rs         Encryption utilities
│   ├── discord_bot.rs    Discord bridge (currently disabled)
│   └── lib.rs            Shared utilities
├── Cargo.toml            Rust dependency manifest
├── Cargo.lock            Pinned dependency versions
├── LICENSE               GPL v3
├── .gitattributes        Git line ending normalisation
├── .gitignore            Git ignore rules
├── .prettierrc           Code formatter config
└── README.md             This file
```

**Who runs what:**
- **Host:** `./target/release/clicord-server`
- **Everyone else:** `./target/release/clicord-client --host <server-ip>`

---

## Troubleshooting

**"Connection refused"**
The server isn't running, or the host/port doesn't match. For internet connections, make sure port forwarding is set up on the router.

**"bad auth"**
Wrong password. It's case-sensitive and must match exactly what was set when the server started.

**Username got a number appended (e.g. `alice_1`)**
That name is already taken. Pick a different one or wait for the other session to disconnect.

**Messages show `[can't decrypt]`**
Password mismatch somewhere. Everyone must use the exact same password — channel keys are derived from it deterministically.

**Voice is choppy or cuts out**
Usually packet loss on a bad connection. The buffer holds ~2.4s but heavy loss still causes gaps. Make sure your mic is set as the default input device (`arecord -l` on Linux, System Preferences on macOS).

**Image looks squished or stretched**
The 0.45 aspect correction is calibrated for typical monospace fonts. Unusual terminal fonts with very different character proportions will skew the output. Terminal limitation, not a bug.

**Permission denied on the binary**
```bash
chmod +x target/release/clicord-server
chmod +x target/release/clicord-client
```

**Port already in use**
```bash
./target/release/clicord-server --port 8766
./target/release/clicord-client --host <ip> --port 8766
```

**ALSA warnings on Linux during voice chat**
Lines starting with `ALSA lib pcm.c:` are harmless warnings from the audio library itself. Audio still works.

---

## License

GPL v3 — see [LICENSE](LICENSE).

---
## Discord Bot Bridge

**⚠️ Currently Disabled**

The Discord bot bridge is currently disabled due to dependency conflicts with the Serenity library. This feature would allow bridging between a Discord server and your chatify server, enabling cross-platform chat.

### Status
- **Current State**: Disabled in Cargo.toml
- **Issue**: Serenity dependency conflicts with other crates
- **Impact**: Bot binary cannot be built until conflicts are resolved

### How it would work (when re-enabled)
- The bot listens to messages in a Discord channel and forwards them to your chatify server (as encrypted messages).
- It also listens for messages from your chatify server (via WebSocket) and posts them to the same Discord channel.
- Note: The bot sees plaintext messages when bridging, so you must trust the environment where the bot runs.

### Setup (for when re-enabled)
1. **Create a Discord bot**:
   - Go to the [Discord Developer Portal](https://discord.com/developers/applications)
   - Create a new application, then go to the "Bot" tab and click "Add Bot"
   - Under "Privileged Gateway Intents", enable "Message Content Intent" (required to read message content)
   - Copy the bot token (you'll need it for the environment variable below)
2. **Invite the bot to your server**:
   - In the Discord Developer Portal, go to "OAuth2" → "URL Generator"
   - Select scopes: `bot` and `applications.commands`
   - Select bot permissions: `Send Messages`, `Read Message History`, `View Channel`
   - Copy the generated URL and open it in a browser to invite the bot to your Discord server
3. **Set environment variables** (create a `.env` file or export them):
   - `DISCORD_TOKEN`: The bot token from step 1
   - `CHATIFY_HOST`: The host where your chatify server is running (default: `127.0.0.1`)
   - `CHATIFY_PORT`: The port where your chatify server is running (default: `8765`)
   - `CHATIFY_PASSWORD`: The password set when starting the chatify server
   - `CHATIFY_CHANNEL`: The chatify channel to bridge to (default: `general`)
   - `CHATIFY_LOG` (optional): Set to `1` to enable logging

### Running the bot (when re-enabled)
After setting the environment variables, build and run the bot:
```bash
cargo build --release
./target/release/discord_bot
```

### Example (when re-enabled)
```bash
export DISCORD_TOKEN="your_bot_token_here"
export CHATIFY_HOST="127.0.0.1"
export CHATIFY_PORT="8765"
export CHATIFY_PASSWORD="your_chatify_password"
export CHATIFY_CHANNEL="general"
export CHATIFY_LOG=1
./target/release/discord_bot
```

---
## Logging and Testing

### Client and Server
- Enable runtime logging: `./target/release/clicord-client --log` or `./target/release/clicord-server --log`
- Quick log test: `./target/release/clicord-client --log-test` (prints sample logs and exits)
- Environment variables: set `CHATIFY_LOG=1` or `LOGGING=1`

Both logging systems provide timestamped output with levels (INFO, DEBUG) to help diagnose issues and verify functionality.
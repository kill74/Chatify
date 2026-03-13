# chatify

```
        __          __  _ ____
  _____/ /_  ____ _/ /_(_) __/_  __
 / ___/ __ \/ __ `/ __/ / /_/ / / /
/ /__/ / / / /_/ / /_/ / __/ /_/ /
\___/_/ /_/\__,_/\__/_/_/  \__, /
                          /____/
```

A Discord-like chat app that runs entirely in your terminal. Self-hosted on your own machine — no accounts, no cloud, no IP leaks, no telemetry. End-to-end encrypted. Rust server, Python client.

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

**Server** (the person hosting):
- Linux x86-64 — pre-built binary included
- OR: any OS with Rust installed to compile from source

**Clients** (everyone connecting):
- Python 3.9+
- `websockets`, `cryptography`, `Pillow`
- Voice chat (optional): `pyaudio` + system audio libraries

---

## Installation

### 1. Install Python dependencies

```bash
pip install -r requirements.txt
```

### 2. (Optional) Install voice chat support

Voice is entirely optional — all other features work without it.

**Ubuntu / Debian:**
```bash
sudo apt install portaudio19-dev
pip install pyaudio
```

**macOS:**
```bash
brew install portaudio
pip install pyaudio
```

**Windows:**
```bash
pip install pyaudio
```

---

## Running the Server

One person in your group runs this. Everyone else connects to their IP.

```bash
chmod +x chatify-server   # first time only
./chatify-server
```

You'll be prompted to set a password. Share it with your group over a secure channel (Signal, in person, etc.). It's never written to disk — memory only, gone when the server stops.

**Options:**

```bash
./chatify-server --host 0.0.0.0 --port 8765    # default: all interfaces
./chatify-server --host 127.0.0.1              # localhost only (for Tor)
./chatify-server --port 9000                   # custom port
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
python client.py
```

Enter your username and the server password. If the username is taken, the server appends a number (`alice_1`, etc.).

**Remote server:**
```bash
python client.py --host 192.168.1.50              # LAN
python client.py --host 12.34.56.78               # internet
python client.py --host 12.34.56.78 --port 9000   # custom port
python client.py --host example.com --tls         # with TLS
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
| `/voice` | Toggle voice chat for your current channel. Run again to leave. Requires pyaudio. |
| `/clear` | Clear the terminal. |
| `/help` | Show command reference. |
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
|---|---|
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
The server binary makes zero outbound connections after startup. No analytics, no pinging an update server, no phoning home. Verify with `strace -e network ./chatify-server` or `ss -tp` if you want to be sure.

**No accounts.**
No email, no phone number, no registration. Username + shared password — that's the entire auth model.

**Fully auditable.**
All source code is included. The server is 383 lines of Rust, the client is 408 lines of Python, and the crypto helpers are 70 lines. Read every line yourself.

---

## Voice Chat

```bash
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

```bash
/image /absolute/path/to/photo.jpg
/image ~/Desktop/meme.png
/image "./folder with spaces/image.gif"
```

**Supported formats:** JPEG, PNG, GIF, BMP, WEBP, and anything else Pillow can open.

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

| Metric | chatify (Rust) | Python server |
|---|---|---|
| Binary size | 1.5 MB stripped | ~30 MB (Python + deps) |
| RAM at idle | ~2 MB | ~30 MB |
| Startup time | <10 ms | ~150 ms |
| Message relay latency | <1 ms (LAN) | 1–5 ms (LAN) |
| GC pauses | None | Occasional |

**Why it's fast:**

Each channel has a `tokio::sync::broadcast::Sender<String>`. When a message arrives, sending it to all subscribers is a single atomic operation — no loops, no per-message locking. `DashMap` provides lock-free concurrent reads for user and channel lookups. Tokio schedules thousands of WebSocket connections as lightweight async tasks sharing a thread pool sized to your CPU. No OS thread per connection, no GC, no interpreter.

---

## Advanced: Tor Hidden Service

Run as a Tor hidden service so neither side learns the other's real IP. The server won't know who's connecting, and clients won't know where the server is.

**1. Bind to localhost only:**
```bash
./chatify-server --host 127.0.0.1 --port 8765
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
torsocks python client.py --host ab3cde4fgh567ijk.onion --port 8765
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
./chatify-server --tls --cert cert.pem --key key.pem
```

**Clients connect with `--tls`:**
```bash
python client.py --host yourserver.com --tls
```

For a browser-trusted cert, use Let's Encrypt with a real domain name.

---

## Building from Source

The included binary targets Linux x86-64. For macOS, Windows, ARM, or if you'd rather not run pre-built binaries:

**Install Rust:**
```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source ~/.cargo/env
```

**Build:**
```bash
cargo build --release
# binary ends up at: target/release/chatify-server
```

`Cargo.lock` is included so you get the exact same dependency versions.

**Dependencies:**

| Crate | Purpose |
|---|---|
| `tokio` | Async runtime, TCP listener, sync primitives |
| `tokio-tungstenite` | Async WebSocket server |
| `futures-util` | Async stream and sink utilities |
| `serde` + `serde_json` | JSON serialisation |
| `dashmap` | Lock-free concurrent hash maps |
| `sha2` | SHA-256 for password hashing |
| `hex` | Hex encoding of hash output |
| `clap` | CLI argument parsing |

---

## File Structure

```
chatify/
├── chatify-server        Pre-built Linux x86-64 server binary (1.5 MB, stripped)
├── src/
│   └── main.rs           Rust server source (383 lines)
├── Cargo.toml            Rust dependency manifest
├── Cargo.lock            Pinned dependency versions
├── client.py             Python client (408 lines)
├── utils.py              Crypto + ASCII art helpers (70 lines)
├── requirements.txt      Python dependencies
├── LICENSE               GPL v3
├── .gitattributes        Git line ending normalisation
└── .prettierrc           Code formatter config
```

**Who runs what:**
- **Host:** `./chatify-server`
- **Everyone else:** `python client.py --host <server-ip>`
- `utils.py` must be in the same directory as `client.py`

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

**`voice needs pyaudio`**
Install pyaudio. See [Installation](#installation). Everything else works without it.

**Voice is choppy or cuts out**
Usually packet loss on a bad connection. The buffer holds ~2.4s but heavy loss still causes gaps. Make sure your mic is set as the default input device (`arecord -l` on Linux, System Preferences on macOS).

**Image looks squished or stretched**
The 0.45 aspect correction is calibrated for typical monospace fonts. Unusual terminal fonts with very different character proportions will skew the output. Terminal limitation, not a bug.

**`ModuleNotFoundError`**
Run `pip install -r requirements.txt` from inside the chatify directory.

**`Permission denied` on the binary**
```bash
chmod +x chatify-server
```

**Port already in use**
```bash
./chatify-server --port 8766
python client.py --host <ip> --port 8766
```

**ALSA warnings on Linux during voice chat**
Lines starting with `ALSA lib pcm.c:` are harmless warnings from the audio library itself. Audio still works.

---

## License

GPL v3 — see [LICENSE](LICENSE).
---
## Logging and Testing

- Python client now supports runtime logging via the --log flag. Example: `python client.py --log`.
- Quick log sanity check: `python client.py --log-test` to print sample log lines to stdout.
- You can also enable logging via environment variables: set CHATIFY_LOG=1 or LOGGING=1.
- Rust server logging is not wired in this repo yet; we can enable similar on the Rust side if you want.

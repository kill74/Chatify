# clicord

```
  ██████╗██╗     ██╗      ██████╗ ██████╗ ██████╗ ██████╗
 ██╔════╝██║     ██║     ██╔════╝██╔═══██╗██╔══██╗██╔══██╗
 ██║     ██║     ██║     ██║     ██║   ██║██████╔╝██║  ██║
 ██║     ██║     ██║     ██║     ██║   ██║██╔══██╗██║  ██║
 ╚██████╗███████╗██║     ╚██████╗╚██████╔╝██║  ██║██████╔╝
  ╚═════╝╚══════╝╚═╝      ╚═════╝ ╚═════╝ ╚═╝  ╚═╝╚═════╝
```

A Discord-like chat app that runs entirely in your terminal. Self-hosted on your own PC, end-to-end encrypted, no accounts, no cloud, no IP leaks, no telemetry. Just a binary you run and a password you share with your crew.

---

## Table of Contents

- [Features](#features)
- [Requirements](#requirements)
- [Installation](#installation)
- [Running the Server](#running-the-server)
- [Connecting as a Client](#connecting-as-a-client)
- [All Commands](#all-commands)
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

---

## Features

- **Multi-channel text chat** — create any channel on the fly with `/join`
- **End-to-end encrypted messages** — the server only ever relays ciphertext, never plaintext
- **Encrypted direct messages** — DMs use a separate key the server cannot derive
- **Image sharing** — send any image file and it auto-converts to ASCII art before sending
- **Real-time voice chat** — per-channel voice rooms with VAD (silence detection) to save bandwidth
- **Zero IP leaks** — server never tells clients each other's IP addresses
- **No disk writes** — all chat history lives in RAM as ciphertext, nothing ever written to disk
- **No accounts, no sign-up** — just a shared password and a username you pick at login
- **Self-hosted** — you run the server on your own machine, you control everything
- **Emotes** — `/me` action messages
- **Colour-coded usernames** — each user gets a deterministic colour so you can tell people apart
- **Persistent channel history** — last 50 messages per channel are cached (still encrypted) so late joiners see context

---

## Requirements

**Server** (the person hosting):
- Linux x86-64 — pre-built binary included, just run it
- OR: any OS with Rust installed (to compile from source, see [Building from Source](#building-from-source))

**Clients** (everyone connecting):
- Python 3.9 or newer
- pip packages: `websockets`, `cryptography`, `Pillow`
- Voice chat (optional): `pyaudio` + system audio libraries

---

## Installation

### 1. Install Python client dependencies

```bash
pip install -r requirements.txt
```

This installs `websockets`, `cryptography`, and `Pillow`. Nothing else. No heavy GUI frameworks, no bloat.

### 2. (Optional) Install voice chat support

Voice chat is entirely optional. Everything else works without it.

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
# if that fails, grab the wheel from: https://www.lfd.uci.edu/~gohlke/pythonlibs/#pyaudio
```

---

## Running the Server

One person in your group runs the server. Everyone else connects to their IP. You need to do this exactly once.

```bash
# make the binary executable (first time only)
chmod +x clicord-server

# start the server
./clicord-server
```

You'll be prompted to set a password. This password is:
- Never stored anywhere — it lives in memory only
- Used to verify connecting clients
- Used to derive the channel encryption keys

Share this password with your group over a secure channel (Signal, in person, etc.).

**Server options:**

```bash
./clicord-server --host 0.0.0.0 --port 8765    # listen on all interfaces (default)
./clicord-server --host 127.0.0.1              # local machine only (good for Tor setup)
./clicord-server --port 9000                   # use a different port
```

**Example output:**
```
server password:
clicord running on ws://0.0.0.0:8765
encryption: chacha20-poly1305 + x25519 | ip privacy: on
ctrl+c to stop

[19:04:01] + alice
[19:04:03] + bob
[19:12:47] - bob
```

The server logs only joins and leaves with timestamps. It logs nothing else — no messages, no IPs, no commands.

**Port forwarding (for internet connections):**
If people are connecting over the internet rather than a local network, you need to forward TCP port 8765 (or whatever port you chose) in your router's settings to point at your machine's local IP. Look up "port forwarding" for your specific router model. Once done, give your friends your public IP (`curl ifconfig.me` to find it).

---

## Connecting as a Client

```bash
python client.py
```

You'll be asked for a username and the server password.

- If someone already has your username, the server appends a number (`alice_1`, `alice_2`, etc.)
- Your username is the only identifying info the server stores about you
- Keypairs are generated fresh every time you start the client

**Connecting to a remote server:**
```bash
python client.py --host 192.168.1.50            # LAN
python client.py --host 12.34.56.78             # internet (use server's public IP)
python client.py --host 12.34.56.78 --port 9000  # custom port
```

**What the screen looks like after connecting:**
```
clicord  logged in as alice  │  #general  │  3 online
type /help for commands
──────────────────────────────────────────────────────
── history ────────────────────────────────────────────
19:01  bob  hey, is this working?
19:02  carol  yeah it works
────────────────────────────────────────────────────────
```

Then you get a prompt:
```
[alice@#general]› 
```

Type to chat, use `/` to run commands.

---

## All Commands

| Command | What it does |
|---|---|
| `/join <channel>` | Switch to a channel. Creates it if it doesn't exist. Channel names are lowercase letters, numbers, hyphens, underscores. No spaces. |
| `/dm <username> <message>` | Send an encrypted direct message. Uses a DH-derived key the server cannot read. |
| `/image <path>` | Send an image file. Converts to ASCII art automatically. Supports JPEG, PNG, GIF, BMP, WEBP. |
| `/me <action>` | Emote. Shows as `* alice waves at everyone`. |
| `/users` | List everyone currently online. |
| `/channels` | List all channels on the server. |
| `/voice` | Toggle voice chat for your current channel. Run again to leave. Requires pyaudio. |
| `/clear` | Clear the terminal. |
| `/help` | Show command reference. |
| `/quit` | Disconnect and exit. Also works: `/exit`, `/q`. |

**Examples:**
```
/join off-topic
/join random
/dm bob hey, check your DMs
/image ~/Downloads/meme.jpg
/image "./my photo with spaces.png"
/me is going to get coffee brb
/voice
```

---

## How Encryption Works

Everything is encrypted on your machine before it ever touches the network. The server is a relay — it handles routing but cannot read anything.

### Channel Messages

```
password + channel name
        │
        ▼
  PBKDF2-SHA256
  (120,000 iterations)
  salt: "clicord:<channelname>"
        │
        ▼
   32-byte key
        │
        ▼
  ChaCha20-Poly1305
  + random 12-byte nonce
        │
        ▼
  base64(nonce + ciphertext)  →  sent to server  →  relayed to channel members  →  decrypted by clients
```

Key points:
- PBKDF2 with 120,000 iterations makes brute-forcing the password from captured ciphertext computationally expensive
- Each channel gets a unique key even if the password is the same (different salt per channel)
- ChaCha20-Poly1305 is authenticated encryption — if anyone tampers with a message in transit, decryption fails and it's dropped
- A fresh random nonce per message means identical messages produce different ciphertext

### Direct Messages

DMs use **X25519 Elliptic Curve Diffie-Hellman** key exchange:

```
alice generates X25519 keypair     bob generates X25519 keypair
(alice_priv, alice_pub)            (bob_priv, bob_pub)

alice_pub ─────────────────────────────────────────► server broadcasts to all
                                   bob_pub ◄───────── server broadcasts to all

alice: DH(alice_priv, bob_pub)     bob: DH(bob_priv, alice_pub)
       │                                  │
       ▼                                  ▼
    SHA-256                            SHA-256
       │                                  │
       ▼                                  ▼
  same 32-byte key  ←──── (identical by DH math) ────►  same 32-byte key

DM: ChaCha20-Poly1305(key) → server relays ciphertext → bob decrypts
```

The server has both public keys but cannot compute the shared secret — that's the mathematical guarantee of Diffie-Hellman. Only the two endpoints can derive the key.

Keypairs are **regenerated every session**, giving session-level forward secrecy. Compromising a key from one session doesn't expose messages from another session.

### What the Server Can and Cannot See

| | Server sees it? |
|---|---|
| Your username | ✅ yes — it's plaintext metadata |
| What channel you're in | ✅ yes — routing requires it |
| Message content | ❌ no — ciphertext only |
| DM content | ❌ no — different key it can't derive |
| Your IP address | ❌ not forwarded to other clients |
| Voice audio content | ❌ no — encrypted with channel key |
| Images | ❌ no — ASCII art encrypted with channel key |

---

## Privacy Guarantees

**No IP leaks.**
The server never includes remote IP addresses in any payload it sends to clients. Other users have zero way of finding your IP through the chat. They see a username, nothing else.

**No disk writes.**
Chat history is held in a `VecDeque` in RAM as ciphertext. The server has no database, no log files, no persistence layer. When the process stops, everything vanishes.

**No telemetry.**
The server binary makes zero outbound connections after startup. It does not phone home, ping an update server, collect analytics, or contact any external service. You can verify this with `strace` or `ss -tp`.

**No accounts.**
No email address, phone number, or registration of any kind. Username + shared password. That's the entire auth model.

**Open source.**
Every line of code is included. Server is 383 lines of Rust. Client is 408 lines of Python. Crypto helpers are 70 lines. Read it yourself.

---

## Voice Chat

```
/voice   → join voice in current channel
/voice   → run again to leave
```

Voice chat is channel-scoped. Each channel has its own voice room. You can be in `#general` voice while chatting in `#off-topic` text.

**Technical details:**
- Captures microphone at **16kHz, mono** using PyAudio
- Chunk size is **960 samples (~60ms)** per packet
- **VAD (voice activity detection):** audio chunks with RMS energy below 350 are silently dropped. This means you're not sending anything when you're not talking, which saves bandwidth and avoids sending background noise/hum
- Audio is encrypted with the channel key before sending
- Server relays audio to all other voice room members
- Playback uses a **queue with 40-chunk capacity** (~2.4s) to absorb network jitter
- Uses background threads for both capture and playback so it doesn't block the chat UI

**Bandwidth:** ~128-256 kbps when actively speaking, near zero when silent.

---

## Sending Images

```
/image /absolute/path/to/file.jpg
/image ~/relative/path/image.png
/image "./path with spaces/photo.jpg"
```

**Supported formats:** JPEG, PNG, GIF, BMP, WEBP, and anything else Pillow can open (most common formats work).

**Conversion process:**
1. Image is opened and any alpha transparency is composited over black
2. Resized to **70 characters wide** using Lanczos resampling (best quality)
3. Height is calculated to maintain aspect ratio with a **0.45 correction factor** for the typical 2:1 height-to-width ratio of monospace terminal characters
4. Converted to greyscale
5. Each pixel's brightness is mapped to one of 70 characters from a dense-to-sparse ramp: `$@B%8&WM#*oahkbdpq...` (darkest) to ` ` (lightest)
6. The resulting text is encrypted and sent like a normal message

**How it looks in the terminal:**
```
19:04  alice sent an image:
┌─ #general ────────────────────────────────────────────
│ @@@@@@@@@@######################@@@@@@@@@@
│ @@@@@@#*oahkbdpqwmZO0QLCJUYXzcv##@@@@@@@
│ @@@#*oahkbdpqwmZO0QLCJUYXzcvun*#@@@@@@
│ ...
└───────────────────────────────────────────────────────
```

The wider your terminal, the better the detail. Images look best at 100+ columns wide.

---

## Performance

The server is compiled Rust using the Tokio async runtime. Here's how it compares:

| Metric | clicord (Rust) | Original Python server |
|---|---|---|
| Binary / process size | 1.5 MB | ~30 MB (Python + deps) |
| RAM at idle | ~2 MB | ~30 MB |
| Startup time | <10 ms | ~150 ms |
| Message relay latency | <1 ms (local) | 1–5 ms (local) |
| GC pauses | None (no GC) | Occasional |

**How message relay works internally:**

Each channel has a Tokio `broadcast::Sender<String>`. When a message arrives:

1. Parse JSON — one allocation
2. Re-serialise with metadata added — one allocation
3. Call `broadcast_sender.send(msg)` — atomic, single operation, all subscribers get it instantly

There are no per-message locks on the hot path. The `DashMap` used for user/channel lookup uses fine-grained sharding so concurrent reads don't block each other.

The Tokio runtime schedules tasks cooperatively across a thread pool sized to your CPU core count. Thousands of simultaneous WebSocket connections each get their own lightweight task (not a thread), so the server stays efficient even under heavy load.

---

## Advanced: Tor Hidden Service

Run the server as a Tor hidden service. Clients connect via `.onion` address and neither side learns the other's real IP — not even the server operator knows who's connecting.

**Step 1 — Bind the server to localhost only:**
```bash
./clicord-server --host 127.0.0.1 --port 8765
```

**Step 2 — Configure Tor** (add to `/etc/tor/torrc`):
```
HiddenServiceDir /var/lib/tor/clicord/
HiddenServicePort 8765 127.0.0.1:8765
```

**Step 3 — Restart Tor:**
```bash
sudo systemctl restart tor
```

**Step 4 — Get your .onion address:**
```bash
sudo cat /var/lib/tor/clicord/hostname
# something like: ab3cde4fgh567ijk.onion
```

Share this address with your group.

**Step 5 — Clients connect with torsocks:**
```bash
torsocks python client.py --host ab3cde4fgh567ijk.onion --port 8765
```

Now:
- The server sees connecting IPs as Tor exit relays, not real IPs
- Clients don't know the server's real IP
- Add this to clicord's existing E2E encryption and you have a very private setup

---

## Advanced: TLS Transport

By default traffic is E2E encrypted at the application layer (ChaCha20-Poly1305) but the transport layer is plaintext WebSocket. Adding TLS gives you an extra layer — useful if you're worried about someone on the network seeing who's talking to whom (even if they can't read messages).

**Generate a self-signed cert (for private/LAN use):**
```bash
openssl req -x509 -newkey rsa:4096 -keyout key.pem -out cert.pem \
  -days 365 -nodes -subj "/CN=clicord"
```

**Start server with TLS:**
```bash
./clicord-server --tls --cert cert.pem --key key.pem
```

**Clients connect with --tls:**
```bash
python client.py --host yourserver.com --tls
```

For a trusted cert (so clients don't get SSL warnings), use Let's Encrypt with a real domain name.

---

## Building from Source

The pre-built binary targets Linux x86-64. For macOS, Windows, ARM, or if you just don't trust pre-built binaries:

**Install Rust:**
```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source ~/.cargo/env
```

**Build:**
```bash
cd clicord
cargo build --release
```

Binary ends up at `target/release/clicord-server`. `Cargo.lock` is included so you get the exact same dependency versions.

**Dependencies used:**

| Crate | Version | Purpose |
|---|---|---|
| `tokio` | 1.x | Async runtime, TCP listener, channels |
| `tokio-tungstenite` | 0.21 | Async WebSocket server |
| `tungstenite` | 0.21 | WebSocket protocol types |
| `futures-util` | 0.3 | Async stream/sink utilities |
| `serde` + `serde_json` | 1.x | JSON serialisation |
| `dashmap` | 5.x | Lock-free concurrent hash maps |
| `sha2` | 0.10 | SHA-256 for password hashing |
| `hex` | 0.4 | Hex encoding of hash output |
| `clap` | 4.4 | CLI argument parsing |

---

## File Structure

```
clicord/
│
├── clicord-server          Pre-built Linux x86-64 server binary (1.5 MB, stripped)
│
├── server-src/
│   └── main.rs             Full Rust server source (383 lines)
│
├── Cargo.toml              Rust dependency manifest
├── Cargo.lock              Pinned dependency versions for reproducible builds
│
├── client.py               Python client TUI (408 lines)
├── utils.py                Crypto + ASCII art helpers (70 lines)
│
└── requirements.txt        Python deps: websockets, cryptography, Pillow
```

**Who runs what:**
- The **host** runs `./clicord-server` (Linux binary, or build with `cargo build --release`)
- **Everyone else** runs `python client.py --host <server-ip>`
- `utils.py` must be in the same directory as `client.py`

---

## Troubleshooting

**"Connection refused"**
The server isn't running, or host/port doesn't match. Double-check `--host` and `--port` on both sides. For internet connections, verify port forwarding is set up on the server's router.

**"bad auth"**
Wrong password. It's case-sensitive and must match exactly what was set when the server started.

**Username got a number appended (e.g. `alice_1`)**
Someone else is already connected with that username. Pick a different name or wait for them to disconnect.

**Messages show `[can't decrypt]`**
Your password doesn't match what was used when those messages were sent. Everyone must use the same server password. The key is derived deterministically from password + channel name — if the password is wrong, decryption fails.

**`voice needs pyaudio`**
You need to install pyaudio. See [Installation](#installation). Everything else works without it.

**Voice: no audio / very choppy**
- Check your mic is the default input device (`arecord -l` on Linux, System Preferences on macOS)
- On Linux, try `pulseaudio --start` if the audio subsystem isn't running
- Choppy audio over bad networks is normal — the buffer holds ~2.4s of audio but high packet loss will still cause gaps
- Make sure you're using headphones, otherwise mic picks up speaker output

**Image looks squished or stretched**
The converter uses a 0.45 aspect correction for typical monospace fonts. If your terminal uses an unusual font with a very different character height, the image will look distorted. This is a terminal limitation, not a bug.

**`ModuleNotFoundError: No module named 'cryptography'` (or similar)**
Run `pip install -r requirements.txt` from inside the clicord directory.

**`Permission denied` running the server binary**
```bash
chmod +x clicord-server
```

**Port 8765 already in use**
Something else is using that port, or a previous server didn't exit cleanly. Use a different port:
```bash
./clicord-server --port 8766
python client.py --host <ip> --port 8766
```

**ALSA errors on Linux (voice chat)**
Lines like `ALSA lib pcm.c: ...` are warnings from the ALSA library, not errors from clicord. They're harmless. Audio still works.

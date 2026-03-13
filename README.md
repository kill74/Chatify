# clicord

encrypted self-hosted cli chat. rust server + python client.

## quick start

```bash
# build server (only needed if not using the pre-built binary)
cargo build --release
./target/release/clicord-server

# or just use the included binary:
./clicord-server

# clients connect with:
pip install -r requirements.txt
python client.py --host <ip> --port 8765
```

## server options

```bash
./clicord-server --host 0.0.0.0 --port 8765   # default
./clicord-server --host 127.0.0.1              # local only
```

## client commands

```
/join <channel>       switch channel
/dm <user> <msg>      encrypted dm (x25519 key exchange)
/image <path>         send image → ascii art
/me <action>          emote
/users                who's online
/channels             list channels
/voice                toggle voice chat (pip install pyaudio)
/clear                clear screen
/quit                 exit
```

## encryption

- **channels** — PBKDF2(password + channel) → ChaCha20-Poly1305. server only sees ciphertext.
- **dms** — X25519 key exchange + ChaCha20-Poly1305. server can't read dms.
- **no ip leaks** — server never forwards client ips to anyone.
- **no disk writes** — all history is in-ram ciphertext only.

## performance

- server: ~1.5MB binary, ~2MB RAM idle, handles thousands of concurrent connections via tokio async
- startup: <10ms
- message latency: sub-millisecond relay (no GC pauses, no interpreter overhead)

## files

```
clicord-server       pre-built linux x86-64 binary
server-src/main.rs   rust source (cargo build --release to rebuild)
Cargo.toml           rust dependencies
client.py            python client (everyone runs this)
utils.py             crypto + ascii art helpers
requirements.txt     pip install -r requirements.txt
```

## tor (extra privacy)

```
# /etc/tor/torrc
HiddenServiceDir /var/lib/tor/clicord/
HiddenServicePort 8765 127.0.0.1:8765

# clients
torsocks python client.py --host yourhash.onion --port 8765
```

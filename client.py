#!/usr/bin/env python3
# clicord client — python3 client.py [--host IP] [--port 8765]
import argparse, asyncio, base64, getpass, json, os, sys, time
from datetime import datetime
from pathlib import Path
import websockets, websockets.exceptions
from utils import channel_key, dh_key, enc, dec, pw_hash, new_keypair, pub_b64, img_to_ascii, CHUNK, RATE, rms

# ── ANSI ─────────────────────────────────────────────────────────────────────

R = "\033[0m"
BOLD = "\033[1m"
DIM = "\033[2m"
RED = "\033[91m"
GRN = "\033[92m"
YLW = "\033[93m"
BLU = "\033[94m"
MAG = "\033[95m"
CYN = "\033[96m"
WHT = "\033[97m"

# colour per username (deterministic)
_UCOLORS = [CYN, GRN, MAG, YLW, BLU, RED, WHT, "\033[36m", "\033[32m", "\033[35m"]
def ucol(name): return _UCOLORS[sum(ord(c) for c in name) % len(_UCOLORS)]

def ts(): return datetime.now().strftime("%H:%M")
def p(text): print(text, flush=True)

# Lightweight runtime logger (enabled via CLI flag or environment)
LOGGING_ENABLED = False

def log_line(level, message):
    if LOGGING_ENABLED:
        ts = datetime.now().strftime("%Y-%m-%d %H:%M:%S")
        print(f"[{ts}] {level}: {message}", flush=True)

def pline(text=""):
    w = os.get_terminal_size().columns if sys.stdout.isatty() else 60
    p(f"{DIM}{'─'*w}{R}" if not text else f"{DIM}── {text} {'─'*(w-len(text)-4)}{R}")

# ── themes ───────────────────────────────────────────────────────────────────

THEMES = {
    "default": {"prompt": BLU, "user": CYN, "system": YLW},
    "matrix": {"prompt": GRN, "user": GRN, "system": DIM + GRN},
    "ocean": {"prompt": CYN, "user": BLU, "system": MAG},
    "fire": {"prompt": RED, "user": YLW, "system": RED}
}

# ── client ───────────────────────────────────────────────────────────────────

class Client:
    def __init__(self):
        self.ws = None
        self.me = ""
        self.pw = ""
        self.ch = "general"
        self.chs = {"general"}
        self.users = {}           # name → pubkey_b64
        self.chan_keys = {}       # ch → bytes
        self.dm_keys = {}         # name → bytes
        self.priv = new_keypair()
        self.running = True
        self.voice = None
        self.in_voice = False
        self.theme = "default"
        self.file_transfers = {}  # track ongoing file transfers
        self.message_history = [] # store recent messages for search
        self.status = {"text": "Online", "emoji": "🟢"}
        self.status = {"text": "Away", "emoji": "🟡"}
        self.status = {"text": "Offline", "emoji": "🔴"}
        self.reactions = {}       # message_id → {emoji: count}

    def ckey(self, ch):
        if ch not in self.chan_keys:
            self.chan_keys[ch] = channel_key(self.pw, ch)
        return self.chan_keys[ch]

    def dmkey(self, name):
        if name not in self.dm_keys:
            pk = self.users.get(name)
            if not pk: raise ValueError(f"unknown user: {name}")
            self.dm_keys[name] = dh_key(self.priv, pk)
        return self.dm_keys[name]

    # ── display ──

    def show(self, text, t=None):
        p(f"{DIM}{t or ts()}{R}  {text}")
        # Store in message history for search
        self.message_history.append({
            "time": t or ts(),
            "text": text,
            "timestamp": time.time()
        })
        # Keep only last 100 messages
        if len(self.message_history) > 100:
            self.message_history.pop(0)

    def show_history(self, msgs):
        if not msgs: return
        pline("history")
        for m in msgs:
            t = datetime.fromtimestamp(m.get("ts", 0)).strftime("%H:%M")
            u = m.get("u", "?")
            if m.get("t") == "msg":
                try:
                    content = dec(self.ckey(m.get("ch", self.ch)), m["c"])
                    self.show(f"{ucol(u)}{u}{R}  {content}", t)
                except:
                    self.show(f"{ucol(u)}{u}{R}  {DIM}[encrypted]{R}", t)
        pline()

    def prompt_str(self):
        theme_prompt = THEMES[self.theme]["prompt"]
        v = " 🎙" if self.in_voice else ""
        return f"{DIM}[{R}{theme_prompt}{self.me}{R}{DIM}@#{self.ch}{R}{DIM}]{R}{v}{theme_prompt} ›{R} "

    # ── connect ──

    async def connect(self, uri, username, password):
        self.me, self.pw = username, password
        log_line("INFO", f"connecting to {uri} as {username}")
        p(f"{DIM}connecting to {BOLD}{uri}{R}{DIM}…{R}")
        self.ws = await websockets.connect(uri, ping_interval=20, ping_timeout=10)

        await self.ws.send(json.dumps({
            "t": "auth", "u": username,
            "pw": pw_hash(password),
            "pk": pub_b64(self.priv),
            "status": self.status
        }))

        resp = json.loads(await self.ws.recv())
        if resp.get("t") == "err":
            raise ConnectionRefusedError(resp.get("m", "auth failed"))

        self.me = resp.get("u", username)
        log_line("INFO", f"logged in as {self.me}. online users: {len(resp.get('users', []))}")
        for u in resp.get("users", []):
            self.users[u["u"]] = u.get("pk", "")

        pline()
        p(f"  {GRN}clicord{R}  logged in as {BOLD}{CYN}{self.me}{R}"
          f"  │  #{self.ch}  │  {len(self.users)} online")
        p(f"  {DIM}type {BOLD}/help{R}{DIM} for commands{R}")
        pline()
        self.show_history(resp.get("hist", []))

    # ── incoming ──

    def on_msg(self, d):
        t, now_ts = d.get("t"), datetime.fromtimestamp(d.get("ts", time.time())).strftime("%H:%M")

        if t == "msg":
            log_line("DEBUG", f"recv message t={t} ch={d.get('ch','?')} u={d.get('u','?')}")
            ch, u = d.get("ch","?"), d.get("u","?")
            try:
                content = dec(self.ckey(ch), d["c"])
                tag = f"{DIM}#{ch}{R} " if ch != self.ch else ""
                me_tag = f"{DIM} (you){R}" if u == self.me else ""
                self.show(f"{ucol(u)}{BOLD}{u}{R}{me_tag} {tag}{content}", now_ts)
                
                # Desktop notification for mentions
                if self.me.lower() in content.lower() and u != self.me:
                    self.notify("Mention", f"{u}: {content}")
            except:
                self.show(f"{RED}[can't decrypt from {u}]{R}", now_ts)

        elif t == "img":
            ch, u = d.get("ch","?"), d.get("u","?")
            try:
                art = dec(self.ckey(ch), d["a"])
                self.show(f"{ucol(u)}{BOLD}{u}{R} {DIM}sent an image:{R}", now_ts)
                p(f"{DIM}┌─ #{ch} ─────────────────────────{R}")
                for line in art.split("\n"):
                    p(f"{DIM}│{R} {line}")
                p(f"{DIM}└────────────────────────────────{R}")
            except:
                self.show(f"{RED}[can't decrypt image from {u}]{R}", now_ts)

        elif t == "dm":
            log_line("DEBUG", f"recv dm t={t} from {d.get('from','?')} to {d.get('to','?')}")
            frm, to = d.get("from","?"), d.get("to","?")
            outgoing = frm == self.me
            peer = to if outgoing else frm
            try:
                content = dec(self.dmkey(peer), d["c"])
                arrow = f"{DIM}→{R}" if outgoing else f"{MAG}←{R}"
                self.show(f"{MAG}{BOLD}dm{R} {arrow} {BOLD}{peer}{R}: {content}", now_ts)
                
                # Desktop notification for DMs
                if not outgoing:
                    self.notify("Direct Message", f"From {frm}: {content}")
            except:
                self.show(f"{RED}[can't decrypt dm]{R}", now_ts)

        elif t == "sys":
            log_line("DEBUG", f"sys message: {d.get('m','')}")
            self.show(f"{YLW}{d.get('m','')}{R}", now_ts)

        elif t == "users":
            for u in d.get("users", []):
                self.users[u["u"]] = u.get("pk", "")
            names = "  ".join(
                f"{BOLD}{CYN}{u}{R}" if u == self.me else f"{ucol(u)}{u}{R}"
                for u in sorted(self.users)
            )
            self.show(f"{DIM}online ({len(self.users)}):{R} {names}")

        elif t == "joined":
            log_line("DEBUG", f"joined channel #{d.get('ch','?')}")
            ch = d.get("ch","?")
            self.ch = ch; self.chs.add(ch)
            self.show(f"{CYN}→ #{ch}{R}")
            self.show_history(d.get("hist", []))

        elif t == "info":
            log_line("DEBUG", f"info update: chs={d.get('chs','[]')}")
            chs = "  ".join(f"#{c}" for c in d.get("chs", []))
            self.show(f"{DIM}channels: {chs}  │  online: {d.get('online','?')}{R}")

        elif t == "vdata":
            log_line("DEBUG", "voice data update")
            if self.voice:
                self.voice.receive(d.get("a", ""))

        elif t == "err":
            self.show(f"{RED}error: {d.get('m','')}{R}")

        elif t == "file_meta":
            sender = d.get("from", "?")
            filename = d.get("filename", "unknown")
            size = d.get("size", 0)
            file_id = d.get("file_id", f"{sender}_{time.time()}")
            
            self.file_transfers[file_id] = {
                "filename": filename,
                "size": size,
                "chunks": [],
                "received": 0
            }
            self.show(f"{ucol(sender)}{sender}{R} wants to send file: {filename} ({size} bytes)")

        elif t == "file_chunk":
            file_id = d.get("file_id")
            if file_id in self.file_transfers:
                chunk_data = d.get("data", "")
                self.file_transfers[file_id]["chunks"].append(chunk_data)
                self.file_transfers[file_id]["received"] += 1
                
                # Check if complete
                total_chunks = (self.file_transfers[file_id]["size"] + 8191) // 8192
                if len(self.file_transfers[file_id]["chunks"]) >= total_chunks:
                    self.save_received_file(file_id)

        elif t == "status_update":
            user = d.get("user")
            status = d.get("status", {})
            # Update user status in memory
            if user in self.users:
                self.users[user]["status"] = status
            self.show(f"{ucol(user)}{user}{R} is now {status.get('emoji', '🟢')} {status.get('text', 'Online')}")

        elif t == "reaction":
            msg_id = d.get("msg_id")
            emoji = d.get("emoji")
            user = d.get("user")
            if msg_id not in self.reactions:
                self.reactions[msg_id] = {}
            if emoji not in self.reactions[msg_id]:
                self.reactions[msg_id][emoji] = 0
            self.reactions[msg_id][emoji] += 1
            self.show(f"{ucol(user)}{user}{R} reacted with {emoji}")

    def save_received_file(self, file_id):
        file_info = self.file_transfers[file_id]
        filename = file_info["filename"]
        chunks = file_info["chunks"]
        
        # Decode and reconstruct file
        import base64
        full_data = "".join(chunks)
        file_data = base64.b64decode(full_data)
        
        # Save to downloads folder
        downloads_dir = Path.home() / "Downloads"
        downloads_dir.mkdir(exist_ok=True)
        save_path = downloads_dir / filename
        
        with open(save_path, 'wb') as f:
            f.write(file_data)
            
        self.show(f"{GRN}✓ File saved: {save_path}{R}")
        del self.file_transfers[file_id]

    def notify(self, title, message):
        """Send desktop notification"""
        try:
            if os.name == 'nt':  # Windows
                from win10toast import ToastNotifier
                toaster = ToastNotifier()
                toaster.show_toast(title, message, duration=5)
            elif sys.platform == 'darwin':  # macOS
                os.system(f"osascript -e 'display notification \"{message}\" with title \"{title}\"'")
            else:  # Linux
                os.system(f"notify-send '{title}' '{message}'")
        except:
            pass  # Silently fail if notifications aren't available

    # ── commands ──

    async def cmd(self, line):
        parts = line.split(None, 2)
        c = parts[0].lower()

        if c == "/join":
            ch = (parts[1] if len(parts) > 1 else "general").lstrip("#")
            await self.ws.send(json.dumps({"t": "join", "ch": ch}))

        elif c == "/dm":
            if len(parts) < 3:
                return self.show(f"{RED}usage: /dm <user> <msg>{R}")
            target, msg = parts[1], parts[2]
            try:
                await self.ws.send(json.dumps({"t": "dm", "to": target, "c": enc(self.dmkey(target), msg)}))
            except ValueError as e:
                self.show(f"{RED}{e}{R}")

        elif c == "/image":
            log_line("DEBUG", f"/image command invoked: path={parts[1] if len(parts)>1 else ''}")
            if len(parts) < 2:
                return self.show(f"{RED}usage: /image <path>{R}")
            path = Path(parts[1].strip("'\""))
            if not path.exists():
                return self.show(f"{RED}file not found: {path}{R}")
            try:
                self.show(f"{DIM}converting…{R}")
                art = img_to_ascii(path.read_bytes(), width=70)
                await self.ws.send(json.dumps({
                    "t": "img", "ch": self.ch, "a": enc(self.ckey(self.ch), art)
                }))
                self.show(f"{GRN}✓ sent{R}")
            except Exception as e:
                self.show(f"{RED}image error: {e}{R}")

        elif c == "/me":
            action = " ".join(parts[1:]) if len(parts) > 1 else "…"
            await self.ws.send(json.dumps({
                "t": "msg", "ch": self.ch,
                "c": enc(self.ckey(self.ch), f"* {self.me} {action}")
            }))

        elif c == "/users":
            await self.ws.send(json.dumps({"t": "users"}))

        elif c == "/channels":
            await self.ws.send(json.dumps({"t": "info"}))

        elif c == "/voice":
            await self.toggle_voice()

        elif c == "/clear":
            os.system("cls" if os.name == "nt" else "clear")
            pline()
            p(f"  {CYN}{self.me}{R}  in {BOLD}#{self.ch}{R}")
            pline()

        elif c in ("/quit", "/exit", "/q"):
            self.running = False

        elif c == "/help":
            cmds = [
                ("/join <ch>",         "switch channel"),
                ("/dm <user> <msg>",   "encrypted direct message"),
                ("/image <path>",      "send image as ascii art"),
                ("/me <action>",       "emote"),
                ("/users",             "list online users"),
                ("/channels",          "list channels"),
                ("/voice",             "toggle voice chat"),
                ("/clear",             "clear screen"),
                ("/quit",              "exit"),
                ("/theme <name>",      "change theme"),
                ("/file <path>",       "send file to channel"),
                ("/search <query>",    "search message history"),
                ("/status <text>",     "set your status"),
                ("/react <emoji>",     "react to last message"),
            ]
            pline("help")
            for cmd_s, desc in cmds:
                p(f"  {CYN}{cmd_s:<22}{R} {DIM}{desc}{R}")
            pline()
            p(f"{DIM}themes: {', '.join(THEMES.keys())}{R}")

        elif c == "/theme":
            theme_name = parts[1] if len(parts) > 1 else "default"
            if theme_name in THEMES:
                self.theme = theme_name
                self.show(f"{GRN}Theme changed to {theme_name}{R}")
            else:
                self.show(f"{RED}Unknown theme. Available: {', '.join(THEMES.keys())}{R}")

        elif c == "/file":
            if len(parts) < 2:
                return self.show(f"{RED}usage: /file <path>{R}")
            path = Path(parts[1].strip("'\""))
            if not path.exists():
                return self.show(f"{RED}file not found: {path}{R}")
            
            # Read file and send
            with open(path, 'rb') as f:
                file_data = f.read()
            
            import base64
            encoded = base64.b64encode(file_data).decode()
            filename = path.name
            file_id = f"{self.me}_{int(time.time())}"
            
            # Send metadata
            await self.ws.send(json.dumps({
                "t": "file_meta",
                "ch": self.ch,
                "filename": filename,
                "size": len(file_data),
                "file_id": file_id
            }))
            
            # Send chunks
            chunk_size = 8192
            chunks = [encoded[i:i+chunk_size] for i in range(0, len(encoded), chunk_size)]
            
            for i, chunk in enumerate(chunks):
                await self.ws.send(json.dumps({
                    "t": "file_chunk",
                    "ch": self.ch,
                    "data": chunk,
                    "index": i,
                    "file_id": file_id
                }))
            
            self.show(f"{GRN}✓ File sent: {filename}{R}")

        elif c == "/search":
            query = " ".join(parts[1:]) if len(parts) > 1 else ""
            if not query:
                return self.show(f"{RED}usage: /search <query>{R}")
            
            results = [msg for msg in self.message_history 
                      if query.lower() in msg["text"].lower()]
            
            if results:
                pline(f"search results for '{query}'")
                for result in results[-10:]:  # Show last 10 matches
                    p(f"{DIM}{result['time']}{R}  {result['text']}")
                pline()
            else:
                self.show(f"{DIM}No results found for '{query}'{R}")

        elif c == "/status":
            status_text = " ".join(parts[1:]) if len(parts) > 1 else "Online"
            self.status["text"] = status_text
            await self.ws.send(json.dumps({
                "t": "status",
                "status": self.status
            }))
            self.show(f"{GRN}Status updated: {status_text}{R}")

        elif c == "/react":
            emoji = parts[1] if len(parts) > 1 else "👍"
            # React to last message (would need message ID tracking)
            await self.ws.send(json.dumps({
                "t": "reaction",
                "emoji": emoji,
                "msg_id": "last"  # Simplified - would need actual message IDs
            }))

        else:
            self.show(f"{RED}unknown command. /help for list{R}")

    # ── voice ──

    async def toggle_voice(self):
        try:
            import pyaudio
        except ImportError:
            return self.show(f"{RED}voice needs pyaudio: pip install pyaudio{R}")

        if self.in_voice and self.voice:
            self.voice["active"] = False
            self.in_voice = False
            self.voice = None
            await self.ws.send(json.dumps({"t": "vleave"}))
            self.show(f"{YLW}🎙 left voice{R}")
            return

        import threading, queue as q_mod
        loop = asyncio.get_event_loop()
        pa = pyaudio.PyAudio()
        play_q = q_mod.Queue(maxsize=40)
        state = {"active": True}
        self.voice = state

        out = pa.open(pyaudio.paInt16, 1, RATE, output=True, frames_per_buffer=CHUNK)
        inp = pa.open(pyaudio.paInt16, 1, RATE, input=True, frames_per_buffer=CHUNK)

        async def _send_audio(b64):
            if self.ws:
                await self.ws.send(json.dumps({"t": "vdata", "a": b64}))

        def capture():
            while state["active"]:
                try:
                    data = inp.read(CHUNK, exception_on_overflow=False)
                    if rms(data) < 350: continue
                    asyncio.run_coroutine_threadsafe(
                        _send_audio(base64.b64encode(data).decode()), loop)
                except: break

        def playback():
            while state["active"]:
                try:
                    out.write(play_q.get(timeout=0.2))
                except: continue

        state["receive"] = lambda b64: play_q.put_nowait(base64.b64decode(b64))

        # store receive fn so on_msg can call it
        self.voice = {**state, "receive": state["receive"],
                      "pa": pa, "inp": inp, "out": out}

        def voice_receive(b64):
            try: play_q.put_nowait(base64.b64decode(b64))
            except: pass

        # monkey-patch receive on the object so on_msg can reach it
        class VoiceHandle:
            def receive(self_, b64): voice_receive(b64)
        vobj = VoiceHandle()
        self.voice = vobj
        self.in_voice = True

        threading.Thread(target=capture, daemon=True).start()
        threading.Thread(target=playback, daemon=True).start()

        await self.ws.send(json.dumps({"t": "vjoin", "r": self.ch}))
        self.show(f"{GRN}🎙 joined voice #{self.ch}  (/voice to leave){R}")

    # ── listen ──

    async def listen(self):
        try:
            async for raw in self.ws:
                try: self.on_msg(json.loads(raw))
                except json.JSONDecodeError: continue
        except websockets.exceptions.ConnectionClosed:
            self.show(f"{RED}disconnected from server{R}")
            self.running = False

    # ── input loop ──

    async def input_loop(self):
        loop = asyncio.get_event_loop()

        def get_input():
            try:
                sys.stdout.write(self.prompt_str())
                sys.stdout.flush()
                return sys.stdin.readline()
            except (EOFError, KeyboardInterrupt):
                return None

        while self.running:
            line = await loop.run_in_executor(None, get_input)
            if line is None:
                self.running = False
                break
            line = line.strip()
            if not line: continue

            if line.startswith("/"):
                await self.cmd(line)
            else:
                try:
                    await self.ws.send(json.dumps({
                        "t": "msg", "ch": self.ch,
                        "c": enc(self.ckey(self.ch), line)
                    }))
                except Exception as e:
                    self.show(f"{RED}send error: {e}{R}")

    # ── run ──

    async def run(self, uri, username, password):
        await self.connect(uri, username, password)
        try:
            await asyncio.gather(self.listen(), self.input_loop())
        finally:
            if self.ws:
                await self.ws.close()
            p(f"\n{DIM}bye{R}\n")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--host", default="127.0.0.1")
    ap.add_argument("--port", type=int, default=8765)
    ap.add_argument("--tls", action="store_true")
    ap.add_argument("--log", action="store_true", help="enable runtime logging to stdout")
    ap.add_argument("--log-test", action="store_true", help="print sample log output for testing and exit")
    args = ap.parse_args()

    # Enable logging via flag or environment variable
    global LOGGING_ENABLED
    LOGGING_ENABLED = bool(args.log or os.environ.get("CHATIFY_LOG") == "1" or os.environ.get("LOGGING") == "1")
    if args.log_test:
        log_line("INFO", "Logging test started")
        log_line("DEBUG", "test-value=42")
        log_line("INFO", "Logging test finished")
        return

    scheme = "wss" if args.tls else "ws"
    uri = f"{scheme}://{args.host}:{args.port}"

    print(f"\n{BOLD}clicord{R}")
    username = input("username: ").strip()
    if not username: sys.exit("username can't be empty")
    password = getpass.getpass("password: ")

    c = Client()
    try:
        asyncio.run(c.run(uri, username, password))
    except ConnectionRefusedError as e:
        sys.exit(f"connection failed: {e}")
    except KeyboardInterrupt:
        pass


if __name__ == "__main__":
    main()

#!/usr/bin/env python3
# clicord client — python client.py [--host IP] [--port 8765]
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

def pline(text=""):
    w = os.get_terminal_size().columns if sys.stdout.isatty() else 60
    p(f"{DIM}{'─'*w}{R}" if not text else f"{DIM}── {text} {'─'*(w-len(text)-4)}{R}")

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
        v = " 🎙" if self.in_voice else ""
        return f"{DIM}[{R}{CYN}{self.me}{R}{DIM}@#{self.ch}{R}{DIM}]{R}{v}{BLU} ›{R} "

    # ── connect ──

    async def connect(self, uri, username, password):
        self.me, self.pw = username, password
        p(f"{DIM}connecting to {BOLD}{uri}{R}{DIM}…{R}")
        self.ws = await websockets.connect(uri, ping_interval=20, ping_timeout=10)

        await self.ws.send(json.dumps({
            "t": "auth", "u": username,
            "pw": pw_hash(password),
            "pk": pub_b64(self.priv),
        }))

        resp = json.loads(await self.ws.recv())
        if resp.get("t") == "err":
            raise ConnectionRefusedError(resp.get("m", "auth failed"))

        self.me = resp.get("u", username)
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
            ch, u = d.get("ch","?"), d.get("u","?")
            try:
                content = dec(self.ckey(ch), d["c"])
                tag = f"{DIM}#{ch}{R} " if ch != self.ch else ""
                me_tag = f"{DIM} (you){R}" if u == self.me else ""
                self.show(f"{ucol(u)}{BOLD}{u}{R}{me_tag} {tag}{content}", now_ts)
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
            frm, to = d.get("from","?"), d.get("to","?")
            outgoing = frm == self.me
            peer = to if outgoing else frm
            try:
                content = dec(self.dmkey(peer), d["c"])
                arrow = f"{DIM}→{R}" if outgoing else f"{MAG}←{R}"
                self.show(f"{MAG}{BOLD}dm{R} {arrow} {BOLD}{peer}{R}: {content}", now_ts)
            except:
                self.show(f"{RED}[can't decrypt dm]{R}", now_ts)

        elif t == "sys":
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
            ch = d.get("ch","?")
            self.ch = ch; self.chs.add(ch)
            self.show(f"{CYN}→ #{ch}{R}")
            self.show_history(d.get("hist", []))

        elif t == "info":
            chs = "  ".join(f"#{c}" for c in d.get("chs", []))
            self.show(f"{DIM}channels: {chs}  │  online: {d.get('online','?')}{R}")

        elif t == "vdata":
            if self.voice:
                self.voice.receive(d.get("a", ""))

        elif t == "err":
            self.show(f"{RED}error: {d.get('m','')}{R}")

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
            ]
            pline("help")
            for cmd_s, desc in cmds:
                p(f"  {CYN}{cmd_s:<22}{R} {DIM}{desc}{R}")
            pline()

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
    args = ap.parse_args()

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

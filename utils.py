import os, base64, hashlib, io, struct
from cryptography.hazmat.primitives.kdf.pbkdf2 import PBKDF2HMAC
from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric.x25519 import X25519PrivateKey, X25519PublicKey
from cryptography.hazmat.primitives.ciphers.aead import ChaCha20Poly1305
from PIL import Image


# ── crypto ──────────────────────────────────────────────────────────────────

def channel_key(password, channel):
    kdf = PBKDF2HMAC(hashes.SHA256(), 32,
                     f"clicord:{channel}".encode(), 120_000)
    return kdf.derive(password.encode())

def dh_key(priv, other_pub_b64):
    raw = base64.b64decode(other_pub_b64)
    shared = priv.exchange(X25519PublicKey.from_public_bytes(raw))
    return hashlib.sha256(shared).digest()

def enc(key, text):
    nonce = os.urandom(12)
    ct = ChaCha20Poly1305(key).encrypt(nonce, text.encode(), None)
    return base64.b64encode(nonce + ct).decode()

def dec(key, token):
    raw = base64.b64decode(token)
    return ChaCha20Poly1305(key).decrypt(raw[:12], raw[12:], None).decode()

def new_keypair():
    return X25519PrivateKey.generate()

def pub_b64(priv):
    return base64.b64encode(
        priv.public_key().public_bytes(serialization.Encoding.Raw,
                                        serialization.PublicFormat.Raw)
    ).decode()

def pw_hash(pw):
    return hashlib.sha256(pw.encode()).hexdigest()


# ── image → ascii ────────────────────────────────────────────────────────────

_CHARS = r"$@B%8&WM#*oahkbdpqwmZO0QLCJUYXzcvunxrjft/\|()1{}[]?-_+~<>i!lI;:,\"^`'. "

def img_to_ascii(data: bytes, width=72) -> str:
    img = Image.open(io.BytesIO(data))
    if img.mode in ("RGBA", "P"):
        bg = Image.new("RGB", img.size, (0, 0, 0))
        bg.paste(img.convert("RGBA"), mask=img.convert("RGBA").split()[-1])
        img = bg
    elif img.mode != "RGB":
        img = img.convert("RGB")
    h = max(8, int(width * (img.height / img.width) * 0.45))
    img = img.resize((width, h), Image.LANCZOS).convert("L")
    n = len(_CHARS) - 1
    px = list(img.getdata())
    rows = ["".join(_CHARS[int(p/255*n)] for p in px[i:i+width])
            for i in range(0, len(px), width)]
    return "\n".join(rows)


# ── voice helpers ────────────────────────────────────────────────────────────

CHUNK, RATE = 960, 16_000

def rms(data):
    s = struct.unpack(f"{len(data)//2}h", data)
    return (sum(x*x for x in s) / len(s)) ** .5 if s else 0

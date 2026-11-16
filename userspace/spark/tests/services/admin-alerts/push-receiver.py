# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

"""The push service of the boot test admin-alerts: what alertd(8) sends
the alerts of a browser to, in place of its maker's (ADR 0566).

    push-receiver.py serve --port PORT --cert CERT --key KEY --dir DIR
                     [--bind ADDRESS] [--host NAME]
    push-receiver.py selftest

serve takes HTTPS on PORT (on 127.0.0.1 unless --bind names an address),
with the certificate and key in the PEM files CERT and KEY, and says
"push receiver ready" once it listens. NAME (10.0.2.2 without) is how the
machine under test reaches it, which the subscription and the token's
audience name.

Before it listens it makes the keys of a browser that does not exist: a
P-256 key and an authentication secret, and writes the subscription such a
browser would make, as PushSubscription.toJSON() gives it, to
DIR/subscription.json. The private key stays in DIR/browser.key (in
hexadecimal), so that the messages it is sent can be read.

Each POST to a path under /push/ is a push: the receiver keeps what came
as DIR/N.json, N from 1, and answers 201, or the status that DIR/status
holds when that file exists (the test writes 410 into it to say the
browser has ended its subscription). The file has the path, the headers
that matter (content-encoding, content-type, ttl, urgency and topic), the
status it answered, the length of the body, and

    plain       the message decrypted as RFC 8291 says, as JSON, or
                plain_error with why it could not be
    vapid       {"ok", "key", "aud", "exp", "why"}: whether the token of
                the Authorization header is RFC 8292's, names this
                service's origin, ends within a day, and is signed by
                the key that header gives (key, which is the public key
                the machine's `alerts push key` prints)

A GET answers 200 and a line of words, for a test that wants to know the
service is there.

Only the standard library is used, and the cryptography is written here
in plain Python, which a few hundred bytes a minute do not make slow:
P-256 (ECDH and ECDSA), HKDF-SHA-256 and AES-128-GCM. selftest checks it
against RFC 8291's Appendix A and FIPS 197, and runs a server beside a
client. It needs openssl for the certificate it makes, and exits 0 when
all pass.
"""

import base64
import hashlib
import hmac
import http.client
import http.server
import json
import os
import secrets
import ssl
import subprocess
import sys
import tempfile
import threading
import time

# P-256: the field, the order of the group, the curve's b (a is -3) and the
# generator.
P = 0xFFFFFFFF00000001000000000000000000000000FFFFFFFFFFFFFFFFFFFFFFFF
N = 0xFFFFFFFF00000000FFFFFFFFFFFFFFFFBCE6FAADA7179E84F3B9CAC2FC632551
B = 0x5AC635D8AA3A93E7B3EBBD55769886BC651D06B0CC53B0F63BCE3C3E27D2604B
G = (
    0x6B17D1F2E12C4247F8BCE6E563A440F277037D812DEB33A0F4A13945D898C296,
    0x4FE342E2FE1A7F9B8EE7EB4A7C0F9E162BCE33576B315ECECBB6406837BF51F5,
)

# What a push may weigh, in bytes: more is not read.
BODY_MAX = 64 * 1024

# How long a connection may be silent, in seconds.
IDLE_SECONDS = 30

# How long a token may be good for, in seconds (RFC 8292: a day).
TOKEN_DAY = 24 * 60 * 60


def b64e(data):
    """`data` as base64url without padding."""
    return base64.urlsafe_b64encode(data).rstrip(b"=").decode("ascii")


def b64d(text):
    """The bytes of base64url `text`, with or without padding."""
    return base64.urlsafe_b64decode(text + "=" * (-len(text) % 4))


# ---- P-256 ----


def point_add(a, b):
    """The sum of two points of the curve, None being the point at infinity."""
    if a is None:
        return b
    if b is None:
        return a
    (x1, y1), (x2, y2) = a, b
    if x1 == x2:
        if (y1 + y2) % P == 0:
            return None
        slope = (3 * x1 * x1 - 3) * pow(2 * y1, -1, P) % P
    else:
        slope = (y2 - y1) * pow(x2 - x1, -1, P) % P
    x3 = (slope * slope - x1 - x2) % P
    return (x3, (slope * (x1 - x3) - y1) % P)


def point_mul(k, point):
    """`k` times `point`."""
    result = None
    addend = point
    while k:
        if k & 1:
            result = point_add(result, addend)
        addend = point_add(addend, addend)
        k >>= 1
    return result


def on_curve(point):
    """Whether `point` is on the curve."""
    x, y = point
    return 0 <= x < P and 0 <= y < P and (y * y - (x * x * x - 3 * x + B)) % P == 0


def point_bytes(point):
    """An uncompressed point: 4, x and y."""
    return b"\x04" + point[0].to_bytes(32, "big") + point[1].to_bytes(32, "big")


def point_from(data):
    """The point that the 65 bytes `data` are, which must be on the curve."""
    if len(data) != 65 or data[0] != 4:
        raise ValueError("not an uncompressed P-256 point")
    point = (int.from_bytes(data[1:33], "big"), int.from_bytes(data[33:], "big"))
    if not on_curve(point):
        raise ValueError("the point is not on the curve")
    return point


def make_key():
    """A private key, as a number, and its public point."""
    private = 1 + secrets.randbelow(N - 1)
    return private, point_mul(private, G)


def ecdsa_sign(private, message):
    """The 64 bytes r and s of ECDSA with SHA-256, for the tests alone."""
    e = int.from_bytes(hashlib.sha256(message).digest(), "big")
    while True:
        k = 1 + secrets.randbelow(N - 1)
        r = point_mul(k, G)[0] % N
        s = pow(k, -1, N) * (e + r * private) % N
        if r and s:
            return r.to_bytes(32, "big") + s.to_bytes(32, "big")


def ecdsa_verify(public, message, signature):
    """Whether `signature`, r and s as 64 bytes, is of `message` by `public`."""
    if len(signature) != 64:
        return False
    r = int.from_bytes(signature[:32], "big")
    s = int.from_bytes(signature[32:], "big")
    if not (0 < r < N and 0 < s < N):
        return False
    e = int.from_bytes(hashlib.sha256(message).digest(), "big")
    w = pow(s, -1, N)
    point = point_add(point_mul(e * w % N, G), point_mul(r * w % N, public))
    return point is not None and point[0] % N == r


# ---- HKDF-SHA-256 ----


def hkdf_extract(salt, ikm):
    return hmac.new(salt, ikm, hashlib.sha256).digest()


def hkdf_expand(prk, info, length):
    out, block, counter = b"", b"", 1
    while len(out) < length:
        block = hmac.new(prk, block + info + bytes([counter]), hashlib.sha256).digest()
        out += block
        counter += 1
    return out[:length]


# ---- AES-128 and GCM ----


def make_sbox():
    """AES's substitution box, made as FIPS 197 says it is."""
    box = [0] * 256
    p = q = 1
    while True:
        p = (p ^ (p << 1) ^ (0x11B if p & 0x80 else 0)) & 0xFF
        q ^= (q << 1) & 0xFF
        q ^= (q << 2) & 0xFF
        q ^= (q << 4) & 0xFF
        if q & 0x80:
            q ^= 0x09

        def rotl(x, n):
            return ((x << n) | (x >> (8 - n))) & 0xFF

        box[p] = q ^ rotl(q, 1) ^ rotl(q, 2) ^ rotl(q, 3) ^ rotl(q, 4) ^ 0x63
        if p == 1:
            break
    box[0] = 0x63
    return box


SBOX = make_sbox()


def xtime(a):
    return ((a << 1) ^ 0x11B) & 0xFF if a & 0x80 else a << 1


def expand_key(key):
    """The 11 round keys, 16 bytes each, of the 16-byte `key`."""
    words = [list(key[4 * i : 4 * i + 4]) for i in range(4)]
    rcon = 1
    for i in range(4, 44):
        t = list(words[i - 1])
        if i % 4 == 0:
            t = [SBOX[b] for b in t[1:] + t[:1]]
            t[0] ^= rcon
            rcon = xtime(rcon)
        words.append([a ^ b for a, b in zip(words[i - 4], t)])
    return [sum(words[4 * r : 4 * r + 4], []) for r in range(11)]


def aes_block(round_keys, block):
    """The 16 bytes `block` encrypted with AES-128."""
    state = [b ^ k for b, k in zip(block, round_keys[0])]
    for r in range(1, 11):
        state = [SBOX[b] for b in state]
        shifted = [0] * 16
        for c in range(4):
            for row in range(4):
                shifted[row + 4 * c] = state[row + 4 * ((c + row) % 4)]
        state = shifted
        if r != 10:
            mixed = []
            for c in range(4):
                a = state[4 * c : 4 * c + 4]
                mixed += [
                    xtime(a[0]) ^ xtime(a[1]) ^ a[1] ^ a[2] ^ a[3],
                    a[0] ^ xtime(a[1]) ^ xtime(a[2]) ^ a[2] ^ a[3],
                    a[0] ^ a[1] ^ xtime(a[2]) ^ xtime(a[3]) ^ a[3],
                    xtime(a[0]) ^ a[0] ^ a[1] ^ a[2] ^ xtime(a[3]),
                ]
            state = mixed
        state = [b ^ k for b, k in zip(state, round_keys[r])]
    return bytes(state)


def gf_mult(x, y):
    """The product of two 128-bit numbers in GCM's field."""
    z, v = 0, y
    for i in range(127, -1, -1):
        if (x >> i) & 1:
            z ^= v
        v = (v >> 1) ^ (0xE1000000000000000000000000000000 if v & 1 else 0)
    return z


def ghash(h, aad, cipher):
    """GHASH of `aad` and `cipher` with the hash key `h`, a number."""
    x = 0
    for part in (aad, cipher):
        for at in range(0, len(part), 16):
            block = part[at : at + 16].ljust(16, b"\0")
            x = gf_mult(x ^ int.from_bytes(block, "big"), h)
    lengths = (len(aad) * 8).to_bytes(8, "big") + (len(cipher) * 8).to_bytes(8, "big")
    return gf_mult(x ^ int.from_bytes(lengths, "big"), h)


def gcm_parts(key, nonce, cipher):
    """The counter-mode transform of `cipher`, which also makes the tag."""
    round_keys = expand_key(key)
    h = int.from_bytes(aes_block(round_keys, bytes(16)), "big")
    first = nonce + (1).to_bytes(4, "big")
    mask = aes_block(round_keys, first)
    out = bytearray()
    for index, at in enumerate(range(0, len(cipher), 16)):
        counter = nonce + (index + 2).to_bytes(4, "big")
        stream = aes_block(round_keys, counter)
        out += bytes(a ^ b for a, b in zip(cipher[at : at + 16], stream))
    return bytes(out), h, mask


def gcm_seal(key, nonce, plain):
    """`plain` encrypted with AES-128-GCM and no additional data, with its tag."""
    cipher, h, mask = gcm_parts(key, nonce, plain)
    tag = (ghash(h, b"", cipher) ^ int.from_bytes(mask, "big")).to_bytes(16, "big")
    return cipher + tag


def gcm_open(key, nonce, sealed):
    """What `sealed`, a cipher text and its 16-byte tag, holds, if the tag is right."""
    if len(sealed) < 16:
        raise ValueError("too short for a tag")
    cipher, tag = sealed[:-16], sealed[-16:]
    plain, h, mask = gcm_parts(key, nonce, cipher)
    wanted = (ghash(h, b"", cipher) ^ int.from_bytes(mask, "big")).to_bytes(16, "big")
    if not hmac.compare_digest(wanted, tag):
        raise ValueError("the tag is wrong")
    return plain


# ---- RFC 8291 ----


def content_keys(shared, auth, ua_public, as_public, salt):
    """The key and nonce of a message, from the secret both ends share."""
    prk_key = hkdf_extract(auth, shared)
    ikm = hkdf_expand(prk_key, b"WebPush: info\0" + ua_public + as_public, 32)
    prk = hkdf_extract(salt, ikm)
    return (
        hkdf_expand(prk, b"Content-Encoding: aes128gcm\0", 16),
        hkdf_expand(prk, b"Content-Encoding: nonce\0", 12),
    )


def decrypt(body, ua_private, ua_public, auth):
    """The text of a push `body` for the browser with this private key, public
    key and authentication secret.

    A single record, as alertd sends: salt, record size, the length of the
    key's ID and the sender's public key as the ID, then the record.
    """
    if len(body) < 21:
        raise ValueError("the body is shorter than a header")
    salt = body[:16]
    size = int.from_bytes(body[16:20], "big")
    id_length = body[20]
    as_public = body[21 : 21 + id_length]
    record = body[21 + id_length :]
    if size < 18 or len(record) > size:
        raise ValueError("the body is more than one record")
    shared = point_mul(ua_private, point_from(as_public))[0].to_bytes(32, "big")
    key, nonce = content_keys(shared, auth, ua_public, as_public, salt)
    plain = gcm_open(key, nonce, record)
    end = len(plain)
    while end and plain[end - 1] == 0:
        end -= 1
    if not end or plain[end - 1] != 2:
        raise ValueError("the last record has no delimiter of 2")
    return plain[: end - 1]


def encrypt(plain, ua_public, auth, salt, as_private):
    """The body of a push of `plain`, as alertd makes one: for the tests."""
    as_public = point_bytes(point_mul(as_private, G))
    shared = point_mul(as_private, point_from(ua_public))[0].to_bytes(32, "big")
    key, nonce = content_keys(shared, auth, ua_public, as_public, salt)
    sealed = gcm_seal(key, nonce, plain + b"\x02")
    return salt + (4096).to_bytes(4, "big") + bytes([len(as_public)]) + as_public + sealed


# ---- RFC 8292 ----


def check_token(header, audience, now):
    """What the `Authorization` header `header` is, as a dict: ok, key, aud,
    exp, and why when it is not ok."""
    seen = {"ok": False, "key": None, "aud": None, "exp": None, "why": None}
    try:
        scheme, _, rest = (header or "").partition(" ")
        if scheme.lower() != "vapid":
            seen["why"] = "the scheme is not vapid"
            return seen
        words = dict(word.strip().split("=", 1) for word in rest.split(","))
        token, key = words["t"], words["k"]
        seen["key"] = key
        head, claims, signature = token.split(".")
        head_json = json.loads(b64d(head))
        claims_json = json.loads(b64d(claims))
        seen["aud"], seen["exp"] = claims_json.get("aud"), claims_json.get("exp")
        if head_json.get("alg") != "ES256":
            seen["why"] = "the algorithm is not ES256"
        elif not ecdsa_verify(point_from(b64d(key)), f"{head}.{claims}".encode(), b64d(signature)):
            seen["why"] = "the signature is not the key's"
        elif seen["aud"] != audience:
            seen["why"] = f"the audience is {seen['aud']}, not {audience}"
        elif not isinstance(seen["exp"], int) or not now < seen["exp"] <= now + TOKEN_DAY:
            seen["why"] = "the token does not end within a day"
        else:
            seen["ok"] = True
    except (KeyError, ValueError, TypeError, AttributeError) as why:
        seen["why"] = f"the header does not read: {why}"
    return seen


# ---- the service ----


class State:
    """What the server keeps: its keys, where it writes and how many it took."""

    def __init__(self, directory, host, port):
        self.directory = directory
        self.host = host
        self.port = port
        self.private, public = make_key()
        self.public = point_bytes(public)
        self.auth = os.urandom(16)
        self.count = 0
        self.lock = threading.Lock()

    def audience(self):
        return f"https://{self.host}:{self.port}"

    def subscription(self):
        """What a browser that made these keys would give."""
        return {
            "endpoint": f"{self.audience()}/push/lab-browser",
            "expirationTime": None,
            "keys": {"p256dh": b64e(self.public), "auth": b64e(self.auth)},
        }

    def status(self):
        """The status to answer with: 201, or what DIR/status holds."""
        try:
            with open(os.path.join(self.directory, "status"), encoding="ascii") as file:
                return int(file.read().strip())
        except (OSError, ValueError):
            return 201

    def keep(self, record):
        """Writes `record` as the next file, whole or not at all."""
        with self.lock:
            self.count += 1
            number = self.count
            record["n"] = number
            name = os.path.join(self.directory, f"{number}.json")
            with open(name + ".new", "w", encoding="utf-8") as file:
                json.dump(record, file, indent=1)
            os.replace(name + ".new", name)
        return number


KEPT_HEADERS = ("content-encoding", "content-type", "ttl", "urgency", "topic")


class Receiver(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.0"

    def setup(self):
        self.request.settimeout(IDLE_SECONDS)
        super().setup()

    def log_message(self, *_):
        pass

    def reply(self, status, text=b""):
        self.send_response(status)
        self.send_header("Content-Type", "text/plain")
        self.send_header("Content-Length", str(len(text)))
        self.end_headers()
        self.wfile.write(text)

    def do_GET(self):
        self.reply(200, b"push receiver\n")

    def do_POST(self):
        state = self.server.state
        try:
            size = int(self.headers.get("Content-Length", "0"))
        except ValueError:
            size = -1
        if size < 0 or size > BODY_MAX:
            self.reply(413)
            return
        body = self.rfile.read(size)
        record = {
            "path": self.path,
            "headers": {name: self.headers.get(name) for name in KEPT_HEADERS},
            "bytes": len(body),
            "at": int(time.time()),
        }
        if not self.path.startswith("/push/"):
            record["plain_error"] = "not a path under /push/"
        else:
            try:
                plain = decrypt(body, state.private, state.public, state.auth)
                record["plain"] = json.loads(plain)
            except (ValueError, KeyError, TypeError) as why:
                record["plain_error"] = str(why)
        record["vapid"] = check_token(
            self.headers.get("Authorization"), state.audience(), int(time.time())
        )
        status = state.status()
        record["answered"] = status
        state.keep(record)
        self.reply(status, b"" if status < 300 else b"gone\n" if status == 410 else b"no\n")


def make_server(directory, address, port, cert, key, host):
    """A server on `address` and `port` (0 for any), whose state is `state`."""
    os.makedirs(directory, exist_ok=True)
    server = http.server.ThreadingHTTPServer((address, port), Receiver)
    server.daemon_threads = True
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    context.load_cert_chain(cert, key)
    # The handshake is done by the thread of the connection, so that a client
    # that does not finish one holds up no other.
    server.socket = context.wrap_socket(
        server.socket, server_side=True, do_handshake_on_connect=False
    )
    server.state = State(directory, host, server.server_address[1])
    return server


def write_keys(server):
    """The browser's subscription and its private key, in the directory."""
    state = server.state
    for name, text in (
        ("subscription.json", json.dumps(state.subscription())),
        ("browser.key", f"{state.private:064x}"),
    ):
        path = os.path.join(state.directory, name)
        with open(path + ".new", "w", encoding="ascii") as file:
            file.write(text + "\n")
        os.replace(path + ".new", path)


def serve(args):
    options = {"--bind": "127.0.0.1", "--host": "10.0.2.2"}
    wanted = ("--port", "--cert", "--key", "--dir")
    while args:
        if len(args) < 2 or not args[0].startswith("--"):
            usage()
        options[args[0]], args = args[1], args[2:]
    if any(name not in options for name in wanted) or not set(options) <= {*wanted, "--bind", "--host"}:
        usage()
    server = make_server(
        options["--dir"], options["--bind"], int(options["--port"]),
        options["--cert"], options["--key"], options["--host"],
    )
    write_keys(server)
    print("push receiver ready", flush=True)
    server.serve_forever()


def usage():
    print(
        "usage: push-receiver.py serve --port PORT --cert CERT --key KEY --dir DIR "
        "[--bind ADDRESS] [--host NAME] | selftest"
    )
    sys.exit(2)


# ---- the self-test ----


def check(condition, what):
    if not condition:
        print(f"FAILED: {what}")
        sys.exit(1)
    print(f"ok: {what}")


def selftest():
    # The curve and the generator.
    check(on_curve(G) and point_mul(N, G) is None, "the generator is on the curve, of order N")
    # FIPS 197, Appendix C.1.
    block = aes_block(expand_key(bytes(range(16))), bytes.fromhex("00112233445566778899aabbccddeeff"))
    check(block.hex() == "69c4e0d86a7b0430d8cdb78070b4c55a", "AES-128 gives FIPS 197's block")
    # RFC 8291, Appendix A.
    plain = b64d("V2hlbiBJIGdyb3cgdXAsIEkgd2FudCB0byBiZSBhIHdhdGVybWVsb24")
    ua_private = int.from_bytes(b64d("q1dXpw3UpT5VOmu_cf_v6ih07Aems3njxI-JWgLcM94"), "big")
    ua_public = b64d(
        "BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4"
    )
    auth = b64d("BTBZMqHH6r4Tts7J_aSIgg")
    salt = b64d("DGv6ra1nlYgDCS1FRnbzlw")
    as_private = int.from_bytes(b64d("yfWPiYE-n46HLnH0KqZOF1fJJU3MYrct3AELtAQ-oRw"), "big")
    message = (
        "DGv6ra1nlYgDCS1FRnbzlwAAEABBBP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocIn"
        "mYWAmS6TlzAC8wEqKK6PBru3jl7A_yl95bQpu6cVPTpK4Mqgkf1CXztLVBSt2Ks3oZwbuwXPXLWyouBWLVWGN"
        "WQexSgSxsj_Qulcy4a-fN"
    )
    check(point_bytes(point_mul(ua_private, G)) == ua_public, "the example's key makes its public key")
    check(b64e(encrypt(plain, ua_public, auth, salt, as_private)) == message, "RFC 8291's message is made")
    check(decrypt(b64d(message), ua_private, ua_public, auth) == plain, "RFC 8291's message is read")
    broken = bytearray(b64d(message))
    broken[-1] ^= 1
    try:
        decrypt(bytes(broken), ua_private, ua_public, auth)
        check(False, "a message with a changed byte is refused")
    except ValueError:
        check(True, "a message with a changed byte is refused")
    # ECDSA, both ways, and a token.
    private, public = make_key()
    signature = ecdsa_sign(private, b"words")
    check(ecdsa_verify(public, b"words", signature), "a signature is verified")
    check(not ecdsa_verify(public, b"other words", signature), "a signature of other words is not")
    now = int(time.time())
    audience = "https://10.0.2.2:9"
    head = b64e(b'{"typ":"JWT","alg":"ES256"}')
    claims = b64e(json.dumps({"aud": audience, "exp": now + 3600, "sub": "https://dukovo.org"}).encode())
    token = f"{head}.{claims}.{b64e(ecdsa_sign(private, f'{head}.{claims}'.encode()))}"
    header = f"vapid t={token}, k={b64e(point_bytes(public))}"
    check(check_token(header, audience, now)["ok"], "a token is good")
    check(not check_token(header, "https://10.0.2.3:9", now)["ok"], "a token for another origin is not")
    check(not check_token(header, audience, now + 7200)["ok"], "a token past its end is not")
    check(not check_token("Bearer x", audience, now)["ok"], "another scheme is not a token")
    served()
    print("all passed")


def served():
    """A server beside a client: a push is kept, read and signed for, and the
    status it answers is the one the directory says."""
    if subprocess.run(["openssl", "version"], capture_output=True).returncode != 0:
        print("the host has no openssl")
        sys.exit(77)
    with tempfile.TemporaryDirectory(prefix="push-receiver-") as folder:
        cert, key = os.path.join(folder, "server.pem"), os.path.join(folder, "server.key")
        made = subprocess.run(
            ["openssl", "req", "-x509", "-newkey", "ec", "-pkeyopt", "ec_paramgen_curve:P-256",
             "-nodes", "-days", "2", "-subj", "/CN=127.0.0.1", "-addext",
             "subjectAltName=IP:127.0.0.1", "-keyout", key, "-out", cert],
            capture_output=True, text=True,
        )
        check(made.returncode == 0, "openssl makes a certificate")
        if made.returncode != 0:
            print(made.stderr)
        directory = os.path.join(folder, "kept")
        server = make_server(directory, "127.0.0.1", 0, cert, key, "127.0.0.1")
        write_keys(server)
        threading.Thread(target=server.serve_forever, daemon=True).start()
        state = server.state
        subscription = json.load(open(os.path.join(directory, "subscription.json")))
        check(subscription["endpoint"] == f"https://127.0.0.1:{state.port}/push/lab-browser",
              "the subscription names the service")
        context = ssl.create_default_context(cafile=cert)

        def post(path, body, extra):
            connection = http.client.HTTPSConnection("127.0.0.1", state.port, context=context, timeout=20)
            headers = {"Content-Type": "application/octet-stream", "Content-Encoding": "aes128gcm", **extra}
            connection.request("POST", path, body=body, headers=headers)
            answer = connection.getresponse()
            answer.read()
            connection.close()
            return answer.status

        sender, sender_public = make_key()
        text = json.dumps({"v": 1, "host": "lab", "count": 1, "first": {"title": "Disk full"}}).encode()
        body = encrypt(text, state.public, state.auth, os.urandom(16), 1 + secrets.randbelow(N - 1))
        now = int(time.time())
        head = b64e(b'{"typ":"JWT","alg":"ES256"}')
        claims = b64e(json.dumps({"aud": state.audience(), "exp": now + 3600,
                                  "sub": "https://dukovo.org"}).encode())
        token = f"{head}.{claims}.{b64e(ecdsa_sign(sender, f'{head}.{claims}'.encode()))}"
        extra = {"Authorization": f"vapid t={token}, k={b64e(point_bytes(sender_public))}",
                 "TTL": "86400", "Urgency": "high", "Topic": "abc"}
        check(post("/push/lab-browser", body, extra) == 201, "a push is answered 201")
        first = json.load(open(os.path.join(directory, "1.json")))
        check(first["plain"]["first"]["title"] == "Disk full", "the push is kept as it was said")
        check(first["vapid"]["ok"] and first["vapid"]["key"] == b64e(point_bytes(sender_public)),
              "its token is good, and names the key")
        check(first["headers"]["urgency"] == "high" and first["headers"]["topic"] == "abc",
              "its headers are kept")
        with open(os.path.join(directory, "status"), "w") as file:
            file.write("410\n")
        check(post("/push/lab-browser", body, {}) == 410, "the status that the directory holds is answered")
        second = json.load(open(os.path.join(directory, "2.json")))
        check(second["answered"] == 410 and not second["vapid"]["ok"], "a push without a token is kept as not good")
        check(post("/push/lab-browser", b"not a message", {}) == 410, "a body that is no message is answered too")
        third = json.load(open(os.path.join(directory, "3.json")))
        check("plain_error" in third and "plain" not in third, "and kept with why it cannot be read")
        server.shutdown()


def main():
    if sys.argv[1:2] == ["serve"]:
        serve(sys.argv[2:])
    elif sys.argv[1:] == ["selftest"]:
        selftest()
    else:
        usage()


if __name__ == "__main__":
    main()

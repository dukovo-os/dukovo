# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

"""A WebSocket client for the boot test admin-terminal (ADR 0517).

    python3 -I websocket_client.py
    python3 -I websocket_client.py --self-test

xtask has no TLS or WebSocket library, so the test starts this program and
drives it one JSON command per line on its standard input. It answers each
line with exactly one JSON line on its standard output, flushed, and prints
nothing else there; a command that cannot be done is answered
{"error": "..."}. Only the standard library is used (Python 3.8 or later).

A command is an object with one key:

    {"open": {"port": P, "cafile": PATH, "host": "localhost:9090",
              "path": "/api/1/terminal", "headers": [[name, value], ...]}}
        Connects to 127.0.0.1:P over TLS, trusting the certificate in PATH
        alone and checking that it names localhost, and writes the opening
        request: Host, Connection, Upgrade, Sec-WebSocket-Version and a new
        Sec-WebSocket-Key, then the given headers. A given header replaces
        the default of its name, and a value of null leaves the default
        out. Answers {"status": 101, "headers": {lower-case name: value},
        "accept_ok": whether Sec-WebSocket-Accept is right for the key}. Any
        other status answers {"status", "headers", "body"}, the body read
        by its Content-Length, and the connection is closed.
    {"text": VALUE}, {"text_raw": "string"}
        One masked text frame holding VALUE as JSON, or that string.
    {"type": {"pane": N, "keys": "..."}}
        One masked binary frame: the pane as 4 bytes, big-endian, then the
        keys in UTF-8.
    {"frame": {"hex": "..."}}
        Those bytes, as they are.
    {"ping": "words"}, {"close": CODE}
        A masked ping or close frame.
    {"acks": true}, {"acks": false}
        While on, each output frame read is acknowledged with {"ack": pane,
        "offset": END}, END being just past what has been read of that pane.
        Turning it on acknowledges what has been read already.
    {"wait": {"seconds": S, KEY: VALUE}}
        Reads until what KEY asks for comes, or S seconds pass: "said" (a
        text message with that key, not part of an earlier wait), "output"
        ([pane, substring]: the pane's output, decoded as UTF-8 with
        replacement, holds it), "bytes" ([pane, count]: it holds at least
        that many), "pong" (a pong of those words), "closed" (a close frame
        or the end of the connection) or "quiet" (nothing: it reads for S
        seconds). Answers {"found": what came or null, "timed_out": bool,
        "said": [the text messages not yet given to a wait], "closed": the
        close frame's code, 0 for a connection that ended without one, or
        null, "lengths": {"pane": bytes of its output so far}}.
    {"output": N}
        {"text", "length", "sha256", "first"} of all the output read of pane
        N, "first" being the offset of its first byte.
    {"forget": N}
        Forgets pane N's output.
    {"quit": true}
        Closes the connection and exits.

Between commands the program still reads what the console sends, answers
its pings, and acknowledges output while acks are on. It also types the
answer to a cursor query (ESC [ 6 n) that a pane's program sends, when the
query ends what has come: the line editor of nukesh asks at each prompt and
waits 2 seconds for it, as it would for the page's terminal. A query with
more output after it is an old one, from a replay, and is not answered.

It checks that each output frame of a pane begins where the one before
ended, except after an "attached" message, whose offset starts the pane's
output again; a gap or an overlap is an {"error": "pane N: output at
OFFSET, expected EXPECTED"} on the next wait. A frame the console must not
send (masked, fragmented, with reserved bits, a control frame over 125
bytes) is an error on every wait after it. A second "open" starts afresh.
"""

import base64
import hashlib
import json
import os
import select
import socket
import ssl
import struct
import sys
import time

GUID = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11"

TEXT, BINARY, CLOSE, PING, PONG = 0x1, 0x2, 0x8, 0x9, 0xA

# The guest has no KVM on most hosts, so its TLS handshake takes a while.
HANDSHAKE_SECONDS = 60

# How long a write may wait for room in the connection.
SEND_SECONDS = 30

# What a program sends to learn where the cursor is. nukesh's line editor
# asks at each prompt and waits up to 2 seconds for the answer, which the
# page's terminal gives at once, so this client does.
CURSOR_QUERY = b"\x1b[6n"

# The most bytes a frame of the console's may have: its output frames hold
# 16 KiB and a header, so a longer one is not the console's.
FRAME_MAX = 1 << 20


def accept_key(key):
    """The Sec-WebSocket-Accept that RFC 6455 (4.2.2) gives for `key`."""
    digest = hashlib.sha1((key + GUID).encode("ascii")).digest()
    return base64.b64encode(digest).decode("ascii")


def mask_bytes(payload, key):
    """`payload` with each byte XOR the 4 bytes of `key`, repeated."""
    length = len(payload)
    if length == 0:
        return b""
    repeated = (key * (length // 4 + 1))[:length]
    both = int.from_bytes(payload, "big") ^ int.from_bytes(repeated, "big")
    return both.to_bytes(length, "big")


def frame(opcode, payload=b"", mask=None):
    """A final frame of `opcode` holding `payload`: masked with the 4 bytes
    of `mask`, as a client's are, or not, as a server's are."""
    flag = 0x80 if mask is not None else 0
    length = len(payload)
    if length < 126:
        head = struct.pack(">BB", 0x80 | opcode, flag | length)
    elif length < 1 << 16:
        head = struct.pack(">BBH", 0x80 | opcode, flag | 126, length)
    else:
        head = struct.pack(">BBQ", 0x80 | opcode, flag | 127, length)
    if mask is None:
        return head + payload
    return head + mask + mask_bytes(payload, mask)


class FrameError(Exception):
    """A frame that a server must not send."""


def read_frame(buffer):
    """The first server frame in `buffer`: (opcode, payload, bytes used), or
    None while its bytes have not all come.

    The console sends whole, unmasked frames with no extension, so any other
    is a fault of the console's, which the test is there to find.
    """
    if len(buffer) < 2:
        return None
    first, second = buffer[0], buffer[1]
    if first & 0x70:
        raise FrameError("a frame with reserved bits set")
    if not first & 0x80:
        raise FrameError("a fragment: the console sends whole messages")
    if second & 0x80:
        raise FrameError("a masked frame from the console")
    opcode = first & 0x0F
    if opcode not in (TEXT, BINARY, CLOSE, PING, PONG):
        raise FrameError("a frame of opcode %d" % opcode)
    length, size = second & 0x7F, 2
    if length == 126:
        if len(buffer) < 4:
            return None
        length, size = struct.unpack(">H", bytes(buffer[2:4]))[0], 4
    elif length == 127:
        if len(buffer) < 10:
            return None
        length, size = struct.unpack(">Q", bytes(buffer[2:10]))[0], 10
    if opcode >= CLOSE and length > 125:
        raise FrameError("a control frame of %d bytes" % length)
    if length > FRAME_MAX:
        raise FrameError("a frame of %d bytes" % length)
    if len(buffer) < size + length:
        return None
    return opcode, bytes(buffer[size : size + length]), size + length


class Pane:
    """The output read of one pane, from the offset of its first byte."""

    def __init__(self, first):
        self.first = first
        self.data = bytearray()
        self.decoded = (0, "")
        # The last bytes that came, in case a cursor query is split between
        # two frames.
        self.tail = b""

    @property
    def end(self):
        """The offset just past what has been read."""
        return self.first + len(self.data)

    def text(self):
        """The output as UTF-8, with replacement for what is not, decoded
        again only when more has come."""
        if self.decoded[0] != len(self.data):
            self.decoded = (len(self.data), self.data.decode("utf-8", "replace"))
        return self.decoded[1]


class Client:
    """One connection, and the state that its commands read."""

    def __init__(self):
        self.start()

    def start(self):
        """Forgets every connection and all that came on it."""
        self.sock = None
        # What came and is not yet a whole frame.
        self.buffer = bytearray()
        # Every text message read, as JSON, and how many of them earlier
        # waits have given.
        self.texts = []
        self.given = 0
        self.panes = {}
        # The rows each pane has, as the console last said.
        self.rows = {}
        self.pongs = []
        self.acks = False
        # The code of the close frame, or 0 once the connection ended with
        # none.
        self.closed = None
        self.ended = True
        self.sent_close = False
        # What was wrong in what came, for the next wait, and a fault that
        # ends all reading.
        self.problems = []
        self.broken = None
        self.quitting = False

    # Connecting.

    def connect(self, spec):
        """The TLS connection to the console, which trusts the certificate
        in spec's cafile alone."""
        raw = socket.create_connection(("127.0.0.1", int(spec["port"])), HANDSHAKE_SECONDS)
        context = ssl.create_default_context(cafile=spec["cafile"])
        # Python 3.13 turns on OpenSSL's strict checks of RFC 5280. The test
        # trusts one certificate, the one the console served at first, so
        # what it asks of the handshake is that the server holds that
        # certificate and that it names localhost.
        context.verify_flags &= ~getattr(ssl, "VERIFY_X509_STRICT", 0)
        try:
            return context.wrap_socket(raw, server_hostname="localhost")
        except Exception:
            raw.close()
            raise

    def handshake(self, sock, spec):
        """Writes the opening request on `sock` and reads the answer's head:
        what the "open" command answers."""
        key = base64.b64encode(os.urandom(16)).decode("ascii")
        defaults = [
            ("Host", spec.get("host", "localhost:9090")),
            ("Connection", "Upgrade"),
            ("Upgrade", "websocket"),
            ("Sec-WebSocket-Version", "13"),
            ("Sec-WebSocket-Key", key),
        ]
        given = [(name, value) for name, value in spec.get("headers", [])]
        replaced = set(name.lower() for name, _ in given)
        headers = [(name, value) for name, value in defaults if name.lower() not in replaced]
        headers += [(name, value) for name, value in given if value is not None]
        sent_key = dict((name.lower(), value) for name, value in headers).get("sec-websocket-key")
        path = spec.get("path", "/api/1/terminal")
        text = "GET %s HTTP/1.1\r\n" % path
        text += "".join("%s: %s\r\n" % pair for pair in headers) + "\r\n"
        self.sock = sock
        self.ended = False
        sock.settimeout(HANDSHAKE_SECONDS)
        sock.sendall(text.encode("utf-8"))
        head, rest = self.read_head(sock)
        status_line, *lines = head.split("\r\n")
        status = int(status_line.split(" ", 2)[1])
        answered = {}
        for line in lines:
            name, _, value = line.partition(":")
            name, value = name.strip().lower(), value.strip()
            answered[name] = answered[name] + ", " + value if name in answered else value
        if status != 101:
            body = self.read_body(sock, rest, answered)
            self.end()
            sock.close()
            return {"status": status, "headers": answered, "body": body.decode("utf-8", "replace")}
        sock.settimeout(SEND_SECONDS)
        self.buffer += rest
        self.process()
        wanted = accept_key(sent_key) if sent_key else None
        return {
            "status": status,
            "headers": answered,
            "accept_ok": answered.get("sec-websocket-accept") == wanted,
        }

    @staticmethod
    def read_head(sock):
        """The head of an answer, up to its blank line, and the bytes that
        came after it."""
        data = b""
        while b"\r\n\r\n" not in data:
            more = sock.recv(4096)
            if not more or len(data) > 1 << 16:
                raise OSError("the answer's head did not end: %r" % data[:200])
            data += more
        head, _, rest = data.partition(b"\r\n\r\n")
        return head.decode("iso-8859-1"), rest

    @staticmethod
    def read_body(sock, rest, headers):
        """The body that follows an answer's head: its Content-Length bytes,
        or all that comes if it has none."""
        wanted = int(headers["content-length"]) if "content-length" in headers else None
        while wanted is None or len(rest) < wanted:
            try:
                more = sock.recv(65536)
            except (socket.timeout, ssl.SSLError):
                break
            if not more:
                break
            rest += more
        return rest if wanted is None else rest[:wanted]

    # Reading.

    def live(self):
        """Whether there is a connection to read."""
        return self.sock is not None and not self.ended

    def has_pending(self):
        """Whether TLS holds bytes that are read already, which a wait on
        the socket would not see."""
        pending = getattr(self.sock, "pending", None)
        return bool(self.live() and pending and pending())

    def end(self):
        """Notes that the connection ended, with a close frame or not."""
        self.ended = True
        if self.closed is None:
            self.closed = 0

    def read_some(self, seconds):
        """Waits up to `seconds` for the console to send something, and takes
        in what comes."""
        if not self.live():
            return
        if not self.has_pending() and not select.select([self.sock], [], [], seconds)[0]:
            return
        try:
            data = self.sock.recv(65536)
        except (socket.timeout, ssl.SSLWantReadError):
            return
        except OSError:
            data = b""
        if not data:
            self.end()
            return
        self.buffer += data
        self.process()

    def process(self):
        """Takes each whole frame out of the buffer."""
        while self.broken is None:
            try:
                found = read_frame(self.buffer)
            except FrameError as err:
                self.broken = str(err)
                return
            if found is None:
                return
            opcode, payload, used = found
            del self.buffer[:used]
            self.take(opcode, payload)

    def take(self, opcode, payload):
        """Does what a frame from the console is for."""
        if opcode == TEXT:
            self.take_text(payload)
        elif opcode == BINARY:
            self.take_output(payload)
        elif opcode == PING:
            self.send(frame(PONG, payload, os.urandom(4)))
        elif opcode == PONG:
            self.pongs.append(payload.decode("utf-8", "replace"))
        else:
            if self.closed is None:
                self.closed = struct.unpack(">H", payload[:2])[0] if len(payload) >= 2 else 1005
            if not self.sent_close:
                self.sent_close = True
                self.send(frame(CLOSE, payload[:2], os.urandom(4)))

    def take_text(self, payload):
        """Keeps a text message, and starts a pane's output again where an
        attach says it begins."""
        try:
            message = json.loads(payload.decode("utf-8"))
        except ValueError:
            self.problems.append("a text message that is not JSON: %r" % payload[:200])
            return
        if not isinstance(message, dict):
            self.problems.append("a text message that is not an object: %r" % payload[:200])
            return
        self.texts.append(message)
        if isinstance(message.get("rows"), int) and "attached" in message:
            self.rows[message["attached"]] = message["rows"]
        offset = message.get("offset")
        if "attached" in message and isinstance(offset, int):
            held = self.panes.get(message["attached"])
            if held is None or held.end != offset:
                self.panes[message["attached"]] = Pane(offset)

    def take_output(self, payload):
        """Adds an output frame to its pane, which must follow on from the
        last."""
        if len(payload) < 12:
            self.problems.append("an output frame of %d bytes" % len(payload))
            return
        number, offset = struct.unpack(">IQ", payload[:12])
        held = self.panes.get(number)
        if held is not None and held.end != offset:
            self.problems.append("pane %d: output at %d, expected %d" % (number, offset, held.end))
            held = None
        if held is None:
            held = self.panes[number] = Pane(offset)
        held.data += payload[12:]
        if self.acks:
            self.acknowledge(number, held)
        seen = held.tail + payload[12:]
        held.tail = seen[-3:]
        # The program waits for the answer to a query that is the last of
        # what it has written. One that more output follows is an old one,
        # which a replay brings back.
        if seen.endswith(CURSOR_QUERY):
            self.answer_cursor(number)

    # Writing.

    def send(self, data):
        """Writes `data`; the reason it could not be, if it could not."""
        if self.sock is None:
            return "there is no connection"
        try:
            self.sock.sendall(data)
        except OSError as err:
            return "%s: %s" % (type(err).__name__, err)
        return None

    def acknowledge(self, number, held):
        """Says that the output of pane `number` is taken in to its end."""
        message = json.dumps({"ack": number, "offset": held.end})
        self.send(frame(TEXT, message.encode("utf-8"), os.urandom(4)))

    def answer_cursor(self, number):
        """Answers a cursor query of pane `number`, as the page's screen
        does, in an answer message: the cursor is on the last row, in the
        first column, where a prompt is."""
        reply = "\x1b[%d;1R" % self.rows.get(number, 24)
        message = json.dumps({"answer": number, "text": reply})
        self.send(frame(TEXT, message.encode("utf-8"), os.urandom(4)))

    def sent(self, data):
        """The answer to a command that writes `data`."""
        failed = self.send(data)
        return {"ok": True} if failed is None else {"ok": True, "send_error": failed}

    # Commands.

    def command(self, request):
        """What the command `request`, an object of one key, answers."""
        if not isinstance(request, dict) or len(request) != 1:
            return {"error": "a command is an object with one key: %r" % (request,)}
        (name, argument), = request.items()
        if name not in COMMANDS:
            return {"error": "no command %r" % name}
        try:
            return getattr(self, "do_" + name)(argument)
        except Exception as err:
            return {"error": "%s: %s" % (type(err).__name__, err)}

    def do_open(self, spec):
        # A second open begins afresh: the refusal of a first leaves a close
        # code that the new connection must not inherit.
        self.shutdown()
        self.start()
        return self.handshake(self.connect(spec), spec)

    def do_text(self, value):
        if isinstance(value, dict) and isinstance(value.get("rows"), int) and "resize" in value:
            self.rows[value["resize"]] = value["rows"]
        return self.sent(frame(TEXT, json.dumps(value).encode("utf-8"), os.urandom(4)))

    def do_text_raw(self, text):
        return self.sent(frame(TEXT, text.encode("utf-8"), os.urandom(4)))

    def do_type(self, spec):
        keys = struct.pack(">I", spec["pane"]) + spec["keys"].encode("utf-8")
        return self.sent(frame(BINARY, keys, os.urandom(4)))

    def do_frame(self, spec):
        return self.sent(bytes.fromhex(spec["hex"]))

    def do_ping(self, words):
        return self.sent(frame(PING, words.encode("utf-8"), os.urandom(4)))

    def do_close(self, code):
        self.sent_close = True
        return self.sent(frame(CLOSE, struct.pack(">H", code), os.urandom(4)))

    def do_acks(self, on):
        self.acks = bool(on)
        if self.acks:
            for number, held in self.panes.items():
                self.acknowledge(number, held)
        return {"ok": True}

    def do_output(self, number):
        held = self.panes.get(number)
        if held is None:
            empty = hashlib.sha256(b"").hexdigest()
            return {"text": "", "length": 0, "sha256": empty, "first": None}
        return {
            "text": held.text(),
            "length": len(held.data),
            "sha256": hashlib.sha256(held.data).hexdigest(),
            "first": held.first,
        }

    def do_forget(self, number):
        self.panes.pop(number, None)
        return {"ok": True}

    def do_quit(self, _):
        self.quitting = True
        self.shutdown()
        return {"ok": True}

    def shutdown(self):
        """Closes the connection."""
        if self.sock is not None:
            try:
                self.sock.close()
            except OSError:
                pass
        self.ended = True

    def looking_for(self, kind, wanted, unread):
        """What a wait of `kind` for `wanted` has found, or None."""
        if kind == "said":
            return next((said for said in self.texts[unread:] if wanted in said), None)
        if kind == "output":
            held = self.panes.get(wanted[0])
            return wanted[1] if held is not None and wanted[1] in held.text() else None
        if kind == "bytes":
            held = self.panes.get(wanted[0])
            return len(held.data) if held is not None and len(held.data) >= wanted[1] else None
        if kind == "pong":
            if wanted not in self.pongs:
                return None
            self.pongs.remove(wanted)
            return wanted
        if kind == "closed":
            return self.closed
        return None

    def do_wait(self, spec):
        kinds = [kind for kind in WAITS if kind in spec]
        if len(kinds) != 1:
            return {"error": "a wait asks for one of said, output, bytes, pong, closed and quiet"}
        kind = kinds[0]
        deadline = time.monotonic() + float(spec.get("seconds", 10))
        unread, found, timed_out = self.given, None, False
        while True:
            if self.problems or self.broken is not None:
                break
            found = self.looking_for(kind, spec[kind], unread)
            if found is not None and kind != "quiet":
                break
            left = deadline - time.monotonic()
            if left <= 0:
                timed_out = kind != "quiet"
                break
            if self.ended or self.closed is not None:
                break
            self.read_some(min(left, 0.25))
        said = self.texts[unread:]
        self.given = len(self.texts)
        if self.problems:
            return {"error": self.problems.pop(0)}
        if self.broken is not None:
            return {"error": self.broken}
        lengths = dict((str(number), len(held.data)) for number, held in self.panes.items())
        return {
            "found": found,
            "timed_out": timed_out,
            "said": said,
            "closed": self.closed,
            "lengths": lengths,
        }


WAITS = ("said", "output", "bytes", "pong", "closed", "quiet")

COMMANDS = (
    "open",
    "text",
    "text_raw",
    "type",
    "frame",
    "ping",
    "close",
    "acks",
    "wait",
    "output",
    "forget",
    "quit",
)


def answer_line(client, line):
    """The line that answers the command in `line`."""
    try:
        request = json.loads(line.decode("utf-8"))
    except ValueError as err:
        return json.dumps({"error": "not JSON: %s" % err})
    return json.dumps(client.command(request), separators=(",", ":"))


def serve(client):
    """Answers the commands on standard input, and reads the connection
    while it waits for them, so that the console's pings are answered and
    its output acknowledged between commands."""
    pending = b""
    while True:
        while b"\n" not in pending:
            watched = [0]
            if client.live():
                watched.append(client.sock)
            ready = select.select(watched, [], [], 0 if client.has_pending() else 1.0)[0]
            if 0 in ready:
                chunk = os.read(0, 65536)
                if not chunk:
                    client.shutdown()
                    return
                pending += chunk
            if client.live() and (client.sock in ready or client.has_pending()):
                client.read_some(0)
        line, _, pending = pending.partition(b"\n")
        sys.stdout.write(answer_line(client, line) + "\n")
        sys.stdout.flush()
        if client.quitting:
            return


def expect(got, want, what):
    """The self-test's check that `got` is `want`."""
    if got != want:
        raise AssertionError("%s: got %r, wanted %r" % (what, got, want))


class Far:
    """The far end of a pair of connected sockets, which the self-test
    plays the console on: it keeps what the client wrote."""

    def __init__(self, sock):
        sock.settimeout(2)
        self.sock = sock
        self.data = b""

    def more(self):
        chunk = self.sock.recv(65536)
        if not chunk:
            raise AssertionError("the client closed the connection")
        self.data += chunk

    def head(self):
        """The client's opening request."""
        while b"\r\n\r\n" not in self.data:
            self.more()
        head, _, self.data = self.data.partition(b"\r\n\r\n")
        return head.decode("ascii")

    def raw(self, count):
        """The next `count` bytes."""
        while len(self.data) < count:
            self.more()
        taken, self.data = self.data[:count], self.data[count:]
        return taken

    def frames(self, count):
        """The next `count` frames, as (opcode, payload); each must be
        masked, as a server needs."""
        found = []
        while len(found) < count:
            if len(self.data) >= 2:
                length, size = self.data[1] & 0x7F, 2
                if length == 126 and len(self.data) >= 4:
                    length, size = struct.unpack(">H", self.data[2:4])[0], 4
                elif length == 127 and len(self.data) >= 10:
                    length, size = struct.unpack(">Q", self.data[2:10])[0], 10
                whole = size + 4 + length
                if len(self.data) >= whole and (length < 126 or size > 2):
                    expect(self.data[1] & 0x80, 0x80, "a client's frame is masked")
                    key = self.data[size : size + 4]
                    body = self.data[size + 4 : whole]
                    opened = bytes(byte ^ key[at % 4] for at, byte in enumerate(body))
                    found.append((self.data[0] & 0x0F, opened))
                    self.data = self.data[whole:]
                    continue
            self.more()
        return found


def self_test():
    """Checks the frames against RFC 6455's examples, then the commands
    with the test itself as the console."""
    expect(accept_key("dGhlIHNhbXBsZSBub25jZQ=="), "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=", "4.2.2's key")
    key = bytes([0x37, 0xFA, 0x21, 0x3D])
    expect(frame(TEXT, b"Hello", key).hex(), "818537fa213d7f9f4d5158", "5.7's masked Hello")
    expect(frame(TEXT, b"Hello").hex(), "810548656c6c6f", "5.7's unmasked Hello")
    expect(frame(PING, b"Hello").hex(), "890548656c6c6f", "5.7's ping")
    expect(frame(PONG, b"Hello", key).hex(), "8a8537fa213d7f9f4d5158", "5.7's masked pong")
    expect(frame(BINARY, bytes(256))[:4].hex(), "827e0100", "5.7's 256 bytes")
    expect(frame(BINARY, bytes(65536))[:10].hex(), "827f0000000000010000", "5.7's 64 KiB")
    expect(frame(BINARY, bytes(256), key)[:8].hex(), "82fe010037fa213d", "a masked 256 bytes")
    for length in range(11):
        body = bytes(range(length))
        naive = bytes(byte ^ key[at % 4] for at, byte in enumerate(body))
        expect(mask_bytes(body, key), naive, "a mask of %d bytes" % length)
    expect(mask_bytes(mask_bytes(bytes(range(200)), key), key), bytes(range(200)), "unmasking")

    expect(read_frame(bytes.fromhex("810548656c6c6f")), (TEXT, b"Hello", 7), "reading Hello")
    expect(read_frame(bytes.fromhex("810548656c")), None, "a frame not yet whole")
    expect(read_frame(bytes.fromhex("82")), None, "a head not yet whole")
    expect(read_frame(frame(BINARY, bytes(300)) + b"more")[2], 304, "a 16-bit length")
    expect(read_frame(frame(BINARY, bytes(70000)))[2], 70010, "a 64-bit length")
    for wrong in [
        "818537fa213d7f9f4d5158",  # masked
        "c10548656c6c6f",  # a reserved bit
        "010348656c",  # not the last fragment
        "800248656c",  # a continuation
        "830100",  # an opcode that is none
        "897e007e",  # a ping of 126 bytes
    ]:
        try:
            read_frame(bytes.fromhex(wrong))
        except FrameError:
            continue
        raise AssertionError("the frame %s is read" % wrong)

    the_commands()
    the_refusals()
    print("websocket_client.py: the self-test passed")


HEAD = (
    "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n"
    "Sec-WebSocket-Accept: %s\r\nSec-WebSocket-Protocol: dukovo-terminal-1\r\n\r\n"
)

# RFC 6455's example key, and the accept key it gives.
KEY = "dGhlIHNhbXBsZSBub25jZQ=="
ACCEPTED = "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="


def output(pane, offset, text):
    """The console's output frame."""
    return frame(BINARY, struct.pack(">IQ", pane, offset) + text)


def said(message):
    """The console's text frame holding `message`."""
    return frame(TEXT, json.dumps(message).encode("utf-8"))


def asked(client, **command):
    """What `client` answers to `command`."""
    return client.command(command)


def the_commands():
    """Each command, with a console that the test plays on the far end of
    a pair of connected sockets."""
    near, sock = socket.socketpair()
    far, client = Far(sock), Client()
    attached = {"attached": 7, "terminal": 1, "offset": 100, "rows": 30, "replay": True}
    # The first frames come with the answer's head, and are not lost.
    sock.sendall(
        (HEAD % ACCEPTED).encode("ascii") + said(attached) + output(7, 100, b"hello")
        + output(7, 105, b" world")
    )
    opening = {"headers": [["Origin", "https://localhost:9090"], ["Sec-WebSocket-Key", KEY],
                           ["Host", None], ["Cookie", "a=b"]]}
    answer = client.handshake(near, opening)
    expect(answer["status"], 101, "the status")
    expect(answer["accept_ok"], True, "the accept key")
    expect(answer["headers"]["sec-websocket-protocol"], "dukovo-terminal-1", "lower-case names")
    request = far.head()
    expect(request.startswith("GET /api/1/terminal HTTP/1.1\r\n"), True, "the request line")
    for line in ["Origin: https://localhost:9090", "Sec-WebSocket-Key: " + KEY, "Cookie: a=b",
                 "Connection: Upgrade", "Upgrade: websocket", "Sec-WebSocket-Version: 13"]:
        expect(line in request, True, "the request has " + line)
    expect("Host:" in request, False, "a header of null leaves the default out")

    found = asked(client, wait={"seconds": 2, "said": "attached"})
    expect(found["found"], attached, "the attached message")
    expect(found["lengths"], {"7": 11}, "the bytes that came with the head")
    expect(asked(client, wait={"seconds": 0.1, "said": "attached"})["found"], None, "said twice")
    found = asked(client, wait={"seconds": 2, "output": [7, "hello world"]})
    expect(found["found"], "hello world", "output across two frames")
    expect(asked(client, output=7), {"text": "hello world", "length": 11, "first": 100,
           "sha256": hashlib.sha256(b"hello world").hexdigest()}, "the output")
    expect(asked(client, output=9)["first"], None, "the output of a pane not attached")

    # Turning acks on says what was read; each frame after it is said too.
    expect(asked(client, acks=True), {"ok": True}, "acks on")
    sock.sendall(output(7, 111, b"!"))
    expect(asked(client, wait={"seconds": 2, "bytes": [7, 12]})["found"], 12, "the bytes")
    taken = [json.loads(body) for _, body in far.frames(2)]
    expect(taken, [{"ack": 7, "offset": 111}, {"ack": 7, "offset": 112}], "acknowledgements")
    asked(client, acks=False)

    # A cursor query split between two frames is answered once, with the
    # pane's last row.
    sock.sendall(output(7, 112, b"$ \x1b[6") + output(7, 117, b"n"))
    expect(asked(client, wait={"seconds": 2, "bytes": [7, 18]})["found"], 18, "the query's bytes")
    expect(far.frames(1), [(TEXT, b'{"answer": 7, "text": "\\u001b[30;1R"}')], "the cursor's answer")
    asked(client, text={"resize": 7, "cols": 100, "rows": 40})
    sock.sendall(output(7, 118, b"\x1b[6n"))
    asked(client, wait={"seconds": 1, "bytes": [7, 22]})
    expect(far.frames(2)[1], (TEXT, b'{"answer": 7, "text": "\\u001b[40;1R"}'), "the answer after a resize")
    # A query with more output after it is an old one, as in a replay: it
    # is not answered, which the next frames that the far end reads show.
    sock.sendall(output(7, 122, b"\x1b[6nold prompt"))
    asked(client, wait={"seconds": 1, "bytes": [7, 36]})

    # A gap is said once, and the output goes on from there.
    sock.sendall(output(7, 300, b"gap"))
    expect(asked(client, wait={"seconds": 0.1, "quiet": True}),
           {"error": "pane 7: output at 300, expected 136"}, "a gap")
    expect(asked(client, output=7)["first"], 300, "the output after a gap")

    # The console's ping is answered between commands, and the client's
    # own is read as the console would.
    sock.sendall(frame(PING, b"x"))
    asked(client, wait={"seconds": 0.1, "quiet": True})
    expect(far.frames(1), [(PONG, b"x")], "a ping is answered")
    asked(client, ping="words")
    expect(far.frames(1), [(PING, b"words")], "a ping is sent")
    sock.sendall(frame(PONG, b"words"))
    expect(asked(client, wait={"seconds": 2, "pong": "words"})["found"], "words", "a pong")

    asked(client, type={"pane": 7, "keys": "ls\r"})
    asked(client, text={"resize": 7, "cols": 100, "rows": 30})
    asked(client, text_raw="not json")
    asked(client, frame={"hex": "810a"})
    sent = far.frames(3)
    expect(sent[0], (BINARY, b"\x00\x00\x00\x07ls\r"), "typing")
    expect(json.loads(sent[1][1]), {"resize": 7, "cols": 100, "rows": 30}, "a text message")
    expect(sent[2], (TEXT, b"not json"), "a text message as given")
    expect(far.raw(2), b"\x81\x0a", "a frame as given")

    late = asked(client, wait={"seconds": 0.2, "said": "never"})
    expect((late["found"], late["timed_out"], late["closed"]), (None, True, None), "a timeout")
    expect(asked(client, forget=7), {"ok": True}, "forget")
    expect(asked(client, output=7)["length"], 0, "forgotten output")

    asked(client, close=1000)
    sock.sendall(frame(CLOSE, struct.pack(">H", 1000) + b"done"))
    closed = asked(client, wait={"seconds": 2, "closed": True})
    expect((closed["found"], closed["closed"]), (1000, 1000), "a close frame")
    expect(far.frames(1), [(CLOSE, struct.pack(">H", 1000))], "the close sent")
    expect(asked(client, quit=True), {"ok": True}, "quit")
    expect(client.live(), False, "the connection after quit")


def the_refusals():
    """An answer that is not 101, a connection that ends with no close
    frame, and what the console must not send."""
    near, sock = socket.socketpair()
    client = Client()
    sock.sendall(b'HTTP/1.1 403 Forbidden\r\nContent-Length: 7\r\nX-A: 1\r\n\r\n{"a":1}extra')
    answer = client.handshake(near, {"headers": [["Origin", None]]})
    expect(answer, {"status": 403, "headers": {"content-length": "7", "x-a": "1"},
                    "body": '{"a":1}'}, "a refusal")
    expect(client.live(), False, "the connection after a refusal")

    near, sock = socket.socketpair()
    client = Client()
    sock.sendall((HEAD % "AAAAAAAAAAAAAAAAAAAAAAAAAAA=").encode("ascii"))
    answer = client.handshake(near, {"headers": [["Sec-WebSocket-Key", KEY]]})
    expect(answer["accept_ok"], False, "a wrong accept key")
    sock.close()
    closed = asked(client, wait={"seconds": 2, "closed": True})
    expect((closed["found"], closed["closed"], closed["timed_out"]), (0, 0, False), "a lost one")

    for wrong in [
        frame(TEXT, b"{}", bytes(4)),  # masked
        bytes.fromhex("c1020000"),  # a reserved bit
        frame(TEXT, b"1"),  # JSON that is no object
        frame(TEXT, b"{"),  # not JSON
    ]:
        near, sock = socket.socketpair()
        client = Client()
        sock.sendall((HEAD % ACCEPTED).encode("ascii") + wrong)
        client.handshake(near, {"headers": [["Sec-WebSocket-Key", KEY]]})
        answer = asked(client, wait={"seconds": 0.1, "quiet": True})
        expect("error" in answer, True, "what the console must not send: %r" % wrong)

    expect("error" in client.command({"no": 1}), True, "an unknown command")
    expect("error" in client.command({"text": 1, "close": 1}), True, "two commands in one")
    expect("error" in client.command({"wait": {"seconds": 1}}), True, "a wait for nothing")
    expect("error" in json.loads(answer_line(client, b"{")), True, "a line that is not JSON")


if __name__ == "__main__":
    if sys.argv[1:] == ["--self-test"]:
        self_test()
    else:
        serve(Client())

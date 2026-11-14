# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

"""The staging reports server: what reportd(8) sends problem reports to
in the boot test and the lab tests, in place of reports.dukovo.org
(ADR 0531).

    report-server.py serve --port PORT --cert CERT --key KEY --dir DIR [--bind ADDRESS]
    report-server.py set-status DIR NUMBER STATUS [--fixed-in VERSION]
                     [--duplicate-of NUMBER] [--reason TEXT]
    report-server.py show DIR NUMBER
    report-server.py list DIR
    report-server.py selftest

serve takes HTTPS on PORT (on every address unless --bind names one),
with the certificate and key in the PEM files CERT and KEY, and says
"report server ready" once it listens. It keeps each report as files in
DIR: NUMBER.txt, or NUMBER.age for a report in the security box, and
NUMBER.json with the hash of the report's key, its status, its box, the
time it came and its content type. The protocol is the one reportd(8)
speaks:

    POST   /v1/reports         the report's exact bytes, as text/plain, or
                               as application/age-encryption with
                               Report-Box: security; Report-Key-Hash is
                               the SHA-256 of the 32-byte key made for
                               this report, in 64 lowercase hex digits. 201 with
                               {"number": N, "status": "received"}.
    GET    /v1/reports/N       with Report-Key, the key in hex, whose
                               SHA-256 is the stored hash: {"number",
                               "status", "duplicate_of", "fixed_in",
                               "reason"}.
    DELETE /v1/reports/N       with Report-Key: 204.

A key that does not match is answered 403 and an unknown number 404. A
body over 256 KiB is answered 413. Numbers start at 1 and are not used
again. No address of a client is stored, printed or logged.

set-status is what the people who handle reports do: the status is one of
received, confirmed, duplicate, fixed and wont-fix, and what GET answers
changes with it. show prints the stored report exactly as it came, and
list one line for each.

selftest starts a server on a free port with a certificate of its own,
made with openssl in a folder that goes with it, and asks it for each
case above; it exits 0 when all pass. Only the standard library is used,
so any lab PC with Python 3 runs it.
"""

import hashlib
import hmac
import http.client
import http.server
import json
import os
import re
import socket
import ssl
import subprocess
import sys
import tempfile
import threading
import time

# The most a report's body may be, in bytes.
BODY_MAX = 256 * 1024

# How much of a refused body is read and thrown away, so that the client,
# which sends all of it before it reads the answer, gets the answer and
# not a reset connection.
DRAIN_MAX = 16 * 1024 * 1024

# How long a connection may be silent, in seconds.
IDLE_SECONDS = 30

STATUSES = ("received", "confirmed", "duplicate", "fixed", "wont-fix")

TEXT = "text/plain"
AGE = "application/age-encryption"

HASH = re.compile(r"[0-9a-f]{64}")
NUMBER = re.compile(r"[1-9][0-9]{0,17}")

LOCK = threading.Lock()


class Store:
    """The reports in a folder: the body, and the record beside it."""

    def __init__(self, folder):
        self.folder = folder
        os.makedirs(folder, exist_ok=True)

    def path(self, name):
        return os.path.join(self.folder, name)

    def write(self, name, data):
        """Writes a file whole or not at all, so that a reader never sees
        half of it."""
        temporary = self.path("." + name + ".new")
        with open(temporary, "wb") as out:
            out.write(data)
        os.replace(temporary, self.path(name))

    def next_number(self):
        """The number of the next report, one past the last given out
        even when that report has been deleted."""
        counter = self.path("last-number")
        try:
            with open(counter, encoding="ascii") as saved:
                last = int(saved.read())
        except (OSError, ValueError):
            last = 0
        for name in os.listdir(self.folder):
            found = re.fullmatch(r"([0-9]+)\.json", name)
            if found:
                last = max(last, int(found.group(1)))
        number = last + 1
        self.write("last-number", str(number).encode("ascii"))
        return number

    def record(self, number):
        """The record of report `number`, or None when there is none."""
        try:
            with open(self.path(f"{number}.json"), encoding="utf-8") as saved:
                return json.load(saved)
        except (OSError, ValueError):
            return None

    def save_record(self, number, record):
        self.write(f"{number}.json", (json.dumps(record, indent=2) + "\n").encode("utf-8"))

    def body_name(self, number):
        record = self.record(number)
        if record is None:
            return None
        return f"{number}.age" if record["box"] == "security" else f"{number}.txt"

    def add(self, body, content_type, box, key_hash):
        with LOCK:
            number = self.next_number()
            extension = "age" if box == "security" else "txt"
            self.write(f"{number}.{extension}", body)
            self.save_record(
                number,
                {
                    "number": number,
                    "key_hash": key_hash.lower(),
                    "status": "received",
                    "duplicate_of": None,
                    "fixed_in": None,
                    "reason": None,
                    "box": box,
                    "time": int(time.time()),
                    "content_type": content_type,
                    "bytes": len(body),
                },
            )
        return number

    def remove(self, number):
        with LOCK:
            name = self.body_name(number)
            for gone in (name, f"{number}.json"):
                if gone:
                    try:
                        os.remove(self.path(gone))
                    except FileNotFoundError:
                        pass

    def numbers(self):
        found = []
        for name in os.listdir(self.folder):
            match = re.fullmatch(r"([0-9]+)\.json", name)
            if match:
                found.append(int(match.group(1)))
        return sorted(found)


class Server(http.server.ThreadingHTTPServer):
    """HTTPS: each connection's handshake runs in the thread that serves
    it, so that a client that stalls in it holds up no one else."""

    allow_reuse_address = True

    def __init__(self, address, store, context):
        super().__init__(address, Handler)
        self.store = store
        self.context = context

    def handle_error(self, request, client_address):
        """Says what went wrong without the client's address, which the
        base class would print."""
        say(f"a connection failed: {sys.exc_info()[0].__name__}", stream=sys.stderr)


class Handler(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    timeout = IDLE_SECONDS
    body_taken = 0

    def setup(self):
        # The handshake is bound by the same limit as the rest of the
        # connection: the socket's own timeout is the one it runs under.
        self.request.settimeout(IDLE_SECONDS)
        self.request = self.server.context.wrap_socket(self.request, server_side=True)
        super().setup()

    def version_string(self):
        return "report-server"

    def finish(self):
        super().finish()
        self.request.close()

    def log_message(self, *_):
        """Logs nothing: a request's line would carry the client's address."""

    def reply(self, status, answer=None):
        data = b"" if answer is None else (json.dumps(answer) + "\n").encode("utf-8")
        self.send_response(status)
        if answer is not None:
            self.send_header("Content-Type", "application/json")
        if status != 204:
            self.send_header("Content-Length", str(len(data)))
        self.send_header("Connection", "close")
        self.end_headers()
        self.wfile.write(data)
        self.close_connection = True

    def refuse(self, status, why):
        self.drain_body()
        self.reply(status, {"error": why})

    def drain_body(self):
        """Reads and drops what the client still sends of the body. A
        client sends all of it before it reads the answer, and a server
        that closes the connection with some unread makes it reset, so
        the client never sees why it was refused."""
        raw = self.headers.get("Content-Length", "")
        left = min(int(raw), DRAIN_MAX) if raw.isascii() and raw.isdigit() else 0
        left -= self.body_taken
        while left > 0:
            chunk = self.rfile.read(min(left, 65536))
            if not chunk:
                break
            left -= len(chunk)

    def number_in_path(self, prefix):
        """The number a path `prefix/N` names, or None."""
        if not self.path.startswith(prefix):
            return None
        rest = self.path[len(prefix) :]
        return int(rest) if NUMBER.fullmatch(rest) else None

    def do_POST(self):
        if self.path != "/v1/reports":
            return self.refuse(404, "no such page")
        fields = self.fields()
        if fields is None:
            return
        content_type, box, key_hash = fields
        length = self.body_length()
        if length is None:
            return
        body = self.rfile.read(length)
        self.body_taken = len(body)
        if len(body) != length:
            return self.refuse(400, "the body is shorter than its Content-Length")
        number = self.server.store.add(body, content_type, box, key_hash)
        say(f"report {number} received: {length} bytes, box {box}")
        self.reply(201, {"number": number, "status": "received"})

    def fields(self):
        """The content type, box and key hash of a report being sent, or
        None once the request is refused."""
        raw = self.headers.get("Content-Type")
        if raw is None:
            return self.refuse(415, "Content-Type is missing")
        media, *parameters = [part.strip().lower() for part in raw.split(";")]
        for parameter in parameters:
            charset = parameter.partition("charset=")[2].strip('"')
            if parameter.startswith("charset=") and charset != "utf-8":
                return self.refuse(415, "a report is UTF-8 text")
        box = self.headers.get("Report-Box")
        if media == TEXT and box is None:
            box = "none"
        elif media == AGE and box == "security":
            pass
        elif media in (TEXT, AGE):
            return self.refuse(
                400, "Report-Box: security goes with application/age-encryption, and only with it"
            )
        else:
            return self.refuse(415, "a report is text/plain or application/age-encryption")
        key_hash = self.headers.get("Report-Key-Hash", "")
        if not HASH.fullmatch(key_hash):
            return self.refuse(
                400, "Report-Key-Hash is the SHA-256 of the key, in 64 lowercase hex digits"
            )
        return raw, box, key_hash

    def body_length(self):
        """How long the body is, or None once the request is refused."""
        if "chunked" in self.headers.get("Transfer-Encoding", "").lower():
            return self.refuse(411, "Content-Length is needed")
        raw = self.headers.get("Content-Length")
        if raw is None:
            return self.refuse(411, "Content-Length is needed")
        if not raw.isascii() or not raw.isdigit():
            return self.refuse(400, "Content-Length is a number of bytes")
        length = int(raw)
        if length == 0:
            return self.refuse(400, "the report is empty")
        if length > BODY_MAX:
            return self.refuse(413, f"a report is at most {BODY_MAX // 1024} KiB")
        return length

    def keyed(self):
        """The report the path names, once the request shows its key:
        (store, number, record). None once the request is refused."""
        number = self.number_in_path("/v1/reports/")
        record = self.server.store.record(number) if number else None
        if record is None:
            return self.refuse(404, "no such report")
        given = self.headers.get("Report-Key", "")
        if not HASH.fullmatch(given):
            return self.refuse(403, "Report-Key is the report's key, in 64 lowercase hex digits")
        hashed = hashlib.sha256(bytes.fromhex(given)).hexdigest()
        if not hmac.compare_digest(hashed, record["key_hash"]):
            return self.refuse(403, "that is not the report's key")
        return self.server.store, number, record

    def do_GET(self):
        found = self.keyed()
        if found is None:
            return
        _, number, record = found
        self.reply(
            200,
            {
                "number": number,
                "status": record["status"],
                "duplicate_of": record["duplicate_of"],
                "fixed_in": record["fixed_in"],
                "reason": record["reason"],
            },
        )

    def do_DELETE(self):
        found = self.keyed()
        if found is None:
            return
        store, number, _ = found
        store.remove(number)
        say(f"report {number} deleted")
        self.reply(204)

    def not_allowed(self):
        self.refuse(405, "only POST, GET and DELETE")

    do_PUT = do_PATCH = do_HEAD = not_allowed


def say(line, stream=None):
    print(line, file=stream or sys.stdout, flush=True)


def serve(arguments):
    options = flags(arguments, {"--port", "--cert", "--key", "--dir", "--bind"})
    missing = [name for name in ("--port", "--cert", "--key", "--dir") if name not in options]
    if missing:
        sys.exit(f"serve needs {', '.join(missing)}")
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    context.minimum_version = ssl.TLSVersion.TLSv1_2
    context.load_cert_chain(options["--cert"], options["--key"])
    store = Store(options["--dir"])
    server = Server((options.get("--bind", ""), int(options["--port"])), store, context)
    say("report server ready")
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass


def flags(arguments, names):
    """The `--name value` pairs of `arguments`, which hold nothing else."""
    found = {}
    rest = iter(arguments)
    for name in rest:
        if name not in names:
            sys.exit(f"unknown option {name}")
        try:
            found[name] = next(rest)
        except StopIteration:
            sys.exit(f"{name} needs a value")
    return found


def report_number(text):
    if not NUMBER.fullmatch(text):
        sys.exit(f"{text} is not a report number")
    return int(text)


def set_status(arguments):
    if len(arguments) < 3:
        sys.exit(
            "usage: set-status DIR NUMBER STATUS [--fixed-in VERSION] [--duplicate-of NUMBER] [--reason TEXT]"
        )
    folder, number, status = arguments[0], report_number(arguments[1]), arguments[2]
    if status not in STATUSES:
        sys.exit(f"the status is one of {', '.join(STATUSES)}, not {status}")
    options = flags(arguments[3:], {"--fixed-in", "--duplicate-of", "--reason"})
    duplicate_of = options.get("--duplicate-of")
    store = Store(folder)
    with LOCK:
        record = store.record(number)
        if record is None:
            sys.exit(f"there is no report {number} in {folder}")
        record["status"] = status
        record["fixed_in"] = options.get("--fixed-in")
        record["duplicate_of"] = report_number(duplicate_of) if duplicate_of else None
        record["reason"] = options.get("--reason")
        store.save_record(number, record)
    say(f"report {number}: {status}")


def show(arguments):
    if len(arguments) != 2:
        sys.exit("usage: show DIR NUMBER")
    store, number = Store(arguments[0]), report_number(arguments[1])
    name = store.body_name(number)
    if name is None:
        sys.exit(f"there is no report {number} in {arguments[0]}")
    with open(store.path(name), "rb") as body:
        sys.stdout.buffer.write(body.read())
        sys.stdout.buffer.flush()


def list_reports(arguments):
    if len(arguments) != 1:
        sys.exit("usage: list DIR")
    store = Store(arguments[0])
    for number in store.numbers():
        record = store.record(number)
        came = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(record["time"]))
        say(f"{number} {record['status']} {record['box']} {record['bytes']} bytes {came}")


def free_port():
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        return probe.getsockname()[1]


def read(folder, name):
    with open(os.path.join(folder, name), "rb") as saved:
        return saved.read()


class Selftest:
    """A server on a free port, and the cases of the protocol put to it."""

    def __init__(self, folder):
        self.folder = folder
        self.reports = os.path.join(folder, "reports")
        self.port = free_port()
        self.failed = 0
        self.keys = {}

    def check(self, passed, what):
        if not passed:
            self.failed += 1
            print(f"selftest: FAILED: {what}", flush=True)

    def start(self):
        cert = os.path.join(self.folder, "cert.pem")
        key = os.path.join(self.folder, "key.pem")
        made = subprocess.run(
            ["openssl", "req", "-x509", "-newkey", "ec", "-pkeyopt", "ec_paramgen_curve:prime256v1"]
            + ["-nodes", "-keyout", key, "-out", cert, "-days", "2", "-subj", "/CN=selftest"]
            + ["-addext", "subjectAltName=IP:127.0.0.1"],
            capture_output=True,
            check=False,
        )
        if made.returncode != 0:
            sys.exit(
                "selftest: openssl made no certificate: " + made.stderr.decode("utf-8", "replace")
            )
        # The client trusts the certificate as its own authority, which
        # the strict checks of a newer Python refuse for a want of key
        # identifiers some openssl configurations leave out.
        self.context = ssl.create_default_context(cafile=cert)
        self.context.verify_flags &= ~ssl.VERIFY_X509_STRICT
        self.server = subprocess.Popen(
            [sys.executable, "-I", os.path.abspath(__file__), "serve", "--port", str(self.port)]
            + ["--cert", cert, "--key", key, "--dir", self.reports, "--bind", "127.0.0.1"],
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
        )
        ready = self.server.stdout.readline().decode("utf-8", "replace").strip()
        self.check(ready == "report server ready", f'the server says "{ready}" when it listens')

    def tool(self, *arguments):
        """Runs this file's command line, as a person at the lab PC does."""
        command = [sys.executable, "-I", os.path.abspath(__file__)] + list(arguments)
        return subprocess.run(command, capture_output=True, check=False)

    def ask(self, method, path, body=None, headers=None):
        """One request over TLS: the status, and the answer's JSON or None."""
        connection = http.client.HTTPSConnection(
            "127.0.0.1", self.port, context=self.context, timeout=20
        )
        try:
            connection.request(method, path, body=body, headers=headers or {})
            response = connection.getresponse()
            data = response.read()
        finally:
            connection.close()
        return response.status, (json.loads(data) if data else None)

    def post(self, body, kind=TEXT + "; charset=utf-8", box=None):
        """Sends a report with a key of its own, which is kept to ask for it."""
        key = os.urandom(32)
        headers = {"Content-Type": kind, "Report-Key-Hash": hashlib.sha256(key).hexdigest()}
        if box:
            headers["Report-Box"] = box
        status, answer = self.ask("POST", "/v1/reports", body, headers)
        if status == 201:
            self.keys[answer["number"]] = key
        return status, answer

    def key_of(self, number):
        return {"Report-Key": self.keys[number].hex()}

    def status_of(self, number):
        return self.ask("GET", f"/v1/reports/{number}", headers=self.key_of(number))

    def count(self):
        return len(Store(self.reports).numbers())

    def run(self):
        text = "Dukovo problem report\n\n== What happened\nIt stopped, éè.\n".encode()
        self.receives(text)
        self.refuses_what_is_wrong()
        self.keeps_to_its_limit()
        self.answers_to_the_key(text)
        self.changes_status_and_shows()
        self.deletes(text)

    def receives(self, text):
        age = b"age-encryption.org/v1\n-> X25519 abc\n\nsecret bytes \x00\xff\n"
        first = self.post(text)
        self.check(first == (201, {"number": 1, "status": "received"}), f"number 1: {first}")
        second = self.post(text + b" again")
        self.check(second == (201, {"number": 2, "status": "received"}), f"number 2: {second}")
        boxed = self.post(age, kind=AGE, box="security")
        self.check(boxed == (201, {"number": 3, "status": "received"}), f"number 3: {boxed}")
        self.check(read(self.reports, "1.txt") == text, "1.txt holds the exact bytes")
        self.check(read(self.reports, "3.age") == age, "3.age holds the exact bytes")
        self.check(
            not os.path.exists(os.path.join(self.reports, "3.txt")), "a box report has no .txt"
        )
        record = json.loads(read(self.reports, "1.json"))
        self.check(record["key_hash"] == hashlib.sha256(self.keys[1]).hexdigest(), "the key's hash")
        self.check(record["box"] == "none" and record["status"] == "received", "box and status")
        self.check(record["content_type"] == "text/plain; charset=utf-8", "the content type")
        self.check(json.loads(read(self.reports, "3.json"))["box"] == "security", "the box")

    def refuses_what_is_wrong(self):
        hashed = hashlib.sha256(b"k" * 32).hexdigest()
        cases = [
            ("no key hash", TEXT, {}, 400),
            ("a short key hash", TEXT, {"Report-Key-Hash": hashed[:60]}, 400),
            ("a key hash that is not hex", TEXT, {"Report-Key-Hash": "z" * 64}, 400),
            ("no content type", None, {"Report-Key-Hash": hashed}, 415),
            ("JSON", "application/json", {"Report-Key-Hash": hashed}, 415),
            ("another charset", TEXT + "; charset=latin-1", {"Report-Key-Hash": hashed}, 415),
            ("age without the box", AGE, {"Report-Key-Hash": hashed}, 400),
            ("text in the box", TEXT, {"Report-Key-Hash": hashed, "Report-Box": "security"}, 400),
            ("another box", AGE, {"Report-Key-Hash": hashed, "Report-Box": "other"}, 400),
            ("no length", TEXT, {"Report-Key-Hash": hashed, "Transfer-Encoding": "chunked"}, 411),
        ]
        for what, kind, headers, wanted in cases:
            if kind:
                headers = dict(headers, **{"Content-Type": kind})
            # A body with no length cannot be read past, so none is sent.
            body = None if "Transfer-Encoding" in headers else b"x"
            status, answer = self.ask("POST", "/v1/reports", body, headers)
            self.check(status == wanted and answer["error"], f"{what}: {wanted}, not {status}")
        headers = {"Content-Type": TEXT, "Report-Key-Hash": hashed}
        status, _ = self.ask("POST", "/v1/reports", b"", headers)
        self.check(status == 400, f"an empty report: 400, not {status}")
        status, _ = self.ask("POST", "/v1/other", b"x", headers)
        self.check(status == 404, f"another path: 404, not {status}")
        status, _ = self.ask("PUT", "/v1/reports", b"x", headers)
        self.check(status == 405, f"PUT: 405, not {status}")
        self.check(self.count() == 3, "a refused request left a report")

    def keeps_to_its_limit(self):
        status, answer = self.post(b"x" * BODY_MAX)
        self.check(status == 201, f"256 KiB: 201, not {status} {answer}")
        number = answer["number"]
        self.check(len(read(self.reports, f"{number}.txt")) == BODY_MAX, "all 256 KiB are kept")
        status, answer = self.post(b"x" * (BODY_MAX + 1))
        self.check(status == 413, f"256 KiB and a byte: 413, not {status} {answer}")
        self.check(self.count() == 4, "the refused report was kept")
        self.deleted(number)

    def deleted(self, number):
        status, _ = self.ask("DELETE", f"/v1/reports/{number}", headers=self.key_of(number))
        self.check(status == 204, f"deleting {number}: 204, not {status}")

    def answers_to_the_key(self, text):
        wanted = {"number": 1, "status": "received"}
        wanted.update({"duplicate_of": None, "fixed_in": None, "reason": None})
        self.check(self.status_of(1) == (200, wanted), f"report 1: {self.status_of(1)}")
        wrong = [
            ("another key", {"Report-Key": os.urandom(32).hex()}),
            ("report 2's key", self.key_of(2)),
            ("no key", {}),
            ("a key that is not hex", {"Report-Key": "nonsense"}),
        ]
        for what, headers in wrong:
            status, answer = self.ask("GET", "/v1/reports/1", headers=headers)
            self.check(status == 403 and answer["error"], f"{what}: 403, not {status}")
        for path in ["/v1/reports/99", "/v1/reports/0", "/v1/reports/x", "/v1/reports/1/2"]:
            status, _ = self.ask("GET", path, headers=self.key_of(1))
            self.check(status == 404, f"{path}: 404, not {status}")
        status, _ = self.ask("GET", "/v1/other/1", headers=self.key_of(1))
        self.check(status == 404, f"another path: 404, not {status}")

    def changes_status_and_shows(self):
        text = read(self.reports, "1.txt")
        done = self.tool("set-status", self.reports, "1", "fixed", "--fixed-in", "0.16.1")
        self.check(done.returncode == 0 and b"report 1: fixed" in done.stdout, f"{done.stderr}")
        _, answer = self.status_of(1)
        self.check(answer["status"] == "fixed" and answer["fixed_in"] == "0.16.1", f"{answer}")
        self.tool("set-status", self.reports, "2", "duplicate", "--duplicate-of", "1")
        _, answer = self.status_of(2)
        self.check(answer["status"] == "duplicate" and answer["duplicate_of"] == 1, f"{answer}")
        self.tool("set-status", self.reports, "3", "wont-fix", "--reason", "works as designed")
        _, answer = self.status_of(3)
        self.check(answer["status"] == "wont-fix", f"{answer}")
        self.check(answer["reason"] == "works as designed", f"{answer}")
        self.tool("set-status", self.reports, "1", "confirmed")
        _, answer = self.status_of(1)
        self.check(answer["status"] == "confirmed" and answer["fixed_in"] is None, f"{answer}")
        self.check(self.tool("set-status", self.reports, "1", "solved").returncode != 0, "status")
        self.check(self.tool("set-status", self.reports, "99", "fixed").returncode != 0, "number")
        shown = self.tool("show", self.reports, "1")
        self.check(shown.returncode == 0 and shown.stdout == text, "show prints the exact bytes")
        self.check(self.tool("show", self.reports, "99").returncode != 0, "show of no report")
        listed = self.tool("list", self.reports).stdout.decode().splitlines()
        self.check(len(listed) == 3, f"list has a line for each report: {listed}")
        self.check(listed[0].startswith("1 confirmed none "), f"list: {listed[0]}")
        self.check(listed[2].startswith("3 wont-fix security "), f"list: {listed[2]}")

    def deletes(self, text):
        status, _ = self.ask("DELETE", "/v1/reports/1", headers={"Report-Key": "0" * 64})
        self.check(status == 403, f"deleting with a wrong key: 403, not {status}")
        self.check(os.path.exists(os.path.join(self.reports, "1.txt")), "a wrong key deleted it")
        status, _ = self.ask("DELETE", "/v1/reports/99", headers=self.key_of(1))
        self.check(status == 404, f"deleting an unknown report: 404, not {status}")
        self.deleted(1)
        self.deleted(3)
        for name in ["1.txt", "1.json", "3.age", "3.json"]:
            self.check(not os.path.exists(os.path.join(self.reports, name)), f"{name} is left")
        status, _ = self.status_of(1)
        self.check(status == 404, f"a deleted report: 404, not {status}")
        status, answer = self.post(text)
        self.check(status == 201 and answer["number"] == 5, f"numbers are not reused: {answer}")

    def finish(self):
        self.server.terminate()
        output = self.server.stdout.read().decode("utf-8", "replace")
        self.server.wait()
        self.check("127.0.0.1" not in output, f"the server printed an address: {output!r}")
        for name in os.listdir(self.reports):
            self.check(b"127.0.0.1" not in read(self.reports, name), f"{name} holds an address")


def selftest():
    with tempfile.TemporaryDirectory(prefix="report-server-") as folder:
        test = Selftest(folder)
        test.start()
        try:
            test.run()
        finally:
            test.finish()
    if test.failed:
        sys.exit(f"selftest: {test.failed} checks failed")
    print("selftest passed")


COMMANDS = {
    "serve": serve,
    "set-status": set_status,
    "show": show,
    "list": list_reports,
}


def main(arguments):
    if arguments == ["selftest"]:
        return selftest()
    if arguments and arguments[0] in COMMANDS:
        return COMMANDS[arguments[0]](arguments[1:])
    sys.exit(__doc__.split("\n\n")[1])


if __name__ == "__main__":
    main(sys.argv[1:])

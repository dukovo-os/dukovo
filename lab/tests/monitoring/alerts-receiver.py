# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

"""The lab PC's webhook receiver for the alerts-webhook test (ADR 0485).

    alerts-receiver.py PORT

It listens for HTTPS on the lab PC's port PORT with server.pem and
server.key of the working directory (alerts-ca.sh makes them), answers
every POST and PUT with 200 but those of a path that starts with
/denied, which get 403 and {"error":"forbidden"}, and writes a line to
received.log for each, whatever the service it is made to look like:

    hook slack POST /slack/T0/B0/XXXX auth=none text=[host] Test alert | ...

The kind is what the path says: ntfy, slack, discord, matrix (a path that
starts with /_matrix), json or denied. auth is bearer when the request
carries the Authorization header of a token the test gave
("Bearer lab-"), and none otherwise. text is what the service would show:
the body of an ntfy message with its Title header in front (title=), the
text of a Slack message, the content of a Discord one, the body of a
Matrix one and the whole body of a json one, on one line, with " | " for
each line break. It says "receiver ready" once it listens.

Only the standard library is used, so any lab PC with Python 3 runs it.
"""

import http.server
import json
import ssl
import sys
import threading
import urllib.parse

LOG = "received.log"
LOCK = threading.Lock()


def kind_of(path):
    """The kind of service a path stands for."""
    if path.startswith("/_matrix/"):
        return "matrix"
    first = urllib.parse.unquote(path).split("/")[1] if path.startswith("/") else ""
    return first if first in ("ntfy", "slack", "discord", "json", "denied") else "other"


def words_of(kind, body):
    """What the service shows of `body`."""
    if kind in ("ntfy", "json", "denied", "other"):
        return body
    try:
        data = json.loads(body)
    except ValueError:
        return "(not JSON) " + body
    key = {"slack": "text", "discord": "content", "matrix": "body"}[kind]
    return str(data.get(key, "(no " + key + ")"))


class Receiver(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.0"

    def log_message(self, *_):
        pass

    def take(self):
        size = int(self.headers.get("Content-Length", "0"))
        body = self.rfile.read(size).decode("utf-8", "replace")
        kind = kind_of(self.path)
        auth = "bearer" if self.headers.get("Authorization", "").startswith("Bearer lab-") else "none"
        title = self.headers.get("Title")
        text = " | ".join(words_of(kind, body).splitlines())
        line = f"hook {kind} {self.command} {self.path} auth={auth}"
        if title:
            line += f" title={title}"
        with LOCK, open(LOG, "a", encoding="utf-8") as log:
            log.write(f"{line} text={text}\n")
        status, answer = (403, b'{"error":"forbidden"}') if kind == "denied" else (200, b"{}")
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(answer)))
        self.end_headers()
        self.wfile.write(answer)

    do_POST = take
    do_PUT = take


def main():
    port = int(sys.argv[1])
    server = http.server.ThreadingHTTPServer(("0.0.0.0", port), Receiver)
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    context.load_cert_chain("server.pem", "server.key")
    server.socket = context.wrap_socket(server.socket, server_side=True)
    print("receiver ready", flush=True)
    server.serve_forever()


if __name__ == "__main__":
    main()

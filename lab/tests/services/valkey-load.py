# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

"""The lab PC's side of services/valkey: a Valkey client of its own.

    valkey-load.py write MACHINE PORT PASSWORD
    valkey-load.py check MACHINE PORT PASSWORD

`write` stores 40,000 keys over 16 connections at once, each sending its
SETs 250 at a time without waiting for each answer, and says how many
Valkey acknowledged:

    wrote 40000

`check`, after the machine restarted Valkey, reads them all back and
counts those that are missing or changed:

    read 40000, missing 0

MACHINE and PORT are the machine's address and the lab PC's port that
reaches its port 6379, as {machine} and {machine-port:6379} give them.
Only the standard library is used, so any lab PC with Python 3 runs it,
with no Valkey or Redis client of its own.
"""

import socket
import sys
import threading
import time

CONNECTIONS = 16
KEYS_EACH = 2500
BATCH = 250

# How long to wait for Valkey to answer after a restart, while it reads
# its data back.
READY_SECONDS = 30


def key(n):
    return f"lab:{n}"


def value(n):
    # About 100 bytes, different for each key.
    return f"value {n} " + "x" * (90 - len(str(n)))


def command(*args):
    """One command in RESP, the protocol's encoding."""
    out = [f"*{len(args)}\r\n".encode()]
    for arg in args:
        data = arg.encode()
        out.append(f"${len(data)}\r\n".encode() + data + b"\r\n")
    return b"".join(out)


class ValkeyError(Exception):
    pass


class Client:
    def __init__(self, host, port, password):
        self.sock = socket.create_connection((host, port), timeout=60)
        self.file = self.sock.makefile("rb")
        self.call("AUTH", password)

    def send(self, commands):
        self.sock.sendall(b"".join(commands))

    def reply(self):
        line = self.file.readline()
        if not line:
            raise ValkeyError("the connection closed")
        kind, rest = line[:1], line[1:-2].decode()
        if kind == b"+":
            return rest
        if kind == b"-":
            raise ValkeyError(rest)
        if kind == b":":
            return int(rest)
        if kind == b"$":
            length = int(rest)
            if length < 0:
                return None
            data = self.file.read(length + 2)
            return data[:-2].decode()
        if kind == b"*":
            count = int(rest)
            return None if count < 0 else [self.reply() for _ in range(count)]
        raise ValkeyError(f"an answer it does not know: {line!r}")

    def call(self, *args):
        self.send([command(*args)])
        return self.reply()

    def close(self):
        self.sock.close()


def ready(host, port, password):
    """A client, once Valkey answers and has read its data back."""
    deadline = time.monotonic() + READY_SECONDS
    while True:
        try:
            client = Client(host, port, password)
            client.call("PING")
            return client
        except (OSError, ValkeyError) as err:
            if time.monotonic() > deadline:
                sys.exit(f"Valkey does not answer: {err}")
            time.sleep(1)


def write(host, port, password, first, counts):
    client = ready(host, port, password)
    done = 0
    for start in range(first, first + KEYS_EACH, BATCH):
        numbers = range(start, start + BATCH)
        client.send([command("SET", key(n), value(n)) for n in numbers])
        done += sum(1 for _ in numbers if client.reply() == "OK")
    client.close()
    counts.append(done)


def check(host, port, password, first, counts):
    client = ready(host, port, password)
    found = 0
    for start in range(first, first + KEYS_EACH, BATCH):
        numbers = range(start, start + BATCH)
        values = client.call("MGET", *(key(n) for n in numbers))
        found += sum(1 for n, got in zip(numbers, values) if got == value(n))
    client.close()
    counts.append(found)


def main():
    if len(sys.argv) != 5 or sys.argv[1] not in ("write", "check"):
        sys.exit(__doc__.split("\n\n")[1])
    mode, host, port, password = sys.argv[1], sys.argv[2], int(sys.argv[3]), sys.argv[4]
    work = write if mode == "write" else check
    counts = []
    threads = [
        threading.Thread(target=work, args=(host, port, password, i * KEYS_EACH, counts))
        for i in range(CONNECTIONS)
    ]
    for thread in threads:
        thread.start()
    for thread in threads:
        thread.join()
    total = CONNECTIONS * KEYS_EACH
    if mode == "write":
        print(f"wrote {sum(counts)}")
    else:
        print(f"read {total}, missing {total - sum(counts)}")


if __name__ == "__main__":
    main()

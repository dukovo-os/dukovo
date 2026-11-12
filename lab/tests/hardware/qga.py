# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

"""Speaks to the machine's QEMU guest agent as a hypervisor does.

Usage: qga.py <socket> <command>...

It connects to the host end of the guest agent channel, syncs with
guest-sync-delimited (retrying while the agent starts), then sends each
command and prints `<command>: <answer>`, the answer as the agent sent
it. guest-shutdown gets no answer when it works, so it is only sent.
"""

import json
import socket
import sys
import time

SYNC_TIME = 90


def read_line(sock, buffer, timeout):
    """The next line, and what came after it."""
    sock.settimeout(timeout)
    while b"\n" not in buffer:
        chunk = sock.recv(4096)
        if not chunk:
            raise EOFError("the channel closed")
        buffer += chunk
    line, _, rest = buffer.partition(b"\n")
    return line, rest


def send(sock, request):
    sock.sendall(json.dumps(request).encode() + b"\n")


def sync(sock):
    """Syncs with the agent, as libvirt and Proxmox do before each use."""
    deadline = time.time() + SYNC_TIME
    number = int(time.time()) % 1000000
    while True:
        # A 0xFF byte resets the agent's reader; the answer starts with one.
        sock.sendall(b"\xff")
        send(sock, {"execute": "guest-sync-delimited", "arguments": {"id": number}})
        buffer = b""
        try:
            sock.settimeout(3)
            while b"\xff" not in buffer:
                chunk = sock.recv(4096)
                if not chunk:
                    raise EOFError("the channel closed")
                buffer += chunk
            buffer = buffer.split(b"\xff")[-1]
            line, buffer = read_line(sock, buffer, 3)
            if json.loads(line).get("return") == number:
                print(f"synced with the agent: {number}", flush=True)
                return buffer
        except (socket.timeout, ValueError):
            pass
        if time.time() > deadline:
            print(f"the agent did not answer within {SYNC_TIME} s", flush=True)
            sys.exit(1)


def main():
    path, commands = sys.argv[1], sys.argv[2:]
    sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    sock.connect(path)
    buffer = sync(sock)
    for command in commands:
        send(sock, {"execute": command})
        if command == "guest-shutdown":
            print(f"{command}: sent", flush=True)
            continue
        line, buffer = read_line(sock, buffer, 10)
        print(f"{command}: {line.decode()}", flush=True)
        if command == "guest-get-time":
            answer = json.loads(line).get("return", 0)
            off = abs(answer / 1e9 - time.time())
            near = "within 5 s of" if off < 5 else f"{off:.0f} s from"
            print(f"the agent's clock is {near} the lab PC's", flush=True)


if __name__ == "__main__":
    main()

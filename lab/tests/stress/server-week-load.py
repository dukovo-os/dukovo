# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

"""The lab PC's side of stress/server-week: the shop's customers.

    server-week-load.py FOLDER MACHINE HTTP-PORT SSH-PORT CONSOLE-PORT SITE ACCOUNT PASSWORD
    server-week-load.py probe MACHINE SSH-PORT CONSOLE-PORT ACCOUNT PASSWORD

FOLDER is where the test keeps its files (/tmp/lab-server-week). MACHINE is
the machine's address and the ports are the lab PC's that reach its ports
80, 22 and 9090, as {machine} and {machine-port:N} give them. SITE is the
name the website answers to, sent as the Host header, and ACCOUNT and
PASSWORD are a person who signs in to the web console.

It sends 20 requests a second to server-week.php over 4 keep-alive
connections, from the moment it prints "load running" until it is ended:
70% reads of one of 10000 items, 20% lists of the newest writes and 10%
writes. A read must bring back the item's own text, which is the md5 of
its number as the machine's setup seeded it; a write must be answered "ok"
and its sequence number. Every 5 minutes it also signs in to the machine
over SSH and runs `service nginx status`, and signs in to the web console's
API and reads its health. `probe` makes the two sign-ins once, says
`signed in over SSH, signed in to the console` or which failed, and ends
with status 1 when one did: the test asks it once after the machine's
setup.

A server that is down is an error, never a stop:
the test cuts its power and its cable on purpose.

Two files in FOLDER come of it, both appended to, never rewritten:

    acked.txt    the sequence number of each write the machine answered
                 "ok" to, one a line, in the order the answers came
    minutes.csv  a line for each minute that had anything, after it ends:
                 minute (seconds since 1970 at its start), requests,
                 errors, writes acknowledged, SSH sign-ins tried, SSH
                 sign-ins that failed, console sign-ins tried, console
                 sign-ins that failed, the page's own time for each
                 request that got an answer (the X-Took-Us header of
                 server-week.php), and the round trip of the same requests
                 as the lab PC measured it, each as `bucket=count` pairs

The week is judged by the page's own time: the lab PC may run other
machines' tests during the week, and its round trip moves with its load. The
round trip is kept to be shown, never judged.

A bucket is a range of latencies that is 5% wider than the one before it,
the first up to 0.5 ms, so that two minutes' buckets add up exactly and a
percentile of any hours is read off the sum with an error of 5%. Errors
count the requests that failed and the sign-ins that did. Only the standard
library is used.
"""

import hashlib
import http.client
import json
import math
import os
import random
import ssl
import subprocess
import sys
import threading
import time

REQUESTS_PER_SECOND = 20
CONNECTIONS = 4
PROBE_SECONDS = 300
FIRST_PROBE_SECONDS = 30
ITEMS = 10000
REQUEST_TIMEOUT = 15
SSH_TIMEOUT = 60

BUCKET_FIRST_MS = 0.5
BUCKET_FACTOR = 1.05

# The 70%, 20% and 10% of reads, lists and writes, as the share a random
# number below 1 stays under.
READS_UP_TO = 0.7
LISTS_UP_TO = 0.9


def bucket(milliseconds):
    """The bucket a latency falls in, counted from 0 for up to 0.5 ms."""
    if milliseconds <= BUCKET_FIRST_MS:
        return 0
    return math.ceil(math.log(milliseconds / BUCKET_FIRST_MS) / math.log(BUCKET_FACTOR))


class Minute:
    """What happened in one minute."""

    def __init__(self):
        self.requests = 0
        self.errors = 0
        self.acked = 0
        self.ssh_tries = 0
        self.ssh_fails = 0
        self.console_tries = 0
        self.console_fails = 0
        self.buckets = {}
        self.round_trips = {}

    def line(self, start):
        pairs = " ".join(f"{b}={n}" for b, n in sorted(self.buckets.items()))
        trips = " ".join(f"{b}={n}" for b, n in sorted(self.round_trips.items()))
        return (
            f"{start},{self.requests},{self.errors},{self.acked},{self.ssh_tries},"
            f"{self.ssh_fails},{self.console_tries},{self.console_fails},{pairs},{trips}\n"
        )


class Record:
    """The minutes so far, and the file each one goes to when it is over."""

    def __init__(self, folder):
        self.lock = threading.Lock()
        self.minutes = {}
        self.sequence = 0
        self.csv = open(os.path.join(folder, "minutes.csv"), "a", buffering=1)
        self.acked = open(os.path.join(folder, "acked.txt"), "a", buffering=1)

    def minute(self):
        start = int(time.time()) // 60 * 60
        return self.minutes.setdefault(start, Minute())

    def next_sequence(self):
        with self.lock:
            self.sequence += 1
            return self.sequence

    def request(self, page_seconds, round_trip_seconds, acked_sequence=None):
        """Counts a request that got its answer: the page took `page_seconds`
        by its own count, and the lab PC waited `round_trip_seconds`."""
        with self.lock:
            minute = self.minute()
            minute.requests += 1
            index = bucket(page_seconds * 1000)
            minute.buckets[index] = minute.buckets.get(index, 0) + 1
            index = bucket(round_trip_seconds * 1000)
            minute.round_trips[index] = minute.round_trips.get(index, 0) + 1
            if acked_sequence is not None:
                minute.acked += 1
                self.acked.write(f"{acked_sequence}\n")

    def failed_request(self):
        with self.lock:
            minute = self.minute()
            minute.requests += 1
            minute.errors += 1

    def probe(self, kind, held):
        with self.lock:
            minute = self.minute()
            if kind == "ssh":
                minute.ssh_tries += 1
                minute.ssh_fails += 0 if held else 1
            else:
                minute.console_tries += 1
                minute.console_fails += 0 if held else 1
            minute.errors += 0 if held else 1

    def write_finished_minutes(self):
        """Appends each minute that has ended, oldest first."""
        now = int(time.time()) // 60 * 60
        with self.lock:
            for start in sorted(s for s in self.minutes if s < now):
                self.csv.write(self.minutes.pop(start).line(start))


def expected_item(key):
    return f"item {key} {hashlib.md5(str(key).encode()).hexdigest()}\n"


def one_request(connection, site, record):
    """Sends one request of the mix and records how it went."""
    choice = random.random()
    sequence = None
    if choice < READS_UP_TO:
        key = random.randint(1, ITEMS)
        path = f"/server-week.php?op=read&key={key}"
        expected = expected_item(key)
    elif choice < LISTS_UP_TO:
        path = "/server-week.php?op=list"
        expected = None
    else:
        sequence = record.next_sequence()
        data = f"{random.getrandbits(64):016x}"
        path = f"/server-week.php?op=write&seq={sequence}&data={data}"
        expected = f"ok {sequence}\n"
    began = time.monotonic()
    try:
        connection.request("GET", path, headers={"Host": site})
        answer = connection.getresponse()
        body = answer.read().decode("utf-8", "replace")
    except (OSError, http.client.HTTPException):
        connection.close()
        record.failed_request()
        return
    round_trip = time.monotonic() - began
    took = answer.getheader("X-Took-Us", "")
    good = took.isdigit() and answer.status == 200 and (
        body == expected if expected is not None else body.startswith("list ")
    )
    if good:
        record.request(int(took) / 1e6, round_trip, sequence)
    else:
        record.failed_request()


def customer(number, machine, port, site, record):
    """One connection's share of the load, paced so that all share the rate."""
    interval = CONNECTIONS / REQUESTS_PER_SECOND
    connection = http.client.HTTPConnection(machine, port, timeout=REQUEST_TIMEOUT)
    due = time.monotonic() + number * interval / CONNECTIONS
    while True:
        delay = due - time.monotonic()
        if delay > 0:
            time.sleep(delay)
        one_request(connection, site, record)
        due = max(due + interval, time.monotonic())


def ssh_session(machine, port):
    """Whether the lab account signs in over SSH and runs the command.

    The command's own status is not read: the lab account is not an
    administrator, and the machine's checks, which run as root, say whether
    the services run.
    """
    key = os.path.join(
        os.environ.get("XDG_CONFIG_HOME", os.path.expanduser("~/.config")),
        "dukovo-lab",
        "lab_ed25519",
    )
    command = [
        "ssh", "-p", str(port), "-i", key,
        "-o", "IdentitiesOnly=yes", "-o", "BatchMode=yes",
        "-o", "StrictHostKeyChecking=no", "-o", "UserKnownHostsFile=/dev/null",
        "-o", "LogLevel=ERROR", "-o", "ConnectTimeout=20",
        f"lab@{machine}", "service nginx status -n 0; echo ssh-signed-in",
    ]
    try:
        done = subprocess.run(
            command, capture_output=True, timeout=SSH_TIMEOUT, stdin=subprocess.DEVNULL
        )
    except (OSError, subprocess.TimeoutExpired):
        return False
    return done.returncode == 0 and b"ssh-signed-in" in done.stdout


def console_session(machine, port, account, password):
    """Whether the account signs in to the web console's API, reads the
    console's health and signs out."""
    origin = f"https://{machine}:{port}"
    # The console's certificate is the machine's own, which the lab PC does
    # not trust, and what is checked is that the console answers.
    context = ssl.create_default_context()
    context.check_hostname = False
    context.verify_mode = ssl.CERT_NONE
    connection = http.client.HTTPSConnection(machine, port, timeout=20, context=context)

    def call(method, path, headers=None, body=None):
        send = {"Origin": origin}
        send.update(headers or {})
        connection.request(method, path, body=body, headers=send)
        answer = connection.getresponse()
        return answer, answer.read()

    try:
        credentials = json.dumps({"name": account, "password": password})
        answer, _ = call(
            "POST", "/api/1/sign-in", {"Content-Type": "application/json"}, credentials
        )
        cookie = None
        for header in answer.msg.get_all("Set-Cookie") or []:
            if header.startswith("__Host-dukovo="):
                cookie = header.split(";", 1)[0]
        if answer.status != 200 or cookie is None:
            return False
        answer, body = call("GET", "/api/1/session", {"Cookie": cookie})
        csrf = json.loads(body)["csrf"]
        answer, body = call("GET", "/api/1/health", {"Cookie": cookie})
        healthy = answer.status == 200 and json.loads(body).get("ok") is True
        # Best effort: a session the console keeps ends by itself.
        call("POST", "/api/1/sign-out", {"Cookie": cookie, "X-Dukovo-CSRF": csrf})
        return healthy
    except (OSError, http.client.HTTPException, ValueError, KeyError):
        return False
    finally:
        connection.close()


def probes(machine, ssh_port, console_port, account, password, record):
    time.sleep(FIRST_PROBE_SECONDS)
    while True:
        began = time.monotonic()
        record.probe("ssh", ssh_session(machine, ssh_port))
        record.probe("console", console_session(machine, console_port, account, password))
        time.sleep(max(0, PROBE_SECONDS - (time.monotonic() - began)))


def probe_once(arguments):
    machine, ssh_port, console_port, account, password = arguments
    ssh = ssh_session(machine, int(ssh_port))
    console = console_session(machine, int(console_port), account, password)
    print(
        f"{'signed in' if ssh else 'could not sign in'} over SSH, "
        f"{'signed in' if console else 'could not sign in'} to the console"
    )
    sys.exit(0 if ssh and console else 1)


def main(arguments):
    if len(arguments) == 6 and arguments[0] == "probe":
        probe_once(arguments[1:])
    if len(arguments) != 8:
        sys.exit(__doc__)
    folder, machine, http_port, ssh_port, console_port, site, account, password = arguments
    record = Record(folder)
    for number in range(CONNECTIONS):
        threading.Thread(
            target=customer, args=(number, machine, int(http_port), site, record), daemon=True
        ).start()
    threading.Thread(
        target=probes,
        args=(machine, int(ssh_port), int(console_port), account, password, record),
        daemon=True,
    ).start()
    print("load running", flush=True)
    while True:
        time.sleep(1)
        record.write_finished_minutes()


if __name__ == "__main__":
    main(sys.argv[1:])

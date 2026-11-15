#!/usr/bin/env python3
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

"""The lab PC's side of web-console/fuzz (ADR 0545).

    fuzz.py MACHINE SSHPORT CONSOLEPORT

MACHINE is the machine's address, SSHPORT the lab PC's port that reaches
its SSH and CONSOLEPORT the port that reaches the console (9090), as
{machine}, {machine-port:22} and {machine-port:9090} give them. It runs in
the test's own folder, after a root step has made the administrator
wc-fuzz with the password PASSWORD below.

It asks the console for its description, GET /api/1/openapi.json, and
runs Schemathesis against the machine with the description as the only
thing it knows of the API, three ways:

    signed out            every operation, with the Origin a browser gives
    a token that reads    every operation of the API (tag api), in every phase
    a token that changes  the example of each change, one change at a time

The first two send what the description says and what it does not say:
values at and beyond the limits of every field, the wrong type, no body, a
body that is not JSON. The third sends the examples alone, since a change
with made-up arguments would change the machine as it pleased; after each
example the change's undo is called, and the service the examples use is put
back as it was found (console.py).

A request that is refused changes nothing, so the first two runs leave the
machine as it was. What each run checks of every answer: it is not a 5xx,
its status, its content type and its headers are among those the
description gives, its body fits the description's schema for that
status, and it came within 2 seconds (10 for a change, which waits for
Spark). Afterwards the test checks what no single answer shows, and says
so, a line each:

    fuzz: the description is OpenAPI 3.1 and has N operations
    fuzz: signed out: no failure in N operations (N requests)
    fuzz: a token that reads: no failure in N operations (N requests)
    fuzz: a token that changes: no failure in N examples (...), each followed by its undo
    fuzz: admind answers, and has not been started again
    fuzz: the journal has the test's own changes, and nothing else (N rows)
    fuzz: the service is as it was found

Schemathesis is pinned: its release and every package it needs by hash
(fuzz-requirements.txt), in a virtual environment of the lab's cache,
$XDG_CACHE_HOME/dukovo-lab/schemathesis, made once for each requirements
file. Exits with 77 where the lab PC cannot do its part: no Python of 3.11
or later with venv, or no way to download the packages. A package whose
file has not a listed hash is a failure, not a skip. LAB_FUZZ_SEED=N sends
the requests Schemathesis made from the seed N, which a failure names.

The tokens go to Schemathesis as a header, and so may show in this PC's
process list: they are for this run alone, and are taken away at the end.
"""

import hashlib
import http.cookiejar
import json
import os
import random
import re
import shutil
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request

from console import (
    HERE,
    Console,
    Failure,
    make_token,
    on_machine,
    put_back,
    remove_token,
    service_of_the_examples,
    service_state,
    unverified_context,
    without_lab_account,
)

VERSION = "4.33.4"
REQUIREMENTS = os.path.join(HERE, "fuzz-requirements.txt")
CACHE = os.path.join(os.environ.get("XDG_CACHE_HOME") or os.path.expanduser("~/.cache"), "dukovo-lab", "schemathesis")

# The account the root step of fuzz.toml makes and its password, who reads the journal.
ACCOUNT = "wc-fuzz"
PASSWORD = "lab-console-password"

READ_TOKEN = "lab-fuzz-read"
READ_SCOPES = "services:read,certificates:read,console:read"
CHANGE_TOKEN = "lab-fuzz"

# What is checked of every answer.
CHECKS = "not_a_server_error,status_code_conformance,content_type_conformance,response_headers_conformance,response_schema_conformance"
# How long an answer may take, in seconds, and a change's, which waits for Spark to settle the service.
SLOWEST = 2
SLOWEST_CHANGE = 10
# The most requests Schemathesis sends for one operation in a phase.
EXAMPLES = 50

# The changes the third run leaves alone: certificate.renew makes the certificate the console
# serves anew, which the boot test admin-actions renews.
LEFT_ALONE = {"certificate.renew"}
# The reads the second run leaves alone: lab.missing, which a lab machine offers, runs a command
# no image has, and is answered 501 (ADM-129) by design, which Schemathesis takes for a server error.
NOT_READ = ["lab.missing"]


def schemathesis():
    """The command of the pinned Schemathesis, in a virtual environment made once for the requirements file."""
    with open(REQUIREMENTS, "rb") as file:
        pinned = hashlib.sha256(file.read()).hexdigest()[:12]
    venv = os.path.join(CACHE, f"venv-{VERSION}-{pinned}")
    command = os.path.join(venv, "bin", "schemathesis")
    if os.path.exists(os.path.join(venv, ".installed")):
        return command
    for candidate in ("python3", "python3.14", "python3.13", "python3.12", "python3.11"):
        found = shutil.which(candidate)
        # The pins were made for 3.11 and later.
        if found and subprocess.run([found, "-c", "import sys; sys.exit(sys.version_info < (3, 11))"]).returncode == 0:
            break
    else:
        print(f"the lab PC has no Python of 3.11 or later, which Schemathesis {VERSION} takes")
        sys.exit(77)
    shutil.rmtree(venv, ignore_errors=True)
    os.makedirs(CACHE, exist_ok=True)
    made = subprocess.run([found, "-m", "venv", venv], capture_output=True, text=True)
    if made.returncode != 0:
        print(f"cannot make a virtual environment with {found}: {(made.stdout + made.stderr).strip()[-300:]}")
        sys.exit(77)
    install = subprocess.run(
        [
            os.path.join(venv, "bin", "python"), "-m", "pip", "install", "--quiet", "--disable-pip-version-check",
            "--require-hashes", "--no-deps", "--only-binary", ":all:", "-r", REQUIREMENTS,
        ],
        capture_output=True, text=True,
    )
    if install.returncode != 0:
        shutil.rmtree(venv, ignore_errors=True)
        said = install.stdout + install.stderr
        if "HASH" in said.upper():
            raise Failure(f"pip refused a file that has not the hash fuzz-requirements.txt lists: {said.strip()[-900:]}")
        print(f"cannot install Schemathesis {VERSION}, as when the lab PC cannot reach the package index: {said.strip()[-300:]}")
        sys.exit(77)
    with open(os.path.join(venv, ".installed"), "w") as file:
        file.write(VERSION + "\n")
    return command


class Administrator:
    """The account wc-fuzz signed in as a browser is, which reads the journal. A token cannot."""

    def __init__(self, machine, port):
        self.origin = f"https://{machine}:{port}"
        self.csrf = None
        self.opener = urllib.request.build_opener(
            urllib.request.HTTPCookieProcessor(http.cookiejar.CookieJar()),
            urllib.request.HTTPSHandler(context=unverified_context()),
        )

    def call(self, method, path, body=None):
        """Gives the status, the text and the Retry-After header (or None) of the answer to a request."""
        headers = {"Origin": self.origin}
        if self.csrf is not None:
            headers["X-Dukovo-CSRF"] = self.csrf
        data = None
        if body is not None:
            headers["Content-Type"] = "application/json"
            data = json.dumps(body).encode()
        request = urllib.request.Request(self.origin + path, data=data, method=method, headers=headers)
        try:
            with self.opener.open(request, timeout=60) as answer:
                return answer.status, answer.read().decode(), answer.headers.get("Retry-After")
        except urllib.error.HTTPError as error:
            return error.code, error.read().decode(), error.headers.get("Retry-After")

    def sign_in(self):
        """Signs in, waiting out the limit of ten tries a minute that the signed-out run may have used up."""
        for _ in range(4):
            status, text, wait = self.call("POST", "/api/1/sign-in", {"name": ACCOUNT, "password": PASSWORD})
            if status == 429:
                time.sleep(int(wait or "10") + 1)
                continue
            if status != 200 or "step" in json.loads(text):
                raise Failure(f"{ACCOUNT} was not signed in, for it was answered {status}: {text}")
            status, text, _ = self.call("GET", "/api/1/session")
            if status != 200:
                raise Failure(f"GET /api/1/session was answered {status}: {text}")
            self.csrf = json.loads(text)["csrf"]
            return
        raise Failure(f"{ACCOUNT} could not be signed in: the console went on answering 429")

    def changes(self):
        """The journal's newest rows, signing in again if the session has ended."""
        for attempt in (1, 2):
            status, text, _ = self.call("GET", "/api/1/changes?limit=200")
            if status == 200:
                return json.loads(text)["changes"]
            if status == 401 and attempt == 1:
                self.sign_in()
                continue
            raise Failure(f"GET /api/1/changes was answered {status}: {text}")
        return []


def described(outsider, folder):
    """Fetches the description signed out, checks what makes it one, writes it to the folder, and gives its path and
    the number of its operations."""
    status, text = outsider.call("GET", "/api/1/openapi.json")
    if status != 200:
        raise Failure(f"GET /api/1/openapi.json was answered {status}: {text[:300]}")
    try:
        document = json.loads(text)
    except ValueError as err:
        raise Failure(f"GET /api/1/openapi.json was answered text that is not JSON: {err}") from err
    if not str(document.get("openapi", "")).startswith("3.1."):
        raise Failure(f"the description says it is OpenAPI {document.get('openapi')}, and it is 3.1")
    operations = sum(1 for item in document.get("paths", {}).values() for method in item if method in ("get", "post", "head"))
    if operations < 20:
        raise Failure(f"the description has {operations} operations, and the console has more than that")
    path = os.path.join(folder, "openapi.json")
    with open(path, "w") as file:
        file.write(text)
    return path, operations


def run_schemathesis(command, folder, document, origin, name, seed, headers, options, slowest):
    """Runs Schemathesis against the console with the description, and gives its output. Fails with
    what it found, and the seed, when any answer is not what the description says."""
    arguments = [
        command, "run", document, "--url", origin, "--tls-verify", "false", "--no-color", "--seed", str(seed),
        "--workers", "2", "--warnings", "off", "--generation-with-security-parameters", "false",
        "--generation-database", "none", "--checks", CHECKS, "--max-response-time", str(slowest),
        "--max-examples", str(EXAMPLES),
    ]
    for header in headers:
        arguments += ["-H", header]
    env = dict(os.environ, NO_PROXY="*", no_proxy="*")
    run = subprocess.run(arguments + options, capture_output=True, text=True, cwd=folder, env=env, timeout=3600)
    output = run.stdout + run.stderr
    if run.returncode != 0:
        # The failures come before the summary; the first few say what is wrong.
        failures = output.find("FAILURES")
        summary = output.rfind("SUMMARY")
        shown = output[failures : failures + 3500] if failures >= 0 else ""
        raise Failure(
            f"{name}: Schemathesis ended with status {run.returncode}; rerun with LAB_FUZZ_SEED={seed}:\n"
            f"{shown}\n{output[summary:][-1500:] if summary >= 0 else output[-1500:]}"
        )
    return output


def tested_in(output, name):
    """The operations and the test cases that Schemathesis's summary of `output` counts. Fails when it
    counts none, which is a run that tested nothing, or when the summary is not there to read."""
    tested = re.search(r"^\s*Tested: (\d+)", output, re.MULTILINE)
    cases = re.search(r"(\d+) generated", output)
    if tested is None or cases is None or int(tested.group(1)) == 0 or int(cases.group(1)) == 0:
        raise Failure(f"{name}: the summary of Schemathesis counts no operation tested: {output[-800:]}")
    return int(tested.group(1)), int(cases.group(1))


def admind_of(machine, ssh_port):
    """What `service list --json` says of admind that changes when it is started again."""
    text = on_machine(machine, ssh_port, "service list --json")
    try:
        rows = json.loads(text)["services"]
    except (ValueError, KeyError) as err:
        raise Failure(f"`service list --json` did not give the services: {err}: {text[:300]}") from err
    row = next((each for each in rows if each.get("name") == "admind"), None)
    if row is None:
        raise Failure("`service list --json` has no admind")
    return {key: row.get(key) for key in ("state", "pid", "started_ms", "restarts")}


def main():
    machine, ssh_port, console_port = sys.argv[1:4]
    without_lab_account()
    command = schemathesis()
    seed = int(os.environ.get("LAB_FUZZ_SEED") or random.randrange(1, 2**31))
    print(f"fuzz: seed {seed}")
    origin = f"https://{machine}:{console_port}"
    outsider = Console(machine, console_port, None)
    try:
        read_token = make_token(READ_TOKEN, machine, ssh_port, READ_SCOPES)
        change_token = make_token(CHANGE_TOKEN, machine, ssh_port)
        with tempfile.TemporaryDirectory(prefix="lab-fuzz-") as folder:
            document, operations = described(outsider, folder)
            print(f"fuzz: the description is OpenAPI 3.1 and has {operations} operations")

            administrator = Administrator(machine, console_port)
            administrator.sign_in()
            first_row = max((row["id"] for row in administrator.changes()), default=0)
            admind = admind_of(machine, ssh_port)
            changer = Console(machine, console_port, change_token)
            actions = changer.ask("GET", "/api/1/actions")
            service = service_of_the_examples(actions)
            before = service_state(changer, service) if service else None

            # A browser's requests carry the Origin of the console's own page, so those that
            # would change anything are asked as a session's are, and the sign-in routes, which
            # a request must pass the origin check to reach, are reached.
            output = run_schemathesis(command, folder, document, origin, "signed out", seed, [f"Origin: {origin}"], [], SLOWEST)
            print("fuzz: signed out: no failure in %d operations (%d requests)" % tested_in(output, "signed out"))

            output = run_schemathesis(
                command, folder, document, origin, "a token that reads", seed,
                [f"Authorization: Bearer {read_token}"],
                ["--include-tag", "api"] + [option for read in NOT_READ for option in ("--exclude-operation-id", read)],
                SLOWEST,
            )
            print("fuzz: a token that reads: no failure in %d operations (%d requests)" % tested_in(output, "a token that reads"))

            ran = []
            for action in actions:
                if not action["changes"] or action["id"] in LEFT_ALONE:
                    continue
                output = run_schemathesis(
                    command, folder, document, origin, f"a token that changes, {action['id']}", seed,
                    [f"Authorization: Bearer {change_token}"],
                    ["--include-operation-id", action["id"], "--phases", "examples"], SLOWEST_CHANGE,
                )
                tested_in(output, action["id"])
                ran.append(action["id"])
                put_back(changer, actions, action, service, before)
            if not ran:
                raise Failure("the list of actions has no change to run")
            print(f"fuzz: a token that changes: no failure in {len(ran)} examples ({', '.join(ran)}), each followed by its undo")

            health = outsider.ask("GET", "/api/1/health")
            if health != {"ok": True}:
                raise Failure(f"GET /api/1/health was answered {health}, and it is {{\"ok\": true}}")
            after = admind_of(machine, ssh_port)
            if after != admind:
                raise Failure(f"admind was started again: `service list --json` said {admind}, and now says {after}")
            print("fuzz: admind answers, and has not been started again")

            rows = [row for row in administrator.changes() if row["id"] > first_row]
            for row in rows:
                if row["how"] != f"token {CHANGE_TOKEN}" or row["action"] not in ran:
                    raise Failure(
                        f"the journal has a change that is not the test's own: row {row['id']}, {row['action']} "
                        f"by {row['who']}, {row['how']}"
                    )
            if not rows:
                raise Failure("the journal has no row for the changes the test made")
            print(f"fuzz: the journal has the test's own changes, and nothing else ({len(rows)} rows)")

            if service and service_state(changer, service) != before:
                raise Failure(f"{service} is {service_state(changer, service)}, and it was found {before}")
            print("fuzz: the service is as it was found")
    finally:
        remove_token(CHANGE_TOKEN, machine, ssh_port)
        remove_token(READ_TOKEN, machine, ssh_port)


if __name__ == "__main__":
    try:
        main()
    except Failure as failure:
        print(f"FAILED: {failure}")
        sys.exit(1)

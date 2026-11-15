# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

"""What the lab PC's Python scripts of the web console's API share (ADR 0515, ADR 0545).

web-console/ansible (ansible.py) and web-console/fuzz (fuzz.py) both ask the
console's API from the lab PC with an API token, which `admin token add`
makes over SSH (token.sh), and both put back the service they change.
Neither is run alone: each is run by its test, in the test's own folder, and
imports this file from there.
"""

import json
import os
import ssl
import subprocess
import sys
import time
import urllib.error
import urllib.parse
import urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))
CONFIG = os.path.join(os.environ.get("XDG_CONFIG_HOME") or os.path.expanduser("~/.config"), "dukovo-lab")
PASSWORD_FILE = os.path.join(CONFIG, "lab_password")
KEY_FILE = os.path.join(CONFIG, "lab_ed25519")

# The states a service passes through on its way to another.
PASSING = {"starting", "stopping", "pending", "waiting"}


class Failure(Exception):
    """What the test found wrong, in a sentence."""


def without_lab_account():
    """Says why the test cannot be run and exits with 77, which skips it, when the lab PC has no key or password of the lab account."""
    if not os.path.exists(PASSWORD_FILE) or not os.path.exists(KEY_FILE):
        print(f"no lab key or password in {CONFIG}: an API token is made with the lab account's password")
        sys.exit(77)


def token_script(*arguments):
    """Runs token.sh with `arguments`, and gives what it printed. Exits with 77 where token.sh does."""
    run = subprocess.run(["sh", os.path.join(HERE, "token.sh"), *arguments], capture_output=True, text=True)
    if run.returncode == 77:
        print(run.stdout.strip())
        sys.exit(77)
    if run.returncode != 0:
        raise Failure(f"token.sh {arguments[0]} ended with status {run.returncode}: {run.stdout}{run.stderr}".strip())
    return run.stdout


def make_token(name, machine, ssh_port, scope="*"):
    """Makes the API token `name` for `scope`, which is `*` or scopes joined by commas, and gives it."""
    token = token_script("make", name, machine, ssh_port, scope).strip()
    if not token.startswith("dkv_"):
        raise Failure("admin token add printed no token")
    return token


def remove_token(name, machine, ssh_port):
    """Takes the token `name` away, and says so when it cannot."""
    try:
        token_script("remove", name, machine, ssh_port)
    except Failure as failure:
        print(f"the token {name} was not taken away: {failure}")


def on_machine(machine, ssh_port, command):
    """Runs `command` on the machine as the lab account, over SSH with the lab's key, and gives its standard output."""
    run = subprocess.run(
        [
            "ssh", "-p", ssh_port, "-i", KEY_FILE,
            "-o", "StrictHostKeyChecking=no", "-o", "UserKnownHostsFile=/dev/null", "-o", "BatchMode=yes",
            "-o", "IdentitiesOnly=yes", "-o", "LogLevel=ERROR",
            f"lab@{machine}", command,
        ],
        capture_output=True, text=True, timeout=120, stdin=subprocess.DEVNULL,
    )
    if run.returncode != 0:
        raise Failure(f"`{command}` on the machine ended with status {run.returncode}: {run.stdout}{run.stderr}".strip())
    return run.stdout


def unverified_context():
    """TLS that checks no certificate: the machine's own is none an authority signed, and every other test of the console reads it as it is."""
    context = ssl.create_default_context()
    context.check_hostname = False
    context.verify_mode = ssl.CERT_NONE
    return context


class Console:
    """The console's API, asked with `token`, or with none."""

    def __init__(self, machine, port, token):
        self.origin = f"https://{machine}:{port}"
        self.token = token
        self.context = unverified_context()

    def call(self, method, path, body=None):
        """Gives the status and the text of the answer to a request."""
        headers = {}
        if self.token is not None:
            headers["Authorization"] = f"Bearer {self.token}"
        data = None
        if body is not None:
            headers["Content-Type"] = "application/json"
            data = json.dumps(body).encode()
        request = urllib.request.Request(self.origin + path, data=data, method=method, headers=headers)
        try:
            with urllib.request.urlopen(request, context=self.context, timeout=120) as answer:
                return answer.status, answer.read().decode()
        except urllib.error.HTTPError as error:
            return error.code, error.read().decode()

    def ask(self, method, path, body=None):
        """The JSON a call answers 200 with."""
        status, text = self.call(method, path, body)
        if status != 200:
            raise Failure(f"{method} {path} was answered {status}: {text}")
        try:
            return json.loads(text)
        except ValueError as err:
            raise Failure(f"{method} {path} was answered {text!r}, which is not JSON: {err}") from err


def address(template, args):
    """`template` with each {name} part filled in from `args` and percent-encoded, and the arguments left over."""
    rest = dict(args)
    parts = []
    for part in template.split("/"):
        name = part[1:-1] if part.startswith("{") and part.endswith("}") else None
        if name is None:
            parts.append(part)
        elif name in rest:
            parts.append(urllib.parse.quote(str(rest.pop(name)), safe="-._~"))
        else:
            raise Failure(f"{template} wants {name}, which the example does not give")
    return "/".join(parts), rest


def route(console, action, args):
    """Calls the route of `action` with `args`, and gives its answer."""
    path, rest = address(action["path"], args)
    if not action["changes"]:
        if rest:
            raise Failure(f"{action['id']}'s example has arguments its address does not take")
        return console.ask("GET", path)
    return console.ask("POST", path, rest)


def service_state(console, name):
    """What the console lists of the service `name` once it has settled, for up to 30 seconds: its
    state and whether it starts at boot."""
    deadline = time.monotonic() + 30
    while True:
        listed = console.ask("GET", "/api/1/services")
        found = [service for service in listed.get("services", []) if service.get("name") == name]
        if not found:
            raise Failure(f"the list of services has no {name}")
        state = (found[0].get("state"), found[0].get("enabled"))
        if state[0] not in PASSING or time.monotonic() >= deadline:
            return state
        time.sleep(0.5)


def service_of_the_examples(actions):
    """The service the examples of the services' changes use, if there is one."""
    return next((a["examples"]["good"][0].get("name") for a in actions if a["area"] == "services" and a["changes"]), None)


def put_back(console, actions, action, service, before):
    """Leaves `service` as `before` has it after `action` was done: the action's undo, and the action
    itself again where the undo left the service elsewhere. Says what is wrong if it is still not.
    An action of another thing than a service has its undo and no more."""
    undo = action["examples"]["undo"]
    if undo is not None:
        undone = next((each for each in actions if each["id"] == undo["action"]), None)
        if undone is None:
            raise Failure(f"the list has no {undo['action']}, the undo of {action['id']}")
        route(console, undone, undo["args"])
    if service is None or action["area"] != "services":
        return
    if service_state(console, service) != before:
        route(console, action, action["examples"]["good"][0])
    now = service_state(console, service)
    if now != before:
        raise Failure(f"after {action['id']} and its undo {service} is {now}, and it was {before}")

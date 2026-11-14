#!/usr/bin/env python3
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

"""The lab PC's side of web-console/ansible (ADR 0514).

    ansible.py MACHINE SSHPORT CONSOLEPORT

MACHINE is the machine's address, SSHPORT the lab PC's port that reaches
its SSH and CONSOLEPORT the port that reaches the console (9090), as
{machine}, {machine-port:22} and {machine-port:9090} give them. It runs in
the test's own folder.

It makes an API token for every scope with `admin token add` (token.sh),
asks the console for the list of actions and, for each action, for the
Ansible task that POST /api/1/show gives for the action's first good
example. Each task is run by ansible-playbook over SSH as the lab account,
in a play of its own and with sudo for a task that becomes root, and the
whole round is made twice. It says what it found, a line each:

    pass 1: every task succeeded
    pass 2: every task succeeded
    reads reported no change
    changes reported changed both times
    state after each change is what its route leaves

A task of ansible.builtin.command cannot tell whether it changed anything,
so a change reports "changed" on the second pass too, as the first;
nothing here claims it is idempotent. That waits for the dukovo.system
collection (docs/deferred.md), whose modules will.

certificate.renew is not run: it makes the certificate the console serves
anew, and the API calls that follow would have to be made again with
another certificate. The boot test admin-actions renews it.
"""

import json
import os
import re
import ssl
import subprocess
import sys
import tempfile
import urllib.error
import urllib.parse
import urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))
LEFT_ALONE = {"certificate.renew"}
TOKEN_NAME = "lab-ansible"
CONFIG = os.path.join(os.environ.get("XDG_CONFIG_HOME") or os.path.expanduser("~/.config"), "dukovo-lab")
PASSWORD_FILE = os.path.join(CONFIG, "lab_password")
KEY_FILE = os.path.join(CONFIG, "lab_ed25519")

# What ansible-playbook prints at the end of a play, for one host.
RECAP = re.compile(
    r"^machine\s*:\s*ok=(?P<ok>\d+)\s+changed=(?P<changed>\d+)\s+unreachable=(?P<unreachable>\d+)"
    r"\s+failed=(?P<failed>\d+)",
    re.MULTILINE,
)


class Failure(Exception):
    """What the test found wrong, in a sentence."""


def indent(text, spaces):
    """`text` with every line that has anything on it indented by `spaces`."""
    pad = " " * spaces
    return "".join(pad + line if line.strip() else line for line in text.splitlines(keepends=True))


def playbook(task):
    """A play of its own for the machine, with the one task, which is the text `POST /api/1/show` gives."""
    return "- hosts: machine\n  gather_facts: false\n  tasks:\n" + indent(task, 4)


def recap_of(output):
    """The counts of the play's recap in `output` as numbers, or None when there is no recap."""
    found = RECAP.search(output)
    return {name: int(count) for name, count in found.groupdict().items()} if found else None


def inventory(machine, ssh_port):
    """The inventory: the lab account over SSH with the lab's key, Python found by name, sudo with the lab's password."""
    return f"""all:
  hosts:
    machine:
      ansible_host: "{machine}"
      ansible_port: {ssh_port}
      ansible_user: lab
      ansible_ssh_private_key_file: "{KEY_FILE}"
      ansible_ssh_common_args: "-o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o IdentitiesOnly=yes"
      # Python from the catalog (docs/handbook/src/python.md).
      ansible_python_interpreter: python3
      # A task that becomes root runs `sudo -H -S -p PROMPT -u root ...`:
      # Ansible asks for -H and -S by default, and -n only when it has no
      # password to give, which it has here. Dukovo's sudo(8) is elevate(8),
      # which takes -S, -p and -u root as sudo does and -H as nothing, since
      # the home it runs with is always root's, so no become_flags are set.
      ansible_become_method: sudo
      ansible_become_password: "{{{{ lookup('ansible.builtin.env', 'LAB_BECOME_PASSWORD') }}}}"
"""


def token_script(*arguments):
    """Runs token.sh with `arguments`, and gives what it printed."""
    run = subprocess.run(["sh", os.path.join(HERE, "token.sh"), *arguments], capture_output=True, text=True)
    if run.returncode == 77:
        print(run.stdout.strip())
        sys.exit(77)
    if run.returncode != 0:
        raise Failure(f"token.sh {arguments[0]} ended with status {run.returncode}: {run.stdout}{run.stderr}".strip())
    return run.stdout


class Console:
    """The console's API, asked with the token. The lab PC trusts no one's certificate here: the
    machine's own is none an authority signed, and every other test of the console reads it as it is."""

    def __init__(self, machine, port, token):
        self.origin = f"https://{machine}:{port}"
        self.token = token
        self.context = ssl.create_default_context()
        self.context.check_hostname = False
        self.context.verify_mode = ssl.CERT_NONE

    def call(self, method, path, body=None):
        headers = {"Authorization": f"Bearer {self.token}"}
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


def route(console, action, args):
    """Calls the route of `action` with `args`, and gives its answer."""
    path, rest = address(action["path"], args)
    if not action["changes"]:
        if rest:
            raise Failure(f"{action['id']}'s example has arguments its address does not take")
        return console.ask("GET", path)
    return console.ask("POST", path, rest)


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


def service_state(console, name):
    """What the console lists of the service `name`: its state and whether it starts at boot."""
    listed = console.ask("GET", "/api/1/services")
    for service in listed.get("services", []):
        if service.get("name") == name:
            return (service.get("state"), service.get("enabled"))
    raise Failure(f"the list of services has no {name}")


class Round:
    """Runs the tasks of the actions through ansible-playbook, in a folder of its own."""

    def __init__(self, console, actions, machine, ssh_port, folder):
        self.console = console
        self.actions = actions
        self.folder = folder
        self.inventory = os.path.join(folder, "inventory.yml")
        with open(self.inventory, "w") as file:
            file.write(inventory(machine, ssh_port))
        with open(PASSWORD_FILE) as file:
            self.password = file.readline().rstrip("\n")
        self.tasks = {}
        for action in actions:
            shown = console.ask("POST", "/api/1/show", {"action": action["id"], "args": action["examples"]["good"][0]})
            self.tasks[action["id"]] = shown["ansible"]

    def by_id(self, action_id):
        for action in self.actions:
            if action["id"] == action_id:
                return action
        raise Failure(f"the list has no {action_id}")

    def run_task(self, action):
        """Runs the task of `action` and gives its counts: ok, changed, unreachable, failed."""
        task = self.tasks[action["id"]]
        name = os.path.join(self.folder, action["id"] + ".yml")
        with open(name, "w") as file:
            file.write(playbook(task))
        env = dict(os.environ)
        env.update(
            ANSIBLE_HOST_KEY_CHECKING="False",
            ANSIBLE_NOCOLOR="1",
            ANSIBLE_RETRY_FILES_ENABLED="False",
            ANSIBLE_LOCAL_TEMP=os.path.join(self.folder, "tmp"),
            ANSIBLE_PIPELINING="False",
            LAB_BECOME_PASSWORD=self.password,
        )
        command = ["ansible-playbook", "-i", self.inventory, name]
        run = subprocess.run(command, capture_output=True, text=True, env=env, timeout=600)
        counts = recap_of(run.stdout)
        if counts is None:
            raise Failure(f"ansible-playbook for {action['id']} printed no recap (status {run.returncode}): {run.stdout[-600:]}{run.stderr[-600:]}")
        if run.returncode != 0 or counts["failed"] or counts["unreachable"] or counts["ok"] != 1:
            raise Failure(f"{action['id']} by Ansible: {counts} (status {run.returncode}): {run.stdout[-900:]}")
        return counts


def one_pass(round_, number, service):
    """Every task once: a read changes nothing, a change is done as its route does it. Gives what it saw of each."""
    reads, changes = [], []
    console = round_.console
    actions = [action for action in round_.actions if action["id"] not in LEFT_ALONE]

    def state():
        return service_state(console, service) if service else None

    before = state()
    for action in (each for each in actions if not each["changes"]):
        counts = round_.run_task(action)
        if counts["changed"]:
            raise Failure(f"pass {number}: {action['id']} reported a change, and it only reads")
        reads.append(action["id"])
    for action in (each for each in actions if each["changes"]):
        args = action["examples"]["good"][0]
        undo = action["examples"]["undo"]
        if undo is None:
            # A restart: the service is as it was, as another process.
            counts = round_.run_task(action)
            now = state()
            if now != before:
                raise Failure(f"pass {number}: after {action['id']} the service is {now}, and it was {before}")
        else:
            undo_action = round_.by_id(undo["action"])
            # From the state the undo leaves, so that the change has something to do.
            route(console, undo_action, undo["args"])
            counts = round_.run_task(action)
            by_ansible = state()
            route(console, undo_action, undo["args"])
            route(console, action, args)
            by_route = state()
            if by_ansible != by_route:
                raise Failure(
                    f"pass {number}: after {action['id']} Ansible leaves the service {by_ansible}, and the route leaves it {by_route}"
                )
            # Put back as it was: the undo, and the change again where the undo left it elsewhere.
            route(console, undo_action, undo["args"])
            if state() != before:
                route(console, action, args)
            now = state()
            if now != before:
                raise Failure(f"pass {number}: after {action['id']} and its undo the service is {now}, and it was {before}")
        changes.append((action["id"], counts["changed"]))
    return reads, changes


def main():
    machine, ssh_port, console_port = sys.argv[1:4]
    if not os.path.exists(PASSWORD_FILE) or not os.path.exists(KEY_FILE):
        print(f"no lab key or password in {CONFIG}: the lab account's password becomes root for Ansible")
        sys.exit(77)
    token = token_script("make", TOKEN_NAME, machine, ssh_port).strip()
    if not token.startswith("dkv_"):
        raise Failure("admin token add printed no token")
    try:
        console = Console(machine, console_port, token)
        actions = console.ask("GET", "/api/1/actions")
        if not actions:
            raise Failure("GET /api/1/actions gave no actions")
        # The service the examples of the services' actions use, if there is one.
        service = next(
            (a["examples"]["good"][0].get("name") for a in actions if a["area"] == "services" and a["changes"]), None
        )
        with tempfile.TemporaryDirectory(prefix="lab-ansible-") as folder:
            round_ = Round(console, actions, machine, ssh_port, folder)
            seen = []
            for number in (1, 2):
                seen.append(one_pass(round_, number, service))
                print(f"pass {number}: every task succeeded")
        if any(not reads for reads, _ in seen):
            raise Failure("the list has no read to run")
        print("reads reported no change")
        if any(changed != 1 for _, changes in seen for _, changed in changes):
            raise Failure(f"a change did not report changed in both passes: {seen[0][1]} then {seen[1][1]}")
        print("changes reported changed both times")
        print("state after each change is what its route leaves")
    finally:
        try:
            token_script("remove", TOKEN_NAME, machine, ssh_port)
        except Failure as failure:
            print(f"the token {TOKEN_NAME} was not taken away: {failure}")


if __name__ == "__main__":
    try:
        main()
    except Failure as failure:
        print(f"FAILED: {failure}")
        sys.exit(1)

#!/usr/bin/env python3
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

"""The lab PC's side of web-console/ansible (ADR 0515, ADR 0545).

    ansible.py MACHINE SSHPORT CONSOLEPORT

MACHINE is the machine's address, SSHPORT the lab PC's port that reaches
its SSH and CONSOLEPORT the port that reaches the console (9090), as
{machine}, {machine-port:22} and {machine-port:9090} give them. It runs in
the test's own folder.

It makes an API token for every scope with `admin token add` (token.sh),
asks the console for the list of actions and, for each action, for the
Ansible task that POST /api/1/show gives for the action's first good
example. Each task is run by ansible-playbook, in a play of its own, with
the pinned ansible-core and the pinned collection dukovo.system that
tests/system/ansible.sh sets up and runs it with, over SSH as the lab
account, and with sudo for a task that becomes root. Each task is run
twice in a row; before a change is run the first time, its undo is
done by its route, so that it has something to do. It says what it found,
a line each:

    pass 1: every task succeeded
    pass 2: every task succeeded
    reads reported no change
    changes with a module reported no change the second time
    commands reported changed both times
    state after each change is what its route leaves
    check mode agreed with Ansible on each change

A change of the services is a task of the collection's module, which reads
what is there and does nothing when it is as asked, so the second run
reports no change; a restart is the one that always does something, and is
said to be left out of that. A task of ansible.builtin.command cannot tell
whether it changed anything, so a change by it reports "changed" the
second time too, as the first. Before a change's first run and after its
second, POST /api/1/check is asked whether the change would change
anything: it must say true before the first run, and after the second
what the module reported, true for a restart and false for the others; a
change that is a command must not be said to change anything again, since
check mode reads the state, and it may say it cannot tell, unless it
compares nothing, as lab.long, which check mode says changes each time. A change whose
undo is itself with the same arguments, as console.reach's, finds nothing
to do from the start, so check mode must say false before it too; and one
that has no undo and compares something, as console.export, starts from
what the machine has, so check mode may say before it that it cannot
tell.

The console's own reads run here as the commands they show: console.about
as `admin about --json`, console.notices as `admin notices --json` and
metrics.now as `metrics now --json`.

certificate.renew is not run: it makes the certificate the console serves
anew, and the API calls that follow would have to be made again with
another certificate. The boot test admin-actions renews it. checks.last,
lab.missing, metrics.usage, metrics.around and service.usage are not run
either, for the reasons at LEFT_ALONE.

Nor are the changes of the network's settings and the firewall's rules
(ADR 0518): each is tried, and Spark undoes it unless it is kept, so it
has no state to begin from and come back to as this round's changes do,
and its keep and undo need a trial running. Their tasks are the commands
the console shows, which the boot test admin-trials runs, and the browser
tests web-console/networking and web-console/firewall try, keep and undo.
"""

import os
import subprocess
import sys
import tempfile

from console import (
    HERE,
    PASSWORD_FILE,
    Console,
    Failure,
    make_token,
    put_back,
    remove_token,
    route,
    service_of_the_examples,
    service_state,
    without_lab_account,
)

# The tasks this test leaves alone, each with its reason: certificate.renew
# makes the machine's certificate anew; checks.last runs dukovo-check, which
# a machine has only once an update brings the checks, and lab.missing runs
# a command that no image has, so both end with a status other than 0;
# metrics.usage, metrics.around and service.usage end with 1 on a machine
# that has run for less than the 10 minutes in which metricsd writes the
# history they read; and the changes that are tried (TRIED) need a trial.
TRIED = {
    "network.set", "network.keep", "network.undo",
    "firewall.add", "firewall.replace", "firewall.remove", "firewall.keep", "firewall.undo",
}
LEFT_ALONE = {"certificate.renew", "checks.last", "lab.missing", "metrics.usage", "metrics.around", "service.usage"} | TRIED
# The changes that compare nothing, so that the console counts each run as a change and
# check mode says each would change something, whatever the machine is like: a restart,
# and a lab build's lab.long, which runs its steps each time.
ALWAYS_CHANGES = {"service.restart", "lab.long"}
TOKEN_NAME = "lab-ansible"
ANSIBLE_SH = os.path.join(HERE, "..", "system", "ansible.sh")
MODULE = "dukovo.system."


def indent(text, spaces):
    """`text` with every line that has anything on it indented by `spaces`."""
    pad = " " * spaces
    return "".join(pad + line if line.strip() else line for line in text.splitlines(keepends=True))


def playbook(task):
    """A play of its own for the machine, with the one task, which is the text `POST /api/1/show` gives."""
    return "- hosts: dukovo\n  gather_facts: false\n  tasks:\n" + indent(task, 4)


def count(name, output):
    """The number that ansible.sh printed for `name` (changed, unreachable or failed), or None.

    ansible.sh prints its counts after Ansible's own output, whose task lines
    also start `changed: [dukovo]` or `failed: [dukovo]`, so the last such
    line is the count.
    """
    for line in reversed(output.splitlines()):
        if line.startswith(name + ": "):
            value = line[len(name) + 2 :].strip()
            return int(value) if value.isdigit() else None
    return None


class Round:
    """Runs the tasks of the actions with ansible.sh, in a folder of its own for each run."""

    def __init__(self, console, actions, machine, ssh_port, folder):
        self.console = console
        self.actions = actions
        self.machine = machine
        self.ssh_port = ssh_port
        self.folder = folder
        with open(PASSWORD_FILE) as file:
            self.password = file.readline().rstrip("\n")
        self.tasks = {}
        for action in actions:
            shown = console.ask("POST", "/api/1/show", {"action": action["id"], "args": action["examples"]["good"][0]})
            self.tasks[action["id"]] = shown["ansible"]
        self.runs = 0

    def by_id(self, action_id):
        for action in self.actions:
            if action["id"] == action_id:
                return action
        raise Failure(f"the list has no {action_id}")

    def is_module(self, action):
        """Whether the task the console shows for `action` is a task of the collection's module."""
        return MODULE in self.tasks[action["id"]]

    def would_change(self, action):
        """What POST /api/1/check says of the first good example of `action`: true, false or None."""
        answer = self.console.ask("POST", "/api/1/check", {"action": action["id"], "args": action["examples"]["good"][0]})
        if set(answer) != {"changed", "now"}:
            raise Failure(f"POST /api/1/check for {action['id']} answered {answer}")
        return answer["changed"]

    def run_task(self, action):
        """Runs the task of `action` and gives how many tasks it changed."""
        self.runs += 1
        folder = os.path.join(self.folder, f"{self.runs:03}-{action['id']}")
        os.mkdir(folder)
        name = os.path.join(folder, action["id"] + ".yml")
        with open(name, "w") as file:
            file.write(playbook(self.tasks[action["id"]]))
        command = ["sh", ANSIBLE_SH, self.machine, self.ssh_port, name]
        run = subprocess.run(command, input=self.password + "\n", capture_output=True, text=True, cwd=folder, timeout=900)
        if run.returncode == 77:
            print(run.stdout.strip())
            sys.exit(77)
        changed, unreachable, failed = (count(key, run.stdout) for key in ("changed", "unreachable", "failed"))
        if changed is None or unreachable is None or failed is None:
            raise Failure(f"ansible.sh for {action['id']} printed no recap (status {run.returncode}): {run.stdout[-900:]}{run.stderr[-600:]}")
        if run.returncode != 0 or failed or unreachable:
            raise Failure(f"{action['id']} by Ansible: changed {changed}, unreachable {unreachable}, failed {failed} (status {run.returncode}): {run.stdout[-1200:]}")
        return changed


def main():
    machine, ssh_port, console_port = sys.argv[1:4]
    without_lab_account()
    token = make_token(TOKEN_NAME, machine, ssh_port)
    try:
        console = Console(machine, console_port, token)
        actions = [action for action in console.ask("GET", "/api/1/actions") if action["id"] not in LEFT_ALONE]
        if not actions:
            raise Failure("GET /api/1/actions gave no actions")
        service = service_of_the_examples(actions)

        def state():
            return service_state(console, service) if service else None

        with tempfile.TemporaryDirectory(prefix="lab-ansible-") as folder:
            round_ = Round(console, actions, machine, ssh_port, folder)
            before = state()
            # What each task reported, for its first run and its second: (action, first, second).
            seen = []
            # What check mode said before a change's first run and after its second.
            checks = []
            for action in actions:
                if not action["changes"]:
                    seen.append((action, round_.run_task(action), round_.run_task(action)))
                    continue
                undo = action["examples"]["undo"]
                if undo is not None:
                    # From the state the undo leaves, so that the change has something to do.
                    route(console, round_.by_id(undo["action"]), undo["args"])
                checked_before = round_.would_change(action)
                first = round_.run_task(action)
                second = round_.run_task(action)
                checked_after = round_.would_change(action)
                checks.append((action, checked_before, checked_after))
                by_ansible = state()
                if undo is None:
                    # A restart: the service is as it was, as another process.
                    if by_ansible != before:
                        raise Failure(f"after {action['id']} the service is {by_ansible}, and it was {before}")
                else:
                    # What the route leaves from that same state.
                    route(console, round_.by_id(undo["action"]), undo["args"])
                    route(console, action, action["examples"]["good"][0])
                    by_route = state()
                    if by_ansible != by_route:
                        raise Failure(
                            f"after {action['id']} Ansible leaves the service {by_ansible}, and the route leaves it {by_route}"
                        )
                put_back(console, actions, action, service, before)
                seen.append((action, first, second))
        print("pass 1: every task succeeded")
        print("pass 2: every task succeeded")

        reads = [(action, first, second) for action, first, second in seen if not action["changes"]]
        if not reads:
            raise Failure("the list has no read to run")
        for action, first, second in reads:
            if first or second:
                raise Failure(f"{action['id']} reported a change, and it only reads: {first}, then {second}")
        print("reads reported no change")

        changes = [(action, first, second) for action, first, second in seen if action["changes"]]
        modules = [each for each in changes if round_.is_module(each[0])]
        commands = [each for each in changes if not round_.is_module(each[0])]
        if not modules:
            raise Failure("no change is a task of the collection's module")
        for action, first, second in modules:
            if first != 1:
                raise Failure(f"{action['id']} did nothing the first time, from the state its undo leaves: changed {first}")
            wanted = 1 if action["id"] in ALWAYS_CHANGES else 0
            if second != wanted:
                raise Failure(f"{action['id']} reported changed {second} the second time, and it is {wanted}")
        left_out = sorted(action["id"] for action, _, _ in modules if action["id"] in ALWAYS_CHANGES)
        said = f" ({', '.join(left_out)} always does something, and is the exception)" if left_out else ""
        print(f"changes with a module reported no change the second time{said}")

        for action, first, second in commands:
            if (first, second) != (1, 1):
                raise Failure(f"{action['id']} is a command, and reported changed {first}, then {second}; it is 1 and 1")
        print("commands reported changed both times")
        print("state after each change is what its route leaves")

        # A module says itself whether it changed anything, so check mode must say the same; a
        # command always reports a change, and check mode, which reads the state, must not say a
        # change that is already made would make one again (it may say it cannot tell); a change
        # that compares nothing is said to change something each time.
        reported = {action["id"]: (first, second) for action, first, second in changes}
        for action, before_run, after_run in checks:
            first, second = reported[action["id"]]
            undo = action["examples"]["undo"]
            # console.reach is put back by itself: its example asks for the reach the machine
            # has, so the change has nothing to do even before it runs.
            undoes_itself = undo is not None and (undo["action"], undo["args"]) == (action["id"], action["examples"]["good"][0])
            # A change with no undo starts from what the machine has: console.export's folder may
            # be a repository from a run before, and check mode cannot tell whether the settings
            # moved since, so it may say so.
            unknown_start = undo is None and action["id"] not in ALWAYS_CHANGES
            if unknown_start:
                wrong = before_run is False
            else:
                wrong = before_run is not (False if undoes_itself else True)
            if wrong:
                raise Failure(f"check mode said {action['id']} would change {before_run} before it ran, and Ansible changed {first}")
            if action["id"] in ALWAYS_CHANGES:
                wrong = after_run is not True
            elif round_.is_module(action):
                wrong = after_run is not bool(second)
            else:
                wrong = after_run is True
            if wrong:
                raise Failure(f"check mode said {action['id']} would change {after_run} after it ran twice, and Ansible's second run changed {second}")
        print("check mode agreed with Ansible on each change")
    finally:
        remove_token(TOKEN_NAME, machine, ssh_port)


if __name__ == "__main__":
    try:
        main()
    except Failure as failure:
        print(f"FAILED: {failure}")
        sys.exit(1)

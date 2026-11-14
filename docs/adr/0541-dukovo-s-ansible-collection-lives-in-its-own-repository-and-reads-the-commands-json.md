<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0541. Dukovo's Ansible collection lives in its own repository and reads the commands' JSON

- Status: Accepted
- Date: 2026-11-14

## Context

A server is rarely set up by hand twice. Most administrators who run more
than a few machines describe them in Ansible, which reaches a machine
over SSH, copies a module there and runs it with the machine's Python.
Dukovo has an SSH server, a Python bundle and `sudo` (ADR 0420), so
Ansible's own modules that run a command already work. What they cannot
do is say whether a run changed anything: `bundle install` and `feature
add` print sentences, and a playbook that parses sentences breaks when a
sentence changes.

Ansible's modules are GPL-3.0 by custom and by the license of the
`module_utils` they import. Dukovo is Apache-2.0 or MIT.

## Decision

1. **The commands an administrator scripts print JSON with `--json`:**
   `bundle list` and `bundle info`, `feature list` and `feature info`,
   `snap list`, `sysupdate status`, and `update`, `update check` and
   `update history`. Each prints one object on one line whose first
   member is `"format": 1`; names are snake_case, times RFC 3339 in UTC,
   sizes bytes, and a value the system does not know is `null`. A later
   member may be added without a new format, and a script ignores what
   it does not know. Each command's man page lists the members under
   JSON, and a golden file in `systools/tests/json/` holds them. An error
   prints as without `--json`, to standard error with the exit status.
2. **The collection `dukovo.system` lives in its own repository,
   `dukovo-os/ansible`, GPL-3.0-or-later,** and is published to Ansible
   Galaxy from a tag. It is never in the image. Its modules `bundle`,
   `feature`, `service`, `update`, `snapshot` and `facts` run Dukovo's
   own commands, read their JSON and never their text, are idempotent,
   take check and diff modes, and report Dukovo's error ID.
3. **Every release carries os-release(5)** at `/usr/lib/os-release`, with
   `ID=dukovo` and the version that runs, and the default settings link
   `/etc/os-release` to it, so that it changes with each update. Ansible
   names a system that is not Linux from `uname`, which already says
   Dukovo; scripts that read os-release are the reason for the file.
4. **How Ansible reaches the machine.** Python is found by its path,
   `/applications/org.python.python/bin/python3`, since `python3` by name
   asks at the console which Python to run. `become` goes through
   `sudo`, which asks for the account's password; there is no rule that
   lets an account pass without one, and a wrong password counts towards
   authd's block of the sign-in port (ADR 0479). Facts are gathered with
   `gather_subset: [min]`, since `/proc` and `/sys` are not Linux's.
5. **Tested from both sides.** The collection's repository runs
   `ansible-test sanity` and unit tests of each module against recorded
   JSON. Dukovo's boot test `ansible` runs a pinned ansible-core with the
   collection pinned by `tools/xtask/src/test/ansible.lock` against an
   installed system over SSH: Python installed with `raw`, then each
   module, then the playbook again reporting no change, then `--check
   --diff` against a changed machine changing nothing. The lab test
   `system/ansible` runs the same from the lab PC and an update applied
   with a restart.

## Alternatives

- **The collection in this repository.** Its GPL-3.0 code would sit in an
  Apache-2.0 or MIT tree, and its releases would follow Dukovo's rather
  than Ansible's.
- **Modules that parse the commands' text.** Every change of wording
  would break a playbook.
- **A JSON API over the network instead of commands.** admind's API serves
  the web console; Ansible's model is a module run on the
  machine, and the commands are what an administrator also runs by hand.

## Consequences

- While `dukovo-os/ansible` does not exist, the lock names the
  collection's tarball by the environment variable
  `DUKOVO_ANSIBLE_COLLECTION`, and the boot test says it is skipped where
  that is not set, as on CI. Once the repository has a release, the lock
  names its URL, commit and SHA-256, and CI runs the test.
- A change to a document's members is a change to its golden file, which
  the review sees.

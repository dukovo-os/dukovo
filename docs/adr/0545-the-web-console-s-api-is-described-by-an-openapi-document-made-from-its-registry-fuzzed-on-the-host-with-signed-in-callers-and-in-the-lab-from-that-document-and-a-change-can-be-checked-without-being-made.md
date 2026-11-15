<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0545. The web console's API is described by an OpenAPI document made from its registry, fuzzed on the host with signed-in callers and in the lab from that document, and a change can be checked without being made

- Status: Accepted
- Date: 2026-11-15

## Context

The console's API (ADR 0514, ADR 0515) is used by the pages, by scripts
with a token and by Ansible. Its routes are a table in admind and its
actions a registry, each with the schema of its arguments, its error IDs
and its examples, but nothing says so in a form a tool can read, so a
script's author reads the man page and a security tester writes requests
by hand.

Its tests send what their authors thought of. ZAP and testssl.sh scan it
from outside (ADR 0514); the host tests of each parser, and a loop of a few
thousand random requests that no one is signed in for (ADR 0515), cover the
code that reads bytes. What no test sends is a request from a person who
is signed in, bent in the ways a client bends one: a cookie that is
another person's, a header twice, a body of the wrong type. That is where a
check that comes after the sign-in could be left out unseen.

The shown Ansible task of a change is `ansible.builtin.command`, which
cannot say whether it changed anything (ADR 0515), so a playbook that runs
it reports "changed" on every run. The collection `dukovo.system` (ADR 0541)
has a module for services that does, and a playbook run with `--check` has
no way to learn from the console whether a change would do anything.

## Decision

1. **The API describes itself.** `GET /api/1/openapi.json`, open to anyone
   since it says only what the man page and the page's own code say,
   answers an OpenAPI 3.1 document that admind renders from its table of
   routes and its registry at each request, so that a route or an action
   that is added is in it and it cannot differ from what answers. Each
   route and each action is one operation, with its path parameters, the
   security it needs (the session's cookie with the `x-dukovo-csrf` header
   for a change, or an API token as a bearer, on the operations tagged
   `api`, the only ones a token may ask), and one response for each
   status it can answer, which lists the error IDs that come with that
   status: those of the checks that come before every route, those of the
   route's handler and, for an action, those of its command. An action's
   arguments are its path parameters and the members of its request body,
   with the checks of the action's own JSON Schema, and the first good
   example of the action is the example of both, so that a tester can send
   it as it is; the route that shows an action takes `{action, args}` as
   its body. Where the table does not know the shape of an answer, the
   document leaves it out and claims none. Unit tests check that the
   document parses, that every route of the table and every action is in
   it, that an action's good examples fit what it says and its bad ones
   that a schema can refuse do not, that each status lists only IDs that
   have that status, and that every error the console has is the answer
   of some operation.
2. **The routes are fuzzed on the host with callers.** A host test beside
   the routes' tests (`admind/routes/tests/fuzz.rs`) sends requests from
   each kind of caller, with the routes' own script of authd and worker:
   nobody, a cookie of a session that is no more, an administrator locked
   and unlocked, a person who is no administrator, one who must add a
   second step, a token of each scope and a token no one was given. A
   round picks a caller and a request to start from: a route of the table
   (every one but the event stream, which does not end, and a test fails
   when a route has none), or an action's good, bad or undo example on its
   own route, as a showing and as a check. It bends it up to three times:
   a header removed, doubled, emptied, 64 KiB long, not UTF-8 or another
   caller's; *Host*, *Origin* or *Sec-Fetch-Site* wrong; the body's bytes
   bent; its JSON with a member removed, doubled, of another type, at its
   limits, with `..`, NUL or 4 KiB in it, or clashing with the address; a
   part of the address made `..`, NUL, very long or not UTF-8.
   `ADMIND_FUZZ=N` sets the requests, 4,000 by default, which take some
   seconds, and `ADMIND_FUZZ_SEED=S` the seed, and a failure names the
   round, the caller and the request. What must hold of every answer:
   nothing panics or hangs; an error is JSON with a known ID, a title and
   a detail, and an answer of the API that works is JSON; a refused
   request, an error other than ADM-123 (the command ran and failed), ran
   nothing as an administrator, wrote no row of the journal, made no
   change of the person's things through their worker and started no
   shell, unless authd itself refused the shell (ADM-132); a read ran
   nothing as an administrator and wrote no row; a check ran nothing but
   the action's probe; a request counts at most one sign-in try for its
   address, exactly one when authd was asked about a sign-in and none on
   another route, and as many wrong-token tries as tokens authd did not
   know; and an answer to a stranger, who brings no live session or known
   token, names no account and holds no session's or token's secret.
3. **The lab fuzzes from the document.** The lab test `web-console/fuzz`
   fetches the document from a machine and sends Schemathesis's requests
   made from it, and from nothing else, three times: signed out, with a
   token that only reads, and with a token of every scope on the examples
   of the changes alone, each followed by its undo, since a change with
   made-up arguments would change the machine as it pleased. Every answer
   must be no 5xx, with a status, a content type and headers the document
   lists and a body that fits its schema, and within 2 seconds (10 for a
   change, which waits for Spark). Afterwards admind must answer and have
   the same process and start in `service list --json`, the journal must
   hold the changes the test made as its token and nothing else, and the
   service the examples use must be as it was found. A lab machine's
   `lab.missing`, which is answered 501 by design, is not read.
   Schemathesis 4.33.4
   (MIT) is pinned by its release and the hash of every package it needs,
   in a requirements file that `pip install --require-hashes` checks, in a
   virtual environment of the lab PC's cache; a lab PC that cannot make
   one skips the test, and a package whose hash is not listed fails it.
4. **A change can be checked without being made.** `POST /api/1/check`
   takes what `POST /api/1/show` takes, `{action, args}`, for the same
   callers, a session signed in, locked or not, or a token whose scopes
   read the action's area, and runs nothing that changes: the action's
   probe as the person, or the token's owner, and a read of the files the
   change names by the journal's rules, so a file that not everyone may
   read tells nothing. It answers
   `{"changed": true | false | null, "now": text | null}`. A read is
   `false`; a change with no probe and no files is `true`, since it always
   does something, as a restart does; otherwise each part it compares must
   be settled for `false`, and the answer is `null` where a part cannot
   tell. An action says whether what its probe picks is already what the
   change leaves (`Action::settled`) and whether the change would leave a
   file as it is (`Action::keeps_file`): the services' changes from Spark's
   word for the service where it is sure, enable and disable from the
   service file as `servicefiles::with_enabled` writes it, and the
   console's reach from what `admin reach` prints and from admin.toml.
   `now` is the part the probe picks. The log has no line for it and the
   journal no row.
5. **A service's change is a task of the collection's module.** Show's
   Ansible task of `service.start`, `stop` and `restart` is
   `dukovo.system.service` with `state: started`, `stopped` or `restarted`;
   `enable` gives `state: started` and `enabled: true`, and `disable`
   `state: stopped` and `enabled: false`; each has `become: true` and no
   `changed_when`, since the module says whether it changed something. Every
   other action keeps `ansible.builtin.command`: `certificate.renew` and
   `console.reach` wait for modules of the collection. An action gives its
   task as an `Ansible` value, and the services' changes build theirs with
   `Ansible::module`. The lab test `web-console/ansible` runs the tasks with
   the ansible-core and the collection that `lab/tests/system/ansible.sh`
   pins and installs, each twice in a row, and says that a change with a
   module reported no change the second time (a restart always does
   something), that a command reported changed both times, that each change
   leaves its service as its route does, and that check mode said each
   change would change something before its first run and, after its
   second, what the module reported.

## Alternatives

- **An OpenAPI file in the repository.** It would drift from the routes,
  and the registry already holds the schemas.
- **A crate that derives the document from the code.** It would add a
  dependency to a program that runs as root for what a few functions do
  from values the registry already has.
- **Fuzzing from the lab alone.** A request from a person who is signed in
  needs the sign-in, an unlock and a worker; the host test has them as the
  routes' tests do and runs in seconds, so a bent request that makes the
  code panic is found before the change that causes it is merged.
- **Schemathesis from a container image, as ZAP.** It would need Docker
  on the lab PC for a Python program, which runs from a virtual environment
  with every file pinned by hash and no container to pull.
- **Ansible's own `--check` alone.** The console cannot run a playbook;
  its answer to what a change would do is a route.

## Consequences

- A new action is described, fuzzed and, if it changes anything, checked
  without a line besides its own entry: the document, the host test and
  the lab test take it from the registry. A new route of the table needs a
  summary and its errors in the document, which does not build without
  them, and a request for the host test to start from, which fails until
  it has one.
- The document says which statuses and IDs an operation can answer with,
  so an answer with a status or an ID that the operation does not list
  fails the lab test; an ID that is listed and that nothing gives is found
  by a person who reads the document, not by a test.
- The document is as long as the table and the registry make it, about
  190 KB on a release image, most of it the lists of errors; it is served
  with `no-store` like the other answers.
- `web-console/fuzz` sends requests at a machine for some minutes. A
  signed-out run uses up the ten sign-in tries a minute that its address
  has, so the test signs in before it and again after it, waiting out the
  limit.
- The lab test of Ansible needs ansible-core 2.22, so Python 3.13 or later
  on the lab PC, and the collection's release, as `system/ansible` does.
- `docs/deferred.md` keeps a row for the actions that have no module in
  the collection yet.

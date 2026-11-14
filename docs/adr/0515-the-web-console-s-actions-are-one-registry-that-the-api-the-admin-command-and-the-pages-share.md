<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0515. The web console's actions are one registry that the API, the admin command and the pages share

- Status: Accepted
- Date: 2026-11-14

## Context

ADR 0514 sets out who is at the browser: the sign-in, the second steps, the
unlock, a worker that runs as the person, and `console-elevate`, which
runs a command as an administrator for an unlocked session. It also keeps
API tokens, which this record puts to use. This record is what the console
does once someone, or a script, is let in: its actions.

A button in a page, a call to the API and a command typed at a terminal
can all do the same thing to a machine. Written apart, they drift: the
page says it restarted a service, and the command it ran did something
else. A person who wants to learn what the page did, or to do it on ten
machines, then has nowhere to look.

Dukovo's commands (`service`, `cert` and the rest) are already the
machine's interface. They work at a terminal and over SSH with no console
running, and those that print JSON document it. authd already runs a
command as an administrator after a password (ADR 0219). What is missing
is one place that says what the console may do and which command each
thing runs, so that the page, the API, the `admin` command and a playbook
say the same.

Two more facts shape it. For a page to be of use to someone who is not an
administrator, their worker has to read what an administrator reads, and
Spark's control socket is root's, so `service list` failed for everyone
else. And a script has no cookie: a token is how it comes in, and a token
must be no more than its owner.

## Decision

- **One registry, in admind.** `admind::actions::REGISTRY` lists every
  action. An action declares its id (`service.restart`); its area
  (`services`), which is also the scope a token needs; whether it changes
  anything (a read is a `GET`, a change a `POST`); its path
  (`/api/1/services/{name}/restart`); who runs its command; the arguments
  it takes, with their checks; the command; its Ansible task; the files a
  change writes; for a change, the probe that tells whether it moved
  anything; the error IDs its command can give; its examples, good and
  bad, and for a change the action that puts it back; and its five
  tests. The routes of the actions, `GET /api/1/actions`,
  `admin actions` and the golden file `lab/actions.toml` are made from
  this list, so none of them can say what another does not. The actions
  are the services' (list, explain, start, stop, restart, enable,
  disable) and the certificates' (list, show, renew); the console's pages
  add their areas' as they come.
- **An action's command is a list of words, and it is what runs.** It is
  never text for a shell. The program is found only in `/system/commands`
  and `/applications/commands`, as `console-elevate` finds it. A read's
  command has `--json`, and the API answers the JSON as the command
  printed it, checked to parse: a command's JSON is its documented
  interface, and the API does not reshape it, so the two cannot drift. A
  change answers `{"done": true, "changed": true, "output": [...]}`, with
  at most the first 200 lines of the command's output, and `changed` as
  the next paragraph says. A command that ends with a
  status other than 0 is ADM-123 (422): the detail is the last line of its
  error output, and the answer's `command` holds its words, its status and
  its error output. When that line ends with an ID in parentheses, as
  `(SVC-104)`, and the action lists the ID, the answer carries that ID in
  place of ADM-123.
- **Arguments are checked before anyone is asked.** The address's
  `{name}` parts and the JSON body fill one set of arguments, and a name
  in both, or a field the action does not have, is ADM-108. A value that
  cannot work is ADM-125 with the `field`, in words that say what was not
  done. A name is checked by the rule of the program that takes it:
  Spark's for a service, certd's for a certificate. Stopping, restarting
  or disabling admind or authd is refused, since the console is made of
  them and the answer would never arrive.
- **The list says what each action takes, as a JSON Schema.** An action's
  `input` in `GET /api/1/actions` is a schema (draft 2020-12, the part with
  `type`, `description`, `minLength`, `maxLength`, `pattern`, `enum`,
  `minimum`, `maximum`, `required` and `additionalProperties: false`) of
  its arguments, so that a client can check what it is about to send and a
  page can make its form without asking the console. The action's fields,
  one table of name, kind, limits and words, make both the schema and the
  checks, and say what the console enforces, no more and no less. A host
  test holds each action's schema to its checks with a validator of that
  part of JSON Schema, on the action's examples and on values at and just
  outside each limit, and fails for a schema that says what the checks do
  not.
- **Every action runs as the person, or as an administrator through
  `console-elevate`.** An action that runs as the person runs in the
  person's worker (ADR 0514) with the person's own rights: no terminal,
  input that ends at once, the worker's environment, 30 seconds, at most 1
  MiB of output and 64 KiB of error output, and 8 at a time for a worker.
  An action that runs as an administrator goes through authd's
  `console-elevate`, as 0514 says: for a session, an unlocked one of an
  administrator; for a token, one whose owner is an administrator and whose
  scopes allow it. At most 32 run at once, all sessions and tokens
  together, and another is answered ADM-105. A command that has not ended
  in 30 seconds is ended (ADM-121). A session of a person who is not an
  administrator asking for an administrator's change is ADM-122, and so is
  a token whose owner is not one, which authd refuses; a locked session is
  ADM-106. Every read of the first actions runs as the person, and every
  change as an administrator.
- **A change says whether it changed anything.** The answer's `changed` is
  true or false, so that a script that asks for what is so already learns
  that nothing was done, as an Ansible task's report says it. A change
  whose effect a read can see has a *probe*, that read, as a command in the
  registry's own form: `service list --json` for a service's start, stop,
  enable and disable, and `admin reach` for the console's reach. The probe
  is run before the change and again after it, and `changed` is whether the
  part of the two outputs that the action picks differs. Start and stop
  pick the state Spark says of the service, one of its nine words; enable
  and disable pick the state and whether it starts at boot; reach picks
  its word. A pick leaves out what moves by itself, such as a process
  number or a time. A probe runs in the worker of the person who asked, as
  the person, for a session and for a token alike (the token owner's
  worker), and never as an administrator, so it can read only what that
  person can; a change that only an administrator can see the effect of
  has no probe. When the console cannot tell, `changed` is true, as Ansible's
  command module says it: a probe that fails, an output the action finds
  nothing in, and a change without a probe all give it. A restart always does
  something, and so does `cert renew`, which makes the certificate anew
  whether or not it is due, so neither has one. The list says which is
  which: an action's `compares` is true when its `changed` is a
  comparison, and false when it is always true, as for every read, which
  has none. A command that fails is ADM-123 and has no `changed`.
- **Show the command.** `POST /api/1/show` with `{"action", "args"}`
  checks the arguments as the action's own route does and runs nothing.
  It answers four forms of the one prepared action. The Command is the
  argv as one line for Dukovo's shell and for sh: a word of
  `A-Za-z0-9@%+=:,./_-` as it is, any other in single quotes, with
  `elevate ` in front of what runs as an administrator, so that the line
  works as it is typed. The API is a `curl` line for the route, with
  `Authorization: Bearer $DUKOVO_TOKEN` and the JSON body, against the
  address the browser used. The Ansible form is one task, and Files is
  what a change writes. Any signed-in session may ask, locked or not, and
  so may a token that may read the area. The shown command is the command
  that runs: both are the one argv, and a boot test types the shown command
  in a pane and compares the state it leaves, and the JSON it prints, with
  the route's.
- **API tokens come in as `Authorization: Bearer`, and only there.** Not
  in an address, which logs and histories keep, and not in a cookie, which
  a browser sends by itself. A request brings a token or a session's
  cookie, never both, and one header only; a bearer that is not shaped
  like a token, or a second one, is ADM-108. Another scheme, such as the
  Basic of a proxy in front of the console, is let be. A token is taken
  for the actions, the list `GET /api/1/actions` and `POST /api/1/show`,
  and nothing else: any other route answers ADM-124, so a token cannot
  sign in, unlock, or make another token. A token needs no `Origin` or
  CSRF token, since a browser never sends one on its own, and `reach`
  still decides whose connection is answered.
- **authd checks the token on every request.** admind passes the token on
  and forgets it. A token that is removed, that ends, or whose owner may no
  longer sign in stops at once; authd answers with the owner, the token's
  name and its scopes, or refuses, and admind answers ADM-115 (401).
  authd slows the guessing of tokens as it slows that of passwords, and
  admind stops an address that gave 10 wrong tokens in a minute (an IPv6
  address counts by its /64) with ADM-105 and `Retry-After`, before authd
  hears of the 11th, since the console is open to any address `reach`
  answers.
- **Scopes are by area.** A scope is `*`, which stands for all that the
  owner may do; an area's name, such as `services`, to read and change it;
  or the name and `:read`, to read it alone. A read needs the area, its
  `:read` or `*`; a change needs the area or `*`; else ADM-124, naming
  the scopes that would do. A change that runs as an administrator also
  needs an owner who is an administrator. admind checks the scopes before
  the action runs, and sends the action's area with the token to authd,
  which checks them again, so that a mistake in admind cannot run a
  command the token was not given. `:read` is never enough to change.
- **A token's worker is kept 5 minutes.** An action that runs as the
  person for a token runs in a worker of the owner, which authd starts
  the first time and admind keeps for 5 minutes after the token's last
  call, one for each token. At most 32 tokens have one at once; another
  is answered ADM-120 until one goes idle. An action that runs as an
  administrator needs none. The worker is linked as a session's is, by
  two pipes.
- **The errors.** ADM-122 the account may look but not change (403);
  ADM-123 the command did not work (422); ADM-124 the token's scopes do
  not cover this, or it was sent to a route that is not an action (403);
  ADM-125 a value cannot work (400). ADM-115 is that the token is not
  one: unknown, ended, or of an account that may not sign in. A locked
  session is ADM-106 as before. Each has a page in `docs/errors`.
- **The log.** admind writes one line for each change asked for, whether
  it ran, was refused or failed: who asked (the person, or the person and
  the token's name), from which address, the action, and how it ended,
  with "changed nothing" for a change that ended well and moved nothing.
  For a read it writes a line only when the read was refused or failed.
  Arguments that do not read are refused before anyone is asked and have
  none, and a read that worked has none, since a page asks for them
  all the time. A line names the action and not its arguments, and never
  holds a token. authd audits each command it runs as an administrator, as
  0514 says, and for a token the line names the area.
- **The `admin` command is the console from a terminal.** `admin
  fingerprint` prints the address and the certificate's fingerprint that
  admind keeps in `/system/live/admind/issue`. `admin actions [--json]`
  prints the list that `GET /api/1/actions` answers. `admin show ACTION
  [NAME=VALUE | NAME:=JSON]...` prints what `POST /api/1/show` answers.
  The last two need no rights and no running console: they are made by the
  same code as the routes, in the same program, so what they print is what
  the API answers. `admin token add NAME --scope SCOPE... [--days N]`,
  `admin token list` and `admin token remove NAME` are the person's own,
  asked of authd as the person, as `passwd` is. `add` asks for the
  password, unseen at a terminal and from the first line of the standard
  input where there is none, and prints the token once. A token lasts 90
  days unless `--days` says, from 1 to 366. There is no `admin run`: the
  command an action runs is the command shown, and a person runs it as it
  is, with `elevate` where it says so. A second way to run actions would
  be a second interface to keep the same.
- **Every action has five tests, and a gate counts them.** The action
  names them, and `lab/actions.toml`, which the registry writes (its host
  test `actions_toml`; `UPDATE_ACTIONS=1 ./ob test unit` writes it
  again), lists them. `./ob lab coverage --check` fails for each test that
  is not in the tree. They are: its arguments' host tests, with each good
  and bad value and the command of each good one; the `admin-actions`
  boot test's call of its route as a reader, as a locked and an unlocked
  administrator, and with a token of the wrong scope and of the right
  one; the same boot test's run of the shown command in a pane, which
  must leave the same state and print the same JSON as the route; the lab
  test `web-console/actions`, in which each browser engine calls the route
  with the session's cookie and CSRF token under the console's content
  security policy; and the lab test `web-console/ansible`, which runs the
  shown task against the machine twice. The boot test and the two lab
  tests take the list from `GET /api/1/actions` and the arguments from
  each action's examples, so that a new action with examples is tried as
  soon as it is registered. The boot test also runs the good example of
  each change whose `compares` is true a second time, from the state the
  first left, and expects `changed: false` and "changed nothing" in the
  log. The lab also scans the console with ZAP's
  baseline (`web-console/zap`) and with testssl.sh (`web-console/testssl`),
  each failing on a finding that its list of accepted ones does not name
  with a reason.
- **What a client sends can be fuzzed.** systools is a library and a
  binary: `src/lib.rs` has the modules and the dispatch, and `src/main.rs`
  calls it, so that a program on the host can link the same code. Its
  `fuzz` feature, which a build for people leaves out, adds a module
  `fuzz` with a `Console` and a function for each parser of what a client
  sends. A `Console` is admind's request handling on a console made from
  the tests' fixture, with an authd that refuses every request, no socket
  and no TLS; `request` takes the bytes of a method, an address, headers
  and a body and gives the status and the body, and one console answers
  millions of them. The functions read an action's address and its `%XX`
  escapes, an address's arguments with a body, each action's arguments,
  the bearer header, the cookie, a token's scopes, the preferences body
  and the settings text it is made to, and a passkey's client data,
  authenticator data, key, attestation and assertion, each from bytes or
  text and giving a simple answer, so that admind's own types stay
  private. A fuzz target is a crate of its own that calls them. The
  module has host tests of its own, with each function on an input that is
  right and one that is not, and a loop of a few thousand inputs from a
  generator with a fixed seed through all of them, which must not panic.
  No image has the module.
- **Ansible tasks are `ansible.builtin.command` for now.** The task has
  `argv:`, never a string for a shell, `become: true` for what runs as an
  administrator, and `changed_when: false` for a read. That module cannot
  tell a change from none, so a change task reports "changed" on every run.
  The `dukovo.system` collection, whose modules report it, is a row in
  `docs/deferred.md`; an area that gets one changes its actions'
  `ansible()` alone.
- **Spark answers status on a read socket.** `/system/live/spark/read` is
  mode 0666 and answers `status` and `show` to anyone, as `systemctl
  status` answers any user on Linux. Everything that changes anything goes
  to the control socket, which only root may write to. `service`'s `list`,
  `status` and `explain` read through it, so the console's reads run as the
  person signed in, as themselves. What a service's own output said, its
  last line and the last a failed health check wrote, is root's, as its
  records in the log are, and is left out of what anyone else reads. The
  socket takes four connections at a time, apart from the control
  socket's, so that a user who keeps them busy keeps no one out of it.

## Consequences

- A new action is one value in the registry, with its examples and its
  five tests; the gate fails until the tests are in the tree. A page that
  adds a button adds an action, and gets the API route, the list entry,
  the shown command and the Ansible task with it.
- A change with a probe costs two reads, run as the person before and
  after it, besides the command. Its `changed` is as good as what the
  probe picks: when the probe cannot say, `changed` is true, and the list
  says which actions can say at all.
- A command's JSON is an interface that the API depends on. A change to
  what `service list --json` prints changes the API's answer, which the
  boot test that compares the route with the command shows.
- A token that names an area, whose owner is an administrator, changes
  that area without an unlock. The unlock is a person at a page proving
  again who they are; a token is a choice its owner made once, with their
  password, for a set of scopes and an end at most 366 days away, and
  `admin token remove` ends it at once. A token is made for one script
  with the scopes it needs, and a script that only reads is given
  `:read`.
- Every request with a token is a call to authd, and a read that runs as
  the person starts a worker of its owner. A script that makes many calls
  pays the call to authd for each, and the worker's start once.
- Nothing runs longer than 30 seconds. The list marks an action that may
  take longer (`long`), and none does yet; a command that does not end in
  that time is hung up.
- An address that is not on `reach` still gets nothing, with a token or
  without. An address on it that guesses tokens is stopped after 10 in a
  minute, and so is every other client behind the same address.
- `admin show` needs no running console, and says `https://HOST:PORT` in
  place of the address when none has been written. Its API line is for a
  token's script; a certificate the machine made itself is not one that
  `curl` knows, and the line leaves that to the person (`cert show
  --pem`, after checking the fingerprint).
- Anyone who can run a program on the machine can read what `service
  list` and `service NAME status` show, as on Linux, but not what a
  service's own output said.
- Change tasks in a playbook report "changed" each run until the
  `dukovo.system` collection exists.
- The lab's tests of the actions run on QEMU and the lab PC, not in CI,
  with the other lab tests of the console (ADR 0510).

## Alternatives considered

- **A token in the address, or in a cookie.** An address is kept in
  logs, histories and proxies, and a cookie is sent by the browser
  without being asked, which brings back the requests made on someone
  else's behalf that 0514 stops. A header that a script sets on purpose
  has neither fault, and the console takes no other.
- **A separate API from the pages'.** Two lists of what the console may do
  would have to be kept equal, a page could do what a script cannot or the
  reverse, and the command the page shows would be a command for a route
  other than the one that ran. The pages call the same routes that scripts
  do.
- **Signed tokens (JWTs).** As for sessions in 0514: a signed token cannot
  be taken back before it ends without a list on the server, which is
  what authd keeps. Asking authd on each request makes a removed token, or
  an account that may no longer sign in, stop at once, for the price of
  one call.
- **Generating the commands from the API, rather than the API from the
  commands.** The commands exist, work at a terminal and over SSH without
  the console, and have their own tests and JSON. Commands made from an
  API would be a second thing to keep right, and a machine could be
  administered only while the console ran. The commands come first, and
  the API runs them.
- **A `changed` that each command prints.** Every command would need a way
  to say it, and several would have to learn it, for a fact that the state
  they change already says. A read before and after, with the command that
  already prints the state, changes none of them.
- **An `admin run` that sends an action's call.** It would be a second way
  to run an action beside the shown command, with its own rules to keep
  the same. A person runs the command as shown.
- **Scope `*` alone for a change as an administrator.** It would give a
  script that only restarts services the power to do everything its owner
  may. A scope that names the area limits the token to that area, and
  authd checks it again.

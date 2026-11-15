<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0561. A secret the web console is given goes to its command's input, and is never kept

- Status: Accepted
- Date: 2026-11-14

## Context

The console's actions are commands (ADR 0515): each is a list of words that
runs, as the person or as an administrator through authd, and the same
words are what the console shows, what the journal keeps (ADR 0516), what a
task keeps, and what authd's audit line says. Some of the work the console
takes on from C5 needs a secret: a new password for an account, a person's
own password and a new one, the recovery key that opens a backup, the
password of a mail account for alerts. Some commands print one: the
recovery key of a new backup place, or a device's private settings for
private access.

A secret cannot be one of a command's words. The words are kept in the
journal, shown in the Show command, written in the task's row and in the
audit, and a command's arguments can be read by anyone who lists the
machine's processes. A command that takes a secret reads it from its
standard input when that is not a terminal, as a script would give it:
`passwd` and `user add` read each password as a line, and the commands
that open a backup with its recovery key read the key the same way when
it is given as `--key -`. Until now the console gave every command an
input that ends at once, so such a command read nothing and failed.

A read is a `GET` (ADR 0515), whose arguments are in its address. An
address is kept in a browser's history, in the log of any proxy on the
way, and is shown in the browser's own address bar. A read that takes a
secret, such as the plan of a move that opens a locked backup, cannot be a
`GET`. Nor can a read whose answer is a secret: a browser may keep a
`GET`'s answer, prefetch it or repeat it.

## Decision

**A secret field.** An action's argument can be a secret
(`Field::secret`): one line of 1 to a set number of characters, with no
NUL. Its schema in `GET /api/1/actions` is text with that length and a
pattern of one line, marked `writeOnly`, the JSON Schema word for a value
that is sent and never given back. admind checks it before anything else
reads it, and its refusal never quotes it.

admind reads it into a `Secret`, which prints and serializes as
`(secret)` wherever it would be shown or kept, and whose bytes are
overwritten with zeros when it goes. A `Secret` has no way to give its
text but its bytes, which only the action's `input` reads: the secrets it
takes, each a line, in the order of its fields. The command's words
cannot hold one, since nothing that makes them can read it.

**To the command's input, and then its end.** admind writes the input to
a new pipe before the command starts, and closes the pipe: the command
reads the lines and then the end, as from a script. An administrator's
command gets the pipe from authd as its input, as before (`console-elevate`
takes four descriptors, the first the input). A person's command gets it
from their worker, to which admind sends the input with the command. The
input of an action is at most 4 KiB, which a pipe holds before its reader
reads any, so the write never waits; the registry's rules hold each
action's secret fields to that.

**Kept nowhere.** The journal, the task's row and authd's audit have the
command's words, which hold no secret. A task keeps the action's
arguments as they serialize, with `(secret)` for each secret, and drops
the secret once its command has it. A refusal says which field and why,
never what was given. The copies that the HTTP server's buffers, the JSON
reader and the link to a person's worker made are freed as they are: only
admind's and the worker's own memory held them.

**A `POST`, a read's too.** An action whose route takes a secret, or whose
command prints one (`SECRET_ANSWER`), is a `POST`, its arguments in the
body. The rule of the registry is: a change is a `POST`; a read is a `GET`
unless it takes or answers a secret. `GET /api/1/actions` says each
action's method, whether it changes anything (`changes`), and whether its
answer is a secret (`secret_answer`). An action whose answer is a secret
is never a task, since a task keeps what its command printed. Every answer
of the API has `Cache-Control: no-store` already.

**Shown, not needed.** The Show command (`POST /api/1/show`, `admin show`)
needs no secret: each secret field given, or required, is shown as
`(secret)`, and one that is sent is dropped. A page gives an optional
secret that will be typed as empty text. The command is shown with what is
typed when it asks ("then the password, typed when asked"), its `curl`
line takes the secret from the shell's variable of the field's name in
capitals (`$PASSWORD`, inside a body in double quotes), and its Ansible
task from the playbook's variable (`stdin: "{{ password }}"`), with
`no_log: true`. The task of an action whose answer is a secret has
`no_log: true` and keeps the answer for the playbook (`register:
answer`).

## Consequences

- An account's password, a person's own, a backup's recovery key and a
  mail password can be given in the console, with the same command a
  person runs at a terminal, and the journal and the audit say what was
  done without saying the secret.
- A read that takes or answers a secret is a `POST`. A script that calls
  it sends a body, as for a change; `GET /api/1/actions` tells it which.
- The `curl` line's variable goes into a JSON string as it is, so a
  secret with a `"` or a `\` in it needs the JSON escape (`\"`, `\\`) in
  the variable. The page and Ansible send any secret as it is.
- The Show command shows a form's command before its secrets are typed,
  and a page never sends a secret to be shown.
- The registry's unit tests feed each action's examples a marker for each
  secret, and hold that the command reads the markers and that nothing
  else made of the action holds a byte of them: its words, its arguments
  as a task keeps them, its Ansible task, and each form of the Show
  command.

## Alternatives considered

- **The secret as an argument of the command.** It would be in the
  journal, the audit, the task, the Show command and the process list.
- **An environment variable.** It is inherited by every program the
  command starts, is readable as the process's environment by the same
  user, and would need authd to pass it through `elevate`, which starts a
  command with a clean environment.
- **A file the command reads.** It would be written to a disk, which
  keeps it after it is removed, and needs a place that only the command
  can read.
- **A `GET` with the secret in a header.** It keeps the secret out of
  the address, but the read would still be a `GET`, which a browser may
  prefetch or repeat, and whose answer it may keep; and a page could no
  longer send every action's arguments the same way.
- **Wiping every copy, in the HTTP server and the link.** Those buffers
  are the libraries' and the link's own, and are freed when the request
  ends; the copies that admind owns, the `Secret` and the input, are the
  ones it keeps for the length of a command or a task, and those are
  wiped.

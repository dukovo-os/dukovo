<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0493. A command elevated in a pane is asked about in that pane

- Status: Accepted
- Date: 2026-11-11

## Context

A command of the catalog that runs with its user's access, such as
`nginx -t`, runs only once its user allowed runs-with-your-access, which
bundled asks about with a warning the first time, at the trusted prompt
of the console the asking process's terminal belongs to (ADRs 0224 and
0251). elevate gives the command it runs a pseudo-terminal of its own,
which it copies to and from, so that Ctrl+C and Ctrl+Z reach the command
and not the asker's shell (ADR 0219). No console shows that terminal, so
consoled answers that the question cannot be asked there, and an
administrator could not run `elevate nginx -t` (ADR 0491). The same held
for `elevate bundle grant`, which asks on a console too.

authd starts every elevated command itself and keeps, for each, the
person who elevated it and the pane they typed `elevate` in.

## Decision

- **The question goes to the pane elevate was typed in.** When bundled
  asks a question for root on a terminal that is the controlling
  terminal of a command authd elevated, and that command still runs as
  root, authd asks it on the console of the pane the command was
  elevated in, of the person who elevated it. The command and its
  children share that terminal, so a shell elevated with `elevate -s`,
  and the commands it runs, are asked about there too.
- **Its words say so.** The question begins "As root, through
  elevate:", so that the person knows whose access they are allowing:
  root's, not their own.
- **What they answer is root's.** bundled keeps the answer as root's, as
  it keeps every person's answers as their own, so "always" holds for
  every administrator's later `elevate nginx -t`.
- **Audited.** authd writes to the audit log that it asked the person on
  their pane for root's command on its terminal, with the question.
- A command elevated outside a pane, or in a pane no console shows, such
  as one over SSH, is answered as before: the question cannot be asked
  there, and the command says how to allow it with `bundle grant` at a
  console.

## Consequences

- An administrator checks nginx's settings with `elevate nginx -t`, and
  any catalog command that runs with its user's access runs through
  elevate after one question.
- Only authd's own record of the elevated commands moves a question, and
  only bundled, which runs as root, may ask authd one; a person's own
  commands are asked about on their own terminal as before.
- The boot test `nginx` runs `elevate nginx -t`, answers the question in
  Alice's pane with "always", and runs it again inside an elevated
  shell without being asked; authd's unit test finds the pane for an
  elevated command's terminal, and none for a session that is not an
  elevated command's, one with no pane, or one that has ended.

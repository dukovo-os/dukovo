<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0691. authd runs an MCP client's reads as an administrator through one program, and takes no command for it

- Status: Accepted
- Date: 2026-11-24

## Context

Some of the console's reads run as an administrator: the whole log, what
the firewall blocked, the disks' own tests, every account's sign-ins
(ADR 0518). A person's account does not see them until the console is
unlocked.

admind runs nothing as an administrator itself. It is confined, and asks
authd (ADR 0514). For an unlocked session, and for an API token whose
scopes name the area, admind sends authd the command to run, and authd
runs it. authd checks who asks and that the command is one of the
system's and no shell. It does not check that the command is the one the
action names, since it has no list of actions. That is a fair trade for a
session and a token: a person unlocked the session, or made the token, in
order to change things, and what admind may then run for them is what
they agreed to.

An MCP client (ADR 0690) was let in to read. If authd took a command for
it as it does for a token, "this client only reads" would rest on admind
alone: an admind that went wrong while holding a client's token could
send any command, and authd would run it. A client whose owner chose
`logs` at "Read all" would in truth hold an administrator's shell, one
fault away.

## Decision

**authd takes no command for a client.** The request `client-read`
carries the client's token or pass, an action's ID and the action's
arguments as JSON, with the four descriptors an elevated command gets.
authd checks the token or the pass itself, reads the client's scopes from
its own record, and works out the areas the client was given whole, not
their reading alone. A client with none is refused.

**authd starts one program, and writes its command line itself:**
`/system/commands/admind --read AREAS ACTION ARGUMENTS`, as an
administrator. The action's ID is at most 64 letters, digits, `.` and
`-`, and the arguments at most 16 KiB with no NUL, so neither can be
another argument or another program.

**That program runs a read, or nothing.** `admind --read` finds the
action in the list of actions that is compiled into it, and goes on only
when the action is a read that runs as an administrator, that a client is
offered, in one of the areas it was given. It asks the same function,
`offer`, that the tools and the page ask (ADR 0690), so the three cannot
disagree. It makes the action's command from the arguments, as the API
does, with the same checks of each value, and refuses an action that
reads its input. It finds the command's program among the system's and
the apps' commands by a plain path, and then becomes that command, so
the read's output and its exit status are the command's own, and there is
no program in between to keep right.

**A refusal is one line and a status no read has.** `ADM-NNN: words` on
the error output, and the status 125. admind gives the client that ID.

**The areas come from authd, not from admind.** admind sends no scope:
what a client reaches as an administrator is what authd's own record of
it says at that moment, so a client that was removed, or made again with
less, reads less at its next call whatever admind believes.

## Consequences

- An admind that goes wrong while holding a client's token or pass reads
  what the client may read, and no more. It cannot run a change, start a
  shell, or read an area the client was not given.
- A read as an administrator costs one more program start than a token's
  does. The reads are short, and a client makes at most 120 calls in a
  minute.
- admind and `admind --read` are one binary, so the list of actions and
  the rule of what is offered are the same in both by construction.
- A read that takes a document on its input, or that runs as a task, is
  not offered to a client. None of the console's reads as an
  administrator needs either today.
- authd's audit log has a line for each such read, with the client, its
  owner and the action.
- What a client asks to change never comes this way. It runs by an
  administrator's own unlocked session, as a click does.

## Alternatives considered

- **The command from admind, as for a token.** One fault in admind then
  gives a client an administrator's rights. The whole of ADR 0690 would
  rest on the program that faces the network.
- **A list of the commands a client may run, in authd.** A second list of
  actions to keep the same as the first, in the one program that should
  know least about the console.
- **Every read as the client's owner.** The reads only an administrator
  has would be out of a client's reach, and the whole log is what a
  client that looks after a server reads most.
- **A helper that stays running as an administrator and answers reads.**
  A long-lived program with an administrator's rights that parses what a
  client sends. One program for each read holds nothing between two
  reads.

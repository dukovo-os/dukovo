<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0562. The web console's administrators act on other accounts, end their sessions and terminals, and try SSH's settings and the console's reach as Spark trials

- Status: Accepted
- Date: 2026-11-15

## Context

The web console's Accounts and Security pages come with C5. On Accounts an
administrator adds, changes, disables and removes other people's accounts,
gives them SSH keys, takes away their API tokens and second steps, and ends
what they have signed in. On Security the administrator changes SSH's
settings, who may reach the console, the console's sign-in rules and the
machine's private access over WireGuard.

Some of these changes can lock a person out, or cut off the browser that
makes them. Turning SSH's passwords off locks out an account that has no
key; a narrower `AllowUsers` locks out whoever it leaves off; making the
console private cuts off a browser outside the machine's networks;
removing the WireGuard device a browser comes through cuts that browser
off; disabling or removing an account, or taking away its administrator
right, can leave a machine that no one can administer.

ADR 0472 made a change to settings a Spark trial: Spark keeps the files as
they were, puts them back unless the change is kept in time, and puts the
service back in the state it was. C4 (ADR 0518) tries the network's and
the firewall's changes that way, and a page proves that its browser still
reaches the console over the new settings before Keep is offered. Spark
tries files beneath `/system/settings/` only.

ADR 0220 had `user` ask before it disabled, removed or took the right away
from the last administrator who can sign in, with `--yes` for a script.
A question in one client is no guard in another: a script, the console's
API, or a second command run at the same time passes by it.

ADR 0517 left out an administrator's view of other people's terminals,
with a row in `docs/deferred.md` for the Accounts page, which shows what
an account has signed in.

## Decision

**authd keeps one administrator.** authd refuses a Disable, a Remove or a
Set that would leave no enabled administrator (ACC-102). The check is
authd's, which every client goes through, so the console, a script and two
commands at once meet the same refusal; `user` no longer asks for `--yes`,
since there is nothing left to confirm.

**Another account's second steps and tokens are root's.** authd answers
three requests of root's, from an administrator inside elevate:
`StepsOf` gives another account's second steps and API tokens as that
account's own `steps` gives them (for `user show`), `StepsReset` removes
its authenticator app, passkeys, recovery codes and trusted browsers, and
`TokenRemoveOf` removes one of its tokens. The person is told at the next
sign-in. An administrator never makes a token for another account: a token
acts as its owner, so its owner makes it.

**The console never acts against the account in use.** An action may name
the argument that holds an account (`NOT_SELF` in the registry, with a
check of the arguments for an action where it applies only to some
changes, as `accounts.set` when it takes the administrator right away).
admind refuses it, before it runs anything, when that account is the
caller: the session's account, or a token's owner (ADM-136). It goes on
Disable, Remove, taking away one's own administrator right, and ending
one's own sessions at the machine. The refusal names the ways that remain:
another administrator, or `elevate user` at a terminal, where the person
types the command and sees what follows. `GET /api/1/actions` lists the
argument of each action that has one.

**Account changes are guarded, not tried.** They change authd's files and
state, not settings beneath `/system/settings/`, and some cannot be put
back by a file: Disable ends every process of the account at once, and a
removed passkey cannot be made again. They are guarded where no client
passes by the guard (authd's rule and admind's `NOT_SELF`), and each
dialog says what ends before it asks.

**An account's sessions and terminals.** `GET
/api/1/accounts/{name}/sessions` lists the account's console sessions,
the oldest first, with the browser, the address, the time of sign-in and
of the last request, the one in use marked; and its console terminals,
with their names and panes and nothing of what they show. `POST
/api/1/accounts/{name}/sessions/{id}/end` ends one as a sign-out does.
An account sees and ends its own; an administrator whose session is
unlocked sees and ends another account's, and may end another account's
terminal through `POST /api/1/terminals/{id}/end`, which closes ADR
0517's deferral. A person who is not an administrator gets no answer that
says another account's terminal is there (ADM-131, as for one that is
not). The session in use is Sign out's (ADM-136); one that has ended is
ADM-139.

They are routes, not actions: they act on admind's own state, which no
command reads or changes, so there is no command to show and no Ansible
task, and an API token is refused, as on every route of the table. Each
look at another account's and each end is an audit line. The ended
session's tabs are told why (`by-administrator`, or `by-you` from the
person's own other session), and authd tells the person at the next
sign-in that an administrator ended their session or terminal.

**SSH's settings are a trial, `ssh`.** `ssh-server set … --try SECONDS`
and `ssh-server off --try SECONDS` begin it: Spark keeps `sshd_config`
and sshd's service file as they were. `set` asks Spark to reload sshd,
which reads its settings again for new connections and keeps the ones
that are open, so that neither the change nor its undo drops the
connection a person may be testing from. `off` asks Spark to stop sshd,
and its undo starts it again. `ssh-server keep` and `ssh-server undo`
end the trial, and every other `ssh-server` change waits for its end
(NET-182). The page edits keywords through a form, never the file as
text, and a change that narrows who may sign in is tried; one that only
widens is not.

There is no `fresh` for SSH. The browser does not come over SSH, and no
request of the page can prove that a new SSH connection works. The page
asks the person to open one, shows the SSH sign-ins since the change, and
leaves Keep to the person. A sign-in over SSH and one at a console
terminal are each a pseudo-terminal and an address, so authd's sign-in
records (ADR 0254) end an `in` line with how the person came, `ssh` or
`console`, as ADR 0517 meant them to; `lastlog --json` gives it as `via`,
and the page lists only the sign-ins whose `via` is `ssh`. A line
written before then has none, and reads as it did.

**An account's session at the machine is ended by authd.** The kernel
lets a process signal only its own session's processes and those below
it, root's no more than another's, so an elevated `user end` cannot
reach an SSH or terminal session of another account. authd started that
session, so `user end` asks authd (`end-session`, root's to ask, for a
session authd started for that account), which hangs it up, and kills
what is left after five seconds when asked again.

**The console's reach is a trial, `console`.** `admin reach any|private
--try SECONDS` begins it: Spark keeps `admin.toml` as it was, and has
admind read it again, at once rather than at its next round. admind marks
that moment, so that the page's check that its browser still reaches the
console (`GET /api/1/console/fresh/console`, C4's route) counts only a
connection made after the new reach is in force. A browser that the new
reach leaves out has its new connection closed before TLS begins, so Keep
stays off for it. `admin keep` and `admin undo` end the trial; `admin
reach` and `admin sign-in`, which change the same file, wait for its end
(ADM-138).

**Private access is guarded by what it would cut off.** WireGuard's
files hold the server's private key and live beneath
`/system/state/wireguard/`, which Spark does not try. An action may say
which sessions it cuts off (`CUTS_OFF` in the registry). admind never
reads a file that holds a private key: before `private.remove` runs for a
session that came through the WireGuard interface, admind runs `wireguard
show --json` through authd as the same administrator, which prints public
keys and addresses only, and refuses the action with ADM-137 when the
device to be removed is the one this browser comes through. When that
read fails, the action is refused too: it fails closed. A session that
did not come through the interface needs no read, so a lost laptop can be
removed from a phone.

**A device reaches the server, not its networks.** `wireguard add` gives
a device the next free address of the tunnel and, in the device's file,
the server's address on the tunnel as its one `AllowedIPs`. The kernel
keeps one network stack for each card and does not route between them,
so a device reaches the console and the machine's other services, and
not the other machines of the server's networks. The page offers no
choice of what a device reaches; reaching those networks waits for
forwarding between cards, with the firewall rules it needs.

**Commands for each.** ssh-server(8) (`show`, `set`, `on`, `off`,
`--try`, `keep`, `undo`), wireguard(8) (`show`, `serve`, `add`,
`remove`), `admin settings --json`, `admin sign-in` and `admin reach
--try`, `keep`, `undo` are what the pages run, and what the Show command
gives to type at a terminal.

## Consequences

- A machine always has an enabled administrator, whatever client asks,
  and the console cannot be used to lock its own person out.
- An administrator can end a session left open on a lost laptop without
  disabling the account, which would end everything it runs.
- A change to SSH or to the console's reach that is not kept goes back
  by itself, and the console says so at the top of every page while it
  runs (the header's chip) and on Security (the banner).
- Keep on an SSH trial proves nothing by itself: the person keeps it
  after their own test. The handbook says to keep an SSH session open
  while a change is tried.
- Removing private access, or the device the browser comes through, is
  refused rather than tried, until WireGuard's settings move beneath
  `/system/settings/`.
- The boot tests `admin-accounts` and `admin-security` and the lab tests
  `web-console/accounts` and `web-console/security` hold each guard: an
  administrator's own account refused, the last administrator refused by
  authd from the console and from `user`, another account's session and
  terminal ended with its audit line, each trial kept and not kept, and
  the device a browser comes through refused while another is removed.

## Alternatives considered

- **Trials for account changes.** authd's state is not settings files,
  and Disable or a removed passkey cannot be put back by a file.
- **Asking before the last administrator goes, as ADR 0220 did.** A
  question in one client is no guard in another, nor between two clients
  at once.
- **An administrator making tokens for other accounts.** A token acts as
  its owner, and its owner alone should hold its secret.
- **`sshd_config` as free text on the page.** One mistake in a typed
  file locks SSH; the form changes keywords and keeps every other line as
  it was.
- **Restarting sshd for each tried change.** A restart ends the open
  connections, among them the one a person tests the change from.
- **WireGuard's files beneath `/system/settings/`, so that Spark can try
  them.** It needs wgd to read its settings from there and a move of
  every machine's files at update, and the files hold private keys, which
  the daily snapshots of `/system/settings/` and the journal's copies
  would then keep.
- **A mesh VPN as the default private access.** It is an app, and
  belongs with the apps (C7).

dukovo-admin(7) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

dukovo-admin - the web console, Dukovo Admin: signing in, second steps, unlocking, the shell and its keys, the palette, the terminal, actions, API tokens, AI clients

# DESCRIPTION

Dukovo Admin is the machine's web console (ADR 0514): pages in a browser,
served by the machine itself, where the people who run it sign in, add a
second step to their sign-in, and unlock the console to make changes as
an administrator. Its pages are in one shell, with a command palette and a
terminal (ADR 0517).

Three programs make it:

- *admind*(8) is the server. It answers HTTPS, serves the pages and checks
  each request. It holds no secret of anyone's.
- *authd*(8) holds every secret and decides every step: who may sign in,
  whether a password, a code or a passkey is right, what a session may
  do. It is the same authd that signs people in at the machine's
  consoles and over SSH, so an account has one password for all.
- A worker, *admind --worker*, which authd starts for each signed-in
  session, runs as the person, and does what the person's own account
  may: it asks authd about their second steps, and runs the commands of
  the actions that run as the person.

admind also keeps a journal of what changed on the machine, and runs the
actions that take longer than a request waits as tasks. Both are in one
store file of admind's own, _/system/state/admind/store.db_ (ADR 0516).

This page says how to reach the console, who may sign in, how, what the
second steps are, how to unlock the console and what ends a session,
what the shell of pages is, which keys it takes, what the palette finds
and what the terminal is, what the actions, the API tokens and the AI
clients are, what the journal and the tasks are, and where to look when
something fails. The
handbook's chapter on the web console does the same task by task.

# REACHING THE CONSOLE

admind listens on port 9090, or the port *admin.toml*(5) names. Open

```
https://duke-nas.local:9090
```

in a browser, with the name of the machine and *.local* after it, or one
of its addresses, and the port. Where the machine has a screen, each of
its sign-in screens (*consoled*(8)) shows the address, and the
certificate's fingerprint, under the banner, once admind runs:

```
 Web console: https://duke-nas.local:9090
 Certificate SHA-256 fingerprint:
 4F:2A:91:C3:07:5B:E8:D4:1A:6C:B2:90:3E:F7:55:0D
 A8:12:C4:7B:69:E0:3D:F1:84:2B:9A:C6:50:17:DE:73
```

admind writes these two lines to _/system/live/admind/issue_, and updates
them when the certificate changes.

The console answers only to the machine's own name, that name with
*.local*, *localhost*, the addresses of the machine's cards and the names
its certificate holds; a request for another name is refused (ADM-114),
so that no one else's DNS can point a browser at it. The port may be any,
so that a router's port forward or an SSH tunnel reaches it.
Whom it answers by address is *reach* in *admin.toml*(5): with *private*,
the default, this machine, the networks of its own cards and the address
ranges no one reaches from the internet; with *any*, everyone. A
connection from anywhere else is closed with no word. *admind*(8) says
more.

# THE FIRST VISIT

Until the machine has a certificate from an authority for its name, the
console serves the machine's own, *this-machine* (*certd*(8)). No authority
vouches for it, so a browser warns once. Use the warning to check the
certificate: the browser shows its SHA-256 fingerprint, and it has to
be the one on the machine's sign-in screen, or the one *cert show
this-machine* (*cert*(8)) prints. When they are the same, go on, and the
browser remembers the certificate for that name. If they differ, do not
sign in until you know why.

A certificate from an authority for the console's name (*cert get*, then
*certificate* in *admin.toml*(5)) needs no warning. While admind serves
one, it also asks browsers to come back over HTTPS alone; it never does
for the machine's own certificate.

# WHO MAY SIGN IN

An account signs in to the console with the name and password it has at the
machine's consoles and over SSH (*login*(1), *sshd*(8)), if it is

- an administrator's (a member of the group *admins*), or
- a member of the group *console*.

*user set --console* _name_ puts an account in *console*, and
*user set --no-console* _name_ takes it out (*user*(8)); an administrator
signs in whether or not it is in the group. Root never signs in to the console:
an administrator does, and makes changes once the console is unlocked. A
disabled account does not. An account that stops being allowed is signed
out of the console at once.

An account that is not an administrator may look at the console and
change its own second steps. The page gives it no *Unlock*, and authd
refuses it an unlock and any command as an administrator.

# SIGNING IN

The sign-in page asks for the account's name and its password. A wrong
name or password is answered *ADM-101*, the same for each, and only after
a wait that doubles from the fourth wrong try, up to a minute (*authd*(8)).
Where the browser has passkeys, the page also offers *Sign in with a
passkey*.

If the account has a second step, the page asks for it next: the code
from the account's authenticator app, or a recovery code, or the
account's passkey. It must come within 5 minutes of the password, from the
same address. *Trust this browser for 30 days*, unticked until the person
ticks it, lets this browser skip the second step the next times (Trusted
browsers, below).

A passkey alone signs in with no password and no code, if the passkey
checks the person with its PIN, fingerprint or face. It is the password
and the second step together. It does not work where the console was
opened by an address (Passkeys, below).

When the sign-in is done, admind gives the browser a cookie,
*\_\_Host-dukovo*, that names the session, and the console shows the page
the person asked for, the Overview by default (THE SHELL); an account that
has no time zone yet is first taken through the pages of its first sign-in.
The session belongs to the name the browser opened: the same console opened
by its name and by its address is two sessions.

# THE SIGN-IN RULES

_/system/settings/admin.toml_ holds the rules (*admin.toml*(5)). Its
*second-step* says who must give a second step after the password:

*"if-set"*
	Those who have added one. The default.

*"administrators"*
	Administrators, and those who have added one.

*"everyone"*
	Everyone who signs in.

An account that the rules ask for a second step, and that has none, signs
in with its password and is held to one page: it may add a second step
and sign out, and nothing else (ADM-118). Once it has added one, the
page asks it to sign in again, with the step. A session ends *session-hours*
after its sign-in (1 to 168, 12 by default) and *idle-minutes* after its
last console page closed (5 to 1440, 30 by default). authd reads the file
at each sign-in, and admind every 30 seconds to end sessions by it, so a
change applies without a restart; a mistake in the file refuses sign-ins
and says why in the log, rather than loosening a rule.

# SECOND STEPS

The page *Second step*, from *Manage the second step* under *Your account*
(THE SHELL), lists what the account has and changes it. Changing a second step asks
for the account's password, or, in an unlocked console, nothing more. An
account may have all of them.

## An authenticator app

The app of the person's phone or computer, which makes a 6-digit code
every 30 seconds from a secret it shares with authd (RFC 6238, with
HMAC-SHA-1, the app's default). *Add the app* shows a QR code and the
secret in text, for apps that cannot scan. The app counts once the code
it shows is typed in, within 10 minutes. authd takes the code of the 30
seconds before, the current 30 and the 30 after, so a phone a few seconds
out is no trouble; each code works once. An account has one app, and
removing it asks for another second step first when the rules need one.

## Passkeys

A passkey is a key held by a device or a security key, which the browser
asks for with a touch, PIN, fingerprint or face (WebAuthn). *Add a
passkey* asks for a name for it, 1 to 64 characters, which the list shows,
and *Rename* and *Remove* change it. An account has 16.

authd takes keys of three kinds, ES256, EdDSA and RS256 (with a key of
2048 to 4096 bits), which is what browsers and security keys make. It
checks each use against the key it kept, that the answer is for the
console's own page, and that a key's use count rises, since a count that
does not may mean the key was copied (a key that always says 0 is taken).
It does not check who made the key (attestation).

A passkey belongs to the name that the console was opened by, which is
what a browser holds a passkey to (the relying party). One added at
*duke-nas.local* does not work at *duke-nas*, or at an address; a browser
refuses a passkey for an address, so none is added or used there. The
sign-in page says so and turns the passkey button off.

A passkey that signs in alone, and a passkey that unlocks the console,
have to check the person (PIN, fingerprint or face), not only be touched.
As a second step after the password, a touch is enough.

## Recovery codes

Adding the first second step makes 10 recovery codes: each 16 letters
and digits, in four groups, such as *ABCD-EFGH-IJKL-MNOP*. They are
shown once, with *Copy* and *Download as text*; authd keeps only a
SHA-256 of each. A recovery code stands for the second step at sign-in, once.
Spaces, dashes and case do not matter. It does not unlock the console.
*Make new codes* makes 10 more and ends the ones there were. Removing the
last second step removes the codes.

## Trusted browsers

*Trust this browser for 30 days* at a second step gives the browser a
second cookie, *\_\_Host-dukovo-trust*, whose SHA-256 authd keeps with a
description of the browser. At a later sign-in the browser gives the
password and skips the second step. An account keeps 16 browsers, the
oldest going first. *Forget trusted browsers* forgets them all.

# UNLOCKING THE CONSOLE

Every session starts locked. The header says *Limited access*, and for an
administrator offers *Unlock*. A locked console shows what the account may
see and runs nothing as an administrator.

*Unlock* asks the administrator to prove again that it is them, as
*elevate*(8) asks for the password at a terminal: with a passkey that
checks the person, with the code of the authenticator app, or with the
password. A recovery code is not enough. authd then unlocks the session
for 15 minutes, and the header shows the minutes left. *Reset to 15 min*
sets the end 15 minutes from now; *Lock* locks it at once. It also locks
when the time runs out, when the person signs out, and a minute after
the last console page of the session closed. A session past
*session-hours* does not unlock.

Only authd decides whether a session is unlocked; the browser's page
only shows it. A request that needs an unlocked console, made while it
is locked, is refused (ADM-106).

## Commands as an administrator

An unlocked session of an administrator runs commands as an administrator,
as *elevate*(8) does, through authd: authd runs them as its own children,
never admind. The commands are those in _/system/commands_ and
_/applications/commands_, never a shell and never a path outside them, with
at most 64 arguments and 32 KiB between them. Each is in the audit log
(*log --audit*), with the session that ran it, and has that session's
first 8 characters in *DUKOVO\_CONSOLE\_SESSION* in its environment. What
runs them is an action (ACTIONS, below), which the command palette runs by
its name (THE COMMAND PALETTE).

# SESSIONS ENDING

A session ends when

- the person signs out, with *Sign out* in the account's menu in the header;
- *session-hours* have passed since the sign-in;
- no console page of it has been open for *idle-minutes*;
- its worker ends, because the account may no longer sign in or authd
  starts again;
- admind stops or starts again. Sessions are in memory only, so a restart
  signs everyone out.

The page shows why under *Sign in*, and the browser forgets its cookie.
The console holds 64 sessions at once (ADM-120). Sign-ins under way,
the challenges given to passkeys and which sessions are unlocked are in
memory too; a sign-in under way and a challenge last 5 minutes.

# THE SHELL

The console's pages sit in one frame (ADR 0517): a header along the top, the
list of pages at the left, and the page. They are files of
_/system/resources/admin_, with no framework and no build step, and a page's
code is read when it is first asked for. The pages are

- *Overview*, at _/_, the first page after the sign-in;
- *Welcome*, at _/welcome_, the first sign-in's pages, shown once to an
  account that has no time zone yet, and *Set up this server*, at _/setup_,
  for administrators;
- *Your account*, at _/account_: the display, the time zone, the way to the
  second step, the command that makes an API token, and the terminal's
  settings;
- *Second step*, at _/second-step_ (SECOND STEPS);
- *Terminal*, at _/terminal_, with *Sessions* and *Terminal settings* under
  it (THE TERMINAL);
- *What error messages mean*, at _/help/errors_, and the page of each error
  ID at _/help/errors/_ID (WHEN IT FAILS).

The list of pages names the machine, and has the *Overview* and, for an
account that may open one, the *Terminal*. A page the console does not have
says so, and a page for administrators tells anyone else that it is.

The header has, from the left: the machine's name, which opens its addresses
and the certificate's fingerprint; *Search or run a command* (THE COMMAND
PALETTE); the lock, *Limited access* with *Unlock* or *Unlocked* with the
minutes left, *Reset to 15 min* and *Lock* (UNLOCKING THE CONSOLE); *Tasks*,
which opens the drawer of the tasks (TASKS); *Help*, with *Keyboard
shortcuts*, *What error messages mean* and *About Dukovo Admin*; and the
account's menu, with *Your account*, the choice of *Dark*, *Light*, *High
contrast* or *Same as this device*, and *Sign out*. *Lock* asks first when
it would end administrator shells, and *Sign out* when terminals run (THE
TERMINAL).

At 1100 pixels and wider, at the default text size, the list of pages stays
at the left. Below that, *Pages* in the header opens it over the page, and
below 720 pixels the frame is a phone's: a bar of tabs with *Overview* and
*More*, which opens the list of pages, the account's menu and *Help*. The
handbook's chapter on the web console, "Finding your way", says how each part
works.

# KEYS

The console takes these keys. None is a key of one character, so that
nothing is done by a key pressed by mistake while typing.

[[ *Keys*
:[ *What they do*
|  Ctrl+K, Meta+K
:  Open the command palette, and close it again
|  Ctrl+/
:  Show these keys
|  Esc
:  Close the dialog, drawer, menu or note in front
|  Arrow keys, Home, End
:  Move in a menu, a list of tabs or the search results
|  Alt+Shift+K
:  Open the command palette from inside a terminal
|  Alt+Shift+Up
:  Leave a top pane of the terminal for the terminal's tabs

Inside a terminal the program has every key, so *Ctrl+K* and *Ctrl+/* are
not the console's there. The terminal has keys of its own (THE TERMINAL).

# THE COMMAND PALETTE

*Ctrl+K*, or *Search or run a command* in the header, opens the palette: a
box that finds a page, an action, a service, a certificate or a terminal by
a few of its letters. Its rows are the pages and their parts (*Terminal ›
Sessions*), the actions (*GET /api/1/actions*), the services and the
certificates the account may read, a row to start, stop, restart, enable or
disable each service and to renew each certificate, and the account's
terminals. The last 8 rows used are kept by the browser, for the account, and
shown first.

Each word typed has to be found, letter by letter and in order, in a row's
title or one of its keywords. The words may come in any order, and case and
accents do not count. A word *kind:*_value_ or *state:*_value_ keeps only the
rows of that kind (*page*, *action*, *service*, *certificate* or
*terminal*) or state. At most 50 rows show.

*Enter* goes to a page or a terminal; runs a read whose arguments are all
known and shows its answer, as a table or as facts, with the JSON; and opens
the form of anything else, made from the action's *input* (ACTIONS), with the
command in one line and *Run* or *Show the answer*. *Tab* opens the form with
the panel of the command, the API call, the Ansible task and the files open. A change
goes the way a page's button does: an administrator whose console is locked
is asked to unlock first (UNLOCKING THE CONSOLE), an account that is not an
administrator is told it can look but not change (ADM-122) and gets no *Run*,
and a long action is a task (TASKS). The handbook's chapter says more, in
"The command palette" and "Showing the command".

# THE TERMINAL

The *Terminal* page shows shells in the browser (ADR 0517). A terminal has a
name and one to four panes, each a shell on a terminal that admind holds, as
*sshd*(8) holds an SSH session's (*admind*(8), Terminals). authd starts the
shell (*authd*(8)): the person's own login shell, with a sign-in record on a
terminal such as _pts/7_ from the browser's address, which *who*(1) and
*last*(1) show; or, for an administrator whose session is unlocked, root's,
as *elevate -i* starts it, which they do not show, as an elevated command is
not recorded, and which the audit log has (*log --audit*). *nukesh*(1) shows
an *admin* badge and a *#* while the shell is root's. A shell has *TERM* set
to *xterm-256color*, *COLORTERM* to *truecolor* and *DUKOVO\_CONSOLE* to *1*.

A terminal is the account's, not the session's. It goes on when the page
closes and when the session ends, and every session of the account shows it.
An administrator's pane is its session's alone, and ends when the console
locks, when the unlock runs out and when the session ends: *Lock* asks first,
and *Reset to 15 min* is in the pane's head when the end is near. A terminal
that no page has shown for its *keep-days* (1 to 30, 7 by default) ends, and
all of them end when admind stops or starts again, since admind holds them in
memory. A browser that opens one again is shown the last 64 KiB of what it
showed.

*admin.toml*(5)'s *[terminal] allowed* says who may have a terminal and
*sessions-per-account* how many each account keeps. An account with no shell
has none (ADM-132). The limits are 4 panes to a terminal, 256 panes in all, 4
connections for terminals to a session and 16 panes to a connection (ADM-130). A
terminal or pane that is not there, has ended, or is another account's is
ADM-131, the same for each. A token never opens one (ADM-124).

The page has tabs for the terminals, panes in a layout (one pane, columns,
rows, a main pane at the left, or a grid) with a border to move, a key bar
for a phone, a *Read the screen* dialog for a screen reader, a paste that asks
first when it has several lines for a program that did not ask, *Sessions*,
which lists, opens, renames and ends the terminals, and *Terminal settings*:
*size*, *scrollback*, *keep-days*, *copy-on-select*, *announce* and
*shortcuts* (PREFERENCES). Its keys, while *shortcuts* is on, are

[[ *Keys*
:[ *What they do*
|  Alt+Shift+T
:  A new terminal
|  Alt+Shift+W
:  End this pane, and ask while a program runs
|  Alt+Shift+PageUp, Alt+Shift+PageDown
:  The tab before, the tab after
|  Alt+Shift+ an arrow key
:  The pane in that direction
|  Alt+Shift+Up in a top pane
:  Leave the pane for the tabs: the way out of the terminal
|  Alt+Shift+\\, Alt+Shift+-
:  Split right, split down
|  Delete on a terminal's tab
:  Close the terminal, and ask while a program runs
|  Alt+Shift+K
:  Open the command palette
|  Alt+Shift+R
:  Read the screen
|  Ctrl+C
:  Copy when text is selected, else send it to the program
|  Ctrl+Shift+V, Shift+Insert
:  Paste
|  Shift+PageUp, Shift+PageDown
:  Scroll the lines above the screen

With *shortcuts* off only *Alt+Shift+Up* and *Alt+Shift+K* stay. Every other
key goes to the program, but the browser keeps *Ctrl+W*, *Ctrl+T*, *Ctrl+N*
and *Ctrl+Tab*: the key bar's *Ctrl*, pressed before *W*, sends *Ctrl+W*. The
handbook's chapter says the rest, in "The terminal".

# ACTIONS

What the console does to the machine, and what it reads from it, is a list
of actions (ADR 0515). Each is one read or one change, with an id of its
area and a verb, such as *service.restart* or *certificate.show*, a route
in the API, and the command that does it. The services' and the
certificates' are the actions of this release, with the console's own: whom
it answers, what it shows of the machine (*console.about*, *console.notices*,
*checks.last*) and the machine's usage (*metrics.now*, *metrics.usage*). A
read runs as the person
who asks, with their own rights. A change runs as an administrator, for
an unlocked session of an administrator (UNLOCKING THE CONSOLE) or for an
API token whose owner is one (API TOKENS); an account that is not an
administrator is told it can look but not change (ADM-122).

*admin actions* lists the actions: each one's id, whether it reads or
changes, who runs it, its route and an example of its command. *admin
actions --json* prints the list that *GET /api/1/actions* answers, which
a script reads to learn what the console does.

*admin show* _action_ [_name_=_value_]... and the console's *Show
command*, which asks *POST /api/1/show*, give the same four things for an
action with its arguments, and run nothing:

- The command, as one line to type at Dukovo's shell or at sh, with
  *elevate* in front when the action runs as an administrator.
- The API call, a *curl* line for the action's route, with the token in
  *DUKOVO\_TOKEN*, against the address the console was asked at.
- The Ansible task, as the YAML of one task for a playbook. For the
  services' changes it is a task of *dukovo.system.service*, the module
  of the collection *dukovo.system* (install it from its release at
  github.com/dukovo-os/ansible, with ansible-core 2.22 or later), which
  reports a change only when it changed something. For every other
  action it runs the command with *ansible.builtin.command*, which cannot
  tell whether a change changed anything, so such a change task reports
  a change each time.
- The files the change writes, when it writes any.

The command shown is the command that runs: the console runs the same
words that it shows, never a shell, so the button, the API call and the
command typed at a terminal do the same thing. *admin*(8) says more, and
shows an example.

A change answers whether it changed anything, as *changed*, true or false,
so that a script that asks for what is so learns that nothing was done. The
console reads what the change moves, as the person, before the command and
after it, and the files the change names, through the journal, and says
true when it cannot tell; *admind*(8) says how. *POST /api/1/check* asks
the same without making the change, and *GET /api/1/openapi.json*
describes the whole API for a tool that reads OpenAPI. An action that the
list marks *long* answers with a task's number and runs as a task (TASKS).

A command that ends with an error is ADM-123, with the command's own
words, and with the command's own ID when the action lists it. A value that
cannot work, such as a service's name that starts with a hyphen, is
ADM-125, and nothing is run. admind's log (*log admind*) has a line for
each change that was asked for, and for a read that was refused or failed.

# API TOKENS

authd keeps API tokens for scripts, for the account that made them. A
token is a line of text,

```
dkv_ACCOUNT_SECRET
```

in which SECRET is 64 hexadecimal digits, shown once and kept only as a
SHA-256. A token has a name of 1 to 32 letters, digits and *-*, its
scopes, a last day at most 366 days after it was made, and the time it
was last used; an account has 32. A scope is *\**, which stands for all
that the owner may do, the name of an area, or the name of an area with
*:read* after it. A token is never more than its owner: it is no use
when the owner may not sign in, and one that changes something as an
administrator needs an owner who is an administrator and a scope that is
*\** or names the area, not its reading alone. Renaming the account ends
its tokens.

A token is made at a terminal, with *admin token add* _name_ *--scope*
_scope_, which asks for the account's password and shows the token once;
*admin token list* lists the account's tokens and *admin token remove*
_name_ removes one (*admin*(8)). A signed-in session makes and removes
them with its own calls to _/api/1/account/tokens/add_ and
_/api/1/account/tokens/remove_ (*admind*(8)), which no page of this
release does.

A script gives the console a token as

```
Authorization: Bearer dkv_ACCOUNT_SECRET
```

in one header, and nowhere else: a token in an address or a cookie is not
read, and one that comes with a session's cookie is refused (ADM-108).
The console takes a token for the actions, their list (*GET
/api/1/actions*), the showing of an action (*POST /api/1/show*) and the
tasks (TASKS), and for no other route: the sign-in, the unlock, the
account's own routes and the journal answer a token with ADM-124. The
scopes decide which actions. A read needs the action's area, its area with
*:read*, or *\**. A change needs the area or *\**. The list names each
action's area: *services* for *service.restart*. A token that is not
enough is ADM-124, which names the scopes that would do. Showing an action
needs the scope that reads its area.

authd checks the token on every request, so a token that is removed, that
has ended, or whose owner may no longer sign in stops working at once
(ADM-115). Ten wrong tokens in a minute from one address stop that address
for the rest of the minute (ADM-105), and authd waits after each wrong
token as it does after a wrong password. A script that reads gets a
worker of the owner's, which the console keeps for 5 minutes after the
token's last call. A token that changes something as an administrator
needs no unlock: its owner chose its scopes and its end when they made
it, and gave their password to do so. Give each token the fewest scopes
its script needs.

# AI CLIENTS

An AI client is a program that speaks MCP, the Model Context Protocol,
which an administrator lets in to look after the machine through the
console's actions (ADR 0690). It is off until an administrator adds one:
the console answers MCP to no one it was not told of.

A client belongs to the administrator who added it. It never does more
than that account may, it stops when the account does, and it always
ends: on the day chosen when it was added, a year later at most. It has a
name of 1 to 32 of *a* to *z*, *0* to *9* and *-*, and an account has 16.

A client reads, and asks. What it reaches is chosen area by area when it
is added, at one of three levels:

- *Read*: the reads its owner's account has in the console before
  unlocking it. As a scope, the area's name with *:read*.
- *Read all*: every read of the area, those an administrator has after
  unlocking as well, such as the whole log.
- *Read all and ask*: those, and asking for the area's changes. As a
  scope, the area's name alone; an area with no change to ask for stops
  at *Read all*.

A change that a client asks for changes nothing. It waits on the
console's *AI clients* page, with the client's reason, the command a yes
would run, whether it would change anything and the files it writes. An
administrator with the console unlocked approves it, and it then runs
once, as that administrator, as a click on the action's own page would;
or refuses it, which a locked console may do too. A request that nobody
answers lapses after an hour. The journal's row and the task name the
administrator, and say which client asked.

No client, whatever it was given, is offered an action that takes or
shows a password, a key or a token, or one that takes a document; a
change to what is a person's own, their account or their files; nor a
change that decides who gets in, which is every change of the accounts,
the sign-in rules, SSH, the private network and the console's own
settings, where the clients are.
A client cannot be changed once it is added: to give it more or less,
remove it and add it again, so that what it reaches is always what an
administrator saw when adding it.

A client connects in one of two ways:

- *Over SSH*, the safer way and the default. The client's MCP settings
  run *ssh* _owner_@_host_ *admin mcp* _name_ (*admin*(8)), so what lets
  it in is its owner's own SSH sign-in, and there is no secret to keep
  on the other machine. Such a client is as trusted as its owner's
  account on this machine.
- *Over HTTPS*, for a client that cannot use SSH. It is given a token,
  dkc_OWNER_SECRET, shown once and kept only as a SHA-256, which it
  sends to _/mcp_ on the console's address as *Authorization: Bearer*.
  The console's own certificate is one the client has to be told to
  trust: *cert show this-machine --pem* prints it. A client's token
  works at _/mcp_ and nowhere else, and an API token does not work
  there.

The *AI clients* page, for administrators, has *Waiting for a yes*, the
requests that wait, each with *Approve* and *Refuse*; the clients, with
how each connects, what it reads and may ask to change, when it was last
used and when it ends, and *Remove*; *What they did*, each change asked
for with what became of it and each client's reads; and *What no client
can do*. *Add a client* asks for its name, how it connects, what it may
reach and when it ends, and then shows the MCP settings to give the
client, with the token of one that connects over HTTPS, once. The page's
entry in the list of pages counts the requests that wait. *admin mcp
add*, *list* and *remove* do the same at a terminal, and *admin mcp*
_name_ *--check* says what a client reaches (*admin*(8)).

What a client reads is sent to the service behind it, which is the
administrator's choice of service and outside this machine. Text that a
client reads, such as a log line, can be written by anyone who can make
the machine log it, and a client may take it for an instruction: this is
why no change runs without a person's yes, and why the yes is given to
the command shown and not to the client's reason. admind's log has a
line for every call a client makes, and the audit log each client let in
or removed and each request approved or refused (*admind*(8)).

# WHAT CHANGED

admind keeps a journal of what changed on the machine and who changed it
(ADR 0516), so that when something stops working the first question, what
changed, has an answer. A change made through the console, from a page or
by a script with a token, has a row once its command has ended, worked or
not: when, who, how (*console*, *token* and its name, or *client* and the
name and owner of the AI client that asked for it), the action and its
command, how the command ended, and each file the action names, as it was
before and after. A file that someone edits by hand, in _/system/settings_
or in a service's own folder, has a row too. Dukovo has no notification of a
file's change, so admind looks at the files every 30 seconds, and finds who
from the audit log: the person whose elevated command named the file, or
*outside the console* when none did.

A row keeps the text of each file, so that what changed can be shown as a
diff, and the change can be put back. *admin put-back* _id_ writes the
earlier text of its files back, as root, and the console's *Put back* runs
it for an administrator's unlocked session. *Keep it* marks a hand edit as
seen by the person who kept it, and changes nothing on the machine. The
Overview's *What changed* card shows the newest five changes, with *See the
change*, *Keep it*, *Put back* and *Undo*; *See all* opens the whole journal on
the Reliability page, which filters it by who made a change, how and on
which day, and turns the console's changes that a person chooses into a shell
script that makes them on another machine (*Copy as a script*). The journal is
also in the routes of *admind*(8), and *admin put-back* puts a change back at a
terminal.

Everyone who is signed in reads the journal. It names people and files and
never the contents of a secret: a file under a service's secrets, one that
not everyone may read, and one in a folder that others may not pass
through are listed by name, with no text. An API token does not read it.
The store keeps the newest 10,000 changes, the last 20 versions of each file
and 64 MiB of text in all. *admin export* (*admin*(8)) copies the settings
into a Git repository of the person's own, for a longer history.
*admind*(8) says the routes, what each answers and what a put back writes.
The page is told of each row that ends by *event: change* on
_/api/1/events_, which says its number, how it was made and when, and
nothing of who or which files.

# TASKS

An action that takes longer than a request may wait, such as an update or
the backup of a whole machine, is marked *long* in the list and runs as a
task (ADR 0516). Its route answers at once with the task's number, and
admind runs its command as an administrator without the 30 seconds that a
short command has, so that the work goes on when the browser is closed and
can be seen from another. A task waits for a place, runs, and ends *done*,
*failed* or *stopped*. Its command says its step and how far it has come with
the lines *step 2 of 4: copying* and *progress 40%* on its standard output,
and its last lines are its log.

The person who asked, or an administrator, stops a task, which sends its
command SIGTERM so that it puts back what it did. A token reads the tasks of
the areas its scopes read, and stops those of the areas they change. A task
is kept for 30 days after it ends, and the journal keeps its change. The
page is told of each change of a task by *event: task* on _/api/1/events_.
None of the actions of this release is long yet. *Tasks* in the header opens
the drawer of the tasks: those that wait or run, those that failed or were
stopped, and those that finished today, with *Stop* for a task that the
person asked for, or any task for an administrator.

# PREFERENCES

A person's preferences are their own: the console's colours (*theme*), the
size of its text (*text*), whether things move (*motion*), the time zone
it shows times in (*zone*), whether pages show the command an action runs
(*commands*, *hidden* or *shown*) and the terminal's (a *[terminal]* table:
*size*, *scrollback*, *copy-on-select*, *keep-days*, *announce* and
*shortcuts*). They are in _~/.config/dukovo-admin/settings.toml_,
which the person's worker reads and writes as them, and which they may edit
by hand: a key that is missing or wrong, a number out of its range included,
reads as its default, so that a typo costs a preference and never the page.
*admind*(8) has the routes and each key's values.

# WHAT IT KEEPS

authd keeps each account's second steps and tokens in one file,
_/system/state/authd/console/_*name*.toml, which only root reads: the
app's secret, each passkey's public key and use count, the recovery codes
and trusted browsers and tokens as SHA-256 hashes, and nothing in the
clear that would sign anyone in. Removing the account removes the file.
The account's AI clients are in a file beside it,
_/system/state/authd/clients/_*name*.toml, with each token as a SHA-256.
*authd*(8) says more.

The sign-in rules and where it listens are in _/system/settings/admin.toml_.
The pages are in _/system/resources/admin_, each with its SHA-256 in
_manifest.txt_, and admind serves only the files that match.

admind's own store, _/system/state/admind/store.db_, is one SQLite file, mode
0600, that only admind writes. It holds the journal of what changed, the
text of the last 20 versions of each file the journal names (64 MiB in all)
and the tasks, and no secret: a secret's contents are never read into it.
What the AI clients did is in _/system/state/admind/clients.jsonl_, the
newest 1000 entries, which name actions and never their arguments; the
requests that wait are in admind's memory only.
Each person's preferences are in their own file,
_~/.config/dukovo-admin/settings.toml_. The terminals are in admind's memory
only, as the sessions are, and a restart ends them (THE TERMINAL). The
palette's list of recent rows is in the browser's own storage, for the
account.

Each console sign-in and a wrong password or code at one are in
the sign-in records on the terminal *web*, from the browser's
address, so *last*(1) and *lastb*(1) show them with the rest. authd's audit
log (*log --audit*) has a line for each sign-in and how it was proved
(password, an app's code, a recovery code with how many are left, a
passkey, a trusted browser), each wrong proof, each unlock, lock, end and
extension, each change to a second step or a token, and each AI client
let in or removed, with the session's first 8 characters. It never has a password, a code, a cookie,
a ticket or a token.

# WHEN IT FAILS

Each error the console gives has an ID, *ADM-101* to *ADM-146*, and a
page of its own in the support center that says what to do. The page
shows the ID with a short title and a sentence on what to do; *admind*(8)
lists them.

The console puts an error where it belongs: under the field whose value
cannot work, in the unlock dialog for a locked console, on the sign-in page
for an ended session, as a countdown for too many tries, and otherwise in a
box with *What happened*, *What to do*, the command's own message when there
is one, the ID and *Copy the details*. *What error messages mean* in the Help
menu lists the pages, which the console serves from this machine at
_/help/errors_. When the machine does not answer, a banner says so and the
page asks again after 5, 10, 30 and 60 seconds, and then every 60.

If the console does not answer at all, check that it runs, and read its
log, which says why it did not start:

```
$ elevate service admind status
$ elevate log -s admind
```

authd's side is *elevate log -s authd* and *elevate log --audit*. When
*admin.toml* or the certificate does not read, or admind cannot listen,
it says so in the log and stops; *service admind restart* starts it again
once that is fixed, and signs everyone out. After a change to the
settings or the certificate, *service admind reload* is enough. A console
whose pages do not load, or load with no style, has files that do not
match their manifest; the log names them. A console that says it keeps no
journal or tasks (ADM-126) could not open its store when it started: the
log says why, and *service admind restart* opens it again once that is
fixed. Everything else in the console works without it.

A person who is locked out of their second step signs in with a recovery
code and adds a new step. One with no recovery code left and no step
works has to ask an administrator, who deletes the account's file in
_/system/state/authd/console_, which forgets all of its second steps,
trusted browsers and tokens at once. The account then signs in with its
password and adds a new step.

# FILES

_/system/settings/admin.toml_
	The sign-in rules, the port, whom the console answers and the
	certificate (*admin.toml*(5)).

_/system/settings/spark/services/admind.toml_
	The service.

_/system/resources/admin_
	The console's pages, with _manifest.txt_.

_/system/state/admind/store.db_
	The store: the journal of what changed, the versions of the files it
	names, and the tasks.

_~/.config/dukovo-admin/settings.toml_
	A person's preferences.

_/system/live/admind/issue_
	The address and the certificate's fingerprint, shown at the
	machine's sign-in screens.

_/system/state/authd/console/_*name*.toml
	An account's second steps and tokens.

_/system/state/authd/clients/_*name*.toml
	An account's AI clients.

_/system/state/admind/clients.jsonl_
	What the AI clients did.

_/system/live/admind/mcp_
	The socket of the AI clients that connect over SSH.

_/system/state/sign-ins_
	The sign-in records (*last*(1)).

# SEE ALSO

*admind*(8), *admin*(8), *authd*(8), *admin.toml*(5), *consoled*(8), *user*(8),
*cert*(8), *certd*(8), *elevate*(8), *service*(8), *sshd*(8), *log*(1),
*last*(1), *who*(1), *nukesh*(1), *dukovo-certificates*(7)

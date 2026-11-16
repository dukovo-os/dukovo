user(8) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

user - make, list, disable, remove, rename and change accounts

# SYNOPSIS

*user add* [*--admin*] [*--full-name* _name_] _name_

*user list* [*--json*]

*user show* [*--json*] [_name_]

*user disable* [*--yes*] _name_

*user enable* _name_

*user remove* [*--keep-home* | *--archive-home* | *--delete-home*] [*--yes*] _name_

*user rename* _name_ _new-name_

*user set* [*--full-name* _name_] [*--admin* | *--no-admin*] [*--elevate-without-password* | *--elevate-with-password*] [*--console* | *--no-console*] [*--jobs-signed-out* *never* | *allowed*] [*--yes*] _name_

*user set* *--full-name* _name_

*user key add* [_name_] _line_

*user key remove* [_name_] _fingerprint_

*user end* _name_ _session_

*user steps reset* _name_

*user token remove* _name_ _token_

# DESCRIPTION

*user* makes, lists and changes Dukovo's accounts, through *authd*(8),
which alone changes the account files (ADR 0218, ADR 0220). Only root
changes an account, which an administrator is inside *elevate*(8),
except that anyone may change their own full name and their own keys.

A command that names an account there is none called is refused with
*ACC-101*, and *user add* and *user rename* for a name an account has
already with *ACC-105*.

*user add* makes the account _name_: its user and group, its password
and its home, _/users/_*name*. It asks for the password twice, unseen,
at the terminal; with no terminal, as in a script, the password is the
first line of the standard input, asked once. No rules are forced on it:
an empty password, or one shorter than 8 characters, gets a warning,
and at a terminal the question whether to keep it. Only root may make
an account, which an administrator is inside *elevate*(8): *elevate
user add bob*.

A name is 1 to 32 lowercase letters, digits, *\_*, *-* and *.*, starting
with a letter or *\_*, and one no user or group has. *authd*(8) says what
an account gets.

*user list* lists the people's accounts, those numbered from 1000, with
their numbers, whether each is an administrator's, whether it is
enabled or disabled, which only root reads (*-* to others), their homes
and full names.

With *--json* it writes one JSON array on the standard output instead,
which the web console reads (*admind*(8)): an object for each of those
accounts, in the order of the account file, with *name*, *uid*,
*full\_name*, *home*, *shell*, *admin* (whether it is in *admins*),
*console* (whether it may sign in to the web console), *groups* (every
group it is in, its own too), and *disabled* (true or false) and
*password* (*set* or *none*). The last two are root's to read, as above,
and are null to anyone else.

*user show* shows one account whole. Root shows any; anyone else shows
their own, with no _name_. With *--json* it writes one JSON object:

```
{"format": 1,
 "account": {"name": "alice", "uid": 1000, "full_name": "Alice Liddell",
             "home": "/users/alice", "shell": "/system/commands/nukesh",
             "admin": true, "console": true, "groups": ["admins", "alice"],
             "disabled": null, "password": null,
             "since_ms": 1760900000000, "password_changed_ms": null},
 "keys": [{"type": "ssh-ed25519", "comment": "laptop",
           "fingerprint": "SHA256:...", "line": 3, "options": false}],
 "steps": {"app": true, "passkeys": [{"name": "Security key", "id": "..."}],
           "recovery_left": 8, "trusted_browsers": 2},
 "tokens": [{"name": "ansible", "scopes": ["*"], "until_ms": 1765600000000}],
 "sessions": [{"id": "4123-1763110000", "kind": "ssh", "terminal": "pts/3",
               "from": "10.8.0.2", "since_ms": 1763110000000, "processes": 5}]}
```

*account* has the fields of *user list --json* and two more: *since\_ms*,
when the account's home folder was made, null where its filesystem keeps
no such time; and *password\_changed\_ms*, which is always null, since the
account files do not keep when a password last changed. *keys* are those
of _~/.ssh/authorized\_keys_, each with its _line_ in the file and
*options* true for a line that has options before its key, which *authd*(8)
passes over; it is null where the file cannot be read safely (not the
person's, or reached through a link). *steps* and *tokens* are what
*authd*(8) keeps: for one's own, as the web console asks it; for another
account, as root. *recovery\_left* is null while the account has no
recovery codes, and *until\_ms* of a token is when it ends. Both are null
where *authd*(8) gives none, which it does not for an account that no one
signs in to the console with. *sessions* are the account's sessions at
the machine, as *who*(1) finds them, with *kind* *ssh* for a terminal
someone came to from an address, which the web console's terminals are
too, *serial* for a serial line, *screen* for the machine's own screen,
and *other* where the terminal says no more; and *id* the number of the
session's leader and the second it began, the one *user end* takes.
*from* is null where the sign-in was at the machine itself or no record
of it is kept; *sessions* is null where the processes cannot be read. Without *--json* the same is written for a person to read. A
field the machine does not say is null, never a value made up for it.

*user key add* adds a public key to an account's _~/.ssh/authorized\_keys_,
and *user key remove* takes one away, by the fingerprint that *user show*
and *ssh-keygen -l* give: *SHA256:* and 43 characters. Root changes any
account's keys. With no _name_, the one operand is the line or the
fingerprint, and the keys are the caller's own, which anyone may change.
The line is one line as *ssh-keygen* writes it in a _.pub_ file, of a kind
Dukovo signs in with (Ed25519, or ECDSA with P-256 or P-384), with no
options before the key, and no longer than 16 KiB; the parser is the one
*authd*(8) reads the file with, so a line is taken only if authd would take
the key from it. Any other is refused with *ACC-103*. A key already there
is not added twice, and that is no error. A fingerprint none of the
account's keys has is *ACC-104*. The folder _.ssh_ is made 0700 and the
file 0600, the account's, as *sshd*(8) needs them, and the file is written
whole into a new one beside it and renamed over the old, so nothing in the
account's home leads root elsewhere.

*user end* ends a session of the account _name_ at the machine, as *user
show* lists it, for a person whose session was left open, or whose phone
or laptop is lost. It checks that the leader of _session_ is a process of
that account, and was made at the second the session says, before it
sends anything, so a number that now belongs to another process is never
ended. *authd*(8) sends the signals, as the session is one it started and
its processes are below it, which the kernel lets no other process
signal, root's neither: SIGHUP to each process of the session that acts
for the account, and SIGKILL to each that is still there 5 seconds
later. It says how many hung up and how many were killed, and the person
is told at their next sign-in. A session that is not there, whose number
is another account's or another process's, or that authd did not start,
is *ACC-104*. It refuses to end the session it is run in.

*user disable* disables the account _name_ at once: nothing signs in to
it, and everything it runs ends, its sessions, the programs it left
running and the commands it runs through *elevate*(8), however many it
starts meanwhile. A folder it handed to another program opens nothing
more. Its password is kept: *user enable* enables it again, as it was.
Someone who knows the password of a disabled account is told it is
disabled when they sign in; anyone else is told the password is wrong.

*user remove* removes the account _name_: it disables it and ends what
it runs, then takes it out of the account files, with its own group, and
removes its own _/tmp_ (*authd*(8)); its sign-in records stay, for root
to read. Its number is never given to an account again, so the files it
leaves are no one else's. At a terminal it asks what becomes of the
account's home, _/users/_*name*: whether to keep it, archive it or
delete it; a script says so with an option. A home elsewhere is kept.

*user rename* gives the account _name_ the name _new-name_, and its own
group too, and moves its home if it is _/users/_*name*. It waits until
nothing runs for the account: until its person has signed out and what
they left running has ended, or the account is disabled.

*user set* changes the account's full name, whether it is an
administrator's, whether it elevates without a password, whether it
signs in to the web console without being an administrator
(*dukovo-admin*(7)), or whether its jobs may run while its person is
signed out. Someone who is no longer an administrator keeps
nothing of the admin set: their elevated commands end, and elevate
asks them nothing more. With no _name_, and *--full-name* alone, it
changes the caller's own full name, which *authd*(8) lets anyone do; the
other options need a _name_ and root.

An account signs in to the web console if it is an administrator's, or
a member of the group *console*, which *--console* puts it in and
*--no-console* takes it out of; authd makes the group, number 12, the
first time. An administrator signs in either way, and root never does.
An account that is no longer allowed is signed out of the console at
once.

*user steps reset* removes the second steps of the account _name_, for
a person who lost the phone or key that holds them: the authenticator
app, the passkeys, the recovery codes and the browsers they trusted. It
says what went. They sign in with their password alone, and are told
why at their next sign-in; when the sign-in rules ask them for a second
step (*admin.toml*(5)), they add a new one before anything else. Their
API tokens stay.

*user token remove* removes the API token named _token_ of the account
_name_ (*admin*(8)), and whatever uses it stops working; *ACC-104* when
the account has no such token. A person removes their own with *admin
token remove*.

An administrator does not disable, remove or rename the account they
are signed in with: another administrator does. Nothing disables or
removes the last administrator whose account is enabled, or makes them
no longer one: *authd*(8) refuses it, at a terminal as from the web
console, since Rescue and recovery need one (*ACC-102*). Make another
account an administrator first. The startup-trouble menu (*login*(1))
resets a disabled administrator's password and enables the account.

# OPTIONS

*--admin*
	Make an administrator's account: one in the *admins* group, which
	may run commands as root with *elevate*(8).

*--full-name* _name_
	The account's full name, which the passwd file keeps after its
	number, as Linux's GECOS field; it may hold no colon or line break,
	and has at most 256 bytes.

*--json*
	With *user list* and *user show*: write JSON, as above.

*--no-admin*
	With *user set*: the account is no longer an administrator's.

*--jobs-signed-out* *never* | *allowed*
	Whether the account's person may have their jobs run while they
	are signed out (ADR 0478). The person chooses with
	*crontab --keep-running* (*crontab*(1)); *never* bars it, whatever
	they chose, and their jobs wait for them to sign in, which tells
	them how many did not run; *allowed* takes the bar away. An
	account is allowed unless it was barred. *authd*(8) keeps it.

*--elevate-without-password*
	With *user set*: the administrator elevates without typing a
	password, as a *NOPASSWD* rule of sudo's lets one on Linux: the
	account goes in the *elevate-without-password* group (ADR 0328).
	A cloud machine's account is made so by *cloud-setup*(8), as it
	signs in with an SSH key and has a password no one knows. Anyone
	who signs in as it, or runs a program as it, can then do anything
	as root: give it to an account whose password, keys and sessions
	are safe.

*--elevate-with-password*
	With *user set*: the administrator types their password to
	elevate again, as by default.

*--console*
	With *user set*: the account signs in to the web console without
	being an administrator: it goes in the *console* group. It may look
	at the console and change its own second steps there; it makes no
	change as an administrator.

*--no-console*
	With *user set*: the account is no longer in the *console* group. An
	administrator still signs in to the console.

*--keep-home*
	With *user remove*: keep the home where it is. Its files stay
	those of the removed account's number, which no account gets
	again, so only administrators reach them; a new account of the
	same name is told so, and how to give it the files.

*--archive-home*
	With *user remove*: pack the home with *tar*(1) and zstd into a
	file in _/users/.removed_, root's alone, named for the account and
	the day, as _bob-2026-10-31.tar.zst_, then delete it.

*--delete-home*
	With *user remove*: delete the home and every file in it. At a
	terminal it asks first.

*-y*, *--yes*
	Go on without asking before deleting a home. *user disable* and
	*user set* take it and ask nothing.

# EXAMPLES

Make an administrator's account for Alice:

```
# user add --admin --full-name "Alice Liddell" alice
Making an administrator's account for alice.
New password:
The same again:
Made alice, number 1000, home /users/alice
```

List the accounts:

```
$ elevate user list
NAME             NUMBER  KIND           STATE     HOME                 FULL NAME
alice              1000  administrator  enabled   /users/alice         Alice Liddell
bob                1001  user           disabled  /users/bob
```

Disable Bob's account, which ends his sessions at once, then enable it
again:

```
$ elevate user disable bob
bob is disabled: 3 of its processes ended. user enable bob enables it again.
$ elevate user enable bob
bob is enabled again.
```

Remove Bob's account, archiving his home:

```
$ elevate user remove --archive-home bob
bob is removed: its home is archived in /users/.removed/bob-2026-10-31.tar.zst,
for administrators only.
```

Let Bob, who is no administrator, sign in to the web console:

```
$ elevate user set --console bob
bob is changed: signs in to the web console.
```

Change your own full name:

```
$ user set --full-name "Alice P. Liddell"
alice is changed: full name "Alice P. Liddell".
```

Add a key to your own account, then take it away by its fingerprint:

```
$ user key add "$(cat ~/.ssh/id_ed25519.pub)"
$ user show | grep -A3 Keys
$ user key remove SHA256:Zk...
```

End a session of Bob's that was left open:

```
$ elevate user show bob
$ elevate user end bob 4123-1763110000
```

# FILES

_/system/state/authd/retired_
	The numbers of removed accounts, which no account gets again: a
	line _number_:_name_:_date_ each, root's alone.

_/users/.removed_
	The archives of removed accounts' homes, root's alone.

# EXIT STATUS

0 when the account was made, listed, shown or changed; 1 when it was not,
saying why.

# SEE ALSO

*passwd*(1), *login*(1), *authd*(8), *admind*(8), *dukovo-admin*(7), *crontab*(1),
*elevate*(8), *id*(1), *tar*(1), *systools*(1)

dukovo-admin(7) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

dukovo-admin - the web console, Dukovo Admin: reaching it, signing in, the second step, unlocking

# DESCRIPTION

Dukovo Admin is the machine's web console (ADR 0514): pages in a browser,
served by the machine itself, where the people who run it sign in, add a
second step to their sign-in, and unlock the console to make changes as
an administrator.

Three programs make it:

- *admind*(8) is the server. It answers HTTPS, serves the pages and checks
  each request. It holds no secret of anyone's.
- *authd*(8) holds every secret and decides every step: who may sign in,
  whether a password, a code or a passkey is right, what a session may
  do. It is the same authd that signs people in at the machine's
  consoles and over SSH, so an account has one password for all.
- A worker, *admind --worker*, which authd starts for each signed-in
  session, runs as the person, and does what the person's own account
  may: it asks authd about their second steps.

This page says how to reach the console, who may sign in, how, what the
second steps are, how to unlock the console and what ends a session, and
where to look when something fails. The handbook's chapter on the web
console does the same task by task.

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
its certificate holds, each with the port; a request for another name is
refused (ADM-114), so that no one else's DNS can point a browser at it.
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
refuses it any command as an administrator.

# SIGNING IN

The sign-in page asks for the account's name and its password. A wrong
name or password is answered *ADM-101*, the same for each, and only after
a wait that doubles from the fourth wrong try, up to a minute (*authd*(8)).
Where the browser has passkeys, the page also offers *Sign in with a
passkey*.

If the account has a second step, the page asks for it next: the code
from the account's authenticator app, or a recovery code, or the
account's passkey. It must come within 5 minutes of the password, from the
same address. *Trust this browser for 30 days*, ticked here, lets this
browser skip the second step the next times (Trusted browsers, below).

A passkey alone signs in with no password and no code, if the passkey
checks the person with its PIN, fingerprint or face. It is the password
and the second step together. It does not work where the console was
opened by an address (Passkeys, below).

When the sign-in is done, admind gives the browser a cookie,
*\_\_Host-dukovo*, that names the session, and the console shows its home
page, which says who is signed in and links to the second step. The
session belongs to the name the browser opened: the same console opened
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

The page *Second step*, from the home page's *Manage the second step*,
lists what the account has and changes it. Changing a second step asks
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
description of the browser. At a later sign-in the browser gives the password and skips the second
step. An account keeps 16 browsers, the oldest going first. *Forget trusted
browsers* forgets them all.

# UNLOCKING THE CONSOLE

Every session starts locked. The header says *Limited access*, and for an
administrator offers *Unlock*. A locked console shows what the account may
see and runs nothing as an administrator.

*Unlock* asks the administrator to prove again that it is them, as
*elevate*(8) asks for the password at a terminal: with a passkey that checks the
person, with the code of the authenticator app, or with the password. A
recovery code is not enough. authd then unlocks the session for 15
minutes, and the header shows the minutes left. *+15 min* sets the end
15 minutes from now; *Lock* locks it at once. It also locks when the
time runs out, when the person signs out, and a minute after the last
console page of the session closed. A session past *session-hours* does
not unlock.

Only authd decides whether a session is unlocked; the browser's page
only shows it. A request that needs an unlocked console, made while it is locked, is
refused (ADM-106).

## Commands as an administrator

An unlocked session of an administrator runs commands as an administrator,
as *elevate*(8) does, through authd: authd runs them as its own children,
never admind. The commands are those in _/system/commands_ and
_/applications/commands_, never a shell and never a path outside them, with
at most 64 arguments and 32 KiB between them. Each is in the audit log
(*log --audit*), with the session that ran it, and has that session's
first 8 characters in *DUKOVO\_CONSOLE\_SESSION* in its environment. The
pages of this release run none of their own.

# SESSIONS ENDING

A session ends when

- the person signs out, with *Sign out* in the header;
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
when the owner may not sign in, and one that runs commands as an
administrator needs the scope *\** and an owner who is an administrator.
Renaming the account ends its tokens.

The console's own calls make a token and remove one, and the list of a
second step names them. None of this release's pages makes one, and
admind takes none: a request with *Authorization: Bearer* and a token is
refused (ADM-115), and no part of the console answers to one yet.

# WHAT IT KEEPS

authd keeps each account's second steps and tokens in one file,
_/system/state/authd/console/_*name*.toml, which only root reads: the
app's secret, each passkey's public key and use count, the recovery codes
and trusted browsers and tokens as SHA-256 hashes, and nothing in the
clear that would sign anyone in. Removing the account removes the file.
*authd*(8) says more.

The sign-in rules and where it listens are in _/system/settings/admin.toml_.
The pages are in _/system/resources/admin_, each with its SHA-256 in
_manifest.txt_, and admind serves only the files that match.

Each console sign-in and a wrong password or code at one are in
the sign-in records on the terminal *web?console*, from the browser's
address, so *last*(1) and *lastb*(1) show them with the rest. authd's audit
log (*log --audit*) has a line for each sign-in and how it was proved
(password, an app's code, a recovery code with how many are left, a
passkey, a trusted browser), each wrong proof, each unlock, lock, end and
extension, and each change to a second step or a token, with the
session's first 8 characters. It never has a password, a code, a cookie,
a ticket or a token.

# WHEN IT FAILS

Each error the console gives has an ID, *ADM-101* to *ADM-121*, and a
page of its own in the support center that says what to do. The page
shows the ID with a short title and a sentence on what to do; *admind*(8)
lists them.

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
match their manifest; the log names them.

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

_/system/live/admind/issue_
	The address and the certificate's fingerprint, shown at the
	machine's sign-in screens.

_/system/state/authd/console/_*name*.toml
	An account's second steps and tokens.

_/system/state/sign-ins_
	The sign-in records (*last*(1)).

# SEE ALSO

*admind*(8), *authd*(8), *admin.toml*(5), *consoled*(8), *user*(8),
*cert*(8), *certd*(8), *elevate*(8), *service*(8), *log*(1), *last*(1),
*dukovo-certificates*(7)

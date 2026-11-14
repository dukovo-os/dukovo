<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# The Dukovo Admin web console

Dukovo Admin is the machine's web console: pages in a browser, served by
the machine itself, for the people who run it. Everything in the free
console stays free, and nothing in it ever moves behind a paywall. This
chapter covers how to reach it, sign in, add a second step, unlock the
console to make changes, use its API from a script, and what to do when
something does not work. The references are dukovo-admin(7), admind(8),
admin(8), admin.toml(5), authd(8) and user(8); the decisions are ADR 0514
and ADR 0515.

| Task | How |
| --- | --- |
| Open the console | `https://NAME.local:9090` in a browser, with the machine's name |
| Find its address and fingerprint | The machine's sign-in screen, `admin fingerprint`, or `cert show this-machine` |
| Let someone who is not an administrator sign in | `elevate user set --console NAME` |
| Take that away again | `elevate user set --no-console NAME` |
| Ask everyone for a second step | `second-step = "everyone"` in `/system/settings/admin.toml` |
| Read what the console did | `elevate log -s admind` and `elevate log --audit` |
| See what the console can do | `admin actions` |
| See the command, API call and Ansible task of an action | `admin show service.restart name=nginx` |
| Make a token for a script | `admin token add NAME --scope services:read` |
| Start the console again | `elevate service admind restart` |

## Reaching the console

The console answers HTTPS on port 9090. Open

```
https://duke-nas.local:9090
```

with the machine's name in place of `duke-nas`, from a computer on the
same network. The name with `.local` is the one the machine answers to on
the local network (see [Networking](networking.md)); its address works as
well. Use the port.

The machine's own sign-in screens show where the console is, under the
welcome banner, once it runs:

```
 Web console: https://duke-nas.local:9090
 Certificate SHA-256 fingerprint:
 4F:2A:91:C3:07:5B:E8:D4:1A:6C:B2:90:3E:F7:55:0D
 A8:12:C4:7B:69:E0:3D:F1:84:2B:9A:C6:50:17:DE:73
```

A machine with no screen shows nothing. Over SSH, `admin fingerprint`
prints the same two lines, and `elevate service admind status` says
whether the console runs. `cert show this-machine` prints the fingerprint
too.

By default the console answers only this machine, the networks the
machine is on, and the address ranges that no one reaches from the
internet, such as `192.168.0.0/16`. A computer anywhere else gets no
answer at all, and the browser says it cannot connect. To answer anyone,
set `reach = "any"` in `/system/settings/admin.toml`, and first make the
administrators use a second step (below). The port is `listen` in the same
file and `network.listen` in the console's service file, which has to
change with it.

The console answers only to the machine's own names. Opened by another
name, as through a router that forwards the port, it says **This is not
the name of this machine** (ADM-114).

## The first visit

Until the machine has a certificate from an authority for its name, the
console uses its own, `this-machine`. Nobody else vouches for it, so the
browser warns once. Use the warning to check the certificate:

1. Find the fingerprint the machine shows, at its sign-in screen or with
   `cert show this-machine`.
2. Ask the browser to show the certificate it was given. It shows a
   SHA-256 fingerprint of 32 pairs of digits.
3. Compare them. If they are the same, tell the browser to go on, and to
   remember the certificate.
4. If they differ, do not sign in until you know why.

The sign-in page also shows the fingerprint, in a note. It came over the
connection you are checking, so it proves nothing about it: compare the
browser's with the machine's own.

The machine makes `this-machine` again 30 days before it runs out and
when it is renamed, so a new fingerprint after either is expected (see
[Certificates](certificates.md)). With a certificate from an authority for
the console's name, the browser does not warn. Get one with `cert get`,
then name it as `certificate` in `/system/settings/admin.toml` and in the
`certificates` of the console's service file (`elevate service edit
admind`), and run `elevate service admind reload`.

## Signing in

The sign-in page asks for an **Account** and a **Password**. They are the
ones you use at the machine's own console and over SSH.

An account signs in if it is an administrator's, or a member of the group
`console`. Root never signs in: an administrator does, and unlocks the
console to make changes. A disabled account does not. To let someone who
is not an administrator look at the console and manage their own sign-in:

```
$ elevate user set --console bob
bob is changed: signs in to the web console.
```

`elevate user set --no-console bob` takes it away, and signs bob out of
the console at once.

What happens next depends on the account:

- With no second step, you are in after the password.
- With one, the page asks for it: **Enter the code from your app**,
  **Use your passkey** or **Enter a recovery code**, with links to the
  others you have. Tick **Trust this browser for 30 days** on a computer
  that only you use, so that it skips the step next time.
- With a passkey, you may skip the password: **Sign in with a passkey**
  asks the passkey, which checks you with its PIN, fingerprint or face,
  and signs you in with nothing typed.

A wrong name or password always gets **The name or the password is wrong**
(ADM-101), whichever is wrong, and only after a wait that grows from the
fourth wrong try in a row to a minute. Wait for the answer; trying at once
makes the next wait longer. The machine's own console and SSH wait the
same way.

A session belongs to the name the browser opened. The console opened as
`duke-nas.local` and as an address are two sessions.

## Adding a second step

A second step is something you have, asked for after the password.
**Second step** in the console, from **Manage the second step** on the
home page, lists what you have and changes it. Each change asks for your
password.

| Second step | What it is |
| --- | --- |
| Authenticator app | An app on your phone that shows a six-digit code every 30 seconds |
| Passkey | A key held by a device or a security key, which the browser asks for |
| Recovery codes | Ten codes, each used once, for when the app or key is lost |

### An authenticator app

1. On **Second step**, under **Authenticator app**, press **Add the app**
   and give your password.
2. Scan the QR code with the app, or type the secret it shows.
3. Type the code the app shows into **Code from the app**, and press
   **Add the app**.

The app counts once a code of it is typed in, within 10 minutes. Codes
change every 30 seconds, and the console takes the one before, the one now
and the one after, so a phone a few seconds out is no trouble. Each code
works once. The app has to keep the right time: set the phone's clock to
automatic.

### A passkey

1. Open the console by the machine's name, not its address: a browser
   keeps a passkey for a name, and none for an address.
2. Press **Add a passkey**, give your password and a name for it, such as
   `YubiKey 5C` or `Phone`, and follow the browser's prompt.
3. Add a second passkey as well, on another device or key, in case the
   first is lost.

A passkey belongs to the name it was added at: one added at
`duke-nas.local` does not work at `duke-nas`. **Rename** and **Remove**
change it later. You may have 16. Passkeys of three kinds work, ES256,
EdDSA and RS256, which is what browsers and security keys make; the
console does not check who made a key.

### Recovery codes

Adding your first second step makes ten recovery codes, such as
`ABCD-EFGH-IJKL-MNOP`, and shows them once. **Copy** them or **Download as
text**, and keep them where the phone is not: in a password manager, or
on paper in a safe place. Each works once, in place of the app or
passkey, at sign-in. They do not unlock the console. **Make new codes**
makes ten more, and the old ones stop working. Removing your last second
step removes the codes.

### Asking everyone for a second step

`second-step` in `/system/settings/admin.toml` says who must give one:

```
[sign-in]
second-step = "administrators"
```

| Value | Who gives a second step after the password |
| --- | --- |
| `"if-set"` | Those who have added one. The default |
| `"administrators"` | Administrators, and those who have added one |
| `"everyone"` | Everyone who signs in |

The change applies to the next sign-in. Someone the rule asks, who has no
second step, signs in with the password and sees only **Second step**:
they may add one and sign out, and nothing else (**This session may only
add a second step**, ADM-118). Once added, they sign in again, with it.
The rule also stops anyone removing their last second step. A mistake in
the file refuses sign-ins and says why in the log, rather than loosening a
rule.

## Unlocking the console

Every session starts locked. The header says **Limited access**, and a
locked console shows what you may see and changes nothing as an
administrator. Only an administrator unlocks it:

1. Press **Unlock** in the header.
2. Prove again that it is you: with your passkey, which checks you; with
   the six-digit code from your app; or with your password. A recovery
   code does not unlock.
3. The header shows **Unlocked** and the minutes left, from 15.

**Reset to 15 min** sets the end to 15 minutes from now, and **Lock**
locks at once. It locks itself when the 15 minutes run out, when you sign
out, and a minute after the last console page of the session is closed.
This is as `elevate` unlocks a terminal for 15 minutes at the machine's
own console. Every command the console runs as an administrator, and every
unlock, is in the audit log: `elevate log --audit`.

A change that needs an unlocked console, tried while it is locked, says
**The console is locked** (ADM-106). A script does not unlock the console:
it uses an API token (see "Using the API").

## Trusted browsers

**Trust this browser for 30 days**, ticked at the second step, lets that
browser sign in with the password alone for 30 days. The console keeps a
hash of a secret that the browser holds, not the secret. You may trust 16
browsers; the oldest goes first. **Forget trusted browsers**, on **Second
step**, forgets them all, which is the right step when a laptop is lost.

## Signing out, and when a session ends

**Sign out**, in the header, ends the session. A session also ends

- 12 hours after the sign-in, by default (`session-hours`, 1 to 168);
- 30 minutes after the last console page of it was closed, by default
  (`idle-minutes`, 5 to 1440);
- when the account may no longer sign in, as when it is disabled;
- when the console starts again. Sessions are kept in memory only, so a
  restart of the console, `elevate service admind restart`, or of authd,
  signs everyone out.

The sign-in page says why you are back. The console holds 64 sessions at
once.

## Using the API

Whatever the console does to the machine, it does through an action: one
read or one change, such as `service.restart`, with a route in an API that
a script can call, and a command that it runs. The page, the API and the
command typed at a terminal do the same thing, so the console can show you
the command, the API call and the Ansible task for any action before you
trust it. The pages of this release have no buttons for the actions yet;
the API and `admin` are how to use them.

`admin actions` lists the actions:

```
$ admin actions
ID                 KIND    RUNS AS        ROUTE
service.list       read    the person     GET /api/1/services                    e.g. service list --json
service.explain    read    the person     GET /api/1/services/{name}/explain     e.g. service diskhealth explain --json
service.start      change  administrator  POST /api/1/services/{name}/start      e.g. elevate service diskhealth start
service.stop       change  administrator  POST /api/1/services/{name}/stop       e.g. elevate service diskhealth stop
service.restart    change  administrator  POST /api/1/services/{name}/restart    e.g. elevate service diskhealth restart
service.enable     change  administrator  POST /api/1/services/{name}/enable     e.g. elevate service diskhealth enable
service.disable    change  administrator  POST /api/1/services/{name}/disable    e.g. elevate service diskhealth disable
certificate.list   read    the person     GET /api/1/certificates                e.g. cert list --json
certificate.show   read    the person     GET /api/1/certificates/{name}         e.g. cert show this-machine --json
certificate.renew  change  administrator  POST /api/1/certificates/{name}/renew  e.g. elevate cert renew this-machine
```

`admin actions --json` prints the same list as JSON, with each action's
area, its error IDs, its examples, its `compares` (see "A call") and its
`input`, a JSON Schema of the arguments it takes, which a script can check
what it is about to send against. A read runs as the person who asks, with
their own rights. A change runs as an administrator.

### Making a token

A script comes in with an API token, a line of text that stands for you
with the scopes you give it and an end date. Make one at a terminal, as
yourself:

```
$ admin token add monitor --scope services:read --days 30
Password:
dkv_alice_6c1f0a...
This token is not shown again: keep it now.
```

`admin token add` asks for your password, which it does not show. A script
that makes a token gives the password as the first line of its standard
input. The token is shown once and is kept only as a hash: if it is lost,
make another. Keep it where only the script reads it, not in the script
itself. An account that may sign in to the console keeps 32 tokens at
most, each named with 1 to 32 letters, digits and hyphens. A token lasts 90 days
unless `--days` says another number, from 1 to 366.

A scope says what the token may do:

| Scope | What the token may do |
| --- | --- |
| `*` | Everything its owner may do |
| `services` | Read and change the services: the area's name |
| `services:read` | Read the services, and change nothing |

`--scope` may be given again, or take several scopes joined by commas.
Each action belongs to an area, which `admin actions --json` names. A read
needs the area, the area with `:read`, or `*`. A change needs the area or
`*`. A token is never more than its owner: a change that runs as an
administrator needs an owner who is an administrator, and the token then
makes it with no unlock, so give each token the fewest scopes its script
needs, and a script that only looks `:read`.

### A call

The script gives the token in one header, `Authorization: Bearer`, and
nowhere else: not in the address, and not with a session's cookie. This is
what `admin show service.list` gives for a read of the services, with the
console's address filled in and the token in `DUKOVO_TOKEN`:

```
$ export DUKOVO_TOKEN=dkv_alice_6c1f0a...
$ curl -sS -H "Authorization: Bearer $DUKOVO_TOKEN" https://duke-nas.local:9090/api/1/services
```

It answers the JSON that `service list --json` prints. The machine's own
certificate is not one that `curl` knows: give `curl` the certificate with
`--cacert`, after comparing its fingerprint as "The first visit" says.
`cert show this-machine --pem` prints it.

```
$ cert show this-machine --pem > console.pem
$ curl -sS --cacert console.pem -H "Authorization: Bearer $DUKOVO_TOKEN" \
    https://duke-nas.local:9090/api/1/services
```

A change is a `POST`, and a token whose scope is `services` or `*` and
whose owner is an administrator may make it:

```
$ admin show service.restart name=diskhealth
Command
elevate service diskhealth restart

API
curl -sS -X POST -H "Authorization: Bearer $DUKOVO_TOKEN" -H "Content-Type: application/json" -d '{}' https://duke-nas.local:9090/api/1/services/diskhealth/restart
...
```

It answers `{"done": true, "changed": true, "output": [...]}` with the
lines the command printed. `changed` is `false` when the change moved
nothing, as when a service that runs is started. The console reads what the
change moves, as you, before the command and after it, and the files it
writes, such as a service's file for enable and disable, and says `true`
when it cannot tell, as for a restart; the list's `compares` says which
actions can tell. A long action's task says `changed` when it is done, from
its files alone. A command that fails is answered as an error, with the
command's own words (ADM-123).

### An Ansible task

`admin show` gives the same action as a task for a playbook, which does
not use a token: it runs the command over SSH as an administrator, with
`become`:

```
$ admin show service.restart name=diskhealth
...
Ansible
- name: "Restart a service: diskhealth"
  ansible.builtin.command:
    argv:
      - "service"
      - "diskhealth"
      - "restart"
  become: true
```

Put it in a play's `tasks`. Ansible needs Python on the machine: see
[Python](python.md). That module cannot tell a change from none, so a
change task reports "changed" each time it runs, and a read has
`changed_when: false`. A module of Dukovo's own that tells a change from
none is a later addition.

### Your tokens

```
$ admin token list
NAME     SCOPES                       ENDS
monitor  services:read, certificates  2026-12-14
$ admin token remove monitor
The API token monitor is removed.
```

A token that has ended is listed with `(ended)` for 30 days and then
forgotten. Removing one takes effect at once: the next request that brings
it is refused. Renaming an account ends its tokens.

### What a script may meet

An error is JSON, `{"error": {"id", "title", "detail", "fix"}}`, and each
has a page in the support center.

- **The request is not right** ([ADM-108](../../errors/ADM-108.md)). A
  request brings a token and a session's cookie, more than one
  `Authorization: Bearer` header, a token that is not shaped like one, or
  arguments the action does not take. Send one header, `Authorization:
  Bearer dkv_...`, and a JSON body of the arguments the route takes.
- **The API token is not accepted** ([ADM-115](../../errors/ADM-115.md)).
  The token is mistyped or cut short, was removed, has ended, or its account
  may no longer sign in to the console. Check the whole token is sent, and
  make a new one if it has gone. Ten wrong tokens in a minute from one
  address stop it for the rest of the minute, with **Too many tries**
  ([ADM-105](../../errors/ADM-105.md)).
- **The API token may not do this** ([ADM-124](../../errors/ADM-124.md)).
  Its scopes do not name the action's area, or it was sent to a route that
  is not an action: a token reads and changes through the actions, and
  cannot sign in, unlock or make another token. The detail names the scope
  it needs; make a token with that scope.

Two more follow from the action itself. **A value cannot work**
([ADM-125](../../errors/ADM-125.md)) names the field and says what it may
be, and nothing was run. **The command did not work**
([ADM-123](../../errors/ADM-123.md)) holds the command's own words.

## What the console keeps

Each person's second steps are in one file kept by authd, which only root
reads, and which holds no secret in a form that signs anyone in: the
recovery codes, trusted browsers and tokens are kept as hashes. Removing an
account removes its file. Each console sign-in is in the sign-in
records, so `last` shows it, on the terminal `web`, with the
browser's address:

```
$ elevate last
alice    web          192.0.2.7        Sat Oct 31 12:50   still logged in
```

`elevate lastb` shows the wrong passwords and codes typed at the
console's sign-in. The audit log, `elevate log --audit`, has a line for
each sign-in and how it was proved, each wrong proof, unlock, and change
of a second step, and never a password, code or cookie.

Signing in to the console does not open your keyring, as signing in at the
machine's own console or over SSH does (see [Your keyring](keyring.md)), and
the machine's own sign-in and SSH ask no second step.

## If a second step is lost

- **The phone is lost, but you have recovery codes.** Sign in with a
  recovery code, press **Remove the app** on **Second step**, and add the
  app again on the new phone. Make new recovery codes if few are left.
- **A passkey is lost.** Sign in another way, **Remove** it, and add a new
  one.
- **Nothing works, and no recovery code is left.** Another administrator
  removes the account's second steps, trusted browsers and tokens by
  deleting its file:

  ```
  $ elevate rm /system/state/authd/console/bob.toml
  ```

  The account then signs in with its password, and adds a second step
  again. The machine's own console and SSH ask no second step, so an
  administrator who cannot sign in to the web console still signs in
  there, to do this (see [Users and administration](users.md)).

## Troubleshooting

Each message carries an ID, **ADM-101** to **ADM-125**, and the support
center has a page for each.

- **The browser cannot connect.** Check the address and the port. Check
  that you are on a network the console answers (`reach`), and that it
  runs: `elevate service admind status`. Its log says why it did not start:
  `elevate log -s admind`.
- **The certificate warning comes back.** The machine made a new
  certificate (see "The first visit"). Compare the fingerprint again.
- **This is not the name of this machine (ADM-114).** Open the console by
  the machine's name or one of its addresses, as its sign-in screen shows
  them.
- **The name or the password is wrong (ADM-101).** Check both, and wait
  for the answer.
- **That code is not right (ADM-102).** Wait for the app's next code, check
  the phone's clock, or use a recovery code.
- **The passkey was not accepted (ADM-103).** Open the console by the name
  the passkey was added at; touch the key and give its PIN or fingerprint
  when it asks.
- **This account may not use the console (ADM-104).** Ask an
  administrator for `elevate user set --console NAME`.
- **Too many tries: wait a moment (ADM-105).** Ten sign-in requests a
  minute from one address is the most, and so is ten wrong API tokens.
  Wait the seconds the page gives.
- **The console is locked (ADM-106).** Press **Unlock**.
- **This account can look but not change (ADM-122).** The change runs as
  an administrator and this account is not one. Ask an administrator to
  make it, or to make this account one (`elevate user set --admin NAME`).
- **This session has ended (ADM-107).** Sign in again.
- **The request did not come from the console's page (ADM-109).** Reload
  the page. A script uses an API token, not a session's cookie (see "Using
  the API").
- **The API token is not accepted (ADM-115)**, **The API token may not do
  this (ADM-124)**, **The request is not right (ADM-108)**, **A value
  cannot work (ADM-125)** or **The command did not work (ADM-123).** See
  "What a script may meet", in "Using the API".
- **authd did not answer (ADM-111)**, **The session's worker did not
  answer (ADM-112)** or **The console could not do that (ADM-121).** Try
  again. If it keeps happening, `elevate log -s admind` and `elevate log -s
  authd` say why.
- **The console is full (ADM-120).** It holds 64 sessions. Try again when
  someone has signed out.
- **The page is blank or has no style.** The console serves a file only if
  it matches its list of checksums, and the log of admind names each that
  does not (ADM-116). The system's files were changed or damaged: update
  the system or restore them (see [Updates, snapshots, backups and
  recovery](updates.md)), then `elevate service admind restart`.

An error whose ID is not here has its own page; the number is the same in
the message and the file name.

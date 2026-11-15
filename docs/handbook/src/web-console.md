<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# The Dukovo Admin web console

Dukovo Admin is the machine's web console: pages in a browser, served by
the machine itself, for the people who run it. Everything in the free
console stays free, and nothing in it ever moves behind a paywall. This
chapter covers how to reach it, sign in, add a second step, unlock the
console to make changes, use its API from a script, see what changed on
the machine and put a change back, follow a long task, set your own
preferences, and what to do when something does not work. The references
are dukovo-admin(7), admind(8), admin(8), admin.toml(5), authd(8) and
user(8); the decisions are ADR 0514, ADR 0515 and ADR 0516.

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
| See what changed on the machine, and who changed it | `https://NAME.local:9090/api/1/changes` in a browser that is signed in; see "What changed" |
| Put a change back | `elevate admin put-back ID` |
| Keep the settings in your own Git | `admin export DIR` |
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
area, its error IDs, its examples, its `compares` (see "A call"), its
`long` (whether it runs as a task: see "Tasks") and its `input`, a JSON
Schema of the arguments it takes, which a script can check what it is about
to send against. A read runs as the person who asks, with
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
`changed_when: false`. A read whose command answers with another status
too, such as `service NAME explain`, which ends with 3 for a service that
does not run well, registers its result and has a `failed_when` that names
the statuses it answers with. A module of Dukovo's own that tells a change from
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
  is not an action or a task: a token reads and changes through the
  actions, and reads and stops tasks, but cannot sign in, unlock, read the
  journal or make another token. The detail names the scope it needs; make
  a token with that scope.

Two more follow from the action itself. **A value cannot work**
([ADM-125](../../errors/ADM-125.md)) names the field and says what it may
be, and nothing was run. **The command did not work**
([ADM-123](../../errors/ADM-123.md)) holds the command's own words.

## What changed

When something stops working, the first questions are what changed, when,
and who changed it. The console keeps a journal that answers them. A change
made through the console and an edit made by hand to the machine's settings
each have a row: when, who, how, which files, and what each file was before
and after.

The pages of this release do not show the journal yet. The API serves it,
and `admin put-back` puts a change back from a terminal. The page for it,
**What changed**, with its buttons **Keep it** and **Put back**, calls the
same routes. A browser that is signed in to the console reads the journal
at its address, as the JSON the route answers:

```
https://duke-nas.local:9090/api/1/changes
https://duke-nas.local:9090/api/1/changes/41
```

The first lists the newest changes first: 50 of them, or as many as
`?limit=N` says, up to 200, and `?since=T`, a Unix time, leaves out the
older ones. The second is one change, with a diff for each file it changed
and the command that would put it back. Everyone who signs in reads the
journal, locked or not. A script's token does not
([ADM-124](../../errors/ADM-124.md)), since the journal names people and
files.

### What a row says

| Field | What it says |
| --- | --- |
| `at` | When the change ended, or was found, as a Unix time |
| `who` | The person who asked; the owner of the token for a script; for a hand edit, the person found, or `outside the console` |
| `how` | `console`; `token monitor` for the token of that name; or `by hand` |
| `action`, `title` | The action, such as `service.enable`, and its title; empty for a hand edit |
| `argv`, `status` | The command the change ran, and the status it ended with |
| `task` | The task the change ran as, for a long action (see "Tasks") |
| `note` | What else there is to say: why a command did not end, which change a put back puts back |
| `files` | Each file the change names or changed: its `path`, and what it was `before` and `after` |
| `seen` | Who kept a hand edit, and when (see "Keep it") |

A file's `before` and `after` each have a `state`. `text` comes with the
file's SHA-256 and its size, and the console keeps the text, so that one
change's answer has the diff. `absent` is a file that was not there. `large`
is a file longer than 1 MiB, with its hash and size and no text. `secret`
and `unreadable` are files whose text the console does not have (see "What
the journal leaves out"). The diff of a file that changed from `private` to
`any` reads:

```
--- /system/settings/admin.toml (before)
+++ /system/settings/admin.toml (after)
@@ -6,4 +6,4 @@
 [console]
 listen = 9090
-reach = "private"
+reach = "any"
 certificate = "this-machine"
```

A diff is at most 2,000 lines in all for one change. A file that has no diff
says why, as `no_diff`: it is as it was, it is not text, its text is not
kept any more, or it is one of those that are kept out.

A change made through the console has its row once its command has ended,
whether it worked or not. A change that was refused, because the console
was locked, the account is not an administrator or a token's scopes do not
cover it, changed nothing and has no row; `elevate log -s admind` says it.
A change that works and moves nothing, such as starting a service that
runs, has a row too.

### Changes made by hand

Someone edits `/system/settings/admin.toml` over SSH, a script writes a
service's file, an administrator at the machine's own console fixes a
setting. The console is not told: Dukovo has no notification of a file's
change. So every 30 seconds it looks. It compares the size and the time of
change of each file under `/system/settings`, 8 folders deep, and in each
folder where an action's files are, 4 deep, up to 10,000 files, with what
they were at its last look, and reads only the files that differ: a few
hundred stats a round. A file that was made, changed or removed, with no
console change under way on it, has a row whose `how` is `by hand`. Names
that begin with a dot, which an editor's work file has on its way to its
name, and links are not looked at.

The console finds who by the audit log, which only root reads and which
the console asks the log service for. The person is the one whose `elevate`
command named the file's path in the 10 minutes before the file changed:
`elevate nano /system/settings/admin.toml` does, and if more than one did,
the last. When none did, the row says `outside the console`. That is an
edit with `service NAME edit`, which names the service and not the file's
path, an editor run as root at the machine's own console, and a file copied
in. It is the audit log's best answer, not proof: an administrator who edits
a file without naming it is "outside the console", and a write by root
soon after a person's command that named the file is put down to that
person.

A file that is changed and changed back between two looks is not seen, and
two changes between two looks are one row. The first look at a new store
only notes the files, and makes no row.

### What the journal leaves out

Everyone who signs in reads the journal, so it holds no secret. A file
under a service's `secrets` folder, a file that not everyone may read (no
read permission for others), and a file in a folder that others may not
pass through are listed by name, with no text, no hash and no diff. The
console runs as root, so it finds each file from the root a part at a time
and never through a link, and it does not watch links: a link that a
service's account puts in its own folder leads it nowhere. A file longer
than 1 MiB is listed by its hash and size, and a file that the console could
not read, or that is not a regular file, as `unreadable`.

### Keep it

A hand edit is news to everyone else who runs the machine. **Keep it** says
that you have looked and that it is as it should be. It changes nothing on
the machine: the row shows who kept it and when, and the first person to
keep it is the one the row names. It needs an unlocked console, which only
an administrator's can be. A change made through the console is kept
already, and **Keep it** on it is
[ADM-128](../../errors/ADM-128.md). The pages of this release have no
**Keep it** yet: it is `POST /api/1/changes/41/keep`, which a page's session
makes.

### Put back

**Put back** writes the earlier text of each file a change changed. It is
an administrator's, with the console unlocked, and the console runs `admin
put-back 41` for you, as an administrator, so that the command a page shows
is the command that runs. At a terminal you run it yourself, which asks for
your password as `elevate` does:

```
$ elevate admin put-back 41
put back /system/settings/spark/services/diskhealth.toml
Spark reads diskhealth's service file when diskhealth next starts: `service diskhealth restart` puts it to use now
change 41 is put back: 1 file
```

It says what it did for each file:

- `put back PATH` for a file it wrote.
- `PATH holds the earlier text already` for a file that reads as it did.
- `; it was changed again after change 41, and that later text is gone`
  after a file that someone changed again after the change: the earlier
  text is written over it.
- For a service's file, which is Spark's, it only writes the file and says
  that Spark reads it when the service next starts. `service NAME restart`
  puts it to use now. Put back puts back text, not what is running: a
  service stays as it runs until it next starts.
- For a bundle service's data file, in `/shared/services`, each service that
  uses it is asked to read it again, as `service NAME reload` does, or the
  line says that the service does not run, and so reads the file when it
  starts.

It keeps each file's owner and mode; a file that it has to make again is
mode 0644. A put back that cannot be done writes nothing, and says why
([ADM-128](../../errors/ADM-128.md)): a file the change made has no earlier
text; a secret, a file not everyone may read, a file the console could not
read and a file longer than 1 MiB have none kept; the console keeps the last
20 versions of a file, so an old change may have lost its text; the account
files, `/system/settings/passwd` and `group`, are written only with each
account's password and home, through `elevate user`, and a change to them
is a row of its own; and a change that changed no file has nothing to put
back. A change's answer says
beforehand, in `put_back`, whether it can be put back, and why not.

A put back made through the console has a row of its own, which says which
change it puts back and can be put back in turn. Typed at a terminal, the
files it writes are hand edits, found at the next look.

### How long it is kept

The journal keeps the newest 10,000 changes, and the text of the last 20
versions of each file, up to 64 MiB of text in all, the oldest going first.
A change that has lost its text can still be read; it cannot be put back. A
change that is older than the journal keeps is
[ADM-127](../../errors/ADM-127.md). For a longer history, `admin export`
copies the settings into a Git repository of your own:

```
$ admin export ~/duke-nas-settings
37 files copied to /users/alice/duke-nas-settings
2 secrets left out, since not everyone may read them: listed in SECRETS.md
committed 3f2a1bc: Settings of duke-nas at 2026-11-14 08:00:00 UTC
```

It runs as you, with no `elevate`, and needs `git`. A new folder, or an
empty one, becomes the repository. Each export is one commit, "Settings of
NAME at TIME", and none when nothing changed. The files are at the paths
they have on the machine, under `system/settings`. A file that not everyone
may read is left out by name, whatever you may read yourself, and so is a
file or folder you cannot read: `SECRETS.md` lists them and says why, and
the repository never has their contents.

## Tasks

Most actions answer in a moment. Some take minutes or hours, such as an
update or the backup of a whole machine, and a request cannot wait that
long: you close the browser, or lose the network, long before. An action
like that is **long**, and `admin actions --json` says which are, in its
`long`. A long action answers at once with a task number, `202` and
`{"task": 42}`, and the console runs its command on its own, as an
administrator, with no 30 seconds' limit. The work goes on without the page,
shows from another browser, and can be stopped. No action of this release
is long yet.

The pages of this release have no **Tasks** drawer yet. The drawer lists
the tasks that wait or run and those that ended lately, each with its step
and bar, and stops one; it calls the routes below.

A task is in one of five states:

| State | What it is |
| --- | --- |
| `waiting` | Asked for, and waiting for a place: at most 32 commands run as an administrator at once, for all sessions and tokens together, tasks included |
| `running` | Its command runs |
| `done` | Its command ended with status 0 |
| `failed` | Its command ended with another status, could not start, or ran out of its 12 hours |
| `stopped` | Someone stopped it, or the console restarted |

Tasks wait for a place in the order they were asked for. At most 64 wait;
another is **Too many tries**
([ADM-105](../../errors/ADM-105.md)), with `Retry-After`, 30 seconds.

A task shows where it is. Its command says so with lines on its standard
output: `step 2 of 4: Copying the files` is the step, which the drawer shows
as "Step 2 of 4: Copying the files", and `progress 40%` is how far the step
has come, which it shows as a bar. Every other line the command writes, and
everything on its error output, is its log, of which the console keeps the
last 2,000 lines.

`GET /api/1/tasks` lists the tasks that wait or run and those that ended in
the last 24 hours, newest first (`?since=T` and `?limit=N` as for the
journal). `GET /api/1/tasks/42` is one task with its `log`. A task has
`state`, `step` and `steps`, `words`, `percent` (empty while the command has
said none), `started` and `ended`, `who` and `how`, `status`, `note` (why it
is as it is, when the status does not say) and `changed`. A page that is
open is told of each change by `event: task` on its stream, at most four
times a second for each task, and a page that opens is told where each task
is.

`changed` says whether a task that is `done` changed anything, as a short
change's answer does, from its files alone: `false` only when its action
names files and each reads the same after it as before it, and `true` when
it names none, since a task runs no read before and after. It is empty
until the task is done, and for a task that did not end well. The journal
has the task's change, with the task's number, when its command ends.

### Stopping a task

The person who asked for a task, or an administrator, stops it, with the
console locked or unlocked: a stop only has the command put back what it
did. The command is sent SIGTERM, as ^C sends it at a terminal, and puts
back what it did. A task that still waits just leaves the queue. A command
that has not ended 30 seconds later is hung up, and one that has not ended
10 seconds after that is let go. A task whose command was sent SIGTERM to
stop it is `stopped`, whatever status the command ended with. A task that
has ended cannot be stopped ([ADM-128](../../errors/ADM-128.md)).

A task may run 12 hours. A command that is still running then is stopped
in the same way, and its task is `failed`: a command that hangs would
hold one of the 32 places through a night and the working day that
follows. Restarting the console, `elevate service admind restart`, ends the
commands of the tasks that run, since authd ends the commands of a console
that has gone. Their tasks are `stopped`, with the note "the console
restarted".

A task is kept for 30 days after it ends, and then forgotten; the journal
keeps its change.

### Tasks from a script

A token reads the tasks of the actions whose area its scopes let it read,
which is how a script that asked for a long action finds how it went. A
task of another area is not there for it
([ADM-127](../../errors/ADM-127.md)). A token stops a task when its scopes
let it change the action's area and its owner is the task's person or an
administrator. It never stops a task whose action the console no longer
has, since there is no area to check its scopes against
([ADM-124](../../errors/ADM-124.md)).

## Your preferences

The console's colours, the size of its text, whether things move and the
time zone it shows times in are yours, not the machine's. They are in
`~/.config/dukovo-admin/settings.toml`, a file only you read (mode 0600, in
a folder that is 0700), which the console writes as you:

```
theme = "dark"
text = 100
motion = "full"
zone = "Europe/Berlin"
```

| Key | Values |
| --- | --- |
| `theme` | `"dark"` (the default), `"light"`, `"high-contrast"`, or `"device"`, which follows the device's own |
| `text` | The size of the text in percent: `100` (the default), `112`, `125` or `150` |
| `motion` | `"full"` (the default) or `"less"` |
| `zone` | A time zone of the machine's own database, such as `"Europe/Berlin"`, or `""`, the default, for none |

You may edit the file by hand; a comment you write stays when the console
writes the file. A key that is missing, or has a value that is none of
these, reads as its default, so a typo costs a preference and never the
page. A file that is not TOML is said so, with its line, and left as it is.
`GET /api/1/account/preferences` reads them, and `POST` changes any of them
and answers all four. A session may change them locked or unlocked; a
script's token may not. The pages of this release do not use them yet. They
are not in the journal.

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

What changed and the tasks are in the console's own store, one file,
`/system/state/admind/store.db`, which only the console writes and only
root reads (mode 0600). `admin put-back` reads it as root while the console
has it open. It keeps no more than this:

| What | How much |
| --- | --- |
| Changes in the journal | The newest 10,000 |
| The versions of a file | The text of its last 20, at most 1 MiB of each, and 64 MiB of text in all, the oldest going first |
| A task | For 30 days after it ends, with the last 2,000 lines of its log, at most 256 KiB |

It holds no secret. A file under a service's `secrets` and a file that not
everyone may read are named in the journal and never read, and a file
longer than 1 MiB is kept by its hash and size. Restarting the console
loses nothing in it: the history and the tasks that ended are there when it
starts again. If the console cannot open the file when it starts, it still
signs people in and runs the actions, but no change has a row, an action
that runs as a task is not started, and the journal and the tasks answer
**The console keeps no journal or tasks now**
([ADM-126](../../errors/ADM-126.md)); `elevate log -s admind` says why.
Your own preferences are not in it: they are in your file,
`~/.config/dukovo-admin/settings.toml`.

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

Each message carries an ID, **ADM-101** to **ADM-128**, and the support
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
  Wait the seconds the page gives. A long action asked for while 64 tasks
  already wait gets it too, with `Retry-After`, 30 seconds (see "Tasks").
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
- **The console keeps no journal or tasks now (ADM-126).** The console
  could not open its store when it started, so "What changed" and the
  tasks are not there and an action that runs as a task is not started.
  Everything else works. `elevate log -s admind` says why (see "What the
  console keeps").
- **The console has no such change or task (ADM-127).** The change or task
  is gone: the journal keeps the newest 10,000 changes and a task is kept
  30 days after it ends. Reload the list.
- **That cannot be done to this change or task (ADM-128).** Keep it is
  for a change made by hand, a change cannot be put back when a file it
  names has no earlier text kept, and a task that ended cannot be stopped.
  Nothing was changed. The detail says which (see "Keep it", "Put back" and
  "Stopping a task").
- **The page is blank or has no style.** The console serves a file only if
  it matches its list of checksums, and the log of admind names each that
  does not (ADM-116). The system's files were changed or damaged: update
  the system or restore them (see [Updates, snapshots, backups and
  recovery](updates.md)), then `elevate service admind restart`.

An error whose ID is not here has its own page; the number is the same in
the message and the file name.

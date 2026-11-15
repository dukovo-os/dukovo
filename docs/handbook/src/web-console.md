<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# The Dukovo Admin web console

Dukovo Admin is the machine's web console: pages in a browser, served by
the machine itself, for the people who run it. Everything in the free
console stays free, and nothing in it ever moves behind a paywall. This
chapter covers how to reach it, sign in, add a second step and unlock the
console to make changes; how to find your way, with the header, the
navigation, the keys and the command palette; the first sign-in of a new
account and your own preferences; the Overview; the Logs; what changed on
the machine, and how to put a change back; showing the command of an action,
and using the API from a script; following a long task; the network cards
and the firewall; the disks and the log's own space; the snapshots and the
backups; moving to new hardware; the terminal; and what to do when something
goes wrong or does not work. The references are dukovo-admin(7), admind(8),
admin(8), admin.toml(5), authd(8), user(8), who(1), last(1), log(1),
network(8), network(5), firewall(8), storage(8), diskhealth(8), scrub(8),
logtrim(8), snap(1), backup(8), snap.toml(5) and backup.toml(5); the decisions
are ADR 0514, ADR 0515, ADR 0516, ADR 0517, ADR 0461, ADR 0463 and ADR 0561.

| Task | How |
| --- | --- |
| Open the console | `https://NAME.local:9090` in a browser, with the machine's name |
| Find its address and fingerprint | The machine's sign-in screen, `admin fingerprint`, or `cert show this-machine` |
| See at a glance whether the machine is all right | The Overview, the first page after you sign in; see "The Overview" |
| Set a new account up | The welcome pages that come before the first page; see "Your first sign-in" |
| Find a page, or run an action by its name | `Ctrl+K`, or **Search or run a command** in the header; see "The command palette" |
| See the keys | `Ctrl+/`; see "Finding your way" |
| Let someone who is not an administrator sign in | `elevate user set --console NAME` |
| Take that away again | `elevate user set --no-console NAME` |
| Ask everyone for a second step | `second-step = "everyone"` in `/system/settings/admin.toml` |
| Open a shell in the browser | **Terminal** in the list of pages; see "The terminal" |
| Open root's shell in the browser | **New administrator terminal**, with the console unlocked; see "The terminal" |
| Read what the console did | `elevate log -s admind` and `elevate log --audit` |
| Read the machine's log in the browser | **Logs** in the list of pages; see "The Logs" |
| Watch the log as lines come | **Follow new lines** on the Logs page; see "Following the log" |
| Keep lines of the log as a file | **Save as file...** on the Logs page; see "Saving lines" |
| Find what happened at a moment on a chart | **Open CPU** and the others, under the Overview's charts; see "From a chart to the logs" |
| See what the console can do | `admin actions` |
| See the command, API call and Ansible task of an action | `admin show service.restart name=nginx`, or `Tab` on its row in the palette; see "Showing the command" |
| Make a token for a script | `admin token add NAME --scope services:read` |
| See what changed on the machine, and who changed it | `https://NAME.local:9090/api/1/changes` in a browser that is signed in; see "What changed" |
| Put a change back | `elevate admin put-back ID` |
| Follow a long task | **Tasks** in the header; see "Tasks" |
| See why a service is not running, and start, stop or restart it | **Services**, then the service; see "Services" |
| See the network cards, or change a card's address | **Networking**, then **Change…**; see "Networking" |
| Allow or deny a port, or see what the firewall blocked | **Firewall**; see "Firewall" |
| See the disks, how full they are and when they will be full | **Storage**; see "Storage" |
| Check a disk's health | **Check now** on the disk's page under **Storage**, or `elevate diskhealth` |
| Check every block of a mirror | **Scrub the mirror** on a disk's page, or `elevate scrub start /volumes/data` |
| Use the room after a disk's last partition | **Grow into it** on the disk's page, or `elevate storage grow sdb`; see "Growing a disk" |
| Change how long the log is kept, or trim it now | **The log's own space**, at the bottom of **Storage**, or `elevate logtrim` |
| See which files have a snapshot, and a copy off the machine | **Snapshots and backups**; see "Snapshots and backups" |
| Bring back a file from a snapshot | **Browse** on a snapshot, tick the file, **Restore N chosen**; see "Bringing back a file" |
| Take a snapshot now | **Take a snapshot now** on the Snapshots and backups page, or `elevate snap create PLACE` |
| Copy the machine's data somewhere else | **Add a backup place…**, or `elevate backup add NAME DESTINATION`; see "Backup places and the recovery key" |
| Back up now, or check a backup by restoring it | **Back up now** and **Check now** on the backup place, or `elevate backup run NAME` and `elevate backup verify NAME` |
| See the recovery key of a backup place again | **Show it again** on the backup place, or `elevate backup key NAME` |
| Find out why a backup is overdue or failed | The banner at the top of **Snapshots and backups**; see "When a backup is overdue or failed" |
| Bring a machine's name, accounts, apps and files to new hardware | **Move to new hardware**, or `elevate backup restore --machine`; see "Move to new hardware" |
| Give up a restore that is staged before the restart | **Cancel** on **Move to new hardware**, or `elevate backup restore --machine --cancel` |
| Undo a network or firewall change that cut the console off | Wait for the time the banner shows, or `elevate network undo` or `elevate firewall undo` at the machine |
| Keep the settings in your own Git | `admin export DIR` |
| Find out what an error means | **Help**, then **What error messages mean**; see "When something goes wrong" |
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
**Second step** in the console, from **Manage the second step** under
**Your account**, lists what you have and changes it. Each change asks for
your password.

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

An administrator terminal ends with the unlock, and **Lock** asks first
when one is open (see "The terminal").

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

**Sign out**, in the account menu under your name, ends the session. When
terminals run, it asks first: **Keep them running**, **End them and sign
out** or **Cancel** (see "The terminal"). A session also ends

- 12 hours after the sign-in, by default (`session-hours`, 1 to 168);
- 30 minutes after the last console page of it was closed, by default
  (`idle-minutes`, 5 to 1440);
- when the account may no longer sign in, as when it is disabled;
- when the console starts again. Sessions are kept in memory only, so a
  restart of the console, `elevate service admind restart`, or of authd,
  signs everyone out.

The sign-in page says why you are back. The console holds 64 sessions at
once. The end of a session leaves your own terminals running, except when
the console starts again, which ends every terminal (see "The terminal").

## Finding your way

The console's pages sit in one frame: a header along the top, the list of
pages at the left, and the page. The frame stays while the page inside it
changes, so the search, Tasks and the lock are where they were. The first
stop of the Tab key is **Skip to the page**, which goes past the frame,
and a new page puts the keyboard on its own heading. The browser's tab is
named for the page and the machine, "Overview · duke-nas · Dukovo Admin",
so that two consoles can be told apart. A page that is slow to come says
**Loading…** in its place.

### The header

From the left:

| Part | What it does |
| --- | --- |
| **Pages** | On a window of middle width only: opens the list of pages over the page (see "The navigation") |
| **Dukovo Admin** | Goes to the Overview |
| The machine's name | Opens a small window with the machine's addresses, each with its port, and its certificate's fingerprint, each with **Copy**. **Esc**, or a press outside it, closes it |
| **Search or run a command**, `Ctrl+K` | Opens the command palette (see "The command palette") |
| **Limited access** or **Unlocked** | The lock (below) |
| **Tasks** | Opens the Tasks drawer (see "The Tasks drawer") |
| **Help** | The Help menu (below) |
| Your name | The account menu (below) |

The lock says what the session may do. An administrator's begins as
**Limited access ·** with **Unlock** after it (see "Unlocking the
console"). Anyone else's says **Limited access** and has no button: the
account looks, and changes its own things, and that is all. Unlocked, the
lock says **Unlocked · 14 min** and counts down, with **Reset to 15 min**
and **Lock** beside it. The count changes its look at 2 minutes left, and a
note says **The console locks in 2 minutes** with **Reset to 15 min** in
it, which stays until you close it or press the button. **Lock** asks first
only when administrator shells would end with it, as **Lock and end 1
administrator shell?**: **Lock and end** goes on and **Cancel** does not
(see "The terminal").

**Tasks** has a number beside it: how many tasks wait or run. When a task
has failed or been stopped since you last opened the drawer, the number is
how many have, and it changes its look; the button says so to a screen
reader, "Tasks, 1 needs a look". On a phone the button is there only while
a task runs or needs a look (see "The Tasks drawer").

**Help** has **Keyboard shortcuts**, **What error messages mean**, which
goes to the pages that say what each ID means (see "When something goes
wrong"), and **About Dukovo Admin**, which shows the system's release, the
console's build and the licence of its files.

The account menu is under your name. It has **Your account**, which holds
the display, the time zone, the second step, the command that makes an API
token and the terminal's settings; **Dark**, **Light**, **High contrast**
and **Same as this device**, under **Display**, which change the colours
at once (see "Your preferences"); and **Sign out**. When terminals run,
**Sign out** asks first (see "Signing out, and when a session ends").

### The navigation

The list of pages shows the machine's name above it, then one entry for
each page the session may use: **Overview**, **Logs**, **Reliability**,
**Networking**, **Firewall**, **Services**, and **Terminal**, which an
account that may not open one does not see (see "The Terminal entry is
missing"). The page you are on is marked, and the entries under it show
while you are in it: **Services** has **Scheduled tasks**, and **Terminal**
has **Sessions** and **Terminal settings**. A number beside **Sessions**
says how many terminals you have, and a screen reader says it with the
name, "Sessions, 2 running". The number is read when the console opens,
when a task or a change comes (at most once in 30 seconds), and every 5
minutes while the tab is in view.

A page you ask for that the console does not have says **The console has
no page at** the address, with **Go to the Overview** and **Search or run a
command**. A page that is for administrators says **This page is for
administrators.** to someone who is not one.

How the frame is laid out depends on the width of the window, in units of
the text's size. At the 100% text size:

| Width | What you see |
| --- | --- |
| 1100 pixels and more | The list of pages stays at the left, and Tasks opens beside the page |
| 720 to 1099 pixels | **Pages** in the header opens the list over the page, and Tasks opens over it too |
| Under 720 pixels, a phone | A bar of tabs along the bottom, **Overview** and **More**. The header keeps the mark, the machine's name, a magnifying glass for the search, the lock as an icon, Tasks while a task runs or needs a look, and, while a change is tried, a warning mark with the time left. A drawer fills the screen |

A larger text size makes the narrower layouts come at a wider window.

**More** opens a sheet that lists **All pages**, then the account menu's
items and the Help menu's, so that the phone has everything the wide header
has. **Terminal** is there. The lock icon unlocks when you press it; when
the console is unlocked it offers **Reset to 15 min**, **Lock** and
**Close**, and for someone who is not an administrator it says **Your
account can look, and change your own things**.

### The keys

| Keys | What they do |
| --- | --- |
| `Ctrl+K`, or `Meta+K` | Opens the command palette, and closes it again |
| `Ctrl+/` | Shows these keys: **Keyboard shortcuts** in the Help menu does the same |
| `Esc` | Closes the dialog, drawer, menu or note in front |
| Arrow keys, `Home`, `End` | Move in a menu, a list of tabs or the search results |
| `Alt+Shift+K` | Opens the command palette from inside a terminal |
| `Alt+Shift+Up` | Leaves a terminal's pane for the terminal's tabs |

None of them is a key of one character, so nothing is done by a key pressed
by mistake while typing. Inside a terminal the program gets every key,
`Ctrl+K` and `Ctrl+/` too, and only the two with `Alt+Shift` are the
console's. The terminal has keys of its own, which "The terminal" lists.
One drawer is open at a time, Tasks, a command or a page's help, and `Esc`
closes it and gives the keyboard back to where it was.

### Notes

A note is a short message that the console puts at the bottom right of the
window, or above the tab bar on a phone, after something you did or
something that came: **Stopping task 42.** or **The console locks in 2
minutes**. At most 3 show at once, the newest last. One that has no button
goes away after 8 seconds, and stays while the pointer is on it or the
keyboard is in it. One that has a button, such as **Open Tasks**, stays
until you press the button or **Close**, which every note has; the one that
says a task runs goes when the task ends, and the note that it ended takes
its place. A screen
reader hears a note politely, without taking the keyboard.

## The command palette

The palette finds a page, an action, a service, a certificate or a terminal
by a few of its letters, and does what Enter asks of it. Press `Ctrl+K` (or
`Meta+K`) on any page, or **Search or run a command** in the header. It
opens over the page with the cursor in its box. `Esc` closes it, and so
does `Ctrl+K` again. Inside a terminal the key is `Alt+Shift+K` (see "The
terminal").

### What it finds

With nothing typed it lists up to 50 rows, the ones you used last first, in
**Recent**, and the rest in their groups:

| Group | What is in it |
| --- | --- |
| Recent | The last 8 rows you opened or ran, kept for your account in this browser, and only there |
| Actions | Each action the console has, by its title, such as **Restart a service**. Each service has its own rows to start, stop, restart, enable and disable it (**Restart nginx**), and each certificate one to renew it (**Renew this-machine**); these show once you type, or when they are in **Recent**. The console's own services, `admind` and `authd`, have none: an action that would end the console is refused |
| Services | Each service Spark runs, with its state: **Failed**, **Running**, **Stopped**, and so on, failed ones first |
| Certificates | Each certificate certd keeps, with its state and when it ends |
| Terminal | **New terminal**, **New administrator terminal** for an administrator, and each terminal you have, by its name and how many panes it has |
| Settings and pages | Each page, and the parts of a page, as **Terminal › Sessions** or **Your account › Time zone** |

A page is not found by someone who may not use it: an administrator's page
is not in a reader's list, and **Terminal** is not in the list of an account
that may not open one.

Type a few letters. Each word you type has to be found in a row's title,
letter by letter and in order, though not side by side, so `rest ngi` finds
**Restart nginx**, and the words may come in any order. Capitals and accents
do not count. A letter that starts a word of the title counts most, and a
title that starts with what you typed goes first. A row's other words, such
as a service's description, count too, for less. The best match is first;
of two as good, the one you used lately, then a page, then an action, then a
service, a certificate or a terminal, then the shorter title. The letters
that matched are marked. At most 50 rows show, and the line under
the list says how many there are, "12 results" or "No results".

A word such as `kind:service` or `state:failed` does not search but keeps
only the rows that fit: the start of a word is enough, `state:fail`. The
kinds are `page`, `action`, `service`, `certificate` and `terminal`. The
states are those of a service (`failed`, `running`, `stopped`, `disabled`,
`starting`, `pending`, `waiting`, `stopping` and `done`) and of a
certificate (`expiring`, which is 14 days or fewer left, `expired`,
`revoked` and `valid`). `state:failed` alone lists the services that failed.
The foot of the palette names two such filters.

If the list of actions cannot be read, the palette says so, **The list of
actions could not be read, so only pages and things are here.**, and still
finds those.

### What Enter does

The row in hand shows its command in one line under its title, when every
argument of it is known. Press `Enter` or press the row:

| The row is | Enter |
| --- | --- |
| A page, or a terminal | Goes there. **New terminal** opens a new one, and a terminal by its name opens it (see "The terminal") |
| A read with every argument known | Runs it as you and shows the answer in the palette: a table of its first 50 rows, or its facts, with the JSON of the answer under **The answer as JSON**. **Back** returns to the list |
| A service | Opens its page (see "Services") |
| A certificate | Shows it, as `cert show NAME` does |
| A change, or a read that needs an argument | Opens the form of the action, with the command it will run in one line under it, and **Run** for a change or **Show the answer** for a read |

`Tab` opens that same form for any action, with the panel that shows the
command, the API call and the Ansible task already open (see "Showing the
command"). `Esc` goes back from a form or an answer to the list, and closes
the palette from the list.

The form is made from the action's `input`, the JSON Schema that `admin
actions --json` lists. A text is a box with the length it may have; a
choice of four or fewer is radio buttons, and one of more is a menu that
starts at **Choose one**; a whole number is a number box; and a yes or no is
a tick. The action's own words are under each box, and a box that must be
filled has **(needed)** in its name. A value that is wrong is said when you
leave its box, and a value that only the console can tell wrong, as
[ADM-125](../../errors/ADM-125.md) does, is said under the box it is
about. An action that takes nothing says **This action takes no arguments.
Nothing to fill in.** An action whose arguments are described in a way the
form does not know has one box in place of them, for JSON, as the API takes
it.

A change goes through the same path as a button on a page would: the
console asks the question that the API asks. In the palette:

- **Needs Unlock** is on the row of a change that an administrator would
  make while the console is locked. **Run** asks you to unlock first, and
  the dialog says what for, **You are about to restart nginx.** The change
  goes on when the unlock is done (see "Unlocking the console").
- **Administrators only** is on the row for an account that is not an
  administrator. Its form says **bob's account can look but not change**
  and **This change needs an administrator.**, and has no **Run** (see
  [ADM-122](../../errors/ADM-122.md)).
- When the change is done the palette closes and a note says **Restart
  nginx: done.**, or **Restart nginx: nothing changed.** when it moved
  nothing, with the row's own title. A long action is a task: the note
  says **Restart nginx: running as task 42**, with **Open Tasks** (see
  "Tasks").
- A change that fails says why in the form, as a page does (see "When
  something goes wrong").

## Your first sign-in

A new account has no time zone yet, so the console asks before it shows
any page. It opens **Welcome to NAME**, which takes a few steps, one at a
time, with the list of them at its left. It keeps the page you asked for
and goes there at the end. It shows only while the account has no time
zone, so it does not come back once you have been through it or pressed
**Skip all**.

| Step | What it asks |
| --- | --- |
| Time zone | The zone to show times in. The page says that times are shown in your browser's zone, and the zone the machine's clock is in. **Change** opens a field for a place such as `Europe/Prague`, which the list completes, and **Use the server's** takes the machine's own. A zone the machine does not have is said under the field ([ADM-125](../../errors/ADM-125.md)) and nothing is kept. **Next** keeps the zone. |
| Display | The colours, the size of the text and whether things move. Each choice shows at once. **Next** keeps them, for every browser you sign in with. |
| Second step | Only for an account that has none: the cards for an authenticator app, a passkey and recovery codes, as the account's own page has them (see "Adding a second step"). |

**Back** goes to the step before, and **Skip for now** leaves a step as it
was. **Skip all** keeps the time zone that is shown, leaves the rest as it
is and ends the pages at once. Everything here can be changed later under
**Your account**.

An administrator is then asked **Set up NAME now?** when pages have steps
to set the machine up with. **Set up** goes to the page for them (see
"Setting the machine up"), and **Later** keeps them in **Finish setting
up** on the Overview. Everyone else goes to the page they asked for.

## Your preferences

The console's colours, the size of its text, whether things move, the
time zone it shows times in, whether pages show the command an action
runs, and how your terminals are, are yours, not the machine's. They are in
`~/.config/dukovo-admin/settings.toml`, a file only you read (mode 0600, in
a folder that is 0700), which the console writes as you:

```
theme = "dark"
text = 100
motion = "full"
zone = "Europe/Berlin"
commands = "hidden"

[terminal]
size = 14
scrollback = 10000
copy-on-select = false
keep-days = 7
announce = false
shortcuts = true
```

| Key | Values |
| --- | --- |
| `theme` | `"dark"` (the default), `"light"`, `"high-contrast"`, or `"device"`, which follows the device's own |
| `text` | The size of the text in percent: `100` (the default), `112`, `125` or `150` |
| `motion` | `"full"` (the default) or `"less"` |
| `zone` | A time zone of the machine's own database, such as `"Europe/Berlin"`, or `""`, the default, for none |
| `commands` | `"hidden"` (the default) or `"shown"`: whether pages show the command an action runs |
| `[terminal]` `size` | The size of the terminal's text in pixels, `12` to `20` (the default is `14`) |
| `[terminal]` `scrollback` | The lines of output the browser keeps for a terminal, `1000` to `50000` (the default is `10000`) |
| `[terminal]` `copy-on-select` | `true` to copy a selection as soon as you make it; `false`, the default |
| `[terminal]` `keep-days` | How many days a terminal that no page is attached to is kept before it ends, `1` to `30` (the default is `7`) |
| `[terminal]` `announce` | `true` to have a screen reader read new output; `false`, the default |
| `[terminal]` `shortcuts` | `true`, the default, for the terminal's Alt+Shift keys; `false` to turn them off |

You may edit the file by hand; a comment you write stays when the console
writes the file. A key that is missing, or has a value that is none of
these, or a number that is out of its range, reads as its default, so a
typo costs a preference and never the page. A file that is not TOML is said
so, with its line, and left as it is. `GET /api/1/account/preferences`
reads them, and `POST` changes any of them and answers all of them; the
terminal's keys are under `terminal` there, and a value out of range is
[ADM-125](../../errors/ADM-125.md) with the key's name. A session may
change them locked or unlocked; a script's token may not. The pages use
them as they are kept: a display choice shows at once and follows you to
every browser you sign in with, and times are shown in your zone. They are
not in the journal.

## The Overview

The Overview is the first page after you sign in, at `/`. It answers
"is this machine all right?" with a score and a short list, and below it
what changed lately, how the machine is used and what it is. Each card
reads for itself. A card that cannot read says why in its own place, with
**Try again**, and the others carry on. Times are in your time zone.

| Card | What it shows | Reads again |
| --- | --- | --- |
| Notices | What the machine kept for the next sign-in, such as a start that fell back to the earlier version | Every 5 minutes |
| Health | The score out of 100 as a ring, its band in words, and what needs a look | Every minute, and when a task or a change comes |
| Finish setting up | The steps that set the machine up, and how many are done | When the page opens |
| What changed | The newest five changes, with the buttons that fit each | Every 5 minutes, and when a change comes |
| Usage | Four charts: CPU, memory, disk and network | Every minute for the last hour, every 5 minutes for the rest |
| This server | What the machine says of itself, and your last sign-ins | Every 5 minutes |
| Using the most memory | The five services that hold the most, with their CPU | Every minute |

A notice is shown as the terminal shows it. **Dismiss** hides it in this
browser only. The same notice with other words is shown again.

### The health score

The score comes from the checks, which `dukovo-check` keeps the result of.
It starts at 100. Each thing the checks find takes off its weight, a check
that **FAILED** takes off twice its weight, and it never goes below 0. The
words under the ring say what its colour says:

| Score | Words |
| --- | --- |
| 90 and over | Good |
| 70 to 89 | Could be better |
| Under 70 | Needs you |

| Check | What it looks for | Takes off |
| --- | --- | --- |
| SEC-101 | A firewall rule lets every port in from everywhere | 10 |
| SEC-102 | The firewall is off or failed | 15 |
| SEC-110 | Passwords sign in over SSH | 6 |
| SEC-111 | SSH lets every account sign in with a password | 4 |
| SEC-112 | Blocking of repeated wrong passwords is off | 6 |
| SEC-120 | An account has an empty password | 20 |
| SEC-121 | No account is an administrator | 10 |
| SEC-130 | A newer release is waiting | 6 |
| SEC-131 | No check for updates in 7 days | 4 |
| SEC-140 | The clock has not synchronized in a day | 4 |
| SEC-150 | A service gave up after its restarts, each, at most 12 in all | 4 |
| SEC-160 | No snapshot schedule covers the people's and shared folders | 8 |
| MET-… | An early warning is active, each | 4 |
| HW-… | The last hardware check warned or failed, each | 6 |
| Any other | A check this table does not know yet | 3 |

**How the score works**, on the card, shows the same table and when the
checks last ran. The score is for administrators. A machine that has no
`dukovo-check` says **Checks are not on this server yet.** A machine whose
checks have not run says **Not checked yet.** and shows the command to run:

```
$ elevate dukovo-check
```

Under the ring is **N things to look at**, the first four of them, and **See
all N** for the rest. The list holds the checks that did not pass and what
the pages add: a service that gave up, a certificate that is revoked, has
ended or ends within 14 days, an administrator's account with no second
step, a task of the last 24 hours that failed or was stopped, a file changed
by hand that nobody has marked seen in 7 days, and a change that is being
tried, such as a service's file under `service try`, with the time it is
undone unless someone keeps it (see "Services"). The worst are first:
what **Needs you** before what is **Worth a look**. A check that names what
a page lists already, such as a service that gave up, is not listed twice.

### What changed

The card lists the newest five changes since the machine last started well,
or in the last 7 days when it does not say. Each row has its time, who made
the change and how, and a dot: a change that needs a look, one that did not
work, or one that was kept. The buttons are the journal's (see "What
changed" below):

- **See the change** shows who and when, and the diff of each file.
- **Keep it** is on a hand edit that nobody has looked at. It asks you to
  unlock first (see "Keep it").
- **Put back** is on a hand edit that can be put back. It says which files
  it writes back and shows the command, and **Cancel** leaves everything as
  it was (see "Put back").
- **Undo** is on a change made through the console that can be put back, and
  does the same as **Put back**.

**Keep it**, **Put back** and **Undo** are for administrators, and each asks
you to unlock the console first. Anyone who signs in can look at the rows.
**See all** opens the whole journal on the Reliability page (see "The
journal").

### Usage

The four charts show the last hour, 24 hours, 7 days or a year, as you
choose with the buttons beside the heading, or with the arrow keys once one
has focus. The choice is in the address, as `?range=7d`, so a link or a
reload shows the same one. A screen reader hears each chart as its average
and its peak, "CPU over the last 24 hours: average 12%, peak 72% at 2:14
AM". **Show the numbers** opens the points as a table. **Open CPU**, under a
chart, opens it to choose a moment on it and see what happened then (see
"From a chart to the logs").

The history is kept for two weeks at one-minute steps, and for a year as
hourly averages. A machine that has kept none yet says **No numbers yet**,
and whether `metricsd`, which writes them, runs.

### This server and what uses the most memory

**This server** shows the machine's name, its system and whether it fell
back to the earlier version, the machine and its processor and memory, how
long it has been up, the time in its zone and whether its clock is in sync,
your last two sign-ins, and the SSH host key. A fact the machine does not
say is left out. **Using the most memory** lists the five services that hold
the most, with their CPU and what they are. A service that is not running
holds none and is left out.

### On a phone

On a phone the Overview is short: the notices, Health with its ring beside
the words, the CPU and memory charts, and a line with how long the machine
has been up. The other cards are on a wider screen.

### Setting the machine up

**Finish setting up** shows how many of the steps that pages register are
done, such as **5 of 6 done**, with **Set up** on each that is not. It is
for administrators, and it is not there while no page has a step. **Set up**
goes to `/setup`, **Make NAME safe in a few minutes**, which is for
administrators too.

The page lists the steps at its left, each with where it is. A step shows
its fields, and the command it runs in a box, as the Show command panel
does (see "Showing the command"). **Set up** asks you to unlock if the
console is locked, and runs the action. A long action is a task: the page says
**Started. Its progress is in Tasks.** and marks the step done when the
task ends. Where
the checks have a score, a step says what it takes it to, such as **This step
takes the score from 62 to 70.** **Skip for now** goes to the next step, and
**Set up again** runs one that is done again. A step that is a page of its own
has **Continue on its page** in place of **Set up**: **Bring everything from your
old machine** is offered on a machine that has no backup place and one person's
account, and goes to Move to new hardware. The Snapshots and backups page
gives the step **Copy your data off this server** too. A machine whose pages
have no steps says **This server has nothing to set up yet.**

## What changed

When something stops working, the first questions are what changed, when,
and who changed it. The console keeps a journal that answers them. A change
made through the console and an edit made by hand to the machine's settings
each have a row: when, who, how, which files, and what each file was before
and after.

The Overview shows the newest five changes, with the buttons **Keep it**,
**Put back**, **Undo** and **See the change** (see "The Overview"), and the
Reliability page shows all of them, with filters and **Copy as a script** (see
"Reliability"). The API serves all of the journal, and `admin put-back` puts a
change back from a terminal. A browser that is signed in to the console reads the journal at
its address, as the JSON the route answers:

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
[ADM-128](../../errors/ADM-128.md). On the Overview it is the button **Keep
it** on a hand edit's row, and the console asks you to unlock first. It is
`POST /api/1/changes/41/keep`.

### Put back

**Put back** writes the earlier text of each file a change changed. It is
an administrator's, with the console unlocked, and the console runs `admin
put-back 41` for you, as an administrator, so that the command a page shows
is the command that runs. On the Overview the button asks first, naming the
files and showing the command; **Cancel** writes nothing. At a terminal you
run it yourself, which asks for your password as `elevate` does:

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

## Showing the command

Whatever a button does, it does through an action (see "Using the API"),
and the console can show what the action runs before you trust it: the
command, as you would type it at a terminal; the call to the API, for a
script; and the Ansible task, for a playbook. They are one call in three
forms, so what you read is what runs. `admin show` gives the same at a
terminal, and the panel asks the console the same question, `POST
/api/1/show`, once for each action with its arguments.

The panel has four tabs under **Show as**:

| Tab | What it shows |
| --- | --- |
| **Command** | The command the action runs, as it is typed at a terminal. **Copy as Ansible task** is here as well |
| **API** | The `curl` call for the route, with the token in `DUKOVO_TOKEN` |
| **Ansible** | The task for a playbook, which runs the command over SSH with `become` (see "An Ansible task") |
| **Files** | The files the call writes. For each, the newest change of it among the last 200 rows of the journal, as a diff, or **No change of this file is in the journal.** **This call changes no file.** when it writes none |

**Copy** puts the tab's text on the clipboard and says **Copied.** If the
browser does not let the page copy, it says **Press Ctrl+C to copy.** with
the text selected. Under the tabs the panel says: "The API and Ansible tabs
are the same call, typed. What you see here is what runs."

You meet the command in four places:

- **In the palette.** The form of an action shows its command in one line
  under the fields once they are filled, and `Tab` on a row opens the
  form with the panel open (see "The command palette"). Press the line to
  open or close the panel under it. Until the fields are right, the panel
  says **Fill in the form to see the command.**
- **Under each step of the setup page.** The step's box, **This step runs**,
  holds the panel whatever the switch says (see "Setting the machine up").
- **On each task's card in the Tasks drawer**, in one line. Press it and
  **Show command** takes the drawer's place with the panel for that task's
  action and arguments (see "The Tasks drawer").
- **Under a button that runs an action, when the Show commands switch is
  on.**

The **Show commands** switch is in the head of the Overview, of the setup page
and of the page of a service. It keeps its state as the `commands`
preference, so it is the same on every browser you sign in with (see "Your
preferences"). When it is on, a button that runs an action shows its command
in one line under it, which opens **Show command** when you press it. The
Overview's buttons are the journal's, **Keep it**, **Put back** and **Undo**,
and not an action's, so the switch shows nothing under them: **Put back**
names the files and shows its command in its own question (see "Put back").

## Using the API

Whatever the console does to the machine, it does through an action: one
read or one change, such as `service.restart`, with a route in an API that
a script can call, and a command that it runs. The page, the API and the
command typed at a terminal do the same thing, so the console can show you
the command, the API call and the Ansible task for any action before you
trust it (see "Showing the command"). The command palette runs any action
by its name (see "The command palette"); the API and `admin` run them from
a script and at a terminal.

`admin actions` lists the actions. These are the first ten, the services'
and the certificates':

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
their own rights, except a read of what only an administrator may see, such
as the whole log or what the firewall blocked, which runs as an
administrator, as a change does (ADR 0518): a token reaches it only with
its area's full scope, never with `area:read`. A change runs as an
administrator.

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

### Checking a change first

`POST /api/1/check` takes what `show` takes, `{"action": ..., "args": ...}`,
for the same callers, and changes nothing. It runs the probe that the
action's `changed` is found from, as you, and reads the files the change
would write, and answers whether the change would do anything:

```
$ curl -sS -X POST -H "Authorization: Bearer $DUKOVO_TOKEN" -H "Content-Type: application/json" \
    -d '{"action": "service.start", "args": {"name": "diskhealth"}}' \
    https://duke-nas.local:9090/api/1/check
{"changed": false, "now": "running"}
```

`changed` is `false` when everything the change moves is as it leaves it,
`true` when it would move something (a restart always does), and `null`
when the console cannot tell: the probe failed, the action has no way to
compare, or a file could not be read. `now` is the part of the probe's
answer that the action compares, here the service's state, and `null` when
there is none. A read is always `false`. A check leaves no row in the
journal and no line in the log.

### What the API says of itself

`GET /api/1/openapi.json` answers a description of every route and action
as OpenAPI 3.1, to anyone, since it says only what this chapter and
admind(8) say. For each one it gives the method and address, the arguments
with their limits, the security it needs (a session with its token, or an
API token, for the routes that take one), and for each status the route can
answer the error IDs that come with it. A tool that reads OpenAPI, such as
a client generator, a mock server or a tester, can use it:

```
$ curl -sS --cacert console.pem https://duke-nas.local:9090/api/1/openapi.json > dukovo.json
```

The lab test `web-console/fuzz` sends requests made from this document and
nothing else to a machine, signed out and with tokens, and fails on any
answer the document does not describe.

### An Ansible task

`admin show` gives the same action as a task for a playbook, which does
not use a token: it runs over SSH as an administrator, with `become`. The
services' changes, `service.start`, `stop`, `restart`, `enable` and
`disable`, are tasks of the module `service` of the collection
`dukovo.system`:

```
$ admin show service.restart name=diskhealth
...
Ansible
- name: "Restart a service: diskhealth"
  dukovo.system.service:
    name: "diskhealth"
    state: "restarted"
  become: true
```

`enable` gives `state: "started"` with `enabled: true`, and `disable`
`state: "stopped"` with `enabled: false`. The module reads what Spark says
of the service and does what is not so yet, so the task reports "changed"
only when it changed something, and a second run of a `start` reports
nothing; a `restart` always does something. `--check` says what it would do.

Install the collection from its release, on the machine that runs Ansible,
which needs ansible-core 2.22 or later and so Python 3.13 or later:

```
$ ansible-galaxy collection install https://github.com/dukovo-os/ansible/releases/download/v0.1.0/dukovo-system-0.1.0.tar.gz
```

Put the task in a play's `tasks`. Ansible needs Python on the machine: see
[Python](python.md). Every other action's task is the module
`ansible.builtin.command` with the command's words as `argv`. That module
cannot tell a change from none, so such a change task reports "changed"
each time it runs, and a read has `changed_when: false`. A read whose
command answers with another status too, such as `service NAME explain`,
which ends with 3 for a service that does not run well, registers its
result and has a `failed_when` that names the statuses it answers with. The
collection's modules for certificates and for the console's reach are a
later addition.

### Passwords and keys

An action that takes a password or a key, such as a new password for an
account or the recovery key that opens a backup, never puts it in the
command's words. The command reads it when it asks, as it would read it
from a script, so the journal, the audit log, a task and the command that
**Show command** gives all say what was done without saying the secret.
The console sends it in the body of a `POST`, never in an address, even
for an action that only reads; `admin actions --json` marks such a field
`writeOnly`.

**Show command** needs no secret, and shows each as `(secret)`. It says
what the command reads, such as "then the password, typed when asked".
The API tab takes the secret from a shell variable named after the field,
such as `$PASSWORD`; set it without leaving it in the shell's history, as
bash's `read -rs PASSWORD` does. A secret with a `"` or a `\` in it needs
the JSON escape in the variable, `\"` or `\\`. The Ansible tab takes it from
the playbook's variable, such as `{{ password }}`, which belongs in an
Ansible vault, and its task has `no_log: true`, so that Ansible's output
does not show it either.

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
  is not for a token (the actions, their list, their showing and checking,
  and the tasks): a token reads and changes through the actions, and reads
  and stops tasks, but cannot sign in, unlock, read the journal or make
  another token, and the description of the API, which anyone may read, is
  asked for without one. The detail names the scope it needs; make
  a token with that scope.

Two more follow from the action itself. **A value cannot work**
([ADM-125](../../errors/ADM-125.md)) names the field and says what it may
be, and nothing was run. **The command did not work**
([ADM-123](../../errors/ADM-123.md)) holds the command's own words.

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

**Tasks**, in the header, opens a drawer that lists the tasks that wait or
run and those that ended lately, each with its step and bar, and stops one
(see "The Tasks drawer" below). It calls the routes below.

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

### The Tasks drawer

**Tasks** in the header opens the drawer. It holds the tasks that wait or
run and those that ended in the last 24 hours, in three groups, each with
its count after its name:

- **Running**: the tasks that wait or run, the newest first.
- **Needs a look**: the tasks that failed or were stopped, the newest
  first.
- **Finished today**: the tasks that ended well since midnight in your time
  zone, one line each, with who asked and when it ended.

When there is none in any group it says **No tasks have run today.** No
action of this release is long (see above), so that is what it says until
one runs. Opening the drawer counts the tasks that need a look as seen, in
this browser, and the number on the button goes back to the tasks that
run.

A task in the first two groups is a card:

- Its title is the action's with what it is about after a dot, **Renew a
  certificate · this-machine**, and under it who asked and when, **Started
  by alice at 2:00 AM · task 42**, or **alice with the API token monitor**
  for a script's.
- At the right, **Step 2 of 4**, **Running**, a percent, **Failed** or
  **Stopped**.
- A task that runs shows where it is, **Step 2 of 4: Copying the files**, or
  **Waiting for its turn**, and a bar. The bar is a moving stripe when the
  command has given no percent.
- A task that has ended says how: **Finished**, **Finished; nothing
  changed**, **Stopped: the console restarted**, **Failed: its command
  ended with status 3**, with when.
- **Show the log** opens the task's log, which is read again every 2
  seconds while it is open, and **Hide the log** closes it. A log with
  nothing in it says **The log is empty.**
- **Stop** is on a task that waits or runs, for the person who asked and for
  any administrator. It asks **Stop task 42?** and says **Its command is
  ended; what it did so far stays.**; **Stop the task** goes on and **Keep
  it running** does not. The drawer then says **Stopping task 42.** Stopping
  is what "Stopping a task" says it is. **Stop** waits while the console
  does not answer (see "When something goes wrong").
- Under the card is the command the task runs, in one line. Press it, and
  **Show command** takes the drawer's place (see "Showing the command").

A task that you asked for, when it ends, is also a note on the page you are
on: **Task 42, Renew a certificate, finished.**, **was stopped.** or **did
not work.**, which has **Show the log** when it did not work. The drawer
says under the groups that tasks keep running when you close the browser or
lose the connection, and that you see them from any device you sign in on.

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

## Services

**Services**, in the list of pages, shows the services Spark runs on this
machine, and each has a page of its own. Anyone who signs in can look at
them. Starting, stopping and restarting are for administrators, who unlock
the console first (see "Unlocking the console"). The list is at `/services`
and a service's page at `/services/NAME`.

### The list

The table has a row for each service, with the ones that failed first and the
rest by name:

| Column | What it says |
| --- | --- |
| Service | Its name, which opens its page |
| Now | Its state, in a word and in a dot that repeats it, so that the colour is not all there is |
| At boot | **Starts at boot**, or **Off** |
| Since | When it came to its state, in your time zone. The full date and time are in the tooltip |
| What it does | Its description |

A service that `service try` is trying a change to has **Being tried** beside
its state. The states are:

| State | What it is |
| --- | --- |
| Running | It has a process |
| Starting | Its process has begun and is not ready yet |
| Done | It ran and ended well, as a service that does one job does |
| Waiting | It has not started, because it waits for a service it needs, or for its turn |
| Stopping | It was asked to stop and has not ended yet |
| Failed | It ended badly, or could not start, and Spark gave up on it |
| Stopped | It was stopped, or it is turned off |

**Filter** keeps the services whose name or description has the words you
type, found as the command palette finds them: letter by letter and in order,
capitals and accents not counting (see "The command palette"). **State**
keeps **Running**, **Failed**, **Stopped**, **Off at boot** or **Being
tried**. Both are in the address, as `?q=ngi&state=failed`, so a link or a
reload shows the same rows. A filter that leaves none says **No service
matches the filter.** and has **Show every service**. A screen reader hears
how many rows are shown, "Services, 3 shown of 12", a moment after you stop
typing.

The list is read again every 10 seconds while the tab is shown, and when a
task or a change comes. It draws a row again only when what the row says
changes, so a link you are on keeps the focus. A service whose name has a dot,
such as `php-fpm8.3`, has the dot written `%2E` in its address
(`/services/php-fpm8%2E3`), as the links of the console write it: an address
whose last part has a dot is read as a file's.

### One service

The page starts with the service's name, its state, whether it starts at boot
and what it does. For a service that is not running, or done, a banner says
why: the sentence that `service NAME explain` gives, such as **nginx is not
running: its start failed 3 times in a row; it last said: ...**, what to do
about it, and a link to the service behind it when another one is the cause.
A service whose health check has failed says so under its state, and one
whose file has a mistake says that Spark runs the last good copy.

These buttons run the actions of the same names, with the console locked or
not as any change goes (see "Unlocking the console"):

| Button | When it is there | What it runs |
| --- | --- | --- |
| **Run now** | The service has no process and is not stopping | `service NAME start` |
| **Stop** | The service is running or starting | `service NAME stop` |
| **Restart** | The service is running or starting | `service NAME restart` |
| **Starts at boot** | Always, as a switch | Turning it on runs `service NAME enable`, and off `service NAME disable` |

Turning **Starts at boot** on also starts the service now, and turning it off
also stops it now, as the commands do. The page says so under the switch, and
a notice says what was done, **diskhealth no longer starts at boot, and is
stopped.** While **Show commands** is on, each button has its command under
it, which opens **Show command** when you press it (see "Showing the
command"). A locked console asks to unlock first and says what for, and
**Cancel** sends nothing. An account that is not an administrator sees no
button, and the words **Administrators can start and stop services.**

`admind` and `authd` are what the console is made of, so stopping one would
end the answer to the press. Their pages have no **Stop** or **Restart**, the
switch cannot be turned off, and the page says why before anyone tries:
**The console runs on admind, so this page does not stop it, restart it or
turn off its start at boot: that is done at a terminal.** It shows the command
to copy, `elevate service admind restart`. The actions refuse them as well
([ADM-125](../../errors/ADM-125.md) names the field `name`).

Under the buttons are cards, each of which reads for itself:

| Card | What it shows |
| --- | --- |
| Runs | Since when it runs, its processes, how many times it was started again, what started it, how long it took to be ready, and its restart rule, such as **Starts again when it fails, and gives up after 5 starts that fail in a row** |
| Needs | The services it requires, wants and starts after, and what it waits for now, each a link to that service |
| Holds | What its file lets it reach and do: the account it acts as, the files it may read and change, the ports it may listen on, and whether it connects out |
| Use | What it has used and been held to, and charts of its processor and memory |
| Limits | What its file lets it use, and the command that changes that |
| Sockets | The sockets Spark holds for it, and whether it listens now. A service that holds none has no such card |
| Timer | That a timer starts it, and when. A service that no timer starts has no such card |
| Being tried | There while `service try` is trying a change to its file: when the change is undone unless it is kept, with `elevate service keep NAME` and `elevate service undo NAME` to copy |
| Recent log | The last 20 lines of the service, and **Open in Logs** |

**Use** has the numbers Spark keeps for the service's resource domain: its
memory now and the most it used so far, what its memory limit did (the
requests for more memory that it refused, and whether the service was stopped
at the limit), its threads, the bytes it read from and wrote to disks, and how
long its requests waited for a disk and were held back by its rates. They are
counted since its first start after the machine started, and the page says so
under them. They come with the page's own reads, every 5 seconds.

Under the numbers are two charts, the service's processor, in percent of one
processor (a service of several threads may pass 100), and its memory, drawn
up to its limit when its file sets one. A **1 h**, **24 h**, **7 days** or
**1 year** control chooses how far back they go, and the range is in the
address, as `?range=7d`. The charts are read when the card opens and when you
change the range, not on a timer, and a service that the machine has kept no
numbers of says **No history yet: the machine keeps it from its first 10
minutes on.**

**Limits** has a row for what the service's file sets: **Memory** (or **No
limit**), **CPU weight**, **Threads**, **Open files**, **Disk weight**, the
rates of disk reads and writes and of their requests, the **Stop signal** and
**Stop timeout** when the file names them, and **Its file**, the path of the
file. The limits are changed in the file, and the card gives the command to
copy, `elevate service try NAME`, which makes a change for a time and undoes
it unless it is kept (see `service(8)`). The console does not edit the file.

**Timer** says that Spark starts the service at the times its file names, and
has **Open Scheduled tasks**. To an administrator who has unlocked the
console it adds **When**, **Next**, **Last** and **Result**, as the Scheduled
tasks page has them. The timers are read once, when the card first shows, and
when the console is unlocked or locked. A locked administrator sees **Unlock to
see its schedule.** with **Unlock**, and anyone else **Administrators see its
schedule.**

**Recent log** shows the whole log of the service to an administrator who has
unlocked the console. For one who has not, it shows the lines of the
administrator's own programs, none as a rule, and **Unlock** under the words
**You see the lines of your own programs. Unlock to see the service's log.**
For anyone else it says **Administrators see the service's log.** **Read
again** reads it again, and so does unlocking or locking the console. It is
read when the page opens and then only when asked, never on a timer, since
each look at the whole log is a line in the audit log (`elevate log
--audit`). **Open in Logs** goes to the Logs page, with the service chosen.

The page is read again every 5 seconds while the tab is shown, after each
button, and when a task or a change comes. What is drawn changes only when
what it says does, so a button you are on stays where it is. A name that is no
service gets **There is no service called NAME** and **Open the list**.

### Scheduled tasks

**Scheduled tasks**, at `/services/scheduled`, lists each service whose file
has a `[timer]` (see `service.toml(5)`): when it runs, when it runs next and
last, and how the last run ended. The soonest comes first, and a timer that
has no time within five years comes last.

| Column | What it says |
| --- | --- |
| Task | The service, which opens its page |
| When | The schedule in words, such as **Every day at 3:30 AM**, **Every 6 hours** or **Monday to Friday at 8:00 AM**, with the schedule as the file writes it under them, `30 3 * * *`, when the words are not the same. A schedule the page does not read is shown as it is written |
| Next | When it runs next, in your time zone, with the whole time in the tooltip. A dash means that the service runs from its timer now, or that no time comes within five years |
| Last | When it last ran, or **Never** |
| Result | **Worked**, **Failed**, or **Not known**. Spark keeps the result of a run only since the machine started, so an earlier one is not known |

A timer runs by the clock of its own `time_zone`, the server's when the file
names none. When that is not the zone you chose, its times say whose they are
and the heads of the columns say **on the server's clock**. A server with no
timers says **No service has a timer.**

Only an administrator who has unlocked the console reads the timers, since
`timerd` answers an administrator alone. A locked administrator sees **Unlock
to see the scheduled tasks.** with **Unlock**, and **Cancel** in the dialog
reads nothing. Anyone else sees **Administrators see the scheduled tasks.**
The page reads them when it opens, when you press **Read again**, and when
the console is unlocked or locked, never on a timer.

### On a phone

The page of a service on a phone has the banner first, then the buttons, one
to a row and as wide as the page, 44 pixels tall at least. **Open its log**
goes to the card of the log, which is below the others. The list is a stack of
cards, each service's state and start under its name, and so are the scheduled
tasks.

## The Logs

The **Logs** page reads the machine's log, which logd keeps, in the
browser: the newest lines first read, with the words to narrow them, and the
lines as they come when you ask for that. It runs `log` for you, as you have
it at a terminal (see log(1)), and what it shows is what that command would
print. ADR 0252 says who may read which lines.

### Choosing the lines

The filters are above the list. Each change reads again, and the address
holds them, so a link to the page shows what you saw.

| Filter | What it does |
| --- | --- |
| **When** | **Last 15 minutes**, **Last hour** (the start), **Last 24 hours**, **Last 7 days**, **Today** or **Custom range**, with a start and an end that you give as a date and a time; leave the end empty to read up to now |
| **Importance** | **Errors only**, **Warnings and errors** (the start), **Information** or **Everything** |
| **Times in** | **Your zone**, **The server's zone** or **UTC**: the zone the times are shown in, and the one a custom range is given in. Changing it does not read again |
| **From** | **Everything**, **The machine itself** (the kernel), or one service by its name |
| **Words in the line** | Words that the line holds, matched as you typed them. Upper-case letters make the search match case, as they do for `log -g`. At most 200 characters |
| **Fold repeated lines** | Lines that follow each other with the same source and text become one row, **repeated 12 times, last at 09:41:07** |

The list shows the newest 200 lines of that time, the oldest at the top and
the newest at the bottom, and scrolls to the bottom when it opens. A long
line wraps and stays whole. **Show older lines**, above the list, asks for
the 200 before the first one shown. When more than 200 lines share one
second, the page says so, with a link that opens that second alone with up
to 1,000 lines. A link may hold `lines` for the number a read asks for,
from 1 to 1,000; the page leaves out a name or value it does not know and
says which.

Open a row with **Enter** or a click. It shows the time in all three zones,
the importance, where the line is from, the process and user when it has
them, the start of the machine's run that it belongs to and its number, and
these buttons: **Show 5 minutes around this**, which sets a custom range of
five minutes either side of the line, keeps **From** and clears the words;
**Open NAME**, for a service's line; **Only this service**, or **Only the
machine itself**; and **Copy line**. Escape closes the row.

**Show commands**, at the top of the page, shows the command of the read
under the filters, to copy and run at a terminal (see "Showing the command").

### Who sees which lines

Only an administrator reads the whole log (ADR 0252). Everyone else reads
the lines of their own programs.

- **An administrator with the console unlocked** reads the whole log. Each
  change of a filter is one command that runs as an administrator, and so
  one line of the audit log (see "Unlocking the console"). Typing in
  **Words in the line** waits for 600 milliseconds after the last key, and
  **Enter** asks at once, so that a word typed is one read and not one for
  each letter.
- **An administrator with the console locked** reads their own lines and
  sees **You see the lines of your own programs. Unlock to read the whole
  log.**, with **Unlock**. Unlocking reads again as an administrator, and
  locking reads again as yourself.
- **An account that is not an administrator** sees **You see the lines of
  your own programs. The machine's log is for administrators.** The page
  has nothing to unlock for that account.

### Following the log

**Follow new lines** shows lines as the machine writes them. It is off
until you switch it on, so that the page does not read the log while no one
looks, and **Pause** stops it. A green **Live** mark sits by the button
while it follows. One follow is one command that runs for as long as you
follow, and one line of the audit log for an administrator, where reading
the log again and again would be one line each time.

While you are at the bottom of the list, new lines scroll into view. When
you have scrolled up, they stay where they are, and a button such as **12
new lines** takes you to the bottom. A screen reader is told how many came,
at most every 5 seconds and not at all when none did; it is not read each
line. Changing a filter while following reads again and follows with the new
filter. **Show commands** shows the command that the follow runs.

A follow stops by itself, and the page says why, with **Follow again**:

| The page says | Why |
| --- | --- |
| **Following stopped: you locked the console.** | Locking ends a follow of the whole log |
| **Following stopped: your unlock ended.** | The unlock ran out |
| **Following stopped: this sign-in ended.** | The session ended |
| **Following stopped: you began following in another window.** | One follow for each sign-in: the newer one stays |
| **Following stopped: it had run for an hour.** | A follow lasts an hour |
| **Following stopped: the lines came faster than this page could take them.** | More than 256 KiB were waiting for the browser |
| **Following stopped: the server is stopping.** | admind is stopping |
| **Following stopped: the log command ended.** | `log` ended by itself; what it said last follows |
| **Following stopped: the connection to the server was lost.** | The connection went with no word from the server |

When more than 100 lines come in a second, the page shows the first 100 and
says how many it left out. A ninth follow on the machine is refused with
ADM-134, and the page counts down the 30 seconds to wait. The page never
opens a follow again by itself.

A follow goes with the tab. A tab that stays hidden for a minute closes its
follow, so that a browser's few connections to the console are free for the
tab that is shown; a quick look at another tab does not. When you come back,
the page reads the lines that came while you were away and follows again.

### Saving lines

**Save as file...** saves the lines the list shows, in the zone **Times in**
shows, as a text file with one line each:

```
2026-11-14 10:30:00.123456 +0100 warning nginx[312]: upstream timed out
```

The line has the time to the microsecond with its offset, the importance,
the source with its process number when there is one, and the text. The
file is named `log-NAME-YYYYMMDD-HHMM.txt`, for the machine and the time
you saved it, in the zone shown. The browser saves it as it saves a
download; the server keeps nothing. Only the lines in the list are saved, so
**Show older lines** first when you want more, and for a long stretch run
`log` at a terminal.

### From a chart to the logs

Under each chart of the Usage card on the Overview is a link, **Open CPU**,
**Open Memory**, **Open Disk** or **Open Network**. It opens the chart on a
page of its own, under Logs, with the card's range, at
`/logs/chart/cpu?range=24h`. The buttons **1 h**, **24 h**, **7 days** and
**1 year** at the top change the range.

Choose a moment on the chart by clicking it. The chart is also a slider, named
**Choose a moment on the CPU chart**: `Tab` reaches it, `Left` and `Right`
move one point, `Page Up` and `Page Down` ten, `Home` and `End` go to the
ends, and `Enter` chooses. A screen reader is told the time and the value of
each point, such as "2:05 PM, 63 percent". The changes of the journal in that
time are marked with small triangles above the chart, and listed under it as
buttons, so that the keyboard reaches them: choosing one chooses its time.

The moment is a step, the chart's point that holds it: one minute on **1 h**,
15 minutes on **24 h**, 2 hours on **7 days** and a day on **1 year**. The
page shows four cards for it, and each reads for itself, so a card that cannot
read says why and the others go on.

| Card | What it shows |
| --- | --- |
| **What happened around** | The journal's rows and the log's warnings and errors in the step and the step before it, the newest last; the newest 12 when there are more |
| **Busiest at** | The five services that used the most CPU and the five that held the most memory in the step, in one table |
| **What moved most** | The services whose CPU or memory changed most against the hour before the step: the mean over the step against the mean over that hour, ranked by the change over the larger of the two, so that a service that went from 1% to 40% ranks above one that went from 50% to 60%. The top five, each as "from 1% to 40%". A service with no numbers in the hour before is "new"; a share of less than 0.5% CPU or 1 MB is not counted |
| **Logs** | The warnings and errors of the step, as the Logs page shows them, with **Open in Logs with this time**, which opens the Logs page for that step. When a step holds more than 200 lines, the card says **The newest 200 lines of this step**; the Logs page has them all |

The moment is in the address, `at=` in milliseconds, so a link shows the same
moment. The times are in your time zone. The Logs card reads as the Logs page
does: an administrator with the console unlocked reads the whole log, and each
read is a line of the audit log; everyone else reads their own lines (see "Who
sees which lines").

## Reliability

**Reliability**, in the list of pages, shows how the machine has fared: its
health, the problems of each day, what happened on each day, its starts, and
the journal of changes. Anyone who signs in can look at the starts, the
changes and one start's page. The errors in the log, the crash dumps and the
alerts are an administrator's, who unlocks the console first (see "Unlocking
the console"). The page is at `/reliability`, one start at
`/reliability/starts/N` and the whole journal at `/reliability/changes`.

The head has **7 days**, **30 days** and **1 year**, which choose how far back
the page looks, and **Copy settings to Git...**. The range is in the address,
as `?range=30d`. A year counts by the month, and the others by the day, in
your time zone. The cards are:

| Card | What it shows |
| --- | --- |
| Health | The score of the last checks, as the Overview has it, such as **92, good**, with **How the score works**. Without `dukovo-check` it says **Checks are not on this server yet.**, and an account that is no administrator, **The health score is for administrators.** |
| Problems a day | A bar for each day (or month) of the failed starts, crashes and errors in the log, with the total over the range in words. **Show the numbers** has them as a table |
| Events by day | A row each for **Starts**, **Services** (errors in the log), **Updates** (a start whose release is not the one before it), **Changes** (the journal's rows) and **Crashes**, and a column for each day (or month) |
| Starts | The starts the machine keeps, newest first |
| A failed start | Only while a start that is kept has failed (see "When a start fails") |
| Crashes | The kernel crash dumps, newest first |
| Alerts | The last 20 changes of the alerts, newest first |

The page counts and does not score. The machine keeps no figure for stability,
so the page makes none up: **Problems a day** is the number of things that went
wrong, and a day with none has no bar.

Each cell of **Events by day** is a button with a dot and its count in words,
**3 errors** or **2 starts, 1 failed**. The dot has a shape as well as a
colour: one for what failed or was taken back, one for a warning, and one for a
change or a restart. Press a cell and the day opens under the table, with
everything that happened in it, oldest first, so that the newest is last. The
errors of one service are one line, **nginx: 3 errors**, with the last of them
under it. The day has **Logs for this day**, which opens the Logs page for the
day in your zone, and, for a day, **Changes this day**, which opens the
journal for it (a month has only the first). Press the same cell again to put
the day away.

### Errors, crashes and alerts

The errors of the log, the crash dumps and the alerts are read by an
administrator who has unlocked the console. A locked administrator sees, on
**Events by day**, **Crashes** and **Alerts**, **Unlock to see errors, crashes
and alerts.** with **Unlock**, and the rest of the page is drawn; anyone else
sees **Administrators see errors, crashes and alerts.** A day then has no
**Services** or **Crashes** count, and the page says that what could not be
read is not counted.

**Crashes** has each dump's time, size and the first line of its panic, and
`elevate dumps show NAME` to copy: the text of a dump is long and root's, so
the page does not show it. Five are listed, and **And 3 older. dumps list
shows them all.** says how many are left. **No crash dumps are kept.** is what
a machine that has not crashed says. **Alerts** lists when each change of an
alert was, whether it was **Raised**, **Cleared** or **Told**, how bad it
was, where it came from and its title.

The log can be counted for the newest 1,000 errors, and the journal for the
newest 200 changes; when a read gave the most it may, the page says **Only the
newest 1,000 errors are counted, so earlier days may have more.**

The page reads each of its sources once when it opens, when you change the
range, when you press **Try again** under one that failed, and when the
console is unlocked or locked. It never reads on a timer: each look at the
whole log is a line in the audit log (`elevate log --audit`).

### Starts

**Starts** has a row for each start the machine keeps, newest first, ten at a
time, and **Show older (N more)** shows ten more.

| Column | What it says |
| --- | --- |
| Started | When the start began, in your time zone, which opens that start's page |
| Version | The release, with the slot it ran from, as in **1.0.0-alpha.1 (slot A)** |
| Ready in | How long it took to be ready, or a dash when its time is not known, or **never** for a start that failed |
| Result | **Good**; **Failed: BOOT-104**, with the ID of the check that failed it; **Fell back**, when the update went back to the release before; **Try 2 of changes bisect**, a restart that a search for a change is making; or **Not finished** |

A start's page has its facts (when it began, its version, how long it took and
its result), and **Where the time went**: the steps of the start, such as the
boot loader and sign-in, each with its time. **Compare with** picks the
start to lay it beside, and the page chooses the last good start. Each step
then has the time it took in that start and the change, **+2.8 s** or **-0.4
s**, and a step that took a tenth of a second or more longer is marked as
slower, which a screen reader says too. Pick **No other start**, or any start that
has its times, to compare with that one instead. **What took longest** lists
the services that took longest to become ready, and the chain of services
that the start waited for, in order.

Only some starts have times. A start without them says **No times are kept for
this start.**, a number that the machine has no start of says **The machine has
no record of start N.** with **Open Reliability**, and a start with no good one
before it says **No good start with times is kept to compare with.**

### When a start fails

When a start that the machine keeps failed its health check, the page has
**A failed start**: **The start at 9:12 AM failed its health check
(BOOT-104).** with **Open the start**, and **These changes came since the last
good start:**, a list of what changed since: the release of Dukovo or Radix,
a set of firmware, the CPU microcode, the boot options or an app, with its
version and when.

With two changes or more the card offers to find which one broke the start:
**Find which change broke it. The machine restarts up to 3 times, and nothing
is changed for good.** The command to copy is `elevate changes bisect`. It
restarts the machine, which the page does not do from a button, so the card
says **Running it from the console comes with the Hardware page.** While the
search runs, the card says which try it is on, **Changes bisect is running: try
2 of 3.**, and gives `elevate changes bisect --stop` to end it. Under them are
`changes --failed`, which lists the changes again, and `troubleshoot`, which
says what the checks suggest. The card is left out while no start that is kept
has failed.

### The journal

**See all**, on the Overview's **What changed** card, opens the whole journal at
`/reliability/changes`: every change the journal has, newest first, fifty at a
time with **Show older (N more)**. The route holds the newest 200, and the
page says so when it has that many. Three choices narrow the rows, and are in
the address, so that a link keeps them:

| Choice | What it keeps |
| --- | --- |
| **Who** | The changes of one person, or **Outside the console** |
| **How** | **In this console**, **By hand**, **Put back** or **Undone by Spark** |
| **Day** | One day, in your time zone |

A link such as `/reliability/changes?who=alice&how=console&day=2026-11-14`
opens with them chosen. Each row is the Overview's (see "What changed"): its
time, who made it and how, a dot, and the buttons that fit it, **See the
change**, **Keep it**, **Put back** and **Undo**, which ask to unlock first.

**Copy as a script** turns changes that you choose into a shell script that
makes them again, on another machine, for instance, or on this one after a
restore. A row can be chosen when it was made in the console, worked, and is
not a put-back (that names a row of this machine's journal). Tick the rows,
or press **Choose all shown**, which chooses every row that can be chosen of
those the filters leave, and press **Copy as a script**. The script is shown
in a box and copied:

```
#!/bin/sh
# Changes made with the Dukovo console, oldest first.
set -e

# 2026-11-14 09:30:00 UTC: Restart a service
elevate service diskhealth restart

# 2026-11-14 09:31:12 UTC: Disable a service
elevate service diskhealth disable
```

Each change has a comment with its time in UTC and its title, and then its
command, as **Show command** gives it (see "Showing the command"), with
`elevate` in front where the action runs as an administrator. The oldest comes
first. Save it as a file and run it with `sh`, as a person who may `elevate`,
which asks as it does at any terminal; `set -e` stops it at the first command
that does not work. A script copies what the console did and no
more: a change made by hand is not in it, and a file's text is not either, so
it makes a change to settings on a machine that has the same files and
services, and no other. **Choose the changes to copy first.** is what the
page says when none is chosen. A console that could not read which commands
run as an administrator does not offer the button, and says why.

### Copying the settings to Git

**Copy settings to Git...** in the head opens a form. **Folder** is filled
with `/users/NAME/HOST-settings`, your own folder, and the form says what it
does: **Copies the files under /system/settings that everyone may read into
this folder as a Git commit. Secrets are left out and listed in SECRETS.md.**
**Copy settings** runs `admin export FOLDER` as you, with no `elevate`, so that
it reads what you may read and writes in your own folder, and shows the
command under the form.

The folder becomes a Git repository if it is new or empty, and a folder that
holds other files and is not a repository is refused, with nothing written in
it. The files are put at the paths they have on the machine, under
`system/settings` in the folder, in one commit, **Settings of duke-nas at
2026-11-14 08:00:00 UTC**, whose author is "Dukovo on duke-nas", the machine's
and no one's own. An export that changes nothing makes no commit and says
**The settings are in Git already.** A file that not everyone may read is a
secret: it is left out by name, and `SECRETS.md` in the folder lists each
path and why, never what is in it. Each export is a commit, so `git log -p`
shows what changed between two of them.

### On a phone

The table of events is a list on a phone, a day to an item, newest first, with
each cell of the day a button 44 pixels tall. The starts and the journal are
stacks of cards, and the buttons are 44 pixels tall, as they are on the other
pages.

## Networking

**Networking**, in the list of pages, shows the machine's network cards and
changes how a card gets its address. Anyone who signs in can look at them.
Changing a card is for administrators, who unlock the console first (see
"Unlocking the console"). The cards are at `/networking` and the settings of
one card at `/networking/NAME`, with the card's name, such as `net0`, or its
MAC address. The page runs `network show`, `network set`, `network keep` and
`network undo` (network(8)); what they write is `/system/settings/network`,
which network(5) describes.

### The cards

There is a section for each card, with its name, which opens its settings,
and its state: **Connected, 1 Gbit/s**, or **No cable**. The card the console
is reached through says **You are connected through this card.** Under it are
what the card has now and how it is set up:

| Row | What it says |
| --- | --- |
| Hardware address | The card's MAC address |
| Set up | **Gets its address by DHCP**, **Has addresses of its own**, or **Left as the machine finds it**; a card that serves DHCP to other machines says so |
| Addresses | Each address with its prefix and where it came from: DHCP, fixed, IPv6 from the router, link-local, or set another way |
| Router | The routers it uses |
| Name servers | The servers the machine asks for names |
| Metered | **Metered**, **Not metered** or **Decided by itself** |
| DHCP lease | The address the lease gives, and until when; **which has ended** when the time has gone by |

**Traffic** draws what each card received and sent, in bits a second, over the
range you choose, the way the Overview does; the range is in the address, as
`?range=7d`. Without any history yet it says so. **Connections** lists the
machine's open TCP connections, the other end of each, and its state. The
connection your own browser has to the console is marked **This browser**.

The page is read again every 10 seconds while the tab is shown, and when a
change or a trial comes. What is drawn changes only when what it says does,
so a link you are on keeps the focus. A name that is no card gets **There is
no such network card** and **Back to the cards**. An account that is not an
administrator sees no **Change…**, and the editor shows the settings with the
words **Administrators change the network settings.**

### Changing a card

**Change…** asks to unlock the console first, if it is locked, and says what
for. The editor starts from what the card does now:

- **From the router (DHCP)**: the card asks for its address when it starts.
  A card that serves DHCP to other machines cannot choose it, and the editor
  says why ([NET-178](../../errors/NET-178.md)).
- **Fixed**: one to four addresses, each with its prefix, such as
  `192.168.1.10/24`; a router, which is on the network of one of them;
  up to three name servers, separated by commas or spaces. When you choose
  **Fixed** the fields start with the addresses, router and servers the card
  has now.
- **Metered**: **Decide by itself**, **Yes** or **No**. A metered card waits to
  download updates and other large things until a time you choose.

The checks are the ones `network set` makes, in the same words, so what the
page refuses is what the command would refuse. A wrong value is said under its
field when you leave it, **192.168.1.10 has no prefix: write it as
ADDRESS/PREFIX, as 192.168.1.10/24**, and again, with focus moved to the field,
when you press **Try it**; nothing is sent until every field is right. While
**Show commands** is on, **This runs** under the form holds the command, made
again a moment after you stop typing (see "Showing the command").

Two buttons make the change:

| Button | What it does |
| --- | --- |
| **Try it** | Makes the change and has Spark undo it by itself unless it is kept, within the time chosen under **Undo by itself unless kept within**, 60 seconds to start with |
| **Apply without trying** | Makes the change for good. It asks first, **Apply without a way back?**, and says to change the card at the machine's screen or over the serial console if it cuts the console off |

### Trying a change

A change that is tried is in force at once, and Spark has put a copy of the
settings file aside. The page shows a banner at the top, which stays in view as
you scroll, with the time left, the time the change is undone, a bar that runs
down, and two buttons:

- **Keep this change** keeps it for good (`network keep`).
- **Undo now** undoes it at once (`network undo`).

The banner says **Applied** when the settings are in force, and then asks the
console whether this browser still reaches it over the new settings, over a
connection opened after the change. A firewall or an address that was changed
is checked when a connection begins, and a connection that is open goes on, so
the answer of an old one would prove nothing ([ADM-133](../../errors/ADM-133.md)).
The page asks again by itself, up to three times at once and then every 3
seconds, until a new connection gets through: **This browser reached the
console over the new settings.** **Keep this change** is off until then, and
says why under the buttons, but **Undo now** is always on. A browser that cannot
reach the console says **This browser cannot reach the console over the new
settings. It is undone at 14:32 unless it is kept.**, and the change is undone
at that time by Spark, which does not need the page for that.

The banner is shown to every session, whoever began the change, so a second
administrator sees who began it, **Begun by alice in this console**, or **Begun
at a terminal**, and can keep or undo it; a page that did not begin it asks the
reach question when **Keep this change** is pressed. An account that is not an
administrator sees the banner and the words **Administrators keep or undo the
change.** The form is off while a change is tried, **Keep or undo the change
being tried first.** (see [NET-176](../../errors/NET-176.md)). When the change
ends the banner says how: **The change was kept.**, or **The change was undone:
it was not kept.** A change that was undone has a row in "What changed".

A screen reader hears the time left when the change begins, and at 60, 30 and
10 seconds, in a quiet voice that does not interrupt; the clock itself is not
read each second. With less motion chosen in your preferences the bar moves in
steps, not smoothly.

While a change is tried, the header of every other page of the console, in
every session, shows a chip with a warning mark: **Trying: the firewall's
rules · 0:41**. It is a link to the page that has the banner, and it goes
when the change is kept or undone. When more than one change is tried it says
**Trying 2 changes** and opens a menu with a link for each, the soonest to be
undone first. The page that has the banner does not show it, as the banner
says the same. The chip counts down by the browser's clock and takes the time
left from the console again every 30 seconds. It says nothing aloud: its name,
which a screen reader reads when you reach it, is the words and the time of
the undo, **Trying: the firewall's rules, undone at 14:32 unless kept**. On a
phone it is the mark and the time.

### When the address changes

A change that takes away the address this browser reaches the console at
cannot be kept from this page: the page loses the console as soon as the
change is made. The editor sees that before the change is made, and says so in
a warning, **This browser reaches the console at 192.168.1.10, which this
change takes away.** It names the new address as a link, **After Try it, open
https://192.168.1.50:9090/networking in a new tab, sign in, and keep the change
there.** The browser asks about the certificate again at the new address, since
the machine's certificate names the machine, not its addresses (see "The first
visit"). The same is true of a change to the card's router, when the browser is
not on the card's own network, and of a change to DHCP, which chooses the
address itself.

The warning changes what is offered:

- **Apply without trying** is not there.
- The times to undo by itself are longer: 5 or 10 minutes when you came by an
  address, and 2, 5 or 10 minutes when you came by the machine's name, which
  finds the new address within a minute. 5 minutes is chosen.
- After **Try it** the banner holds the link to the new address. The answer
  to **Try it** has no way back once the address is gone, so the page waits
  15 seconds for it, and then shows the change being tried by its own clock,
  with the link, and says that this browser cannot reach the console over the
  new settings.

Nothing is lost if you cannot get there: the change is undone at the time the
banner shows, and the console is where it was. Another session that came by the
same address loses the console too, and shows the card as it was once the change
is undone.

## Firewall

**Firewall**, in the list of pages, shows the firewall's rules and the rules'
effect, and changes them. Anyone who signs in can see the rules and ask what a
rule would do to a connection. Changing the rules, and seeing what was blocked
and what listens, are for administrators, who unlock the console first. The
page is at `/firewall`. It runs `firewall list`, `firewall allow`, `firewall
deny`, `firewall replace`, `firewall remove`, `firewall keep`, `firewall undo`,
`firewall test`, `firewall status` and `firewall dropped` (firewall(8)); the
rules are in `/system/settings/firewall`.

### Rules in order

**Rules** lists the rules in the order they are checked: the first that fits a
connection decides it. Each is a number and the rule in words, **Allow TCP port
8080 from 192.168.1.20**, with its command under it while **Show commands** is
on. Under them the page says what happens when no rule decides: **A ping is
answered. A connection reaches a port only while a service listens on it with
the right to be reached; anything else reaches this machine alone.** A line of
the file that is no rule is listed with the reason, **Line 7 is not a rule:
...**.

An administrator has **Change** and **Remove** on each rule, and **Add a rule**
under them. A rule that names a program has neither, since the form has no field
for it: it says **Changed at a terminal**. **Add a rule** opens a form with:

- **Allow** or **Deny**.
- A ready-made service to start from, such as **SSH 22** or **Web 443**.
- The protocol: **TCP**, **UDP**, **TCP and UDP** or **Ping**, which has no
  port.
- A port or a range, such as `8000-8100`. A port that is no port is said under
  its field, **`70000` is not a port, 1 to 65535, or a range of them**.
- **From**: **Anywhere**, **This network**, the network of the card the console
  reached you through, or **An address or network…**.
- **On**: any card, or one.
- **Undo by itself unless kept within**, as for a network change.

A new rule goes at the end, so the rules above it decide first. **Change** and
**Remove** open the same form, **Replace rule 2** and **Remove rule 2**. The
form is in the address, `#add`, `#add?replace=2` and `#add?remove=2`, so that a
link opens it, and **Escape** closes it and gives focus back.

**Try for 60 seconds** makes the change and has Spark undo it by itself unless
it is kept, with the banner and the buttons of "Trying a change". **Apply
without trying** is there only when the page is sure that the change leaves the
console reachable. While a change is tried the buttons of the rules are off,
**Keep or undo the change being tried first.** ([NET-177](../../errors/NET-177.md)).

**Test a connection** asks which rule would decide a new connection, before
anyone makes it: the protocol, the port, the address it comes from, which starts
as your own, and the card. **Rule 2 decides: allowed.** is what `firewall test`
says. A rule that names a program is **passed over**, since the test does not
know which program listens, and the answer says so.

### What was blocked

**Blocked in the last 24 hours** is for an administrator who has unlocked the
console; a locked one sees **Unlock to see what the firewall blocked.** and
**Unlock**, and anyone else **Administrators see what the firewall blocked.**
It groups what the kernel dropped by the address, the protocol, the port and the
card: **203.0.113.9 tried port 22 on net0 at least 14 times, last 4 minutes
ago**, with what the kernel said, such as that the program listening for it is
for this machine alone. **Allow this** opens the form with the rule that would
have let it in, `allow tcp 22 from 203.0.113.9`, for you to check and try. The
kernel logs at most five a second, so more were dropped than counted.

Addresses blocked for guessing passwords are listed apart, **Blocked for
guessing passwords**, with a link to the Security page, where there is one, to
manage them. **Read again** reads the card again; it is read when the page opens and after an
unlock, and not on a timer, since each look is a line in the audit log (`elevate
log --audit`).

**What listens** lists the sockets the machine listens on, with the program and
who reaches each: **Every machine reaches it**, **Some machines or cards reach
it**, **No other machine reaches it by rule 3** or **This machine only**. It is
for administrators in the same way.

### When a rule would cut you off

A firewall is checked when a connection begins, so a rule that denies the
console's port to your address stops this browser's next connection, and the
page would lose the console as soon as the rule is made. The form checks the
rules as they would be after the change, against the connection of this
browser, before it is made: a connection from your address, through the card
you came by, to the console's port, which admind answers. When the first rule
to fit it denies it, the form says so:

**This would cut off your own connection. The console reaches you from
192.168.1.20 through net0 on port 9090, and rule 2 would deny it.**

The warning is an alert, so a screen reader says it once; **Try for 60 seconds**
is still there, since a rule you can undo by itself is the way to find out, and
**Apply without trying** is not. After **Try**, the banner cannot reach the
console over the new rules and says so, **Keep this change** stays off, and
Spark takes the rule away at the time shown, with the page back at the console.
What the browser tried in that time is under **Blocked**, and **Allow this**
makes the rule that would let it in. At the machine, `elevate firewall undo`
takes a change back at once.

### On a phone

The cards of Networking and Firewall are a stack, one under the other, and each
table is a list of cards, the number or the name over its words. The banner
stays at the top of the screen; its buttons are as wide as the page, 44 pixels
tall at least. The form's choices are one to a row.

## Storage

**Storage**, in the list of pages, shows the machine's disks and what is on
them: how full each volume is and when it will be full, the last hour of reads
and writes, each disk's health, and the room the system's log takes. Anyone who
signs in sees all of it but the log's size. Looking after a disk is for
administrators, who unlock the console first: checking a disk now, scrubbing a
mirror, growing a disk into free space, and changing the log's limits. The page
is at `/storage`, and `/storage/sdb` opens the disk `sdb` beside the list. It
reads `storage show` and runs `diskhealth`, `scrub`, `storage grow` and
`logtrim` (storage(8), diskhealth(8), scrub(8) and logtrim(8)). The console
formats and replaces no disk: those stay commands at a terminal, with the steps
written out on the page.

### Disks and the mirror

At the top of the page is a banner when something needs a look, the worst first:
a volume that has **lost a disk**, a disk that **says it is failing**, a disk
whose count of moved sectors rose, or a mirror that is **catching up** after a
disk that was away came back. A machine in good order has none.

Under it is a card for each volume: **System** for the volume the machine runs
from, **Data** for the one that holds `/users` and `/shared`, **System and
data** when one volume holds both, and any other volume by its label. A card
says how much is used, **1.24 TB used of 2 TB on sda and sdb**, with a bar that
turns warm at 90 percent and red at 97, and when the volume is on two disks or
more, the state of the mirror:

| The card says | It means |
| --- | --- |
| **Mirror healthy** | Every file is on each disk: either could fail and nothing would be lost |
| **Mirror missing a disk** | A disk is gone. The files are on the disk that is left, with no second copy |
| **Mirror catching up** | A disk was away and is being brought up to date. It does this by itself |
| **Changing layout** | A balance is changing how the volume's blocks are kept. Until it ends, some may have no second copy |
| **Spread over sda and sdb, with no second copy** | The volume uses two disks to be bigger, not safer. A disk that fails takes files with it |
| **Two copies on one disk** | The metadata is kept twice on a single disk, which guards against a bad sector, not a bad disk |

The card also says when the volume will be full, **Full in about 7 months at the
rate of the last 30 days**: a straight line through the last 30 days of use,
drawn once there are 7 days to draw it from. A machine with less says **Not
enough history yet**, and one whose use is falling says nothing. A card whose
volume will be full within 30 days is warm whatever its bar says, and the
Overview lists it. **Reads and writes, last hour** is the chart of the disks'
work, with **Show the numbers** under it as on the Overview.

**Disks, drawn to scale** has a row for each disk, USB sticks too, with its
model and size and a bar of its partitions, each as wide as its share of the
disk and named for what it holds, **System**, **Data mirror · 1.24 TB used**,
**Swap**, **Not used yet**. Room after the last partition is drawn hatched. A
partition too narrow for words is a number, and the key under the bar says what
the numbers are. The pill at the right is the disk's health: **Healthy**,
**Wearing**, **Failing**, or **Not checked** for a disk that does not report
one, as a USB stick or a virtual disk does. A row is a link to the disk's own
page.

A disk's page has the facts of one disk: its model and serial number, in full,
since you need it to find the disk in a case; what it is used for, **Half of the
data mirror (Btrfs RAID1)**; how old it is, **2 years 6 months powered on
(21,804 hours)**; its temperature; its counts of moved and waiting sectors (and
media errors and the share of its rated writes for an NVMe drive); how many read
errors the last scrub fixed; and when its last full check was, which only an
administrator reads. **Check now** reads the disk and shows every number it
reports in a table under the facts, with **Show the numbers as diskhealth
prints them** under that; it runs `diskhealth`.

**Scrub the mirror** reads every block of the volume and mends one that does not
read from the other copy; for a volume of one disk the button is **Check every
block**, which finds a bad block and cannot mend it. The page says what it asks
for before it is pressed: **This reads every block of 1.24 TB: about 3 hours.
The machine stays usable, a little slower.** The estimate is from how long the
last scrub took. A scrub is a task: its button says **Scrubbing… 41%** and
opens the task in the Tasks drawer, and when it is done the facts say how many
read errors it fixed.

On a phone the disks are a stack of cards with their bars as wide as the page,
and an opened disk is a view of its own with **Back to the disks** over it. The
buttons are as wide as the page, 44 pixels tall at least.

### When a disk wears

A disk that begins to fail says so before it does: the count of sectors it has
moved to spares rises, or sectors wait to be moved. The machine watches the
counts, and when one rises the page says so at the top and the Overview lists
it: **sdb moved 8 worn-out sectors to spares. It still works and the mirror is
healthy, but a count that grows is often the first sign a disk is wearing out.
Keep backups current and have a replacement ready.** On a disk that is in no
mirror, the next sentence is that its files have no second copy on this machine.
**Check sdb now** reads the disk and shows the numbers. A few moved sectors on
an old disk can stay as they are for years; a count that keeps rising, or a
disk that says it is **failing**, is the time to replace it.

**Replace this disk…**, and **How to replace a mirror disk** in the banner, show
the steps and their commands in a window, each with **Copy**. The page runs
none of them: each can erase a disk if a wrong name is typed, so you type them
at a terminal, as an administrator, and check the names against the list of
disks on the page first.

1. Look after the disk that is left. While a disk is missing or failing, the
   other holds the only full copy. Copy what matters off it if you can.
2. Put the new disk in. It shows on the page as **Not used yet**, and it must be
   as big as the disk it replaces, or bigger.
3. Give it partitions like the old disk's: `elevate rdisk -l sda` shows the
   table of the disk that is left, and `elevate rdisk sdd` makes the new
   disk's.
4. Rebuild onto the new partition: `elevate replace start DEVID
   /devices/sdd1 /volumes/data`, where `DEVID` is the missing disk's device
   number, 1 or 2, or its device, such as `/devices/sdb1`, when it is still
   there. It prints when the new disk holds everything, and the machine stays
   usable meanwhile.
5. If the volume is the system's own mirror, put the start-up code on the new
   disk too: `elevate rgrub-setup sdd`.
6. Press **Scrub the mirror**, to check every block of the new copy.

**Use a new disk…**, in the head of the page, gives the steps for a disk that is
new to the machine: to make a volume of its own, `elevate rdisk sdd` and
`elevate mkfs-btrfs -L NAME /devices/sdd1`; to make a mirror of an existing
volume, `elevate device add /devices/sdd1 /volumes/data` and then `elevate
balance start -dconvert=raid1 -mconvert=raid1 /volumes/data`, which turns the
data and the metadata into two copies. A volume that is being made a mirror this
way says **Changing layout** until the balance ends, since some blocks have no
second copy yet. When the page for backup places is
there, a disk with nothing on it has **Use it for backups**.

### Growing a disk

A disk can be bigger than the partition on it: a cloud machine's disk made
larger in the provider's panel, or a virtual disk made larger by the host. When
the room after a disk's last partition is more than 1 percent of the disk and
that partition is Btrfs, its page says so: **This disk has 20 GB after its last
partition, which the data volume can grow into.** **Grow into it** makes the
partition reach the end of the disk and the filesystem reach the end of the
partition, while the volume stays in use. It runs `elevate
storage grow sda`, which is `rdisk --grow` and `device resize max` together;
with **Show commands** on, the command is under the button. The page reads the
disks again and the room is gone. Nothing shrinks a partition from the page.

### The log's own space

The last card is the system's log, which is kept on the machine and is bounded
by limits so that it never fills a disk. It says how much room it takes and what
it keeps: **The system's log takes 1.2 GB in 92 files, 80 MB of it the audit
log. It keeps 90 days of records, and at most 4 GB or 10 percent of its disk,
whichever is less; the audit log keeps a year.** Only an administrator sees the
size, which the log's own service reports to root alone: a locked administrator
sees **Unlock to see the log's size.** and **Unlock**, and anyone else
**Administrators see the log's size.** The card is read when the page opens, when
the console is unlocked or locked and after a change, and not on a timer.

**Change…** opens a form with the four limits:

- **Keep records for**, in days, 1 to 3650, or empty for no limit.
- **Take at most**, a size such as `4G` or `500M`, or empty for no limit. `G`
  counts in 1024s, as the log does; `GB` counts in thousands.
- **Take at most**, in percent of the disk the log is on, 1 to 100, or empty for
  no limit.
- **Keep the audit log for**, in days, 30 to 3650, or empty for no limit.

A number that is out of range is said under its field and nothing is sent. The
form sends only the fields you changed, and the command it runs is under it with
**Show commands** on; `elevate logtrim set` makes the same change at a terminal.
The limits are kept in `/system/settings/log`, which log(5) describes. The log
trims itself when a day begins and after each 64 MiB it writes; **Trim now**
trims it to its limits at once, as `elevate logtrim` does at a terminal, when
you want the room back before then.

## Snapshots and backups

**Snapshots and backups**, in the list of pages, shows what keeps the machine's
files safe: the snapshots on the machine itself, and the backups that copy them
somewhere else. It says which of the places the machine keeps data in have a
snapshot and a copy off the machine, lists the snapshots by day, and lists the
places the backups go to. Anyone who signs in to the console sees it, except
what a backup place holds, which is for administrators, and anyone can bring
back a file of their own from a snapshot. Looking after the backups is for
administrators, who unlock the console first: taking a snapshot now, adding a
backup place, backing up and checking now, restoring from a backup, and
removing a place. The page is at `/snapshots`; `/snapshots/backups` is the page
scrolled to the backup places, `/snapshots/backups/nas` opens the place `nas`
beside the list, and `/snapshots/browse` is a snapshot's folder. It reads `snap
list` and `backup list` and runs `snap` and `backup` (snap(1) and backup(8)).
"Updates, snapshots, backups and recovery" says what the commands do.

### What is protected

At the top of the page is a banner when a backup needs a look, the worst first;
a machine whose backups are in order has none. "When a backup is overdue or
failed" says what each banner means.

**What is protected** has a row for each place the machine keeps data in:
**People's files** (`/users`), **Shared folders and websites** (`/shared`),
**Settings** (`/system/settings`), and **Apps and their data** when a backup
place holds them. Each row says three things in one look:

| The column | What it says |
| --- | --- |
| **Snapshots on this server** | **Daily · 3:02 AM**, the rule that made the newest snapshot and its time; **None yet: the first is made within the hour** for a place that waits for its first; **Not made** for a place that the snapshot service does not see |
| **Copy off this server** | Each backup place that holds the data, with the day of its last good backup, **nas · Nov 10**, in red when it failed and warm when it is overdue; **Not copied off this server** when none does |
| **Last restore check** | The newest check that restored the backups and compared them, **Nov 11, passed**; **Never**, with **Check now**; or **failed** |

A snapshot lives on the same disk as the files it keeps. It brings back a file
that was deleted or changed, but a disk that is lost takes the snapshots with
it, so the page does not call a place protected for having one: the copy off
this server is what does that. A place that the snapshot service does not see
is told under the table, **snap skips /shared**, with the command that adds it,
`elevate service edit snap`. The Overview lists the same things: a backup that
failed or is overdue, a check that failed, and **No snapshots of /shared** when
the schedule leaves out `/users` or `/shared`.

### Bringing back a file

**Snapshots** lists them newest first by day, under **Today**, **Yesterday**
and **Earlier**. A row is a time, why it is kept, and a **Browse** for each
place the snapshot covers. **Earlier** is a line that says how many, **14 daily
and 4 weekly**, until you press **Show all**. The heading says the rule, **kept
daily for 7 days and weekly for 4 weeks**. A snapshot with a name is kept until
someone deletes it. One that came from a backup says so, **Restored from nas:
its backup of Nov 10**, and the one made before an update says **Before the
last update: the settings as they were.**

**Browse** opens a folder of a snapshot as it was, with the folders above it as
links and each entry's size. Beside each is how it compares with the same path
now: **changed since** or **deleted since**, and nothing for what is the same.
A folder opens by its name. Tick the files and folders you want and press
**Restore N chosen**, up to 100 at a time. Each comes back beside the current
one with the snapshot's time in its name: `report.odt` becomes `report (Nov 13
3:02 AM).odt`, a folder `photos (Nov 13 3:02 AM)`. Nothing is overwritten, so a
file that still exists is kept as it is and you choose between the two. The
time is on your clock, in the time zone of your account, and the page lists the
paths that it made. A folder that holds more than the page shows says
**Showing the first 2,000 entries. Open a folder below to see fewer.**

What you see in a snapshot is what you could see then: the page browses and
restores as you. An administrator whose console is unlocked sees every file of
a folder outside their own home, and restores as the administrator, which is
how one account's file is brought back for another. While the console is
locked, an administrator is told **Some files here may be hidden while the
console is locked**, with **Unlock to see every file**. A reader restores files
of their own and sees no button that changes anything else. At a terminal,
`snap restore` brings files back from a snapshot (snap(1)).

**Take a snapshot now**, in the head of the page, is for an administrator. It
opens a window with a box for each place, all ticked, and an optional name; the
button **Take a snapshot** makes one of each place that is ticked and says
which were made. A snapshot with a name is kept until someone deletes it, so
give one only for a reason, such as `before-the-move`. A name that looks like a
time is refused, since the snapshot service gives those itself, and so is one
that starts with a dot or holds a `/`. The snapshot service makes its own each
day, so the button is for the moment before a change. With **Show commands**
on, the window shows the command it runs, `snap create --name NAME PLACE`.

### Backup places and the recovery key

**Backup places** is a table of where the backups go: the place's name and where
it is, **SSH to 192.168.1.20** or **USB disk BACKUP**, how often, the day of
the last good backup, and how it is now. Now is **OK**, **failed**, **3 days
overdue**, **check failed**, **Waiting for the first backup**, **Not yet** for
a place that is backed up by hand and has no backup, or **Backing up… 41%**
while one runs. A place's name opens it beside the list. It shows:

- **Holds** and **Keeps**: the places it copies, and how much it keeps, **3
  chains of up to 7 backups** or **7 daily, 4 weekly, 12 monthly**.
- **Restore check**: when the newest backups were last restored and compared,
  with **Check now** for an administrator.
- **Backups there**: the times of the backups the place holds, for each place
  it copies, newest first, each with **Restore this…**. An administrator whose
  console is unlocked sees them, and a reader is told that administrators do.
- **Recovery key**: when it was shown, **Shown once on Oct 2**, with **Show it
  again** for an administrator.

An administrator also has **Back up now**, **Restore files…**, **Restore the
whole machine…** (which goes to Move to new hardware) and **Remove…**. **Back up
now** and **Check now** are tasks: the button says how far it is, and the Tasks
drawer follows it.

**Add a backup place…** asks where the backups go:

- **A plugged-in disk**: a USB disk or another removable disk that is mounted
  under `/volumes`. It is offered only when one is plugged in, and the backups
  go in the folder `dukovo-backup` on it. A disk that another place already
  uses is left out.
- **An SSH server**: another machine, such as a NAS, written as
  `backup@192.168.1.20:/srv/dukovo`, or as `ssh://user@host:2222/folder` for
  another port. The server has to accept this machine's key first, which the
  page shows with **Copy**: put it in the account's `authorized_keys` on the
  server. The place is added only when the machine can write there, so a
  server that does not answer is refused with the reason.
- **S3 storage**: added at a terminal for now, since the command reads its keys
  from the environment. The page gives the command, `elevate backup add NAME
  s3://HOST/BUCKET`.

Then a name, of 1 to 32 letters, digits, `-` and `_`, which is how the place is
named on the page and in commands (the page suggests one from the disk's label
or the server's name); **Back up** every day, every week or by hand, which a
disk starts as; and **Check by restoring** every week, every month or never.
**More** has how the backups are kept: as Btrfs streams, which suit a Dukovo
machine or a disk, or as a restic repository, which suits a share that a NAS
keeps. With **Show commands** on, the command is under the form.

Adding a place makes its recovery key, which reads its backups on any machine,
and the page shows it once, in a window named **Recovery key for nas**, with
**Copy** and **Save as file**. Keep it somewhere other than this machine:
without it and this machine, the backups cannot be read, and with it, anyone
who has the backups can read them. The console does not keep the key. It is in
the window until you press **I have kept it**, and then it is gone from the
page, the console's log and everything the browser stored. The window then
asks **Back up to nas now?**, since the first backup sends everything and takes
a while; it runs as a task, and you can leave the page meanwhile.

**Show it again** asks the machine for the key once more, with the console
unlocked, and shows it in the same window. The page asks for it only when you
press the button, and never again by itself, since the answer is a secret.
**Restore files…** asks which place and which time, and has the backup received
into the snapshots of that place as `restored-TIME`. Nothing is overwritten.
When it is done the page shows that snapshot, and you pick the files you want
back as above. **Remove…** forgets the place and its key and leaves the backups
where they are. Only the recovery key reads them then, so show it again first
if you have not kept a copy.

### When a backup is overdue or failed

The banner says what is wrong in the words of the command, and what still keeps
the files:

| The banner says | It means, and what to do |
| --- | --- |
| **The last backup to nas failed: …** | The command's own reason follows: a server that did not answer, a disk that is not plugged in, a disk that is full. The banner says when the last good backup was, and whether the snapshots on this server are current: if they are, files changed since can still be brought back, but nothing is outside the machine. Put the cause right and press **Back up now**. **Back up to a plugged-in disk instead…** is there when one is plugged in |
| **Backups to nas are 3 days overdue.** | The days since the last good backup that backup(8) counts as overdue. The banner is warm from the first overdue day and red from twice the days allowed, 3 days for a place backed up every day and 14 for one backed up every week. Press **Back up now** |
| **Restoring nas's newest backups to check them failed: …** | The check can fail for a reason of its own, such as a full disk. The backups may still be good. Press **Check now** to try again |
| **A whole-machine restore from nas is staged for the next start.** | **Open Move to new hardware** is where it is cancelled before the machine starts again |

A reader sees the same banners, with **An administrator can back up now.** in
place of the buttons that back up.

On a phone, **What is protected** is a card for each place, the snapshots are
one column with **Browse** as a button, and a folder is a list with a box for
each row and **Restore N chosen** fixed at the foot of the page. The buttons
are 44 pixels tall at least.

## Move to new hardware

**Move to new hardware**, in the list of pages, takes the backups of one
machine to this one. When a computer is lost or replaced, install Dukovo on the
new one, plug in the disk that holds the backups (or have the backup place of
this machine reach them), and this page brings the old machine's name,
accounts and passwords, settings, apps and files across. Nothing on this machine
changes until it restarts, and you can cancel until then. The page is for
administrators, who unlock the console first; a reader who opens it is told so.
It is at `/move`; `/move?from=nas` opens at the key with the backup place `nas`
chosen, and `/move?source=FOLDER` with a folder of a machine's backups. A
machine that was just installed offers it in **Finish setting up**, as **Bring
everything from your old machine**, with **Continue on its page**, and a backup
place has **Restore the whole machine…**. The page reads `backup find` and
`backup restore --machine --plan` and runs `backup restore --machine`
(backup(8)); "Restoring a whole machine", in "Updates, snapshots, backups and
recovery", says what the command does.

The page has four steps, a list with numbers and a mark on each that is done,
and the open one beside it. A step opens when the ones before it are done, and
you can go back to one that is done and change what you chose.

### Find the backup

The first step lists the backups it can use: each machine's backups on a
plugged-in disk, **USB disk BACKUP** with **duke-nas, today 1:30 AM** under it
(the machine and the time of its newest backup), the backup places this machine
has (a machine that was just installed has none), and **A folder…** for a path
that you type, such as `/volumes/BACKUP/dukovo-backup/duke-nas`. The disks are
found under `/volumes` by `backup find`, which an administrator's unlocked
console runs; a locked console says **Unlock the console to look for backups on
the plugged-in disks**, with **Unlock**, and the places are listed meanwhile.
With nothing plugged in the page says **No backups found on the plugged-in
disks**: plug the disk in and press **Look again**. **Continue** with nothing
chosen says **Choose the backup to move from.**

### Unlock it

The backups are encrypted, and only their recovery key opens them: the key that
was shown when the backup place was added (see "Backup places and the recovery
key"). Give it in one of two ways:

- **A key file on a plugged-in disk**, which `backup find` lists as **Recovery
  key from the KEYS stick**. The page chooses one by itself when it is named
  after the machine, `duke-nas-recovery.key`, or was made for the backup place
  you chose.
- **Typed or pasted** into **Recovery key**. A key that is typed wins, and
  clears the choice of a file: the page sends one of the two and never both.

**Unlock it** reads the record of the backups. A key that is not theirs is
**This key does not open these backups** (BAK-103), under the field, and the
field is empty again. Backups that a newer Dukovo made are refused too
(BAK-107): update this machine, or restore on one that is current.

A typed key is kept in its field and nowhere else. It goes in the body of the
request that reads the backups and the one that stages the restore, and never
to the Show command panel, whose command has an empty key in its place. The page
says so under the field: after **Unlock it** has taken the key the field is
empty, so **Stage it** asks for it again, unless you chose a key file, which is
only a path.

### Check what changes

Once the backups are open, the page shows what the restore does, in three cards.

**What comes across** has a row for each of:

| The row | What it says |
| --- | --- |
| **Name, accounts and passwords** | **duke-nas · owner and helper, with their passkeys and SSH keys**; backups made by an earlier release do not name the accounts, and say **its accounts** |
| **Settings** | `/system/settings`, and **with 1 change for the new network port** when a choice below changes it |
| **Apps** | **6 apps at the same versions, each with the permissions it was given** |
| **Files** | `/users` and `/shared` with their size as the backups record it, **Comes across**, or **Does not fit** |
| **Files on other disks** | Only when the old machine backed up a disk of its own, such as `/volumes/PHOTOS`: **Restored beside**, as a snapshot on this machine's disk of the same name when it has one; otherwise it stays in the backups |
| **Older snapshots** | **Not copied**: they stay in the backups, and this machine starts its own |

**Differences in the hardware** compares the old machine with this one. Each
row has a word that says whether anything is asked of you:

| The row | What it says |
| --- | --- |
| **Network ports** (**1 choice**) | The cards the old machine had and the cards this one has. For each old card that had fixed addresses there is a choice: **Give it duke-nas's address, 192.168.1.12, fixed** on the card the page pairs it with (by order, and you may pick another), or **Let it get an address automatically**. A card that is left to ask gets its address from the network, which is safe. Two old cards cannot take one port, and the page says so. A firewall rule that names a card this machine will not have is listed: **The firewall rule "deny ping on net1" names a port this machine does not have, so it does nothing here.** |
| **Disks** (**Fits**, **No mirror** or **Does not fit**) | What the old machine kept its data on, a mirror of two 2 TB disks, what this machine has, and whether the files fit, with how much is to spare. **No mirror** says a disk that fails loses the changes since the last backup, and where this console has Storage, how to add a disk |
| **Serial console** (**Plug it in**) | The old machine sent its console to a USB serial adapter and this one has none: plug the same adapter in |
| **Processor and graphics** (**Nothing to change**) | The two models, or the one |

Backups made before Dukovo kept the machine's hardware with them (a record of
format 1) have no such rows. The page says **These backups were made before
Dukovo kept the machine's hardware with them, so the differences cannot be
shown**, and still offers each old card's address, from the backup's network
settings alone. A choice is read as soon as you make it: with a key file the
backups are read again with it, so that what is said of the firewall follows;
with a typed key, which is gone from the page, the choices are kept and go to
**Stage it** as they are.

**After the restart** says what happens: **Turn the old duke-nas off before this
machine restarts**, in red when its fixed address is kept, since otherwise two
machines answer as duke-nas and both run the nightly backups; that the services
start with duke-nas's settings and the console checks each one and each website,
and lists what fails a check on the Overview with a way to fix or undo it; and
that this fresh install is kept, read-only, in `/snapshots/machine-restore`
until you remove it. It says where to open the console then, `https://duke-nas.local:9090/`
and, when a fixed address is kept, that address too, and to sign in as one of
duke-nas's administrators. **The same as these commands** is the Show command
panel for **Stage it** with your choices, and the command that restarts.

### Restart to finish

**Stage it** reads the old machine's files into a place that this machine
starts from. It is a task, **Step 2 of 5: receiving /users**, and takes as long as
reading the files takes; the Tasks drawer follows it, and you can leave the page
meanwhile. It is off, with the reason under it, when the files do not fit (**The
files need 1.24 TB and this machine has 900 GB free**), when two old cards were
given one port, and when a restore is staged already. A task that fails says why
and **Nothing is staged**: what it received is removed.

When it is done the step says **Staged from nas. Nothing changes until this
machine restarts, and you can cancel until then.** with **Cancel**, which takes
what was staged away and leaves the machine as it is. A restore that was staged
at a terminal, or in another tab, is shown the same way when the page opens,
with **Stage it** off. A second `backup restore --machine` is refused with
BAK-104, and leaves the first as it was: when another tab staged one while this
page still offered **Stage it**, its task fails that way, and the page shows the
one staged first, with **Cancel**. To finish, restart the machine. The console has no button for that yet:
the page gives `elevate reboot` in a box with **Copy**, to type at a terminal
(**Terminal** in the list of pages opens one). At the restart, before any
service starts, the staged places take the place of the new install's, and what
they replaced is kept in `/snapshots/machine-restore`. The console is then at
the old machine's name.

At a terminal the same four steps are:

```sh
elevate backup find
elevate backup restore --machine --plan --key FILE SOURCE
elevate backup restore --machine --key FILE SOURCE
elevate backup restore --machine --cancel
```

`--plan` reads the backups and says what would change, and changes nothing;
`--json` gives what the page reads. `--card OLD=NEW` or `--card
OLD=automatic`, with the MACs of the cards, is a network choice.

On a phone the page is one column: the steps are a list above the open step,
the choices are cards the width of the screen, buttons are 44 pixels tall at
least and fill the width, and the command box scrolls sideways inside itself.

## The terminal

The **Terminal** page puts a shell in the browser: the login shell of your
account, on a terminal of the machine, as you have it over SSH or at the
machine's own screen. An administrator may open root's shell there too. The
console holds the terminals, not the page, so they go on when the page
closes. admind(8) has the routes and the limits, admin.toml(5) the
`[terminal]` table that says who may have one, and ADR 0517 why it is made
as it is.

### Opening a terminal

**Terminal** in the list of pages shows your terminals as tabs, and opens
one when you have none. **New** makes another, so do **New terminal** in the
palette (see "The command palette") and `Alt+Shift+T`. A new terminal has
one pane, a shell, and is named for you and the machine, `alice@duke-nas`,
or `root@duke-nas` for an administrator's. Typing `exit` ends the shell and
closes the pane at once. A terminal ends with its last pane, and when you
have none the page says **No terminal is open.** with **New terminal**.

The shell starts as an SSH session's does, with your last sign-in and your
notices. `TERM` is `xterm-256color`, `COLORTERM` is `truecolor`, and
`DUKOVO_CONSOLE` is `1`, which a script may test to know that it runs in the
console. The browser's language is not passed on: the shell has the
machine's.

The terminal's colours follow the console's display (Dark, Light or High
contrast). Text that a program colours so that it would be hard to read on
its background, such as white on yellow, is drawn in the nearest colour that
has a contrast of at least 4.5:1, as the console's own text has. Dim text
keeps the program's colour.

Who may have a terminal is `allowed` in the `[terminal]` table of
`/system/settings/admin.toml`: `"everyone"`, which is the default,
`"administrators"` or `"nobody"`. An account with no shell has none. Where an
account has none, the **Terminal** entry is not in its list of pages (see
"The Terminal entry is missing"), and the page says why if it opens the
address ([ADM-132](../../errors/ADM-132.md)). The console reads the table
again every 30 seconds.

### Your own terminal and an administrator terminal

An own terminal is you, with your rights. A shell in it is a sign-in of
its own on a terminal such as `pts/7`, from the browser's address, so `who`
and `last` list it as they list an SSH session (see who(1) and last(1)),
and the audit log has a line when it starts.

An administrator terminal is root's login shell, as `elevate -i` starts it.
It is for an administrator, with the console unlocked:

1. Choose **New administrator terminal**, in the menu beside **New** or in
   the palette. A locked console asks you to unlock first and then goes on
   (see "Unlocking the console"). Someone who is not an administrator does
   not see it, and is refused by
   [ADM-122](../../errors/ADM-122.md) if they ask by its address.
2. The tab has a shield, and the pane's head says **Administrator · ends
   when the console locks, in 14 min**. The prompt of nukesh(1) shows an
   **admin** badge and a `#` while the shell is root's.
3. In the last 2 minutes of the unlock the head has **Reset to 15 min**,
   which does what the one in the header does.
4. The pane ends when the console locks, when the unlock runs out, and when
   the session ends, and what runs in it stops: the shell is hung up, and
   what is left of its process group is ended 5 seconds later. A program
   that has left the group goes on. **Lock** asks first, with **Lock and
   end** (see "Finding your way"). The pane stays with **The console
   locked, which ends administrator panes.**, and **Close this pane** takes
   it off.

An administrator's shell is not a sign-in record, as an elevated command is
not, so `who` and `last` do not list it. The audit log has a line for its
start and, with the reason, for its end: `elevate log --audit`. Only the
session that opened it sees it: the Terminal and Sessions pages of another
session, even of the same person, do not list an administrator pane, and a
terminal with no pane to show is left out. A pane you make with **Split**
is your own shell, in a terminal of either kind. For root's, open **New
administrator terminal**.

### Tabs, panes and layouts

Each terminal is a tab, with its name, a dot while a terminal that is not in
front has new output, and a shield when it has an administrator pane. A
screen reader hears all three, "alice@duke-nas, administrator, new output".
Press a tab to show it. Double-click it, or choose **Rename this terminal**
in the **New** menu, to name it: **Rename the terminal**, **Name**, up to 40
characters. The tab's **x** ends the terminal for a pointer; the keyboard
has **Close this terminal** in the menu and `Delete` on the tab. It asks
**End this terminal?** when a program is known to run in it. The Left and
Right arrow keys move between the tabs, `Home` and `End` go to the first and
the last, and `Down` shows the terminal of the tab you are on.

A terminal has one to four panes, each its own shell. **Split right** and
**Split down** add a pane, in columns or in rows, and say **A terminal has
4 panes at most.** at the fifth. The panes lie as the layout says, which
the **New** menu offers once there is more than one pane:

| Layout | How the panes lie |
| --- | --- |
| **One pane** | Only the pane in hand shows, as on a phone: choose another layout to see the others |
| **Columns** | Side by side |
| **Rows** | One over the other |
| **Main pane at the left** | The first tall at the left, the others in a column at its right |
| **Grid** | Rows of two: two side by side with two panes, two over one with three, two over two with four |

A split along the line the panes already lie in keeps the layout, and one
across it makes a grid. **First split**, in the same menu, sets the first
border to 30%, 50% or 70%. Each border between panes is a separator named
**Resize panes**: drag it, or focus it and use the arrow keys, which move it
5% a press. A border stays between 20% and 80% of the room it divides. The
layout and the shares are kept with the terminal, so every browser that
shows it draws it the same.

A pane has a head and a screen. The head says the terminal's name and the
pane's number, as `alice@duke-nas · 2`, then, for an administrator pane, its
words, then the title that the program in it sets, **Newest ↓** when you
have scrolled back, which jumps to the newest line, and **Pane menu**. The
menu has **Read the screen**, **Copy** (when there is a selection),
**Paste**, **Clear the scrollback**, **Split right**, **Split down**, and
**End this pane**, which says **Close this pane** once the pane has ended.
**End this pane** asks **End this pane?** when a program is known to run in
it: **A program is running in it, and it ends too.**

Above the panes, at the right of the tabs, are **New** with the menu
**More terminal actions** beside it, and then **Split right**, **Split
down**, **A−** and **A+** for the text, **Keys** and **Full screen**.
**A−** and **A+** change the size of the text by 2 pixels, from 12 to 20,
and keep it as the `size` preference, for every browser you sign in with.
**Keys** shows the key bar (see "The terminal on a phone"), and **Full
screen** shows the terminal alone over the whole screen. Under the panes, a
line says where the connection stands:

| Line | What it means |
| --- | --- |
| **Connected · terminals keep running when you close this page, until the console restarts** | All is well |
| **Connecting…** | The page is opening its connection |
| **Reconnecting… try 3** | The connection fell. The page tries again after 1, 2, 5 and 10 seconds and then every 30. A pane that comes back goes on where it was, or shows the last 64 KiB if the console no longer has what it missed |
| **Disconnected · the session ended** | The session ended |

### The keys of the terminal

With the **Shortcut keys** setting on, which it is by default, these keys
are the console's. The list is also on the settings page, under **Terminal
keys**.

| Keys | What they do |
| --- | --- |
| `Alt+Shift+T` | A new terminal |
| `Alt+Shift+W` | Ends this pane, and asks while a program runs |
| `Alt+Shift+PageUp`, `Alt+Shift+PageDown` | The tab before, the tab after |
| `Alt+Shift+` an arrow | The pane in that direction |
| `Alt+Shift+Up` in a top pane | Leaves the pane for the tabs: the way out of the terminal |
| `Alt+Shift+\`, `Alt+Shift+-` | Split right, split down |
| `Delete` on a terminal's tab | Closes the terminal, and asks while a program runs |
| `Alt+Shift+K` | Opens the command palette |
| `Alt+Shift+R` | Reads the screen (see below) |
| `Ctrl+C` | Copies when text is selected, and otherwise goes to the program |
| `Ctrl+Shift+V`, `Shift+Insert` | Pastes |
| `Shift+PageUp`, `Shift+PageDown` | Scroll the lines above the screen |

With the setting off, for people whose system changes the keyboard layout on
`Alt+Shift`, only `Alt+Shift+Up` and `Alt+Shift+K` stay, so there is always
a way out. Every other key goes to the program. The browser keeps `Ctrl+W`,
`Ctrl+T`, `Ctrl+N` and `Ctrl+Tab` for itself, and a page never sees them: on
the key bar, press **Ctrl** and then **W** to send `Ctrl+W` to the program.

### Selecting, pasting and reading the screen

Drag to select, or hold `Shift` and drag while a program has asked for the
mouse. A double-click selects a word, a triple-click a line, and a line
that wrapped is copied as one. **Copy as soon as text is selected**, in the
settings, copies when you let go; **Copy** in the pane menu and `Ctrl+C`
copy otherwise. The page says **Copied.**, or **The browser did not let the
page copy. Read the screen has the text to copy.**

A paste takes the control characters out of the text, other than tabs and
line breaks, so that it cannot end a program's paste early or start a
sequence of its own. It goes in as a bracketed paste when the program asked
for one. A paste of several lines to a program that did not ask is put to
you first: **Paste?**, **Paste 12 lines? Each line may run as a command.**,
with **Paste** and **Cancel**. A paste of more than 1,048,576 characters is
refused, **That is too much to paste: 1 MB at most.** A browser that does
not let the page read the clipboard says **The browser did not let the page
paste. Use Ctrl+Shift+V.**

The screen is drawn for the eye, and a screen reader does not read it.
**Read the screen**, in the pane menu or on `Alt+Shift+R`, opens a dialog
with the screen and the 500 lines above it as text, in a text area that
only reads. A screen reader moves by line and word in it, and you may select
and copy; **Copy all** copies the lot and **Close** leaves. **The screen is
empty.** says it when there is nothing. With **Read new output aloud** on, a
screen reader hears new output as it comes, at most every half second and
500 characters at once, as "…and 40 more lines", with the escape sequences
taken out.

A program's bell flashes the pane's border once. With less motion, in your
preferences or in the device's, it lights the tab's dot instead.

### The terminal on a phone

On a phone the page shows one pane at a time, and a second row of tabs,
**Pane 1**, **Pane 2** and so on, chooses it. The terminal's tools other than
**New** and its menu are hidden, and the key bar is always there, above the
phone's keyboard. On a wider screen **Keys** shows it.

The key bar has **Esc**, **Tab**, **Ctrl** and **Alt**, the four arrows,
`|`, `/`, `~` and `-`. **Ctrl** and **Alt** stay down for the next key you
press: **Ctrl**, then a letter, sends that letter with `Ctrl`. It is also
how you send `Ctrl+W` from a computer's browser.

### What happens to a terminal

| When | What happens |
| --- | --- |
| The page closes, or you go to another page | Your own terminals go on. An administrator's go on until the console locks, which it does a minute after the last page of the session is closed (see "Unlocking the console"). Opening a terminal again shows what its last 64 KiB of output said, and the program is told to draw its screen again |
| Another browser signs in as you | It lists your own terminals and opens them. Two browsers that show one pane see one screen and both may type, and the pane has the size that was set last |
| You sign out | **Sign out** asks first when terminals run, **You have 2 terminals running.**: **Keep them running**, **End them and sign out** or **Cancel**. Administrator panes end either way |
| The session ends in another way: its 12 hours, no console page open for a while | Your own terminals go on and an administrator's end. Sign in again to find them |
| The console locks, or the unlock runs out | Administrator panes end, with the note above |
| A page has not shown a terminal for 7 days | It ends. The days are `keep-days`, 1 to 30, in the settings, counted from when a page last opened it |
| The console restarts, by an update or `elevate service admind restart` | Every terminal ends, since the console holds them in memory only. You sign in again, and the Terminal page shows none of them |
| The shell ends, as with `exit` | The pane closes, and the terminal with its last pane |
| You end a pane or a terminal | It asks while a program is known to run in it |

### Sessions and settings

**Sessions**, under **Terminal**, lists every terminal you have, as a table
with a name, how many panes it has, when it was made and last used, the
address it was opened from, whether it is open (**in this tab**, **in
another tab**, **in this tab and another** or **not open**) and whether a
program runs in it (**Yes**, **No**, **2 of 3 panes**, or **Not known**).
An administrator terminal has a pill, **administrator**. Each row has
**Open**, **Rename** and **End**, which asks **End this terminal?** and **A
program is running in NAME, and it ends too.** when one runs. **New
terminal** makes one. The list reads again every 10 seconds, and says **No
terminal is open. Open one.** when there is none. Terminals that no page
shows are there, so Sessions is where to find what you left running.

**Terminal settings**, also under **Terminal** and on **Your account**,
keeps these as you change them, for every browser you sign in with (see
"Your preferences"):

| Setting | What it sets |
| --- | --- |
| **Text size (pixels)** | 12 to 20 |
| **Lines kept (lines)** | The lines above the screen that your browser keeps for a pane, 1000 to 50000. The console keeps the last 64 KiB of output besides |
| **Keep a terminal for (days)** | How long a terminal that no page shows lives, 1 to 30. It applies to the terminals you make after you set it |
| **Copy as soon as text is selected** | `copy-on-select` |
| **Read new output aloud** | `announce` |
| **Shortcut keys** | `shortcuts` |

Under them is the card **Terminal keys**, the table of keys above. A page
that is open takes a new setting at once, except the days, which a terminal
gets when it is made.

### The limits

| What | The most |
| --- | --- |
| Terminals for an account | 32, `sessions-per-account` in admin.toml(5), 1 to 64 |
| Panes in a terminal | 4 |
| Panes the console holds in all | 256 |
| Browser tabs of one session that show terminals | 4 |
| Panes one page shows at once | 16 |
| Output the console keeps for a pane | 64 KiB |
| A terminal's name | 1 to 40 characters |

Past one of the first five the console answers
[ADM-130](../../errors/ADM-130.md), which says which. A page of the console
that is open in a fifth tab says **This browser has the Terminal open in 4
tabs, as many as it may. Close one.** A pane that the page cannot take in
as fast as its program writes is held back, as a slow serial line holds a
program, and a page that has not taken in a pane's output for 30 seconds is
detached from it and attaches again.

A terminal or a pane that is not there, has ended, or belongs to another
account is [ADM-131](../../errors/ADM-131.md), the same for each, so that
one cannot tell them apart. A token never opens a terminal
([ADM-124](../../errors/ADM-124.md)): a script has SSH.

## When something goes wrong

Every failure that the console's server reports has an ID, such as
`ADM-106`, and a page that says what it means and what to do. Where the
report shows depends on what went wrong: a value that cannot work is said at
its field, and not in a box at the foot of the page.

| What went wrong | Where it shows |
| --- | --- |
| A value that cannot work, [ADM-125](../../errors/ADM-125.md), that names a field | Under the field, which is marked and takes the keyboard |
| The account looks and does not change, [ADM-122](../../errors/ADM-122.md) | A warning in place, in the console's own words |
| The console is locked, [ADM-106](../../errors/ADM-106.md) | The unlock dialog, and then the same thing is tried again; where a page has no way to try again, **The console is unlocked. Press the button again.** |
| The session has ended, [ADM-107](../../errors/ADM-107.md) | The sign-in page, which says **Sign in again:** and why |
| Too many tries, [ADM-105](../../errors/ADM-105.md) | **Too many tries. Try again in 40 s.**, counting down, and the button that tried waits |
| The console's helper for your session stopped, [ADM-111](../../errors/ADM-111.md) or [ADM-112](../../errors/ADM-112.md) | **The console's helper for your session stopped.** with **Try again** |
| A part of Dukovo that the machine lacks, [ADM-129](../../errors/ADM-129.md) | In place of the answer: **This needs `dukovo-check`, which this server does not have yet.** |
| The machine does not answer | A banner across the page (below) |
| Anything else | The error view |

### The error view

The error view is a box in the page's place, with these parts in this
order:

- **A title**, such as **The command did not work**, which is the console's
  own words for the failure.
- **What happened**, the detail the console gives.
- **What to do**, the way out the console gives, and the buttons the page
  adds.
- **The program's own message**, folded, when a command failed: what the
  command printed on its error output, as it printed it. Open it to read it.
- At the foot, the ID, **What ADM-123 means**, which opens the help page of
  that ID, and **Copy the details**, which copies the ID, the words above,
  the command and its status, the HTTP status, the page and the time, one to
  a line, for a report to whoever runs the machine.

A screen reader hears the box as an alert when it comes after something you
did, and as part of the page when the page opened with it.

A change whose answer never came back is not sent again by itself, since it
may have been done. The note says **The answer did not come back. Look in
What changed and Tasks to see if it ran.**, with **Open Tasks**.

### The help pages for errors

**What error messages mean**, in the Help menu, lists the error pages by
area, in the order of their IDs, with the title of each. **Filter by ID or
words** narrows the list as you type, and a line says how many pages are
left, **35 pages** or **No page matches.** Each entry opens the page of
that ID, which is on this machine: it says what the error means and what to
do, and links to the pages it names. They are the support center's pages,
[ADM-101](../../errors/ADM-101.md) to [ADM-135](../../errors/ADM-135.md). An
ID that has no page, as `/help/errors/ADM-999` asks, says **There is no
help page for ADM-999.** with a way back to the list. **What ADM-123
means**, in the error view, goes to the same page.

### When the console does not answer

When the console is not answering, a banner across the top says **duke-nas is
not answering. Trying again in 30 s.**, with **Try now**. It looks again
after 5, 10, 30 and 60 seconds, and then every 60, and says **duke-nas is
not answering. Trying now.** while it looks. The page keeps what it showed
and marks it **As of 2:02 PM**, so that you can tell it is old. A button that
changes something waits while the machine does not answer, with the tip
**The server is not answering**; in this release that is **Stop**, in the
Tasks drawer. A change asked for in the palette or by a button while the
machine does not answer is not sent: **The server is not answering. Nothing
was done.** When the machine answers again, to a look or to anything the
page asks meanwhile, the banner goes at once, and the Overview, Tasks and the
numbers in the navigation read again.

### Tasks that need a look

A task that failed or was stopped is in **Needs a look** in the Tasks
drawer, and **Tasks** in the header shows how many have not been seen. Its
card says how it ended, **Failed: its command ended with status 3**, and
**Show the log** has the last 2,000 lines of what it printed (see "The
Tasks drawer"). When you asked for it, a note says so too, **Task 42, Renew
a certificate, did not work.**, with **Show the log**. The Overview lists
the tasks of the last 24 hours that failed or were stopped among what needs
a look (see "The health score"), and the journal has the task's change when
its command ended (see "What changed").

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
`~/.config/dukovo-admin/settings.toml`. Nor are the terminals, which the
console holds in its memory only and which a restart ends (see "The
terminal"). The palette's list of recent rows, and the tasks you have seen,
are kept by your browser and nowhere else.

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

Each message carries an ID, **ADM-101** to **ADM-132**, and the support
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
- **Ansible says it cannot resolve the module `dukovo.system.service`.**
  The collection is not installed on the machine that runs Ansible.
  Install it as "An Ansible task" says; it needs ansible-core 2.22 or
  later, which `ansible --version` shows.
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
- **The Overview says "Checks are not on this server yet" (ADM-129).**
  The machine has no `dukovo-check`, which an update brings. The rest of
  the Overview works.
- **The Overview says "Not checked yet" (ADM-123).** The checks have not
  run, so there is no score. Run `elevate dukovo-check` at a terminal.
- **Usage says "No numbers yet".** The machine has kept no history, which
  `metricsd` writes. It comes within 10 minutes of `metricsd` starting; the
  card says if `metricsd` is not running.
- **The banner says "duke-nas is not answering".** The console, `admind`,
  does not answer the page. Look at `elevate service admind status` and
  `elevate log -s admind`. The page tries again by itself (see "When
  something goes wrong").
- **Logs shows only my own lines.** The machine's log is for
  administrators (ADR 0252). An administrator whose console is locked sees
  **Unlock to read the whole log**: press **Unlock**. An account that is
  not an administrator has no more to unlock; an administrator may read
  the whole log with `elevate log`, or make the account one (`elevate user
  set --admin NAME`). See "Who sees which lines".
- **Logs says "No lines match".** The lines of that time and importance do
  not hold the words. Try **Last 24 hours**, **Information**, or fewer
  words; the buttons under the message do the first two. Words are matched
  as typed, so a dot or a bracket is that character and not a pattern.
- **The chart says "No numbers yet".** The machine has kept no history,
  which `metricsd` writes. It comes within 10 minutes of `metricsd`
  starting; the page says if `metricsd` is not running. There is no moment to
  choose until it does.
- **The chart view says "The history has no numbers for ...".** The history
  has no point for that moment: `metricsd` did not run then, or the address
  names a time outside the range. What happened around it and its log lines
  still show, as they do for a change that restarted `metricsd`; for the
  busiest services, choose another range, or another moment.
- **Following stopped.** The page says why (see "Following the log"), with
  **Follow again**. After **The connection to the server was lost**, look
  at the banner (see "When the console does not answer").
- **The Terminal entry is missing, or the page says "The terminal is not
  open to this account" (ADM-132).** The list of pages shows **Terminal**
  only to an account that may open one. `allowed` in the `[terminal]` table
  of `/system/settings/admin.toml` may be `"administrators"`, which leaves
  out an account that is not one, or `"nobody"`; the console reads the
  table every 30 seconds, and a page learns it when it loads, so reload
  after the change. An account with no shell has no terminal either: a
  service's account has none, and is not for a person to sign in with. See
  "The terminal".
- **Too many terminals (ADM-130).** The detail says which limit: 32
  terminals for the account (end some on **Sessions**), 4 panes in a
  terminal, 256 panes in the console, 16 panes on a page, or 4 browser tabs
  for one session (close one). A terminal that no page shows still counts
  until it is ended or its days run out.
- **There is no such terminal (ADM-131).** The terminal or pane has ended, or
  is not the account's. Reload the page, and look at **Sessions**.
- **My terminals ended.** The console holds terminals in its memory, so a
  restart of the console ends all of them: an update that restarts it,
  `elevate service admind restart`, or a crash. It ends the sessions too, so
  you sign in again, and the Terminal page shows none. `elevate log -s
  admind` says when it started. Work that must outlive the console belongs
  in a service, or in a program run with `nohup`. Other ends are narrower:
  an administrator terminal ends when the console locks, when the unlock
  runs out and when the session ends; **End them and sign out** ends your
  own; and a terminal that no page showed for its **Keep a terminal for**
  days ends. The pane says why when it stays, **The console locked, which
  ends administrator panes.**
- **Ctrl+W closes the browser's tab.** The browser keeps `Ctrl+W`,
  `Ctrl+T`, `Ctrl+N` and `Ctrl+Tab` for itself, and the page never sees
  them. Press **Keys**, at the right of the terminal's tabs, then **Ctrl**,
  then **W**: the key bar sends `Ctrl+W` to the program. On a phone the key
  bar is always there.
- **Ctrl+K does nothing in the terminal.** The program in a pane has every
  key, `Ctrl+K` too. `Alt+Shift+K` opens the command palette from there.
- **The page is blank or has no style.** The console serves a file only if
  it matches its list of checksums, and the log of admind names each that
  does not (ADM-116). The system's files were changed or damaged: update
  the system or restore them (see [Updates, snapshots, backups and
  recovery](updates.md)), then `elevate service admind restart`.

- **A change was undone by itself.** A change to the network settings or the
  firewall's rules that was tried is undone by Spark at the time its banner
  shows unless **Keep this change** was pressed first, and also when the
  machine starts again while it runs. The page says **The change was undone: it
  was not kept.**, and the list of changes has a row for it (see "What
  changed"). Make the change again, and keep it; if the page could not reach the
  console over it, the next entry says what to look at.
- **I cannot reach the console after a change.** A change to the address, the
  router or a firewall rule can stop the browser that made it. Wait for the time
  the banner showed: Spark undoes the change by itself, and the console is where
  it was. At the machine, `elevate network undo` or `elevate firewall undo`
  undoes it at once. A change that was applied without trying has nothing to
  undo it: change it at the machine's screen or over the serial console, with
  `elevate network set` or `elevate firewall remove`, or put the file back from
  "What changed" (see "Put back").
- **Keep this change is off (ADM-133, ADM-135).** The page has not yet reached
  the console over a new connection since the change, which is what turns
  **Keep this change** on, and it asks again by itself. Reload the page, or
  open the console in a new tab, to open new connections; if none gets
  through, the change cuts this browser off and is undone at its time.
  **Nothing is being tried any more** means the change was kept or undone
  already.
- **The settings are being tried (NET-176, NET-177).** A second change cannot
  be made while one is tried. Keep or undo the first.

An error whose ID is not here has its own page; the number is the same in
the message and the file name.

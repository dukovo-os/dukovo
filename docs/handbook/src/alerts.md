<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# Alerts: email, ntfy and webhooks

This chapter describes the alerts, the system's way of telling you that
something needs you: a disk that is failing, a backup that is late, a
service that keeps stopping. It covers where they show, how to have them
reach you away from the machine, by email, a phone's notifications or a
chat room, and what they send. The full references are alertd(8),
alerts(8) and alerts.toml(5). "Setting up alert email" and "What each
alert means" are the next two pages.

## What an alert is

An alert is something that is wrong now and that you can do something
about. It has a title that says what, a detail that says what to do, and
a severity: a *warning* is for what to see to soon, before it costs data,
and *critical* is for what costs data or is about to.

The alertd service keeps them. The system's services raise them (the
disk checks, the backups, the update check, and Spark when a service will
not stay up), and so can your own scripts. When one is raised, alertd
tells you once, however long it stays:

- at once, as a notification on the screen, which a critical alert keeps
  there until you press a key;
- at your next sign-in, as a line for each active alert, such as `Alert:
  Disk WDC (sda) is failing (since 2026-11-13 10:02, alerts list)`;
- by email and to the webhooks you set up, if any, about 30 seconds
  later, so that alerts that come together are one message.

When the problem is gone, alertd clears the alert and tells you that too.
An alert that is active and gets worse, from a warning to critical, is
told again; one that only changes its words is not.

Until you set a channel up, alerts reach the screen and the sign-in only.

## Looking at them

Only root reads alerts, so an administrator types `elevate alerts`:

```
$ elevate alerts
SEVERITY  SINCE (UTC)       SOURCE      TITLE
critical  2026-11-13 10:02  diskhealth  Disk WDC (sda) is failing
warning   2026-11-12 22:15  backup      The last backup to Off_Site was 3 days ago
```

`elevate alerts history` shows the newest changes, what was raised and
cleared and when (`-n 50` for more), and `elevate alerts --json` gives a
script the same data. The source is the service that raised the alert;
what the alert means and what to do is in "What each alert means", and
each alert's own detail says which command to run first.

## Telling you away from the machine

Name where alerts go in `/system/settings/alerts.toml`
(alerts.toml(5)). The file holds a password and tokens, so alertd reads
it only when root alone can read it. Make it private before you put
anything in it:

```
$ elevate chmod 600 /system/settings/alerts.toml
$ elevate nano /system/settings/alerts.toml
$ elevate service alertd reload
$ elevate alerts test
```

`alerts test` sends a test alert through every channel at once and says
how each went: `email to ops@example.com: sent`, or why not, in the
words of the server that refused it. Run it after each change.

### Email

Alerts go by email through your mail provider's submission server, as a
mail program sends mail. alertd never sends without TLS, so the server
must offer it. "Setting up alert email" goes through it step by step; in
short:

```
[email]
to = ["you@example.com"]
server = "smtp.example.com"
user = "alerts@example.com"
password = "app-password"
```

### ntfy, for a phone

ntfy is a free app for Android and iOS that shows a notification when
something is posted to a topic, on the public server at ntfy.sh or on one
you run. There is no account to make:

1. Install the ntfy app and subscribe to a topic. Pick a name nobody
   will guess, such as `server-alerts-7f3a9c`: on the public server the
   topic's name is all that keeps others from reading and posting to it.
2. Add it to the file:

   ```
   [[webhook]]
   kind = "ntfy"
   url = "https://ntfy.sh/server-alerts-7f3a9c"
   ```

3. `elevate service alertd reload`, then `elevate alerts test`. The phone
   shows "Test alert" within a few seconds.

A critical alert is sent at ntfy's highest priority and a warning at
its next, so the phone can treat them differently. For a server of your
own that needs a sign-in, add `token = "tk_..."`, the access token it
gave you.

### Matrix, Slack, Discord and your own program

Each is a `[[webhook]]` with its own `kind`:

```
[[webhook]]
kind = "matrix"
url = "https://matrix.example.org"
room = "!abc123:example.org"
token = "syt_..."

[[webhook]]
kind = "slack"
url = "https://hooks.slack.com/services/T000/B000/XXXX"

[[webhook]]
kind = "discord"
url = "https://discord.com/api/webhooks/0000/XXXX"

[[webhook]]
kind = "json"
url = "http://192.168.1.20:8080/alerts"
```

For Matrix, make an account for the machine, invite it to the room, and
use its access token; `url` is the homeserver's address and `room` the
room's ID, which your Matrix app shows in the room's settings. A
Slack or Discord address is made in the app's or channel's settings, and
is a secret as it stands, which is another reason the file is root's
alone. A `json` webhook posts the changes as JSON to a program of yours
(alertd(8) shows the shape), and is the one kind that may use plain
`http`, to an address on your own network.

### Which alerts go where

Each channel takes `severity = "critical"` to hear only the critical
alerts, and `cleared = false` not to hear that an alert has cleared. So a
phone can buzz only for trouble while the mail keeps the whole record:

```
[email]
to = ["you@example.com"]
server = "smtp.example.com"
user = "alerts@example.com"
password = "app-password"

[[webhook]]
kind = "ntfy"
url = "https://ntfy.sh/server-alerts-7f3a9c"
severity = "critical"
cleared = false
```

## When a message does not arrive

`elevate alerts status` shows each channel's last success, last failure
and why, and how many messages wait:

```
$ elevate alerts status
email to you@example.com
  last sent   2026-11-13 10:02:41 UTC
  waiting     0 messages
```

A message that cannot be delivered is kept and tried again after 1, 5,
15 and 30 minutes, and then every hour, for up to 24 hours; a restart or
a reboot does not lose it. After 24 hours it is dropped, and the log says
so (`elevate log alertd`). So when a provider is down for a morning, you
hear everything after it is back.

If `alerts test` says "No channels are set up" after you named one, the
file was refused. The log says why: usually the file can be read by
others, which `chmod 600` and `elevate service alertd reload` fix, or a
setting has a mistake, which the log names with its line:

```
$ elevate log alertd
```

## Alerts of your own

A script of root's raises an alert with `alerts raise` and clears it
with `alerts clear`, and `alerts event` tells of something that happened
once, with nothing to clear:

```
#!/bin/sh
if /volumes/data/bin/copy-nightly; then
	alerts clear copy-failed
else
	alerts raise copy-failed --title "The nightly copy failed" \
		--detail "Run /volumes/data/bin/copy-nightly by hand to see why."
fi
```

Raising an alert that is already active changes nothing you are told, so
a script may run it every time it finds the fault. Alerts from commands
run with `elevate` all belong to the same source, so give each a key
with the script's name, as `copy-failed` here.

## Privacy

Alerts go nowhere you did not name. alertd sends nothing until the file
names a channel, only over TLS, and never writes a password, a token or a
webhook address in the log. What an alert says can name a disk's model
and serial number, a mount point or a service, and goes with the
machine's name to the email account and webhooks you set up; keep to
providers you trust with that. The active alerts, the last 1000 to 2000
changes and the messages not yet delivered are kept on the machine in
`/system/state/alerts`, in files that root alone reads. See "Privacy and
security reporting".

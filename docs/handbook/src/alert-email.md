<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# Setting up alert email

This page sets the machine up to email its alerts, through your mail
provider's submission server, and tests it. It needs an email account the
machine may send from, and about five minutes. The settings are described
in alerts.toml(5), and the chapter "Alerts: email, ntfy and webhooks"
explains the rest.

## What you need from your provider

Your provider publishes the settings a mail program uses to send mail,
under a name such as "SMTP settings" or "outgoing mail server". You need:

- the server's name, such as `smtp.example.com`;
- the port: 587, which starts TLS after connecting (STARTTLS), or 465,
  which has TLS from the first byte. Dukovo uses either. A provider that
  offers only port 25, or a server that does not offer TLS, cannot be
  used: alerts are never sent in the clear, and nor is the password;
- the account's user name, usually its full address, and a password.
  Most providers want an app password for a program: a password made in
  the account's security settings for this machine alone, which you can
  revoke without changing your own. Make one, and use it here.

The address the alerts go to need not be the account's: it can be any
address, yours or a team's list.

## Name the account

Make the file private first, as it will hold the password, then edit it:

```
$ elevate chmod 600 /system/settings/alerts.toml
$ elevate nano /system/settings/alerts.toml
```

Add an `[email]` table to the comments that are there:

```
[email]
to = ["you@example.com"]
server = "smtp.example.com"
user = "alerts@example.com"
password = "the-app-password"
```

`to` is a list, so `["you@example.com", "team@example.com"]` sends one
message to both. The port is 587 unless you add `port = 465`. The alerts
come from `alerts@` and the machine's name unless you add `from =
"alerts@example.com"`: some providers refuse mail from any address but
the account's own, and then `from` must be that address.

If you leave out `user` and `password`, alertd sends without signing in,
which only a server on your own network allows.

## Test it

```
$ elevate service alertd reload
$ elevate alerts test
email to you@example.com: sent
```

A test mail arrives with the subject `[machine] Test alert`, a few
seconds later. If the line says `failed`, it says why in the provider's
own words, and the table below says what to change. Fix the file, then
reload and test again.

| What `alerts test` says | What to do |
| --- | --- |
| `the server refused the password (535 ...)` | The user name or password is wrong. Check for a space at either end, and use an app password if the provider asks for one. |
| `the server does not offer STARTTLS, so nothing was sent` | Port 587 on this server does not start TLS. Try `port = 465`, or the server your provider names for submission. |
| `its certificate is not for smtp.example.com` | `server` is not a name the server's certificate has. Use the name your provider gives. |
| `its certificate is not from an authority the system trusts or the file ca names` | The provider's certificate is signed by an authority Dukovo does not know: a company's own. Put its certificate in `/system/settings/ssl` and add `ca = "/system/settings/ssl/example-ca.pem"`. |
| `cannot connect to smtp.example.com:587: ...` | The machine cannot reach the server: check the network, and that the provider or a firewall does not block port 587 from this network. |
| `the server refused the sender (553 ...)` | The provider does not allow this `from`. Set `from` to the account's own address. |
| `the server refused the recipient ... (550 ...)` | The provider will not deliver to that address. Check it for a typo. |
| `No channels are set up` | The file was not read. `elevate log alertd` says why; the usual cause is a file others can read: `elevate chmod 600 /system/settings/alerts.toml`. |

## Only the alerts that matter

To be mailed only for critical alerts, add `severity = "critical"`, and
`cleared = false` to leave out the mails that say an alert has cleared.
Alerts that come within 30 seconds of one another are one mail, with the
count and the first title in its subject: `[machine] 3 alerts: Disk WDC
(sda) needs a look, and 2 more`.

If the provider is down when an alert is raised, the mail is kept and
sent when it is back, for up to 24 hours (`elevate alerts status` shows
what waits).

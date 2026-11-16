alerts.toml(5) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

alerts.toml - where alerts are sent besides the screen

# SYNOPSIS

_/system/settings/alerts.toml_

# DESCRIPTION

*alertd*(8) tells the owner of each alert on the screen and at the next
sign-in. This file names the other places it tells: an *[email]* table
for the address of a mail account, and a *[[webhook]]* table for each
address an app takes alerts at, such as a phone's notifications, a chat
room or a program of the owner's (ADR 0485). Each of these is a
*channel*. The file is TOML; the default file holds comments only, with
an example of each, and alertd sends nothing beyond the screen until a
channel is named in it.

*service alertd reload* (*service*(8)) has alertd read the file again; it
reads it when it starts too. *alerts test* (*alerts*(8)) sends a test
through every channel and says how each went, so run it after each change.
The commands *alerts email*, *alerts webhook* and *alerts channel* write
the file for you, and the web console's Alerts page runs them (*Changing
it by command* below).

```
[email]
to = ["ops@example.com"]
server = "smtp.example.com"
user = "alerts@example.com"
password = "app-password"

[[webhook]]
kind = "ntfy"
url = "https://ntfy.sh/server-alerts-7f3a9c"
```

# KEEPING IT SECRET

The file holds secrets: a password, tokens, and the addresses of Slack
and Discord, which are tokens themselves. Once it names a channel,
alertd reads it only when it is root's and nobody else can read it. A
file that another user owns or that the group or others can read is
refused, no channel is set up, and the log says what to do:

```
alertd: /system/settings/alerts.toml can be read by others; chmod 600 /system/settings/alerts.toml
```

*chmod 600 /system/settings/alerts.toml*, and *chown root* when it is
another's. alertd never prints a password, a token or an address in the
log or in the output of *alerts*: a channel is shown by its kind and its
host.

# EMAIL

*[email]* is the account of a mail provider, which alertd sends through
as a mail program does, by the provider's submission server. It is one
table, and it sends one message to all the addresses.

*to =* _list_
	The addresses the alerts go to, such as *["ops@example.com"]*.
	It is needed.

*server =* _text_
	The provider's submission server, a host name, such as
	*smtp.example.com*. The port goes in *port*. It is needed.

*port =* _number_
	*587* (the default) connects and then starts TLS with STARTTLS
	(RFC 3207); *465* has TLS from the first byte (RFC 8314). No
	other port is taken.

*user =* _text_, *password =* _text_
	The account to sign in with, which most providers need for
	submission. The two go together. Many providers want an app
	password, made for this use, in place of the account's own. alertd
	signs in only once the connection is encrypted, by AUTH PLAIN
	(RFC 4616), or by AUTH LOGIN when that is all the server offers.

*from =* _text_
	The sender's address. It is *alerts@* and the machine's name by
	default; some providers refuse any but the account's own address.

*ca =* _path_
	The PEM file of a certificate authority to trust besides the
	system's (*dukovo-certificates*(7)), for a provider or a company
	server whose certificate it signed. It must be in
	_/system/settings/ssl_ or a folder in it.

*severity =* _text_, *cleared =* _true_ | _false_, *about =* _list_
	What the channel is told (below).

alertd never sends email without TLS, and never a password without it:
a server on port 587 that does not offer STARTTLS is refused before
anything is sent, and says why. The server's certificate must come from
an authority the system trusts or the file *ca* names, and be valid for
the *server*'s name; no setting turns that off.

A message is plain text in UTF-8, with the machine's name in the subject
and the alerts' words in the body (*alertd*(8)). It is marked as sent by
a program (*Auto-Submitted: auto-generated*), so that a vacation reply
does not answer it. The channel is named *email to* and the first
address, with *and* _N_ *more* when there are others.

# WEBHOOKS

A *[[webhook]]* table is one address, and there may be any number.

*kind =* _text_
	What the address speaks: *ntfy*, *matrix*, *slack*, *discord* or
	*json*. It is needed.

*url =* _text_
	The address. It must be *https*; only a *json* webhook may use
	*http*, and then only to a machine on the owner's own network:
	*localhost*, a loopback address, a private IPv4 address or an IPv6
	unique-local or link-local one. It is needed.

*token =* _text_
	A bearer token the server asks for in the *Authorization* header,
	for *ntfy*, *matrix* and *json*. Matrix needs one. Slack and
	Discord have none, as their address holds the secret.

*room =* _text_
	For *matrix* only: the room's ID, such as *!abc123:example.org*. It
	is needed there.

*severity =* _text_, *cleared =* _true_ | _false_, *about =* _list_
	What the channel is told (below).

The channel is named for its kind and the host of its address, such as
*ntfy (ntfy.sh)*; two with the same name are numbered. A server's
certificate must come from an authority the system trusts
(*dukovo-certificates*(7)), which is Mozilla's list unless the owner
replaced it.

Each kind is sent as that service documents it:

*ntfy*
	A POST to the address, which is a topic's, such as
	_https://ntfy.sh/my-topic_. The message's subject is the
	notification's title, its body the text, and the priority is 5 for
	a critical alert, 4 for a warning and 3 for the clearing of one.
	On a public server the topic's name is the only secret there is:
	make it long and not a word, or run a server of your own and use
	*token*.

*matrix*
	A message of type *m.text* to the room, through the homeserver
	whose address *url* is, as the bot or user the *token* is the
	access token of; that user must be in the room. Each message has
	its own transaction ID, which is the same on every try, so that the
	room shows it once.

*slack*
	An incoming webhook's address, which Slack makes in an app's
	settings. The message is the subject, a line break and the body.

*discord*
	A webhook's address, from a channel's integrations. The message is
	the same text, cut to the 2000 characters Discord takes.

*json*
	The changes as data, for a program of the owner's, as a POST of
	*application/json*:

	```
	{"version":1,"host":"server","changes":[{"state":"raised",
	"source":"metricsd","key":"fs-full:/data","severity":"warning",
	"title":"/data is 97% full","detail":"...","at":1794996131}]}
	```

	(on one line). *state* is *raised*, *cleared*, *event* or *test*;
	*at* is seconds since 1970; *changes* has every change in the
	message. Answer with any 2xx status.

A message is delivered when the server answers with a 2xx status. Any
other answer is said as it came, so that *alerts test* names what to fix:
*ntfy answered 403 Forbidden: forbidden*.

# BROWSERS

A browser that asks for alerts from the web console is a channel too,
and alertd keeps it in a file of its own, not here (*alertd*(8),
*Browsers*). The *[push]* table says one thing about them:

*private =* _list_
	The networks a browser's push service may be on besides the
	internet, each an address or an address and its prefix length:
	*["10.0.2.2", "fd00::/8"]*. Without it, alertd posts only to a
	push service whose every address is on the internet, so that a
	page cannot make it post into the local network. List a network
	only for a push service of one's own on it.

# WHAT A CHANNEL IS TOLD

*severity =* _text_
	*warning*, the default, sends every alert and every clearing;
	*critical* sends only the critical alerts, and the events and
	clearings of those. Any other word is a mistake.

*cleared =* _true_ | _false_
	Whether the channel is told when an alert clears. *true* is the
	default; a phone that should buzz only for trouble may say *false*.

*about =* _list_
	The kinds of alert the channel is told of, a list of *"data"*
	(data at risk), *"stopped"* (something stopped) and *"hardware"*
	(hardware running hot), such as *["data", "hardware"]*. An alert
	says what it is about (*alertd*(8), *Kinds of alert*), and goes only
	to the channels that are told of that kind. Without *about*, the
	channel is told of every kind, as before. An alert that is about
	none goes to every channel.

A test always goes to every channel. The screen is told of every change
whatever these say.

A channel that is told of some kinds hears nothing of the others: a
phone that is told of *["data"]* does not buzz for a certificate that is
running out. The alert is still on the screen and in *alerts list*, and
in every channel that is told of its kind.

# MISTAKES

alertd says what is wrong in the log, a line for each mistake that names
the file, the channel and the setting, at each start and reload (*log
alertd*), and leaves out the channel it is in while the others go on. A
setting it does not know, a value of the wrong kind, a port that is not
587 or 465, an address that is not an email address or a host, a *url*
that is not https, a *ca* outside _/system/settings/ssl_, a user without
a password, a Matrix webhook with no room or token, a *room* or a *token*
where the kind has none, and a line that is not TOML (in which case there
are no channels at all) are mistakes.

A mistake in *about* is the one that leaves no channel out. A word that is
no kind of alert is left out of the list, and the log says so with a
warning (ALR-106); a list that is empty, has no word that is a kind or is
not a list is read as no *about*, and the channel is told of every kind. A
mistake can only tell a channel more than it asked for, never less, so no
alert is lost to it.

A release before the one that has *about* does not know the setting, and
leaves out every channel that has it, as it does for any setting it does
not know. After going back to such a release, *alerts channel* _channel_
*--about all* takes the setting out of the channel, or the file can be
edited by hand; the channels without *about* go on as before.

# CHANGING IT BY COMMAND

The file can be written by hand, and the commands of *alerts*(8) write it
too, which is what the web console's Alerts page runs:

*alerts email set* *--to* _address_ *--server* _host_ [...]
	Writes the *[email]* table, from the options *--to*, *--server*,
	*--port*, *--from*, *--user*, *--severity*, *--cleared* and
	*--about*. The password is the first line of the standard input
	with *--password -*, never an argument. A setting the options do
	not give is left out of the table, except *ca*, which is kept.

*alerts email remove*
	Takes the table out.

*alerts webhook add* _kind_ [...]
	Adds a *[[webhook]]* table after the others, from *--room*,
	*--severity*, *--cleared* and *--about*. The address, and the
	token if the kind has one, are the first two lines of the standard
	input.

*alerts webhook remove* _number_
	Takes out the *[[webhook]]* table at that place in the file,
	counted from 1 as *alerts status --json* says. The ones after it
	move up.

*alerts channel* _channel_ *--about* _words_
	Writes or takes out the *about* of *email*, *webhook:*_number_ or
	a browser, *push:*_id_.

Each of them reads the file as alertd would before it writes it, and
refuses a change that alertd would leave out, with alertd's own words,
and the file stays as it was. They keep the comments of the file and the
order of its keys, and write it whole or not at all. They make it root's
and mode 600 first, since it holds secrets. Then alertd is asked to read
it again.

# EXAMPLES

Email through a provider's submission server, for critical alerts only,
and to two people:

```
[email]
to = ["ops@example.com", "me@example.org"]
from = "alerts@server.example.com"
server = "smtp.example.com"
user = "alerts@example.com"
password = "app-password"
severity = "critical"
```

The same through port 465, to a company's server whose certificate
the company's authority signed:

```
[email]
to = ["ops@example.com"]
server = "mail.example.com"
port = 465
ca = "/system/settings/ssl/example-ca.pem"
```

A phone with the ntfy app that buzzes for data at risk and for hardware,
and for nothing else, and a Matrix room that hears everything:

```
[[webhook]]
kind = "ntfy"
url = "https://ntfy.sh/server-alerts-7f3a9c"
about = ["data", "hardware"]

[[webhook]]
kind = "matrix"
url = "https://matrix.example.org"
room = "!abc123:example.org"
token = "syt_..."
severity = "critical"
```

Slack and Discord, and a program on the owner's own network:

```
[[webhook]]
kind = "slack"
url = "https://hooks.slack.com/services/T000/B000/XXXX"

[[webhook]]
kind = "discord"
url = "https://discord.com/api/webhooks/0000/XXXX"

[[webhook]]
kind = "json"
url = "http://192.168.1.20:8080/alerts"
token = "a-long-secret"
```

# FILES

_/system/settings/alerts.toml_
	This file.

_/system/state/alerts/push.json_
	The browsers, which are not in this file (*alertd*(8)).

_/system/settings/ssl_
	Where the certificate authorities it names are.

# SEE ALSO

*alertd*(8), *alerts*(8), *dukovo-certificates*(7), *dukovo-privacy*(7),
*service*(8)

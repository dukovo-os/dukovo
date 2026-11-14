sendmail(8) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

sendmail - send a message through the mail account alerts.toml names

# SYNOPSIS

*sendmail* [*-t*] [*-i*] [*-f* _sender_] [*-F* _name_] [_recipient_ ...]

# DESCRIPTION

sendmail reads a message from the standard input and sends it, as the
programs that mail from a Linux server (*cron*, scripts, *mail*) expect
_/usr/sbin/sendmail_ to (ADR 0478). It is no mail server and keeps no
queue: the message goes at once to the provider's submission server
that the *[email]* table of _/system/settings/alerts.toml_ names
(*alerts.toml*(5), ADR 0485), with that table's account and *from*
address, and the command ends when the server has taken it or has not.
The connection is TLS always: STARTTLS on port 587, or TLS from the first
byte on 465, and a server that does not offer it, or whose certificate
the system does not trust, is refused before anything is sent.

The recipients are the arguments. With *-t* they are the addresses in the
message's *To*, *Cc* and *Bcc* headers too, and its *Bcc* headers are
removed before it is sent. An address without a domain gets the
machine's name. A message with no *Date* or *From* header gets them: the
time, and the sender's name at the machine's.

The settings file is root's and others cannot read it (*alerts.toml*(5)),
so a person who is not root runs *elevate sendmail*; a job that the
system runs for *crontab*(1)'s *MAILTO* is sent as root.

# OPTIONS

*-t*
	Take recipients from the headers too.

*-f*, *-r* _sender_
	The envelope sender, as the server sees it, which is what bounces go
	to. The empty one, *-f ''*, is the null sender. Without it the
	*from* of alerts.toml is the sender, which some providers insist is
	the account's own.

*-F* _name_
	The name before the address of a *From* header that is added.

*-i*, *-oi*, *-v*, *-m*, and the other *-o* options
	Are taken and change nothing: the message always ends where the input
	does, so a line of one dot does not end it.

*-bm*
	Is the default.

# WHAT DUKOVO HAS INSTEAD

*-bd*, *-bp*, *-q* and the other modes that run a mail server or read a
queue say that Dukovo has none and end with status 64. A message that
cannot be sent now is lost unless its sender tries again; the command's
status, and a line in the log, _sendmail: not sent to_ ... (*log*(1)),
say so.

# EXIT STATUS

As *sysexits*(3): 0 when the server took the message; 64 for a wrong
command line or no recipients; 65 for a message over 25 MiB, which
servers refuse; 69 when the server or its certificate refused the
message for good (a recipient or the sender refused, the password, no
TLS), the server's own words on the standard error; 74 when the input
cannot be read; 75 when the server cannot be reached, goes silent, or
says to try again later; 78 when alerts.toml names no *[email]* server.

# FILES

_/system/settings/alerts.toml_
	The mail account.

# SEE ALSO

*alerts.toml*(5), *alertd*(8), *crontab*(1), *elevate*(8), *service*(8)

report(1) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

report - write a problem report, read all of it, and send it, save it or hand it to a phone

# SYNOPSIS

*report* [*--security*] [*--save*[=_file_]] [*--port* _port_]

*report crash* [_id_] [*--security*] [*--save*[=_file_]] [*--port* _port_]

*report failure* [*--security*] [*--save*[=_file_]] [*--port* _port_]

*report list*

*report status* [_number_]

*report delete* _number_

*report forget* _id_

*report retry*

# DESCRIPTION

*report* tells the people who make Dukovo about a problem (ADRs 0531 and
0532). It asks what the problem is with and a few questions, has
*reportd*(8) put the report together, shows all of it, and sends it only
when a key says so. What is shown is exactly what is sent: reportd sends
the bytes the person saw and nothing else, and nothing leaves the computer
before a key or *--save* asks for it.

```
$ report
What is the problem with?

  1  A command or an app
  2  A service
  3  The hardware
  4  Installing or updating Dukovo
  5  Something else

Choose 1 to 5: 3

Finish each answer with an empty line.

What happened?
The screen goes black for a second when the lid is closed and opened.

What did you expect?
It stays on.

How can it be made to happen again? (Enter to skip)

That is the whole report: 14 lines, 612 bytes. Nothing has left this computer.
1 piece was cut out as names or addresses, each shown as a mark such as [user].

Press S to send it, E to edit it, F to save it to a file, P to download it on a
phone, or C to cancel: s
Sent as report 7. report status 7 shows where it stands.
```

Each answer ends with an empty line. The first is needed, except for a
failed start, where every answer may be skipped; the others may be left
empty. The report is plain text
with these parts: a first line, the version of Dukovo and its channel, what
the report is about, what the computer says of itself, the answers, and,
for a crash, the crash and the log lines that belong to it.

*What the computer says of itself* is the maker and model the firmware
names, the processor, the memory, the vendor and device numbers of the
PCI and USB devices (*8086:15b8*, never a serial number), and the language
setting. A fact the computer does not give is left out.

*What is cut out.* reportd cuts out of every line, the person's own words
too, what names a person or a machine, and puts a mark in its place so that
the preview shows where. The marks are:

[[ *Mark*
:- *Replaces*
|  *[user]*
:  the name of an account, and the name in a home folder's path, _/users/[user]_
|  *[host]*
:  the computer's name and the names in its hosts file; the default name *dukovo* stays
|  *[IPv4]*, *[IPv6]*
:  an address; the loopback and unspecified addresses stay
|  *[MAC]*
:  a hardware address
|  *[email]*
:  an email address
|  *[Wi-Fi]*
:  the name of a Wi-Fi network the computer knows
|  *[serial]*
:  a serial number of the firmware or a disk
|  *[machine]*
:  the machine ID

The computer keeps no Wi-Fi network settings yet, so *[Wi-Fi]* cuts nothing
at present. Cutting finds what the computer knows and what is shaped like an
address; it cannot know every secret. Read the preview before pressing *S*:
*E* edits it.

# COMMANDS

With no command, *report* asks what the problem is with. For a command or
an app it asks the name; for a service it shows the services there are and
asks which. The log of a service goes with its report, which only an
administrator may send: anyone else is told to try *elevate report*.

*crash* [_id_]
	Reports a crash that reportd keeps (ADR 0530): the newest one not
	reported yet, or the one named by _id_, as *list* shows it, such as
	_20261113T100211-4012_. The report holds what happened, the
	signature, the frames of the stack and, for a service, its last log
	lines. A crash is reported once: naming one that was reported says
	which report it is.

*failure*
	Reports the failed start that the notice at the sign-in screen tells
	of (ADR 0501): the notice, what *changes --failed* lists, and the
	warnings and errors of the failed start. The questions may be
	skipped. Only an administrator may. On the sign-in screen, *R* signs an
	administrator in to run this command (*login*(1)).

*list*
	Shows the crashes that are kept, the reports that wait to be sent and
	the reports that were sent, each as a table. A crash line has its
	ID, when it happened in UTC, the program, how many times it was seen,
	the report it went into, and what happened.

*status* [_number_]
	Shows where a report stands, as the reports server says: *Received*,
	*Confirmed*, *A duplicate of report* _n_, *Fixed in* _version_ or
	*Will not be fixed* with the reason. Without a _number_ it shows every
	report sent from this computer. When the server cannot be asked, the
	line says why.

*delete* _number_
	Deletes a report sent, on the reports server and here.

*forget* _id_
	Puts a crash away: it is not offered again by *report crash*.

*retry*
	Sends the reports that wait in the queue now. A report that cannot
	be sent, as on a computer with no network, is kept and tried again by
	reportd; *retry* does not wait for its next try. It says what became
	of each report.

# THE PREVIEW

The text is shown whole, in the pager, *$PAGER* or *less*, or plain when the
output is no terminal. Then one key, without Enter, chooses; any other key
asks again. The keys are letters in either case.

*S*
	Sends the report to the reports server. When the computer has no
	network, or the server does not answer, the report is kept in the
	queue and sent when it can be; *report list* shows it and *report
	retry* tries it again.

*L*
	Leaves the logs out: reportd drafts the report again without its log
	sections. It is offered only when the report has a log and the text
	was not edited.

*E*
	Opens the text in an editor: *$VISUAL*, else *$EDITOR*, else *nano*. The
	editor works on a copy in a file that only its user reads, which is
	removed when the editor ends. What the editor leaves is cut again by
	reportd, so that a name typed into it is cut like the rest, and shown
	again. An editor that fails leaves the text as it was.

*F*
	Saves the report to a file; see SAVING.

*P*
	Offers the report once to a phone; see SAVING.

*C*
	Cancels. Nothing was sent or saved.

When the input is not a terminal, as in a script, a key is the first letter
of a line, and an empty line is Enter.

# SAVING

With *F*, or *--save* and a file, the report is written exactly as shown.
The file is made so that only its user reads it (mode 0600) and is written
under another name in the same folder, synced, renamed, and the folder
synced, so that a stick pulled out at any moment holds the whole file or
none. A file that is there already is not replaced. A stick's folder works
as any path. With *F* the name is asked, _report.txt_ in the current
folder if only Enter is pressed.

With *P*, or *--save* and no file, the report is served once over HTTPS to
a phone on the same network. *report* prints

- an address, _https://192.168.1.20:43517/_token_/report.txt_, with this
  computer's IPv4 address first, an IPv6 one if it has no IPv4, and the
  token 128 random bits;
- the same address as a QR code, which a phone's camera reads;
- the SHA-256 fingerprint of the certificate; and
- *Waiting for one download (10 minutes)*.

The certificate is made for this download alone, for the computer's own
addresses, and is valid for days. A phone does not know it and warns of it.
That is the warning working: compare the fingerprint the phone shows with
the one on the screen, and go on if they are the same. The first request for
the address gets the report, as a text file to save; every later request for
it gets *410 Gone*, any other address *404*, and the listener closes after
the first download or after 10 minutes, whichever comes first. Nothing is
sent from the computer to anyone else, and *Ctrl+C* stops the wait. A
download that broke before its last byte does not count, and the phone
may ask again.

A program that a person runs and that listens is reached from this computer
alone until a rule of the firewall allows it (*firewall*(8)). The phone
reaches the download only when an administrator has allowed its port, from
another terminal, while it waits:

```
$ elevate firewall allow tcp 43517
```

The port is chosen by the system, and the screen names it. To make the rule
first, give the port with *--port*, and take the rule away after with
*elevate firewall remove*.

# OPTIONS

*--security*
	Sends the report into the security box, encrypted to the security
	team's key: the reports server keeps no readable copy and no one
	else reads it. *report* says so under the preview. When no key is
	known, reportd refuses, and the refusal says where to write
	instead. It cannot be used with *--save*.

*--save*[=_file_]
	Saves the report instead of sending it. The preview is shown, then
	the report is written to _file_, or served once to a phone when no
	file is given, without asking a key. A name that is the word of a
	command, such as *list*, is the command and not the file: write
	*--save=list* or *--save ./list* for the file.

*--port* _port_
	The port the download to a phone listens on, from 1 to 65535, for
	*P* and for *--save* without a file. Without it the system chooses a
	free one. It is for making a firewall rule before the download starts.

*--help*
	Print the usage.

# EXAMPLES

Report the newest crash, and read the preview first:

```
$ report crash
Reporting the crash of nginx, SIGSEGV (address not mapped), 2026-11-13 10:02 UTC
```

Write the report to a file and look at it later, on a computer that is not
connected:

```
$ report --save report.txt
```

See where the reports sent stand:

```
$ report status
Report 7: The screen goes black for a second when the lid is closed and opened.
  Sent 2026-11-13 10:05 UTC
  Status: Fixed in 0.16.1
```

# EXIT STATUS

0 when what was asked was done: sent or put in the queue, saved,
downloaded, shown, or cancelled with *C*. 1 when it could not be: a
mistake in the command line, reportd not running (the message names
*elevate service reportd start*), a refusal by reportd, which is shown in
its own words, no answer from reportd within its time (20 seconds, or 150
for what asks the reports server), a file that is there already, input that
ended or *Ctrl+C* before a key, and, for *retry*, a report that still
waits.

# ENVIRONMENT

*VISUAL*, *EDITOR*
	The editor of *E*; *nano* if neither is set.

*PAGER*
	The pager of the preview; *less* if it is not set.

*TMPDIR*
	Where the editor's copy is made; _/tmp_ if it is not set.

# FILES

_/system/live/reports/socket_
	The socket of *reportd*(8) that *report* asks.

_report.txt_
	The name *F* proposes, in the current folder.

# SEE ALSO

*reportd*(8), *login*(1), *firewall*(8), *elevate*(8), *changes*(1),
*dukovo-privacy*(7), *systools*(1)

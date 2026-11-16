report(1) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

report - write a problem report, read all of it, and send it, save it or hand it to a phone

# SYNOPSIS

*report* [*--security*] [*--save*[=_file_]] [*--port* _port_]

*report crash* [_id_] [*--security*] [*--save*[=_file_]] [*--port* _port_]

*report failure* [*--security*] [*--save*[=_file_]] [*--port* _port_]

*report list* [*--json*]

*report status* [_number_] [*--json*]

*report delete* _number_

*report forget* _id_

*report text* _draft_ *--sha256* _hash_

*report retry* [*--json*]

*report crashes* [*--json*]

*report draft* *--about* _kind_ *--happened* _text_ [*--crash* _id_] [*--name* _name_] [*--expected* _text_] [*--again* _text_] [*--without-logs*] *--json*

*report send* _draft_ *--sha256* _hash_ *--title* _text_ [*--crash* _id_] [*--security*] *--json*

*report redact* _draft_ *--sha256* _hash_ *--json*

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

*list* [*--json*]
	Shows the crashes that are kept, the reports that wait to be sent and
	the reports that were sent, each as a table. A crash line has its
	ID, when it happened in UTC, the program, how many times it was seen,
	the report it went into, and what happened. With *--json* it prints
	the reports that wait and the reports that were sent as the JSON
	described under *THE FORMS THAT PRINT JSON*, and asks the reports
	server where each sent report stands, as *status* does, so it needs
	the network to be as quick as that.

*crashes* [*--json*]
	Shows the crashes that reportd keeps, newest first, as the first
	table of *list* does, or with *--json* as JSON. A person sees their
	own crashes; an administrator, with *elevate*(8), sees all of them.

*status* [_number_] [*--json*]
	Shows where a report stands, as the reports server says: *Received*,
	*Confirmed*, *A duplicate of report* _n_, *Fixed in* _version_ or
	*Will not be fixed* with the reason. Without a _number_ it shows every
	report sent from this computer. When the server cannot be asked, the
	line says why. With *--json*, as described under *THE FORMS THAT PRINT
	JSON*.

*delete* _number_
	Deletes a report sent, on the reports server and here.

*forget* _id_
	Puts a crash away: it is not offered again by *report crash*.

*text* _draft_ *--sha256* _hash_
	Prints the text of a draft that reportd keeps for you, as it is,
	while its SHA-256 is _hash_ (ADR 0568). The web console's Report a
	problem page keeps its drafts so, and this is how it hands one to a
	phone; a draft lasts 60 minutes from its last change.

*retry* [*--json*]
	Sends the reports that wait in the queue now. A report that cannot
	be sent, as on a computer with no network, is kept and tried again by
	reportd; *retry* does not wait for its next try. It says what became
	of each report. With *--json* it prints them as JSON, and ends with 0
	whatever became of each.

*draft* *--about* _kind_ *--happened* _text_ ... *--json*
	Drafts a report from the options, as the questions of *report* would
	draft it, and prints the draft as JSON. reportd keeps the draft for
	the person who asked for it, for 60 minutes from its last change, at
	most 8 for a person and 64 in all, and the JSON names it by its ID and
	the SHA-256 of its text. Nothing is sent. *--about* says what the
	report is about: *crash* (with *--crash* _id_), *service* or
	*program* (with *--name* _name_), *failure* (the failed start,
	which only an administrator may report; its answers may be left out),
	*hardware*, *updates* (or *update*) or *other*. *--happened*,
	*--expected* and *--again* are the three answers, each at most 2,000
	characters, and the first is needed except for a failed start.
	*--without-logs* leaves the logs out. It prints JSON alone, so
	*--json* is needed.

*send* _draft_ *--sha256* _hash_ *--title* _text_ ... *--json*
	Sends the draft exactly as it was drafted or last redacted, and
	prints what became of it as JSON. _hash_ is the SHA-256 of the text
	the person read: a draft that has since changed is refused
	(*RPT-102*), and one that has ended, or is another person's, is
	refused too (*RPT-101*). _text_ of *--title* is what the report is
	kept under here. *--crash* names the crash it reports, so that *list*
	says so. With *--security* the report goes to the security team,
	encrypted, as it does for *report --security*; with no key set that
	is refused with *RPT-103*. A draft is dropped once it is sent or
	queued, so a second *send* is *RPT-101*.

*redact* _draft_ *--sha256* _hash_ *--json*
	Reads the edits as JSON on standard input, at most 64 KiB, applies
	them to the draft and prints the draft as JSON, with a new hash. The
	edits are an object whose *cuts* are the whole list of cuts, which
	replaces the earlier ones, and whose *answers* may give the three
	answers again. A cut names a *section* of the report (*crash*,
	*failed-start*, *log* or *start-log*) and a *line* of it counting from
	0, with *through* for the last of several lines, or with *from* and
	*to* for the characters of that line, counted from 0; the part cut
	becomes *[cut]*. A line the system gathered can be cut and never
	typed into (*RPT-105*), and the lines that make the report what it is
	cannot be changed (*RPT-104*). It changes nothing that is kept but
	the draft.

# THE FORMS THAT PRINT JSON

*draft*, *send* and *redact* print one line of JSON, and *crashes*,
*list*, *status* and *retry* do with *--json*, for the web console and
for a script. The text of the interactive *report*, and of *--save*, is
not changed. Each answer has _format_, the version of its members, 1. A
member that is not known is *null*.

*draft* and *redact* print the draft:

```
{"format":1,"draft":"0123456789abcdef0123456789abcdef","sha256":"ab12...","bytes":612,"text":"...","cuts":1,"has_logs":false,"cut_by":{"user":1,"host":0,"ipv4":0,"ipv6":0,"mac":0,"email":0,"wifi":0,"serial":0,"machine":0,"you":0},"parts":[{"title":"What happened","bytes":71}],"sections":[],"typed":[{"id":"happened","title":"What happened","first_line":9,"lines":1}],"cut_by_you":{"lines":0,"parts":0}}
```

_draft_ is the kept draft's ID and _sha256_ the hash, in hex, of _text_,
which is _bytes_ long: the bytes that are sent. _cuts_ is how many pieces
were cut out, _cut_by_ how many by each kind of mark (*user*, *host*,
*ipv4*, *ipv6*, *mac*, *email*, *wifi*, *serial*, *machine*) and by the
person (*you*). _has_logs_ says whether the text has log sections that
*--without-logs* leaves out. _parts_ are the sizes of the parts of the
report, each with its _title_, and _sections_ and _typed_ say where the
sections the system gathered and the person's own answers are in the
text: each has an _id_, a _title_, the _first_line_ of the text it
starts at (from 0, after its heading) and how many _lines_ it has.
_cut_by_you_ is the lines and the parts of lines that *redact* cut.

*send* prints:

```
{"format":1,"number":7,"state":"sent","why":null}
```

_state_ is *sent*, with the _number_ the reports server gave it, or
*queued* while it waits to be sent, with _number_ *null* and _why_ the
reason, as that no network is up. A queued report is tried again by
reportd; *list* shows it and *retry* sends it now.

*crashes* prints _crashes_, newest first, each an object of the crash's
_id_, _kind_, _program_, _version_ and _service_, _at_ms_ (when it
happened, in milliseconds since 1970), _what_ happened, how many times it
was _seen_ and the number of the report it went into, _reported_.

*list* prints _reports_, those that wait first, oldest first, and then
those that were sent, newest first, each with _number_ (none while it
waits), _title_, _state_ (*queued* or *sent*), _queued_ms_, _sent_ms_,
_tries_ and _next_ms_ for one that waits, _receipt_ (the ID the queue
gave it, for one that waits), _security_ (whether it goes to the security
team, encrypted) and _why_ (why the last try failed). *status* prints
_reports_, each with _number_, _title_, _sent_ms_, _status_ in the words
of *status*, _unreachable_ (true when the server could not be asked, and
then _status_ is *null*) and _why_. *retry* prints _reports_, each the
_receipt_ of one that was waiting, its _number_ if it was sent, its
_state_ and _why_.

A mistake in the command line, or in the edits *redact* reads, ends with
status 2, and a refusal by reportd with 1, so that a script tells them
apart.

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
	instead (*RPT-103*). It cannot be used with *--save*.

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

*--json*
	Print JSON, as *THE FORMS THAT PRINT JSON* says: for *list*,
	*status*, *retry* and *crashes*, and always for *draft*, *send* and
	*redact*.

*--sha256* _hash_
	The SHA-256, in hex, of the text of the draft that *text*, *send* or
	*redact* names. The draft is read only while its text still has it.

*--about* _kind_, *--crash* _id_, *--name* _name_
	What *draft* is about, as *draft* says. *--crash* also goes with
	*send*.

*--happened* _text_, *--expected* _text_, *--again* _text_
	The answers of *draft*, in place of the three questions.

*--without-logs*
	*draft* leaves out the logs.

*--title* _text_
	What *send* keeps the report under.

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

Draft a report about the hardware for a program, whose answer has the
draft's ID and hash, and send it as it was drafted:

```
$ report draft --about hardware --happened 'The screen goes black.' --json
{"format":1,"draft":"0123456789abcdef0123456789abcdef","sha256":"ab12..."}
$ report send 0123456789abcdef0123456789abcdef --sha256 ab12... --title 'Black screen' --json
{"format":1,"number":7,"state":"sent","why":null}
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
mistake in the command line (2 for *draft*, *send* and *redact*, which a
script tells from a refusal), reportd not running (the message names
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

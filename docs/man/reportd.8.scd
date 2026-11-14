reportd(8) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

reportd - keep a record of each crash, and send the problem reports that
people approve

# SYNOPSIS

*reportd*

# DESCRIPTION

reportd keeps a record of each crash on the machine (ADRs 0530 and 0531):
a program that a signal ended, a service that ended with a panic, and the
kernel's own panic. It offers each new kind of crash once, at the next
sign-in, drafts the text of a report when *report*(1) asks it to, and sends
the text a person approved to the reports server, or keeps it in a queue
until the server can be reached. Spark runs it as the *reportd* service,
after *logd*(8) and before *alertd*(8).

*report*(1) is its command, and the only way to make or send a report:
reportd sends nothing that a person did not see in full and approve, and
has no setting that sends reports by itself. A report goes to the server
that *report.toml*(5) names, which is the project's own unless the file
says otherwise. *dukovo-privacy*(7) lists what a report holds and what is
never sent.

## What a crash is

There are three kinds.

*signal*
	A program that ended by a signal of a fault, SIGSEGV, SIGBUS,
	SIGILL, SIGTRAP, SIGFPE or SIGSYS, with no handler for it, or by
	SIGABRT. When such a process ends, the kernel leaves a record of it
	for reportd (*radix-crashes*(4)). A process that another signal
	ended, such as SIGKILL or SIGTERM, leaves none, and neither does a
	process the system cannot run without, such as Spark.

*panic*
	A service built on Rust's standard library that panicked. The
	program prints where and why to its standard error, which Spark
	copies to the log, and ends with no signal, so the kernel has
	nothing to record. reportd follows the log for the lines of services
	that say *panicked at*, and keeps a crash with the place of the panic
	(file, line and column) and the message, which is the lines the same
	process wrote next, up to the note on *RUST_BACKTRACE*. A panic that
	the program survives, in a thread it goes on without, counts too. A
	panic of a program that no service runs is not kept.

*kernel*
	The kernel's own panic. Each file that *dumps*(8) saved in
	_/system/state/crash_ in the last 30 days becomes a crash, once, with
	the dump's *Panic* section alone: the report and the backtrace as
	the consoles showed them. The words of the kernel's stacks, which can
	hold secrets, are never kept. reportd remembers which dumps it took
	in _/system/state/reports/dumps.json_.

## What is kept of a crash

For each crash reportd keeps, as a file of _/system/state/reports/crashes_
that only root reads, the program's file name and version, the service it
ran for, the user it ran as, the time, the signal and what it means, the
address that faulted, the frames, the signature, how many times a crash
with the same signature was seen, and whether it has been reported or put
away. It keeps no registers, no bytes of the stack and no environment: the
kernel's record has them, and reportd drops them once it has unwound the
stack.

To unwind it, reportd reads each program the record maps, once, and
follows the call frame information the program carries, or the chain of
frame pointers where an address has none. It stops after 64 frames, at an
address no mapping holds, or at a frame that does not move up the stack. A
frame is the module and the offset in its file, with the module's build
ID, and the function's name and offset where the program's symbol table
names it. A program built without symbols still has a signature, made of
offsets.

reportd reads the programs of _/system/commands_, _/system/libraries_
and _/system/tests_, which its service sees. A program elsewhere, in a
home, in _/shared_ or on a volume, is not read, so that reportd, which
runs as root, never parses a file a user could have made: its frames
come from the chain of frame pointers alone, as its file and offset.

A crash belongs to a service when the name of the resource domain of its
process, which the kernel's record holds (*radix-crashes*(4)), is the name
of a service that Spark lists (*radix-scheduling*(7)).

At most 100 crashes are kept, within 8 MiB. Each is deleted after 30 days,
or at once by *report forget*.

## The signature

A crash's signature says which crash it is, so that the same one is not
offered twice. It is made of

- for a *signal*, the program's file name, the signal, and the first three
  functions of the stack;
- for a *panic*, the program's file name, the word *panic*, the place of
  the panic, and its message with the numbers and the quoted parts left
  out;
- for a *kernel* crash, the word *kernel*, the panic's message without its
  numbers, and the first three functions of its backtrace.

The frames of the abort and panic machinery (*abort*, *raise*, *core::panicking*,
*std::panicking*, *rust_panic* and the like) are left out first, so that a
program that aborts from two places in its own code has two signatures. A
function that has no name stands as its module and offset. The signature's
ID is the first 16 hexadecimal digits of the SHA-256 of these words.

## What is offered

A crash whose signature has not been offered before is offered once, to the
people who can report it: reportd writes a line in its notice for the next
sign-in, _/system/live/sign-in/reportd_ (*login*(1)), and sends one
notification to *notifyd*(8):

```
the service web stopped at 2026-11-13 10:02: SIGSEGV (address not mapped) in ngx_http_core_run_phases. `report crash` reports it, `report forget` puts it away.
```

(on one line). A program that is not the service it ran for is named too,
as in *nginx (the service web) stopped at*; a crash that no service ran
says *nginx stopped at*. The line goes when the crash is reported or put
away, and a signature that was offered is never offered again: reportd
remembers it for a year, in _/system/state/reports/offered.json_. A second
crash of the same kind adds a count to the first and nothing to the notice.

The log gets one warning for each crash, offered or not, with the program
or the service, what happened and the crash's ID, such as
*20261113T100211-4012*:

```
nginx (the service web): SIGSEGV (address not mapped) in ngx_http_core_run_phases; report crash 20261113T100211-4012 reports it
```

When crashes came faster than the kernel keeps their records, reportd
logs how many were lost: *1 crashes came too fast to be kept*
(*radix-crashes*(4)).

## Who sees what

reportd learns who asks from the kernel (*SO_PEERCRED*), never from the
request. Any user may ask. A user sees the crashes of the programs that
ran as them and the reports they sent. An administrator sees every crash,
and is the only one who may put a service's log, or the failed start's
log, in a report: they hold what other people's programs wrote. reportd
drafts and redacts the text as root, and shows each user only what that
user may see.

## Sending a report

reportd sends a report only when *report*(1) asks it to, with the exact
bytes the person saw in the preview. It does not change them.

The server is _https://reports.dukovo.org_ unless *report.toml*(5) names
another. reportd checks the server's certificate against the system's
roots, and the authorities of *ca*, and sends nothing over a connection
that is not encrypted. A report is one request:

```
POST /v1/reports
Content-Type: text/plain; charset=utf-8
Report-Key-Hash: <the SHA-256 of a key made for this report, in lowercase hexadecimal>
User-Agent: dukovo-report/<version>
Accept: application/json
```

with the text as the body, and the connection closed after the answer. The
server answers *201* and *{"number": N, "status": "received"}*. The key is 32
random bytes made for this report, which reportd keeps and the server knows
only by its hash. Whoever holds the key may ask the server where the report
stands, with *GET /v1/reports/N*, answered by *200* and *{"number", "status",
"duplicate_of", "fixed_in", "reason"}*, and have it deleted with *DELETE
/v1/reports/N*, answered by *204*; both carry the key, as 64 lowercase
hexadecimal digits, in *Report-Key*, and nobody else may do either. A report
the server does not know when it is deleted is taken as deleted. A request
carries no cookie, no account and nothing else.

A report for the security team (*report --security*) is encrypted with age
to the keys in the file that *security-recipients* names, before it is
queued, and the queue holds only that. It goes as
*application/age-encryption* with the header *Report-Box: security*.
Without a file of keys to encrypt to, *report --security* refuses and says
where to write instead (security@dukovo.org).

The server answers a refusal with a status of 4xx and the reason as JSON,
*{"error":"..."}*, which reportd shows in the server's own words and logs
as a warning. An answer of 5xx or 429, or no connection, is passing and
is not logged. Either way the report waits in the queue and is tried
again as below.

A report that cannot be sent, because there is no network or the server
does not take it, waits in _/system/state/reports/queue_, in a file of the
text and a file of what is known of it. reportd tries it again after 1, 5,
15 and 60 minutes, and then every hour for 30 days. A report that is
still not sent after that is dropped, and the log says so. *report list* shows what waits and why, and *report retry* tries
at once.

reportd holds the reports a person sent in _/system/state/reports/sent_:
the number, the key, the title, the time and the user. *report status* asks
the server, only when it is run, how each of the person's reports stands:
*received*, *confirmed*, *duplicate* of another, *fixed* in a version, or
*wont-fix* with a reason. *report delete* asks the server to delete one and
forgets it here.

## The socket

reportd answers on _/system/live/reports/socket_, which every user may
connect to. A program sends one request, a line of JSON of at most 384 KiB,
and reads the answer, a line of JSON for each item, after which reportd
closes the connection. Every request has an *ask*:

*{"ask":"crashes"}*
	The crashes the caller may see and has not put away, newest first,
	a line each, without
	their frames: *id* (the time and the process number, such as
	*20261113T100211-4012*), *kind* (*signal*, *panic* or *kernel*),
	*program*, *version*, *service*, *user*, *at* in seconds since 1970,
	*what* in one line, *signature*, *seen*, and *reported* with the
	report's number once it is reported. Then *{"end":true}*.

*{"ask":"draft","about":{...},"answers":{...}}*
	The draft of a report. *about* is *{"kind":"crash","id":"..."}*,
	*{"kind":"service","name":"..."}*, *{"kind":"program","name":"..."}*,
	*{"kind":"hardware"}*, *{"kind":"updates"}*, *{"kind":"failure"}*
	or *{"kind":"other"}*. *answers* holds *happened*, *expected* and
	*again*, the last two optional. *"without_logs":true* leaves out
	the log. The answer is *{"text":"...","cuts":N,"has_logs":true}*:
	the text, redacted, which is the bytes that are sent if the person
	sends it, how many pieces redaction cut out (each is marked in the
	text), and whether the text has a log that *without_logs* would
	leave out. A crash is drafted for its own user and for an
	administrator; a service's log, and the failed start, for an
	administrator alone.

*{"ask":"redact","text":"..."}*
	The same redaction for a text that the person edited, answered as a
	draft is.

*{"ask":"send","text":"...","title":"...","security":false,"crash":"..."}*
	Queues the text, as it is, and tries to send it at once. *title* is
	kept with the report on this machine, at most 80 characters, and is
	not sent apart from the text. *crash*, when there is one, is marked
	reported once the report is sent. The answer is
	*{"id":"...","number":N}* when it was sent, and *{"id":"...",
	"waiting":"..."}* with the reason when it waits in the queue.

*{"ask":"queue"}*
	The reports that wait, a line each: *id*, *title*, *since*, *tries*,
	the last *error*, *next* try and *security*. Then *{"end":true}*.

*{"ask":"retry"}*
	Tries each waiting report now, and answers a line for each, as *send*
	does, and then *{"end":true}*.

*{"ask":"status"}* or *{"ask":"status","number":N}*
	Where each of the caller's sent reports stands, or the one: a line
	with *number*, *title*, *sent*, *status*, *duplicate_of*, *fixed_in*,
	*reason*, *security* and, when the server could not be asked, *error*.
	Then *{"end":true}*.

*{"ask":"delete","number":N}*
	Deletes the caller's report from the server and from this machine.
	It answers *{"ok":true}*.

*{"ask":"forget","crash":"..."}*
	Puts the crash away: it is not offered again and *report crash*
	skips it. It answers *{"ok":true}*.

A request reportd does not take is answered with *{"error":"..."}* and the
reason.

## Settings

reportd reads _/system/settings/report.toml_ when it starts (*report.toml*(5)).
*service reportd restart* makes it read the file again. A key it does not
know, and a value it cannot use, are logged and leave the default.
Without the file, reports go to the project's server. The system brings no
file of keys for the security box yet, so *report --security* refuses, and
names the address to write to, until *report.toml*(5) names a file of keys.

## Turning it off

*crashrecords=off* on the kernel's command line (*dukovo-boot*(7)) leaves
no record of a signal: reportd still keeps the panics of services and the
kernel's dumps. *enabled = false* in reportd's service file, *service edit
reportd* (*service*(8)), stops the service, and with it every crash's
record; nothing is queued or sent while it is off.

# FILES

_/devices/crashes_
	The kernel's records of crashes (*radix-crashes*(4)).

_/system/live/reports/socket_
	The socket reportd answers on.

_/system/settings/report.toml_
	The server, its authority and the security box's keys
	(*report.toml*(5)).

_/system/resources/report/security-recipients.txt_
	The security box's keys, when the system brings them: a key on each
	line.

_/system/state/reports/crashes_
	The crashes kept, a file each, readable by root alone.

_/system/state/reports/offered.json_
	The signatures offered, and when.

_/system/state/reports/dumps.json_
	The kernel's dumps already taken as crashes.

_/system/state/crash_
	The kernel's dumps, saved by *dumps*(8), which reportd reads.

_/system/state/reports/queue_
	The reports that wait to be sent.

_/system/state/reports/sent_
	The reports that were sent, with the keys that ask the server about
	them.

_/system/live/sign-in/reportd_
	The notice the next sign-in shows.

_/system/settings/spark/services/reportd.toml_
	The service.

# EXIT STATUS

As a service, reportd does not exit but when it is stopped with SIGTERM,
with 0; it exits with 1 if it cannot start, as when it cannot make its
socket.

# SEE ALSO

*report*(1), *report.toml*(5), *radix-crashes*(4), *dumps*(8),
*alertd*(8), *notifyd*(8), *login*(1), *service*(8), *spark*(8), *log*(1),
*dukovo-boot*(7), *dukovo-privacy*(7)

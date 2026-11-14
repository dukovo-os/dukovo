<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0531. Problem reports are drafted on the machine and sent only as approved

- Status: Accepted
- Date: 2026-11-13

## Context

A crash, a panic of a service, or a kernel panic that nobody reports is
fixed only if it happens on a developer's machine. The usual answers
each send too much or ask too much:

- Windows Error Reporting, Apple's crash reporter and Ubuntu's apport
  send core files or memory excerpts, some of them by default.
  What they hold is hard for a person to read, so the choice to send is
  made blind.
- A bug tracker asks for an account, a description in its form, the
  system's version and a log the person must find and paste, and shows
  everything posted to everyone.

What a developer needs is small: what the person saw and expected, the
system's version and hardware, and for a crash a trace with function
names and a signature that groups the same crash from many machines.
What a person needs is to see every byte before it leaves, to know
names and addresses were cut out, and to know nothing is sent without
them.

## Decision

- **reportd keeps crashes; report sends what the person approved.**
  - reportd (`systools/src/reportd`) runs as a service and reads the
    kernel's crash records (ADR 0530). For each it opens the program's
    files by the paths the record names, unwinds the stack with their
    call frame information (`.eh_frame`, with `.eh_frame_hdr`'s table,
    through gimli), falls back on the frame-pointer chain where a file
    has none, and names each frame by the file's symbols with Rust's
    names demangled, up to 64 frames.
  - It follows the log for services' `panicked at` lines: a program on
    Rust's standard library unwinds on a panic and ends with status 101,
    with no signal and so no record. The location and the message's
    lines after it make the entry.
  - It takes in the kernel's saved crash dumps (ADR 0483): the `==
    Panic` section only, the report and backtrace the consoles showed,
    never a stack's words.
- **A signature makes the same crash one crash.** A program's crash is
  signed by its file's name, the signal and its first three frames that
  are not the machinery of an abort or a panic; a panic by the program,
  the location and the message with its numbers and quoted parts left
  out; a kernel panic by its message so and the first three functions of
  its backtrace. The signature's ID is the first 16 hex digits of its
  SHA-256.
- **What is kept** (`/system/state/reports`, root's alone): each crash as
  a file of JSON holding the program, its version, its service, its
  user, the time, what ended it, the trace as lines and the signature,
  and nothing of its memory. A crash whose signature an unreported one
  has counts on that one. At most 100 crashes in 8 MiB are kept, none
  past 30 days.
- **Each new kind of crash is offered once.** A signature not offered in
  the last year puts a line on the sign-in screens (ADR 0504) and one
  notice: what stopped, when, and that `report crash` reports it and
  `report forget` puts it away. The line goes when the crash is
  reported or put away; the same signature is never offered again.
- **A report is drafted as root, from what its asker may read.** report
  asks reportd over `/system/live/reports/socket` (`reports::protocol`);
  the kernel says who asks. A person drafts reports of the crashes of
  programs that ran as them; an administrator of every crash, of a
  service with its log's last 200 lines, and of a failed start with its
  notice and `changes`'s list, which report reads and sends (reportd
  starts before the health check writes them, and every user may read
  them), and that start's warnings and errors. The
  draft opens with the facts: Dukovo's version, channel and edition, the
  machine's maker and model, the processor, the memory, the PCI and USB
  IDs and the language.
- **Every report is redacted, and the person sees the exact bytes.**
  Account names, homes' paths, the host's names, IP and MAC addresses,
  email addresses, known Wi-Fi networks, serial numbers and the machine
  ID are each replaced by a mark (`[user]`, `[IPv4]`, …), the person's
  own words too; an edited draft is redacted again. `send` carries the
  bytes the person was shown, and reportd sends those bytes unchanged.
- **Nothing is sent without the person's `send`.** There is no automatic
  sending and no setting that turns it on.
- **Sending** (`reportd::server`): HTTPS to the server
  `/system/settings/report.toml` names (`https://reports.dukovo.org` by
  default), with the system's roots and any `ca` the file adds.
  - `POST /v1/reports`, the text as `text/plain; charset=utf-8`, with
    `Report-Key-Hash`, the SHA-256 of 32 random bytes made for this
    report, and `User-Agent: dukovo-report/VERSION`: the answer is the
    report's number.
  - `GET /v1/reports/N` and `DELETE /v1/reports/N` with `Report-Key`,
    the key itself: only its holder asks after the report or deletes it.
  - No cookie, no account, nothing else of the machine. The key and
    the number are kept on the machine with the report's title; the
    text is not.
- **The queue.** A report is queued before its first try. A try that
  fails is tried again after 1, 5, 15 and 60 minutes, then hourly, for
  30 days, then dropped with a line in the log. A refusal by the server
  is logged as a warning and tried the same way, as a server's mistake
  passes too. `report retry` tries at once.
- **The security box.** A report sent with `--security` is encrypted with
  age to the security team's X25519 keys before it is queued, and sent
  as `application/age-encryption` with `Report-Box: security`: the
  queue, the server and anyone between hold only what the team can
  read. With no keys configured, it is refused, naming
  security@dukovo.org.
- **Status is asked only when the person asks**: `report status` asks
  the server about the person's own reports, with each one's key.

## Consequences

- A person who sees a crash offered can report it in a minute, and sees
  every byte that goes, with names and addresses already cut out.
- Developers get traces with function names and signatures that group
  the same crash across machines, without core files.
- Not yet, each a row of docs/deferred.md:
  - the program files of an app in a person's home are not read, so its
    frames have no names;
  - panics of programs that are not services;
  - attachments: screenshots and files;
  - the status asked with the update check, and an update's notes
    saying it fixes a problem the person reported;
  - outbound firewall rules that hold reportd to its server;
  - the web console's page for reporting a problem;
  - the security team's own key in the system: until then the box is
    set up by report.toml.
- Tests:
  - **Units:** a record read and refused field by field; unwinding a
    real stack of the test program by its call frame information and by
    frame pointers; each signature rule; the store's counting, limits
    and offers; panics from log lines; a kernel dump's section; the
    queue's waits, sending and dropping; the server protocol's requests
    and answers; age to the team's key and no other; report.toml's
    settings and mistakes; who may draft what.
  - **The `problem-report` boot test** against a staging server on the
    host: crashes of a test service offered once and listed; a report
    previewed, redacted, sent, its status changed and read back, and
    deleted; a report queued while the server is down and sent by
    `report retry`; the security box's file opaque on the server; and a
    panic's location. `problem-report-access` checks that a person who
    is not an administrator sees only their own crashes.
  - **The lab's tests** `reports/crash-report` and
    `reports/phone-download` against the staging server on the lab PC.

## Alternatives considered

- **Sending core files or minidumps.** They hold memory the person
  cannot read, so their consent to send would be blind. A trace made on
  the machine holds what a developer reads first, and the person can
  read it too.
- **Unwinding on the server, from raw stacks.** It needs the stack's
  words to leave the machine, which may hold passwords and documents.
  Unwinding on the machine sends names and offsets alone.
- **An automatic-send setting.** Every report is a person's choice
  after seeing it; a setting that sends without that turns a report
  tool into telemetry.
- **A web form on the bug tracker.** It needs an account and a browser,
  and the person must gather the version, the hardware and the trace by
  hand. The machine knows them.

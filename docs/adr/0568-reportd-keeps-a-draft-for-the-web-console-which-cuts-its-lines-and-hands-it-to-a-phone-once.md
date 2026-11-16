<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0568. reportd keeps a draft for the web console, which cuts its lines and hands it to a phone once

- Status: Accepted
- Date: 2026-11-16

## Context

reportd drafts and redacts a report, and sends only the text a person
approved (ADR 0531); report(1) shows every byte, lets the person change
the text in an editor, and hands it to a stick or a phone (ADR 0532).
The web console's Report a problem page drafts over the same socket. A
page cannot hand the person an editor on the machine, and a text the
browser sends back could be any text: a page with a fault, or a script
in it, could send what the person never read. The page needs a way for
the person to take out what they do not want sent, which reportd can
check, and the phone download that report(1) has, without a listener of
its own and a firewall rule.

## Decision

- **reportd keeps the drafts the page makes.**
  - A `draft` with `"keep": true` is kept in memory for its user, under
    an ID of 128 random bits, and answered with the ID, the SHA-256 of
    its text, its sections (each with its ID, its heading, its first
    line in the text and its number of lines), and the size of each
    part, which the page shows before the text.
  - reportd keeps 8 drafts for a person and 64 in all, each for 60
    minutes from its last change; a newer one pushes out the oldest,
    and a restart forgets them all.
  - `send` with `draft` and `sha256` in place of a text sends the
    draft as reportd holds it: only its own user's (RPT-101), and only
    while the hash is its text's (RPT-102), so the bytes sent are the
    bytes the person read. The draft is dropped once queued, and kept
    when it cannot be queued.
- **The person cuts; reportd applies the cuts.**
  - `redact` with a draft, its hash and `edits` changes the kept draft.
    The edits are the whole list each time, against the draft as
    reportd drafted it: cuts of whole lines, or of parts of a line,
    which read `[cut]`, in what the system gathered (a crash, a failed
    start, a service's log, the start's log), and the person's three
    answers typed again.
  - The head, which says what the report is about and where it came
    from, and the headings cannot be cut (RPT-104). A gathered line can
    be cut but not typed into, so what reaches a developer from the
    system is what the system wrote, or less (RPT-105). An answer line
    that starts with `==` gets a space before it, so typed text never
    passes for a heading.
  - reportd puts the text together again, adds a head line that says
    how many lines and parts of lines the sender cut, and redacts the
    whole again. Every answer counts what was taken out by kind
    (`cut_by`: names, host names, addresses, serial numbers and the
    rest, and the person's own cuts), which the page shows beside the
    text.
  - The page shows the text that reportd answers, never one of its own
    making, and sends by the hash of that text.
- **A phone gets the text once, from the console.**
  - `report text DRAFT --sha256 HASH` prints a kept draft's text, with
    the refusals of `send`.
  - `POST /api/1/reports/drafts/{draft}/phone` has the person's worker
    run it, and admind keeps the text in memory under a token of 128
    random bits, at most 4 for a session and 32 in all, for 10 minutes.
    The answer is the link, `https://HOST:PORT/once/TOKEN/report.txt`,
    when it ends, and the console certificate's fingerprint, which the
    page shows beside the QR code it draws, as report(1) does.
  - The host is the address the browser used when it is a private one;
    else the machine's first private address, IPv4 first, since a phone
    may not resolve the machine's name.
  - `GET /once/TOKEN/report.txt` needs no session. The first request
    gets the text as a file to keep; a `HEAD` counts as that request,
    since a chat program that looks at a link for a preview is not the
    person. A later request is told in plain words that the link was
    used (410), and a token that is not kept that there is nothing
    (404), both naming ADM-140. A text whose connection was lost before
    it was handed over is kept for the next request. Each request counts
    against its address's tries, as a sign-in does, and the token is in
    no log.
- **report(1) and the socket's other callers are unchanged**: a draft
  without `keep`, and `send` and `redact` with a text, work as before.

## Consequences

- A person on the web console can take out a line of a log, or a name
  in one, before sending, and reportd sends exactly the text they read,
  whatever the browser does.
- What a report says of the system can be made shorter by its sender
  but not changed, and the report says that it was cut.
- A phone on the same network gets a report from a machine the browser
  reaches, through the console's own port, with no new listener and no
  firewall rule. The link is good for one download in 10 minutes; a
  person who loses it makes another.
- Drafts and links are in memory only: a restart of reportd or admind
  ends them, and the page says how to start again.
- Tests: units for keeping, cutting, re-redacting and the refusals of
  reportd's drafts, for the links' one use, their limits and their
  hosts, and routes tests for the link's answers; the `admin-reports`
  boot test drafts, cuts and sends from the console and fetches a link
  twice, and the lab's web console tests cut a line of a log and send
  the report.

## Alternatives considered

- **The page sends its own text, and reportd redacts it again.** It
  works for report(1), whose person edits the file themselves, but a
  page's text is the browser's, and reportd could not tell the person's
  text from another.
- **Free editing of every line in the page.** A gathered line changed
  by hand misleads whoever reads the report; cutting keeps what is left
  true, and the answers take anything the person wants to add.
- **The one-time server of report(1), started by the console.** It
  needs a port the firewall allows, which an administrator must open
  and close; the console's port is open already.
- **A link that needs the session.** The phone has no session, and
  signing in on it to download one file is more than the file is worth.

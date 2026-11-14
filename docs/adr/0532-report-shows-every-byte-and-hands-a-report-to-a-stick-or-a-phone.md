<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0532. report shows every byte, and hands a report to a stick or a phone

- Status: Accepted
- Date: 2026-11-13

## Context

reportd (ADR 0531) keeps crashes, drafts reports and sends what a
person approves. The person needs a command that asks what happened,
shows the report as it will leave, and lets them send it, change it, or
take it elsewhere. Many machines that most need a report cannot send
one: a server without a route out, a machine whose network is the
problem, or one that failed to start and shows only the failure notice
of ADR 0501. Their owner has a USB stick or a phone at hand.

## Decision

- **report(1) asks three questions, then shows the draft whole.**
  - `report` asks what the problem is about (a command or app, a
    service, the hardware, installing or updating, something else),
    then "What happened?", "What did you expect?" and "How can it be
    made to happen again?", the last of them optional. `report crash
    [ID]` reports a crash reportd keeps, the newest not yet reported
    when no ID is given, and `report failure` the failed start the
    failure notice tells of; their questions are optional.
  - reportd drafts and redacts the text (ADR 0531). report shows it in
    the pager, then reads one key: `S` sends it, `L` leaves out its
    logs and shows it again, `E` edits it in the editor (and reportd
    redacts it again), `F` saves it to a file, `P` hands it to a phone,
    `C` cancels. Nothing goes anywhere before `S`, `F` or `P`. Off a
    terminal the answers and the key are lines of standard input.
  - `report list` lists the crashes kept, the reports waiting and those
    sent; `report status [N]`, `report delete N`, `report forget ID`
    and `report retry` act on them. `--security` sends to the security
    box.
- **`--save FILE` writes the exact bytes to a file**, mode 0600 and
  synced, so a stick's folder takes the report to another machine.
- **`--save` alone, or `P`, serves the report once to a phone.**
  - A listener on a free port, or the one `--port` names, serves HTTPS
    with a P-256 certificate made for this download alone (rcgen), at
    `https://ADDRESS:PORT/TOKEN/report.txt` for each of up to three of
    the machine's addresses a phone can reach, IPv4 first, where the
    token is 128 random bits.
  - report draws the address as a QR code in half blocks (qrcodegen),
    with the address as text and the certificate's SHA-256 fingerprint:
    the phone warns of a certificate it does not know, and the
    fingerprint is what the person checks.
  - The first request for that path gets the bytes; every request after
    it gets 410, and any other path 404. The listener closes after the
    download or 10 minutes.
  - The firewall keeps what a person's program listens on for the
    machine alone (ADR 0334), and report does not change its rules: the
    screen gives the command that allows the port, which `--port` lets
    an administrator allow before the download starts.
- **The failure notice's keys.** While a failed start's notice is shown
  (ADR 0501), login(1) on a console adds: "Press R to report this
  failure, Q for its support page as a QR code, or Enter to sign in."
  `Q` draws the support page's address as a QR code; `R` asks for an
  administrator's name and password, and authd starts a session that
  runs `report failure` and ends: its session request carries the
  command. Any other key starts the usual sign-in, as the name's first
  letter.

## Consequences

- A machine with no way out still gets its report to a developer: on a
  stick, or through a phone's browser, in the same bytes the person
  saw.
- The one-time server listens while it waits, for one download with an
  unguessable path; a phone reaches it once the firewall allows the
  port, which is a rule an administrator adds and removes.
- A failed start can be reported from the sign-in screen by its
  administrator, without a working shell.
- Tests: units for the preview's keys, the QR encoder against known
  codes, the one-time server (the second fetch refused, a wrong path
  refused, the time limit) and login's keys; the `problem-report` boot
  test reports by every route, `problem-report-access` reports a failed
  start from the sign-in screen, and the lab's `reports/phone-download`
  fetches a report once from the lab PC and is refused the second time.

## Alternatives considered

- **A form in the web console only.** A machine whose network or start
  failed has no web console. The console's page is to come, over the
  same socket.
- **Copying the text to the clipboard or showing it to retype.** A
  console has no clipboard to share with a phone, and retyping a trace
  loses it.
- **Plain HTTP for the phone.** Anyone on the network could read the
  report as it passes; a certificate made for one download, with its
  fingerprint on the screen, keeps it to the phone.

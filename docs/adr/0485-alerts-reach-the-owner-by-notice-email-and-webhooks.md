<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0485. Alerts reach the owner by notice, email and webhooks

- Status: Accepted
- Date: 2026-11-13

## Context

A server is mostly run by nobody looking at it. Its services already
notice trouble:

- diskhealth reads each disk's SMART record (ADR 0157);
- backup knows when a destination is overdue or failed (ADR 0504);
- the update health check knows when an update fell back (ADR 0501);
- dumps knows when the machine restarted after a kernel panic (ADR
  0483);
- metricsd (ADR 0484) sees a filesystem filling or a RAID volume
  degraded.

Until now each of them said so in the log, on the screen of whoever was
signed in, and on the sign-in screen. On a machine in a cupboard that
is nobody. ADR 0501 asked for its failure notice to go out by email,
and docs/deferred.md kept that open.

Cockpit, Webmin, Proxmox VE and TrueNAS each send email alerts, and
TrueNAS and Proxmox also post to services such as Slack and ntfy. Each
keeps its own list of what to watch and sends each problem once, not
at every check. On a Linux server without them, the same comes from a
mix: smartd's `-m`, mdadm's `MAILADDR`, cron's `MAILTO`, each
needing a local MTA. Many of those messages go unread because the MTA
was never set up.

## Decision

- **One service, alertd, keeps the alerts and tells the owner.**
  - It answers on `/system/live/alerts/socket`, one line of JSON a
    request: `raise`, `event`, `clear`, `set`, `list`, `history`, `test`
    and `status` (`alerts::protocol`). Every service's view holds that
    folder read-only, as it holds notifyd's (ADR 0217), so every
    service can reach it.
  - Only root's processes may ask. An alert belongs to its sender's
    resource domain, its source: a service's name, or the session's for
    a shell. Its identity is the source and the sender's own key
    (`smart:WD-1234`), so a sender clears and sets only its own.
  - `alerts::client` gives services `raise`, `event`, `clear` and `set`,
    each giving up after 2 seconds. A service never fails, or waits on
    alertd, because alertd is not there. It logs once and goes on.
  - Programs that run as the machine starts, before alertd, may wait up
    to a minute for it (`event_at_start`).
- **An alert is told when it changes, never at every check.**
  - A new alert is told once. So is one that goes from warning to
    critical, one that clears, and an event: a fallback, a panic, a
    failed staging.
  - A sender that knows its whole list sends it with `set`, which
    clears what is gone. The list survives a restart of either side,
    and nothing is told twice.
  - The active alerts and the newest 1000 to 2000 changes are kept in
    `/system/state/alerts`, written by a new file renamed over the old
    one. One source holds at most 100 active alerts and all sources
    1000, so one service in a loop cannot crowd out the others.
- **Where it goes.**
  - Always to the screen through notifyd, and to the sign-in notice
    while active (ADR 0504).
  - To each channel `/system/settings/alerts.toml` names: email, and
    webhooks for ntfy, Matrix, Slack, Discord, or JSON for a program of
    the owner's. A channel's `severity` and `cleared` choose what it
    gets.
  - Changes are gathered for 30 seconds into one message per channel,
    so that a dozen disks filling at once send one email.
  - A failed send is kept in the outbox and retried after 1, 5, 15 and
    30 minutes, then hourly, for a day. Channels are independent, so a
    dead webhook holds up no email.
  - `alerts test` sends a test through every channel at once and prints
    the server's own words for a refusal.
- **Email goes through the owner's submission server, never in the
  clear.**
  - Port 587 with STARTTLS, or 465 with TLS from the first byte.
  - A server that does not offer STARTTLS is refused before anything is
    sent, as is data sent before the TLS handshake (RFC 3207's
    injection).
  - The certificate must chain to the system's roots or to the file
    `ca` names in `/system/settings/ssl`.
  - Authentication is AUTH PLAIN or AUTH LOGIN, inside TLS.
  - The message is plain UTF-8 text, with `Auto-Submitted:
    auto-generated` so that vacation replies stay away.
- **alerts.toml holds secrets** (a password, tokens, webhook addresses
  that are tokens themselves).
  - A file that names a channel is read only when it is root's and no
    one else may read it. Otherwise only the screen is told, and the
    log says to `chmod 600`.
  - A secret never appears in a log line, an error or `alerts status`.
  - A channel with a mistake is left out, its mistake logged, and the
    others go on.
  - A webhook must use HTTPS, except a `json` hook to a private address.
- **The producers and their keys.**
  - **diskhealth:** `smart:SERIAL`, and the early warning
    `smart-rising:SERIAL`.
  - **backup:** `backup-failed:NAME` and `backup-overdue:NAME`.
  - **metricsd:** `fs-full`, `fs-filling`, `btrfs-missing`,
    `btrfs-behind`, `memory:low`, `temp` and `certificate` (ADR 0484).
  - **alertd itself:** `service:NAME` (source `spark`) for a service
    that has not been as asked for a minute. It reads Spark's `show`
    every 10 s and judges each service as `service list --failed` does,
    so that PID 1 never waits on a socket.
  - **Events:**
    - `update-fell-back` and `start-failed` from the health check: ADR
      0501's notice, which now reaches email too;
    - `update-failed`, `update-not-staged` and `update-not-verified`;
    - `kernel-panic` from `dumps save`.
  - Each is on by default. A channel's `severity = "critical"` keeps
    the warnings to the screen.

## Consequences

- An owner who sets one email address or one ntfy topic hears of a
  failing disk, a degraded volume, a full filesystem, a missed backup,
  a failed update and a panic, once each, with nothing else to install.
- A service whose trouble an owner should hear of raises an alert. The
  service graph (thread A's work on Spark) and certd use the same
  calls.
- Not yet, each a row of docs/deferred.md:
  - A bundle's service, which runs as an account of its own, cannot
    raise alerts.
  - An alert cannot be silenced for a while. Alerts are told only when
    they change, so a long-standing one does not repeat.
  - The web console's alert page and push notices.
- Tests:
  - **Units:**
    - every rule of the state, and the per-source cap;
    - alerts.toml's mode rule, and each mistake;
    - email against a TLS server on lo: STARTTLS and implicit TLS, a
      server without STARTTLS refused with nothing sent, both sign-ins,
      a refused password's words, dot-stuffing, quoted-printable and an
      RFC 2047 subject;
    - each webhook kind's request;
    - batching, retries and the outbox across a restart;
    - each producer's alerts.
  - **The `monitoring` boot test:** on an installed system, `alerts
    test` reaches a mail server on lo through STARTTLS and a receiver of
    each webhook kind; alerts raised, raised again, cleared and raised
    across a restart of alertd are each told once by every channel; a
    refused password and a refused address are said in the servers'
    words; a user who is not root is refused; and an alerts.toml others
    can read sets up no channel.
  - **The lab's tests:** `monitoring/alerts-email` and
    `monitoring/alerts-webhook` against a mail server and a receiver on
    the lab PC, and `monitoring/early-warning-disk`, where a full USB
    stick's alert is mailed and cleared.

## Alternatives considered

- **A local MTA (Postfix, OpenSMTPD) and each tool mailing as on
  Linux.** It is a second mail server to keep secure, it sends each
  tool's mail in each tool's form, and it sends the same problem at
  every check. Few owners set it up well.
- **Plain SMTP on port 25 as a fallback.** It sends what a server knows
  of its disks and its passwords' failures across the network
  unencrypted. A submission server with TLS is what every mail provider
  offers.
- **A hook in Spark for failed services.** Spark is PID 1, and must
  never wait on a socket. alertd reading Spark's state every 10 s costs
  nothing and keeps the rule `service list --failed` uses.
- **Repeating an active alert each day.** It trains the owner to ignore
  the messages. `alerts list` and the sign-in notice show what is
  active.

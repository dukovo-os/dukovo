<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0518. The web console reads what only administrators may see through elevate, follows the log through authd, and tries network and firewall changes as Spark trials

- Status: Accepted
- Date: 2026-11-14

## Context

ADR 0515 made every action of the web console one entry in a registry,
and held that a read runs as the person who asks and never as an
administrator. The first pages kept to that: the Overview, the Terminal
and the Tasks drawer read what any account may read.

The Services, Logs, Networking and Firewall pages need more. Some of
what they show only root may read: the whole log (ADR 0252 keeps no
group that reads it, so that every look at another account's lines is a
line of the audit log), what the firewall dropped, which program listens
on which port, the timers, which timerd answers for root alone, and the
crash dumps. The Logs page also follows the log as it is written, which
a read every few seconds would turn into an audit line every few
seconds.

The Networking and Firewall pages change the settings that carry the
console's own connection. A change of a card's address or a firewall
rule that goes wrong cuts off the browser that made it, and a machine
that is reached only over the network then needs a person at its screen.
ADR 0472 gave Spark trials for this: a change is tried for a time and
undone unless it is kept. The console has to use them, and has to know,
before it offers to keep a change, whether the browser still reaches the
machine over the new settings. A firewall rule is checked when a
connection begins, and a connection that is open goes on through a rule
that refuses new ones, so a request that came by an old connection
proves nothing.

## Decision

### Reads that need an administrator

- **A read may run as an administrator** when its command needs root.
  It runs through authd's `console elevate`, as a change does: for an
  administrator's session that is unlocked (ADM-106 when it is locked,
  ADM-122 for a person who is not an administrator), or for a token
  whose scopes name the area in full. A `:read` scope never reaches an
  elevated command, as before. Each run is a line of the audit log.
- **A session's request for one must come from the console's own
  page.** A `Sec-Fetch-Site` header other than `same-origin` is ADM-109.
  The session cookie is `SameSite=Strict` already; this is a second lock
  on a read that would otherwise send no proof of its page, since only a
  request with a body carries the console's CSRF header.
- **A page never runs one on a timer.** It runs when the page opens,
  when the person changes a filter or asks to read again, and after the
  session unlocks. A locked administrator sees what they may read, with
  a line that says what unlocking shows; a person who is not an
  administrator sees the same without the button.
- **A read's arguments come from the query** as well as the path, typed
  as the action's schema says, and a name the action does not take is
  ADM-108. Show command's `curl` line carries the query.
- **A read may answer in lines.** An action whose command prints a JSON
  value a line, as `log --output json` does, answers an array of the
  values, and a line that is not JSON fails the read as other output
  that is not JSON does.

### Following the log

The Logs page follows the log with one `log --output json --follow
--lines 0` for as long as it follows. admind opens two pipes and asks
authd, with a new `console follow` request, to start `log` on their
write ends: as the person, who sees their own programs' lines, or as
root for the whole log, for an administrator whose session is unlocked.
authd writes the command itself from the options it checks; admind
sends no command, and nothing but `log` starts this way. authd ends a
follow of the whole log on its own clock when the session locks, its
unlock runs out or it ends, as it ends an administrator's shell (ADR
0517). admind sends the records to the page as server-sent events, one
follow a session and eight at once, each for an hour at most, and ends
one whose tab falls behind.

### Network and firewall changes are Spark trials

- **The network's and the firewall's changes are trials** named
  `network` and `firewall` (ADR 0472). `network set` and the firewall's
  `add`, `replace` and `remove` take `--try SECONDS`; `keep` and `undo`
  end the trial. A second change while one is tried is refused (NET-176,
  NET-177). The pages offer windows of 60 seconds to 10 minutes.
- **Keep is the person's choice, once the browser has reached the
  console over the new settings.** admind keeps, for each trial, a time
  at or after the moment its change took effect: when the console's
  action that began it answered, or, for one begun at a terminal, when
  admind first saw Spark list it. `GET /api/1/console/fresh/{trial}`
  answers whether the request's connection was accepted after that time,
  with the browser's address and the address, port and card it came to.
  A request by an older connection is ADM-133, and admind ends that
  connection once the answer is sent (a GOAWAY on HTTP/2, ADR 0519, sent
  before the answer; a close on HTTP/1.1), so the page's next request
  opens a new connection that the new settings decide. The page asks at
  most three times in a row, then again every few seconds until the
  browser reaches the console or the time runs out, and Keep is off
  until it has.
- **A change that takes away the browser's own address** is tried for
  at least 2 minutes, and 5 when the browser came by an address rather
  than a name. The page says the new address, and the change is kept
  from a page opened there, which proves the new address works. The
  answer to its try has no way back once the address is gone, so the
  page waits 15 seconds for it, then ends the request, connection and
  all, so that the browser does not send it again by itself, and shows
  the trial as it asked for it, by its own clock, until the console
  answers again.
- **Every session sees the trial**, from Spark's document: a banner on
  the Networking and Firewall pages with the time left and who began it,
  and a line on the Overview.
- **An undo Spark made by itself is a row of What changed.** admind
  reads Spark's read socket before it looks for hand edits, and a trial
  Spark undid because it was not kept, because a service failed with it,
  or because the machine started again while it ran, has a row of its
  own, by Spark, with Spark's words and the files it put back, in place
  of a hand edit by no one. Spark's document names those files.

## Consequences

- The registry's test holds that a read runs as the person, or as an
  administrator when its command needs root, and that a long action is
  still a change. A reviewer of an elevated read checks that its
  command needs root and that no page runs it on a timer.
- A token that only reads cannot see the whole log or what the firewall
  dropped. That is as ADR 0252 and authd's scopes intend.
- Every look at the whole log, and every follow of it, is an audit line,
  so a person who reads the log through the console leaves the same trace
  as one who runs `elevate log`.
- A tried change that is never kept is undone by Spark even when admind,
  the browser and the network are gone, and the machine starting again
  undoes it before any service reads the file.
- A connection the browser holds open is ended once during a trial; the
  page's next request opens another, which costs one TLS handshake.
- The `fresh` check proves that this browser reaches the console over a
  new connection. It does not prove that every other client still
  reaches every other service, which the person decides before Keep.

## Alternatives considered

- **A group that may read the whole log.** ADR 0252 rejected it: every
  look at another account's lines is an audit line.
- **admind reading logd itself as root.** It would skip authd's unlock
  and audit, which are the reasons elevated actions go through authd.
- **A page that runs an elevated read every few seconds** to follow the
  log. Each run is an audit line, and lines would be missed between
  reads.
- **A WebSocket for the follow.** The follow goes one way, which
  server-sent events carry over plain HTTP/2, with no second protocol.
- **Checking reach over any connection.** An old connection passes a
  rule that refuses new ones, so the browser would be told it reaches the
  console when it no longer could.
- **An undo timer in admind.** It ends with admind, and a power cut
  forgets it; Spark's trial survives both.
- **Keeping a change by itself once the browser reaches the console.**
  The person decides whether the change does what they meant, and a
  change of address cannot be reached from the old page at all.

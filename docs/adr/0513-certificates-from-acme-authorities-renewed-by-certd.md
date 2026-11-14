<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0513. Certificates from ACME authorities, renewed by certd

- Status: Accepted
- Date: 2026-11-13

## Context

certd keeps the machine's own certificate and the ones `cert add` takes
(ADR 0512), but a site on the internet needs one that browsers trust,
and the way almost everyone gets one is from Let's Encrypt, or another
authority that speaks ACME (RFC 8555). Those certificates last 90 days
and shortly 45, so a person cannot be the one who renews them.

Getting one means proving that you hold a name. Over HTTP (HTTP-01) the
authority fetches a file from port 80 of the name's address; over DNS
(DNS-01) it looks a TXT record up, which also proves a wildcard name.
Both need something Dukovo's services did not have: certd had no
network and no port, and the firewall closes inbound ports to a command
an administrator runs (ADR 0334).

Two more things shape the work. Dukovo links one multicall of the
system's tools, which already carries an HTTP client over rustls with the
system's authorities (`tuf::http`), and a second TLS stack in it would
double what an auditor has to read. And a program that answers strangers
on port 80 must not hold what certd holds: the store with every key.

## Decision

- **`cert get NAME` gets a certificate, `cert revoke NAME` revokes one,
  and certd renews them.** The ACME client is the instant-acme library,
  with a small client of its own (`cert/acme/http.rs`) that sends its
  requests through `tuf::http`, so the system's authorities are the
  only roots and the library brings in no TLS stack; `tuf::http` now
  keeps all of an answer's headers, since ACME's `Replay-Nonce`,
  `Location` and `Retry-After` are in them. An answer over 1 MiB is
  refused. The library runs on a tokio runtime of one thread that
  exists only while an order does.
- **The names are given with `--name`, or are NAME itself.** One to 100
  names, the first the certificate's common name; a name is a DNS name,
  and a wildcard only with `--dns`, since DNS-01 alone proves one.
  `--key` is `ecdsa-p256` (the default) or `ecdsa-p384`, a new key for
  each certificate; `--profile` names one of the authority's profiles,
  which the authority refuses with its own words when it has none of that
  name; `--staging` is
  Let's Encrypt's staging directory, `--dry-run` goes through every step
  against it and installs nothing, and `--directory URL` is any other
  authority, over HTTPS (HTTP only to a private address, for an
  authority of a lab or a company). A certificate that came from an
  authority is got again with `cert get NAME`, which takes the names,
  the directory, the key type and the profile it was got with; one added
  by hand is refused until it is removed.
- **An account is made on first use of a directory**, and kept in
  `/system/state/certd/accounts/HOST.json` (`HOST-PORT.json` with a
  port), in a folder only root enters, mode 0600, as the library's
  credentials with the time it was made and the contact address. The
  authority's terms of service are shown and agreed to, on a terminal or
  with `--agree-terms`; a command with neither refuses. No contact
  address is given unless `--email` has one: Let's Encrypt no longer
  mails notices of expiry, and certd tells a person itself.
- **Preflight tells what is wrong before the authority is asked**, in
  words that name what to do: a clock before the system's making, a
  name that does not resolve, one that leads to a private address when
  the authority is on the internet, one with an IPv6 address on a
  machine with none (a warning), and a firewall that closes port 80.
  `--dns` skips all but the clock, since the authority never connects.
  The authority's own problems are put into plain words by type, each
  with its detail: it could not reach the name, could not look it up,
  reached another server, the name's CAA record forbids it, a rate limit
  (with the time to try again), a name it does not issue for.
- **HTTP-01 is answered in one of two ways.** When port 80 is free, the
  command (or certd) binds it, on both address families, and starts
  `acme-answer`, an entry of the same multicall, in a view of its own
  that holds the one program: it gets the two sockets, the answers on
  its standard input and nothing else, and serves `GET` and `HEAD` of
  `/.well-known/acme-challenge/TOKEN` for the tokens it was given until
  its standard input closes. A request head is up to 8 KiB, a
  connection lives 10 seconds, 64 at once, one request each. When nginx
  holds port 80, the answers go into
  `/system/live/certd/acme-challenge/TOKEN` and nginx serves the folder
  for every site that includes `acme-challenge.conf`, which its
  default server does; nginx's service file has to let it read
  `/system/live/certd`, which the bundle's manifest cannot say, so the
  handbook shows the one line to add. A port taken by anything else is
  refused, saying so.
- **certd's service holds `network.outbound` and `network.listen = [80]`.**
  The right to listen opens nothing until a socket is bound
  (ADR 0334): certd binds port 80 only while an order runs, and the
  firewall still decides who gets through. A service that names a port
  takes connections unless a rule denies them, so certd needs no rule; a
  command an administrator runs takes them only when a rule allows,
  which is why `cert get` says to `elevate firewall allow tcp 80` and
  does not open it itself. When nginx holds the port, no rule is
  needed either.
- **DNS-01 is done by hand.** `cert get NAME --dns` shows each TXT record
  to add, all at once since a name and its wildcard are one name with
  two values, waits for Enter, then looks each record up through the
  system's resolver every 10 seconds for up to 10 minutes. Without a
  terminal it is refused, unless `--yes` says to wait for the records to
  show without asking, which is how a script runs it. certd does not
  renew such a certificate, since it cannot add records; it tells a
  person at 21 days and again at 7 days left to run the command again.
- **certd renews by the authority's renewal information (ARI, RFC 9773)**
  at each of its looks, every 12 hours and up to an hour more. For each
  certificate with `challenge = "http-01"` it asks for the time the
  authority suggests, picks a random time inside the window, and keeps
  it as `renew_at` in `certificate.toml`; a window that moved takes the
  kept time back, and one that has begun starts now. An authority that
  gives none has the certificate renewed when two thirds of its life are
  gone, 30 days before a 90-day certificate ends. The next time to ask
  is the authority's `Retry-After`, between an hour and a day, and is
  kept in memory: a restart asks once more. A renewal names the
  certificate it replaces, so that the authority counts it as a renewal
  for its rate limits, gets a new key, and goes through the same steps as
  `cert get`. `cert renew NAME` renews at once whatever `renew_at` says.
- **A renewal that fails is said, and tried again at the next look.** The
  log has the plain message; from 14 days left the sign-in notice and a
  notification say that it could not be renewed and when it runs out,
  critical from 7 days.
- **`cert revoke NAME [--reason ...]` revokes through the account** and
  marks the certificate `revoked` in `certificate.toml`. `cert list`
  shows it, certd renews nothing revoked and tells a person once, and
  the services keep using the certificate until it is replaced, which the
  command says.
- **The lab runs Pebble**, Let's Encrypt's own test authority, with its
  DNS server, built from one pinned release by the lab PC (`lab/pebble`).
  The test `services/acme` gets certificates by HTTP-01 standalone and
  through nginx, by DNS-01 for a name and its wildcard, renews one by
  hand and one by certd, revokes one, and checks that a closed firewall
  is told before the authority is asked.

## Consequences

- `systools` grows by about 1 MiB (3.6 percent): the ACME library, its
  HTTP types and an async runtime of one thread.
- certd now has the network and holds the account keys, so a flaw in it
  reaches further than before; the responder, which strangers talk to,
  holds none of it: it runs in a view of its one program, with its
  sockets, its answers and, unless they go to a terminal, the caller's
  errors, and reaches nothing else. The account keys sit in the store's own folder, where
  a backup of `/system/state` takes them with the certificates.
- nginx has to be told once to read `/system/live/certd` before `cert get`
  can answer through it. Without the line the authority is answered with
  404 and `cert get` says what the authority said; the handbook has the
  line.
- A machine behind a router needs port 80 forwarded to it for HTTP-01:
  preflight checks the name and the firewall, not the router, and the
  authority's own message says what it could not reach.
- certd cannot ask Spark whether nginx runs, so when port 80 is taken
  it takes nginx to hold it and writes the answers; if it is not nginx,
  the authority's check fails and says so.
- Not done, and each a row of `docs/deferred.md`: DNS providers' APIs
  (so a wildcard certificate cannot yet renew by itself), External
  Account Binding (ZeroSSL and Google Trust Services), TLS-ALPN-01 and
  certificates for IP addresses.

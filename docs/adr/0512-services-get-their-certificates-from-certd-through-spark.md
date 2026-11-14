<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0512. Services get their certificates from certd through Spark

- Status: Accepted
- Date: 2026-11-13

## Context

A service that serves TLS needs a certificate and its key. Until now a
service got them only from an administrator, who ran `openssl` and
`chown` by hand and put the files beside the service's settings
(ADR 0491, nginx's HTTPS server). Nothing made them again before they ran
out, and nothing told the service that they had changed. The web
console's service, admind, serves HTTPS from its first boot and has no
administrator's hands to wait for.

Spark runs the services, each in a view of the file tree that holds what
its file names. It already gives a service its secrets as read-only
files owned by the service's account (ADR 0216), by changing the owner of
the real files; one file cannot belong to two accounts, so a certificate
that two services read cannot be given that way. Spark is single-threaded
and holds the whole tree, so it reads nothing that another process
can change without checking it, and it does not wait on one.

Certificates differ from secrets in that something makes them: the
machine's own, and later ones from an authority on the network. That
something is a service of its own, which must not hold Spark's control
socket, since the socket stops any service and powers the machine off.

## Decision

- **certd keeps the certificates, and `cert` is its command.** Both are
  in systools. The store is `/system/state/certd/certificates`: for each
  name a link `NAME` to the folder of its current generation, `.NAME.G`,
  which holds `fullchain.pem` (the certificate first), `key.pem`
  (readable by root alone) and `certificate.toml` (whether certd made it
  or it was added, and when). A writer makes a whole new generation and
  moves the link with one rename, so a reader sees the old pair or the
  new one, never a new chain with the old key; the generation before is
  kept. Writers, certd and `cert`, take an exclusive lock on the store
  and wait for it up to a minute. Readers take none. The folder is
  readable by all, as `/var` is on Linux: chains are public, and keys are
  not.
- **A service names its certificates** with `certificates = ["NAME", ...]`
  in its file. A name is 1 to 64 of `a-z`, `0-9`, `-` and `.`, beginning
  with a letter or a digit, with no `..`: one harmless component of a
  path. One checker, `manifest::service::check_certificate`, serves the
  service file, Spark, `cert` and certd. certd's own file names none.
- **Spark copies each certificate for each service that names it**, at
  each start of the service, into `/system/live/certificates/SERVICE`,
  laid out as certd keeps it, given to the service's account, with mode
  0400 for the files and 0500 for the folders. The service finds it as
  `CERTIFICATES_DIRECTORY` says. No service may name that folder or one
  that holds it. It is a folder of its own, not in Spark's
  `/system/live/spark`, which services that ask Spark about the others
  hold for its control socket, as alertd does. Spark
  trusts nothing in certd's folder: it follows no link but a
  certificate's own, and only to one of that certificate's generations by
  the name certd gives it; it reads regular files alone, each up to 64 KiB,
  after looking at them and again after opening them; and it opens
  nothing that would wait. A service that cannot be given a certificate
  does not start, and Spark says why.
- **certd asks Spark through a socket of its own**,
  `/system/live/spark/certd/socket`, in a folder that certd's view holds
  read-only. It answers one request, `certificate NAME`, and only from
  certd's own process; `cert` asks through the control socket, which is
  root's. Spark copies the certificate anew for each running service that
  names it and sends it SIGHUP: at once, unless one went less than 10
  seconds ago or the service started less than 10 seconds ago, and then
  once, when the time is up. A service reads its certificates again on
  SIGHUP; nginx, PostgreSQL and admind do.
- **`this-machine` is certd's.** It is an ECDSA P-256 key, made by
  rcgen on ring, and a certificate for the machine's name, its name with
  `.local` and `localhost` (`localhost` alone for a machine called that),
  valid from an hour before it is made for 800 days, for a TLS server and
  not a certificate authority. Apple's systems refuse a server
  certificate valid for more than 825 days, even one a person chose to
  trust. certd makes it when it starts, and at each look, every 12 hours
  and up to an hour more, when it is missing or does not read, when 30
  days or fewer are left, or when its first name is not the machine's.
  Each time it logs the new fingerprint, and if the certificate was
  there before it keeps a sign-in notice of it, since a person who
  trusted the old fingerprint needs the new.
- **A clock before the system's making is wrong.** certd makes and judges
  nothing then, since a machine whose battery is flat boots with 1970 on
  its clock and would make a certificate dated then; it looks again each
  minute. `cert check` fails with the clock as the reason. `cert add` and
  `cert renew` refuse too.
- **certd's service is ready when `cert check` passes**: the clock is
  right, and `this-machine` reads and has more than 30 days left. Spark
  runs the check each second until it first passes, with 120 seconds to
  get there, and then every hour.
- **`cert add` takes a certificate from elsewhere** and checks it, in
  this order, each refusal in words: the name; the files (a chain of 1 to
  10 certificates and a key, each file up to 64 KiB, nothing else in
  them); that the key is one TLS can sign with and is the first
  certificate's; that the first certificate is valid now, has a name and is
  not an authority's; and that the system's authorities
  (`/system/settings/ssl/certs/ca-certificates.crt`, so one an
  administrator added counts) vouch for the chain with webpki, for a TLS
  server, with the file's other certificates as intermediates. A
  self-signed certificate is taken, with a note that browsers warn of it. A
  chain that stops at an issuer no authority signed is refused with the
  issuer's name and that the file most likely lacks its intermediate
  certificate, which browsers other than Chrome refuse too, unless
  `--untrusted` says the authority is a private one.
- **certd warns before an added certificate runs out**: with 21 days
  or fewer left it puts a line in the sign-in notice (ADR 0504) and sends a
  notification, critical at 7 days or fewer and once it has run out.
- **`cert remove` refuses a certificate a service's file names**, naming
  the file, since the service would not start without it.

## Consequences

- A service that serves TLS names its certificate, and reads it again
  on SIGHUP instead of when it starts. nginx's settings read the files in
  `CERTIFICATES_DIRECTORY`; a change of certificate under load loses no
  request (the lab test `services/nginx`).
- A service gets its own copy of a certificate. A copy costs a few
  kilobytes of memory and a rewrite on each change; no service's key is
  another's to read, and a service that holds no certificate holds no
  folder.
- certd can put a wrong certificate in front of a service, or none, but
  it cannot make Spark read another service's files or follow a link out
  of its folder: the `certificates` boot test makes a certd that tries a
  folder, a link out, a key that is a link, a key of 65 KiB and a named
  pipe, and each is refused with its reason.
- Spark sends one SIGHUP in 10 seconds to a service, however many
  certificates change: a burst of renewals reloads a service once.
- A browser warns of `this-machine` until told to trust its
  fingerprint, which `cert show this-machine` prints; a renewal changes
  the fingerprint, which the sign-in notice tells of.
- A restart instead of SIGHUP, for a service that cannot read its
  certificates again, is deferred. So are RSA keys made by certd,
  since ring makes none and ECDSA is what every browser takes, and
  certificates from an authority on the network (ACME), which the next
  change brings: certd then needs the network and a port, and keeps
  what `cert add` takes beside what it gets.
- A machine whose clock stays wrong for 120 seconds has Spark stop
  certd and start it again, which logs the wait again each time: the clock
  is timed's to set.
- A certificate from a private authority goes in only with the
  authority among the system's, or with `--untrusted`:
  `dukovo-certificates(7)` says how.

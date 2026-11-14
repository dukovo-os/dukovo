<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0514. The web console signs people in through authd, and runs each session's work as the person

- Status: Accepted
- Date: 2026-11-14

## Context

Dukovo Admin runs the machine from a browser. A program that can restart
services, change the firewall and add accounts, and that answers anyone
who can reach its port, is the most valuable target on the machine. It
parses HTTP and TLS from strangers, which is the code most likely to have
a flaw, so it must not hold what an attacker wants: the passwords and
second steps of the people who sign in. It must also not run as anyone
with more rights than the person at the browser has.

Dukovo already has the parts for this. authd keeps the accounts and is
the only program that checks a password and starts a session (ADRs 0218,
0250); it runs a command as an administrator after the password, and
for 15 minutes after (ADR 0219); it audits what it does (ADR 0211) and
records each sign-in (ADR 0254). The consoles and SSH sign in through
it. The web console is one more way in, and should keep to the same
accounts, waits and records, not make a second set.

A browser adds what a console has no use for. A page can be made to send
a request on someone else's behalf (CSRF), and a name can be pointed at a
machine it was never meant for (DNS rebinding). Passkeys belong to a
name, and browsers refuse one for an address. Authenticator apps and
passkeys have to be checked on the machine, with no account at a
company's server, since a server may have no route to the internet.

Three more facts shape it. The firewall opens a running service's port to
every machine (ADR 0334), while the console is for the networks the
machine is on. admind's certificate comes from certd, as any service's
does (ADR 0512). And the console is built in parts: this record is the
sign-in, the second steps, unlock and the server; the actions, the API
that carries them and the console's tasks come after, and each gets its
own record.

## Decision

- **Three processes, each trusting the next less.** admind, which runs
  as root in a view that holds its port, its certificate, authd's
  socket, a state folder and the console's files, and nothing else. authd,
  which keeps every secret and decides every step. A worker for each
  session, which acts for the person.
- **authd keeps every secret.** The passwords' hashes, the secrets of
  authenticator apps, passkeys' public keys and counts, recovery codes'
  hashes, trusted browsers' hashes and API tokens' hashes are in
  `/system/state/authd/console/NAME.toml`, one file for each account that
  has any, mode 0600 in a folder of mode 0700 that no other service's
  view holds. authd writes it anew beside the old one, syncs it and
  renames it over, as it writes its other files. admind never sees a
  hash or a secret, only what a browser sends and what authd answers.
- **The requests are authd's.** A request is `console` and a verb
  (`accounts::console`). admind, which runs as root, asks for the
  sign-in, its second step, the unlock, the elevated commands and the
  checks of tokens; authd refuses these from any other program. A person's
  own second steps and tokens are asked by their worker, so authd sees
  the person asking, as it does for `passwd`.
- **One set of sign-in rules, in authd.** `/system/settings/admin.toml`
  (admin.toml(5)) says whether the second step is asked of everyone who
  has one (`if-set`, the default), of administrators too
  (`administrators`) or of everyone, how many hours a session lasts
  (1 to 168, 12 by default) and how many minutes it lasts with no page open
  (5 to 1440, 30). A value out of range or a name that is no setting is
  refused, so that a typo never loosens a rule. An account the rule asks
  for a second step that has none signs in to a session that may only add
  one, and signs in again once it has. Who may sign in: administrators and
  the members of the group `console`, number 12, which `user set --console`
  adds a person to. Root never does, and a disabled account never does.
- **Signing in goes in steps, held as a ticket.** The password is checked
  first, and the account's right to the console after it, so that only
  someone who knows the password learns that the account may not use it.
  authd answers that the sign-in is done, that a second step is wanted
  and of which kinds, or that the rules ask for one the account has not
  got. A ticket is 32 random bytes in hexadecimal, for that account and
  that address, used once and kept 5 minutes; authd keeps 128 and lets the
  oldest go first. Tickets, challenges, apps being added and unlocks are in
  authd's memory alone, so a restart of authd forgets them and the console
  signs in again.
- **A wrong proof waits as a wrong password does.** A wrong password, a
  wrong code and a wrong token are answered after the wait authd puts on a
  wrong password at a console (2 seconds for each of the first three in a
  row, then twice as long, up to a minute), counted for the account and the
  browser's address together. A right proof ends the run. A name that is no
  account's is checked against a hash all the same. Each wrong password or
  code at a sign-in is a record for `lastb`.
- **App codes follow RFC 6238.** HMAC-SHA-1, 30-second steps, 6 digits,
  with RFC 4226's truncation, as every authenticator app makes them. The
  step before now, now and the step after are taken, for a clock that is a
  little off; a step at or before the last taken is refused, so a code
  works once. They are compared in constant time. The secret is 20 random
  bytes, kept in base32 as it is, since each code is made from it; the
  page draws its QR code and writes the secret as text for an app that
  cannot scan.
- **Recovery codes are made with the first second step.** Ten, of 16
  letters and digits (80 random bits) in groups of four, shown once and
  kept as SHA-256: 80 random bits need no slower hash. Each works once in
  place of the second step at sign-in, not for unlock.
  Making new ones replaces all ten; removing the last second step removes
  them.
- **Passkeys are checked by authd, to WebAuthn Level 3 (sections 7.1 and
  7.2),** with `ring`: ES256 (P-256), EdDSA (Ed25519) and RS256 (RSA of 2048 to
  4096 bits), and a decoder of the CBOR subset the specification needs,
  refusing anything else by name. In order: the client data names the
  right ceremony, a challenge that authd gave for this and has not used
  (kept 5 minutes, used once whether it is right or wrong), the console's
  origin and no embedding page; the authenticator data has the relying
  party's hash and says that a person touched the key, and for a passkey
  that is the whole sign-in, and for unlock, that the authenticator checked
  the person too (a PIN, a fingerprint); the signature is the key's over
  the authenticator data and the client data's hash; and the count rises,
  unless it is 0 each time, as a synced passkey's is. A count that does
  not rise may be a copied key: the use is refused and the old count
  stays. A passkey is for one name, the one the console was opened by when
  it was added, so it is offered only there; a name that is an address is
  refused, as browsers refuse it. An account keeps 16 passkeys, each with a
  name its person gives it.
- **Attestation is not checked.** The console asks for none. The key it
  trusts is the key of a person who is signed in and proves again who
  they are when they add it.
- **A passkey may be the whole sign-in.** Its user handle, 16 random bytes
  made with the account's first passkey, names the account, and the
  authenticator must have checked the person, so that the key and a PIN or
  a fingerprint are two proofs.
- **"Trust this browser for 30 days" skips the second step, not the
  password.** authd makes a token of 32 random bytes, keeps its SHA-256
  with the browser's description and the day it ends, and admind puts the
  token in the cookie `__Host-dukovo-trust`. An account keeps 16. A token
  that is not trusted any more is cleared from the browser and the step is
  asked. Forgetting trusted browsers forgets all of them.
- **admind is one thread of tokio, with hyper's HTTP/1.1 and
  tokio-rustls.** It serves TLS 1.3 and 1.2, the 1.2 suites that exchange
  keys with ECDHE and encrypt with AES-GCM or ChaCha20, no certificate
  from the browser and no early data, with the certificate certd gives it
  (`this-machine`, until admin.toml names another), read again on SIGHUP.
  It listens on port 9090 on IPv4 and IPv6; admin.toml may name 443 or a
  port from 1024, and the service file lets it listen on 9090 alone, so a
  new port is changed in both.
- **admind answers only the networks the machine is on, until told
  otherwise.** It looks at each connection's address before TLS, and
  closes one it does not answer without a byte: with `reach = "private"`,
  this machine, its cards' networks, loopback, link-local, 10.0.0.0/8,
  172.16.0.0/12, 192.168.0.0/16, 100.64.0.0/10, fc00::/7, and an IPv4
  address carried in IPv6 as itself. `reach = "any"` answers all. The log
  says what it closed once a minute for each address, never once for each
  try.
- **Limits before sign-in.** 512 connections at once and 32 from one
  address (an IPv6 address counts by its /64); 10 seconds for the TLS
  handshake and the first request's head together; a head of at most 16
  KiB and 64 headers; 60 seconds of keep-alive; an address of at most 2
  KiB; a body of at most 64 KiB before sign-in and 1 MiB after; ten
  sign-in requests a minute from one address, on top of authd's waits.
- **A session is a cookie and a row in admind's memory.** The cookie
  `__Host-dukovo` is 32 random bytes in base64url, `Secure`, `HttpOnly`,
  `SameSite=Strict`, with no domain. admind keeps its SHA-256, so its
  memory holds nothing a browser could send. A session ends at its hours,
  at its idle minutes with no page open, at sign-out, when its worker ends
  and when admind stops; admind then tells authd, which forgets the
  session, and the browser's next request gets the answer for a session
  that has ended. Sessions are never written down: a restart of admind
  signs everyone out, which the page says. At most 64 are signed in.
- **A request that changes anything passes three checks, all of them.**
  Its `Origin` is the console's own, built from the name the browser
  asked for; its `Sec-Fetch-Site` is absent or `same-origin`; and its
  `X-Dukovo-CSRF` is the session's token, compared in constant time. The
  routes that sign a browser in have no token yet and keep the first two.
  A body is `application/json`; nothing takes a form. A request's `Host`
  must be this machine's name, an address of it, `localhost` or a name of
  its certificate, with any port, or it is answered 421, so that no other
  name's DNS can point a browser here; the port is free so that a port
  forward or an SSH tunnel works, and the `Origin` is checked against it.
- **Every answer carries a strict content security policy** (the
  console's own files, no inline script or style, no frames), `nosniff`,
  `Referrer-Policy: no-referrer`, the same-origin policies, a permissions
  policy that keeps the camera, microphone and location off and the
  passkey functions on, and no `Server` header. Strict-Transport-Security
  comes only while admind serves an authority's certificate: for the
  machine's own it would stop the browser's one-time warning from being
  passed.
- **The worker is `admind --worker`, authd's child, as the person.** authd
  starts it for the person and their groups in the person's session root,
  with no bypass right and no control right, the network a session gets
  (ADR 0257), and its link to admind: the pipe it reads admind's calls
  from as descriptor 0, and the pipe it writes the replies to as 3. Pipes,
  not a pair of sockets, as a socket cannot be sent to authd (ADR 0091).
  It opens no socket and listens on nothing. admind and the worker speak
  JSON in frames of a 4-byte length and at most 4 MiB, many calls at once,
  told apart by an id; admind gives up on a call after 30 seconds, and the
  worker works on 16 at once. The worker ends when admind closes the
  calls' pipe, when its account is
  disabled (authd ends what an account runs, ADR 0220), or after 12 hours
  with no call.
- **Unlock is a second proof of the person.** For 15 minutes after a
  passkey (with the person checked), an app code or the password, the
  session's changes run as an administrator. authd keeps the end of the
  unlock in memory, for the session; one tap sets it 15 minutes from now;
  locking, signing out, and a minute with no console page open end it. It
  is always asked, for an account that elevates without a password at a
  terminal too, and it is refused for a session that may only add a second
  step and for one past its hours.
- **Elevated commands are authd's children, started as elevate's are.**
  `console-elevate` runs a command as an administrator for a session that
  is unlocked, with `elevate::start` (ADR 0219) and the same environment,
  `SUDO_USER` being the person, and `DUKOVO_CONSOLE_SESSION`, the first
  eight characters of the session, so that the journal can say which
  session ran it. The command is found only in `/system/commands` and
  `/applications/commands`, is never a shell, and has at most 64
  arguments and 32 KiB. admind holds its pipes, so a command lives on
  after the page that started it; it ends with admind, by SIGHUP, as
  elevate's end with elevate. authd takes at most 32 connections from
  admind, which is also the most commands that run at once.
- **An API token is never more than its owner, and always runs out.**
  `dkv_ACCOUNT_SECRET`, the secret being 32 random bytes in hexadecimal,
  shown once and kept as SHA-256, with a name of 1 to 32 letters, digits and
  `-`, its scopes (`*`, an area, or an area with `:read`), the day it ends,
  at most 366 days after it was made, and when it was last used. An account
  keeps 32. Its owner's rights are checked as for a session: a command run as
  an administrator needs the owner to be an administrator and, in this
  release, the scope `*`. Renaming an account ends its tokens. admind will
  take a token only as `Authorization: Bearer`, never in an address or a
  cookie, and never beside a session's cookie. It takes none in this
  release: the API that does comes in the next part, and an answer says so.
- **The errors have IDs.** An error is `{"error": {"id", "title",
  "detail", "fix"}}`, `ADM-101` to `ADM-121`, each with a page in
  `docs/errors`. The detail says what happened this time, in authd's own
  words where authd refused, which it writes for people.
- **Audit and records.** authd audits each sign-in and how it was proved,
  each sign-out, wrong proof, change of a second step, unlock, extension
  and elevated command, never with a secret in it, and a control character
  as `?` (ADR 0211). A sign-in and a sign-out of the console are records
  of the sign-in file, with the place and the browser's address, so that
  `last` and a person's next "Last sign-in" say them (ADR 0254).
- **The console's files are checked when admind starts.** The build
  installs them in `/system/resources/admin` with a `manifest.txt` of
  each file's SHA-256, and admind keeps in memory the files that match
  it and serves no other, so that a file changed on the disk does not
  become a page of the console. They are plain modules and style sheets
  with no framework, no build step and no file from another site.
- **The machine's own sign-in screen says where to open the console.**
  admind writes its address and the certificate's fingerprint to
  `/system/live/admind/issue`, which consoled shows under its banner.
  Someone at the machine can compare the fingerprint with the browser's
  warning the first time.
- **A lab build has one more route.** Built with the `lab` feature,
  admind answers `POST /api/1/test/elevate`, which runs a command in an
  unlocked session, so that the lab can see an unlocked session do what
  it is for. The build that ships has no such route.

## Consequences

- A flaw in admind lets an attacker act as the sessions that are signed
  in, and as the unlocked ones while they are unlocked. It does not let
  them sign in as anyone without a proof, or read a secret, since admind
  has none. That is the most a program that serves a console can promise.
- Every step of a sign-in is a call to authd, and a wrong proof waits
  there. authd's memory holds the sign-ins under way, the unlocks and the
  console sessions, and its restart ends each session's worker, which is
  its child, so a console signs in again after one.
- The secrets are only in authd, and the rules for them are in the place
  the consoles' and SSH's sign-ins already are. A second step for those is
  a change to `login` and sshd that moves nothing; it is a deferred row
  in `docs/deferred.md`, as is opening the keyring at a console sign-in,
  which this release does not do.
- Passkeys and app codes are checked on the machine, with no company's
  server in between. A passkey works by name: a machine opened by its
  address signs in with a password and an app.
- A restart of admind signs everyone out. The sessions are in no file, so
  none can be read from a disk or taken from a backup.
- The console has no HTTP/2 and checks no attestation. Each is a row in
  `docs/deferred.md`.
- An address that is not on `reach` gets nothing, not an error, and an
  administrator who sets `reach = "any"` is told in admin.toml(5) to ask
  administrators for a second step first. A later part of the console
  makes that one choice in a page.
- Blocking an address at the firewall after failed sign-ins is a deferred
  row for the console's Security page; authd's waits and admind's limits
  slow guessing until it exists.
- A token has an owner, scopes and an end, and cannot be used yet. The
  next part adds the `admin` command and the API that take it.
- The error IDs are part of what a person can quote; each page in
  `docs/errors` is checked against the IDs the code gives, as the other
  areas' are (ADR 0420).
- The console's lab tests (ADR 0510) cover the sign-in, passkeys with a
  browser's virtual authenticator, unlock, and each route's refusal of a
  request from another origin.

## Alternatives considered

- **A bridge for each session, with sudo for administrator's work.** A
  process runs as the person who signed in, and a second program asks for
  the password again and runs the command through sudo. Dukovo has
  authd, which already signs people in, asks for the password again and
  runs commands as an administrator, with waits, an audit and an end for
  each command. A second way would be a second set of rules to keep equal.
  An elevated command here is authd's child, held by the console's server,
  and the unlock is a state authd can show, time and end.
- **admind holding the secrets.** The program that reads the network
  would then hold every hash, secret and key, and a flaw in it would be a
  flaw in all of them. SSH and the consoles would need a copy of the rules.
- **A session store on disk.** It would survive a restart, but a lost
  disk or a read flaw would give out sessions that are alive. Signing in
  again after a restart is cheap, and the page says why.
- **Signed tokens (JWTs) for sessions.** A signed token cannot be taken
  back before it ends without a list kept on the server, which is a
  session store again. Signing out, locking and "this account may no
  longer use the console" must act at once, and an opaque cookie that is a
  key into authd's and admind's memory does.
- **HTTP/2.** Nothing the console does needs it; HTTP/1.1 is the smaller
  surface, and the browser's six connections are enough with one event
  stream for each page. It is a deferred row, for when a need shows.

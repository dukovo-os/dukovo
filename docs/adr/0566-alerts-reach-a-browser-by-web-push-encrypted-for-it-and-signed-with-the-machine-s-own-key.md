<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0566. Alerts reach a browser by Web Push, encrypted for it and signed with the machine's own key

- Status: Accepted
- Date: 2026-11-16

## Context

alertd tells the owner of each alert on the screen, at the next
sign-in, by email and by webhooks (ADR 0485). The web console is where
an administrator looks after the machine, often from a phone, and a
browser can show a notification from a site even when no page of it is
open: Web Push. The browser's maker runs a push service, which carries
a message from the site to the browser, and three standards say how:
the message is encrypted for the browser alone (RFC 8291, with RFC
8188's `aes128gcm`), the site signs each request with a key of its own
that the browser subscribed with (VAPID, RFC 8292), and the push
service passes the message on when the browser is next online (RFC
8030).

What a browser gives the site, its endpoint at the push service and
two keys, is a secret: whoever holds it can send to that browser. The
endpoint is also an address that alertd posts to, which a page gives
it, so it must not lead alertd into the local network.

## Decision

- **The machine's key.** alertd makes a VAPID key pair, ECDSA on P-256,
  the first time it starts, and keeps it in
  `/system/state/alerts/vapid.pem` (PKCS #8, mode 0600), written to a
  new file, made durable and renamed. A new key would end every
  browser's subscription, so a key file that is there and does not
  read is never replaced: push is off, and the log says why, until it
  is mended. The socket's `push-key` gives the public half as a
  browser takes it, the uncompressed point in base64url.
- **The browsers.** The socket's `push-add` keeps a browser's
  subscription in `/system/state/alerts/push.json` (mode 0600): an ID
  of 8 random bytes, the name the person gave it, the endpoint, the
  browser's `p256dh` and `auth`, the account it is for, when it was
  added, the least severity it is told, whether it is told of clears,
  and the kinds of alert it asked for. At most 32 are kept; the same
  endpoint again replaces what is kept for it and keeps its ID.
  `push-remove` takes one away and `push-list` lists them, without an
  endpoint or a key. None of the secrets is ever in the log, an error,
  an answer or `Debug`: a browser is known by its name and its push
  service's host. Only root's processes may ask, as for every ask of
  the socket; the web console asks for an administrator.
- **Each browser is a channel.** It is named `push to NAME`, after
  alerts.toml's channels, and is gathered for 30 seconds, queued,
  tried again and dropped after a day as an email is. The browsers are
  kept in alertd's own file, not in alerts.toml: a browser adds and
  removes itself, and alerts.toml is written by people, with their
  comments and order.
- **A message.** One JSON object of at most 3,000 bytes before it is
  encrypted: the machine's name, how many changes the gathered message
  holds, and the first change (its state, severity, title, source, key
  and time, and its detail cut to 600 characters). It is encrypted as
  RFC 8291 says, with a new ephemeral P-256 key and 16 random bytes of
  salt each time, in one record, and posted with `TTL: 86400`,
  `Urgency: high` for a critical alert and `normal` otherwise, a
  `Topic` made from the alert's source and key, so that a newer push of
  the same alert replaces one not yet delivered, and an ES256 token for
  the endpoint's origin, good for 12 hours, whose contact is the
  project's address. The console's service worker writes what the
  browser shows from it.
- **The answers.** 201 or 202 is delivered. 404 or 410 means the
  browser ended the subscription: alertd removes the browser, logs it
  (ALR-104), and records the event `push-ended:ID`, which the other
  channels are told, so that a phone that stopped getting alerts does
  not go unnoticed. 413 is sent once more with the detail cut to 200
  characters. Any other answer is a failure that is tried again, with
  the push service's words in `alerts status` (ALR-105). At most 4 KiB
  of an answer is read.
- **Where alertd posts.** An endpoint is an `https` address of at most
  1,024 bytes. Its host is looked up at each message, and refused when
  any of its addresses is unspecified, loopback, private, shared,
  link-local, multicast, reserved or kept for documentation, for IPv6
  also unique local or an IPv4 address of such a kind inside one;
  alertd then connects only to an address it checked, with no second
  lookup, and follows no redirect. An endpoint written as an address
  is checked the same way when it is added. alerts.toml's
  `[push] private` lists the networks a push service of one's own may
  be on.
- **The console's part.** The Alerts page offers push to administrators
  only, as the asks that add and remove a browser are an
  administrator's. Its service worker shows a notification for each
  push, from the message's words, and opens the console's page that the
  alert's source and key point to; it has no `fetch` handler and keeps
  no pages, so the console never works from a cache that an update
  left behind. admind's content security policy lets the console's own
  worker and manifest load, and nothing else.
- **Alerts that reach nobody.** dukovo-check warns (SEC-170) when at
  least one channel is set and none has taken a message in a day while
  each failed in that time. A machine as installed, with no channel, is
  not warned: it tells its owner on the screen and at sign-in, and
  whether to add a channel is the owner's choice.
- **ring does it all.** ECDH on P-256, HKDF, AES-128-GCM and ECDSA
  signing are ring's, which alertd has already through rustls: no new
  crate. RFC 8291's Appendix A example is a unit test: its keys, salt
  and text give its message, byte for byte.

## Consequences

- An administrator who turns alerts on in a browser gets them as the
  device's own notifications, phone or computer, with no account at a
  third party and nothing to install.
- The push service of the browser's maker carries every message and
  cannot read one. It sees this machine's IP address, when each message
  is sent and how big it is, the endpoint it gave the browser, and this
  machine's public key, the same on every message. The console's
  Alerts page and the handbook's privacy page say so before push is
  turned on, and nothing is sent until it is.
- A browser runs a service worker only for a page whose certificate it
  trusts, so push needs the console to have a certificate the browser
  trusts: one from Let's Encrypt, or the machine's own made trusted on
  that device. Until then the page says so, and email and webhooks
  reach a phone.
- A lost or broken key file ends every browser's alerts; each browser
  must turn them on again after the key is mended or removed.
- A push service on a private network is refused unless the owner
  lists its network; a lab's fake push service is listed that way.
- Tests: units for the RFC 8291 example, the token's shape and
  signature, the key's mode and that a broken key is not replaced, the
  store's limits and secrets, the address rules, the message's size,
  the topic and the refusals; alertd's tests for a browser's channel,
  its end and the socket's asks; the `admin-alerts` boot test with a
  push service of its own on the lab network that decrypts what it
  gets.

## Alternatives considered

- **ntfy or another relay.** It works today as a webhook (ADR 0485),
  but needs an app and a topic at a third party; push needs neither,
  and its messages cannot be read on the way.
- **The browsers in alerts.toml.** A page would write a file people
  edit, and fight their comments; alertd's own file holds what only
  alertd and the console change.
- **A key made again when the file does not read.** It would end every
  subscription silently; a broken file is told instead.
- **Checking the endpoint's host only when it is added.** A name can
  lead to another address later; it is checked at each message, at the
  address alertd then connects to.
- **A crate for Web Push.** The ones there are bring an HTTP client and
  a TLS stack of their own; ring and tuf's HTTP over rustls are already
  alertd's.

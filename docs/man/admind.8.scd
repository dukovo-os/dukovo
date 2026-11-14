admind(8) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

admind - the web console's server, Dukovo Admin

# SYNOPSIS

*admind*

*admind --worker*

# DESCRIPTION

admind is the web console (ADR 0514): it answers HTTPS on port 9090, or
the port *admin.toml*(5) names, serves the console's pages from
_/system/resources/admin_, and answers the pages' calls under _/api/1_.
Spark runs it as the *admind* service, after *authd*(8) and *certd*(8),
with the certificate certd keeps for the machine, *this-machine* until
*admin.toml*(5) names another.

admind holds no secret of anyone's. *authd*(8) checks every password,
code, passkey and token, signs a person in as themselves, and decides who
may unlock a session and what an unlocked session runs as an
administrator. admind keeps the sessions in memory alone, so that
restarting it signs everyone out, and asks authd for each step. The
second steps and tokens of the person who is signed in are done by a
worker, *admind --worker*, which authd starts for each session as that
person, without the rights of an administrator. The worker takes no
connection of its own: it answers admind over the descriptor authd gives
it.

*admind* alone is the server, which Spark runs; *admind --worker* is what
authd runs, and is not for typing.

## Whom it answers

The firewall (*firewall*(8)) opens a running service's port to every
machine, so admind checks each connection's address itself, before it
says a byte. With *reach = "private"*, the default, it answers this
machine, the networks of its own cards, and the address ranges no one
reaches from the internet: loopback, link-local, 10.0.0.0/8,
172.16.0.0/12, 192.168.0.0/16, 100.64.0.0/10 (where Tailscale puts
machines) and fc00::/7. Any other connection is closed with no word, and
the log says so once a minute for each address, never once for each try.
*reach = "any"* answers all.

It serves at most 512 connections at once and 32 from one address (an
IPv6 address counts by its /64). A client has 10 seconds from its
connection for the TLS handshake and the head of its first request, a
head is at most 16 KiB and 64 headers, and a connection that sends no
head for 60 seconds is closed.

## What it checks

A request meets these checks in this order, and the first that fails
answers it: its method is *GET*, *HEAD* or *POST* and its address at most
2 KiB; its *Host* is this machine's name, an address of it or a name of
its certificate, so that no other name's DNS can point a browser here
(421); it brings no API token, which the console does not take yet
(401); its address is a route or a file; a route for the signed in has a
session (401), and one that is not for a session that may only add a
second step has one that may do more (403); a request that changes
anything comes from the console's own page, by *Origin*,
*Sec-Fetch-Site* and the session's *X-Dukovo-CSRF* token (403); and its
body is JSON, at most 64 KiB before sign-in and 1 MiB after (415, 413).
The sign-in routes also take at most ten requests a minute from one
address (429, with *Retry-After*), on top of the wait authd puts on a
wrong password.

Every answer carries a strict *Content-Security-Policy* and the headers
that go with it, and no *Server* header. The session's cookie is
*\_\_Host-dukovo*, *Secure*, *HttpOnly* and *SameSite=Strict*. Browsers
are asked to come back over HTTPS alone (*Strict-Transport-Security*)
only while admind serves a certificate from an authority, never the
machine's own, which would stop the browser's one-time warning from being
passed.

## Sessions

A session ends at *session-hours* after its sign-in, *idle-minutes* after
its last page closed, when the person signs out, when its worker ends, or
when admind stops. admind says why to each page that is open, which shows
it, and tells authd, which forgets the session. A session that is unlocked
(15 minutes, by the password, a code or a passkey again) locks itself a
minute after its last page closed.

## Errors

An error is *{"error": {"id", "title", "detail", "fix"}}*, with an ID that
a person can quote, *ADM-101* and on: *ADM-101* the name or the password
is wrong, *ADM-102* a code is wrong, *ADM-103* a passkey was not accepted,
*ADM-104* the account may not use the console, *ADM-105* too many tries,
*ADM-106* the console is locked, *ADM-107* the session has ended,
*ADM-108* the request is not what the route takes, *ADM-109* it did not
come from the console's page, *ADM-110* authd refused, *ADM-111* authd did
not answer, *ADM-112* the worker did not answer, *ADM-113* too big,
*ADM-114* not this machine's name, *ADM-115* API tokens are not taken
yet, *ADM-116* nothing is at that address, *ADM-117* the route takes
another method, *ADM-118* the session may only add a second step,
*ADM-119* the body is not JSON, *ADM-120* the console is full, *ADM-121*
the console could not do that. *detail* says what happened this time, in
authd's own words where authd refused.

# FILES

_/system/settings/admin.toml_
	The sign-in rules, the port, whom it answers and the certificate
	(*admin.toml*(5)).

_/system/resources/admin_
	The console's pages. Its _manifest.txt_ lists the SHA-256 of each
	file, and admind serves only the files that match.

_/system/live/admind/issue_
	The address and the certificate's fingerprint, for the sign-in
	text of the consoles.

_/system/live/authd/socket_
	authd's socket.

_/system/settings/spark/services/admind.toml_
	The service.

# SIGNALS

*SIGHUP* (*service admind reload*) reads the sign-in rules, *reach* and
the certificate again, for the connections that come after. One that does
not read leaves the old in use, and the log says why. A new port takes a
restart. *SIGTERM* ends the sessions and stops.

# LOG

*log admind* has a line for the start, with the address and the
certificate's fingerprint, and for each sign-in, sign-out and refusal, with
the browser's address; never a password, code, cookie, token, ticket or
challenge.

# EXIT STATUS

admind runs until it is stopped, and SIGTERM ends it with 0. It exits
with 1 when it cannot start: when *admin.toml* or the certificate does
not read, or it cannot listen.

# SEE ALSO

*admin.toml*(5), *authd*(8), *certd*(8), *cert*(8), *firewall*(8),
*service*(8), *log*(1)

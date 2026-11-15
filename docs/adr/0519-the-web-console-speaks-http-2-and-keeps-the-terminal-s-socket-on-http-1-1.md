<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0519. The web console speaks HTTP/2, and keeps the Terminal's socket on HTTP/1.1

- Status: Accepted
- Date: 2026-11-14

## Context

admind spoke HTTP/1.1 alone (ADR 0514). A browser opens at most six
HTTP/1.1 connections to one site, which all its tabs share. Each console
tab holds one of them for its event stream, so a seventh tab waited for
a free connection, and with the Logs page's follow (ADR 0518) two tabs
of the Logs page hold four. Each new connection also costs a TLS
handshake, and a browser that has not yet accepted the console's own
certificate asks the person again.

HTTP/2 carries every request and stream of a browser on one connection.
hyper, which admind already uses, has an HTTP/2 server; the Rust crate
it uses for it, h2, was already in the tree's lock files. admind's limits
were written for HTTP/1.1: a place for each connection (512, and 32 from
one address), 20 seconds for the handshake and the first request's
head, 16 KiB and 64 headers for a head, 60 seconds of quiet before a
connection closes, and a `Host` check against DNS rebinding.

The Terminal's socket is a WebSocket. Over HTTP/2 a WebSocket needs the
extended CONNECT of RFC 8441, which hyper's server supports only through
an upgrade path that admind's handover of the connection's place does
not follow. Browsers open a connection of their own for a WebSocket,
offering HTTP/1.1 alone, when the server does not announce RFC 8441.

## Decision

- **ALPN offers `h2`, then `http/1.1`.** A browser that offers HTTP/2
  gets it; one that offers HTTP/1.1 alone, as a browser does for a
  WebSocket, gets HTTP/1.1, and a client that offers nothing speaks
  HTTP/1.1 as before. The TLS 1.2 suites admind has (ECDHE with GCM or
  ChaCha20) are the ones HTTP/2 allows.
- **The limits hold for streams.** An HTTP/2 connection takes one place,
  as an HTTP/1.1 one does. It has at most 64 streams open at once. A
  stream's head (its header list) is held to the same 16 KiB. The
  handshake and the first request have the same 20 seconds. A connection
  with no stream open for 60 seconds is told to go away (GOAWAY), which
  ends it once its streams have; an open event stream or follow keeps it.
  The network's keep-alive probes find a browser that went away, as
  before.
- **The `Host` check reads `:authority`.** On HTTP/2 the host is the
  request's `:authority`, which hyper puts in the URI; a `Host` header
  beside it must name the same (RFC 9113 8.3.1), and a request that
  names two is refused as another machine's (421, ADM-114). On
  HTTP/1.1 the `Host` header alone counts, as before.
- **The Terminal's socket stays on HTTP/1.1.** admind does not announce
  RFC 8441, so a browser opens an HTTP/1.1 connection for the socket,
  which takes over that connection's place as before (ADR 0517).
- **The streams run on admind's one thread.** An HTTP/2 connection's
  streams are futures that hyper spawns; admind gives hyper an executor
  that spawns them on its current-thread runtime, beside the sessions
  they read, which are not shared between threads.

## Consequences

- A browser holds one connection to the console for all its tabs, plus
  one for each Terminal page's socket; the seventh tab no longer waits.
- A page that must reach admind over a connection made after a change
  (ADR 0518's trials) cannot ask the browser for one: admind tells the
  connection to go away, and the browser's next request opens a new one.
- The lab's browser tests run over HTTP/2, as people's browsers do; the
  unit tests ask over both, with hyper's client.
- RFC 8441 for the Terminal's socket, which would save that connection,
  is not planned: one connection for the Terminal is cheap, and the
  handover of its place stays as reviewed.

## Alternatives considered

- **HTTP/1.1 with fewer streams.** One event stream shared by a
  browser's tabs (a SharedWorker or a BroadcastChannel leader) frees
  connections, but the Logs page's follow, the Terminal and every request
  still compete for six, and a tab whose leader closes must take over the
  stream. HTTP/2 removes the limit where it is.
- **HTTP/3.** It needs QUIC over UDP, a second listener and a second TLS
  stack, for a console on a local network where HTTP/2's one connection
  is enough.
- **The WebSocket over HTTP/2.** It needs RFC 8441 and a handover of a
  stream's place in place of a connection's, which admind's limits do
  not count; it saves one connection a Terminal page.

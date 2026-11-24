<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0692. The MCP server speaks five revisions of the protocol, and keeps no session in any

- Status: Accepted
- Date: 2026-11-24

## Context

The Model Context Protocol is JSON-RPC 2.0 with dated revisions, and the
programs that speak it are not all at the same one. There are two eras.

The four revisions 2024-11-05, 2025-03-26, 2025-06-18 and 2025-11-25
begin with a handshake: the client sends `initialize` with the revision
it wants and what it can do, the server answers with the revision they
will speak and what it can do, and the client sends
`notifications/initialized`. Over HTTP a server of these revisions may
give the client a session ID to send with each request, and may answer a
request with a stream of events in place of one JSON body. The two oldest
allow a batch, several messages in one array.

Revision 2026-07-28 has no handshake. Every request says its revision and
its client's capabilities in `params._meta`, under the keys
`io.modelcontextprotocol/protocolVersion` and
`io.modelcontextprotocol/clientCapabilities`. A client begins with
`server/discover`, and falls back to `initialize` when a server does not
know that method. Every result says its `resultType`. Over HTTP each
request repeats its revision, its method and, for a call, the tool's
name in the headers `MCP-Protocol-Version`, `Mcp-Method` and `Mcp-Name`,
so that what is between the client and the server can route a request
without reading its body.

admind keeps its sessions in memory, and restarts when the system is
updated, when its settings change in a way a reload cannot take, and when
an administrator restarts it. A client's settings are written once and
then left for months.

## Decision

**All five revisions are served.** A message is of the newest when its
`params._meta` holds one of the two keys, and of the older four when it
does not. A client that begins with `server/discover` is answered, with
the list of all five; one that begins with `initialize` is answered with
the revision it asked for when that is one of the four, and with
2025-11-25 when it is not, to take or leave. A revision that is not
spoken is error -32022, with the list.

**There is no session, in either era.** Nothing that the handshake
settles changes an answer here: the server has tools and nothing else,
their list does not change while it runs, and it sends a client no
request of its own. So a request of an older revision is answered whether
`initialize` came first or not, `/mcp` gives no session ID and reads
none, and a client goes on without a word after admind restarts. Who the
client is comes with every message, as its token or its pass, and authd
is asked each time (ADR 0690).

**`/mcp` is one message in and one JSON body out.** `POST` alone: `GET`
and `DELETE`, which the older revisions use for a stream and for ending a
session, are 405. There is no stream of events. A notification is
answered 202 with no body. A batch of at most 16 is taken from the two
oldest revisions alone, and a request that names no revision in its
header is of 2025-03-26, the last revision before that header. For the
newest revision the three headers must be there and agree with the body,
or the answer is error -32020 and the status 400; a header's value that a
header cannot carry comes as `=?base64?`, the base64 of its UTF-8, and
`?=`. The status is the one the protocol fixes for each error: 400 for
JSON that does not read (-32700), a message that is not one (-32600), an
envelope that is wrong and a revision that is not spoken; 404 for a
method the newest revision does not have here; and 200 for everything
else, a tool's failure included.

**The socket of the clients over SSH is the same server.** One message a
line, in either era, with no headers to agree with, and no answer to a
notification.

**What it answers.** `server/discover` and `tools/list` say `ttlMs`
300000 and `cacheScope` `private`: a client may keep them five minutes,
for itself. The capabilities are `tools` and nothing more. A tool's
result has `content` with the answer as text, `structuredContent` with
it as JSON, and `isError`; in the newest revision also `resultType`,
`complete`. A tool that fails answers such a result, with the console's
error as the API gives it, and not an error of the protocol: the client
reads the result and can try again, where an error would only end the
call. Each tool says whether it reads (`readOnlyHint`), and that it
reaches this server and nothing beyond it.

**What it does not do.** No resources, no prompts, no completions, no
logging level, and none of the questions a server may ask a client's
user. A request for a change waits for a person at the console and not
for the client, so `ask` waits 45 seconds and then answers the request's
number, and `request` says later where it stands. The protocol's own
tasks are an extension that few clients have.

## Consequences

- A client set up once keeps working across restarts of admind and
  updates of the system, with nothing to renew.
- The bridge of `admin mcp` needs no state of the protocol either: it
  carries lines, and knows only which requests are on their way.
- A client is told nothing it did not ask for. It learns that a request
  was answered by asking `request` again, or from the answer of the `ask`
  that still waits.
- No connection to `/mcp` stays open, so the console's count of
  connections and its reach need nothing new.
- A revision after 2026-07-28 is refused with the list of those spoken
  until `protocol.rs` learns it. Its unit tests hold each revision's
  messages, headers and statuses.
- A client that speaks only a revision older than 2024-11-05 is offered
  2025-11-25 in answer to its `initialize`, and leaves.

## Alternatives considered

- **Sessions, as the older transport allows.** A session's state would be
  lost at each restart of admind, and every client would have to notice
  and begin again. There is nothing here for a session to hold.
- **The newest revision alone.** The programs in use today are spread
  over all five, and a client that cannot connect says little of why.
- **A stream of events for `ask`.** It would hold a connection open for
  as long as a person takes to answer, and a restart would lose it. The
  request stands in the console whether the client still listens or not.
- **The protocol's tasks for a request that waits.** They fit, and few
  clients have them yet. `request` does the same with an ordinary tool,
  and tasks can be added beside it.

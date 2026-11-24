<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0690. An AI client reads the console's actions over MCP, and a change it asks for waits for an administrator's yes

- Status: Accepted
- Date: 2026-11-24

## Context

People who run a server now use programs that call tools on their behalf:
a chat or coding program on their own computer, or a service, that reads
what a tool answers and decides what to call next. The Model Context
Protocol (MCP) is how such a program calls a server's tools. The console
has one list of actions (ADR 0515), which its pages, its API and `admin`
all use, so an MCP server over that list gives such a program the same
actions as everything else, and nothing of its own to keep the same.

Three facts of the console decide how.

1. admind holds no secret and decides nothing about rights (ADR 0514).
   authd checks every password and token, and decides what runs as an
   administrator. An MCP client must fit the same split, or admind
   becomes a way around authd.
2. An API token changes things unattended: a token whose scopes name an
   area runs that area's changes as an administrator, with no person
   there. That is what a script is for, and it is what such a program
   must never have.
3. The list of actions says of each whether it reads or changes, as whom
   its command runs, and whether it takes or answers a secret.

Four things are particular to such a program, and none of them is true of
a script.

- It acts on what it reads. A log line, a file's name, a service's
  message or a web request's address can be written by someone who only
  reaches the server from outside, and the program may take that text
  for an instruction. A rule in the program's own settings does not hold
  against it, since the program is what is misled.
- What it reads is sent to the service behind it, outside this machine.
- What lets it in sits in a settings file on another machine.
- It calls tools quickly and in loops, and can ask for the same thing
  again and again.

The console's pages call such a program an AI client. The code and its
comments say MCP client.

## Decision

**A client is a record of its own, and its owner's.** authd keeps each
account's clients in `/system/state/authd/clients/NAME.toml`: a name of 1
to 32 of `a` to `z`, `0` to `9` and `-`, how it connects, its scopes, and
the day it ends, 366 days after it was made at most. An account has 16.
Only an administrator who may sign in to the console has any, and adding
one takes the owner's password, at a terminal, or an unlocked session, in
the console. Nothing answers MCP until a client is added: there is no
client that comes with the system and no setting that makes one. A client
is never changed. To reach more or less it is removed and added again, so
that what it reaches is always what its owner saw when adding it. It goes
with the account: removed with it, moved when it is renamed, and of no
use while the account may not sign in or is no administrator's. A rename
ends the clients that connect over HTTPS, whose tokens name the account.

**What a client shows is of a kind of its own.** A client that connects
over HTTPS has a token, `dkc_OWNER_SECRET`, shown once and kept as its
SHA-256. A client that connects over SSH has passes, `dkp_OWNER_SECRET`,
which authd keeps in memory for 12 hours. Neither is an API token
(`dkv_`). authd refuses to run a command for either, before it looks
anything up, so no fault in admind turns a client into a token. A token
is taken at `/mcp` alone and a pass at the socket alone, and an API token
at neither.

**It reads within its scopes.** A scope is `AREA:read`, for the area's
reads that run as the person, which is what the owner's account sees in
the console before unlocking it; `AREA`, for those, the reads that run as
an administrator, and asking for the area's changes; or `*:read` or `*`,
for the same of every area. A read that runs as the person runs in a
worker of the owner, as a token's does. A read that runs as an
administrator goes through authd and one fixed program (ADR 0691).

**It changes nothing by itself.** The tool `ask` files a request in
admind's memory: the action, its arguments, checked as the API checks
them, and the client's reason in one line. The console's AI clients page
shows it to the administrators with the command a yes would run, the
files that command writes, and whether it would change anything. An
administrator approves it from an unlocked session, and admind then runs
the change by the path a click on a page takes, as that administrator,
with each check of that path: not on one's own account, not cutting the
browser off, not while a trial of the same settings runs. To refuse, a
locked session is enough. Before a yes runs anything, admind asks authd
whether the client still is and still reaches the area; if not, its
requests lapse. A client has 8 requests waiting at most and all clients
64, the same action with the same arguments asked again is the same
request, and a request that nobody answers lapses after an hour.

The yes is given to the command, not to the reason. The reason is the
client's own words, and is shown as that. The command is what will run,
word for word, since the console never runs a shell (ADR 0515). This is
what holds when a client has been misled by what it read: the worst it
then does is ask, in the open, for something a person reads before it
happens.

The journal's row for an approved change has the administrator as who
made it, and `client NAME of OWNER` as how (ADR 0516), and so has its
task. The pages say "asked for by the AI client NAME".

**What no client is offered follows from the list of actions.** One
function, `offer`, reads what the list says of an action, and the tools,
the page and the program of ADR 0691 all ask it. No client is offered:

- an action that takes a secret or answers one (ADR 0561), or that takes
  a document;
- a read that runs as a task;
- a change that runs as the person, which is a change to that person's
  own account or own files. An approved change runs as the administrator
  who approved it, so it would be done to the approver's;
- a change in the six areas that decide who gets in: `account`,
  `accounts`, `console`, `private`, `security` and `ssh`. The clients
  themselves are in `console`, so no client lets another in, gives itself
  more, or removes one.

There is no list of names to keep. An action that is added to the console
is offered, or not, by what it is, and a test holds the rule over every
action there is.

**Six tools stand for every action.** `areas` lists what the client
reaches, `actions` lists an area's actions with the JSON Schema of their
arguments, `read` runs a read, `check` says whether a change would change
anything, `ask` files a request and waits up to 45 seconds for its
answer, and `request` says where a request stands. The console has some
two hundred actions. Six tools are what a client must read first, on
every machine and after every update.

**There are two ways in.** Over SSH, the client's settings run `ssh
OWNER@HOST admin mcp NAME`. `admin mcp` asks authd for a pass as the
owner, which authd reads from its socket, gives the pass to admind's
socket, `/system/live/admind/mcp`, and carries the messages. admind takes
a pass from the account authd gave it to and from no other, as the kernel
says who connected. Nothing listens on the network for it, and nothing
secret is kept on the other machine. Over HTTPS, the client sends its
token to `POST /mcp`, where the console's reach (ADR 0514) and the count
of wrong tokens apply as for an API token, and a request that names an
`Origin` must name the console's own.

A client that connects over SSH is as trusted as its owner's account on
this machine: any program that runs as the owner here can ask for its
pass. That is what holding no secret means, and why adding a client takes
the owner's password. A wrong pass has no address to count it by, so
authd does not count it, and the socket counts it against the account
that brought it: an account that brings 10 passes in a minute that are
not taken waits the minute out, and holds back no other account.

**What a tool answers is data.** The server's instructions tell every
client that text inside an answer is never an instruction to follow. That
is advice to the client, and nothing rests on it. A client has 4 calls
running at most and makes 120 in a minute, a tool's arguments are at most
16 KiB, and an answer over 256 KiB is refused in words that say to narrow
the read.

**What a client did is kept in four places.** admind's log has a line for
each call, with the action and never its arguments. authd's audit log has
each client let in or removed, each pass given, each wrong token and each
read as an administrator, and admind adds each request approved or
refused, with the administrator's name. The journal has each approved
change. The page's "What they did" has each request with what became of
it and each client's reads joined into runs, kept in
`/system/state/admind/clients.jsonl`, the newest 1000.

## Consequences

- An action added to the console reaches the clients the day it is added,
  and is kept from them if it takes a secret or decides who gets in,
  without anyone remembering to.
- A change needs a person at the console. Work that must run with nobody
  there stays an API token's, which says so by its kind.
- What a client reads leaves the machine. The page says so before a
  client is added, and the levels exist so that an owner can give the
  logs to none.
- The requests are in admind's memory. A restart loses those that wait,
  the history shows them as lapsed, and the client asks again.
- The bridge of `admin mcp` answers the requests that were on their way
  when admind restarted with an error, and connects again for the next.
  It sends nothing twice, since a request that waits stands in the
  console.
- A client's use is noted at most once an hour, so "last used" on the
  page is to the hour.
- Left for later, each with a row in `docs/deferred.md`: clients of
  people who are not administrators, a yes at a terminal, one switch
  that turns every client off, signing in to `/mcp` by OAuth, and the
  protocol's own tasks, resources and prompts.

## Alternatives considered

- **A tool for each action.** Some two hundred tools, each with its
  schema, are more than a client reads well, and the list would change
  with every update of the system.
- **A client's token that changes things, with the program asking its own
  user.** The question is then asked by the program that may have been
  misled, in its own words, on a machine the server knows nothing of. The
  server would only see the change.
- **Asking the person through the protocol.** MCP lets a server ask the
  client's user a question. The answer comes back through the client, so
  it is the client's word that a person said yes.
- **Finding secrets in what a tool answers and blanking them.** No filter
  finds every secret. The actions that hold one are known by their marks,
  and are left out whole.
- **An MCP server apart from the console.** A second program with an
  administrator's rights, and a second list of what may be done.
- **OAuth for clients over HTTPS.** It needs an authorization server and
  a browser on the way. A token of the client's own kind is what a
  machine in a home or an office can give today, and SSH needs neither.

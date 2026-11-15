<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0517. The web console's pages are modules in one shell, and its terminals are the person's shells on terminals admind holds

- Status: Accepted
- Date: 2026-11-14

## Context

ADRs 0514 to 0516 give the web console a server: sign-in through authd,
a worker per session that runs commands as the person, actions with a
registry, a journal of what changed, and tasks. The console still has no
pages past its sign-in. Fifteen pages come over the next pull requests,
each from a different part of the system, and each must look and behave
as one console: the same header, navigation, keys, errors, colours and
text sizes, readable by a screen reader and usable at phone width.

The console's pages are served by admind from the machine, with a
content security policy that allows nothing but the console's own files
(ADR 0514). There is no network at build time for a package registry,
and a release must be built from the tree alone. Whatever the pages are
made of has to be in the tree, small enough to load on a slow line, and
reviewable as source.

People who run servers also want a terminal in the browser: to run the
command a page shows, to follow a log, to fix what no page covers yet.
A terminal is a shell, and a shell is the most powerful thing the console
can hand out. It has to obey the same rules as a shell over SSH: who may
have one, the sign-in records `who` and `last` show, the audit, and what
an administrator's shell may do and for how long. It also has to go on
when the page is closed, because a long command must not die with a tab.

## Decision

### Pages

- **ES modules with no framework and no build step.** The pages are
  JavaScript modules in `userspace/admin/`, imported by relative paths,
  as browsers load them. The tree's files are what the browser runs: no
  bundler, no transpiler, no package registry. Views build DOM with
  `dom.js`'s `h()`, never strings of markup; text from the server is set
  as text. Logic that needs no DOM is in modules that take and give
  plain values (`*-model.js` and most of `lib/`), which Node's test
  runner reaches on the build machine.
- **One shell, and a list of pages.** `index.html` loads `app.js`, which
  reads the session and shows the sign-in pages or the shell: the
  header, the navigation, the Tasks drawer, the error view, the keys.
  `pages.js` lists each page's `entry.js`, which is data: its path,
  title, icon, place in the navigation, who it is for, and functions that
  import its code. Loading the list loads no page. A page module gets a
  view (its title, its main element, the API, the time, a signal that
  ends when the person leaves) and gives back what the shell needs.
- **Parts.** A page folder may give parts that other pages call: the
  Overview's needs and rows, Finish setting up's steps, the palette's
  providers, a badge in the navigation. The Overview and the palette
  grow as pages arrive, and a page's folder may give parts before its
  page exists. A part has five seconds; one that fails or runs out of
  time is left out and named in the browser's console, so one page never
  breaks another.
- **Budgets, checked.** The shell's start is under 150 KB as served, and
  each page under its own budget; a host test walks the import graph.
  Another host test holds every file to the content security policy's
  rules: no `innerHTML`, `eval`, inline script or style, `javascript:`
  address, or import that is not relative. admind serves each file with
  the manifest's hash as its `ETag` and answers 304 when the browser
  holds it, so a reload fetches only what changed.
- **The pages cost an idle machine nothing.** admind checks each file
  against the manifest when it starts, as ADR 0514 says, but keeps none
  of them: it reads a file from the disk each time it sends it, checks
  it against the manifest again, and serves nothing that has changed
  since it started. A 304 reads nothing. Every page a later PR adds is
  bytes on the disk, not in admind's memory, which the idle machine's
  budget of 100 MiB (ADR 0427) counts. This supersedes the words of ADR
  0514 that admind keeps the files in memory.
- **A page that goes stops waiting, and its calls go on.** A page's
  signal ends its timers and listeners, and the wait for its calls, but
  not the calls: admind speaks HTTP/1.1, where a fetch the browser
  aborts closes its connection, and each new connection costs a TLS
  handshake, two while the browser accepts a certificate it does not
  trust. Chromium then hands a page load connections that other
  requests began without that leave, each refused, and after 32 gives
  up with `ERR_TOO_MANY_RETRIES`; the lab's shell test found it in one
  move from the Overview and back. A call no page waits for is read to
  its end and dropped, and its connection is used again.
- **The Overview reads only actions.** Each card reads through routes
  that exist, or through read actions of the registry (`console.about`,
  `console.notices`, `checks.last`, `metrics.now`, `metrics.usage`),
  which run in the worker as the person, show their command, and have
  an Ansible task, as every action does. The health score is the
  console's: a table of weights over the kept result of the system's
  checks, 100 less a weight per finding, never below 0. An ID the table
  does not know weighs 3, so a new check never breaks the score.
- **The palette is built from the registry.** Ctrl+K opens one list of
  pages, the registry's actions with the forms their JSON Schemas give,
  things the pages' parts provide, and what the person ran last. A
  change runs through the same path as the button on its page, so a
  locked administrator is asked to unlock first, and the command is
  shown before it runs when the person asks.

### Terminals

- **admind holds the terminal; authd starts the shell.** For each pane,
  admind opens a pseudo-terminal pair, keeps the master, and passes the
  other side to authd with the console's new `terminal` request (the
  console session, `own` or `admin`, the terminal type). authd checks
  that the session is live and full, that the account has a shell, that
  `[terminal] allowed` in `admin.toml` lets it in, and its own bound on
  console shells. It then starts the person's login shell as it starts
  an SSH session's: `login --session` on that terminal, a sign-in record
  with the place `console`, the terminal's name and the browser's
  address, and the sign-in notices. `who` and `last` show it. authd
  answers with the shell's process and reaps it as it does an SSH
  shell's.
- **An administrator's shell ends with the unlock.** An administrator
  whose console session is unlocked may open an administrator pane:
  root's login shell, started as `elevate -i` starts it and audited as an
  elevated session of the console session. authd ends it on its own
  clock when the session locks, its unlock runs out or the session ends:
  SIGHUP to the shell's process group, SIGKILL five seconds later. admind
  shows an administrator pane only to the console session that opened
  it. In a person's own pane, `elevate` asks for the password as over
  SSH: the console's unlock never passes into it.
- **A WebSocket, with its framing written in admind.** The Terminal page
  has one socket, `GET /api/1/terminal`, which carries every pane it
  shows. `websocket.rs` speaks RFC 6455: masked client frames, messages
  of at most 64 KiB, control frames of at most 125 bytes, UTF-8 text,
  ping every 25 seconds, no extensions. The handshake checks the session
  cookie, the `Origin`, `Sec-Fetch-Site`, and the CSRF token, which the
  page sends as a subprotocol (`csrf.TOKEN`), since a browser lets a page
  set no other header on a socket. JSON text messages attach, detach,
  resize, acknowledge and answer; binary messages carry a pane's typed bytes in
  and its output out, each output chunk with its offset.
- **Flow is held back by acknowledgements.** The page acknowledges
  output once it has taken it in. While a socket has more than 256 KiB
  of a pane's output unacknowledged, admind stops reading that master,
  and the program waits as on a slow line. A socket over that for 30
  seconds is detached from the pane, so one stalled browser does not
  stop the others.
- **Terminals live in admind's memory.** Each pane keeps its last 64
  KiB of output and follows the private modes its program set, so a page
  that comes back, or another browser of the same account, sees it
  again: exactly what follows its last offset when the ring still has
  it, else the ring from a line's start after the modes. A program's
  questions to its terminal (where the cursor is, what terminal this
  is) are answered by the page's screen, in an `answer` message apart
  from typing, and only for output written after the page attached:
  what a page sees again has had its answers. When several pages show a
  pane, admind writes the answers of the first opened alone, so a
  program that asks once is answered once. Nothing of a terminal is
  written to disk. Terminals go on when the page closes, end
  after `keep-days` unopened, and end when admind stops. The routes of
  `/api/1/terminals` list, make, split, lay out, rename and end them;
  they are routes, not actions, since they act on live terminals and have
  no command to show. An API token gets none of them.
- **Limits.** 32 terminals per account by default (1 to 64), 4 panes a
  terminal, 256 panes in admind, 4 sockets a console session, 16 panes a
  socket, 64 KiB of typing waiting per pane. At those limits admind
  holds about 32 MiB for terminals, and its service limit is 128M.
- **Text is readable whatever the program's colours.** Each theme's
  colours have 4.5:1 against the terminal's background, but a program may
  put any colour on any other. Text with less than 4.5:1 against its own
  background is drawn in the colour nearest the program's that has it,
  moved towards white or black, by the WCAG 2.2 formula; dim and invisible
  text, the selection and the cursor keep their colours. The rule is in
  its own module, loaded at a screen's first draw.
- **The journal names the administrator pane.** An administrator pane's
  shell edits files as root without naming them on a command line. When
  the journal finds a hand edit made while exactly one account's
  administrator panes ran, it names that account, "in an administrator's
  terminal of this console".

## Consequences

- Every page from C4 on is a folder with an `entry.js`, a `page.js` and
  its parts, and a line in `pages.js`. A page that is not built yet
  simply is not listed; nothing in the shell changes for it.
- With no framework, the shell carries its own small router, focus
  handling and dialogs, and each is tested. There is no dependency to
  update or audit, and the browser runs the files the review read.
- A console restart, by an update or `service admind restart`, ends every
  terminal. The page and the handbook say so. A keeper process that
  holds terminals across restarts, and `term attach` from SSH, are a
  later pull request.
- A pane's shell is a full sign-in: an account that may not sign in
  over SSH may still have a console terminal when `admin.toml` allows it,
  and an account with no shell never does.
- Another administrator's view of a person's terminals, the shell's exit
  status in the page's `ended` message, and the browser's language for
  the shell's `LANG` are in `docs/deferred.md`.
- Each console tab holds one of the six connections a browser opens to
  admind over HTTP/1.1, for its event stream, so a seventh tab waits for
  a free one. HTTP/2, which carries every tab on one connection, is in
  `docs/deferred.md` for the next pull request.
- The emulator is written for the console and tested with recorded and
  built sessions; it follows the sequences `xterm-256color` describes
  that programs on Dukovo send, not every terminal's quirks.

## Alternatives considered

- **A framework with a build step.** It needs a package registry at
  build time and a bundler in the tree, and the files the browser runs
  are no longer the files reviewed. The console's pages are forms,
  tables and a few charts, which plain modules do well.
- **The event stream for output and a POST for each input.** Every page
  already holds one event stream; a second per Terminal page uses up
  HTTP/1.1's six connections a host, output must be base64, and POSTs
  over several connections arrive out of order. A socket is one ordered
  stream of binary frames with a way to hold back a flood.
- **An existing terminal emulator library.** It is far larger than the
  page's budget and does not build without a bundler, and its code would
  be the console's largest dependency to keep current.
- **The worker making the terminal and starting the shell.** A worker
  ends with its session, and a shell it started would skip authd's
  sign-in rules, records and audit.
- **A keeper process now.** It is a new service with its own protocol and
  its own failure cases, and the first terminals work without it. It
  comes after the console's pages.

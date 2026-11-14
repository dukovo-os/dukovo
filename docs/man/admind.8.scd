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
second steps and tokens of the person who is signed in, and the
commands of the actions that run as the person, are done by a worker,
*admind --worker*, which authd starts for each session as that person,
without the rights of an administrator. The worker takes no connection
of its own: it answers admind over the descriptor authd gives it.

What the console does is a list of actions (ADR 0515): each a read or a
change, with a route of its own under _/api/1_ and the command it runs.
A page calls them with the browser's session, and a script with an API
token (API TOKENS, below). The command an action runs is the command the
console shows, so the button, the API call and the command typed at a
terminal do the same; *admin*(8) lists the actions and prints the
command, the API call and the Ansible task of each.

admind also keeps a journal of what changed on the machine, "What changed",
and runs the actions that take longer than a request waits as tasks that go
on without the browser that asked (ADR 0516). Both are in a store of its
own, one SQLite file. A person's preferences are their own file. The store,
the journal, the tasks, the preferences and the stream of events that tells
a page of them have a section each below.

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
answers it:

. Its method is *GET*, *HEAD* or *POST* (405 for another, ADM-117), and
  its address is at most 2 KiB (400, ADM-108), before anything looks at
  what it asks for.
. Its *Host* is this machine's name, an address of it or a name of its
  certificate, with any port, so that no other name's DNS can point a
  browser here (421, ADM-114).
. It brings an API token or a session's cookie, never both, and a token
  only as one *Authorization: Bearer* header; a bearer that is not shaped
  like a token, a second one, or a token with a cookie is refused (400,
  ADM-108). Any other scheme there, such as the *Basic* of a proxy in
  front of the console, is let be.
. Its address is a route's, an action's, or a file's. An address of the
  API that is neither is 404 (ADM-116), and one that takes another method
  is 405 (ADM-117).
. A token is for the actions, the list of them, the showing of one and
  the tasks, and for no other route (403, ADM-124), and authd must take
  it: one it does not is 401 (ADM-115). An address that gave 10 wrong
  tokens in a minute is stopped before authd is asked (429, ADM-105).
. The route's access. A route for the signed in needs a session, and an
  action's route a session or a token (401, ADM-107, with the cookie
  cleared once the session has ended). A route that is not for a session
  that may only add a second step needs one that may do more (403,
  ADM-118).
. A request that changes anything with a session comes from the
  console's own page, by *Origin*, *Sec-Fetch-Site* and the session's
  *X-Dukovo-CSRF* token (403, ADM-109). A request with a token needs
  none of them, since a browser sends no token of its own accord. The
  routes that sign a browser in keep the first two, and count each
  request against the ten an address may make in a minute.
. Its body is JSON (415, ADM-119), at most 64 KiB before sign-in and 1 MiB
  after (413, ADM-113).
. For an action, its arguments are checked (400, ADM-108 for what the
  action does not take, ADM-125 for a value that cannot work), then
  whether the caller may ask for it (ADM-124, ADM-122, ADM-106), and only
  then is its command run. For the showing of an action, which runs
  nothing, the arguments are checked the same way, and a token needs the
  scope that reads the action's area.

The sign-in routes take at most ten requests a minute from one address;
the next is answered 429 (ADM-105) with *Retry-After*, on top of the wait
authd puts on a wrong password.

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

## Actions

An action's route is *GET* for a read and *POST* for a change, at an
address under _/api/1_ that the list gives, such as
*POST /api/1/services/{name}/restart*. The services' and the certificates'
actions are listed by *admin actions*, and the console's pages add their
areas'. Besides the routes of the actions there are two:

*GET /api/1/actions*
	The list: a JSON array with an object for each action, with its *id*,
	*title*, *area*, *changes*, *method*, *path*, *runs*, *long*,
	*compares*, *errors*, *tests*, *input* and *examples*. *compares* is
	true for a change whose *changed* is found by comparing what it moves
	before and after it, and false for any other action. *input* is the JSON
	Schema of the action's arguments. *admin actions --json* prints the
	same. A session or a token of any scope may ask.

*POST /api/1/show*
	Takes *{"action": "service.restart", "args": {"name": "nginx"}}* and
	runs nothing. It answers *command*, the command the action runs as
	one line for Dukovo's shell and for sh, with *elevate* in front when it
	runs as an administrator; *api*, a *curl* line for the same action
	against the address the browser used; *ansible*, the YAML of one task;
	and *files*, the paths a change writes. Any signed-in session may ask,
	locked or not, and so may a token whose scopes read the action's
	area. *admin show* prints the same.

An action's arguments are the parts of its address, such as _name_ above,
and the members of the JSON object in its body, which may be empty,
together. A name in both, a member the action does not have, or a body
that is not an object is ADM-108; a value that cannot work, such as a
service's name that starts with *-*, is ADM-125, with the *field* it is
in, before anything is run. An action's *input* in the list says the same
as a JSON Schema (draft 2020-12): an object with a property for each
argument, with its *type* and its limits (*minLength*, *maxLength*,
*pattern*, *enum*, *minimum* or *maximum*), a list of the *required*
ones, and *additionalProperties* set to false. A script or a page can
check what it is about to send against it.

A read runs its command in the worker of the person or the token's owner,
as that person, and answers the JSON the command printed with *--json*,
as it printed it. A change that runs as the person runs in the worker
too. A change that runs as an administrator goes through *authd*(8), for
an unlocked session of an administrator, or for a token whose owner is an
administrator. A change answers *{"done": true, "changed": true,
"output": [...]}* with the first 200 lines the command printed.
*changed* says whether the change moved anything, so that asking for
what is so already answers *false*. For an action whose *compares* is
true, admind runs a read of what the change moves, in the worker of the
person or the token's owner and so as that person, before the command
and again after it, and *changed* is whether the part of the two outputs
that the action picks differs; the services' start, stop, enable and
disable read *service list --json*, and the console's reach reads *admin
reach*. An action that writes files, as enable and disable write the
service's file, also has the journal read each before and after, and
changed nothing only when each reads the same: a service's file, not
Spark's copy of it, says whether it starts at boot. *changed* is *true*
when admind cannot tell: the read failed, the action finds nothing in
what it printed, or a file cannot be read. A change whose action does
not compare, such as a restart or *cert renew*, which makes the
certificate anew whether or not it is due, always answers *true*. A
command runs 30 seconds at most (ADM-121 for one that has not ended; an
action marked *long* is a task, below), a worker runs 8 at once, and admind
runs 32 at once as an administrator, for all sessions and tokens together,
the commands of tasks among them (ADM-105 for the next short command; a
task waits for a place instead). A command
that ends with a status other than 0 is ADM-123 (422), except a read
that answers with other statuses too: *service explain*, which ends
with 3 for a service that does not run, is not ready or fails its
health check, answers with its explanation after 0 and 3 alike, and its
Ansible task fails only after other statuses. For ADM-123, the *detail* is
the last line it wrote as an error, the *command* member has its words,
its status and its error output, and when that line ends with an error
ID in parentheses that the action lists, such as *(SVC-104)*, the
answer's *id* is that ID.

A person who is not an administrator is ADM-122 for a change that runs as
an administrator, and a session that is locked is ADM-106: *Unlock* in the
page, then try again.

The person's own tokens are made and removed by a signed-in session, not
by a token:

*POST /api/1/account/tokens/add*
	Takes *{"proof", "name", "scopes", "days"}*, the proof being the
	password, or empty while the session is unlocked, and answers
	*{"token"}*, which is shown once.

*POST /api/1/account/tokens/remove*
	Takes *{"name"}* and removes that token.

*admin token add* and *admin token remove* (*admin*(8)) do the same from a
terminal.

## API tokens

A script gives a token as *Authorization: Bearer dkv_...*, in one header,
and nowhere else: a token in an address or a cookie is not read, and a
token with a session's cookie is refused (ADM-108). A token may ask for
the actions, their list, the showing of one and the tasks (Tasks, below).
For any other route, such as sign-in, unlock, the account's own or the
journal, the answer is ADM-124.

admind holds no token: it passes each one to *authd*(8) with each request,
and authd answers with the owner, the token's name and its scopes, or
refuses (ADM-115). A token that is removed, that has ended, or whose
account may no longer sign in to the console stops working at once.
authd also waits longer after each wrong token, as after a wrong
password, and admind stops an address that has given 10 wrong tokens in a
minute: ADM-105 and *Retry-After*, without asking authd, until the minute
is over. The right token from that address waits too.

A scope is *\**, all that the owner may do; the name of an area, such as
*services*, for reading and changing what it holds; or the name and
*:read*, for reading it alone. admind checks them before it runs the
action: a read needs the area, *area:read* or *\**, and a change needs the
area or *\**, else ADM-124, naming what would do. A change that runs as an
administrator is also checked by authd, which is told the area, and
needs an owner who is an administrator; one that is not is refused in
authd's words (ADM-110). A token is never more than its owner.

For a read, or a change that runs as the person, a token's actions run in
a worker of its owner, which authd starts at the token's first such call.
admind keeps it for 5 minutes after the token's last call, one for each
token, and for at most 32 tokens at once (ADM-120 for another until one
goes idle). A change that runs as an administrator needs none. The
console's reach still applies: a connection from an address it does not
answer is closed, with a token or without.

## The store

admind keeps what the console must not lose with a restart in one SQLite
file, _/system/state/admind/store.db_ (ADR 0516): the journal of what
changed, the versions of the files the journal names, and the tasks. The
file is mode 0600, in a folder that is 0700. It uses SQLite's rollback
journal and not a write-ahead log, so that *admin put-back* (*admin*(8)),
which reads the file as root in a process of its own, needs nothing but the
file. admind is the only program that writes it. The file's version is
SQLite's *user\_version*: admind brings an older file up to date when it
starts, and refuses a file that a newer admind made.

When the file cannot be opened, admind says why in the log and goes on:
people sign in and the actions run, but no change has a row, an action that
runs as a task is not started, and the routes of the journal and of the
tasks answer ADM-126 (503). The store keeps at most:

- the newest 10,000 changes, the oldest going first;
- of each file, the text of its last 20 versions and, of all files, 64 MiB
  of text, the oldest going first; a file longer than 1 MiB has its hash and
  size kept and no text;
- each task for 30 days after it ends.

## What changed

The journal says what changed on the machine and who changed it. Any
signed-in session reads it, locked or not, since it names people and files
and never the contents of a secret; only a session that may just add a
second step does not (ADM-118). A token does not read it (ADM-124).

A change made through the console has a row when its command has ended,
worked or not: the Unix time, who (the person, or the owner of the token),
how (*console*, or *token* and the token's name), the action and the command
it ran, the command's exit status, the task it ran as, and each file the
action names, with what it was before the command and after it, by SHA-256.
The files an action names are those *POST /api/1/show* gives as *files*.
The text of each side is kept as a version, so that a page shows a diff and
the change can be put back. A command that admind could not run (ADM-121),
or whose worker did not answer (ADM-112), has a row with no status and a
note that says why, since it may have changed what it names. A change that
was refused or not started, as when the console is locked, the account is
not an administrator, a token's scopes do not cover it or authd did not
answer, changed nothing and has no row: the log says it. A read has none.

A change made by hand has a row too. Every 30 seconds admind compares the
size and modification time of each file under _/system/settings_ (8 folders
deep) and in each folder an action's files are in (4 deep), at most 10,000
files, with what they were at its last look, and reads and hashes only
those that differ. A file made, changed or removed while no console change
is under way on it has a row whose *how* is *by hand*, one for each person
found and one for the rest. The person is the one whose elevated command
named the file's path, as a whole word, in the 10 minutes before the file
changed, by the audit log (*log --audit*), which only root reads and which
admind asks logd for; if more than one did, the last. A change that no such
command names is *outside the console*: an edit by *service* _name_ *edit*,
which names the service and not the path, an editor run as root at the
machine's own console, or a file copied in. Names that begin with a dot,
which an editor's work file has on its way to its name, and links are not
looked at. A file changed and changed back between two looks is not seen,
and two changes between two looks are one row. The first look at a store
that holds no file only notes the files, and makes no row.

A file under a service's secrets (_/system/state/spark/secrets_), a file
that not everyone may read (no read bit for others), and a file in a folder
that others may not pass through is listed by name, with no hash, text or
diff. admind runs as root, so it finds each file from the root a part at a
time and never through a link, opens it only once it knows it is a regular
file, and does not watch links: a link that a service's account puts in its
own folder leads it nowhere. A file longer than 1 MiB is listed with its
hash and size and no text.

*GET /api/1/changes*
	The changes made at Unix time *since* or later (all, when it is not
	given), newest first, at most *limit* of them (50 when it is not given,
	at most 200). A value that is not a number in range is ADM-125 with
	the *field*. The answer is *{"changes": [...]}*. Each change has *id*,
	*at*, *who* (the person, or *outside the console*), *how* (*console*,
	*token NAME* or *by hand*), *action* and *title* (null for a hand edit
	and for an action the console no longer has), *argv* (the command),
	*status*, *task*, *note*, *files* and *seen*. Each of *files* has its
	*path* and what the file was *before* and *after*: an object whose
	*state* is *absent*, *text* or *large*, which have *sha256* and *size*,
	*secret* or *unreadable*. *seen* is null, or *{"by", "at"}* for a hand
	edit that someone has kept.

*GET /api/1/changes/{id}*
	One change as above, with a unified diff for each file that it
	changed, as *diff* (and *diff\_truncated*, true when it stops short),
	or the reason there is none as *no\_diff*: the file is as it was, it is
	a secret, admind could not read it, it is longer than 1 MiB, its text is
	not kept any more or is not text, or the change's diffs are 2,000 lines
	already, the most that its answer holds. It also has *put\_back*, which
	is *{"possible": true, "command": "elevate admin put-back 41"}*, or has
	*possible* false and *why*. A change that is not there is ADM-127 (404).

*POST /api/1/changes/{id}/keep*
	Marks a hand edit seen by the person, and answers the change. It is
	the person's own say that they have looked, and it changes nothing on
	the machine, so it asks for no more than a session that is unlocked
	(ADM-106 while it is locked), which only an administrator's can be. A
	change that someone kept stays as they marked it. A change made through
	the console is kept already: ADM-128 (409).

*POST /api/1/changes/{id}/put-back*
	Writes back the earlier text of each file the change changed. It runs
	*admin put-back* _id_ as an administrator through authd (*admin*(8)), so
	that the command the answer shows is the command that ran. It is a
	route and not an action of the list, since its argument is a row of
	the journal. It needs an administrator's unlocked session, as an
	action that runs as an administrator does (ADM-122, ADM-106), and a
	token never may (ADM-124). The answer is *{"done": true, "output": [...],
	"command": "elevate admin put-back 41", "change": 52}*: the lines the
	command printed, the command, and the number of the put back's own row
	in the journal, which can be put back in turn. A change
	that cannot be put back is ADM-128 before anyone is asked, with the
	reason: a file it made has no earlier text, and a secret, a file not
	everyone may read, a file admind could not read, a file longer than
	1 MiB and a text that the store has forgotten have none kept; a change
	that changed no file has nothing to put back. A command that does not
	work is ADM-123.

A put back of a Spark service file, one in _/system/settings/spark/services_
or its _settings.d_, writes it and says that Spark reads it when the service
next starts; *service* _name_ *restart* puts it to use now. A file of a
bundle service's data is written and followed by a request to the service to
read it again, as *service* _name_ *reload* makes. A put back of a file that
was changed again after the change says that the later text is gone.

## Tasks

An action marked *long* in the list (*GET /api/1/actions*) does not wait for
its command. Its route answers 202 with *{"task": 42}* at once, after the
checks of any action, and admind runs the command as an administrator
through authd, without the 30 seconds that a short command has, so that the
work goes on when the browser has gone and can be seen from another. Only an
action that runs as an administrator can be long. The task is kept in the
store: with none, the action is not started (ADM-126).

A task is *waiting* for one of the 32 places that elevated commands share,
in the order the tasks were asked for; *running* while its command runs;
and then *done* (the command ended with status 0), *failed* (it ended with
another status, did not start, or ran out of time) or *stopped*. At most 64
tasks wait; asking for another is ADM-105 (429) with *Retry-After*, 30
seconds.

The command's standard output is read a line at a time. A line *step N of
M: WORDS*, with N from 1 to M, says that the task is at step N of M and what
the step is (the words, at most 200 characters), and clears the bar; a line
*progress P%*, with P from 0 to 100, says how far the step has come. Every
other line, and each line of the error output, is the
task's log, of which the last 2,000 lines, at most 256 KiB, are kept, each
cut at 1,024 bytes. A task that is *done* has *changed* too, as a short
change does, from its files alone: false only when its action names files
and each reads the same after the command as before it, and true when it
names none, since a task runs no read before and after. It is null until
the task is done, and for a task that did not end well.

A task may run 12 hours. Its person, or an administrator, may stop it. A
stop sends the command SIGTERM, as ^C does, which asks it to put back what it
did, and a task that still waits just leaves the queue. A command that has
not ended 30 seconds later is hung up, and one that has not ended 10
seconds after that is let go. A task whose command was sent SIGTERM to stop
it is *stopped*, whatever status the command ended with. A command that is
still running after 12 hours is sent SIGTERM and hung up in the same way,
and its task is *failed*. When admind starts, it ends every task that
the last admind left *waiting* or *running* as *stopped*, with the note
"the console restarted", since authd hangs up the commands of an admind
that has gone. A task is kept for 30 days after it ends, and then
forgotten. The journal keeps its change, with the task's number, from when
its command ended; a task that was stopped before it began has none.

*GET /api/1/tasks*
	The tasks that wait or run, and those that ended at Unix time *since*
	or later (in the last 24 hours when it is not given), newest first, at
	most *limit* of them (50 when it is not given, at most 200). The answer
	is *{"tasks": [...]}*. Each task has *id*, *action*, *title*, *args*,
	*argv*, *who*, *how* (*console* or *token NAME*), *started*, *state*,
	*step*, *steps*, *words*, *percent* (null while the command has said
	none), *ended*, *status*, *note* (why it is as it is, when the status
	does not say) and *changed*.

*GET /api/1/tasks/{id}*
	One task as above, with its *log*, an array of lines. A task that is
	not there is ADM-127 (404).

*POST /api/1/tasks/{id}/stop*
	Asks the task to stop, as above, and answers *{"stopping": true,
	"task": {...}}* at once; the end follows, in the task's *state* and in
	the event that tells it. It is for the task's own person, or an
	administrator (ADM-122 for another person), from a session that is locked
	or unlocked, since a stop only has the command put back what it did.
	A task that has ended is ADM-128 (409).

A session reads every task, as it reads the journal. A token reads the tasks
of the actions whose area its scopes let it read, which is how a script that
asked for a long action finds how it went; a task it may not read is not
there for it (ADM-127), and the list leaves it out. A token stops a task only
when its scopes let it change the area of the task's action and its owner is
the task's person or an administrator (ADM-124 when its scopes do not, and
for a task whose action the console no longer has, since there is no area to
check the scopes against).

## Preferences

A person's preferences are their own, not the machine's: how the console
looks to them and the time zone it shows times in.

*GET /api/1/account/preferences*
	Answers *{"theme", "text", "motion", "zone"}*.

*POST /api/1/account/preferences*
	Takes any of the four and answers all of them. A member that is not
	given stays as it was. A member that the route does not have, or of
	the wrong type, is ADM-108; a value that cannot work is ADM-125 with
	the *field*, before the person's worker is asked.

*theme* is *dark* (the default), *light*, *high-contrast* or *device*, which
follows the device's own. *text* is the size of the text in percent: 100
(the default), 112, 125 or 150. *motion* is *full* (the default) or *less*.
*zone* is a time zone of the machine's own database, such as
*Europe/Berlin*, or empty (the default) for none.

They are in _~/.config/dukovo-admin/settings.toml_, mode 0600 in a folder
that is 0700, which the person's worker reads and writes as them, through
the same editing as a settings file has: a comment the person wrote stays,
and a change that changes nothing writes nothing. The file is theirs to edit
by hand. A key that is missing or has a value that is none of the above
reads as its default, so that a typo costs a preference and never the page;
a file that is not TOML is told with its line (ADM-121) and left as it is.
Any signed-in session may read and change them, locked or not, but not a
session that may only add a second step (ADM-118), and not a token
(ADM-124). They are not in the journal.

## Events

*GET /api/1/events* is a page's stream of events, as server-sent events, for
a signed-in session. It tells:

*event: unlock*
	*{"unlock\_seconds": 600}*, the seconds left of the session's unlock, or
	null when it is locked, when the stream opens and whenever it changes.

*event: session*
	*{"ended": "idle"}*, why the session ended, one of *signed-out*,
	*expired*, *idle*, *restarted* or *worker*. The stream ends after it.

*event: task*
	A task's *id*, *action*, *state*, *step*, *steps*, *words*, *percent*,
	*ended*, *status*, *note* and *changed*, as the list has them, and none
	of its *args*, *argv* or *log*, which *GET /api/1/tasks/{id}* has. It is
	told for each change of the task's state, step, words or progress, at
	most four times a second for each task: what comes sooner waits, and
	only the latest is sent. A stream that opens is told where each task is
	that waits, runs, or ended in the last minute.

A comment line, *: keep*, comes every 25 seconds, so that nothing between
the browser and admind closes a stream that says nothing. A session with a
stream open is not idle; once its last stream has closed and a minute has
passed, an unlocked session locks itself.

## Errors

An error is *{"error": {"id", "title", "detail", "fix"}}*, with an ID that
a person can quote, *ADM-101* and on: *ADM-101* the name or the password
is wrong, *ADM-102* a code is wrong, *ADM-103* a passkey was not accepted,
*ADM-104* the account may not use the console, *ADM-105* too many tries
(sign-in requests, wrong API tokens, or elevated commands at once),
*ADM-106* the console is locked, *ADM-107* the session has ended,
*ADM-108* the request is not what the route takes, *ADM-109* it did not
come from the console's page, *ADM-110* authd refused, *ADM-111* authd did
not answer, *ADM-112* the worker did not answer, *ADM-113* too big,
*ADM-114* not this machine's name, *ADM-115* the API token is not
accepted (it is unknown, it has ended, or its account may not sign in to
the console), *ADM-116* nothing is at that address, *ADM-117* the route
takes another method, *ADM-118* the session may only add a second step,
*ADM-119* the body is not JSON, *ADM-120* the console is full, *ADM-121*
the console could not do that, *ADM-122* the account can look but not
change, *ADM-123* the command did not work, *ADM-124* the API token may
not do this, *ADM-125* a value cannot work, *ADM-126* the console keeps no
journal or tasks now, *ADM-127* it has no such change or task, *ADM-128*
that cannot be done to this change or task. *detail* says what happened
this time, in authd's own words where authd refused. ADM-125 also has the
*field* whose value cannot work, and ADM-123 the *command* with its words,
status and error output. Each ID has a page in the support center that
says what to do.

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

_/system/live/log/query_
	logd's query socket, which admind asks for the audit log when it looks
	for who changed a file by hand (What changed).

_/system/state/admind/store.db_
	The store: the journal, the versions of the files it names, and the
	tasks (The store). Mode 0600; admind is the only program that writes it,
	and *admin put-back* (*admin*(8)), as root, reads it.

_~/.config/dukovo-admin/settings.toml_
	A person's preferences (Preferences), which their worker reads and
	writes as them.

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

It has a line for each change an action was asked for, whether it ran, was
refused or failed, and for a read only when it was refused or failed. The
line says who asked, as *alice* or, for a call with a token, as *alice
(API token monitor)*, then the address, the action's id and how it ended:
*ended with status* and the command's exit status, with *, changed
nothing* after it for a change that ended well and moved nothing, or *was
not run:* and the error's ID. It has the action and not its arguments. A
read that worked has no line, and nor has a request whose arguments did
not read (ADM-108, ADM-125), which is refused before anyone is asked.
*authd*(8)'s audit log (*log --audit*) has each command run as an
administrator, with the session's first 8 characters or the token's name
and area.

A long action that was asked for has a line that says *is task* and the
task's number, such as *alice from 192.0.2.7: service.update is task 42*,
and one when the task ends, with its person, the action and its state, such
as *task 42 of alice (service.update) ended with status 0: done*. A task
stopped while it waited has *was stopped by alice before it began*. A stop
has *alice (API token monitor) from 192.0.2.7: asked task 42 to stop*, a
Keep it has *alice from 192.0.2.7 kept change 41*, and a put back has the
line of an action, *alice from 192.0.2.7: changes.put-back 41 ended with
status 0*. When it starts, admind says how many tasks the last admind left
that it ended as *stopped*, and, each day, how many tasks that were kept 30
days it forgot. When its round finds files changed by hand it says how many
changes are in the journal, such as *2 changes made by hand are in the
journal*. A store that does not open is said with the reason and *the
console keeps no journal and runs no tasks*. Other lines say what the store
or the journal could not do: *the journal could not look at the files*, *the
journal could not keep* an action's change, *the journal cannot tell who
changed a file* (logd did not answer, so the rows say *outside the
console*), and *task 42 could not be saved*.

# EXIT STATUS

admind runs until it is stopped, and SIGTERM ends it with 0. It exits
with 1 when it cannot start: when *admin.toml* or the certificate does
not read, or it cannot listen.

# SEE ALSO

*admin*(8), *admin.toml*(5), *dukovo-admin*(7), *authd*(8), *certd*(8),
*cert*(8), *firewall*(8), *service*(8), *log*(1)

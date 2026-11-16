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
the log says so once a minute for each address and reason, never once for
each try.
*reach = "any"* answers all.

It serves at most 512 connections at once and 32 from one address (an
IPv6 address counts by its /64). A client has 20 seconds from its
connection for the TLS handshake and the head of its first request, which
is longer than a browser keeps a connection it opened ahead and has not
used. A head is at most 16 KiB and 64 headers, and a longer one is answered
431 with no body, which the OpenAPI description gives for every operation;
a connection that sends no head for 60 seconds is closed.

A browser that offers HTTP/2 by ALPN gets it, and every tab's requests
and event streams share its one connection (ADR 0519): at most 64
streams at once, each head at most 16 KiB, and the connection is told to
go away once no stream has been open for 60 seconds. The host of an
HTTP/2 request is its *:authority*, and a *Host* header beside it must
name the same. The Terminal's socket is a WebSocket on a connection of
its own over HTTP/1.1, which a browser opens for it. A client that
offers no protocol speaks HTTP/1.1.

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
. A token is for the actions, the list of them, the showing and the
  checking of one and the tasks, and for no other route, the open ones
  among them (403, ADM-124), and authd must take it: one it does not is
  401 (ADM-115). An address that gave 10 wrong
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
  then is its command run. For the showing and the checking of an
  action, which run no change, the arguments are checked the same way, and
  a token needs the scope that reads the action's area.

The sign-in routes take at most ten requests a minute from one address;
the next is answered 429 (ADM-105) with *Retry-After*, on top of the wait
authd puts on a wrong password. authd counts each wrong password, code
or API token for the browser's address, and an address that keeps
failing is blocked at the firewall, as over SSH (*blocking.toml*(5)).

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

## Following the log

*GET /api/1/logs/follow*
	The Logs page's follow of the log, as server-sent events. It takes
	*service*, *kernel*, *priority* and *grep* in its query, as
	*logs.read* does and checked by the same code, and *all=true* for the
	whole log, which needs an administrator's unlocked session (ADM-122,
	ADM-106) and, as an elevated read, a request from the console's own
	page (ADM-109). authd starts one *log --output json --follow --lines
	0* for it on two pipes admind reads (*authd*(8), *follow*), as the
	person or, for the whole log, as root, so that following the log is
	one command, and one audit line, for as long as the page follows.
	The events are *command*, once, with *shown*, the command as *Show
	command* gives it; *record*, a record each, as *log* prints it;
	*skipped*, with *count*, when more than 100 records came in a second
	(the first 100 are sent); and *ended*, with *why*, before the stream
	closes: *locked* or *unlock-ended* (for the whole log), *session-ended*,
	*replaced* (the session began another follow), *time* (an hour
	passed), *slow* (the tab left 256 KiB unread), *stopped* (admind is
	stopping) or *failed*, with *error*, log's last line of errors. A
	session has one follow, admind runs 8 at once, and a ninth is ADM-134
	with *Retry-After: 30*. A refusal is a JSON answer before the stream
	begins. When admind ends a follow, it asks authd to end its process.

## Trying a change

The Networking and Firewall pages try a change (*network set* or a
*firewall* change with *--try*): Spark undoes it unless it is kept in time
(ADR 0472, ADR 0518). Before the page offers to keep it, it asks whether
the browser still reaches the console over the new settings. A firewall
rule is checked when a connection begins, and one that is open goes on, so
only a request that came by a connection opened after the change took
effect proves it.

admind keeps, for each trial, a time at or after the moment its change
took effect: when the console's action that began it answered, or, for a
trial begun at a terminal or before admind started, when admind first saw
Spark list it. A trial whose end moved between two looks was kept or undone
and begun again, and begins again for admind too.

*GET /api/1/console/fresh/{trial}*
	For a signed-in session. *trial* is *network* or *firewall* (a trial's
	name, as *spark*(8) takes it; another is ADM-108). It is answered
	*{"fresh": true, "from": "192.168.1.20", "via": {"address":
	"192.168.1.10", "port": 9090, "card": "net0"}}* when the request's
	connection was accepted after the trial began: *from* is the browser's
	address and *via* the address and port the connection came to and the
	card that holds the address (null when no card does). A connection
	accepted before is ADM-133, 409, and admind ends it once the answer is
	sent (a close on HTTP/1.1; on HTTP/2 a GOAWAY, sent before the answer
	so that the browser knows it when the answer comes), so that the
	browser's next request opens a new one. A trial Spark does not list is ADM-135,
	404, and a Spark that does not answer is ADM-121.

*GET /api/1/session* gives *from* and *via* as well, so that a page knows
the address it would take away.

## Actions

An action's route is *GET* for a read and *POST* for a change, at an
address under _/api/1_ that the list gives, such as
*POST /api/1/services/{name}/restart*. A read that takes a secret, or
answers with one, is a *POST* too, with its arguments in the body, so that
no secret is ever in an address (see *Secrets*, below). The services' and the certificates'
actions are listed by *admin actions*, and the console's pages add their
areas'. Besides the routes of the actions there are these:

*GET /api/1/actions*
	The list: a JSON array with an object for each action, with its *id*,
	*title*, *area*, *changes*, *method*, *path*, *runs*, *long*,
	*compares*, *secret\_answer*, *not\_self*, *cuts\_off*, *restarts*,
	*errors*, *tests*, *input* and *examples*. *compares* is true for a
	change whose *changed* is found by comparing what it moves before and
	after it, and false for any other action. *secret\_answer* is true for
	an action whose command prints a secret. *restarts* is true for a
	change that restarts or powers off the machine, which is refused while
	a trial runs (see *Restarting the machine*, below). *input* is the JSON Schema of the action's arguments, in which
	a secret is text marked *writeOnly*. *admin actions --json* prints the
	same. A session or a token of any scope may ask.

*POST /api/1/show*
	Takes *{"action": "service.restart", "args": {"name": "nginx"}}* and
	runs nothing. It answers *command*, the command the action runs as
	one line for Dukovo's shell and for sh, with *elevate* in front when it
	runs as an administrator; *api*, a *curl* line for the same action
	against the address the browser used; *ansible*, the YAML of one task;
	*typed*, what the command reads when it asks, such as "then the
	password, typed when asked", or null; and *files*, the paths a change
	writes. It needs no secret, and drops one it is sent: each is shown as
	*(secret)*. Any signed-in session may ask,
	locked or not, and so may a token whose scopes read the action's
	area. *admin show* prints the same.

	For the services' changes, *service.start*, *service.stop*,
	*service.restart*, *service.enable* and *service.disable*, the task is
	one of the module *dukovo.system.service* of the Ansible collection
	*dukovo.system*, with *state* set to *started*, *stopped* or
	*restarted*, and *become: true*, and with no *changed_when*, since
	the module says whether it changed something. *service.enable* sets
	*state* to *started* and *enabled* to *true*, and *service.disable*
	sets *state* to *stopped* and *enabled* to *false*. Install the
	collection from its release at github.com/dukovo-os/ansible with
	*ansible-galaxy collection install*, which needs ansible-core 2.22 or
	later. The task of every other action is *ansible.builtin.command*
	with the command's words as *argv*, with *become: true* for what runs
	as an administrator and *changed_when: false* for a read.

*POST /api/1/check*
	Takes what *show* takes and makes no change. It runs the action's
	probe, as the person or the token's owner, and reads the files the
	change names, and answers *{"changed": true, "now": "stopped"}*:
	*changed* is *false* when what the change moves is already as the
	change leaves it, *true* when it would move something, and *null* when
	admind cannot tell; *now* is the part of the probe's output that the
	action compares, or *null*. A read answers *false*; a change that has no
	probe and names no file, as a restart, answers *true*. Nothing is
	written to the journal or the log. A probe that does not answer makes
	*changed* *null*, not an error. It is refused as *show* is.

*GET /api/1/openapi.json*
	The description of the API as an OpenAPI 3.1 document, made at each
	request from the table of routes and the actions the list gives. Anyone
	may ask, with no session; a token is refused (ADM-124), as for every
	route that is not for it. Each route and each action is an operation
	named by its route or its ID, with the address's parameters, the body
	of an action's arguments (and *show*'s and *check*'s *action* and
	*args*), the security it needs, and for each status it can answer the
	error IDs that come with it. A client generator, a mock server or a
	tester such as Schemathesis reads it. The lab test *web-console/fuzz*
	sends the requests it makes from the document to a machine, signed out
	and with tokens, and fails on any answer that is a 5xx, that the
	document does not give, or that takes more than 2 seconds.

The Overview and the pages that show the machine read it with five actions
of their own. Each runs a command as the person, so any signed-in session
may ask, an administrator or not, and a token needs the read scope of its
area:

*GET /api/1/console/about* (*console.about*, area *console*)
	*admin about --json*: the machine's *host*, *addresses*, console *port*
	and certificate *fingerprint*, the *release* and whether this start is
	the other slot's *fallback*, the *machine* (*maker*, *model*), *processor*,
	*memory*, seconds *up*, the *zone*, whether the *clock* is *synchronized*,
	the *ssh\_host\_key*, the *last\_good\_start* and the person's last two
	*sign\_ins*. What the person may not read is null, never an error.

*GET /api/1/console/notices* (*console.notices*, area *console*)
	*admin notices --json*: *{"notices": [{"name", "lines", "at"}]}*, the
	notices that the consoles show at sign-in, and the failed start's.

*GET /api/1/checks/last* (*checks.last*, area *security*)
	*dukovo-check --last --json*: the last result of the health checks, as
	printed, also when a check failed or warned (status 1 or 2). Before
	any result is kept the answer is 422 (ADM-123) with the command's
	status 3. A machine that does not have *dukovo-check* answers 501
	(ADM-129).

*GET /api/1/metrics/now* (*metrics.now*, area *metrics*)
	*metrics now --json*: the latest values.

*GET /api/1/metrics/usage/{range}* (*metrics.usage*, area *metrics*)
	*metrics show --json* of the seven series of the Usage card, over
	*range*, one of *1h*, *24h*, *7d* or *1y*, with a point for each minute,
	15 minutes, 2 hours or day. Any other word is ADM-125. A machine that has
	kept no history yet answers ADM-123 with *metrics*'s own words.

The Services page's scheduled tasks and the Reliability page add seven
more, for what the health check, timerd, alertd and the crash dumps keep.
The reads of the starts run as the person, since the records of the
starts are readable by everyone, and need the read scope of their area for
a token. The other three reads run as an administrator, as above, and
*console.export* is a change that runs as the person:

*GET /api/1/timers* (*service.timers*, area *services*)
	*service list --timers --json*: an array with an object for each
	service that has a timer, with its *service*, *schedule*, *next\_ms*,
	*last\_ms*, *last\_result* and *zone*, as service(8) says it. timerd
	answers only root, so it runs as an administrator.

*GET /api/1/starts* (*reliability.starts*, area *reliability*)
	*changes --list --json*: an array with an object for each start kept,
	newest first: its *start* number, *at\_ms*, *release*, *slot*,
	*result* (*good*, *failed*, *fell-back* or *bisect-try*, null for a
	start not yet finished), the *check* that failed it, *ready\_ms* and the
	*changes* it brought, as changes(1) says it.

*GET /api/1/starts/failed* (*reliability.failed*, area *reliability*)
	*changes --failed --json*: *{"failed", "since\_good", "bisect"}*, the
	newest start that failed or null, what changed since the last good
	start, and the search for the change: whether it *running*, the *try*
	it waits on and the most tries it takes, *of*.

*GET /api/1/starts/{start}* (*reliability.start*, area *reliability*)
	*startup --start N --compare --json*: the steps of start _start_, a
	number from 1 to 4294967295, beside the last good start's. *startup*
	ends with status 1 for a start with no times, one that is not kept and
	one with no good start to compare with, and prints text then, not JSON:
	the answer is its output after status 0 alone, and status 1 is
	ADM-123.

*GET /api/1/dumps* (*reliability.dumps*, area *reliability*)
	*dumps list --json*, as an administrator: the crash dumps kept,
	newest first, with each one's name, time, size and the first line of
	its panic.

*GET /api/1/alerts/history* (*reliability.alerts*, area *reliability*)
	*alerts history -n 200 --json*, as an administrator: the newest 200
	changes to the alerts, an array of the lines of JSON that *alerts*
	prints.

*POST /api/1/console/export* (*console.export*, area *console*)
	*admin export DIR* with *{"dir": "/users/ann/nas-settings"}*, as the
	person, not elevated: copies the settings that the person can read into
	a Git repository in _dir_, an absolute path of at most 4096 bytes with
	no NUL, and lists the rest in _SECRETS.md_. It writes nothing outside
	_dir_ and names no file of the machine for the journal. *changed* is
	whether the repository's newest commit is another one after the
	export than before it, as *git -C dir rev-parse --verify -q HEAD*
	says it: an export of settings that did not change makes no commit
	and answers *false*. Nothing puts an export back.

The Snapshots page adds six (ADR 0461, ADR 0515), in the area *snapshots*.
A path in them is absolute, of at most 4096 bytes, with no NUL and no *.*
or *..* part, and a snapshot is named as *snap*(1) lists it. The reads and
the person's restore run as the person, whose own rights decide what they
see and may copy; the other three run as an administrator:

*GET /api/1/snapshots* (*snapshots.list*, area *snapshots*)
	*snap list [PLACE] --json*, as the person: the snapshots of the place
	_place_, such as _/users_, or the count of each place's without it. A
	place that fails is named on the command's error output and left out,
	so the answer is its output after status 1 as well as after 0.

*GET /api/1/snapshots/browse* (*snapshots.browse*, area *snapshots*)
	*snap browse PLACE SNAPSHOT [FOLDER] --json*, as the person, who sees
	what they could read when the snapshot was made. *folder* is the place
	or within it, and the place's top without it. A snapshot that is not
	there is BAK-105.

*GET /api/1/snapshots/browse/all* (*snapshots.browse-all*, area *snapshots*)
	The same command as an administrator, who sees any person's files.

*POST /api/1/snapshots/take* (*snapshots.take*, area *snapshots*)
	*snap create [--name NAME] PLACE*, as an administrator: a read-only
	snapshot of _place_. *name* is a file name that does not start with a
	dot and is not a time; its time is the name without one.

*POST /api/1/snapshots/restore* (*snapshots.restore*, area *snapshots*)
	*snap restore --from SNAPSHOT --beside LABEL PATH...*, as the person:
	copies each of *paths*, 1 to 100 paths from */* a line, back from
	*snapshot* beside the original, as *notes (label).txt*, and writes over
	nothing. *label* is 1 to 40 printable characters with no */*. A
	snapshot that is not there is BAK-105, and a file already there by the
	name of the copy is BAK-106.

*POST /api/1/snapshots/restore/all* (*snapshots.restore-all*, area *snapshots*)
	The same command as an administrator, who copies back anyone's files,
	into folders that other accounts own: *snap restore --beside* takes
	the owner's rights for the copy before it creates anything.

The Backups on that page add nine (ADR 0463, ADR 0515), in the area
*backups*. A backup place's name is 1 to 32 letters, digits, *-* and *\_*,
not starting with *-*, as *backup*(8) says, and a new one is not called
*ssh-key*, the name of an address below. The list is the person's own; the
other eight run as an administrator, since what a place holds and its
recovery key are not a person's to read, and every change is the
machine's. A name that no place has is BAK-101:

*GET /api/1/backups* (*backups.list*, area *backups*)
	*backup list --json*, as the person: the places the machine backs up
	to, each with what it last did and when one is overdue, as backup(8)
	says it.

*GET /api/1/backups/ssh-key* (*backups.ssh-key*, area *backups*)
	*backup ssh-key --json*: the machine's public key, which a server is
	told to accept.

*GET /api/1/backups/{name}* (*backups.show*, area *backups*)
	*backup show NAME --json*: what the place holds, by place.

*POST /api/1/backups/{name}/run* (*backups.run*, area *backups*)
	*backup run --progress NAME*, a task: a round to the place now. A
	round that is running is BAK-102.

*POST /api/1/backups/{name}/verify* (*backups.verify*, area *backups*)
	*backup verify --progress --restore NAME*, a task: reads every backup
	of the place through and restores a sample. BAK-102 as above.

*POST /api/1/backups/add* (*backups.add*, area *backups*)
	*backup add [--format F] --every E --verify V NAME TO*: adds a place
	at _to_, a folder from */* or an *ssh://* address. *every* is *day*,
	*week* or *hand*, *verify* is *never*, *week* or *month*, and *format*
	is *btrfs* or *restic*. It makes the place's recovery key and
	prints it once, so it has *secret\_answer* set. *compares* is true: it
	reads *backup list --json* before and after, for the place's *to*,
	*format*, *every* and *verify*.

*POST /api/1/backups/{name}/remove* (*backups.remove*, area *backups*)
	*backup remove NAME*: takes the place away. The backups stay where
	they are, and the recovery key is the only way to read them.
	*compares* is true, as for *backups.add*.

*POST /api/1/backups/{name}/key* (*backups.key*, area *backups*)
	*backup key NAME --json*: the place's recovery key, shown to the one
	who asked and kept nowhere. It is a *POST* with *secret\_answer* set,
	though it changes nothing.

*POST /api/1/backups/{name}/restore* (*backups.restore*, area *backups*)
	*backup restore --progress --place PLACE [--at TIME] NAME*, a task:
	receives the newest backup of _place_, or that of *at*, written
	*YYYY-MM-DD-HHMMSS* as *backup show* lists it, as a read-only snapshot
	beside the place's others. It writes over nothing. BAK-102 as above.

The Move to new hardware page adds four (ADR 0464, ADR 0561), in the area
*move*. They run as an administrator, since they read every machine's
backups and write the places that the next start puts in place. *source*
is the name of a backup place, or the path of a folder, such as
_/volumes/OLD/dukovo-backup/lab-machine_, with no *.* or *..* part; the
recovery key is given as *key\_file*, the path of a file, or as *key*,
typed, and exactly one of them (ADM-125 for none or both). *key* is a
secret (see *Secrets*): the command reads it from its standard input,
and a task keeps it as *(secret)*. *at*, when given, is *YYYY-MM-DD-HHMMSS*,
and *cards* is up to 8 lines of *OLD=NEW* or *OLD=automatic*, each the
choice for a card of the old machine that *backup restore --card* takes:

*GET /api/1/move/sources* (*move.find*, area *move*)
	*backup find --json*: the machines' backups on the disks plugged in,
	each with its *source*, *machine*, *newest* time and whether its
	volume is *removable*, and the recovery keys found on them.

*POST /api/1/move/plan* (*move.plan*, area *move*)
	*backup restore --machine --plan --json*: what a restore of the
	whole machine would bring across, the accounts, apps and places, the
	space it takes (*needed*) and the space *free*, and how this
	machine's hardware differs, with a *choice* for each network card. It
	receives nothing and changes nothing, but takes a key, so it is a
	*POST*. A key that does not open the backups is BAK-103, and backups
	made by a newer Dukovo are BAK-107.

*POST /api/1/move/stage* (*move.stage*, area *move*)
	*backup restore --machine --progress*, a task: receives each place of
	the machine for the next start to put in place, as *move.plan* says
	it. A restore staged already is BAK-104, and BAK-103 and BAK-107 are
	as above. *move.cancel* undoes it.

*POST /api/1/move/cancel* (*move.cancel*, area *move*)
	*backup restore --machine --cancel*: takes what was staged away,
	before the next start has put it in place.

The Accounts page adds fourteen (ADR 0515, ADR 0561), in the area
*accounts*. An account's name is 1 to 32 lowercase letters, digits, *\_*,
*-* and *.*, starting with a letter or *\_*, and not *help*, as *user*(8)
says. The list is the person's own; the other thirteen run as an
administrator, since the last sign-ins, a person's keys, second steps,
tokens and sessions and every change are not a person's to read or make.
A name that no account has is ACC-101. *full\_name* has at most 256 bytes
and no colon or line break. A password is a secret (see *Secrets*): the
command reads it from its standard input, and it is one line of at most
1023 characters and 1024 bytes:

*GET /api/1/accounts* (*accounts.list*, area *accounts*)
	*user list --json*, as the person: the people's accounts with what
	each can do, as user(8) says it. *disabled* and *password* are null,
	since only root reads them.

*GET /api/1/accounts/list/all* (*accounts.list-all*, area *accounts*)
	*user list --json*, as an administrator: the same list, with
	*disabled* and *password*. The list's page reads it for an
	administrator who has unlocked, to mark the accounts that are
	disabled and those that sign in with a key alone.

*GET /api/1/sign-ins/last* (*accounts.last*, area *accounts*)
	*lastlog --json*: when each account last signed in, from where, and
	the wrong passwords typed for it since.

*GET /api/1/accounts/{name}* (*accounts.show*, area *accounts*)
	*user show NAME --json*: the account whole, with its keys, second
	steps, tokens and sessions at the machine, as user(8) says it.
	ACC-101.

*POST /api/1/accounts/add* (*accounts.add*, area *accounts*)
	*user add [--admin] [--full-name TEXT] NAME*: *admin* and *full\_name*
	are optional, and so is *password*: an account made without one has
	none until one is set. A name that an account has is ACC-105.
	*compares* is true: it reads *user list --json* before and after, for
	the account's row, and names _/system/settings/passwd_ and
	_/system/settings/group_ as its files.

*POST /api/1/accounts/{name}/set* (*accounts.set*, area *accounts*)
	*user set --yes NAME* with *--full-name TEXT* for *full\_name*,
	*--admin* or *--no-admin* for *admin*, and *--console* or
	*--no-console* for *console*: each is optional, and at least one is
	given. *compares* is true, as for
	*accounts.add*. ACC-101, and ACC-102 when it would leave no
	administrator.

*POST /api/1/accounts/{name}/password* (*accounts.password*, area *accounts*)
	*passwd NAME*, as an administrator, who needs no old password:
	*password* is the new one. It names no file and has no probe, so
	*changed* is always true: authd's file of passwords is not in admind's
	view. ACC-101.

*POST /api/1/accounts/{name}/disable* (*accounts.disable*, area *accounts*)
	*user disable --yes NAME*: nothing signs in to the account, and what
	it runs ends. ACC-101 and ACC-102.

*POST /api/1/accounts/{name}/enable* (*accounts.enable*, area *accounts*)
	*user enable NAME*: the account signs in again, with its password.
	ACC-101.

*POST /api/1/accounts/{name}/remove* (*accounts.remove*, area *accounts*)
	*user remove --keep-home | --archive-home | --delete-home --yes NAME*:
	*home* is *keep*, *archive* or *delete*, and has no default.
	*compares* is true, as for *accounts.add*. ACC-101 and ACC-102.

*POST /api/1/accounts/{name}/keys/add* (*accounts.key-add*, area *accounts*)
	*user key add NAME LINE*: *line* is an SSH public key, one line of at
	most 16 KiB that the parser *authd*(8) reads keys with takes, and has no
	options. ACC-103 for any other. It names no file and has no probe,
	so *changed* is always true: admind sees no home folder, and a
	probe runs as the person, who may not read another account's keys.

*POST /api/1/accounts/{name}/keys/remove* (*accounts.key-remove*, area *accounts*)
	*user key remove NAME FINGERPRINT*: *fingerprint* is *SHA256:* and 43
	characters, as *user show* gives it. ACC-104 when none of the
	account's keys has it. *changed* is always true, as for
	*accounts.key-add*.

*POST /api/1/accounts/{name}/tokens/remove* (*accounts.token-revoke*, area *accounts*)
	*user token remove NAME TOKEN*: *token* is a name of 1 to 32 of *a*
	to *z*, *0* to *9* and *-*. ACC-104 when the account has no such token.
	*authd*(8) keeps the state, so it names no file.

*POST /api/1/accounts/{name}/second-step/reset* (*accounts.steps-reset*, area *accounts*)
	*user steps reset NAME*: takes the account's authenticator app,
	passkeys, recovery codes and trusted browsers away. It names no file.

*POST /api/1/accounts/{name}/end* (*accounts.end*, area *accounts*)
	*user end NAME SESSION*: *session* is as *user show* gives it, the
	number of the session's leader and the second it began with a dash
	between, as *4123-1763110000*. ACC-104 when the account has no such
	session.

Your account adds five, in the area *account*. Each runs as the person,
with the person's own rights, and none takes the name of an account: the
commands take the caller's own when none is given. The two passwords of a
change are secrets, read as two lines of the command's standard input, the
current one first, and are at most 511 characters each:

*GET /api/1/account/show* (*account.show*, area *account*)
	*user show --json*, as the person: their own account whole.

*POST /api/1/account/name* (*account.name*, area *account*)
	*user set --full-name TEXT*: *full\_name* may be empty, which clears
	it. *compares* is true: it names _/system/settings/passwd_ and reads
	*user show --json* before and after, for *account*.

*POST /api/1/account/password* (*account.password*, area *account*)
	*passwd*, with *current* and *new*. A wrong *current* is authd's
	refusal, ADM-123 with its words, after its wait. It names no file, as
	*accounts.password*, so *changed* is always true.

*POST /api/1/account/keys/add* (*account.key-add*, area *account*)
	*user key add LINE*, as for *accounts.key-add*. ACC-103. It reads
	*user show --json* before and after, for *keys*, and names no file,
	since the file's folder is the caller's name, which the action does
	not know.

*POST /api/1/account/keys/remove* (*account.key-remove*, area *account*)
	*user key remove FINGERPRINT*, as for *accounts.key-remove*. ACC-104.

The Security page adds seven, in the area *security*, with *checks.last*.
The sign-in rules are the person's to read, as *admin.toml* is readable by
every account; the others run as an administrator, as above. Whom the
console answers is the area *console*'s, below. An address or a network is
read as *firewall*(8) reads it, and the refusal names the field *address*
or *network* (ADM-125):

*POST /api/1/checks/run* (*checks.run*, area *security*)
	*dukovo-check --json*, as an administrator: runs the health checks now,
	the ones only root can run too, and keeps the result, which
	*checks.last* then gives. It is a *POST* because it writes the kept
	result. Its answer is the result as printed, and status 1 or 2, a
	failed or a warned check, is an answer too, as for *checks.last*;
	status 3 is ADM-123. It names no file, has no undo and compares
	nothing, so *changed* is always true. The console never runs it on a
	timer.

*GET /api/1/security/sign-in* (*security.sign-in*, area *security*)
	*admin settings --json*, as the person: the sign-in rules, the
	console's *listen* port, *reach* and *certificate*, and the terminals'
	rules, as *admin*(8) says it, with *wrong* the reason _admin.toml_
	does not read.

*POST /api/1/security/sign-in* (*security.sign-in-set*, area *security*)
	*admin sign-in [--second-step W] [--session-hours N] [--idle-minutes
	N]*: sets the rules given, in _admin.toml_, and has admind read them at
	once. *second\_step* is *if-set*, *administrators* or *everyone*,
	*session\_hours* is 1 to 168 and *idle\_minutes* 5 to 1440; one of them
	at least is needed (ADM-125 with the field *second\_step* for none).
	*compares* is true: it reads *admin settings --json* before and after,
	for *sign\_in*. While a change of the console's reach is tried, it is
	ADM-138.

*GET /api/1/security/blocked* (*security.blocked*, area *security*)
	*firewall blocked --json*, as an administrator: the addresses that
	wrong passwords have blocked, with the settings of the blocks, as
	*firewall*(8) says it.

*POST /api/1/security/unblock* (*security.unblock*, area *security*)
	*firewall unblock ADDRESS*: lifts a block, so that the address is
	let in at once. *address* is an IPv4 address, or an IPv6 address or its
	network of 64 bits. The blocks are in a file only authd opens, which
	is not in admind's view, so it names no file and *changed* is always
	true. Nothing puts a block back.

*POST /api/1/security/never-block* (*security.never-block*, area *security*)
	*firewall never-block NETWORK*: keeps *network*, an address or a
	network as *192.0.2.0/24*, out of the blocks. It names
	_/system/settings/blocking.toml_, and *security.never-block-remove* undoes it.

*POST /api/1/security/never-block/remove* (*security.never-block-remove*, area *security*)
	*firewall never-block remove NETWORK*: lets the blocks count the
	network again. *security.never-block* undoes it.

The SSH page adds six (area *ssh*), of which the first is the person's read
and the rest run as an administrator, as *ssh-server*(8) does. A change
may be tried, as the Networking and Firewall pages try theirs: *try*, from
10 to 3600 seconds, ends the command with *--try SECONDS*, and Spark
undoes it unless it is kept (the trial *ssh*). While it is tried, a change
is NET-182:

*GET /api/1/ssh* (*ssh.show*, area *ssh*)
	*ssh-server show --json*, as the person: whether sshd is on and
	runs, its port, and what its file says of passwords, keys, who may
	sign in, forwarding and its mistakes.

*POST /api/1/ssh* (*ssh.set*, area *ssh*)
	*ssh-server set [passwords yes|no] [keys yes|no] [allow-users NAME...] [allow-groups NAME...] [--try S]*,
	with *passwords* and *keys* as *yes* or *no*, and *allow\_users* and
	*allow\_groups* as *everyone* or up to 32 names with a space between
	them, each by the rule of an account's name (*user*(8)). One at
	least is needed, and *passwords* and *keys* may
	not both be *no*, which would leave nobody to sign in (ADM-125 with the
	field). A file with a mistake in it is NET-183. *compares* is true: it
	reads *ssh-server show --json* before and after, for the settings of
	the file.

*POST /api/1/ssh/on* (*ssh.on*, area *ssh*)
	*ssh-server on*: turns SSH on, as *service sshd enable* does. It names
	sshd's service file and compares *on* in *ssh-server show --json*.
	*ssh.off* undoes it.

*POST /api/1/ssh/off* (*ssh.off*, area *ssh*)
	*ssh-server off [--try S]*: turns SSH off, at once or tried.
	*ssh.on* undoes it.

*POST /api/1/ssh/trial/keep* (*ssh.keep*, area *ssh*)
	*ssh-server keep*: keeps the change that is being tried. With none
	it is NET-179.

*POST /api/1/ssh/trial/undo* (*ssh.undo*, area *ssh*)
	*ssh-server undo*: undoes the change that is being tried. NET-179 as
	above.

The console's reach (*console.reach*) takes *try* too, and then runs *admin
reach WORD --try SECONDS*: Spark undoes the change unless it is kept (the
trial *console*, which covers all of _admin.toml_, the sign-in rules as
well). While it is tried, *console.reach* and *security.sign-in-set* are
ADM-138.
Two actions in the area *console* keep or undo it, both as an administrator
and NET-179 with no trial running:

*POST /api/1/console/trial/keep* (*console.keep*, area *console*)
	*admin keep*: keeps the change that is being tried.

*POST /api/1/console/trial/undo* (*console.undo*, area *console*)
	*admin undo*: undoes the change that is being tried. It names
	_admin.toml_, and compares what *admin settings --json* gives of it.

The Private access page adds four, in the area *private*, which run
*wireguard*(8) as an administrator: its interface's file holds the
server's private key, and the system's socket for it is root's, so even
the read needs the area's full scope for a token. A device's name is 1 to
32 letters, digits, *-* and *\_*, not starting with *-*:

*GET /api/1/private* (*private.show*, area *private*)
	*wireguard show --json*: the interface, with its address, port,
	endpoint and public key, and each device with its name, addresses and
	what wgd says of it, as *wireguard*(8) says it. It prints public keys
	only.

*POST /api/1/private/serve* (*private.serve*, area *private*)
	*wireguard serve [--endpoint HOST] [--port P] [--address A]*: sets the
	server up, with its key, or changes only what is given, and has wgd
	read the file. *endpoint* is a host's name or an address of at most 253
	bytes, *port* 1024 to 65535, *address* an address and its length, as
	*10.8.0.1/24*. Its file, _/system/state/wireguard/wg0.conf_, holds
	the server's private key and is not in admind's view, so this action
	and the two below name no file, and their *changed* is always true.

*POST /api/1/private/devices/add* (*private.add*, area *private*)
	*wireguard add NAME --json*: adds a device, makes its key and prints
	the device's file, with its private key, once. The device reaches
	the server's address on the tunnel. It prints a secret, so it has
	*secret\_answer* set. *private.remove* undoes it. A machine without
	private access is NET-184, and a name in use NET-185.

*POST /api/1/private/devices/remove* (*private.remove*, area *private*)
	*wireguard remove NAME*: takes the device away, so that its key no
	longer reaches the machine. A device that is not there is NET-184.
	A request that came through *wg0* is refused with ADM-137 when the
	device holds the browser's address, as removing it would cut the
	browser off and no trial puts a key back: admind reads the devices
	with *wireguard show --json*, as the caller's administrator, first,
	and refuses too when that read fails or does not say.

*Restarting the machine*. The Hardware page, and the pages that offer
Restart now, restart or shut down the machine with *shutdown*(8), a
minute after the change answers or at a time of day: the minute lets the
page count down, and gives a person who clicked by mistake the time to
cancel. Spark stops admind with the rest, and the page waits for the
console to answer again.

*GET /api/1/power* (*power.status*, area *power*)
	*shutdown --show --json*: the restart or shut down that is set, if one
	is. Spark's read socket answers anyone, so it runs as the person.

*POST /api/1/power/restart* (*power.restart*, area *power*)
	*shutdown -r +1 --no-wall*, or *shutdown -r HH:MM --no-wall* with
	*at*, a time of day from 00:00 to 23:59 by the machine's clock.
	*power.cancel* undoes it.

*POST /api/1/power/shutdown* (*power.shutdown*, area *power*)
	*shutdown -P +1 --no-wall*: turns the machine off in a minute. Only
	its power button turns it on again. *power.cancel* undoes it.

*POST /api/1/power/cancel* (*power.cancel*, area *power*)
	*shutdown -c --no-wall*: cancels what is set. With nothing set it
	changes nothing, and says so.

A change with *restarts* set is refused before it runs while a trial of
settings runs (ADM-142), which the restart would undo half way: keep or
undo the trial first. A restart or a shut down is refused while the search
for a bad change runs (UPD-106), whose tries restart the machine on their
own; the search is refused while a restart or a shut down is set. admind
reads Spark's read socket for the trials and what is set, and *changes
bisect --status --json*, as the caller's administrator, for the search.
When either does not answer, the change is refused all the same: the
guard fails closed.

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
as it printed it. A read whose command only root may run, such as the
whole log, what the firewall dropped or the timers, runs as an
administrator through *authd*(8), as a change does (ADR 0518, which
changed ADR 0515's rule that a read always runs as the person): for an
administrator's unlocked session (ADM-106 when it is locked, ADM-122 for
anyone else), or a token whose scopes name its area in full, never
*area:read* (ADM-124). A request of a session for one must come from the
console's own pages: a *Sec-Fetch-Site* header other than *same-origin*
is ADM-109. Each run is a line of the audit log, so the pages never run
one on a timer. A read's arguments come from its address's query as well
as its path: */api/1/logs?service=nginx&lines=200*; a number is read as
one, a flag as *true* or *false*, and a name the action does not take is
ADM-108. A read whose command prints a JSON value a line, as *log -o
json* does, answers an array of the lines' values.

A change that runs as the person runs in the worker too. A change that runs as an administrator goes through *authd*(8), for
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
its status and its error output (*argv*, *status* and *stderr*), and when
that line ends with an error ID in parentheses that the action lists,
such as *(SVC-104)*, the answer's *id* is that ID.

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

## Secrets

An action that takes a password or a key takes it as a secret argument
(ADR 0561): one line of text, at most as long as its *maxLength*, with no
NUL, marked *writeOnly* in the action's *input*. It is checked before
anything else reads it (ADM-108 for one that is not text, ADM-125 for one
that is empty, too long or not one line), and a refusal names the field,
never what it holds. The secret is never one of the command's words: the
command reads the action's secrets on its standard input, a line each in
the order of the fields, and then the input's end, as from a script.
An administrator's command gets that input from *authd*(8) as its first
descriptor; a person's command gets it from their worker. The input is at
most 4 KiB.

An action that takes a document, such as a certificate's key and chain,
takes it as text of many lines, at most as many bytes as its
*maxLength*, with no NUL (ADR 0567); a document that is a secret is
marked *writeOnly* and kept as a secret is. The command reads it on its
standard input as it is, after the action's secret lines, in the order of
the fields. Such an action's input may be more than a pipe holds, up to
128 KiB: admind, or the worker, writes it while the command runs, so a
command that prints before it has read all of it never waits, and ends
the writer, whose copy is overwritten with zeros, before the action
answers. A command that ends before it has read all of its input is no
failure of the action: its status says what it did.

Nothing admind keeps holds a secret. The journal and the audit log have
the command's words, a task keeps its arguments with *(secret)* for each
secret and drops the secret once the command has it, and *POST
/api/1/show* shows each as *(secret)*. admind overwrites its own copy with
zeros when it is done with it.

An action whose command prints a secret, such as a new recovery key, has
*secret\_answer* set in the list. It is a *POST*, never a task, and its
Ansible task has *no\_log: true* and registers the answer as *answer*.
Every answer of the API is *Cache-Control: no-store*.

The *curl* line of *POST /api/1/show* takes each secret from a shell
variable named after its field in capitals, such as *$PASSWORD*, inside a
body in double quotes: a secret with a *"* or a *\\* in it needs the JSON
escape in the variable (*\\"* or *\\\\*). Its Ansible task takes each from
the playbook's variable of the field's name on *stdin*, with *no\_log:
true*. A document that is a secret comes from a variable named after its
field in capitals with *\_JSON* added, such as *$KEY\_JSON*, which holds
it as a JSON string, quotes and escapes included (*jq -Rs . key.pem*
makes one).

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
command names, made while one account's administrator's terminal of the
console ran (Terminals), is that account's, with the note *in an
administrator's terminal of this console*; when two accounts' ran, it is no
one's. A change that none of these names is *outside the console*: an edit
by *service* _name_ *edit*, which names the service and not the path, an
editor run as root at the machine's own console, or a file copied in. Names that begin with a dot,
which an editor's work file has on its way to its name, and links are not
looked at. A file changed and changed back between two looks is not seen,
and two changes between two looks are one row. The first look at a store
that holds no file only notes the files, and makes no row.

A change being tried that Spark undid by itself (*spark*(8), ADR 0472),
because it was not kept in time, because a service failed with it, or
because the machine started again while it ran, has a row whose *who* is
*Spark* and *how* is *undone*, with Spark's words why as its note and each
file Spark put back. admind reads Spark's read socket at the start of each
look, before it looks for hand edits, so that the file's change back is the
undo's and not a hand edit. An undo a person asked for has the row of the
action that asked, or the hand edit of the person who ran *network undo*,
*firewall undo* or *service try* at a terminal.

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
	*at*, *who* (the person, *outside the console*, or *Spark*), *how*
	(*console*, *token NAME*, *by hand* or *undone*), *action* and *title*
	(null for a hand edit, an undo and an action the console no longer
	has), *argv* (the command),
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
looks to them, the time zone it shows times in, whether pages show the
command an action runs, and how their terminals are.

*GET /api/1/account/preferences*
	Answers *{"theme", "text", "motion", "zone", "commands", "terminal"}*,
	where *terminal* is *{"size", "scrollback", "copy-on-select",
	"keep-days", "announce", "shortcuts"}*.

*POST /api/1/account/preferences*
	Takes any of them, and any of the terminal's, and answers all of them.
	A member that is not given stays as it was. A member that the route
	does not have, or of the wrong type, is ADM-108; a value that cannot
	work is ADM-125 with the *field*, such as *terminal.size*, before the
	person's worker is asked.

*theme* is *dark* (the default), *light*, *high-contrast* or *device*, which
follows the device's own. *text* is the size of the text in percent: 100
(the default), 112, 125 or 150. *motion* is *full* (the default) or *less*.
*zone* is a time zone of the machine's own database, such as
*Europe/Berlin*, or empty (the default) for none. *commands* is *hidden*
(the default) or *shown*: whether pages show the command an action runs.

The terminal's are a *[terminal]* table in the file: *size* of the text in
pixels, 12 to 20 (14); *scrollback*, the lines the browser keeps, 1000 to
50000 (10000); *copy-on-select*, whether a selection is copied as soon as it
is made (false); *keep-days*, how many days a terminal that no page is
attached to is kept before it ends, 1 to 30 (7); *announce*, whether new
output is read aloud (false); and *shortcuts*, whether the terminal's
Alt+Shift keys are on (true).

They are in _~/.config/dukovo-admin/settings.toml_, mode 0600 in a folder
that is 0700, which the person's worker reads and writes as them, through
the same editing as a settings file has: a comment the person wrote stays,
and a change that changes nothing writes nothing. The file is theirs to edit
by hand. A key that is missing or has a value that is none of the above
reads as its default, a number out of its range included, so that a typo
costs a preference and never the page;
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

*event: change*
	*{"id": 41, "how": "console", "at": 1790000000}*, a row of the journal
	that has ended: its number, how it was made (*console*, *token* and the
	token's name, or *by hand*) and when, in seconds since 1970. It is told
	once to every stream of every signed-in session, as the journal is
	everyone's to read, and says no person, action or file. A stream that
	opens is told of the rows after it opened; the page reads the rest from
	*GET /api/1/changes*.

*event: trial*
	*{"trials": [{"id": "firewall", "left\_ms": 41000, "page": "/firewall",
	"what": "The firewall's rules"}]}*, the trials of settings that run:
	each one's ID (*network*, *firewall*, *ssh*, *console* or
	*service.NAME*), the milliseconds Spark has left before it undoes it
	unless it is kept, the page that has its banner and, in words that
	start a sentence, what is being tried. The list is every trial that
	runs, and empty when none does. It is told to every stream of every
	signed-in session, as a trial is the machine's and names no person.
	A stream that opens while a trial runs is told at once. After that it
	is told when a change begins a trial (once Spark has said how long the
	trial has) or ends it (a keep or an undo), and at each of admind's
	30-second rounds, which read Spark, so that a trial begun, kept or
	undone elsewhere is told, and the time left is corrected while a trial
	runs. The round that finds the last trial gone tells the empty list
	once. The page counts down by its own clock between two tellings.

*event: power*
	*{"shutdown": {"end": "reboot", "in\_seconds": 41, "at\_ms":
	1800000060000}}*, the restart or shut down that is set, as *shutdown
	--show --json* prints it, with the seconds left as of when it is
	sent; *{"shutdown": null}* when none is. It is told to every stream
	of every signed-in session: to one that opens while one is set, at
	once, and to all of them each time what is set changes. admind sees
	that when it reads Spark: as soon as a *power* change answers, and at
	each 30-second round, so a restart set or cancelled at a terminal is
	told too.

A comment line, *: keep*, comes every 25 seconds, so that nothing between
the browser and admind closes a stream that says nothing. A session with a
stream open is not idle; once its last stream has closed and a minute has
passed, an unlocked session locks itself.

## Terminals

The Terminal page shows shells in the browser: each terminal has a name and
one to four panes, each a shell on a terminal (_/devices/pts/N_) that admind
holds, as *sshd*(8) holds an SSH session's. *authd*(8) starts the shell: a
person's own login shell, with a sign-in record that *who*(1) and *last*(1)
show from the browser's address, or, for an administrator whose session is
unlocked, root's. A terminal is the account's, not the session's: the
person's own panes go on when the page closes and when the session ends, and
any of the account's sessions shows them. An administrator's pane is its
session's alone: another session does not see it, and it ends when the
session locks, its unlock runs out, or it ends. A terminal that no page has
shown for its *keep\_days* (7 unless asked, 1 to 30) ends, and all of them
end when admind stops or restarts.

*admin.toml*(5)'s *[terminal] allowed* says who may have one (everyone, the
administrators, or no one), and *sessions-per-account* how many each account
keeps (32 unless set). An account with no shell has none (ADM-132). The
routes are for a signed-in session that may do all that is a person's: a
token never may (ADM-124), since a terminal is a person's and a script has
SSH. Each POST needs the session's token, as every change does.

*GET /api/1/terminals*
	*{"terminals": [...], "most": 32, "sockets": {"open": 1, "most": 4}}*:
	the account's terminals, each with *id*, *name*, *made*, *used* (Unix
	times), *from* (the address it was opened from), *keep\_days*, *layout*,
	*sizes* and *panes*, each pane with *id*, *admin*, *cols*, *rows*,
	*busy* (whether a program other than the shell runs in it, or null when
	that cannot be told), *started* and *attached* (how many pages show it).
	An administrator's pane of another session is left out, and so is a
	terminal with no pane left to show.

*POST /api/1/terminals* *{"cols": 80, "rows": 24, "name": "logs",
"admin": false, "keep\_days": 7}*
	A new terminal with one pane, of 2 to 500 columns and 1 to 300 rows;
	*name* (1 to 40 characters, none a control character) is the account's
	name and the machine's unless given, or *root@* and the machine's for
	an administrator's. The answer is *{"terminal": {...}}*. An
	administrator's pane needs an administrator (ADM-122) whose session is
	unlocked (ADM-106). ADM-130 when the account has as many terminals as
	it may keep, or admind holds 256 panes.

*POST /api/1/terminals/{id}/panes* *{"cols": 80, "rows": 24, "admin":
false, "layout": "columns"}*
	One more pane, four at most (ADM-130). A terminal of one pane is laid
	out in columns unless *layout* says otherwise.

*POST /api/1/terminals/{id}/layout* *{"layout": "grid", "sizes": [0.5]}*
	The layout, one of *one*, *columns*, *rows*, *main-left* and *grid*,
	and up to three sizes between the panes, each 0.2 to 0.8.

*POST /api/1/terminals/{id}/rename* *{"name": "build"}*
	A new name.

*POST /api/1/terminals/{id}/end*, *POST /api/1/terminals/panes/{pane}/end*
	Ends the terminal, or one pane, and its shell. A terminal whose last
	pane ends ends too. *{"done": true}*.

A terminal or pane that is not there, has ended, or is another account's is
ADM-131, the same for each, so that one cannot tell them apart.

*GET /api/1/terminal* opens the WebSocket (RFC 6455) that shows the panes.
The request must ask for the protocol *dukovo-terminal-1* and carry the
session's token as another protocol, *csrf.* and the token, since a page can
set no other header on it; it must come from the console's own page, by
*Origin* and *Sec-Fetch-Site* (ADM-109), and the session may have four open
(ADM-130). The answer is 101, with no extension: nothing is compressed.
admind takes masked frames of at most 64 KiB a message, and closes the
socket with 1002 for a frame that breaks the protocol, 1007 for text that
is not UTF-8, 1008 for a message that is not one below, 1009 for one too
big, 1011 when the page does not take what is sent, 1001 as admind stops,
and 4001 when the session ends. It pings every 25 seconds and closes a
socket that has answered none for 60 (1008).

A page sends JSON text and binary messages of typing:

*{"attach": 12, "cols": 120, "rows": 40, "since": 81920}*
	Shows pane 12. Without *since*, the page is sent the last output the
	pane keeps (64 KiB), from the start of a line; with it, the output from
	that offset on, if the pane still keeps it. The pane takes the size
	asked when no other page shows it.

*{"detach": 12}*, *{"resize": 12, "cols": 120, "rows": 40}*
	Stops showing a pane; gives it a size.

*{"ack": 12, "offset": 90112}*
	How much of the pane's output the page has taken in.

Binary: the pane as 4 bytes, big-endian, then what was typed, 16 KiB at most.

admind sends:

*{"attached": 12, "terminal": 3, "name": "alice@nas", "admin": false,
"cols": 120, "rows": 40, "offset": 65536, "replay": true, "modes": "..."}*
	The pane is shown, from *offset*. A replay has *modes*: the sequences
	that put a new screen in the modes the pane's program set (the
	alternate screen, the cursor keys, the mouse, bracketed paste), to be
	read before the output. The program is then given the size again, one
	row less and back, so that a full-screen program draws itself anew.

*{"ended": 12, "why": "exited"}*
	The pane ended: *exited* (its shell ended), *ended* (it was ended),
	*locked*, *signed-out*, *idle* (the session ended), or *restart*. A
	*status*, the shell's exit status, is there only when admind knows it,
	which it does not yet: authd reaps the shell.

*{"detached": 12, "why": "slow"}*
	The page did not take the pane's output for 30 seconds.

*{"changed": 3}*
	Terminal 3's name, layout or panes changed: read the list again.

*{"error": {...}, "pane": 12}*
	What asked of pane 12 could not be done, such as ADM-131 or ADM-130
	for a seventeenth pane on one socket.

Binary: the pane as 4 bytes, the offset of the first byte as 8, both
big-endian, then the output, 16 KiB at most.

While a page has not taken 256 KiB of a pane's output, admind stops reading
the pane, so that its program waits, as on a slow line; reading goes on
once the page is within 64 KiB. A page that stays behind for 30 seconds is
detached from the pane, so that other pages go on. At most 64 KiB of typing
waits for a pane; past that, admind reads no more from that page until the
shell has taken some.

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
that cannot be done to this change or task, *ADM-129* this machine does not
have the program yet, *ADM-130* too many terminals, *ADM-131* no such
terminal, *ADM-132* the terminal is not open to this account, *ADM-133* the
question came by a connection opened before the change, *ADM-134* the
console follows as many logs as it may, *ADM-135* nothing is being tried by
that name, *ADM-136* not on your own account from the console, *ADM-137* the
browser comes through what the change takes away, *ADM-139* the session has
ended already, *ADM-140* the one-time link was used or has ended, *ADM-141*
the certificate does not name the address the browser uses, and *ADM-142* a
trial runs, which a restart would undo. *ADM-138* is given by *admin*(8),
not admind. *detail* says what happened this time, in authd's own words where
authd refused. ADM-125 also has the *field* whose value cannot work, and
ADM-123 the *command* with its words, status and error output. Each ID has a
page in the support center that says what to do.

# FILES

_/system/settings/admin.toml_
	The sign-in rules, the port, whom it answers and the certificate
	(*admin.toml*(5)).

_/system/resources/admin_
	The console's pages. Its _manifest.txt_ lists the SHA-256 of each
	file, and admind serves only the files that match. It checks each
	when it starts and reads it again, and checks it again, each time it
	sends it, keeping none in memory: a file changed since admind started
	is not served, and the log says so. A file's *ETag*
	is its hash from the manifest, in quotes, and a request whose
	*If-None-Match* names it, or is a single star, is answered 304 with
	the *ETag* and *Cache-Control: no-cache* and no body, so that a reload
	fetches only what changed. Its _help/errors_ folder holds the error pages of
	the support center, as _ID.md_ files, and _index.json_, a list of each
	one's *id*, *title* and *area*.

_/system/live/admind/issue_
	The address and the certificate's fingerprint, for the sign-in
	text of the consoles.

_/system/live/authd/socket_
	authd's socket.

_/system/live/log/query_
	logd's query socket, which admind asks for the audit log when it looks
	for who changed a file by hand (What changed).

_/system/live/spark/read_
	Spark's read socket, which answers anyone, for the trials being run
	and the ones Spark undid (Trying a change, What changed).

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

Each terminal's pane has a line as it opens, such as *terminal 3 pane 12
opened for alice as an administrator from 192.0.2.7 (pts/7)*, as a page
shows it and stops (*pane 12 attached*, *pane 12 detached*), and as it ends,
with why: *terminal 3 pane 12 (pts/7) ended: exited*. A socket has a line as
it opens, and one when it is closed because it answered no ping. Nothing
typed or shown is ever in the log. authd's audit log has each shell's start
and end.

# EXIT STATUS

admind runs until it is stopped, and SIGTERM ends it with 0. It exits
with 1 when it cannot start: when *admin.toml* or the certificate does
not read, or it cannot listen.

# SEE ALSO

*admin*(8), *admin.toml*(5), *dukovo-admin*(7), *authd*(8), *blocking.toml*(5),
*certd*(8), *cert*(8), *firewall*(8), *service*(8), *log*(1), *backup*(8),
*snap*(1)

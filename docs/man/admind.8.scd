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
. A token is for the actions, the list of them and the showing of one,
  and for no other route (403, ADM-124), and authd must take it: one it
  does not is 401 (ADM-115). An address that gave 10 wrong tokens in a
  minute is stopped before authd is asked (429, ADM-105).
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
	*title*, *area*, *changes*, *method*, *path*, *runs*, *long*, *errors*,
	*tests* and *examples*. *admin actions --json* prints the same. A
	session or a token of any scope may ask.

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
in, before anything is run.

A read runs its command in the worker of the person or the token's owner,
as that person, and answers the JSON the command printed with *--json*,
as it printed it. A change that runs as the person runs in the worker
too. A change that runs as an administrator goes through *authd*(8), for
an unlocked session of an administrator, or for a token whose owner is an
administrator. A change answers *{"done": true, "output": [...]}* with
the first 200 lines the command printed. A command runs 30 seconds at
most (ADM-121 for one that has not ended), a worker runs 8 at once, and
admind runs 32 at once as an administrator, for all sessions and tokens
together (ADM-105 for the next). A command that ends
with a status other than 0 is ADM-123 (422): the *detail* is the last line
it wrote as an error, the *command* member has its words, its status and
its error output, and when that line ends with an error ID in parentheses
that the action lists, such as *(SVC-104)*, the answer's *id* is that ID.

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
the actions, their list and the showing of one. For any other route,
such as sign-in, unlock or the account's own, the answer is ADM-124.

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
not do this, *ADM-125* a value cannot work. *detail* says what happened
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
*ended with status* and the command's exit status, or *was not run:* and
the error's ID. It has the action and not its arguments. A read that worked
has no line, and nor has a request whose arguments did not read (ADM-108,
ADM-125), which is refused before anyone is asked. *authd*(8)'s audit log
(*log --audit*) has each command run as an administrator, with the
session's first 8 characters or the token's name and area.

# EXIT STATUS

admind runs until it is stopped, and SIGTERM ends it with 0. It exits
with 1 when it cannot start: when *admin.toml* or the certificate does
not read, or it cannot listen.

# SEE ALSO

*admin*(8), *admin.toml*(5), *dukovo-admin*(7), *authd*(8), *certd*(8),
*cert*(8), *firewall*(8), *service*(8), *log*(1)

website(8) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

website - list, check, add, take to HTTPS, remove and count the websites nginx serves

# SYNOPSIS

*website* [*list*] [*--json*]++
*website check* [*--json*]++
*website add* _name_ [*--name* _dns-name_]... [*--kind* *static*|*php*] [*--folder* _folder_] *--owner* _account_ [*--certificate* _certificate_] [*--progress*]++
*website add* _name_ ... *--dry-run* [*--json*]++
*website https* _name_ *--certificate* _certificate_ [*--progress*]++
*website https* _name_ *--off* [*--progress*]++
*website remove* _name_ [*--files*] [*--progress*]++
*website count* [*--now*] [*--json*]

# DESCRIPTION

website changes which websites *nginx* serves, and tells which it does.
A website is a file, _/system/settings/websites/_name_.conf_, which
nginx's settings include, and a folder, by default
_/shared/www/sites/_name_, which holds the pages. The file is written
from a template, so that a file website wrote is told from one a person
changed or wrote (see THE FILES). nginx's own settings, which are in its
data folder, are not changed: the web server section of the handbook
(in the chapter on services) says how nginx is installed and turned on.

What a person types never reaches a file as it is typed. A name must be
a name by the rule certificates follow (*cert*(8)): lowercase letters,
digits and _-_ in labels of up to 63 characters, with a dot between
them, at most 253 characters in all; a name that is not ASCII is written
in punycode, as _xn--..._. A wildcard and an address by number are not
names a website answers to. The site's name, which is the first name,
is also its file's name, so it has at most 250 characters. A folder is
under _/shared/www/sites_ and is made of letters, digits and _-_, _.\_@+_,
each part starting with a letter, a digit or _\__. An account is named by the
rule accounts follow, and it is the account of a person.

Every change is checked before anything is changed, and a check that
fails changes nothing. nginx's own check (*nginx -t*) is run on the
new set of sites before it is live, so that settings nginx refuses never
reach a restart or a reload. The files are written whole and renamed into
place. When the change is made, each name of the site is asked for a file
website put in its folder, over this machine's own address, to see that
nginx serves what was meant; a site that does not answer is taken back.
A step that fails takes the steps before it back, last first, and the
message says what could not be taken back, if anything.

Each site has a log of its requests of its own, which nginx writes in
its data folder and which *count* counts once a day: how many requests
the site answered each day, how many of them failed and how many bytes
it sent, and how big its folder is. The log holds only the time, the
status and the size of each request, never an address, a path or an
agent, and it is kept for two days (see THE FILES and
*dukovo-privacy*(7)).

One change runs at a time. A second waits for the first, for up to a
minute.

*list*, and the *--help* of any command, may be run by anyone who can
read the files. *check*, which asks nginx, *count*, which reads nginx's
data folder, and *add*, *https* and *remove*, which change nginx and its
service file, need root: run them with *elevate*(8).

# COMMANDS

*website* [*list*] [*--json*]
	Shows one row for each website: its name, its kind (*static*, *php*
	or *other* for a file website does not read as one of those), its
	certificate when it serves HTTPS, its size (the folder as *count* last
	measured it, with a _+_ when the measure stopped short, or _-_ before
	the first count), and its folder. Under the table is a line for
	what is wrong with a site that can be seen from the files:
	its folder is not there, nginx has no copy of its certificate, PHP-FPM
	is off or not installed, or its file does not read as nginx's
	settings. With *--json*, see JSON.

*website check* [*--json*]
	Asks nginx whether it accepts the settings as they are, and whether
	_nginx.conf_ has the three parts the websites need: the line that
	includes the websites' folder, the answer 404 under _/sites/_ in
	its default server, which keeps one site's folder from being served
	by another, and the definition of the format of the sites' logs.
	When nginx refuses the file of one site, the report says which. Ends
	with status 1 when nginx's check fails or a part is missing.

*website add* _name_ ... *--owner* _account_
	Adds a website that answers to _name_ and to each *--name*, at most
	20 names. *--kind* is *static* (the default), which serves files,
	or *php*, which also hands the files that end in _.php_ to PHP-FPM
	and never runs PHP in _uploads_. *--folder* names the folder, which
	website makes, owned by *--owner*, with a starter page when it has no
	_index.html_ or _index.php_; a folder that is there is kept as it is,
	and a link is refused. A PHP site's _uploads_ folder belongs to the
	account _php_. *--certificate* serves the site over HTTPS with that
	certificate of *certd*(8), which must name every name of the site:
	port 80 then answers with a redirect to HTTPS, except for what
	*certd* needs to prove the names to an authority. website adds what
	nginx's service file needs for the certificate and for port 443, as
	it is told in THE FILES, restarts nginx when that changes and nginx
	runs, and turns on nginx, and PHP-FPM for a PHP site, when they are
	off. nginx then reads the new sites as it restarts again, since a
	socket that is new to it, as port 443's, is taken up only as nginx
	starts; otherwise it reloads. After a restart or a start, website
	waits, for up to 30 seconds, until nginx answers on port 80 before it
	goes on. *--progress* tells each step as it begins, as _step N of M:
	words_.

*website add* _name_ ... *--dry-run* [*--json*]
	Does the checks, all of them rather than the first that fails, and
	changes nothing that stays: it tells what would be checked, made,
	written and started, what the firewall says of the ports the site is
	reached on, and whether nginx accepts the new set of sites. nginx's
	check of the new set is left for later when the site needs a
	certificate nginx has no copy of yet, which nginx can see only once
	it restarts. Ends with status 1 when a check fails. *--json* prints
	the plan as JSON, see JSON. It does not go with *--progress*.

*website https* _name_ *--certificate* _certificate_
	Serves the site over HTTPS with the certificate, as *website add*
	does, and makes port 80 a redirect. The file website wrote is
	written again with the HTTPS server in it. A site whose file was
	changed by hand is not rewritten (WEB-107): add the HTTPS server to
	it yourself, as THE FILES shows. Nothing is changed when the site
	has that certificate already.

*website https* _name_ *--off*
	Serves the site over HTTP only again, and gives back what nginx's
	service file held for it when no other site needs it: the
	certificate and the sockets for port 443. The default server goes
	with the last site that is served over HTTPS, when it is as website
	wrote it.

*website remove* _name_ [*--files*]
	Removes the site's file, has nginx read its settings again, and gives
	back what nginx's service file held for it. The new set of sites
	is checked by nginx first, so that a hand edit of another site that
	nginx refuses is told before anything is changed. The site's folder
	is kept, and its name told, unless *--files* is given, which removes
	it and all that is in it, last, once all else is done, and only when
	no other site serves it too. The site's log files go with it. The
	certificate is kept: *cert remove*, with *elevate*, removes it.

*website count* [*--now*] [*--json*]
	Counts, for each site, the requests its log holds of yesterday (a day
	by UTC, which is how the log's times are kept): the visits, which are
	the requests answered with a status from 200 to 399, the errors, from
	400 to 599, and the bytes sent for both. It also measures the size of
	each site's folder, as *bundle*(1) measures what an app takes: in a
	process of its own, at the lowest priority for the disk and the
	processor, and with the same limits. The numbers are kept (see THE
	FILES) and *list* shows them. A day counted again is counted anew.
	*--now* counts today so far, from the file nginx is writing. The
	daily run lowers its own priority for the disk and the processor as
	well, which *--now*, that a person waits for, does not. A site that
	has no log file, which is a site added before the sites were logged,
	has nothing counted, and the line says it has no log of its
	requests, until *https* writes its file again.

	The log of a site is its file and the two copies *logturn*(8) keeps,
	_NAME.log_, _NAME.log.1_ and _NAME.log.2.zst_. *logturn* turns a file
	over when a day has passed since it last did, not at midnight, so
	the lines of one day can be in all three: each line is counted for the
	day its own time says. The files are nginx's, so they are opened
	without following a link, and a line that is not a request as the
	format writes it, or is cut short, is passed over. A file is read as
	far as 2 GiB or 60 seconds go; what is counted of it is then _at
	least_, which *list* shows as *complete* _false_. A file that cannot be
	read is told, and counted as far as it was read.

	The service *website-count* (*service*(8), *timerd*(8)) runs it each
	day, a little after midnight in UTC, and a machine that was off then
	counts soon after it starts; it counts the one day before, so a
	machine that was off for more than a day counts the last day alone,
	and a person runs *count* for *--now*. It prints a line for each
	site: the day, the visits, the errors, the bytes sent and the size of
	the folder, with _at least_ before the numbers of a day or a folder
	that a limit stopped short. With *--json*, see JSON. It ends with status 0 when it counted
	what it could; what it could not read it prints, and writes to the
	system's log.

	```
	# website count
	blog.example.org: 2026-11-16: 214 visits, 9 errors, 1.2M sent; folder 12M in 340 files
	shop.example.org: no log of its requests; folder 4.0M in 61 files
	```

# THE FILES

_/system/settings/websites/_name_.conf_
	The site, as website writes it: a comment that names it, then one
	server for port 80 (two servers when it is served over HTTPS: the
	first redirects, the second serves), with the names, the two
	_access\_log_ lines that have each request go to the system's log and
	to the site's own file, the folder, the index files, and these
	locations: files whose names start with a dot
	are never served, except under _.well-known_; a PHP site passes
	_.php_ to PHP-FPM on port 9000 and never under _/uploads/_; a static
	site answers 404 to _.php_. A file website did not write, or one
	someone changed, is shown by *list* as _by\_hand_, and a file it did
	not write at all as _added\_by\_hand_; a person's own file there is
	served as it is.

_/system/settings/websites/\_default.conf_
	The server that refuses the handshake of an HTTPS request for a name
	no site has. It is written with the first site served over HTTPS and
	removed with the last, when it is still the text website wrote. Any
	file here whose name starts with _\__ is not a site.

_/system/settings/spark/services/settings.d/nginx.toml_
	The override of nginx's service file, which keeps what the sites
	need and never changes the service file itself, so that a bundle's
	update keeps it. website keeps these keys: *certificates*, the
	certificates of certd that nginx is given, each site's and those a
	person put there; *permissions.files.read*, which gets the folder
	certd's copies are in when a certificate is named; and *sockets*,
	which gets a socket for port 443 on every IPv4 and IPv6 address when
	a site needs HTTPS. A key you write there is kept. A list website
	changes is written whole: what was there, and what the sites add.
	The file starts with a comment that says so. A certificate that is
	new to nginx is in its view only once nginx restarts, which is why
	website restarts nginx when this file changes and nginx runs.

_/shared/www/sites/_name_
	The site's folder, unless *--folder* names another, owned by the
	person *--owner* names.

_/shared/services/org.nginx.nginx/nginx/logs/sites/_name_.log_
	The site's log of its requests, in nginx's data folder, a line for
	each: the time in seconds with thousandths since 1970, the status and
	the bytes of the answer's body, as _1763175600.123 200 5120_. There
	is no address, no path and no agent in it. *logturn*(8) turns it over
	and keeps the last two turned copies, _name_.log.1_ and
	_name_.log.2.zst_, so a line is kept for about two days. *website
	remove* removes them. nginx's account owns the folder; *add* makes it
	when it is not there.

_/system/state/websites/counts.json_
	What *count* kept, for anyone to read: for each site its days, newest
	first and at most 400, each with the visits, the errors, the bytes and
	whether the day was counted to its end, and the size of its folder.
	What is kept of a site that is gone stays for 30 days, in case the
	site comes back. It holds no more than the log does.

_/shared/services/org.nginx.nginx/nginx/conf/nginx.conf_
	nginx's settings, which website reads and does not change. It must
	include _/system/settings/websites/\*.conf_ at the end of its _http_
	block, answer 404 under _/sites/_ in its default server, and define
	the format of the sites' logs in its _http_ block, before the
	servers: _log\_format site '$msec $status $body\_bytes\_sent';_, as the
	bundle's file does (*check* says what is missing, and the lines to
	add). nginx's check of a new set runs on a copy of this file in its
	folder, which website removes again.

# JSON

All of it has *format* 1.

*list --json* is an object with *nginx* and *php\_fpm*, each with
*installed*, *enabled* and *running*, and *websites*, an array with
for each site: *name*; *names*; *file*, its path; *kind*; *folder* and
*owner* (the account's name, or *null*); *https*; *certificate*, the
name of certd's certificate, and *certificate\_path* for an
_ssl\_certificate_ that is not certd's; *redirects\_http*; *by\_hand*;
*added\_by\_hand*; *problems*, an array of objects with *id* (*null* when
no error has one) and *words*; *text*, the file's text, left empty
for a file of more than 64 KiB; *size*; and *visits*.

*size* is *null* until *count* has measured the site, and else an object
with *bytes*, what the folder costs on disk; *files*, how many files are
in it; *complete*, which is *false* when a limit stopped the measure and
the numbers are at least; and *measured\_ms*, when, in milliseconds since
1970.

*visits* is *null* until *count* has counted a day of the site, and else
an object with *yesterday*, the visits of the day before today (0 when
that day was not counted); *last\_7\_days* and *last\_30\_days*, the visits
of that many days before today and of today when it was counted;
*complete*, which is *false* when a day in *days* was counted only as far
as the limits went; and *days*, the newest first and at most 30, each an
array of the day (_2026-11-15_), the visits, the errors and the bytes
sent.

*check --json* is an object with *nginx\_conf*, which has *path*,
*includes*, *hides\_sites* and *logs\_sites*, whether the format of the
sites' logs is defined; *check*, which has *passed* and *line*,
nginx's line that says what is wrong; and *websites*, an array of
objects with *name* and *problems*, which has the line of nginx that
names the site's file as WEB-103.

*count --json* is an object with *day*, the UTC day that was counted;
*sites*, an array with for each site its *name*, *day* (an object with
*day*, *visits*, *errors*, *bytes* and *complete*, or *null* when the site
has no log file), *size* (as in *list*) and *problems*, what could not be
read of its log, as words; and *problems*, what went wrong that is no
site's.

*add --dry-run --json* is an object with *checks*, an array of objects
with *id*, *result* (*passed*, *warning* or *failed*) and *words*;
*folder*, with *path*, *exists*, *entries* and *owner*; *files*, each
with *path*, *new* and *text*; *steps*, each with *words* and *argv*, the
command line when the step is a command and an empty array when it is
not; *restarts\_nginx*; *turns\_on*, the services it turns on; *firewall*,
for each port with *port*, *allowed*, *rule* and *text*; and
*nginx\_check*, which is *passed*, *failed* or *later*, with *nginx\_line*.

# EXIT STATUS

0 when it did what was asked; 1 when it was refused or failed, with a
message that starts with *website:* and ends with the error's ID in
parentheses when it has one; 2 for a mistake in the command line, and
for what was asked of an account or a folder that cannot be used. *check*
and *add --dry-run* end with 1 when a check failed, which they have told
on standard output.

# ERRORS

The ID ends the last line of the message, as _(WEB-103)_.

*WEB-101*
	A website answers to a name already, or a file of the site is
	there. The message names the name and the file. Choose another
	name, or remove the site first.

*WEB-102*
	nginx's settings do not hold what the websites need: the line that
	includes the websites' folder, the answer 404 under _/sites/_, or the
	definition of the format of the sites' logs. The message gives the
	lines to add and where.

*WEB-103*
	nginx's check failed, on the settings as they are or on the new set,
	or nginx's service file does not pass Spark's check, so nothing was
	changed, or what was changed was taken back. The message has nginx's
	own line, with the file it names as the site's file.

*WEB-104*
	The site did not answer after the change, or nginx did not run with
	it, so the change was taken back. The message says which port and
	name did not answer, and with what.

*WEB-105*
	nginx, or for a PHP site PHP-FPM, is not installed. Install the
	bundle: *bundle install --for-everyone org.nginx.nginx*.

*WEB-106*
	The certificate does not name every name of the site, or certd has
	none of that name. The message says which names it lacks.

*WEB-107*
	The site's file was changed by hand, so website does not write it.
	Add the HTTPS server to it yourself.

# EXAMPLES

Add a site, with a folder for the account _alice_, and look at what it
says it would do first:

```
elevate website add blog.example.org --owner alice --dry-run
elevate website add blog.example.org --name www.blog.example.org --owner alice
```

Get a certificate for it, then serve it over HTTPS:

```
elevate firewall allow tcp 80
elevate cert get blog.example.org --name blog.example.org --name www.blog.example.org --agree-terms
elevate website https blog.example.org --certificate blog.example.org
```

A PHP site over HTTPS at once, whose certificate is there:

```
elevate website add shop.example.org --kind php --owner alice --certificate shop.example.org
```

What the machine serves, and whether nginx accepts it:

```
website list
elevate website check
```

How many requests a site had today so far, and what it had yesterday:

```
elevate website count --now
website list --json
```

Take a site back to HTTP, and remove it with its folder:

```
elevate website https blog.example.org --off
elevate website remove blog.example.org --files
```

# SEE ALSO

*cert*(8), *certd*(8), *dukovo-privacy*(7), *elevate*(8), *firewall*(8),
*logturn*(8), *service.toml*(5), *service*(8), *timerd*(8), the web server
section of the handbook's chapter on services

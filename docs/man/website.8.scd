website(8) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

website - list, check, add, take to HTTPS and remove the websites nginx serves

# SYNOPSIS

*website* [*list*] [*--json*]++
*website check* [*--json*]++
*website add* _name_ [*--name* _dns-name_]... [*--kind* *static*|*php*] [*--folder* _folder_] *--owner* _account_ [*--certificate* _certificate_] [*--progress*]++
*website add* _name_ ... *--dry-run* [*--json*]++
*website https* _name_ *--certificate* _certificate_ [*--progress*]++
*website https* _name_ *--off* [*--progress*]++
*website remove* _name_ [*--files*] [*--progress*]

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

One change runs at a time. A second waits for the first, for up to a
minute.

*list*, and the *--help* of any command, may be run by anyone who can
read the files. *check* asks nginx, and *add*, *https* and *remove*
change nginx and its service file, so they need root: run them with
*elevate*(8).

# COMMANDS

*website* [*list*] [*--json*]
	Shows one row for each website: its name, its kind (*static*, *php*
	or *other* for a file website does not read as one of those), its
	certificate when it serves HTTPS, and its folder. Under the table is
	a line for what is wrong with a site that can be seen from the files:
	its folder is not there, nginx has no copy of its certificate, PHP-FPM
	is off or not installed, or its file does not read as nginx's
	settings. With *--json*, see JSON.

*website check* [*--json*]
	Asks nginx whether it accepts the settings as they are, and whether
	_nginx.conf_ has the two parts the websites need: the line that
	includes the websites' folder, and the answer 404 under _/sites/_ in
	its default server, which keeps one site's folder from being served
	by another. When nginx refuses the file of one site, the report says
	which. Ends with status 1 when nginx's check fails or a part is
	missing.

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
	no other site serves it too. The certificate is kept: *cert
	remove*, with *elevate*, removes it.

# THE FILES

_/system/settings/websites/_name_.conf_
	The site, as website writes it: a comment that names it, then one
	server for port 80 (two servers when it is served over HTTPS: the
	first redirects, the second serves), with the names, the folder, the
	index files, and these locations: files whose names start with a dot
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

_/shared/services/org.nginx.nginx/nginx/conf/nginx.conf_
	nginx's settings, which website reads and does not change. It must
	include _/system/settings/websites/\*.conf_ at the end of its _http_
	block and answer 404 under _/sites/_ in its default server, as the
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
no error has one) and *words*; and *text*, the file's text, left empty
for a file of more than 64 KiB.

*check --json* is an object with *nginx\_conf*, which has *path*,
*includes* and *hides\_sites*; *check*, which has *passed* and *line*,
nginx's line that says what is wrong; and *websites*, an array of
objects with *name* and *problems*, which has the line of nginx that
names the site's file as WEB-103.

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
	includes the websites' folder, or the answer 404 under _/sites/_.
	The message gives the lines to add and where.

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

Take a site back to HTTP, and remove it with its folder:

```
elevate website https blog.example.org --off
elevate website remove blog.example.org --files
```

# SEE ALSO

*cert*(8), *certd*(8), *elevate*(8), *firewall*(8), *service.toml*(5),
*service*(8), the web server section of the handbook's chapter on
services

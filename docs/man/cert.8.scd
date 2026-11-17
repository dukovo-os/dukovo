cert(8) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

cert - list, show, add, get, renew, revoke, remove and check the machine's certificates, and see and forget the ones that could not be got

# SYNOPSIS

*cert list* [*--json*]++
*cert show* _name_ [*--json*] [*--pem*]++
*cert add* _name_ *--chain* _file_ *--key* _file_ [*--replace*] [*--untrusted*]++
*cert add* _name_ *--from* *-* [*--replace*] [*--untrusted*]++
*cert get* _name_ [*--name* _domain_]... [*--dns*] [*--staging*] [*--dry-run*] [*--directory* _url_] [*--email* _address_] [*--agree-terms*] [*--key* *ecdsa-p256*|*ecdsa-p384*] [*--profile* _profile_] [*--yes*] [*--progress*]++
*cert get* _name_ [*--name* _domain_]... [*--dns*] [*--staging*] [*--directory* _url_] *--check* [*--json*]++
*cert history* [_name_] [*-n* _number_] [*--json*]++
*cert wanted* [*--json*]++
*cert forget* _name_++
*cert revoke* _name_ [*--reason* _reason_]++
*cert remove* _name_++
*cert renew* _name_++
*cert check*

# DESCRIPTION

cert shows and changes the certificates that *certd*(8) keeps in
_/system/state/certd/certificates_, each under a name, which Spark copies
for the services whose files name it (*service.toml*(5), ADR 0512). A
service that serves TLS, such as *nginx* or the web console, reads its
certificate and key from the copy it is given. The machine has one
certificate of its own, *this-machine*, which certd makes; the others
are ones you add, from an authority or of your own making, and ones
*cert get* gets from Let's Encrypt or another authority that speaks ACME
(RFC 8555, ADR 0513), which certd then renews.

A name is 1 to 64 of the letters _a_ to _z_, the digits and _-_ and _._,
beginning with a letter or a digit, with no two dots in a row. It is
usually the site's name, as *example.com*.

*list*, *show*, *check*, *history*, *wanted* and *get* with *--check* may
be run by anyone, as certificates are public and keys are not shown. *add*,
*get*, *remove*, *renew* and *revoke* change what services are given, and
*forget* what certd asks for, so they need root: run them with
*elevate*(8).

# COMMANDS

*cert list* [*--json*]
	Shows one row for each certificate, by name: its name, its kind
	(*self-signed* for this-machine, *added* for one that *cert add*
	took), its first two names and the number of others, the day it runs
	out with the days left, or *ran out* and the day, in the error's
	colour when the terminal has colour, and the services whose files
	name it. A certificate from an authority (*acme*) also shows the day
	certd will renew it, as _renews_ and the date, once certd has chosen
	it (at its next look: *certd*(8)), and _revoked_ in the
	error's colour once it is revoked. With *--json*, an array of objects
	with *name*, *kind*, *names*, *not\_before* and *not\_after* (RFC 3339,
	UTC), *days\_left*, *fingerprint\_sha256* (lowercase hexadecimal, no
	colons), *issuer*, *trusted* (whether the system's authorities vouch
	for it), *used\_by* and *account*, and for a certificate from an
	authority *renews* (RFC 3339, UTC, when certd has chosen it) and
	*revoked* (*true* when it is). *account* is the account at the
	authority the certificate came from, *made\_ms* (when it was made,
	in milliseconds since 1970) and *contact* (whether it has a contact
	address), and *null* for a certificate no authority issued; it never
	has the address, which only root reads (*certd*(8)). A certificate
	that does not read gets a line on standard error and makes the exit
	status 1.

*cert show* _name_ [*--json*] [*--pem*]
	Shows everything about a certificate, a line each: its kind, when
	it was made or added, its names, subject, issuer, the times it is
	valid from and to, the serial number, the fingerprint (SHA-256, in
	the form browsers show it, _AB:CD:..._), the type of its key, the
	subjects of the chain, whether the system's authorities vouch for it
	and if not why, the services that name it, and the paths of its
	files. For a certificate from an authority it also shows the
	authority's address (*Authority*), how the names were proved
	(*Proved by*: *http-01* or *dns-manual*), what was asked for
	(*Asked for*: the names and the type of key), and the day certd will
	renew it (*Renews*) or that it is revoked (*Revoked*). It never
	shows the key. With *--pem*, it prints the chain,
	the certificate first, which is what a client given a copy of it
	trusts, as in *curl --cacert*. With *--json*, the object *list*
	gives, without *account*, with *made*, *serial*, *key*, *chain* (the
	subjects), *why\_not\_trusted* and *files*, and *chain\_pem*, the
	chain as *--pem* prints it, which is public as the file is.

*cert add* _name_ *--chain* _file_ *--key* _file_ [*--replace*] [*--untrusted*]++
*cert add* _name_ *--from* *-* [*--replace*] [*--untrusted*]
	Adds the certificate in _file_ of *--chain* with the key in the
	file of *--key*, as the authority gave them: PEM, the server's
	certificate first and then the intermediate certificates that lead
	to the authority's. The chain holds 1 to 10 certificates and
	nothing else but white space; the key is one PEM private key
	(PKCS#8, PKCS#1 or SEC1); neither file may be over 64 KiB. This-machine
	is certd's and is not added. A name that exists is refused unless
	*--replace* is given, which swaps the files and has the services
	that name it read them again; it works only on a certificate added
	by hand. *cert add* checks, in this order, and refuses with the
	reason in words:
	- the name, and that it does not exist, or does with *--replace*;
	- that the files are as described;
	- that the key is one the system can sign with (RSA, ECDSA or
	  Ed25519) and is the first certificate's;
	- that the first certificate is valid now, has at least one DNS or
	  IP name, and is not a certificate authority's;
	- that the system's authorities vouch for the chain
	  (_/system/settings/ssl/certs/ca-certificates.crt_, so an
	  authority you added is trusted too), for a TLS server, with the
	  file's other certificates as intermediates.
	A chain whose last certificate's issuer no authority of the system
	signed is refused with the issuer's name: the file most often lacks
	the intermediate certificate, which your authority gives with the
	certificate, and which belongs after it in the file. Browsers other
	than Chrome refuse such a certificate too. *--untrusted* takes a
	chain that none of the system's authorities vouch for, for a
	private authority that the clients trust and this machine does not.
	A self-signed certificate is taken, with a note that browsers warn
	about it unless told to trust it.

	With *--from -* in place of the two files, the key and the chain
	are one PEM text on standard input, in either order, as the web
	console sends them: one private key, and the certificates, the
	server's first. White space between the blocks is ignored and
	anything else that is not a block is refused. The input is at most
	128 KiB, of which the key and the chain keep 64 KiB each. It is
	checked as the two files are. A check that fails ends with its
	reason, and with an error ID in parentheses at the end of it, whose
	page says what to do: *CRT-111* when the chain does not lead to an
	authority this machine trusts (most often the intermediate
	certificate is missing, which a private authority is also told by
	*--untrusted*), *CRT-112* when the key is not the certificate's or
	not one the system signs with, and *CRT-113* when the certificate is
	not valid now, names no DNS or IP name, or is an authority's.

*cert get* _name_ [*--name* _domain_]... [*--dns*] [*--staging*] [*--dry-run*] [*--directory* _url_] [*--email* _address_] [*--agree-terms*] [*--key* _type_] [*--profile* _profile_] [*--yes*] [*--progress*]++
*cert get* _name_ [*--name* _domain_]... [*--dns*] [*--staging*] [*--directory* _url_] *--check* [*--json*]
	Gets a certificate for one or more domain names from an ACME
	authority, Let's Encrypt unless *--directory* or *--staging* says
	another, and installs it under _name_ (*certificates from an
	authority*, below). The names are the *--name* options, in order, the
	first the certificate's common name; with none, _name_ is the one
	name, and must then be a domain. A certificate takes 1 to 100 names,
	and a name that starts with \*. (a wildcard) only with *--dns*.
	Where _name_ is a certificate that *cert get* made, it is got again
	with the names, the authority, the type of key and the profile it
	had, unless the options say others; a certificate that was added by
	hand, or *this-machine*, is refused.

	An order by HTTP that fails once it has begun, as when a name does
	not lead to this machine, or the authority cannot reach it or
	refuses it, is kept for certd to ask again (*A certificate that could
	not be got*, below), and the message ends with *certd tries again
	every hour; cert forget* _name_ *stops it*. That is not so for an
	order made with *--dry-run*, *--staging* or *--dns*. certd asks
	through the account at the authority. Where there is none yet,
	because the order failed in the checks before the authority was
	asked, the order is kept only with *--agree-terms*, which an account
	needs, and certd makes the account at its first try, with the
	*--email* the order had, as the order would have; without it the
	order is not kept, and the message says nothing of certd. A later
	*cert get* of the same name makes the record anew, and one that works
	takes it away.

	*--dns*
		Prove the names by DNS instead of by HTTP: the command shows
		the TXT records to add, and waits (*proving a name by DNS*,
		below).

	*--staging*
		Use Let's Encrypt's staging authority, whose certificates no
		browser trusts and whose limits are loose, to try the steps.

	*--dry-run*
		Go through every step against Let's Encrypt's staging
		authority, or the one *--directory* names, and install
		nothing.

	*--directory* _url_
		Use another authority, by the address of its directory. It
		must be an https address; an http one only on a private
		network, for an authority of a lab or a company. Not with
		*--staging*.

	*--email* _address_
		The contact address for a new account. Without it the account
		has none: Let's Encrypt no longer mails notices of expiry, and
		certd tells you itself.

	*--agree-terms*
		Agree to the authority's terms of service, which a new account
		shows and asks about; a command with no terminal and no
		*--agree-terms* is refused.

	*--key* *ecdsa-p256*|*ecdsa-p384*
		The type of the new key, ECDSA P-256 unless it says
		otherwise. A new key is made for every certificate.

	*--profile* _profile_
		One of the profiles the authority offers (Let's Encrypt's
		*classic*, say); the authority refuses a name it has not.

	*--yes*
		Replace a certificate that has more than 30 days left without
		asking, and let *--dns* run with no terminal.

	*--progress*
		Tell each step on a line of its own, for a program that shows
		them: *step* _N_ *of* _M_*:* and the words of the step, again
		with other words when what the step is doing changes, and
		*progress* _P_*%* for how far it has come. The steps are the
		checks, the order, the authority's check of the names, the
		certificate and, unless *--dry-run*, its installing; under
		*--dns* a step waits for the TXT records, and before it a
		line *add:* _name_ *TXT* "_value_" is written for each, as a
		DNS provider's page takes it, the name without its last dot.

	*--check*
		Make the checks that come before an order (*certificates from
		an authority*, below) and order nothing, so that anyone may ask,
		and not only root. It exits 0 when no check failed and 1 when
		one did, with each check's result on standard output; a
		mistake in what is asked ends with 2 and its reason on
		standard error. A name of one word, such as *localhost*, is let
		through, so that the check says what is wrong with it. *--check*
		does not go with *--progress*, which tells the steps of an
		order.

	*--json*
		With *--check*, the result as one JSON object: *format* (1),
		*checks*, an array of objects with *id* (the error ID of a
		warning or a failure that has one, else *null*), *result*
		(*passed*, *warning*, *failed* or *skipped*), *name* (the DNS
		name it is about, *null* for a check of the machine) and
		*words*; *firewall\_rule*, the number *firewall list* gives
		the rule that decides on port 80, *null* when none decides;
		*port80\_holder*, which program holds port 80 as far as a
		person who cannot bind it can tell (*nginx*, *certd*, the
		name of a service, or *another program*), *null* when it is
		free or not told; *terms\_url*, the address of the authority's
		terms of service, *null* when it gave none in a few seconds;
		and *dns\_records*, the name and *type* of each TXT record that
		*--dns* asks for. The IDs are *CRT-101* to *CRT-106*.

*cert history* [_name_] [*-n* _number_] [*--json*]
	Shows what was done to the certificates, the oldest first, from
	_/system/state/certd/history_, which certd and *cert* write: a line
	for each time the machine's certificate was made, a certificate was
	got, renewed, added, replaced, revoked or removed, and each time
	getting or renewing one failed, with the reason and the error ID
	at its end when it has one. With a _name_, that certificate's
	lines alone; with *-n*, the newest _number_ of them. A line has the
	time, the certificate, what happened, the words, and who did it
	(*certd* or *cert*), with how long an order took when that is
	known. With *--json*, an array of objects with *at\_ms*
	(milliseconds since 1970), *name*, *event* (*made*, *renewed*,
	*got*, *added*, *replaced*, *revoked*, *removed* or *failed*),
	*detail*, *took\_ms* (or *null*) and *by*. Anyone may read it.

*cert wanted* [*--json*]
	Shows the certificates that could not be got, which certd asks the
	authority for again: one row for each, by name, with its first two
	names and the number of others, how many tries failed, the time of
	the first try, the time of the next, or _given up_ when certd tries
	no more, and why the last try failed. With none, it says that no
	certificate is being tried again. With *--json*, an object with
	*format* (1) and *wanted*, an array of objects with *name*, *names*,
	*directory* (the authority's address), *first\_ms*, *last\_ms* and
	*next\_ms* (milliseconds since 1970), *tries*, *why* and *given\_up*.
	The account at the authority is left out. Anyone may read it.

	```
	$ cert wanted
	NAME         NAMES                         TRIES  FIRST TRY            NEXT TRY             WHY
	example.com  example.com, www.example.com  3      2026-11-16 09:12:00  2026-11-16 12:12:00  the authority could not reach http://example.com/.well-known/acme-challenge/
	```

*cert forget* _name_
	Has certd stop asking for a certificate that could not be got, and
	takes away its alert. It takes away the record whatever it holds,
	one that does not read too, which certd leaves alone. It is refused
	when there is no record of _name_. It changes no certificate: one
	that is in the store stays.

*cert revoke* _name_ [*--reason* _reason_]
	Revokes a certificate from an authority, through the account that
	got it, and marks it revoked: *cert list* shows it, certd renews
	nothing revoked, and the services that name it keep using it until
	you replace it with *cert get* _name_. The reason is one of
	*unspecified*, *key-compromise*, *superseded* and
	*cessation-of-operation*. A certificate that was added by hand, or
	*this-machine*, is refused: only the authority that made one can
	revoke it.

*cert remove* _name_
	Removes the certificate and its files, and the record of it that
	certd asks again from. It is refused for this-machine, and while a
	service's file names the certificate: the message names the file, and
	the line goes from it first. A name that has only such a record is
	removed too.

*cert renew* _name_
	For this-machine, makes a new certificate now, as certd does when
	it is due, and has the services that name it read it; two within a
	few seconds are told once. A certificate from an authority is got
	again now, as *cert get* does, whatever day certd chose, and the new
	certificate replaces the old with the authority told which it
	replaces; one proved by *--dns* is refused, since certd cannot add
	the records, and is renewed with *cert get* _name_ *--dns*. A
	certificate added by hand renews where it came from: add the
	authority's new one with *cert add* _name_ *--replace*.

*cert check*
	Exits 0 when the clock is right and this-machine reads and has more
	than 30 days left; else 1 with the reason on standard error. It is
	certd's health check (*certd*(8)), and the service is ready when it
	passes.

# CERTIFICATES FROM AN AUTHORITY

An ACME authority gives a certificate to whoever proves they hold the
names, and *cert get* proves them in one of two ways. Before it asks the
authority anything it checks, and says in words what to fix: the clock,
that each name resolves, that no name leads to a private address when the
authority is on the internet, and that the firewall lets the authority
reach port 80. It warns, and goes on, when a name has an IPv6 address and
this machine has none, since the authority tries IPv6 first. The checks
of names and port are left out under *--dns*, which never has the
authority connect.

*Proving a name by HTTP* (the default) has the authority ask port 80 of
the name's address for a file. Port 80 must reach this machine from the
internet: on a machine behind a router, the router forwards it, which
the handbook's Certificates chapter explains. If port 80 is free,
*cert get* answers it itself, for the minutes the check takes, with a
small server of its own that holds nothing else: its own two sockets and
the answers. A command of an administrator takes connections only when
the firewall has a rule that allows them (*firewall*(8)), so the check
fails until *elevate firewall allow tcp 80* is done, which *cert get*
says before it asks the authority; certd, a service that names port 80,
needs no rule. If port 80 is nginx's, *cert get* puts the answer in
_/system/live/certd/acme-challenge_ and nginx serves it there, for every
site that includes its _acme-challenge.conf_, as the default site does,
once nginx's service file lets it read _/system/live/certd_ (the
handbook shows the line). If another program holds port 80, *cert get*
says so.

*Proving a name by DNS* (*--dns*) has the authority look up a TXT record
at _\_acme-challenge._ followed by the name, the same name for a domain
and its wildcard, with one value each. *cert get* shows every record,
each as a line of a zone file, and waits until you have added them and
pressed Enter; with no terminal, *--yes* has it wait for the records to
show without asking, and then its first look comes at once, before the
records are there, so *resolved*(8) may keep its answer of none for the
time the zone gives (an hour at most): *service resolved reload* empties
it. It then asks the system's resolver for each record
every 10 seconds for up to 10 minutes, and goes on when all show. The
records are for this one certificate and are no use again: they may
stay or go.

The certificate that comes back is checked as *cert add* checks one: the
key, the dates and the names. That the system's authorities vouch for it
is checked for Let's Encrypt; for another authority it is only shown
(*cert show*), since which authorities to trust is the administrator's
business (*dukovo-certificates*(7)). It is then installed, the services
that name it are told, and the command says when it runs out and that
certd renews it. A certificate proved by DNS is not renewed by certd,
which cannot add the TXT records: certd warns at 21 days and again at
7 days left, and you run *elevate cert get* _name_ *--dns* again.

Each authority has an account, made when you first use it, with a key
that stays in _/system/state/certd/accounts_, readable by root alone.
*cert get* shows the authority's terms of service and asks whether you
agree, unless *--agree-terms* says so.

certd renews a certificate proved by HTTP at the day the authority's
renewal information (ARI, RFC 9773) suggests, a random time inside the
window the authority names, or two thirds of the way through its life
where the authority says nothing: *certd*(8) says how. *cert renew*
does it at once.

*A certificate that could not be got.* When *cert get* fails once the
order has begun, cert keeps what it was asked for: the names, the authority,
the type of key and the profile, and the account it asked through.
certd asks for the certificate again by itself, as *cert get* does,
every hour for the day after the first try, then every 6 hours, until 14
days after the first try, when it gives up. An authority that asks to
be left alone for a time, as a rate limit does, is left alone for it. Each
try is a line of *cert history*, and when one works the certificate is
installed, the services that name it are told, and the record is taken
away. certd gives a record up early when the account at the authority
was taken away, or when a certificate of that name was added by hand
since; a record given up stays, so that *cert wanted* shows why, until
*cert forget* or a *cert get* that works takes it away.

When the first day is over and the certificate is still not got, certd
raises the alert *certificate:*_name_, "The certificate _name_ could not
be had for a day", with the reason of the last try (*CRT-123*); when it
gives up, *certificate-given-up:*_name_ (*CRT-124*). Both clear by
themselves when the record is taken away.

# THE CHANGE IS TOLD TO SPARK

After *add*, *get*, *remove*, *renew* and *revoke*, cert asks Spark, on its control
socket, to copy the certificate anew for the services that name it, and
Spark sends each a SIGHUP (*spark*(8)). If Spark refuses or does not
answer, cert says *certificates changed, but Spark did not copy them*,
with the reason, and that the services that use it need a restart; the
change itself was made and the exit status is 0.

A change to what certd asks for again (a *get* that failed or worked, a
*remove*, a *forget*) has Spark reload certd, which looks at once, so that
its alerts follow. If Spark does not do it, cert says so and the change
stands: certd looks at the records when its next try is due or at its
next look.

# FILES

_/system/state/certd/certificates_
	The store (*certd*(8)).

_/system/state/certd/certificates/.lock_
	Held while a certificate is written. A second writer waits up to a
	minute, and then says that certd or another cert is changing
	certificates and to try again. It is not held while an authority
	is asked.

_/system/state/certd/accounts_
	The account at each authority, a file for each: its host, with the
	port when the address has one, and _.json_. Readable by root alone.

_/system/state/certd/accounts.json_
	When each account was made and whether it has a contact, never the
	contact or the key, which *cert list* shows to everyone. certd
	writes it (0644).

_/system/state/certd/wanted_
	The certificates that could not be got, a file _name_.toml for each,
	which certd asks for again (0600, in a folder of 0700, root's): the
	names, the authority and the account's file, the type of key, the
	profile, the contact address and whether the terms were agreed to
	(for an account that is made later), the time of the first, last and
	next try, how many failed and why the last did. It is read and
	written only while the store's lock is held.

_/system/state/certd/wanted.json_
	What *cert wanted* shows, the same without the account: *cert*
	and certd write it with each record (0644).

_/system/state/certd/history_
	What *cert history* shows: a JSON line for each event, which certd
	and *cert* append while they hold the store's lock. It is cut to
	the newest 1000 lines when it passes 2000 (0644).

_/system/live/certd/acme-challenge_
	Where the answers to an authority's check go when nginx holds port
	80; each file is taken away when the order ends.

_/system/settings/ssl/certs/ca-certificates.crt_
	The authorities *add* checks a chain against
	(*dukovo-certificates*(7)).

_/system/settings/spark/services_
	The service files, which *list*, *show* and *remove* read to find
	who names a certificate.

# EXIT STATUS

0 when it did what was asked; 1 when it was refused or failed, with a
message that starts with *cert:* and names the cause; 2 for a mistake in
the command line. *cert get --check* ends with 1 when a check failed,
which it has told on standard output, and with 2 when it was refused.

# EXAMPLES

Get a certificate from Let's Encrypt for two names of a site, which
the machine's port 80 must be reachable for, and have nginx use it:

```
elevate firewall allow tcp 80
elevate cert get example.com --name example.com --name www.example.com --email you@example.com --agree-terms
```

Try the same against Let's Encrypt's staging authority first, which
installs nothing:

```
elevate cert get example.com --dry-run --agree-terms
```

Ask what is wrong before ordering, which anyone may do:

```
cert get example.com --name example.com --name www.example.com --check
```

A wildcard certificate, by DNS:

```
elevate cert get example.com --name example.com --name '*.example.com' --dns
```

Add the certificate an authority issued for a site, with its key, as
the files the authority sent, and serve it with nginx:

```
elevate cert add example.com --chain example.com.chain.pem --key example.com.key
elevate service edit nginx
elevate service nginx restart
```

Put _certificates = ["example.com"]_ in nginx's file and name
_/system/live/certificates/nginx/example.com/fullchain.pem_ and
_.../key.pem_ in its settings (*nginx.conf*, in the bundle's data
folder).

Add the key and the chain from one text, as a program that holds them
does:

```
cat example.com.key example.com.chain.pem | elevate cert add example.com --from -
```

What was done to the certificates:

```
cert history example.com -n 5
```

See which certificates certd asks for again, and stop it for one that is not
wanted any more:

```
cert wanted
elevate cert forget example.com
```

The fingerprint of this machine's own certificate, to compare with the
one a browser shows before trusting it:

```
cert show this-machine
```

# SEE ALSO

*certd*(8), *dukovo-certificates*(7), *elevate*(8), *firewall*(8),
*resolved*(8), *service.toml*(5), *service*(8), *spark*(8), *systools*(1)

cert(8) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

cert - list, add, get, renew, revoke and remove the machine's certificates

# SYNOPSIS

*cert list* [*--json*]++
*cert show* _name_ [*--json*] [*--pem*]++
*cert add* _name_ *--chain* _file_ *--key* _file_ [*--replace*] [*--untrusted*]++
*cert get* _name_ [*--name* _domain_]... [*--dns*] [*--staging*] [*--dry-run*]++
[*--directory* _url_] [*--email* _address_] [*--agree-terms*]++
[*--key* *ecdsa-p256*|*ecdsa-p384*] [*--profile* _profile_] [*--yes*]++
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

*list*, *show* and *check* may be run by anyone, as certificates are
public and keys are not shown. *add*, *get*, *remove*, *renew* and
*revoke* change what services are given, so they need root: run them
with *elevate*(8).

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
	with *name*, *kind*, *names*, *not_before* and *not_after* (RFC 3339,
	UTC), *days_left*, *fingerprint_sha256* (lowercase hexadecimal, no
	colons), *issuer*, *trusted* (whether the system's authorities vouch
	for it) and *used_by*, and for a certificate from an authority
	*renews* (RFC 3339, UTC, when certd has chosen it) and *revoked*
	(*true* when it is). A certificate that does not read gets a line on
	standard error and makes the exit status 1.

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
	gives.

*cert add* _name_ *--chain* _file_ *--key* _file_ [*--replace*] [*--untrusted*]
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

*cert get* _name_ [*--name* _domain_]... [*--dns*] [*--staging*] [*--dry-run*] [*--directory* _url_] [*--email* _address_] [*--agree-terms*] [*--key* _type_] [*--profile* _profile_] [*--yes*]
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
	Removes the certificate and its files. It is refused for
	this-machine, and while a service's file names the certificate: the
	message names the file, and the line goes from it first.

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

# THE CHANGE IS TOLD TO SPARK

After *add*, *get*, *remove*, *renew* and *revoke*, cert asks Spark, on its control
socket, to copy the certificate anew for the services that name it, and
Spark sends each a SIGHUP (*spark*(8)). If Spark refuses or does not
answer, cert says *certificates changed, but Spark did not copy them*,
with the reason, and that the services that use it need a restart; the
change itself was made and the exit status is 0.

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
the command line.

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
_/system/live/spark/certificates/nginx/example.com/fullchain.pem_ and
_.../key.pem_ in its settings (*nginx.conf*, in the bundle's data
folder).

The fingerprint of this machine's own certificate, to compare with the
one a browser shows before trusting it:

```
cert show this-machine
```

# SEE ALSO

*certd*(8), *dukovo-certificates*(7), *elevate*(8), *firewall*(8),
*resolved*(8), *service.toml*(5), *service*(8), *spark*(8), *systools*(1)

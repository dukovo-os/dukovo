cert(8) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

cert - list, add, remove and renew the machine's certificates

# SYNOPSIS

*cert list* [*--json*]++
*cert show* _name_ [*--json*] [*--pem*]++
*cert add* _name_ *--chain* _file_ *--key* _file_ [*--replace*] [*--untrusted*]++
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
are ones you add, from an authority or of your own making.

A name is 1 to 64 of the letters _a_ to _z_, the digits and _-_ and _._,
beginning with a letter or a digit, with no two dots in a row. It is
usually the site's name, as *example.com*.

*list*, *show* and *check* may be run by anyone, as certificates are
public and keys are not shown. *add*, *remove* and *renew* change what
services are given, so they need root: run them with *elevate*(8).

# COMMANDS

*cert list* [*--json*]
	Shows one row for each certificate, by name: its name, its kind
	(*self-signed* for this-machine, *added* for one that *cert add*
	took), its first two names and the number of others, the day it runs
	out with the days left, or *ran out* and the day, in the error's
	colour when the terminal has colour, and the services whose files
	name it. With *--json*, an array of objects with *name*, *kind*,
	*names*, *not_before* and *not_after* (RFC 3339, UTC), *days_left*,
	*fingerprint_sha256* (lowercase hexadecimal, no colons), *issuer*,
	*trusted* (whether the system's authorities vouch for it) and
	*used_by*. A certificate that does not read gets a line on standard
	error and makes the exit status 1.

*cert show* _name_ [*--json*] [*--pem*]
	Shows everything about a certificate, a line each: its kind, when
	it was made or added, its names, subject, issuer, the times it is
	valid from and to, the serial number, the fingerprint (SHA-256, in
	the form browsers show it, _AB:CD:..._), the type of its key, the
	subjects of the chain, whether the system's authorities vouch for it
	and if not why, the services that name it, and the paths of its
	files. It never shows the key. With *--pem*, it prints the chain,
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

*cert remove* _name_
	Removes the certificate and its files. It is refused for
	this-machine, and while a service's file names the certificate: the
	message names the file, and the line goes from it first.

*cert renew* _name_
	For this-machine, makes a new certificate now, as certd does when
	it is due, and has the services that name it read it; two within a
	few seconds are told once. A certificate added by hand renews where
	it came from: add the authority's new one with *cert add* _name_
	*--replace*.

*cert check*
	Exits 0 when the clock is right and this-machine reads and has more
	than 30 days left; else 1 with the reason on standard error. It is
	certd's health check (*certd*(8)), and the service is ready when it
	passes.

# THE CHANGE IS TOLD TO SPARK

After *add*, *remove* and *renew*, cert asks Spark, on its control
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
	certificates and to try again.

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

*certd*(8), *dukovo-certificates*(7), *elevate*(8), *service.toml*(5), *service*(8),
*spark*(8), *systools*(1)

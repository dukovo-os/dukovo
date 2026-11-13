certd(8) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

certd - keep the machine's own certificate, renew the ones from an authority and watch the others

# SYNOPSIS

*certd*

# DESCRIPTION

certd keeps the certificates that *cert*(8) lists, in
_/system/state/certd/certificates_ (ADR 0512). Spark runs it as the
*certd* service, after *timed*(8), with the place it keeps them in and
the folder of its socket to Spark read-only, the network to reach an
authority and port 80 to answer it (*spark*(8), *service.toml*(5)). It
takes no argument.

The machine's own certificate is *this-machine*, which certd makes for
the services that name it, such as the web console and *nginx*: an
ECDSA P-256 key and a certificate that names the machine, as *name*,
*name*.local, which *discoverd*(8) answers for, and *localhost*, the
name first, and *localhost* alone for a machine still called that. It
is valid from an hour before it is made, for clocks that differ, and
for 800 days: Apple's systems refuse a TLS server certificate valid for
more than 825 days, even one a person chose to trust. It is for a TLS
server, and not a certificate authority's.

The certificate is self-signed, so a browser warns of it until told
to trust it. *cert show this-machine* shows its SHA-256 fingerprint,
which a browser shows too: compare them, then trust it. Giving the
browser the chain *cert show this-machine --pem* prints makes it trust
that certificate and no other.

# WHAT IT DOES

When it starts, and then every 12 hours and up to an hour more, so that
machines do not all wake together, certd looks at the certificates:

- It makes this-machine when it is missing or does not read, when 30
  days or fewer are left, and when its first name is not the machine's
  name any more. Each time it logs one line with the new fingerprint, and
  tells Spark, which copies the certificate for the services that name
  it and sends each a SIGHUP, at most one in 10 seconds (*spark*(8)). It
  logs nothing when nothing was due. A certificate that was there
  before and is made again also gets a line in the notice the next
  sign-in shows, and in *notifications*(1), with its new fingerprint:
  a person who trusted the old one needs it.
- For each certificate that *cert add* took, with 21 days or fewer
  left, it puts a line in the sign-in notice (_/system/live/sign-in_),
  and sends a notification when what the notice says changed: normal,
  and critical when 7 days or fewer are left or once the certificate
  has run out. The line says to add its new version with *cert add*
  _name_ *--replace*. With nothing to tell, the notice is removed.
- For each certificate that *cert get* got by HTTP (*cert*(8)), it
  renews it when its day has come (*Renewal*, below). A certificate
  that *cert get* got with *--dns* is told as one added by hand is, at
  21 days and again at 7 days left, with the line saying to run
  *elevate cert get* _name_ *--dns* again, since certd cannot add the
  TXT records.

A clock that is before the system's making is wrong: a machine whose
battery is flat boots with 1970 on it. certd makes and judges no
certificate then, since one made then would be dated 1970; it logs that
it waits, and looks again each minute until the clock is right, which
*timed*(8) sets from the network. *cert check* fails for the same
reason, so the service is not ready until then.

A look at which a certificate could not be made is repeated 5 minutes
later.

# RENEWAL

certd renews a certificate from an authority at a day it chooses and
keeps in the certificate's _certificate.toml_ as *renew_at*, which
*cert list* and *cert show* tell. It asks the authority for the window
in which it suggests renewing (ACME renewal information, ARI, RFC 9773),
naming the certificate by its authority key identifier and serial
number, and picks a time at random inside the window, so that the
machines of a site do not all ask on the same minute; a window that has
begun is cut short to start now. A time already kept stays while it is
inside the window; one the window no longer holds, as when the authority
asks for certificates to be replaced sooner, is replaced by a new pick.
The authority says when to ask again, and certd does not ask before: an
hour at the least and a day at the most, kept in memory, so that a restart
asks once more. An authority that has no renewal information has the
certificate renewed when two thirds of its life are gone, which is 30
days before a certificate of 90 days runs out and 15 before one of 45.

When the day has come, certd gets a new certificate for the same names,
by the same means, as *cert get* does with the key type and the profile
the certificate was got with, a new key, and the account kept in
_/system/state/certd/accounts_. The order names the certificate it
replaces, which the authority counts as a renewal for its rate limits.
The new certificate is installed, Spark copies it for the services that
name it and sends each a SIGHUP, and certd logs one line with the day it
now runs out. It listens on port 80 for the minutes the authority's check
takes, which the right to listen the service holds opens only while a
socket is bound (the firewall still decides who gets through, and a
service that holds the port needs no rule); where nginx holds the port,
certd writes the answers into _/system/live/certd/acme-challenge_ for
nginx to serve, as *cert*(8) says.

A renewal that fails is logged with the reason in words and tried again
at the next look. From 14 days left, the sign-in notice says that it
could not be renewed and when the certificate runs out, with a
notification, critical from 7 days. A certificate revoked with *cert
revoke* is not renewed, and the notice says to get a new one with *cert
get*.

# READINESS

Spark counts the service ready once its health check, *cert check*
(*cert*(8)), passes, and runs it every hour afterwards: this-machine
reads and has more than 30 days left, and the clock is right. A clock
that stays wrong for the 120 seconds the service has to get ready has
Spark stop certd and start it again, as the file's *restart* says, and
certd logs again that it waits.

# SIGNALS

*SIGHUP*
	Look at the certificates now: *service certd reload*
	(*service*(8)) sends it.

*SIGTERM*
	End.

# FILES

_/system/state/certd/certificates_
	One folder of the store for each name's generation, _.name.7_, and
	a link _name_ to the current one. A generation holds _fullchain.pem_
	(the certificate first, then the chain), _key.pem_ (mode 0600, read
	by root alone) and _certificate.toml_, which says whether it is
	*self-signed* or *added* and when. A certificate is written whole
	into a new generation and a link is moved to it with one rename, so
	a reader sees the old pair or the new, never a new chain with the old
	key; the generation before is kept. Anyone may read the chains: they
	are public. The folder _/system/state_ is readable by all, as
	_/var_ is on Linux.

_/system/state/certd/certificates/.lock_
	Held while a certificate is written, by certd and by *cert*(8), and
	not while an authority is asked.

_/system/state/certd/accounts_
	The account at each authority (*cert*(8)).

_/system/live/certd/acme-challenge_
	The answers to an authority's check, one file a token, which certd
	puts here while an order runs when nginx holds port 80, and takes
	away afterwards.

_/system/live/spark/certd/socket_
	Spark's socket for certd, through which it asks for a copy
	(*certificate* _name_). It answers certd's process alone.

_/system/live/sign-in/certd_
	The notice of what needs a person's attention.

_/system/settings/spark/services/certd.toml_
	Its service (*service.toml*(5)).

# EXIT STATUS

0 when it was ended; 1 for an argument, as it takes none.

# LIMITS

Not yet: certificates proved by DNS that renew by themselves, which
need the DNS provider's own interface; External Account Binding, which
some authorities ask for; TLS-ALPN-01; certificates for IP addresses;
and RSA keys, which certd does not make (ring makes none, and every
browser takes ECDSA), though one added by hand with *cert add* works.
certd does not ask Spark whether nginx runs, so when port 80 is taken it
takes nginx to hold it.

# SEE ALSO

*cert*(8), *dukovo-certificates*(7), *timed*(8), *service.toml*(5),
*service*(8), *spark*(8), *systools*(1)

kinit(1) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

kinit - get a Kerberos ticket

# SYNOPSIS

*kinit* [*-V*] [*-l* _lifetime_] [*-r* _renewable_] [*-f* | *-F*] [_principal_]

*kinit* *-k* [*-t* _keytab_] [*-V*] [*-l* _lifetime_] [_principal_]

# DESCRIPTION

*kinit* proves who you are to your realm's KDC and gets a
ticket-granting ticket, with which tickets for services are taken
without asking again (ADR 0397). It asks for the password of _principal_, unseen at a
terminal, or reads it as a line of the standard input; with *-k* it
uses the principal's key from a keytab instead.

_principal_ is *name*[*/*_instance_][*@*_REALM_]: without a realm, it is
in krb5.conf's *default_realm*. Without a principal, *kinit* asks for
the one whose tickets your keyring holds, or else for your own name.

The tickets go in your keyring (*keyring*(1)), each as the entry
*krb5* _client server_, in place of the tickets of whoever held them
before. They close with the keyring when your last session ends, and
*kdestroy*(1) removes them sooner.

Dukovo speaks the AES encryption types of RFCs 3962 and 8009, which
every KDC of this century offers: MIT's, Heimdal's, Samba's and Active
Directory's. A principal with only DES or RC4 keys cannot sign in.

# OPTIONS

*-l* _lifetime_
	Asks for a ticket lasting _lifetime_, such as *10h*, *1d12h*,
	*90m* or *36000* (seconds), in place of krb5.conf's
	*ticket_lifetime*. The KDC may give less.

*-r* _renewable_
	Asks for a ticket that may be renewed for _renewable_.

*-f*, *-F*
	Asks for a ticket that may, or may not, be forwarded to another
	host.

*-k*
	Proves the principal with its key from a keytab
	(/system/settings/krb5.keytab, or *KRB5_KTNAME*'s), without a
	password. Without a principal, it is the keytab's first.

*-t* _keytab_
	The keytab for *-k*.

*-V*
	Says whom it signed in as.

# ENVIRONMENT

*KRB5_CONFIG*
	The krb5.conf to read, in place of /system/settings/krb5.conf.

*KRB5_KTNAME*
	The keytab for *-k*, with or without MIT's *FILE:* before it.

# EXAMPLES

```
$ kinit alice@EXAMPLE.ORG
Password for alice@EXAMPLE.ORG:
$ klist
```

# DIAGNOSTICS

*the password or key is wrong*
	The KDC refused the proof: check the password, or the keytab's
	key version against the KDC's (*klist -k*).

*the KDC knows no such principal*
	Check the name and the realm.

*this computer's clock is more than five minutes from the KDC's*
	Kerberos refuses a clock that far off; *timedatectl*(1) shows
	it.

*no KDC answered*
	Each KDC is named, and why: a refused connection means none
	listens there, a timeout that something is in the way.
	krb5.conf(5) says where the KDCs are.

# EXIT STATUS

0 when the tickets were got and kept; 1 when not, saying why.

# SEE ALSO

*klist*(1), *kdestroy*(1), *krb5.conf*(5), *keyring*(1), *systools*(1)

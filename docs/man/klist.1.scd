klist(1) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

klist - list Kerberos tickets, or a keytab's keys

# SYNOPSIS

*klist* [*-e*] [*-s*]

*klist* *-k* [*-e*] [_keytab_]

# DESCRIPTION

*klist* lists the Kerberos tickets in your keyring (ADR 0397): whose
they are, then each ticket's start, end and service, in UTC, with its
flags. The ticket-granting ticket that *kinit*(1) got comes first; the
tickets for services that were taken with it follow.

The flags are *F* forwardable, *f* forwarded, *P* proxiable,
*R* renewable, *I* initial (from *kinit*, not from another ticket) and
*A* pre-authenticated.

# OPTIONS

*-e*
	Names each session key's encryption type, or each key's with
	*-k*.

*-s*
	Lists nothing; the exit status says whether your keyring holds a
	ticket-granting ticket that has not ended.

*-k* [_keytab_]
	Lists the keys of _keytab_ (/system/settings/krb5.keytab, or
	*KRB5_KTNAME*'s): each key's version and principal.

# EXAMPLES

```
$ klist
Ticket cache: keyring
Default principal: alice@EXAMPLE.ORG

Valid starting           Expires                  Service principal
2026-11-06 08:34:54 UTC  2026-11-06 18:34:54 UTC  krbtgt/EXAMPLE.ORG@EXAMPLE.ORG
	Flags: IA
```

# EXIT STATUS

0 when it listed; 1 when your keyring holds no tickets, the keytab does
not read, or with *-s*, when there is no ticket-granting ticket that
has not ended.

# SEE ALSO

*kinit*(1), *kdestroy*(1), *keyring*(1), *krb5.conf*(5)

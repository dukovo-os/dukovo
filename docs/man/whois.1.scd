whois(1) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

whois - ask the registries who holds a domain, an address or an AS number

# SYNOPSIS

*whois* [*-h* _host_] [*-p* _port_] [*--verbose*] _object_...

# DESCRIPTION

whois asks the WHOIS servers about _object_, a domain, an IPv4 or IPv6
address or network, or an autonomous system's number, and prints what
they answer (RFC 3912). Several words are one question.

Without *-h* it asks IANA's server, _whois.iana.org_, whose answer names
the registry that holds the object, and follows the referrals from
there: a registry's *refer:*, *whois:* or *ReferralServer: whois://*, and
a domain registry's *Registrar WHOIS Server:*, up to four servers, never
one twice. It prints every answer, each after a line naming its server,
as FreeBSD's whois does. ARIN's server is asked for the networks that
hold an address (*n +* _address_) rather than for a summary (ADR 0341).

```
$ whois example.com
% whois.iana.org:

domain:       EXAMPLE.COM
...
```

What the registries answer is theirs, in their own words; many now give
a domain's holder only through RDAP, on the web.

# OPTIONS

*-h* _host_, *--host* _host_
	Ask _host_ alone, and follow nothing.

*-p* _port_, *--port* _port_
	Ask at _port_, not 43.

*--verbose*
	Say on standard error which server is asked, and what.

# EXIT STATUS

0 when the first server answered; 1 for an option or an argument it does
not take; 2 when the first server could not be reached.

# SEE ALSO

*dig*(1), *host*(1), *systools*(1)

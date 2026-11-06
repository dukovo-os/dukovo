krb5.conf(5) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

krb5.conf - Kerberos realms and their KDCs

# SYNOPSIS

/system/settings/krb5.conf

# DESCRIPTION

*krb5.conf* says which realm a name without one is in, where each
realm's KDCs are, which realm a host is in, and what tickets are asked
for (ADR 0397). Dukovo reads MIT's format, so a krb5.conf copied from a
Linux host works as it is; what Dukovo does not use, it skips.

Sections start with a name in brackets. Each line in one is
_name_ *=* _value_, and a value of *{* starts a group that a *}* line
ends. Lines starting with *#* or *;* are comments.

The *KRB5_CONFIG* variable names another file. With no file, every
name must give its realm, and no KDC is known.

Dukovo does not look KDCs up in DNS: each realm names its own.

# [libdefaults]

*default_realm* = _REALM_
	The realm of a principal named without one.

*ticket_lifetime* = _lifetime_
	How long a ticket asked for lasts: *10h*, *1d*, *36000*. One day
	unless set.

*renew_lifetime* = _lifetime_
	How long a ticket may be renewed for. Not renewable unless set.

*forwardable* = *true* | *false*
	Whether tickets are asked to be forwardable.

*permitted_enctypes* = _types_, *default_tkt_enctypes* = _types_
	The encryption types to ask for, the first choice first:
	*aes256-sha2*, *aes128-sha2*, *aes256-cts*, *aes128-cts*, their
	full names, or *aes* for all four. Names of other types are
	skipped. All four unless set.

# [realms]

Each realm is a group:

*kdc* = [*tcp/*|*udp/*]_host_[*:*_port_]
	One of the realm's KDCs, at port 88 unless one is given; *kdc*
	may be given more than once, and each is tried in turn. Each is
	reached over TCP, then over UDP, unless *tcp/* or *udp/* keeps
	it to one.

# [domain_realm]

_host_ = _REALM_, *.*_domain_ = _REALM_
	The realm a host is in, by its own name or, with a dot first,
	by any domain it ends in. A service on a host, as an NFS server,
	is asked for in that realm.

# EXAMPLES

```
[libdefaults]
    default_realm = EXAMPLE.ORG
    ticket_lifetime = 10h

[realms]
    EXAMPLE.ORG = {
        kdc = kdc1.example.org
        kdc = kdc2.example.org
    }

[domain_realm]
    .example.org = EXAMPLE.ORG
```

# SEE ALSO

*kinit*(1), *klist*(1), *kdestroy*(1)

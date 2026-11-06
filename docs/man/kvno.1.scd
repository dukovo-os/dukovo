kvno(1) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

kvno - take Kerberos tickets for services

# SYNOPSIS

*kvno* _service_...

# DESCRIPTION

*kvno* takes a ticket for each _service_, such as
*nfs/server.example.org*, with the ticket-granting ticket that
*kinit*(1) got, keeps it in your keyring beside it (ADR 0397), and
writes the version of the service's key the ticket is under. A ticket
already kept that has not ended is used again.

A key version that differs from the one in the service's keytab
(*klist -k* on the service's host) means the keytab is out of date:
the service cannot open the tickets the KDC gives for it.

A _service_ without a realm is in krb5.conf's *default_realm*.

# EXAMPLES

```
$ kvno nfs/server.example.org
nfs/server.example.org@EXAMPLE.ORG: kvno = 2
```

# EXIT STATUS

0 when every ticket was taken; 1 when one was not, saying why on the
standard error.

# SEE ALSO

*kinit*(1), *klist*(1), *keyring*(1), *krb5.conf*(5)

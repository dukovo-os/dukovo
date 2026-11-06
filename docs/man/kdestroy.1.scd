kdestroy(1) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

kdestroy - throw away your Kerberos tickets

# SYNOPSIS

*kdestroy* [*-q*]

# DESCRIPTION

*kdestroy* removes every Kerberos ticket from your keyring: the
*krb5* entries that *kinit*(1) and the services you reached made
(ADR 0397). Your keyring's other entries stay.

The tickets go anyway when your keyring closes, as when your last
session ends.

# OPTIONS

*-q*
	Says nothing, which is what *kdestroy* always does; MIT's
	scripts pass it.

# EXIT STATUS

0 when the tickets are gone, or there were none; 1 when the keyring
could not be reached, as when it is locked.

# SEE ALSO

*kinit*(1), *klist*(1), *keyring*(1)

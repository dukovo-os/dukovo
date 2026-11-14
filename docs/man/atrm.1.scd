atrm(1) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

atrm - remove jobs queued with at

# SYNOPSIS

*atrm* _job_...

# DESCRIPTION

atrm removes the jobs of the numbers given, which *atq*(1) lists. You
remove your own; an administrator removes anyone's. It is *at -d*
(ADR 0478).

# EXIT STATUS

0, or 1 when a job is not there or is not yours, or *timerd*(8) cannot be
asked; the others are removed all the same.

# SEE ALSO

*at*(1), *atq*(1), *timerd*(8)

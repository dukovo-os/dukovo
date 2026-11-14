atq(1) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

atq - list the jobs queued with at

# SYNOPSIS

*atq*

# DESCRIPTION

atq lists the jobs that *at*(1) queued for you, one on each line: the
job's number, the time it runs and its owner. An administrator sees
every person's jobs. It is *at -l* (ADR 0478).

# EXIT STATUS

0, or 1 when *timerd*(8) cannot be asked.

# SEE ALSO

*at*(1), *atrm*(1), *timerd*(8)

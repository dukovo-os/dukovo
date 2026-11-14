batch(1) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

batch - run commands when the load is low (not built)

# SYNOPSIS

*batch*

# DESCRIPTION

On other systems, batch queues commands that run once the load is low.
Dukovo does not have that yet (docs/deferred.md): batch says so and
exits with status 1. Use *at*(1), which runs commands at a time, for
what must run later.

# EXIT STATUS

1.

# SEE ALSO

*at*(1), *crontab*(1)

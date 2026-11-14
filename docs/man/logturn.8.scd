logturn(8) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

logturn - turn over the log files of services

# SYNOPSIS

*logturn* [*--now*] [_name_...]

# DESCRIPTION

logturn turns over the log files that services write themselves
(ADR 0478), those that a service's *[logs]* names (*service.toml*(5)).
The system's own timer starts it each hour; with no names it takes every
service whose file has a *[logs]*, and with names, those.

A file is due when its day or week is over since logturn last turned it,
or when it holds as many bytes as the file's *turn_over* says. logturn
then

. shifts the files turned over before up one place, _x.log.1_ to
  _x.log.2_, and removes those beyond *keep*;
. renames _x.log_ to _x.log.1_, which keeps its owner and mode, and
  nothing is cut or copied, so that a service that still has the file
  open writes on into it, and no line is lost;
. asks Spark to have the service open its files again, once for each
  service, as *reopen* says: a signal, or a restart;
. compresses the files turned over before this one, from _x.log.2_ on,
  with zstd, as _x.log.2.zst_, with the owner, mode and times the file had.
  _x.log.1_ is never compressed at once, since the service may write to it
  until it has opened its file again.

A file that is not there, or is empty, is not turned over. A relative
pattern is in the service's data folder: the _HOME_ of its environment,
which for a bundle's service is the folder of its account, else
_/system/state/_name.

# OPTIONS

*--now*
	Turn over each file that holds anything, due or not.

# FILES

_/system/state/logturn/turned.json_
	When each file was last turned over.

# EXIT STATUS

0, or 1 when a file could not be turned over, with the reason in the log.

# SEE ALSO

*service.toml*(5), *timerd*(8), *spark*(8), *log*(1), *logd*(8)

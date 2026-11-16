lastlog(8) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

lastlog - show each account's last sign-in

# SYNOPSIS

*lastlog* [*-u* _name_] [*-t* _days_] [*-b* _days_] [*--json*]

# DESCRIPTION

lastlog shows when each account last signed in, on what terminal and
from where, as shadow's lastlog does, from the sign-in records
*authd*(8) keeps (ADR 0254): root's account and the people's, in the
order of _/system/settings/passwd_. An account that never signed in
shows *\*\*Never signed in\*\**.

Root reads every account's. Anyone else reads their own alone, and
gets a hint on the standard error saying so; an administrator runs
*elevate lastlog*.

With *--json* it writes one JSON array on the standard output, which the
web console reads (*admind*(8)), an object for each account the options
leave, in the same order:

```
[{"name": "alice", "sign_in": {"at_ms": 1763110000000, "terminal": "web",
  "from": "192.168.1.25", "via": "console"}, "failed": 0, "last_failed": null}]
```

*sign_in* is the account's newest sign-in, and *last_failed* the newest
wrong password typed for it since; each is null where there was none,
and *sign_in* is null for an account that never signed in. *at_ms* is
Unix milliseconds. *from* is null where the sign-in was at the machine
itself. *via* says how the person came: *ssh*, or *console* for the web
console and its terminals; it is null at the machine itself, for a
wrong password, and for a sign-in recorded before it was kept. *failed* is how many wrong passwords were typed for the account
since its last sign-in, or ever, if it never signed in. The hint is not
written.

# OPTIONS

*-u* _name_, *--user* _name_
	Only the account _name_, or of that number. Another person's is
	refused to anyone but root.

*-t* _days_, *--time* _days_
	Only sign-ins of the last _days_ days.

*-b* _days_, *--before* _days_
	Only sign-ins older than _days_ days, and the accounts that never
	signed in.

*--json*
	Write the accounts as a JSON array, as above.

# EXAMPLES

```
$ elevate lastlog
Username         Port     From             Latest
root                                       **Never signed in**
alice            pts/0                     Sat Oct 31 12:40:01 +0000 2026
bob              pts/1                     Sat Oct 31 12:46:12 +0000 2026
```

# FILES

_/system/state/sign-ins/_*number*
	Each account's sign-ins, which its person and root read.

# EXIT STATUS

0; 1 if the accounts cannot be read, for an account there is not or
another person's, and for a mistake in the command line.

# SEE ALSO

*last*(1), *authd*(8), *user*(8), *systools*(1)

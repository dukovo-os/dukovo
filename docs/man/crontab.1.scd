crontab(1) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

crontab - keep your crontab, and whether your jobs run while you are signed out

# SYNOPSIS

*crontab* [*-u* _user_] _file_ | *-*++
*crontab* [*-u* _user_] *-l* | *-e* | *-r* [*-i*] | *--status*++
*crontab* *--keep-running* [*on* | *off* | *default*]

# DESCRIPTION

A crontab is a list of lines, each a time and a command that *timerd*(8)
runs at that time, as you, with _/bin/sh_ (ADR 0478). *crontab*(5) says
how a line is written. Each person has one, kept by timerd and not in a
file of theirs: crontab sends it to timerd, which checks it and refuses a
wrong line with its line and column, and installs nothing then.

With a _file_, or *-* for standard input, crontab installs the text as
your crontab, in place of the one you had.

# OPTIONS

*-l*
	Print your crontab. Status 1 and "no crontab" when you have none.

*-e*
	Edit your crontab with the program _VISUAL_ or _EDITOR_ names, or
	*vi*, on a copy. The text is checked when the editor ends: if it
	does not read, the mistake is shown with its line, and you edit it
	again or leave the crontab as it was.

*-r*
	Remove your crontab. With *-i*, ask first.

*--status*
	Say, for each line, its time, when it runs next and how it last ended,
	with the last of its output, or why it did not run.

*-u* _user_
	The crontab of _user_. Only root may.

*--keep-running* [*on* | *off* | *default*]
	Say, or set, whether your jobs run while you are signed out. *on*
	lets them, *off* does not, and *default* goes back to the system's
	setting, which is off unless the system's default says otherwise (the
	*server* preset turns it on). Without a word, it says what it is. An
	administrator can bar it for your account (*user*(8)); crontab then says so, and
	your jobs run only while you are signed in. A job that runs
	signed out has no access to your keyring, which needs your password.

# EXIT STATUS

0, or 1 when timerd refuses (a wrong line, a crontab that is not there,
a limit) or cannot be asked, or 2 when the command line is wrong.

# EXAMPLES

Install a crontab from a file, and look at it:

```
crontab mine.cron
crontab -l
crontab --status
```

Let your jobs run while you are signed out:

```
crontab --keep-running on
```

# SEE ALSO

*crontab*(5), *at*(1), *timerd*(8), *authd*(8), *user*(8), *systools*(1)

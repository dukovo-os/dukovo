crontab(5) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

crontab - the lines of a person's crontab

# DESCRIPTION

A crontab is text of lines, which *crontab*(1) gives to *timerd*(8)
(ADR 0478). It reads as cronie's does. A line is one of

- empty, or a comment, which starts with *#*;
- a variable, _NAME=value_, which holds for the lines after it;
- a job, a time and a command.

A text of more than 64 KiB, or of more than 500 jobs, or 100 variables, is
refused. A wrong line is refused with its line and column.

# JOBS

A job is five fields of time, and the command, which runs with
_/bin/sh_:

```
minute hour day-of-month month day-of-week command
```

Each field is a number, a range _a-b_, a list _a,b,c_, a step _\*/n_ or
_a-b/n_, or *\**. The months and the days of the week may be written as
three letters (*jan*, *mon*), and the day of the week as 0 to 7, where
both 0 and 7 are Sunday. When both day fields are limited, a day
matching either runs the job; when one starts with *\**, both must match.

A word takes the place of the five fields: *\@hourly*, *\@daily* (or
*\@midnight*), *\@weekly*, *\@monthly*, *\@yearly* (or *\@annually*), and
*\@reboot*, which runs the job once each time timerd starts, shortly after.

In the command, a *%* that no backslash precedes ends it: the text after
it is the job's standard input, and each further *%* makes a new line of
it.

# VARIABLES

_SHELL_
	Must be _/bin/sh_, which is the only shell.

_CRON_TZ_
	The time zone of the lines after it, a name of the time zone database
	(_Europe/Berlin_); the system's when left out.

_MAILTO_
	Where a job's output goes, when it has any: an address, which the
	mail settings (*sendmail*(8)) send it to. With none set, or when
	there is no server, the output is kept as *crontab --status* shows it.

Any other name is put in the job's environment, as is _HOME_, _LOGNAME_
and _USER_ of the person.

# SEE ALSO

*crontab*(1), *at*(1), *timerd*(8), *service.toml*(5)

at(1) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

at, atq, atrm, batch - run commands once, at a time

# SYNOPSIS

*at* [*-f* _file_] _time_++
*at* *-l* | *-c* _job_ | *-d* _job_...++
*atq*++
*atrm* _job_...++
*batch*

# DESCRIPTION

*at* reads commands from standard input, or from _file_, and queues them
to run once at _time_, as you, with _/bin/sh_ (ADR 0478). *timerd*(8)
keeps the queue and runs the job, with the environment and the working
folder that *at* had, except _TERM_, _DISPLAY_, _OLDPWD_, _SHLVL_ and
the underscore variable, which belong to the terminal and the shell. It
says "job N at _time_". If the time has passed when the machine starts,
the job runs then.

*atq* lists your jobs, one for each line: the number, the time, and
the owner. An administrator sees every job. *atrm* removes jobs.

*batch* does not run jobs: *at* jobs that wait for a low load are not
built (docs/deferred.md). It says so and exits with status 1.

# OPTIONS

*-f* _file_
	Read the commands from _file_.

*-l*
	As *atq*.

*-c* _job_
	Print the script of _job_, as it will run.

*-d* _job_...
	As *atrm*.

# TIME

The time is words, as at has always had them:

- *now*, *noon*, *midnight*, *teatime* (4 pm), a time of day as _HH:MM_,
  _HHMM_, or with *am* or *pm*;
- a date after it: *today*, *tomorrow*, a day of the week, a month name
  and day (*nov 14*, with a year), _MM/DD/YY_, _DD.MM.YY_ or
  _YYYY-MM-DD_;
- an offset: *+* _N_ *minutes*, *hours*, *days*, *weeks* or *months*, or
  *next* followed by one of those units.

*at 17:30 tomorrow* and *at now + 1 hour* are two. A time of day that has
passed today is tomorrow's. The time is of the system's time zone.

You queue 200 jobs at most, and a job's text is at most 128 KiB.

# EXIT STATUS

0, or 1 when the time is not understood, the job is not there, or timerd
refuses; 2 when the command line is wrong.

# SEE ALSO

*atq*(1), *atrm*(1), *batch*(1), *crontab*(1), *timerd*(8), *systools*(1)

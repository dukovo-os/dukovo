<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# Scheduled jobs: crontab and at

Two commands run your commands later, and a third decides whether they
run when you are not signed in. This chapter describes them (ADR 0478).
The references are crontab(1), crontab(5), at(1) and timerd(8). A
service that should start at set times has a timer instead: see
[Running services](services.md).

## A crontab

A crontab is your list of commands with the times they run, as on other
systems. Write it in a file and install it:

```
$ cat mine.cron
# Each weekday at 18:30
30 18 * * 1-5   tar czf ~/backup.tgz ~/documents
@reboot         echo started >> ~/starts
$ crontab mine.cron
$ crontab -l
```

`crontab -e` edits it with `$EDITOR` and checks it when you save. A line
that is wrong is refused with its line and column, and nothing is
installed:

```
$ printf '61 * * * * true\n' | crontab -
crontab: line 1, column 1: the minute is from 0 to 59, not 61
```

`crontab --status` says for each line when it runs next, when it last
ran, how it ended and the last of what it wrote. A job that wrote output
and has `MAILTO` set has it mailed to that address once a mail server is
set (`man sendmail`); otherwise the output is kept for `--status`.
`crontab -r` removes the crontab.

A job runs with `/bin/sh`, in your home folder, as you. If the machine
was off at the time, a line runs once soon after it starts, as `anacron`
would, so that a nightly job is not lost to a night the machine was off.

## One time: at

`at` runs commands once:

```
$ echo 'tar czf ~/once.tgz ~/documents' | at 17:30 tomorrow
job 3 at Sat Nov 14 17:30:00 2026
$ atq
3       Sat Nov 14 17:30:00 2026        alice
$ atrm 3
```

The time is in words (`now + 2 hours`, `teatime tomorrow`, `next week`) or as
a date (`2026-12-01`, `dec 1`). The job runs in the folder where you
queued it, with the environment you had, except the terminal's. `at -c 3`
shows the script of job 3. `batch`, which runs a job when the load is low,
does not exist yet.

## While you are signed out

A job of yours that comes when you are signed out does not run, unless you
said it may. Off is the system's default; the `server` preset turns it on.
This keeps a person's commands from running on a shared computer once
they have gone, and it keeps their keyring, which opens with their
password, shut:

```
$ crontab --keep-running
Your jobs do not run while you are signed out. That is the system's default, off.
$ crontab --keep-running on
Your jobs run while you are signed out. You turned it on.
```

A job that runs signed out has no keyring. A job that did not run for
this reason is counted, and your next sign-in says how many.
An administrator can forbid it for an account with
`elevate user set --jobs-signed-out never NAME`, and take the bar away
with `allowed`. `crontab --keep-running` then says it is barred.

An administrator reads the crontab of anyone with `crontab -u NAME -l`,
and root's own crontab runs for as long as the machine does.

## What the machine keeps

`timerd` keeps crontabs and queued jobs in `/system/state/timerd`, so they
survive a restart, and runs them. `log timerd` says what ran. A crontab
holds up to 500 lines; you may queue 200 `at` jobs.

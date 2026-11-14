<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0478. timerd starts services and runs a person's jobs at their times, and logturn turns over a service's logs

- Status: Accepted
- Date: 2026-11-14

## Context

Nothing on Dukovo ran at a time. A backup, a clean-up or a certificate
renewal needed a service that slept in a loop (updated and backup each
kept a clock of their own), a person had no `crontab` or `at`, and a log
file a service wrote itself, such as nginx's `access_log` to a file
(ADR 0491), grew until the disk was full, since nothing turned it over.
Three rows of `docs/deferred.md` waited on the same thing: timers for
services (ADR 0216), a person's jobs while they are signed out (ADR
0220), and the turnover of a service's own logs (ADR 0491).

systemd has timer units, with `OnCalendar=`, `Persistent=` and
`RandomizedDelaySec=`; Linux has cron, anacron and `at`; and logrotate
turns logs over by a file of its own and runs from a cron job.

## Decision

1. **A service file may have a `[timer]`.** `at` is a list of calendars,
   which are a crontab line's five fields or one of `hourly`, `daily`,
   `weekly`, `monthly` and `yearly`; `every` is a duration of `w`, `d`,
   `h`, `m` and `s` (one second to 366 days) after the last start;
   `after_boot` makes the first start of an `every` timer that long after
   the machine starts; `spread` puts a time off by up to that much;
   `catch_up` (true unless it says otherwise) makes a time missed while
   the machine was off run once, 20 seconds after timerd starts; and
   `time_zone` is a name of the time zone database. A timer needs `at` or
   `every`. The manifest's `service` of a bundle has the same table.
2. **timerd is a service of its own, not part of Spark.** Calendar
   arithmetic needs the time zone database, which a program with the C
   library reads and Spark does not, and a mistake in it stops one
   timer, not the machine. timerd keeps the times of every enabled
   service with a timer, and asks Spark for a start with `start NAME
   --by timer`, which Spark treats as any other start: it checks what the
   service needs and runs it as its file says. A timer does not enable a
   service, and boot leaves a service with a timer off between its times,
   as `service enable` does, which has timerd read the files at once
   rather than at its next minute's reading. One that is starting or
   running when its time comes is skipped, and the log says so.
3. **The clock may move.** timerd sleeps until the next time, a minute at
   most, and compares the clock with the monotonic clock on each wake,
   with 5 seconds of leeway. A clock that moved forward leaves the times
   it passed due, and each timer runs once for them, however many it
   passed; one that moved back runs no time that already ran again, and
   each timer's next time is worked out from its last run. The minute
   between readings of the files is the monotonic clock's, which a clock
   set back does not stretch. `spread` is
   worked out from the timer's name and a random number timerd keeps in
   `/system/state/timerd/seed`: the machine has no identifier of its own
   to use, and the number keeps a timer's delay the same for that
   machine and different from other machines'.
4. **A person has a crontab and `at` jobs.** `crontab` reads cronie's
   format (`NAME=value` lines, `@` words, `@reboot`, `%` for input, and
   `CRON_TZ`, `MAILTO` and `SHELL=/bin/sh`), refuses a wrong line with its
   line and column, and keeps the text with timerd, so the file is
   not the person's to edit. `at` takes the time in words and dates as
   `at` on Linux does, keeps the environment and the folder, and
   `atq` and `atrm` list and remove the jobs; `batch`, which waits for a
   low load, is not built (`docs/deferred.md`). Who asks is the user the
   kernel gives for the connection to timerd's socket, so a person reads
   and changes their own jobs, and an administrator anyone's. The limits
   are 500 lines and 64 KiB for a crontab, 200 `at` jobs for a person and
   1000 for the machine, and the last 64 KiB of a job's output.
5. **authd runs a person's job, as the person.** timerd holds no way to
   become someone: it sends authd a `job` request with the job's owner,
   folder, command and environment, and one descriptor for its output.
   authd starts `sh -c` with the command as it starts a session's first
   program, as the person and their groups, in their view with their own
   `/tmp` and a session's network, in the job's folder, with the null
   device as its input and the descriptor as its output and error, and
   says when it ends on the same connection, as for a session. A job of
   a person who is not signed in at a console or over SSH runs only if
   they said it may (`crontab --keep-running on`), or chose nothing and
   the system's default says so, and no administrator barred it
   (`user set --jobs-signed-out never NAME`). The default is off unless
   `/system/settings/jobs.toml` holds `keep_running = true`, which the
   `server` preset writes. Such a job has no keyring, which opens with the
   person's password. authd keeps each person's choice, the bar and a
   count of the jobs that did not run while they were signed out in
   `/system/state/authd/jobs`, by their number, and their next sign-in
   says how many did not. Root's crontab is run for root.
6. **A service may have `[logs]`, and logturn turns the files over.**
   `files` are patterns in the service's data folder, `turn_over` is
   `daily`, `weekly` or a size, `keep` is 1 to 100 (14), `compress` is
   true unless it says otherwise, and `reopen` is a signal, `restart` or
   `none`. The system's own timer starts `logturn` each hour. For a file
   that is due it shifts the turned files up and removes those past
   `keep`; renames `x.log` to `x.log.1`, which keeps its owner and mode
   and loses no line, since a service that has the file open writes on
   into it; asks Spark to `reopen NAME` once for each service, which sends
   the signal or restarts it; and compresses the files from `x.log.2` on
   with zstd, never `x.log.1` while the service may still write to it,
   keeping each file's owner, mode and times. nginx's bundle names
   `logs/*.log` and `SIGUSR1`.
7. **The commands an administrator knows from other Linux servers
   answer in their own words.** `systemctl` is service(8) with
   systemd's verbs, words and exit statuses: `name.service` is the
   service, `name.timer` the timer in its file, and a verb or a kind of
   unit Dukovo does not have says what to run instead. `journalctl` is
   log(1) with journalctl's options and defaults (`-u`, `-n`, `-p`,
   `-k`, `--since`, `-o json`). `hostnamectl` shows and sets the
   machine's name, with the release, the kernel and the architecture.
   `sendmail` sends a message through the mail server alerts.toml(5)
   names, as cron's `MAILTO` and scripts expect, and ends with
   sendmail's statuses: 64 without a recipient, 78 with no server, 75
   when the server should be tried again.
8. **The boot tests `timers`, `crontab`, `log-turnover` and
   `admin-commands`, and the lab tests of the same names, cover them,**
   and the boot test `nginx` turns over an `access_log`.

## What it is not

- No `batch`: jobs that wait for a low load are a row of
  `docs/deferred.md`.
- No timer for updated's or backup's own schedules yet: they keep their
  clocks (`docs/deferred.md`).
- No calendar events as systemd writes them (`Mon..Fri 09:00`), no
  monotonic timers other than `every` and `after_boot`, and no timer that
  starts when another service ends.
- No mail of a job's output without a mail server: it is kept for
  `crontab --status`.

## Consequences

- A person's job needs no daemon of their own, and a service's schedule is
  in its own file, next to what it runs.
- A clock set back never runs a job again, and one set forward runs it once;
  a machine that was off runs the missed time once, soon after it starts.
- A log file a service writes itself is kept to `keep` turned files, the
  older compressed, and an `access_log` of nginx's no longer grows
  without end.
- authd holds the setting for a person's jobs, so the only code that
  starts a program as a person is the code that starts their sessions.

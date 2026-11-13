<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0471. The service command shows what a service does, and waits for what it asked

- Status: Accepted
- Date: 2026-11-13

## Context

ADR 0470 gave services an order of their own: a service waits for what
it requires, is ready when its file says, and is stopped with what it
requires. The `service` command still sent a request and returned:
`service web start` printed nothing on success and came back while web
was still waiting for its database, so a script went on before the
service was there, and a person learnt that the start had failed only
by asking again. `service NAME status` said what Spark knew of the
service's state, but not what it may reach, which processes it runs,
who stopped it, or what it wrote before it ended; and `log -s NAME`
showed the service's own lines without Spark's lines about it, so a
start that never happened left nothing under the service's name.

systemd's `systemctl start` waits for the start job, silently, and on a
failure says "see systemctl status and journalctl -xeu". Its `status`
shows the unit's processes and last lines, but not what the unit may
reach.

## Decision

- **A start, stop or restart waits** until the service has settled, as
  systemctl's does, and shows Spark's progress as it comes. The request
  carries `--wait` (spark(8)): Spark answers it as before, keeps the
  connection open as a watch, writes to it each of its log lines about
  the services the request reaches, word for word, and ends with a last
  line, `ok:` or `error:`. A start reaches the service and every
  service it requires, wants or starts after, at any depth; a stop, the
  service and every service that requires it.
- **Settled** is, for a start or restart, the service ready, a setup
  step done, or a program that ran and ended well; or a start of it or
  of a service it waits for that failed, was refused or was given up
  on, which the last line names, with the last line the failed program
  wrote and when Spark tries again. For a stop, it is the service and
  those stopped with it ended. Each comes within the ready timeouts of
  ADR 0470 and the 5 s stop wait, so a wait ends; 15 minutes is a guard.
- **A watch never holds Spark up.** Spark stays one thread: a watch is
  a connection in its poll loop, written without blocking, and its
  lines are appended as Spark says them. At most four watches run at
  once, within the eight connections, so that other requests always
  find room; a fifth is answered without one and says so. A watch whose
  reader has gone, or has left 64 KiB unread, is closed, and the
  request goes on.
- **`service`** prints the lines without `spark: `, exits 0 when the
  service settled as asked and 1 with the reason when it did not.
  Ctrl-C ends the wait, not the request, and says how to follow it;
  `--no-wait` (systemctl's `--no-block`) returns at once, and `-q`
  prints only a failure.
- **`service list`** is a table of every service: its state, with
  `health failing` where a running service's check fails, whether it
  starts at boot, and since when; under it, the services not running as
  asked.
  `--failed` lists those alone and exits 1 when there are some;
  `--json` prints Spark's `show` document. A service that is off as it
  is meant to be, a driver the kernel left none of its cards or one
  Rescue or essentials leave off, is not among them: `show` marks it
  `off_as_meant`, so a check on `--failed` stays quiet on a healthy
  machine.
- **`service NAME status`** adds when each state began and how long it
  took to be ready; who last asked to start, stop or restart it; the
  last line its output wrote before it ended; what it holds (its own
  view or the whole tree, the account it acts for, the places it reads
  and changes, its secrets, its ports with those open now, the
  machine's settings it may set, its cards, its memory limit and CPU
  weight); its processes, their memory and CPU time; and its last
  lines in the log, five unless `-n` says otherwise, left out when the
  log does not answer.
- **`service NAME explain`** says why a service is in its state, in
  one sentence, and what to do. It follows what the service waits for,
  or what stopped it, and then what that waits for or was stopped by,
  up to 16 services, to the one at the root, and says what each is
  doing in Spark's own words (`why`, `cause`, `state`). It ends with
  the fix: what to repair at the root, and the start that brings the
  service back with it. The sentence is made from the `show` document
  alone, in spark-api, so the web console says what the command says;
  `--json` prints it with the chain and the root. It exits 0 for a
  service that is done or runs with its health check not failing, and
  3 for any other. `show` gains the fields it needs: `retry_ms`,
  `ready_by_ms` and the health check's `restart_after`.
- **`show`** gains the fields that need: `cause` (the service a `why`
  names), `last_line`, `last_request` and `holds`. They are added
  without a new `show_version`, as spark(8) lets a client expect.
- **`service NAME logs`** is `log -s NAME`, with log(1)'s options; and
  `log -s NAME`, as journalctl's `-u`, takes Spark's lines that name the
  service as a word as well as the service's own.
- **`service check` says where a file is wrong, and what putting it to
  use would do.** A file that does not read is shown as a compiler shows
  a mistake: `PATH:LINE:COLUMN: message`, the line with carets under
  the setting, and `= fix:` with what to do: the nearest setting's name
  for a misspelt one, the template's line for a value of the wrong kind,
  the setting to change for one that another needs. A mistake in an
  override file names the override's line and the file it is set over.
  `--brief` prints the first line alone, with the fix after it. For a
  file that reads, the settings that differ from the copy Spark read
  last (`/system/state/spark/good`, which only root reads) and the
  `show` document go through `spark_api::dry_run`, which says whether
  the service restarts, what starts with it and what keeps running; or,
  for a service that does not run, when the change applies and whether
  the service would start. It is made in spark-api, so the web console
  says what the command says. `service edit` and `service try` check
  the copy the same way and say it before the copy takes the file's
  place.

## Consequences

- A script's `service web start` returns once web is there, or fails
  with the reason, where systemctl leaves the reason to `systemctl
  status` and the journal. The lines a person sees are the log's, so
  the two never disagree.
- Every caller of `service NAME start` now waits. The system's own
  callers that only mean to start or stop a service in passing, reader
  and braille, pass `--quiet`; tests that look at a service on its way
  pass `--no-wait`.
- A person who asks why a service is not running gets the answer in one
  command, through every service between it and the cause, where
  systemd leaves the chain to `systemctl list-dependencies` and the
  journal. The console and the command cannot disagree: the words are
  made once.
- A watch costs Spark a buffer and a check each turn of its loop; four
  at most keep that small, and a fifth caller still has its service
  started.
- Spark's log lines are now an interface: the watch and `log -s` carry
  them to people, and tests read them. Their words change only with
  their docs and tests. The `why` texts are an interface too: `explain`
  reads them to tell a service stopped by a request from one that ended.
- `/processes` scoped to a service stays with ADR 0473; status counts a
  service's processes by their domain, as the kernel names it.
- A person who changes a file learns where it is wrong and what to write,
  and what the change does before it does it: `systemd-analyze verify`
  gives a line and no fix, and systemd never says what `daemon-reload`
  and a restart would change. Only root sees the second, as only root
  reads Spark's copy; others get the check alone, and are told so.
- The `service-graph` boot test starts services through waits, a
  failing one among them, runs four waits at once with a fifth start
  beside them, and ends one wait with Ctrl-C, and checks what `explain`
  says of each state it makes, and what `check` says of a file with a
  mistake, of a change to a service that runs and of one that does not;
  the lab test `services/command` does the same with nginx from the
  catalog on the lab's machines, `services/explain` follows a
  requirement to a service Spark gave up on, and `services/check` says
  where a file and its override are wrong.

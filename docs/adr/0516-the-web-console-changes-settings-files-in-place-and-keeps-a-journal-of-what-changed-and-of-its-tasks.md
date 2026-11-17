<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0516. The web console changes settings files in place, and keeps a journal of what changed and of its tasks

- Status: Accepted
- Date: 2026-11-14

## Context

ADR 0515 gives the console its actions: each runs a command, as the person
or as an administrator, and answers when the command ends. Three things are
still missing before the console's pages can manage a machine.

First, Dukovo's settings are text files that people read and edit by hand,
in `/system/settings` and in each bundle service's folder. A console that
kept the settings in a database of its own, and wrote the files from it,
would have two truths that drift: a comment a person wrote would vanish the
next time the console wrote the file, and an edit made at a terminal would
be lost, or fought over. The files have to stay the truth, and a change the
console makes has to be the smallest edit a careful person would make.

Second, a machine changes in more than one way. A person changes a setting
in the console, another edits a file over SSH, a script with an API token
restarts a service. When something stops working, the first question is
what changed, when and by whom, and the next is how to put it back.
Dukovo has no notification of a file's change, so the console cannot be
told of an edit; it has to look.

Third, some actions take minutes or hours: an update, a backup of a whole
machine, a certificate from an authority over a slow line. A request
cannot wait that long, and a person closes the browser, or loses the
network, long before. The work must go on without the page, be seen from
another browser, and be stoppable.

A person also has a few preferences of their own (colours, the size of
the text, whether things move, the time zone the console shows times in),
which belong to them, not to the machine.

## Decision

- **A change edits the file in place, keeping what a person wrote.**
  `settings_edit::change` reads a TOML file as a document (`toml_edit`,
  the `toml` crate's sibling), lets the action change values, and writes
  it back: every comment, blank line and order of keys stays, a replaced
  value keeps the comment after it, and a new key goes after the others of
  its table. The whole file is written as one: the new text goes into a
  new file beside it, with the old file's owner and mode, is made
  durable, and takes the old file's name in one rename, and the folder is
  made durable too, so a reader sees the old file or the new one and a
  power cut leaves one of the two. A change that leaves the text as it
  was writes nothing, so that the journal and a second run of the same
  change see no change. A file that is not TOML is refused, with its
  line, and left as it is. `console.reach` (whom the console answers,
  `private` or `any`, in `/system/settings/admin.toml`, through `admin
  reach`) is the first action that edits a settings file.
- **One store, admind's.** The journal, the versions of the files it
  names and the tasks are in one SQLite file (ADR 0511),
  `/system/state/admind/store.db`, mode 0600, with a rollback journal
  rather than a write-ahead log, so that `admin put-back`, which reads it
  as root in a process of its own, needs nothing but the file. admind is
  its only writer. Its version is SQLite's `user_version`; admind brings
  an older store up to date and refuses a newer one. When the store
  cannot be opened, the console still changes things, and the journal's
  and the tasks' routes answer ADM-126.
- **A console change has a row in the journal.** When a change's command
  has ended, worked or not, the journal has a row: when, who, how
  (`console`, or `token NAME` for an API token), the action and its
  command, its exit status, the task it ran as, and each file the action
  names, with what it was before and after, by SHA-256. The text of each
  side is kept as a version, so that the page shows a diff (unified, at
  most 2,000 lines a change) and the change can be put back. A command
  that was not run, because authd or the person's rights refused it,
  changed nothing and has no row; the log says it.
- **A change made by hand has a row too.** Every 30 seconds admind
  compares the size and modification time of each file under
  `/system/settings` (8 folders deep) and in each folder an action's files
  are in (4 deep), at most 10,000 files, with what they were at its last
  look, and reads and hashes only those that differ: a few hundred stats
  a round. A file made, changed or removed with no console change under
  way on it is a `by hand` row. The person is found in authd's audit log:
  the person whose elevated command named the file's path, as a whole
  word, in the 10 minutes before; the rest are "outside the console".
  The account files, `/system/settings/passwd` and `group`, which authd
  writes, are a row of their own. Names that begin with a dot, an
  editor's file on its way to its name, are not looked at.
- **Everyone signed in reads the journal; no one reads a secret in it.**
  The journal names people and files, not the contents of secrets. A file
  under a service's `secrets`, one that not everyone may read, and one in
  a folder that others may not pass through, is listed by name, with no
  text, hash or diff. admind runs as root, so it finds each file from the
  root a part at a time and never through a link, opens it only once it
  knows it is a regular file, and does not watch links: a link that a
  service's account puts in its own folder leads it nowhere. A file
  longer than 1 MiB is listed by its hash and size. The store keeps the
  last 20 versions of a file and 64 MiB of text in all, the oldest going
  first, and the newest 10,000 rows. API tokens do not read the journal.
- **Keep it, and Put back.** Keep it marks a hand edit seen by the person,
  from an unlocked session; it changes nothing on the machine. Put back
  writes back the earlier text of each file a change changed, as root,
  through `console-elevate` of `admin put-back ID`, so the command the page
  shows is the one that runs, and it has a row of its own, which can be
  put back in turn. It needs an administrator's unlocked session, as an
  administrator's action does. It writes with `settings_edit::write`,
  says when a file was changed again after the change and that the later
  text is gone, and asks a bundle service whose folder holds the file to
  reload. A Spark service file is only written: Spark reads it when the
  service next starts, as after `service NAME edit`. It refuses a file the
  change made, a secret, a file longer than 1 MiB, a version the store
  has forgotten and the account files, and says why. authd writes the
  account files together with each account's password, home and sharing
  password, so a list written back alone could take an account away, or
  an administrator's group, and leave the rest of it behind: accounts
  change through `user`.
- **A long action is a task.** An action marked long answers 202
  `{"task": ID}` at once. admind runs its command as an administrator
  through `console-elevate`, without the 30 seconds a short command gets,
  and keeps the task in the store: `waiting` for one of the 32 places
  that elevated commands share, `running`, then `done`, `failed` or
  `stopped`. At most 64 tasks wait; one more is ADM-105, with
  `Retry-After`. A long action runs as an administrator only, since a
  person's worker does not pass on what a command writes as it writes it.
  The command's lines `step N of M: WORDS` and `progress P%` are the task's
  step and bar; every other line is its log, of which the last 2,000
  lines, 256 KiB and 1,024 bytes a line are kept. `event: task` on the
  event stream tells each tab of a change, at most four a second a task,
  and a tab that opens is told where each live task is. A task may run 12
  hours. Its person or an administrator stops it, from a session locked
  or not, or a token that may change its area and whose owner is one of
  them: the command gets SIGTERM, as from ^C, and puts back what it did;
  30 seconds later it is hung up, and 10 seconds after that let go. When
  admind starts it ends the tasks the last admind left as `stopped`, "the
  console restarted", since authd hangs up an admind's commands when it
  goes. A task is kept 30 days; the journal keeps its change, with the
  task's number, when it ends. A task that is `done` says whether it
  changed anything, as a short change's answer does (ADR 0515): `changed`
  is false only when its action names files and each reads the same after
  it as before. A task runs no probe, so one whose action names no files
  always says true.
- **A person's preferences are their own file.** `GET` and `POST
  /api/1/account/preferences` read and change `theme` (`dark`, `light`,
  `high-contrast` or `device`), `text` (100, 112, 125 or 150 percent),
  `motion` (`full` or `less`) and `zone` (a time zone of the machine's
  database, or empty), in `~/.config/dukovo-admin/settings.toml`, mode
  0600 in a folder 0700, which the person's worker writes as them through
  `settings_edit`. A key that is missing or wrong reads as its default
  (dark, 100, full, none), so a typo costs a preference and never the
  page.
- **`admin export DIR` copies the settings into Git.** Run as the person,
  it copies the files the round watches under `/system/settings` into
  `DIR` at their machine paths, as a Git repository, one commit an export,
  "Settings of NAME at TIME", and none when nothing changed. Files the
  person may not read, and secrets, are left out by name in `SECRETS.md`,
  so a person keeps the machine's settings history in their own Git.
- **Errors.** ADM-126 is no store (503), ADM-127 a change or task that is
  not there (404), and ADM-128 something the console cannot do with it
  (409), such as a second stop of a task that has ended, or Keep it on a
  change made through the console.

## Consequences

- A person who edits a file by hand sees the change on the console within
  30 seconds, with its diff, and can put it back. A file changed and
  changed back between two looks is not seen, and two changes between two
  looks are one row.
- Who made a hand edit is the audit log's best answer, not a proof: an
  administrator's shell that edits a file without naming it on the command
  line is "outside the console", and a person whose elevated command named
  the file is taken for a write by root soon after. The audit line is
  authd's as the kernel's log keeps it, its first 200 bytes, so a path that
  a long command names after them is not seen.
- `settings_edit::change` reads, edits and writes; what another program
  writes to the file in between is lost. The console's changes are small
  and quick, and the journal shows the result.
- Put back puts back text, not a machine's running state. Putting back a
  service's enable leaves the service as it runs until it next starts; the
  action that undoes an action (ADR 0515's examples) is the way to change
  what runs.
- Up to 64 MiB of settings history and up to 30 days of tasks live in one
  file in `/system/state`. A whole-machine backup carries the settings,
  not their history: a machine restored from one starts its journal
  again.
- The tests of tasks need a long action, and no action people use is long
  yet. A lab build has one, `lab.long`, which runs `admin lab-steps`;
  the lab's images are lab builds, and the boot test images and releases
  are not, so the boot test `admin-journal` runs its task cases only where
  the build has it, and the lab test `web-console/tasks` always does.

## Alternatives considered

- **The settings in a database, written out to files.** Two truths, and
  every comment a person writes lost at the console's next write.
- **A Git repository of `/system/settings` kept by the console.** Git is
  not on every machine, a repository in the settings is one more thing to
  back up and to keep secrets out of, and it records only what is
  committed. The journal sees every change, and `admin export` gives a
  person Git when they want it.
- **Notification of file changes.** Dukovo has none. A round of stats every
  30 seconds costs well under a millisecond on a machine's few hundred
  settings files.
- **Tasks in the person's worker.** A worker ends with its session, and a
  long command must outlive the browser that asked for it.
- **A write-ahead log for the store.** A reader in another process then
  needs the log's shared memory file as well, and `admin put-back` reads
  the store as root while admind writes it; a rollback journal needs
  only the file, and the store's writes are few.

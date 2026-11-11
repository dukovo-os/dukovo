<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0492. Valkey is a service bundle, on a poll event loop, with its data in its data folder

- Status: Accepted
- Date: 2026-11-11

## Context

Valkey, the open-source continuation of Redis, is the second of the 1.0
server services in the Phase 16 plan, after nginx (ADR 0491), whose
service bundle it follows. Its event loop has a back end for epoll,
kqueue and event ports, which Radix does not have, and falls back to
`select`, which holds only `FD_SETSIZE` descriptors: too few for
Valkey's 10,000 clients. Its Lua scripting is a module built into the
server, which it finds with `dlopen(NULL)` and `dlsym`, which find no
symbols in a static program. It allocates with jemalloc by default, and
reads the time zone's offset for its log from `gettimeofday`, whose
time-zone argument the C library leaves alone. Its tools,
`valkey-check-rdb` and `valkey-check-aof`, are the server itself run by
another name.

## Decision

- **A port of the catalog.** `ports/valkey` builds Valkey's stable
  series for Radix as a POSIX system, with the C library's allocator,
  without systemd, and with TLS from OpenSSL 3.5's libraries, which the
  openssl port keeps for bundles (ADR 0491). Three patches fit it to
  Radix:
  - an event loop on `poll(2)`, `ae_poll.c`, which Valkey uses where
    the system has none of its other back ends: its descriptors are
    kept packed in the array `poll` takes, with each one's place kept
    by descriptor, so that adding, changing and removing one takes
    constant time, and it holds as many as Valkey's limit;
  - static modules found in a table the server keeps rather than with
    `dlsym`, so its Lua scripting loads;
  - the time zone's offset from `tzset`, as on Linux.
- **Commands that are another program's.** A recipe's `aliases` name
  commands that run another of the bundle's programs by their own name,
  which ob writes into the manifest as commands of that program; a
  bundle's command already gets its own name as its first argument.
  `valkey-check-rdb` and `valkey-check-aof` are `valkey-server`'s.
  `valkey-cli` and `valkey-benchmark` are commands of their own; all
  four run with the access of whoever runs them.
- **The service.** Valkey's service runs as the account valkey once an
  administrator enables it, is started again if it fails, listens on
  port 6379, and may connect out to copy a primary server's data, as a
  replica. It has no memory limit: its data is in memory, and how much
  it may hold is the administrator's choice, with `maxmemory`.
  `valkey-service` copies the bundle's settings into the data folder,
  as files of the account's, as nginx's does, and keeps
  `valkey-server` in the foreground.
- **Its settings and data are in its data folder.** `valkey.conf` is in
  its `conf` folder and its snapshot and append-only files in its
  `data` folder. The bundle's settings answer on the machine itself
  only, in protected mode, take a snapshot after an hour if a key
  changed, five minutes if 100 did and a minute if 10,000 did, and keep
  the append-only file off, as Valkey's own do; each line says how to
  change it, and every setting is in the bundle's `doc/valkey.conf`.
  Valkey reads its settings only as it starts, so an administrator
  restarts it, and it saves its data as it stops.
- **Its log is the system's.** Valkey writes its log to its standard
  output (`logfile ""`), which the system's log keeps under its name.

## Consequences

- An administrator runs Valkey with `elevate bundle install
  --for-everyone io.valkey.valkey` and `elevate service valkey
  enable`, and programs on the machine reach it at `127.0.0.1:6379`.
  The bundle is about 8 MB: each of its three programs links OpenSSL.
- The boot test `valkey` installs the bundle from its `.dkv` file,
  enables it, and checks that `valkey-cli` reaches it; that keys saved,
  and a key written after the save, are there after a restart; that
  with the append-only file turned on a key written after it is there
  after another restart; that `valkey-check-rdb` and `valkey-check-aof`
  find its files sound; that `valkey-benchmark` runs with its 50
  clients, which connect at once (ADR 0494); its log; and that its
  files are the account's. The lab test `services/valkey` has the
  lab PC, with a client of its own in Python, store 40,000 keys over 16
  connections and find them all after a restart.
- Valkey's active defragmentation, which needs jemalloc, is off.
- `MODULE LOAD` loads no module from a file, as Valkey's programs are
  static.
- **Not yet** (docs/deferred.md):
  - A Unix socket that other bundles' services, such as PHP-FPM, reach:
    a service's data folder is its account's alone, so they reach
    Valkey over TCP on the machine itself.
  - Valkey on the kernel's event queue, and with jemalloc.
  - Valkey's own test suite, run against the machine from the lab PC.

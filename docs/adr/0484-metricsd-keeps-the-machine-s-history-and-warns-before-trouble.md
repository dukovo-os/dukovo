<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0484. metricsd keeps the machine's history and warns before trouble

- Status: Accepted
- Date: 2026-11-13

## Context

When a server misbehaves, the first questions are about its recent
past. When did the disk start filling? Was the CPU busy last night?
Which service grew its memory? Is a RAID volume running on one disk?
`top` and `df` answer for now only.

Each panel keeps a history:

- Cockpit records metrics through PCP, with pmlogger keeping a day by
  default.
- Webmin keeps a few graphs through its System and Server Status module.
- Proxmox VE keeps round-robin databases (RRD), with a year at coarse
  steps.
- TrueNAS uses Netdata, and before that collectd with RRD.

Each also warns of a full disk or a degraded pool, and most can be
scraped by Prometheus. node_exporter's metric names are what Grafana's
dashboards expect. A Linux server without a panel has none of this
until someone installs and tunes several tools.

The web console (Phase 16, thread C) needs charts of the past to draw.
The alerts of ADR 0485 need someone to notice trouble while there is
still time to act: a disk at 93% is a warning, and at 100% it is lost
writes.

## Decision

- **A service, metricsd, samples the machine each minute** (`metricsd
  run`, step 60 s):
  - CPU, load, memory and swap;
  - each disk's reads, writes and busy time;
  - each filesystem's use;
  - each network card's traffic and errors;
  - the temperatures;
  - each service's CPU and memory, summed over its resource domain;
  - each mounted Btrfs volume's missing and lagging disks, from
    `/kernel/fs/btrfs/<uuid>/`, which the kernel now shows as Linux's
    `/sys/fs/btrfs` does.

  A series is `metric{instance}`, at most 1024 of them. Rates come from
  two samples over the time between them. A counter that went back, as
  when a disk is replaced, gives a missing value, never a negative one.
- **The history is kept in append-only files** in
  `/system/state/metrics`:
  - one file a UTC day of a row a step, kept two weeks;
  - one file a UTC month of each hour's average, minimum and maximum,
    kept 13 months.

  Each record carries its kind, length and a CRC-32. A names record
  sets the columns of the rows after it, and another follows when a
  series is added. Rows are written every ten steps with one write and a
  sync. A power cut costs at most those ten minutes. The file is cut
  back to its last whole record at the next start.
  - Appending suits Btrfs: copy on write never rewrites an old extent,
    and snapshots go on sharing them.
  - About 2 MB a day for a typical server, and under 40 MB in all.
  - `metricsd::store::read` is how the `metrics` command and the web
    console read it.
- **The latest values are in `/system/live/metrics/now`**, a `name
  value` line each. `metrics` shows them, lists the series and prints
  any series over any range.
- **Prometheus can scrape the machine with node_exporter's names.**
  - It is off until metrics.toml(5) gives `listen` addresses and the
    `allow` networks.
  - It serves plain HTTP on port 9100, as node_exporter does, to the
    allowed networks alone, closing others at once.
  - Values are read fresh at each scrape, with `dukovo_` series for
    services, Btrfs volumes and active alerts.
- **Early warnings go to alertd** as one `set` after a step whose set
  of problems changed, and every 10 minutes in any case:

  | Key | Raised when | Severity |
  | --- | --- | --- |
  | `fs-full:MOUNT` | 90% used | warning; critical at 97% |
  | `fs-filling:MOUNT` | the last 6 hours' trend fills it within 3 days | warning |
  | `btrfs-missing:VOLUME` | a disk is missing or failed | critical |
  | `btrfs-behind:VOLUME` | a disk missed changes | warning |
  | `memory:low` | under 5% available for 5 minutes | warning |
  | `temp:SENSOR` | within 5 °C of its critical point for 5 minutes | warning; critical at it |
  | `certificate:FILE` | a certificate in `/system/settings/ssl` ends within 14 days | warning; critical within 3 |

  - Each clears with a margin (below 87% for `fs-full`, for example), so
    a value on the line does not flap.
  - The thresholds are metrics.toml's `[warnings]`.
  - A title names the thing and the number: "/data is 93% full (1.2 TB
    of 1.3 TB)". The detail names the command that helps.
- metricsd is memory-limited to 32 MiB, and its own `service.cpu`
  series shows its cost.

## Consequences

- From its first boot a server keeps a year of its own history, and an
  owner who sets an email in alerts.toml hears of a filling or degraded
  disk before it costs data.
- Grafana dashboards made for node_exporter work unchanged.
- The web console's charts read the store directly, with nothing to
  install.
- The store's format is Dukovo's own. It is read through
  `metricsd::store::read`, and `metrics --since … --json` exports it.
- Not yet, each a row of docs/deferred.md: TLS and authentication on the
  Prometheus endpoint. It serves only the networks `allow` names, as most
  node_exporter setups do.
- Tests:
  - **Units:**
    - the store's records, torn and damaged files, the hours, keeping,
      and reading ranges;
    - each source's parsing;
    - each warning with its hysteresis;
    - the Prometheus text and its access rule.
  - **The `monitoring` boot test:** metricsd at a 1-second step, a
    filesystem filled past the line, the alert at alertd and by email,
    the endpoint scraped, and metricsd's own CPU under 1% of a core.
  - **The lab's tests** read a real machine's sensors and disks.

## Alternatives considered

- **PCP, collectd or Netdata as a port.** Each is a large C program
  with its own daemons and formats, tuned for Linux's /proc. metricsd
  reads Dukovo's own files, in a few hundred lines a source.
- **RRD files.** They are rewritten in place at each step. On Btrfs that
  fragments them and makes each snapshot keep a copy.
- **SQLite.** It is a dependency, and needs care with sync and WAL for
  what is a time series that is only ever appended.
- **Keeping history in memory alone.** It is lost at each restart,
  which is when it is most needed.

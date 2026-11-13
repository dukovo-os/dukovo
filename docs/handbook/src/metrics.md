<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# Metrics and early warnings

This chapter describes what the machine keeps of its own state, how to
look at it, the warnings that come before trouble turns into lost data,
and how to let a Prometheus server read it. The full references are
metricsd(8), metrics(1) and metrics.toml(5).

The `metricsd` service reads the machine once a minute: the processors,
the load, memory and swap, each disk's reads and writes, each
filesystem's use, each network card, the temperatures, what each
service uses, and each Btrfs volume's missing disks. It keeps what it
reads, and tells `alertd` of a problem it sees coming. Nothing has to be
installed or switched on, and nothing leaves the machine unless a
Prometheus server is allowed to ask.

## What is kept

| What | Where | Kept |
| --- | --- | --- |
| A reading a minute, of every series | `/system/state/metrics/minutes`, a file a day | Two weeks |
| Each hour's average, least and greatest | `/system/state/metrics/hours`, a file a month | 13 months |
| The newest reading, as text | `/system/live/metrics/now` | Until the next minute; a boot starts without it |

A machine with 100 series, which is a few disks, cards, sensors and
services, takes about 20 MB for all of it; at most 1024 series are kept.
The files are only ever added to, so Btrfs snapshots share them, and a
power cut loses the last ten minutes at most. A machine that was off for
a month keeps what it had.

A series is a metric and, for what there are several of, an instance in
braces: `cpu`, `memory.used`, `fs.used{/volumes/data}`, `net.rx{net0}`,
`temp{coretemp-isa-0000/Package id 0}`. `metrics list` shows what there is, and
metricsd(8) says what each one means.

## Looking at the machine

`metrics` shows the latest values, the minute's reading, with their
units:

```
$ metrics cpu load memory.available fs.used
cpu                     4.2%
load                    0.31
memory.available        5.8 GB
fs.used{/volumes/sda3}  23 GB
fs.used{/data}          1.2 TB
```

A name without an instance means every instance, so `fs.used` is the use
of every filesystem. `metrics list` names the series the history holds.

`metrics show` shows the points of a series over a time range, from an
hour ago to now unless `--since` and `--until` say another. A time is one
that `log` takes: `yesterday`, `-6h`, `2026-11-13 08:00`; a length of time
alone, as `90m` or `2d`, is that long ago. `--every` averages the points
into steps, so that a day of a minute each, 1440 lines, reads as 24:

```
$ metrics show 'fs.used{/data}' --since 1d --every 1h
fs.used{/data}
2026-11-12 11:00:00  1.2 TB
2026-11-12 12:00:00  1.2 TB
2026-11-12 13:00:00  1.2 TB
...
```

A range that starts within the last two weeks is read from the minutes,
and an older one from the hours, which give their averages. `--json`
prints for a script, with plain numbers: bytes, bytes a second, percent
and degrees Celsius. `--utc` shows and reads the times in UTC.

`metrics` reads the files metricsd wrote, and asks nothing of it, so it
shows what was kept when metricsd has stopped too. If it says that only
an administrator reads them, run it as `elevate metrics`. If its first
line says the values are old, the service has stopped: `service metricsd
status` says why, and `log metricsd` shows what it wrote.

## Early warnings

After each reading, metricsd works out which of these are true and tells
`alertd`, which tells the owner of each problem once, and says when it is
over. Each has a hysteresis, so that a value that stays on its line does
not raise and clear the same alert again and again. Each title ends in an
error ID, and the support center has a page for each, saying what it
means and what to do.

| Warning | Raised when | ID |
| --- | --- | --- |
| A filesystem is nearly full | It is 90 percent full; critical from 97 | MET-101 |
| A filesystem is filling fast | It is half full and its use over the last 6 hours reaches its size within 3 days | MET-102 |
| A disk of a Btrfs volume is missing | A disk of the volume is missing or has failed; critical | MET-103 |
| A disk of a Btrfs volume is behind | A disk missed changes | MET-104 |
| Memory is nearly used up | Less than 5 percent is available for 5 minutes | MET-105 |
| A sensor is close to its limit | It is within 5 degrees of its critical temperature for 5 minutes | MET-106 |
| A certificate is about to end | One in `/system/settings/ssl` ends within 14 days; critical within 3 | MET-107 |

A filesystem mounted read-only is not checked, since freeing space on it
is not something its owner can do. A filesystem that fills at a steady
rate is found by the rate: one that is 60 percent full and grows fast
warns days before it is 90 percent full. A sensor with neither a
critical temperature nor a maximum is not checked.

The numbers are the defaults, which `/system/settings/metrics.toml`
holds as comments. To change them, edit the file and run `service metricsd
reload`:

```
[warnings]
filesystem = 80
filling-days = 7
temperature-margin = 10
certificate-days = 30
```

`filesystem` is from 20 to 96 percent, `filling-days` and
`temperature-margin` from 1 to 30, and `certificate-days` from 4 to 365.
A value out of its range keeps its default, and `log metricsd` says so,
with the file, the table and the setting. The warnings go where
alertd(8) sends alerts: to the console, and to the channels that the
owner names in alerts.toml(5).

## Prometheus

metricsd answers a Prometheus server with the metric names that
node_exporter uses, so that a dashboard made for node_exporter shows
the machine: the processors, load, memory, disks, filesystems, network
cards and temperatures. A panel for a metric that Dukovo does not have
stays empty. Dukovo adds metrics of its own: what each service uses
(`dukovo_service_cpu_seconds_total`, `dukovo_service_memory_bytes`), the
disks of Btrfs volumes that are missing or behind, and the number of
alerts that are active.

The endpoint is off until `/system/settings/metrics.toml` names where it
listens and who may ask. Both are needed. For a machine at 192.168.1.10
that a Prometheus server at 192.168.1.20 scrapes:

```
[prometheus]
listen = ["192.168.1.10:9100"]
allow = ["192.168.1.20"]
```

`allow` is a list of addresses, or networks with the length of their
prefix, such as `192.168.1.0/24` or `fd00::/8`. A connection from any
other address is closed before a byte of it is read. Without `allow`,
`listen` is ignored, and `log metricsd` says so.

Run `elevate service metricsd reload`, and check that it answers:

```
$ curl http://192.168.1.10:9100/metrics | head -4
# HELP node_cpu_seconds_total Seconds the processors spent in each mode.
# TYPE node_cpu_seconds_total counter
node_cpu_seconds_total{cpu="0",mode="idle"} 41263.12
node_cpu_seconds_total{cpu="0",mode="system"} 412.77
```

On the Prometheus server, add the machine to its scrape settings:

```
scrape_configs:
  - job_name: dukovo
    static_configs:
      - targets: ["192.168.1.10:9100"]
```

`promtool check metrics` reads what the endpoint gives, as Prometheus
does, and says when something in it is wrong:

```
$ curl -s http://192.168.1.10:9100/metrics | promtool check metrics
```

The values are read when they are asked for, not taken from the history,
and the counters count from boot. The endpoint answers `GET /metrics`
only; any other path gets 404 and any other method 405.

The endpoint is plain HTTP, with no TLS and no password: `allow` is all
that says who may ask. Name the addresses of the monitoring server and no
others, and do not open it to a network that the machine does not trust.
The service is allowed to listen on port 9100 only; to use another,
change `network.listen` in its service file with `service edit metricsd`.
TLS and passwords for the endpoint are not there yet.

## Privacy

The history says how much the machine does, never what is in a file or
what anyone does with it. It stays on the machine, in
`/system/state/metrics`; metricsd sends nothing on its own, and the
Prometheus endpoint answers only the networks that `allow` names, only
when asked. The warnings go to alertd(8), which shows them on the machine
and sends them only where the owner names in alerts.toml(5).
`enabled = false` in the service file (`service edit metricsd`) turns the
whole service off. "Privacy and security reporting" lists everything the
machine sends on its own.

## When it does not work

| What you see | What to do |
| --- | --- |
| `metrics` says it cannot read a file, and that only an administrator reads it | Run it as `elevate metrics` |
| `metrics` says the values are old, or that there is no history | `service metricsd status` and `log metricsd` say whether the service runs and why not |
| `metrics show` says there are no points | The history starts when metricsd first ran: try a shorter `--since`, and check the name with `metrics list` |
| A series is missing | A machine with no sensors has no `temp`, and one with no Btrfs has no `btrfs.missing`; a series that has no value at a minute is missing for it |
| The endpoint does not answer | `log metricsd` says what it found in metrics.toml, and whether it could bind the address; an address that the machine does not have yet is tried again every 10 seconds |
| The endpoint closes the connection | The server's address is not in `allow` |

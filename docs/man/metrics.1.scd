metrics(1) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

metrics - show what metricsd keeps of the machine: now, and its history

# SYNOPSIS

*metrics* [*now*] [_series_...] [*--json*]

*metrics list* [_series_...] [*--json*]

*metrics show* _series_... [*--since* _time_] [*--until* _time_] [*--every* _time_] [*--json*] [*--utc*]

# DESCRIPTION

metrics shows what *metricsd*(8) has read of the machine (ADR 0484): the
processors, load, memory, disks, filesystems, network cards,
temperatures, services and Btrfs volumes. It shows the latest values, the
series that the history holds, and the points of any series over any time
range, from the last two weeks at one minute to the last year at one hour.

metrics reads the files metricsd writes, as the web console does, and
does not ask metricsd anything, so it works when metricsd has stopped,
for what it kept. What a person may read of those files is what their
folders' modes say; a person who is refused is told, and an
administrator reads them with *elevate metrics* (*elevate*(8)).

# SERIES

A series is named as the history names it: a metric and, for what there
are several of, an instance in braces, *cpu*, *fs.used{/data}*,
*net.rx{net0}*. A name without an instance means every instance of the
metric, so *fs.used* is the use of every filesystem. *metrics list* shows
what there is; *metricsd*(8) says what each series means and its unit.

# COMMANDS

*metrics* [*now*] [_series_...]
	The latest values: a line for each series, its name and its value
	with its unit, as metricsd wrote them at its last step. With names,
	only those series. The first reading after metricsd starts has no
	rates, so *cpu* and the other rates are missing from it. When the
	values are 3 minutes old or more, a line on the standard error
	says so, since metricsd is probably not running.

*metrics list* [_series_...]
	The names of the series the history holds, one on a line, in the
	order metricsd keeps them; with names, those that match. It exits
	with 1 when there is none.

*metrics show* _series_...
	The points of the series over a time range, each series under its
	name, a line for each point with its time and its value. A point is
	a step of the history, a minute for the last two weeks and an hour's
	average further back. A time with no value for a series, because
	nothing could be read then, has no line. It exits with 1 when
	there are no points in the range.

# OPTIONS

*--since* _time_
	Where the range starts: an hour ago unless this says another.

*--until* _time_
	Where it ends: now unless this says another.

*--every* _time_
	Average the points into steps of this long, each shown at its
	step's start: *30*, *30s*, *5m*, *1h* or *1d*. A week's points
	are too many to read at a minute each; *--every 1h* shows it as 168
	lines.

*--json*
	Print for a script: *now* as an object of series and numbers,
	*list* as an array of names, and *show* as an array of objects
	with the name of a *series* and its *points*, each a pair of seconds
	since 1970 and a number. The numbers are plain, with no unit: bytes,
	bytes a second, percent, degrees Celsius.

*--utc*
	Show the times of *show* in UTC, and read a time without a zone in
	UTC. By default they are the machine's local time.

# TIMES

A time is one that *log*(1) takes: *now*, *today*, *yesterday*; *-1h*,
*+30min*, *2 days ago*; *YYYY-MM-DD* with a *HH:MM* or *HH:MM:SS* after
it, or a time alone for today. A time is in local time unless *--utc* is
given or it ends in *UTC* or *Z*. A length of time alone, as *10m*, *2h*
or *1d*, is that long ago, so that *--since 10m* is the last 10 minutes.

A range that starts within the last 14 days is read from the minute
files, and one that starts earlier from the hour files, which give each
hour's average and so have 24 points a day.

# VALUES

Values are shown with their unit: percent for *cpu*, *disk.busy* and
*service.cpu*; bytes in powers of 1000 (*1.2 TB*) for memory, filesystems
and *service.memory*; bytes a second (*6.0 KB/s*) for *disk.read*,
*disk.write*, *net.rx* and *net.tx*; degrees Celsius for *temp*; the
load as it is; and a count for the rest.

# EXIT STATUS

0 on success; 1 when there is nothing to show, when a file cannot be
read, or for a mistake in the command line.

# EXAMPLES

The latest values of the processors and of each filesystem:

```
$ metrics cpu fs.used fs.size
cpu             4.2%
fs.used{/}      23 GB
fs.used{/data}  1.2 TB
fs.size{/}      250 GB
fs.size{/data}  3.9 TB
```

How full */data* was over the last day, an hour at a time, and the
same for a script:

```
$ metrics show 'fs.used{/data}' --since 1d --every 1h
$ metrics show 'fs.used{/data}' --since 1d --every 1h --json
```

Which series there are for the temperatures:

```
$ metrics list temp
```

# SEE ALSO

*metricsd*(8), *metrics.toml*(5), *log*(1), *elevate*(8), *free*(1),
*df*(1), *top*(1), *sensors*(1), *systools*(1)

metricsd(8) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

metricsd - keep the machine's history, warn early, and answer Prometheus

# SYNOPSIS

*metricsd run* [*--step* _seconds_]

# DESCRIPTION

metricsd reads the machine once a step (ADR 0484): the processors,
load, memory and swap, each disk's reads and writes, each filesystem's
use, each network card, the temperatures, what each service uses, and
each Btrfs volume's missing disks. It keeps each reading in a history of
two weeks at the step and a year at one record an hour, and writes the
newest reading as text, so that *metrics*(1) and the web console show
the machine's past and now with nothing to install. Spark runs *metricsd
run* as the *metricsd* service, after the network (*spark*(8)).

metricsd also warns before trouble turns into lost data: a filesystem
nearly full or filling fast, a Btrfs volume with a disk missing, memory
nearly used up, a sensor close to its limit, a certificate about to end.
It gives its warnings to *alertd*(8), which tells the owner of each once
(*alerts*(8)).

When *metrics.toml*(5) names an address, metricsd answers a Prometheus
server there with node_exporter's metric names, so that dashboards made
for node_exporter work as they are. Nothing leaves the machine
otherwise: metricsd sends nothing on its own (*dukovo-privacy*(7)).

The step is 60 seconds. *--step* sets it from 1 second to an hour, for
tests; the history's files hold rows at the step they were written at.

metricsd never ends because of what it reads or writes. A source that is
not there, a machine with no sensor or no Btrfs, has no series. A folder
that cannot be made or a disk that takes no writes is said once in the
log (*log*(1)) and again when it is over, and the rows wait in memory, a
day of them at most, until the disk takes them.

# SERIES

A series is a metric and, for what there are several of, an instance:
*fs.used{/data}*. At most 1024 series are kept; one that would be the
1025th is left out, and the log says so once. A series that has no value
at a step, or no value yet, is missing for it.

[[ *Metric*
:[ *Instance*
:[ *Unit, for each step*
|  cpu, cpu.user, cpu.system
:  none
:  percent of all processors
|  load
:  none
:  the load over 1 minute
|  memory.used, memory.available, swap.used
:  none
:  bytes
|  disk.read, disk.write
:  the disk
:  bytes a second
|  disk.busy
:  the disk
:  percent of the step with a request in flight
|  fs.used, fs.size
:  the mount point
:  bytes
|  net.rx, net.tx
:  the card
:  bytes a second
|  net.errors
:  the card
:  receive and send errors and drops in the step
|  temp
:  chip/label, or chip/tempN without a label; zone/type for a thermal zone
:  degrees Celsius
|  service.cpu
:  the service
:  percent of one processor
|  service.memory
:  the service
:  bytes resident
|  btrfs.missing, btrfs.behind
:  the volume's label, else its UUID
:  disks

Rates are over the real time between two readings, on the monotonic
clock. The first reading after metricsd starts has no rates, and a
counter that went backwards, because a card or a disk was replaced, gives
no value for that step, never a negative one.

*memory.used* is what is neither free nor the page cache, which the
kernel gives back when programs need it; *memory.available* is the free
memory and the page cache. *fs.used* is the blocks that are not free,
times the block size, and *fs.size* all of them. The filesystems are
those of _/processes/mounts_ that are Btrfs, ext4, XFS, FAT, NTFS or
exFAT, wherever they are mounted, and those mounted read-only too. One
mounted in several places, as the system's volume is at
_/volumes/sda3_ and its folders at _/system_, _/users_ and the rest, is
one series, named by the first of its mounts there, so that its history
and its warnings are not repeated for each. The
loopback card is left out of the history's *net.rx*, *net.tx* and
*net.errors*, but not of the endpoint's, as node_exporter has it. A
thermal zone of the firmware whose type another zone has is told by its
number: *zone/acpitz-0*. *service.cpu* and *service.memory* are those of
the processes of a service's resource domain (*spark*(8)); the processes
of sessions and apps count in *cpu* and *memory.used* only.

# THE HISTORY

The history is in _/system/state/metrics_. A file for each UTC day in
_minutes_ holds a row for each step, and a file for each UTC month in
_hours_ holds, for each hour, the average, the least and the greatest of
each series over it. Each file is a run of records, each with its kind,
its length and a CRC-32, and is only ever added to, so that Btrfs never
rewrites an old extent for it and snapshots go on sharing them.

Rows are kept in memory and written, with a sync, after each 10 steps
and when metricsd is stopped: a power cut loses the last 10 steps' at
most. A file that a power cut left with half a record is cut back to its
last whole record when metricsd starts, and a record that fails its
checksum ends the reading of its file, with no error. A series that is
new is added at the step it first has, and its earlier rows have no
value for it.

Once an hour, minute files older than 15 days are deleted, and hour
files older than 13 months, but the newest 14 of the first kind and 13
of the second always stay, so that a machine that was off for a month
keeps what it had.

*metrics*(1) and the web console read these files themselves, from the
minute files for a range that starts within 14 days, and from the hour
files, which give their averages, for an older one.

The newest row is also written, as lines of a series' name, a space and
its value, to _/system/live/metrics/now_, at each step: it is written
beside the file and renamed over it, so that a reader never has half of
it. A name may hold spaces, a sensor's label, so the value is what
follows the last one. A series with no value has no line. The file is
in the live folder, so a boot begins without it.

# EARLY WARNINGS

After each step metricsd works out the whole set of problems the reading
shows and gives it to *alertd*(8) as its alerts, when it has changed
since the last time and in any case every 10 minutes, so that alertd's
view catches up after a restart of either. A set that alertd does not
take, because it is not running, is tried again after 10 seconds, and
said once in the log. Each check has a hysteresis, so that a value on its
line does not raise and clear its alert again and again. The lines are
set in *metrics.toml*(5).

[[ *Key*
:[ *Raised when*
:[ *Severity*
:[ *Cleared when*
|  fs-full:_mount_
:  the filesystem is *filesystem* percent full (90 unless set)
:  warning; critical from 97 percent
:  it is below *filesystem* less 3 percent; a critical one is a warning again below 94 percent
|  fs-filling:_mount_
:  the filesystem is half full and the line through the last 6 hours of its use, 30 readings at least, reaches its size within *filling-days* (3)
:  warning
:  the line reaches it in more than 1.5 times *filling-days*, or it is under half full
|  btrfs-missing:_volume_
:  a disk of the volume is missing or failed
:  critical
:  none is
|  btrfs-behind:_volume_
:  a disk of the volume missed changes
:  warning
:  none has
|  memory:low
:  less than 5 percent of memory is available for 5 steps in a row
:  warning
:  10 percent is
|  temp:_chip/label_
:  a sensor is *temperature-margin* degrees (5) below its critical temperature, or at its maximum where it has no critical one, for 5 steps in a row
:  warning; critical at its critical temperature
:  it is 3 degrees below the line it was raised at; a critical one is a warning again 3 degrees below its critical temperature
|  certificate:_file_
:  the first certificate in a *.pem* or *.crt* file of _/system/settings/ssl_ ends within *certificate-days* (14)
:  warning; critical within 3 days, or once it has ended
:  it is renewed, or the file is gone

A filesystem mounted read-only is not checked, since freeing space on it
is not something its owner can do. A sensor with neither a critical
temperature nor a maximum is not checked. A file in _/system/settings/ssl_
that holds only a key is skipped, and the certificates are read at start
and then once an hour, not at each step. After a restart the line of a
filesystem's filling starts from the history, not from nothing.

Each title names the thing and the number: "/data is 93% full (1.2 TB of
1.3 TB)", "/data will be full in about 2 days at the last 6 hours' rate",
"Btrfs volume data is missing 1 of its 2 disks; its data has no second
copy until it is replaced". Each detail says what to do, and ends with an
error ID, *MET-101* for a full filesystem, *MET-102* for a filling one,
*MET-103* and *MET-104* for a missing and a lagging disk, *MET-105* for
memory, *MET-106* for a sensor and *MET-107* for a certificate. The
support center has a page for each.

# PROMETHEUS

With *listen* and *allow* in *metrics.toml*(5), metricsd answers *GET
/metrics* on each address in Prometheus's text format, version 0.0.4, as
node_exporter does for the metrics it has. The values are read when they
are asked for, not taken from the history, and counters count from boot.

*node_cpu_seconds_total*{cpu,mode}, with the modes *user*, *system* and
*idle*; *node_load1*, *node_load5* and *node_load15*;
*node_memory_MemTotal_bytes*, *node_memory_MemAvailable_bytes*,
*node_memory_SwapTotal_bytes* and *node_memory_SwapFree_bytes*;
*node_filesystem_size_bytes*, *node_filesystem_free_bytes* and
*node_filesystem_avail_bytes*{device,fstype,mountpoint};
*node_disk_read_bytes_total*, *node_disk_written_bytes_total*,
*node_disk_reads_completed_total*, *node_disk_writes_completed_total*
and *node_disk_io_time_seconds_total*{device};
*node_network_receive_bytes_total*, *node_network_transmit_bytes_total*,
*node_network_receive_errs_total* and
*node_network_transmit_errs_total*{device};
*node_hwmon_temp_celsius*{chip,sensor}; *node_boot_time_seconds* and
*node_time_seconds*; *node_uname_info*{sysname,release,machine,nodename}.

Dukovo's own are *dukovo_service_cpu_seconds_total*{service} and
*dukovo_service_memory_bytes*{service}; *dukovo_btrfs_disks_missing*{volume}
and *dukovo_btrfs_disks_behind*{volume}; and
*dukovo_alerts_active*{severity}, the alerts that are active as alertd
says, left out when alertd does not answer.

One thread answers one connection at a time. A request from an address
that is not in *allow* is closed before a byte of it is read. The head of
a request may be 8 KiB, and all of it has to arrive within 5 seconds; a
longer one is answered with 431, and one that is slow is closed. Any path
other than _/metrics_ is answered with 404 and any method but GET with
405, and each answer closes the connection. The endpoint is plain HTTP,
for a network the owner trusts: it has no TLS and no password, and *allow*
is what says who may ask. An address that cannot be bound yet, because
the card has not got it, is tried again every 10 seconds, and said once.

# SIGNALS

*SIGHUP*
	Read _metrics.toml_ again: *service metricsd reload* (*service*(8))
	sends it. An endpoint whose *listen* and *allow* did not change is
	left alone, and the thresholds apply from the next step.

*SIGTERM*
	Write the rows waiting and end.

# FILES

_/system/settings/metrics.toml_
	The Prometheus endpoint and the warnings' thresholds
	(*metrics.toml*(5)). The file that comes with the system holds
	only comments.

_/system/state/metrics/minutes/_YYYY-MM-DD_
	A UTC day's rows, one for each step.

_/system/state/metrics/hours/_YYYY-MM
	A UTC month's hours.

_/system/live/metrics/now_
	The newest row as text, for *metrics now*.

_/system/settings/ssl_
	The certificates that are checked for their end.

_/system/live/alerts/socket_
	alertd's socket, where the alerts are given.

_/system/settings/spark/services/metricsd.toml_
	Its service. It holds the history, the newest row and the port 9100
	to listen on (*service.toml*(5)). It reads each filesystem's space
	from the kernel's _/kernel/fs/space_ (*radix-sysfs*(5)), so it needs
	no view of the mounts themselves.

# EXIT STATUS

*run* exits with 0 when it was stopped, and with 1 for a mistake in the
command line.

Not yet: TLS and passwords on the Prometheus endpoint, which *allow*
stands in for.

# EXAMPLES

See the machine now, then the last hour of a filesystem's use, with
the points averaged to the quarter hour:

```
$ metrics
$ metrics show fs.used --since 1h --every 15m
```

Let a Prometheus server at 192.168.1.20 scrape the machine, which is at
192.168.1.10:

```
# cat /system/settings/metrics.toml
[prometheus]
listen = ["192.168.1.10:9100"]
allow = ["192.168.1.20"]

# service metricsd reload
$ curl http://192.168.1.10:9100/metrics
```

# SEE ALSO

*metrics*(1), *metrics.toml*(5), *alertd*(8), *alerts*(8), *log*(1),
*dukovo-privacy*(7), *radix-proc*(5), *radix-sysfs*(5), *service*(8),
*spark*(8), *systools*(1)

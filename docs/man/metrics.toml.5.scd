metrics.toml(5) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

metrics.toml - the Prometheus endpoint and the early warnings of metricsd

# SYNOPSIS

_/system/settings/metrics.toml_

# DESCRIPTION

*metricsd*(8) reads this file to know where to answer a Prometheus
server, and how early to warn (ADR 0484). The file that comes with the
system holds only comments, so the history and the warnings run with
their defaults and nothing answers on the network, until a setting is
uncommented. The file is TOML, in two tables.
*service metricsd reload* (*service*(8)) has metricsd read it again; it
reads it when it starts too.

A setting with a mistake in it, a value out of its range, an address
that does not read or a name that is not a setting, keeps its default, or
is left out of its list, while the others go on, so that a typo never
switches the history off. Each mistake is said in the log (*log
metricsd*) with the file, the table and the setting, at the start and at
each reload; a file that is not TOML at all gives the defaults, and the
log gives the line of the mistake. One mistake is not forgiven: *listen*
without a usable *allow* is not opened.

```
[prometheus]
listen = ["192.168.1.10:9100"]
allow = ["192.168.1.0/24", "fd00::/8"]

[warnings]
filesystem = 90
filling-days = 3
temperature-margin = 5
certificate-days = 14
```

# THE [prometheus] TABLE

*listen =* _list of text_
	The addresses and ports to answer on, each _address_*:*_port_, an
	IPv6 address in brackets: *"192.168.1.10:9100"*,
	*"[fd00::10]:9100"*. None, the default, is off. The service file
	lets metricsd listen on port 9100 only (*service.toml*(5)); to use
	another port, change *network.listen* in it with *service edit
	metricsd*. An address the machine does not have yet, because its
	card has not got it, is tried again every 10 seconds.

*allow =* _list of text_
	The networks that may ask, each an address with the length of its
	prefix, *"192.168.1.0/24"*, *"fd00::/8"*, or an address alone for one
	machine. It is needed with *listen*. A connection from any other
	address is closed before a byte is read. An IPv4 address that arrives
	on an IPv6 socket counts as the IPv4 address.

The endpoint answers plain HTTP: *GET /metrics* with node_exporter's
metric names and Dukovo's own (*metricsd*(8) lists them). It has no TLS
and no password, so *allow* is all that says who may ask: name the
networks of the monitoring server and no more, and do not open it to a
network that the machine does not trust.

# THE [warnings] TABLE

*filesystem =* _percent_
	The percent of a filesystem in use that is a warning, from 20 to
	96. It is critical from 97. The warning is cleared 3 percent
	below this. The default is 90.

*filling-days =* _days_
	A filesystem that is half full or more, and fills within this many
	days at the rate of the last 6 hours, is a warning. From 1 to 30. The
	default is 3.

*temperature-margin =* _degrees_
	A sensor this many degrees Celsius below its critical temperature
	is a warning. From 1 to 30. The default is 5.

*certificate-days =* _days_
	A certificate in _/system/settings/ssl_ that ends within this many
	days is a warning, and critical within 3 days. From 4 to 365. The
	default is 14.

The numbers are whole numbers. The other warnings, a disk of a Btrfs
volume that is missing or behind, and memory nearly used up, have no
settings.

# EXAMPLES

Let a monitoring server on the local network scrape the machine, and
warn about disks at 80 percent:

```
[prometheus]
listen = ["192.168.1.10:9100"]
allow = ["192.168.1.20"]

[warnings]
filesystem = 80
```

# SEE ALSO

*metricsd*(8), *metrics*(1), *alertd*(8), *alerts.toml*(5),
*service*(8), *service.toml*(5)

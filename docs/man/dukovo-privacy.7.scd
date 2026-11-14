dukovo-privacy(7) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

dukovo-privacy - what Dukovo keeps and what it sends

# DESCRIPTION

Dukovo's privacy promise (ADR 0212):

- *No telemetry.* Dukovo collects nothing about how it is used. No part
  of it sends usage figures, crashes, identifiers or anything else about
  its user or the machine to anyone on its own. A crash is reported only
  when a person chooses to, reads the whole report and sends it (*SENT
  WHEN THE USER SENDS IT*).
- *Nothing leaves the machine unless the user starts it*, but for what a
  network needs to work and the update checks, each listed below. A
  program the user runs sends what it is asked to; Dukovo itself sends
  nothing on its own.
- *No account and no ads.* Dukovo needs no online account, and shows no
  ads.

Each service that sends something on its own is listed here, with what
it sends, where and how to turn it off, before it ships. Anything sent
that is not listed is a bug.

# KEPT ON THE MACHINE

*The log and the audit log*
	In _/system/state/log_, by *logd*(8): 90 days of the log and a year
	of the audit log, in at most 4 GiB or a tenth of their file system
	(*log*(5)).

*The crash log*
	In reserved memory, shown at the next boot and kept in the log
	(*radix-debug*(7)). It goes into a report only if the user copies
	it there.

*Crash dumps*
	When the kernel panics, a text dump of its state, in reserved memory
	until the next boot and then in _/system/state/crash_: the ten
	newest, in at most 64 MiB, readable by root alone (*dumps*(8),
	*radix-debug*(7)). It holds what the kernel held, including the
	contents of its stacks. A report of the panic holds the dump's
	*Panic* section alone, which is what the screen showed; it holds
	none of the stacks' words. Any more of the dump goes into a report
	only if the user copies it there.

*Crash records*
	When a signal ends a program, as SIGSEGV does, the kernel keeps a
	record of how, in its memory, until *reportd*(8) reads it: three
	of its registers and up to 64 KiB of the program's stack, which can
	hold secrets. Only root can read the records, and they are never
	written to a disk (*radix-crashes*(4)). *crashrecords=off* on the
	kernel's command line (*dukovo-boot*(7)) makes none.

*Crashes*
	In _/system/state/reports/crashes_, by *reportd*(8), in files that
	root alone reads: for each crash of a program, a service or the
	kernel, the program's name and version, the service, the user, the
	time, the signal, the frames of the stack as module and function
	names and offsets, and a signature of them. Not the registers, not
	the bytes of the stack, and not the program's environment. At most
	100 crashes, within 8 MiB, each for 30 days or until *report
	forget* puts it away.

*Reports*
	The reports that were sent, in _/system/state/reports/sent_: the
	number the server gave each, a key that asks the server about it,
	and its title and time; and the reports that wait for the network, in
	_/system/state/reports/queue_, with their text, until they are sent
	or 30 days have passed. The person's reports are theirs: *report
	list* shows a user only their own.

*Notifications*
	In *notifyd*(8)'s memory only, the last 64, until it stops.

*Metrics*
	In _/system/state/metrics_, by *metricsd*(8): a row of the
	processors, load, memory, disks, filesystems, network cards,
	temperatures and services for each minute of the last two weeks, and
	each hour's average, least and greatest for 13 months
	(*metrics*(1)). It says how much the machine does, never what is
	in a file or what anyone does with it. The newest row is also in
	_/system/live/metrics_, which a boot starts without.

*Alerts*
	The active alerts and the newest 1000 to 2000 changes, and the
	messages not yet delivered for 24 hours at most, in files of
	_/system/state/alerts_ that root alone reads (*alertd*(8)).

# SENT ON ITS OWN

*netd*
	A DHCP request on each network card, from boot, asking for an
	address, router and DNS servers, and carrying the card's hardware
	(MAC) address as its client identifier, and no host name; then a
	renewal at half the lease's time (*netd*(8)). None for a card with
	addresses of its own, or that *network*(5) has netd leave alone.

*resolved*
	Only the names programs ask for, to the DNS servers that
	*resolved*(5) names or else DHCP gave, over DNS over TLS when they
	take it, so that the network on the way does not read them
	(*resolved*(8)); _localhost_, _.local_ and _invalid_ names never
	leave the machine. It sends nothing on its own.

*timed*
	A request for the time, over Network Time Security, to
	_time.cloudflare.com_, _nts.netnod.se_ or _ptbtime1.ptb.de_, the
	first that answers, every 64 seconds to 17 minutes: a TLS
	connection, then NTP packets with random numbers and the cookies
	the server gave, which say nothing of the machine; and to
	_pool.ntp.org_ over plain NTP only while none of them answers
	(*timed*(8)). Another server is named, or none, in *timed*(5);
	*enabled = false* in its service file, *service edit timed*
	(*service*(8)), turns it off.

*netlog*
	The log, to a computer the user names, only when *netlog*(5) or
	the *netlog=* flag (*dukovo-boot*(7)) names one, to debug a
	machine; by default it sends nothing (*netlog*(8)).

*logforward*
	The log, as logd keeps it, to the syslog servers the user names in
	*logforward.toml*(5), over TLS, TCP or UDP, from the moment each is
	named, and again to a server that missed some of it; by default it
	sends nothing (*logforward*(8)). Taking the destination out of the
	file, or *enabled = false* in its service file, *service edit
	logforward* (*service*(8)), stops it.

*metricsd*
	Nothing on its own. Where *metrics.toml*(5) names an address and
	the networks that may ask, a Prometheus server of those networks
	gets the machine's figures when it asks, in plain HTTP; by
	default it listens nowhere (*metricsd*(8)). *enabled = false* in
	its service file, *service edit metricsd* (*service*(8)), turns the
	service off.

*alertd*
	The title and detail of each alert, with the machine's name and the
	time, to the email account and the webhook addresses the user names
	in *alerts.toml*(5), over TLS only. An alert can name a disk's
	model and serial number, a mount point or a service. By default it
	sends nothing; taking the channel out of the file, then *service
	alertd reload* (*service*(8)), or *enabled = false* in its service
	file stops it (*alertd*(8)).

*updated*
	Ten minutes after each start, then once a day, a read of the
	update channel the machine follows, at _updates.dukovo.org_ or the
	server *updates.toml*(5) names, for a newer system and firmware set,
	and of the app catalogs for app updates; the requests carry nothing
	about the machine. *check = "off"* in *updates.toml*(5) turns it
	off, as does *enabled = false* in its service file (*updated*(8)).

Planned, each to be described here in full before it ships:

*Discovery* (Phase 12)
	The machine's name, announced on the local network only.

# SENT WHEN THE USER SENDS IT

*reportd*
	A problem report, only when a person sends it with *report*(1)
	after the preview shows every byte of it, and the report goes to
	_reports.dukovo.org_ or the server *report.toml*(5) names, over TLS.
	It holds

	- what the person wrote in answer to three questions: what
	  happened, what was expected, and how to make it happen again;
	- facts about the machine: the Dukovo version, channel and
	  edition, the maker and model the firmware gives, the processor's
	  name, the amount of memory, the vendor and device IDs of the
	  PCI and USB devices without any serial number, and the language;
	- for a crash, the program and its version, the signal, and the
	  frames of the stack as names and offsets;
	- for a service, up to its last 200 lines of the log, which only
	  an administrator may add and which every report can leave out
	  (*L* at the preview);
	- for a failed start, the failure notice, the changes between the
	  starts and the failed start's warnings and errors.

	Before the preview, reportd cuts out of every line, the person's
	words too, and marks each cut: the names of the accounts, the paths of
	their home folders, the host name and the names in the hosts file,
	IPv4 and IPv6 addresses (not the loopback address), MAC addresses,
	email addresses, the names of the Wi-Fi networks the machine knows, the
	serial numbers the machine has, and the machine's ID. The person
	may edit the text before it is sent, and what is sent is exactly the
	text they saw last. Nothing else goes: no registers, no bytes of the
	stack, no environment, no files, no account, no cookie and no
	identifier of the machine. The request carries a hash of a key that
	is made for the report, which lets only the sender ask the server
	where the report stands and delete it.

	*report --security* encrypts the report to the keys of the security
	team, with age, before it is queued, so that only they can read it.
	*report --save* writes the text to a file, or offers it once to a
	phone on the local network, and sends nothing to a server. A report
	that cannot be sent waits in a queue on the machine and is tried
	again for 30 days (*reportd*(8)).

	reportd sends nothing else. *report forget* puts a crash away, and
	*enabled = false* in its service file, *service edit reportd*
	(*service*(8)), turns the service off.

# REPORTING A SECURITY PROBLEM

Privately, through the repository's GitHub Security tab (*Report a
vulnerability*), as _SECURITY.md_ in the source tree says; never in a
public issue.

# SEE ALSO

*alertd*(8), *alerts.toml*(5), *logd*(8), *log*(5), *logforward*(8),
*metrics*(1), *metricsd*(8), *metrics.toml*(5), *netd*(8), *netlog*(8),
*notifyd*(8), *radix-crashes*(4), *radix-debug*(7), *report*(1),
*report.toml*(5), *reportd*(8), *resolved*(8), *timed*(8)

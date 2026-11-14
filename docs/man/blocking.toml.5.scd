blocking.toml(5) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

blocking.toml - when an address that guesses passwords is blocked, for
how long, and which networks never are

# SYNOPSIS

_/system/settings/blocking.toml_

# DESCRIPTION

*authd*(8) counts the wrong passwords that come from each address over
the network, whatever account they name. When one address gives too many
in a short time, authd has the firewall drop what that address starts on
the ports people sign in at, for a while, and for longer each time it
comes back (ADR 0479). Blocking is on from the first start, with the
values below; this file changes them.

authd reads the file again whenever it changes, so a change needs no
restart: before each request it answers, it looks at the file's time of
change and length, and reads it if either is not what it was. What
changed is told in the audit log. A file authd cannot read, or a value
outside its range, is logged with the reason, and the settings stay as
they were. Without the file, every setting has its default. The file is
TOML:

```
on = true
tries = 5
within = "10m"
first = "10m"
longest = "24h"
never = []
```

*on*
	Whether addresses are blocked at all: *true* by default. With
	*false*, nothing is counted and every block is lifted, but the
	blocks are kept, and those whose time is not up are in force again
	when it is *true* once more. *dukovo-check*(8) warns of it
	(*SEC-112*).

*tries*
	How many wrong passwords from one address, within *within*, block
	it: 5 by default, from 2 to 100. One is never enough, so that a
	single mistyped password cannot block anyone.

*within*
	The time they must come within, from the first of them to the last:
	"10m" by default, from a minute to a day.

*first*
	How long the first block lasts: "10m" by default, at least a minute
	and at most *longest*.

*longest*
	The longest a block lasts: "24h" by default, at most 30 days.

*never*
	The networks that are never blocked, as a list of strings: an
	address, or a network written with its prefix, as *"192.168.1.0/24"*
	or *"2001:db8::/48"*. Empty by default; at most 256. The bits of an
	address past its prefix must be zero, so that *"192.168.1.5/24"* is
	refused, not taken for another network than it says.
	*firewall never-block* (*firewall*(8)) adds a network to the list and
	takes it out, and keeps the rest of the file, its comments too.

A duration is a number and its unit, from the largest unit to the
smallest, each once: *w*eeks, *d*ays, *h*ours, *m*inutes and *s*econds,
as *"90m"*, *"1h30m"* or *"2d"*. A key that is not one of these is a
mistake, so that a misspelled one does not silently leave blocking as
it was.

# WHAT IS COUNTED

Each of these is one failure for the address the client came from:

- a wrong password over SSH (*sshd*(8));
- a wrong sharing password, which a Windows PC signs in to the shares
  with (*smbd*(8));
- a wrong help-session code, from a helper signing in to a help session
  (*help-session*(1));
- a wrong password, second-step code or API token at the web console
  (*admind*(8)), for the browser's address;
- an SSH connection that tried to sign in, was refused every time, and
  ended: *sshd* says so once, as it ends, only if it made a try and did
  not sign in. A wrong password is not counted twice.

An address is an IPv4 address, or an IPv6 network of 64 bits, since one
computer holds a whole /64 and a guesser could otherwise step through
it. An IPv4 address written as IPv6 is the IPv4 one. A right password
from an address clears its count, but not its record of blocks; at the
web console, only a whole sign-in does, its second step too. The
counts are kept in memory only, and authd forgets the oldest past 4096
addresses, so a scan from many addresses cannot fill it.

The slowing of wrong passwords, which *authd*(8) describes under WRONG
PASSWORDS, goes on as it did, for each account.

# DOUBLING

A block that comes within a day of the end of the one before lasts
twice as long as that one: *first*, twice *first*, four times, and so
on, up to *longest*. After a day without a block, the next is a *first*
again. *firewall blocked* says how many blocks in a row an address has
had. *firewall unblock* lifts a block and forgets the record, so that
the address's next block is a first.

# WHAT A BLOCK STOPS

The kernel's firewall drops what a blocked address starts on the sign-in
ports, before any rule: the ports the service files of *sshd*, *smbd*
and *admind* name as the ones they listen on (*service.toml*(5)), with
the changes the files in _/system/settings/spark/services-override_ make
to them, or port 22 when none names one. The drop is counted and logged
as the firewall's others are:

```
Firewall: dropped a TCP connection from 203.0.113.9:51334 to port 22 on net0: the address is blocked
```

Nothing else is stopped: the address still reaches a web site on port
80, and every other port, so that a phone network's shared
address does not lose the web site for a guesser on it. A block does not
end a connection already open, but authd refuses that connection's next
password unread, saying "this address is blocked for guessing passwords"
and for how much longer, and *sshd* ends the connection. The block stops
by itself when its time is up, in the kernel too if authd is late.

# NEVER BLOCKED

- Loopback, and this computer's own addresses, which the firewall never
  filters.
- The networks in *never*.
- An address an administrator is signed in from over SSH or at the web
  console, while that session lasts, so that a mistyped password from a laptop on a network
  that *never* does not list cannot lock out the person who is fixing
  the computer. It counts nothing and blocks nothing for that address
  until the session ends.
- The console and the serial port, which are not on the network.

A block made before its network was added to *never* is not given to the
kernel while *never* names it.

# THE BLOCKS

authd keeps the blocks in _/system/state/authd/blocked_, of mode 0600, a
line _address_ _until_ _started_ _times_ _why_ for each: the address, or
the IPv6 network as *2001:db8:4:7::/64*, when the block ends and when it
began in seconds since 1970, how many blocks in a row, and the reason,
as *5 wrong passwords over SSH in 2 minutes, for root, admin and test*.
They outlast a restart of authd or of the computer: authd gives the
kernel those whose time is not up as it starts. A block is kept for a
day after it ends, so that the next one lasts longer.

Each block, unblock and change of this file is a line of the audit log
(*logd*(8)):

```
authd: blocked 203.0.113.9 for 10 minutes: 5 wrong passwords over SSH in 2 minutes, for root, admin and test
authd: unblocked 203.0.113.9, for process 412 (user 0)
authd: blocking addresses that guess passwords: on: 5 tries within 10 minutes block for 10 minutes, doubling to 1 day
```

Each administrator's next sign-in says how many addresses were blocked
since their last: "Since you last signed in, 3 addresses were
blocked for guessing passwords: *elevate firewall blocked* lists those
still blocked."

# FILES

_/system/settings/blocking.toml_
	These settings. Mode 0644, root's. Its default is in
	_/system/resources/default-settings/blocking.toml_.

_/system/state/authd/blocked_
	The blocks, which only root reads.

_/system/settings/spark/services/sshd.toml_, _smbd.toml_, _admind.toml_
	The service files whose listening ports a block closes.

# EXAMPLES

Block after 3 wrong passwords in 5 minutes, for 30 minutes the first
time and up to a week for one that keeps coming back, and never the
office network:

```
tries = 3
within = "5m"
first = "30m"
longest = "7d"
never = ["192.168.1.0/24"]
```

# SEE ALSO

*authd*(8), *firewall*(8), *sshd*(8), *smbd*(8), *radix-firewall*(2),
*dukovo-check*(8), *service.toml*(5), *logd*(8)

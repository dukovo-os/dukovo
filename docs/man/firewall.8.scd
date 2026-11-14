firewall(8) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

firewall - the firewall's rules, what listens, and the addresses blocked
for guessing passwords

# SYNOPSIS

*firewall* [*status*]

*firewall list*

*firewall allow*|*deny* [*tcp*|*udp*|*ping*|*any*] [_port_[*-*_port_]] [*from* _network_] [*on* _interface_] [*program* _path_]

*firewall allow process* _number_

*firewall remove* _number_

*firewall load*

*firewall blocked*

*firewall unblock* _address_

*firewall never-block* [*remove*] _network_

# DESCRIPTION

The kernel's firewall decides what other machines may start on this
one: a TCP connection, a UDP datagram that answers nothing this machine
sent, and a ping (*radix-firewall*(2), ADR 0334). What this machine
sends itself, on *lo* or to any of its own addresses, always arrives,
and so do the answers to what it sends.

The first rule that matches decides. When none does:

- a ping is answered;
- what a service listens on is open while the service runs: Spark gives
  the ports its file's *network.listen* names the right to be reached
  (*service.toml*(5)), so turning *sshd* on opens port 22, and stopping
  it closes it;
- what a program a person runs listens on, even an administrator's
  inside *elevate*(8), is for this machine alone until a rule allows it.

What is not let in is dropped without an answer, counted, and logged
in the system's log, at most five packets a second:

```
Firewall: dropped a TCP connection from 192.168.1.20:51334 to port 8000 on net0: process 77 listens for this machine alone
```

The rules are kept in _/system/settings/firewall_, a rule a line, in
order. The *firewall* service gives them to the kernel at boot, before
the services that listen start.

*status*, or no command, says how many rules the kernel holds and how
many packets it dropped since the machine started, and lists each
socket that listens: its protocol and port, its process, its program,
and who reaches it. For a program that listens for this machine alone,
it says the one command that allows it from other machines:

```
$ firewall
Firewall: 0 rules; 3 packets dropped since the machine started
Other machines reach what services listen on, and what the rules allow; pings are answered.

Listening      Process  Program                          Reached from
tcp 22              41  /system/commands/sshd            other machines (its service)
tcp 8000            77  /users/alice/bin/server          this machine only
                        To allow it from other machines: firewall allow program /users/alice/bin/server
udp 68              30  /system/commands/netd            other machines (its service)
```

*list* lists the rules of _/system/settings/firewall_, numbered.

*allow* and *deny* add a rule at the end of the file and give the
kernel the file's rules. A rule's words:

*tcp*, *udp*, *ping* or *any*
	What it matches: connections, datagrams, pings, or all three, the
	default.

_port_ or _first_*-*_last_
	The ports it is for; all of them if none is given. A ping has no
	port: it matches only a rule for every port.

*from* _network_
	Only from these machines: an address, or a network such as
	*192.168.1.0/24* or *2001:db8::/32*.

*on* _interface_
	Only what comes by this interface, such as *net0*, a bridge or a
	VLAN (*ip*(8)).

*program* _path_
	Only what comes to a port the program at _path_ listens on, which
	the kernel knows by its file, not its name, so that no program
	passes for another. It comes last, and takes the rest of the line.
	A rule with only this allows the program from everywhere, on every
	port it listens on.

*allow process* _number_ allows the program that process _number_ runs
from other machines, by its path, as *allow program* does: the rule
stays for that program, whatever process runs it next.

*remove* _number_ takes away rule _number_, as *list* numbers them.

*load* gives the kernel the file's rules again, after the file was
changed by hand. A rule whose program or interface is not there is left
out, and said so; so is a line that is not a rule.

# BLOCKED ADDRESSES

Before any rule, the kernel's firewall asks its list of blocked
addresses: those *authd*(8) saw guess passwords, each for a time
(*blocking.toml*(5), ADR 0479). A block stops what its address starts on
the sign-in ports, the ones *sshd*, *smbd* and *admind* listen on, and
nothing else, and it ends by itself. The three commands below ask
*authd*, which keeps the list and its settings; they need an
administrator, so run them with *elevate*(8), and authd refuses them
from anyone else.

*blocked* lists the addresses blocked now: the address, until when, how
many blocks in a row it has had, and why. Until is the local time of day
when the block ends today, and the date and the time when it does not;
with nothing blocked it says so in one line. An address is an IPv4
address, or an IPv6 network of 64 bits, since one computer holds a whole
/64:

```
$ elevate firewall blocked
Blocked            Until  In a row  Why
203.0.113.9        14:32  2         5 wrong passwords over SSH in 2 minutes, for root, admin and test
2001:db8:4:7::/64  14:05  1         5 refused SSH sign-ins in 4 minutes
$ elevate firewall blocked
No address is blocked.
```

*unblock* _address_ lifts the block of an address at once, and forgets
the blocks it had before, so that if it guesses again its next block is
a first one and not a longer one. It takes an IPv4 address, an IPv6
address, which stands for its /64, or the /64 written out, as
*2001:db8:4:7::/64*:

```
$ elevate firewall unblock 203.0.113.9
firewall: 203.0.113.9 is no longer blocked
```

*never-block* _network_ adds a network to the *never* list of
_/system/settings/blocking.toml_, so that none of its addresses is ever
blocked, and *never-block remove* _network_ takes it out again. A
network is an address or an address and its prefix, as *192.168.1.0/24*.
The rest of the file stays as it was, comments too, and authd reads it
at once. A network already in the list, or not in it to be taken out,
is said so and changes nothing:

```
$ elevate firewall never-block 192.168.1.0/24
firewall: 192.168.1.0/24 is never blocked
$ elevate firewall never-block remove 192.168.1.0/24
firewall: 192.168.1.0/24 may be blocked again
```

What a block drops is counted and logged, as the rules' drops are, with
its reason:

```
Firewall: dropped a TCP connection from 203.0.113.9:51334 to port 22 on net0: the address is blocked
```

Reading needs the firewall capability's read right and changing the
rules its control right: an administrator runs firewall with
*elevate*(8), and the *firewall* service holds it (*admin.firewall*,
*service.toml*(5)). Changing the file needs a write to
_/system/settings_, as root's. *blocked*, *unblock* and *never-block*
use no capability of their own: they ask *authd*, which gives the kernel
the blocks, so that it answers an administrator's command inside
*elevate* and no one else's.

# EXIT STATUS

0 when it did what was asked; 1 for a rule it cannot read, a program or
interface that is not there, an address or network it does not read, a
network that is in the list already or is not in it, an address that is
not blocked, a command authd refused, or a wrong command; 2 when the
kernel refused, as without the firewall capability.

# FILES

_/system/settings/firewall_
	The rules, a rule a line; *#* starts a comment.

_/system/settings/spark/services/firewall.toml_
	The service that loads them at boot.

_/system/settings/blocking.toml_
	When addresses are blocked, and the networks that never are
	(*blocking.toml*(5)).

_/system/state/authd/blocked_
	The blocks, which authd keeps through a restart.

# EXAMPLES

A web server for everyone, a port for the office network alone, and no
pings on one card:

```
$ elevate firewall allow tcp 80
firewall: rule 1 is `allow tcp 80`
$ elevate firewall allow tcp 8000-8100 from 192.168.1.0/24
firewall: rule 2 is `allow tcp 8000-8100 from 192.168.1.0/24`
$ elevate firewall deny ping on net1
firewall: rule 3 is `deny ping on net1`
```

# SEE ALSO

*radix-firewall*(2), *blocking.toml*(5), *authd*(8), *ip*(8), *ss*(8),
*service.toml*(5), *elevate*(8), *systools*(1)

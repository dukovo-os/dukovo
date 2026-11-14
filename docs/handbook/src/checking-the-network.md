<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# Checking the network

When the network does not work, the cause is in one of a few places, and
the order matters: a missing cable explains a missing address, which
explains a missing gateway, and so on up to the page that does not load.
`netcheck` checks the places in that order, stops at the first that fails
and tells you what to do about it, so that you start at the cause and not
at the symptom (netcheck(8)).

```
$ netcheck
link      passed       net0 has a link
address   passed       net0 has 10.0.2.15/24
gateway   passed       10.0.2.2 answers in 0.4 ms
dns       passed       10.0.2.3 answers
time      passed       timed set the clock from a time server
mtu       passed       packets of 1500 bytes reach 10.0.2.2
internet  not checked  it sends nothing off the local network: netcheck --internet checks the update server
netcheck: every layer passed.
```

It needs no privilege and sends nothing off the local network, unless you
add `--internet`. Run it first, before you change a setting.

## Reading the result

Each line is a layer, with how it came out: *passed*, *warning*, *FAILED*,
or *not checked*. A layer that failed has the ID of its page, what is
wrong, one sentence on what to do and the address of the page:

```
$ netcheck
link      passed       net0 has a link
address   passed       net0 has 10.0.2.15/24
gateway   FAILED       NET-122: the gateway 10.0.2.99 does not answer
                       Check that the router at that address is on and in this network, and that the card's
                       router setting or the DHCP server names the right one.
                       Help: https://dukovo.org/support/0.16.0/errors/NET-122.html
dns       not checked  an earlier layer failed
time      not checked  an earlier layer failed
mtu       not checked  an earlier layer failed
internet  not checked  an earlier layer failed
netcheck: failed at the gateway layer; the layers after it are not checked.
```

The layers after a failure are not checked because they could say nothing
true: a DNS server that does not answer tells you nothing while the
gateway to it is down. Fix the one that failed, then run `netcheck`
again: it may stop at the next. A *warning* does not stop it: it says
something is wrong that does not keep the network from working, and its
page says what.

Every ID has a page in the support center, under "Error messages", that
says what the message means, how to fix it and how to tell that it is
fixed:

| Layer | What is checked | Failure | Warning |
| --- | --- | --- | --- |
| link | The machine has a network card, and a cable in it | NET-101 no card, NET-102 no link | |
| address | A card with a link has an address | NET-111 | |
| gateway | The default route's router answers | NET-121 no route, NET-122 no answer | NET-122 one of IPv4's and IPv6's routers does not answer |
| dns | A DNS server answers | NET-131 none set, NET-132 none answers | |
| time | The clock was set from a time server lately | | NET-141 |
| mtu | A packet of the card's full size gets through | | NET-161 |
| internet | The update server resolves, answers and is trusted (only with `--internet`) | NET-171 name, NET-172 no answer, NET-173 certificate, NET-174 clock | NET-175 IPv6 |

The exit status says it too, for scripts: 0 when no layer failed or gave
a warning, 1 when one failed, 2 when only warnings were given, and 3 when
the check could not run.

## The internet

By default, `netcheck` asks only the router and the DNS servers on the
card's own network. With `--internet`, it goes on to the update server the
system follows, the `server` in /system/settings/sysupdate/updates.toml
(updates.toml(5)): it looks the name up, connects to its port and, for an
`https://` address, checks its certificate against the system's roots by
the machine's clock. It also asks the DNS servers beyond the local network,
such as a public resolver. It connects to nothing else.

```
$ netcheck --internet
...
internet  passed       updates.dukovo.org answers on port 443 and its certificate is trusted
netcheck: every layer passed.
```

The update server is a good test of the internet because it is what the
system needs most from it: a machine that reaches it can be updated. A
network that lets other sites through and not this one shows as the
failure of the *internet* layer.

## One card, and scripts

A machine with several cards is checked as a whole: the link layer passes
when a card has a link, and the layers after it look at the cards that have
one. `--card net1` checks the one card, which finds the cause on a machine
whose first card works and whose second does not:

```
$ netcheck --card net1
```

`--json` gives the same results as JSON, for a script or a monitoring tool,
with the layer, how it came out, the ID and the advice of each:

```
$ netcheck --json | jq -r '.checks[] | select(.result == "failed") | .id'
NET-122
```

## From Rescue

Rescue, the start menu's repair environment, has a choice "Check the
network": it turns networking on, as "Turn networking on" does, waits up to
20 seconds for an address, and runs `netcheck`. It is the way to find out
why a machine whose slots are both broken cannot reach the network
(see "Updates, snapshots, backups and recovery").

## What it does not check

- **Whether another machine has the same address.** netd does not report
  an address that is in use twice yet, so `netcheck` cannot say so. A card
  that works and then stops while another machine on the network has the
  same address is a cause to look for by hand.
- **The firewall.** The firewall filters what other machines start, not
  what this machine sends (see "The firewall" in "Networking"), so none of
  its rules can stop the checks here. A network that filters this machine's
  traffic shows as a failure of the *dns* or the *internet* layer.
- **The speed of a card.** `nic` says what a card negotiated.

## Troubleshooting

- **`netcheck` says the link failed, but the cable is in.** The card may
  need a moment after the cable goes in: run it again after a few seconds.
  `nic net0` says what the card senses, and `ip link` whether the card is
  up. In a virtual machine, check that the hypervisor's cable is connected.
- **The gateway answers `netcheck` but pages do not load.** Run `netcheck
  --internet`: the failure moves to the layer that is the cause, a DNS
  server that does not answer, or a name that does not resolve.
- **Every layer passes and a program still cannot connect.** The program's
  own settings are the cause: a proxy, a wrong port, or a name the DNS
  servers do not know that is not the update server's. `resolvectl query`
  with the name, `ping` with its address and `nc -zv` with the address and
  port say which.
- **The DNS layer is *not checked*.** Every DNS server the machine has is
  beyond the local network, such as 1.1.1.1; `netcheck --internet` asks
  them.
- **The time layer warns.** The machine has not set its clock from a time
  server since it started, as it was just started, or the network blocks
  time servers: see NET-141. It matters when the update server's
  certificate fails, as NET-174 says.
- **`netcheck` says it could not run.** Its exit status is 3: the list of
  network cards cannot be read, or an option is wrong. `netcheck --help`
  gives the options.
- **A certificate is not trusted on one network and is on another.** A
  proxy, or a page you must sign in to, answers in the server's place on
  the first: see NET-173.

See also the chapters "Networking", for what the layers are and how to set
a card up, and "Updates, snapshots, backups and recovery", for Rescue.

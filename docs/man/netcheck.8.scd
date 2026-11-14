netcheck(8) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

netcheck - find the layer where the network stops working

# SYNOPSIS

*netcheck* [*--internet*] [*--card* _name_] [*--json*]

# DESCRIPTION

netcheck checks the layers a packet needs, in the order it needs them, and
stops at the first that fails: the layers after it cannot say anything
true. It says what failed, one sentence on what to do, and the ID of a page
in the support center that explains it (ADR 0489). It needs no privilege.

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

A layer that fails is shown with its ID, what is wrong, what to do and where
to read more, and the layers after it are *not checked*:

```
$ netcheck
link      passed       net0 has a link
address   passed       net0 has 10.0.2.15/24
gateway   passed       10.0.2.2 answers in 0.4 ms
dns       FAILED       NET-132: no DNS server answers: 10.0.2.99 does not
                       Check that each server is on and reachable from this network, or name another with
                       servers = in /system/settings/resolved.
                       Help: https://dukovo.org/support/0.16.0/errors/NET-132.html
time      not checked  an earlier layer failed
mtu       not checked  an earlier layer failed
internet  not checked  an earlier layer failed
netcheck: failed at the dns layer; the layers after it are not checked.
```

The layers are these. A card is every network card but the loopback, a
tunnel such as a WireGuard interface, which has no link of its own, and a
bridge's port, whose bridge has the addresses. With *--card*, it is the one
named.

*link*
	A card exists and at least one has a link: the cable is in and the
	other end is on. The layers after it look at the cards that have one.
	See *NET-101* and *NET-102*.

*address*
	A card that has a link has an address that reaches beyond its own
	network: an IPv4 address, or an IPv6 one that is neither link-local nor
	still being tested. See *NET-111*.

*gateway*
	A card with an address has a default route, and the router it names
	answers an echo request, as *ping*(8) sends. It is asked twice, each
	time for two seconds. Each version of IP with a default route has its
	router asked: when none answers the layer fails, and when one does not
	while another does, as with an IPv4 router that is gone beside an
	IPv6 one that answers, it is a warning. See *NET-121* and *NET-122*.

*dns*
	*resolved*(8) has a server to ask, and one of those on the local network
	answers a question about the root: any answer, even a refusal, shows
	the server is there. A server that *resolved* asks only over TLS or HTTPS
	is asked for a connection. Servers beyond the local network, such as a
	public resolver, are asked only with *--internet*; without it, a machine
	whose servers are all beyond its network has this layer *not checked*.
	See *NET-131* and *NET-132*.

*time*
	*timed*(8) has set the clock from a time server since the start, and
	within the last day. A failure here is a warning, as the network works
	without it, but a clock that is wrong makes a TLS certificate seem not
	yet valid or no longer valid. See *NET-141*.

*mtu*
	A packet of the card's full size, as large as its MTU lets through,
	reaches the gateway that answered a small one. See *NET-161*, a warning.

*internet*
	Only with *--internet*: the update server the system follows
	(*updates.toml*(5)) has a name that resolves, answers on its port, and
	for an *https://* address has a certificate this system trusts, by this
	machine's clock. Of each IP version the machine has an address of, the
	first two addresses of the server are tried. See *NET-171*, *NET-172*,
	*NET-173*, *NET-174* and, a warning, *NET-175*.

netcheck sends nothing off the local network unless *--internet* is given:
only the gateway and the DNS servers on a card's network are asked, and the
update server with the option.

It does not say whether the firewall (*firewall*(8)) lets something through:
the firewall filters what other machines start, so none of its rules can
stop this machine's own DNS, HTTPS or time. A network that filters what
this machine sends shows as a failure of the *dns* layer, or of the
*internet* one.

# OPTIONS

*--internet*
	Also check the update server, and the DNS servers beyond the local
	network. This connects to the machine named in the *server* setting of
	*updates.toml*(5), and nothing else beyond the local network.

*--card* _name_
	Check only the card _name_, as *ip link* lists it (*net0*). A name that
	is no card of the machine fails the *link* layer.

*--json*
	Print the results as JSON in place of text. The object has the version
	of the system, the _time_ of the check in UTC, the _status_, which is
	*passed*, *warning* or *failed*, and _checks_, an array with an object
	for each layer: its _layer_, its _result_ (*passed*, *warning*, *failed*
	or *not checked*), its _id_ or *null*, its _detail_ and its _advice_ or
	*null*.

```
$ netcheck --json
{
  "version": "0.16.0",
  "time": "2026-11-13T09:41:07Z",
  "status": "failed",
  "checks": [
    {
      "layer": "link",
      "result": "failed",
      "id": "NET-102",
      "detail": "net0 has no link",
      "advice": "Check the cable at both ends and that the switch or router it goes to is on."
    },
    {
      "layer": "address",
      "result": "not checked",
      "id": null,
      "detail": "an earlier layer failed",
      "advice": null
    }
  ]
}
```

(The example is cut after two layers: the output holds all seven.)

*-h*, *--help*
	Print the usage.

# EXIT STATUS

0 when no layer failed or gave a warning, whether it passed or was not
checked because the option that checks it was not given; 1 when a layer
failed; 2 when none failed but a layer gave a warning; 3 when the check
could not run: an option it does not take, or the list of the machine's
network cards cannot be read.

# FILES

_/system/settings/network_
	How *netd*(8) sets each card up, which decides whether the *address*
	layer says DHCP got no answer or the settings give no address
	(*network*(5)).

_/system/settings/resolved_
	The DNS servers *resolved*(8) asks (*resolved*(5)).

_/system/settings/sysupdate/updates.toml_
	The update server that *--internet* checks (*updates.toml*(5)).

_/system/live/timed/status_
	What *timed*(8) says about the clock.

# SEE ALSO

*dukovo-rescue*(8), *ip*(8), *netd*(8), *network*(5), *nic*(8), *ping*(8),
*resolved*(5), *resolved*(8), *resolvectl*(1), *systools*(1), *timed*(8),
*timedatectl*(1), *updates.toml*(5), the handbook's chapter "Checking the
network", and the page of each ID in the support center's "Error messages".

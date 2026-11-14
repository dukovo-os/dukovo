<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0489. netcheck says where the network stops working

- Status: Accepted
- Date: 2026-11-13

## Context

"The network does not work" is the commonest report from a server, and
the answer depends on how far a packet gets: a cable, an address, the
gateway, DNS, the clock, the packet size, or the internet beyond. On
Linux each layer has its own tool (`ip link`, `ip addr`, `ping`, `dig`,
`timedatectl`, `curl`), and the person must know the order to run them
in and read each one's output. Windows has its network troubleshooter,
which fixes some things and explains little; NetworkManager has
`nmcli general` and `connectivity check`, which say whether the internet
is reached but not where it stops.

Dukovo has the facts in its own tools: `ip`'s interfaces and routes,
netd's settings, resolved's servers, timed's status, ping's echo and the
TLS code the update service uses (ADR 0466). What is missing is one
command that asks them in order and says, with an ID and one sentence,
what to do.

## Decision

- **`netcheck [--internet] [--card NAME] [--json]`** (netcheck(8))
  checks the layers in the order a packet needs them and stops at the
  first that fails; the layers after it say `not checked`, since nothing
  they found would be true. A warning does not stop it. Any user may run
  it. The exit status is 0 when every layer passed, 1 when one failed, 2
  for warnings only and 3 when it could not run, a mistake on the
  command line included, so that 1 always means a layer failed.
- **The layers**, each failure or warning with a `NET-` error ID whose
  page in the support center says what to do:
  - link: a card (NET-101), and a link on one (NET-102). Bridge ports and
    tunnels are not cards here; `--card` checks one card, and the later
    layers use the cards that have a link.
  - address: an address on a card with a link (NET-111), with the advice
    that fits a card that asks DHCP and one whose settings give none.
  - gateway: a default route (NET-121), and a gateway that answers an
    echo request (NET-122); a link-local IPv6 gateway is asked through
    its card. The gateway of each version of IP is asked, and one that
    does not answer beside one that does is a warning, as the later
    layers can still be checked through the one that answers.
  - DNS: servers (NET-131), and one that answers a question for the root's
    name servers (NET-132). Any answer counts, a refusal too, so no
    internet is needed. A server resolved asks over TLS or HTTPS only is
    asked for a connection.
  - time: the clock set from a time server since the start, and within a
    day (NET-141, a warning).
  - MTU: an echo request of the card's full size reaches a gateway that
    answered a small one (NET-161, a warning).
  - internet, with `--internet` only: the update server the system
    follows, from its update settings, resolves (NET-171), answers on its
    port (NET-172), and gives a certificate the system trusts (NET-173),
    valid by the clock (NET-174). Where the machine and the server both
    have IPv6 and only IPv4 reaches it, that is a warning (NET-175). A
    machine set to a private update server checks that one.
- **Nothing leaves the local network unless `--internet` is given**: the
  gateway and the DNS servers on a card's network are asked, and DNS
  servers beyond it only with the option. Without it, a machine whose
  servers are all remote says the DNS layer was not checked.
- **Rescue** gains "Check the hardware", which runs `hardware check`
  (ADR 0488), and "Check the network", which turns networking on, waits
  up to 20 seconds for an address, and runs `netcheck`. Each waits for
  Enter before the choices come back.
- **Repair startup** on the install stick (ADR 0503) makes again an EFI
  system partition whose FAT cannot be read, the one on the disk with the
  Dukovo root, and only once the person types the disk's name: it erases
  the partition, and with it any other system's startup files there. It
  then fills the partition as an install does, Rescue's files from the
  stick's release included. Where it finds no Dukovo, it names the
  firmware's SATA mode when a controller is in RAID or IDE mode and none
  in AHCI mode; where Secure Boot is on, which `efiboot --secure-boot`
  reads, it says to turn it off, as Dukovo's startup files are not signed
  for it before 1.0.

## Consequences

- One command says where the network stops, in the order a person would
  look, with a page for each finding, and `--json` gives the web console
  and scripts the same result.
- netcheck reads what the services say rather than asking the network
  again where a service already knows, so it agrees with them.
- The firewall filters only what other machines start (ADR 0334), so no
  rule of it can stop the machine's own DNS, HTTPS or time: there is no
  firewall layer until the firewall has outbound rules. netd neither
  probes an address before it uses it nor reports a conflict, so an
  address another machine uses is not found either. docs/deferred.md
  keeps both.
- Rescue's choices are numbered 0 to 11, with the two checks among them;
  the scripts and lab tests that typed the old numbers type the new ones.

## Alternatives considered

- **A troubleshooter that changes settings, as Windows' does.** It hides
  what was wrong, and a server's network settings are the owner's to
  change; netcheck says what to change and where.
- **Reaching a public host, such as a search engine, for the internet
  layer.** That tells a third party of the machine, and checks a server
  the machine does not need; the update server is the one it must reach.
- **Running every layer whatever fails.** A gateway that does not answer
  makes every later layer fail too, and the list of failures then hides
  the one that matters.

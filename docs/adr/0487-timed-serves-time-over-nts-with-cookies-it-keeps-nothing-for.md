<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0487. timed serves time over NTS, with cookies it keeps nothing for

- Status: Accepted
- Date: 2026-11-13

## Context

timed has set the clock over Network Time Security (RFC 8915) since ADR
0194, which left serving time to others for later. A server on a
company's or a home's network is often what the other machines there
should take their time from. Some networks keep only one machine
allowed out to the internet. Some have no internet at all. Many have
printers, cameras and switches whose logs are only useful when their
clocks agree with the servers'.

The usual answer is chrony or ntpd running beside the system's own
client. Chrony serves NTS when it is given a certificate. Few networks
set that up, so the machines on them take plain NTP, which anyone on
the way can change. A Dukovo server already runs an NTS client and
holds the certificates its web console and sites use (certd). It can
serve authenticated time with one setting, and offer plain NTP only to
the devices that cannot do better.

## Decision

- **timed serves the time it keeps.** With timed(5)'s `serve = nts` or
  `serve = nts+ntp`, it listens on TCP port 4460 for NTS key
  establishment and on UDP port 123 for NTP, over IPv4 and IPv6. The
  default is `off`. Ports 123 and 4460 are in timed's service file, so
  the firewall lets clients reach them only while timed runs. Nothing
  listens on them until `serve` is set.
- **Key establishment.**
  - It is TLS 1.3 alone, with the protocol `ntske/1` and the certificate
    chain and key that `serve-certificate` and `serve-key` name. Both
    must be files in `/system/settings/ssl`, as logforward's are (ADR
    0482).
  - A client must ask for NTPv4 and AES-SIV-CMAC-256. An unknown
    critical record gets error 0, and a malformed request error 1. A
    request for another protocol or algorithm gets an empty record of
    that kind and no cookies.
  - At most 64 run at once. Each has 10 seconds and 4 KiB of request.
  - `service timed reload` serves a renewed certificate from then on.
- **Cookies hold the client's keys, sealed with a master key only the
  server holds (RFC 8915 §6's suggested format).**
  - A cookie is the master key's number, a nonce, and the AEAD
    algorithm with both exported keys, sealed with AES-SIV-CMAC-256.
    The number is the associated data.
  - Its 104 bytes are a multiple of four, so an extension field carries
    it with no padding.
  - A new master key is made each day, and the three newest are kept.
    A cookie therefore works for two days at least, while the server
    keeps nothing per client.
  - The keys live in memory only. After a restart, a client's cookie
    gets the kiss code `NTSN` and the client runs key establishment
    again, which RFC 8915 expects of it.
- **NTP answers.**
  - An NTS request is answered only if its cookie opens and its
    Authenticator checks with the key the cookie holds. A request that
    fails either gets an `NTSN` kiss that echoes its Unique Identifier.
  - The answer is sealed with the server-to-client key. It carries one
    new cookie for each one used and each placeholder, at most eight,
    so that no two requests share a cookie.
  - It is never longer than the request, so it cannot amplify an attack
    on a forged source address.
  - A plain request of 48 bytes is answered only with `nts+ntp`.
    Requests with extension fields that are not NTS's are not answered.
- **What is served is what timed last set the clock by.**
  - The stratum is one more than that server's, at most 15.
  - The root delay and dispersion are the server's plus this round trip,
    with dispersion growing at 15 ppm since the sync.
  - The reference ID is the server's IPv4 address, or the first four
    bytes of the MD5 of its IPv6 address (RFC 5905 §7.3).
  - Three hours after the last sync, or before the first, the answer
    says it is not synchronized (leap indicator 3, stratum 16), so
    clients do not take it.
  - For a network with no other source, `serve-unsynced = yes` serves
    the machine's own clock at stratum 10 with the reference ID `LOCL`,
    as chrony's `local stratum 10` does.
- **Who is answered, and how often.**
  - The default is the networks of the machine's own cards, read again
    each minute. `serve-allow` names others, or `any`.
  - Each client address has a bucket of eight answers, one coming back
    every two seconds, for the 4096 addresses heard from last.
  - Past it, NTS requests are dropped, since a kiss that is not
    authenticated is one an NTS client must not trust. A plain request
    gets the `RATE` kiss, at most once a second.
- **netd's DHCP server gives this machine as the time server** (option
  42) while `serve = nts+ntp` lets the clients' network in (ADR 0486),
  as those clients ask in plain NTP; with NTS alone it gives none. `timedatectl timesync-status`
  shows how many answers and key establishments were served.

## Consequences

- One setting and a certificate make a Dukovo server its network's
  authenticated time source, for chrony, NTPsec, other Dukovo machines
  and anything else that speaks NTS. Devices that only speak plain NTP
  can be served beside them.
- A restart costs each client one key establishment, which takes a few
  milliseconds and happens without anyone noticing.
- Not yet:
  - Several servers cannot share master keys, which RFC 8915 allows so
    that a client's cookies work at any of them.
  - The time is served as timed's last sample has it. timed does not
    choose among several sources or train the clock's frequency (ADR
    0194), so a server for a large network is better fed by more than
    one upstream server, which timed asks in turn.
  - docs/deferred.md lists both.
- Tests:
  - **netproto:** cookies sealed and opened, refused under another key,
    a dropped key, a changed byte or a renumbered key.
  - **timed's serving:**
    - key establishment over real TLS with tls-test's certificate,
      whose cookies then authenticate an NTS request, and a client that
      offers no `ntske/1` refused;
    - every error record;
    - answers sealed with the right cookie count, never longer than the
      request;
    - `NTSN` for a bad cookie or Authenticator;
    - plain NTP only with `nts+ntp`, with `RATE` past the limit;
    - the rate limit and its forgetting;
    - master keys turned over daily;
    - the header when synced, local or unsynchronized;
    - the settings.
  - **The `network-serve` boot test:** a second machine takes its time
    from the first over NTS, and over plain NTP only when it is served.
  - **The lab's `time/nts-server`:** chrony on the lab PC takes NTS from
    the machine.

## Alternatives considered

- **Running chrony as the server.** It serves NTS well, but it would be
  a second clock daemon beside timed, with its own idea of the time and
  its own settings. timed already holds the sync and the keys' code.
- **Keeping cookies in a table, as netservices-test's test server
  does.** The table grows with every client, must be bounded, and
  loses every client at a restart all the same. Sealed cookies cost
  nothing per client.
- **Serving plain NTP by default.** Most clients can do NTS, and plain
  NTP served by default would teach a network to take time anyone on
  the way can change. It stays a choice for the devices that need it.

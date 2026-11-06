<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0399. NFS with Kerberos: RPCSEC_GSS in nfs and nfsd

- Status: Accepted
- Date: 2026-11-06

## Context

NFS with AUTH_SYS takes the client's word for who calls: any machine
on a share's network can say it is anyone. Kerberos is how NFS proves
who calls, and how it signs and seals what goes over the network:
`sec=krb5` checks who calls, `krb5i` signs each call and reply, and
`krb5p` seals them. Each is RPCSEC_GSS (RFC 2203) with Kerberos's
GSS-API mechanism under it (RFC 4121), which Dukovo now has (ADR 0398),
with tickets in each person's keyring (ADR 0397).

On Linux, the kernel's client and server do RPCSEC_GSS, and daemons
(rpc.gssd, rpc.svcgssd or gssproxy) set the contexts up for them with
MIT's library and each person's ticket cache. nfs(1) and nfsd(8) are
programs, not the kernel's, so they do it themselves.

## Decision

- **Both sides speak RPCSEC_GSS version 1 with the three services**,
  from one module, `nfsd::gss`: the credential, the protected bodies,
  and the verifiers. A body's sequence number goes inside it, as RFC
  2203 says, so that a body cannot be moved to another call.
- **A mount has one context for each person who calls through it**, as
  rpc.gssd keeps one for each user. nfs(1) asks authd for the person's
  tickets (a new authd request, `kerberos-tickets`, which only root or
  the person may make, and which apps in a sandbox cannot, as the app
  proxy does not pass it on) and gets a ticket for `nfs/HOST` with them.
  The mount's own walk to its folder, as root, goes under the
  machine's `host/` or `nfs/` key from the keytab, as Linux's does, or
  else under the tickets of the person who mounted through elevate.
- **The calls that keep a client's state go as AUTH_SYS** (EXCHANGE_ID,
  CREATE_SESSION, a SEQUENCE alone): they belong to no person, and RFC
  8881's SP4_NONE lets them. nfsd answers SP4_MACH_CRED with NOTSUPP, as
  before, and Linux's client then asks for SP4_NONE.
- **nfsd accepts contexts with the machine's keytab, for `nfs/` alone.**
  A principal of one part in the server's own realm is served as the
  person's account of the same name; any other, a machine's `host/`
  among them, is served as no one, as root is with AUTH_SYS. Contexts
  last as long as their ticket, at most 1024 are kept, and each keeps a
  window of 128 sequence numbers, beyond which a call is dropped as a
  replay.
- **Each share says which flavors it takes**, with `nfs-sec` in
  shares.toml (`share add --nfs-sec krb5p`), as Linux's exports do with
  `sec=`; none listed is `sys`, as before. A call with another flavor
  gets WRONGSEC where it enters the share, by RFC 8881's rules
  (2.6.3.1.1), and SECINFO lists what the share takes; the root takes
  every flavor and lists them all.
- **The tests:** nfs's and nfsd's unit tests mount with each service in
  the tests' process; the nfs-kerberos boot test mounts both ways
  against Linux's kernel, with MIT's KDC, rpc.gssd and rpc.svcgssd on
  the host. While writing it, nfs was also run against nfs-ganesha, and
  nfsd against libtirpc's RPCSEC_GSS client, both with MIT's library.

## Consequences

- A Dukovo user mounts a Kerberos export after `kinit`, and Linux's
  clients mount a Dukovo share with `sec=krb5p`; the keys stay in nfs
  and nfsd, never in a kernel.
- krb5p costs a seal and an open of each call and reply in nfs and
  nfsd; nfsd does them while it holds its server's lock, as it does the
  rest of a call. Faster paths wait for a benchmark that asks for them.
- Not yet: principals mapped to accounts by a table (Linux's idmapd
  `[Translation]`), contexts held across a restart of nfsd (a client sets
  up another), and SP4_MACH_CRED.

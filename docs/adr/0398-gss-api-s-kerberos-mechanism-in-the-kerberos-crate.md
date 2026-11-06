<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0398. GSS-API's Kerberos mechanism in the kerberos crate

- Status: Accepted
- Date: 2026-11-06

## Context

RPCSEC_GSS (RFC 2203), which NFS's `sec=krb5`, `krb5i` and `krb5p`
are, does not speak Kerberos directly. It carries GSS-API tokens: a
client and a service set up a security context with them, then sign
or seal each call and reply. With Kerberos underneath, the tokens are
RFC 4121's: an AP-REQ and AP-REP in RFC 2743's framing to set up, and
MIC and Wrap tokens after. Linux's client and server reach these
through MIT's libgssapi_krb5 (in rpc.gssd, rpc.svcgssd and gssproxy),
and the kernel takes over the context's keys once it is set up.

Dukovo has its own Kerberos client (ADR 0397) and no MIT library, so
it needs the mechanism too, for both sides: nfs(1) starts contexts,
nfsd(8) accepts them.

## Decision

- **A `gss` module in the kerberos crate**, beside the AS and TGS
  exchanges it builds on, rather than a general GSS-API with
  mechanisms to choose from. The calls are the ones RPCSEC_GSS needs:
  start a context from a service ticket, accept one with a keytab,
  and sign, check, seal and open messages.
- **RFC 4121's tokens only** (the "CFX" tokens of the AES types). The
  crate speaks only AES (ADR 0397), and RFC 1964's tokens are for DES
  and RC4. SPNEGO is not spoken: NFS names the Kerberos mechanism
  directly.
- **Mutual authentication with an acceptor subkey.** A client asks the
  service to prove itself, as Linux's does; the service answers with a
  subkey of its own, as MIT's acceptor does, so that what follows is
  under a key only the two sides know, not the session key the KDC
  also made. Both sides read either choice of the other's.
- **No replay cache.** A replayed AP-REQ sets up a context whose key
  only the true client knows: its subkey travels sealed in the
  authenticator, the service's in its answer. Whoever replays one can
  neither sign a call nor read a reply with it, and RPCSEC_GSS signs
  every call. A replay cache would be a file written at each new
  context, for no gain.
- **No channel bindings and no delegation.** NFS uses neither; a
  client's bindings are not checked, as RFC 4121 lets an acceptor
  without its own do, and delegated credentials are not taken.
- **Wraps are not rotated when sent** (RRC 0, as Linux sends them) and
  are turned back when the other side rotated them, as Windows does.
- **Tested against MIT's own GSS-API.** The unit tests set up contexts
  both ways for each AES type, with and without mutual
  authentication, and check MICs, sealed and signed Wraps, rotated
  Wraps, and the refusals of a stale keytab and another service's
  answer. While writing it, the module was run against MIT krb5 1.20's
  gss-client and gss-server, both ways, with each AES type; the NFS
  boot tests that follow, against Linux's client and server, are what
  keeps it so in CI.

RPCSEC_GSS in nfs and nfsd is the next step.

## Consequences

- RPCSEC_GSS needs no C library: a context's keys stay in the program
  that set it up, as nfs and nfsd are, rather than going to a kernel.
- The module is about 700 lines with its tests, under the same
  cryptography as the rest of the crate.
- A service that accepts tickets for several principals in one keytab
  is told which one a context is for, and checks it.
- Not yet: GSS-API for other protocols (SSH's gssapi-with-mic, SMB's
  SPNEGO), which would take the same calls behind SPNEGO's framing.

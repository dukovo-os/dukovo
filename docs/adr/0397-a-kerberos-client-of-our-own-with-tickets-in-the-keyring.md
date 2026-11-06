<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0397. A Kerberos client of our own, with tickets in the keyring

- Status: Accepted
- Date: 2026-11-06

## Context

The roadmap's 13h asks for Kerberos 5 for the network file systems:
`kinit`, `klist` and `kdestroy`, with each person's tickets kept in
their keyring (ADR 0393) rather than in files, a keytab for the
machine's services, and RPCSEC_GSS (`sec=krb5`, `krb5i`, `krb5p`) in
nfsd and nfs. It is done when a Dukovo user mounts a Linux server's
`krb5p` export after `kinit`, and Linux's client mounts a Dukovo share
with `krb5p`. This first step is the client: the exchanges with a KDC,
the commands, and the tickets in the keyring.

The usual way is MIT's libkrb5 or Heimdal, ported as C. MIT krb5 is
MIT-licensed but large (over 300,000 lines of C, with its own
profile library, plugin loader and replay cache), keeps tickets in
files or the Linux kernel's keyring, and would bring its own
cryptography beside the RustCrypto crates the tree already has. The
Rust crates on crates.io for Kerberos are either bindings to those
libraries or incomplete clients without the AES types of RFC 8009.

## Decision

- **A crate of our own, `kerberos`,** on RustCrypto's AES, HMAC, SHA-1,
  SHA-2 and PBKDF2 crates, which the tree already has through ssh-key
  (each Apache-2.0 OR MIT), and ring's system random: no new
  dependency. It holds the DER that messages use, RFC 3961's n-fold
  and key derivation, the AES types of RFC 3962 (17 and 18, with
  HMAC-SHA1) and RFC 8009 (19 and 20, with HMAC-SHA2), RFC 4120's
  messages, the AS exchange with PA-ENC-TIMESTAMP after the KDC's
  PREAUTH_REQUIRED and ETYPE-INFO2, and the TGS exchange. The tests
  hold the RFCs' own vectors.
- **AES only.** DES and RC4 are broken and every KDC of this century
  offers AES; a principal with only those keys is told so.
- **krb5.conf as MIT writes it**, at /system/settings/krb5.conf or
  where `KRB5_CONFIG` says: `default_realm`, lifetimes, forwardable,
  the encryption types, each realm's `kdc` lines (TCP, then UDP, or
  MIT's `tcp/` and `udp/`), and `[domain_realm]`. What a client does
  not need is skipped, so a Linux host's file works as it is. KDCs are
  not looked up in DNS: each realm names its own.
- **Tickets in the keyring.** Each credential is the entry
  `krb5 CLIENT SERVER`, holding it in MIT's ccache credential format
  (version 4). The keyring is sealed under the person's password and
  closes when their last session ends, so a ticket is never a file
  under /tmp that outlives its owner's session or that another program
  of theirs finds by its path. As with MIT's default cache, one client
  at a time: kinit for another principal replaces the tickets.
- **Keytabs as MIT's kadmin writes them** (format 0x0502), at
  /system/settings/krb5.keytab or where `KRB5_KTNAME` says, read with
  `kinit -k` and `klist -k`; keys of older types are skipped.
- **kinit, klist, kvno and kdestroy** in systools, with MIT's options
  that matter here. kvno takes a service's ticket and names its key's
  version, which is how a stale keytab shows.
- **Tested against MIT's own KDC.** The `kerberos` boot test runs
  krb5kdc on the host as the test's user, on a port of its own, and
  signs in with a password, a wrong password, a keytab and an
  aes128-cts principal; the KDC's log is checked as well as the
  guest's screen.

The next steps are GSS-API's Kerberos mechanism (RFC 4121), then
RPCSEC_GSS in the NFS client and in nfsd.

## Consequences

- Kerberos is under 4,000 lines of Rust with its tests, read as RFC 4120 is,
  and its cryptography is the tree's own, already audited crates.
- A Linux host's krb5.conf and a KDC's keytab work as they are.
- Tickets are not readable by MIT's tools on Dukovo, which has none;
  the format is MIT's, so moving one is a copy of bytes.
- Not yet: FAST armor, PKINIT (smart cards), cross-realm tickets,
  renewing a ticket in place (`kinit -R`), and DNS lookups of KDCs.
  Each is a later step when someone needs it.

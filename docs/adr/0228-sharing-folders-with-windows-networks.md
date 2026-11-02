<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0228. Sharing folders with Windows networks: smbd, signed in through authd

- Status: Accepted
- Date: 2026-11-02

## Context

Phase 13f.2. Windows PCs, Macs and Linux machines open each other's
shared folders over SMB 2 and 3; ADR 0227 made Dukovo a client of them,
and this is the other side: folders of Dukovo's, shared. Linux does it
with Samba, which is GPL-3.0, millions of lines of C, and a domain
controller, a print server and a WINS server besides; ksmbd, the
kernel's, needs a kernel module, which Radix keeps out (the protocol is
a program's, as the client's is). What a home or a small office needs
is far less: SMB 3.1.1 (and 3.0 for older Macs and NAS boxes), NTLMv2
sign-in, signing and encryption, and the files.

Two things must not change for it. A person's login password stays an
Argon2id hash that only authd reads (ADR 0218); NTLMv2 needs the
password's NT hash (MD4 of its UTF-16), which is a password's
equivalent and must never be the login password's. And each person sees
in a share only what their own permissions let them, as at their own
shell, which a server running as root and checking modes itself would
get wrong somewhere. Chase chose a separate sharing password.

## Decision

- **smbd(8)** (userspace/programs/smbd), in every image, is the server,
  of SMB 3.0, 3.0.2 and 3.1.1 only: SMB 1 and SMB 2.x are refused.
  Every session is signed (AES-CMAC, or AES-GMAC on 3.1.1) and
  encrypted (AES-128 or AES-256 in GCM or CCM on 3.1.1, GCM first;
  AES-128-CCM on 3.0 and 3.0.2), with 3.1.1's pre-authentication
  integrity (SHA-512) over the sign-in; a client that cannot encrypt is
  refused, and a message that is not encrypted gets a signed
  ACCESS_DENIED. smb2 (ADR 0227)
  gives it the wire formats it shares with the client, and RustCrypto's
  aes-gcm, aes, cmac and sha2 the cryptography.
- **Two halves.** `smbd`, the *listener*, is a Spark service of its own
  (smbd.toml), root's, holding port 445 and authd's socket and nothing
  of the tree beyond what every service reads. It negotiates, answers
  the SPNEGO and NTLMSSP CHALLENGE, and sends the three NTLM messages to
  authd as a `share-signin` request, with three descriptors: a pipe, the
  connection and its log. authd checks NTLMv2 against the sharing
  password's hash of the account the PC names, starts `smbd --worker`
  *as that person*, as it starts a shell (ADR 0250), with the pipe as
  its input and the connection as its descriptor 3, records the sign-in
  for last(1) on the terminal `smb`, and answers the account's name and
  the session key. The listener checks the mechListMIC, derives the
  keys, writes them to the pipe and sends the last SESSION_SETUP,
  signed; then the connection is the worker's alone. The worker serves
  the shares in shares.toml(5) with the person's own access: the kernel
  decides what each open, read, write, rename and delete may do.
- **The sharing password** is set with `share password` (share(1)),
  after the person's login password, or by root for anyone; only its NT
  hash is kept, in /system/state/authd/sharing, root's and mode 0600,
  beside the Argon2id hashes and never mixed with them. An empty one is
  refused: with none, the network could sign in. Clearing it ends that
  person's share sessions. A wrong one waits as a wrong login password
  does (authd(8)), by account and address, and is recorded for
  lastb(1); a name with no sharing password costs the same time.
- **On only when asked.** smbd.toml is `enabled = false`: port 445 is
  closed until `share on` sets `enabled = true` and starts it; `share
  off` stops it, ends every share session, and turns it off again.
  `share add NAME FOLDER [--read-only] [--comment TEXT]` and `share
  remove NAME` change shares.toml, root's to change and everyone's to
  read; the worker reads it again within a second of a change, and
  drops the connections to a share taken away or changed.
- **The listener's limits**, against a network that is not friendly: a
  frame before sign-in is at most 64 KiB, read exactly so that none of
  the worker's bytes are taken; a sign-in has 60 seconds, authd 90; 64
  sign-ins at once, and three tries on one connection. The worker
  drops a connection on a message that fails to decrypt, a message
  number used twice or far outside its window, a second NEGOTIATE, or a
  VALIDATE_NEGOTIATE_INFO that does not match.
- **The files**, as Windows expects them: names in any case, `.` and
  `..` listed, names Windows cannot hold and links that lead out of the
  share left out, `..` refused; the read-only attribute is the write
  bits; times, sizes, renames (and case-only renames), delete-on-close,
  byte-range locks among the person's own handles, compounds, credits
  up to 512, the MxAc and QFid create contexts, listing the shares over
  srvsvc's NetShareEnumAll on IPC$, and the security descriptor as the
  modes say. A change notice is held until it is cancelled or its
  handle closed.
- **Tested** by smbd's unit tests on the host, which sign smb2's client
  and Samba's smbclient in to the two halves on the loopback, and by the
  `smbd` boot test, where the host's smbclient reaches the guest through
  a port QEMU forwards, with encryption required: shares listed, files
  copied both ways, a folder made; a wrong password, a write to a
  read-only share and SMB 1 refused; and the sharing passwords, share(1)
  and `share on` and `off` checked in the guest.

## Consequences

- A Windows PC opens `\\dukovo\Public` with the person's name and
  sharing password, and Linux's `mount -t cifs` and macOS's Finder do
  too; their files stay the person's, with their modes.
- A bug in the worker acts only as the person who signed in; the
  listener, root's, parses only negotiation and NTLMSSP, in 64 KiB.
- The NT hash is a password's equivalent: anyone who reads it signs in
  to the shares, but not to Dukovo. It is kept as the login hashes are.
- **Not yet:** access lists in the Security tab, and fuzzing smbd
  (13f.3); change notices that watch the folder; locks that other
  sessions and local programs see; leases and oplocks; Kerberos and
  domains; SMB 3.1.1's POSIX extensions; multichannel and RDMA;
  shadow copies (TWrp is answered as not found); NetBIOS and WS-Discovery,
  so a PC finds the machine by its address or name, not in Network.

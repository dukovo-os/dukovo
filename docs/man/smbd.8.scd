smbd(8) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

smbd - share folders with Windows PCs and other computers over SMB 3

# SYNOPSIS

*smbd*

*smbd --worker*

# DESCRIPTION

smbd serves the folders *shares.toml*(5) lists to Windows PCs, Macs and
Linux machines on the network, over SMB 3, always signed and encrypted
(ADR 0228). Spark runs it as the *smbd* service while sharing is on:
*share on* turns it on, and *share off* off (*share*(1)). It is off
until then, and port 445 with it.

A PC signs in with the name of a person's account and their sharing
password, which *share password* sets apart from the password they sign
in to Dukovo with. Then it sees each share as that person would at
their own shell: what their files' owners, modes and access lists let
them read and change, and nothing more.

smbd speaks SMB 3.0, 3.0.2 and 3.1.1, which every Windows from 8 on,
macOS and Linux's *mount -t cifs* speak. SMB 1, which is insecure, and
SMB 2.0 and 2.1, which cannot encrypt, are refused, and so is a client
that will not encrypt. Each message is signed with AES-CMAC, or AES-GMAC
on 3.1.1, and encrypted with AES-GCM or AES-CCM.

# HOW IT WORKS

*smbd*, as Spark runs it, as root, listens on port 445 of every address,
IPv4 and IPv6, and signs PCs in. It answers each PC's NEGOTIATE and
NTLMSSP's challenge, then asks *authd*(8) whether the PC's answer is
right, sending the three NTLM messages with a *share-signin* request.
authd checks NTLMv2 against the person's sharing password, records the
sign-in for *last*(1), or the wrong password for *lastb*(1), and starts
*smbd --worker* as that person, with the connection. smbd checks the
sign-in's message integrity code, hands the worker the session's keys
and sends the PC the last answer of the sign-in; from then on the
connection is the worker's alone.

*smbd --worker* is one PC's session, which runs as the person who
signed in, with their groups, as their shell would: the kernel decides
what each open, read, write, rename and delete may do. It is not run by
hand. It reads *shares.toml*(5) again within a second of a change: a
share taken away, or whose folder or *read-only* changed, is no longer
connected to.

Before a PC has signed in, a message is at most 64 KiB, a sign-in has
60 seconds, and a connection three tries; at most 64 PCs sign in at
once. A message that is not encrypted is refused, and one that fails to
decrypt, or uses a message number twice, ends the connection.

# THE FILES

A share is a folder: its files and folders are shown in any case, as
Windows looks names up, with *.* and *..*; names Windows cannot hold,
and symbolic links that lead out of the share, are left out, and *..*
never leads out. A file is *read-only* to Windows when it has no write
bits, and marking it so takes them away. Renames, case-only ones too,
times, sizes, deleting on close and byte-range locks work as Windows
expects; locks hold among the PC's own handles. A share marked
*read-only* in *shares.toml*(5) changes nothing, whatever the modes.

The PC asks the shares' names over IPC$, as *net view* and Windows'
Explorer do; *IPC$* is not a name a share takes.

# FILES

_/system/settings/shares.toml_
	The shares (*shares.toml*(5)).

_/system/state/authd/sharing_
	The sharing passwords' NT hashes, a line _name_*:*_hash_*:* each,
	root's and of mode 0600, which only *authd*(8) reads.

_/system/settings/spark/services/smbd.toml_
	smbd's service file (*service.toml*(5)), whose *enabled* *share on*
	and *share off* set; the system's own is
	_/system/resources/default-settings/spark/services/smbd.toml_.

# EXIT STATUS

smbd runs until Spark stops it, which ends every share session too; 1
if it cannot listen on port 445. *smbd --worker* ends with its
connection.

# SEE ALSO

*share*(1), *shares.toml*(5), *authd*(8), *smb*(1), *service.toml*(5),
*last*(1), *lastb*(1)

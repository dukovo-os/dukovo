nfsd(8) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

nfsd - share folders with Linux and other Unix machines over NFS 4.2

# SYNOPSIS

*nfsd*

*nfsd --worker*

# DESCRIPTION

nfsd serves the folders *shares.toml*(5) lists to Linux, BSD and other
Unix machines over NFS 4.1 and 4.2 (ADR 0341), to the networks each
share names under *nfs*. Spark runs it as the *nfsd* service while NFS
is on: *share nfs on* turns it on, and *share nfs off* off
(*share*(1)). It is off until then, and port 2049 with it. A share that
names no network is not served over NFS, even while NFS is on.

A client mounts a share by its name, in any case:

	mount -t nfs4 192.168.1.20:/Music /mnt/music

or the root, _192.168.1.20:/_, which holds a folder for each share the
client's address may reach. NFS 4.0, 3 and 2 are refused: Linux's
client, and those of the BSDs and macOS, speak 4.1 or 4.2.

# WHO A CALL IS

Each call says who makes it, AUTH_SYS's way: a user and group number,
which nfsd takes at the client's word, as every NFS server does without
Kerberos. So a share names only networks whose machines, and whose
administrators, you trust, as it trusts each to say truly who calls. A
person's number on the client must be the same as on Dukovo, where the
first person made is 1000. A call is served with the groups the person
has here; the groups it names are not taken.

Root on a client is served as no one, number 65534, which may do only
what a file's modes let everyone do; so is a call that says no one, and
one whose number is no person's here: a service's, a disabled
account's, or one there is no account for.
Owners and groups are numbers, as Linux's client sends them with
AUTH_SYS; a name is refused. Kerberos (RPCSEC_GSS) is refused for now.

# HOW IT WORKS

*nfsd*, as Spark runs it, as root, listens on port 2049 of every
address, IPv4 and IPv6, over TCP. A connection from an address that no
share names is closed. It keeps the clients' state, their sessions,
opens and locks, and answers what needs no file; for everything else it
asks a worker.

*nfsd --worker* serves one person's calls as that person, with their
groups, as their shell would: the kernel decides what each lookup,
open, read, write, rename and remove may do. *authd*(8) starts one for
each person calls come as, and one as no one for root's, when nfsd asks
with an *nfs-worker* request; it sees the shared folders alone, and
nothing of the rest of the system. It is not run by hand. nfsd reads
*shares.toml*(5) again within a second of a change, and starts the
workers again if a share changed: a share taken away answers no more,
and a share made *read-only* refuses writes from then on.

A file's handle names its share, its device and its inode number: it
lasts through renames and through nfsd's restarts, and a file removed
is gone (NFS4ERR_STALE).

# WHAT IT KEEPS AND WHAT IT DOES NOT

A client's lease is 90 seconds: a client that calls nothing for twice
that loses its opens and locks. Byte-range locks are POSIX's, which
*flock*(1) on Linux's client also takes, and hold among the clients of
nfsd; files opened to deny others are kept from them.

nfsd keeps its clients' state in memory alone. After a restart, a
client opens its files again and takes its locks again: there is no
grace period, and a lock another took in the meantime is not given
back. Writes the client did not commit are written again, as the write
verifier changed. nfsd gives no delegations and no pNFS layouts, and
the change attribute is the time a file last changed.

At most 64 connections are open, 512 clients known and 128 sessions
held, eight of them a client's; a session has at most 64 slots and
keeps answers of up to 8 KiB to send again. A COMPOUND holds at most
128 operations, and a call at most a megabyte and 64 KiB.

# FILES

_/system/settings/shares.toml_
	The shares, and the networks each is served to over NFS
	(*shares.toml*(5)).

_/system/settings/spark/services/nfsd.toml_
	nfsd's service file (*service.toml*(5)), whose *enabled* *share nfs
	on* and *share nfs off* set; the system's own is
	_/system/resources/default-settings/spark/services/nfsd.toml_.

# EXIT STATUS

nfsd runs until Spark stops it, which ends every worker too; 1 if it
cannot listen on port 2049. *nfsd --worker* ends when nfsd closes its
pipe to it.

# SEE ALSO

*share*(1), *shares.toml*(5), *authd*(8), *smbd*(8), *service.toml*(5)

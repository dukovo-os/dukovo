nfs(1) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

nfs - mount a folder a Linux or Unix machine shares over NFS, and list
folders

# SYNOPSIS

*nfs mount* [*-f*] [*-o* _options_] _server_:_/path_ _directory_

*nfs ls* [*-o* _options_] _server_:_/path_

# DESCRIPTION

nfs is Dukovo's client of NFS 4.2 and 4.1, the protocol Linux servers,
NAS boxes and other Unix machines share folders with, and Dukovo's own
*nfsd*(8).

*mount* mounts the server's folder _path_ at _directory_ as a filesystem
it serves through _/devices/fuse_ (ADR 0226): every program then uses
its files as any others. nfs goes on in the background, serving the
folder until it is unmounted with *umount*(8); with *-f* it stays in the
foreground. *mount*(8) runs it for *mount -t nfs* and *mount -t nfs4*,
and for a source _server:/path_.

*ls* lists a server's folder, a _/_ after each folder's name.

The folder is named _server:/path_, as Linux names one, with an IPv6
address in brackets (_[fe80::1]:/srv_), or _nfs://server:port/path_ for
a port other than 2049. A Linux server's path is the folder it exports;
Dukovo's nfsd names each share _/Name_.

Mounting needs the administrator, as on Linux: run *nfs mount* with
*elevate*(8).

## Who calls

Each call carries its caller's user and group numbers, and the numbers
of the other groups the caller is in on Dukovo, at most 16, as Linux's
client sends them (AUTH_SYS): the server checks those numbers against
its files' owners and modes. Use the same numbers for a person on both
machines. The server takes them at its word, so share folders over NFS
only to networks you trust; most servers serve root as no one (root
squash). Kerberos (*sec=krb5*) is not spoken yet.

## The files

Owners and modes are the server's, shown by number, or by the name a
server gives, when an account here has it. The kernel checks them, as
the server does again. Files, folders, symbolic and hard links, FIFOs
and sockets are made on the server; device nodes are not.

What is asked of the kernel is answered for a second: another client's
change shows after it. Data written is the server's at once, and on its
disk after *fsync*(2), or once nfs has it committed, past 16 MiB or when
the kernel lets the file go; nfs keeps it until then, and writes it
again if the server restarts before. A server that restarts, or a network that drops
for a while, is connected again as the folder is used. Locks are this
machine's alone: other clients do not see them.

# OPTIONS

*-f*
	*mount* stays in the foreground, and ends when the folder is
	unmounted.

*-o* _options_
	Comma-separated, as Linux's mount.nfs takes them. Options nfs has no
	use for are taken and change nothing: the kernel's caching options
	(*ac*, *noac*, *actimeo=* and the like), *async*, *sync*, *intr*,
	*nolock*, *local_lock=*, *nconnect=*, *clientaddr=*, *\_netdev*,
	*nofail*, *defaults*, *auto*, *noauto*, *nosuid*, *nodev*, *noatime*
	and *x-* options. The others:

	*vers=*_version_, *nfsvers=*_version_
		*4.2* or *4.1*; *4*, as without it, speaks 4.2, else 4.1. NFS 2,
		3 and 4.0 are refused.

	*port=*_port_
		The server's port, 2049 by default.

	*addr=*_address_
		The server's address, which its name is then not looked up for.

	*hard*, *soft*
		With *hard*, the default, a call waits for as long as the server
		is away; with *soft* it fails (EIO) after *retrans* tries.
		Mounting tries *retrans* times either way.

	*timeo=*_n_
		Tenths of a second each reply is waited for: 600 by default.

	*retrans=*_n_
		The tries a *soft* call makes again: 2 by default.

	*rsize=*_bytes_, *wsize=*_bytes_
		The most each read and write carries, at most what the server
		takes, and 1 MiB.

	*ro*, *rw*
		Mount only to read, or to write too (the default).

	*proto=tcp*, *sec=sys*
		What nfs speaks; others are refused.

# EXIT STATUS

0 when the folder is mounted, or listed; 1 when the server cannot be
reached, or refuses the folder or the mount; 2 for a mistake in the
command line.

# EXAMPLES

Mount a Linux server's export at _/srv/music_ and use it:

```
$ mkdir ~/music
$ elevate mount -t nfs nas.local:/srv/music ~/music
$ ls ~/music
$ elevate umount ~/music
```

List a folder, from a server on another port:

```
$ nfs ls nfs://nas.local:2050/srv/music
```

# SEE ALSO

*mount*(8), *umount*(8), *nfsd*(8), *share*(1), *elevate*(8),
*smb*(1), *radix-files*(7)

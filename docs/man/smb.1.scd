smb(1) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

smb - mount a share of a Windows network, and list shares and folders

# SYNOPSIS

*smb mount* [*-f*] [*-o* _options_] _//server/share_[_/folder_] _directory_

*smb shares* [*-o* _options_] _//server_

*smb ls* [*-o* _options_] _//server/share_[_/folder_]

# DESCRIPTION

smb is Dukovo's client of SMB 2 and 3, the protocol of Windows' shared
folders, which Samba and most NAS boxes serve too. It signs in to the
server, signed or encrypted as the server asks, and:

*mount* mounts the share, or a folder of it, at _directory_ as a
filesystem it serves through _/devices/fuse_ (ADR 0226): every program
then uses its files as any others. smb goes on in the background,
serving the share until it is unmounted with *umount*(8); with *-f* it
stays in the foreground. *mount*(8) runs it for *mount -t smb* and
*mount -t cifs*, and for a source that starts with _//_.

*shares* lists the server's shares of files, with their comments,
without its administrative ones (_C$_, _ADMIN$_).

*ls* lists a folder of a share, a _/_ after each folder's name.

The share is named _//server/share_, _\\\\server\\share_ or
_smb://server/share_, with _:port_ after the server for a port other
than 445, and an IPv6 address in brackets.

Mounting needs the administrator, as on Linux: run *smb mount* with
*elevate*(8).

## The files

SMB keeps no owners and modes as POSIX does. Every file is shown as the
mounting user's, or as *uid=* and *gid=* say, with *file_mode=* or
*dir_mode=*, less the write bits when the server marks the file
read-only, as Linux's cifs shows a share without the POSIX extensions.
The modes shown are what the kernel checks. A *chmod*(1) that takes the
write bits away marks the file read-only on the server, and one that
gives them back unmarks it, and a file made without them, as
*install -m 444* makes one, is made read-only; owners do not change
(EPERM). Symbolic and hard links, device nodes and FIFOs are not made
on a share (EPERM).

What is asked of the kernel is answered for a second: another client's
change shows after it. A rename replaces a file at its new name, as
*rename*(2) does. A share whose server restarts, or whose network drops
for a while, is connected again as it is used.

## The password

The password is *password=*'s, else the credentials file's, else
*$PASSWD*'s, else smb asks for it on the terminal. The account is
*user=*'s, else the credentials file's, else *$USER*. A credentials file
holds a line for each of _username=_, _password=_ and _domain=_, as
mount.cifs reads one; keep it readable by its owner alone (*chmod 600*).
*guest* signs in without an account, where the server allows guests.

# OPTIONS

*-f*
	*mount* stays in the foreground, and ends when the share is
	unmounted.

*-o* _options_
	Comma-separated, as Linux's mount.cifs takes them; a doubled comma is
	a comma of a value. Options smb has no use for are taken and
	changed nothing: *vers=*, *sec=*, *iocharset=*, *cache=*,
	*\_netdev*, *nofail*, *defaults*, *auto*, *noauto*, *nosuid*,
	*nodev*, *nounix*, *serverino*, *noserverino*, *noperm*, *soft* and
	*hard*. The others:

	*user=*_name_, *username=*_name_
		The account. _domain/name_ or _domain\\name_ names its domain too,
		and _name%password_ its password.

	*password=*_password_, *pass=*_password_
		The account's password.

	*domain=*_domain_, *dom=*_domain_, *workgroup=*_domain_
		The account's domain or workgroup.

	*credentials=*_file_, *cred=*_file_
		The account, its password and domain from _file_, where the
		options do not give them.

	*guest*
		Sign in as a guest.

	*port=*_port_
		The server's port, over the one the share names.

	*uid=*_uid_, *gid=*_gid_
		The user and group, as numbers, that the files are shown as: the
		mounting user's by default.

	*file_mode=*_mode_, *dir_mode=*_mode_
		The files' and folders' modes, in octal: 0755 by default.

	*ro*, *rw*
		Mount only to read, or to write too (the default).

# EXIT STATUS

0 when the share is mounted, or listed; 1 when the server cannot be
reached, the sign-in or the share is refused, or the mount is; 2 for a
mistake in the command line.

# EXAMPLES

Mount a share of a Windows PC at _~/media_, as its owner's:

```
$ mkdir ~/media
$ elevate smb mount -o user=ann,uid=1000,gid=1000 //desktop/media ~/media
Password for ann@//desktop/media:
$ ls ~/media
$ elevate umount ~/media
```

List a NAS's shares, and a folder of one, as a guest:

```
$ smb shares -o guest //nas.local
$ smb ls -o guest //nas.local/public/photos
```

# SEE ALSO

*mount*(8), *umount*(8), *elevate*(8), *radix-files*(7),
*radix-directory*(2)

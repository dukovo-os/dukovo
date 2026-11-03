mount(8) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

mount - mount a disk or a share at a folder, or list the mounts

# SYNOPSIS

*mount* [*-l*] [*-t* _kinds_]

*mount* [*-r*|*-w*] [*-t* _kind_] [*-o* _options_] _source_ _folder_

# DESCRIPTION

Without a source and a folder, mount lists the mounts, as Linux's does:
_source_ on _folder_ type _kind_ (_rw_ or _ro_), with each filesystem's
label after it with *-l*. *-t* lists only the mounts of the kinds it
names, separated by commas; a share's mount is of kind _fuse_, and is
listed for _smb_ and _cifs_ too, and an NFS folder's for _nfs_ and
_nfs4_.

With them, mount mounts _source_ at _folder_, which must be there:

- A share of a Windows network, of Samba or of a NAS, with *-t smb*,
  *-t cifs* or a _source_ that starts with _//_: mount runs *smb*(1),
  which signs in, mounts the share and serves it, with the options
  given, which are mount.cifs's (smb(1) lists them).
- A folder a Linux or other Unix machine shares over NFS, with *-t nfs*,
  *-t nfs4* or a _source_ _server:/path_: mount runs *nfs*(1), which
  mounts the folder and serves it, with the options given, which are
  mount.nfs's (nfs(1) lists them).
- Anything else is a disk or a partition: its path, or its name under
  _/devices_ (_vdb1_ for _/devices/vdb1_). Its filesystem is the kind
  *-t* names, _fat_, _exfat_, _ntfs_, _btrfs_, _ext4_, _udf_ or
  _iso9660_, or else whichever of them the disk holds. A disk that
  refuses writes, or a filesystem Dukovo only reads (an NTFS, a Btrfs or
  ext4 with features it does not write), is mounted only to read, with a
  warning, unless *-w* asked to write it.

Volumes that come and go, sticks and cards, are *volumed*(8)'s, which
mounts them at _/volumes_; *eject*(1) and *remount*(8) are for those.
Dukovo has no fstab: mount always takes both a source and a folder.

Mounting needs the administrator: run mount with *elevate*(8). A folder
is mounted on as it is seen from the root; a mount beneath a view, as a
bundle's, is refused.

# OPTIONS

*-t* _kind_, *--types* _kind_
	The filesystem's kind, or for a list, kinds separated by commas.

*-o* _options_, *--options* _options_
	Comma-separated. For a disk: *ro*, *rw*, *defaults*, and *compress*
	or *compress=zstd* for a Btrfs. For a share, those *smb*(1) takes;
	for an NFS folder, those *nfs*(1) takes.

*-r*, *--read-only*
	Mount only to read.

*-w*, *--rw*, *--read-write*
	Mount to write; fail rather than mount only to read.

*-l*
	List each filesystem's label too.

# EXIT STATUS

0 when the mounts are listed or the source is mounted; 1 when it is not
there, holds no filesystem Dukovo mounts, is in use, or the mount is
refused, and for a mistake in the command line. For a share, smb(1)'s;
for an NFS folder, nfs(1)'s.

# EXAMPLES

```
# mount
# mount -t cifs -o user=ann,uid=1000 //desktop/media /home/ann/media
# mount -t nfs nas.local:/srv/music /home/ann/music
# mount -r /devices/nvme0n1p3 /home/ann/windows
# mount -t btrfs -o compress vdb /home/ann/backups
```

# SEE ALSO

*umount*(8), *smb*(1), *nfs*(1), *findmnt*(8), *volumed*(8), *eject*(1),
*elevate*(8), *radix-files*(7)

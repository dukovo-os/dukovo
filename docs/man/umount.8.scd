umount(8) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

umount - unmount a filesystem

# SYNOPSIS

*umount* _folder_|_source_...

# DESCRIPTION

umount unmounts the filesystem mounted at each _folder_, or the one last
mounted from each _source_, as *mount*(8) lists them: a disk's name, or
its device's path under _/devices_, or a share's _//server/share_. A disk's filesystem writes what it holds in
memory first. A share's *smb*(1) ends when its share is unmounted.

A filesystem that files are open on, or that something is mounted on,
is busy, and stays mounted.

Unmounting needs the administrator: run umount with *elevate*(8).
Volumes at _/volumes_ are best taken out with *eject*(1), which also
removes their folder.

# EXIT STATUS

0 when each is unmounted; 1 when one is not a mount, is busy, or its
unmount is refused, or for a mistake in the command line.

# SEE ALSO

*mount*(8), *smb*(1), *eject*(1), *elevate*(8)

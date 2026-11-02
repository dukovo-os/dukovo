shares.toml(5) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

shares.toml - the folders shared with Windows PCs and other computers

# SYNOPSIS

_/system/settings/shares.toml_

# DESCRIPTION

shares.toml lists the folders *smbd*(8) shares with Windows PCs, Macs
and Linux machines on the network (ADR 0228), and *nfsd*(8) with the
networks each names over NFS (ADR 0341). *share*(1) writes it:
*share add*, *share remove* and *share list*. It is root's to change and
everyone's to read. smbd and nfsd read it again within a second of a
change.

It is TOML: *shares_version*, then a *[[share]]* table for each share.

```
shares_version = 1

[[share]]
name = "Music"
folder = "/users/chase/Music"
comment = "Chase's music"
read-only = true
nfs = ["192.168.1.0/24", "fd00::/64"]
```

*shares_version* = *1*
	The file's format.

*name* = _"name"_
	What a PC asks for, as _\\\\computer\\name_, in any case: 1 to 80
	characters, none a control character or one of
	*\\ / : \* ? " < > | [ ] ; = + ,*, not starting or ending with a
	space, not only dots, and not *IPC$*. No two shares have the same
	name in any case.

*folder* = _"path"_
	The folder shared: an absolute path, without *.* or *..*.

*comment* = _"text"_
	Shown beside the name in the PC's list of shares: one line of at
	most 256 characters. None unless set.

*read-only* = *true* | *false*
	Whether nothing may be changed through the share, whatever the
	files' modes allow; *false* unless set.

*nfs* = [_"network"_, ...]
	The networks whose machines *nfsd*(8) serves the share to over
	NFS: an address and its prefix, as _"192.168.1.0/24"_, or one
	machine's address, IPv4 or IPv6; at most 64. None unless set, and
	then the share is not served over NFS. NFS with AUTH_SYS takes each
	machine at its word for who calls.

Whoever signs in reaches the folder as themselves: the owners, groups,
modes and access lists of its files decide what they may read and
change, as at their own shell (*smbd*(8)). Over NFS, a call is served as
the number the client says, and root's as no one's (*nfsd*(8)).

smbd and nfsd say in the log when they cannot read the file, or a line
of it is wrong, and then share no folder until it is mended; *share*(1)
refuses to change a file it cannot read.

# SEE ALSO

*share*(1), *smbd*(8), *nfsd*(8)

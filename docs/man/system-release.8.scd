system-release(8) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

system-release - check, sign and make keys for Dukovo's system releases

# SYNOPSIS

*system-release check* [*--root* _file_] [*--version* _version_] _repository_

*system-release make* *--tree* _dir_ *--keys* _dir_ *--out* _dir_
*--version* _version_ *--commit* _commit_ *--toolchain* _toolchain_
*--built* _seconds_ [*--updates-from* _version_]

*system-release keys* _dir_

# DESCRIPTION

A release of Dukovo's system is one signed list of every file of
_/system_, *system.json*, with each file's path, kind, mode, size and
SHA-256, and the files themselves, each kept once under its SHA-256
(ADR 0308). The install stick carries one on its DUKOVO_LIVE partition,
in the folder _release_; the installer installs from it, and the updates
of Phase 15 will fetch the same kind of release from the update server.

A release's _repository_ is laid out as the update server will serve it:
the list is a target of The Update Framework, signed through the four
top-level roles, root, targets, snapshot and timestamp, in _metadata/_,
the list itself in _targets/system/_, and the files in _files/_.

*check* reads the release in _repository_ as a computer installing it
does: from the root this system trusts, _/system/resources/updates/root.json_,
it checks every signature and version of the metadata and the list's
hash, then reads every file of the release and checks its size and
SHA-256. It checks offline, as a USB stick must be: everything but the
metadata's expiry, which says that a server still vouches for it and
cannot be known from a stick, so a stick a year old still checks. Each
file that does not match is named by its path in the release:

```
# system-release check /volumes/sda3/release
system-release: release 0.14.1 (8f2c0e61d4a7b3c9), built 2026-11-06T04:18:31Z from commit 01d66b7..., is signed by the trusted root
system-release: all 2312 files match the signed list (210 MiB)
```

*make* and *keys* are for the machine that builds Dukovo, where *ob*(1)
runs them. *make* lists the slot staged in the folder *--tree*, signs the
list with the keys in *--keys* and lays out its repository in *--out*,
which it makes afresh, then reads the repository back as *check* does.
*keys* makes an Ed25519 key for each role in _dir_, as _<role>.key_,
readable by its owner alone, and the first root, _root.json_, which names
them. Until releases are signed with the project's YubiKeys, each build
machine signs with keys of its own, which *ob* makes in
_.cache/release-keys_, and its images trust only those.

# OPTIONS

*--root* _file_
	Check against the root in _file_ instead of the system's.

*--version* _version_
	The release to check, when the repository holds several; or the
	version of the release *make* makes.

*--tree* _dir_
	The slot to make a release of: what _/system_ will hold.

*--keys* _dir_
	The keys *keys* made.

*--out* _dir_
	Where *make* lays out the repository.

*--commit* _commit_, *--toolchain* _toolchain_
	The git commit and the Rust toolchain the release was built from,
	which its list names.

*--built* _seconds_
	When the release was built, in seconds since 1970, from which its
	metadata expires.

*--updates-from* _version_
	The oldest version that may update to this one directly; this
	version by default.

# EXIT STATUS

0
	The release checked whole, or was made, or the keys were.

1
	It did not: a signature, version or hash failed, a file is missing
	or damaged, or a file could not be read or written. Write a damaged
	stick again.

2
	The command line was wrong.

# FILES

_/system/resources/updates/root.json_
	The root of the releases this system trusts.

# SEE ALSO

*dukovo-boot*(7), *spark*(8), *rdisk*(8), *ob*(1)

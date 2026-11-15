system-release(8) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

system-release - check, sign, offer, publish, pack, keep Rescue of and make keys for Dukovo's system releases, and receive and judge the reports of its canaries

# SYNOPSIS

*system-release check* [*--root* _file_] [*--version* _version_] _repository_ | _file_

*system-release rescue* *--out* _esp_ [*--root* _file_] [*--version* _version_] _repository_

*system-release make* *--tree* _dir_ *--keys* _dir_ *--out* _dir_
*--version* _version_ *--commit* _commit_ *--toolchain* _toolchain_
*--built* _seconds_ [*--updates-from* _version_]

*system-release keys* [*--rotate* _role_ | *--online* _key_] _dir_

*system-release online-key* *--out* _key_ _secret_

*system-release add* *--repo* _dir_ *--keys* _dir_ _release_

*system-release channel* *--repo* _dir_ *--keys* _dir_ _channel_ _version_

*system-release channel* *--repo* _dir_ *--keys* _dir_ *--firmware* _version_|*none* _channel_

*system-release firmware* *--repo* _dir_ *--keys* _dir_ *--version* _version_ *--microcode* _file_ *--license* _license_

*system-release pull* *--repo* _dir_ *--keys* _dir_ *--version* _version_ *--reason* _text_ *--advice* *rollback*|*stay*

*system-release refresh* *--repo* _dir_ (*--keys* _dir_ | *--online-key* _secret_ [*--plan* _file_])

*system-release publish* *--repo* _dir_ *--online-key* _secret_ [*--plan* _file_] _incoming_

*system-release rollout* *--repo* _dir_ *--online-key* _secret_ *--plan* _file_ *pause* _version_ _reason_ | *resume* _version_ | *canaries* *--canaries* _file_ *--reports* _dir_

*system-release canary-receive* *--socket* _path_ *--canaries* _file_ *--reports* _dir_

*system-release canary* *--canaries* _file_ *add* _name_ _key_ | *remove* _name_ | _name_ *red*|*green* _reason_ | _name_ *auto*

*system-release canaries show* *--canaries* _file_ *--reports* _dir_ [*--repo* _dir_ *--plan* _file_] [*--json*]

*system-release canaries changed* *--canaries* _file_ *--reports* _dir_ *--repo* _dir_ *--plan* _file_

*system-release pack* *--repo* _dir_ [*--root* _file_] *--version* _version_ *--out* _file_

*system-release served* [*--json*] [*--root* _file_] _url_

# DESCRIPTION

A release of Dukovo's system is one signed list of every file of
_/system_, *system.json*, with each file's path, kind, mode, size and
SHA-256, and the files themselves, each kept once under its SHA-256
(ADR 0308). The install stick carries one on its DUKOVO_LIVE partition,
in the folder _release_; the installer installs from it, and
*sysupdate*(8) fetches the same kind of release from the update server.

A release's _repository_ is laid out as the update server will serve it:
the list is a target of The Update Framework, signed through the four
top-level roles, root, targets, snapshot and timestamp, in _metadata/_,
the list itself in _targets/system/_, and the files in _files/_.

*check* reads the release in _repository_, or in an update _file_, as a
computer installing it
does: from the root this system trusts, _/system/resources/updates/root.json_,
it checks every signature and version of the metadata and the list's
hash, then reads every file of the release and checks its size and
SHA-256. It checks offline, as a USB stick must be: everything but the
metadata's expiry, which says that a server still vouches for it and
cannot be known from a stick, so a stick a year old still checks. Each
file that does not match is named by its path in the release:

```
# system-release check /volumes/sda3/release
system-release: release 0.15.0 (8f2c0e61d4a7b3c9), built 2026-11-06T04:18:31Z from commit 01d66b7..., is signed by the trusted root
system-release: all 2312 files match the signed list (210 MiB)
```

*rescue* checks the release in _repository_ as *check* does, and writes
it as Rescue (*dukovo-rescue*(8), ADR 0460) into _boot/rescue_ of the
EFI system partition mounted at _esp_: its startup folder, a boot archive
of the rest of the release, and the repository's signed metadata and
list. Every file is checked against the signed list as it is written,
and the copy is read back; it is written beside the one there, as
_boot/rescue.new_, and switched in by two renames, so that a power cut
leaves one whole copy. The installer does the same for each disk that
starts the system.

```
# system-release rescue --out /volumes/sda1 /volumes/sda3/release
system-release: Rescue of release 0.15.0, 2312 files of 229 MiB, in /volumes/sda1/boot/rescue
```

An update file, _dukovo-VERSION.update_, is a release's repository as
one POSIX tar archive, for a machine with no network or for a mirror
(ADR 0466). It holds the metadata with every root, the list and every
file, once, and is read where it is; Linux's *tar* lists and unpacks it.

*make*, *keys*, *online-key*, *add*, *channel*, *refresh*, *publish*,
*pack* and *served* are for the machines that build Dukovo and serve
its updates, where *ob*(1) and the update server's scripts run them. *make* lists the slot staged in the folder *--tree*, signs the
list with the keys in *--keys* and lays out its repository in *--out*,
which it makes afresh, then reads the repository back as *check* does.
*keys* makes an Ed25519 key for each role in _dir_, as _<role>.key_,
readable by its owner alone, and the first root, _root.json_, which names
them. *keys --rotate* _role_ replaces that role's key and signs the next
root, _N.root.json_, with the root key before it, and with the new one
too when the root key is the one replaced; every root is kept, so that a
computer that trusts the first walks to the newest. *keys --online*
_key_ signs the next root naming the update server's online key, the
public key in the file _key_, for snapshot and timestamp beside the keys
in _dir_, either of which signs alone (ADR 0467). Until releases are
signed with the project's YubiKeys, each build machine signs with keys
of its own, which *ob* makes in _.cache/release-keys_, and its images
trust only those.

*online-key* makes the update server's online key: an Ed25519 key whose
secret half goes into the new file _secret_, readable by its owner
alone, and whose public half goes into *--out*, for a root to name.

The update repository, *--repo*, holds every release a server offers,
each file once, and the channels, stable, beta and nightly, each naming
one release (ADR 0466). *add* adds a release that *make* made, after
reading it from the first root and checking every file; a version the
repository offers already with another list is refused, since a version
is never used twice. *channel* points a channel at a release the
repository offers. With keys for snapshot and timestamp in *--keys*,
*add* and *channel* sign a new snapshot and timestamp too; without them,
the update server signs those once the repository is deployed.
*firmware* adds a firmware set of the optional firmware package (ADR
0468): the CPU microcode _file_, with its license as an SPDX expression,
under a version such as *2026.11.12*, signed as a release is, with the
file kept in the store by its SHA-256. *channel --firmware* names the
set a channel offers, or *none*; pointing a channel at a release keeps
the set it names.
*refresh* signs a new timestamp, and a new snapshot when the one there
does not list the newest targets, has two days left or less, or a new
root has been added, as the update server does every six hours, with
the keys in *--keys* or the online key in *--online-key*. *pack* writes
one release as an update file, and reads it back from *--root*, by
default the repository's first root.

*pull* withdraws a release the repository offers (ADR 0469): it signs
_withdrawn/version.json_ among the targets, with the reason and the
advice to machines running it, to go back or to stay. A machine never
stages a withdrawn release. A withdrawal is never undone, and the
version is never used again.

With *--plan*, *refresh* and *publish* also sign the rollout of each
release a channel offers (ADR 0469), and of each firmware set one offers,
as *firmware-*_version_ (ADR 0544): the highest of its waves open, from
the update server's plan in _file_, which they keep up to date. The
waves open on days 0, 1, 3, 5 and 7 after a channel first offered the
release: to the canaries, then to the machines numbered below 10, 100,
500 and 1000. The targets delegate _rollout/\*_ alone to the key the
newest root gives timestamp, which signs the rollouts; the role is good
for a day, and is signed anew when what it lists changes or it has
twelve hours left. *rollout* changes the plan and signs the rollouts at
once: *pause* holds a release's waves where they are, with a reason
machines show (*pause firmware-2026.2.1* a firmware set's); *resume* lets
them open on schedule again; and *canaries* works out which canaries
are not green from the list and their reports (see CANARIES below) and
holds every release's and set's waves while there is one, or frees them.
The old *canary* _name_ *red*|*green* is refused: *canary* below changes
the list.

```
$ system-release rollout --repo /srv/dukovo-system/system.8Fw1 --online-key online.key --plan rollout.json pause 0.15.1 "disk errors on NVMe"
system-release: Dukovo 0.15.1 is paused: disk errors on NVMe; /srv/dukovo-system/system.8Fw1 signed again, snapshot 5 and timestamp 12
system-release: Dukovo 0.15.1: wave 1 open, paused: disk errors on NVMe
```

*publish* is what the update server runs when a deploy is ready (ADR
0467). From the folder _incoming_ it takes the numbered roots and
targets, and nothing else of the metadata; the release lists and
channels, in _targets/_; and the files, in _files/_, each checked
against the SHA-256 it is named by as it is copied, and never replacing
one. A first root other than the repository's own is refused. Then it
signs a snapshot and timestamp with the online key, reads every
release list and channel back from the first root as a computer
online does, and checks that every file each release lists is in the
store at its size and that every channel offers a release the
repository offers:

```
$ system-release publish --repo /srv/dukovo-system/system.5Kq2 --online-key online.key incoming
system-release: /srv/dukovo-system/system.5Kq2 offers 3 releases, newest Dukovo 0.15.1, with 212 new files; signed again, snapshot 4 and timestamp 9
system-release: the stable channel offers Dukovo 0.15.1
system-release: the beta channel offers Dukovo 0.15.1
```

# CANARIES

The canaries are the project's own machines that take each release
first. Each signs a report every hour (ADR 0533), and a release's waves
open on schedule only while every canary is green. The update server
judges them, with these commands, and holds the waves of every release
and set, and of the catalog's bundles, while one is not.

*canary-receive* is the receiver of the reports, which the web server
passes _POST /canary/v1/report_ to through the unix socket _path_,
which it makes readable and writable by its owner and group. It holds
no key and needs no network. A report is kept, as that canary's one
file in _dir_, only when its envelope is no larger than 16 KiB, its key is on the
list in _file_, its signature verifies, the name that key was enrolled
under is the name in the report, its time is no more than 5 minutes
ahead of the server's clock and it is newer than the report kept. It
answers 200 when it keeps one, and otherwise the reason: 400 for a
request that is not a report or whose time is ahead, 403 for a key that is
not on the list, a signature that does not verify or another canary's
name, 409 for a report that is not newer, 413 for one that is too large
and 503 when it is busy. It reads the list for each request, and takes no
chunked body: the web server sends the length. A request with any other
key writes nothing.

*canary* changes the list in _file_, which belongs to root: *add* puts the
canary _name_ on it with the public key, in hex, that *sysupdate canary
enrol* prints (ADR 0533); *remove* takes it off; _name_ *red* or *green*
sets its colour by hand, with the reason, which wins over its reports;
and _name_ *auto* goes back to them. A canary added is not green until
its first report is. A name is 1 to 32 of a-z, 0-9 and -, not first -.

*canaries show* prints each canary on the list: its colour and why, when
it last reported, the release it runs and a colour set by hand. With
*--repo*, the repository the update server serves, and *--plan*, it also
says whether the plan holds the waves for the canaries not green. With
*--json* it prints the same as JSON.

A canary is not green when any of these holds:

- it is red by hand;
- it has never reported;
- its newest report is more than 2 hours old;
- its last health check failed;
- its last update fell back;
- *dukovo-check* found failures, or did not run;
- a service has failed;
- with *--repo* and *--plan*, its channel has offered a newer release for
  more than 6 hours, as the plan says, and it still runs an older one.

A colour set by hand wins over all of them. A report is checked again
against the list each time it is judged, so a canary taken off the list
counts for nothing and a report the list does not vouch for is not
a green one.

*canaries changed* prints one line and exits 0 when the canaries that are
not green are not those the plan in _file_ holds the waves for, so that
*rollout canaries* is to sign the rollouts again; it prints nothing and
exits 3 when they are the same. The update server runs it after a
report or a change of the list and every few minutes besides, and signs
only when it says so. The signed rollout names the canaries, never the
reasons, which stay on the server.

*served* reads the repository an update server serves at _url_ as a
computer online does, from *--root*: every signature, version, hash and
expiry, every release list and every channel, but none of the files.
With *--json* it prints, as JSON, each role's version and expiry, every
release with its rollout and withdrawal, every firmware set with its
rollout, and every channel: what *ob updates status* reads for Dukovo
Ops.

# OPTIONS

*--root* _file_
	Check against the root in _file_ instead of the system's.

*--version* _version_
	The release to check, when the repository holds several; the
	version of the release *make* makes; or that of the firmware set
	*firmware* adds.

*--microcode* _file_
	The CPU microcode of the firmware set *firmware* adds.

*--license* _license_
	The license of *--microcode*, as an SPDX expression.

*--firmware* _version_|*none*
	The firmware set *channel* names for the channel, or none.

*--reason* _text_, *--advice* *rollback*|*stay*
	Why *pull* withdraws the release, and whether machines running
	it should go back to the version before it or stay on it until
	the next release.

*--plan* _file_
	The update server's plan of the waves, which is never served.

*--canaries* _file_
	The list of canaries, root's file on the update server.

*--reports* _dir_
	The folder of the canaries' reports, one file for each canary.

*--socket* _path_
	The unix socket *canary-receive* listens on.

*--json*
	With *served* and *canaries show*, print JSON.

*--tree* _dir_
	The slot to make a release of: what _/system_ will hold.

*--keys* _dir_
	The keys *keys* made.

*--repo* _dir_
	The update repository.

*--rotate* _role_
	The role whose key *keys* replaces: root, targets, snapshot or
	timestamp.

*--online* _key_
	The update server's online key, a public key in JSON, for the next
	root to name for snapshot and timestamp.

*--online-key* _secret_
	The secret half of the update server's online key, which signs the
	snapshot and timestamp.

*--out* _dir_
	Where *make* lays out the repository; for *rescue*, the EFI system
	partition's mount point; for *pack*, the update file; for
	*online-key*, the public half of the key.

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
	The release checked whole, or was made, written as Rescue, offered,
	published or read back, or the keys were.

1
	It did not: a signature, version or hash failed, a file is missing
	or damaged, or a file could not be read or written, as on an EFI
	system partition too small for Rescue. Write a damaged stick again.

2
	The command line was wrong.

3
	With *canaries changed*, the canaries that are not green are the
	ones the plan already holds the waves for: nothing to sign.

# FILES

_/system/resources/updates/root.json_
	The root of the releases this system trusts.

# SEE ALSO

*sysupdate*(8), *dukovo-boot*(7), *dukovo-rescue*(8), *spark*(8),
*rdisk*(8), *ob*(1)

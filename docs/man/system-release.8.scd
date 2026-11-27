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

*system-release list* *--tree* _dir_ *--version* _version_ *--commit* _commit_
*--toolchain* _toolchain_ *--built* _seconds_ [*--updates-from* _version_] [*--out* _file_]

*system-release keys* [*--rotate* _role_ | *--online* _key_] _dir_

*system-release online-key* *--out* _key_ _secret_

*system-release release-key* *--yubikey-helper* _file_ [*--serial* _number_] _keys_

*system-release first-root* *--yubikey-helper* _file_ *--out* _metadata_ _keys_

*system-release next-root* *--yubikey-helper* _file_ *--out* _metadata_ [*--repo* _dir_] [*--yes*] _keys_

*system-release add* *--repo* _dir_ *--keys* _dir_ _release_

*system-release fetch* *--root* _file_ [*--roots* _metadata_] *--out* _dir_ _url_

*system-release medium* *--root* _file_ *--tree* _dir_ *--out* _dir_ _url_ _list_

*system-release sums* *--root* _file_ *--yubikey-helper* _file_ [*--serial* _number_] *--version* _version_ *--out* _dir_ [*--record* _file_] [*--yes*] _image_...

*system-release offer* *--repo* _dir_ (*--keys* _dir_ | *--yubikey-helper* _file_ [*--serial* _number_]) *--tree* _dir_ [*--advisories* _dir_] [*--record* _file_] [*--yes*] _list_ _channel_

*system-release channel* *--repo* _dir_ (*--keys* _dir_ | *--yubikey-helper* _file_ [*--serial* _number_]) [*--advisories* _dir_] _channel_ _version_

*system-release channel* *--repo* _dir_ (*--keys* _dir_ | *--yubikey-helper* _file_ [*--serial* _number_]) [*--advisories* _dir_] *--firmware* _version_|*none* _channel_

*system-release advisories* *--repo* _dir_ (*--keys* _dir_ | *--yubikey-helper* _file_ [*--serial* _number_]) _dir_

*system-release advisories* *--check* _dir_

*system-release firmware* *--repo* _dir_ *--keys* _dir_ *--version* _version_ *--microcode* _file_ *--license* _license_

*system-release pull* *--repo* _dir_ (*--keys* _dir_ | *--yubikey-helper* _file_ [*--serial* _number_]) *--version* _version_ *--reason* _text_ *--advice* *rollback*|*stay*

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
cannot be known from a stick, so a stick a year old still checks. When
the metadata lists more than one release, as the update server's does
on the project's sticks, it reads the one *--version* names, or else
the one the file _version_ in _repository_ names. Each
file that does not match is named by its path in the release:

```
# system-release check /volumes/sda3/release
system-release: release 0.15.0 (8f2c0e61d4a7b3c9), built 2026-11-06T04:18:31Z from commit 01d66b7..., is signed by the trusted root
system-release: all 2312 files match the signed list (210 MiB)
```

*rescue* checks the release in _repository_ as *check* does, the one
*--version* or the file _version_ names among several, and writes
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

*make*, *list*, *keys*, *online-key*, *add*, *channel*, *advisories*, *refresh*,
*publish*, *pack* and *served* are for the machines that build Dukovo and serve
its updates, where *ob*(1) and the update server's scripts run them. *make* lists the slot staged in the folder *--tree*, signs the
list with the keys in *--keys* and lays out its repository in *--out*,
which it makes afresh, then reads the repository back as *check* does.
*list* writes the list *make* would sign, unsigned, to *--out* or the
standard output, and needs no keys: builds on machines whose keys differ
compare it, and the SHA-256 of its bytes, to see that they made the same
release (ADR 0535, *ob*(1) *release build*).
*keys* makes an Ed25519 key for each role in _dir_, as _<role>.key_,
readable by its owner alone, and the first root, _root.json_, which names
them. *keys --rotate* _role_ replaces that role's key and signs the next
root, _N.root.json_, with the root key before it, and with the new one
too when the root key is the one replaced; every root is kept, so that a
computer that trusts the first walks to the newest. *keys --online*
_key_ signs the next root naming the update server's online key, the
public key in the file _key_, for snapshot and timestamp beside the keys
in _dir_, either of which signs alone (ADR 0467). The project's releases
are signed on its YubiKeys (*offer*), and its install images carry
them as the update server serves them (*medium*); each build machine
signs the release of the other images it builds with keys of its own,
which *ob* makes in _.cache/release-keys_, and those images trust only
them.

*online-key* makes the update server's online key: an Ed25519 key whose
secret half goes into the new file _secret_, readable by its owner
alone, and whose public half goes into *--out*, for a root to name.

*release-key* and *first-root* are the key ceremony of the project's
releases (ADRs 0457 and 0538), which ob(1) runs as *ob release key
yubikey* and *ob release root*. *release-key* writes the public half of
the ECDSA P-256 key in slot 95 of the YubiKey plugged in, or of the one
*--serial* names, as _keys_/yubikey-_serial_.json, and the key alone in
PEM as _keys_/yubikey-_serial_.pem, through the helper
*--yubikey-helper*, tools/yubikey-piv.py in the source tree. It refuses
a key that was imported rather than made on the YubiKey. *first-root*
signs root version 1, good for a year, on both YubiKeys whose files are
in _keys_, each asking for its PIN and a touch: it names both YubiKeys'
keys for root, targets, snapshot and timestamp, either signing alone,
and the update server's key in _keys_/online.json for snapshot and
timestamp too. It writes _metadata_/1.root.json and prints the root's
SHA-256 last, and refuses when that file is there already, or when a
YubiKey is not plugged in or holds another key than its file names.

*next-root* signs each root after the first (ADR 0780), as *ob release
root* does once _metadata_/1.root.json is there. It reads the roots in
_metadata_, each of which must follow the one before, and makes the
next from _keys_ as it is now: the YubiKeys whose files are there, one
or two, for every role, and _keys_/online.json for snapshot and
timestamp too. The newest root's root keys sign it, on each YubiKey
plugged in whose key that root names or whose file is in _keys_; a
YubiKey new to the root signs it too, so that it is shown to work, and
must be plugged in. It shows the root, what it adds and what it drops,
and signs only once *root* _N_ is typed, or with *--yes*. With *--repo*,
it first adds to the repository the roots of _metadata_ it lacks, then
writes the new root into it and signs its targets again as they are,
with a key the new root names, so that they check under it and
delegate the rollouts to the online key it names. It writes
_metadata_/_N_.root.json last, and prints its SHA-256.

The update repository, *--repo*, holds every release a server offers,
each file once, and the channels, stable, beta and nightly, each naming
one release (ADR 0466). *add* adds a release that *make* made, after
reading it from the first root and checking every file; a version the
repository offers already with another list is refused, since a version
is never used twice. *channel* points a channel at a release the
repository offers. With keys for snapshot and timestamp in *--keys*,
*add* and *channel* sign a new snapshot and timestamp too; without them,
the update server signs those once the repository is deployed.

The project's releases are signed on one of its YubiKeys, from what the
update server serves (ADR 0778), as *ob release sign* does. *fetch*
reads the repository the server at _url_ serves, from the first root in
*--root*, as a computer online does, expiry checked, and writes it into
the new folder *--out*: every root by version, the newest targets as
served, and the file of each target by its SHA-256; a server that serves
nothing yet gives the first root alone. With *--roots* _metadata_, the
roots after the first that _metadata_ holds and the server does not
serve yet are added, each checked to follow the one before; it refuses
when the server serves a root _metadata_ does not hold, or holds as
other bytes. The releases' files stay on the
server. *offer* then adds the release whose list *system-release list*
wrote in _list_, whose files are in the tree *--tree*, each checked
against the list as it is copied into the store, and points _channel_ at
it, keeping the firmware set the channel offers, with the advisories of
*--advisories*, in one targets version. It refuses a version that is
not newer than every release the repository offers, and a list that is
not byte for byte as *list* writes it. It shows what it is about to
sign, and signs only once the version is typed, or with *--yes*. With
*--record* _file_, it writes the Rekor entry of the YubiKey's signature
to _file_, which *ob release sign* sends to Sigstore's log.
*channel*, *advisories* and *pull* sign on a YubiKey too, on a fetched
repository, with *--yubikey-helper* in place of *--keys*. The newest
root must name the YubiKey's key in slot 95 for targets, or nothing is
asked of it.
The project's install stick and ISO of a release carry it as the
update server serves it (ADR 0779), as *ob release images* makes them.
*medium* reads the server at _url_ from the first root in *--root*, as
*fetch* does, and lays out in the new folder *--out* the release whose
list *system-release list* wrote in _list_: the roots walked to, the
newest timestamp, snapshot and targets as served, the list, the
signatures *troubleshoot*(8) matches, the file _version_ naming the
release, and the files from the tree *--tree*, each checked against the
list. It refuses unless the server offers the release with that list
byte for byte, and reads the folder back as *check* does. *sums*
writes _SHA256SUMS_ of each _image_ into *--out*, as *sha256sum* writes
it, shows it and the YubiKey that will sign, and once the version is
typed, or with *--yes*, signs it whole with the YubiKey's key in slot
95, which the root in *--root* must name for targets. The signature, in
DER, goes into _SHA256SUMS.yubikey-SERIAL.sig_, which *openssl* checks
with that key's PEM, _releases/keys/yubikey-SERIAL.pem_ in the source
tree (*ob*(1) *release images*). With *--record* _file_, it writes the Rekor entry of
the signature to _file_.
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

*advisories* signs the security advisories of a folder as targets of the
repository (ADR 0534), and *channel --advisories* does the same in the one
targets version that offers the release. The folder, such as the source
tree's _security/advisories_, holds one OSV 1.6 file for each, named
_DKSA-YYYY-NNNN.json_ for its id. Each is read as *ob security check* reads
it: valid, with Dukovo's fields (the system is the package *dukovo*, an app is
named by its ID, the ranges are in Dukovo's version order), the id its
file's name, a number of its own, and no text that still says TODO. If one
does not check, each problem is named by its file and nothing is signed.
The targets are _advisories/DKSA-YYYY-NNNN.json_ for each, and
_advisories/index.json_, the list of them with each one's time of change,
severity, summary and SHA-256, which a machine reads first (*update*(8)).
Files are named by their SHA-256 on disk, like the other targets, so that the
server never replaces one. An advisory changed in the folder is signed
again, one removed leaves the targets, and a folder as the repository has it
already signs nothing: the command says so. *advisories --check* only checks
the folder, and prints how many advisories there are and the worst severity.
The update server takes _targets/advisories_ with the other folders of a
deploy, and refuses a file there that is not named by its hash or is not the
advisory its name says.

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
expiry, every release list and every channel, and the security advisories
with their index, which must agree, but none of the files.
With *--json* it prints, as JSON, each role's version and expiry, every
release with its rollout and withdrawal, every firmware set with its
rollout, every channel and every security advisory's index entry: what
*ob updates status* reads for Dukovo Ops. A server that has had nothing
deployed has no timestamp; *served* then says it serves no releases yet,
or with *--json* prints *{"url": ..., "published": false}*, and succeeds.

# OPTIONS

*--root* _file_
	Check against the root in _file_ instead of the system's; for
	*fetch* and *medium*, the first root to read the server from; for
	*sums*, the root that names the release key that signs.

*--version* _version_
	The release to check, when the repository holds several; the
	version of the release *make* makes; that of the firmware set
	*firmware* adds; or, for *sums*, that of the images, which is typed
	to sign.

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

*--advisories* _dir_
	With *channel*, the folder of security advisories to sign with the
	channel.

*--check*
	With *advisories*, only check the folder.

*--tree* _dir_
	The slot to make a release of: what _/system_ will hold. For
	*offer* and *medium*, the tree the release list was made of.

*--keys* _dir_
	The keys *keys* made.

*--yubikey-helper* _file_
	Sign on a YubiKey, through the helper _file_, tools/yubikey-piv.py
	in the source tree, which asks for the PIN and the touch: with its
	key in slot 95 for *offer*, *channel*, *advisories*, *pull* and
	*sums*, which the newest root must name for targets.

*--serial* _number_
	The YubiKey to use, by the serial number on its label, when more
	than one is plugged in.

*--record* _file_
	For *offer* and *sums*, where to write the Rekor entry of the
	signature.

*--yes*
	For *offer* and *sums*, sign without asking for the version to be
	typed; for *next-root*, without asking for *root* _N_.

*--roots* _metadata_
	For *fetch*, the folder of the roots the source tree holds,
	_releases/metadata_, whose roots after the first are added to what
	the server serves.

*--repo* _dir_
	The update repository; for *next-root*, the one to write the new
	root into and sign the targets of again.

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
	Where *make* and *medium* lay out the repository, and *fetch*
	writes it; for *sums*, the folder of _SHA256SUMS_ and its signature; for
	*list*, the file that takes the list, in place of the standard
	output; for *rescue*, the EFI system partition's mount point; for
	*pack*, the update file; for *online-key*, the public half of the
	key.

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

*sysupdate*(8), *update*(8), *dukovo-boot*(7), *dukovo-rescue*(8),
*spark*(8), *rdisk*(8), *ob*(1)

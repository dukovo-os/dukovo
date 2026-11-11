bundle(1) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

bundle - make, check, sign, pack, install, update and run bundles, and say what they may do

# SYNOPSIS

*bundle new* _id_ [_folder_] [*--name* _name_] [*--publisher* _name_] [*--license* _expression_]

*bundle check* [_folder_ | _file_]

*bundle schema*

*bundle upgrade-manifest* [_folder_ | _file_]

*bundle key*

*bundle sign* [_folder_] [*--key* _secret-key-file_]

*bundle pack* [_folder_] [*--key* _secret-key-file_] [*-o* _file_]

*bundle verify* [_folder_ | _file_*.dkv*] [*--key* _public-key_]

*bundle unpack* _file_*.dkv* _folder_

*bundle feed* _folder_ [_file_*.dkv*...] [*--key* _secret-key-file_] [*--days* _n_]

*bundle install* _file_*.dkv* [*--unsigned*] [*--for-everyone*] [*--yes*]

*bundle install* _id_ [*--from* _catalog_] [*--for-everyone*] [*--yes*]

*bundle search* _word_...

*bundle catalog*

*bundle catalog add* _name_ _url_ _root-file_

*bundle catalog remove* _name_

*bundle update* [_id_...] [*--check*] [*--for-everyone*] [*--yes*]

*bundle update --automatic* *on*|*off* [*--for-everyone*]

*bundle rollback* _id_ [*--for-everyone*]

*bundle hold* _id_ [*--for-everyone*]

*bundle unhold* _id_ [*--for-everyone*]

*bundle history* [_id_]

*bundle default* _name_ [_line_ | *--clear*] [*--for-everyone*]

*bundle list*

*bundle info* _id_

*bundle remove* _id_ [*--for-everyone*] [*--keep-data* | *--purge*]

*bundle run* [*--command* _name_] _id_ [_argument_...]

*bundle grant* _id_ _permission_ [_folder_*=read* | _folder_*=read-write* | _app-id_]...

*bundle revoke* _id_ _permission_

# DESCRIPTION

A bundle is an app, a command-line tool or a service in one folder,
which its _manifest.json_ describes (*manifest.json*(5)). bundle makes
and checks manifests with the same reader that installs bundles, so what
*bundle check* says is what installing will say. It signs a bundle,
packs it into one _.dkv_ file to share, and checks and unpacks one. It
installs, lists and removes bundles through *bundled*(8), as apt and
dpkg do packages, runs them, each in a sandbox of its own, and gives
them the permissions you allow. It finds bundles in the catalogs this
computer trusts and installs them from there (CATALOGS), and updates
them from there, keeping the version each update replaced to go back to
(UPDATES).

A signed bundle has _files.sha256_, the SHA-256 of each of its other
files as *sha256sum*(1) writes them, and _signature_, minisign's Ed25519
signature of that list, whose first line names the key that made it. The
signature thus covers every file, and the bundle checks the same way as a
folder and as a _.dkv_ file, which is a zip of the folder.

*new* _id_ [_folder_]
	Makes _folder_, or a folder named for the last part of _id_, with
	a _bin_ folder and a _manifest.json_ for a bundle of one program and
	its command, which runs in the bundle's sandbox and asks for no
	permission. _id_ is reverse DNS, lowercase, such as
	*org.example.notes*. *--name* gives what people call it (the last
	part of _id_ unless given), *--publisher* who publishes it (your
	user name unless given), and *--license* its license as an SPDX
	expression, *LicenseRef-Proprietary*, your own terms, unless given.
	It writes over no manifest that is there already.

*check* [_folder_ | _file_]
	Checks the manifest of the bundle in _folder_, the current folder
	unless given, or the manifest _file_. Each error is a line of its
	own on standard error, as _file_*:*_line_*:* _why_; a note, such as
	a permission asked for without a *why*, goes to standard output.
	Given a folder, it also checks that each program the manifest names
	is there, for each architecture, and may be run, and that its icon
	is there. It prints _file_*: ok:* _id_ _version_ when all is well,
	and exits with 1 when anything is wrong.

*schema*
	Prints the JSON Schema (draft 2020-12) of the manifest, made from
	the types the reader uses, for an editor that checks a manifest as
	it is written.

*upgrade-manifest* [_folder_ | _file_]
	Brings a manifest of an earlier version up to the newest this
	Dukovo reads. Version 1 is the first, so for now it checks the
	manifest and says it is the newest.

*key*
	Prints your developer key, a minisign public key (*RW* and 54 more
	characters), making the key first if you have none. The developer
	key signs your own builds; *bundle new* makes it the first time.

*sign* [_folder_] [*--key* _secret-key-file_]
	Checks the bundle in _folder_, the current folder unless given, as
	*check* does, then writes its SBOM, _sbom.spdx_, if it has none,
	the list of its files, _files.sha256_, and the signature of the list,
	_signature_. It signs with your developer key, or with the minisign
	secret key _secret-key-file_, which must have no password
	(*minisign -G -W* makes one). An SBOM written here lists the bundle
	alone; list the libraries the bundle carries in your own, so that
	their flaws are found.

*pack* [_folder_] [*--key* _secret-key-file_] [*-o*, *--output* _file_]
	Packs the bundle in _folder_ into _id_*-*_version_*.dkv* in the
	current folder, or _file_. A folder whose signature checks is packed
	as it is; one that is not signed or has changed since is signed
	first, as *sign* does, and *--key* signs it again with that key. The
	same folder packs to the same bytes.

*verify* [_folder_ | _file_*.dkv*] [*--key* _public-key_]
	Checks a bundle's signature and every file against it: no file more
	or less than its list holds, and each as it was signed. It prints
	_what_*: ok:* _id_ _version_, the files, and whose key signed it and
	when, or why it does not check, and exits with 1. With *--key*, a
	public key (*RW*...) or a minisign public key file, it also fails
	unless that key signed the bundle.

*unpack* _file_*.dkv* _folder_
	Checks the archive and unpacks it into _folder_, which it makes and
	which must not be there. The archive is read by a process of its own
	that holds the archive, the new folder and a pipe for its report, and
	nothing else, not even a root: a path in the archive cannot lead out
	of the folder. It refuses encryption, zip64, links, device files,
	set-ID bits, names outside the folder or twice, files that unpack to
	more than 100 times the archive's size or more than the disk has
	free, and any file not as it was signed; then it removes the folder.
	A program the manifest names may be run once unpacked, as may a file
	the archive marks so.

*feed* _folder_ [_file_*.dkv*...] [*--key* _secret-key-file_] [*--days* _n_]
	Signs the update feed of bundles that you publish yourself, from no
	catalog (OWN FEEDS): a small TUF repository in _folder_, which it
	makes the first time. It adds each file given, and signs every role
	anew with the key the file *--key* names, or your developer key,
	which must be the one the files' manifests name in *update.key*.
	The feed is good for _n_ days, 30 unless given, at most 180: run
	*bundle feed* _folder_ again before then. Serve _folder_*/site* at
	the address *update.feed* names; _folder_*/repository* holds what it
	signed and _folder_*/store* the files, which are signed once each.

*install* _file_*.dkv* [*--unsigned*] [*--for-everyone*] [*--yes*]
	Installs the bundle for you, in your _applications_ folder, or with
	*--for-everyone*, which only an administrator may, through
	*elevate*(8), in _/applications_. *bundled*(8) unpacks and checks it,
	and shows the install sheet: what it is, whose key signed it and
	whether this computer knows the key, its size, the permissions it
	asks for in the system's own words, and its commands. bundle asks
	*Install it?*; *--yes* answers for scripts. Installed, the bundle is
	sealed: root's and read-only, so nothing changes it. A file that a
	catalog this computer trusts lists installs as that catalog's, with
	no warning (CATALOGS). A bundle signed by your developer key installs
	with no warning for you; any other key this computer does not know
	installs after a warning, and from then on only that key may install
	that ID. A bundle that runs services installs only for everyone:
	its services run as accounts of their own, which the sheet names
	(*bundled*(8), ADR 0385). A file with no signature is refused, unless
	*--unsigned* says to install it anyway (UNSIGNED BUNDLES).

*install* _id_ [*--from* _catalog_] [*--for-everyone*] [*--yes*]
	Fetches the newest version of _id_ from the first catalog that lists
	it, or from _catalog_, and installs it as a file, with the catalog's
	name on its sheet. bundle takes what it is given for a file when it
	has a _/_, ends in _.dkv_ or names a file that is there.

*search* _word_...
	Reads the catalogs anew and lists the newest version of each bundle
	whose ID or name holds every _word_, in any case: ID, version,
	catalog, its known flaws (FLAWS) and name. It exits with 1 when none
	does. A catalog that cannot be read is left out, and bundle says
	why.

*catalog*
	Lists the catalogs: name, on or off, how many bundles each listed
	when bundled last read it, when that was, followed by *!* once what
	it read has expired, and where it is.

*catalog add* _name_ _url_ _root-file_
	Adds a catalog, which only an administrator may, through
	*elevate*(8). _name_ is a lowercase letter, then up to 31 lowercase
	letters, digits and *-*; _url_ is where its repository is, by
	*https*, *http* or *file*; and _root-file_ is the root it starts
	from, which its publisher gives. bundled reads the catalog from that
	root before it keeps anything.

*catalog remove* _name_
	Removes a catalog, through *elevate*(8). One the system comes with
	is turned off instead, so that it stays off.

*update* [_id_...] [*--check*] [*--for-everyone*] [*--yes*]
	Updates your bundles, or each _id_ given, or with *--for-everyone*
	everyone's, through *elevate*(8), each to the newest version the
	catalog it came from lists; see UPDATES. One that asks for no more
	than the version it replaces updates without a question; one that
	asks for more shows its sheet and asks, which *--yes* answers. A held
	one is left as it is, and bundle says so. A service bundle's new
	version that fails its health check goes back by itself, with its
	data, and is held (*bundled*(8)). With *--check*, it lists
	what would update, with the version installed, the newest and the
	catalog, or *its own feed*, and changes nothing.

*update --automatic* *on*|*off* [*--for-everyone*]
	Says whether your bundles, or everyone's, through *elevate*(8),
	update by themselves, daily; it is off unless you turn it on. Turned
	on, they update at once. One whose new version asks for more waits
	for you to run *bundle update*, and *bundle history* says so.

*rollback* _id_ [*--for-everyone*]
	Goes back to the version the last update replaced, and holds the
	bundle there. Its data, as that version left it, comes back at its
	next run, and its services' data at once. Running it again goes
	forward again.

*hold* _id_ [*--for-everyone*], *unhold* _id_ [*--for-everyone*]
	Holds a bundle at its version, so that no update changes it, or
	lets it update again.

*history* [_id_]
	Shows what was done to your bundles and everyone's, or to _id_, the
	newest last: each install, update, going back, removal, hold and
	release, update that waits for you and update that failed, with
	when, who asked or that it did so by itself, and, for a version
	that failed its health check, why bundled went back. It also shows
	the known flaws the daily check found in an installed version
	(FLAWS).

*default* _name_ [_line_ | *--clear*] [*--for-everyone*]
	With _line_, makes it your default line of the ID _name_ names, its
	ID or one of its commands: the line that runs unless a project asks
	for another (LINES); with *--for-everyone*, everyone's, which an
	administrator sets through *elevate*(8), and which your own comes
	before. *--clear* forgets it. With neither, says which line runs in
	the working folder and why, the lines installed, the defaults, and
	the files a project chooses with.

*list*
	Lists the bundles you can run, yours and everyone's: ID, version,
	whose, its known flaws (FLAWS), and name.

*info* _id_
	Shows an installed bundle's sheet, with every known flaw of its
	version, what you said of its permissions
	and since when, when it was installed, where, and where its data is,
	which version it can go back to, and whether it is held.

*remove* _id_ [*--for-everyone*] [*--keep-data* | *--purge*]
	Removes your bundle, or with *--for-everyone* one installed for
	everyone, through *elevate*(8). It asks whether to delete its
	settings and data, _~/app-data/id_, and the copies of it kept for
	going back, too; *--keep-data* and *--purge* answer for scripts. The
	version kept to go back to goes with it. Its services stop and go;
	their accounts and data, in _/shared/services/id_, stay unless
	*--purge*. Kept data is picked up by a later version from
	the same publisher; what you said of its permissions goes with its
	data and stays with kept data.

*run* [*--command* _name_] _id_ [_argument_...]
	Runs the bundle's command _name_, or its first command, or with no
	command its only program, with each _argument_, options included;
	see RUNNING.

*grant* _id_ _permission_ [_folder_*=read* | _folder_*=read-write* | _app-id_]...
	Gives the bundle _permission_ from now on, as its manifest asks for
	it, or with less: *documents=read* for its *documents=read-write*,
	or some of the folders or apps it names. bundled asks you first, on
	the console's bottom line, *Y* or *N*, so that no program can give
	itself a permission by running bundle; on a terminal that is not a
	console's it refuses. It gives only what the bundle asks for, and
	nothing an administrator gives.

*revoke* _id_ _permission_
	Takes _permission_ away from the bundle, now and from now on,
	without a question; a run of it that holds it stops at once (see
	PERMISSIONS). *bundle grant* gives it back.

# RUNNING

A bundle's commands are on your *PATH*: *bundled*(8) links each, by its
name, in _/applications/commands_ or your _applications/commands_, to
bundle, which run by a name that is none of its own runs the bundle
command of that name. *notes* and *bundle run org.example.notes* run the
same program.

Before anything runs, bundle asks bundled which bundle it is and what it
recorded at install, and checks the bundle's folder against that: still
sealed (root's, mode 0555), its signed list the one recorded and signed
by the key recorded, its manifest the one the list names, and the
program a sealed file the list names. A bundle that fails any check does
not run, and bundle says why.

A command whose manifest says it runs with your access runs as a program
of the system's, with all you hold, once you allowed
*runs-with-your-access*, asked with a warning the first time it runs;
the install sheet says so plainly too. Run through *elevate*(8), it
runs as root, and root's question is asked in the pane elevate was
typed in (ADR 0493). Any other runs in the bundle's
sandbox (ADR 0225), a process whose root is a view of:

- _/system/commands_, _libraries_, _resources_ and _settings_,
  _/system/live/notify_, _/processes_ and _/kernel_, read-only;
- _/devices/null_, _zero_, _random_ and _urandom_;
- the bundle's own folder, read-only, at its path;
- its data folder, _~/app-data/id_, which it may change, made if it
  is not there, mode 0700, and its home;
- a _/tmp_ of its own, which is _/tmp/.app-id_ of yours;
- the links of _/_ and _/system_, so _/bin/sh_ is there;
- what you allowed it (PERMISSIONS).

Nothing else of your home, nor of anyone's, is in it. It holds the
network only as its manifest asks: with *network.outbound*, to connect
out; with *network.listen*, to listen on those ports, out of those you
may (from 1024 for anyone but an administrator); and without either, no
network at all, so it cannot make a socket. Its environment is clean:
*HOME*, *USER*, *LOGNAME*, *PATH*=_/system/commands_, *TERM*, and your
*LANG*, *LC\_\** and *TZ*. Its descriptors 0 to 2 are yours; bundle
waits for it and exits with its status.

# LINES

A bundle whose manifest has a *line*, such as Node.js 22, is a bundle of
its own, _id_*@*_line_ (ADR 0387): *org.nodejs.node@22* installs beside
*org.nodejs.node@20*, updates within line 22, and never replaces it. Each
command, *bundle*, *info*, *update*, *rollback*, *hold* and *remove*,
takes the full ID; *bundle install* _id_ from a catalog takes its newest
line, and _id_*@*_line_ that line.

Lines of one ID may add the same commands, and each line's commands are
linked a second time as _name_*-*_line_: *node-22* always runs line 22.
When several lines installed for you or for everyone add the command
you run, the one that runs is:

. the line a project asks for: from the working folder up to _/_, the
  nearest folder holding one of the files the manifest's *line_files*
  names, such as _.nvmrc_; its first word, a *v* before it dropped, is
  a version or a line, and the line it is of, or the newest of it,
  runs. A version no installed line is or holds stops the command,
  which says so and which file asks for it; a word that is no version,
  such as *lts/iron*, is passed over;
. your default (*bundle default*), then everyone's;
. the newest line installed.

The line an add-on was added with comes before all of these, while the
add-on runs (ADD-ONS).

# ADD-ONS

A line whose manifest has *add_ons*, such as Node.js 22, keeps what its
package manager installs for each person in a folder of theirs,
_~/applications/add-ons/id@line_ (ADR 0388): its commands that run with
your access find the folder in the variables the manifest names, such as
*NPM_CONFIG_PREFIX*, so *npm install -g* installs there with no
*elevate*. When the folder of the add-ons' commands changed, as a run
of such a command ends, *bundled*(8) links each of them beside your
commands, where they are on your *PATH* until they go. Through
*elevate*, an administrator's add-ons are everyone's, in
_/applications/add-ons/id@line_.

An add-on's command runs with your access, as a program of yours, on
the line it was added with: what it runs of the line's commands is of
that line, whichever runs elsewhere, as *BUNDLE_LINES* says. A command
the system has, or a bundle adds, comes before an add-on's of the same
name, and none named as a system command is linked.

The add-ons are the line's data: *bundle remove* takes their commands
away, and with *--purge* the folder too; with *--keep-data* they stay,
and come back when the line is installed again.

# PERMISSIONS

What else a program gets is what you allow (ADR 0224), of the
permissions its bundle asks for, as many as the program keeps
(*programs.*_name_*.permissions*). One given at install
(*network.outbound*, *network.listen*, *start-at-login*) or always
(*notifications*) is given unless you took it away. One asked on first
use or with a warning is asked the first time the bundle runs wanting
it, on the console's bottom line, which *consoled*(8) draws and no
program can draw, read or answer:

```
 [permission] Notes (org.example.notes) asks to: use your documents folder, to read and change it      O: once    A: always    D: don't allow
```

*O* allows it this time, *A* from now on, and *D* not, from now on;
*Esc* or *Ctrl+C* close the question, which leaves it out this time.
One that reaches what other apps keep, such as all of your files, says
*[warning]*, in white on red. On a narrow console the words are cut,
never the keys. A program that requires a permission you did not allow
does not run, saying how to give it; one that asks for it as optional
runs without it. Only the consoles ask: on another terminal, give it
with *bundle grant* at a console first.

Given, a permission is part of the program's view, each place at its
real path:

- *files.folder*: the folders of yours it names, such as
  _~/documents_, read-only or not as given, each made if it is not
  there;
- *files.home*: all of your home, which holds its data folder;
- *files.volumes*: _/volumes_, the drives you plug in;
- *files.shared*: the folders of _/shared_ it names;
- *files.app-data*: the data folders of the other apps it names, to
  read;
- *devices.serial*: the serial ports' terminals there are when it
  starts, _/devices/ttyACM_*N* and _/devices/ttyUSB_*N*, those you can
  open;
- *network.local*: _/system/live/discoverd_, whose socket says what
  *discoverd*(8) finds on your networks.

Each program also reaches its own entries of your keyring (ADR 0396):
the runner serves them for as long as it runs, at authd's path,
_/system/live/authd_, so *keyring*(1) works in the sandbox. The name
_name_ an app gives is the entry *app* _ID name_ of your keyring, which
*keyring list* shows outside it, and the app lists only its own. It
may set, get, list and remove them, and nothing else of authd's. Given
*keyring.shared*, asked with a warning, it reaches every entry by its
own name, as your programs do.

Its network is given as a capability (ADR 0337): *network.outbound*
reaches machines beyond your networks, the internet, and this computer's
own services; *network.local* the machines on your networks (their
addresses, broadcasts and multicast groups, link-local addresses), and
joining groups. A program with *network.outbound* alone cannot reach
your printer, your router's page or a machine next to it; one with
*network.local* alone reaches nothing beyond them.

*processes.debug* cannot be given yet, nor any permission an
administrator gives: a program that requires one does not
run. A later version that asks for more than you allowed is asked again;
one that asks for the same or less is not. What you said holds only for
the publisher's key it was said of.

Taking a permission away with *bundle revoke* reaches each run of the
bundle that holds it at once. A view keeps what is attached to it and a
socket outlives the capability it was made with, so the program and
what it started are stopped, *SIGTERM* and two seconds later *SIGKILL*,
and bundle exits with 143; its next run goes without.

# CATALOGS

A catalog is a repository of bundles, signed as The Update Framework
says (ADR 0380), that *bundled*(8) reads and installs from (ADR 0383).
Dukovo comes with its own, *dukovo*; an administrator may add others.
bundled reads each one from the root it was set with, and checks every
file it fetches, metadata and bundles, against the keys that root and
those after it name.

A catalog lists each bundle's _.dkv_ file by its SHA-256 and length, so
it vouches for that very file, wherever it came from: one *bundle
install* fetched, or one copied from a disk. Such a file installs with
no warning, and its sheet names the catalog. From then on its ID
installs, for you or for everyone, only from a file that same catalog
lists, whatever key signed it. A catalog whose metadata expired vouches
for nothing until it is read again.

A file no catalog lists installs as before: signed by your developer key
with no warning, or by any other key after one, which also says that no
catalog this computer trusts lists it.

# FLAWS

A catalog lists the known flaws of its bundles (ADR 0389): its update
server matches the components each bundle's SBOM, _sbom.spdx_, lists
with the advisories OSV gathers, and signs the list each day. Each time
bundled reads the catalog, it keeps the list, while the list is less
than a week old.

*bundle search* and *bundle list* show what it says in the FLAWS column:
*none*; how many and the worst, *low*, *medium*, *high*, *critical* or
*unknown*, such as *2, high*; or *-* when nothing tells, for a bundle
from no catalog, one its catalog's list did not look at, or one whose
catalog has no list that is good. The sheet of a bundle from a catalog
says so too, and names the five worst, each with its advisory, the
component it is in, and the version of that component that fixes it;
*bundle info* names them all.

Some minutes after *bundled*(8) starts, then daily, it reads anew each
catalog your bundles come from, and writes the flaws of each installed
version in the history once for each change. Nothing is removed or
blocked: whether to update, go back or remove a flawed bundle is
yours.

# UPDATES

A bundle updates from the catalog it is bound to (ADR 0384): one
installed from a file no catalog listed updates from its own feed, when
its manifest names one (OWN FEEDS), or else only from a newer file.
An update is an install, with every check an install has. Its sheet is
shown, and you are asked, only when the new version asks for more than
the one it replaces: a permission the old one did not ask for, more of
one, or a command that runs with your access. The sheet says which
version it replaces and marks what is new in this version.

The version an update replaces is kept beside the new one, sealed as
it was, to go back to with *bundle rollback*; only the last is kept.
Going back holds the bundle, so that no update undoes it; *bundle
unhold* lets it update again.

The first time another version of an app runs for you, bundle keeps a
copy of the app's data as the version that ran before left it, in
_~/app-data/.snapshots/id/version_, and says so; the new version goes on
with the data. When you go back to that version, its copy comes back in
place of the data, and what the newer version left is kept in turn. A
copy is kept for 30 days.

Updating by themselves, once turned on, runs some minutes after
*bundled*(8) starts and then daily. An update that asks for more than
the version it replaces never installs by itself: it waits for *bundle
update*.

# OWN FEEDS

A bundle that no catalog lists can still update, from a feed its
publisher signs (ADR 0390). Its manifest's *update* names the feed, a
small TUF repository served by *https* or *http*, and the minisign key
it is rooted in (*manifest.json*(5)); the sheet says *It updates from
its own feed at* that address. *bundled*(8) reads the feed that the
installed version names with the client it reads catalogs with,
starting from the feed's first root, which must name that key alone for
root and be signed by it, so a feed cannot name a new key for itself.
From there TUF refuses an old feed served again, one past its time, and
files other than those it signs. bundled fetches the newest version the
feed lists of the bundle's ID and line, and installs it as any file is:
signed by the key the bundle is bound to, with every check an install
has. *bundle history* says *from its feed*. *bundle feed* makes and
signs such a feed.

# UNSIGNED BUNDLES

A bundle with no signature at all, such as one zipped by hand, is
refused: nothing says who made it, or that it is as they made it.
*bundle install --unsigned* _file_*.dkv* is the override, given for
each bundle (ADR 0391). It warns, then signs the file with your
developer key, in a copy in your temporary folder, which only you see,
and installs that copy, as your own build: the bundle's ID is bound to
your developer key, so a later version of it needs *--unsigned* too,
and the publisher's own signed version installs only once this one is
removed. A file whose signature fails is never overridden: someone
changed it after it was signed. A folder with no signature is signed by
*bundle pack*, which makes its file.

# FILES

_$XDG_DATA_HOME/bundle/developer.key_, _developer.pub_
	Your developer key, a minisign secret key without a password, mode
	0600, and its public half. _$XDG_DATA_HOME_ is _~/.local/share_
	unless set.

_/applications_, _~/applications_
	The bundles for everyone and your own, sealed, and their
	_commands_ folders.

_~/app-data/id_
	A bundle's settings and data, its home when it runs.

_~/app-data/.snapshots/id_
	The copies of a bundle's data that each version left, for 30 days,
	and _.ran_, the version that last ran.

_/applications/.previous-id_, _~/applications/.previous-id_
	The version of a bundle that its last update replaced, sealed.

_.nvmrc_, _.node-version_, ...
	A project's file that says which line runs a command, as a bundle's
	*line_files* name them (LINES).

_~/applications/add-ons/id@line_, _/applications/add-ons/id@line_
	What a line's package manager installed for you, or for everyone
	(ADD-ONS).

_/system/state/grants/uid_
	What each person said of each bundle's permissions, which only
	*bundled*(8) writes.

_/system/settings/catalogs/name_
	A catalog: _url_, _root.json_, and _off_ when it is turned off
	(*bundled*(8)).

# EXIT STATUS

0 when the manifest reads, and 1 when it does not, a file it names is
missing, a bundle does not check, or the command line is wrong. *install*
and *remove* exit with 1 when bundled refused, and *grant* and *revoke*
when it refused or you said no. *run* and a bundle's
command exit with the program's status, 128 and the signal's number for
one a signal ended, or 1 when it did not run.

# EXAMPLES

```
$ bundle new org.example.notes
wrote notes/manifest.json
put the program in notes/bin/notes, then check the bundle with `bundle check notes`
$ bundle check notes
notes/manifest.json: program notes for x86_64 is not at bin/notes
$ bundle pack notes
notes: signed org.example.notes 0.1.0, 3 files, with your developer key RWS...
wrote org.example.notes-0.1.0.dkv: org.example.notes 0.1.0, 3 files, 1.7K, signed by your developer key RWS...
$ bundle unpack org.example.notes-0.1.0.dkv /tmp/notes
/tmp/notes: unpacked org.example.notes 0.1.0, 3 files, signed by your developer key RWS...
$ bundle install org.example.notes-0.1.0.dkv
notes 0.1.0, for yourself
  org.example.notes, by alice
  ...
Install it? [y/N] y
installed org.example.notes 0.1.0, for you
$ notes
```

Its first run asks about the permissions it asks for on first use, on
the bottom line. To give it your documents to read only, ahead:

```
$ bundle grant org.example.notes files.folder documents=read
gave org.example.notes files.folder (documents=read)
$ bundle revoke org.example.notes files.folder
took files.folder away from org.example.notes
```

The same checks with other tools, on any system:

```
$ cd notes && minisign -Vm files.sha256 -x signature -P RWS...
$ sha256sum -c files.sha256
$ unzip -t org.example.notes-0.1.0.dkv
```

From a catalog:

```
$ bundle search ripgrep
ID                            VERSION  CATALOG  FLAWS  NAME
io.github.burntsushi.ripgrep  15.2.0   dukovo   none   ripgrep
$ bundle install io.github.burntsushi.ripgrep
ripgrep 15.2.0, for yourself
  io.github.burntsushi.ripgrep, by Dukovo catalog
  From catalog dukovo, which this computer trusts: it lists this very file.
  31 files, 1.8 MB
  Known flaws: none, by catalog dukovo's list of 2026-11-03.
  ...
Install it? [y/N] y
installed io.github.burntsushi.ripgrep 15.2.0, for you
```

Updating, going back, and holding there:

```
$ bundle update --check
ID                            INSTALLED  NEWEST  CATALOG
io.github.burntsushi.ripgrep  15.2.0     15.3.0  dukovo
1 to update, 0 up to date
$ bundle update
installed io.github.burntsushi.ripgrep 15.3.0 in place of 15.2.0, for you
1 updated, 0 not, 0 up to date
$ bundle rollback io.github.burntsushi.ripgrep
went back to io.github.burntsushi.ripgrep 15.2.0, from 15.3.0; ...
$ bundle update --automatic on
Your bundles update by themselves from now on, daily, from their catalogs; ...
```

# SEE ALSO

*bundled*(8), *manifest.json*(5), *service.toml*(5), *consoled*(8), *authd*(8), *elevate*(8), *sha256sum*(1), *zip*(1), *systools*(1)

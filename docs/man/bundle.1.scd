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

*bundle key rollover* *--id* _id_ *--from* _old-secret-key-file_ *--to* _new-public-key_ [*--add-to* _file_]

*bundle sign* [_folder_] [*--key* _secret-key-file_]

*bundle pack* [_folder_] [*--key* _secret-key-file_] [*-o* _file_] [*--zstd*]

*bundle verify* [_folder_ | _file_*.dkv*] [*--key* _public-key_]

*bundle unpack* _file_*.dkv* _folder_

*bundle feed* _folder_ [_file_*.dkv*...] [*--key* _secret-key-file_] [*--days* _n_]

*bundle install* _file_*.dkv* [*--unsigned*] [*--for-everyone*] [*--yes*]

*bundle install* _id_ [*--from* _catalog_] [*--sha256* _hex_] [*--for-everyone*] [*--yes*]

*bundle search* _word_...

*bundle search --json* [_word_...]

*bundle sheet* _id_ [*--from* _catalog_] [*--json*]

*bundle catalog*

*bundle catalog add* _name_ _url_ _root-file_

*bundle catalog remove* _name_

*bundle update* [_id_...] [*--check*] [*--for-everyone*] [*--yes*]

*bundle update* [_id_...] *--for-everyone* *--tonight* [*--yes*]

*bundle update* _id_... *--for-everyone* *--not-tonight*

*bundle update --automatic* *on*|*off* [*--for-everyone*]

*bundle rollback* _id_ [*--for-everyone*]

*bundle hold* _id_ [*--for-everyone*] [*--why* _text_]

*bundle unhold* _id_ [*--for-everyone*]

*bundle history* [_id_]

*bundle default* _name_ [_line_ | *--clear*] [*--for-everyone*]

*bundle list* [*--json*]

*bundle info* _id_ [*--json*]

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
	is there, for each architecture, and may be run, that a Linux
	bundle's programs are Linux programs or scripts and none is
	Dukovo's (ADR 0625), and that its icon is there. It prints _file_*: ok:* _id_ _version_ when all is well,
	and exits with 1 when anything is wrong.

*schema*
	Prints the JSON Schema (draft 2020-12) of the manifest, made from
	the types the reader uses, for an editor that checks a manifest as
	it is written.

*upgrade-manifest* [_folder_ | _file_]
	Brings a manifest of an earlier version up to the newest this
	Dukovo reads, version 2: a version 1 manifest is a version 2
	manifest without *runtime*, so its *manifest_version* is changed in
	place and nothing else. A manifest that is the newest already is
	checked and left as it is.

*key*
	Prints your developer key, a minisign public key (*RW* and 54 more
	characters), making the key first if you have none. The developer
	key signs your own builds; *bundle new* makes it the first time.

*key rollover* *--id* _id_ *--from* _old-secret-key-file_ *--to* _new-public-key_ [*--add-to* _file_]
	Writes a link of the file _rollover_, with which a bundle moves its
	ID from one key to another. An ID is bound to the key that signed
	its first install, and a bundle signed by another key is refused
	unless it carries a link, signed by the key the ID is bound to,
	that moves it. The link says that _id_ moves from the key of the
	minisign secret key _old-secret-key-file_, which must have no
	password, to _new-public-key_ (*RW*... or a public key file), as of
	now. It is printed, or with *--add-to* added to _file_, made if it is
	not there, after one empty line, for a bundle that moves more than
	once; the file is refused if it is not a rollover file, if its last
	link does not move to the key _old-secret-key-file_ holds, or if it
	moves another ID, a key it has been at already, or at a later time.
	At most 8 links are kept.

	The file _rollover_ goes at the top of the bundle's folder, beside
	_manifest.json_. *sign*, run with the new key, signs it with the
	rest of the bundle's files, and the computer follows the links from
	the key it has bound to the key that signed the bundle. A link never
	moves an ID back: once the ID is bound to the new key, a bundle the
	old key signs is refused.

*sign* [_folder_] [*--key* _secret-key-file_]
	Checks the bundle in _folder_, the current folder unless given, as
	*check* does, then writes its SBOM, _sbom.spdx_, if it has none,
	the list of its files, _files.sha256_, and the signature of the list,
	_signature_. It signs with your developer key, or with the minisign
	secret key _secret-key-file_, which must have no password
	(*minisign -G -W* makes one). An SBOM written here lists the bundle
	alone; list the libraries the bundle carries in your own, so that
	their flaws are found.

*pack* [_folder_] [*--key* _secret-key-file_] [*-o*, *--output* _file_] [*--zstd*]
	Packs the bundle in _folder_ into _id_*-*_version_*.dkv* in the
	current folder, or _file_. A folder whose signature checks is packed
	as it is; one that is not signed or has changed since is signed
	first, as *sign* does, and *--key* signs it again with that key. The
	same folder packs to the same bytes. A file is deflated, or stored
	when that is not smaller; with *--zstd* it is packed with zstd
	instead, which unpacks faster than deflate but which many _unzip_
	programs do not read. A bundle may be of
	any size and hold up to 1,048,576 files; a file of more than 256 MiB
	is read twice, not held in memory.

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
	of the folder. It refuses encryption, methods other than stored,
	deflate and zstd, links, device files, set-ID bits, names outside the
	folder or twice, files that unpack to more than 100 times the
	archive's size or more than the disk has free, and any file not as it
	was signed; then it removes the folder.
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
	To publish a version again with other files, give it the next build
	number, as in *1.1.0\_2* (*manifest.json*(5)); a file with no other
	files than the build it follows is refused, and one of a version
	already signed is kept as it was signed. A file beside it of the
	same name ending in *.reason* in place of *.dkv*, when there is one,
	holds one line, at most 200 bytes, that computers show with the
	update as why it was built again.

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

*install* _id_ [*--from* _catalog_] [*--sha256* _hex_] [*--for-everyone*] [*--yes*]
	Fetches the newest version of _id_ from the first catalog that lists
	it, or from _catalog_, and installs it as a file, with the catalog's
	name on its sheet. bundle takes what it is given for a file when it
	has a _/_, ends in _.dkv_ or names a file that is there. With
	*--sha256*, only the file whose SHA-256 is _hex_, 64 hexadecimal
	digits, installs, which is the one whose sheet *bundle sheet*
	showed: if the catalog lists another file by now, a newer
	version, bundled says so before it fetches anything, in words that
	end in *APP-102*, and nothing is installed (SHEETS).

*search* _word_...
	Reads the catalogs anew and lists the newest version of each bundle
	whose ID or name holds every _word_, in any case: ID, version,
	catalog, its known flaws (FLAWS) and name. It exits with 1 when none
	does. A catalog that cannot be read is left out, and bundle says
	why.

*search --json* [_word_...]
	Prints the same search as one line of JSON (JSON) in place of the
	table, and with no _word_ lists every bundle the catalogs list. A
	catalog that cannot be read is in the document, with why. That
	nothing matches is no failure, and the status is 0. That no catalog
	could be read is, and the status is 1: the document is printed
	first, naming each catalog and why, and the error follows on
	standard error, in words that end in *APP-103*. A _word_ that
	starts with *-* follows a *--*.

*sheet* _id_ [*--from* _catalog_] [*--json*]
	Shows the sheet that *install* _id_ would show, of the newest
	version of _id_ that the first catalog that lists it, or _catalog_,
	lists, and installs nothing (SHEETS). It is the sheet of an install
	for everyone, the one that can run services, and says on a last line,
	*It would not install*, why bundled would refuse it, if it would.
	*--json* prints the facts of the sheet as one line of JSON (JSON)
	instead, with which file it is. It exits with 1 when no catalog lists
	_id_, in words that end in *APP-101*, and when none could be read,
	which end in *APP-103*; that the install would be refused is no
	failure.

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
	one is left as it is, and bundle says so. With no _id_, one whose
	catalog sends its newest version out in waves waits for this
	computer's wave, and bundle says on which day it opens and why the
	waves are held, if they are; one named is taken at once (WAVES). A
	service bundle's new version that fails its health check goes back
	by itself, with its data, and is held (*bundled*(8)). With *--check*,
	it lists what would update, with the version installed, the newest
	and the catalog, or *its own feed*, and, for each that waits for its
	wave, why and how to take it now, and changes nothing. A row whose
	installed and newest versions differ only in build ends with
	*(rebuild)*, and, when the catalog gave a reason, *rebuilt:* and the
	reason are printed under it (UPDATES).

*update* [_id_...] *--for-everyone* *--tonight* [*--yes*]
	Asks, through *elevate*(8), that the update of each _id_ given, or of
	every app everyone has that has one, is installed in tonight's
	window and not now: an app's services restart in the middle of the
	day when it updates, and this puts that off. The window is the hour
	before the restart time of *updates.toml*(5) (*update set --restart*),
	and 2:00 to 3:00 without one; see TONIGHT. An update that asks for
	more than the version it replaces shows its sheet, as above, and is
	not queued unless *--yes* says it is taken as it asks. A held app, and
	one with no update, are not queued: when apps are named and one of
	them cannot be, none is. Asking again for an app changes the version
	it will install to the newest one, and keeps when it was first asked.

*update* _id_... *--for-everyone* *--not-tonight*
	Takes each _id_ off tonight's window: its update waits for *bundle
	update* or for updating by themselves, as before.

*update --automatic* *on*|*off* [*--for-everyone*]
	Says whether your bundles, or everyone's, through *elevate*(8),
	update by themselves, daily; it is off unless you turn it on. Turned
	on, they update at once, and each one whose wave has not opened
	waits for it (WAVES). One whose new version asks for more waits for
	you to run *bundle update*, and *bundle history* says so.

*rollback* _id_ [*--for-everyone*]
	Goes back to the version the last update replaced, and holds the
	bundle there. Its data, as that version left it, comes back at its
	next run, and its services' data at once. Running it again goes
	forward again. It refuses a version that a catalog or a feed
	withdrew (WITHDRAWALS).

*hold* _id_ [*--for-everyone*] [*--why* _text_], *unhold* _id_ [*--for-everyone*]
	Holds a bundle at its version, so that no update changes it, or
	lets it update again. *--why* keeps a reason with the hold: one line
	of 1 to 200 characters, with no control character, which *list* and
	*update --check* say beside the bundle and which everyone who can
	run it sees, so it is not for what should stay private. It is the
	note of the hold's event in the history, so it goes with that event
	after the history's newest thousand or so; a hold of a bundle that
	is held already changes nothing, the reason it has included.

*history* [_id_]
	Shows what was done to your bundles and everyone's, or to _id_, the
	newest last: each install, update, going back, removal, hold and
	release, withdrawal of an installed version, update that waits for
	you and update that failed, with
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

*list* [*--json*]
	Lists the bundles you can run, yours and everyone's: ID, version,
	whose, its known flaws (FLAWS), and name; a bundle whose version a
	catalog or its feed withdrew says so, with the reason, and says if
	it is stopped. *--json* prints them as one line of JSON (JSON).

*info* _id_ [*--json*]
	Shows an installed bundle's sheet, with every known flaw of its
	version, what you said of its permissions
	and since when, when it was installed, where, and where its data is,
	and what the data takes when *bundled* has measured it (for a bundle
	installed for everyone that keeps data of its services),
	which version it can go back to, whether it is held, and whether
	its version was withdrawn. *--json* prints the same facts as one
	line of JSON (JSON).

*remove* _id_ [*--for-everyone*] [*--keep-data* | *--purge*]
	Removes your bundle, or with *--for-everyone* one installed for
	everyone, through *elevate*(8). It asks whether to delete its
	settings and data, _~/app-data/id_, and the copies of it kept for
	going back, too; *--keep-data* and *--purge* answer for scripts. The
	version kept to go back to goes with it. Its services stop and go,
	with their accounts and the folders they made in
	_/system/live/services_; only their data, in _/shared/services/id_,
	stays unless *--purge*. Installing the bundle again gives each
	account back its number, so the data is its own again. Kept data is
	picked up by a later version from the same publisher; what you said
	of its permissions goes with its data and stays with kept data.

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
with *bundle grant* at a console first. A program of the system's that
runs a command of a bundle installed for everyone as its own, as
*database restore* runs PostgreSQL's *pg\_restore*, checks the bundle as
*bundle run* does and asks nobody: an administrator asked for what it
does.

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

Every run follows *bundled*(8) so, whether it holds a permission or
not. When a catalog or a feed withdraws the version it runs as
malicious (WITHDRAWALS), bundle prints *bundle: id was stopped: why*
and stops the program and what it started in the same way. *bundle run*
of a stopped bundle, or a command of it, fails with "stopped by catalog
c: why; remove it, or *bundle update* id takes a fixed version".

# SOCKETS

A bundle can offer a socket to others (ADR 0385, 0477): its manifest names
it in *provides*, and a bundle that needs it names it in *uses* as
_PROVIDER_ID/NAME_ (*manifest.json*(5)). Installing the second bundle
grants it when the provider's *to* allows: for *"anyone"* at once, for
*"publisher"* when both are signed by one key, for *"approved"* when you
approve it: the sheet says "uses the socket _NAME_ of _PROVIDER_, which it
offers to the programs you approve", and your yes, or *--yes*, approves it
with the rest. For *"system"* never. The provider must be installed for
everyone first. A bundle that requires a socket it cannot have is not
installed, and one that only uses it is installed without. A bundle that
is not given the socket does not see it in its view, and the socket's mode
refuses it anyway. A person's own programs do not reach a socket offered
to the approved yet.

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

A catalog may also vouch for the publisher of a bundle, not only for
its files (ADR 0592): Dukovo's does for each bundle built from its own
ports. A file of such an ID, signed by another key, is refused unless
that catalog lists it, which keeps a file someone installed first from
holding the ID against the real app. If the ID is already bound here to
another key, installing it from that catalog replaces the app: the sheet
says *Warning*, names the old key and the day it was bound, and says the
app of that key is removed and its data set aside, not deleted. The old
bundle is removed, with its services' accounts, and its previous version
is not kept; its data, in _~/app-data/ID_ and its copies, moves to
_~/app-data/.set-aside/ID-YYYY-MM-DD/_ as *data* and *snapshots*, and
for an install for everyone the same in each person's home, and
_/shared/services/ID_ to _/shared/services/.set-aside/ID-YYYY-MM-DD_;
a name already there gets *-2*, *-3*. What people allowed the old key's
app is forgotten. Only an administrator replaces an app installed for
everyone, with *--for-everyone*; a person's install never takes an ID
from one. From then on the ID installs only from that catalog's files.

A publisher whose bundle no catalog lists moves it to a new key with a
rollover file (*key rollover*). A bundle signed by the new key that
carries it, with links from the key the ID is bound to, installs: the
sheet says *A new key for* the ID, names the old key and the day the
publisher moved the bundle, and from then on only the new key may
install it. What was allowed the old key's app moves to the new key.

A file no catalog lists installs as before: signed by your developer key
with no warning, or by any other key after one, which also says that no
catalog this computer trusts lists it.

# SHEETS

*bundle sheet* shows what an install shows, before one: bundled fetches
the newest version of the bundle from the catalogs, checks the file as it
comes, and unpacks it only in a folder of your own, which it removes,
so nothing of the bundle is installed, run or left. The facts are those
of the sheet of an install for everyone, whoever asks, so that someone who
may not install it can read what it asks for.

bundled keeps the one file whose sheet was read last, in the folder of its
catalog's downloads (*bundled*(8)), until another sheet's takes its place
or an install takes it. *bundle install* with *--sha256* then uses that
file, with no second download. The hash names the file the sheet showed,
and no other: a catalog that lists another file by then, which is a newer
version, is *APP-102*, and nothing is fetched or installed. Read the sheet
again, then, which shows what the new file asks for.

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

Versions order as *manifest.json*(5) says: as semantic versioning
orders them, and then by build, the number a version may end in, as in
*15.2.0_2*; a version with no build number is build 1. A catalog gives
a version it signed again, with other files, such as after a fix to the
C library, its next build, so that computers update to it as to any
newer version. *bundle update --check* marks such a row *(rebuild)*, and
says why the catalog made it:

```
$ bundle update --check
ID                            INSTALLED  NEWEST    CATALOG
io.github.burntsushi.ripgrep  15.2.0     15.2.0_2  dukovo  (rebuild)
  rebuilt: built again with the C library's fix
1 to update, 0 up to date
```

A rebuild goes out in waves, can be withdrawn and rolled back, as any
version can.

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

# TONIGHT

*bundle update --tonight* keeps the request in *bundled*(8)'s state, and
*bundle list* says ", installs tonight" at the app's line. In the window,
*bundled* installs each app asked for, one at a time, as an update that
nobody is there for: it asks for more than the version it replaces only
when *--yes* was said, else it waits as an update by itself does, and
*bundle history* says so; a held app is left alone, and one that fails
says why. The history's row names who asked, and says "as asked, in the
window". A request made inside the window runs at once, and none starts in
the last 5 minutes of it.

A computer that was off through the window, or whose *bundled* was not
running, installs nothing when it is on in the day: the request waits
for the next window, and *bundle list --json* says when a window passed
without it.

# WAVES

A catalog's update server may send the newest version of a bundle out in
waves (ADR 0544), as Dukovo's own releases go out (*sysupdate*(8)): the
project's canary computers take it first, and the rest on days 1, 3, 5
and 7 after the server first saw that version, a few at first and then
everyone, each computer by the number it draws and keeps in
_/system/state/sysupdate/wave_. A computer is a canary by *canary* in
_updates.toml_ (*updates.toml*(5)). If something goes wrong, the server's
operator pauses the waves, and the computers not reached yet wait. A
catalog that signs no waves has every update taken at once.

The updates by themselves and *bundle update* with no _id_ wait for the
wave. Of an update that waits, bundle says which version it is, on which
day after its release this computer's wave opens, why the waves are
held, if they are, and what takes it now; *bundle update --check* says
it in place of a row of its table, and counts it apart:

```
$ bundle update --check
org.example.app 2.1.0 reaches machines in waves; this machine's opens on day 7
  the waves are held: paused: crashes on start
  `bundle update org.example.app` takes it now
0 to update, 0 up to date, 1 waiting for their wave
```

Three updates do not wait. *bundle update* _id_, which names the bundle,
takes it at once, as *sysupdate stage --version* does for the system.
An update that fixes a known flaw goes at once, whatever its wave: when
the catalog's list of flaws (FLAWS) names a high or critical flaw in the
version installed that it does not name in the newer version, which the
list looked at, a computer running the flawed version does not wait up
to a week for the fix. And a bundle installed anew, with *bundle install*
_id_, takes the newest version, since waves are for updates. A bundle
from its own feed (OWN FEEDS) has no waves. A rollout that cannot be
read holds the update back, and bundle says why; naming the bundle takes
it.

# WITHDRAWALS

A catalog, or the feed of a bundle (OWN FEEDS), may withdraw a version it
listed (ADR 0593): as *broken*, for a version that does not work as it
should, or as *malicious*, for one that does harm, with a reason and the
day. The target stays listed. Only the keys that sign what the catalog
lists, or the publisher's for a feed, can say it, never the online key
that signs the flaws and the waves. A withdrawal that cannot be read
withdraws the version as broken, and stops nothing that runs.

Each time *bundled*(8) reads a catalog or a feed, in a search, a sheet,
an install, an update or its daily check, it keeps what it did not know
of, for good: a later read that drops a withdrawal, or calls it broken
where it was malicious, changes nothing. A withdrawn version is not
offered by *search*, a sheet or an update, *bundle rollback* refuses to
go back to it, and *bundle install* refuses it, also from a file that
a catalog or the feed withdrew:

```
$ bundle install ./app-2.1.0.dkv
bundle: org.example.app 2.1.0 was withdrawn by catalog dukovo on 2026-11-20: it sends the keys away
```

An installed version that is withdrawn is left at once, in the daily
check, at *bundled*'s start and when it learns of the withdrawal:

- A *broken* version goes back to the version the last update kept
  beside it, with that version's data, if that one is not withdrawn
  too, and is not held there. With none kept, it keeps running, and
  the next newer version that is not withdrawn is taken at once, ahead
  of the waves: by itself where the bundles update by themselves, else
  when you run *bundle update*.
- A *malicious* version is stopped: its services are dropped, with their
  accounts and data kept, each run of it ends (RUNNING), and its folder
  is made mode 0500, so that no run of it can start; its commands stay,
  and say why. It then goes back as a broken one does. With no good
  version, it stays installed and stopped until you remove it, or
  *bundle update* _id_ takes a version that is not withdrawn.
- A held bundle goes back, or is stopped, all the same, and stays held:
  the next good version waits for you.

*bundle list* says "withdrawn by catalog c: why" beside a withdrawn
version that runs, and "stopped by catalog c: why; remove it, or *bundle
update* id takes a fixed version" beside one that is stopped; *bundle
info* says so too. *bundle history* shows "version was withdrawn: note",
and each withdrawal of an installed version is also an audit line, a
notice at your next sign-in and one from *notifyd*(8).

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

# JSON

*list --json*, *info --json*, *search --json* and *sheet --json* print one
line of JSON, an object whose
first member is *format*, 1 (ADR 0541), which scripts and Ansible's
*dukovo.system* collection read in place of the text. A later field may
be added without a new *format*; read the members you know. Names are in
snake_case, times are RFC 3339 in UTC, sizes are bytes, and a value that
is not known is *null*. An error is told to standard error as without
*--json*, with the same exit status, and prints no JSON, except that
*search --json*, when no catalog could be read, prints its document first.

*bundle list --json* has *bundles*, a list with an object for each
bundle you can run, by ID and then yours before everyone's:

*id*
	The bundle's ID, with its line when it has one (*org.nodejs.node@22*).

*name*
	Its name, as it says it.

*version*
	The version installed.

*for_everyone*
	*true* for one installed for everyone, *false* for yours.

*catalog*
	The catalog it is bound to, or *null* for one that updates from its
	own feed or from none (UPDATES).

*held*
	Whether it is held at its version.

*held_reason*, *held_by*, *held_since_ms*
	While it is held: why, the text *bundle hold --why* kept, or *null*
	where none was given; who, the user ID that held it, or *null*;
	and since when, in milliseconds since 1970. All *null* when it is
	not held.

*update*
	The newest version its catalog listed when bundled last read it, if
	that is newer than *version*, else *null*. The list reads no
	network; *bundle update --check* reads the catalogs anew.

*tonight*
	*null*, or, for an app of everyone's whose update is asked for
	tonight's window (TONIGHT), an object: *to*, the version it
	installs, *asked_ms*, when it was asked, and *by*, the user ID of who
	asked, *missed_ms*, when a window first passed without it, or *null*,
	and *starts_ms*, when the window it installs in starts, in
	milliseconds since 1970: now, in the window with time left.

*withdrawn*
	*null*, or, for a version a catalog or the bundle's own feed
	withdrew (ADR 0593), an object: *source*, *catalog:*_name_ or
	*feed:*_place_; *kind*, *broken* or *malicious*; *reason*, the text
	it gave; *since*, when, in seconds since 1970; and *stopped*, *true*
	once a malicious version is stopped: it stays installed but cannot
	run.

*flaws*
	What the catalog's list of known flaws says of this version
	(FLAWS), an object: *state*, *unknown* when it says nothing, *none*,
	or *known*; *count*, how many it knows; and *worst*, the worst
	severity, *low*, *medium*, *high*, *critical* or *unknown*, or *null*
	when it knows none.

*bundle info* _id_ *--json* is one bundle's facts, as the sheet shows them:

*id*, *name*, *version*, *publisher*, *summary*, *for_everyone*
	As in *list*, with the publisher's name and the bundle's summary.

*key*
	The key that signed it.

*trust*
	Why the key is trusted, an object: *kind*, one of *catalog*,
	*developer*, *bound* (its first install here), *bound_for_everyone*,
	*new_key*, *takes* and *unknown*; *catalog*, its name for *catalog*,
	for *takes* and for a *new_key* a catalog lists; *since*, from when,
	when that is known (for *new_key* the time of the move, for *takes*
	the time the key it replaces was bound); *old*, for *new_key* and
	*takes*, the key it replaces, as *RW*...; and *moved*, for *new_key*,
	the time its publisher signed that the ID moved to the key that signed
	this file.

*files*, *bytes*
	How many files it has, and the bytes they take.

*support*
	Where its publisher takes problem reports, or *null*.

*update_feed*
	Its own feed, for a bundle no catalog lists, or *null*.

*flaws*
	An object: *state*, *unknown* when the catalog's list says nothing
	of this version or there is none, *none*, or *listed*; *list_made*,
	when the list was made; and *known*, each flaw as an object with
	*id*, *severity*, *component*, *fixed* (the version of the
	component that fixes it, or *null*) and *summary*.

*permissions*
	Each permission it asks for, an object: *name*; *lets*, what it
	lets the bundle do, in the system's words; *tier*, when it is
	asked; *need*, *required* or *optional*; *folders*, each with
	*folder* and *access*; *ports*; *apps*; and *why*, in the bundle's
	own words, or *null*.

*commands*, *commands_with_your_access*
	The names of the commands that run in the sandbox, and of those
	that run with your access, outside it.

*services*
	What each of its services is, in a line, as the sheet says it.

*service_list*
	The same services as a list of objects: *name*; *account*, the account
	it runs as; and *start*, *on* when Spark starts it at each boot, *off*
	when it does not, which is also so for one installed for yourself or
	one an administrator has not enabled yet.

*license*
	Its license, as the SPDX expression its manifest gives.

*provides*, *uses*
	The sockets and interfaces its manifest offers and the ones it
	talks to, each an object of *interface*, as *NAME/MAJOR*, and
	*socket*, which is the socket's name for *provides* and the
	provider's ID and the name, *ID/NAME*, for *uses*; each is *null*
	where the entry names the other.

*used_by*
	The bundles installed in the same place whose *uses* name an
	interface or a socket this bundle *provides*, each an object of *id*,
	*name* and *what*, the interface or the socket, as *uses* gives it,
	that they use. A bundle that uses nothing of it is not listed.

*grants*
	What you said of its permissions, each an object: *permission*,
	*said* (*allow* or *never*), *since*, and *given*, what it gives, in
	words.

*installed*
	When it was installed.

*folder*, *data*
	Where it is, and where its data is, in words.

*data_size*
	What the data of its services takes, as *bundled* measured it, or
	*null* where nothing was measured: for a bundle installed for you
	alone, and until the first measure after it was installed. An object:
	*bytes*, the space the files take on the disk, counted by the blocks
	they use, a file with two names once, a link as itself; *files*, how
	many files; *complete*, *false* when the walk was ended by its limits
	and the count is no more than that (the sheet says "at least"); and
	*measured_ms*, when, in milliseconds since 1970. It is as old as the
	last measure, a day at most; *info* measures nothing.

*previous*
	The version it can go back to, or *null*.

*held*
	Whether it is held at its version.

*withdrawn*
	As in *list*.

*bundle search --json* has *bundles*, a list with an object for each
bundle the catalogs list, the newest version of each, catalog by catalog
in the order bundled reads them, and *unread*, a list with an object for
each catalog that could not be read, with *catalog* and *why*, in words. A
bundle is an object of:

*id*, *name*, *version*, *flaws*
	As in *list*: the ID, its name, the newest version and what the
	catalog's list of known flaws says of it.

*catalog*
	The catalog that lists it.

*installed*
	The version installed for you, or else for everyone, or *null*.

*bundle sheet* _id_ *--json* is the facts of *info*, as the sheet shows
them before an install: *id*, *name*, *version*, *publisher*, *summary*,
*key*, *trust*, *files*, *bytes*, *support*, *update_feed*, *flaws*,
*permissions*, *commands*, *commands_with_your_access*, *services*,
*service_list*, *license*, *provides* and *uses* as there, and, which *info* has not,
*catalog*, *sha256*, *download_bytes*, *services_need_everyone*, *data*,
*installed* and *refusal*. It has none of *used_by*, *grants*, *folder*,
*data_size*, *previous* and *held*, which only an installed bundle has.

*catalog*
	The catalog whose file it is.

*sha256*
	The file's SHA-256, 64 lowercase hexadecimal digits, which *install*
	takes with *--sha256*.

*download_bytes*
	The file's size, in bytes.

*services_need_everyone*
	Whether it has services, which run for no one in particular, so that
	only an administrator installs it, for everyone.

*data*
	The folder of each of its services' accounts, where its data will be,
	a list; for *info*, *data* is a sentence.

*installed*
	The version installed now, for you or else for everyone, or *null*;
	for *info*, when it was installed.

*refusal*
	Why bundled would not install it now, in its words, or *null*.

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

_/system/state/bundled/withdrawn_
	The versions a catalog or a feed withdrew, which only *bundled*(8)
	writes (WITHDRAWALS).

_/system/settings/catalogs/name_
	A catalog: _url_, _root.json_, and _off_ when it is turned off
	(*bundled*(8)).

# EXIT STATUS

0 when the manifest reads, and 1 when it does not, a file it names is
missing, a bundle does not check, or the command line is wrong. *install*
and *remove* exit with 1 when bundled refused, *search* when it finds no
bundle (with *--json*, only when no catalog could be read), *sheet* when no
catalog lists the bundle or none could be read, and *grant* and *revoke*
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

The sheet before the install, and the install of that very file:

```
$ bundle sheet io.github.burntsushi.ripgrep
...
$ bundle sheet io.github.burntsushi.ripgrep --json | jq -r .sha256
3f1c...
$ elevate bundle install io.github.burntsushi.ripgrep --for-everyone --sha256 3f1c...
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

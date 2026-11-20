bundled(8) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

bundled - install, seal, update and remove bundles, and keep what they may do

# SYNOPSIS

*bundled*

*bundled measure* _id_ _folder_ [_id_ _folder_]...

# DESCRIPTION

bundled installs bundles, seals them so that nothing changes them, and
removes them, for whoever asks with *bundle*(1) (ADR 0223), from files
or from the catalogs this computer trusts (ADR 0383), and updates them
from there, by themselves for those who turn that on (ADR 0384). It links the
commands of the bundles it installed where the shell finds them, and
tells *bundle run* which bundle to run and what it recorded of it (ADR
0225). It keeps what each person said of each bundle's permissions, and
asks them through the console's trusted prompt (ADR 0224). Spark runs it as the *bundled* service, after authd, whose
accounts say where each home is, and before the consoles. It acts for
root, past the owners and modes of files, since it seals each bundle as
root's in each person's home.

It answers on _/system/live/bundled/socket_, which anyone may connect to,
one request per connection, and knows who asks by the user the kernel
gives for the connection, never by what the request says. Each request
is a line of tab-separated words; *bundle*(1) is the only program meant
to speak it.

# PLACES

A bundle is installed for everyone, in _/applications/ID_, or for one
person, in _ID_ in the _applications_ folder of their home. Installing or
removing for everyone is an administrator's, through *elevate*(8); a
person installs and removes their own. bundled installs only in a place's
folder that is its owner's (root's for _/applications_), opens every name
there one part at a time and never through a link, so nothing a person
puts in their folder leads it elsewhere.

# INSTALLING

The archive comes with the request, as a descriptor. bundled makes a
folder of its own, _.installing-RANDOM_, mode 0700, in the place's
folder, and unpacks the archive into it with the confined unpacker of
*bundle*(1), which holds only the archive and that folder. Then it checks
the folder again itself: the signature, the list, every file's SHA-256,
and that it holds only folders and regular files.

It decides whether to trust the bundle (ADR 0383). An ID bound to a
catalog installs only from a file that catalog lists, whatever key
signed it. Otherwise an ID bound to a key installs only as that key
signs, and any other key for that ID is refused. A file a catalog lists
is trusted as that catalog's (CATALOGS); the asker's developer key is
trusted for their own installs; any other key installs only after a
warning that this computer does not know it and that no catalog it
trusts lists the file, and from then on the ID is bound to it. A command named as one of _/system/commands_, or
as one another bundle in reach adds, is refused.

It answers with the install sheet: the name, ID, version and publisher,
the key and why it is trusted or not, the size, the permissions asked
for in the system's own words and how each is given, and the commands,
saying plainly which run with your access, outside the sandbox. bundle
asks, unless *--yes* answered. Then bundled seals the folder (each file
root's, mode 0444, or 0555 for a program; each folder root's, 0555),
renames it to its ID in place of the same ID from the same key, records
it, binds the ID to its key, and to its catalog if it came from one, at
its first install there, links its commands, and writes a security event to the audit log. Anything that
fails removes the folder it made, and nothing is recorded.

# COMMANDS

Each place has a _commands_ folder of bundled's, root's, mode 0755:
_/applications/commands_ and each person's _applications/commands_. It
holds a symbolic link by each command of the place's bundles, to
_/system/commands/bundle_, which runs the command of the name it was
called by (*bundle*(1), RUNNING). A session's *PATH* names both folders
after _/system/commands_, so installing never edits *PATH* or a shell's
profile. bundled makes each folder again from what it recorded after
each install and removal, and at its start; a _commands_ there that is
not a folder of root's is not bundled's, and it links nothing in it.

# RUNNING

Asked which bundle to run, by its ID or by a command it adds, bundled
answers with the asker's own bundle before everyone's: its place, ID and
key, and the SHA-256 of the signed list it recorded at install. *bundle
run* checks the folder against them before it runs anything.

A bundle with a line is kept by its full ID, _ID@LINE_ (ADR 0387), and
the key and catalog its ID is bound to bind all its lines. Lines of one
ID may add the same commands, which no other two bundles may; each
line's commands are linked a second time as _NAME-LINE_. When the ID or
command asked for is several lines', bundled answers with the lines, the
files a project chooses them with, and the asker's default line and
everyone's, and *bundle*(1) chooses the line in the asker's working
folder, which bundled cannot read, and asks again for it.

A line's add-ons (ADR 0388) are in _~/applications/add-ons/ID@LINE_,
each person's own, and an administrator's in
_/applications/add-ons/ID@LINE_. Asked by *bundle*(1) after a run that
changed them, and whenever it links a place's commands, bundled links
each command in the folder the manifest's *add_ons* names beside the
commands of the place, except one named as a system command, reading
only names, through folders each the place owner's and never through a
link. Asked for a command no bundle adds, it answers with the add-on of
that name of the asker's lines, their own before everyone's, and the
add-ons' path; a link to one that went goes then. Removing a line
with *--purge* removes its add-ons with its data.

# GRANTS

bundled keeps what each person said of each bundle's permissions, the
grants: for each, the bundle's ID and its publisher's key, the
permission, allow or never, since when, what it gives (each folder, read
or read-write, or app) and what the bundle asked for then. *bundle run*
gets those of the asker's, for the bundle's key only, and decides what
the program gets (*bundle*(1), PERMISSIONS).

Asked *ask* about a permission the asker has not answered for good,
bundled checks that the bundle is the one installed, reads the
permission from its sealed manifest itself, and words the question in
the system's own words. It finds the terminal of the process that asks,
its session's, from _/processes_, and asks *authd*(8) to have
*consoled*(8) ask it on that console's bottom line, where no program
draws, reads or answers. *bundle grant* asks the same way, *Y* or *N*.
An answer for good is recorded, and every answer is a security event of
the audit log. A terminal that is not a console's is answered
*elsewhere*, and nothing is given. Taking a permission away needs no
question: bundled records it, and tells each run of the bundle that
watches, on the *watch* connection its runner keeps, at once.

# CATALOGS

bundled reads the catalogs set in _/system/settings/catalogs_ with
Dukovo's TUF client (ADR 0380), as root; no other program does. Each is
a folder by its name, which holds _url_, where its repository is, and
_root.json_, the root to start from; a file _off_ there turns it off.
Spark copies the catalogs the system comes with into it from
_/system/resources/default-settings/catalogs_, so bundled turns one of
those off rather than removing it.

For each catalog, bundled keeps the metadata it trusts, the root it
started from, and an index of the bundles the catalog's targets list:
each one's path, SHA-256, length, ID, version and name, with when it was
read and when the soonest of the metadata it came from expires. It reads
a catalog anew, from what it trusts, for *bundle search*, *bundle sheet*
and *bundle install* by ID, and writes the index only once every check has
passed.
It starts again from _root.json_ when that file changes.

Asked to install an ID, it reads the catalogs in order, those the system
comes with first, and fetches the newest version from the first that
lists it, or from the one asked for, through the client, which checks
its length and SHA-256 as it comes. The file is its own until the
install ends, and then removed. Asked to install a file, it sums the
archive and looks it up in each index that has not expired: a catalog
lists it if it has that ID with that SHA-256 and length.

An install by ID may name the file it is to install by its SHA-256,
which the client sends as the last word of its request, as 64
hexadecimal digits: if the catalog lists another file of that ID by then, a
newer version, bundled says so before it fetches anything, in words that
end in *APP-102*, and installs nothing. When no catalog lists the ID it
says so in words that end in *APP-101*, and when no catalog could be
read, in words that end in *APP-103*. An older *bundle*, which sends no
hash, installs the newest file, as it always did.

Asked for the sheet of an ID, bundled fetches the newest version as an
install does, checks the file as an install does, and answers with the
sheet an install for everyone shows, and then the same facts as one
document of JSON, with the catalog, the file's SHA-256 and size, the
version installed now, if there is one, and why the install would be
refused, if it would; each asker reads the one it asked for. It unpacks
the file in a folder of the asker's own, which it removes, installs
nothing and records nothing. Of the files it fetched for sheets it keeps
only the one read last, in its catalog's _downloads_, named by its
SHA-256, so that the install that follows finds it there and fetches
nothing again: another sheet's file takes its place, an install takes it
and removes it, and none outlasts bundled's start. Any person may ask
for a sheet or a search, as no one's bundles change; the file is checked
against its length and SHA-256 as it comes, whoever asks.

Asked to search with its JSON, bundled answers with one document: the
newest version of each bundle that the catalogs it could read list,
whose ID or name holds every word, each with its catalog, what the
catalog's list of known flaws says of it and the version the asker has
installed, or else everyone has; and each catalog it could not read,
with why. That no catalog could be read is an error after the document,
in words that end in *APP-103*.

Each time it reads a catalog, bundled also fetches the catalog's list
of known flaws (ADR 0389), which its targets delegate to the role
*flaws*, and keeps it beside the index while it was made less than
eight days ago; one it cannot fetch is not kept. *bundle search*,
*bundle list*, *bundle info* and the sheet read it.

Adding and removing a catalog are root's, and each is a security event
of the audit log. A catalog is kept only once bundled has read it from
the root it came with.

# UPDATES

Asked to update, bundled reads anew the catalogs the place's bundles are
bound to and, for each bundle a catalog lists a newer version of, and
that is not held, fetches the newest and installs it as above, with one
difference: it shows the sheet and asks only when the new version asks
for more than the one it replaces, a permission or more of one, or a
command that runs with the person's access. Updating everyone's bundles
is an administrator's.

A catalog may also send the newest version of a bundle out in waves
(ADR 0544). Its snapshot then lists the delegated role *rollout*, the
online key's, whose targets _rollout/ID/VERSION.json_ say which wave is
open for that version: bundled reads it with the catalog's client, and
refuses one that does not name that ID and version, or a wave above 4.
An update of a bundle the request did not name waits until a wave that
reaches this computer is open, as *sysupdate*(8) does for the system:
by the number in _/system/state/sysupdate/wave_, which bundled draws
and keeps as sysupdate does, or *canary* in _updates.toml_
(*updates.toml*(5)). A bundle named in the request is taken at once, and
so is an update that fixes a known flaw: when the catalog's kept list
of flaws (below) names a high or critical flaw in the version installed
that it does not name in the newer one, which the list looked at, no
wave holds it back. A catalog that signs no rollout of that version has
it taken at once; one whose rollout cannot be read holds the update, with
why, unless it is a fix. What waits is not fetched, written to
_history_ or counted as failed: *bundle update --check* and the updater's
log, each day, say which version waits and why.

A bundle bound to no catalog updates from its own feed, when the
manifest of its installed version names one in *update* (ADR 0390): a
small TUF repository, which bundled reads with the client it reads the
catalogs with, one for each place and bundle. The client starts from
the feed's _metadata/1.root.json_, which must name *update.key* alone
for root and be signed by it, and starts over when a manifest names
another address, or a key the root it trusts does not name. bundled
fetches the newest version the feed lists of the bundle's ID and line,
by *https* or *http*, and installs it as any file is: its key must be
the one its ID is bound to. Its history says *from its feed*.

An update keeps the version it replaces as _.previous-ID_ beside it,
sealed, in place of the one kept before, and moves its record to
_previous_. The switch is renames in the place's folder, made while
bundled holds the registry's lock. Asked to go back, bundled checks
that _.previous-ID_ is its own, sealed, and the very version recorded,
by its key, list and version, then makes the two change places and
holds the bundle there.

A held bundle, in _held_, is left as it is by every update. Each place
may update by itself (_automatic_): bundled's updater does so ten
minutes after it starts, then daily, and at once when it is turned on.
An update by itself that asks for more than the version it replaces is
not installed; it waits for a person, and bundled writes so once for
that version in the history and each time in its log.

The daily check runs with the updater, whether or not any place
updates by itself: it reads anew each catalog that installed bundles
are bound to, and writes the known flaws of each installed version to
its log and, once for each change, to _history_ as *flawed*. It changes
no bundle. The installed versions with known flaws are kept for the next
sign-in in _/system/live/sign-in/bundled_, which the sign-in screens show
under the issue text (*consoled*(8), ADR 0504), and when that list
changes, the consoles are told at once (*notifyd*(8)). A catalog that
could not be read leaves the list as it was.

Once a day, after the check, bundled measures what the data of the apps
installed for everyone takes, so that *bundle info* can say it. It
starts *bundled measure* for it, a process of its own that takes each
folder after its app's ID (bundled gives them; it is not for you to
run). The process runs at the lowest priority for the disk and the
processor, and walks nothing if it cannot be given that. It walks
_/shared/services/ID/ACCOUNT_ of each such app and each account its
services run as: it counts the blocks of each file (a file with two
names once), counts a symbolic link and does not follow it, never enters
another file system, and reads no content and keeps no name. The walk of
one app ends after 10 minutes or 5,000,000 entries, the whole after an
hour, and is then marked incomplete; bundled ends the process if it is
still there after 70 minutes. A person's own apps are not measured: their
data is in their home. Nothing measures when asked. The result is
_sizes.json_, which a later day replaces whole.

An app installed for everyone can also be asked to update tonight
(*bundle update --tonight*, *bundle*(1)). The request is kept in
_tonight.json_ and an update installs in the window: the hour that ends
at the time *restart* names in _updates.toml_ when *unattended* is on
(*updates.toml*(5)), else 02:00 to 03:00 by the local clock; the window
may cross midnight. The updater wakes when the window starts, and at
least hourly while requests wait. In the window it installs each app
asked for, one at a time, as an update by itself that nobody is there
for: it asks for more than the version it replaces only when the person
asked with *--yes*, else the update waits as an update by itself does,
and says so once for that version. A held app is left alone and the
history says "held". A request made inside the window runs at once,
and none starts in the last 5 minutes of it, so that the restart at its
end does not cut one off. When the computer was off or bundled was not
running through the window, nothing installs in the day after; the
request stays for the next window and *bundle list --json* says when a
window passed without it. Each request is taken away when its update is
installed, has failed, is held, or waits for a yes that was not given.

bundled writes each install, update, going back, removal, hold and
release, waiting update and failed one to _history_. A person is shown
their own bundles' events and everyone's; root is shown all.

# SERVICES

A bundle whose manifest declares services installs only for everyone
(ADR 0385), as they run for no one. For each account its services name,
or each service's name, bundled asks *authd*(8) for an account of its
own: a number from 100, a group of its own, no password and no shell,
whose home is its data folder, _/shared/services/ID/ACCOUNT_, which
bundled makes, the account's alone. An account of that name with that
home is taken up again; another of that name stops the install.

Once the bundle is in place, bundled writes each service's file,
_/system/settings/spark/services/NAME.toml_ (*service.toml*(5)), checked
as Spark reads it: the program in the bundle's folder, run as its
account, its arguments, environment, restart, limits and secrets, order
70, and what it may do: read the bundle's folder, change its data
folder, and of what the bundle asks for and the program keeps, connect
out, listen on its ports and reach the folders of _/shared_ it names. A
person's permissions never reach a service. It tells *spark*(8) to
start each service that starts at boot; one that starts when enabled
waits for *service* _NAME_ *enable*. A service name that a file bundled
did not write has already, the system's or an administrator's, stops
the install.

Each service's environment holds *HOME*, *USER* and *LOGNAME*, its
account's, and *RUNTIME_DIRECTORY*, _/system/live/services/ACCOUNT_,
which Spark makes as it starts, the account's: the sockets it provides
go there.

# UPDATING SERVICES

An update or going back stops the services the version before ran, with
*stop NAME --keep-sockets*, so that Spark keeps their sockets open and
connections wait for the new version and none is refused (ADR 0477), and
waits until Spark says they have stopped (ADR 0386). A service that waits
for a connection, with its sockets held and no process, has stopped. The
wait is as long as the longest *stop.timeout* among the services it
stops (*service.toml*(5)), 5 seconds for one that says none, and 10 seconds
more, for Spark to end what is left of them and to say so (ADRs 0386
and 0477). It copies each
account's data folder to _/shared/services/ID/.snapshots/ACCOUNT/VERSION_
for the version that ran, keeping each file's owner, as *bundle run*
does for an app's data; going back to a version whose copy is there,
the two change places, so that what the newer one left is kept in turn,
and the data comes back at once. A copy goes once it is 30 days old, at
the next update.

It writes the service files again, keeping what an administrator
enabled or disabled; one an administrator changed in any other way
stays as it is, and the new one goes beside it as _NAME.toml.default_,
which *service check* compares with it. An administrator's changes
belong in an override file, _settings.d/NAME.toml_, which updates never
touch (*service.toml*(5)). It takes away the services the version
before ran that this one does not, and starts each enabled one; a new
service counts as asking for more.

After an update, each started service whose manifest names a *health*
socket must answer on it, in its _RUNTIME_DIRECTORY_, within 60 seconds.
One whose manifest has a *health* command and no socket must be running,
as *spark*(8) reports it, with its last health check passed, within the
same 60 seconds (ADR 0522); one that has stopped for good fails at once.
If one does not, bundled goes back by itself, as *bundle rollback*
does: the version before, its services' data as that version left it,
and a hold, so that no update tries the failed version again unasked.
The update fails with why, and the history keeps it.

# SOCKETS

A service of a bundle names its *sockets* in its manifest
(*manifest.json*(5)), and bundled writes them into its service file with
*start*, so that Spark holds them as it does a service file's, and a
service that starts *"on-connection"* waits for its first connection
(ADR 0477). It writes the service's *stop* and *backup* in the same way
as *[stop]* and *[backup]*.

A bundle offers a socket in *provides*. As it installs the provider,
bundled has *authd*(8) make the socket's group, _ACCOUNT-SOCKET_, for
*"publisher"* and *"approved"*, and removes it with the bundle. A bundle
that names the socket in *uses*, as _PROVIDER_ID/NAME_, is granted it as
it is installed when the provider's *to* allows: *"anyone"*; *"publisher"*
when both are signed by the same key; *"approved"* when the person who
installs it approves it, which the install's sheet asks ("uses the socket
_NAME_ of _PROVIDER_, which it offers to the programs you approve") and
the install's yes, or *--yes*, answers; *"system"* never. The provider
must be installed for everyone and offer the socket. A required socket
that is not given stops the install, with why and what to do; an
optional one is left out, and the install says so. A bundle that adds
such a socket to an update asks for more, as a new permission does. A
given socket's place is added to the *files.read-write* of every service
of the bundle, and the socket's group, when it has one, to its *groups*,
in the service file bundled writes. Without the grant a service has
neither. The provider's service file says who connects: *to = "anyone"*
or *to = "system"*, or *group = "ACCOUNT-socket"*, which *authd*(8)
makes (*SocketGroup*) and records as the bundle's, and removes with the
account.

# REMOVING

bundled removes the sealed folder, the version kept to go back to, and
the record, and with *--purge* the asker's _app-data/ID_, its copies in
_app-data/.snapshots/ID_ and what they said of its permissions too, and
unlinks its commands. The ID stays bound
to its key, so a later version from the same publisher picks kept data
up and one from anyone else is refused. At its start it forgets a
recorded bundle whose folder is gone, and removes the working folders
it left behind.

Removing a bundle installed for everyone stops its services, with a plain
*stop*, which closes their sockets, and removes
their files. authd removes their accounts, whose numbers are retired,
and bundled removes their folders in _/system/live/services_. Their data,
_/shared/services/ID_, stays, owned by the retired numbers, for a later
install to take up: authd gives each account its old number again when
its data folder is still owned by that number. With *--purge* bundled
removes the data too.

# FILES

_/system/state/bundled/installed_
	Each bundle installed: its place, ID, version, key, the SHA-256 of
	its signed list, and when.

_/system/state/bundled/bound_
	Each ID's key, for everyone and for each person, since its first
	install there.

_/system/state/bundled/catalog-bound_
	Each ID installed from a catalog, and which, for everyone and for
	each person, since its first install from it.

_/system/state/bundled/previous_
	Each version an update replaced and kept to go back to, recorded as
	in _installed_.

_/system/state/bundled/held_
	Each bundle held at its version, and since when.

_/system/state/bundled/automatic_
	Each place whose bundles update by themselves, and since when.

_/system/state/bundled/defaults_
	The default line of each ID with lines, for everyone and for each
	person (*bundle default*).

_/system/state/bundled/sizes.json_
	What the data of each app installed for everyone takes, per app and
	per folder, and when it was measured; mode 0600. Apps no longer
	installed are dropped when it is written.

_/system/state/bundled/tonight.json_
	The apps asked to update tonight: for each, the version asked
	for, whether *--yes* was said, who asked and when, and when a
	window first passed without it; mode 0600.

_/system/state/bundled/history_
	What bundled did to bundles, a line of tab-separated fields each,
	mode 0600; it keeps about the newest thousand.

_/system/state/bundled/catalogs/NAME_
	Each catalog's client: _tuf_, the metadata it trusts,
	_first-root.json_, the root it started from, _index_, _flaws.json_,
	its list of known flaws, and _downloads_, the files being
	installed, and the one whose sheet was read last.

_/system/settings/catalogs/NAME_
	Each catalog: _url_, _root.json_ and _off_.

_/system/state/bundled/feeds/PLACE/ID_
	The client of each bundle's own feed, for everyone or a user
	number: _tuf_, the metadata it trusts, _key_ and _url_, what it
	started from, _index_, and _downloads_, the files being installed.

_/system/state/bundled/developer-keys_
	Each person's developer key, which *bundle new* and *bundle key*
	tell bundled.

_/system/state/grants/UID_
	Each person's grants, by their user number, mode 0600. One bundled
	cannot read is set aside as _UID.bad_, and its person is asked
	again.

_/system/state/bundled/services/NAME_
	Each service file bundled wrote: the bundle's ID and the account.

_/system/settings/spark/services/NAME.toml_
	The service files it writes for bundles' services.

_/shared/services/ID/ACCOUNT_
	The data folder of each account of a bundle's services, its home.

_/shared/services/ID/.snapshots/ACCOUNT/VERSION_
	The data of an account as each version left it.

_/system/live/services/ACCOUNT_
	The sockets of an account's services.

_/system/live/bundled/socket_
	Where it answers.

_/applications_, _~/applications_
	The bundles for everyone and each person's own, the versions kept
	to go back to, _.previous-ID_, and their _commands_ folders.

# SEE ALSO

*bundle*(1), *manifest.json*(5), *authd*(8), *consoled*(8), *service*(8), *elevate*(8), *log*(1)

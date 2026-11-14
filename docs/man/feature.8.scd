feature(8) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

feature - add and remove what this machine does

# SYNOPSIS

*feature* [*list*] [*--json*]

*feature info* _id_ [*--json*]

*feature add* [*--yes*] _id_...

*feature remove* [*--keep-data* | *--purge*] _id_...

# DESCRIPTION

A feature is a named group of what a machine does, such as File
sharing: the services it turns on and the bundles it installs, as its
file in _/system/resources/features_ says (*feature.toml*(5), ADR 0316).
The files are part of the signed system, so what a feature does
changes only with an update. The installer offers the features of the
profile chosen; *feature* adds and removes them at any time after.

*list*, or no command, lists every feature: its ID, its name, and
whether it is *added* (each service it names on and each bundle it
names installed for everyone), *not added*, or *partly* added, as when
*share off* turned smbd off.

*info* says what a feature does, and whether each of its services is
on and each of its bundles installed.

*list* and *info* take *--json*, which prints the same as one line of
JSON (JSON).

*add* installs, for everyone, each bundle a feature lists and is not
installed, from the catalogs the system trusts (*bundle*(1)), showing
each one's install sheet unless *--yes* is given; then it turns each
service on, now and at each boot (*service*(8)). A service's own file
names the ports it listens on, and the firewall lets other machines
reach them while it runs (*firewall*(8)), so adding a feature changes
no firewall rule.

*remove* turns a feature's services off and removes its bundles, but
those another feature still needs, which it names: one added, or partly
added by a part of its own. A bundle's settings
and data stay unless *--purge* is given, so adding the feature again
picks them up.

*add* and *remove* need root: run them with *elevate*(8).

# EXAMPLES

```
$ feature
ID            STATE      NAME
file-sharing  not added  File sharing
$ elevate feature add file-sharing
File sharing is added: smbd and nfsd run, and start at each boot.
$ feature info file-sharing
File sharing (file-sharing): added
Shares folders with Windows PCs, Macs and Linux machines over SMB, and with Unix machines over NFS. `share add` picks the folders.
  service smbd: on
  service nfsd: on
```

# JSON

*list --json* and *info --json* print one line of JSON, an object whose
first member is *format*, 1 (ADR 0541), which scripts and Ansible's
*dukovo.system* collection read in place of the text. A later field may
be added without a new *format*. Names are in snake_case. An error is
told to standard error as without *--json*, with the same exit status,
and prints no JSON.

*feature list --json* has *features*, a list with an object for each
feature, by ID; *feature info --json* has the members of that object
at the top. The members are:

*id*
	The feature's ID.

*name*
	Its name.

*about*
	What it does, as its file says.

*state*
	*added*, *partly* or *not added*, as *list* shows it.

*services*
	The services it turns on, each an object with *name* and *on*,
	whether the service starts at boot.

*bundles*
	The bundles it installs, each an object with *id* and *installed*,
	whether it is installed for everyone.

# EXIT STATUS

0 when everything asked was done; 1 when something failed or the
command line was wrong, said on standard error.

# FILES

_/system/resources/features/ID.toml_
	The features (*feature.toml*(5)).

# SEE ALSO

*feature.toml*(5), *bundle*(1), *service*(8), *share*(1),
*firewall*(8), *dukovo-install*(8)

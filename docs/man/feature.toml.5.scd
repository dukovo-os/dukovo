feature.toml(5) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

feature.toml - a feature: the services and bundles it adds

# SYNOPSIS

_/system/resources/features/ID.toml_

# DESCRIPTION

Each file describes one feature that *feature*(8) adds and removes and
that the installer offers (ADR 0316). Its ID is its name without
_.toml_: 1 to 40 lowercase letters, digits and _-_, starting with a
letter. The files are part of the signed system.

```
feature_version = 1
name = "File sharing"
about = "Shares folders with Windows PCs, Macs and Linux machines."
services = ["smbd", "nfsd"]
bundles = []
```

# SETTINGS

*feature_version*
	The version of the file: 1.

*name*
	The feature's name, as lists and the installer show it: 1 to 40
	characters on one line.

*about*
	What it is for, in a sentence or two: up to 400 characters.

*services*
	The services it turns on, by their names in *service.toml*(5): up
	to 32, none twice. A service's own file names the ports it listens
	on, so a feature names none.

*bundles*
	The bundles it installs for everyone, by their IDs, with a line as
	_ID@LINE_ where it has one (*manifest.json*(5)): up to 32, none
	twice.

A feature lists at least one service or bundle. Any other setting is an
error, said with its line.

# SEE ALSO

*feature*(8), *service.toml*(5), *manifest.json*(5), *bundle*(1)

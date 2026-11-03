#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# Signs the list of the bundles' known flaws each day (ADR 0389), as the
# user dukovo-catalog, once flaws-fetch.sh has fetched OSV's advisories:
# catalog-repo matches them with the components each bundle's SBOM
# lists, signs the list with the online key, which the targets delegate
# it to, and publishes it with a new snapshot and timestamp (site.sh).
# Advisories more than two days old are not used: the list then expires
# a week after it was signed, and clients say they cannot tell a
# bundle's flaws instead of trusting an old list.
set -eu

# shellcheck source=tools/update-server/site.sh
. /usr/local/lib/dukovo-catalog/site.sh

advisories=/var/lib/dukovo-flaws

if [ ! -e "$home/repo/metadata/root.json" ]; then
	echo "No catalog has been deployed yet."
	exit 0
fi
set --
for file in "$advisories"/*.zip; do
	[ -f "$file" ] || continue
	if [ -n "$(find "$file" -mmin +2880)" ]; then
		echo "flaws.sh: $file is more than two days old: is dukovo-flaws-fetch failing?" >&2
		exit 1
	fi
	set -- "$@" "$file"
done
if [ "$#" = 0 ]; then
	echo "flaws.sh: there are no advisories in $advisories yet" >&2
	exit 1
fi
sign_site flaws --store "$home/store" "$@"

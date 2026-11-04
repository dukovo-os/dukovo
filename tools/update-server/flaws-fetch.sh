#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# Fetches OSV's advisories each day (ADR 0389), as the user dukovo-flaws,
# for flaws.sh to sign the list of the bundles' known flaws from: for
# each ecosystem ECOSYSTEMS names, OSV's export of all its advisories. Of
# the update server's jobs, it alone reaches the web, and it cannot read
# the catalog's key; what it fetches is only ever read.
set -eu

dir=/var/lib/dukovo-flaws
ecosystems=${ECOSYSTEMS:-crates.io}

for ecosystem in $ecosystems; do
	case $ecosystem in
	*[!A-Za-z0-9.]* | .*)
		echo "flaws-fetch.sh: $ecosystem is not an ecosystem's name" >&2
		exit 1
		;;
	esac
	curl --fail --silent --show-error --proto '=https' --max-filesize 1G \
		--retry 3 --output "$dir/$ecosystem.zip.new" \
		"https://osv-vulnerabilities.storage.googleapis.com/$ecosystem/all.zip"
	mv "$dir/$ecosystem.zip.new" "$dir/$ecosystem.zip"
done

# An ecosystem no longer named is no longer read.
for file in "$dir"/*.zip; do
	[ -f "$file" ] || continue
	name=${file##*/}
	case " $ecosystems " in
	*" ${name%.zip} "*) ;;
	*) rm -f "$file" ;;
	esac
done

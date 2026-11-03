#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# Publishes what a deploy copied into the incoming folder (ADR 0382), as
# the user dukovo-catalog, when dukovo-catalog-publish.path sees `ready`:
# the catalog's public keys and metadata, from the source tree, and the
# bundles they sign, which are added to the store and never replaced.
# catalog-repo then signs a snapshot and timestamp with the online key,
# lays out the catalog and reads it back with the client before it is
# served (site.sh). What happened is written to `result`, for the deploy
# to show.
set -eu

# shellcheck source=tools/update-server/site.sh
. /usr/local/lib/dukovo-catalog/site.sh

incoming=/var/lib/dukovo-catalog-incoming

rm -f "$incoming/ready" "$incoming/result"

# Each step's failure is checked here: `set -e` does not hold in a
# function whose status is tested.
publish() {
	new=$home/repo.new
	rm -rf "$new"
	mkdir -p "$new/keys" "$new/metadata" "$home/store/bundles" || return 1
	# Only the files the repository holds, by name.
	for file in "$incoming"/catalog/keys/*.json; do
		if [ -f "$file" ]; then cp "$file" "$new/keys/" || return 1; fi
	done
	for file in "$incoming"/catalog/metadata/*.json; do
		if [ -f "$file" ]; then cp "$file" "$new/metadata/" || return 1; fi
	done
	# The online key's public half is the server's own, made here.
	cp "$home/online.json" "$new/keys/online.json" || return 1
	# The first root never changes once deployed: every later root, and
	# so every targets, must follow from it, as on a client, so that
	# nothing else is published even by a login that may deploy.
	if [ -e "$home/repo/metadata/1.root.json" ] &&
		! cmp -s "$new/metadata/1.root.json" "$home/repo/metadata/1.root.json"; then
		echo "1.root.json is not the first root this server published; nothing was published."
		return 1
	fi
	for file in "$incoming"/store/bundles/*.dkv; do
		[ -f "$file" ] || continue
		name=${file##*/}
		if [ ! -e "$home/store/bundles/$name" ]; then
			cp "$file" "$home/store/bundles/$name.partial" &&
				mv "$home/store/bundles/$name.partial" "$home/store/bundles/$name" || return 1
		fi
	done
	rm -rf "$home/repo.old"
	if [ -e "$home/repo" ]; then mv "$home/repo" "$home/repo.old" || return 1; fi
	mv "$new" "$home/repo" || return 1
	if ! sign_site publish --store "$home/store"; then
		# Keep the repository that what is served came from.
		rm -rf "$home/repo"
		if [ -e "$home/repo.old" ]; then mv "$home/repo.old" "$home/repo"; fi
		return 1
	fi
	rm -rf "$home/repo.old"
}

if output=$(publish 2>&1); then
	status=published
else
	status=failed
fi
printf '%s\n%s\n' "$status" "$output" >"$incoming/result.partial"
mv "$incoming/result.partial" "$incoming/result"
echo "$output"
[ "$status" = published ]

# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# What publish.sh and refresh.sh share (ADR 0382), which they read with
# `.`: one of them runs at a time, and what they sign is written beside
# the catalog being served, into a copy of it made of hard links, which
# is served only once catalog-repo has read it back as a client would.
# /srv/dukovo/catalog is a link to the folder served, and pointing it at
# the next is one rename, so a client sees the old catalog or the new,
# never part of each, and a catalog that fails its check is never seen.

home=/var/lib/dukovo-catalog
program=/usr/local/lib/dukovo-catalog/catalog-repo
site=/srv/dukovo

exec 9>"$home/lock"
flock 9

# Runs catalog-repo with "$@" and `--out` a copy of the folder served,
# then serves the copy if catalog-repo succeeded.
sign_site() {
	current=$(readlink "$site/catalog" 2>/dev/null || true)
	# A copy left by a run that was stopped part way is not served.
	for dir in "$site"/catalog.*; do
		if [ -d "$dir" ] && [ ! -L "$dir" ] && [ "${dir##*/}" != "$current" ]; then
			rm -rf "$dir"
		fi
	done
	next=$(mktemp -d "$site/catalog.XXXXXXXX") || return 1
	chmod 0755 "$next" || return 1
	if [ -n "$current" ]; then
		# catalog-repo writes each file anew and renames it into place, so
		# the files the two folders share are never changed.
		cp -al "$site/$current/." "$next/" || return 1
	fi
	if ! "$program" --repo "$home/repo" --key "$home/online.key" "$@" --out "$next"; then
		rm -rf "$next"
		return 1
	fi
	ln -sfn "${next##*/}" "$site/catalog.new" || return 1
	mv -T "$site/catalog.new" "$site/catalog" || return 1
	if [ -n "$current" ]; then
		rm -rf "${site:?}/$current"
	fi
}

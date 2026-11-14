# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# What publish.sh and refresh.sh share (ADR 0382), which they read with
# `.`: one of them runs at a time, and what they sign is written beside
# the catalog being served, into a copy of it made of hard links, which
# is served only once catalog-repo has read it back as a client would.
# /srv/dukovo/catalog is a link to the folder served, and pointing it at
# the next is one rename, so a client sees the old catalog or the new,
# never part of each, and a catalog that fails its check is never seen.
# On a Dukovo server nginx serves it from dukovo-catalog-site.

# Where things are: an Ubuntu server's places unless the environment names
# others, as the service files of a Dukovo server do (dukovo/README.md).
home=${DUKOVO_CATALOG_HOME:-/var/lib/dukovo-catalog}
program=${DUKOVO_CATALOG_PROGRAM:-/usr/local/lib/dukovo-catalog/catalog-repo}
site=${DUKOVO_CATALOG_SITE:-/srv/dukovo}

work=
lock_held=
# Ends a run: takes away the online key's copy and lets the next run start.
finish() {
	if [ -n "$work" ]; then rm -rf "$work"; fi
	if [ -n "$lock_held" ]; then rm -rf "$lock_held"; fi
}
trap finish EXIT

# One run at a time. flock(1) does it where there is one. Dukovo has
# none: there the lock is a folder, which mkdir makes for one run only,
# in a place that is empty again after a restart, and a lock held more
# than 30 minutes belongs to a run that was killed and is taken over.
if command -v flock >/dev/null 2>&1; then
	exec 9>"$home/lock"
	flock 9
else
	lock=${DUKOVO_LOCK:?DUKOVO_LOCK names the lock folder where there is no flock}
	# The folder is Spark's, made for a job and gone after a restart; a
	# rollout run by hand finds it missing before the first job.
	mkdir -p "${lock%/*}"
	waited=0
	until mkdir "$lock" 2>/dev/null; do
		if [ -n "$(find "$lock" -maxdepth 0 -mmin +30 2>/dev/null)" ]; then
			echo "site.sh: taking over $lock, held for more than 30 minutes" >&2
			rm -rf "$lock.stale"
			mv "$lock" "$lock.stale" 2>/dev/null && rm -rf "$lock.stale"
			continue
		fi
		if [ "$waited" -ge 600 ]; then
			echo "site.sh: another run has held $lock for 10 minutes" >&2
			exit 75
		fi
		sleep 5
		waited=$((waited + 5))
	done
	lock_held=$lock
fi

# The online key: a file in the home on an Ubuntu server. On a Dukovo
# server it is the job's Spark secret, which holds the key as base64,
# since a secret loses a trailing newline and a key is binary; it is
# decoded into a folder of the job's own /tmp, which `finish` takes away.
# `ob catalog rollout` runs catalog-rollout.sh through elevate, outside
# any job: it reads the refresh job's secret from Spark's folder as root.
if [ -e "$home/online.key" ]; then
	key=$home/online.key
else
	secret=${SECRETS_DIRECTORY:-/system/state/spark/secrets/dukovo-catalog-refresh}/online-key
	if [ ! -e "$secret" ]; then
		echo "there is no online key: \`service secret set\` puts it in" >&2
		exit 1
	fi
	work=$(umask 077 && mktemp -d)
	(umask 077 && base64 -d "$secret" >"$work/online.key")
	key=$work/online.key
fi

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
	if ! "$program" --repo "$home/repo" --key "$key" "$@" --out "$next"; then
		rm -rf "$next"
		return 1
	fi
	ln -sfn "${next##*/}" "$site/catalog.new" || return 1
	mv -T "$site/catalog.new" "$site/catalog" || return 1
	if [ -n "$current" ]; then
		rm -rf "${site:?}/$current"
	fi
}

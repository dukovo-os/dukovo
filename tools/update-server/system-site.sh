# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# What system-publish.sh and system-refresh.sh share (ADR 0467), which
# they read with `.`: one of them runs at a time, as the user
# dukovo-system, and what they sign is written into a copy of the
# repository being served, made of hard links, which is served only once
# system-release has read it back as a computer would.
# /srv/dukovo-system/system is a link to the folder served, and pointing
# it at the next is one rename, so a computer sees the old repository or
# the new, never part of each, and one that fails its check is never
# seen. Caddy serves it as /system/ through the link
# /srv/dukovo/system, which setup.sh makes.
#
# On the server DUKOVO_SERVER_ROOT is empty. The boot tests set it to a
# folder that stands in for the server's /, and run these scripts there.
# The other places are an Ubuntu server's unless the environment names
# others, as the service files of a Dukovo server do (dukovo/README.md).

server_root=${DUKOVO_SERVER_ROOT:-}
home=$server_root${DUKOVO_SYSTEM_HOME:-/var/lib/dukovo-system}
program=$server_root${DUKOVO_SYSTEM_PROGRAM:-/usr/local/lib/dukovo-system/system-release}
site=$server_root${DUKOVO_SYSTEM_SITE:-/srv/dukovo-system}

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
			echo "system-site.sh: taking over $lock, held for more than 30 minutes" >&2
			rm -rf "$lock.stale"
			mv "$lock" "$lock.stale" 2>/dev/null && rm -rf "$lock.stale"
			continue
		fi
		if [ "$waited" -ge 600 ]; then
			echo "system-site.sh: another run has held $lock for 10 minutes" >&2
			exit 75
		fi
		sleep 5
		waited=$((waited + 5))
	done
	lock_held=$lock
fi

# The online key: a file in the home on an Ubuntu server. On a Dukovo
# server it is a Spark secret, which holds the key as base64, since a
# secret loses a trailing newline and a key is binary: the job's own, or
# the refresh job's when `ob release rollout` runs this through elevate,
# outside a job. It is decoded into a folder of /tmp that `finish` takes
# away.
if [ -e "$home/online.key" ]; then
	key=$home/online.key
else
	secret=${SECRETS_DIRECTORY:-/system/state/spark/secrets/dukovo-system-refresh}/online-key
	work=$(umask 077 && mktemp -d)
	(umask 077 && base64 -d "$secret" >"$work/online.key")
	key=$work/online.key
fi

# Runs system-release's command "$1" on a copy of the repository served,
# with `--repo` the copy and the online key, and the rest of "$@"; then
# serves the copy if system-release succeeded.
sign_site() {
	command=$1
	shift
	current=$(readlink "$site/system" 2>/dev/null || true)
	# A copy left by a run that was stopped part way is not served.
	for dir in "$site"/system.*; do
		if [ -d "$dir" ] && [ ! -L "$dir" ] && [ "${dir##*/}" != "$current" ]; then
			rm -rf "$dir"
		fi
	done
	next=$(mktemp -d "$site/system.XXXXXXXX") || return 1
	chmod 0755 "$next" || return 1
	if [ -n "$current" ]; then
		# system-release writes each file anew and renames it into
		# place, so the files the two folders share are never changed.
		cp -al "$site/$current/." "$next/" || return 1
	fi
	if ! "$program" "$command" --repo "$next" --online-key "$key" "$@"; then
		rm -rf "$next"
		return 1
	fi
	ln -sfn "${next##*/}" "$site/system.new" || return 1
	mv -T "$site/system.new" "$site/system" || return 1
	if [ -n "$current" ]; then
		rm -rf "${site:?}/$current"
	fi
}

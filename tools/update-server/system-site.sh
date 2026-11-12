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

server_root=${DUKOVO_SERVER_ROOT:-}
home=$server_root/var/lib/dukovo-system
incoming=$server_root/var/lib/dukovo-system-incoming
program=$server_root/usr/local/lib/dukovo-system/system-release
site=$server_root/srv/dukovo-system

exec 9>"$home/lock"
flock 9

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
	if ! "$program" "$command" --repo "$next" --online-key "$home/online.key" "$@"; then
		rm -rf "$next"
		return 1
	fi
	ln -sfn "${next##*/}" "$site/system.new" || return 1
	mv -T "$site/system.new" "$site/system" || return 1
	if [ -n "$current" ]; then
		rm -rf "${site:?}/$current"
	fi
}

#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# The machine's side of the lab test bundles/every-catalog-app: each bundle
# the catalogs list is installed, its commands run once, and it is removed;
# the system's paths, with their type, mode, owner, group and size, are
# recorded before the install and after the removal and must differ only by
# the records that outlive a bundle (ADR 0591), and, after a removal that
# keeps its data, by that data. Run as root.
#
#   every-catalog-app.sh purge|keep
#
# purge installs, runs and removes each bundle with --purge. keep removes
# it keeping its data, installs it again, and removes it with --purge: the
# data stays between the two, and the second install finds it again. One
# line a bundle, ok-ID, FAIL-ID with why, or skip-ID for one installed
# for everyone before the test, which it leaves alone; then `MODE N of M`,
# N of M bundles having left nothing. What a failed step printed is in
# /tmp/every-catalog-app.log.
mode=$1
work=/tmp/every-catalog-app
held=0
checked=0

# Every path the removal of a bundle must not leave, recorded in the file
# `$1`: the system's settings and state, /applications, /shared and the
# homes but the lab account's and root's, each a subvolume of its own, and
# /system/live to two levels, where Spark and the services keep what is
# running.
record() {
	{
		find /applications /shared /system/settings /system/state /users \
			\( -path /users/lab -o -path /users/root \) -prune -o -printf '%p %y %m %U %G %s\n'
		find /system/live -maxdepth 2 -printf '%p %y %U %G\n'
	} 2>/dev/null | sort >"$1"
}

# Whether the bundle `$1` is installed for everyone. One that was before
# the test is the machine's: the test leaves it as it is.
installed() {
	tab=$(printf '\t')
	grep -q "^everyone$tab$1$tab" /system/state/bundled/installed 2>/dev/null
}

# What outlives a bundle, as the patterns of a path in a record. bundled's
# registry files, which it writes whole at each change, hold the bindings
# of an ID to its publisher's key (ADR 0383) and the history, and are
# empty otherwise; the folder of the records of its service files, and the
# folder of the services' data, stay; authd's retired numbers are never
# given again (ADR 0220); the logs keep what was said while the bundle was
# there, and the other records of the system change as it runs; and an
# app's folders in a person's home are no one bundle's. The boot test
# bundle-leaves-nothing holds the list, with the reason for each.
outlives='/system/state/bundled/(installed|bound|developer-keys|catalog-bound|previous|held|automatic|defaults|history|catalogs(/.*)?|services)|/system/state/authd/retired|/shared/services|/system/state/(log|sign-ins|starts|metrics|hardware|reports|crash|timed|timerd|netd|sshd|consoled)(/.*)?|/users/[^/]+/(app-data|app-data/.snapshots|applications|applications/commands)'

# What a removal that keeps the data of the bundle `$1` leaves besides:
# its data folder in /shared/services, and its data in each person's
# app-data and what they said of it, the grants.
kept() {
	printf '/shared/services/%s(/.*)?|/users/[^/]+/app-data/%s(/.*)?|/system/state/grants(/.*)?' "$1" "$1"
}

# Whether the records `$2` and `$3` differ only by `$1`: the lines that
# differ and the paths of `$1` are printed, as the log says.
same_but() {
	diff "$2" "$3" | grep '^[<>]' | grep -v -E "^[<>] ($1)( |\$)" >"$work/left"
	if [ -s "$work/left" ]; then
		cat "$work/left" >>/tmp/every-catalog-app.log
		return 1
	fi
}

# Runs each command of the bundle `$1` once, by its path, as a script runs
# one, which no question is asked of. A command that takes none of these
# options to say its version fails the bundle.
run_commands() {
	names=$(bundle info "$1" --json | grep -o '"commands[a-z_]*":\[[^]]*\]' |
		grep -o '"[A-Za-z0-9_.-]*"' | tr -d '"' | grep -v '^commands' | sort -u)
	for name in $names; do
		ran=1
		for option in --version -V -v --help; do
			if timeout 60 "/applications/commands/$name" "$option" </dev/null >>/tmp/every-catalog-app.log 2>&1; then
				ran=0
				break
			fi
		done
		[ "$ran" = 0 ] || return 1
	done
}

install() {
	bundle install --for-everyone --yes "$1" >>/tmp/every-catalog-app.log 2>&1
}

remove() {
	bundle remove --for-everyone "$2" "$1" >>/tmp/every-catalog-app.log 2>&1
}

# One bundle, as `$mode` says: what went wrong, if anything.
try() {
	id=$1
	record "$work/before"
	install "$id" || return 1
	run_commands "$id" || return 1
	case "$mode" in
	purge)
		remove "$id" --purge || return 1
		record "$work/after"
		same_but "$outlives" "$work/before" "$work/after" || return 1
		;;
	keep)
		remove "$id" --keep-data || return 1
		record "$work/kept"
		same_but "$outlives|$(kept "$id")" "$work/before" "$work/kept" || return 1
		install "$id" || return 1
		remove "$id" --purge || return 1
		record "$work/after"
		same_but "$outlives" "$work/before" "$work/after" || return 1
		;;
	esac
}

rm -rf "$work" /tmp/every-catalog-app.log
mkdir -p "$work"
ids=$(bundle search --json | grep -o '"id":"[^"]*"' | cut -d'"' -f4 | sort -u)
for id in $ids; do
	if installed "$id"; then
		echo "skip-$id: installed before the test"
		continue
	fi
	checked=$((checked + 1))
	if try "$id"; then
		held=$((held + 1))
		echo "ok-$id"
	else
		bundle remove --for-everyone --purge "$id" >/dev/null 2>&1
		echo "FAIL-$id: $(tail -n 1 /tmp/every-catalog-app.log | cut -c1-120)"
	fi
done
echo "$mode $held of $checked"

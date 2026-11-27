#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# The machine's side of stress/server-week, run as root on the machine by
# server-week.sh over SSH, or by the test's own steps.
#
#   sh server-week-machine.sh setup SITE ACCOUNT PASSWORD
#   sh server-week-machine.sh check
#   sh server-week-machine.sh dump
#   sh server-week-machine.sh teardown SITE ACCOUNT
#
# setup installs the four bundles from the lab account's home, turns their
# services on and makes the shop: the website SITE for PHP, the database
# week with its two tables and 10000 items, the page server-week.php, which
# is in the lab account's home, and the web console account ACCOUNT with
# PASSWORD. It then lets only ports 22, 80 and 9090 in from other machines,
# and has a backup of the machine's places, which hold the site and
# PostgreSQL's dumps, to a folder of the machine's own disk, which a check
# repeats when the last one is 50 minutes old. It prints `setup done`.
#
# check prints one line, `check` and then NAME=VALUE words: the memory in
# use (neither free nor the page cache, as ob soak reads it) in KiB, the
# processes, the files open in all of them, the space /shared's filesystem
# uses in KiB, the writes PostgreSQL holds, the seconds since the last
# backup, whether the 10000 items are as setup made them (same or
# CHANGED), and whether the four services run (up, or down:NAMES).
#
# dump prints the sequence number of every write PostgreSQL holds, one a
# line. teardown puts the machine back and prints `restored`.
set -u

bundles="org.nginx.nginx net.php.php org.postgresql.postgresql io.valkey.valkey"
services="nginx php-fpm postgresql valkey"
psql=/applications/org.postgresql.postgresql/bin/psql
state=/system/state/server-week
shelf=/system/state/server-week-shelf
backup_name=server-week
# A backup older than this many seconds is made again by a check, as the
# test asks for one every hour.
backup_every=3000

fail() {
	echo "server-week-machine: $*"
	exit 1
}

# Waits up to $2 seconds for the service $1 to run.
wait_running() {
	waited=0
	while [ "$waited" -lt "$2" ]; do
		if service "$1" status -n 0 | grep -q 'active (running)'; then
			return 0
		fi
		waited=$((waited + 1))
		sleep 1
	done
	return 1
}

# The answer of one query, as the superuser, which root is by peer
# authentication.
query() {
	"$psql" -U postgres -d week -X -A -t -c "$1"
}

items_md5() {
	query "SELECT md5(string_agg(id::text || ':' || value, ',' ORDER BY id)) FROM week_items"
}

# Seconds since the last backup finished, or -1 when none did.
backup_age() {
	done_ms=$(backup list --json | grep -o '"done_ms": *[0-9]*' | grep -o '[0-9]*$' | head -n 1)
	if [ -z "$done_ms" ]; then
		echo -1
		return
	fi
	awk -v ms="$done_ms" -v now="$(date +%s)" 'BEGIN { printf "%d\n", now - ms / 1000 }'
}

setup() {
	site=$1
	account=$2
	password=$3
	folder=/shared/www/sites/$site
	mkdir -p "$state" || fail "no folder $state"
	chmod 700 "$state"

	for id in $bundles; do
		# nginx from the lab's catalog, `labtest` (lab-catalog.sh), the
		# others from the files the test copied.
		case "$id" in
		org.nginx.nginx) set -- "$id" --from labtest ;;
		*) set -- "/users/lab/$id.dkv" ;;
		esac
		installed=$(bundle install --for-everyone --yes "$@" 2>&1 | tail -n 1)
		case "$installed" in
		"installed $id"*) ;;
		*) fail "bundle $id: $installed" ;;
		esac
	done
	for name in $services; do
		service "$name" enable || fail "service $name enable failed"
	done
	# PostgreSQL makes its cluster at its first start, which takes minutes
	# on a machine without KVM.
	wait_running postgresql 600 || fail "postgresql is not running"
	for name in nginx php-fpm valkey; do
		wait_running "$name" 120 || fail "$name is not running"
	done

	website add "$site" --kind php --owner lab || fail "website add $site failed"
	cp /users/lab/server-week.php "$folder/server-week.php" || fail "no page in $folder"
	chmod 644 "$folder/server-week.php"

	# The password is shown once, in this JSON, and the page reads it from a
	# file only root and the account php may read.
	added=$(database add week --site "$site" --json) || fail "database add week: $added"
	secret=$(printf '%s\n' "$added" | sed -n 's/.*"password": *"\([^"]*\)".*/\1/p')
	[ -n "$secret" ] || fail "database add week gave no password: $added"
	printf '%s\n' "$secret" >"$folder/.week-database"
	chown root:php "$folder/.week-database" || fail "no account php"
	chmod 640 "$folder/.week-database"

	query "CREATE TABLE week_items (id integer PRIMARY KEY, value text NOT NULL); INSERT INTO week_items SELECT g, md5(g::text) FROM generate_series(1, 10000) AS g; CREATE TABLE week_writes (seq bigint PRIMARY KEY, data text NOT NULL, written timestamptz NOT NULL DEFAULT now()); ALTER TABLE week_items OWNER TO week; ALTER TABLE week_writes OWNER TO week" >/dev/null || fail "the tables were not made"
	items_md5 >"$state/items.md5" || fail "no md5 of the items"

	printf '%s\n' "$password" | user add --admin "$account" || fail "user add $account failed"

	# The rules are made last, as the website's own check goes through the
	# machine's addresses, which always arrive.
	firewall list >"$state/firewall.before"
	for rule in "allow tcp 22" "allow tcp 80" "allow tcp 9090" "deny tcp"; do
		# shellcheck disable=SC2086 # the rule is words of the command
		firewall $rule >/dev/null || fail "firewall $rule failed"
	done

	mkdir -p "$shelf" || fail "no folder $shelf"
	backup add --every hand --verify never "$backup_name" "$shelf" >"$state/backup-add.out" ||
		fail "backup add failed: $(tail -n 3 "$state/backup-add.out")"
	backup run "$backup_name" >/dev/null || fail "the first backup failed"
	echo "setup done"
}

check() {
	memory=$(awk '/^total:/ { t = $2 } /^free:/ { f = $2 } /^page_cache:/ { c = $2 } END { print t - f - c }' /processes/meminfo)
	processes=$(ps -e -o pid= | wc -l | tr -d ' ')
	files=$(lsof | wc -l | tr -d ' ')
	disk=$(df -Pk /shared | awk 'NR == 2 { print $3 }')
	rows=$(query "SELECT count(*) FROM week_writes" 2>/dev/null)
	if [ "$(items_md5 2>/dev/null)" = "$(cat "$state/items.md5")" ]; then
		items=same
	else
		items=CHANGED
	fi
	down=""
	for name in $services; do
		if ! service "$name" status -n 0 | grep -q 'active (running)'; then
			down="$down,$name"
		fi
	done
	if [ -z "$down" ]; then
		running=up
	else
		running="down:${down#,}"
	fi
	age=$(backup_age)
	if [ "$age" -lt 0 ] || [ "$age" -gt "$backup_every" ]; then
		backup run "$backup_name" >/dev/null 2>&1
		age=$(backup_age)
	fi
	echo "check memory=$memory processes=$processes files=$files disk=$disk rows=${rows:--} backup_age=$age items=$items services=$running"
}

teardown() {
	site=$1
	account=$2
	bad=0
	undo() {
		"$@" >/dev/null 2>&1 || {
			echo "server-week-machine: $* failed"
			bad=1
		}
	}
	undo website remove "$site" --files
	undo database remove week --role --force
	undo backup remove "$backup_name"
	for rule in "allow tcp 22" "allow tcp 80" "allow tcp 9090" "deny tcp"; do
		# shellcheck disable=SC2086 # the rule is words of the command
		undo firewall remove $rule
	done
	for id in $bundles; do
		undo bundle remove --for-everyone --purge "$id"
	done
	undo user remove --delete-home --yes "$account"
	rm -rf "$state" "$shelf"
	if [ "$bad" -eq 0 ]; then
		echo restored
	else
		exit 1
	fi
}

case "${1:-}" in
setup) setup "${2:?site}" "${3:?account}" "${4:?password}" ;;
check) check ;;
dump) query "COPY (SELECT seq FROM week_writes ORDER BY seq) TO STDOUT" ;;
teardown) teardown "${2:?site}" "${3:?account}" ;;
*)
	echo "say setup, check, dump or teardown" >&2
	exit 2
	;;
esac

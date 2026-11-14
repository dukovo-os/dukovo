#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# The machine's side of the lab test services/postgresql for the service,
# which its administrator, root, checks and sets up.
#
#   postgresql-service.sh STEP
#
# The steps: ready, which waits for the service and has the administrator
# make the lab account's role; prepare, which makes the roles and databases
# of pgbench and of PHP's tests; stop, which stops the service with a
# session open and starts it again; and hook, which has Spark run the
# service's backup hook. Each prints ok-NAME for each check that held and
# FAIL-NAME for each that did not, then `STEP N of M`, N of M checks having
# held; what failed is in /tmp/postgresql-service.log.
bin=/applications/org.postgresql.postgresql/bin
step=$1
held=0
checked=0

say() {
	checked=$((checked + 1))
	case "$1" in
	ok-*) held=$((held + 1)) ;;
	esac
	echo "$1"
}

check() {
	name=$1
	shift
	if "$@" >>/tmp/postgresql-service.log 2>&1; then
		say "ok-$name"
	else
		say "FAIL-$name"
	fi
}

# The superuser's query, which root asks by peer authentication.
admin_sql() {
	"$bin/psql" -U postgres -X -A -t -c "$1"
}

wait_ready() {
	tries=0
	until service postgresql status -n 0 | grep -q 'active (running)'; do
		tries=$((tries + 1))
		[ "$tries" -lt 300 ] || return 1
		sleep 1
	done
}

# Whether the service's log has a line with `$1`, which logd takes from the
# kernel's a moment after the server wrote it.
logged() {
	tries=0
	until log postgresql | grep -q "$1"; do
		tries=$((tries + 1))
		[ "$tries" -lt 10 ] || return 1
		sleep 1
	done
}

# A session that sleeps for a minute, which the fast shutdown ends.
session_open() {
	"$bin/psql" -U postgres -X -A -t -c 'SELECT pg_sleep(60)' >/dev/null 2>&1 &
	tries=0
	until test "$(admin_sql "SELECT count(*) FROM pg_stat_activity WHERE query = 'SELECT pg_sleep(60)' AND state = 'active'")" = 1; do
		tries=$((tries + 1))
		[ "$tries" -lt 30 ] || return 1
		sleep 1
	done
}

stops_within_ten_seconds() {
	began=$(date +%s)
	service postgresql stop || return 1
	test $(($(date +%s) - began)) -le 10
}

# Started again, the server finds its cluster shut down, not crashed.
restarts_without_recovery() {
	service postgresql start && wait_ready &&
		! log postgresql | grep -q 'not properly shut down'
}

# How many checkpoints a CHECKPOINT has made, in the log.
checkpoints() {
	log postgresql | grep -c 'checkpoint starting: immediate force wait'
}

# Spark runs the before hook of every running service that has one, as
# backup(8) does around its snapshots, and the answer names the service:
# the hook is the bundle's psql running a CHECKPOINT.
hook_checkpoints() {
	before=$(checkpoints)
	printf 'hooks before\n' | nc -w 20 -U /system/live/spark/control | grep -q '^ok: postgresql' ||
		return 1
	tries=0
	until test "$(checkpoints)" -gt "$before"; do
		tries=$((tries + 1))
		[ "$tries" -lt 10 ] || return 1
		sleep 1
	done
}

case "$step" in
ready)
	check ready wait_ready
	check administrator test "$(admin_sql 'SELECT 6 * 7')" = 42
	check person "$bin/createuser" -U postgres lab
	;;
prepare)
	# A role and a database for pgbench, with a password, as it connects
	# over the network; and for PHP's tests a role that may do what some of
	# them do, such as reading the server's files, with the database
	# `test` that they expect.
	check bench_role admin_sql "CREATE ROLE bench LOGIN PASSWORD 'bench-pw'"
	check bench_database "$bin/createdb" -U postgres --owner bench bench
	check php_role admin_sql "CREATE ROLE phptest SUPERUSER LOGIN PASSWORD 'phptest-pw'"
	check php_database "$bin/createdb" -U postgres --owner phptest test
	;;
stop)
	check session session_open
	check stop_time stops_within_ten_seconds
	check fast_shutdown logged 'received fast shutdown request'
	check shut_down logged 'database system is shut down'
	check restart restarts_without_recovery
	;;
hook)
	check hook hook_checkpoints
	;;
esac
echo "$step $held of $checked"

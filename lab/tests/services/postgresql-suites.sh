#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# The machine's side of the lab test services/postgresql: PostgreSQL's own
# test suites and its programs, run against a server started by hand.
#
#   postgresql-suites.sh STEP
#
# The steps, in the order the test runs them: commands, cluster, tools,
# core, plpgsql, isolation, contrib and finish. The server is the lab
# account's, with its cluster in the lab account's home, on the disk and not
# in the memory of /tmp, so that the suites' writes are real ones, and its
# socket in /tmp. The suites are the bundle's (tests, with a schedule each),
# and run as `make installcheck` runs them, each with an output folder of
# its own in /tmp/pg-suites. A suite that starts a server of its own
# (own-server) is run as `make check` runs it. A contrib module with
# isolation tests runs them after its others, as a suite of their own,
# with the extensions that isolation-options names loaded.
#
# Each step prints what it found: a suite its pg_regress summary, the steps
# that check programs a line `NAME N of M`, N of M checks having held. The
# tests that do not pass on Dukovo are in postgresql-skips.txt, each with
# the reason: a line is the suite's name, such as `regress`, `plpgsql`,
# `isolation`, `contrib/pgcrypto` or, for the isolation tests of a contrib
# module, `contrib/pgrowlocks/isolation`, and the test's name. pg_regress
# has no list of tests to leave out, so the schedule it is given is made
# without them.
bundle=/applications/org.postgresql.postgresql
bin=$bundle/bin
tests=$bundle/tests
cluster=/users/lab/pg
out=/tmp/pg-suites
skips=/users/lab/postgresql-skips.txt
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
	if "$@" >>"$out/steps.log" 2>&1; then
		say "ok-$name"
	else
		say "FAIL-$name"
	fi
}

# The query's answer alone, from the server on the socket in /tmp.
sql() {
	"$bin/psql" -h /tmp -d postgres -X -A -t "$@"
}

# Writes $3, the schedule $2 of the suite $1 without the tests the skip
# list names for it. A line of a schedule holds tests that run at once, so
# a name is taken out of its line, and a line left with none is dropped.
without_skipped() {
	cp "$2" "$3"
	grep "^$1 " "$skips" 2>/dev/null | while read -r _ name _; do
		sed -i -E "/^test:/ s/ $name( |\$)/\1/" "$3"
	done
	sed -i '/^test: *$/d' "$3"
}

# Runs pg_regress, or pg_isolation_regress when the program is named in
# $1, for the suite $2 with the options after it, and prints its summary
# line and, for each test that failed, the line of the skip list that would
# leave it out. pg_regress writes its results as TAP: `# All N tests
# passed.` or `# M of N tests failed.`, and `not ok 7 + name 12 ms` for each
# test that failed.
run_suite() {
	program=$1
	name=$2
	shift 2
	mkdir -p "$out/$name"
	"$tests/$program" --bindir="$bin" --outputdir="$out/$name" "$@" >"$out/$name.log" 2>&1
	status=$?
	summary=$(grep -e 'tests passed' -e 'tests failed' "$out/$name.log" | tail -n 1 | sed 's/^[# ]*//')
	if [ -n "$summary" ]; then
		echo "$name: $summary"
	else
		echo "$name: no summary, the log ends:"
		tail -n 10 "$out/$name.log"
	fi
	sed -n -E "s|^not ok +[0-9]+ +[-+] +([^ ]+).*|$name \1 failed|p" "$out/$name.log"
	return $status
}

# Runs one kind of the tests of the contrib module $1: $2 is regress for the
# tests pg_regress runs, from the module's schedule, or isolation for those
# pg_isolation_regress runs, from its isolation_schedule. The suite is named
# contrib/NAME, or contrib/NAME/isolation, in the skip list and in what it
# prints. A module has the kinds its makefile has, and it is counted, in
# $suites and in $failed if it fails, once it has a test left to run. The
# tests run in the server on /tmp, or in one that pg_regress starts when
# the module needs settings of its own (own-server), and from the module's
# folder, as make runs them: a test reads its data from a path such as
# data/int2.data. The isolation tests first have the extensions of
# isolation-options loaded.
contrib_suite() {
	module=$1
	kind=$2
	folder="$tests/contrib/$module"
	if [ "$kind" = isolation ]; then
		suite=contrib/$module/isolation
		list="$folder/isolation_schedule"
		program=pg_isolation_regress
	else
		suite=contrib/$module
		list="$folder/schedule"
		program=pg_regress
	fi
	if [ ! -f "$list" ]; then
		return 0
	fi
	flat=$(echo "$suite" | tr / -)
	without_skipped "$suite" "$list" "$out/$flat.schedule"
	if [ ! -s "$out/$flat.schedule" ]; then
		echo "$suite: no test left to run"
		return 0
	fi
	suites=$((suites + 1))
	if [ "$kind" = isolation ]; then
		set -- --dbname=isolation_regression $(cat "$folder/isolation-options")
	else
		set -- --dbname=contrib_regression --dlpath="$tests/regress"
	fi
	if [ -f "$folder/own-server" ]; then
		set -- "$@" --temp-instance="$out/server-$flat"
		for conf in $(cat "$folder/own-server"); do
			set -- "$@" --temp-config="$folder/$conf"
		done
	else
		set -- "$@" --host=/tmp
	fi
	(cd "$folder" && run_suite "$program" "$suite" "$@" --inputdir="$folder" \
		--schedule="$out/$flat.schedule") || failed=$((failed + 1))
}

# The programs of the bundle, each run for its version: one that starts is
# linked and has found libpq.
every_command_runs() {
	for program in clusterdb createdb createuser dropdb dropuser initdb oid2name pg_amcheck \
		pg_archivecleanup pg_basebackup pg_checksums pg_combinebackup pg_config pg_controldata \
		pg_createsubscriber pg_ctl pg_dump pg_dumpall pg_isready pg_receivewal pg_recvlogical \
		pg_resetwal pg_restore pg_rewind pg_test_fsync pg_test_timing pg_upgrade pg_verifybackup \
		pg_waldump pg_walsummary pgbench postgres psql reindexdb vacuumdb vacuumlo; do
		check "$program" "$bin/$program" --version
	done
}

# A server a run before left up would hold the socket and the port. The
# server's own connections to itself, dblink's and postgres_fdw's, look for
# its socket in /tmp too, where libpq's default is the service's folder.
make_cluster() {
	if [ -f "$cluster/postmaster.pid" ]; then
		"$bin/pg_ctl" -D "$cluster" -m immediate -t 60 stop || :
	fi
	rm -rf "$cluster"
	"$bin/initdb" -D "$cluster" --locale-provider=builtin --builtin-locale=C.UTF-8 --locale=C -E UTF8 &&
		PGHOST=/tmp "$bin/pg_ctl" -D "$cluster" -o '-k /tmp' -l "$out/server.log" -t 300 start &&
		test "$(sql -c 'SELECT 6 * 7')" = 42
}

amcheck_finds_no_fault() {
	sql -c 'CREATE EXTENSION IF NOT EXISTS amcheck' &&
		"$bin/pg_amcheck" -h /tmp -d postgres
}

backup_verifies() {
	rm -rf /users/lab/pg-backup
	"$bin/pg_basebackup" -h /tmp -D /users/lab/pg-backup -X stream -c fast &&
		"$bin/pg_verifybackup" /users/lab/pg-backup
}

waldump_reads_the_log() {
	segment=$(ls "$cluster/pg_wal" | grep '^[0-9A-F]\{24\}$' | sort | head -n 1)
	"$bin/pg_waldump" -p "$cluster/pg_wal" -n 5 "$segment" | grep -q 'rmgr:'
}

lists_databases() {
	"$bin/oid2name" -h /tmp | grep -q 'All databases'
}

stopped_cluster_is_sound() {
	"$bin/pg_checksums" --check -D "$cluster" && "$bin/pg_resetwal" -n -D "$cluster"
}

mkdir -p "$out"
case "$step" in
commands)
	every_command_runs
	;;
cluster)
	check cluster make_cluster
	;;
tools)
	check vacuumdb "$bin/vacuumdb" -h /tmp --all --analyze
	check reindexdb "$bin/reindexdb" -h /tmp --all
	check clusterdb "$bin/clusterdb" -h /tmp --all
	check amcheck amcheck_finds_no_fault
	check oid2name lists_databases
	check vacuumlo "$bin/vacuumlo" -h /tmp postgres
	check pg_controldata test "$("$bin/pg_controldata" "$cluster" | grep 'cluster state')" != ''
	check pg_basebackup backup_verifies
	check pg_waldump waldump_reads_the_log
	check pg_isready "$bin/pg_isready" -h /tmp
	check createdb "$bin/createdb" -h /tmp tools
	check pg_dump "$bin/pg_dump" -h /tmp -f "$out/tools.sql" tools
	check dropdb "$bin/dropdb" -h /tmp tools
	;;
core)
	without_skipped regress "$tests/regress/parallel_schedule" "$out/regress.schedule"
	run_suite pg_regress regress --host=/tmp --inputdir="$tests/regress" --dlpath="$tests/regress" \
		--max-concurrent-tests=20 --schedule="$out/regress.schedule"
	;;
plpgsql)
	without_skipped plpgsql "$tests/plpgsql/schedule" "$out/plpgsql.schedule"
	run_suite pg_regress plpgsql --host=/tmp --dbname=pl_regression --inputdir="$tests/plpgsql" \
		--schedule="$out/plpgsql.schedule"
	;;
isolation)
	without_skipped isolation "$tests/isolation/isolation_schedule" "$out/isolation.schedule"
	run_suite pg_isolation_regress isolation --host=/tmp --inputdir="$tests/isolation" \
		--schedule="$out/isolation.schedule"
	;;
contrib)
	failed=0
	suites=0
	for dir in "$tests"/contrib/*; do
		contrib_suite "${dir##*/}" regress
		contrib_suite "${dir##*/}" isolation
	done
	echo "contrib: $failed failed of $suites"
	;;
finish)
	"$bin/pg_ctl" -D "$cluster" -m fast -t 300 stop
	check checks_and_dry_reset stopped_cluster_is_sound
	check pg_test_fsync "$bin/pg_test_fsync" -f "$cluster/fsync-test" -s 1
	check pg_test_timing "$bin/pg_test_timing" -d 1
	rm -rf "$cluster" /users/lab/pg-backup
	;;
esac
case "$step" in
commands | tools | finish)
	echo "$step $held of $checked"
	;;
esac

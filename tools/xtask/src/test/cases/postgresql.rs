// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! PostgreSQL, the catalog's database server, runs by hand (ADR 0521).
//! Dukovo's root is on a disk, so that the bundle has room, and a second
//! disk holds the `.dkv` files `ob ports` packed, PostgreSQL's among them,
//! and the scripts that make the checks. The bundle has a service (ADR
//! 0522), so it installs only for everyone: Alice does it through elevate
//! and leaves the service off. Her psql asks, with a warning, whether it
//! may run with her access, then says it is PostgreSQL 19. The programs
//! find the bundle's folder from their own path, which the kernel gave
//! them: pg_config names it, initdb makes a cluster in /tmp by its command
//! name, and pg_ctl starts the server that is beside it, on a socket in
//! /tmp, as a person developing would. A script checks a table, an index
//! and a join, the modules pgcrypto, pg_trgm, hstore and pg_stat_statements,
//! a PL/pgSQL function, a transaction rolled back, a time zone of the
//! system's, a notification from a second session, a parallel query with
//! two workers, a statement ended by its timeout and a checkpoint. The
//! server listens on localhost, as PostgreSQL does unless it is told
//! otherwise, and psql reaches it there over TCP. pg_dump copies a table
//! into another database, and the rows are there after a fast stop and a
//! start, and after the postmaster is killed, when the log says the server
//! recovered. The power button ends it.

use super::super::*;

/// The disk of bundles' label.
const LABEL: &str = "BUNDLES";

/// Where Spark mounts it: the second virtio disk's name.
const VOLUME: &str = "/volumes/vdb";

/// PostgreSQL's bundle file.
const FILE: &str = "org.postgresql.postgresql-19.0.dkv";

/// The steps' script, by its path on the disk.
const SCRIPT: &str = "/volumes/vdb/postgresql.sh";

/// Dukovo's root on a copy of the test image, and the bundles, with the
/// checks' scripts beside them.
const DISKS: &[Disk] = &[
    Disk { label: image::ROOT_LABEL, ..btrfs_disk(Bus::Virtio, 512, ROOT, root_checked) },
    Disk { fat: POSTGRESQL_FILES, ..bundle_files_disk(192 * 1024, LABEL) },
];

/// The test.
pub const CASE: Case = Case {
    name: "postgresql",
    about: "PostgreSQL, the catalog's database server, runs by hand (ADR 0521): installed for everyone through elevate from its .dkv file, psql asks whether it may run with her access and says it is 19; pg_config and initdb find the bundle's folder from the program's path; a server started by hand on a socket in /tmp passes a script's checks of tables, indexes, joins, modules, PL/pgSQL, rollback, time zones, notifications, a parallel query, a statement timeout and a checkpoint, answers over TCP on localhost, copies a table through pg_dump, and keeps its rows through a fast stop and through the postmaster being killed",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    cmdline: "lockdep",
    cpus: 2,
    memory: "2G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &[],
    script: postgresql,
    disks: DISKS,
    second: None,
};

/// The SQL of the checks that the server alone answers, which the script
/// runs with psql: each prints `ok-<name>` if it holds and `FAIL-<name>` if
/// not.
const CHECKS_SQL: &str = r"-- Run by `psql -v ON_ERROR_STOP=1` against the server of the postgresql
-- test: each check prints ok-NAME if it holds and FAIL-NAME if not.
\pset tuples_only on
\pset format unaligned

CREATE FUNCTION pg_temp.check(name text, held boolean) RETURNS text
    LANGUAGE sql AS $$ SELECT CASE WHEN $2 THEN 'ok-' ELSE 'FAIL-' END || $1 $$;

-- The plan of a query, as text.
CREATE FUNCTION pg_temp.plan(query text) RETURNS text LANGUAGE plpgsql AS $$
DECLARE
    line text;
    plan text := '';
BEGIN
    FOR line IN EXECUTE 'EXPLAIN (COSTS OFF) ' || query LOOP
        plan := plan || line || E'\n';
    END LOOP;
    RETURN plan;
END
$$;

-- A table, an index and a join.
CREATE TABLE items (id int PRIMARY KEY, name text NOT NULL, price numeric(8, 2));
INSERT INTO items SELECT i, 'item-' || i, i * 1.5 FROM generate_series(1, 1000) AS i;
CREATE INDEX items_name ON items (name);
CREATE TABLE orders (item int REFERENCES items, quantity int);
INSERT INTO orders SELECT 1 + i % 100, i FROM generate_series(1, 500) AS i;
ANALYZE;
SELECT pg_temp.check('table', (SELECT count(*) FROM items) = 1000);
SET enable_seqscan = off;
SELECT pg_temp.check('index',
    pg_temp.plan($$SELECT id FROM items WHERE name = 'item-77'$$) LIKE '%items_name%');
RESET enable_seqscan;
SELECT pg_temp.check('join',
    (SELECT sum(o.quantity) FROM orders AS o JOIN items AS i ON i.id = o.item
        WHERE i.name = 'item-5') = 1020);

-- The contrib modules, each a shared library the server loads.
CREATE EXTENSION pgcrypto;
SELECT pg_temp.check('pgcrypto', encode(digest('abc', 'sha256'), 'hex')
    = 'ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad');
CREATE EXTENSION pg_trgm;
SELECT pg_temp.check('pg_trgm', similarity('dukovo', 'dukovo') = 1
    AND similarity('dukovo', 'radix') < 0.2);
CREATE EXTENSION hstore;
SELECT pg_temp.check('hstore', ('a => 1, b => 2'::hstore -> 'b') = '2');
CREATE EXTENSION pg_stat_statements;
SELECT pg_temp.check('pg_stat_statements', (SELECT count(*) FROM pg_stat_statements) > 0);

-- A PL/pgSQL function, which initdb's own template databases need the
-- language for.
CREATE FUNCTION factorial(n int) RETURNS bigint LANGUAGE plpgsql AS $$
DECLARE
    result bigint := 1;
BEGIN
    FOR i IN 2..n LOOP
        result := result * i;
    END LOOP;
    RETURN result;
END
$$;
SELECT pg_temp.check('plpgsql', factorial(10) = 3628800);

-- A transaction that is rolled back leaves no row.
BEGIN;
INSERT INTO items VALUES (5000, 'gone', 1);
ROLLBACK;
SELECT pg_temp.check('rollback', (SELECT count(*) FROM items WHERE id = 5000) = 0);

-- Time zones are the system's.
SET timezone = 'Europe/Paris';
SELECT pg_temp.check('timezone',
    extract(hour FROM timestamptz '2026-07-01 12:00:00+00') = 14);

CHECKPOINT;
SELECT pg_temp.check('checkpoint', true);
";

/// The steps, a script the test runs from the disk of bundles, one for
/// each argument: it starts the server by hand and asks it, and counts the
/// checks that held, which it prints after them.
const STEPS_SCRIPT: &str = r#"# The steps of the postgresql test that the shell takes, one for each
# argument: a server started by hand, as a person would to develop, with
# its cluster in /tmp/pg and its socket in /tmp, the service's folder not
# being there for a server that is not the service. Each step prints
# ok-NAME for each check that held and FAIL-NAME for each that did not, then
# how many held, and its commands' output goes to /tmp/steps.log.
here=$(cd "$(dirname "$0")" && pwd)
data=/tmp/pg
options="-k /tmp -c shared_preload_libraries=pg_stat_statements"
held=0
checked=0

# Prints a check's word and counts it.
say() {
	checked=$((checked + 1))
	case "$1" in
	ok-*) held=$((held + 1)) ;;
	esac
	echo "$1"
}

# A query's answer alone, from the server on the socket in /tmp.
sql() {
	psql -h /tmp -d postgres -X -A -t "$@"
}

check() {
	name=$1
	shift
	if "$@" >>/tmp/steps.log 2>&1; then
		say "ok-$name"
	else
		say "FAIL-$name"
	fi
}

make_cluster() {
	initdb -D "$data" --locale-provider=builtin --builtin-locale=C.UTF-8 --locale=C -E UTF8
}

start() {
	pg_ctl -D "$data" -o "$options" -l /tmp/pg.log -t 180 start
}

stop() {
	pg_ctl -D "$data" -m fast -t 180 stop
}

notified() {
	psql -h /tmp -d postgres -X -q <<'SQL' | grep -q 'Asynchronous notification "chan" with payload "hi"'
LISTEN chan;
\! psql -h /tmp -d postgres -X -q -c "NOTIFY chan, 'hi'"
SELECT 1;
SQL
}

parallel() {
	sql -c 'CREATE TABLE big AS SELECT i, md5(i::text) AS h FROM generate_series(1, 100000) AS i' &&
		sql -c 'SET max_parallel_workers_per_gather = 2' -c 'SET parallel_setup_cost = 0' \
			-c 'SET parallel_tuple_cost = 0' -c 'SET min_parallel_table_scan_size = 0' \
			-c 'EXPLAIN (ANALYZE, COSTS OFF, TIMING OFF) SELECT count(*) FROM big' |
		grep -q 'Workers Launched: 2'
}

ended_by_timeout() {
	sql -c 'SET statement_timeout = 500' -c 'SELECT pg_sleep(5)' 2>&1 |
		grep -q 'canceling statement due to statement timeout'
}

over_tcp() {
	test "$(psql -h 127.0.0.1 -d postgres -X -A -t -c 'SELECT 6 * 7')" = 42
}

copied() {
	createdb -h /tmp copy &&
		pg_dump -h /tmp -d postgres -t items |
		psql -h /tmp -d copy -X -q -v ON_ERROR_STOP=1 &&
		test "$(psql -h /tmp -d copy -X -A -t -c 'SELECT count(*) FROM items')" = 1000
}

kept_through_a_stop() {
	stop && start && test "$(sql -c 'SELECT count(*) FROM items')" = 1000
}

# The postmaster is killed with no time to write anything. Its other
# processes see that it is gone and end, which a start before they have
# finds its shared memory in use and refuses; so the start is tried again.
recovered() {
	kill -9 "$(head -1 "$data/postmaster.pid")" || return 1
	tries=0
	until start; do
		tries=$((tries + 1))
		[ "$tries" -lt 15 ] || return 1
		sleep 2
	done
	grep -q 'automatic recovery in progress' /tmp/pg.log &&
		test "$(sql -c 'SELECT count(*) FROM items')" = 1000
}

case "$1" in
init)
	check initdb make_cluster
	check version test "$(cat "$data/PG_VERSION")" = 19
	;;
start)
	check start start
	check running pg_ctl -D "$data" status
	check connects test "$(sql -c 'SELECT 6 * 7')" = 42
	;;
checks)
	psql -h /tmp -d postgres -X -q -v ON_ERROR_STOP=1 -f "$here/postgresql.sql" >/tmp/sql.out 2>&1
	# Whatever the script printed besides a check's word is a failure.
	while read -r line; do
		if [ -n "$line" ]; then
			say "$line"
		fi
	done </tmp/sql.out
	check notify notified
	check parallel parallel
	check statement_timeout ended_by_timeout
	;;
tcp)
	check tcp over_tcp
	;;
copy)
	check copy copied
	;;
stop)
	check stop_start kept_through_a_stop
	;;
crash)
	check recovery recovered
	;;
finish)
	check finish stop
	;;
esac
echo "$1 $held of $checked"
"#;

/// Makes the disk of bundles, with the checks' SQL, `postgresql.sql`, and
/// the steps' script, `postgresql.sh`, beside the bundles.
pub fn make_disk(ctx: &Ctx, disk: &Disk, path: &Path) -> Result<(), String> {
    make_bundle_files_with(
        ctx,
        disk,
        path,
        &[("postgresql.sql", CHECKS_SQL), ("postgresql.sh", STEPS_SCRIPT)],
    )
}

fn postgresql(vm: &mut Vm, case: &Case) -> Result<(), String> {
    owner_signs_in(vm, case)?;
    vm.expect_anywhere("bundled: 0 bundles installed; answering at /system/live/bundled/socket")?;
    vm.expect_anywhere(&format!("spark: mounted the FAT filesystem of vdb at {VOLUME}"))?;
    // For everyone, through elevate, as the bundle has a service: it
    // installs for no one else. The service waits to be enabled, which
    // this test never does.
    answer_prompt(
        vm,
        "$",
        &format!(
            "clear; cd {VOLUME}; elevate bundle install --for-everyone --yes {FILE} | tail -1; cd"
        ),
    )?;
    answer_prompt(vm, &format!("{FILE}:"), "queen of hearts")?;
    answer_prompt_within(
        vm,
        "$",
        "clear; echo \"installed $(ls -d /applications/org.postgresql.postgresql* | wc -l)\"",
        1800,
    )?;
    vm.expect_screen(&["installed 1"])?;
    // psql runs with the access of whoever runs it: its first run asks,
    // with a warning.
    run_answering_until(vm, "clear; psql --version", "psql (PostgreSQL) 19", 120)?;
    // The bundle's folder, which PostgreSQL finds from its program's path:
    // the kernel gave it (patch 0001), as the command was found by its name
    // in /system/commands, which links to the bundle.
    answer_prompt(vm, "$", "clear; pg_config --bindir")?;
    vm.expect_screen(&["/applications/org.postgresql.postgresql/bin"])?;
    // initdb makes the cluster, and pg_ctl finds postgres beside itself.
    run_step(vm, SCRIPT, "init", 2, 600)?;
    run_step(vm, SCRIPT, "start", 3, 300)?;
    run_step(vm, SCRIPT, "checks", 14, 600)?;
    // The server listens on localhost unless told otherwise: it answers there.
    run_step(vm, SCRIPT, "tcp", 1, 300)?;
    run_step(vm, SCRIPT, "copy", 1, 300)?;
    // A fast stop and a start keep the rows. Killed, the postmaster leaves
    // the cluster as a crash does: the log says the next start recovered.
    run_step(vm, SCRIPT, "stop", 1, 300)?;
    run_step(vm, SCRIPT, "crash", 1, 300)?;
    run_step(vm, SCRIPT, "finish", 1, 300)?;
    vm.press_power_button()?;
    vm.expect("Power: `spark` asked to power off the machine; syncing the filesystems")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}

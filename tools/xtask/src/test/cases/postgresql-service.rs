// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! PostgreSQL's service, the database server of the PostgreSQL bundle, runs
//! for the system (ADR 0522). Dukovo's root is on a disk, so that the
//! bundles have room, and a second disk holds the `.dkv` files `ob ports`
//! packed, PostgreSQL's and PHP's among them, with the scripts that make
//! the checks. Through elevate, Alice installs PostgreSQL for everyone:
//! authd makes the account postgres, and the service waits until she
//! enables it. Enabled, its first start makes the cluster with initdb, in
//! the account's data folder with the bundle's pg_hba.conf, and the service
//! is ready once pg_isready says it takes connections. The server's socket
//! is in the service's live folder, where libpq looks for it: `elevate
//! psql -U postgres` is the superuser, by the peer authentication of root,
//! and Alice, once `createuser` made her a role, is herself in psql, by
//! hers. A role with a password owns a database made with English's locale
//! of the C library, whose words sort as glibc sorts them and whose
//! collation has the version of the locales' data; the role connects over
//! 127.0.0.1 with its password, and not with another, and a query runs with
//! two parallel workers. With PHP's bundle installed too, a script's pgsql
//! and PDO connections read and insert, with a prepared statement, and roll
//! a transaction back. With a session open, stopping the service returns
//! within ten seconds: the log has the fast shutdown request and the
//! server shut down, and the start after it needs no recovery. A reload
//! has the server read its settings again. The power button ends it.

use super::super::*;

/// The disk of bundles' label.
const LABEL: &str = "BUNDLES";

/// Where Spark mounts it: the second virtio disk's name.
const VOLUME: &str = "/volumes/vdb";

/// PostgreSQL's bundle file.
const POSTGRESQL_FILE: &str = "org.postgresql.postgresql-19.0.dkv";

/// PHP's bundle file.
const PHP_FILE: &str = "net.php.php-8.5.12.dkv";

/// The steps' script, by its path on the disk.
const SCRIPT: &str = "/volumes/vdb/service.sh";

/// Dukovo's root on a copy of the test image, and the bundles, with the
/// checks' scripts beside them.
const DISKS: &[Disk] = &[
    Disk { label: image::ROOT_LABEL, ..btrfs_disk(Bus::Virtio, 512, ROOT, root_checked) },
    Disk { fat: POSTGRESQL_SERVICE_FILES, ..bundle_files_disk(192 * 1024, LABEL) },
];

/// The test.
pub const CASE: Case = Case {
    name: "postgresql-service",
    about: "PostgreSQL's service runs for the system (ADR 0522): installed for everyone from its .dkv file and enabled, it makes its cluster at its first start and is ready once pg_isready passes; elevate psql -U postgres is the superuser by root's peer authentication and a person with a role is herself; a role with a password owns a database of English's locale of the C library, which sorts as glibc does and has the locales' version, and connects over 127.0.0.1 with its password and not another; a parallel query runs two workers; PHP's pgsql and PDO connect, insert, read and roll back; stopped with a session open it returns within ten seconds by the fast shutdown and starts again without recovery; a reload reads its settings",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    // As a normal boot: Spark starts the services, bundled among them.
    cmdline: "lockdep",
    cpus: 2,
    memory: "2G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &[],
    script: postgresql_service,
    disks: DISKS,
    second: None,
};

/// The steps, a script the test runs from the disk of bundles, one for
/// each argument: it asks the service as an administrator and as a person,
/// and counts the checks that held, which it prints after them.
const STEPS_SCRIPT: &str = r#"# The steps of the postgresql-service test that the shell takes, one for
# each argument: the service as an administrator and a person use it. Each
# step prints ok-NAME for each check that held and FAIL-NAME for each that
# did not, then how many held, and its commands' output goes to
# /tmp/steps.log. The commands of an administrator go through elevate,
# which the test has unlocked in the pane.
here=$(cd "$(dirname "$0")" && pwd)
data=/shared/services/org.postgresql.postgresql/postgres/data
socket=/system/live/services/postgres/.s.PGSQL.5432
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

check() {
	name=$1
	shift
	if "$@" >>/tmp/steps.log 2>&1; then
		say "ok-$name"
	else
		say "FAIL-$name"
	fi
}

# The server's answer to a query alone, as the superuser, by the socket.
admin_sql() {
	elevate psql -U postgres -X -A -t -c "$1"
}

# The same as the role shop, over the network with its password, as a
# program on this machine that has no account of its own to show.
shop_sql() {
	PGPASSWORD=shop-pw psql -h 127.0.0.1 -U shop -d shop -X -A -t "$@"
}

is_ready() {
	elevate service postgresql status -n 0 | grep -q 'active (running)'
}

wait_ready() {
	tries=0
	until is_ready; do
		tries=$((tries + 1))
		[ "$tries" -lt 120 ] || return 1
		sleep 1
	done
}

# Whether the service's log has a line with `$1`, which logd takes from
# the kernel's a moment after the server wrote it.
logged() {
	tries=0
	until elevate log postgresql | grep -q "$1"; do
		tries=$((tries + 1))
		[ "$tries" -lt 10 ] || return 1
		sleep 1
	done
}

words="éclair Éclair zoo zebra résumé resume eclair côté côte coté cote coop co-op banana apple Zürich Zurich Zebra Résumé Banana Apple APPLE 9 100 10"
sorted="10 100 9 apple Apple APPLE banana Banana co-op coop cote coté côte côté eclair éclair Éclair resume résumé Résumé zebra Zebra zoo Zurich Zürich"

# A database made with English's locale of the C library sorts as glibc's
# does, which the C library's data follows.
sorted_as_glibc() {
	test "$(shop_sql -c "SELECT string_agg(w, ' ' ORDER BY w) FROM unnest(string_to_array('$words', ' ')) AS w")" = "$sorted"
}

# The version recorded for its collation is the one of the locales' data.
collation_version_recorded() {
	test "$(shop_sql -c "SELECT datcollversion FROM pg_database WHERE datname = 'shop'")" = "$(cat /system/libraries/locale/VERSION)"
}

wrong_password_refused() {
	if PGPASSWORD=not-the-password psql -h 127.0.0.1 -U shop -d shop -X -c 'SELECT 1'; then
		return 1
	fi
	return 0
}

parallel() {
	shop_sql -c 'CREATE TABLE big AS SELECT i, md5(i::text) AS h FROM generate_series(1, 100000) AS i' &&
		shop_sql -c 'SET max_parallel_workers_per_gather = 2' -c 'SET parallel_setup_cost = 0' \
			-c 'SET parallel_tuple_cost = 0' -c 'SET min_parallel_table_scan_size = 0' \
			-c 'EXPLAIN (ANALYZE, COSTS OFF, TIMING OFF) SELECT count(*) FROM big' |
		grep -q 'Workers Launched: 2'
}

# A session that sleeps for a minute, opened as the person alice, which the
# service's stop ends: the fast shutdown disconnects it, and the stop is
# over in seconds, not after the minute.
session_open() {
	psql -d postgres -X -A -t -c 'SELECT pg_sleep(60)' >/dev/null 2>&1 &
	tries=0
	until test "$(admin_sql "SELECT count(*) FROM pg_stat_activity WHERE query = 'SELECT pg_sleep(60)' AND state = 'active'")" = 1; do
		tries=$((tries + 1))
		[ "$tries" -lt 30 ] || return 1
		sleep 1
	done
}

stops_within_ten_seconds() {
	began=$(date +%s)
	elevate service postgresql stop || return 1
	test $(($(date +%s) - began)) -le 10
}

# Started again, the server finds its cluster shut down, not crashed.
restarts_without_recovery() {
	elevate service postgresql start && wait_ready &&
		! elevate log postgresql | grep -q 'not properly shut down'
}

reloads() {
	elevate service postgresql reload && logged 'received SIGHUP, reloading configuration files'
}

case "$1" in
ready)
	check ready wait_ready
	check owner test "$(elevate stat -c %U "$data")" = postgres
	check socket test -S "$socket"
	check hba elevate grep -q 'map=administrators' "$data/pg_hba.conf"
	;;
admin)
	check superuser test "$(admin_sql 'SELECT current_user')" = postgres
	check version test "$(admin_sql "SELECT current_setting('server_version_num')::int / 10000")" = 19
	;;
person)
	check createuser elevate createuser -U postgres alice
	check peer test "$(psql -d postgres -X -A -t -c 'SELECT current_user')" = alice
	;;
database)
	check role elevate psql -U postgres -X -q -c "CREATE ROLE shop LOGIN PASSWORD 'shop-pw'"
	check createdb elevate createdb -U postgres --owner shop --template template0 --locale-provider libc --locale en_US.UTF-8 shop
	check password shop_sql -c 'SELECT 1'
	check refused wrong_password_refused
	check collation sorted_as_glibc
	check collation_version collation_version_recorded
	;;
parallel)
	check parallel parallel
	;;
stop)
	check session session_open
	check stop_time stops_within_ten_seconds
	check fast_shutdown logged 'received fast shutdown request'
	check shut_down logged 'database system is shut down'
	check restart restarts_without_recovery
	;;
reload)
	check reload reloads
	;;
php)
	php "$here/service.php" >/tmp/php.out 2>&1
	# Whatever PHP printed besides a check's word is a failure.
	while read -r line; do
		if [ -n "$line" ]; then
			say "$line"
		fi
	done </tmp/php.out
	;;
esac
echo "$1 $held of $checked"
"#;

/// The checks PHP makes, a script that connects to the service as a page
/// would.
const CHECKS_PHP: &str = r#"<?php
// The checks of the postgresql-service test that PHP makes: its pgsql and
// pdo_pgsql extensions reach the service over 127.0.0.1 with the password
// of the role shop, as a page would: the account that runs PHP-FPM's pages
// is none of the service's roles. Each check prints ok-<name> if it holds
// and FAIL-<name> if not.
function c($name, $held) {
    echo ($held ? "ok-" : "FAIL-") . $name . "\n";
}

$link = pg_connect("host=127.0.0.1 dbname=shop user=shop password=shop-pw");
c("pg_connect", $link !== false);
pg_query($link, "CREATE TABLE guests (id serial PRIMARY KEY, name text NOT NULL, visits int NOT NULL DEFAULT 0)");
pg_query_params($link, "INSERT INTO guests (name) VALUES ($1)", ["alice"]);
c("pg_query", pg_fetch_result(pg_query($link, "SELECT name FROM guests"), 0, 0) == "alice");

$pdo = new PDO("pgsql:host=127.0.0.1;dbname=shop", "shop", "shop-pw");
$pdo->setAttribute(PDO::ATTR_ERRMODE, PDO::ERRMODE_EXCEPTION);
c("pdo", $pdo->query("SELECT current_user")->fetchColumn() == "shop");

// A prepared statement, which the server plans once for its parameters.
$insert = $pdo->prepare("INSERT INTO guests (name, visits) VALUES (?, ?)");
$insert->execute(["hatter", 3]);
$read = $pdo->prepare("SELECT visits FROM guests WHERE name = ?");
$read->execute(["hatter"]);
c("prepared", $read->fetchColumn() == 3);

$pdo->beginTransaction();
$pdo->exec("INSERT INTO guests (name) VALUES ('dormouse')");
$pdo->rollBack();
c("rollback", $pdo->query("SELECT count(*) FROM guests WHERE name = 'dormouse'")->fetchColumn() == 0);

try {
    new PDO("pgsql:host=127.0.0.1;dbname=shop", "shop", "not-the-password");
    $refused = false;
} catch (PDOException $e) {
    $refused = true;
}
c("refused", $refused);
"#;

/// Makes the disk of bundles, with the steps' script, `service.sh`, and
/// PHP's checks, `service.php`, beside the bundles.
pub fn make_disk(ctx: &Ctx, disk: &Disk, path: &Path) -> Result<(), String> {
    make_bundle_files_with(
        ctx,
        disk,
        path,
        &[("service.sh", STEPS_SCRIPT), ("service.php", CHECKS_PHP)],
    )
}

/// Installs `file` for everyone, through elevate, which is unlocked, and
/// waits up to `seconds` for it to be in /applications as `folder`.
fn install(vm: &mut Vm, file: &str, folder: &str, seconds: u64) -> Result<(), String> {
    answer_prompt(
        vm,
        "$",
        &format!(
            "clear; cd {VOLUME}; elevate bundle install --for-everyone --yes {file} | grep -e \
             'as the account' -e installed; cd"
        ),
    )?;
    answer_prompt_within(vm, "$", &format!("clear; ls -d /applications/{folder}*"), seconds)?;
    vm.expect_screen(&[&format!("/applications/{folder}")])
}

fn postgresql_service(vm: &mut Vm, case: &Case) -> Result<(), String> {
    owner_signs_in(vm, case)?;
    vm.expect_anywhere("bundled: 0 bundles installed; answering at /system/live/bundled/socket")?;
    vm.expect_anywhere(&format!("spark: mounted the FAT filesystem of vdb at {VOLUME}"))?;
    // For everyone, through elevate: the service waits to be enabled.
    unlock(vm)?;
    install(vm, POSTGRESQL_FILE, "org.postgresql.postgresql", 1800)?;
    vm.expect_anywhere("authd: made the account postgres (")?;
    // Enabled, it makes its cluster and starts.
    unlock(vm)?;
    answer_prompt(vm, "$", "clear; elevate service postgresql enable; echo \"enable $?\"")?;
    vm.expect_screen(&["enable 0"])?;
    vm.expect_anywhere("database system is ready to accept connections")?;
    // psql runs with the access of whoever runs it, and elevate with root's:
    // each first run asks, with a warning, in her pane (ADR 0493).
    run_answering_until(vm, "clear; psql --version", "psql (PostgreSQL) 19", 120)?;
    unlock(vm)?;
    run_answering_until(vm, "clear; elevate psql --version", "psql (PostgreSQL) 19", 120)?;
    run_step(vm, SCRIPT, "ready", 4, 300)?;
    run_step(vm, SCRIPT, "admin", 2, 120)?;
    run_step(vm, SCRIPT, "person", 2, 120)?;
    run_step(vm, SCRIPT, "database", 6, 300)?;
    run_step(vm, SCRIPT, "parallel", 1, 300)?;
    // PHP's bundle, which is large: installed last.
    unlock(vm)?;
    install(vm, PHP_FILE, "net.php.php", 1800)?;
    run_answering_until(vm, "clear; php -v | head -n 1", "PHP 8.5.12 (cli)", 120)?;
    run_step(vm, SCRIPT, "php", 6, 300)?;
    unlock(vm)?;
    run_step(vm, SCRIPT, "stop", 5, 300)?;
    run_step(vm, SCRIPT, "reload", 1, 120)?;
    vm.press_power_button()?;
    vm.expect("spark: every service has stopped")?;
    vm.expect("Power: `spark` asked to power off the machine; syncing the filesystems")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}

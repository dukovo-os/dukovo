// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The Websites and the Databases of the web console, Dukovo Admin: the
//! websites' six actions and the databases' (ADR 0569), on a machine whose
//! nginx serves websites, whose PHP-FPM runs their pages and whose
//! PostgreSQL keeps their databases. Dukovo's root is on a disk with
//! 512 MiB of room after it, which the root grows over, in use, as the
//! test `postgresql-service` does, since the three bundles do not fit the
//! image's root; a second disk holds their `.dkv` files and the script that
//! makes the checks that would take too long to type. alice, the owner and
//! an administrator, installs nginx, PHP and PostgreSQL for everyone and
//! turns nginx and PostgreSQL on; lab-reader may use the console and is no
//! administrator.
//!
//! Every read asks the console's API as alice and compares what it answers
//! with what the command prints in her pane. What the pane's commands print
//! reaches the host through nc, to a port on the host's loopback that
//! QEMU's user network gives the guest at 10.0.2.2.
//!
//! A machine with no site. websites.list is what `website list --json`
//! prints: nginx on, PHP-FPM installed and off, no site; lab-reader reads
//! the same. websites.check is what `elevate website check --json` prints
//! and passes: nginx accepts its settings, which include the websites'
//! folder and answer 404 under /sites/ in the default server. lab-reader is
//! ADM-122 for the check.
//!
//! The plan. websites.plan of a static site named for the machine,
//! dukovo.local, with the machine's certificate is what the command prints
//! for `website add` with the options dry-run and json, with no check that
//! failed and the folder not there yet.
//!
//! A static site over HTTPS. websites.add of it is a task of seven steps that
//! ends with `Added dukovo.local: https://dukovo.local/`. With the machine's
//! chain as the only authority, nothing left out, its starter page answers
//! over HTTPS; port 80 answers 301 to HTTPS; the override of nginx's service
//! file names this-machine and port 443; the default server, which refuses
//! the handshake of a name no site has, is there; the folder is alice's. The
//! list has the site, with its certificate and not written by hand.
//!
//! A PHP site over HTTP. websites.add of php.lab.example, which answers to
//! www.php.lab.example too, turns PHP-FPM on: its index.php answers through
//! it and its code is not shown, a .php file in its uploads folder, which
//! the account php owns, answers 404, and so does a file whose name starts
//! with a dot, and /sites/ under the default server of port 80. The other
//! name answers, and the first site still does.
//!
//! A site file nginx does not read. A file written by hand into
//! /system/settings/websites makes websites.check fail with nginx's line,
//! naming the file and WEB-103 for its site. websites.add of another site is
//! a task that fails with WEB-103, "before any change", and changes nothing:
//! the files and the folders are as they were. The file taken away, the
//! check passes.
//!
//! A database for a site. databases.add of phpdb for the PHP site answers
//! with the password, once, in the first line of its output. A page that the
//! script writes into the PHP site's folder connects through PHP-FPM by PDO
//! over 127.0.0.1 as that role with that password, inserts a row and reads
//! it back, and with another password is refused. databases.list is what
//! the command `database list --json` prints but for the sizes and the
//! sessions: the server listens on localhost, with no TLS yet, and the
//! database and its role are the command's. A second add of the name is
//! DB-102.
//!
//! Other machines may connect. databases.allow from the guest's own network
//! with the machine's certificate is a task of seven steps that has PostgreSQL
//! restart: it listens on every address, with TLS, and has the hostssl line
//! for the database. A psql that checks the machine's name against its chain
//! connects over TLS from 10.0.2.15, one with another password does not, and
//! one without TLS does not. A second allow changes nothing. databases.deny
//! takes the line out and has PostgreSQL listen on this machine only again,
//! TLS staying on; a second deny says there is no line.
//!
//! A database removed. With a session open, databases.remove is DB-103 by
//! the route and by the command, naming the program that holds the session;
//! with force and role it ends the session and takes the database and its
//! role away.
//!
//! The sites taken away. websites.https with off on the first site, then
//! websites.remove of both: nginx's override has no certificate and no
//! socket for port 443, the default server is gone, the folders are kept and
//! nginx answers with its welcome page, port 443 shut.
//!
//! Who may. lab-reader is ADM-122 for every change and for every read of an
//! administrator's.

use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::super::console_client::{Browser, Console, Pane, elevated};
use super::super::*;
use super::admin_backups::{READER, READER_PASSWORD, Test, Who};

/// The disk of bundles' label.
const LABEL: &str = "BUNDLES";

/// Where Spark mounts it: the second virtio disk's name.
const VOLUME: &str = "/volumes/vdb";

/// The steps' script, by its path on the disk.
const SCRIPT: &str = "/volumes/vdb/websites.sh";

/// The bundles the test installs: the start of each file's name, and the
/// folder it makes in /applications.
const BUNDLES: [(&str, &str); 3] = [
    ("org.nginx.nginx-", "org.nginx.nginx"),
    ("net.php.php-", "net.php.php"),
    ("org.postgresql.postgresql-", "org.postgresql.postgresql"),
];

/// The first site, a static one named as the machine is, which the
/// machine's certificate names.
const MACHINE: &str = "dukovo.local";

/// The second site, a PHP one, which the script's requests lead to this
/// machine.
const PHP: &str = "php.lab.example";

/// The database made for the PHP site, which is its role's name too.
const DATABASE: &str = "phpdb";

/// The program name the open session gives, which the refusal of the
/// removal names.
const SLEEPER: &str = "websites-test";

/// Where the password of the database's role is kept in the guest, for the
/// script: the same path as the script's own.
const PASSWORD_FILE: &str = "/tmp/phpdb.pw";

/// The members of `database list --json` that move by themselves, the
/// sizes and the sessions, which are left out when it is compared.
const MOVING: [&str; 2] = ["bytes", "sessions"];

/// Dukovo's root on a copy of the test image with 512 MiB of room after it,
/// and the bundles, with the checks' script and page beside them.
const DISKS: &[Disk] = &[
    Disk {
        label: image::ROOT_LABEL,
        kib: 512 * 1024,
        ..btrfs_disk(Bus::Virtio, 512, ROOT_ROOM, root_checked)
    },
    Disk { fat: WEBSITES_FILES, ..bundle_files_disk(192 * 1024, LABEL) },
];

/// The test.
pub const CASE: Case = Case {
    name: "admin-websites",
    about: "The web console's Websites and Databases, on nginx, PHP-FPM and PostgreSQL installed for everyone on a root grown over its disk: websites.list is what website list --json prints, with no site, for alice and lab-reader, and websites.check passes for alice and is ADM-122 for lab-reader; websites.plan of a static site named dukovo.local with this-machine's certificate is what website add --dry-run --json prints, with no failed check; websites.add of it is a task of seven steps after which it answers over HTTPS with this-machine's chain alone, port 80 answers 301, the override of nginx's service file names this-machine and port 443, and the default server refuses the handshake of a name no site has; websites.add of a PHP site turns PHP-FPM on, its index.php answers through it without its code, and a .php file in its uploads folder, a dot file and /sites/ answer 404; a file nginx does not read in /system/settings/websites makes websites.check fail with nginx's line and WEB-103, and websites.add of another site fails with WEB-103 before any change and changes nothing; databases.add of phpdb for the PHP site answers with the password, once, a PDO page of the PHP site connects with it, inserts and reads and is refused with another password, databases.list is what database list --json prints but for the sizes and the sessions, and a second add is DB-102; databases.allow from 10.0.2.0/24 with this-machine's certificate has PostgreSQL listen on every address with TLS, a psql that checks the name connects over TLS and one without TLS or with another password does not, a second allow changes nothing, and databases.deny has it listen on localhost again, a second deny saying there is no line; databases.remove with a session open is DB-103 by the route and the command, and with force and role ends the session and takes the database and its role away; websites.https with off and websites.remove of both sites leave nginx's override with no certificate and no port 443, the default server gone, the folders kept and nginx's welcome page; lab-reader is ADM-122 for every change and every administrator's read (ADR 0569)",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    // As a normal boot: Spark starts the services, admind among them.
    cmdline: "lockdep",
    cpus: 2,
    memory: "2G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &["-netdev", "user,id=n0", "-device", "virtio-net-pci,netdev=n0"],
    script: admin_websites,
    disks: DISKS,
    second: None,
};

/// The steps, a script the test runs from the disk of bundles, one for
/// each argument: it reaches the sites and the database as a visitor, a page
/// and a client of another machine would, and counts the checks that held,
/// which it prints after them.
const STEPS_SCRIPT: &str = r#"# The steps of the admin-websites test that the shell takes, one for each
# argument: the sites and the database as a visitor, a page and a client
# of another machine reach them. Each step prints ok-NAME for each check
# that held and FAIL-NAME for each that did not, then how many held, and its
# commands' output goes to /tmp/steps.log. The commands of an administrator
# go through elevate, which the test has unlocked in the pane.
here=$(cd "$(dirname "$0")" && pwd)
machine=dukovo.local
php=php.lab.example
chain=/tmp/chain.pem
conf=/system/settings/websites
override=/system/settings/spark/services/settings.d/nginx.toml
sites=/shared/www/sites
password=/tmp/phpdb.pw
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

# Runs a check again each second for up to 30 seconds, until it holds.
eventually() {
	tries=0
	until "$@"; do
		tries=$((tries + 1))
		[ "$tries" -lt 30 ] || return 1
		sleep 1
	done
}

# The status code of a request, whose body is thrown away.
code() {
	curl -s -o /dev/null -w '%{http_code}' "$@"
}

# A request to the PHP site over HTTP: its name leads to this machine by
# --resolve.
over_http() {
	curl -s --resolve "$php:80:127.0.0.1" "$@"
}

# A request to the first site over HTTPS, which trusts the machine's own
# chain alone, and so checks the name it names.
over_https() {
	curl -s --cacert "$chain" --resolve "$machine:443:127.0.0.1" "$@"
}

# Whether a file is there, which only root may look for in these folders.
has_file() {
	elevate stat "$1"
}

lacks_file() {
	if elevate stat "$1"; then
		return 1
	fi
	return 0
}

waits_for_welcome() {
	tries=0
	until curl -s http://127.0.0.1/ | grep -q 'Welcome to nginx!'; do
		tries=$((tries + 1))
		[ "$tries" -lt 60 ] || return 1
		sleep 1
	done
}

starter_answers() {
	over_https "https://$machine/" | grep -q "$machine works. Put your site's files in $sites/$machine."
}

redirects_to_https() {
	test "$(code --resolve "$machine:80:127.0.0.1" "http://$machine/")" = 301 &&
		curl -sI --resolve "$machine:80:127.0.0.1" "http://$machine/" |
		grep -qi "^location: https://$machine/"
}

names_the_certificate() {
	elevate grep -q 'this-machine' "$override"
}

names_port_443() {
	elevate grep -q 'port = 443' "$override"
}

folder_is_alice_s() {
	test -f "$sites/$machine/index.html" && test "$(stat -c %U "$sites/$machine")" = alice
}

# The handshake of a name no site has is refused: -k leaves out the check of
# the certificate, so that only the server's refusal fails the request.
refuses_the_handshake() {
	if curl -sk --resolve "unknown.lab.example:443:127.0.0.1" "https://unknown.lab.example/"; then
		return 1
	fi
	return 0
}

https_answers_200() {
	test "$(over_https -o /dev/null -w '%{http_code}' "https://$machine/")" = 200
}

php_page() {
	over_http "http://$php/" | grep -q "$php works"
}

php_code_not_shown() {
	if over_http "http://$php/" | grep -q '<?php'; then
		return 1
	fi
	return 0
}

uploads_are_php_s() {
	test "$(elevate stat -c %U "$sites/$php/uploads")" = php
}

upload_is_404() {
	elevate sh -c "echo '<?php echo 1; ?>' > $sites/$php/uploads/x.php" &&
		test "$(code --resolve "$php:80:127.0.0.1" "http://$php/uploads/x.php")" = 404
}

sites_are_404() {
	test "$(code http://127.0.0.1/sites/$php/index.php)" = 404 &&
		test "$(code http://127.0.0.1/sites/)" = 404
}

dot_file_is_404() {
	elevate sh -c "echo secret > $sites/$php/.env" &&
		test "$(code --resolve "$php:80:127.0.0.1" "http://$php/.env")" = 404
}

other_name_answers() {
	curl -s --resolve "www.$php:80:127.0.0.1" "http://www.$php/" | grep -q "$php works"
}

# What the sites' files and the folders of the sites hold, as a sum, which a
# change that is refused or taken back leaves as it was.
sums() {
	elevate sh -c "ls $conf $sites; cat $conf/*.conf $override" 2>/dev/null | sha256sum
}

keeps_the_sums() {
	sums >/tmp/before.sum
}

sums_are_the_same() {
	test "$(sums)" = "$(cat /tmp/before.sum)"
}

# A file that nginx does not read, as someone would write one by hand.
broken_file() {
	elevate sh -c "printf 'server {\n    listen 80;\n    server_name broken.lab.example;\n    no_such_directive on;\n}\n' > $conf/broken.lab.example.conf"
}

broken_file_removed() {
	elevate rm "$conf/broken.lab.example.conf" && lacks_file "$conf/broken.lab.example.conf"
}

page_is_there() {
	cp "$here/db.php" "$sites/$php/db.php" && chmod 644 "$sites/$php/db.php"
}

page_reads_a_row() {
	over_http "http://$php/db.php?pw=$(cat $password)" | grep -q 'read:hatter'
}

page_is_refused() {
	over_http "http://$php/db.php?pw=not-the-password" | grep -q 'refused:.*password authentication failed'
}

# A client of the guest's network that trusts the machine's chain alone and
# checks the machine's name, which hostaddr leads to the guest's address on
# the network the allow names.
tls_psql() {
	PGPASSWORD="$1" psql "host=$machine hostaddr=10.0.2.15 dbname=phpdb user=phpdb sslmode=verify-full sslrootcert=$chain" \
		-X -A -t -c 'SELECT ssl FROM pg_stat_ssl WHERE pid = pg_backend_pid()'
}

tls_connects() {
	test "$(tls_psql "$(cat $password)")" = t
}

tls_does_not_connect() {
	if tls_connects; then
		return 1
	fi
	return 0
}

wrong_password_refused() {
	if tls_psql not-the-password; then
		return 1
	fi
	return 0
}

without_tls_refused() {
	if PGPASSWORD="$(cat $password)" psql "host=$machine hostaddr=10.0.2.15 dbname=phpdb user=phpdb sslmode=disable" -X -c 'SELECT 1'; then
		return 1
	fi
	return 0
}

# A session that sleeps, opened as the database's role over 127.0.0.1 by a
# program that gives its name, which the refusal of the removal names.
session_open() {
	PGPASSWORD="$(cat $password)" PGAPPNAME=websites-test psql -h 127.0.0.1 -U phpdb -d phpdb -X -c 'SELECT pg_sleep(600)' >/tmp/sleeper.out 2>&1 &
	tries=0
	until test "$(elevate psql -U postgres -X -A -t -c "SELECT count(*) FROM pg_stat_activity WHERE application_name = 'websites-test' AND state = 'active'")" = 1; do
		tries=$((tries + 1))
		[ "$tries" -lt 30 ] || return 1
		sleep 1
	done
}

session_ended() {
	eventually grep -q -e 'terminating connection' -e 'FATAL' -e 'server closed' /tmp/sleeper.out
}

without_443() {
	if elevate grep -q 'port = 443' "$override"; then
		return 1
	fi
	return 0
}

without_the_certificate() {
	if elevate grep -q 'this-machine' "$override"; then
		return 1
	fi
	return 0
}

folders_kept() {
	test -d "$sites/$machine" && test -d "$sites/$php"
}

no_site_files() {
	test -z "$(elevate ls "$conf" 2>/dev/null)"
}

port_443_closed() {
	if curl -s --cacert "$chain" --resolve "$machine:443:127.0.0.1" "https://$machine/"; then
		return 1
	fi
	return 0
}

cert show this-machine --pem >"$chain"
case "$1" in
welcome)
	check welcome waits_for_welcome
	;;
static)
	check starter eventually starter_answers
	check redirect redirects_to_https
	check certificate names_the_certificate
	check port_443 names_port_443
	check folder folder_is_alice_s
	check default_server has_file "$conf/_default.conf"
	check handshake refuses_the_handshake
	;;
php)
	check page eventually php_page
	check code_not_shown php_code_not_shown
	check uploads uploads_are_php_s
	check upload_404 upload_is_404
	check sites_404 sites_are_404
	check dot_404 dot_file_is_404
	check other_name other_name_answers
	check first_site starter_answers
	;;
broken)
	check written broken_file
	check sums keeps_the_sums
	;;
unchanged)
	check sums sums_are_the_same
	check no_folder lacks_file "$sites/other.lab.example"
	;;
mended)
	check removed broken_file_removed
	;;
page)
	check page page_is_there
	;;
pdo)
	check reads page_reads_a_row
	check refused page_is_refused
	;;
tls)
	check connects tls_connects
	check wrong_password wrong_password_refused
	check without_tls without_tls_refused
	;;
denied)
	check refused tls_does_not_connect
	;;
session)
	check open session_open
	;;
ended)
	check ended session_ended
	;;
sites_gone)
	check welcome waits_for_welcome
	check no_443 without_443
	check no_certificate without_the_certificate
	check folders folders_kept
	check no_default lacks_file "$conf/_default.conf"
	check no_sites no_site_files
	check closed port_443_closed
	;;
esac
echo "$1 $held of $checked"
"#;

/// The page of the PHP site that reaches the database as a page would:
/// over 127.0.0.1 with the password it is given, inserting a row and
/// reading the last one back, or saying why the server refused it.
const DATABASE_PAGE: &str = r#"<?php
// The page of the admin-websites test that PHP-FPM serves: it connects to
// the database phpdb as its role, inserts a row and reads the last one back,
// or says why the server refused the connection.
$password = $_GET["pw"] ?? "";
try {
    $pdo = new PDO("pgsql:host=127.0.0.1;port=5432;dbname=phpdb", "phpdb", $password);
    $pdo->setAttribute(PDO::ATTR_ERRMODE, PDO::ERRMODE_EXCEPTION);
    $pdo->exec("CREATE TABLE IF NOT EXISTS guests (id serial PRIMARY KEY, name text NOT NULL)");
    $pdo->prepare("INSERT INTO guests (name) VALUES (?)")->execute(["hatter"]);
    echo "read:" . $pdo->query("SELECT name FROM guests ORDER BY id DESC LIMIT 1")->fetchColumn() . "\n";
} catch (PDOException $e) {
    echo "refused:" . str_replace("\n", " ", $e->getMessage()) . "\n";
}
"#;

/// Makes the disk of bundles: the bundles of the catalog, with the steps'
/// script, `websites.sh`, and the page that reaches the database, `db.php`,
/// beside them.
pub fn make_disk(ctx: &Ctx, disk: &Disk, path: &Path) -> Result<(), String> {
    make_bundle_files_with(
        ctx,
        disk,
        path,
        &[("websites.sh", STEPS_SCRIPT), ("db.php", DATABASE_PAGE)],
    )
}

/// Installs the bundle whose file starts with `start` for everyone,
/// through elevate, which is unlocked, and waits up to `seconds` for it to
/// be in /applications as `folder`.
fn install(vm: &mut Vm, start: &str, folder: &str, seconds: u64) -> Result<(), String> {
    answer_prompt(
        vm,
        "$",
        &format!(
            "clear; cd {VOLUME}; elevate bundle install --for-everyone --yes {start}*.dkv | grep \
             -e 'as the account' -e installed; cd"
        ),
    )?;
    // Counted, so that the command's own line on the screen is not taken
    // for its answer.
    let count = format!("clear; echo \"installed $(ls -d /applications/{folder}* | wc -l)\"");
    answer_prompt_within(vm, "$", &count, seconds)?;
    vm.expect_screen(&["installed 1"])
}

/// A step of the script: elevate is unlocked in the pane, then the script
/// runs the step and every one of its `checks` holds.
fn step(vm: &mut Vm, name: &str, checks: usize, seconds: u64) -> Result<(), String> {
    unlock(vm)?;
    run_step(vm, SCRIPT, name, checks, seconds)
}

/// What the route answers and what the command prints in alice's pane are
/// the same JSON, which is returned. The members called `skip`, which move
/// by themselves, are left out of both, wherever they are.
fn same_as_command(
    vm: &mut Vm,
    test: &mut Test,
    route: &str,
    command: &str,
    skip: &[&str],
) -> Result<Value, String> {
    unlock(vm)?;
    test.unlocked()?;
    let mut said = match test.read(Who::Alice, route) {
        Ok(said) => said,
        // What the command says, its errors with it, tells why.
        Err(why) => {
            let text = test.pane.output(vm, &format!("{command} 2>&1"))?;
            return Err(format!("{why}; `{command}` prints {:?}", text.trim()));
        }
    };
    let mut printed = test.printed(vm, command)?;
    without(&mut said, skip);
    without(&mut printed, skip);
    if said != printed {
        return Err(format!("{route} answers {said}, and `{command}` printed {printed}"));
    }
    Ok(said)
}

/// `value` without the members called `names`, at any depth.
fn without(value: &mut Value, names: &[&str]) {
    match value {
        Value::Object(members) => {
            members.retain(|name, _| !names.contains(&name.as_str()));
            members.values_mut().for_each(|member| without(member, names));
        }
        Value::Array(items) => items.iter_mut().for_each(|item| without(item, names)),
        _ => {}
    }
}

/// An error naming `what` unless `value` has each member of `wanted`, equal.
fn has_members(what: &str, value: &Value, wanted: &Value) -> Result<(), String> {
    let members = wanted.as_object().ok_or("the members wanted are no object")?;
    match members.iter().find(|(name, member)| value.get(name.as_str()) != Some(*member)) {
        Some((name, member)) => Err(format!("{what} has no {name}: {member} in {value}")),
        None => Ok(()),
    }
}

/// The row of `list`, in the array `array`, whose name is `name`.
fn row<'a>(list: &'a Value, array: &str, name: &str) -> Option<&'a Value> {
    list[array].as_array()?.iter().find(|row| row["name"] == name)
}

/// The lines of a task's log.
fn log_of(task: &Value) -> Vec<&str> {
    task["log"].as_array().into_iter().flatten().filter_map(Value::as_str).collect()
}

/// An error naming `what` unless the log of `task` has the line `line`.
fn logs(what: &str, task: &Value, line: &str) -> Result<(), String> {
    if log_of(task).contains(&line) {
        Ok(())
    } else {
        Err(format!("the log of {what} has no line {line:?}: {:?}", log_of(task)))
    }
}

/// A task as alice that ends failed: the task once it has ended, within
/// `seconds`.
fn failed_task(test: &mut Test, path: &str, body: &Value, seconds: u64) -> Result<Value, String> {
    test.unlocked()?;
    let reply = test.post(Who::Alice, path, body)?.must(path, 202)?;
    let id = reply.json()?["task"].as_i64().ok_or("no task number")?;
    let deadline = Instant::now() + Duration::from_secs(seconds);
    loop {
        let task = test.read(Who::Alice, &format!("/api/1/tasks/{id}"))?;
        if !matches!(task["state"].as_str(), Some("waiting" | "running")) {
            if task["state"] != "failed" {
                return Err(format!("the task of {path} ended as {task}, not failed"));
            }
            return Ok(task);
        }
        if Instant::now() >= deadline {
            return Err(format!("the task of {path} is {task} after {seconds} s"));
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

fn admin_websites(vm: &mut Vm, case: &Case) -> Result<(), String> {
    owner_signs_in(vm, case)?;
    vm.expect_anywhere("bundled: 0 bundles installed; answering at /system/live/bundled/socket")?;
    vm.expect_anywhere("netd: net0 has 10.0.2.15/24")?;
    vm.expect_anywhere("admind: answers at ")?;
    vm.expect_anywhere(&format!("spark: mounted the FAT filesystem of vdb at {VOLUME}"))?;

    // alice's password once for this pane, then the reader, who is in group
    // console and no administrator.
    elevated(vm)?;
    answer_prompt(
        vm,
        "$",
        &format!(
            "clear; elevate sh -c 'echo {READER_PASSWORD} | user add {READER}; user set --yes \
             --console {READER}'"
        ),
    )?;
    vm.expect(&format!("authd: changed the account {READER}"))?;
    vm.expect_screen(&[&format!("Made {READER}")])?;

    the_machine_set_up(vm)?;

    // The sessions begin after the long installs, which they would not
    // outlast.
    let mut test = Test {
        console: Console::open(vm)?,
        pane: Pane::open()?,
        alice: Browser::default(),
        reader: Browser::default(),
    };
    test.sign_in()?;

    the_machine_with_no_site(vm, &mut test)?;
    the_plan(vm, &mut test)?;
    a_static_site_over_https(vm, &mut test)?;
    a_php_site(vm, &mut test)?;
    a_site_file_nginx_does_not_read(vm, &mut test)?;
    a_database_for_a_site(vm, &mut test)?;
    other_machines_may_connect(vm, &mut test)?;
    a_database_removed(vm, &mut test)?;
    the_sites_taken_away(vm, &mut test)?;
    what_needs_an_administrator(&mut test)?;

    vm.press_power_button()?;
    vm.expect("spark: every service has stopped")?;
    vm.expect("Power: `spark` asked to power off the machine; syncing the filesystems")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}

/// The machine set up: the root grown over the disk's room, in use, the
/// three bundles installed for everyone, nginx and PostgreSQL on (PHP-FPM
/// stays off, which the first PHP site turns on), and the first runs of
/// psql, which ask in the pane with a warning, answered.
fn the_machine_set_up(vm: &mut Vm) -> Result<(), String> {
    unlock(vm)?;
    answer_prompt(vm, "$", "clear; elevate rdisk --grow /devices/vda")?;
    vm.expect_screen(&["/devices/vda3 grows from 512 MiB to 1 GiB."])?;
    answer_prompt(vm, "$", "clear; elevate device resize max /system/settings")?;
    vm.expect_screen(&["Resized device id 1 of '/system/settings' to "])?;
    for (start, folder) in BUNDLES {
        unlock(vm)?;
        install(vm, start, folder, 1800)?;
    }
    unlock(vm)?;
    answer_prompt(vm, "$", "clear; elevate service nginx enable; echo \"enable $?\"")?;
    vm.expect_screen(&["enable 0"])?;
    vm.expect_anywhere("start worker processes")?;
    answer_prompt(vm, "$", "clear; elevate service postgresql enable; echo \"enable $?\"")?;
    vm.expect_screen(&["enable 0"])?;
    vm.expect_anywhere("database system is ready to accept connections")?;
    // psql runs with the access of whoever runs it, and elevate with root's:
    // each first run asks, with a warning, in her pane (ADR 0493).
    run_answering_until(vm, "clear; psql --version", "psql (PostgreSQL) 19", 120)?;
    unlock(vm)?;
    run_answering_until(vm, "clear; elevate psql --version", "psql (PostgreSQL) 19", 120)?;
    step(vm, "welcome", 1, 120)
}

/// A machine with no site: nginx on and PHP-FPM installed and off, the same
/// as the command prints, for alice and for the reader; nginx's check passes
/// as the route and the command say, and the reader is refused it.
fn the_machine_with_no_site(vm: &mut Vm, test: &mut Test) -> Result<(), String> {
    let list = same_as_command(vm, test, "/api/1/websites", "website list --json", &[])?;
    if list["websites"] != json!([]) {
        return Err(format!("the list has a site before any is added: {list}"));
    }
    let on = json!({"installed": true, "enabled": true, "running": true});
    has_members("nginx", &list["nginx"], &on)?;
    has_members("PHP-FPM", &list["php_fpm"], &json!({"installed": true, "enabled": false}))?;
    let reader = test.read(Who::Reader, "/api/1/websites")?;
    if reader != list {
        return Err(format!("the reader reads {reader}, and alice {list}"));
    }
    let check =
        same_as_command(vm, test, "/api/1/websites/check", "elevate website check --json", &[])?;
    has_members(
        "nginx's settings",
        &check["nginx_conf"],
        &json!({"includes": true, "hides_sites": true}),
    )?;
    if check["check"]["passed"] != json!(true) || check["websites"] != json!([]) {
        return Err(format!("the check of a machine with no site says {check}"));
    }
    test.get(Who::Reader, "/api/1/websites/check")?.refused("the reader's check", 403, "ADM-122")?;
    Ok(())
}

/// The plan of the machine's site over HTTPS is what the command prints,
/// with no check that failed and no folder yet.
fn the_plan(vm: &mut Vm, test: &mut Test) -> Result<(), String> {
    let route =
        format!("/api/1/websites/plan/{MACHINE}?kind=static&owner=alice&certificate=this-machine");
    let command = format!(
        "elevate website add {MACHINE} --kind static --owner alice --certificate this-machine \
         --dry-run --json"
    );
    let plan = same_as_command(vm, test, &route, &command, &[])?;
    let checks = plan["checks"].as_array().ok_or_else(|| format!("the plan has no checks: {plan}"))?;
    if checks.is_empty() || checks.iter().any(|check| check["result"] == "failed") {
        return Err(format!("a check of the plan failed: {checks:?}"));
    }
    if plan["folder"]["exists"] != json!(false) || plan["nginx_check"] == json!("failed") {
        return Err(format!("the plan says what a first site does not: {plan}"));
    }
    Ok(())
}

/// The machine's site over HTTPS, added as a task, answers as the script
/// checks, and the list has it with its certificate.
fn a_static_site_over_https(vm: &mut Vm, test: &mut Test) -> Result<(), String> {
    let body =
        json!({"name": MACHINE, "kind": "static", "owner": "alice", "certificate": "this-machine"});
    let (task, most) = test.task("/api/1/websites/add", &body, 900)?;
    if task["action"] != "websites.add" || task["steps"] != json!(7) || most < 1 {
        return Err(format!("the task of the add is {task}, its step seen at {most}"));
    }
    logs("the add", &task, &format!("Added {MACHINE}: https://{MACHINE}/"))?;
    step(vm, "static", 7, 300)?;
    let list = same_as_command(vm, test, "/api/1/websites", "website list --json", &[])?;
    let site = row(&list, "websites", MACHINE)
        .ok_or_else(|| format!("the list has no {MACHINE} after its add: {list}"))?;
    has_members(
        "the first site",
        site,
        &json!({"kind": "static", "https": true, "certificate": "this-machine",
                "redirects_http": true, "by_hand": false, "owner": "alice"}),
    )
}

/// A PHP site over HTTP with another name, which turns PHP-FPM on.
fn a_php_site(vm: &mut Vm, test: &mut Test) -> Result<(), String> {
    let body = json!({"name": PHP, "names": format!("www.{PHP}"), "kind": "php", "owner": "alice"});
    let (task, _) = test.task("/api/1/websites/add", &body, 900)?;
    logs("the add of the PHP site", &task, &format!("Added {PHP}: http://{PHP}/"))?;
    step(vm, "php", 8, 300)?;
    let list = same_as_command(vm, test, "/api/1/websites", "website list --json", &[])?;
    let site = row(&list, "websites", PHP)
        .ok_or_else(|| format!("the list has no {PHP} after its add: {list}"))?;
    has_members("the PHP site", site, &json!({"kind": "php", "https": false, "certificate": null}))?;
    if site["names"] != json!([PHP, format!("www.{PHP}")]) {
        return Err(format!("the PHP site answers to {}", site["names"]));
    }
    let on = json!({"installed": true, "enabled": true, "running": true});
    has_members("PHP-FPM", &list["php_fpm"], &on)
}

/// A file that nginx does not read, written by hand, which the check names
/// with nginx's line and which an add refuses to go on beside, before it
/// changes anything; with the file taken away, the check passes.
fn a_site_file_nginx_does_not_read(vm: &mut Vm, test: &mut Test) -> Result<(), String> {
    step(vm, "broken", 2, 120)?;
    let check =
        same_as_command(vm, test, "/api/1/websites/check", "elevate website check --json", &[])?;
    let line = check["check"]["line"].as_str().unwrap_or_default();
    if check["check"]["passed"] != json!(false) || !line.contains("broken.lab.example.conf") {
        return Err(format!("the check of a site file nginx does not read says {check}"));
    }
    let site = row(&check, "websites", "broken.lab.example")
        .ok_or_else(|| format!("the check names no site broken.lab.example: {check}"))?;
    if site["problems"][0]["id"] != json!("WEB-103") {
        return Err(format!("the problem of the broken site is {site}"));
    }
    let before = same_as_command(vm, test, "/api/1/websites", "website list --json", &[])?;
    let other = json!({"name": "other.lab.example", "kind": "static", "owner": "alice"});
    let task = failed_task(test, "/api/1/websites/add", &other, 300)?;
    let log = log_of(&task);
    let last = log.last().copied().unwrap_or_default();
    if task["status"] != json!(1) || !last.ends_with("(WEB-103)") || !last.contains("before any change")
    {
        return Err(format!("the add beside a broken file ended as {task}, its log {log:?}"));
    }
    step(vm, "unchanged", 2, 120)?;
    let after = same_as_command(vm, test, "/api/1/websites", "website list --json", &[])?;
    if after != before {
        return Err(format!("the list was {before}, and after the refused add {after}"));
    }
    step(vm, "mended", 1, 120)?;
    let check =
        same_as_command(vm, test, "/api/1/websites/check", "elevate website check --json", &[])?;
    if check["check"]["passed"] != json!(true) {
        return Err(format!("the check with the broken file gone says {check}"));
    }
    Ok(())
}

/// A database for the PHP site, whose password the answer has, once: the
/// site's page reaches it with that password and with no other, and the
/// list is what the command prints. A second add is DB-102.
fn a_database_for_a_site(vm: &mut Vm, test: &mut Test) -> Result<(), String> {
    test.unlocked()?;
    let body = json!({"name": DATABASE, "site": PHP, "locale": "en_US.UTF-8"});
    let answer =
        test.post(Who::Alice, "/api/1/databases/add", &body)?.must("the database's add", 200)?.json()?;
    let line = answer["output"][0].as_str().ok_or_else(|| format!("the add's answer is {answer}"))?;
    let made: Value = serde_json::from_str(line)
        .map_err(|err| format!("the first line of the add's answer, {line:?}, is no JSON: {err}"))?;
    has_members(
        "the answer of the add",
        &made,
        &json!({"format": 1, "database": DATABASE, "role": DATABASE, "host": "127.0.0.1", "port": 5432}),
    )?;
    let password = made["password"].as_str().unwrap_or_default().to_string();
    let plain = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-';
    if password.chars().count() != 32 || !password.chars().all(plain) {
        return Err(format!("the password in the add's answer is not 32 of A-Za-z0-9_-: {made}"));
    }
    // The script reads it from a file: characters of that set are safe
    // between quotes.
    test.ends_with(vm, &format!("echo '{password}' > {PASSWORD_FILE}"), 0)?;
    step(vm, "page", 1, 120)?;
    step(vm, "pdo", 2, 120)?;

    let list = same_as_command(vm, test, "/api/1/databases", "elevate database list --json", &MOVING)?;
    has_members(
        "the server",
        &list["server"],
        &json!({"installed": true, "enabled": true, "running": true, "listen": "localhost",
                "ssl": false, "certificate": null}),
    )?;
    let database = row(&list, "databases", DATABASE)
        .ok_or_else(|| format!("the list has no database {DATABASE}: {list}"))?;
    has_members(
        "the database",
        database,
        &json!({"owner": DATABASE, "encoding": "UTF8", "provider": "libc", "locale": "en_US.UTF-8",
                "site": PHP, "made_by_database": true, "collation_mismatch": false}),
    )?;
    let role =
        row(&list, "roles", DATABASE).ok_or_else(|| format!("the list has no role {DATABASE}: {list}"))?;
    has_members("the role", role, &json!({"login": true, "password": true, "made_by_database": true}))?;
    if row(&list, "roles", "postgres").is_none() {
        return Err(format!("the list has no role postgres: {list}"));
    }
    let access = list["access"].as_array().ok_or_else(|| format!("the list has no access: {list}"))?;
    if access.is_empty() || access.iter().any(|line| line["by_hand"] != json!(true)) {
        return Err(format!("the access of a server with no line of the command's is {access:?}"));
    }
    if list["problems"] != json!([]) {
        return Err(format!("the list of the databases has problems: {}", list["problems"]));
    }

    // A second add of the name is the command's refusal, by the route and by
    // the command, which ends with status 1.
    test.post(Who::Alice, "/api/1/databases/add", &body)?.refused("a second add", 422, "DB-102")?;
    let said = test.pane.output(
        vm,
        &format!("elevate database add {DATABASE} --site {PHP} --json 2>&1 | tail -n 1"),
    )?;
    let wanted = format!(
        "database: there is a database called {DATABASE} already: choose another name, or remove \
         it first (DB-102)"
    );
    if said.trim() != wanted {
        return Err(format!("the last line of the second add is {said:?}, not {wanted:?}"));
    }
    test.ends_with(vm, &format!("elevate database add {DATABASE} --site {PHP} --json 2>/dev/null"), 1)
}

/// Other machines may connect, from the guest's own network with the
/// machine's certificate, and then may not.
fn other_machines_may_connect(vm: &mut Vm, test: &mut Test) -> Result<(), String> {
    let path = format!("/api/1/databases/{DATABASE}/allow");
    let body = json!({"from": "10.0.2.0/24", "certificate": "this-machine"});
    let (task, _) = test.task(&path, &body, 1200)?;
    if task["steps"] != json!(7) {
        return Err(format!("the task of the allow has {} steps, not 7", task["steps"]));
    }
    let allowed = format!("10.0.2.0/24 may connect to {DATABASE} over TLS, as {DATABASE}, with the password");
    logs("the allow", &task, &allowed)?;
    logs("the allow", &task, "PostgreSQL restarted, and its open sessions ended")?;
    let list = same_as_command(vm, test, "/api/1/databases", "elevate database list --json", &MOVING)?;
    has_members(
        "the server after the allow",
        &list["server"],
        &json!({"listen": "*", "ssl": true, "certificate": "this-machine"}),
    )?;
    let line = json!({"type": "hostssl", "databases": [DATABASE], "users": [DATABASE],
                      "address": "10.0.2.0/24", "method": "scram-sha-256", "by_hand": false});
    let has_line =
        list["access"].as_array().into_iter().flatten().any(|has| has_members("", has, &line).is_ok());
    if !has_line {
        return Err(format!("the access has no line {line}: {}", list["access"]));
    }
    step(vm, "tls", 3, 300)?;

    // The same allow again changes nothing.
    test.task(&path, &body, 1200)?;
    let again = same_as_command(vm, test, "/api/1/databases", "elevate database list --json", &MOVING)?;
    if again != list {
        return Err(format!("a second allow changed the list from {list} to {again}"));
    }

    let path = format!("/api/1/databases/{DATABASE}/deny");
    let (task, _) = test.task(&path, &json!({}), 1200)?;
    logs("the deny", &task, &format!("10.0.2.0/24 may no longer connect to {DATABASE}"))?;
    logs("the deny", &task, "PostgreSQL listens on this machine only again, and restarted")?;
    let list = same_as_command(vm, test, "/api/1/databases", "elevate database list --json", &MOVING)?;
    has_members("the server after the deny", &list["server"], &json!({"listen": "localhost", "ssl": true}))?;
    if list["access"].as_array().into_iter().flatten().any(|has| has["by_hand"] == json!(false)) {
        return Err(format!("the access after the deny has a line of the command's: {}", list["access"]));
    }
    step(vm, "denied", 1, 300)?;
    step(vm, "pdo", 2, 120)?;
    // With no line to take out, it says so and is done.
    let (task, _) = test.task(&path, &json!({}), 600)?;
    logs("the second deny", &task, &format!("{DATABASE} has no line in pg_hba.conf for other machines"))
}

/// With a session open, the removal is refused, by the route and by the
/// command, naming the program that holds it; with force and role it ends
/// the session and takes the database and its role away.
fn a_database_removed(vm: &mut Vm, test: &mut Test) -> Result<(), String> {
    step(vm, "session", 1, 300)?;
    let path = format!("/api/1/databases/{DATABASE}/remove");
    test.unlocked()?;
    let words = format!("{DATABASE} has 1 open session, from {SLEEPER}");
    let refused = test
        .post(Who::Alice, &path, &json!({}))?
        .refused("the removal with a session open", 422, "DB-103")?;
    if !refused.detail().contains(&words) {
        return Err(format!("the removal is refused with {:?}, not with {words:?}", refused.detail()));
    }
    let said = test.pane.output(vm, &format!("elevate database remove {DATABASE} 2>&1 | tail -n 1"))?;
    let wanted = format!("database: {words}: stop those programs, or use --force to end them (DB-103)");
    if said.trim() != wanted {
        return Err(format!("the last line of the removal is {said:?}, not {wanted:?}"));
    }
    test.ends_with(vm, &format!("elevate database remove {DATABASE} 2>/dev/null"), 1)?;

    let answer = test
        .post(Who::Alice, &path, &json!({"force": true, "role": true}))?
        .must("the removal with force", 200)?
        .json()?;
    let output = answer["output"].as_array().cloned().unwrap_or_default();
    for line in [format!("removed the database {DATABASE}"), format!("removed the role {DATABASE}")] {
        if !output.contains(&json!(line)) {
            return Err(format!("the removal with force answered {answer}, with no {line:?}"));
        }
    }
    let list = same_as_command(vm, test, "/api/1/databases", "elevate database list --json", &MOVING)?;
    if row(&list, "databases", DATABASE).is_some() || row(&list, "roles", DATABASE).is_some() {
        return Err(format!("the database or its role is there after the removal: {list}"));
    }
    step(vm, "ended", 1, 120)
}

/// The first site over HTTP only again, then both sites removed with their
/// folders kept.
fn the_sites_taken_away(vm: &mut Vm, test: &mut Test) -> Result<(), String> {
    let path = format!("/api/1/websites/{MACHINE}/https");
    let (task, _) = test.task(&path, &json!({"off": true}), 900)?;
    if task["action"] != "websites.https" {
        return Err(format!("the task of https off is {task}"));
    }
    let list = same_as_command(vm, test, "/api/1/websites", "website list --json", &[])?;
    let site = row(&list, "websites", MACHINE)
        .ok_or_else(|| format!("the list has no {MACHINE}: {list}"))?;
    has_members("the first site over HTTP only", site, &json!({"https": false, "certificate": null}))?;
    for name in [MACHINE, PHP] {
        let (task, _) = test.task(&format!("/api/1/websites/{name}/remove"), &json!({}), 900)?;
        if task["action"] != "websites.remove" {
            return Err(format!("the task of the removal of {name} is {task}"));
        }
    }
    let list = same_as_command(vm, test, "/api/1/websites", "website list --json", &[])?;
    if list["websites"] != json!([]) {
        return Err(format!("the list after both removals is {list}"));
    }
    step(vm, "sites_gone", 7, 300)
}

/// The reader is ADM-122 for every change, and for every read that an
/// administrator runs, of these pages; the list of the websites, which a
/// person reads, is open to both.
fn what_needs_an_administrator(test: &mut Test) -> Result<(), String> {
    let site = json!({"name": MACHINE, "kind": "static", "owner": "alice"});
    let database = json!({"name": DATABASE, "site": PHP, "locale": "en_US.UTF-8"});
    let allow = json!({"from": "10.0.2.0/24", "certificate": "this-machine"});
    let asks: Vec<(String, Value)> = vec![
        ("/api/1/websites/add".to_string(), site),
        (format!("/api/1/websites/{MACHINE}/https"), json!({"off": true})),
        (format!("/api/1/websites/{MACHINE}/remove"), json!({})),
        ("/api/1/databases/add".to_string(), database),
        (format!("/api/1/databases/{DATABASE}/allow"), allow),
        (format!("/api/1/databases/{DATABASE}/deny"), json!({})),
        (format!("/api/1/databases/{DATABASE}/remove"), json!({})),
    ];
    for (path, body) in &asks {
        test.post(Who::Reader, path, body)?.refused(path, 403, "ADM-122")?;
    }
    for path in [
        "/api/1/websites/check",
        "/api/1/websites/plan/dukovo.local?kind=static&owner=alice",
        "/api/1/databases",
    ] {
        test.get(Who::Reader, path)?.refused(path, 403, "ADM-122")?;
    }
    test.read(Who::Reader, "/api/1/websites")?;
    Ok(())
}

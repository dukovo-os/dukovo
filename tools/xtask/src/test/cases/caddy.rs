// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Caddy, the catalog's web server built with Go, serves (ADRs 0600 and
//! 0601). Dukovo's root is on a disk, and a second disk holds the `.dkv`
//! file `ob ports` packed for Caddy, with the script that makes the checks.
//! Through elevate, Alice installs it for everyone: authd makes the account
//! caddy, and the service waits until she enables it. Enabled, it takes
//! the port Spark holds for it and serves the page she puts in /shared/www,
//! a 404 for a page that is not there and for the folders of website(8)'s
//! sites; its settings are the account's, as is its admin API's socket.
//! `caddy version` asks, in her pane, whether Caddy may run with her
//! access, and then with root's through elevate; `caddy validate` passes
//! the settings it copied. Settings that do not read fail validate, and a
//! reload leaves Caddy serving as it was, its log saying so; mended, it
//! serves after a restart. With the machine's own certificate from certd(8)
//! named in its service file, port 443 added to its sockets and the HTTPS
//! site of its settings turned on, a restart has it serve HTTPS to a
//! client that trusts that certificate and no other, and offer HTTP/2 to
//! one that asks for it; three reloads while 300 requests come over HTTP
//! and HTTPS lose none of them.
//! certd makes the certificate anew, and Caddy serves the new one, read
//! again on Spark's SIGHUP, without a restart. Three restarts and an
//! install of its file over itself, which is an update, while connectloop
//! (userspace/posix/connectloop.c) makes 300 requests one after another,
//! refuse none and fail none: Spark holds its sockets (ADR 0477). Stopped,
//! it ends gracefully on TERM and its port refuses. The power button ends
//! it.

use super::super::*;

/// The disk of bundles' label.
const LABEL: &str = "BUNDLES";

/// Where Spark mounts it: the second virtio disk's name.
const VOLUME: &str = "/volumes/vdb";

/// Caddy's bundle file.
const FILE: &str = "com.caddyserver.caddy-2.11.9.dkv";

/// The start of the name of the bundle file: the only bundle on the disk.
pub(in crate::test) const BUNDLE_PREFIX: &str = "com.caddyserver.caddy-";

/// The steps' script, by its path on the disk.
const SCRIPT: &str = "/volumes/vdb/caddy.sh";

/// Dukovo's root on a copy of the test image, and Caddy's bundle, with the
/// checks' script beside it.
const DISKS: &[Disk] = &[
    Disk { label: image::ROOT_LABEL, ..btrfs_disk(Bus::Virtio, 512, ROOT, root_checked) },
    Disk { fat: CADDY_FILES, ..bundle_files_disk(64 * 1024, LABEL) },
];

/// The test.
pub const CASE: Case = Case {
    name: "caddy",
    about: "Caddy, the catalog's web server built with Go, serves (ADRs 0600, 0601): installed for everyone from its .dkv file and enabled, it runs as its own account on the port Spark holds and serves a page in /shared/www, a 404 for a missing page and for website(8)'s folders; caddy version asks in her pane, and caddy validate passes its settings; settings that do not read fail validate and a reload leaves it serving, with the reason in its log; with the machine's certificate from certd named in its service file and port 443 added it serves HTTPS and offers HTTP/2, and reloads while 300 requests come over HTTP and HTTPS lose none; a renewal is served without a restart (ADR 0512); restarts and an update while 300 requests come refuse none; stopped, it ends gracefully and its port refuses",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    // As a normal boot: Spark starts the services, bundled among them.
    cmdline: "lockdep",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &[],
    script: caddy,
    disks: DISKS,
    second: None,
};

/// The steps, a script the test runs from the disk of bundles, one for
/// each argument: it uses the service as an administrator and as a client,
/// and counts the checks that held, which it prints after them.
const STEPS_SCRIPT: &str = r#"# The steps of the caddy test that the shell takes, one for each
# argument. Each step prints ok-NAME for each check that held and
# FAIL-NAME for each that did not, then how many held, and its commands'
# output goes to /tmp/steps.log. The commands of an administrator go
# through elevate, which the test has unlocked in the pane.
here=$(cd "$(dirname "$0")" && pwd)
conf=/shared/services/com.caddyserver.caddy/caddy/conf
service_file=/system/settings/spark/services/caddy.toml
page='<p>Hello from Dukovo</p>'
held=0
checked=0

check() {
	name=$1
	shift
	checked=$((checked + 1))
	if "$@" >>/tmp/steps.log 2>&1; then
		held=$((held + 1))
		echo "ok-$name"
	else
		echo "FAIL-$name"
	fi
}

# Whether the service's log has a line that matches $1, which logd takes
# from the kernel's a moment after Caddy wrote it.
logged() {
	tries=0
	until elevate log caddy | grep -q "$1"; do
		tries=$((tries + 1))
		[ "$tries" -lt 10 ] || return 1
		sleep 1
	done
}

# The status of the answer to a request for $1.
status() {
	curl -s -o /dev/null -w '%{http_code}' "$@"
}

# Whether the front page is the one in /shared/www, within 30 seconds:
# the service may still be starting.
serves() {
	tries=0
	until test "$(curl -s http://localhost/)" = "$page"; do
		tries=$((tries + 1))
		[ "$tries" -lt 30 ] || return 1
		sleep 1
	done
}

# The same over HTTPS, for a client that trusts the certificate in the
# file $1 alone and checks that it names localhost.
serves_https() {
	tries=0
	until test "$(curl -s --cacert "$1" https://localhost/)" = "$page"; do
		tries=$((tries + 1))
		[ "$tries" -lt 30 ] || return 1
		sleep 1
	done
}

pages_put() {
	elevate sh -c "echo '$page' > /shared/www/index.html && mkdir -p /shared/www/sites/shop &&
		echo shop > /shared/www/sites/shop/index.html"
}

# A line that does not read: an opening brace that nothing closes.
broken() {
	elevate sh -c "echo 'oops {' >> $conf/Caddyfile"
}

refused_by_validate() {
	! elevate caddy validate --config "$conf/Caddyfile" --adapter caddyfile
}

mended() {
	elevate sed -i '$d' "$conf/Caddyfile"
}

# As the Caddyfile's comments say: the machine's own certificate and port
# 443, as the service's second socket, in its file; its HTTPS site turned
# on; and a restart, which has Spark copy the certificate and make the
# socket.
https_set_up() {
	elevate sed -i '/^secrets = /a certificates = ["this-machine"]' "$service_file" &&
		elevate sh -c "printf '\n[[sockets]]\nport = 443\nname = \"https\"\n' >> $service_file" &&
		elevate sed -i '/^#https:\/\/ {/,/^#}/s/^#//' "$conf/Caddyfile" &&
		elevate service caddy restart
}

# Caddy offers HTTP/2 to a client that asks for it in TLS's handshake
# (ALPN). The system's curl speaks HTTP/1.1 alone, so openssl asks.
offers_http2() {
	echo | openssl s_client -connect localhost:443 -alpn h2 -CAfile /tmp/first.pem 2>/dev/null |
		grep -q '^ALPN protocol: h2'
}

# 300 requests at 50 a second, over HTTP and HTTPS, while Caddy reloads its
# settings three times: each is served.
reloads_under_load() {
	curl -s --cacert /tmp/first.pem --rate 50/s --out-null -w '%{http_code}\n' \
		'{http,https}://localhost/?n=[1-150]' >/tmp/codes &
	load=$!
	sleep 1
	for i in 1 2 3; do
		elevate service caddy reload || return 1
		sleep 1
	done
	wait "$load"
	test "$(grep -c '^200$' /tmp/codes)" = 300
}

# Each start of Caddy logs its first settings once; a reload does not.
starts() {
	elevate log caddy | grep -c 'serving initial configuration'
}

# certd makes the certificate anew: Spark copies it for the service and
# sends it SIGHUP, and Caddy serves the new one without starting again. A
# client that trusts only the certificate before is refused.
renewal_served() {
	before=$(starts)
	elevate cert renew this-machine && cert show this-machine --pem >/tmp/renewed.pem || return 1
	test "$(cat /tmp/renewed.pem)" != "$(cat /tmp/first.pem)" || return 1
	serves_https /tmp/renewed.pem || return 1
	! curl -s -o /dev/null --cacert /tmp/first.pem https://localhost/ &&
		test "$(starts)" = "$before"
}

# connectloop's 300 requests, one after another, while Caddy restarts
# three times and its bundle is installed over itself: none is refused and
# none fails.
restarts_under_load() {
	/system/tests/posix/connectloop 127.0.0.1 80 300 40 'GET / HTTP/1.0\r\n\r\n' >/tmp/loop &
	loop=$!
	for i in 1 2 3; do
		elevate service caddy restart || return 1
		sleep 1
	done
	elevate bundle install --for-everyone --yes "$here"/com.caddyserver.caddy-*.dkv || return 1
	wait "$loop"
	grep -q 'connections 300 refused 0 failed 0' /tmp/loop
}

port_refuses() {
	! curl -s -o /dev/null http://localhost/
}

case "$1" in
http)
	check pages pages_put
	check page serves
	check missing test "$(status http://localhost/missing.html)" = 404
	check sites test "$(status http://localhost/sites/shop/)" = 404
	check settings test "$(elevate stat -c '%U %a' "$conf/Caddyfile")" = "caddy 600"
	check admin_socket test "$(elevate stat -c %U /system/live/services/caddy/admin.sock)" = caddy
	;;
settings)
	check valid elevate caddy validate --config "$conf/Caddyfile" --adapter caddyfile
	check broken broken
	check refused refused_by_validate
	check reload elevate service caddy reload
	check kept logged 'caddy-service: Caddy kept the settings it had'
	check still_serves serves
	check mended mended
	check restart elevate service caddy restart
	check serves_again serves
	;;
https)
	check set_up https_set_up
	check certificate sh -c 'cert show this-machine --pem >/tmp/first.pem'
	check served serves_https /tmp/first.pem
	check http2 offers_http2
	check http_too serves
	;;
reloads)
	check reloads reloads_under_load
	;;
renewal)
	check renewal renewal_served
	;;
restarts)
	check restarts restarts_under_load
	check serves serves
	;;
stop)
	check stop elevate service caddy stop
	check graceful logged '"shutdown complete","signal":"SIGTERM","exit_code":0'
	check refuses port_refuses
	;;
esac
echo "$1 $held of $checked"
"#;

/// Makes the disk: Caddy's bundle and the steps' script, `caddy.sh`.
pub fn make_disk(ctx: &Ctx, disk: &Disk, path: &Path) -> Result<(), String> {
    make_bundle_disk(ctx, disk, path, |name| name.starts_with(BUNDLE_PREFIX))?;
    add_disk_files(path, &[("caddy.sh", STEPS_SCRIPT)])
}

fn caddy(vm: &mut Vm, case: &Case) -> Result<(), String> {
    owner_signs_in(vm, case)?;
    vm.expect_anywhere("bundled: 0 bundles installed; answering at /system/live/bundled/socket")?;
    vm.expect_anywhere(&format!("spark: mounted the FAT filesystem of vdb at {VOLUME}"))?;
    // For everyone, through elevate: its service waits to be enabled.
    answer_prompt(
        vm,
        "$",
        &format!(
            "clear; cd {VOLUME}; elevate bundle install --for-everyone --yes {FILE} | grep -e \
             'as the account' -e installed; cd"
        ),
    )?;
    answer_prompt(vm, &format!("{FILE}:"), "queen of hearts")?;
    vm.expect_screen(&[
        "caddy, as the account caddy, starting once an administrator enables it",
        // The line goes on past the screen's edge.
        "installed com.caddyserver.caddy 2.11.9, for everyone",
    ])?;
    vm.expect_anywhere("authd: made the account caddy (")?;
    // Enabled, it starts, and serves from the port Spark holds.
    answer_prompt(vm, "$", "clear; elevate service caddy enable; echo \"enable $?\"")?;
    vm.expect_screen(&["enable 0"])?;
    vm.expect_anywhere("serving initial configuration")?;
    // caddy runs with the access of whoever runs it, and through elevate
    // with root's: each first run asks, with a warning, in her pane (ADR
    // 0493).
    run_answering_until(vm, "clear; caddy version", "v2.11.9", 120)?;
    unlock(vm)?;
    run_answering_until(vm, "clear; elevate caddy version", "v2.11.9", 120)?;
    run_step(vm, SCRIPT, "http", 6, 120)?;
    unlock(vm)?;
    run_step(vm, SCRIPT, "settings", 9, 300)?;
    unlock(vm)?;
    run_step(vm, SCRIPT, "https", 5, 300)?;
    run_step(vm, SCRIPT, "reloads", 1, 120)?;
    unlock(vm)?;
    run_step(vm, SCRIPT, "renewal", 1, 120)?;
    unlock(vm)?;
    run_step(vm, SCRIPT, "restarts", 2, 300)?;
    unlock(vm)?;
    run_step(vm, SCRIPT, "stop", 3, 120)?;
    vm.press_power_button()?;
    vm.expect("spark: every service has stopped")?;
    vm.expect("Power: `spark` asked to power off the machine; syncing the filesystems")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}

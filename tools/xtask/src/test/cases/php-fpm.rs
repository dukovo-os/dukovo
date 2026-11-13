// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! PHP-FPM, the PHP bundle's service, runs PHP pages for nginx (ADR 0520).
//! Dukovo's root is on a disk, so that the bundles have room, and a second
//! disk holds the `.dkv` files `ob ports` packed, nginx's and PHP's among
//! them. Through elevate, Alice installs both for everyone and enables both
//! services: PHP's runs as the account named `php`, with its log in the
//! system's, and `elevate php-fpm -t` checks its settings as root, after
//! asking in her pane whether the command may run with root's access. With
//! nginx's PHP location turned on, as its settings say, and a site in
//! /shared/www, nginx shows the front page that `index.php` prints, and a
//! page that checks its surroundings finds the FastCGI interface, the
//! account `php`, opcache, a session, a POST field, a file written into a
//! folder the account owns and none written into the site's own. A PHP page
//! that is not there is a 404. Three reloads of PHP-FPM while 150 requests
//! come lose none and leave its process as it was. A setting that does not
//! read is named by `php-fpm -t` and stops a reload, as upstream's PHP-FPM
//! does, and a restart serves again once it is gone. A PHP error is in `log
//! php-fpm`, and with the service stopped, nginx answers 502. The power
//! button ends it.

use super::super::*;
use std::sync::atomic::{AtomicU32, Ordering};

/// The disk of bundles' label.
const LABEL: &str = "BUNDLES";

/// Where Spark mounts it: the second virtio disk's name.
const VOLUME: &str = "/volumes/vdb";

/// nginx's bundle file.
const NGINX_FILE: &str = "org.nginx.nginx-1.30.6.dkv";

/// PHP's bundle file.
const PHP_FILE: &str = "net.php.php-8.5.12.dkv";

/// nginx's settings' folder, in its account's data folder.
const NGINX_CONF: &str = "/shared/services/org.nginx.nginx/nginx/conf";

/// PHP-FPM's, in its account's data folder.
const FPM_HOME: &str = "/shared/services/net.php.php/php";

/// How many checks `info.php` prints: one `ok-<name>` each.
const CHECKS: usize = 8;

/// Dukovo's root on a copy of the test image, and the bundles.
const DISKS: &[Disk] = &[
    Disk { label: image::ROOT_LABEL, ..btrfs_disk(Bus::Virtio, 512, ROOT, root_checked) },
    bundle_files_disk(128 * 1024, LABEL),
];

/// The test.
pub const CASE: Case = Case {
    name: "php-fpm",
    about: "PHP-FPM, the PHP bundle's service, runs PHP pages for nginx (ADR 0520): installed with nginx for everyone from their .dkv files and enabled, it runs as its own account, and elevate php-fpm -t checks its settings after asking in her pane; with nginx's PHP location turned on and a site in /shared/www, the front page is index.php, and a page finds the FastCGI interface, the account php, opcache, a session, a POST field, a file written into a folder the account owns and none into the site's own; a missing PHP page is a 404; three reloads while 150 requests come lose none and keep its process; a setting that does not read is named by php-fpm -t and stops a reload, and a restart serves again once it is gone; a PHP error is in log php-fpm; stopped, nginx answers 502",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    // As a normal boot: Spark starts the services, bundled among them.
    cmdline: "lockdep",
    cpus: 2,
    // On 2 GiB the install of the bundle's 20,000 files to the root stops
    // with no_memory (and Files says its write-back to the disk ran out of
    // it): a gap of the kernel, in docs/deferred.md.
    memory: "4G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &[],
    script: php_fpm,
    disks: DISKS,
    second: None,
};

/// Unlocks elevate in the pane, or keeps it unlocked for 15 minutes more,
/// asking the password if it is locked: a long step may take it from the
/// pane.
fn unlock(vm: &mut Vm) -> Result<(), String> {
    static UNLOCKS: AtomicU32 = AtomicU32::new(0);
    let n = UNLOCKS.fetch_add(1, Ordering::Relaxed);
    let marker = format!("unlocked-{n}");
    answer_prompt(vm, "$", &format!("clear; elevate -v; echo unlocked-$(echo {n})"))?;
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut answered = false;
    loop {
        let screen = vm.screen()?;
        if screen.iter().any(|line| line.trim() == marker) {
            return Ok(());
        }
        if !answered && screen.iter().any(|line| line.contains("to unlock elevate in this pane")) {
            answer_prompt(vm, "in this pane:", "queen of hearts")?;
            answered = true;
        }
        if Instant::now() >= deadline {
            return screen_shows(&screen, &[&marker]);
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

fn php_fpm(vm: &mut Vm, case: &Case) -> Result<(), String> {
    owner_signs_in(vm, case)?;
    vm.expect_anywhere("bundled: 0 bundles installed; answering at /system/live/bundled/socket")?;
    vm.expect_anywhere(&format!("spark: mounted the FAT filesystem of vdb at {VOLUME}"))?;
    // Both for everyone, through elevate: the services wait to be enabled.
    unlock(vm)?;
    answer_prompt(
        vm,
        "$",
        &format!(
            "clear; cd {VOLUME}; elevate bundle install --for-everyone --yes {NGINX_FILE} | grep \
             -e 'as the account' -e installed; cd"
        ),
    )?;
    vm.expect_screen(&["installed org.nginx.nginx 1.30.6, for everyone"])?;
    // About twenty thousand files, most of them PHP's own tests.
    unlock(vm)?;
    answer_prompt(
        vm,
        "$",
        &format!(
            "clear; cd {VOLUME}; elevate bundle install --for-everyone --yes {PHP_FILE} | grep \
             -e 'as the account' -e installed; cd"
        ),
    )?;
    answer_prompt_within(vm, "$", "clear; ls -d /applications/net.php.php*", 1800)?;
    vm.expect_screen(&["/applications/net.php.php"])?;
    vm.expect_anywhere("authd: made the account php (")?;
    // Enabled, they start.
    unlock(vm)?;
    answer_prompt(
        vm,
        "$",
        "clear; elevate service nginx enable; elevate service php-fpm enable; echo \"enable $?\"",
    )?;
    vm.expect_screen(&["enable 0"])?;
    vm.expect_anywhere("start worker processes")?;
    vm.expect_anywhere("ready to handle connections")?;
    answer_prompt(
        vm,
        "$",
        "clear; elevate service php-fpm status | grep -o 'active (running) as process'",
    )?;
    vm.expect_screen(&["active (running) as process"])?;
    // Its settings, checked as root: the command asks, in her pane, whether
    // it may run with root's access (ADR 0493), and she allows it always.
    run_answering_until(vm, "clear; elevate php-fpm -t", "test is successful", 120)?;
    // The site: an index page, a page of checks, a page with an error, and
    // a folder the account php may write in.
    answer_prompt(
        vm,
        "$",
        "clear; printf '%s\\n' '<?php echo \"php-front\\n\";' > ~/index.php; printf '%s\\n' \
         '<?php' 'function c($n,$v){echo ($v?\"ok-\":\"FAIL-\").$n.\" \";}' \
         'c(\"sapi\",PHP_SAPI==\"fpm-fcgi\");' \
         'c(\"user\",posix_getpwuid(posix_geteuid())[\"name\"]==\"php\");' \
         'c(\"opcache\",opcache_get_status()[\"opcache_enabled\"]);' > ~/info.php",
    )?;
    answer_prompt(
        vm,
        "$",
        "clear; printf '%s\\n' 'session_start();c(\"session\",session_id()!=\"\");' \
         'c(\"method\",$_SERVER[\"REQUEST_METHOD\"]==\"POST\");' \
         'c(\"post\",($_POST[\"a\"]??\"\")==\"1\");' \
         'c(\"upload\",@file_put_contents(__DIR__.\"/uploads/u.txt\",\"x\")===1);' \
         'c(\"readonly\",@file_put_contents(__DIR__.\"/n.txt\",\"x\")===false);' \
         'echo \"n=\",$_GET[\"n\"]??0,\"\\n\";' >> ~/info.php; printf '%s\\n' '<?php' \
         'trigger_error(\"boom-fpm\",E_USER_WARNING);' > ~/err.php",
    )?;
    unlock(vm)?;
    answer_prompt(
        vm,
        "$",
        "clear; elevate sh -c 'cd /users/alice && cp index.php info.php err.php /shared/www/ && \
         mkdir /shared/www/uploads && chown php /shared/www/uploads'; echo \"site $?\"",
    )?;
    vm.expect_screen(&["site 0"])?;
    // With nginx's PHP location off, as shipped, none of it is served.
    answer_prompt(
        vm,
        "$",
        "clear; curl -s -o /dev/null -w 'off %{http_code}\\n' http://localhost/info.php",
    )?;
    vm.expect_screen(&["off 404"])?;
    // Turned on, as the settings' comments say: the line that returns 404
    // is commented out, and the three after it are not.
    answer_prompt(
        vm,
        "$",
        &format!(
            "clear; elevate sh -c 'cd {NGINX_CONF} && sed -i -e \"s/^ *return 404;/#&/\" -e \
             \"s/^ *#\\(try_files\\|fastcgi_pass\\|include fastcgi\\)/    \\1/\" nginx.conf && \
             service nginx reload'; echo \"on $?\""
        ),
    )?;
    vm.expect_screen(&["on 0"])?;
    // The front page is index.php, whose output nginx passes on.
    answer_prompt(
        vm,
        "$",
        "clear; for i in 1 2 3 4 5 6 7 8 9 10; do curl -s http://localhost/ | grep -q php-front && \
         echo 'front page' && break; sleep 1; done",
    )?;
    vm.expect_screen(&["front page"])?;
    answer_prompt(
        vm,
        "$",
        &format!(
            "clear; curl -s -d a=1 'http://localhost/info.php?n=7' > ~/info.txt; echo \"checks \
             $(tr ' ' '\\n' < ~/info.txt | grep -c '^ok-') of {CHECKS}\"; tr ' ' '\\n' < \
             ~/info.txt | grep -e FAIL- -e '^n='"
        ),
    )?;
    vm.expect_screen(&[&format!("checks {CHECKS} of {CHECKS}"), "n=7"])?;
    // The upload is the account php's, and the session is kept in its
    // data folder.
    answer_prompt(
        vm,
        "$",
        &format!(
            "clear; elevate stat -c 'upload owner %U' /shared/www/uploads/u.txt; elevate sh -c \
             'ls {FPM_HOME}/sessions | grep -q sess_ && echo session kept'"
        ),
    )?;
    vm.expect_screen(&["upload owner php", "session kept"])?;
    // A PHP page that is not there is a 404, and so is one in a folder
    // that is not.
    answer_prompt(
        vm,
        "$",
        "clear; curl -s -o /dev/null -w 'missing %{http_code}\\n' http://localhost/nope.php; curl \
         -s -o /dev/null -w 'folder %{http_code}\\n' http://localhost/nope/x.php",
    )?;
    vm.expect_screen(&["missing 404", "folder 404"])?;
    // Nothing PHP-FPM said at its start is more than a notice.
    unlock(vm)?;
    answer_prompt(
        vm,
        "$",
        "clear; echo \"problems $(elevate log php-fpm | grep -c -e WARNING -e ERROR -e ALERT)\"",
    )?;
    vm.expect_screen(&["problems 0"])?;
    // Reloads while requests come: 150 at 50 a second, three reloads, none
    // lost, and the process the same.
    unlock(vm)?;
    run_within(
        vm,
        "clear; elevate service php-fpm status | grep -o 'as process [0-9]*' > /tmp/before; curl -s \
         --rate 50/s --out-null -w '%{http_code}\\n' 'http://localhost/info.php?n=[1-150]' > \
         /tmp/codes & sleep 1; elevate sh -c 'for i in 1 2 3; do service php-fpm reload; sleep 1; \
         done'; wait; echo \"served $(grep -c '^200$' /tmp/codes) of $(grep -c . /tmp/codes)\" > \
         /tmp/served; elevate service php-fpm status | grep -o 'as process [0-9]*' > /tmp/after",
        300,
    )?;
    answer_prompt(
        vm,
        "$",
        "clear; cat /tmp/served; cmp -s /tmp/before /tmp/after && echo 'same process'",
    )?;
    vm.expect_screen(&["served 150 of 150", "same process"])?;
    // Settings that do not read: php-fpm -t names the line. A reload reads
    // the settings again, and PHP-FPM stops with the line in its log, as
    // upstream's does; once the line is gone, a restart serves again.
    unlock(vm)?;
    answer_prompt(
        vm,
        "$",
        &format!(
            "clear; elevate sh -c 'echo bogus-setting = 1 >> {FPM_HOME}/conf/php-fpm.conf; \
             n=$(php-fpm -t 2>&1 | grep -c \"unknown entry\"); echo checked $n; service php-fpm \
             reload; for i in 1 2 3 4 5 6 7 8 9 10; do sleep 1; log php-fpm | grep -q \"unknown \
             entry\" && echo refused && break; done; sed -i \"\\$d\" {FPM_HOME}/conf/php-fpm.conf'"
        ),
    )?;
    vm.expect_screen(&["checked 1", "refused"])?;
    answer_prompt(
        vm,
        "$",
        "clear; elevate service php-fpm restart; echo \"restart $?\"; for i in 1 2 3 4 5 6 7 8 9 \
         10; do curl -s http://localhost/ | grep -q php-front && echo serving && break; sleep 1; \
         done",
    )?;
    vm.expect_screen(&["restart 0", "serving"])?;
    // A PHP error is in its log.
    unlock(vm)?;
    answer_prompt(
        vm,
        "$",
        "clear; curl -s http://localhost/err.php; elevate sh -c 'log php-fpm | grep -q boom-fpm && \
         echo error logged'",
    )?;
    vm.expect_screen(&["error logged"])?;
    // Stopped, nginx has nothing to pass pages to.
    answer_prompt(
        vm,
        "$",
        "clear; elevate service php-fpm stop; sleep 2; curl -s -o /dev/null -w 'stopped \
         %{http_code}\\n' http://localhost/info.php",
    )?;
    vm.expect_screen(&["stopped 502"])?;
    vm.press_power_button()?;
    vm.expect("spark: every service has stopped")?;
    vm.expect("Power: `spark` asked to power off the machine; syncing the filesystems")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}

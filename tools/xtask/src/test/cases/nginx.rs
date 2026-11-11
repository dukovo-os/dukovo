// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! nginx, the catalog's web server, serves (ADR 0491). Dukovo's root is on
//! a disk, so that the bundle has room, and a second disk holds the `.dkv`
//! files `ob ports` packed, nginx's among them. Through elevate, Alice
//! installs it for everyone: authd makes the account nginx, bundled makes
//! /shared/www, which its service reads, and the service waits until she
//! enables it. Enabled, it starts as nginx, with its log in the system's,
//! and serves its welcome page, then the page she puts in /shared/www, and
//! a 404 for a page that is not there. Settings that do not read leave it
//! as it was on a reload, serving, with the wrong line in its log. With a
//! certificate of its own made with openssl and the HTTPS
//! server of its settings turned on, `service nginx reload` has it serve
//! HTTPS too; three more reloads while 300 requests come over HTTP and
//! HTTPS lose none of them. `log nginx` holds a line for each request, and
//! its process number's file is the account's. The power button ends it.

use super::super::*;

/// The disk of bundles' label.
const LABEL: &str = "BUNDLES";

/// Where Spark mounts it: the second virtio disk's name.
const VOLUME: &str = "/volumes/vdb";

/// nginx's bundle file.
const FILE: &str = "org.nginx.nginx-1.30.6.dkv";

/// Its settings' folder, in its account's data folder.
const CONF: &str = "/shared/services/org.nginx.nginx/nginx/conf";

/// Dukovo's root on a copy of the test image, and the bundles.
const DISKS: &[Disk] = &[
    Disk { label: image::ROOT_LABEL, ..btrfs_disk(Bus::Virtio, 512, ROOT, root_checked) },
    bundle_files_disk(128 * 1024, LABEL),
];

/// The test.
pub const CASE: Case = Case {
    name: "nginx",
    about: "nginx, the catalog's web server, serves (ADR 0491): installed for everyone from its .dkv file and enabled, it runs as its own account and serves its welcome page, then a page in /shared/www; settings that do not read leave it serving, with the wrong line in its log; with a certificate of its own it serves HTTPS too, and reloads while 300 requests come over HTTP and HTTPS lose none; log nginx holds the requests",
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
    script: nginx,
    disks: DISKS,
    second: None,
};

fn nginx(vm: &mut Vm, case: &Case) -> Result<(), String> {
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
        "nginx, as the account nginx, starting once an administrator enables it",
        // The line goes on past the screen's edge.
        "installed org.nginx.nginx 1.30.6, for everyone; its service nginx starts once",
    ])?;
    vm.expect_anywhere("authd: made the account nginx (100) of a service of org.nginx.nginx")?;
    // bundled made the folder it reads; enabled, it starts.
    answer_prompt(
        vm,
        "$",
        "clear; ls -ld /shared/www | cut -c1-10; elevate service nginx enable; echo \"enable $?\"",
    )?;
    vm.expect_screen(&["drwxr-xr-x", "enable 0"])?;
    vm.expect_anywhere("start worker processes")?;
    // Its welcome page, until /shared/www has a page of its own.
    answer_prompt(vm, "$", "clear; curl -s http://localhost/ | grep -o 'Welcome to nginx!'")?;
    vm.expect_screen(&["Welcome to nginx!"])?;
    answer_prompt(
        vm,
        "$",
        "clear; elevate sh -c 'echo \"<p>Hello from Dukovo</p>\" > /shared/www/index.html'; \
         curl -s http://localhost/; curl -s -o /dev/null -w 'missing %{http_code}\\n' \
         http://localhost/missing.html",
    )?;
    vm.expect_screen(&["<p>Hello from Dukovo</p>", "missing 404"])?;
    // Settings that do not read: the reload leaves nginx as it was, still
    // serving, and its log says which line is wrong. The line goes again.
    answer_prompt(
        vm,
        "$",
        &format!(
            "clear; elevate sh -c 'echo oops >> {CONF}/nginx.conf; service nginx reload; for i in \
             1 2 3 4 5; do sleep 1; log nginx | grep -q \"emerg.*nginx.conf:\" && echo refused && \
             break; done; sed -i \"\\$d\" {CONF}/nginx.conf'; curl -s http://localhost/"
        ),
    )?;
    vm.expect_screen(&["refused", "<p>Hello from Dukovo</p>"])?;
    // HTTPS: a certificate of its own, the account's to read, and the
    // HTTPS server of its settings turned on, as their comments say.
    answer_prompt(
        vm,
        "$",
        &format!(
            "clear; elevate sh -c 'cd {CONF} && openssl req -x509 -newkey ec -pkeyopt \
             ec_paramgen_curve:P-256 -nodes -days 365 -subj /CN=dukovo -keyout key.pem -out \
             cert.pem 2>/dev/null && chown nginx:nginx key.pem cert.pem && sed -i \
             \"/^    #server {{/,/^    #}}/s/^    #/    /\" nginx.conf && service nginx reload' && \
             echo reloaded"
        ),
    )?;
    vm.expect_screen(&["reloaded"])?;
    answer_prompt(
        vm,
        "$",
        "clear; for i in 1 2 3 4 5; do curl -sk -o /dev/null -w 'https %{http_code}\\n' \
         https://localhost/ && break; sleep 1; done",
    )?;
    vm.expect_screen(&["https 200"])?;
    // Reloads under load: 300 requests at 50 a second, over HTTP and
    // HTTPS, while it reloads three times; none is lost.
    answer_prompt(
        vm,
        "$",
        "clear; curl -sk --rate 50/s --out-null -w '%{http_code}\\n' \
         '{http,https}://localhost/?n=[1-150]' > /tmp/codes & sleep 1; elevate sh -c 'for i in 1 \
         2 3; do service nginx reload; sleep 1; done'; wait; echo \"served $(grep -c '^200$' \
         /tmp/codes) of $(grep -c . /tmp/codes)\" > /tmp/served",
    )?;
    answer_prompt_within(vm, "$", "clear; cat /tmp/served", 120)?;
    vm.expect_screen(&["served 300 of 300"])?;
    // Its log is the system's; its process number's file is its
    // account's.
    answer_prompt(
        vm,
        "$",
        "clear; elevate log nginx | grep -q '\"GET /?n=' && echo 'requests \
         logged'; elevate stat -c 'pid file: %U %G' /system/live/services/nginx/nginx.pid",
    )?;
    vm.expect_screen(&["requests logged", "pid file: nginx nginx"])?;
    vm.press_power_button()?;
    vm.expect("spark: every service has stopped")?;
    vm.expect("Power: `spark` asked to power off the machine; syncing the filesystems")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}

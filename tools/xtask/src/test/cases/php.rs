// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! PHP, the catalog's PHP, runs (ADR 0499). Dukovo's root is on a disk, so
//! that the bundle has room, and a second disk holds the `.dkv` files `ob
//! ports` packed, PHP's among them. The bundle has a service, PHP-FPM
//! (ADR 0520), so it installs only for everyone: Alice does it through
//! elevate. `php` asks, with a warning, whether it may run with her access,
//! then says it is PHP 8.5.12 and lists its extensions, each built in. Its
//! php.ini is the one beside its program in /applications, and PHP_BINARY
//! is its program's path there, which the kernel gave it, as the folder is
//! not one a search of PATH finds. A script on the disk of bundles checks
//! what PHP's extensions are for: XML, multibyte strings, JSON, hashes,
//! OpenSSL, regular expressions, time zones, compression, files, processes
//! forked and run, sockets, System V shared memory and semaphores, opcache
//! and curl. Tests of PHP's own suite, which the disk lists, check what the
//! port changes in PHP.

use std::process::Command;

use super::super::*;

/// The disk of bundles' label.
const LABEL: &str = "BUNDLES";

/// Where Spark mounts it: the second virtio disk's name.
const VOLUME: &str = "/volumes/vdb";

/// PHP's bundle file.
const FILE: &str = "net.php.php-8.5.12.dkv";

/// How many modules `php -m` lists under "PHP Modules": Core and the
/// forty-two the bundle builds in, libxml among them, which the XML ones
/// need, and Zend OPcache. OPcache is a Zend extension as well, and is
/// listed again under "Zend Modules".
const MODULES: usize = 43;

/// How many checks [`CHECKS_SCRIPT`] makes.
const CHECKS: usize = 24;

/// PHP's program, by its path, as a script names it.
const PROGRAM: &str = "/applications/net.php.php/bin/php";

/// Tests of PHP's own suite for what the port changes in PHP: a socket that
/// accept(2) gives is blocking, as the kernel makes it, when the listening
/// one is not (patch 0010), and host names are looked up with the C
/// library's gethostbyname(3), as build.sh tells configure, which the
/// sockets extension's tests do to make their listening socket; and calls
/// nested too deep end in an Error, not a crash, as build.sh tells
/// configure the stack grows down.
const SUITE: &[&str] = &[
    "Zend/tests/gh18572.phpt",
    "ext/standard/tests/streams/gh8472.phpt",
    "ext/standard/tests/network/gethostbyname_basic003.phpt",
    "ext/standard/tests/network/gethostbynamel_basic1.phpt",
    "ext/sockets/tests/gh17921.phpt",
    "ext/sockets/tests/socket_connect_params.phpt",
    "ext/sockets/tests/socket_create_listen-nobind.phpt",
    "ext/sockets/tests/socket_create_listen.phpt",
    "ext/sockets/tests/socket_create_listen_invalid_port.phpt",
    "ext/sockets/tests/socket_create_listen_used.phpt",
    "ext/sockets/tests/socket_read_params.phpt",
    "ext/sockets/tests/socket_send_params.phpt",
    "ext/sockets/tests/socket_sendto_invalid_port.phpt",
    "ext/sockets/tests/socket_sendto_params.phpt",
    "ext/sockets/tests/socket_set_block-retval.phpt",
    "ext/sockets/tests/socket_set_nonblock-retval.phpt",
    "ext/sockets/tests/socket_set_nonblock.phpt",
];

/// Dukovo's root on a copy of the test image, and the bundles, with the
/// checks' script and the list of the suite's tests beside them.
const DISKS: &[Disk] = &[
    Disk { label: image::ROOT_LABEL, ..btrfs_disk(Bus::Virtio, 512, ROOT, root_checked) },
    Disk { fat: PHP_FILES, ..bundle_files_disk(128 * 1024, LABEL) },
];

/// The test.
pub const CASE: Case = Case {
    name: "php",
    about: "PHP, the catalog's PHP, runs (ADR 0499): installed for everyone through elevate from its .dkv file, php asks whether it may run with her access and says it is 8.5.12; php -m lists its extensions, all built in; its php.ini and PHP_BINARY are in /applications, where the kernel's path to the program puts them; a script checks its XML, multibyte, JSON, hash, OpenSSL, regular expression, time zone, compression, file, process, socket, System V and shared memory, opcache and curl code",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    cmdline: "lockdep",
    cpus: 2,
    memory: "2G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &[],
    script: php,
    disks: DISKS,
    second: None,
};

/// The checks, a script the test runs from the disk of bundles with
/// opcache on: each prints `ok-<name>` if it holds and `FAIL-<name>` if
/// not. The child is forked before anything is printed, so that its exit
/// flushes nothing of its parent's.
const CHECKS_SCRIPT: &str = r#"<?php
function c($name, $held) {
    echo ($held ? "ok-" : "FAIL-") . $name . "\n";
}

$child = pcntl_fork();
if ($child == 0) {
    exit(7);
}
pcntl_waitpid($child, $status);
$forked = pcntl_wexitstatus($status) == 7;

// DOM with XPath, SimpleXML, XMLWriter.
$document = new DOMDocument;
$document->loadXML("<a><b>x</b><b>y</b></a>");
c("dom", (new DOMXPath($document))->query("//b")->length == 2);
c("simplexml", (int) simplexml_load_string("<a><b>7</b></a>")->b == 7);
$writer = new XMLWriter;
$writer->openMemory();
$writer->startElement("r");
$writer->endElement();
c("xmlwriter", trim($writer->outputMemory()) == "<r/>");

// Multibyte strings, JSON, a hash, regular expressions with Unicode,
// random bytes.
c("mbstring", mb_strtoupper("\u{e9}") == "\u{c9}");
c("json", json_decode(json_encode(["a" => [1, 2]]), true) == ["a" => [1, 2]]);
c("hash", hash("sha256", "abc")
    == "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
c("pcre", preg_match('/^\p{Lu}\p{Ll}+$/u', "\u{c9}lan") == 1);
c("random", strlen(random_bytes(16)) == 16);

// OpenSSL's cipher both ways, a time zone's offset, compression.
$key = random_bytes(32);
$iv = random_bytes(16);
$sealed = openssl_encrypt("hi", "aes-256-cbc", $key, 0, $iv);
c("openssl", openssl_decrypt($sealed, "aes-256-cbc", $key, 0, $iv) == "hi");
$noon = new DateTime("2026-07-01 12:00:00", new DateTimeZone("Europe/Paris"));
c("date", $noon->format("P") == "+02:00");
c("zlib", gzuncompress(gzcompress("abc")) == "abc");
c("bz2", bzdecompress(bzcompress("abc")) == "abc");

// A file, the child forked above and its status, a program run.
file_put_contents("/tmp/p.txt", "x");
c("files", file_get_contents("/tmp/p.txt") == "x");
c("pcntl", $forked);
$program = proc_open("echo hi", [1 => ["pipe", "w"]], $pipes);
$said = stream_get_contents($pipes[1]);
proc_close($program);
c("proc_open", trim($said) == "hi");

// A server and a client in one script, not blocking, with select.
$server = stream_socket_server("tcp://127.0.0.1:0", $errno, $error);
$address = stream_socket_get_name($server, false);
$client = stream_socket_client("tcp://" . $address, $errno, $error, 5);
stream_set_blocking($client, false);
$read = [$server];
$none = null;
$selected = stream_select($read, $none, $none, 5) == 1;
$accepted = stream_socket_accept($server, 1);
fwrite($client, "ping");
$read = [$accepted];
$selected = $selected && stream_select($read, $none, $none, 5) == 1;
c("sockets", $selected && fread($accepted, 4) == "ping");

// Shared memory and semaphores of System V, and shmop.
$segment = shmop_open(0xA1B2, "c", 0644, 64);
shmop_write($segment, "hey", 0);
c("shmop", shmop_read($segment, 0, 3) == "hey");
shmop_delete($segment);
$semaphore = sem_get(0xB1C2);
c("sysvsem", sem_acquire($semaphore) && sem_release($semaphore));
sem_remove($semaphore);
$shared = shm_attach(0xC1D2, 1024);
shm_put_var($shared, 1, "v");
c("sysvshm", shm_get_var($shared, 1) == "v");
shm_remove($shared);

// curl reading a file: URL, and the extensions' own functions.
$curl = curl_init("file:///tmp/p.txt");
curl_setopt($curl, CURLOPT_RETURNTRANSFER, true);
c("curl", curl_exec($curl) == "x");
c("ctype", ctype_digit("12"));
c("tokenizer", count(token_get_all("<?php echo 1;")) > 3);
c("session", session_status() == PHP_SESSION_NONE);

// Opcache, on for the command line: its shared memory is there.
c("opcache", opcache_get_status()["opcache_enabled"]);
"#;

/// Makes the disk of bundles, with the checks' script, `checks.php`, and
/// the list of the suite's tests, `suite.txt`, beside the bundles.
pub fn make_disk(ctx: &Ctx, disk: &Disk, path: &Path) -> Result<(), String> {
    make_bundle_files(ctx, disk, path)?;
    let image = path.display().to_string();
    let list = SUITE.join("\n") + "\n";
    for (name, text) in [("checks.php", CHECKS_SCRIPT), ("suite.txt", list.as_str())] {
        let source = path.with_extension(name);
        std::fs::write(&source, text)
            .map_err(|err| format!("writing {}: {err}", source.display()))?;
        let copied = host_tool(
            Command::new("mcopy").args(["-i", &image]).arg(&source).arg(format!("::/{name}")),
        );
        let _ = std::fs::remove_file(&source);
        copied?;
    }
    Ok(())
}

fn php(vm: &mut Vm, case: &Case) -> Result<(), String> {
    owner_signs_in(vm, case)?;
    vm.expect_anywhere("bundled: 0 bundles installed; answering at /system/live/bundled/socket")?;
    vm.expect_anywhere(&format!("spark: mounted the FAT filesystem of vdb at {VOLUME}"))?;
    // For everyone, through elevate, as the bundle has a service: it
    // installs for no one else. About twenty thousand files, most of them
    // its test suite.
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
        "clear; echo \"installed $(ls -d /applications/net.php.php* | wc -l)\"",
        1800,
    )?;
    vm.expect_screen(&["installed 1"])?;
    // php runs with the access of whoever runs it: its first run asks,
    // with a warning.
    run_answering_until(vm, "clear; php -v | head -n 1", "PHP 8.5.12 (cli)", 120)?;
    // The extensions, all built in: some by name, and all by their number.
    answer_prompt(
        vm,
        "$",
        "clear; php -m | grep -x -e dom -e mbstring -e openssl -e curl -e pcntl -e sockets -e \
         'Zend OPcache' | sed 's/^/ext /'",
    )?;
    vm.expect_screen(&[
        "ext dom",
        "ext mbstring",
        "ext openssl",
        "ext curl",
        "ext pcntl",
        "ext sockets",
        "ext Zend OPcache",
    ])?;
    answer_prompt(
        vm,
        "$",
        "clear; echo \"modules $(php -m | sed -n '/^\\[PHP Modules\\]/,/^$/p' | grep -c '^[A-Za-z]')\"",
    )?;
    vm.expect_screen(&[&format!("modules {MODULES}")])?;
    // Its php.ini and its program are beside each other in
    // /applications: the kernel gave PHP the program's path.
    answer_prompt(vm, "$", "clear; php -i | grep 'Loaded Configuration'")?;
    vm.expect_screen(&["Loaded Configuration File => /applications/net.php.php/etc/php.ini"])?;
    answer_prompt(vm, "$", "clear; php -r 'echo \"binary \", PHP_BINARY, \"\\n\";'")?;
    vm.expect_screen(&[&format!("binary {PROGRAM}")])?;
    // What php.ini's settings say, and a certificate bundle to find.
    answer_prompt(
        vm,
        "$",
        "clear; php -r 'echo \"ini \", ini_get(\"openssl.cafile\"), \" \", ini_get(\"curl.cainfo\"), \
         \"\\n\";'",
    )?;
    vm.expect_screen(&[
        "ini /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/ca-certificates.crt",
    ])?;
    // The checks, from the disk, with opcache on for the command line.
    run_within(
        vm,
        &format!("clear; php -d opcache.enable_cli=1 {VOLUME}/checks.php > /tmp/ok 2>&1"),
        300,
    )?;
    answer_prompt(
        vm,
        "$",
        &format!(
            "clear; grep -v '^ok-' /tmp/ok; echo \"checks $(grep -c '^ok-' /tmp/ok) of {CHECKS}\""
        ),
    )?;
    vm.expect_screen(&[&format!("checks {CHECKS} of {CHECKS}")])?;
    // A part of PHP's own test suite, the disk's list of it, run as the
    // lab runs the whole of it: on a copy, as run-tests.php writes beside
    // each test, and by the program's path.
    run_within(
        vm,
        &format!(
            "clear; sh -c 'cd /applications/net.php.php/tests && for f in run-tests.php $(cat \
             {VOLUME}/suite.txt); do mkdir -p /tmp/t/$(dirname $f) && cp $f /tmp/t/$f; done'"
        ),
        300,
    )?;
    run_within(
        vm,
        &format!(
            "clear; sh -c 'cd /tmp/t && TEST_PHP_EXECUTABLE={PROGRAM} {PROGRAM} run-tests.php -q \
             --no-color $(cat {VOLUME}/suite.txt) > /tmp/suite 2>&1'"
        ),
        900,
    )?;
    // The failed tests' names, if any, and the count of those that passed,
    // from run-tests.php's summary.
    answer_prompt(
        vm,
        "$",
        "clear; sed -n '/^FAILED TEST SUMMARY/,/^=====/p' /tmp/suite | grep '\\[' | cut -c 1-78; \
         grep '^Tests passed' /tmp/suite | tr -s ' '",
    )?;
    vm.expect_screen(&[&format!("Tests passed : {} (100.0%)", SUITE.len())])?;
    vm.press_power_button()?;
    vm.expect("Power: `spark` asked to power off the machine; syncing the filesystems")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}

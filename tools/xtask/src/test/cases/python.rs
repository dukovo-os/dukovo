// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Python, the catalog's CPython, runs (ADR 0495). Dukovo's root is on a
//! disk, so that the bundle has room, and a second disk holds the `.dkv`
//! files `ob ports` packed, Python's among them. Alice installs it for
//! herself, in her own applications folder; python3 asks, with a
//! warning, whether it may run with her access, then says it is 3.15.2
//! on radix, having found its library there. Its modules in C import,
//! TLS finds the system's root certificates, and the interactive
//! interpreter answers, as does a script that names it on its `#!`
//! line. A virtual environment has a pip of its own, and pip3 runs. A
//! part of CPython's test suite passes; the lab's test runs the whole of
//! it.

use super::super::*;

/// The disk of bundles' label.
const LABEL: &str = "BUNDLES";

/// Where Spark mounts it: the second virtio disk's name.
const VOLUME: &str = "/volumes/vdb";

/// Python's bundle file.
const FILE: &str = "org.python.python-3.15.2.dkv";

/// Test files of CPython's suite that pass on Radix and take seconds
/// each under emulation: the language, its compression and hashing
/// modules, select, paths and terminals.
const SUITE: &str = "test_grammar test_re test_zlib test_bz2 test_lzma test_zstd test_hashlib \
                     test_select test_pathlib test_pty";

/// Dukovo's root on a copy of the test image, and the bundles.
const DISKS: &[Disk] = &[
    Disk { label: image::ROOT_LABEL, ..btrfs_disk(Bus::Virtio, 512, ROOT, root_checked) },
    bundle_files_disk(128 * 1024, LABEL),
];

/// The test.
pub const CASE: Case = Case {
    name: "python",
    about: "Python, the catalog's CPython, runs (ADR 0495): installed for one person from its .dkv file, python3 asks whether it may run with her access and finds its library in her applications folder; its modules in C import, TLS finds the root certificates, the interactive interpreter and a script run by its #! line answer, a virtual environment has its own pip, and a part of CPython's test suite passes",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    cmdline: "lockdep",
    cpus: 2,
    // Each of the suite's two workers runs Python programs of its own.
    memory: "2G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &[],
    script: python,
    disks: DISKS,
    second: None,
};

fn python(vm: &mut Vm, case: &Case) -> Result<(), String> {
    owner_signs_in(vm, case)?;
    vm.expect_anywhere("bundled: 0 bundles installed; answering at /system/live/bundled/socket")?;
    vm.expect_anywhere(&format!("spark: mounted the FAT filesystem of vdb at {VOLUME}"))?;
    // For herself: in ~/applications, not /applications.
    answer_prompt(vm, "$", &format!("clear; cd {VOLUME}; bundle install --yes {FILE} | tail -1; cd"))?;
    answer_prompt_within(vm, "$", "clear; ls -d ~/applications/org.python.python*", 300)?;
    vm.expect_screen(&["/users/alice/applications/org.python.python"])?;
    // python3 runs with the access of whoever runs it: its first run asks,
    // with a warning.
    answer_prompt(vm, "$", "clear; python3 -c 'import sys; print(sys.platform, sys.version_info[:3])'")?;
    vm.expect_screen(&["[warning] ", "A: always"])?;
    vm.type_text("a")?;
    vm.expect_screen(&["radix (3, 15, 2)"])?;
    // Its modules in C are built in; TLS trusts the system's roots.
    answer_prompt(
        vm,
        "$",
        "clear; python3 -c 'import ssl, zlib, bz2, lzma, curses, decimal, pyexpat, \
         compression.zstd; print(\"roots\", ssl.create_default_context().cert_store_stats()[\"x509_ca\"] \
         > 100)'",
    )?;
    vm.expect_screen(&["roots True"])?;
    // The interactive interpreter.
    answer_prompt(vm, "$", "clear; python3")?;
    vm.expect_screen(&[">>>"])?;
    vm.type_text("6 * 7\n")?;
    vm.expect_screen(&["42"])?;
    vm.type_text("exit()\n")?;
    // A script that names it on its #! line ("\043\041" to printf) finds
    // its library too: the path the kernel gives is the script's then.
    answer_prompt(
        vm,
        "$",
        "clear; printf '\\043\\041%s\\nimport sys; print(\"exe\", sys.executable)\\n' \
         ~/applications/org.python.python/bin/python3 > s",
    )?;
    answer_prompt(vm, "$", "clear; chmod +x s; ./s")?;
    vm.expect_screen(&["exe /users/alice/applications/org.python.python/bin/python3"])?;
    // A virtual environment, with its own pip, and the bundle's pip3.
    // Making it installs pip into it, which takes a while under emulation.
    answer_prompt(
        vm,
        "$",
        "clear; python3 -m venv /tmp/v && /tmp/v/bin/pip --version | cut -d' ' -f1,2,4 | sed \
         's/^/venv /' > /tmp/pips; pip3 --version | cut -d' ' -f1,2 >> /tmp/pips",
    )?;
    answer_prompt_within(vm, "$", "clear; cat /tmp/pips", 300)?;
    vm.expect_screen(&["venv pip 26.2.2 /tmp/v/lib/python3.15/site-packages/pip", "pip 26.2.2"])?;
    // A part of CPython's own test suite.
    answer_prompt(
        vm,
        "$",
        &format!("clear; python3 -m test -j2 {SUITE} > /tmp/suite 2>&1; tail -n 1 /tmp/suite > /tmp/result"),
    )?;
    answer_prompt_within(vm, "$", "clear; cat /tmp/result", 600)?;
    vm.expect_screen(&["Result: SUCCESS"])?;
    vm.press_power_button()?;
    vm.expect("Power: `spark` asked to power off the machine; syncing the filesystems")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}

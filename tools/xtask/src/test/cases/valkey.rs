// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Valkey, the catalog's key-value store, keeps its data (ADR 0492).
//! Dukovo's root is on a disk, so that the bundle has room, and a second
//! disk holds the `.dkv` files `ob ports` packed, Valkey's among them.
//! Through elevate, Alice installs it for everyone: authd makes the
//! account valkey, and the service waits until she enables it. Enabled,
//! it runs as valkey on its poll(2) event loop, with its log in the
//! system's. Her valkey-cli asks, with a warning, whether it may run with
//! her access, then reaches it; she stores a string and a list, saves,
//! and stores one more key. Restarted, Valkey reads back all three: the
//! save's two and the third, which it saved as it stopped; a Lua script
//! reads one too. With the append-only file turned on, a key written
//! after it is there after another restart. valkey-check-rdb finds its
//! snapshot sound and valkey-check-aof its append-only files, and
//! valkey-benchmark runs SET and GET with its 50 clients. Its data is the
//! account's. The power button ends it.

use super::super::*;

/// The disk of bundles' label.
const LABEL: &str = "BUNDLES";

/// Where Spark mounts it: the second virtio disk's name.
const VOLUME: &str = "/volumes/vdb";

/// Valkey's bundle file.
const FILE: &str = "io.valkey.valkey-9.1.3.dkv";

/// Its data folder, in its account's.
const DATA: &str = "/shared/services/io.valkey.valkey/valkey/data";

/// Dukovo's root on a copy of the test image, and the bundles.
const DISKS: &[Disk] = &[
    Disk { label: image::ROOT_LABEL, ..btrfs_disk(Bus::Virtio, 512, ROOT, root_checked) },
    bundle_files_disk(128 * 1024, LABEL),
];

/// The test.
pub const CASE: Case = Case {
    name: "valkey",
    about: "Valkey, the catalog's key-value store, keeps its data (ADR 0492): installed for everyone from its .dkv file and enabled, it runs as its own account; what valkey-cli stores is there after a restart, whether saved, saved as it stopped or in the append-only file, and a Lua script reads it; valkey-check-rdb and valkey-check-aof find its files sound, and valkey-benchmark runs with its 50 clients connecting at once (ADR 0494)",
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
    script: valkey,
    disks: DISKS,
    second: None,
};

fn valkey(vm: &mut Vm, case: &Case) -> Result<(), String> {
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
        "valkey, as the account valkey, starting once an administrator enables it",
        // The line goes on past the screen's edge.
        "installed io.valkey.valkey 9.1.3, for everyone; its service valkey starts",
    ])?;
    vm.expect_anywhere("authd: made the account valkey (100) of a service of io.valkey.valkey")?;
    answer_prompt(vm, "$", "clear; elevate service valkey enable; echo \"enable $?\"")?;
    vm.expect_screen(&["enable 0"])?;
    vm.expect_anywhere("Ready to accept connections tcp")?;
    // valkey-cli runs with the access of whoever runs it: its first run
    // asks, with a warning.
    answer_prompt(vm, "$", "clear; valkey-cli ping")?;
    vm.expect_screen(&["[warning] ", "A: always"])?;
    vm.type_text("a")?;
    vm.expect_screen(&["PONG"])?;
    // Saved, and saved as it stops: a restart reads back all three.
    answer_prompt(
        vm,
        "$",
        "clear; valkey-cli set greeting 'hello from Dukovo'; valkey-cli rpush letters a b c; \
         valkey-cli save; valkey-cli set after-save kept",
    )?;
    vm.expect_screen(&["OK", "(integer) 3"])?;
    answer_prompt(vm, "$", "clear; elevate service valkey restart; echo \"restart $?\"")?;
    vm.expect_screen(&["restart 0"])?;
    vm.expect_anywhere("DB loaded from disk")?;
    answer_prompt(
        vm,
        "$",
        "clear; for i in 1 2 3 4 5; do valkey-cli ping > /dev/null 2>&1 && break; sleep 1; done; \
         valkey-cli get greeting; valkey-cli lrange letters 0 -1 | tr '\\n' ' '; echo; valkey-cli \
         get after-save",
    )?;
    vm.expect_screen(&["hello from Dukovo", "a b c", "kept"])?;
    // Its Lua scripts run, in the module built into it.
    answer_prompt(
        vm,
        "$",
        "clear; valkey-cli eval \"return 'lua: ' .. server.call('get', KEYS[1])\" 1 greeting",
    )?;
    vm.expect_screen(&["lua: hello from Dukovo"])?;
    // The append-only file: turned on, it is written whole, and a key
    // written after it is there after another restart, read from it.
    answer_prompt(
        vm,
        "$",
        "clear; valkey-cli config set appendonly yes; valkey-cli config rewrite; for i in 1 2 3 4 \
         5 6 7 8 9 10; do valkey-cli info persistence | grep -q 'aof_rewrite_in_progress:0' && \
         break; sleep 1; done; valkey-cli set logged yes",
    )?;
    vm.expect_screen(&["OK"])?;
    answer_prompt(vm, "$", "clear; elevate service valkey restart; echo \"restart $?\"")?;
    vm.expect_screen(&["restart 0"])?;
    answer_prompt(
        vm,
        "$",
        "clear; for i in 1 2 3 4 5; do valkey-cli ping > /dev/null 2>&1 && break; sleep 1; done; \
         echo \"logged: $(valkey-cli get logged), aof: $(valkey-cli config get appendonly | tail \
         -1)\"",
    )?;
    vm.expect_screen(&["logged: yes, aof: yes"])?;
    // Its files, copied out of its data folder, are sound; they are the
    // account's.
    answer_prompt(
        vm,
        "$",
        &format!(
            "clear; elevate tar -C {DATA} -cf - dump.rdb appendonly | tar -C /tmp -xf -; \
             valkey-check-rdb /tmp/dump.rdb | grep -o 'RDB looks OK'; valkey-check-aof \
             /tmp/appendonly/appendonly.aof.manifest | tail -1; elevate stat -c 'data: %U %G' \
             {DATA}/dump.rdb"
        ),
    )?;
    vm.expect_screen(&[
        "RDB looks OK",
        "All AOF files and manifest are valid",
        "data: valkey valkey",
    ])?;
    // Its 50 clients connect at once, before the server takes any (ADR
    // 0494). Slow under emulation: its next prompt may take a while.
    answer_prompt(
        vm,
        "$",
        "clear; valkey-benchmark -q -n 500 -t set,get | tr '\\r' '\\n' | grep -o -e '^SET:' -e \
         '^GET:' | sort -u | tr '\\n' ' ' > /tmp/bench; echo >> /tmp/bench",
    )?;
    answer_prompt_within(vm, "$", "clear; cat /tmp/bench", 240)?;
    vm.expect_screen(&["GET: SET:"])?;
    // Its log is the system's.
    answer_prompt(
        vm,
        "$",
        "clear; elevate log valkey | grep -q 'Ready to accept connections' && echo 'log holds it'",
    )?;
    vm.expect_screen(&["log holds it"])?;
    vm.press_power_button()?;
    vm.expect("spark: every service has stopped")?;
    vm.expect("Power: `spark` asked to power off the machine; syncing the filesystems")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}

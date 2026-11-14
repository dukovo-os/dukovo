// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! SQLite, the library Dukovo's programs link, works on Radix (ADR
//! 0511). Dukovo's root is on a Btrfs disk, and alice signs in as the
//! owner and runs sqlite-test, a Rust program on rusqlite and
//! libsqlite3-sys that links the sqlite port's library, in a folder of
//! her home. It says the library is 3.54.1, threadsafe and without
//! extension loading; a rollback journal commits and a second connection
//! counts the rows; a reader is not held up by a writer; a second writer
//! waits for its busy timeout and gets SQLITE_BUSY; another process
//! waits for the writer's fcntl locks, gets SQLITE_BUSY, and writes once
//! the writer has committed; WAL in exclusive locking mode checkpoints
//! to an empty log and makes no shared-memory file; and the integrity
//! check, FTS5 and the JSON functions answer. The power button ends it.

use super::super::*;

/// Dukovo's root on a copy of the test image, so that the database is on
/// Btrfs, not in memory.
const DISKS: &[Disk] =
    &[Disk { label: image::ROOT_LABEL, ..btrfs_disk(Bus::Virtio, 512, ROOT, root_checked) }];

/// The test.
pub const CASE: Case = Case {
    name: "sqlite",
    about: "SQLite works on Radix (ADR 0511): sqlite-test, a Rust program on rusqlite linked to the sqlite port's library, checks the version and options, a rollback journal on Btrfs, a reader beside a writer, a second writer's busy timeout, another process taking the database's fcntl locks, WAL in exclusive locking mode, and the integrity check, FTS5 and JSON",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    cmdline: "lockdep",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &[],
    script: sqlite,
    disks: DISKS,
    second: None,
};

/// The lines sqlite-test prints, in order, as the screen shows them.
const LINES: &[&str] = &[
    "sqlite-test: linked SQLite 3.54.1, threadsafe, without extension loading",
    "sqlite-test: a rollback journal on btrfs",
    "sqlite-test: a reader is not held up by a writer",
    "sqlite-test: a second writer waits, then gets SQLITE_BUSY",
    "sqlite-test: another process waits for a writer, then sees its commit",
    "sqlite-test: WAL in exclusive locking mode",
    "sqlite-test: integrity, FTS5 and JSON",
    "sqlite-test: every check passed",
];

fn sqlite(vm: &mut Vm, case: &Case) -> Result<(), String> {
    owner_signs_in(vm, case)?;
    // The program's output goes to a file, with its exit code on a last line,
    // so that the lines are read from the screen when it has ended.
    answer_prompt(
        vm,
        "$",
        "clear; /system/tests/posix/sqlite-test /users/alice/sqlite-check > /tmp/sqlite 2>&1; \
         echo \"exit code $?\" >> /tmp/sqlite",
    )?;
    answer_prompt_within(vm, "$", "clear; cat /tmp/sqlite", 600)?;
    vm.expect_screen(LINES)?;
    vm.expect_screen(&["exit code 0"])?;
    vm.expect_screen_without("sqlite-test: FAILED")?;
    vm.press_power_button()?;
    vm.expect("spark: every service has stopped")?;
    vm.expect("Power: `spark` asked to power off the machine; syncing the filesystems")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}

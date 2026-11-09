// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! *ls*'s own views (ADR 0440), at the console: *--header* titles the
//! long listing's columns and *--octal* puts each mode in octal before
//! its letters. *--field-colors=always* colours the permission letters
//! and an old time even into a pipe, while *--color=always* into a file
//! colours only the names, as GNU's does. *--icons=always* puts a Nerd
//! Font character before each name, and *--icons* alone puts none into a
//! pipe.

use super::super::*;

pub const CASE: Case = Case {
    name: "ls-views",
    about: "ls --header titles the long listing's columns, --octal shows modes as 0640, --field-colors=always colours the permission letters and old times, --color=always into a file colours only names, and --icons puts Nerd Font icons before names only when asked or on a terminal",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    cmdline: "console-sign-in=off",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &[],
    script: ls_views,
    disks: &[],
    second: None,
};

/// The `ls-views` test.
fn ls_views(vm: &mut Vm, _case: &Case) -> Result<(), String> {
    vm.expect("spark: started consoled (/system/commands/consoled) as process ")?;
    vm.expect_screen(&[&format!("{RELEASE} {VERSION} (tty1)"), "# "])?;

    // A folder and a file of mode 0640, last changed in 2020.
    vm.type_text(
        "mkdir -p /tmp/v/Docs && echo hi > /tmp/v/notes.txt && chmod 640 /tmp/v/notes.txt \
         && touch -d 2020-01-01 /tmp/v/notes.txt && echo \"made: $((40 + 2))\"\n",
    )?;
    vm.expect_screen(&["made: 42"])?;

    vm.type_text("ls -l --header --octal /tmp/v | cat\n")?;
    vm.expect_screen(&["Octal Permissions Links Owner Group", "0640  -rw-r-----"])?;

    // `cat -v` shows each escape as ^[: yellow r, red w, faint dashes,
    // and the old time faint.
    vm.type_text("ls -l --field-colors=always /tmp/v/notes.txt | cat -v\n")?;
    vm.expect_screen(&[
        "^[[33mr^[[0m^[[31mw^[[0m^[[90m-^[[0m^[[33mr^[[0m^[[90m-----^[[0m",
        "^[[90mJan  1  2020^[[0m",
    ])?;
    vm.type_text(
        "ls -l --color=always /tmp/v > /tmp/l; echo \"letters in a file: $(grep -c 33m /tmp/l)\"\n",
    )?;
    vm.expect_screen(&["letters in a file: 0"])?;

    // The folder's icon is U+F07B and the text file's U+F15C, in UTF-8.
    vm.type_text("ls --icons=always /tmp/v | od -An -tx1 | head -1\n")?;
    vm.expect_screen(&["ef 81 bb 20 44 6f 63 73 0a ef 85 9c"])?;
    vm.type_text("ls --icons /tmp/v | od -c | head -1\n")?;
    vm.expect_screen(&["D   o   c   s  \\n   n   o   t   e   s"])?;

    vm.type_text("poweroff\n")?;
    vm.expect("spark: asked to power off the machine")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}

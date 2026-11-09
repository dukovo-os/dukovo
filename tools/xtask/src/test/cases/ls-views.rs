// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! *ls*'s own views (ADRs 0440 and 0441), at the console: *--header*
//! titles the long listing's columns and *--octal* puts each mode in
//! octal before its letters. *--field-colors=always* colours the
//! permission letters and an old time even into a pipe, while
//! *--color=always* into a file colours only the names, as GNU's does.
//! *--icons=always* puts a Nerd Font character before each name, and
//! *--icons* alone puts none into a pipe. *--tree* draws a folder's
//! insides beneath it and counts them, *--level* stops it, *--total-size*
//! gives a folder the size of what it holds, and *--git* gives each file
//! its letters in a repository.

use super::super::*;

pub const CASE: Case = Case {
    name: "ls-views",
    about: "ls --header titles the long listing's columns, --octal shows modes as 0640, --field-colors=always colours the permission letters and old times, --color=always into a file colours only names, --icons puts Nerd Font icons before names only when asked or on a terminal, --tree draws a folder's insides to --level, --total-size sizes folders by what they hold and --git shows each file's state in its repository",
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

    // A tree of two folders and three files (ADR 0441).
    vm.type_text(
        "mkdir -p /tmp/t/a/b && printf 12345 > /tmp/t/a/b/c.txt && printf 123 > /tmp/t/a/d.txt \
         && printf 123456 > /tmp/t/e.txt && echo \"made: $((40 + 3))\"\n",
    )?;
    vm.expect_screen(&["made: 43"])?;
    vm.type_text("ls --tree /tmp/t | tail -1; ls --level=1 /tmp/t | tail -1\n")?;
    vm.expect_screen(&["2 folders, 3 files", "1 folder, 1 file"])?;
    // The fourth line, `│   │   └── c.txt`, in UTF-8.
    vm.type_text("ls --tree /tmp/t | head -4 | tail -1 | od -An -tx1 | head -1\n")?;
    vm.expect_screen(&["e2 94 82 20 20 20 e2 94 82 20 20 20 e2 94 94 e2"])?;
    vm.type_text("echo \"a holds: $(ls -ld --total-size /tmp/t/a | cut -d' ' -f5)\"\n")?;
    vm.expect_screen(&["a holds: 8"])?;

    // A repository of it, with e.txt changed, n.txt new and d.txt
    // deleted and staged; a folder takes the letters of what is in it.
    vm.type_text(
        "cd /tmp/t && git init -q -b main . && git add . && git -c user.name=T -c \
         user.email=t@t commit -q -m first && echo more >> e.txt && echo new > n.txt && git rm \
         -q a/d.txt && echo \"ready: $((40 + 4))\"\n",
    )?;
    vm.expect_screen(&["ready: 44"])?;
    vm.type_text("ls -l --git /tmp/t | cat\n")?;
    vm.expect_screen(&["D- a", "-M e.txt", "-N n.txt"])?;

    vm.type_text("poweroff\n")?;
    vm.expect("spark: asked to power off the machine")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}

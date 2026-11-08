// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The install stick's *Install Dukovo (serial console)* entry on a
//! computer with no screen at all (ADR 0318): with *install
//! console=ttyS0* on the command line, consoled finds no screen and
//! stops, and serial-console runs *dukovo-install*'s screens on the first
//! serial port, `/devices/ttyS0`. The test is the terminal at the cable's
//! far end, of 100 columns by 30 rows, which tells serial-console its
//! size when asked, and reads the screens as that terminal shows them.
//! Live console leaves them for root's shell on the port, where the line
//! has that size, TERM=vt220, and *dukovo-install* shows them again.
//! Each page is answered by keys typed on the port, and the review says
//! the system keeps the serial console; Restart on the Done page restarts
//! the computer. The host finds the disk sound, as *install-uefi* does,
//! and the second boot is from that disk alone, still with no screen: its
//! boot menu's *console=ttyS0* brings the sign-in to the port, where
//! alice signs in with the password typed in the screens.

use super::super::*;
use super::install_uefi::INSTALL_DISKS;

pub const CASE: Case = Case {
    name: "install-serial",
    about: "With no screen, the stick's serial console entry runs the installer's screens on ttyS0, read as a 100 by 30 terminal at the far end, which tells its size: Live console and back through root's shell there, every page answered by keys on the port, Restart from Done; the disk boots alone, with the sign-in on the serial port",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    // As the Install Dukovo (serial console) entry gives it: the test
    // image's menu starts Try Dukovo, whose `live console-sign-in=off`
    // come before this.
    cmdline: "install console=ttyS0",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Live,
    cpu_model: None,
    // No video card, so no screen: only the serial port. One network
    // card, so that the network page's fields are in a known order.
    qemu: &["-vga", "none", "-netdev", "user,id=n0", "-device", "virtio-net-pci,netdev=n0"],
    script: screens,
    disks: INSTALL_DISKS,
    second: Some(SecondBoot { cmdline: "", before: super::install_uefi::nothing, script: installed }),
};

/// The size of the terminal at the far end, as a terminal program's
/// window would be: the installer's own size.
const COLS: usize = 100;
const ROWS: usize = 30;

/// alice's password, typed into the screens.
const PASSWORD: &str = "queen of hearts";

/// The computer's name typed into the screens.
const HOSTNAME: &str = "serial";

/// How long the install may take, in seconds: it copies the whole system.
const INSTALL_SECONDS: u64 = 240;

/// How long the screens may take to start: they check the release's
/// signatures first.
const START_SECONDS: u64 = 90;

/// The keys a terminal sends, as the screens read them.
const ENTER: &str = "\r";
const TAB: &str = "\t";
const UP: &str = "\x1b[A";
const DOWN: &str = "\x1b[B";
const BACKSPACE: &str = "\x7f";

/// The kernel's line saying COM1 is a terminal, before any program opens
/// it: the last of the kernel's own the port shows for sure.
const LINE_MADE: &str = "is the terminal /devices/ttyS0, at 115200 baud";

/// The first boot, from the stick: the screens on the serial port,
/// answered by keys typed there.
fn screens(vm: &mut Vm, _case: &Case) -> Result<(), String> {
    vm.expect(LINE_MADE)?;
    // The terminal at the far end, from now on, before serial-console
    // asks its size; GRUB and the firmware are done with the port.
    vm.serial_terminal(COLS, ROWS);
    vm.expect("spark: the live system: /system is the boot archive, in memory, and no disk is the root")?;
    let welcome = [&format!("Install Dukovo {VERSION}") as &str, "Step 1 of 6", "How do you want to install?"];
    vm.expect_terminal_within(&welcome, START_SECONDS)?;

    // Live console, the button three before the first field, leaves the
    // screens for root's shell on the port, which has the terminal's size
    // and type; the shell starts them again.
    vm.send(&[UP, UP, UP, ENTER].concat())?;
    // The terminal's rows end where their text does, without the space
    // after the prompt.
    vm.expect_terminal(&[&format!("{RELEASE} {VERSION} (ttyS0)"), "root@dukovo /"])?;
    // The command line does not hold the answer whole.
    vm.send("echo \"li\"\"ne: $(stty size) $TERM $(tty)\"\r")?;
    vm.expect_terminal(&["line: 30 100 vt220 /devices/ttyS0"])?;
    vm.send("dukovo-install\r")?;
    vm.expect_terminal_within(&welcome, START_SECONDS)?;

    // Network: the computer's name and its time zone, found by the start
    // of its name.
    vm.send(ENTER)?;
    vm.expect_terminal(&["Step 2 of 6", "Network cards found:", "Automatic (DHCP)"])?;
    vm.send(&[DOWN, DOWN, DOWN].concat())?;
    vm.send(&BACKSPACE.repeat(6))?;
    vm.send(HOSTNAME)?;
    vm.send(DOWN)?;
    vm.send("america/denver")?;
    vm.expect_terminal(&["America/Denver"])?;

    // Disk: the blank disk.
    vm.send(ENTER)?;
    vm.expect_terminal(&["Step 3 of 6", "vda", "Erase the disk"])?;
    vm.send(&[" ", ENTER].concat())?;

    // Software: the profile as it starts, and on to Next past the
    // features, the web console and Back.
    vm.expect_terminal(&["Step 4 of 6", "What is this machine for?"])?;
    vm.send(&[DOWN, DOWN, DOWN, TAB, TAB, TAB, ENTER].concat())?;

    // You: the name makes the user name.
    vm.expect_terminal(&["Step 5 of 6", "The administrator account."])?;
    vm.send("Alice Liddell")?;
    vm.send(&[TAB, TAB].concat())?;
    vm.send(PASSWORD)?;
    vm.send(TAB)?;
    vm.send(PASSWORD)?;
    vm.send(ENTER)?;

    // Review: the system keeps the serial console it was installed
    // over, and the disk's name confirms the install.
    vm.expect_terminal(&[
        "Step 6 of 6",
        "Erase     vda",
        "alice (Alice Liddell), administrator",
        "America/Denver · keyboard us · serial console ttyS0",
        "Type vda to confirm",
    ])?;
    vm.send("vda")?;
    vm.send(ENTER)?;
    vm.expect_terminal(&["Installing Dukovo"])?;
    vm.expect_terminal_within(&["Remove the install stick, then restart."], INSTALL_SECONDS)?;
    vm.expect_terminal(&["Dukovo is installed"])?;

    // Restart, the Done page's first button, restarts the computer,
    // which ends QEMU under -no-reboot.
    vm.send(ENTER)?;
    vm.expect_exit(QEMU_EXIT_RESET)
}

/// The second boot, from the installed disk alone, with no screen: the
/// menu's `console=ttyS0` brings the sign-in to the serial port.
fn installed(vm: &mut Vm, _case: &Case) -> Result<(), String> {
    vm.expect(LINE_MADE)?;
    vm.serial_terminal(COLS, ROWS);
    vm.expect("spark: /system is Dukovo's root on ")?;
    vm.expect_terminal_within(&[&format!("{RELEASE} {VERSION} (ttyS0)"), "Name:"], 60)?;
    vm.send("alice\r")?;
    vm.expect_terminal(&["Password:"])?;
    vm.send(&format!("{PASSWORD}\r"))?;
    vm.expect_terminal(&[&format!("alice@{HOSTNAME} ~")])?;

    // The installed menu started the kernel with the console's flag, the
    // line has the terminal's size, and the answers kept the choice.
    vm.send(
        "echo \"fl\"\"ag: $(grep -c console=ttyS0 /kernel/cmdline) $(stty size) $TERM $(tty)\"\r",
    )?;
    vm.expect_terminal(&["flag: 1 30 100 vt220 /devices/ttyS0"])?;
    vm.send(
        "echo \"ans\"\"wers: $(grep -c '\"serial_console\": true' \
         /system/settings/install/answers.json) $(cat /system/settings/hostname)\"\r",
    )?;
    vm.expect_terminal(&[&format!("answers: 1 {HOSTNAME}")])?;
    // authd recorded the sign-in as on the serial port.
    vm.send("echo \"la\"\"st: $(last -n 1 alice | grep -c ' ttyS0 ')\"\r")?;
    vm.expect_terminal(&["last: 1"])?;

    vm.press_power_button()?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}

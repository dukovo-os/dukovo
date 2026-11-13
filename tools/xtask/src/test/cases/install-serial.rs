// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The install stick's *Install Dukovo (serial console)* entry on a
//! computer with no screen at all, and two serial ports, COM1 and COM2
//! (ADRs 0318 and 0481): with *install serial-console=any* on the command
//! line, consoled finds no screen and stops, and serial-console offers
//! the installer on every serial port. COM1's terminal and COM2's socket
//! each show `Press Enter to install Dukovo on this port`, with their own
//! names, and the installer's screens come up on the port Enter is
//! pressed on, COM1, `/devices/ttyS0`. Enter on COM2 then says that the
//! installer is running on ttyS0 and gives root's shell there. The test
//! is the terminal at COM1's cable's far end, of 100 columns by 30 rows,
//! which tells serial-console its size when asked, and reads the screens
//! as that terminal shows them. Live console leaves them for root's shell
//! on the port, where the line has that size, TERM=vt220, and
//! *dukovo-install* shows them again. Each page is answered by keys typed
//! on the port, and the review says the system keeps the serial console;
//! Restart on the Done page restarts the computer. The host finds the
//! disk sound, as *install-uefi* does, and the second boot is from that
//! disk alone, still with no screen: the serial console's settings the
//! installer wrote beside the boot menu give the kernel `console=ttyS0,
//! 115200n8`, which only GRUB reading them can do, and bring the sign-in
//! to the port, where alice signs in with the password typed in the
//! screens. There *serial* lists the port as the primary console, and
//! with alice's password it narrows the kernel log to ttyS0, shows the
//! menu on COM1, and refuses a port the computer lacks.

use super::super::*;
use super::install_uefi::INSTALL_DISKS;

pub const CASE: Case = Case {
    name: "install-serial",
    about: "With no screen and two serial ports, the stick's serial console entry offers the installer on each and runs its screens on the one Enter is pressed on, read as a 100 by 30 terminal at the far end, which tells its size, while the other port says where the installer runs and has root's shell: Live console and back through root's shell, every page answered by keys on the port, Restart from Done; the disk boots alone, with the console flags from the serial settings the installer wrote, the sign-in on the serial port, and serial listing and changing them",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    // As the Install Dukovo (serial console) entry gives it: the test
    // image's menu starts Try Dukovo, whose `live console-sign-in=off`
    // come before this.
    cmdline: "install serial-console=any",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Live,
    cpu_model: None,
    // No video card, so no screen: only the serial ports, COM2 a socket
    // the test reads and types on. One network card, so that the network
    // page's fields are in a known order.
    qemu: &[
        "-vga",
        "none",
        "-chardev",
        SERIAL_LINE[0],
        "-device",
        "isa-serial,index=1,chardev=line0",
        "-netdev",
        "user,id=n0",
        "-device",
        "virtio-net-pci,netdev=n0",
    ],
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

/// The first boot, from the stick: the screens on the serial port Enter
/// is pressed on, answered by keys typed there.
fn screens(vm: &mut Vm, case: &Case) -> Result<(), String> {
    // QEMU listens on COM2's socket from its start, before the firmware
    // runs; what the port sends before the test connects is lost.
    let line = vm.dir().join(format!("{}.line", case.name));
    let mut com2 = SocketLine::connect(&line, "COM2", vm.deadline)?;
    vm.expect(LINE_MADE)?;
    // The terminal at the far end, from now on, before serial-console
    // asks its size; GRUB and the firmware are done with the port.
    vm.serial_terminal(COLS, ROWS);
    vm.expect("spark: the live system: /system is the boot archive, in memory, and no disk is the root")?;
    // Each port offers the installer, by its own name, and runs nothing
    // until Enter is pressed on it.
    vm.expect_terminal_within(
        &["Press Enter to install Dukovo on this port (ttyS0)."],
        START_SECONDS,
    )?;
    com2.expect("Press Enter to install Dukovo on this port (ttyS1).")?;
    vm.send(ENTER)?;
    let welcome = [
        &format!("Install Dukovo {VERSION} (serial console)") as &str,
        "Step 1 of 6 - Welcome",
        "How do you want to install?",
    ];
    vm.expect_terminal_within(&welcome, START_SECONDS)?;

    // The installer runs on COM1 now: Enter on COM2 says so, and COM2 has
    // root's shell. The command line does not hold the answer whole.
    com2.send(ENTER)?;
    com2.expect("The installer is running on ttyS0.")?;
    com2.expect_prompt()?;
    com2.send("echo \"sec\"\"ond: $(tty)\"\r")?;
    com2.expect("second: /devices/ttyS1\r\n")?;
    com2.expect_prompt()?;

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
    // over, and the disk's name confirms the install. On the port the
    // screens are plain ASCII, the dots between the settings dashes.
    vm.expect_terminal(&[
        "Step 6 of 6",
        "Erase     vda",
        "alice (Alice Liddell), administrator",
        "America/Denver - keyboard us - serial console ttyS0",
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

    // The installed menu started the kernel with the console's flag from
    // the serial settings beside it, which give the line too: the menu
    // alone would have given `console=ttyS0`, with no options. The line
    // has the terminal's size, and the answers kept the choice.
    vm.send(
        "echo \"fl\"\"ag: $(grep -c console=ttyS0,115200n8 /kernel/cmdline) $(stty size) $TERM \
         $(tty)\"\r",
    )?;
    vm.expect_terminal(&["flag: 1 30 100 vt220 /devices/ttyS0"])?;
    vm.send(
        "echo \"ans\"\"wers: $(grep -c '\"serial_console\": true' \
         /system/settings/install/answers.json) $(cat /system/settings/hostname)\"\r",
    )?;
    vm.expect_terminal(&[&format!("answers: 1 {HOSTNAME}")])?;
    // serial lists the console as the primary one; the settings the next
    // start reads are on the EFI system partition, which only root reads.
    vm.send("clear; serial\r")?;
    vm.expect_terminal(&["primary, 115200n8", "only root reads the boot menu's serial"])?;
    // An administrator narrows the log to ttyS0, and serial then says so,
    // with the menu on COM1 at its speed; a port the computer lacks is
    // refused. One elevated shell, so that one password prompt is on the
    // screen: a second would match the first's text still there.
    vm.send(
        "clear; elevate -k sh -c 'serial log ttyS0 > /dev/null; serial; serial console ttyS9'\r",
    )?;
    vm.expect_terminal(&["password for alice:"])?;
    vm.send(&format!("{PASSWORD}\r"))?;
    vm.expect_terminal(&["Kernel log: ttyS0", "Boot menu:  com0 at 115200 baud", "no ttyS9"])?;
    // authd recorded the sign-in as on the serial port.
    vm.send("echo \"la\"\"st: $(last -n 1 alice | grep -c ' ttyS0 ')\"\r")?;
    vm.expect_terminal(&["last: 1"])?;

    vm.press_power_button()?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}

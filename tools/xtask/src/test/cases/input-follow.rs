// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Input follows the screens (ADR 0706), on a machine with a PS/2
//! keyboard, a USB keyboard and a USB mouse, whose consoles consoled
//! shows. display-test (userspace/programs/display-test) runs as
//! `display-test input`: the test tells it what to do on a serial port
//! of its own, COM2, and it says what each of its descriptions of the
//! input files read. It borrows the screens from consoled, as a desktop
//! does, and ties a description of every device's file, of
//! _/devices/kbd_ and of _/devices/mouse_ to its holder. While that
//! holder is in front, what is tied to it reads both keyboards and the
//! mouse. Lent on to a second holder, the screens take the keys with
//! them: the first holder's descriptions read `paused` and no key, and
//! the second's read them. A key held down while the screens pass is
//! let go for the holder that read it go down, whose device then says
//! that nothing is down, and the holder that comes to the front reads
//! nothing of it, not even its release. What was tied to a holder that
//! ended reads nothing more. A description tied to nobody reads
//! everything throughout. Given back, the screens show the consoles,
//! and the shell there has read none of the keys typed while the
//! program had the screens.

use super::super::*;

/// The display the USB keyboard and mouse are bound to, so that QMP's
/// input-send-event reaches them and send-key the PS/2 keyboard.
pub(super) const VIDEO: &str = "video0";

/// The machine of the tests of who hears a key: the USB keyboard and
/// mouse of `usb-input`, and COM2 on a socket, where the test tells
/// display-test what to do.
pub(super) const MACHINE: &[&str] = &[
    "-vga",
    "none",
    "-device",
    "VGA,id=video0",
    "-device",
    "usb-kbd,bus=xhci.0,port=2,id=usbkbd,display=video0",
    "-device",
    "usb-mouse,bus=xhci.0,port=3",
    "-chardev",
    SERIAL_LINE[0],
    "-device",
    "isa-serial,index=1,chardev=line0",
];

/// The command line of those tests: a normal boot, and display-test
/// told what to do on COM2.
pub(super) const CMDLINE: &str = "lockdep console-sign-in=off -- --services --posix /system/tests/posix/display-test input /devices/ttyS1";

/// The test.
pub const CASE: Case = Case {
    name: "input-follow",
    about: "input follows the screens (ADR 0706): what a program ties to its holder of the screens reads the keyboards and the mouse only while that holder is in front, a key held while the screens pass is let go for the one and never reaches the next, what was tied to a holder that ended reads nothing more, a reader tied to nobody reads everything, and the consoles' shell reads none of what was typed while a program had the screens",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    cmdline: CMDLINE,
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: MACHINE,
    script: input_follow,
    disks: &[],
    second: None,
};

/// How display-test's line of failure starts.
const FAILED: &str = "display-test: FAILED";

/// What display-test says after the last line of an answer of several.
pub(super) const ALL: &str = "input: that is all";

/// display-test, told what to do on COM2.
pub(super) struct Told {
    line: SocketLine,
}

impl Told {
    /// Connects to COM2, and waits for the boot to end, for the two USB
    /// devices, for display-test to read the port and for the consoles.
    pub(super) fn start(vm: &mut Vm, case: &Case) -> Result<Told, String> {
        let found = [
            "USB: 1-6: interface 0: a keyboard, read into /devices/kbd",
            "USB: 1-7: interface 0: a mouse, read into /devices/mouse",
        ];
        Told::start_with(vm, case, &found)
    }

    /// As [`Told::start`], on a machine whose input devices the kernel's
    /// log names with the lines `found`.
    pub(super) fn start_with(vm: &mut Vm, case: &Case, found: &[&str]) -> Result<Told, String> {
        // QEMU listens on COM2's socket from its start.
        let line = vm.dir().join(format!("{}.line", case.name));
        let line = SocketLine::connect(&line, "COM2", vm.deadline)?;
        boot_report(vm, case)?;
        vm.expect("Boot complete.")?;
        for found in found.iter().chain(&["input: ready"]) {
            vm.expect_anywhere(found)?;
        }
        vm.expect_screen(&[&format!("{RELEASE} {VERSION} (tty1)"), "# "])?;
        Ok(Told { line })
    }

    /// Waits for display-test's line `wanted`, and fails at once if it
    /// says that it failed.
    pub(super) fn says(vm: &mut Vm, wanted: &str) -> Result<(), String> {
        match vm.expect_first(&[wanted, FAILED])? {
            0 => Ok(()),
            _ => Err(format!("display-test failed{}", program_line(vm, "")?)),
        }
    }

    /// Tells display-test `word`, and waits for it to say `done`.
    /// Returns what the log gained meanwhile.
    pub(super) fn tell(&mut self, vm: &mut Vm, word: &str, done: &str) -> Result<String, String> {
        let from = vm.cursor;
        self.line.send(&format!("{word}\n"))?;
        Told::says(vm, done).map_err(|why| format!("after `{word}`: {why}"))?;
        Ok(vm.log().get(from..vm.cursor).unwrap_or_default().to_string())
    }

    /// Tells display-test `word`, waits for it to say `done`, and fails
    /// unless it said each of `lines` before, in any order.
    pub(super) fn tell_all(
        &mut self,
        vm: &mut Vm,
        word: &str,
        lines: &[&str],
        done: &str,
    ) -> Result<(), String> {
        let said = self.tell(vm, word, done)?;
        match lines.iter().find(|line| !said.lines().any(|said| said.trim_end().ends_with(*line))) {
            None => Ok(()),
            Some(line) => {
                Err(format!("after `{word}`, display-test did not say `{line}`:\n{said}"))
            }
        }
    }

    /// Asks what each description read since it was last asked, and
    /// fails unless each one named in `wanted` read just what is said
    /// of it there.
    pub(super) fn heard(&mut self, vm: &mut Vm, wanted: &[(&str, &str)]) -> Result<(), String> {
        let said = self.tell(vm, "heard", ALL)?;
        for (who, what) in wanted {
            let start = format!("input: {who} heard: ");
            let read = said.lines().find_map(|line| Some(line.split_once(&start)?.1.trim_end()));
            if read != Some(what) {
                let read = read.unwrap_or("(no line for it)");
                return Err(format!("{who} heard `{read}`, not `{what}`"));
            }
        }
        Ok(())
    }

    /// Tells display-test to end, and fails unless it passed.
    pub(super) fn end(mut self, vm: &mut Vm) -> Result<(), String> {
        self.line.send("quit\n")?;
        screens::passed(vm)
    }
}

/// QMP's event of a pointer that moved `value` along `axis`.
pub(super) fn rel(axis: &str, value: i32) -> String {
    format!(r#"{{"type":"rel","data":{{"axis":"{axis}","value":{value}}}}}"#)
}

/// QMP's event of a pointer that points at `value` of 0x7fff along
/// `axis`.
pub(super) fn abs(axis: &str, value: i32) -> String {
    format!(r#"{{"type":"abs","data":{{"axis":"{axis}","value":{value}}}}}"#)
}

/// QMP's event of a pointer's `button` going down, or up.
pub(super) fn button(button: &str, down: bool) -> String {
    format!(r#"{{"type":"btn","data":{{"down":{down},"button":"{button}"}}}}"#)
}

fn input_follow(vm: &mut Vm, case: &Case) -> Result<(), String> {
    let mut told = Told::start(vm, case)?;
    told.tell(vm, "borrow", "input: ours has the screens")?;
    // In front: a (30) on the USB keyboard, b (48) on the PS/2 one, and
    // the mouse.
    vm.type_on(VIDEO, "a")?;
    vm.type_text("b")?;
    vm.input_events(VIDEO, &[rel("x", 10), rel("y", -5)])?;
    told.heard(
        vm,
        &[
            ("kbd, tied to nobody", "+30 -30 +48 -48"),
            ("mouse, tied to nobody", "10,-5,0,0,0,0,0,0"),
            ("kbd, tied to ours", "+30 -30 +48 -48"),
            ("mouse, tied to ours", "10,-5,0,0,0,0,0,0"),
            ("PS/2 keyboard, tied to ours", "+48 -48"),
            ("QEMU USB Keyboard, tied to ours", "+30 -30"),
            ("QEMU USB Mouse, tied to ours", "rel0:10 rel1:-5"),
        ],
    )?;
    // Lent on, the screens take the keys with them: c (46).
    told.tell(vm, "lend", "input: theirs has the screens")?;
    vm.type_on(VIDEO, "c")?;
    vm.input_events(VIDEO, &[rel("x", 3)])?;
    told.heard(
        vm,
        &[
            ("kbd, tied to nobody", "+46 -46"),
            ("mouse, tied to nobody", "3,0,0,0,0,0,0,0"),
            ("kbd, tied to ours", "nothing"),
            ("mouse, tied to ours", "nothing"),
            ("PS/2 keyboard, tied to ours", "paused"),
            ("QEMU USB Keyboard, tied to ours", "paused"),
            ("QEMU USB Mouse, tied to ours", "paused"),
            ("kbd, tied to theirs", "+46 -46"),
            ("mouse, tied to theirs", "3,0,0,0,0,0,0,0"),
        ],
    )?;
    told.tell(vm, "back", "input: ours has the screens again")?;
    held_while_the_screens_pass(vm, &mut told)?;
    // The second holder ends with the screens: d (32) is the first
    // holder's, and what was tied to the second reads nothing more.
    told.tell(vm, "end", "input: theirs ended, and ours has the screens again")?;
    vm.type_on(VIDEO, "d")?;
    told.heard(vm, &[("kbd, tied to ours", "+32 -32"), ("kbd, tied to theirs", "nothing")])?;
    // Given back, the screens show the consoles, whose shell read none
    // of a, b, c, x and d: the line typed now is all there is on it.
    told.tell(vm, "give", "input: the consoles have the screens")?;
    vm.type_on(VIDEO, "echo $((6 * 7)) usb\n")?;
    vm.expect_screen(&["# echo $((6 * 7)) usb", "42 usb"])?;
    vm.type_text("echo $((6 * 8)) ps2\n")?;
    vm.expect_screen(&["# echo $((6 * 8)) ps2", "48 ps2"])?;
    told.end(vm)
}

/// x (45) is held down on the USB keyboard while the screens are lent
/// on: the holder that read it go down reads it let go, and its
/// keyboard says that nothing is down; the holder that comes to the
/// front reads nothing of the key, not even its release.
fn held_while_the_screens_pass(vm: &mut Vm, told: &mut Told) -> Result<(), String> {
    let keyboard = "input: QEMU USB Keyboard, tied to ours has down:";
    vm.keys_on(VIDEO, &["x"], true)?;
    std::thread::sleep(Duration::from_millis(300));
    told.tell_all(vm, "down", &[&format!("{keyboard} 45")], ALL)?;
    told.tell(vm, "lend", "input: theirs has the screens")?;
    told.tell_all(vm, "down", &[&format!("{keyboard} nothing")], ALL)?;
    // Long enough to repeat, for the reader tied to nobody alone.
    std::thread::sleep(Duration::from_millis(600));
    vm.keys_on(VIDEO, &["x"], false)?;
    told.heard(
        vm,
        &[
            ("kbd, tied to nobody", "+45 -45"),
            ("kbd, tied to ours", "+45 -45"),
            ("QEMU USB Keyboard, tied to ours", "resumed +45 paused"),
            ("kbd, tied to theirs", "nothing"),
        ],
    )
}

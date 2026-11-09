// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A braille display: QEMU's `usb-braille`, a Baum SuperVario 40 as an
//! FTDI chip under Baum's IDs, whose serial line the test answers on as
//! the display, in Baum's protocol. The kernel makes it a terminal;
//! brailled finds it by its IDs and starts BRLTTY with its Baum driver,
//! which reads the consoles from consoled through the screen driver for
//! Dukovo: the display shows the line the cursor is on, in North American
//! Braille Computer Code. A routing key moves the shell's cursor, by the
//! keys BRLTTY types through consoled; the display keys pan along a long
//! line; consoled's Ctrl+Alt+Delete menu shows on its own last row, with
//! the cursor; and the display unplugged stops BRLTTY (ADR 0326).

use std::io::{ErrorKind, Read, Write};
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

use super::super::*;

pub const CASE: Case = Case {
    name: "braille",
    about: "a Baum braille display on QEMU's usb-braille, answered by the test: brailled starts BRLTTY for it, it shows the cursor's line, its routing key moves the shell's cursor, its display keys pan, and consoled's menu shows on its own row",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    cmdline: "lockdep console-sign-in=off",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &["-chardev", BRAILLE_LINE, "-device", "usb-braille,bus=xhci.0,chardev=braille,id=display"],
    script: braille,
    disks: &[],
    second: None,
};

/// The byte that starts each of Baum's packets; one in a packet's data is
/// sent twice.
const ESC: u8 = 0x1b;

/// The display's cells.
const CELLS: usize = 40;

/// Baum's packets: what BRLTTY asks (display data, which with one byte
/// asks for the cell count, and the device's identity) and the keys'.
const DISPLAY_DATA: u8 = 0x01;
const DEVICE_IDENTITY: u8 = 0x84;
const DISPLAY_KEYS: u8 = 0x24;
const ROUTING_KEYS: u8 = 0x22;

/// The display keys that pan: Display2 back a window, Display5 on.
const DISPLAY2: u8 = 0x02;
const DISPLAY5: u8 = 0x10;

/// How long the test waits for the display to show something.
const SHOW_WAIT: Duration = Duration::from_secs(60);

/// The display's dots for the lowercase letters, as literary braille and
/// North American Braille Computer Code have them: dot 1 is the lowest
/// bit, as ISO 11548-1 and Baum's protocol number them.
const LETTERS: [u8; 26] = [
    0x01, 0x03, 0x09, 0x19, 0x11, 0x0b, 0x1b, 0x13, 0x0a, 0x1a, 0x05, 0x07, 0x0d, 0x1d, 0x15, 0x0f,
    0x1f, 0x17, 0x0e, 0x1e, 0x25, 0x27, 0x3a, 0x2d, 0x3d, 0x35,
];
/// The digits 0 to 9 in that code: the letters a to j a row lower.
const DIGITS: [u8; 10] = [0x34, 0x02, 0x06, 0x12, 0x32, 0x22, 0x16, 0x36, 0x26, 0x14];
/// Dot 7, which makes a capital, and dot 8, which with dot 7 BRLTTY shows
/// the cursor with.
const DOT7: u8 = 0x40;
const DOT8: u8 = 0x80;

/// The character a cell shows, in that code: letters, digits, the
/// space and a few signs, and `?` for anything else. The cursor's dots
/// are left out.
fn character(cell: u8) -> char {
    let cell = if cell & DOT8 != 0 { cell & !(DOT7 | DOT8) } else { cell };
    let (dots, capital) = (cell & !DOT7, cell & DOT7 != 0);
    if let Some(at) = LETTERS.iter().position(|&l| l == dots) {
        let letter = char::from(b'a' + at as u8);
        return if capital { letter.to_ascii_uppercase() } else { letter };
    }
    if capital {
        return '?';
    }
    if let Some(at) = DIGITS.iter().position(|&d| d == dots) {
        return char::from(b'0' + at as u8);
    }
    match dots {
        0x00 => ' ',
        0x3c => '#',
        0x29 => '%',
        0x24 => '-',
        0x2c => '+',
        0x28 => '.',
        0x0c => '/',
        _ => '?',
    }
}

/// The display, as the test plays it on its line.
struct Display {
    line: UnixStream,
    /// What came that is not yet a whole packet.
    pending: Vec<u8>,
    /// What the cells show.
    cells: [u8; CELLS],
}

impl Display {
    /// Plugs the display in: connects to its line, which QEMU listens on.
    fn plug_in(path: &Path) -> Result<Display, String> {
        let line = UnixStream::connect(path).map_err(|e| format!("{}: {e}", path.display()))?;
        line.set_read_timeout(Some(Duration::from_millis(50)))
            .map_err(|e| format!("braille line: {e}"))?;
        Ok(Display { line, pending: Vec::new(), cells: [0; CELLS] })
    }

    /// Sends a packet, its ESCs doubled.
    fn send(&mut self, packet: &[u8]) -> Result<(), String> {
        let mut bytes = vec![ESC];
        for &byte in packet {
            bytes.push(byte);
            if byte == ESC {
                bytes.push(ESC);
            }
        }
        self.line.write_all(&bytes).map_err(|e| format!("braille line: {e}"))
    }

    /// Reads what BRLTTY sent until the line is quiet, answers what it
    /// asks and keeps what the cells are to show.
    fn serve(&mut self) -> Result<(), String> {
        let mut buffer = [0u8; 1024];
        loop {
            match self.line.read(&mut buffer) {
                Ok(0) => return Err("QEMU closed the braille display's line".into()),
                Ok(n) => self.pending.extend_from_slice(&buffer[..n]),
                Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => break,
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(e) => return Err(format!("braille line: {e}")),
            }
        }
        for packet in take_packets(&mut self.pending) {
            match packet.split_first() {
                Some((&DEVICE_IDENTITY, [])) => {
                    let mut answer = vec![DEVICE_IDENTITY];
                    answer.extend_from_slice(b"SuperVario 40   ");
                    self.send(&answer)?;
                }
                Some((&DISPLAY_DATA, [_])) => self.send(&[DISPLAY_DATA, CELLS as u8])?,
                Some((&DISPLAY_DATA, cells)) if cells.len() == CELLS => {
                    self.cells.copy_from_slice(cells);
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// What the cells show.
    fn text(&self) -> String {
        self.cells.iter().map(|&cell| character(cell)).collect()
    }

    /// Serves the line until the cells show `needle`; says what they show.
    fn expect(&mut self, needle: &str) -> Result<String, String> {
        let until = Instant::now() + SHOW_WAIT;
        loop {
            self.serve()?;
            let text = self.text();
            if text.contains(needle) {
                return Ok(text);
            }
            if Instant::now() >= until {
                return Err(format!("the braille display shows {text:?}, not {needle:?}"));
            }
        }
    }

    /// Presses and lets go of the display keys `keys`.
    fn press(&mut self, keys: u8) -> Result<(), String> {
        self.send(&[DISPLAY_KEYS, keys])?;
        self.serve()?;
        self.send(&[DISPLAY_KEYS, 0])
    }

    /// Presses and lets go of the routing key over cell `cell`.
    fn route(&mut self, cell: usize) -> Result<(), String> {
        let mut keys = [0u8; 1 + CELLS / 8];
        keys[0] = ROUTING_KEYS;
        keys[1 + cell / 8] = 1 << (cell % 8);
        self.send(&keys)?;
        self.serve()?;
        keys[1 + cell / 8] = 0;
        self.send(&keys)
    }
}

/// The whole packets at the start of `bytes`, each without its ESC and
/// with its doubled ESCs single, taken out of it. A packet ends where the
/// next one's ESC starts or, the line being quiet, where `bytes` do; a
/// last ESC alone may be the first of two, and waits.
fn take_packets(bytes: &mut Vec<u8>) -> Vec<Vec<u8>> {
    let mut packets = Vec::new();
    let mut packet: Option<Vec<u8>> = None;
    let mut at = 0;
    while at < bytes.len() {
        match (bytes[at], bytes.get(at + 1)) {
            (ESC, Some(&ESC)) if packet.is_some() => {
                if let Some(packet) = packet.as_mut() {
                    packet.push(ESC);
                }
                at += 2;
            }
            (ESC, None) => break,
            (ESC, Some(_)) => {
                packets.extend(packet.replace(Vec::new()));
                at += 1;
            }
            (byte, _) => {
                if let Some(packet) = packet.as_mut() {
                    packet.push(byte);
                }
                at += 1;
            }
        }
    }
    packets.extend(packet);
    bytes.drain(..at);
    packets
}

/// The `braille` case.
fn braille(vm: &mut Vm, case: &Case) -> Result<(), String> {
    boot_report(vm, case)?;
    vm.expect("Boot complete.")?;
    vm.expect_anywhere("brailled: looking for a braille display")?;
    vm.expect("consoled: 6 consoles; tty1 is shown")?;
    vm.expect_screen(&[&format!("{RELEASE} {VERSION} (tty1)"), "# "])?;
    // Plugged in, it is a terminal, which brailled finds by its IDs.
    let mut display = Display::plug_in(&braille_line(vm.dir(), case.name))?;
    vm.expect_anywhere("(0403:fe72")?;
    vm.expect_anywhere("interface 0: FT232BM, the terminal /devices/ttyUSB0")?;
    vm.expect_anywhere(
        "brailled: found Baum SuperVario 40 on /devices/ttyUSB0: starting BRLTTY with its bm driver",
    )?;
    vm.expect_anywhere("consoled: braille display connected")?;
    // A short prompt, so that the lines below fit the display's 40 cells
    // as the test reads them; nukesh's own, with the user, host and path,
    // takes 22 of them.
    vm.type_text("PS1='% '\n")?;
    // The line the cursor is on.
    vm.type_text("echo dukovo braille")?;
    let shown = display.expect("% echo dukovo braille")?;
    // The routing key over the d moves the cursor there, by the keys
    // BRLTTY types: what is typed next goes before it.
    let d = shown.find("dukovo").ok_or("no dukovo")?;
    display.route(d)?;
    display.expect("% echo dukovo braille")?;
    std::thread::sleep(Duration::from_secs(2));
    display.serve()?;
    vm.type_text("x\n")?;
    vm.expect_screen(&["xdukovo braille"])?;
    // A line longer than the display: the window with the cursor shows
    // its end; Display2 pans back to its start, and Display5 on again.
    vm.type_text("echo the-braille-display-pans-along-a-line-longer-than-itself")?;
    display.expect("longer-than-itself")?;
    display.press(DISPLAY2)?;
    display.expect("% echo the-braille")?;
    display.press(DISPLAY5)?;
    display.expect("longer-than-itself")?;
    vm.press(&["ctrl-u"])?;
    // consoled's own row, below the console's: its menu, with the
    // cursor, which BRLTTY follows there, on its first letter, whose dots
    // 7 and 8 hide that it is a capital.
    vm.press(&["ctrl-alt-delete"])?;
    display.expect("trl+Alt+Delete")?;
    vm.press(&["esc"])?;
    display.expect("% ")?;
    // Unplugged, BRLTTY is stopped.
    vm.monitor("device_del display")?;
    drop(display);
    vm.expect_anywhere("brailled: Baum SuperVario 40 was unplugged: stopping BRLTTY")?;
    vm.type_text("braille\n")?;
    vm.expect_screen(&["Braille: on", "Display: none plugged in"])?;
    vm.type_text("poweroff\n")?;
    vm.expect("spark: asked to power off the machine")?;
    vm.expect("spark: every service has stopped")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn baums_packets_are_taken_whole_with_their_escs_single() {
        let mut bytes = vec![ESC, 0x84, ESC, 0x01, 0x00, ESC, 0x01, 1, ESC, ESC, 2, ESC];
        let packets = take_packets(&mut bytes);
        assert_eq!(packets, [vec![0x84], vec![0x01, 0x00], vec![0x01, 1, ESC, 2]]);
        // The ESC left may be the first of two.
        assert_eq!(bytes, [ESC]);
        bytes.extend([0x08]);
        assert_eq!(take_packets(&mut bytes), [vec![0x08]]);
    }

    #[test]
    fn cells_read_as_north_american_braille_computer_code() {
        let cells = [0x29, 0x3c, 0x00, 0x11, 0x09, 0x13, 0x15, 0x00, 0x02, 0x34, 0x24, 0x43, 0x99];
        let text: String = cells.iter().map(|&c| character(c)).collect();
        // The last cell has the cursor's dots 7 and 8 over the d.
        assert_eq!(text, "%# echo 10-Bd");
    }
}

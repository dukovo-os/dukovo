// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! QEMU's two-port PCI serial card, `pci-serial-2x`: 16550As on PCI, as
//! the ports of the lab's ASIX AX99100 card and Intel AMT's
//! Serial-over-LAN port show themselves. The kernel finds both ports in
//! the card's I/O BAR and makes them consoles beside COM1 (ADR 0360).
//! The first port, a Unix socket the test reads and types on, gets the
//! whole boot log, with what was logged before the card was found
//! replayed, in `\r\n` lines, and `kdb` answers a command typed on it.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::sync::{Arc, Mutex};

use super::super::*;

pub const CASE: Case = Case {
    name: "pci-serial",
    about: "a two-port PCI serial card: the kernel log from its start, and kdb, on its ports",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    cmdline: "qemu-exit kdb lockdep",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &["-chardev", SERIAL_LINE[0], "-device", "pci-serial-2x,chardev1=line0"],
    script: pci_serial,
    disks: &[],
    second: None,
};

/// The `pci-serial` case.
fn pci_serial(vm: &mut Vm, case: &Case) -> Result<(), String> {
    // QEMU listens on the port's socket from its start, before the
    // firmware runs; what the port sends before the test connects is lost.
    let mut port = Port::connect(&vm.dir().join(format!("{}.line", case.name)), vm.deadline)?;
    boot_report(vm, case)?;
    vm.expect_anywhere("Serial: QEMU PCI 16550A adapter at 0000:00:")?;
    vm.expect_anywhere(
        ": 2 ports at 115200 baud, 8N1; the kernel log and kdb are on them",
    )?;
    vm.expect("Boot complete.")?;
    vm.expect("kdb on CPU 0, boot complete")?;
    vm.expect("kdb> ")?;
    // The log from its first line, replayed when the card was found.
    port.expect(&format!("Radix {VERSION} ({RELEASE} {VERSION})\r\n"))?;
    port.expect("KASLR: kernel moved by 0x")?;
    port.expect("Serial: QEMU PCI 16550A adapter at 0000:00:")?;
    // And what was logged after, which the `serial` thread sends.
    port.expect("Boot complete.\r\n")?;
    port.expect("kdb on CPU 0, boot complete")?;
    port.expect("kdb> ")?;
    // `kdb` answers a command typed on the port, on the port and on COM1.
    port.send("cpus\r")?;
    port.expect("2 CPUs")?;
    port.expect("kdb> ")?;
    vm.expect("2 CPUs")?;
    vm.expect("kdb> ")?;
    port.send("continue\r")?;
    port.expect("kdb: continuing")?;
    vm.expect("kdb: continuing")?;
    vm.expect_exit(QEMU_EXIT_SUCCESS)
}

/// The card's first port, through the Unix socket QEMU listens on for it.
struct Port {
    stream: UnixStream,
    /// What the port sent so far.
    output: Arc<Mutex<Vec<u8>>>,
    /// End of the last match.
    cursor: usize,
    deadline: Instant,
}

impl Port {
    fn connect(path: &Path, deadline: Instant) -> Result<Port, String> {
        let stream = loop {
            match UnixStream::connect(path) {
                Ok(stream) => break stream,
                Err(err) if Instant::now() >= deadline => {
                    return Err(format!("{}: {err}", path.display()));
                }
                Err(_) => std::thread::sleep(Duration::from_millis(20)),
            }
        };
        let mut reader = stream.try_clone().map_err(|err| err.to_string())?;
        let output = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&output);
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            while let Ok(n @ 1..) = reader.read(&mut buf) {
                sink.lock().expect("the port's lock is not poisoned").extend_from_slice(&buf[..n]);
            }
        });
        Ok(Port { stream, output, cursor: 0, deadline })
    }

    /// Waits until the port has sent `needle` after the last match.
    fn expect(&mut self, needle: &str) -> Result<(), String> {
        loop {
            {
                let output = self.output.lock().expect("the port's lock is not poisoned");
                let found = output[self.cursor..]
                    .windows(needle.len())
                    .position(|window| window == needle.as_bytes());
                if let Some(at) = found {
                    self.cursor += at + needle.len();
                    return Ok(());
                }
                if Instant::now() >= self.deadline {
                    let text = String::from_utf8_lossy(&output);
                    let start = text.len().saturating_sub(2000);
                    let tail = text.get(start..).unwrap_or(&text);
                    return Err(format!(
                        "the PCI serial port never sent {needle:?}; its last bytes:\n{tail}"
                    ));
                }
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Types `text` on the port.
    fn send(&mut self, text: &str) -> Result<(), String> {
        for byte in text.bytes() {
            self.stream.write_all(&[byte]).map_err(|err| format!("typing on the port: {err}"))?;
            std::thread::sleep(Duration::from_millis(5));
        }
        Ok(())
    }
}

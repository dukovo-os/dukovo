// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A virtio console with one port, named as QEMU names its guest agent's
//! channel, whose host end the test holds through a Unix socket: the
//! kernel makes the port the terminal `/devices/vport0p1` and links it
//! from `/devices/virtio-ports/org.qemu.guest_agent.0`; what a program
//! writes there reaches the host as it is, with no carriage return added,
//! and what the host sends while the program has the port open is read
//! as it is; the port taken away goes, with its link (ADR 0327).

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

use super::super::*;

pub const CASE: Case = Case {
    name: "virtio-ports",
    about: "a virtio console port named org.qemu.guest_agent.0, held by the test on the host: it is /devices/vport0p1 and linked by its name, carries bytes as they are both ways, and goes when it is taken away",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    cmdline: "lockdep console-sign-in=off",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &[
        "-device",
        "virtio-serial-pci,id=vser0",
        "-chardev",
        PORT_LINE,
        "-device",
        "virtserialport,bus=vser0.0,chardev=port,name=org.qemu.guest_agent.0,id=agent",
    ],
    script: virtio_ports,
    disks: &[],
    second: None,
};

/// The port's name, as QEMU's guest agent's.
const NAME: &str = "org.qemu.guest_agent.0";

/// Reads from the host's end of the port until `want` came, for at most
/// two minutes.
fn read_until(line: &mut UnixStream, want: &[u8]) -> Result<(), String> {
    line.set_read_timeout(Some(Duration::from_secs(120))).map_err(|e| e.to_string())?;
    let mut got = Vec::new();
    let mut buffer = [0u8; 256];
    while !got.ends_with(want) {
        match line.read(&mut buffer) {
            Ok(0) => return Err("the port's socket closed".into()),
            Ok(n) => got.extend_from_slice(&buffer[..n]),
            Err(err) => {
                let got = String::from_utf8_lossy(&got);
                return Err(format!("waited for {want:?} on the port, got {got:?}: {err}"));
            }
        }
    }
    if got != want {
        let got = String::from_utf8_lossy(&got);
        return Err(format!("the port carried {got:?}, not only {want:?}"));
    }
    Ok(())
}

/// The `virtio-ports` case.
fn virtio_ports(vm: &mut Vm, case: &Case) -> Result<(), String> {
    boot_report(vm, case)?;
    let path = port_line(vm.dir(), case.name);
    let mut line = UnixStream::connect(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    vm.expect_anywhere("Ports: the virtio console at ")?;
    vm.expect_anywhere(&format!("Ports: vport0p1 is /devices/virtio-ports/{NAME}"))?;
    vm.expect("Boot complete.")?;
    vm.expect("consoled: 6 consoles; tty1 is shown")?;
    vm.expect_screen(&[&format!("{RELEASE} {VERSION} (tty1)"), "# "])?;
    vm.type_text(&format!("readlink /devices/virtio-ports/{NAME}; ls /devices | grep vport\n"))?;
    vm.expect_screen(&["../vport0p1", "vport0p1"])?;
    // A program, as the guest agent is, has the port open both ways: a
    // line to the host, then what the host answers once it has the line,
    // which tells it the port is open. A raw port adds no carriage return
    // to the line, and keeps the one the host sends.
    vm.type_text(&format!(
        "clear; sh -c 'exec 3<>/devices/virtio-ports/{NAME}; echo ready >&3; head -c 8 <&3 | od -c | head -1; echo done'\n"
    ))?;
    read_until(&mut line, b"ready\n")?;
    line.write_all(b"pong42\r\n").map_err(|e| e.to_string())?;
    vm.expect_screen(&["0000000   p   o   n   g   4   2  \\r  \\n", "done"])?;
    // The port taken away goes, with its link.
    vm.monitor("device_del agent")?;
    vm.expect("Ports: vport0p1 was taken away")?;
    vm.type_text("echo links: $(ls /devices/virtio-ports | wc -l) ports: $(ls /devices | grep -c vport)\n")?;
    vm.expect_screen(&["links: 0 ports: 0"])?;
    vm.type_text("poweroff\n")?;
    vm.expect("spark: asked to power off the machine")?;
    vm.expect("spark: every service has stopped")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}

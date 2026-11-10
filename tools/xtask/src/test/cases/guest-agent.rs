// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The QEMU guest agent (ADR 0329): the test holds the host's end of the
//! virtio port org.qemu.guest_agent.0 through a Unix socket, as Proxmox
//! and libvirt do, and speaks the protocol to guest-agent(8). A request
//! begun and cut off by 0xFF is thrown away, and guest-sync-delimited
//! answers after 0xFF, as clients begin; then it pings with an id, lists
//! the commands, and asks the machine's name, system, time and network
//! cards, the card's address among them; a command it does not answer
//! and a line that is not JSON are refused with QEMU's error classes; and
//! guest-shutdown powers the machine off cleanly, with no answer.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

use super::super::*;

pub const CASE: Case = Case {
    name: "guest-agent",
    about: "QEMU's guest agent protocol on the virtio port org.qemu.guest_agent.0, held by the test as Proxmox holds it: guest-sync-delimited after 0xFF, ping, info, the machine's name, system, time and network cards with their addresses, refusals in QEMU's error classes, and guest-shutdown powering the machine off cleanly (ADR 0329)",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    cmdline: "console-sign-in=off",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &[
        "-netdev",
        "user,id=n0",
        "-device",
        "virtio-net-pci,netdev=n0,mac=52:54:00:9a:00:01",
        "-device",
        "virtio-serial-pci,id=vser0",
        "-chardev",
        PORT_LINE,
        "-device",
        "virtserialport,bus=vser0.0,chardev=port,name=org.qemu.guest_agent.0",
    ],
    script: guest_agent,
    disks: &[],
    second: None,
};

/// Reads one answer from the host's end of the port: up to its newline,
/// for at most two minutes.
fn answer(line: &mut UnixStream) -> Result<String, String> {
    line.set_read_timeout(Some(Duration::from_secs(120))).map_err(|e| e.to_string())?;
    let mut got = Vec::new();
    let mut byte = [0u8; 1];
    while got.last() != Some(&b'\n') {
        match line.read(&mut byte) {
            Ok(0) => return Err("the port's socket closed".into()),
            Ok(_) => got.push(byte[0]),
            Err(err) => {
                let got = String::from_utf8_lossy(&got);
                return Err(format!("waited for an answer on the port, got {got:?}: {err}"));
            }
        }
    }
    // 0xFF is not UTF-8: it is shown as such.
    Ok(got.iter().map(|&b| if b == 0xFF { "<FF>".to_string() } else { char::from(b).to_string() }).collect())
}

/// Sends `request` and reads its answer, which must hold each of `wants`.
fn ask(line: &mut UnixStream, request: &str, wants: &[&str]) -> Result<String, String> {
    line.write_all(request.as_bytes()).map_err(|e| e.to_string())?;
    let got = answer(line)?;
    for want in wants {
        if !got.contains(want) {
            return Err(format!("{request} was answered {got:?}, without {want:?}"));
        }
    }
    Ok(got)
}

/// The `guest-agent` case.
fn guest_agent(vm: &mut Vm, case: &Case) -> Result<(), String> {
    // QEMU has made the port's socket once the kernel speaks.
    boot_report(vm, case)?;
    let path = port_line(vm.dir(), case.name);
    let mut line = UnixStream::connect(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    vm.expect_anywhere(
        "guest-agent: answering the host on /devices/virtio-ports/org.qemu.guest_agent.0",
    )?;
    vm.expect_anywhere("netd: net0 has 10.0.2.15/24")?;
    vm.expect_screen(&[&format!("{RELEASE} {VERSION} (tty1)"), "# "])?;

    // A client begins: what came before 0xFF is thrown away, and the
    // answer comes after a 0xFF of its own.
    line.write_all(b"{\"execute\": \"guest-").map_err(|e| e.to_string())?;
    line.write_all(b"\xff{\"execute\":\"guest-sync-delimited\",\"arguments\":{\"id\":4242}}")
        .map_err(|e| e.to_string())?;
    let synced = answer(&mut line)?;
    if synced != "<FF>{\"return\":4242}\n" {
        return Err(format!("guest-sync-delimited was answered {synced:?}"));
    }
    ask(&mut line, "{\"execute\":\"guest-ping\",\"id\":7}", &["\"id\":7", "\"return\":{}"])?;
    ask(
        &mut line,
        "{\"execute\": \"guest-info\"}",
        &["\"name\":\"guest-network-get-interfaces\"", "\"name\":\"guest-shutdown\"", "\"success-response\":false"],
    )?;
    let os = format!("\"name\":\"{RELEASE}\"");
    let version = format!("\"version\":\"{VERSION}\"");
    ask(&mut line, "{\"execute\": \"guest-get-osinfo\"}", &["\"id\":\"dukovo\"", &os, &version, "\"machine\":\"x86_64\""])?;
    ask(&mut line, "{\"execute\": \"guest-get-time\"}", &["\"return\":1"])?;
    let named = ask(&mut line, "{\"execute\": \"guest-get-host-name\"}", &["\"host-name\":\""])?;
    let name = named.split("\"host-name\":\"").nth(1).and_then(|rest| rest.split('"').next()).unwrap_or("");
    vm.type_text("clear; echo name=$(hostname)\n")?;
    vm.expect_screen(&[&format!("name={name}")])?;
    ask(
        &mut line,
        "{\"execute\": \"guest-network-get-interfaces\"}",
        &[
            "\"hardware-address\":\"52:54:00:9a:00:01\"",
            "\"ip-address\":\"10.0.2.15\"",
            "\"prefix\":24",
            "\"ip-address-type\":\"ipv6\"",
            "\"rx-bytes\":",
        ],
    )?;
    // Refusals, in QEMU's error classes.
    ask(&mut line, "{\"execute\": \"guest-exec\", \"id\": \"x\"}", &["\"class\":\"CommandNotFound\"", "\"id\":\"x\""])?;
    ask(&mut line, "hello\n", &["\"class\":\"GenericError\"", "not JSON"])?;

    // guest-shutdown answers nothing, and the machine powers off cleanly.
    line.write_all(b"{\"execute\": \"guest-shutdown\"}").map_err(|e| e.to_string())?;
    vm.expect("guest-agent: the host asked to power the machine off")?;
    vm.expect("spark: asked to power off the machine")?;
    vm.expect("spark: every service has stopped")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}

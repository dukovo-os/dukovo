// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! PCIe errors are reported to the kernel and counted per function
//! (Phase 19, ADR 0663). QEMU's q35 machine under UEFI has an extra root
//! port, `aerport`, with a virtio network card behind it (`aernic`, MAC
//! address 52:54:00:12:34:71, on the user network) whose AER capability
//! is turned on (`aer=on`). QEMU's e1000e would not do: the error
//! reporting enables of its device control cannot be set, so it never
//! sends an error message. The firmware's \_OSC gives the kernel AER and
//! the PCIe capability (QEMU masks DPC off), so the kernel turns the
//! error reporting of the card and the root port on, logs how many root
//! ports it watches, and takes the root port's error messages. QEMU's
//! monitor then makes the card send a correctable receiver error
//! (`RCVR`) and, once the kernel has logged it, an uncorrectable
//! completion timeout (`COMP_TIME`), which the default severity makes
//! non-fatal. Where the kernel does not log one within 30 s, the failure
//! shows what QEMU holds in the card's and the root port's registers.
//!
//! The kernel must say the card's address, its IDs and the error's name
//! in each line, with the root port that took the message, and must not
//! mark the card failed. From the console as root: the card's file
//! aer_dev_correctable has `RxErr 1` and a total of 1, its file
//! aer_dev_nonfatal has `CmpltTO 1` and a total of 1, its file
//! aer_dev_fatal has a total of 0, its owner (aer_owner) is `kernel` and
//! its error_state is `normal`; the root port counts one corrected and
//! one non-fatal message in its files aer_rootport_total_err_cor and
//! aer_rootport_total_err_nonfatal; the record in /kernel/ras/errors has
//! one corrected and one recoverable `aer` line; the command `hardware
//! devices --json` gives the card `pcie_errors` with the same counts;
//! and the card still works: its interface, found by the MAC address, has
//! its address from the user network and its link up, and the gateway
//! answers a ping through it.
//!
//! The card is found by its IDs (1af4:1041) in the PCI devices folder of
//! /kernel/bus, and the root port as the function whose file
//! aer_rootport_total_err_cor reads 1, so that no bus number is fixed.
//! QEMU sends the messages only if the card's and the port's device
//! control enable the error class and the port's bridge control has SERR
//! on (`pcie_aer_msg`), which is what the kernel must have done.

use super::super::*;

/// The card's MAC address.
const MAC: &str = "52:54:00:12:34:71";

/// What QEMU is given: the root port and the virtio network card behind
/// it, with its AER capability.
const DEVICES: &[&str] = &[
    "-device",
    "pcie-root-port,id=aerport,bus=pcie.0,chassis=20,slot=20",
    "-netdev",
    "user,id=aernet",
    "-device",
    "virtio-net-pci,netdev=aernet,bus=aerport,id=aernic,mac=52:54:00:12:34:71,aer=on",
];

pub const CASE: Case = Case {
    name: "aer",
    about: "PCIe error reporting (AER) on q35 under UEFI: the firmware's _OSC gives the kernel AER, which watches the root ports; QEMU's monitor makes a virtio network card (aer=on) behind an extra root port send a correctable (RCVR) and a non-fatal (COMP_TIME) error, which the kernel logs with the card's address and the root port, counts in the card's aer_dev_correctable and aer_dev_nonfatal and the root port's aer_rootport_total_err files, records in /kernel/ras/errors and shows in hardware devices --json; the card stays normal and keeps its address and its route (ADR 0663)",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    cmdline: "console-sign-in=off",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: DEVICES,
    script: aer,
    disks: &[],
    second: None,
};

/// What a failure shows of the kernel's log: its PCI and PCIe lines.
fn pcie_log(vm: &Vm) -> String {
    let lines: Vec<String> = vm
        .log()
        .lines()
        .filter(|line| line.contains("PCIe:") || line.contains("PCI:"))
        .map(|line| line.trim().to_string())
        .collect();
    lines.join("\n")
}

/// The text after `start` in the kernel's log line with `needle`, up to
/// the next space or the end of the line.
fn word_after(log: &str, needle: &str, start: &str) -> Result<String, String> {
    let line = log
        .lines()
        .find(|line| line.contains(needle))
        .ok_or_else(|| format!("the kernel never said a line with \"{needle}\""))?;
    let rest = line
        .split(start)
        .nth(1)
        .ok_or_else(|| format!("\"{line}\" has no \"{start}\""))?;
    Ok(rest.split_whitespace().next().unwrap_or("").to_string())
}

/// Types `command` at the root prompt and returns the lines the screen
/// shows once it has run, without the lines that still have a `$` (the
/// typed command). The command ends with an `end-N` line of its own,
/// which the typed text spells with a command, so the echo cannot be
/// taken for it. Each result line is a label and its value: the typed
/// label comes before its `$(`, so a line shows only once the command has
/// run.
fn guest(vm: &mut Vm, command: &str) -> Result<Vec<String>, String> {
    static RUNS: AtomicU32 = AtomicU32::new(0);
    let n = RUNS.fetch_add(1, Ordering::Relaxed);
    answer_prompt(vm, "#", &format!("clear; {command}; echo \"end-$(echo {n})\""))?;
    let marker = format!("end-{n}");
    let deadline = Instant::now() + Duration::from_secs(40);
    loop {
        let screen = vm.screen()?;
        if screen.iter().any(|line| line.trim() == marker) {
            return Ok(screen
                .iter()
                .map(|line| line.trim().to_string())
                .filter(|line| !line.is_empty() && !line.contains('$'))
                .collect());
        }
        if Instant::now() >= deadline {
            return screen_shows(&screen, &[&marker]).map(|()| Vec::new());
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

/// What a failed check shows besides its reason: the kernel's PCI and
/// PCIe lines, and what the card's and the root port's files hold (the
/// counts that are not 0), as far as the screen takes them.
fn failed(vm: &mut Vm, card: &str, port: &str, reason: &str) -> String {
    let dump = format!(
        "cd /kernel/bus/pci/devices/{card}; grep -v ' 0$' aer_dev_correctable aer_dev_nonfatal \
         aer_dev_fatal; grep . aer_owner error_state ../{port}/aer_rootport_total_err_*; \
         grep -o 'pcie_errors[^}}]*' /tmp/devs.json; tail -n 4 /kernel/ras/errors | cut -c1-70"
    );
    let files = match guest(vm, &dump) {
        Ok(lines) => lines.join("\n"),
        Err(err) => format!("(the files could not be read: {err})"),
    };
    format!("{reason}\nthe kernel's PCI lines:\n{}\nthe files:\n{files}", pcie_log(vm))
}

/// Checks that `lines` has each of `wanted`, as a line of its own.
fn has_lines(lines: &[String], wanted: &[&str]) -> Result<(), String> {
    for line in wanted {
        if !lines.iter().any(|shown| shown == line) {
            return Err(format!(
                "the console never printed the line \"{line}\"; it printed: {}",
                lines.join(" | ")
            ));
        }
    }
    Ok(())
}

/// Has QEMU's monitor make the card send `error`. QEMU answers `OK id:
/// aernic` and where the card is when it sent the message.
fn inject(vm: &mut Vm, error: &str) -> Result<(), String> {
    let command = format!("pcie_aer_inject_error aernic {error}");
    let said = vm.monitor_output(&command, 0)?;
    if said.starts_with("OK id: aernic ") {
        Ok(())
    } else {
        Err(format!("QEMU's monitor, asked to `{command}`, said: {}", said.trim()))
    }
}

/// Waits up to 30 s for the kernel's line with `needle`, which comes
/// within a second of the error that makes it; when it does not, says
/// what QEMU holds in the card's and the root port's registers.
fn error_line(vm: &mut Vm, needle: &str) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !vm.log().contains(needle) {
        if Instant::now() >= deadline || vm.exited().is_some() {
            return Err(format!(
                "no \"{needle}\" within 30 s of the error\nthe kernel's PCI lines:\n{}\n{}",
                pcie_log(vm),
                registers(vm)
            ));
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    Ok(())
}

/// What QEMU holds in the configuration space of the card and of the
/// root port: the first 256 bytes, then the 64 of the AER capability's
/// place (0x100), as QEMU's monitor reads them through the machine's
/// ECAM window, whatever the kernel thinks it wrote. Each is found by its
/// IDs in the kernel's PCI lines.
fn registers(vm: &mut Vm) -> String {
    let mut out = String::new();
    for (what, ids) in [("the card", "1af4:1041 class"), ("the root port", "1b36:000c class")] {
        let dump = word_after(&vm.log(), ids, "PCI: ").and_then(|address| {
            let base = ecam_of(vm, &address)?;
            let low = vm.monitor_output(&format!("xp /64wx {base:#x}"), 0)?;
            let aer = vm.monitor_output(&format!("xp /16wx {:#x}", base + 0x100), 0)?;
            Ok(format!("{address}:\n{}\n{}", low.trim_end(), aer.trim_end()))
        });
        let dump = dump.unwrap_or_else(|err| format!("(not read: {err})"));
        out.push_str(&format!("QEMU's registers of {what}, {dump}\n"));
    }
    out
}

/// The physical address of the configuration space of the function at
/// `address` (`0000:01:00.0`), in the ECAM window QEMU's memory tree
/// names `pcie-mmcfg-mmio`.
fn ecam_of(vm: &mut Vm, address: &str) -> Result<u64, String> {
    let tree = vm.monitor_output("info mtree", 0)?;
    let window = tree
        .lines()
        .find(|line| line.contains("pcie-mmcfg-mmio"))
        .and_then(|line| line.trim().split('-').next())
        .and_then(|start| u64::from_str_radix(start, 16).ok())
        .ok_or("QEMU's memory tree has no pcie-mmcfg-mmio window")?;
    let place = place_of(address).ok_or_else(|| format!("`{address}` is not a PCI address"))?;
    Ok(window + place)
}

/// Where the configuration space of the function at `address`
/// (`0000:01:00.0`) is in its segment's ECAM window.
fn place_of(address: &str) -> Option<u64> {
    let mut parts = address.split(':').skip(1);
    let bus = u64::from_str_radix(parts.next()?, 16).ok()?;
    let (device, function) = parts.next()?.split_once('.')?;
    let device = u64::from_str_radix(device, 16).ok()?;
    let function = u64::from_str_radix(function, 16).ok()?;
    Some(bus << 20 | device << 15 | function << 12)
}

fn aer(vm: &mut Vm, case: &Case) -> Result<(), String> {
    boot_report(vm, case)?;
    // The kernel's lines come while it starts, among the ones boot_report
    // reads past.
    vm.expect_anywhere("PCIe: AER watches ")?;
    vm.expect_anywhere("Boot complete.")?;
    let log = vm.log();
    let granted = log.lines().any(|line| {
        line.contains("PCI: ACPI host bridge ")
            && line.contains(" gives the kernel AER")
            && line.contains("the PCIe capability")
    });
    if !granted {
        return Err(format!(
            "the firmware's _OSC did not give the kernel AER and the PCIe capability:\n{}",
            pcie_log(vm)
        ));
    }

    // The card's interface, by its MAC address in the kernel's line, has
    // its address from netd before the errors: its driver is running.
    vm.expect_anywhere(&format!("MAC {MAC}"))?;
    let nic = word_after(&vm.log(), &format!("MAC {MAC}"), "Net: ")?;
    vm.expect_anywhere(&format!("netd: {nic} has 10.0.2."))?;

    // A correctable receiver error, then, once it is logged, an
    // uncorrectable completion timeout, which the default severity
    // makes non-fatal.
    inject(vm, "RCVR")?;
    error_line(vm, "(1af4:1041) corrected error: RxErr; via ")?;
    inject(vm, "COMP_TIME")?;
    error_line(vm, "(1af4:1041) uncorrected non-fatal error: CmpltTO")?;
    let log = vm.log();
    let addr = word_after(&log, "(1af4:1041) corrected error: RxErr; via ", "PCIe: ")?;
    let via = word_after(&log, "(1af4:1041) corrected error: RxErr; via ", "; via ")?;
    if let Some(line) = log.lines().find(|line| line.contains("marked failed")) {
        return Err(format!("the kernel marked a device failed: {}", line.trim()));
    }
    if !log.contains(&format!("Net: {nic} is a virtio network card at {addr}")) {
        return Err(format!("{nic} is not the virtio card at {addr} that the errors came from"));
    }

    vm.expect("spark: started consoled (/system/commands/consoled) as process ")?;
    vm.expect_screen(&[&format!("{RELEASE} {VERSION} (tty1)"), "# "])?;

    // The card by its IDs, and the root port as the function whose count
    // of corrected messages is 1.
    let found = guest(
        vm,
        "for d in /kernel/bus/pci/devices/*; do grep -q 0x1af4 $d/vendor && grep -q 0x1041 \
         $d/device && C=${d##*/}; done; for f in \
         /kernel/bus/pci/devices/*/aer_rootport_total_err_cor; do [ \"$(cat $f)\" = 1 ] && \
         R=${f%/*}; done; echo \"card $C\"; echo \"port ${R##*/}\"",
    )?;
    let (card, port) = (format!("card {addr}"), format!("port {via}"));
    if let Err(err) = has_lines(&found, &[&card, &port]) {
        return Err(failed(vm, &addr, &via, &err));
    }

    // The card's and the root port's counts.
    let counts = guest(
        vm,
        &format!(
            "cd /kernel/bus/pci/devices/{addr}; \
             echo \"ce $(grep -c '^RxErr 1$' aer_dev_correctable) $(grep -c '^TOTAL_ERR_COR 1$' \
             aer_dev_correctable)\"; \
             echo \"ne $(grep -c '^CmpltTO 1$' aer_dev_nonfatal) $(grep -c \
             '^TOTAL_ERR_NONFATAL 1$' aer_dev_nonfatal)\"; \
             echo \"fe $(grep -c '^TOTAL_ERR_FATAL 0$' aer_dev_fatal)\"; \
             echo \"owner $(cat aer_owner)\"; echo \"state $(cat error_state)\"; \
             echo \"rp $(cat ../{via}/aer_rootport_total_err_cor) \
             $(cat ../{via}/aer_rootport_total_err_nonfatal) \
             $(cat ../{via}/aer_rootport_total_err_fatal)\"; \
             echo \"rpowner $(cat ../{via}/aer_owner)\"; \
             echo \"rpstate $(cat ../{via}/error_state)\""
        ),
    )?;
    let wanted = ["ce 1 1", "ne 1 1", "fe 1", "owner kernel", "state normal", "rp 1 1 0"];
    let rest = ["rpowner kernel", "rpstate normal"];
    if let Err(err) = has_lines(&counts, &wanted).and(has_lines(&counts, &rest)) {
        return Err(failed(vm, &addr, &via, &err));
    }

    // The record has the two errors, one corrected and one that was not
    // corrected and went on.
    let record = guest(
        vm,
        "echo \"ras $(grep -c ' aer corrected pcie x1 ' /kernel/ras/errors) $(grep -c \
         ' aer recoverable pcie x1 ' /kernel/ras/errors) $(grep -c ' aer ' /kernel/ras/errors)\"",
    )?;
    if let Err(err) = has_lines(&record, &["ras 1 1 2"]) {
        return Err(failed(vm, &addr, &via, &err));
    }

    // The same counts in the command's JSON, which one line holds (a `.`
    // stands for a quote).
    let devices = guest(
        vm,
        "hardware devices --json > /tmp/devs.json; echo \"pe $(grep -o \
         'owner.:.kernel.,.corrected.:1,.nonfatal.:1,.fatal.:0,.failed.:false' /tmp/devs.json | \
         wc -l)\"",
    )?;
    if let Err(err) = has_lines(&devices, &["pe 1"]) {
        return Err(failed(vm, &addr, &via, &err));
    }

    // The card still works: the interface with the MAC address has its
    // address and its link, and the gateway answers through it.
    let works = guest(
        vm,
        &format!(
            "for n in /kernel/class/net/*; do grep -qi {MAC} $n/address && I=${{n##*/}}; done; \
             echo \"nic $I $(cat /kernel/class/net/$I/ipv4) $(cat /kernel/class/net/$I/operstate)\"; \
             echo \"ping $(ping -c 2 -W 3 10.0.2.2 | grep -c '64 bytes from 10.0.2.2')\""
        ),
    )?;
    let up = works.iter().find(|line| line.starts_with("nic ")).map_or("", String::as_str);
    let parts: Vec<&str> = up.split_whitespace().collect();
    let addressed = parts.len() == 4
        && parts[1] == nic
        && parts[2].starts_with("10.0.2.")
        && parts[3] == "up";
    if !addressed {
        let reason = format!("the card {nic} has not its address and link: \"{up}\"");
        return Err(failed(vm, &addr, &via, &reason));
    }
    let answered = works
        .iter()
        .find_map(|line| line.strip_prefix("ping "))
        .and_then(|count| count.trim().parse::<u32>().ok())
        .unwrap_or(0);
    if answered == 0 {
        let reason = format!("the gateway 10.0.2.2 did not answer a ping through {nic}");
        return Err(failed(vm, &addr, &via, &reason));
    }

    if vm.log().contains("Radix panic") {
        return Err("the kernel panicked".into());
    }
    vm.type_text("poweroff\n")?;
    vm.expect("spark: asked to power off the machine")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_come_from_the_kernels_line() {
        let log = "[ 1.0] PCIe: 0000:03:00.0 (1af4:1041) corrected error: RxErr; via 0000:00:03.0\n";
        let needle = "(1af4:1041) corrected error: RxErr; via ";
        assert_eq!(word_after(log, needle, "PCIe: ").unwrap(), "0000:03:00.0");
        assert_eq!(word_after(log, needle, "; via ").unwrap(), "0000:00:03.0");
        assert!(word_after(log, "no such line", "x").is_err());
    }

    #[test]
    fn the_net_line_names_the_interface() {
        let log = "Net: net0 is a virtio network card at 0000:03:00.0, MAC \
                   52:54:00:12:34:71, 255 receive\n";
        assert_eq!(word_after(log, &format!("MAC {MAC}"), "Net: ").unwrap(), "net0");
    }

    #[test]
    fn functions_have_their_place_in_the_window() {
        assert_eq!(place_of("0000:00:03.0"), Some(0x18000));
        assert_eq!(place_of("0000:01:00.0"), Some(0x10_0000));
        assert_eq!(place_of("0000:1a:1f.7"), Some(0x1a0_0000 | 0xf_8000 | 0x7000));
        assert_eq!(place_of("bogus"), None);
    }

    #[test]
    fn lines_are_matched_whole() {
        let lines = vec!["ce 1 1".to_string(), "rp 1 1 0".to_string()];
        assert!(has_lines(&lines, &["ce 1 1", "rp 1 1 0"]).is_ok());
        assert!(has_lines(&lines, &["ce 1"]).is_err());
    }
}

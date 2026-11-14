// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! hostnamectl(1): the machine's name and what it runs on, with systemd's
//! hostnamectl's commands (ADR 0478).
//!
//! The name is `/system/settings/hostname` (hostname(5)), which
//! `sethostname(2)` writes for an administrator; it is the static host
//! name, and Dukovo has no other. `status` adds what the system says of
//! itself: the release and kernel from `/kernel/version`, the
//! architecture from uname(2), the hardware's vendor and model and the
//! firmware's version and date from the DMI fields in
//! `/kernel/class/dmi/id` (the ones `hwinfo` and `dukofetch` read), and
//! this boot's ID from logd(8), which names each boot. Dukovo keeps no
//! machine ID, so the line is left out. The boot test `admin-commands`
//! runs `status` and `set-hostname`.

use std::fs;
use std::io;

use crate::args::{Arg, Args};
use crate::dukofetch::parse_version;
use crate::logd::{self, Boot};
use crate::util::{self, Fail, fail, usage};

/// The kernel's version line: `Radix 0.15.0 (Dukovo Server 0.15.0)`.
const VERSION: &str = "/kernel/version";

/// The firmware's tables, as Linux shows them in `/sys/class/dmi/id`.
const DMI: &str = "/kernel/class/dmi/id";

/// The longest host name, in bytes (hostname(5)).
const NAME_MAX: usize = 64;

/// The longest one part of a host name, between dots (RFC 1035).
const LABEL_MAX: usize = 63;

/// What `status` shows, each as the machine says it, or none.
#[derive(Debug, Default, PartialEq, Eq)]
struct Facts {
    hostname: String,
    boot_id: Option<String>,
    os: Option<String>,
    kernel: Option<String>,
    architecture: String,
    vendor: Option<String>,
    model: Option<String>,
    firmware_version: Option<String>,
    firmware_date: Option<String>,
}

/// hostnamectl(1).
pub fn main(args: &mut Args) -> Result<i32, Fail> {
    let mut operands = Vec::new();
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Long(l) if l == "static" => {}
            Arg::Long(l) if l == "transient" => {
                return Err(usage(
                    "--transient: Dukovo has one host name, the static one in \
                     /system/settings/hostname; `hostnamectl set-hostname NAME` sets it"
                        .into(),
                ));
            }
            Arg::Long(l) if l == "pretty" => {
                return Err(usage(
                    "--pretty: Dukovo keeps no pretty host name; the name \
                     `hostnamectl set-hostname NAME` sets is the machine's name"
                        .into(),
                ));
            }
            Arg::Long(l) if l == "no-ask-password" || l == "no-pager" => {}
            Arg::Operand(word) => operands.push(word.to_string_lossy().into_owned()),
            other => return Err(args.unknown(&other)),
        }
    }
    let words: Vec<&str> = operands.iter().map(String::as_str).collect();
    match words.as_slice() {
        [] | ["status"] => {
            print!("{}", status_text(&facts()));
            Ok(0)
        }
        ["hostname"] => {
            println!("{}", util::hostname());
            Ok(0)
        }
        ["hostname" | "set-hostname", name] => set_hostname(name),
        ["set-hostname", ..] => {
            Err(usage("set-hostname takes one name, `hostnamectl set-hostname NAME`".into()))
        }
        [verb @ ("set-icon-name" | "set-chassis" | "set-deployment" | "set-location"), ..] => {
            Err(usage(format!(
                "{verb}: Dukovo keeps only the host name; `hostnamectl \
                 set-hostname NAME` sets it"
            )))
        }
        [other, ..] => {
            Err(usage(format!("no command '{other}': status, hostname or set-hostname")))
        }
    }
}

/// Gives the machine the name `name`, which `sethostname(2)` writes to
/// the settings for an administrator.
fn set_hostname(name: &str) -> Result<i32, Fail> {
    check_name(name).map_err(|why| usage(format!("'{name}': {why}")))?;
    // SAFETY: the pointer and length are the name's own bytes.
    let set = unsafe { libc::sethostname(name.as_ptr().cast(), name.len() as _) };
    if set != 0 {
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::EPERM) {
            return Err(fail(format!(
                "only an administrator may name the machine: run `elevate hostnamectl \
                 set-hostname {name}`"
            )));
        }
        return Err(util::failed(&"set-hostname", error));
    }
    Ok(0)
}

/// Why `name` is no host name: dot-separated words of letters, digits and
/// `-`, none starting or ending with `-`, each 63 bytes at most and the
/// whole 64, as hostname(5) and the installer take them.
fn check_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("a host name is not empty".into());
    }
    if name.len() > NAME_MAX {
        return Err(format!("a host name is {NAME_MAX} bytes at most"));
    }
    let fits = |label: &str| {
        !label.is_empty()
            && label.len() <= LABEL_MAX
            && label.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
            && !label.starts_with('-')
            && !label.ends_with('-')
    };
    if name.split('.').all(fits) {
        Ok(())
    } else {
        Err("use letters, digits and - in words (not at a word's start or end), \
             separated by dots"
            .into())
    }
}

/// What the machine says of itself.
fn facts() -> Facts {
    let version = fs::read_to_string(VERSION).ok().and_then(|text| parse_version(&text));
    let dmi = |field: &str| {
        let text = fs::read_to_string(format!("{DMI}/{field}")).ok()?;
        let text = text.trim();
        (!text.is_empty()).then(|| text.to_string())
    };
    Facts {
        hostname: util::hostname(),
        boot_id: this_boot(),
        os: version.as_ref().map(|(_, (name, number))| format!("{name} {number}")),
        kernel: version.as_ref().map(|((name, number), _)| format!("{name} {number}")),
        architecture: architecture(&util::uname().2),
        vendor: dmi("sys_vendor"),
        model: dmi("product_name"),
        firmware_version: dmi("bios_version"),
        firmware_date: dmi("bios_date"),
    }
}

/// This boot's ID, as logd(8) names it; none while logd does not answer.
fn this_boot() -> Option<String> {
    let mut boots = Vec::new();
    let request = logd::Request { ask: "boots".into(), ..logd::Request::default() };
    logd::ask(&request, |line| {
        boots.extend(serde_json::from_str::<Boot>(line).ok());
        Ok(true)
    })
    .ok()?;
    boots.pop().map(|boot| boot.boot)
}

/// uname(2)'s machine as hostnamectl names the architecture: systemd's
/// own words for the ones it knows, else the machine's.
fn architecture(machine: &str) -> String {
    match machine {
        "x86_64" => "x86-64",
        "i386" | "i486" | "i586" | "i686" => "x86",
        "aarch64" => "arm64",
        other => other,
    }
    .to_string()
}

/// `status`'s lines: each label right-aligned before a colon, as
/// hostnamectl's, and none for a fact that is not there.
fn status_text(facts: &Facts) -> String {
    let rows = [
        ("Static hostname", Some(facts.hostname.as_str())),
        ("Boot ID", facts.boot_id.as_deref()),
        ("Operating System", facts.os.as_deref()),
        ("Kernel", facts.kernel.as_deref()),
        ("Architecture", Some(facts.architecture.as_str())),
        ("Hardware Vendor", facts.vendor.as_deref()),
        ("Hardware Model", facts.model.as_deref()),
        ("Firmware Version", facts.firmware_version.as_deref()),
        ("Firmware Date", facts.firmware_date.as_deref()),
    ];
    let mut text = String::new();
    for (label, value) in rows {
        if let Some(value) = value {
            text.push_str(&format!("{label:>17}: {value}\n"));
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dell() -> Facts {
        Facts {
            hostname: "web01".into(),
            boot_id: Some("00112233445566778899aabbccddeeff".into()),
            os: Some("Dukovo Server 0.15.0".into()),
            kernel: Some("Radix 0.15.0".into()),
            architecture: "x86-64".into(),
            vendor: Some("Dell Inc.".into()),
            model: Some("OptiPlex 7040".into()),
            firmware_version: Some("1.12.2".into()),
            firmware_date: Some("07/12/2023".into()),
        }
    }

    #[test]
    fn status_lays_out_what_the_machine_says() {
        let lines = [
            "  Static hostname: web01",
            "          Boot ID: 00112233445566778899aabbccddeeff",
            " Operating System: Dukovo Server 0.15.0",
            "           Kernel: Radix 0.15.0",
            "     Architecture: x86-64",
            "  Hardware Vendor: Dell Inc.",
            "   Hardware Model: OptiPlex 7040",
            " Firmware Version: 1.12.2",
            "    Firmware Date: 07/12/2023",
        ];
        assert_eq!(status_text(&dell()), format!("{}\n", lines.join("\n")));
    }

    #[test]
    fn a_machine_without_firmware_tables_or_a_log_leaves_those_lines_out() {
        let bare =
            Facts { hostname: "dukovo".into(), architecture: "x86-64".into(), ..Facts::default() };
        assert_eq!(status_text(&bare), "  Static hostname: dukovo\n     Architecture: x86-64\n");
    }

    #[test]
    fn architectures_have_systemds_names() {
        assert_eq!(architecture("x86_64"), "x86-64");
        assert_eq!(architecture("aarch64"), "arm64");
        assert_eq!(architecture("riscv64"), "riscv64");
    }

    #[test]
    fn a_host_name_is_words_of_letters_digits_and_dashes() {
        for good in ["web01", "a", "web-01.example.com", &"a".repeat(63)] {
            assert_eq!(check_name(good), Ok(()), "{good}");
        }
        let long_label = "a".repeat(64);
        let long_name = format!("{}.{}.{}", "a".repeat(30), "b".repeat(30), "c".repeat(30));
        for bad in [
            "",
            "-web",
            "web-",
            "web 01",
            "web_01",
            "a..b",
            ".web",
            "web.",
            "web\n",
            "é",
            &long_label,
            &long_name,
        ] {
            assert!(check_name(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn the_options_dukovo_lacks_say_what_it_has() {
        let run = |items: &[&str]| {
            main(&mut Args::new("hostnamectl", items.iter().map(Into::into).collect()))
        };
        for (items, said) in [
            (&["--transient", "status"][..], "one host name"),
            (&["--pretty", "set-hostname", "x"], "no pretty host name"),
            (&["set-chassis", "server"], "only the host name"),
            (&["reboot"], "no command 'reboot'"),
            (&["set-hostname"], "takes one name"),
            (&["set-hostname", "bad-"], "letters, digits"),
        ] {
            let failed = run(items).unwrap_err();
            assert!(failed.usage && failed.message.contains(said), "{items:?}: {}", failed.message);
        }
    }
}

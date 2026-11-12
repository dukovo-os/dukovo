// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The lab network's half of `ob lab` (ADR 0429): what a real lab
//! machine boots from the lab PC before each fresh boot, and the DHCP and
//! TFTP server that boots it.
//!
//! dukovo-lab's `amt` driver asks AMT to boot the machine from the
//! network once; the firmware gets its address and its boot file from
//! dnsmasq on the lab card; the boot file is the network GRUB
//! ([`net_grub`]), which reads the boot script the driver wrote for that
//! boot and loads the writer ([`writer_kit`]) from the driver's own HTTP
//! server; the writer writes the image under test to the machine's disk.
//! The contract between the two sides is dukovo-os/lab's
//! docs/net-boot.md.
//!
//! Plain `ob lab` sets it up ([`set_up`]) once a machine file names a
//! real machine: it builds the writer and the network GRUB, installs
//! them in `/srv/dukovo-lab`, writes dnsmasq's configuration from the
//! machine files, and runs dnsmasq as a service of its own. Each step
//! that would change nothing is skipped; the ones that change something
//! ask for sudo.
//!
//! dnsmasq answers on the lab card alone: the card that holds the lab
//! PC's address on the lab network (`10.77.0.1/24`), and never one that
//! carries this PC's default route, which would be the home network. It
//! serves no DNS, offers no gateway, and gives addresses only to the
//! machines in the machine files, by their MAC addresses.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

use crate::process::{self, step};
use crate::{Context as _, Ctx, Error, Result, bail, baseline, bench, grub};

mod check;

pub use check::check;

/// The network GRUB in the TFTP folder, the boot file dnsmasq names.
pub const GRUB_FILE: &str = "dukovo-lab/grubx64.efi";

/// The boot scripts' folder in the TFTP folder, which the lab's user
/// writes.
const BOOT_SCRIPTS: &str = "dukovo-lab/boot";

/// The network GRUB's built-in script: the boot script dukovo-lab wrote
/// for this machine's boot, by its MAC address, and otherwise back to the
/// firmware, which goes on to the next boot entry: the disk.
pub const GRUB_SCRIPT: &str = "source (tftp)/dukovo-lab/boot/${net_default_mac}.cfg\nexit\n";

/// The writer's kernel command line, which dukovo-lab adds the console,
/// the boot's URL and the machine's address to. dukovo-lab's kits name
/// the first serial port; the machine file's console comes after it and
/// wins. The kernel says only its warnings, so the console is the
/// writer's.
pub const WRITER_CMDLINE: &str = "console=ttyS0 loglevel=4";

/// What the writer says once it is done, as its kit's `done`.
const WRITER_DONE: &str = "lab-writer: done";

/// The modules the writer loads where the kernel has them as modules:
/// the disk controllers' and the network cards' of the lab's machines and
/// of most PCs (Intel's and Realtek's cards, with Realtek's PHY driver,
/// which r8169 asks for by name), and QEMU's for the writer's own check.
const WRITER_MODULES: &[&str] =
    &["ahci", "nvme", "e1000e", "igb", "igc", "e1000", "r8169", "realtek"];

/// What the network GRUB adds to the images' GRUB for x86-64 UEFI (ADR
/// 0306's [`grub::MODULES`] and [`grub::EFI_MODULES`]): the firmware's
/// network card, DHCP's and TFTP's answers and HTTP, which load the
/// writer, Linux's loader, and `exit`, back to the firmware.
const NET_MODULES: &[&str] = &["efinet", "net", "tftp", "http", "linux", "minicmd"];

/// The writer's kit: the folder dukovo-lab's `amt.writer` names.
const KIT_FILE: &str = "kit.toml";

/// The writer's initramfs, in the kit.
const WRITER_INITRD: &str = "lab-writer.cpio";

/// dnsmasq's configuration for the lab network, and the service that
/// runs it.
const DNSMASQ_CONF: &str = "/etc/dukovo-lab/dnsmasq.conf";
const SERVICE: &str = "dukovo-lab-dnsmasq.service";
const SERVICE_FILE: &str = "/etc/systemd/system/dukovo-lab-dnsmasq.service";

/// dnsmasq, as Debian's, Ubuntu's and Mint's dnsmasq-base install it:
/// NetworkManager needs it, so it is there already as a rule.
const DNSMASQ: &str = "/usr/sbin/dnsmasq";

/// Builds the writer as a static program, which runs on any x86-64 Linux,
/// and returns it.
fn writer_static(ctx: &Ctx) -> Result<PathBuf> {
    let target = ctx.target.join("linux-guest");
    let mut cmd = process::cargo(ctx);
    cmd.args(["build", "--quiet", "--release", "-p", "lab-writer"])
        .args(["--target", "x86_64-unknown-linux-gnu"])
        .env("CARGO_TARGET_DIR", &target)
        .env("RUSTFLAGS", "-C target-feature=+crt-static");
    process::run(ctx, &mut cmd)?;
    Ok(target.join("x86_64-unknown-linux-gnu/release/lab-writer"))
}

/// Builds the writer's kit and gives its folder, `out/lab-writer`: the
/// pinned Linux of the benchmarks (ADR 0290), an initramfs whose init is
/// the writer with the modules [`WRITER_MODULES`] names, and `kit.toml`
/// naming them in the form dukovo-lab reads.
pub fn writer_kit(ctx: &Ctx) -> Result<PathBuf> {
    step("Building the lab's disk writer, which real lab machines boot from the lab network");
    let dir = ctx.out.join("lab-writer");
    process::create_dir(&dir)?;
    let program = writer_static(ctx)?;
    let init = std::fs::read(&program).context(|| format!("reading {}", program.display()))?;
    let linux = baseline::fetch(ctx)?;
    let mut modules = Vec::new();
    let paths = bench::linux_modules(&linux.kernel, Some(&linux.modules), WRITER_MODULES)?;
    for (i, path) in paths.iter().enumerate() {
        let file = path.file_name().map_or_default(|name| name.to_string_lossy());
        let bytes = std::fs::read(path).context(|| format!("reading {}", path.display()))?;
        modules.push((format!("{i:02}-{file}"), bytes));
    }
    let initrd = dir.join(WRITER_INITRD);
    std::fs::write(&initrd, initramfs(&init, &modules))
        .context(|| format!("writing {}", initrd.display()))?;
    process::copy(&linux.kernel, &dir.join("vmlinuz"))?;
    std::fs::write(dir.join(KIT_FILE), kit_file())
        .context(|| format!("writing {}", dir.join(KIT_FILE).display()))?;
    Ok(dir)
}

/// The writer's `kit.toml`.
fn kit_file() -> String {
    format!(
        "# The Dukovo Lab's disk writer (ADR 0429), which real lab machines boot\n\
         # from the lab network: `ob lab` makes this folder.\n\
         kernel = \"vmlinuz\"\n\
         initrd = \"{WRITER_INITRD}\"\n\
         cmdline = \"{WRITER_CMDLINE}\"\n\
         done = \"{WRITER_DONE}\"\n"
    )
}

/// The writer's initramfs: the console device, the writer as `/init`,
/// and `modules`, by name, in `/modules`.
fn initramfs(init: &[u8], modules: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    bench::newc_entry(&mut out, 1, "dev", 0o040_755, (0, 0), &[]);
    bench::newc_entry(&mut out, 2, "dev/console", 0o020_600, (5, 1), &[]);
    bench::newc_entry(&mut out, 3, "init", 0o100_755, (0, 0), init);
    bench::newc_entry(&mut out, 4, "modules", 0o040_755, (0, 0), &[]);
    for (ino, (name, bytes)) in (5..).zip(modules) {
        bench::newc_entry(&mut out, ino, &format!("modules/{name}"), 0o100_644, (0, 0), bytes);
    }
    bench::newc_entry(&mut out, 0, "TRAILER!!!", 0, (0, 0), &[]);
    out
}

/// Builds the network GRUB and gives its path, `out/lab-net/grubx64.efi`:
/// the images' GRUB (ADR 0306) for x86-64 UEFI, with [`NET_MODULES`] and
/// [`GRUB_SCRIPT`] built in, and no files of its own to read.
pub fn net_grub(ctx: &Ctx) -> Result<PathBuf> {
    let grub = grub::build(ctx)?;
    step("Building the network GRUB that real lab machines boot from the lab network");
    let dir = ctx.out.join("lab-net");
    process::create_dir(&dir)?;
    let script = dir.join("grub-net.cfg");
    std::fs::write(&script, GRUB_SCRIPT).context(|| format!("writing {}", script.display()))?;
    let image = dir.join("grubx64.efi");
    let mut cmd = Command::new(&grub.mkimage);
    cmd.args(["-O", "x86_64-efi", "-d"])
        .arg(&grub.efi)
        .args(["-p", "(tftp)/dukovo-lab", "-o"])
        .arg(&image)
        .arg("-c")
        .arg(&script)
        .args(grub::MODULES)
        .args(grub::EFI_MODULES)
        .args(NET_MODULES);
    process::run(ctx, &mut cmd)?;
    Ok(image)
}

/// A real machine, from dukovo-lab's machine files, as the lab network
/// knows it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Host {
    name: String,
    mac: String,
    address: String,
    lab_address: String,
    tftp: PathBuf,
    writer: PathBuf,
}

/// The real machines in `dukovo-lab machines --json`'s answer: those whose
/// driver is `amt`.
fn real_machines(json: &str) -> Result<Vec<Host>> {
    let value: Value = serde_json::from_str(json)
        .map_err(|err| Error::new(format!("dukovo-lab's list of machines is not JSON: {err}")))?;
    let mut hosts = Vec::new();
    let machines = value.get("machines").and_then(Value::as_array).cloned().unwrap_or_default();
    for machine in &machines {
        if machine.get("driver").and_then(Value::as_str) != Some("amt") {
            continue;
        }
        let name = machine.get("name").and_then(Value::as_str).unwrap_or("?").to_string();
        let text = |key: &str| {
            machine
                .get("amt")
                .and_then(|amt| amt.get(key))
                .and_then(Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| Error::new(format!("the machine {name} has no amt.{key}")))
        };
        let mac = text("mac")?.to_ascii_lowercase();
        let valid = mac.split(':').count() == 6
            && mac
                .split(':')
                .all(|part| part.len() == 2 && part.bytes().all(|b| b.is_ascii_hexdigit()));
        if !valid || mac == "00:00:00:00:00:00" {
            bail!("the machine {name}'s amt.mac is {mac}: put in its network card's MAC address");
        }
        hosts.push(Host {
            mac,
            address: text("machine_address")?,
            lab_address: text("lab_address")?,
            tftp: PathBuf::from(text("tftp")?),
            writer: PathBuf::from(text("writer")?),
            name,
        });
    }
    hosts.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(hosts)
}

/// A network card of this PC's with an IPv4 address on it.
#[derive(Debug, PartialEq, Eq)]
struct Address {
    card: String,
    address: String,
    prefix: u8,
}

/// The IPv4 addresses in `ip -4 -o address show`'s answer, one a line:
/// `2: enp3s0    inet 10.77.0.1/24 brd ... scope global enp3s0 ...`.
fn addresses(text: &str) -> Vec<Address> {
    text.lines()
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            let card = words.nth(1)?.split('@').next()?.to_string();
            if words.next()? != "inet" {
                return None;
            }
            let (address, prefix) = words.next()?.split_once('/')?;
            Some(Address { card, address: address.into(), prefix: prefix.parse().ok()? })
        })
        .collect()
}

/// The cards in `ip -4 route show default`'s answer: those this PC's
/// default routes leave through.
fn default_cards(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| {
            let words: Vec<&str> = line.split_whitespace().collect();
            let at = words.iter().position(|word| *word == "dev")?;
            Some(words.get(at + 1)?.to_string())
        })
        .collect()
}

/// The lab card: the one card holding `lab_address` with a /24, and no
/// default route.
fn lab_card(addresses: &[Address], default_cards: &[String], lab_address: &str) -> Result<String> {
    let holding: Vec<&Address> = addresses.iter().filter(|a| a.address == lab_address).collect();
    let card = match holding.as_slice() {
        [] => {
            return Err(Error::new(format!(
                "no network card of this PC has the lab network's address {lab_address}"
            ))
            .hint(format!(
                "give the lab card (the one cabled to the lab switch) that address, with no \
                 gateway: `nmcli connection add type ethernet ifname <card> con-name dukovo-lab \
                 ipv4.method manual ipv4.addresses {lab_address}/24 ipv4.never-default yes \
                 ipv6.method disabled`, then ./ob lab again"
            )));
        }
        [one] => one,
        many => bail!(
            "{} cards have the lab network's address {lab_address}: {}",
            many.len(),
            many.iter().map(|a| a.card.as_str()).collect::<Vec<_>>().join(", ")
        ),
    };
    if default_cards.contains(&card.card) {
        return Err(Error::new(format!(
            "{} holds the lab network's address {lab_address} but also carries this PC's \
             default route, so it is not a lab card of its own; dnsmasq would answer the \
             home network there",
            card.card
        ))
        .hint("the lab network needs a card of its own, cabled to the lab switch alone"));
    }
    if card.prefix != 24 {
        bail!(
            "{}'s address {lab_address} is a /{}; the lab network is a /24",
            card.card,
            card.prefix
        );
    }
    Ok(card.card.clone())
}

/// dnsmasq's configuration: DHCP and TFTP on `card` alone, for `hosts`.
fn dnsmasq_conf(card: &str, lab_address: &str, tftp: &Path, hosts: &[Host]) -> String {
    let network = match lab_address.rsplit_once('.') {
        Some((first, _)) => format!("{first}.0"),
        None => lab_address.to_string(),
    };
    let mut conf = format!(
        "# The Dukovo Lab's network (ADR 0429): DHCP and TFTP for the lab's real\n\
         # machines on {card} alone. `ob lab` writes this file from dukovo-lab's\n\
         # machine files; a change by hand is lost the next time.\n\
         \n\
         # No DNS server.\n\
         port=0\n\
         # The lab card alone, on its own address.\n\
         interface={card}\n\
         bind-dynamic\n\
         # Addresses for the machines in the machine files alone, by their MAC\n\
         # addresses, with no gateway and no DNS server.\n\
         dhcp-range={network},static,255.255.255.0,1h\n"
    );
    for host in hosts {
        conf.push_str(&format!("dhcp-host={},{},{}\n", host.mac, host.address, host.name));
    }
    conf.push_str(&format!(
        "dhcp-option=3\n\
         dhcp-option=6\n\
         # x86-64 UEFI firmware boots the network GRUB.\n\
         dhcp-match=set:efi-x86_64,option:client-arch,7\n\
         dhcp-match=set:efi-x86_64,option:client-arch,9\n\
         dhcp-boot=tag:efi-x86_64,{GRUB_FILE}\n\
         enable-tftp={card}\n\
         tftp-root={}\n\
         log-dhcp\n",
        tftp.display()
    ));
    conf
}

/// The service that runs dnsmasq for the lab network.
fn service_file() -> String {
    format!(
        "# The Dukovo Lab's network (ADR 0429), written by `ob lab`.\n\
         [Unit]\n\
         Description=DHCP and TFTP for the Dukovo Lab's network\n\
         After=network.target\n\
         \n\
         [Service]\n\
         ExecStart={DNSMASQ} --keep-in-foreground --conf-file={DNSMASQ_CONF} \
         --pid-file=/run/dukovo-lab-dnsmasq.pid\n\
         Restart=on-failure\n\
         RestartSec=5s\n\
         \n\
         [Install]\n\
         WantedBy=multi-user.target\n"
    )
}

/// One file to put in place with sudo, unless it is there already.
struct Install {
    from: PathBuf,
    to: PathBuf,
}

/// Sets up the lab network for the real machines in dukovo-lab's machine
/// files (`tool`'s, in `config`), as plain `ob lab` does: nothing without
/// one.
pub fn set_up(ctx: &Ctx, tool: &Path, config: &Path) -> Result<()> {
    let mut list = Command::new(tool);
    list.arg("--config").arg(config).args(["machines", "--json"]);
    let listed = process::output(ctx, &mut list)?;
    let hosts = real_machines(&String::from_utf8_lossy(&listed.stdout))?;
    let Some(first) = hosts.first() else { return Ok(()) };
    for host in &hosts[1..] {
        if (&host.lab_address, &host.tftp, &host.writer)
            != (&first.lab_address, &first.tftp, &first.writer)
        {
            bail!(
                "the machines {} and {} name different lab addresses, TFTP folders or writers; \
                 the lab network has one of each",
                first.name,
                host.name
            );
        }
    }
    let names: Vec<&str> = hosts.iter().map(|host| host.name.as_str()).collect();
    step(&format!("Setting up the lab network for {}", names.join(", ")));
    if !Path::new(DNSMASQ).is_file() {
        return Err(Error::new(format!("the lab network needs dnsmasq, at {DNSMASQ}"))
            .hint("`sudo apt install dnsmasq-base`, then ./ob lab again"));
    }
    if process::find_program("systemctl").is_none() {
        bail!("the lab network runs dnsmasq as a systemd service, and this PC has no systemctl");
    }
    let mut show = Command::new("ip");
    show.args(["-4", "-o", "address", "show"]);
    let shown = process::output(ctx, &mut show)?;
    let mut routes = Command::new("ip");
    routes.args(["-4", "route", "show", "default"]);
    let routed = process::output(ctx, &mut routes)?;
    let card = lab_card(
        &addresses(&String::from_utf8_lossy(&shown.stdout)),
        &default_cards(&String::from_utf8_lossy(&routed.stdout)),
        &first.lab_address,
    )?;

    let kit = writer_kit(ctx)?;
    let grub = net_grub(ctx)?;
    let staged = ctx.out.join("lab-net");
    let conf = staged.join("dnsmasq.conf");
    std::fs::write(&conf, dnsmasq_conf(&card, &first.lab_address, &first.tftp, &hosts))
        .context(|| format!("writing {}", conf.display()))?;
    let service = staged.join(SERVICE);
    std::fs::write(&service, service_file())
        .context(|| format!("writing {}", service.display()))?;
    let mut files = vec![
        Install { from: grub, to: first.tftp.join(GRUB_FILE) },
        Install { from: conf, to: DNSMASQ_CONF.into() },
        Install { from: service, to: SERVICE_FILE.into() },
    ];
    for name in [KIT_FILE, "vmlinuz", WRITER_INITRD] {
        files.push(Install { from: kit.join(name), to: first.writer.join(name) });
    }
    let changed: Vec<&Install> = files.iter().filter(|file| !same(&file.from, &file.to)).collect();
    let scripts = first.tftp.join(BOOT_SCRIPTS);
    let writable = writable(&scripts);
    if !changed.is_empty() || !writable {
        step("Installing the lab network's files, which takes sudo");
    }
    for file in &changed {
        let mut install = Command::new("sudo");
        install.args(["install", "-D", "-m", "644"]).arg(&file.from).arg(&file.to);
        process::run(ctx, &mut install)?;
    }
    if !writable {
        let mut whoami = Command::new("id");
        whoami.arg("-un");
        let user =
            String::from_utf8_lossy(&process::output(ctx, &mut whoami)?.stdout).trim().to_string();
        let mut folder = Command::new("sudo");
        folder.args(["install", "-d", "-m", "755", "-o", &user]).arg(&scripts);
        process::run(ctx, &mut folder)?;
    }
    let service_changed = changed.iter().any(|file| {
        file.to.as_path() == Path::new(DNSMASQ_CONF) || file.to.as_path() == Path::new(SERVICE_FILE)
    });
    if service_changed {
        for args in [
            &["systemctl", "daemon-reload"][..],
            &["systemctl", "enable", SERVICE],
            &["systemctl", "restart", SERVICE],
        ] {
            process::run(ctx, Command::new("sudo").args(args))?;
        }
    } else {
        let mut active = Command::new("systemctl");
        active.args(["is-active", "--quiet", SERVICE]);
        if !process::status(ctx, &mut active)?.success() {
            step("Starting dnsmasq for the lab network, which takes sudo");
            process::run(ctx, Command::new("sudo").args(["systemctl", "start", SERVICE]))?;
        }
    }
    eprintln!(
        "    note: dnsmasq answers on {card} alone, for {}; `dukovo-lab machines check` checks \
         the rest",
        names.join(", ")
    );
    Ok(())
}

/// Whether `to` holds what `from` does.
fn same(from: &Path, to: &Path) -> bool {
    match (std::fs::read(from), std::fs::read(to)) {
        (Ok(from), Ok(to)) => from == to,
        _ => false,
    }
}

/// Whether this user can write in `folder`.
fn writable(folder: &Path) -> bool {
    let probe = folder.join(".ob-lab-probe");
    let wrote = std::fs::write(&probe, b"").is_ok();
    let _ = std::fs::remove_file(&probe);
    wrote
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The TFTP folder, as dukovo-lab's machine files have it unless they
    /// say otherwise.
    const DEFAULT_TFTP: &str = "/srv/dukovo-lab/tftp";

    fn host(name: &str, mac: &str, address: &str) -> Host {
        Host {
            name: name.into(),
            mac: mac.into(),
            address: address.into(),
            lab_address: "10.77.0.1".into(),
            tftp: DEFAULT_TFTP.into(),
            writer: "/srv/dukovo-lab/writer".into(),
        }
    }

    #[test]
    fn reads_the_real_machines_from_dukovo_labs_list() {
        let json = r#"{"machines": [
            {"name": "qemu", "driver": "qemu", "qemu": {}},
            {"name": "optiplex", "driver": "amt", "amt": {
                "address": "10.77.0.10", "mac": "AA:BB:CC:00:11:22", "machine_address": "10.77.0.2",
                "lab_address": "10.77.0.1", "tftp": "/srv/dukovo-lab/tftp",
                "writer": "/srv/dukovo-lab/writer", "password_file": "p"}}]}"#;
        assert_eq!(
            real_machines(json).unwrap(),
            [host("optiplex", "aa:bb:cc:00:11:22", "10.77.0.2")]
        );
        let placeholder = json.replace("AA:BB:CC:00:11:22", "00:00:00:00:00:00");
        let err = real_machines(&placeholder).unwrap_err().to_string();
        assert!(err.contains("put in its network card's MAC address"), "{err}");
        assert!(real_machines(r#"{"machines": []}"#).unwrap().is_empty());
    }

    #[test]
    fn finds_the_lab_card_and_refuses_the_home_one() {
        let shown = "1: lo    inet 127.0.0.1/8 scope host lo\\       valid_lft forever\n\
                     2: wlp2s0    inet 192.168.1.20/24 brd 192.168.1.255 scope global dynamic wlp2s0\n\
                     3: enp3s0    inet 10.77.0.1/24 brd 10.77.0.255 scope global noprefixroute enp3s0\n";
        let routes = "default via 192.168.1.1 dev wlp2s0 proto dhcp src 192.168.1.20 metric 600\n";
        let (shown, routes) = (addresses(shown), default_cards(routes));
        assert_eq!(shown.len(), 3);
        assert_eq!(routes, ["wlp2s0"]);
        assert_eq!(lab_card(&shown, &routes, "10.77.0.1").unwrap(), "enp3s0");
        let err = lab_card(&shown, &routes, "10.77.0.5").unwrap_err().to_string();
        assert!(err.contains("no network card"), "{err}");
        let home = default_cards("default via 10.77.0.254 dev enp3s0\n");
        let err = lab_card(&shown, &home, "10.77.0.1").unwrap_err().to_string();
        assert!(err.contains("default route"), "{err}");
        let wide = addresses("3: enp3s0    inet 10.77.0.1/16 scope global enp3s0\n");
        assert!(lab_card(&wide, &[], "10.77.0.1").unwrap_err().to_string().contains("/16"));
    }

    #[test]
    fn dnsmasq_answers_the_lab_card_and_its_machines_alone() {
        let hosts = [host("optiplex", "aa:bb:cc:00:11:22", "10.77.0.2")];
        let conf = dnsmasq_conf("enp3s0", "10.77.0.1", Path::new(DEFAULT_TFTP), &hosts);
        for line in [
            "port=0\n",
            "interface=enp3s0\n",
            "bind-dynamic\n",
            "dhcp-range=10.77.0.0,static,255.255.255.0,1h\n",
            "dhcp-host=aa:bb:cc:00:11:22,10.77.0.2,optiplex\n",
            "dhcp-option=3\n",
            "dhcp-option=6\n",
            "dhcp-boot=tag:efi-x86_64,dukovo-lab/grubx64.efi\n",
            "enable-tftp=enp3s0\n",
            "tftp-root=/srv/dukovo-lab/tftp\n",
        ] {
            assert!(conf.contains(line), "no {line:?} in:\n{conf}");
        }
        assert!(service_file().contains(&format!("--conf-file={DNSMASQ_CONF}")));
    }

    #[test]
    fn the_writers_kit_is_one_dukovo_lab_reads() {
        let kit = kit_file();
        assert!(kit.contains("kernel = \"vmlinuz\"\n"));
        assert!(kit.contains("initrd = \"lab-writer.cpio\"\n"));
        // dukovo-lab's kits name the first serial port.
        assert!(kit.contains("cmdline = \"console=ttyS0 "), "{kit}");
        let cpio = initramfs(b"INIT", &[("00-ahci.ko.zst".into(), b"MOD".to_vec())]);
        let text = String::from_utf8_lossy(&cpio);
        for name in ["dev/console", "init", "modules/00-ahci.ko.zst", "TRAILER!!!"] {
            assert!(text.contains(name), "no {name} in the initramfs");
        }
    }
}

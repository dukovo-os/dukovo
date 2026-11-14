// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! netcheck(8) finds the layer where the network stops working (ADR
//! 0489). On the live system, with QEMU's user network, every layer
//! passes: the link, the address DHCP gave, the gateway 10.0.2.2, the DNS
//! server 10.0.2.3, a packet of the card's full size, and, as nothing is
//! sent off the local network without *--internet*, the internet layer is
//! not checked. The clock may or may not have been set by a time server,
//! as QEMU's network may or may not reach one, so the time layer passes or
//! warns with NET-141 and the exit status is 0 or 2 with it. *--card*
//! names the one card, and one that is not there fails the link layer
//! with NET-101.
//!
//! Then layers are made to fail, and netcheck must stop there, with the
//! layers after it not checked and the exit status 1: a DNS server in
//! resolved's settings that nothing answers (NET-132); the card's link
//! cut with QEMU's *set_link* (NET-102), which *--json* must give as the
//! same failure with its ID, and restored. A default IPv4 route through
//! an address nothing has, as a wrong static *router* gives it, warns
//! with NET-122, as QEMU's IPv6 router still answers and the layers after
//! it are checked through that one. With timed stopped and its status
//! removed, the time layer warns (NET-141) and the layers after it are
//! still checked.
//!
//! The update server is then the live system's own HTTPS server, openssl's
//! *s_server* as the install tests start it, whose certificate is from an
//! authority the system is told to trust: *--internet* passes it. The
//! other server's certificate nothing trusts, which fails the internet
//! layer with NET-173; and with the clock set to 2020, which timed no
//! longer keeps, the trusted server's certificate is not yet valid, which
//! fails with NET-174.

use super::super::*;
use super::install_uefi::{live_shell, wait_for_screen};
use super::install_web::serve;

pub const CASE: Case = Case {
    name: "netcheck",
    about: "netcheck finds the layer where the network stops working (ADR 0489): every layer passes under QEMU's user network and the internet layer is checked only with --internet; a card that is not there, a DNS server nothing answers and a cut link each fail their layer with its ID and stop the layers after it, which --json reports; an IPv4 gateway that is not there beside an IPv6 one that answers warns, and so does a time server not heard from; the live system's HTTPS server as update server passes, one whose authority is not trusted fails, and a clock set years back fails on the certificate's dates",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    // The live menu entry has its own flags, `live` and
    // `console-sign-in=off`, before these.
    cmdline: "",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Live,
    cpu_model: None,
    qemu: &["-netdev", "user,id=n0", "-device", "virtio-net-pci,netdev=n0"],
    script: netcheck,
    disks: &[],
    second: None,
};

/// How long a run of netcheck may take: its worst case is a gateway or a
/// DNS server asked twice for two seconds, and a TLS connection, each
/// slower without KVM.
const RUN_SECONDS: u64 = 60;

/// How long the network may take to come up, or to come back after the
/// link is restored: DHCP, then the gateway's answer.
const UP_SECONDS: u64 = 90;

/// The update server's address in updates.toml, for each of the live
/// system's HTTPS servers (userspace/programs/installer/tests/install/
/// https-server.sh).
const TRUSTED: &str = "https://127.0.0.1:8443/";
const UNTRUSTED: &str = "https://127.0.0.1:8444/";

/// What one run of netcheck showed.
struct Run {
    /// The screen after it: its output, then the exit status's line.
    lines: Vec<String>,
    /// Its exit status.
    status: u32,
}

impl Run {
    /// The line of `layer`, which starts with the layer's name.
    fn layer(&self, layer: &str) -> Result<&str, String> {
        self.lines
            .iter()
            .map(|line| line.trim_end())
            .find(|line| {
                line.strip_prefix(layer).is_some_and(|rest| rest.starts_with(' '))
            })
            .ok_or_else(|| format!("netcheck showed no line for the {layer} layer: {}", self.shown()))
    }

    /// Checks that the line of `layer` says `result` and each of `words`.
    fn expect(&self, layer: &str, result: &str, words: &[&str]) -> Result<(), String> {
        let line = self.layer(layer)?;
        let said = line.strip_prefix(layer).map_or("", str::trim_start);
        if !said.starts_with(result) {
            return Err(format!("the {layer} layer should be \"{result}\": {line}; {}", self.shown()));
        }
        match words.iter().find(|word| !self.lines.iter().any(|line| line.contains(**word))) {
            Some(missing) => Err(format!("netcheck did not say \"{missing}\": {}", self.shown())),
            None => Ok(()),
        }
    }

    /// Checks that every layer after `layer` was not checked.
    fn stops_at(&self, layer: &str, layers: &[&str]) -> Result<(), String> {
        let after = layers.iter().skip_while(|name| **name != layer).skip(1);
        for name in after {
            self.expect(name, "not checked", &[])?;
        }
        Ok(())
    }

    /// What the screen showed, on one line, for a failure's message.
    fn shown(&self) -> String {
        self.lines.iter().map(|line| line.trim()).filter(|line| !line.is_empty()).collect::<Vec<_>>().join(" | ")
    }
}

/// The layers, in the order netcheck checks them.
const LAYERS: [&str; 7] = ["link", "address", "gateway", "dns", "time", "mtu", "internet"];

/// Runs `netcheck args` in the live system's shell, with `label` to tell
/// its run from another's on the screen. The exit status is shown as 100
/// more than it is, so that the command line, which the shell shows as
/// typed, does not hold what the test waits for.
fn run(vm: &mut Vm, label: &str, args: &str) -> Result<Run, String> {
    vm.type_text(&format!("clear; netcheck {args}; echo \"{label} status: $((100 + $?))\"\n"))?;
    let needle = format!("{label} status: 10");
    wait_for_screen(vm, &needle, RUN_SECONDS)?;
    let lines = vm.screen()?;
    // The command line the shell shows holds `$((100 + $?))` where the
    // status is, which does not parse.
    let shown = format!("{label} status: ");
    let status = lines
        .iter()
        .find_map(|line| line.split_once(&shown)?.1.trim().parse::<u32>().ok())
        .and_then(|number| number.checked_sub(100))
        .ok_or_else(|| format!("netcheck's exit status is not on the screen: {}", lines.join(" | ")))?;
    Ok(Run { lines, status })
}

/// Runs the shell command `command` and checks that it succeeded.
fn sh(vm: &mut Vm, label: &str, command: &str) -> Result<(), String> {
    vm.type_text(&format!("clear; {command}; echo \"{label} exit: $((100 + $?))\"\n"))?;
    wait_for_screen(vm, &format!("{label} exit: 10"), RUN_SECONDS)?;
    if vm.screen()?.iter().any(|line| line.contains(&format!("{label} exit: 100"))) {
        Ok(())
    } else {
        Err(format!("`{command}` failed: {}", vm.screen()?.join(" | ")))
    }
}

/// Runs netcheck until the network is up, as DHCP and the gateway's
/// answer take a moment after the live system starts or the link is
/// restored, and gives the run in which the layers up to the MTU's are
/// as they should be: those the link to the gateway and the DNS server
/// make, the clock's layer may pass or warn.
fn up(vm: &mut Vm, label: &str) -> Result<Run, String> {
    let deadline = Instant::now() + Duration::from_secs(UP_SECONDS);
    let mut attempt = 0;
    loop {
        attempt += 1;
        let run = run(vm, &format!("{label}{attempt}"), "")?;
        let layers = ["link", "address", "gateway", "dns", "mtu"];
        if layers.iter().all(|layer| run.expect(layer, "passed", &[]).is_ok()) {
            return Ok(run);
        }
        if Instant::now() >= deadline {
            return Err(format!("the network did not come up: {}", run.shown()));
        }
        std::thread::sleep(Duration::from_secs(3));
    }
}

/// Runs netcheck until its `layer` fails, as the kernel takes a moment to
/// see the link cut, and gives that run.
fn until_failed(vm: &mut Vm, label: &str, layer: &str) -> Result<Run, String> {
    let deadline = Instant::now() + Duration::from_secs(UP_SECONDS);
    let mut attempt = 0;
    loop {
        attempt += 1;
        let run = run(vm, &format!("{label}{attempt}"), "")?;
        if run.expect(layer, "FAILED", &[]).is_ok() {
            return Ok(run);
        }
        if Instant::now() >= deadline {
            return Err(format!("netcheck never failed the {layer} layer: {}", run.shown()));
        }
        std::thread::sleep(Duration::from_secs(2));
    }
}

/// Points the update settings at `server`.
fn update_server(vm: &mut Vm, label: &str, server: &str) -> Result<(), String> {
    sh(
        vm,
        label,
        &format!(
            "mkdir -p /system/settings/sysupdate; echo 'server = \"{server}\"' > \
             /system/settings/sysupdate/updates.toml"
        ),
    )
}

fn netcheck(vm: &mut Vm, _case: &Case) -> Result<(), String> {
    live_shell(vm)?;

    // Every layer passes, and the internet layer is not checked: nothing
    // is sent off the local network. The clock's layer passes if QEMU's
    // network reached a time server, and warns if not.
    let healthy = up(vm, "healthy")?;
    healthy.expect("address", "passed", &["10.0.2.15"])?;
    healthy.expect("gateway", "passed", &["10.0.2.2 answers in"])?;
    healthy.expect("dns", "passed", &["10.0.2.3 answers"])?;
    healthy.expect("mtu", "passed", &["packets of 1500 bytes reach 10.0.2.2"])?;
    healthy.expect("internet", "not checked", &["it sends nothing off the local network"])?;
    let time = healthy.layer("time")?;
    let (passed, warned) = (time.contains(" passed "), time.contains("NET-141"));
    if !(passed || warned) {
        return Err(format!("the time layer neither passed nor warned: {time}"));
    }
    let (summary, status) = if passed {
        ("every layer passed", 0)
    } else {
        ("passed, with warnings", 2)
    };
    healthy.expect("link", "passed", &[&format!("netcheck: {summary}.")])?;
    if healthy.status != status {
        return Err(format!("netcheck's exit status was {}, not {status}", healthy.status));
    }

    // One card, and one that is not there.
    let card = run(vm, "card", "--card net0")?;
    card.expect("link", "passed", &["net0 has a link"])?;
    card.expect("gateway", "passed", &[])?;
    let missing = run(vm, "missing", "--card net9")?;
    missing.expect("link", "FAILED", &["NET-101", "no network card is named net9"])?;
    missing.stops_at("link", &LAYERS)?;
    if missing.status != 1 {
        return Err(format!("a missing card gave the exit status {}, not 1", missing.status));
    }

    // A DNS server nothing answers: on the card's network, so asked
    // without --internet. The layers before it passed, and those after it
    // are not checked.
    sh(vm, "dnsset", "echo 'servers = 10.0.2.99' > /system/settings/resolved")?;
    let dns = run(vm, "dns", "")?;
    dns.expect("gateway", "passed", &[])?;
    dns.expect("dns", "FAILED", &["NET-132", "10.0.2.99"])?;
    dns.stops_at("dns", &LAYERS)?;
    if dns.status != 1 {
        return Err(format!("a dead DNS server gave the exit status {}, not 1", dns.status));
    }
    sh(vm, "dnsreset", "echo > /system/settings/resolved")?;
    up(vm, "dnsback")?;

    // An IPv4 gateway that is not there: the route a wrong static
    // `router` makes, set at once with ip(8) as netd's reload would. QEMU's
    // IPv6 router still answers, so it is a warning, and the layers after
    // it are checked through that router.
    sh(vm, "route", "ip route replace default via 10.0.2.99 dev net0")?;
    let gateway = run(vm, "gateway", "")?;
    gateway.expect("address", "passed", &[])?;
    gateway.expect("gateway", "warning", &["NET-122", "10.0.2.99", "fe80::2 answers in"])?;
    gateway.expect("dns", "passed", &[])?;
    gateway.expect("mtu", "passed", &["fe80::2"])?;
    if gateway.status != 2 {
        return Err(format!("a wrong IPv4 gateway gave the exit status {}, not 2", gateway.status));
    }
    sh(vm, "routeback", "ip route replace default via 10.0.2.2 dev net0")?;
    up(vm, "routed")?;

    // The link cut, as a cable pulled: the first layer fails, and --json
    // gives the same, with the status, the ID and each layer.
    vm.monitor("set_link n0 off")?;
    let cut = until_failed(vm, "cut", "link")?;
    cut.expect("link", "FAILED", &["NET-102", "net0 has no link"])?;
    cut.stops_at("link", &LAYERS)?;
    if cut.status != 1 {
        return Err(format!("a cut link gave the exit status {}, not 1", cut.status));
    }
    sh(
        vm,
        "json",
        // netcheck's own exit status is 1 here; the command's is grep's.
        "netcheck --json > /tmp/netcheck.json; grep -F -e '\"status\"' -e '\"id\": \"NET' \
         -e '\"layer\"' /tmp/netcheck.json",
    )?;
    let json = vm.screen()?;
    let screen = json.join(" | ");
    for needle in [
        "\"status\": \"failed\"",
        "\"id\": \"NET-102\"",
        "\"layer\": \"link\"",
        "\"layer\": \"address\"",
        "\"layer\": \"gateway\"",
        "\"layer\": \"dns\"",
        "\"layer\": \"time\"",
        "\"layer\": \"mtu\"",
        "\"layer\": \"internet\"",
    ] {
        if !json.iter().any(|line| line.contains(needle)) {
            return Err(format!("netcheck --json did not give {needle}: {screen}"));
        }
    }
    vm.monitor("set_link n0 on")?;
    up(vm, "linked")?;

    // timed is stopped and its status removed, as on a machine that never
    // heard from a time server: a warning, and the layers after it are
    // still checked.
    sh(vm, "timed", "service timed stop; rm -f /system/live/timed/status")?;
    let unsynced = run(vm, "unsynced", "")?;
    unsynced.expect("gateway", "passed", &[])?;
    unsynced.expect("time", "warning", &["NET-141"])?;
    unsynced.expect("mtu", "passed", &["packets of 1500 bytes reach 10.0.2.2"])?;
    unsynced.expect("internet", "not checked", &["passed, with warnings."])?;
    if unsynced.status != 2 {
        return Err(format!("a warning gave the exit status {}, not 2", unsynced.status));
    }

    // The update server is the live system's own, whose authority the
    // system trusts.
    serve(vm)?;
    update_server(vm, "trusted", TRUSTED)?;
    let trusted = run(vm, "internet", "--internet")?;
    trusted.expect("dns", "passed", &["10.0.2.3 answers"])?;
    trusted.expect("internet", "passed", &["127.0.0.1 answers on port 8443"])?;
    if trusted.status != 2 {
        return Err(format!(
            "--internet gave the exit status {}, not 2, with only the clock's warning",
            trusted.status
        ));
    }

    // The other server's certificate is self-signed.
    update_server(vm, "untrusted", UNTRUSTED)?;
    let untrusted = run(vm, "untrusted", "--internet")?;
    untrusted.expect("time", "warning", &["NET-141"])?;
    untrusted.expect("internet", "FAILED", &["NET-173", "something answers for 127.0.0.1"])?;
    if untrusted.status != 1 {
        return Err(format!("an untrusted certificate gave the exit status {}, not 1", untrusted.status));
    }

    // A clock years back: the trusted server's certificate is not yet
    // valid by it. timed is stopped, so that it does not keep the clock.
    update_server(vm, "trustedagain", TRUSTED)?;
    sh(vm, "clock", "timedatectl set-time 2020-01-01 12:00:00")?;
    let early = run(vm, "early", "--internet")?;
    early.expect("internet", "FAILED", &["NET-174"])?;
    if early.status != 1 {
        return Err(format!("a clock set back gave the exit status {}, not 1", early.status));
    }

    vm.type_text("poweroff\n")?;
    vm.expect("spark: asked to power off the machine")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}

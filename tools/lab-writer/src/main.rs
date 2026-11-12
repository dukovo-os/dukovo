// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The Dukovo Lab's disk writer (ADR 0429): the init of a small Linux
//! system that a real lab machine boots from the lab network before each
//! fresh boot. It writes the image under test to the one disk with the
//! serial number the lab names, reads it back, and restarts the machine
//! into it.
//!
//! The lab's side is dukovo-os/lab's docs/net-boot.md. The kernel's
//! command line carries what the writer needs:
//!
//! - `dukovo.lab=` the boot's base URL, `http://10.77.0.1:7431/<token>/`;
//! - `dukovo.lab.ip=` its own address with its prefix, `10.77.0.2/24`;
//! - `dukovo.lab.mac=` the MAC address of the card to give it.
//!
//! Then, saying on the console what it does as it goes:
//!
//! 1. It loads the disk and network drivers `ob lab` put in `/modules`,
//!    sets the card's address and waits for its link.
//! 2. It reads the plan (`GET plan`) and finds the disk with the plan's
//!    serial number: no other disk is ever opened for writing, and none
//!    is written if two have that number.
//! 3. It zeroes the image's runs of zeros and the disk's last mebibyte,
//!    writes the data as it streams in (`GET data`), flushes the disk's
//!    cache and reads the image's bytes back past the page cache.
//! 4. It posts their SHA-256 (`POST done`). On the lab's 200 and
//!    `restart` it restarts the machine, into its disk; on any other
//!    answer, or any failure (after `POST failed`), it powers the machine
//!    off. It never restarts into the network.

mod disk;
mod http;
mod net;
mod plan;

use std::io::Write as _;
use std::os::fd::AsRawFd as _;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use http::{Base, Client};
use net::Address;
use plan::Plan;

/// Where `ob lab` puts the kernel modules for the machine's disks and
/// cards, named so that they load in the order of their names.
const MODULES: &str = "/modules";

/// finit_module(2)'s flag for a compressed module, which the kernel
/// uncompresses itself.
const MODULE_INIT_COMPRESSED_FILE: libc::c_uint = 4;

/// How long the card and the disk may take to show up once their drivers
/// load, and the card's link to come up.
const DEVICE_TIME: Duration = Duration::from_secs(60);

/// What the writer says on the console before each line.
const SAYS: &str = "lab-writer:";

/// What the kernel's command line gives the writer.
#[derive(Debug, PartialEq, Eq)]
struct Settings {
    base: Base,
    address: Address,
    mac: String,
}

impl Settings {
    /// Reads the writer's words from the kernel's command line.
    fn parse(cmdline: &str) -> Result<Settings, String> {
        let word = |name: &str| {
            cmdline
                .split_whitespace()
                .filter_map(|word| word.strip_prefix(name)?.strip_prefix('='))
                .next_back()
                .ok_or_else(|| format!("the kernel's command line has no {name}="))
        };
        Ok(Settings {
            base: Base::parse(word("dukovo.lab")?)?,
            address: Address::parse(word("dukovo.lab.ip")?)?,
            mac: net::parse_mac(word("dukovo.lab.mac")?)?,
        })
    }
}

/// How the write ends.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Ending {
    /// Into the disk just written.
    Restart,
    PowerOff,
}

fn main() {
    let init = std::process::id() == 1;
    if init {
        mount("proc", "/proc");
        mount("sysfs", "/sys");
        mount("devtmpfs", "/dev");
    }
    // A panic is a failure like any other: say it, and power off.
    std::panic::set_hook(Box::new(|info| {
        say(&format!("failed: {info}"));
        finish(Ending::PowerOff);
    }));
    say("the Dukovo Lab's disk writer is starting");
    let cmdline = std::fs::read_to_string("/proc/cmdline").unwrap_or_default();
    let settings = match Settings::parse(&cmdline) {
        Ok(settings) => settings,
        Err(err) => {
            say(&format!("failed: {err}"));
            finish(Ending::PowerOff);
        }
    };
    if init {
        load_modules();
    }
    let client = Client::new(settings.base.clone());
    let ending = match run(&settings, &client) {
        Ok(ending) => ending,
        Err(err) => {
            say(&format!("failed: {err}"));
            // The lab hears of it, if the network got that far.
            let _ = client.post("failed", "text/plain", err.as_bytes());
            Ending::PowerOff
        }
    };
    finish(ending);
}

/// Writes the image, and says how the boot ends.
fn run(settings: &Settings, client: &Client) -> Result<Ending, String> {
    let card = net::configure(&settings.mac, settings.address, DEVICE_TIME)?;
    say(&format!(
        "{card} is {}/{}; the lab PC is at {}",
        settings.address.ip,
        settings.address.prefix,
        settings.base.url()
    ));
    let answer = client.get("plan")?;
    if answer.status != 200 {
        return Err(format!("the lab answered {} for the plan: {}", answer.status, answer.text()));
    }
    let plan = Plan::parse(&answer.body)?;
    tell(
        client,
        &format!(
            "the plan: {} bytes, {} of them data in {} runs, to the disk {}",
            plan.size,
            plan.data_bytes(),
            plan.extents.len(),
            plan.disk
        ),
    );
    let disk = disk::find(&plan.disk, plan.size, DEVICE_TIME)?;
    tell(
        client,
        &format!(
            "writing {} ({} bytes), serial number {}",
            disk.device.display(),
            disk.bytes,
            plan.disk
        ),
    );
    let (status, length, mut data) = client.open("data")?;
    if status != 200 || length != plan.data_bytes() {
        return Err(format!(
            "the lab answered {status} with {length} bytes for the data, not 200 with {}",
            plan.data_bytes()
        ));
    }
    let started = Instant::now();
    let mut progress = Tenths { said: 0 };
    let sha256 = disk::write(&disk.device, disk.bytes, &plan, &mut data, &mut progress)?;
    tell(
        client,
        &format!("wrote and read back in {} s: SHA-256 {sha256}", started.elapsed().as_secs()),
    );
    let done = format!("{{\"sha256\": \"{sha256}\"}}");
    let answer = client.post("done", "application/json", done.as_bytes())?;
    if answer.status == 200 && answer.text() == "restart" {
        say("done; restarting into the disk");
        Ok(Ending::Restart)
    } else {
        say(&format!(
            "the lab answered {} ({}): the disk does not hold the image; powering off",
            answer.status,
            answer.text()
        ));
        Ok(Ending::PowerOff)
    }
}

/// Says each tenth of the data written.
struct Tenths {
    said: u64,
}

impl disk::Progress for Tenths {
    fn wrote(&mut self, done: u64, total: u64) {
        let tenths = done * 10 / total.max(1);
        if tenths > self.said {
            self.said = tenths;
            let mib = |bytes: u64| bytes as f64 / disk::MIB as f64;
            say(&format!("wrote {}% ({:.1} of {:.1} MiB)", tenths * 10, mib(done), mib(total)));
        }
    }
}

/// Says `text` on the console.
fn say(text: &str) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{SAYS} {text}");
    let _ = out.flush();
}

/// Says `text` on the console and in the lab's log.
fn tell(client: &Client, text: &str) {
    say(text);
    let _ = client.post("log", "text/plain", text.as_bytes());
}

/// Ends the boot: restarts or powers off the machine, as its process 1,
/// once what it has written is on the disk and the console has sent its
/// last lines. Outside a machine of its own (a test), it exits.
fn finish(ending: Ending) -> ! {
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
    for fd in [std::io::stdout().as_raw_fd(), std::io::stderr().as_raw_fd()] {
        // SAFETY: tcdrain only waits; on a descriptor that is not a
        // terminal it fails at once.
        unsafe { libc::tcdrain(fd) };
    }
    if std::process::id() != 1 {
        std::process::exit(i32::from(ending != Ending::Restart));
    }
    // SAFETY: sync takes nothing and cannot fail.
    unsafe { libc::sync() };
    let command = match ending {
        Ending::Restart => libc::LINUX_REBOOT_CMD_RESTART,
        Ending::PowerOff => libc::LINUX_REBOOT_CMD_POWER_OFF,
    };
    // SAFETY: as process 1, this program may restart or power off the
    // machine; everything it wrote is on the disk.
    unsafe { libc::reboot(command) };
    loop {
        std::thread::park();
    }
}

/// Mounts a filesystem of kind `kind` on `at`, which the writer's init
/// finds empty: `/proc`, `/sys`, and `/dev` as the kernel's devtmpfs,
/// where the disks are.
fn mount(kind: &str, at: &str) {
    let _ = std::fs::create_dir_all(at);
    let (Ok(kind_c), Ok(at_c)) = (std::ffi::CString::new(kind), std::ffi::CString::new(at)) else {
        return;
    };
    // SAFETY: the strings are NUL-terminated and live through the call;
    // these filesystems take no data.
    let result = unsafe {
        libc::mount(kind_c.as_ptr(), at_c.as_ptr(), kind_c.as_ptr(), 0, std::ptr::null())
    };
    if result != 0 {
        say(&format!("cannot mount {at}: {}", std::io::Error::last_os_error()));
    }
}

/// Loads the kernel modules in [`MODULES`], in the order of their names.
/// One that fails is said and passed over: the machine may not need it.
fn load_modules() {
    let Ok(entries) = std::fs::read_dir(MODULES) else { return };
    let mut paths: Vec<PathBuf> = entries.filter_map(|entry| Some(entry.ok()?.path())).collect();
    paths.sort();
    for path in paths {
        let file = match std::fs::File::open(&path) {
            Ok(file) => file,
            Err(err) => {
                say(&format!("cannot open {}: {err}", path.display()));
                continue;
            }
        };
        let compressed = path.extension().is_some_and(|e| e == "zst" || e == "xz" || e == "gz");
        let flags = if compressed { MODULE_INIT_COMPRESSED_FILE } else { 0 };
        // SAFETY: the descriptor is open, and the parameters are an empty
        // NUL-terminated string that lives through the call.
        let result =
            unsafe { libc::syscall(libc::SYS_finit_module, file.as_raw_fd(), c"".as_ptr(), flags) };
        let err = std::io::Error::last_os_error();
        // EEXIST: built in, or loaded already. ENODEV: no such hardware.
        if result != 0 && !matches!(err.raw_os_error(), Some(libc::EEXIST | libc::ENODEV)) {
            say(&format!("cannot load {}: {err}", path.display()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, SocketAddrV4};

    #[test]
    fn reads_its_words_from_the_kernels_command_line() {
        let settings = Settings::parse(
            "console=ttyS0 loglevel=4 console=ttyS4,115200 \
             dukovo.lab=http://10.77.0.1:7431/0f1e2d/ dukovo.lab.ip=10.77.0.2/24 \
             dukovo.lab.mac=AA:BB:CC:00:11:22\n",
        )
        .unwrap();
        assert_eq!(settings.base.server, SocketAddrV4::new(Ipv4Addr::new(10, 77, 0, 1), 7431));
        assert_eq!(settings.base.path, "/0f1e2d/");
        assert_eq!(settings.address, Address { ip: Ipv4Addr::new(10, 77, 0, 2), prefix: 24 });
        assert_eq!(settings.mac, "aa:bb:cc:00:11:22");
        let err = Settings::parse("console=ttyS0 dukovo.lab=http://10.77.0.1/t/").unwrap_err();
        assert!(err.contains("no dukovo.lab.ip="), "{err}");
        let err = Settings::parse("dukovo.labx=1").unwrap_err();
        assert!(err.contains("no dukovo.lab="), "{err}");
    }
}

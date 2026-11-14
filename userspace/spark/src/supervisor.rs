// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Spark as the supervisor of the system's services (Phase 9, ADR 0101).
//!
//! Each service runs in a resource domain of its own below `services`,
//! the domain the kernel gave Spark (radix-resource-domain(2)), with the
//! CPU weight, memory limit and out-of-memory class its file gives
//! (`services`). Its standard output and error are a pipe whose lines
//! Spark copies to the kernel log, each after the service's name, where
//! logd(8) finds them by Spark's process number (ADR 0210); its input is
//! _/devices/null_. When it ends
//! Spark starts it again as its file says, waiting 0.1 s after a quick
//! end, then twice as long after each one in a row, up to 30 s; a service
//! the out-of-memory policy stopped ends as if killed, and is started
//! again the same way.
//!
//! `service reload` sends a running service SIGHUP, as systemd's
//! ExecReload does for a daemon that reads its settings again on it.
//! Every stop of a service (`service stop` and `restart`, a stop because
//! a service it requires stopped, a start that was not ready in time, a
//! restart for failed health checks, and shutting down) sends the signal
//! its file's `[stop]` names, SIGTERM unless it names another, and stops
//! the service's domain if it has not ended once the file's `timeout` has
//! passed, 5 s unless it says, as systemd sends SIGKILL after
//! TimeoutStopSec (ADR 0477). Once a service's process ends, the
//! processes it left in its domain end too, as systemd's default
//! KillMode=control-group ends them: Spark stops the domain, and starts
//! the service again only once the domain is empty.
//!
//! A service's sockets (`[[sockets]]`, ADR 0477) are Spark's: it makes
//! them at boot, before any service starts, holds them across restarts,
//! and passes them to each start from descriptor 3, as systemd's socket
//! activation does (`sockets`). A service that starts `on-connection`
//! is not started at boot: it waits, listening, and the first connection
//! or datagram at one of its sockets starts it; once it ends, or is
//! stopped, it waits again. `service stop` of any other closes its
//! sockets, unless asked to keep them (`--keep-sockets`, as bundled's
//! update asks), and so does giving up on a service. A service with a
//! `[timer]` is not started at boot either: timerd(8) starts it (`start
//! NAME --by timer`, ADR 0478). Enabling a service does for it what boot
//! does (`start NAME --as-at-boot`, which `service enable` sends): an
//! on-connection service waits, and one with a timer is left to timerd.
//!
//! Spark hears of its children's ends from a notification the kernel
//! signals (radix-process(2)'s `notify_children`), and waits in `poll`
//! for it, the services' output, the program it was asked to run, and the
//! control socket, `/system/live/spark/control`: shutdown(8), reboot(8),
//! poweroff(8), halt(8) and service(8) send a request there, one line, and
//! read the answer. Shutting down stops the services the highest stop
//! rank first, so that each stops before the services it names (ADR
//! 0470): each service of a group gets its own stop signal at once, and
//! its domain is stopped once its own timeout has passed; the group is
//! done when each of them has ended. Then Spark calls the power
//! capability (radix-power(2)). Pressing
//! the power button, which the kernel tells of through another
//! notification (`notify_button`), powers the machine off the same way,
//! at once, as systemd-logind's default HandlePowerKey=poweroff does. The
//! host of a virtual machine asking it to restart, which the kernel tells
//! of through a third (`notify_restart`, ADR 0424), restarts it the same
//! way, at once.
//!
//! Once it supervises, Spark starts the hardware watchdog, if the kernel
//! drives one, with a timeout of WATCHDOG_SECONDS, and pets it each third
//! of that from its loop (ADR 0143): a Spark that stops responding, or a
//! kernel that stops running it, restarts the machine, as systemd's
//! RuntimeWatchdogSec does.
//!
//! A driver program's service (`devices.pci`) gets the cards it drives
//! from the root device capability the kernel gave Spark (radix-device(2),
//! ADR 0198): Spark opens each card that matches, up to DEVICE_SLOTS,
//! gives the service the card's capability in SLOT_DEVICE and the slots
//! after it, and keeps no copy, so the card stops when the service ends
//! and is free for the service's next start. A service whose file names
//! cards of which the kernel left none is not started.

/// Logs a security event of Spark's: the line with authpriv's `<86>` in
/// front, which logd(8) keeps in the audit log too (ADR 0211).
macro_rules! audit {
    ($($arg:tt)*) => { println!("<86>spark: {}", format_args!($($arg)*)) };
}

mod certd;
mod health;
mod show;
mod starting;
pub mod trial;
mod watch;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use libradix::abi::capability::{self, ObjectKind, Rights};
use libradix::abi::device::{self, DeviceInfo};
use libradix::abi::directory::{FileKind, OpenFlags};
use libradix::abi::errors::Kernel;
use libradix::abi::file::{self as file_abi, DescriptionFlags};
use libradix::abi::notification::WaitFlags;
use libradix::abi::process::{
    self, ChildWaitFlags, ClockKind, LimitKind, PipeFlags, PollEntry, PollEvents, PollFlags,
};
use libradix::abi::resource_domain::{self, NO_LIMIT};
use libradix::abi::startup::{
    DEVICE_SLOTS, SLOT_CLOCK, SLOT_DEVICE, SLOT_FD_BASE, SLOT_FIREWALL, SLOT_FIRST_EXTRA, SLOT_LOG,
    SLOT_NETWORK, SLOT_PLATFORM, SLOT_PROCESS, SLOT_RANDOM, SLOT_ROOT, SLOT_SETTINGS,
};
use libradix::abi::{names as names_abi, network, power, signal, syscalls};
use radix_rt::fs::{Dir, File};
use radix_rt::{SYS, println, slots};

use crate::services::{self, Restart, Spec};
use crate::{certificates, sockets, views};
use manifest::service::{ReadyWhen, Reopen};
use spark_api::bisect;
use spark_api::show::Request;
use starting::Wanted;
use watch::{Told, Verb, Watch, say};

/// Where the service files are.
pub const SERVICES_DIR: &str = "system/settings/spark/services";

/// The control socket, and the directory it is in.
const CONTROL_DIR: &str = "system/live/spark";
const CONTROL: &str = "system/live/spark/control";

/// The slots a POSIX program's capability space starts with: room for 64
/// file descriptors, as Linux's first table of them has. The C library
/// grows the space as more are opened, so a program pays for the room it
/// uses, and a fork copies no more.
pub const POSIX_SLOTS: u32 = 128;

/// The environment a POSIX program starts with: text is UTF-8 (the
/// C.UTF-8 locale, radix-posix(7)), and nano is the editor programs
/// open a file in for its user (ADR 0090).
pub const POSIX_ENV: [&str; 5] =
    ["PATH=/system/commands", "HOME=/", "TERM=dumb", "LANG=C.UTF-8", "EDITOR=nano"];

const MS: u64 = 1_000_000;
const SECOND: u64 = 1_000 * MS;

/// The services Rescue starts of those enabled (ADR 0460): the log, the
/// firewall, notices, the screen reader and its sound and braille, and
/// the consoles, where dukovo-rescue(8) runs. The rest, networking among
/// them, stay off until someone starts them.
pub const RESCUE_SERVICES: [&str; 8] =
    ["logd", "firewall", "notifyd", "audiod", "readerd", "brailled", "consoled", "serial-console"];

/// Where the system's own service files are: a service with none here
/// was added, by an app or an administrator, and stays off at a start
/// with `essentials` (ADR 0501).
const ESSENTIALS: &str = "system/resources/default-settings/spark/services";

/// Where changes bisect keeps the lists of services a try leaves out
/// (ADR 0475).
const BISECT: &str = "system/state/bisect";

/// The hardware watchdog's timeout, unless `--watchdog` gives another.
pub const WATCHDOG_SECONDS: u32 = 60;

/// A service that ran this long before it ended starts again at once.
const STEADY: u64 = 10 * SECOND;
/// The first wait before a service that ended quickly starts again, and
/// the longest.
const BACKOFF_FIRST: u64 = 100 * MS;
const BACKOFF_MAX: u64 = 30 * SECOND;
/// The ends soon after their start, each well, in a row after which an
/// on-connection service waits the backoff before a connection starts it
/// again: one that ends after each connection it serves starts again at
/// once, and one that never takes the connection that waits does not
/// start again and again for good, as systemd's TriggerLimitBurst= holds
/// a socket.
const QUICK_ENDS_FREE: u32 = 20;
/// How often Spark looks whether what a service left behind has ended.
const DRAIN_POLL: u64 = 100 * MS;
/// How long shutting down waits for a group of services after the last
/// of their timeouts has passed and their domains were stopped.
const KILL_WAIT: u64 = SECOND;

/// The longest line of a service's output kept; longer ones are cut.
const LINE_MAX: usize = 1024;
/// The longest last line of a service's output `show` gives.
const LAST_LINE_MAX: usize = 200;

/// The most control connections served at once.
const CONNECTIONS_MAX: usize = 8;
/// How long a control connection may take to send its request: one that
/// sends nothing does not keep its place for good.
const CONNECTION_WAIT: u64 = 10 * SECOND;

/// The monotonic clock, in nanoseconds.
pub fn now() -> u64 {
    process::clock(&SYS, SLOT_PROCESS, ClockKind::Monotonic).map_or(0, |c| c.ns)
}

/// What a wait status says, in words.
fn ending(status: u32) -> String {
    if status & 0x7f == 0 {
        format!("exited with code {:#x}", status >> 8 & 0xff)
    } else {
        format!("was ended by signal {}", status & 0x7f)
    }
}

/// Whether a wait status is a failure: an exit code other than 0, or a
/// signal.
fn failed(status: u32) -> bool {
    status != 0
}

/// The block `exec` takes (radix-process(2)): `path` and `args` as the
/// arguments, POSIX_ENV with `env` over it, and the working directory `/`
/// and `path` as the extra strings.
pub fn exec_block(path: &str, args: &[&str], env: &[String]) -> Vec<u8> {
    let mut environment: Vec<&str> = POSIX_ENV
        .iter()
        .copied()
        .filter(|base| {
            let name = base.split('=').next().unwrap_or("");
            !env.iter().any(|e| e.split('=').next() == Some(name))
        })
        .collect();
    environment.extend(env.iter().map(String::as_str));
    let mut block = Vec::new();
    let header = [process::EXEC_VERSION, 1 + args.len() as u32, environment.len() as u32, 2];
    for word in header {
        block.extend_from_slice(&word.to_le_bytes());
    }
    let strings =
        [path].into_iter().chain(args.iter().copied()).chain(environment).chain(["/", path]);
    for string in strings {
        block.extend_from_slice(string.as_bytes());
        block.push(0);
    }
    block
}

/// Makes the description in `slot` not wait.
fn non_blocking(slot: u32) {
    let _ = file_abi::status_flags(&SYS, slot, true, DescriptionFlags::NON_BLOCKING);
}

/// Account `name`, which a service's file names (ADR 0385): its numbers,
/// from the accounts file, with `groups`, the numbers of the groups its
/// file adds (ADR 0477), and Spark's `root` acting for it, stamped with
/// the `names` capability. Root's number is refused: a service that acts
/// for root names no account.
fn account(
    root: &Dir,
    names: Option<u32>,
    name: &str,
    groups: Vec<u32>,
) -> Result<views::Account, String> {
    let names =
        names.ok_or("the kernel gave Spark no names capability, which runs it as its account")?;
    let (user, group) = account_numbers(root, name)?;
    let slot = slots::take().ok_or("no free slot")?;
    let others = views::group_bytes(&groups);
    if let Err(status) = names_abi::stamp(&SYS, names, root.slot(), slot, user, group, &others) {
        slots::give_back(slot);
        return Err(format!("cannot act for {name}: {:#x}", status.0));
    }
    let root = Dir::own(slot).map_err(|err| format!("cannot act for {name}: {err}"))?;
    Ok(views::Account { root, user, group, groups, names })
}

/// The numbers of the groups `names`, from the groups file (ADR 0477),
/// with a line for each that is not there: a service runs without a
/// group that is gone, as when the bundle that offered it a socket was
/// removed, which reaches no more than it could before.
fn group_numbers(root: &Dir, service: &str, names: &[String]) -> Vec<u32> {
    if names.is_empty() {
        return Vec::new();
    }
    let text = root.read_file(&manifest::service::GROUP[1..]).unwrap_or_default();
    let text = core::str::from_utf8(&text).unwrap_or_default();
    names
        .iter()
        .filter_map(|name| {
            let number = manifest::service::group_number(text, name);
            if number.is_none() {
                println!("spark: {service}: there is no group {name}; it runs without it");
            }
            number
        })
        .collect()
}

/// Account `name`'s user and group numbers, from the accounts file. Root's
/// number is refused: a service that acts for root names no account.
fn account_numbers(root: &Dir, name: &str) -> Result<(u32, u32), String> {
    let passwd = root
        .read_file(&manifest::service::PASSWD[1..])
        .map_err(|err| format!("{}: {err}", manifest::service::PASSWD))?;
    let (user, group) = core::str::from_utf8(&passwd)
        .ok()
        .and_then(|text| manifest::service::account_numbers(text, name))
        .ok_or_else(|| format!("there is no account {name}"))?;
    if user == 0 {
        return Err(format!(
            "{name} has root's number: a service that acts for root names no account"
        ));
    }
    Ok((user, group))
}

/// Gives `child` the network capabilities `spec` asks for, derived from
/// Spark's `network`, in the slots from SLOT_NETWORK: one to connect out,
/// to the machines on this one's networks too (the local right, ADR
/// 0337), one to listen on each range of ports, with the inbound right,
/// so that other machines reach the service there through the firewall
/// (ADR 0334), and for admin.network one with only the control right,
/// which sets the interfaces up (ADR 0190). They have the copy and
/// transfer rights, so that they last through `fork` (ADR 0183).
fn grant_network(network: Option<u32>, spec: &Spec, child: u32) -> Result<(), String> {
    if spec.network_grants() == 0 {
        return Ok(());
    }
    let network = network.ok_or("Spark has no network capability to give")?;
    let kept = Rights::COPY.union(Rights::TRANSFER);
    let outbound =
        spec.outbound.then_some((0, 1 << 16, Rights::SEND.union(Rights::LOCAL).union(kept)));
    let listen = spec
        .listen
        .iter()
        .map(|&(first, count)| (first, count, Rights::RECEIVE.union(Rights::INBOUND).union(kept)));
    let configure = spec.configure.then_some((0, 1 << 16, Rights::CONTROL.union(kept)));
    let grants = outbound.into_iter().chain(listen).chain(configure);
    for (to, (first, count, rights)) in (SLOT_NETWORK..).zip(grants) {
        let slot = slots::take().ok_or("no free slot")?;
        let given = network::restrict(&SYS, network, slot, first, count, rights)
            .and_then(|()| process::share(&SYS, child, slot, to, Rights::ALL, 0));
        let _ = capability::delete(&SYS, slot);
        slots::give_back(slot);
        given.map_err(|status| format!("giving it the network failed: {:#x}", status.0))?;
    }
    Ok(())
}

/// Gives `child` the platform capability in SLOT_PLATFORM if `spec` asks
/// for admin.hardware, with the read and control rights, and the copy and
/// transfer rights so that it lasts through `fork` (ADR 0208).
fn grant_platform(platform: Option<u32>, spec: &Spec, child: u32) -> Result<(), String> {
    if !spec.hardware {
        return Ok(());
    }
    let platform = platform.ok_or("Spark has no platform capability to give")?;
    let rights = Rights::READ.union(Rights::CONTROL).union(Rights::COPY).union(Rights::TRANSFER);
    process::share(&SYS, child, platform, SLOT_PLATFORM, rights, 0)
        .map_err(|status| format!("giving it the platform failed: {:#x}", status.0))
}

/// Gives `child` the clock capability in SLOT_CLOCK if `spec` asks for
/// admin.time, with the read and control rights, and the copy and
/// transfer rights so that it lasts through `fork` (ADR 0194).
fn grant_clock(clock: Option<u32>, spec: &Spec, child: u32) -> Result<(), String> {
    if !spec.time {
        return Ok(());
    }
    let clock = clock.ok_or("Spark has no clock capability to give")?;
    let rights = Rights::READ.union(Rights::CONTROL).union(Rights::COPY).union(Rights::TRANSFER);
    process::share(&SYS, child, clock, SLOT_CLOCK, rights, 0)
        .map_err(|status| format!("giving it the clock failed: {:#x}", status.0))
}

/// Gives `child` the firewall capability in SLOT_FIREWALL if `spec` asks
/// for admin.firewall, with the read and control rights, and the copy and
/// transfer rights so that it lasts through `fork` (ADR 0334).
fn grant_firewall(firewall: Option<u32>, spec: &Spec, child: u32) -> Result<(), String> {
    if !spec.firewall {
        return Ok(());
    }
    let firewall = firewall.ok_or("Spark has no firewall capability to give")?;
    let rights = Rights::READ.union(Rights::CONTROL).union(Rights::COPY).union(Rights::TRANSFER);
    process::share(&SYS, child, firewall, SLOT_FIREWALL, rights, 0)
        .map_err(|status| format!("giving it the firewall failed: {:#x}", status.0))
}

/// Gives `child` the kernel settings capability in SLOT_SETTINGS if `spec`
/// asks for admin.tune, with the read and control rights, and the copy
/// and transfer rights so that it lasts through `fork` (ADR 0476).
fn grant_settings(settings: Option<u32>, spec: &Spec, child: u32) -> Result<(), String> {
    if !spec.tune {
        return Ok(());
    }
    let settings = settings.ok_or("Spark has no kernel settings capability to give")?;
    let rights = Rights::READ.union(Rights::CONTROL).union(Rights::COPY).union(Rights::TRANSFER);
    process::share(&SYS, child, settings, SLOT_SETTINGS, rights, 0)
        .map_err(|status| format!("giving it the kernel settings failed: {:#x}", status.0))
}

/// Gives `domain` the disk weight and rates and the thread limit `spec`'s
/// file names. A call fails only for a value the file's reading checked or
/// a capability Spark made itself, so its results cannot matter.
fn limit_domain(domain: u32, spec: &Spec) {
    let rate = |limit: Option<u64>| limit.unwrap_or(NO_LIMIT);
    let rates = [spec.io_read, spec.io_write, spec.io_read_ops, spec.io_write_ops].map(rate);
    let _ = resource_domain::set_io(
        &SYS,
        domain,
        spec.io_weight,
        rates[0],
        rates[1],
        rates[2],
        rates[3],
    );
    let _ = resource_domain::set_threads(&SYS, domain, spec.threads);
}

/// Sets `child`'s open files limit, soft and hard, to what `spec`'s file
/// says before its program starts, if it says (ADR 0476).
fn limit_files(spec: &Spec, child: u32) -> Result<(), String> {
    let Some(files) = spec.open_files else { return Ok(()) };
    let files = u64::from(files);
    process::start_limit(&SYS, child, LimitKind::Files, files, files)
        .map_err(|status| format!("setting its open files limit failed: {:#x}", status.0))
}

/// A card the kernel left for a driver program: its PCI address, vendor
/// and device.
#[derive(Clone, Copy, Debug)]
struct Card {
    address: u32,
    vendor: u16,
    device: u16,
}

/// The cards the kernel left for driver programs, held or not, as the
/// root device capability `devices` lists them.
fn cards(devices: u32) -> Vec<Card> {
    let mut buffer = [0u8; device::DEVICES_MAX as usize * DeviceInfo::SIZE];
    let Ok(listed) = device::list(&SYS, devices, &mut buffer) else { return Vec::new() };
    let count = (listed.count as usize).min(device::DEVICES_MAX as usize);
    buffer
        .as_chunks::<{ DeviceInfo::SIZE }>()
        .0
        .iter()
        .take(count)
        .map(|info| {
            let u16_at = |at: usize| u16::from_le_bytes([info[at], info[at + 1]]);
            let at = DeviceInfo::OFFSET_ADDRESS;
            Card {
                address: u32::from_le_bytes([info[at], info[at + 1], info[at + 2], info[at + 3]]),
                vendor: u16_at(DeviceInfo::OFFSET_VENDOR),
                device: u16_at(DeviceInfo::OFFSET_DEVICE),
            }
        })
        .collect()
}

/// The cards in `devices` that `spec` drives, at most DEVICE_SLOTS.
fn cards_for(devices: Option<u32>, spec: &Spec) -> Vec<Card> {
    let Some(devices) = devices.filter(|_| !spec.devices.is_empty()) else { return Vec::new() };
    let mut cards = cards(devices);
    cards.retain(|card| spec.devices.iter().any(|id| id.matches(card.vendor, card.device)));
    cards.truncate(DEVICE_SLOTS as usize);
    cards
}

/// A PCI address as lspci(8) writes it.
fn pci_address(address: u32) -> String {
    format!(
        "{:04x}:{:02x}:{:02x}.{}",
        address >> 16,
        (address >> 8) & 0xff,
        (address >> 3) & 0x1f,
        address & 7
    )
}

/// Gives `child` the cards `spec` drives, in SLOT_DEVICE and the slots
/// after it, with the read and control rights. Spark keeps no copy: a
/// card is open only while its driver holds it. A card another holds is
/// skipped; the kernel waits for one whose last driver is still letting
/// go of it. Fails if `spec` names cards and none could be given.
fn grant_devices(devices: Option<u32>, spec: &Spec, child: u32) -> Result<(), String> {
    if spec.devices.is_empty() {
        return Ok(());
    }
    let devices = devices.ok_or("Spark has no device capability to give")?;
    let rights = Rights::READ.union(Rights::CONTROL);
    let mut given = 0;
    let mut refused = None;
    for card in cards_for(Some(devices), spec) {
        let slot = slots::take().ok_or("no free slot")?;
        let shared = device::open(&SYS, devices, card.address, slot)
            .and_then(|()| process::share(&SYS, child, slot, SLOT_DEVICE + given, rights, 0));
        let _ = capability::delete(&SYS, slot);
        slots::give_back(slot);
        match shared {
            Ok(()) => given += 1,
            Err(status) => refused = Some((card.address, status.0)),
        }
    }
    match (given, refused) {
        (0, Some((address, status))) => Err(format!(
            "it was given none of the cards it drives: opening {} failed: {status:#x}",
            pci_address(address)
        )),
        (0, None) => Err("the kernel left it none of the cards it drives".into()),
        _ => Ok(()),
    }
}

/// Where a service is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    /// Not running, and not to be started: turned off, stopped by a
    /// request, ended for good, refused, or never started.
    Stopped,
    /// To start once the services it names are ready or off (ADR 0470).
    Pending,
    /// Running as this process, and not yet ready.
    Starting(u64),
    /// Running as this process, and ready.
    Running(u64),
    /// Ended with 0, as a setup step that is ready once it has ended.
    Done,
    /// To start at this time.
    Waiting(u64),
    /// Its process ended with this status, and Spark stopped its domain,
    /// where processes it left still run; once they have gone, it is
    /// started again or not, as the status says. Spark looks again at the
    /// time given.
    Draining(u32, u64),
    /// Given up on after `give_up_after` failed starts in a row.
    Failed,
    /// Not running: an on-connection service, whose sockets Spark holds
    /// and, from the time given, polls; a connection starts it (ADR 0477).
    Listening(u64),
}

impl State {
    /// Its process, while it is starting or running.
    fn pid(self) -> Option<u64> {
        match self {
            State::Starting(pid) | State::Running(pid) => Some(pid),
            _ => None,
        }
    }
}

/// A service and its process.
struct Service {
    spec: Spec,
    state: State,
    /// Its domain's capability, once it is made; it stays for restarts.
    domain: Option<u32>,
    /// The read end of its output pipe.
    output: Option<File>,
    line: Vec<u8>,
    started_ns: u64,
    /// Quick ends in a row, which the backoff doubles with.
    quick: u32,
    /// Failed starts in a row, which `give_up_after` counts: a start
    /// that could not be made, an end before it was ready, a start that
    /// was not ready in time, a failing end soon after it was, and a
    /// restart for failed health checks.
    failures: u32,
    /// Its failed starts since boot, counted as `failures` is but never
    /// set back: a trial undoes a change once one more has failed.
    failed_starts: u64,
    restarts: u32,
    last: Option<u32>,
    /// Stopped by a request: not started again until asked.
    held: bool,
    /// Asked to restart: started again whatever its file says.
    restarting: bool,
    /// Asked to stop or restart: when Spark stops its domain if it has
    /// not ended.
    stop_by: Option<u64>,
    /// Spark stopped its domain, so what is left in it is ending already.
    killed: bool,
    /// How many times the out-of-memory policy had stopped its domain.
    oom_stops: u32,
    /// How many charges its domain's memory limit had refused, as of its
    /// last end.
    refused: u64,
    /// Whether its last end was a stop at its memory limit: a charge was
    /// refused, and the page that needed it ended the process.
    at_limit: bool,
    /// The text of its file that `spec` was read from, kept as the last
    /// good copy.
    good: services::Texts,
    /// What was wrong with its file when last read, said once.
    complaint: Option<String>,
    /// Why it could not start, when it last could not, for the request
    /// that asked.
    why: Option<String>,
    /// The service `why` names, when it names one.
    cause: Option<String>,
    /// Whether `why` says why it is off as it is meant to be, which is
    /// no failure: see [`Service::leave_off`].
    off_as_meant: bool,
    /// How many times Spark has tried to start it, which a watch counts
    /// from.
    attempts: u32,
    /// The last line its output wrote, from its last start on.
    last_line: Option<String>,
    /// The last request that started, stopped or restarted it.
    last_request: Option<Request>,
    /// When it came to its state.
    since_ns: u64,
    /// What made it start: "boot", "a request", or the service whose
    /// file pulled it in.
    wanted_by: Option<String>,
    /// While it is pending: the services it waits for.
    waiting_for: Vec<String>,
    /// While it is starting: when its start has failed if it is not ready.
    ready_by: u64,
    /// While it starts with `ready.when = "socket"`: what was at the
    /// socket's path as it started, which does not count (its number and
    /// change time).
    socket_before: Option<(u64, u64)>,
    /// When it was ready, since it last started.
    ready_ns: Option<u64>,
    /// Restarted for failed health checks: its end is a failed start.
    unhealthy: bool,
    /// Its health checks.
    checks: health::Checks,
    /// Its stop rank: shutdown stops the highest first, a service before
    /// what it depends on.
    rank: u32,
    /// Why it never starts, when it is one of services that wait for each
    /// other.
    circle: Option<String>,
    /// When Spark last had it read its certificates again, or it started,
    /// and whether a change since waits for RELOAD_SPACING to pass (ADR
    /// 0512).
    reloaded_ns: u64,
    reload_due: bool,
    /// The sockets Spark holds for it (ADR 0477), once made.
    sockets: Option<sockets::Held>,
    /// Asked to stop with `--keep-sockets`: its sockets stay open once it
    /// has ended.
    keep_sockets: bool,
}

impl Service {
    /// A stopped service that runs `spec`, read from `good`.
    fn new(spec: Spec, good: services::Texts) -> Service {
        let rank = spec.order;
        Service {
            spec,
            state: State::Stopped,
            domain: None,
            output: None,
            line: Vec::new(),
            started_ns: 0,
            quick: 0,
            failures: 0,
            failed_starts: 0,
            restarts: 0,
            last: None,
            held: false,
            restarting: false,
            stop_by: None,
            killed: false,
            oom_stops: 0,
            refused: 0,
            at_limit: false,
            good,
            complaint: None,
            why: None,
            cause: None,
            off_as_meant: false,
            attempts: 0,
            last_line: None,
            last_request: None,
            since_ns: now(),
            wanted_by: None,
            waiting_for: Vec::new(),
            ready_by: 0,
            socket_before: None,
            ready_ns: None,
            unhealthy: false,
            checks: health::Checks::default(),
            rank,
            circle: None,
            reloaded_ns: 0,
            reload_due: false,
            sockets: None,
            keep_sockets: false,
        }
    }

    /// Asks process `pid`, its own, to stop with its file's stop signal;
    /// `step_stops` stops its domain once its stop timeout has passed, if
    /// it has not ended. A stop asked for already keeps its time.
    fn ask_to_stop(&mut self, pid: u64) {
        self.stop_by.get_or_insert(now() + self.spec.stop_timeout);
        let signal = u32::from(self.spec.stop_signal.number());
        let _ = process::kill(&SYS, SLOT_PROCESS, pid as i64, signal);
    }

    /// Whether it waits for a connection once it is not running: it starts
    /// on-connection, is enabled, and Spark holds its sockets.
    fn waits_for_connections(&self) -> bool {
        self.spec.on_connection && self.spec.enabled && self.sockets.is_some()
    }

    /// Reads what its domain's memory limit and the out-of-memory policy
    /// did since its process last ended, as that process ended with
    /// `status`, and keeps whether this end was at its memory limit.
    /// Returns what the end says of it, as a clause after the ending.
    fn limit_end(&mut self, status: u32) -> &'static str {
        let info = self.domain.and_then(|d| resource_domain::info(&SYS, d).ok());
        let (stops, refused) =
            info.map_or((self.oom_stops, self.refused), |info| (info.oom_stops, info.refused));
        let killed = status & 0x7f == signal::SIGKILL;
        let by_policy = killed && stops > self.oom_stops;
        // A page the limit refused ends the process with SIGKILL, as a
        // stop that was asked for does, which is no stop at the limit.
        self.at_limit = killed
            && !by_policy
            && refused > self.refused
            && self.stop_by.is_none()
            && !self.killed;
        (self.oom_stops, self.refused) = (stops, refused);
        if by_policy {
            " (stopped by the out-of-memory policy)"
        } else if self.at_limit {
            " (stopped at its memory limit)"
        } else {
            ""
        }
    }

    /// Puts it in `state`, as of now.
    fn set(&mut self, state: State) {
        self.state = state;
        self.since_ns = now();
    }

    /// Says why it is not running as asked, or that nothing keeps it
    /// from it, with `cause` the service `why` names, if it names one.
    fn set_why(&mut self, why: Option<String>, cause: Option<&str>) {
        self.why = why;
        self.cause = cause.map(String::from);
        self.off_as_meant = false;
    }

    /// Notes that it is off as it is meant to be, for `why`: the kernel
    /// left it none of the cards it drives, or Rescue or essentials leave
    /// it off until it is started. `service list` does not count it as
    /// not running as asked, nor sysupdate's health check as failing.
    fn leave_off(&mut self, why: String) {
        self.set_why(Some(why), None);
        self.off_as_meant = true;
    }

    /// When Spark must look at it again, given that it is `time`: its
    /// restart, the end of its stop's wait, its next readiness look, or
    /// its next health check.
    fn next_look(&self, time: u64) -> Option<u64> {
        let state = match self.state {
            State::Waiting(at) | State::Draining(_, at) => Some(at),
            State::Starting(_) if self.stop_by.is_none() => match self.spec.ready.when {
                ReadyWhen::Listening(_) | ReadyWhen::Socket(_) => {
                    Some(self.ready_by.min(time + starting::READY_POLL))
                }
                _ => Some(self.ready_by),
            },
            State::Starting(_) | State::Running(_) => self.stop_by,
            // From then on its sockets are polled.
            State::Listening(at) => (at > time).then_some(at),
            State::Stopped | State::Pending | State::Done | State::Failed => None,
        };
        let check = match self.state {
            State::Starting(_) | State::Running(_) if self.stop_by.is_none() => {
                self.checks.next_look()
            }
            _ => None,
        };
        let reload = match self.state {
            State::Running(_) if self.reload_due => Some(self.reloaded_ns + certd::RELOAD_SPACING),
            _ => None,
        };
        state.into_iter().chain(check).chain(reload).min()
    }
}

/// The program Spark was asked to run, on a pseudo-terminal, until it
/// ends.
pub struct Program {
    pub path: String,
    pub number: u64,
    pub master: File,
    pub line: Vec<u8>,
    /// Every descriptor of its terminal has closed: the master is not
    /// polled any more, which would say so again at once, for good.
    pub hung_up: bool,
}

/// What shutting down ends with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum End {
    PowerOff,
    Restart,
    Halt,
}

impl End {
    fn words(self) -> &'static str {
        match self {
            End::PowerOff => "power off",
            End::Restart => "restart",
            End::Halt => "halt",
        }
    }
}

/// A shutdown asked for.
#[derive(Clone, Copy, Debug)]
enum Shutdown {
    /// To start at this time.
    Pending { end: End, at: u64 },
    /// Stopping the services of stop rank `rank` and above, each asked to
    /// stop and its domain stopped once its own timeout has passed;
    /// `deadline` is when those still not ended are as good as gone.
    Stopping { end: End, rank: u32, deadline: u64 },
}

/// A connection to the control socket, what it has sent so far, and
/// when it is closed if its request is not whole by then, or its answer
/// not read.
struct Connection {
    file: File,
    /// The process that connected and the user it acts for, as the
    /// kernel says (0 if it cannot).
    peer: u64,
    user: u32,
    request: Vec<u8>,
    until: u64,
    /// Whether its request was whole and is answered.
    answered: bool,
    /// What is to be written to it, and how much of that is: a long
    /// answer, as `show`'s, may not fit the socket at once, and a watch
    /// adds lines as Spark says them.
    out: Vec<u8>,
    written: usize,
    /// While its request waits for the service to settle.
    watch: Option<Watch>,
    /// Whether it came to certd's socket, which answers certd alone, and
    /// one request.
    certd: bool,
}

/// The services' backup hooks a `hooks` request runs (ADR 0463), one
/// after another, answered once the last ends.
struct Hooks {
    /// The connection that asked, which gets the answer.
    connection: File,
    /// `before` or `after`.
    phase: &'static str,
    /// The services whose hooks are still to run, by index, in order.
    queue: Vec<usize>,
    running: Option<HookRun>,
    /// A line for each hook run: `ok: NAME` or `failed: NAME: why`.
    answer: String,
}

/// A hook's process.
struct HookRun {
    index: usize,
    pid: u64,
    output: Option<File>,
    line: Vec<u8>,
    /// When it is stopped if it has not ended.
    until: u64,
    stopped: bool,
}

/// Spark's supervision.
pub struct Supervisor {
    root: Dir,
    /// The `services` domain, the power capability, the network
    /// capability, the platform capability, the names capability and
    /// the clock capability, from the kernel.
    services_domain: Option<u32>,
    power: Option<u32>,
    network: Option<u32>,
    platform: Option<u32>,
    names: Option<u32>,
    clock: Option<u32>,
    /// The root device capability (ADR 0198), from the kernel.
    devices: Option<u32>,
    /// The firewall capability (ADR 0334), from the kernel.
    firewall: Option<u32>,
    /// The kernel settings capability (ADR 0476), from the kernel.
    settings: Option<u32>,
    /// The notification the kernel signals when a child changes.
    children: Option<u32>,
    /// The notification the kernel signals when the power button is
    /// pressed, if it can hear it.
    button: Option<u32>,
    /// The notification the kernel signals when the host of a virtual
    /// machine asks it to restart.
    restart: Option<u32>,
    listener: Option<File>,
    /// certd's socket (ADR 0512), whose one connection at a time is in
    /// `connections` too, beside the control socket's.
    certd_listener: Option<File>,
    connections: Vec<Connection>,
    services: Vec<Service>,
    /// What Spark said about the services in this turn of its loop, for
    /// the watches.
    told: Vec<Told>,
    /// The directory the service files are in, beneath the root.
    dir: String,
    program: Option<Program>,
    /// The backup hooks running, if a request asked for them.
    hooks: Option<Hooks>,
    shutdown: Option<Shutdown>,
    /// The watchdog's timeout to ask for, 0 for none, and whether to stop
    /// responding once it runs: at the first start (`--test-hang`), or at
    /// every start (the kernel's `test-hang`).
    watchdog_seconds: u32,
    hang: bool,
    hang_always: bool,
    /// Whether this is Rescue (ADR 0460), which starts only
    /// [`RESCUE_SERVICES`].
    rescue: bool,
    /// Whether only the system's own services start, which the start
    /// menu's Troubleshooting asks for with the kernel's `essentials`
    /// (ADR 0501): none an app or an administrator added.
    essentials: bool,
    /// The try of changes bisect this start is, and the services its list
    /// leaves out (ADR 0475).
    bisect_try: Option<(u8, Vec<String>)>,
    /// While the watchdog runs: how often Spark pets it, and when next.
    pets: Option<(u64, u64)>,
    /// The settings being tried, each undone unless it is kept in time
    /// (ADR 0472).
    trials: Vec<trial::Trial>,
    /// The trials undone since boot, the newest last, for `show`.
    undone: Vec<spark_api::show::Undone>,
}

impl Supervisor {
    /// Spark's supervision of the tree under `root`, with the capabilities
    /// the kernel gave it from SLOT_FIRST_EXTRA on.
    pub fn new(root: Dir) -> Supervisor {
        let holds = |slot: u32, kind: ObjectKind| {
            capability::info(&SYS, slot).is_ok_and(|info| info.kind == kind)
        };
        let domain = SLOT_FIRST_EXTRA + 1;
        let power = SLOT_FIRST_EXTRA + 2;
        let network = SLOT_FIRST_EXTRA + 3;
        let platform = SLOT_FIRST_EXTRA + 4;
        let names = SLOT_FIRST_EXTRA + 5;
        let clock = SLOT_FIRST_EXTRA + 6;
        let devices = SLOT_FIRST_EXTRA + 7;
        let firewall = SLOT_FIRST_EXTRA + 8;
        let settings = SLOT_FIRST_EXTRA + 9;
        let children = slots::take().filter(|&slot| {
            process::create(&SYS, SLOT_PROCESS, ObjectKind::Notification, 0, slot).is_ok()
                && process::notify_children(&SYS, SLOT_PROCESS, slot).is_ok()
        });
        if children.is_none() {
            println!("spark: cannot hear of children ending; services are not watched");
        }
        let power = holds(power, ObjectKind::Power).then_some(power);
        // Not supported where the kernel cannot hear the button: the
        // firmware keeps it.
        let button = power.and_then(|power| {
            slots::take().filter(|&slot| {
                process::create(&SYS, SLOT_PROCESS, ObjectKind::Notification, 0, slot).is_ok()
                    && power::notify_button(&SYS, power, slot).is_ok()
            })
        });
        let restart = power.and_then(|power| {
            slots::take().filter(|&slot| {
                process::create(&SYS, SLOT_PROCESS, ObjectKind::Notification, 0, slot).is_ok()
                    && power::notify_restart(&SYS, power, slot).is_ok()
            })
        });
        Supervisor {
            root,
            services_domain: holds(domain, ObjectKind::ResourceDomain).then_some(domain),
            power,
            network: holds(network, ObjectKind::Network).then_some(network),
            platform: holds(platform, ObjectKind::Platform).then_some(platform),
            names: holds(names, ObjectKind::Names).then_some(names),
            clock: holds(clock, ObjectKind::Clock).then_some(clock),
            devices: holds(devices, ObjectKind::Device).then_some(devices),
            firewall: holds(firewall, ObjectKind::Firewall).then_some(firewall),
            settings: holds(settings, ObjectKind::KernelSettings).then_some(settings),
            children,
            button,
            restart,
            listener: None,
            certd_listener: None,
            connections: Vec::new(),
            services: Vec::new(),
            told: Vec::new(),
            dir: String::from(SERVICES_DIR),
            program: None,
            hooks: None,
            shutdown: None,
            watchdog_seconds: WATCHDOG_SECONDS,
            hang: false,
            hang_always: false,
            rescue: false,
            essentials: false,
            bisect_try: None,
            pets: None,
            trials: Vec::new(),
            undone: Vec::new(),
        }
    }

    /// Makes this Rescue (ADR 0460): of the enabled services, only
    /// [`RESCUE_SERVICES`] start, so that nothing reaches the network or
    /// the machine's disks until the owner asks.
    pub fn set_rescue(&mut self) {
        self.rescue = true;
    }

    /// Has only the system's own services start (ADR 0501): those with a
    /// file among the system's defaults, as they are set.
    pub fn set_essentials(&mut self) {
        self.essentials = true;
    }

    /// Makes this start try `number` of changes bisect (ADR 0475): the
    /// services its list names stay off until they are started. A try
    /// with no list, or one that does not read, leaves none out.
    pub fn set_bisect_try(&mut self, number: u8) {
        let list = self.root.read_file(&format!("{BISECT}/{}", bisect::file(number)));
        let list = list.ok().and_then(|bytes| String::from_utf8(bytes).ok());
        self.bisect_try = Some((number, list.map_or_else(Vec::new, |list| bisect::names(&list))));
    }

    /// The try of changes bisect that leaves service `name` out, if one
    /// does.
    pub(super) fn left_out(&self, name: &str) -> Option<u8> {
        let (number, names) = self.bisect_try.as_ref()?;
        names.iter().any(|left| left == name).then_some(*number)
    }

    /// Whether `name` is one of the system's own services, with a file
    /// among the defaults the system brings.
    fn own(&self, name: &str) -> bool {
        self.root.read_file(&format!("{ESSENTIALS}/{name}.toml")).is_ok()
    }

    /// Sets the watchdog's timeout, 0 to leave it off, and whether Spark
    /// stops responding once it runs: at the first start, `hang`, or at
    /// every start, `always`.
    pub fn set_watchdog(&mut self, seconds: u32, hang: bool, always: bool) {
        self.watchdog_seconds = seconds;
        self.hang = hang;
        self.hang_always = always;
    }

    /// Starts the watchdog, if the kernel drives one. With `--test-hang`,
    /// Spark then stops responding, unless the watchdog restarted the
    /// machine at the end of the last boot; with the kernel's `test-hang`,
    /// whether it did or not.
    fn start_watchdog(&mut self) {
        let Some(power) = self.power else { return };
        if self.watchdog_seconds == 0 {
            return;
        }
        // Not supported without a watchdog: the kernel logs that it has
        // none.
        let Ok(started) = power::start_watchdog(&SYS, power, self.watchdog_seconds) else {
            return;
        };
        if started.restarted {
            println!("spark: the watchdog restarted the machine: the last boot stopped responding");
        }
        let every = u64::from(started.seconds) * SECOND / 3;
        self.pets = Some((every, now() + every));
        if self.hang_always {
            println!("spark: not responding, as the kernel's test-hang asks, at every start");
            loop {
                core::hint::spin_loop();
            }
        }
        if self.hang && !started.restarted {
            println!("spark: not responding, as --test-hang asks, until the watchdog restarts");
            loop {
                core::hint::spin_loop();
            }
        }
    }

    /// Pets the watchdog if it is time to.
    fn pet_watchdog(&mut self, time: u64) {
        let (Some(power), Some((every, next))) = (self.power, self.pets) else { return };
        if time >= next {
            let _ = power::pet_watchdog(&SYS, power);
            self.pets = Some((every, time + every));
        }
    }

    /// Opens the control socket, and certd's. The control socket's folder
    /// may be there already: the record of a root on a disk is written
    /// beside it first (ADR 0473).
    pub fn listen(&mut self) {
        if let Err(err) = services::make_dirs(&self.root, CONTROL_DIR) {
            println!("spark: cannot make {err}");
            return;
        }
        self.listener = self.listen_at(CONTROL, CONNECTIONS_MAX);
        if let Err(err) = self.root.make_dir(certd::SOCKET_DIR) {
            println!("spark: cannot make /{}: {err}", certd::SOCKET_DIR);
            return;
        }
        self.certd_listener = self.listen_at(certd::SOCKET, 1);
    }

    /// A socket listening at `path` beneath the root, with room for
    /// `backlog` connections waiting to be taken.
    fn listen_at(&self, path: &str, backlog: usize) -> Option<File> {
        let slot = slots::take()?;
        let flags = PipeFlags::NON_BLOCKING.union(PipeFlags::CLOSE_ON_EXEC);
        let made =
            process::socket(&SYS, SLOT_PROCESS, slot, flags).map_err(|s| s.0).and_then(|()| {
                let name = alloc::format!("/{path}");
                libradix::abi::directory::bind_socket(
                    &SYS,
                    self.root.slot(),
                    path.as_bytes(),
                    slot,
                    name.as_bytes(),
                )
                .map_err(|s| s.0)?;
                file_abi::listen(&SYS, slot, backlog as u32).map_err(|s| s.0)
            });
        match made {
            Ok(()) => match File::own(slot) {
                Ok(listener) => {
                    println!("spark: listening at /{path}");
                    Some(listener)
                }
                Err(err) => {
                    println!("spark: cannot listen at /{path}: {err}");
                    None
                }
            },
            Err(status) => {
                let _ = capability::delete(&SYS, slot);
                slots::give_back(slot);
                println!("spark: cannot listen at /{path}: {status:#x}");
                None
            }
        }
    }

    /// Reads the service files in `dir` and starts the enabled ones, each
    /// once what it names is ready (ADR 0470), lowest order first. An old
    /// file is converted first; a file that does not read runs its last
    /// good copy, if Spark kept one.
    pub fn start_services(&mut self, dir: &str) {
        self.dir = String::from(dir);
        // A trial the last start left unkept is undone before any file is
        // read; Rescue changes nothing, and the next start undoes it.
        if !self.rescue {
            for id in trial::undo_left(&self.root) {
                self.note_undone(id, &spark_api::trial::Why::Restarted);
            }
        }
        services::convert_old(&self.root, dir);
        let entries = match self.root.open_dir(dir, Rights::READ).and_then(|d| d.entries()) {
            Ok(entries) => entries,
            Err(err) => return println!("spark: cannot read /{dir}: {err}"),
        };
        let mut loaded = Vec::new();
        for entry in entries {
            let Some(name) = entry.name.strip_suffix(".toml") else { continue };
            if entry.kind != FileKind::File {
                continue;
            }
            match self.load(name) {
                Ok(service) => loaded.push(service),
                Err(err) => println!("spark: {err}; it is not started"),
            }
        }
        loaded.sort_by(|a, b| (a.spec.order, &a.spec.name).cmp(&(b.spec.order, &b.spec.name)));
        let enabled = loaded.iter().filter(|s| s.spec.enabled).count();
        println!("spark: {} services in /{dir}, {enabled} enabled", loaded.len());
        if self.rescue {
            println!(
                "spark: Rescue starts only the consoles, the log, the firewall and the reader; \
                 networking stays off until it is turned on"
            );
        }
        if self.essentials {
            println!(
                "spark: essentials: only the system's own services start; those an app or an \
                 administrator added stay off until they are started"
            );
        }
        if let Some((number, names)) = &self.bisect_try {
            let left = if names.is_empty() { String::from("nothing") } else { names.join(", ") };
            println!("spark: try {number} of changes bisect: leaving out {left}");
        }
        self.services.extend(loaded);
        self.settle_graph();
        let mut booting = Vec::new();
        for index in 0..self.services.len() {
            let spec = &self.services[index].spec;
            if !spec.enabled {
                continue;
            }
            match self.left_off(&spec.name) {
                Some(why) => self.services[index].leave_off(why),
                None if spec.timer => self.services[index]
                    .leave_off(String::from("timerd starts it at the times its file names")),
                None => booting.push(index),
            }
        }
        // Every socket before any service starts, so that services that
        // connect to each other start in any order (ADR 0477). One that
        // cannot be made fails that service's start, which says why.
        for &index in &booting {
            if !self.services[index].spec.sockets.is_empty()
                && let Err(why) = self.hold_sockets(index)
            {
                let name = &self.services[index].spec.name;
                println!("spark: {name}: {why}");
            }
        }
        for index in booting {
            if self.services[index].spec.on_connection {
                self.wait_for_connections(index);
            } else {
                self.want(index, Wanted::Boot);
            }
        }
        self.step_pending();
    }

    /// Makes service `index`'s sockets if Spark holds none for it, or
    /// makes them again if its file, read again since, names others.
    fn hold_sockets(&mut self, index: usize) -> Result<(), String> {
        let service = &mut self.services[index];
        let current = service.sockets.as_ref().map(|held| held.made_from(&service.spec.sockets));
        match current {
            Some(true) => return Ok(()),
            Some(false) => {
                if let Some(held) = service.sockets.take() {
                    held.close(&self.root);
                }
            }
            None => {}
        }
        if service.spec.sockets.is_empty() {
            return Ok(());
        }
        let owner = match &service.spec.account {
            Some(name) => Some(account_numbers(&self.root, name)?),
            None => None,
        };
        service.sockets = Some(sockets::make(&self.root, self.network, &service.spec, owner)?);
        Ok(())
    }

    /// Closes service `index`'s sockets, if Spark holds any for it.
    fn close_sockets(&mut self, index: usize) {
        if let Some(held) = self.services[index].sockets.take() {
            held.close(&self.root);
        }
    }

    /// Has on-connection service `index` wait for a connection at its
    /// sockets, made first if they are not; if they cannot be, it is a
    /// start that could not be made, tried again as its file says.
    fn wait_for_connections(&mut self, index: usize) {
        if let Err(why) = self.hold_sockets(index) {
            let name = &self.services[index].spec.name;
            say(&mut self.told, index, format!("could not listen for {name}: {why}"));
            self.services[index].set_why(Some(why), None);
            return self.schedule(index, None);
        }
        let service = &mut self.services[index];
        service.set_why(None, None);
        service.set(State::Listening(now()));
    }

    /// Service `name` as its file in the services directory says, or as
    /// the last good copy of it says if the file does not read; what is
    /// wrong with the file if neither does.
    fn load(&self, name: &str) -> Result<Service, String> {
        match services::read(&self.root, &self.dir, name) {
            Ok((spec, text)) => {
                if services::good(&self.root, name).is_none_or(|(_, good)| good != text) {
                    services::keep_good(&self.root, name, &text);
                }
                Ok(Service::new(spec, text))
            }
            Err(err) => {
                let (spec, good) = services::good(&self.root, name).ok_or_else(|| err.clone())?;
                println!("spark: {err}; it runs its last good copy");
                let mut service = Service::new(spec, good);
                service.complaint = Some(err);
                Ok(service)
            }
        }
    }

    /// Adds service `name` from its file in the services directory, which
    /// Spark did not read at boot; returns its index, or why not.
    fn add(&mut self, name: &str) -> Result<usize, String> {
        let unknown = || format!("no service is called {name}");
        services::check_name(name).map_err(|_| unknown())?;
        services::convert_old(&self.root, &self.dir);
        // Only a file that is there: a removed one's last good copy is not
        // a service.
        self.root.stat(&format!("{}/{name}.toml", self.dir)).map_err(|_| unknown())?;
        let service = self.load(name).inspect_err(|err| {
            println!("spark: {err}; it is not started");
        })?;
        self.services.push(service);
        Ok(self.services.len() - 1)
    }

    /// Reads service `index`'s file again, as it is about to start: a file
    /// that reads is its new last good copy; one that does not leaves it
    /// as it was, and Spark says what is wrong with it once. New links or
    /// a new order settle the graph again.
    fn reread(&mut self, index: usize) {
        let name = self.services[index].spec.name.clone();
        match services::read(&self.root, &self.dir, &name) {
            Ok((spec, text)) => {
                let service = &mut self.services[index];
                if text != service.good {
                    services::keep_good(&self.root, &name, &text);
                    service.good = text;
                }
                let moved =
                    spec.links() != service.spec.links() || spec.order != service.spec.order;
                service.spec = spec;
                service.complaint = None;
                if moved {
                    self.settle_graph();
                }
            }
            Err(err) => {
                let service = &mut self.services[index];
                if service.complaint.as_ref() != Some(&err) {
                    say(&mut self.told, index, format!("{err}; it runs its last good copy"));
                    service.complaint = Some(err);
                }
            }
        }
    }

    /// Hands over the program Spark was asked to run, to watch until it
    /// ends.
    pub fn watch_program(&mut self, program: Program) {
        non_blocking(program.master.slot());
        self.program = Some(program);
    }

    /// Says so when there is nothing to supervise but the control socket:
    /// no services, and no program.
    pub fn note_idle(&self) {
        if self.services.is_empty() && self.program.is_none() {
            println!("spark: nothing more to do; waiting");
        }
    }

    /// Starts service `index`, whose links let it: it is running, or
    /// starting until it is ready as its `ready.when` says.
    fn start(&mut self, index: usize) {
        self.services[index].attempts += 1;
        self.services[index].last_line = None;
        let spec = &self.services[index].spec;
        if !spec.devices.is_empty() && cards_for(self.devices, spec).is_empty() {
            // Not an end: there is nothing for it to drive, as on a
            // machine where the kernel drives its cards itself.
            let why = String::from("the kernel left it none of the cards it drives");
            say(&mut self.told, index, format!("not starting {}: {why}", spec.name));
            let service = &mut self.services[index];
            service.leave_off(why);
            service.set(State::Stopped);
            self.stop_dependents(index);
            return;
        }
        // What a run before left at its socket's path does not count.
        let socket_before = match &spec.ready.when {
            ReadyWhen::Socket(path) => self.socket_there(path),
            _ => None,
        };
        let started = self.spawn(index);
        let time = now();
        let service = &mut self.services[index];
        let (number, output) = match started {
            Ok(started) => started,
            Err(what) => {
                say(
                    &mut self.told,
                    index,
                    format!("could not start {}: {what}", service.spec.name),
                );
                service.set_why(Some(what), None);
                // As if it had ended at once.
                return self.schedule(index, None);
            }
        };
        let line =
            format!("started {} ({}) as process {number}", service.spec.name, service.spec.program);
        say(&mut self.told, index, line);
        service.output = Some(output);
        service.started_ns = time;
        // It reads the certificates it starts with; a change comes at
        // RELOAD_SPACING after its start at the soonest.
        service.reloaded_ns = time;
        service.reload_due = false;
        service.set_why(None, None);
        service.unhealthy = false;
        service.checks = health::Checks::default();
        service.socket_before = socket_before;
        let ready = &service.spec.ready;
        if ready.when == ReadyWhen::Started {
            service.ready_ns = Some(time);
            if let Some(health) = &service.spec.health {
                service.checks.next = time + u64::from(health.every) * SECOND;
            }
            service.set(State::Running(number));
        } else {
            service.ready_ns = None;
            service.ready_by = time + u64::from(ready.timeout) * SECOND;
            if ready.when == ReadyWhen::Healthy {
                service.checks.next = time;
            }
            service.set(State::Starting(number));
        }
    }

    /// Copies service `index`'s certificates anew, makes its domain if it
    /// has none, and its process in it, running its program. Returns the
    /// process's number and the read end of its output.
    fn spawn(&mut self, index: usize) -> Result<(u64, File), String> {
        self.hold_sockets(index)?;
        let spec = &self.services[index].spec;
        // Its certificates are copied anew as it starts (ADR 0512), not
        // for the processes of its health checks and hooks, which read the
        // copies it has.
        let account = match &spec.account {
            Some(name) if !spec.certificates.is_empty() => {
                Some(account(&self.root, self.names, name)?)
            }
            _ => None,
        };
        certificates::copy_all(&self.root, spec, account.as_ref())?;
        let (program, arguments) = (spec.program.clone(), spec.arguments.clone());
        self.spawn_as(index, &program, &arguments, true)
    }

    /// Makes a process of service `index`'s, in its domain, with its
    /// account and view, running `program` with `arguments`, and with
    /// the sockets Spark holds for it if `with_sockets`: the service
    /// itself, not a hook. Returns the process's number and the read end
    /// of its output.
    fn spawn_as(
        &mut self,
        index: usize,
        program_path: &str,
        arguments: &[String],
        with_sockets: bool,
    ) -> Result<(u64, File), String> {
        let root = self.root.slot();
        let services_domain = self.services_domain;
        let service = &mut self.services[index];
        let spec = &service.spec;
        let names = match (spec.names, self.names) {
            (false, _) => None,
            (true, Some(slot)) => Some(slot),
            (true, None) => return Err("the kernel gave Spark no names capability".into()),
        };
        if service.domain.is_none()
            && let Some(parent) = services_domain
        {
            let slot = slots::take().ok_or("no free slot")?;
            let limit = spec.memory_pages.unwrap_or(NO_LIMIT);
            let made = resource_domain::create(
                &SYS,
                parent,
                spec.name.as_bytes(),
                spec.weight,
                limit,
                spec.oom,
                slot,
            );
            match made {
                Ok(()) => {
                    service.domain = Some(slot);
                    limit_domain(slot, spec);
                }
                Err(status) => {
                    slots::give_back(slot);
                    return Err(format!("cannot make its domain: {:#x}", status.0));
                }
            }
        } else if let Some(domain) = service.domain {
            // Its file may have changed them since; its out-of-memory
            // class is its domain's for good.
            let _ = resource_domain::set_weight(&SYS, domain, spec.weight);
            let _ = resource_domain::set_limit(&SYS, domain, spec.memory_pages.unwrap_or(NO_LIMIT));
            limit_domain(domain, spec);
        }
        // Its account, if not root's (ADR 0385): Spark's root acting for
        // it, through which its program and its view are opened.
        let account = match &spec.account {
            Some(name) => {
                let groups = group_numbers(&self.root, &spec.name, &spec.groups);
                Some(account(&self.root, self.names, name, groups)?)
            }
            None => None,
        };
        let program = account
            .as_ref()
            .map_or(&self.root, |account| &account.root)
            .open(program_path.trim_start_matches('/'), OpenFlags::NONE, Rights::READ)
            .map_err(|err| format!("{program_path}: {err}"))?;
        // Its root: a view of what its file declares (ADR 0217).
        let view = views::build(&self.root, account.as_ref(), spec, service.domain)?;
        let root = match &view.root {
            views::Root::View(dir) => dir.slot(),
            views::Root::Spark => root,
        };
        let null = self
            .root
            .open("devices/null", OpenFlags::NONE, Rights::READ.union(Rights::WRITE))
            .map_err(|err| format!("/devices/null: {err}"))?;
        let (read, write) = (slots::take().ok_or("no free slot")?, slots::take());
        let Some(write) = write else {
            slots::give_back(read);
            return Err("no free slot".into());
        };
        if let Err(status) = process::pipe(&SYS, SLOT_PROCESS, read, write, PipeFlags::NONE) {
            slots::give_back(read);
            slots::give_back(write);
            return Err(format!("cannot make a pipe: {:#x}", status.0));
        }
        let finish = |slot: u32| {
            let _ = capability::delete(&SYS, slot);
            slots::give_back(slot);
        };
        let output = match File::own(read) {
            Ok(output) => output,
            Err(err) => {
                finish(write);
                return Err(format!("{err}"));
            }
        };
        let Some(child) = slots::take() else {
            finish(write);
            return Err("no free slot".into());
        };
        let made = match service.domain {
            Some(domain) => resource_domain::create_process(&SYS, domain, POSIX_SLOTS, child),
            None => process::create(&SYS, SLOT_PROCESS, ObjectKind::Process, POSIX_SLOTS, child),
        };
        if let Err(status) = made {
            finish(write);
            slots::give_back(child);
            return Err(format!("cannot make its process: {:#x}", status.0));
        }
        let number = process::info(&SYS, child).map_or(0, |info| info.number);
        // The root, the random source and the log as Spark has them, with
        // the copy right that `fork` needs; /devices/null as its input and the
        // pipe as its output and errors.
        let shares = [
            (root, SLOT_ROOT),
            (SLOT_RANDOM, SLOT_RANDOM),
            (SLOT_LOG, SLOT_LOG),
            (null.slot(), SLOT_FD_BASE),
            (write, SLOT_FD_BASE + 1),
            (write, SLOT_FD_BASE + 2),
        ];
        // Its sockets, as descriptors from 3 (sd_listen_fds(3)), then the
        // places it mounts beneath, then the names capability if its file
        // asks for it.
        let held = service.sockets.as_ref().filter(|_| with_sockets);
        let listen: Vec<u32> = held.map_or_else(Vec::new, |held| held.slots().collect());
        let sockets = listen.iter().zip(3..).map(|(&slot, fd)| (slot, SLOT_FD_BASE + fd));
        let first_mount = 3 + listen.len() as u32;
        let mounts =
            view.mounts.iter().zip(first_mount..).map(|(dir, fd)| (dir.slot(), SLOT_FD_BASE + fd));
        let names_fd = first_mount + view.mounts.len() as u32;
        let names = names.map(|slot| (slot, SLOT_FD_BASE + names_fd));
        let args: Vec<&str> = arguments.iter().map(String::as_str).collect();
        let mut environment = spec.environment.clone();
        if let Some(held) = held {
            environment.push(format!("LISTEN_FDS={}", held.len()));
            environment.push(format!("LISTEN_PID={number}"));
            environment.push(format!("LISTEN_FDNAMES={}", held.names()));
        }
        if names.is_some() {
            environment.push(format!("NAMES_DESCRIPTOR={names_fd}"));
        }
        if !spec.secrets.is_empty() {
            environment
                .push(format!("SECRETS_DIRECTORY={}", manifest::service::secrets_dir(&spec.name)));
        }
        if !spec.certificates.is_empty() {
            let dir = manifest::service::certificates_dir(&spec.name);
            environment.push(format!("CERTIFICATES_DIRECTORY={dir}"));
        }
        if !spec.mounts.is_empty() {
            environment.push(format!("MOUNT_PLACES={}", spec.mounts.join(":")));
        }
        let named = match (&account, self.names) {
            (Some(account), Some(slot)) => {
                let groups = views::group_bytes(&account.groups);
                names_abi::name_process(&SYS, slot, child, account.user, account.group, &groups)
                    .map_err(|status| format!("cannot name its process: {:#x}", status.0))
            }
            _ => Ok(()),
        };
        let started = named
            .and_then(|()| {
                let mut shares = shares.iter().copied().chain(sockets).chain(mounts).chain(names);
                shares.try_for_each(|(from, to)| {
                    process::share(&SYS, child, from, to, Rights::ALL, 0)
                        .map_err(|status| format!("sharing slot {to} failed: {:#x}", status.0))
                })
            })
            .and_then(|()| grant_network(self.network, spec, child))
            .and_then(|()| grant_platform(self.platform, spec, child))
            .and_then(|()| grant_clock(self.clock, spec, child))
            .and_then(|()| grant_firewall(self.firewall, spec, child))
            .and_then(|()| grant_settings(self.settings, spec, child))
            .and_then(|()| grant_devices(self.devices, spec, child))
            .and_then(|()| limit_files(spec, child))
            .and_then(|()| {
                let block = exec_block(program_path, &args, &environment);
                process::exec(&SYS, child, program.slot(), &block)
                    .map_err(|status| format!("exec failed: {:#x}", status.0))
            });
        // The child stays in the tree after its capability goes, for `wait`.
        finish(child);
        finish(write);
        started.map(|()| {
            non_blocking(read);
            (number, output)
        })
    }

    /// Service `index` ended with `status` (None: it could not start), and
    /// what it left in its domain has ended: it is done, started again
    /// later, given up on, or stopped, as its file and the requests say.
    /// A failed start counts toward its `give_up_after`: one that could
    /// not be made, an end before it was ready, a restart for failed
    /// health checks, or a failing end within STEADY of being ready.
    fn schedule(&mut self, index: usize, status: Option<u32>) {
        let shutting_down = matches!(self.shutdown, Some(Shutdown::Stopping { .. }));
        let time = now();
        let service = &mut self.services[index];
        service.last = status;
        let asked = core::mem::take(&mut service.restarting);
        let unhealthy = core::mem::take(&mut service.unhealthy);
        let counted = !asked && !service.held && !shutting_down;
        let finished = counted
            && status == Some(0)
            && service.spec.ready.when == ReadyWhen::Ended
            && service.ready_ns.is_none();
        let steady = service.ready_ns.is_some_and(|ready| time.saturating_sub(ready) >= STEADY);
        let failed_start = counted
            && !finished
            && (status.is_none()
                || service.ready_ns.is_none()
                || unhealthy
                || status.is_some_and(failed) && !steady);
        if failed_start {
            service.failures += 1;
            service.failed_starts += 1;
        } else if steady || finished {
            service.failures = 0;
        }
        if finished {
            // A setup step: ready, and done, which those that require it
            // count as ready.
            service.ready_ns = Some(time);
            service.set(State::Done);
            say(&mut self.told, index, format!("{} is done", service.spec.name));
            return;
        }
        // A restart request is a start, whatever the file says.
        let restart = asked && !service.held && !shutting_down;
        let again = restart
            || counted
                && match service.spec.restart {
                    Restart::Always => true,
                    Restart::OnFailure => failed_start || status.is_none_or(failed),
                    Restart::Never => false,
                };
        if !again {
            return self.settle_end(index, status);
        }
        let give_up = service.spec.give_up_after;
        if !restart && give_up > 0 && service.failures >= give_up {
            let last = match (&service.why, status) {
                (Some(why), _) => why.clone(),
                (None, Some(status)) => format!("it {}", ending(status)),
                (None, None) => String::from("it could not start"),
            };
            let name = &service.spec.name;
            let line = format!(
                "gave up on {name} after {give_up} failed starts in a row (the last: {last}); \
                 `service {name} start` tries again"
            );
            say(&mut self.told, index, line);
            service.set_why(
                Some(format!("it failed to start {give_up} times in a row (the last: {last})")),
                None,
            );
            service.set(State::Failed);
            self.close_sockets(index);
            self.stop_dependents(index);
            return;
        }
        if status.is_some() && !unhealthy && time.saturating_sub(service.started_ns) >= STEADY {
            service.quick = 0;
        }
        let wait = (BACKOFF_FIRST << service.quick.min(20)).min(BACKOFF_MAX);
        service.quick += 1;
        service.restarts += 1;
        service.set(State::Waiting(time + wait));
        let line = format!(
            "starting {} again in {}.{} s",
            service.spec.name,
            wait / SECOND,
            wait % SECOND / (100 * MS)
        );
        say(&mut self.told, index, line);
    }

    /// Service `index` ended with `status` and is not started again. An
    /// on-connection service waits for the next connection, which leaves
    /// what requires it running, as it takes connections still; after a
    /// failing end soon after its start, not asked for, or more than
    /// QUICK_ENDS_FREE quick ones in a row, only once the backoff of
    /// restarts has passed, so that one that ends without taking the
    /// connection that waits is not started again at once, for good. Any
    /// other is stopped, which stops the services that require it, and
    /// its sockets are closed, unless its stop asked to keep them. Only a
    /// service whose `ready.when` is `ended` is done when it ends well.
    fn settle_end(&mut self, index: usize, status: Option<u32>) {
        let shutting_down = matches!(self.shutdown, Some(Shutdown::Stopping { .. }));
        let service = &mut self.services[index];
        if let Some(status) = status.filter(|&status| failed(status))
            && service.why.is_none()
            && !service.held
        {
            service.set_why(Some(format!("it {}", ending(status))), None);
        }
        let keep = core::mem::take(&mut service.keep_sockets);
        if !shutting_down && service.waits_for_connections() {
            let time = now();
            let quick = !service.held && time.saturating_sub(service.started_ns) < STEADY;
            let at = if quick {
                service.quick += 1;
                let over = match status {
                    Some(0) => service.quick.saturating_sub(QUICK_ENDS_FREE),
                    _ => service.quick,
                };
                match over {
                    0 => time,
                    over => time + (BACKOFF_FIRST << (over - 1).min(20)).min(BACKOFF_MAX),
                }
            } else {
                service.quick = 0;
                time
            };
            service.set(State::Listening(at));
            return;
        }
        service.set(State::Stopped);
        if !keep {
            self.close_sockets(index);
        }
        if !shutting_down {
            self.stop_dependents(index);
        }
    }

    /// Copies what is in `output` to the log, a line at a time, each
    /// after `name: ` unless it starts with it, keeping the last in `last`
    /// if given. Returns false at its end.
    fn copy_lines(
        output: &File,
        line: &mut Vec<u8>,
        name: Option<&str>,
        mut last: Option<&mut Option<String>>,
    ) -> bool {
        let mut flush = |line: &mut Vec<u8>| {
            if let (Some(said), Some(last)) = (flush_line(line, name), last.as_deref_mut()) {
                *last = Some(kept(said));
            }
        };
        let mut buffer = [0u8; 512];
        loop {
            match file_abi::read(&SYS, output.slot(), file_abi::CURRENT, &mut buffer) {
                Ok(read) if read.count == 0 => {
                    flush(line);
                    return false;
                }
                Ok(read) => {
                    for &byte in &buffer[..read.count as usize] {
                        match byte {
                            b'\r' => {}
                            b'\n' => flush(line),
                            _ if line.len() < LINE_MAX => line.push(byte),
                            _ => {}
                        }
                    }
                }
                // Would block, or gone.
                Err(_) => return true,
            }
        }
    }

    /// Reaps the children that ended.
    fn reap(&mut self) {
        loop {
            let Ok(done) = process::wait(&SYS, SLOT_PROCESS, -1, ChildWaitFlags::NO_HANG) else {
                return;
            };
            if done.pid == 0 {
                return;
            }
            self.ended(done.pid, done.status);
        }
    }

    /// Process `pid` ended with `status`: a backup hook, the program
    /// Spark was asked to run, a health check, or a service.
    fn ended(&mut self, pid: u64, status: u32) {
        if self.hooks.as_ref().and_then(|h| h.running.as_ref()).is_some_and(|r| r.pid == pid) {
            self.hook_ended(status);
            return;
        }
        if self.program.as_ref().is_some_and(|p| p.number == pid) {
            let Some(mut program) = self.program.take() else { return };
            Self::copy_lines(&program.master, &mut program.line, None, None);
            flush_line(&mut program.line, None);
            println!("spark: process {pid} ({}) {}", program.path, ending(status));
            self.note_idle();
            return;
        }
        if self.check_ended(pid, status) {
            return;
        }
        let Some(index) = self.services.iter().position(|s| s.state.pid() == Some(pid)) else {
            return;
        };
        let service = &mut self.services[index];
        if let Some(output) = service.output.take() {
            let name = service.spec.name.clone();
            Self::copy_lines(&output, &mut service.line, Some(&name), Some(&mut service.last_line));
            if let Some(said) = flush_line(&mut service.line, Some(&name)) {
                service.last_line = Some(kept(said));
            }
        }
        let cause = service.limit_end(status);
        let line = format!("{} (process {pid}) {}{cause}", service.spec.name, ending(status));
        say(&mut self.told, index, line);
        service.stop_by = None;
        // A check of a service that has ended says nothing of it.
        let checking = service.checks.stop();
        if !self.drain(index, status, checking) {
            self.schedule(index, Some(status));
        }
    }

    /// Service `index`'s process ended with `status`: stops what it left
    /// in its domain, if anything. Returns whether it waits for that to
    /// end before the service is started again. With `checking`, its
    /// health check was running, and was stopped: what is left may be
    /// that check alone, so it is not logged as left behind.
    fn drain(&mut self, index: usize, status: u32, checking: bool) -> bool {
        let service = &mut self.services[index];
        let killed = core::mem::take(&mut service.killed);
        let Some(domain) = service.domain else { return false };
        let threads = resource_domain::info(&SYS, domain).map_or(0, |info| info.threads);
        if threads == 0 {
            return false;
        }
        if checking {
            let _ = resource_domain::stop(&SYS, domain);
        } else if !killed {
            let plural = if threads == 1 { "" } else { "s" };
            let line = format!(
                "{} left {threads} thread{plural} behind; stopping its domain",
                service.spec.name
            );
            say(&mut self.told, index, line);
            let _ = resource_domain::stop(&SYS, domain);
        }
        service.set(State::Draining(status, now() + DRAIN_POLL));
        true
    }

    /// Moves on the stops that are due: a service that did not end in
    /// STOP_WAIT has its domain stopped, and one whose domain has emptied
    /// is started again or not.
    fn step_stops(&mut self, time: u64) {
        for index in 0..self.services.len() {
            let service = &mut self.services[index];
            match service.state {
                State::Starting(pid) | State::Running(pid)
                    if service.stop_by.is_some_and(|by| by <= time) =>
                {
                    service.stop_by = None;
                    let line = format!(
                        "{} did not end in {} s; stopping its domain",
                        service.spec.name,
                        service.spec.stop_timeout / SECOND
                    );
                    say(&mut self.told, index, line);
                    match service.domain {
                        Some(domain) => {
                            service.killed = true;
                            let _ = resource_domain::stop(&SYS, domain);
                        }
                        None => {
                            let _ = process::kill(&SYS, SLOT_PROCESS, pid as i64, signal::SIGKILL);
                        }
                    }
                }
                State::Draining(status, _) => {
                    let empty = service.domain.is_none_or(|d| {
                        resource_domain::info(&SYS, d).is_ok_and(|i| i.threads == 0)
                    });
                    if empty {
                        self.schedule(index, Some(status));
                    } else {
                        service.state = State::Draining(status, time + DRAIN_POLL);
                    }
                }
                _ => {}
            }
        }
    }

    /// Takes the connections waiting at the control socket, or at certd's
    /// if `certd`: as many as there is room for, CONNECTIONS_MAX of the
    /// control socket's and one of certd's, so that neither keeps the
    /// other out.
    fn accept(&mut self, certd: bool) {
        let listener = if certd { &self.certd_listener } else { &self.listener };
        let Some(listener) = listener else { return };
        while self.has_room(certd) {
            let Some(slot) = slots::take() else { return };
            if file_abi::accept(&SYS, listener.slot(), slot, PipeFlags::NON_BLOCKING).is_err() {
                slots::give_back(slot);
                return;
            }
            if let Ok(file) = File::own(slot) {
                let until = now() + CONNECTION_WAIT;
                let (peer, user) = file_abi::socket_state(&SYS, slot)
                    .map_or((0, 0), |state| (state.peer, state.user));
                self.connections.push(Connection {
                    file,
                    peer,
                    user,
                    request: Vec::new(),
                    until,
                    answered: false,
                    out: Vec::new(),
                    written: 0,
                    watch: None,
                    certd,
                });
            }
        }
    }

    /// Whether there is room for another connection to the control
    /// socket, or to certd's if `certd`.
    fn has_room(&self, certd: bool) -> bool {
        let open = self.connections.iter().filter(|c| c.certd == certd).count();
        open < if certd { 1 } else { CONNECTIONS_MAX }
    }

    /// Reads what connection `index` sent; once it is a whole line,
    /// answers it, and closes the connection once the answer is written,
    /// or, for a watch, once it has settled. A watch with nothing to write
    /// is polled only for its reader going.
    fn serve(&mut self, index: usize) {
        if self.connections[index].idle() {
            self.connections.remove(index);
            return;
        }
        if self.connections[index].answered {
            return self.write_answer(index);
        }
        let mut buffer = [0u8; 256];
        let connection = &mut self.connections[index];
        let done =
            match file_abi::read(&SYS, connection.file.slot(), file_abi::CURRENT, &mut buffer) {
                Ok(read) if read.count == 0 => true,
                Ok(read) => {
                    connection.request.extend_from_slice(&buffer[..read.count as usize]);
                    connection.request.contains(&b'\n') || connection.request.len() > 1024
                }
                Err(_) => false,
            };
        if !done {
            return;
        }
        let request = String::from_utf8_lossy(&connection.request).into_owned();
        let request = request.lines().next().unwrap_or("").trim().to_string();
        let asker = (connection.peer, connection.user);
        if connection.certd {
            let answer = self.answer_certd(&request, asker.0);
            let connection = &mut self.connections[index];
            connection.answered = true;
            connection.out = answer.into_bytes();
            connection.until = now() + CONNECTION_WAIT;
            return self.write_answer(index);
        }
        if let Some(phase) = request.strip_prefix("hooks ") {
            let connection = self.connections.remove(index);
            self.begin_hooks(connection.file, phase.trim());
            return;
        }
        let (answer, watch) = self.answer(&request, asker);
        let connection = &mut self.connections[index];
        connection.answered = true;
        connection.out = answer.into_bytes();
        connection.until = watch.as_ref().map_or(now() + CONNECTION_WAIT, Watch::until);
        connection.watch = watch;
        self.write_answer(index);
    }

    /// Writes what the socket takes of what connection `index` has to
    /// write, and closes the connection once all of it is written, unless
    /// it is a watch that has not settled, or once its reader has gone.
    fn write_answer(&mut self, index: usize) {
        let connection = &mut self.connections[index];
        while connection.written < connection.out.len() {
            let rest = &connection.out[connection.written..];
            match file_abi::write(&SYS, connection.file.slot(), file_abi::CURRENT, rest) {
                Ok(wrote) if wrote.count > 0 => connection.written += wrote.count as usize,
                // The rest goes once the reader has read some.
                Err(status) if Kernel::from_status(status) == Some(Kernel::WouldBlock) => return,
                _ => {
                    self.connections.remove(index);
                    return;
                }
            }
        }
        if connection.watch.is_some() {
            connection.out.clear();
            connection.written = 0;
        } else {
            self.connections.remove(index);
        }
    }

    /// The answer to the control request `request` from `asker`, a
    /// process and the user it acts for, and the watch it asked for, if
    /// any; a request that changes something is a security event.
    fn answer(&mut self, request: &str, asker: (u64, u32)) -> (String, Option<Watch>) {
        let mut words = request.split_whitespace();
        let verb = words.next().unwrap_or("");
        let rest: Vec<&str> = words.collect();
        let peer = asker.0;
        let answer = match verb {
            "poweroff" | "reboot" | "halt" => {
                let end = match verb {
                    "poweroff" => End::PowerOff,
                    "reboot" => End::Restart,
                    _ => End::Halt,
                };
                let force = rest.contains(&"--force");
                let delay = rest.iter().find_map(|w| w.parse::<u64>().ok()).unwrap_or(0);
                if force {
                    audit!("asked to {} the machine at once, by process {peer}", end.words());
                    self.power(end);
                }
                // A delay of centuries waits for good, not round to soon.
                let at = now().saturating_add(delay.saturating_mul(SECOND));
                self.shutdown = Some(Shutdown::Pending { end, at });
                if delay == 0 {
                    audit!("asked to {} the machine, by process {peer}", end.words());
                    format!("ok: going to {} now\n", end.words())
                } else {
                    audit!("the machine will {} in {delay} s, by process {peer}", end.words());
                    format!("ok: going to {} in {delay} seconds\n", end.words())
                }
            }
            "cancel" => match self.shutdown {
                Some(Shutdown::Pending { end, .. }) => {
                    self.shutdown = None;
                    audit!("the {} was cancelled, by process {peer}", end.words());
                    "ok: cancelled\n".into()
                }
                Some(Shutdown::Stopping { .. }) => "error: shutting down already\n".into(),
                None => "error: no shutdown is pending\n".into(),
            },
            "status" => self.answer_status(),
            "show" => self.answer_show(rest.first().copied()),
            "reload" => self.answer_reload(rest.first().copied(), peer),
            "certificate" => self.answer_certificate(&rest, peer),
            "reopen" => self.answer_reopen(rest.first().copied(), peer),
            "start" => return self.answer_service(Verb::Start, &rest, asker),
            "stop" => return self.answer_service(Verb::Stop, &rest, asker),
            "restart" => return self.answer_service(Verb::Restart, &rest, asker),
            "try" => self.answer_try(&rest, asker),
            "keep" => self.answer_keep(rest.first().copied(), peer),
            "undo" => self.answer_undo(rest.first().copied(), peer),
            "" => "error: an empty request\n".into(),
            other => format!("error: unknown request `{other}`\n"),
        };
        (answer, None)
    }

    /// The answer to `reload NAME` from process `peer`: SIGHUP for the
    /// service, if it runs.
    fn answer_reload(&self, name: Option<&str>, peer: u64) -> String {
        let Some(name) = name else {
            return "error: reload needs a service's name\n".into();
        };
        let Some(service) = self.services.iter().find(|s| s.spec.name == name) else {
            return format!("error: no service is called {name}\n");
        };
        let Some(pid) = service.state.pid() else {
            return format!("error: {name} is not running\n");
        };
        let _ = process::kill(&SYS, SLOT_PROCESS, pid as i64, signal::SIGHUP);
        audit!("asked {name} to reload, by process {peer}");
        format!("ok: asked {name} to reload\n")
    }

    /// The answer to `reopen NAME` from process `peer`: has the service
    /// open its log files again, as its file's `logs.reopen` says, once
    /// logturn(8) has moved them (ADR 0478).
    fn answer_reopen(&mut self, name: Option<&str>, peer: u64) -> String {
        let Some(name) = name else {
            return "error: reopen needs a service's name\n".into();
        };
        let Some(index) = self.services.iter().position(|s| s.spec.name == name) else {
            return format!("error: no service is called {name}\n");
        };
        let service = &self.services[index];
        let Some(reopen) = service.spec.reopen else {
            return format!("error: {name}'s file names no log files of its own\n");
        };
        let running = matches!(service.state, State::Starting(_) | State::Running(_))
            && service.stop_by.is_none();
        match (reopen, service.state.pid()) {
            (Reopen::Nothing, _) => format!("ok: {name} opens its log files for each line\n"),
            _ if !running => {
                format!("ok: {name} is not running; it opens its log files as it starts\n")
            }
            (Reopen::Signal(signal), Some(pid)) => {
                let _ = process::kill(&SYS, SLOT_PROCESS, pid as i64, u32::from(signal.number()));
                audit!("asked {name} to open its log files again, by process {peer}");
                format!("ok: sent {name} {} to open its log files again\n", signal.name())
            }
            (Reopen::Restart, Some(_)) => {
                audit!("asked to restart {name} to open its log files again, by process {peer}");
                self.answer_restart(index)
            }
            (_, None) => format!("ok: {name} is not running\n"),
        }
    }

    /// The answer to `start`, `stop` or `restart` NAME, from `asker`; with
    /// `--wait` after the name, and an `ok:` answer, the watch that follows
    /// it until the service settles, unless WATCHES_MAX run already. A
    /// stop may keep the service's sockets open (`--keep-sockets`, as
    /// bundled's update asks), a start may be timerd's (`--by timer`), and
    /// one that `service enable` sends does what boot does for the service
    /// (`--as-at-boot`).
    fn answer_service(
        &mut self,
        verb: Verb,
        rest: &[&str],
        (peer, user): (u64, u32),
    ) -> (String, Option<Watch>) {
        let word = verb.word();
        let Some((name, flags)) = rest.split_first() else {
            return (format!("error: {word} needs a service's name\n"), None);
        };
        let name = *name;
        let (mut wait, mut keep, mut by_timer, mut as_at_boot) = (false, false, false, false);
        let mut flags = flags.iter();
        while let Some(&flag) = flags.next() {
            match (verb, flag) {
                (_, "--wait") => wait = true,
                (Verb::Stop, "--keep-sockets") => keep = true,
                (Verb::Start, "--by") if flags.next() == Some(&"timer") => by_timer = true,
                (Verb::Start, "--as-at-boot") => as_at_boot = true,
                _ => {
                    let takes = match verb {
                        Verb::Start => "--wait, --as-at-boot and --by timer",
                        Verb::Stop => "--wait and --keep-sockets",
                        Verb::Restart => "--wait",
                    };
                    return (format!("error: {word} takes a service's name, and {takes}\n"), None);
                }
            }
        }
        if matches!(self.shutdown, Some(Shutdown::Stopping { .. })) {
            return ("error: shutting down\n".into(), None);
        }
        // A file added since boot, as `service new` adds one, is read when
        // its service is first started.
        let found = match self.services.iter().position(|s| s.spec.name == name) {
            Some(index) => Ok(index),
            None if verb != Verb::Stop => self.add_linked(name),
            None => Err(format!("no service is called {name}")),
        };
        let index = match found {
            Ok(index) => index,
            Err(err) => return (format!("error: {err}\n"), None),
        };
        audit!("asked to {word} {name}, by process {peer}");
        let watch = if wait { self.watch(index, verb) } else { None };
        let unwatched = wait && watch.is_none();
        let mut answer = match verb {
            Verb::Start if as_at_boot => self.answer_as_at_boot(index),
            Verb::Start => {
                self.answer_start(index, if by_timer { Wanted::Timer } else { Wanted::Request })
            }
            Verb::Stop => self.answer_stop(index, keep),
            Verb::Restart => self.answer_restart(index),
        };
        self.services[index].last_request =
            Some(Request { verb: String::from(word), user, process: peer, ms: now() / MS });
        if !answer.starts_with("ok:") {
            return (answer, None);
        }
        if unwatched {
            answer.truncate(answer.trim_end().len());
            answer.push_str(&format!("; not watched: {} watches already\n", watch::WATCHES_MAX));
        }
        let watch = watch.map(|mut watch| {
            self.follow(&mut watch);
            watch
        });
        (answer, watch)
    }

    /// The answer to `status`: a line a service, its fields after tabs,
    /// for scripts (spark(8)); `show` says more.
    fn answer_status(&self) -> String {
        let mut text = String::new();
        for service in &self.services {
            let (state, pid) = match service.state {
                State::Running(pid) => ("running", pid.to_string()),
                State::Listening(_) => ("listening", "-".into()),
                State::Starting(pid) => ("waiting", pid.to_string()),
                State::Pending | State::Waiting(_) => ("waiting", "-".into()),
                State::Draining(..) => ("stopping", "-".into()),
                State::Stopped if !service.spec.enabled => ("disabled", "-".into()),
                State::Stopped | State::Done | State::Failed => ("stopped", "-".into()),
            };
            let last = service.last.map_or("-".into(), |s| format!("{s:#x}"));
            text.push_str(&format!(
                "{}\t{state}\t{pid}\t{}\t{last}\t{}\t{}\n",
                service.spec.name,
                service.restarts,
                service.spec.restart.name(),
                service.spec.description
            ));
        }
        if let Some(Shutdown::Pending { end, at }) = self.shutdown {
            let left = at.saturating_sub(now()) / SECOND;
            text.push_str(&format!("#\tshutdown\t{}\t{left}\n", end.words()));
        }
        text
    }

    /// Starts service `index` for a request, or for its timer, with what
    /// it requires and wants; the answer says whether it started, or what
    /// it waits for.
    fn answer_start(&mut self, index: usize, wanted: Wanted) -> String {
        let service = &mut self.services[index];
        let name = service.spec.name.clone();
        let stopping = match service.state {
            State::Starting(_) | State::Running(_) => service.stop_by.is_some(),
            State::Draining(..) => true,
            _ => false,
        };
        if stopping {
            // It starts again once it has ended.
            service.held = false;
            service.restarting = true;
            return format!("ok: {name} starts again once it has stopped\n");
        }
        match service.state {
            State::Running(pid) => return format!("ok: {name} is running as process {pid}\n"),
            State::Starting(pid) => {
                return format!("ok: {name} is starting as process {pid}\n");
            }
            // A setup step that is done runs again, as it did before
            // service files could say when a service is ready.
            State::Stopped
            | State::Pending
            | State::Done
            | State::Waiting(_)
            | State::Failed
            | State::Draining(..)
            | State::Listening(_) => {}
        }
        self.want(index, wanted);
        self.step_pending();
        self.started(index)
    }

    /// The answer to `start NAME --as-at-boot`, which `service enable`
    /// sends: for an enabled service that is not running, what boot does.
    /// An on-connection service waits for a connection at its sockets
    /// (ADR 0477), one with a timer is left off for timerd to start at its
    /// times (ADR 0478), and any other service starts.
    fn answer_as_at_boot(&mut self, index: usize) -> String {
        let idle = matches!(
            self.services[index].state,
            State::Stopped | State::Failed | State::Done | State::Waiting(_) | State::Listening(_)
        );
        if !idle {
            return self.answer_start(index, Wanted::Request);
        }
        // Its file was just changed to enable it: read it as a start would.
        self.reread(index);
        let spec = &self.services[index].spec;
        if !spec.enabled {
            return self.answer_start(index, Wanted::Request);
        }
        if spec.timer {
            let name = spec.name.clone();
            // As at boot: stopped, holding no sockets, until its time.
            let service = &mut self.services[index];
            service.held = false;
            service.failures = 0;
            service.restarting = false;
            if !matches!(service.state, State::Stopped | State::Done) {
                service.set(State::Stopped);
            }
            if let Some(held) = service.sockets.take() {
                held.close(&self.root);
            }
            service.leave_off(String::from("timerd starts it at the times its file names"));
            return format!("ok: {name} is left for its timer\n");
        }
        if !spec.on_connection {
            return self.answer_start(index, Wanted::Request);
        }
        let service = &mut self.services[index];
        service.held = false;
        service.failures = 0;
        self.wait_for_connections(index);
        self.started(index)
    }

    /// What became of a request to start service `index`.
    fn started(&self, index: usize) -> String {
        let service = &self.services[index];
        let name = &service.spec.name;
        let why = service.why.as_deref().unwrap_or("it ended at once");
        match service.state {
            State::Running(pid) => format!("ok: started {name} as process {pid}\n"),
            State::Starting(pid) => format!(
                "ok: started {name} as process {pid}; it is ready once {}\n",
                ready_words(&service.spec.ready.when)
            ),
            State::Pending => {
                let waits = starting::and_list(&service.waiting_for);
                let verb = if service.waiting_for.len() == 1 { "is" } else { "are" };
                format!("ok: {name} starts once {waits} {verb} ready\n")
            }
            State::Done => format!("ok: {name} ran and is done\n"),
            State::Listening(_) => format!("ok: {name} waits for a connection\n"),
            State::Waiting(at) => {
                let wait = at.saturating_sub(now());
                format!(
                    "error: could not start {name}: {why}; Spark tries again in {}.{} s\n",
                    wait / SECOND,
                    wait % SECOND / (100 * MS)
                )
            }
            State::Stopped | State::Failed | State::Draining(..) => {
                format!("error: could not start {name}: {why}\n")
            }
        }
    }

    /// Stops service `index` for a request, and those that require it;
    /// none starts again until asked. An on-connection service that is
    /// still enabled waits for a connection once it has ended, as `service
    /// disable` alone stops that; any other closes its sockets, unless
    /// `keep` asks to keep them open, as bundled's update does.
    fn answer_stop(&mut self, index: usize, keep: bool) -> String {
        // `service disable` turns its file's `enabled` off, then asks for
        // the stop: what the file says now decides whether it listens.
        let enabled = services::read(&self.root, &self.dir, &self.services[index].spec.name)
            .map(|(spec, _)| spec.enabled);
        let service = &mut self.services[index];
        if let Ok(enabled) = enabled {
            service.spec.enabled = enabled;
        }
        let name = service.spec.name.clone();
        service.held = true;
        service.restarting = false;
        service.keep_sockets = keep;
        service.set_why(None, None);
        let running = match service.state {
            State::Starting(pid) | State::Running(pid) => {
                // It ends, and `ended` leaves it stopped; `step_stops`
                // stops its domain if it does not.
                service.quick = 0;
                service.ask_to_stop(pid);
                true
            }
            // What it left is ending; then it stays stopped.
            State::Draining(..) => true,
            State::Stopped
            | State::Pending
            | State::Waiting(_)
            | State::Done
            | State::Failed
            | State::Listening(_) => {
                service.keep_sockets = false;
                if service.waits_for_connections() {
                    if !matches!(service.state, State::Listening(_)) {
                        service.set(State::Listening(now()));
                    }
                } else {
                    if service.state != State::Stopped {
                        service.set(State::Stopped);
                    }
                    if !keep && let Some(held) = service.sockets.take() {
                        held.close(&self.root);
                    }
                }
                false
            }
        };
        let service = &self.services[index];
        let after = if service.waits_for_connections() {
            "; it starts again at the next connection"
        } else if keep && service.sockets.is_some() {
            "; Spark keeps its sockets open"
        } else {
            ""
        };
        let others = self.stop_dependents(index);
        match (running, others.is_empty()) {
            (true, _) => {
                format!("ok: stopping {name}{after}{}\n", dependents_words(&others, ", and"))
            }
            (false, true) => format!("ok: {name} is not running{after}\n"),
            (false, false) => format!(
                "ok: {name} is not running{after}{}\n",
                dependents_words(&others, "; stopping")
            ),
        }
    }

    /// Restarts service `index` for a request: a running one is stopped
    /// and started again; any other starts as a start request starts it.
    fn answer_restart(&mut self, index: usize) -> String {
        let service = &mut self.services[index];
        let name = service.spec.name.clone();
        match service.state {
            State::Starting(pid) | State::Running(pid) => {
                service.held = false;
                service.restarting = true;
                service.quick = 0;
                service.failures = 0;
                service.ask_to_stop(pid);
                format!("ok: restarting {name}\n")
            }
            State::Draining(..) => {
                service.held = false;
                service.restarting = true;
                format!("ok: {name} starts again once what it left has ended\n")
            }
            State::Stopped
            | State::Pending
            | State::Waiting(_)
            | State::Done
            | State::Failed
            | State::Listening(_) => {
                self.want(index, Wanted::Request);
                self.step_pending();
                self.started(index)
            }
        }
    }

    /// A connection or a datagram waits at socket `k` of service `index`:
    /// it starts, if it still waits for one, as its file says now.
    fn connected(&mut self, index: usize, k: usize) {
        let service = &self.services[index];
        if !matches!(service.state, State::Listening(_)) {
            return;
        }
        let what = service.spec.sockets.get(k).map_or_else(String::new, |socket| {
            format!(" at {}", sockets::words(socket, &sockets::live_folder(&service.spec)))
        });
        let line = format!("a connection{what} starts {}", service.spec.name);
        say(&mut self.told, index, line);
        self.want(index, Wanted::Connection);
    }

    /// The power button was pressed: powers the machine off now, unless it
    /// is shutting down already. A shutdown set for later is moved to now,
    /// and a restart or halt becomes a power-off.
    fn button_pressed(&mut self) {
        if let Some(Shutdown::Stopping { end, .. }) = self.shutdown {
            println!("spark: the power button was pressed; going to {} already", end.words());
            return;
        }
        audit!("the power button was pressed; powering off the machine");
        self.shutdown = Some(Shutdown::Pending { end: End::PowerOff, at: now() });
    }

    /// Restarts the machine in `seconds`, as an installed system whose
    /// root no disk holds does, so that starts that do not finish bring
    /// Rescue up (ADR 0460).
    pub fn restart_in(&mut self, seconds: u64) {
        println!(
            "spark: restarting in {seconds} seconds; after three starts that do not finish, \
             the boot menu starts Rescue"
        );
        self.shutdown = Some(Shutdown::Pending { end: End::Restart, at: now() + seconds * SECOND });
    }

    /// The host of the virtual machine asked it to restart: restarts it
    /// now, unless it is shutting down already. A shutdown set for later
    /// is moved to now, and becomes a restart.
    fn host_restart(&mut self) {
        if let Some(Shutdown::Stopping { end, .. }) = self.shutdown {
            println!("spark: the host asks for a restart; going to {} already", end.words());
            return;
        }
        audit!("the host asks for a restart; restarting the machine");
        self.shutdown = Some(Shutdown::Pending { end: End::Restart, at: now() });
    }

    /// Ends the machine with the power capability.
    fn power(&mut self, end: End) -> ! {
        if let Some(slot) = self.power {
            let done = match end {
                End::PowerOff => power::power_off(&SYS, slot),
                End::Restart => power::restart(&SYS, slot),
                End::Halt => power::halt(&SYS, slot),
            };
            if let Err(status) = done {
                println!(
                    "spark: could not {} the machine: {:#x}; halting it",
                    end.words(),
                    status.0
                );
                let _ = power::halt(&SYS, slot);
            }
        } else {
            println!("spark: the kernel gave no power capability; nothing can be turned off");
        }
        crate::idle()
    }

    /// The highest stop rank among the services that run: a service's
    /// rank is above those of the services it names (ADR 0470).
    fn running_rank(&self) -> Option<u32> {
        self.services.iter().filter(|s| s.state.pid().is_some()).map(|s| s.rank).max()
    }

    /// Whether service `service` runs, with a stop rank of `rank` or above.
    fn runs_from(service: &Service, rank: u32) -> bool {
        service.state.pid().is_some() && service.rank >= rank
    }

    /// Asks the services of stop rank `rank` to end, each with its own
    /// stop signal, at `time`; returns when the last of their timeouts
    /// passes.
    fn terminate(&mut self, rank: u32, time: u64) -> u64 {
        let mut last = time;
        for service in &mut self.services {
            if let Some(pid) = service.state.pid()
                && service.rank == rank
            {
                service.ask_to_stop(pid);
                last = last.max(service.stop_by.unwrap_or(time));
            }
        }
        last
    }

    /// Moves a shutdown on: starts it when it is due, and stops the
    /// services one group at a time, then ends the machine.
    fn step_shutdown(&mut self) {
        let time = now();
        match self.shutdown {
            Some(Shutdown::Pending { end, at }) if at <= time => {
                let running = self.services.iter().filter(|s| s.state.pid().is_some());
                println!(
                    "spark: shutting down to {}: stopping {} services",
                    end.words(),
                    running.count()
                );
                // A connection no longer starts anything; the sockets close
                // as the machine ends.
                for service in &mut self.services {
                    if matches!(
                        service.state,
                        State::Waiting(_) | State::Pending | State::Listening(_)
                    ) {
                        service.set(State::Stopped);
                    }
                }
                if let Some(program) = &self.program {
                    let _ =
                        process::kill(&SYS, SLOT_PROCESS, program.number as i64, signal::SIGTERM);
                }
                self.next_group(end, time);
            }
            Some(Shutdown::Stopping { end, rank, deadline }) => {
                if !self.services.iter().any(|s| Self::runs_from(s, rank)) {
                    return self.next_group(end, time);
                }
                if deadline > time {
                    return;
                }
                // Each had its domain stopped once its timeout passed
                // (`step_stops`), and KILL_WAIT more: they are as good as
                // gone.
                for service in &mut self.services {
                    if Self::runs_from(service, rank) {
                        service.set(State::Stopped);
                    }
                }
                self.next_group(end, time);
            }
            _ => {}
        }
    }

    /// Stops the next group of services, or once none runs, what their
    /// processes left behind, and ends the machine.
    fn next_group(&mut self, end: End, time: u64) {
        match self.running_rank() {
            Some(rank) => {
                let last = self.terminate(rank, time);
                self.shutdown = Some(Shutdown::Stopping { end, rank, deadline: last + KILL_WAIT });
            }
            None => {
                for service in &self.services {
                    if let Some(domain) = service.domain {
                        let _ = resource_domain::stop(&SYS, domain);
                    }
                }
                println!("spark: every service has stopped");
                self.power(end);
            }
        }
    }

    /// When the loop must look again without a wake-up: the next restart,
    /// readiness look, health check or shutdown step, the watchdog's next
    /// pet, or the end of a trial.
    fn deadline(&self) -> Option<u64> {
        let time = now();
        let restarts = self.services.iter().filter_map(|s| s.next_look(time));
        let shutdown = match self.shutdown {
            Some(Shutdown::Pending { at, .. }) => Some(at),
            Some(Shutdown::Stopping { deadline, .. }) => Some(deadline),
            None => None,
        };
        let connections = self.connections.iter().map(|c| c.until);
        let pet = self.pets.map(|(_, next)| next);
        let hook = self.hooks.as_ref().and_then(|h| h.running.as_ref()).map(|r| r.until);
        let trial = self.trials_deadline();
        restarts.chain(shutdown).chain(connections).chain(pet).chain(hook).chain(trial).min()
    }

    /// Starts the `phase` backup hooks of the running services that have
    /// one, in their order (the reverse after), for `connection`, which
    /// gets the answer once the last ends.
    fn begin_hooks(&mut self, connection: File, phase: &str) {
        let reply = |connection: &File, text: &str| {
            let _ = file_abi::write(&SYS, connection.slot(), file_abi::CURRENT, text.as_bytes());
        };
        let phase = match phase {
            "before" => "before",
            "after" => "after",
            _ => return reply(&connection, "error: hooks are run before or after\n"),
        };
        if self.hooks.is_some() {
            return reply(&connection, "error: backup hooks are running already\n");
        }
        let mut queue: Vec<usize> = (0..self.services.len())
            .filter(|&i| {
                let service = &self.services[i];
                let command = if phase == "before" {
                    &service.spec.hook_before
                } else {
                    &service.spec.hook_after
                };
                matches!(service.state, State::Running(_)) && !command.is_empty()
            })
            .collect();
        queue.sort_by_key(|&i| (self.services[i].spec.order, self.services[i].spec.name.clone()));
        if phase == "before" {
            // Taken from the end.
            queue.reverse();
        }
        audit!("running the {phase} backup hooks of {} services", queue.len());
        self.hooks = Some(Hooks { connection, phase, queue, running: None, answer: String::new() });
        self.next_hook();
    }

    /// Starts the next hook, or answers once none is left.
    fn next_hook(&mut self) {
        loop {
            let Some(hooks) = &mut self.hooks else { return };
            let Some(index) = hooks.queue.pop() else {
                let hooks = self.hooks.take();
                if let Some(hooks) = hooks {
                    let bytes = hooks.answer.as_bytes();
                    let _ =
                        file_abi::write(&SYS, hooks.connection.slot(), file_abi::CURRENT, bytes);
                }
                return;
            };
            let phase = hooks.phase;
            let spec = &self.services[index].spec;
            let command = if phase == "before" { &spec.hook_before } else { &spec.hook_after };
            let (program, arguments) = (command[0].clone(), command[1..].to_vec());
            let (name, seconds) = (spec.name.clone(), spec.hook_seconds);
            match self.spawn_as(index, &program, &arguments, false) {
                Ok((pid, output)) => {
                    println!("spark: {name}'s {phase} backup hook ({program}) is process {pid}");
                    let until = now() + seconds * SECOND;
                    let run = HookRun {
                        index,
                        pid,
                        output: Some(output),
                        line: Vec::new(),
                        until,
                        stopped: false,
                    };
                    if let Some(hooks) = &mut self.hooks {
                        hooks.running = Some(run);
                    }
                    return;
                }
                Err(why) => {
                    println!("spark: {name}'s {phase} backup hook could not start: {why}");
                    if let Some(hooks) = &mut self.hooks {
                        hooks.answer.push_str(&format!("failed: {name}: could not start: {why}\n"));
                    }
                }
            }
        }
    }

    /// Logs what the running hook wrote, after its service's name.
    fn hook_output(&mut self) {
        let Some(run) = self.hooks.as_mut().and_then(|h| h.running.as_mut()) else { return };
        let name = self.services[run.index].spec.name.clone();
        if let Some(output) = &run.output
            && !Self::copy_lines(output, &mut run.line, Some(&name), None)
        {
            run.output = None;
        }
    }

    /// The running hook ended with `status`.
    fn hook_ended(&mut self, status: u32) {
        let Some(hooks) = &mut self.hooks else { return };
        let Some(mut run) = hooks.running.take() else { return };
        let name = self.services[run.index].spec.name.clone();
        if let Some(output) = run.output.take() {
            Self::copy_lines(&output, &mut run.line, Some(&name), None);
            flush_line(&mut run.line, Some(&name));
        }
        let line = if run.stopped {
            format!("failed: {name}: stopped after its time limit\n")
        } else if failed(status) {
            format!("failed: {name}: {}\n", ending(status))
        } else {
            format!("ok: {name}\n")
        };
        println!("spark: {name}'s {} backup hook {}", hooks.phase, ending(status));
        hooks.answer.push_str(&line);
        self.next_hook();
    }

    /// Stops the running hook if its time is up.
    fn step_hook(&mut self, time: u64) {
        let Some(run) = self.hooks.as_mut().and_then(|h| h.running.as_mut()) else { return };
        if run.stopped || time < run.until {
            return;
        }
        run.stopped = true;
        let _ = process::kill(&SYS, SLOT_PROCESS, run.pid as i64, signal::SIGKILL);
    }

    /// Supervises for good.
    pub fn run(&mut self) -> ! {
        self.start_watchdog();
        loop {
            // The children's notification, the power button's, the host's
            // restarts', the control socket, certd's, the connections, the
            // services' outputs, their health checks' and the program's
            // terminal, in that order.
            let mut entries: Vec<(PollEntry, Source)> = Vec::new();
            let readable = PollEvents::READABLE.0 as u16;
            let writable = PollEvents::WRITABLE.0 as u16;
            let mut add = |slot: u32, events: u16, source: Source| {
                entries.push((PollEntry { fd: slot as i32, events, found: 0 }, source));
            };
            if let Some(slot) = self.children {
                add(slot, readable, Source::Children);
            }
            if let Some(slot) = self.button {
                add(slot, readable, Source::Button);
            }
            if let Some(slot) = self.restart {
                add(slot, readable, Source::Restart);
            }
            // A socket whose connections are all taken waits, so that one
            // more waiting to be taken does not wake Spark again at once.
            let rooms = [self.has_room(false), self.has_room(true)];
            if let Some(listener) = self.listener.as_ref().filter(|_| rooms[0]) {
                add(listener.slot(), readable, Source::Listener);
            }
            if let Some(listener) = self.certd_listener.as_ref().filter(|_| rooms[1]) {
                add(listener.slot(), readable, Source::CertdListener);
            }
            for (i, connection) in self.connections.iter().enumerate() {
                // Its request, or room for the rest of its answer; a watch
                // with nothing to write hears only of its reader going,
                // which poll always says.
                let events = if connection.idle() {
                    0
                } else if connection.answered {
                    writable
                } else {
                    readable
                };
                add(connection.file.slot(), events, Source::Connection(i));
            }
            let time = now();
            for (i, service) in self.services.iter().enumerate() {
                if let Some(output) = &service.output {
                    add(output.slot(), readable, Source::Output(i));
                }
                // A connection, or a datagram, at an on-connection
                // service's sockets starts it.
                if let (State::Listening(from), Some(held)) = (service.state, &service.sockets)
                    && from <= time
                {
                    for (k, slot) in held.slots().enumerate() {
                        add(slot, readable, Source::Socket(i, k));
                    }
                }
                if let Some(output) = service.checks.output() {
                    add(output.slot(), readable, Source::Check(i));
                }
            }
            if let Some(program) = &self.program
                && !program.hung_up
            {
                add(program.master.slot(), readable, Source::Program);
            }
            if let Some(output) =
                self.hooks.as_ref().and_then(|h| h.running.as_ref()).and_then(|r| r.output.as_ref())
            {
                add(output.slot(), readable, Source::Hook);
            }
            let mut polled: Vec<PollEntry> = entries.iter().map(|(entry, _)| *entry).collect();
            let (timeout, flags) = match self.deadline() {
                Some(at) => (at.saturating_sub(now()), PollFlags::NONE),
                None => (0, PollFlags::FOREVER),
            };
            let address = polled.as_mut_ptr() as u64;
            let ready = process::poll(
                &SYS,
                SLOT_PROCESS,
                address,
                polled.len() as u32,
                0,
                timeout,
                0,
                flags,
            );
            if ready.is_ok_and(|r| r.ready > 0) {
                // Connections go from the end, so their indices stay right.
                for (entry, (_, source)) in polled.iter().zip(&entries).rev() {
                    if entry.found == 0 {
                        continue;
                    }
                    match *source {
                        Source::Children => {
                            if let Some(slot) = self.children {
                                let _ = syscalls::wait(&SYS, slot, WaitFlags::POLL, 0);
                            }
                        }
                        Source::Button => {
                            if let Some(slot) = self.button {
                                let _ = syscalls::wait(&SYS, slot, WaitFlags::POLL, 0);
                            }
                            self.button_pressed();
                        }
                        Source::Restart => {
                            if let Some(slot) = self.restart {
                                let _ = syscalls::wait(&SYS, slot, WaitFlags::POLL, 0);
                            }
                            self.host_restart();
                        }
                        Source::Listener => self.accept(false),
                        Source::CertdListener => self.accept(true),
                        Source::Connection(i) => self.serve(i),
                        Source::Output(i) => {
                            let service = &mut self.services[i];
                            if let Some(output) = &service.output {
                                let name = service.spec.name.clone();
                                let last = Some(&mut service.last_line);
                                if !Self::copy_lines(output, &mut service.line, Some(&name), last) {
                                    service.output = None;
                                }
                            }
                        }
                        Source::Check(i) => self.services[i].checks.read_output(),
                        Source::Socket(i, k) => self.connected(i, k),
                        Source::Hook => self.hook_output(),
                        Source::Program => {
                            if let Some(program) = &mut self.program {
                                let open = Self::copy_lines(
                                    &program.master,
                                    &mut program.line,
                                    None,
                                    None,
                                );
                                let hang_up = PollEvents::HANG_UP.0 as u16;
                                program.hung_up = !open || entry.found & hang_up != 0;
                            }
                        }
                    }
                }
            }
            self.reap();
            let time = now();
            // A connection that sent no whole request in time is closed;
            // its client reads the end. A watch's time is its own.
            self.connections.retain(|c| c.watch.is_some() || c.until > time);
            self.step_stops(time);
            self.step_hook(time);
            self.step_ready(time);
            self.step_health(time);
            self.step_reloads(time);
            for index in 0..self.services.len() {
                let service = &self.services[index];
                if let State::Waiting(at) = service.state
                    && at <= time
                {
                    if service.spec.on_connection
                        && service.spec.enabled
                        && service.sockets.is_none()
                    {
                        // Its sockets could not be made: it tries again.
                        self.reread(index);
                        self.wait_for_connections(index);
                    } else {
                        // It starts again once what it names is ready, as
                        // its file says now.
                        self.reread(index);
                        self.services[index].set(State::Pending);
                    }
                }
            }
            self.step_trials(time);
            self.step_pending();
            self.step_shutdown();
            self.step_watches(now());
            self.pet_watchdog(time);
        }
    }
}

/// What a poll entry is for.
#[derive(Clone, Copy)]
enum Source {
    Children,
    Button,
    Restart,
    Listener,
    CertdListener,
    Connection(usize),
    Output(usize),
    Check(usize),
    /// Socket `k` of service `i`, which waits for a connection.
    Socket(usize, usize),
    Hook,
    Program,
}

/// When a service whose `ready.when` is `when` is ready, as a clause.
fn ready_words(when: &ReadyWhen) -> String {
    match when {
        ReadyWhen::Started => String::from("started"),
        ReadyWhen::Listening(Some(port)) => format!("it listens on port {port}"),
        ReadyWhen::Listening(None) => String::from("it listens on its ports"),
        ReadyWhen::Socket(path) => format!("{path} is there"),
        ReadyWhen::Healthy => String::from("its health check passes"),
        ReadyWhen::Ended => String::from("it has ended"),
    }
}

/// The services a stop stopped with the one asked for, `names`, after
/// `lead`: ", and web, which requires it"; nothing if there are none.
fn dependents_words(names: &[String], lead: &str) -> String {
    match names {
        [] => String::new(),
        [one] => format!("{lead} {one}, which requires it"),
        more => format!("{lead} {}, which depend on it", starting::and_list(more)),
    }
}

/// Logs `line` after `name: `, unless it starts with that, and empties it;
/// returns its text without its level, if it held any. A `<N>` before the
/// text, which sets its level as on Linux (sd-daemon(3)'s `<3>` for an
/// error), stays in front, where the kernel reads it.
fn flush_line(line: &mut Vec<u8>, name: Option<&str>) -> Option<String> {
    if line.is_empty() {
        return None;
    }
    let text = String::from_utf8_lossy(line);
    let (level, text) = match text.strip_prefix('<').and_then(|rest| rest.split_once('>')) {
        Some((digits, rest))
            if (1..=9).contains(&digits.len()) && digits.bytes().all(|b| b.is_ascii_digit()) =>
        {
            (&text[..digits.len() + 2], rest)
        }
        _ => ("", &*text),
    };
    match name {
        Some(name) if !text.starts_with(name) || !text[name.len()..].starts_with(':') => {
            println!("{level}{name}: {text}");
        }
        _ => println!("{level}{text}"),
    }
    let said = String::from(text);
    line.clear();
    Some(said)
}

/// A service's line as `show` keeps it: at most LAST_LINE_MAX bytes.
fn kept(mut line: String) -> String {
    if line.len() > LAST_LINE_MAX {
        let mut end = LAST_LINE_MAX;
        while !line.is_char_boundary(end) {
            end -= 1;
        }
        line.truncate(end);
    }
    line
}

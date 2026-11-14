// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Service files (service.toml(5), ADR 0216): what Spark starts at boot
//! and keeps running.
//!
//! Each `NAME.toml` file in `/system/settings/spark/services` describes
//! one service, in TOML, read by the manifest crate as `service check`
//! reads it. The system's own come from
//! `/system/resources/default-settings/spark/services`, which Spark copies
//! in when they are missing, so an administrator turns one off with
//! `enabled = false` rather than by removing it.
//!
//! Spark reads a service's file again each time it starts the service.
//! A file that does not read never takes a service down: Spark says which
//! line is wrong and why, and runs the last copy that read, which it keeps
//! in `/system/state/spark/good`. An old file of Phases 9 to 12,
//! `NAME.service` with lines of `key = value` (ADR 0101), is converted to
//! `NAME.toml` with its comments, and kept as `NAME.service.old`.
//!
//! A service reaches the network only as its file says (ADR 0183):
//! `network.outbound = true` lets it connect out, and `network.listen`
//! names the ports it may listen on. `admin.network = true` lets it set
//! the interfaces up, their addresses and routers, as ip(8) does (ADR
//! 0190). `admin.hardware = true` gives it the platform capability, which
//! sets the fans, the LEDs and the ARGB header (ADR 0208), and
//! `admin.time = true` the clock capability, which sets the time of day
//! and the real-time clock (ADR 0194), and `admin.firewall = true` the
//! firewall capability, which gives the packet filter its rules (ADR
//! 0334). The ports a service listens on take what other machines start:
//! they are open while it runs (ADR 0334). A driver program's
//! `devices.pci` names the cards it drives, and Spark gives it each card
//! the kernel left for a program that one matches (ADR 0198). It reaches
//! the tree only through the view Spark builds of what its file declares
//! (`views`, ADR 0217).

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use libradix::abi::capability::Rights;
use libradix::abi::errors::File as FileError;
use libradix::abi::resource_domain::OomClass;
use manifest::service::{self, Health, OVERRIDES, Oom, Override, PciId, Place, Ready};
use radix_rt::fs::Dir;
use radix_rt::println;
use spark_api::graph::Link;

pub use manifest::service::{Restart, check_name};

/// Where Spark keeps the last copy of each service file that read.
pub const GOOD: &str = "system/state/spark/good";

/// One service file, as Spark runs it.
#[derive(Clone, Debug)]
pub struct Spec {
    /// The file's name without `.toml`: the service's and its domain's.
    pub name: String,
    pub description: String,
    /// The program's absolute path.
    pub program: String,
    pub arguments: Vec<String>,
    /// Lower starts first; shutdown stops the highest first.
    pub order: u32,
    pub restart: Restart,
    /// The CPU weight of its domain.
    pub weight: u32,
    /// The memory limit of its domain, in pages.
    pub memory_pages: Option<u64>,
    /// Its domain's out-of-memory class: service or app.
    pub oom: OomClass,
    /// More of the environment, as `NAME=value`.
    pub environment: Vec<String>,
    pub enabled: bool,
    /// Whether it may connect out, to any address and port.
    pub outbound: bool,
    /// The ranges of ports it may listen on, as (first, count).
    pub listen: Vec<(u32, u32)>,
    /// Whether it may set the interfaces up: admin.network.
    pub configure: bool,
    /// Whether it gets the platform capability: admin.hardware.
    pub hardware: bool,
    /// Whether it gets the clock capability: admin.time.
    pub time: bool,
    /// Whether it gets the firewall capability: admin.firewall.
    pub firewall: bool,
    /// The places its view holds beyond what every service's does.
    pub places: Vec<Place>,
    /// The places it mounts filesystems beneath, which it gets as
    /// descriptors too.
    pub mounts: Vec<String>,
    /// Whether it runs with Spark's root, the whole tree, in place of a
    /// view.
    pub whole_tree: bool,
    /// Whether it gets the names capability (ADR 0218).
    pub names: bool,
    /// The names of its secrets.
    pub secrets: Vec<String>,
    /// The names of the certificates Spark copies for it (ADR 0512).
    pub certificates: Vec<String>,
    /// The PCI cards it drives: devices.pci.
    pub devices: Vec<PciId>,
    /// The account it runs as, a bundle's service's own (ADR 0385); when
    /// none, it acts for root.
    pub account: Option<String>,
    /// The commands it runs before and after backups' snapshots, and how
    /// long each may run, in seconds (ADR 0463).
    pub hook_before: Vec<String>,
    pub hook_after: Vec<String>,
    pub hook_seconds: u64,
    /// The services it requires, wants and starts after (ADR 0470).
    pub requires: Vec<String>,
    pub wants: Vec<String>,
    pub after: Vec<String>,
    /// The failed starts in a row after which Spark stops trying; 0 for
    /// never.
    pub give_up_after: u32,
    /// When it is ready, and how long it has to get there.
    pub ready: Ready,
    /// Its health check, if it has one.
    pub health: Option<Health>,
}

impl Spec {
    /// How many network capabilities it gets: one to connect out, one
    /// for each range of ports it listens on, and one to set the
    /// interfaces up.
    pub fn network_grants(&self) -> usize {
        usize::from(self.outbound) + self.listen.len() + usize::from(self.configure)
    }

    /// The services its file names, each with how: what the service
    /// graph goes by.
    pub fn links(&self) -> Vec<(String, Link)> {
        let requires = self.requires.iter().map(|name| (name.clone(), Link::Requires));
        let wants = self.wants.iter().map(|name| (name.clone(), Link::Wants));
        let after = self.after.iter().map(|name| (name.clone(), Link::After));
        requires.chain(wants).chain(after).collect()
    }
}

/// Reads the service file `name`.toml with `text`.
pub fn parse(name: &str, text: &str, over: Option<&Override>) -> Result<Spec, String> {
    check_name(name)?;
    let file = service::from_toml_over(text, over).map_err(|err| err.to_string())?;
    file.check_links(name)?;
    file.check_certificates(name)?;
    let network = &file.permissions.network;
    // A service that names certificates starts after certd, which makes
    // this machine's and keeps the rest, when certd is starting too.
    let mut after = file.after;
    let linked = [&file.requires, &file.wants, &after]
        .iter()
        .any(|names| names.iter().any(|name| name == service::CERTD));
    if !file.certificates.is_empty() && !linked {
        after.push(service::CERTD.to_string());
    }
    Ok(Spec {
        name: name.to_string(),
        description: file.description,
        program: file.program,
        arguments: file.arguments,
        order: file.order,
        restart: file.restart,
        weight: file.limits.cpu_weight,
        memory_pages: file.limits.memory.map(|bytes| bytes.div_ceil(4096)),
        oom: match file.limits.oom {
            Oom::Service => OomClass::Service,
            Oom::App => OomClass::App,
        },
        environment: file
            .environment
            .iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect(),
        enabled: file.enabled,
        outbound: network.outbound,
        listen: network.listen.iter().map(|p| (u32::from(p.first), p.count())).collect(),
        configure: file.permissions.admin.network,
        hardware: file.permissions.admin.hardware,
        time: file.permissions.admin.time,
        firewall: file.permissions.admin.firewall,
        places: file.permissions.places(),
        mounts: file.permissions.files.mount.clone(),
        whole_tree: file.permissions.runs_with_your_access,
        names: file.permissions.names,
        secrets: file.secrets,
        certificates: file.certificates,
        devices: file.permissions.devices.pci,
        account: file.account,
        hook_before: file.backup.before,
        hook_after: file.backup.after,
        hook_seconds: u64::from(file.backup.time_limit),
        requires: file.requires,
        wants: file.wants,
        after,
        give_up_after: file.give_up_after,
        ready: file.ready,
        health: file.health,
    })
}

/// The read and write rights.
const READ_WRITE: Rights = Rights::READ.union(Rights::WRITE);

/// Converts each old `NAME.service` in `dir` that has no `NAME.toml` to
/// one, keeping the old file as `NAME.service.old`; returns how many.
pub fn convert_old(root: &Dir, dir: &str) -> usize {
    let Ok(at) = root.open_dir(dir, READ_WRITE) else { return 0 };
    let Ok(entries) = at.entries() else { return 0 };
    let mut converted = 0;
    for entry in entries {
        let Some(name) = entry.name.strip_suffix(".service") else { continue };
        if at.stat(&format!("{name}.toml")).is_ok() {
            continue;
        }
        match convert(&at, name) {
            Ok(()) => {
                println!(
                    "spark: converted /{dir}/{name}.service to {name}.toml; the old file is \
                     {name}.service.old"
                );
                converted += 1;
            }
            Err(err) => println!("spark: cannot convert /{dir}/{name}.service: {err}"),
        }
    }
    converted
}

/// Converts `name`.service in `dir` to `name`.toml.
fn convert(dir: &Dir, name: &str) -> Result<(), String> {
    let old = format!("{name}.service");
    let text = dir.read_file(&old).map_err(|err| format!("{err}"))?;
    let text = String::from_utf8(text).map_err(|_| String::from("it is not UTF-8 text"))?;
    let converted = manifest::legacy::convert(&text).map_err(|err| err.to_string())?;
    write_new(dir, &format!("{name}.toml"), &converted)?;
    dir.rename(&old, dir, &format!("{old}.old")).map_err(|err| format!("{err}"))?;
    // Sync so the conversion survives a power loss as a whole: the new
    // .toml is already synced by write_new, and this lands the rename.
    dir.sync().map_err(|err| format!("{err}"))
}

/// Writes `text` to `path` in `dir` whole or not at all: to a new file
/// first, synced to the disk, then renamed over it, and the rename
/// synced too. Without the syncs an unclean power loss can leave the
/// file renamed but empty, because the rename reaches the disk while
/// the new file's data is still in the cache behind it; a boot then
/// reads the file as present but blank. (A power cut just after boot
/// used to wipe the service files this way, so no service started.)
pub(crate) fn write_new(dir: &Dir, path: &str, text: &str) -> Result<(), String> {
    write_bytes(dir, path, text.as_bytes())
}

/// Writes `bytes` to `path` in `dir` whole or not at all, as
/// [`write_new`] writes text.
pub(crate) fn write_bytes(dir: &Dir, path: &str, bytes: &[u8]) -> Result<(), String> {
    let new = format!("{path}.new");
    dir.create(&new)
        .and_then(|file| {
            file.write_all_at(0, bytes)?;
            // The new file's data must be on the disk before the rename,
            // and the rename on the disk after it.
            file.sync()?;
            dir.rename(&new, dir, path)?;
            dir.sync()
        })
        .map_err(|err| {
            let _ = dir.remove(&new);
            format!("cannot write {path}: {err}")
        })
}

/// The texts a service is read from: its file, and the file in
/// [`OVERRIDES`] that sets keys over it, if there is one (ADR 0386).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Texts {
    pub file: String,
    pub over: Option<String>,
}

/// Reads the text at `path` beneath `root`; None if nothing is there.
fn read_text(root: &Dir, path: &str) -> Result<Option<String>, String> {
    match root.read_file(path) {
        Ok(bytes) => {
            String::from_utf8(bytes).map(Some).map_err(|_| format!("/{path}: it is not UTF-8 text"))
        }
        Err(err) if err.is(FileError::NotFound) => Ok(None),
        Err(err) => Err(format!("/{path}: {err}")),
    }
}

/// Reads service `name`'s file in `dir`, with its override in
/// `dir/settings.d` set over it: what they say and their texts.
pub fn read(root: &Dir, dir: &str, name: &str) -> Result<(Spec, Texts), String> {
    // A name is never a path.
    check_name(name)?;
    let path = format!("{dir}/{name}.toml");
    let file = read_text(root, &path)?.ok_or_else(|| format!("/{path}: no such file"))?;
    let over_path = format!("{dir}/{OVERRIDES}/{name}.toml");
    let over = read_text(root, &over_path)?;
    let read = match &over {
        Some(text) => {
            Some(Override::from_toml(text).map_err(|err| format!("/{over_path}: {err}"))?)
        }
        None => None,
    };
    let spec = parse(name, &file, read.as_ref()).map_err(|err| match &over {
        Some(_) => format!("/{path}, with /{over_path} over it: {err}"),
        None => format!("/{path}: {err}"),
    })?;
    Ok((spec, Texts { file, over }))
}

/// The last copy of service `name`'s file and override that read, if
/// Spark kept one.
pub fn good(root: &Dir, name: &str) -> Option<(Spec, Texts)> {
    read(root, GOOD, name).ok()
}

/// Keeps `texts` as the last good copy of service `name`'s file and its
/// override.
pub fn keep_good(root: &Dir, name: &str, texts: &Texts) {
    let file = format!("{name}.toml");
    let overrides = format!("{GOOD}/{OVERRIDES}");
    let kept = make_dirs(root, GOOD)
        .and_then(|()| root.open_dir(GOOD, READ_WRITE).map_err(|err| format!("{err}")))
        .and_then(|dir| write_new(&dir, &file, &texts.file))
        .and_then(|()| match &texts.over {
            Some(text) => make_dirs(root, &overrides)
                .and_then(|()| {
                    root.open_dir(&overrides, READ_WRITE).map_err(|err| format!("{err}"))
                })
                .and_then(|dir| write_new(&dir, &file, text)),
            None => match root.remove(&format!("{overrides}/{file}")) {
                Err(err) if !err.is(FileError::NotFound) => Err(format!("{err}")),
                _ => Ok(()),
            },
        });
    if let Err(err) = kept {
        println!("spark: cannot keep a good copy of {name}'s file in /{GOOD}: {err}");
    }
}

/// Makes `path` beneath `root` and each directory on the way.
pub fn make_dirs(root: &Dir, path: &str) -> Result<(), String> {
    let mut at = String::new();
    for part in path.split('/') {
        if !at.is_empty() {
            at.push('/');
        }
        at.push_str(part);
        // One that is there may be on a read-only filesystem, as
        // /system is, where making it fails as read-only.
        if root.stat(&at).is_ok() {
            continue;
        }
        match root.make_dir(&at) {
            Err(err) if !err.is(FileError::Exists) => return Err(format!("/{at}: {err}")),
            _ => {}
        }
    }
    Ok(())
}

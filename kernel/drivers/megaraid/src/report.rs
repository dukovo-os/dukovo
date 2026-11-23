// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What the driver tells of a controller: the text of the files under
//! `/kernel/raid/<n>` (radix-sysfs(5)), from the controller's answers to
//! its management commands.
//!
//! [`gather`] asks the controller what it has (its own facts, its
//! volumes, its disks, its configuration and its battery) and writes the
//! `controller`, `volumes`, `disks` and `battery` files; the host keeps
//! the text and serves it until the next gathering. [`write_event`]
//! writes one line of the `events` file from an event the controller
//! sent. The formats are the files' contract: key lines (`key value`)
//! and record lines (TAB-separated `key=value` fields), described in
//! radix-sysfs(5) and the step's design.

use core::fmt::{self, Display, Write};

use crate::Target;
use crate::dcmd::{
    BbuStatus, BbuType, CachePolicy, Class, Config, CtrlInfo, DiskCache, EventArgs, EventDetail,
    LdInfo, LdList, LdListEntry, LdState, MediaType, PdAddress, PdInfo, PdInterface, PdList,
    PdState, Progress, RaidLevel, Span, Text, mbox, opcode,
};

/// A management command failed, or its answer could not be read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Failed;

/// A writer that refused the text is a failure of the gathering too.
impl From<fmt::Error> for Failed {
    fn from(_: fmt::Error) -> Failed {
        Failed
    }
}

/// Runs a management command: its opcode, its mailbox, and the room for
/// its answer, whose length is the length asked. Returns how many bytes
/// the controller answered with.
pub type Fetch<'a> = dyn FnMut(u32, &[u8; 12], &mut [u8]) -> Result<usize, Failed> + 'a;

/// The room [`gather`] needs for the answers it holds at once.
pub const SCRATCH: usize = 64 * 1024;

/// What the host knows of a controller that its firmware does not say.
#[derive(Clone, Copy, Debug)]
pub struct Place<'a> {
    /// `mfi` or `fusion`.
    pub interface: &'a str,
    /// The part, `SAS3508` ([`crate::Family::name`]).
    pub chip: &'a str,
    /// Its PCI address, `0000:3b:00.0`.
    pub pci: &'a str,
    /// Its NUMA node, if the firmware names one.
    pub numa: Option<u32>,
    /// How many commands the driver keeps in flight.
    pub commands: usize,
}

/// Where [`gather`] writes each file.
pub struct Files<'a> {
    /// `controller`, without its `state` and `fault` lines, which the
    /// host writes when the file is read.
    pub controller: &'a mut dyn Write,
    /// `volumes`.
    pub volumes: &'a mut dyn Write,
    /// `disks`.
    pub disks: &'a mut dyn Write,
    /// `battery`: left empty when the controller has none.
    pub battery: &'a mut dyn Write,
}

/// What [`gather`] found that the host acts on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Summary {
    /// A volume or a disk has a task running (a rebuild, an
    /// initialisation, a check, a patrol read): the host gathers again
    /// in 30 seconds, as its progress moves without an event.
    pub task: bool,
    /// The `battery` file was written.
    pub battery: bool,
    /// The physical disks it wrote a record for.
    pub disks: u16,
}

/// The name the host gave a volume or a pass-through disk (`sda`):
/// writes it to the text and returns true, or writes nothing and returns
/// false if it has none.
pub type OsName<'a> = dyn Fn(Target, &mut dyn Write) -> Result<bool, fmt::Error> + 'a;

/// The size of a block the controller counts sizes in, in bytes.
const BLOCK: u64 = 512;

/// The mailbox of a command that takes none.
const NO_MBOX: mbox::Mailbox = [0; 12];

/// The disk's enclosure device ID when it is in none.
const NO_ENCLOSURE: u16 = 0xffff;

/// What [`gather`] counts while it writes the volumes and the disks, for
/// the controller's and the summary's lines.
#[derive(Default)]
struct Tally {
    /// Volumes written.
    volumes: u32,
    /// Volumes degraded or partly degraded.
    degraded: u32,
    /// Volumes offline.
    offline: u32,
    /// Disks written.
    disks: u32,
    /// Disks passed through to the host.
    jbod: u32,
    /// A volume set to write back writes through now.
    write_through: bool,
    /// A volume or a disk has a task running.
    task: bool,
}

/// A value that is `-` when it is not known.
struct Dash<T>(Option<T>);

impl<T: Display> Display for Dash<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0 {
            Some(v) => v.fmt(f),
            None => f.write_char('-'),
        }
    }
}

/// A firmware text, or `-` if it holds nothing printable.
fn text_or_dash(t: Text<'_>) -> Dash<Text<'_>> {
    Dash((!t.is_empty()).then_some(t))
}

/// A writer that passes on only the printable ASCII of what it is given,
/// so that no value holds a TAB, a newline or any other control byte.
struct Clean<'a>(&'a mut dyn Write);

impl Write for Clean<'_> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for c in s.chars().filter(|c| (' '..='~').contains(c)) {
            self.0.write_char(c)?;
        }
        Ok(())
    }
}

/// Writes one key line: `key value` and a newline.
fn line(out: &mut dyn Write, key: &str, value: impl Display) -> fmt::Result {
    writeln!(out, "{key} {value}")
}

/// One record line being written: fields of `key=value` apart by TABs.
struct Record<'a> {
    out: &'a mut dyn Write,
    first: bool,
}

impl<'a> Record<'a> {
    /// Starts a record line on `out`.
    fn new(out: &'a mut dyn Write) -> Record<'a> {
        Record { out, first: true }
    }

    /// Writes the separator and `key=`.
    fn begin(&mut self, key: &str) -> fmt::Result {
        if !self.first {
            self.out.write_char('\t')?;
        }
        self.first = false;
        write!(self.out, "{key}=")
    }

    /// Writes the field `key` with `value`.
    fn put(&mut self, key: &str, value: impl Display) -> fmt::Result {
        self.begin(key)?;
        write!(self.out, "{value}")
    }

    /// Writes a field of the host's name for `target`, or `-` if it has
    /// none.
    fn os(&mut self, key: &str, os: &OsName<'_>, target: Option<Target>) -> fmt::Result {
        self.begin(key)?;
        let named = match target {
            Some(t) => os(t, &mut Clean(&mut *self.out))?,
            None => false,
        };
        if named { Ok(()) } else { self.out.write_char('-') }
    }

    /// Writes a field of device IDs, comma-separated, or `-` if `each`
    /// gives none: `each` calls the function it is given for every ID.
    fn ids(
        &mut self,
        key: &str,
        each: impl FnOnce(&mut dyn FnMut(u16) -> fmt::Result) -> fmt::Result,
    ) -> fmt::Result {
        self.begin(key)?;
        let mut count = 0usize;
        let out = &mut *self.out;
        each(&mut |id| {
            if count > 0 {
                out.write_char(',')?;
            }
            count += 1;
            write!(out, "{id}")
        })?;
        if count == 0 { self.out.write_char('-') } else { Ok(()) }
    }

    /// Ends the line.
    fn finish(self) -> fmt::Result {
        self.out.write_char('\n')
    }
}

/// Runs a command into `buf` and returns the part of it the controller
/// wrote.
fn ask<'b>(
    fetch: &mut Fetch<'_>,
    op: u32,
    mailbox: &mbox::Mailbox,
    buf: &'b mut [u8],
) -> Result<&'b [u8], Failed> {
    let n = fetch(op, mailbox, buf)?;
    let buf: &'b [u8] = buf;
    Ok(buf.get(..n).unwrap_or(buf))
}

/// Splits off the first `at` bytes of `buf`, if it has them.
fn split(buf: &mut [u8], at: usize) -> Result<(&mut [u8], &mut [u8]), Failed> {
    if buf.len() < at { Err(Failed) } else { Ok(buf.split_at_mut(at)) }
}

/// Reads the configuration into `buf`: its header first, which says how
/// long the whole is. `None` if the controller refuses it, or it does not
/// fit.
fn read_config<'b>(fetch: &mut Fetch<'_>, buf: &'b mut [u8]) -> Option<Config<'b>> {
    let head = ask(fetch, opcode::CFG_READ, &NO_MBOX, buf.get_mut(..Config::HEADER)?).ok()?;
    let size = usize::try_from(u32::from_le_bytes(head.get(..4)?.try_into().ok()?)).ok()?;
    if size < Config::HEADER {
        return None;
    }
    let whole = ask(fetch, opcode::CFG_READ, &NO_MBOX, buf.get_mut(..size)?).ok()?;
    Config::parse(whole).ok()
}

/// A volume's state, in the contract's words.
fn ld_state(state: LdState) -> &'static str {
    match state {
        LdState::Optimal => "optimal",
        LdState::PartiallyDegraded => "partly-degraded",
        LdState::Degraded => "degraded",
        LdState::Offline => "offline",
        LdState::Other(_) => "-",
    }
}

/// A disk's state, in the contract's words.
fn pd_state(state: PdState) -> &'static str {
    match state {
        PdState::UnconfiguredGood => "unconfigured-good",
        PdState::UnconfiguredBad => "unconfigured-bad",
        PdState::HotSpare => "hot-spare",
        PdState::Offline => "offline",
        PdState::Failed => "failed",
        PdState::Rebuild => "rebuilding",
        PdState::Online => "online",
        PdState::Copyback => "copyback",
        PdState::System => "jbod",
        PdState::Other(_) => "other",
    }
}

/// A volume's level, in the contract's words.
fn level(level: RaidLevel) -> &'static str {
    match level {
        RaidLevel::Raid0 => "raid0",
        RaidLevel::Raid1 => "raid1",
        RaidLevel::Raid5 => "raid5",
        RaidLevel::Raid6 => "raid6",
        RaidLevel::Raid1E => "raid1e",
        RaidLevel::Raid00 => "raid00",
        RaidLevel::Raid10 => "raid10",
        RaidLevel::Raid50 => "raid50",
        RaidLevel::Raid60 => "raid60",
        RaidLevel::Unknown => "other",
    }
}

/// A disk's interface, in the contract's words.
fn interface(interface: PdInterface) -> &'static str {
    match interface {
        PdInterface::Sas => "sas",
        PdInterface::Sata => "sata",
        PdInterface::Nvme => "nvme",
        _ => "-",
    }
}

/// A disk's medium, in the contract's words.
fn media(media: Option<MediaType>) -> &'static str {
    match media {
        Some(MediaType::Hdd) => "hdd",
        Some(MediaType::Ssd) => "ssd",
        _ => "-",
    }
}

/// What a volume does with the disks' caches, in the contract's words.
fn disk_cache(cache: DiskCache) -> &'static str {
    match cache {
        DiskCache::Unchanged => "default",
        DiskCache::Enabled => "on",
        DiskCache::Disabled => "off",
        DiskCache::Other(_) => "-",
    }
}

/// `back` for a policy that caches writes, else `through`.
fn write_word(policy: CachePolicy) -> &'static str {
    if policy.write_back() { "back" } else { "through" }
}

/// What the battery's type makes of the controller's `battery` line.
fn battery_word(kind: BbuType) -> &'static str {
    match kind {
        BbuType::None => "none",
        BbuType::Ibbu | BbuType::Bbu => "battery",
        BbuType::Other(_) => "other",
    }
}

/// A task that runs: its name and, if the controller says, its progress.
type Running = Option<(&'static str, Option<Progress>)>;

/// Writes the `task`, `percent` and `elapsed` fields of a record: `none`
/// if `task` is `Some(None)`, all `-` if it is not known (`None`).
fn put_task(rec: &mut Record<'_>, task: Option<Running>, tally: &mut Tally) -> fmt::Result {
    let (name, progress) = match task {
        None => ("-", None),
        Some(None) => ("none", None),
        Some(Some((name, progress))) => {
            tally.task = true;
            (name, progress)
        }
    };
    rec.put("task", name)?;
    rec.put("percent", Dash(progress.map(|p| p.percent())))?;
    rec.put("elapsed", Dash(progress.map(|p| p.elapsed_secs)))
}

/// The task a volume runs, by precedence: a reconstruction, a foreground
/// initialisation, a background initialisation, a check.
fn volume_task(info: &LdInfo<'_>) -> Running {
    if info.reconstruction().is_some() || info.reconstruct_active() {
        Some(("reconstruct", info.reconstruction()))
    } else if let Some(p) = info.foreground_init() {
        Some(("init", Some(p)))
    } else if let Some(p) = info.background_init() {
        Some(("background-init", Some(p)))
    } else {
        info.consistency_check().map(|p| ("check", Some(p)))
    }
}

/// The task a disk runs: a rebuild, a patrol read, a clear, a copyback
/// (which the controller gives no progress for).
fn disk_task(info: &PdInfo<'_>) -> Running {
    if let Some(p) = info.rebuild() {
        Some(("rebuild", Some(p)))
    } else if let Some(p) = info.patrol() {
        Some(("patrol", Some(p)))
    } else if let Some(p) = info.clear() {
        Some(("clear", Some(p)))
    } else if info.state() == PdState::Copyback {
        Some(("copyback", None))
    } else {
        None
    }
}

/// Calls `f` with the device ID of every present disk of the arrays that
/// `spans` lie on, in span order.
fn each_member(
    config: &Config<'_>,
    spans: impl Iterator<Item = Span>,
    f: &mut dyn FnMut(u16) -> fmt::Result,
) -> fmt::Result {
    for span in spans {
        for array in config.arrays().filter(|a| a.array_ref() == span.array_ref) {
            for member in array.members().filter(|m| !m.is_missing()) {
                f(member.device_id)?;
            }
        }
    }
    Ok(())
}

/// The target ID of the volume that has the disk `id` as a member of one
/// of its arrays.
fn volume_of(config: &Config<'_>, id: u16) -> Option<u8> {
    config.lds().find_map(|ld| {
        let member = ld.spans().any(|span| {
            config
                .arrays()
                .filter(|a| a.array_ref() == span.array_ref)
                .any(|a| a.members().any(|m| m.device_id == id))
        });
        member.then(|| ld.target_id())
    })
}

/// Asks for the volume's details and writes its record.
fn write_volume(
    fetch: &mut Fetch<'_>,
    info_buf: &mut [u8],
    entry: LdListEntry,
    config: Option<&Config<'_>>,
    os: &OsName<'_>,
    out: &mut dyn Write,
    tally: &mut Tally,
) -> Result<(), Failed> {
    let target = u16::from(entry.target_id);
    // The length asked is the structure's own, as firmware expects.
    let info_buf = info_buf.get_mut(..LdInfo::LEN).ok_or(Failed)?;
    let info = ask(fetch, opcode::LD_GET_INFO, &mbox::ld_info(target), info_buf)
        .ok()
        .and_then(|b| LdInfo::parse(b).ok());
    let cfg = info.map(|i| i.config());
    // The members come from the drive's own spans, or else from the
    // configuration's entry for it.
    let spans_of = cfg.or_else(|| config?.lds().find(|l| l.target_id() == entry.target_id));
    tally.volumes += 1;
    match entry.state {
        LdState::Degraded | LdState::PartiallyDegraded => tally.degraded += 1,
        LdState::Offline => tally.offline += 1,
        _ => {}
    }
    if let Some(c) = cfg
        && c.default_cache_policy().write_back()
        && !c.current_cache_policy().write_back()
    {
        tally.write_through = true;
    }

    let mut rec = Record::new(out);
    rec.put("target", target)?;
    rec.put("name", Dash(cfg.map(|c| text_or_dash(c.name()))))?;
    rec.put("level", Dash(cfg.map(|c| level(c.raid_level()))))?;
    rec.put("size", entry.size_blocks.saturating_mul(BLOCK))?;
    rec.put("stripe", Dash(cfg.and_then(|c| c.stripe_bytes())))?;
    rec.put("state", ld_state(entry.state))?;
    rec.put("write", Dash(cfg.map(|c| write_word(c.current_cache_policy()))))?;
    rec.put("default-write", Dash(cfg.map(|c| write_word(c.default_cache_policy()))))?;
    let read = |c: &crate::dcmd::LdConfig<'_>| {
        let p = c.current_cache_policy();
        if p.read_ahead() || p.read_adaptive() { "ahead" } else { "none" }
    };
    rec.put("read", Dash(cfg.map(|c| read(&c))))?;
    rec.put("disk-cache", Dash(cfg.map(|c| disk_cache(c.disk_cache()))))?;
    rec.ids("disks", |emit| match (config, spans_of) {
        (Some(config), Some(c)) => each_member(config, c.spans(), emit),
        _ => Ok(()),
    })?;
    rec.os("os", os, Some(Target::Volume(target)))?;
    put_task(&mut rec, info.map(|i| volume_task(&i)), tally)?;
    rec.finish()?;
    Ok(())
}

/// Asks for the disk's details and writes its record.
fn write_disk(
    fetch: &mut Fetch<'_>,
    info_buf: &mut [u8],
    addr: PdAddress,
    config: Option<&Config<'_>>,
    os: &OsName<'_>,
    out: &mut dyn Write,
    tally: &mut Tally,
) -> Result<(), Failed> {
    let id = addr.device_id;
    let info_buf = info_buf.get_mut(..PdInfo::LEN).ok_or(Failed)?;
    let info = ask(fetch, opcode::PD_GET_INFO, &mbox::pd_info(id), info_buf)
        .ok()
        .and_then(|b| PdInfo::parse(b).ok());
    let state = info.map(|i| i.state());
    tally.disks += 1;
    let jbod = state.is_some_and(|s| s.is_jbod());
    if jbod {
        tally.jbod += 1;
    }
    let attached = addr.enclosure_device_id != NO_ENCLOSURE;

    let mut rec = Record::new(out);
    rec.put("id", id)?;
    rec.put("enclosure", Dash(attached.then_some(addr.enclosure_device_id)))?;
    rec.put("index", Dash(attached.then_some(addr.enclosure_index)))?;
    rec.put("slot", addr.slot)?;
    rec.put("state", Dash(state.map(pd_state)))?;
    let kind = info.map(|i| match i.interface_ext() {
        Some(ext) if ext != PdInterface::Unknown => ext,
        _ => i.interface(),
    });
    rec.put("interface", Dash(kind.map(interface)))?;
    rec.put("media", Dash(info.map(|i| media(i.media_type()))))?;
    let inquiry = info.map(|i| i.inquiry());
    rec.put("vendor", Dash(inquiry.map(|q| text_or_dash(q.vendor()))))?;
    rec.put("model", Dash(inquiry.map(|q| text_or_dash(q.product()))))?;
    rec.put("serial", Dash(info.and_then(|i| i.serial()).map(|s| text_or_dash(s))))?;
    rec.put("revision", Dash(inquiry.map(|q| text_or_dash(q.revision()))))?;
    rec.put("size", Dash(info.map(|i| i.coerced_size().saturating_mul(BLOCK))))?;
    rec.put("raw-size", Dash(info.map(|i| i.raw_size().saturating_mul(BLOCK))))?;
    rec.put("media-errors", Dash(info.map(|i| i.media_errors())))?;
    rec.put("other-errors", Dash(info.map(|i| i.other_errors())))?;
    rec.put("predictive", Dash(info.map(|i| i.predictive_failures())))?;
    rec.put("smart-alert", Dash(info.map(|i| if i.smart_alert() { "yes" } else { "no" })))?;
    rec.put("temperature", Dash(info.and_then(|i| i.temperature())))?;
    rec.put("volume", Dash(config.and_then(|c| volume_of(c, id))))?;
    let spare = config.and_then(|c| c.spares().find(|s| s.device_id() == id));
    rec.put("spare", spare.map_or("-", |s| if s.is_dedicated() { "dedicated" } else { "global" }))?;
    rec.os("os", os, jbod.then_some(Target::Disk(id)))?;
    put_task(&mut rec, info.map(|i| disk_task(&i)), tally)?;
    rec.finish()?;
    Ok(())
}

/// Writes the battery file.
fn write_battery(out: &mut dyn Write, status: &BbuStatus<'_>, write_through: bool) -> fmt::Result {
    let flags = status.flags();
    let state = if flags.pack_missing() {
        "missing"
    } else if status.state_of_health_good() == Some(false) {
        "failed"
    } else if flags.learn_cycle_active() {
        "learning"
    } else if flags.charge_active() {
        "charging"
    } else if flags.discharge_active() {
        "discharging"
    } else if flags.voltage_low()
        || flags.temperature_high()
        || flags.learn_cycle_failed()
        || flags.learn_cycle_timeout()
    {
        "degraded"
    } else {
        "optimal"
    };
    let learn = if flags.learn_cycle_active() {
        "active"
    } else if flags.learn_cycle_failed() {
        "failed"
    } else if flags.learn_cycle_timeout() {
        "timed-out"
    } else if flags.learn_cycle_requested() {
        "requested"
    } else {
        "none"
    };
    line(out, "type", battery_word(status.battery_type()))?;
    line(out, "state", state)?;
    line(out, "charge", Dash(status.relative_charge()))?;
    line(out, "temperature", status.temperature_c())?;
    line(out, "voltage", status.voltage_mv())?;
    line(out, "current", status.current_ma())?;
    line(out, "learn", learn)?;
    line(out, "write-through", if write_through { "yes" } else { "no" })?;
    write!(out, "flags ")?;
    let names = [
        (flags.pack_missing(), "pack-missing"),
        (flags.voltage_low(), "voltage-low"),
        (flags.temperature_high(), "temperature-high"),
        (flags.charge_active(), "charging"),
        (flags.discharge_active(), "discharging"),
        (flags.learn_cycle_requested(), "learn-requested"),
        (flags.learn_cycle_active(), "learn-active"),
        (flags.learn_cycle_failed(), "learn-failed"),
        (flags.learn_cycle_timeout(), "learn-timeout"),
        (flags.i2c_error(), "i2c-errors"),
    ];
    let mut any = false;
    for (_, name) in names.iter().filter(|(set, _)| *set) {
        if any {
            out.write_char(',')?;
        }
        any = true;
        out.write_str(name)?;
    }
    if !any {
        out.write_char('-')?;
    }
    out.write_char('\n')
}

/// Asks the controller, through `fetch`, what it has, holding its answers
/// in `scratch` ([`SCRATCH`] bytes at least), and writes the files. A
/// controller that refuses an optional command (the battery's, on a part
/// with none) still gives the rest; one that refuses the controller's
/// own facts, the volumes or the disks fails the whole.
pub fn gather(
    fetch: &mut Fetch<'_>,
    scratch: &mut [u8],
    place: &Place<'_>,
    os: &OsName<'_>,
    files: &mut Files<'_>,
) -> Result<Summary, Failed> {
    let (ctrl_buf, rest) = split(scratch, CtrlInfo::LEN)?;
    let (ld_buf, rest) = split(rest, LdList::LEN)?;
    let (pd_buf, rest) = split(rest, PdList::LEN)?;
    // A part for the details of one volume, one disk or the battery, which
    // each answer in turn, a disk's being the longest; the configuration
    // has what is left.
    const _: () = assert!(LdInfo::LEN <= PdInfo::LEN && BbuStatus::LEN <= PdInfo::LEN);
    let (info_buf, cfg_buf) = split(rest, PdInfo::LEN)?;

    // The commands the files cannot do without.
    let ctrl = CtrlInfo::parse(ask(fetch, opcode::CTRL_GET_INFO, &NO_MBOX, ctrl_buf)?)
        .map_err(|_| Failed)?;
    let lds =
        LdList::parse(ask(fetch, opcode::LD_GET_LIST, &NO_MBOX, ld_buf)?).map_err(|_| Failed)?;
    let query = mbox::pd_list_query_all();
    let pds =
        PdList::parse(ask(fetch, opcode::PD_LIST_QUERY, &query, pd_buf)?).map_err(|_| Failed)?;
    // The configuration, which names the volumes' and the spares' disks.
    let config = read_config(fetch, cfg_buf);
    let config = config.as_ref();

    let mut tally = Tally::default();

    // The volumes, by target ID.
    let mut last = None;
    while let Some(entry) =
        lds.iter().filter(|e| last.is_none_or(|l| e.target_id > l)).min_by_key(|e| e.target_id)
    {
        last = Some(entry.target_id);
        write_volume(fetch, info_buf, entry, config, os, files.volumes, &mut tally)?;
    }

    // The physical disks, by device ID; enclosures and the like are not
    // disks.
    let mut last = None;
    while let Some(addr) = pds
        .iter()
        .filter(|a| a.scsi_device_type == 0 && last.is_none_or(|l| a.device_id > l))
        .min_by_key(|a| a.device_id)
    {
        last = Some(addr.device_id);
        write_disk(fetch, info_buf, addr, config, os, files.disks, &mut tally)?;
    }

    // The battery, which the controller's `battery` line names.
    let status = if ctrl.hw_present().bbu() {
        info_buf
            .get_mut(..BbuStatus::LEN)
            .and_then(|buf| ask(fetch, opcode::BBU_GET_STATUS, &NO_MBOX, buf).ok())
            .and_then(|b| BbuStatus::parse(b).ok())
    } else {
        None
    };
    let battery = match (ctrl.hw_present().bbu(), &status) {
        (false, _) => "none",
        (true, Some(s)) => battery_word(s.battery_type()),
        // The controller has one but will not say what: keep that.
        (true, None) => "other",
    };
    let status = status.filter(|s| s.battery_type() != BbuType::None);
    if let Some(s) = &status {
        write_battery(files.battery, s, tally.write_through)?;
    }

    let mode = match (tally.volumes, tally.jbod) {
        (0, _) => "hba",
        (_, 0) => "raid",
        _ => "mixed",
    };
    let out = &mut *files.controller;
    line(out, "model", text_or_dash(ctrl.product_name()))?;
    line(out, "serial", text_or_dash(ctrl.serial_number()))?;
    line(out, "package", text_or_dash(ctrl.package_version()))?;
    line(out, "firmware", Dash(ctrl.firmware_version().map(text_or_dash)))?;
    line(out, "interface", place.interface)?;
    line(out, "chip", place.chip)?;
    line(out, "pci", place.pci)?;
    line(out, "numa", Dash(place.numa))?;
    line(out, "cache", Dash(ctrl.cache_memory_mb().map(|mb| u64::from(mb) << 20)))?;
    line(out, "mode", mode)?;
    line(out, "battery", battery)?;
    line(out, "temperature", Dash(ctrl.roc_temperature()))?;
    line(out, "volumes", tally.volumes)?;
    line(out, "degraded", tally.degraded)?;
    line(out, "offline", tally.offline)?;
    line(out, "disks", tally.disks)?;
    line(out, "critical", ctrl.pd_disks_critical())?;
    line(out, "failed", ctrl.pd_disks_failed())?;
    line(out, "commands", place.commands)?;

    Ok(Summary {
        task: tally.task,
        battery: status.is_some(),
        disks: u16::try_from(tally.disks).unwrap_or(u16::MAX),
    })
}

/// The volume, disk and progress an event's arguments name.
#[derive(Default)]
struct Named {
    volume: Option<u16>,
    disk: Option<crate::dcmd::PdRef>,
    percent: Option<u8>,
}

/// What the arguments of an event name.
fn named(args: EventArgs<'_>) -> Named {
    let mut n = Named::default();
    match args {
        EventArgs::Ld(ld)
        | EventArgs::LdCount { ld, .. }
        | EventArgs::LdLba { ld, .. }
        | EventArgs::LdOwner { ld, .. }
        | EventArgs::LdState { ld, .. }
        | EventArgs::LdStrip { ld, .. } => n.volume = Some(ld.target_id),
        EventArgs::LdProgress { ld, progress } => {
            n.volume = Some(ld.target_id);
            n.percent = Some(progress.percent());
        }
        EventArgs::CdbSense { pd, .. }
        | EventArgs::Pd(pd)
        | EventArgs::PdError { pd, .. }
        | EventArgs::PdLba { pd, .. }
        | EventArgs::PdState { pd, .. } => n.disk = Some(pd),
        EventArgs::PdProgress { pd, progress } => {
            n.disk = Some(pd);
            n.percent = Some(progress.percent());
        }
        EventArgs::LdLbaPdLba { ld, pd, .. } | EventArgs::PdLbaLd { pd, ld, .. } => {
            n.volume = Some(ld.target_id);
            n.disk = Some(pd);
        }
        _ => {}
    }
    n
}

/// An event's class, in the contract's words.
fn class(class: Class) -> &'static str {
    match class {
        Class::Debug => "debug",
        Class::Progress => "progress",
        Class::Info => "info",
        Class::Warning => "warning",
        Class::Critical => "critical",
        Class::Fatal => "fatal",
        Class::Dead => "dead",
        Class::Other(_) => "-",
    }
}

/// An event's locale bits as a comma list, or `-` for none.
struct Locales(u16);

impl Display for Locales {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        const NAMES: [&str; 8] =
            ["volume", "disk", "enclosure", "battery", "sas", "controller", "config", "cluster"];
        let mut any = false;
        for (bit, name) in NAMES.iter().enumerate() {
            if self.0 & (1 << bit) != 0 {
                if any {
                    f.write_char(',')?;
                }
                any = true;
                f.write_str(name)?;
            }
        }
        if any { Ok(()) } else { f.write_char('-') }
    }
}

/// An event's time stamp: seconds since 2000-01-01, or `boot+N` for the
/// seconds since the controller started, which it marks with 0xFF in the
/// top byte.
struct Stamp(u32);

impl Display for Stamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0 >> 24 == 0xff {
            write!(f, "boot+{}", self.0 & 0x00ff_ffff)
        } else {
            self.0.fmt(f)
        }
    }
}

/// Writes the event `detail` (a `CTRL_EVENT_GET` or `CTRL_EVENT_WAIT`
/// answer, [`crate::dcmd::EventDetail`]) as a line of the `events` file,
/// with its newline. Writes nothing for an answer that holds no event
/// (sequence number 0 and no description, which QEMU's model gives).
pub fn write_event(detail: &[u8], out: &mut dyn Write) -> fmt::Result {
    let Ok(event) = EventDetail::parse(detail) else {
        return Ok(());
    };
    let text = event.description();
    if event.sequence() == 0 && text.is_empty() {
        return Ok(());
    }
    let n = named(event.args());
    let mut rec = Record::new(out);
    rec.put("seq", event.sequence())?;
    rec.put("time", Stamp(event.time()))?;
    rec.put("class", class(event.class()))?;
    rec.put("locale", Locales(event.locale()))?;
    rec.put("code", format_args!("{:#06x}", event.code()))?;
    rec.put("volume", Dash(n.volume))?;
    rec.put("disk", Dash(n.disk.map(|d| d.device_id)))?;
    rec.put("enclosure", Dash(n.disk.map(|d| d.enclosure_index)))?;
    rec.put("slot", Dash(n.disk.map(|d| d.slot)))?;
    rec.put("percent", Dash(n.percent))?;
    rec.put("text", text_or_dash(text))?;
    rec.finish()
}

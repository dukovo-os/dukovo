// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! cloud-setup(8): a cloud machine's setup at its first boot, in the
//! style of cloud-init (ADR 0328).
//!
//! The service runs at each boot of a cloud image, and:
//!
//! 1. finds what the cloud says of the machine: a config disk labelled
//!    `cidata` (NoCloud's, as Proxmox, libvirt and QEMU users attach), or
//!    on DigitalOcean and AWS, by the firmware's maker, their metadata
//!    service at 169.254.169.254 ([`find`]);
//! 2. grows the root's partition to its disk's end and the root's Btrfs
//!    over it, with `rdisk --grow` and `device resize max` (ADR 0327),
//!    unless the user data says not to ([`grow`]);
//! 3. once for each instance the cloud names, sets the machine up as the
//!    meta-data and the user data say ([`set_up`]): its name, its network
//!    cards, the accounts with their SSH keys, files to write, and
//!    commands or a script to run. The instance's ID is kept in
//!    [`STATE`], so a restart of the same machine does none of this
//!    again, and a machine made from its disk does.
//!
//! What the user data holds that cloud-setup does not do is said in the
//! log, key by key ([`config::Config::left_out`]); cloud-config(5) lists
//! what it does.
//!
//! If a machine is not set up as its data says, start with the log's
//! `cloud-setup:` lines, then [`find`] for where the data came from.

pub mod config;
pub mod network;
#[cfg(test)]
mod tests;
pub mod yaml;

use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command as Program;
use std::time::{Duration, Instant};

use accounts::request::{Answer, Request};

use self::config::{Account, Command, Config, MetaData, UserData, WriteFile};
use crate::args::{Arg, Args};
use crate::util::{self, Fail};
use crate::{account, probe, proc};

/// Where cloud-setup keeps the ID of the instance it set up last.
pub const STATE: &str = "/system/state/cloud-setup";

/// The label of NoCloud's config disk, in any case.
const CONFIG_LABEL: &str = "cidata";

/// The metadata service of DigitalOcean and AWS.
const METADATA: &str = "169.254.169.254:80";

/// How long a config disk the kernel found may take to be mounted, and
/// the network to reach the metadata service.
const MOUNT_WAIT: Duration = Duration::from_secs(30);
const NETWORK_WAIT: Duration = Duration::from_secs(90);

/// Where the firmware says who made the machine (radix-sysfs(5)).
const DMI: &str = "/kernel/class/dmi/id";

/// The commands cloud-setup runs.
const RDISK: &str = "/system/commands/rdisk";
const DEVICE: &str = "/system/commands/device";
const SERVICE: &str = "/system/commands/service";
const SHELL: &str = "/bin/sh";

/// The network(5) file netd reads.
const NETWORK_FILE: &str = "/system/settings/network";

/// Writes a line of the log, `cloud-setup:` first.
macro_rules! say {
    ($($arg:tt)*) => {
        println!("cloud-setup: {}", format_args!($($arg)*))
    };
}

/// What the cloud says of the machine, and where it came from.
#[derive(Debug, Default)]
pub struct Found {
    /// Where it came from, for the log.
    pub origin: String,
    /// The meta-data.
    pub meta: MetaData,
    /// The user data, if there is any.
    pub user_data: Option<Vec<u8>>,
    /// NoCloud's `network-config`, if the disk has one.
    pub network: Option<String>,
}

/// The program: `cloud-setup [--source DIR] [--dry-run]`.
pub fn main(args: &mut Args) -> Result<i32, Fail> {
    let (mut source, mut dry_run) = (None, false);
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Long(l) if l == "source" => source = Some(PathBuf::from(args.value()?)),
            Arg::Short('n') => dry_run = true,
            Arg::Long(l) if l == "dry-run" => dry_run = true,
            Arg::Short('h') => return help(),
            Arg::Long(l) if l == "help" => return help(),
            other => return Err(args.unknown(&other)),
        }
    }
    let mut ok = true;
    let found = match source {
        Some(dir) => Some(read_nocloud(&dir)?),
        None => find(),
    };
    let user_data = found.as_ref().and_then(|f| f.user_data.as_deref()).map(config::user_data);
    let user_data = match user_data {
        None => UserData::Empty,
        Some(Ok(data)) => data,
        Some(Err(why)) => {
            say!("{why}; the user data is left out");
            ok = false;
            UserData::Empty
        }
    };
    let config = match &user_data {
        UserData::Config(config) => Some(config),
        _ => None,
    };
    if !grow(config, dry_run) {
        ok = false;
    }
    let Some(found) = found else {
        say!("no config disk labelled {CONFIG_LABEL} and no cloud's metadata: nothing to set up");
        return Ok(i32::from(!ok));
    };
    say!("instance {} from {}", found.meta.instance, found.origin);
    let done = fs::read_to_string(Path::new(STATE).join("instance")).ok();
    if done.as_deref().map(str::trim) == Some(found.meta.instance.as_str()) {
        say!("instance {} was set up at an earlier boot", found.meta.instance);
        return Ok(i32::from(!ok));
    }
    ok &= set_up(&found, &user_data, dry_run);
    if !dry_run {
        let kept = fs::DirBuilder::new().recursive(true).mode(0o755).create(STATE).and_then(|()| {
            fs::write(Path::new(STATE).join("instance"), format!("{}\n", found.meta.instance))
        });
        if let Err(err) = kept {
            say!("cannot keep the instance's ID in {STATE}: {}", util::strip_os_error(&err));
            ok = false;
        }
    }
    say!(
        "instance {} {}",
        found.meta.instance,
        if ok { "set up" } else { "set up, with the errors above" }
    );
    Ok(i32::from(!ok))
}

/// `cloud-setup --help`.
fn help() -> Result<i32, Fail> {
    print!(
        "Usage: cloud-setup [--source DIR] [--dry-run]

Sets a cloud machine up at its first boot from a config disk labelled
cidata, or from DigitalOcean's or AWS's metadata service: its name, its
network cards, accounts and their SSH keys, files and commands, in the
style of cloud-init (cloud-config(5)). At every boot it grows the root's
partition and Btrfs to the disk's end. The cloud-setup service runs it.

  --source DIR   Read meta-data, user-data and network-config from DIR
  -n, --dry-run  Say what would be done, and do nothing
"
    );
    Ok(0)
}

/// Finds what the cloud says: a config disk first, then the metadata
/// service of the cloud the firmware names.
fn find() -> Option<Found> {
    if let Some(dir) = config_disk() {
        match read_nocloud(&dir) {
            Ok(found) => return Some(found),
            Err(fail) => say!("{}", fail.message),
        }
    }
    let cloud = cloud()?;
    say!("on {}: asking its metadata service", cloud.name());
    if !wait_for_service() {
        say!("the metadata service at {METADATA} did not answer");
        return None;
    }
    let found = match cloud {
        Cloud::DigitalOcean => digitalocean(),
        Cloud::Aws => aws(),
    };
    found.map_err(|why| say!("{why}")).ok()
}

/// Where a config disk labelled `cidata` is mounted, waiting for Spark or
/// volumed(8) to mount one the kernel found.
fn config_disk() -> Option<PathBuf> {
    let mounted = || {
        proc::mounts().ok()?.into_iter().find_map(|mount| {
            let label = mount.label.as_deref()?;
            label.eq_ignore_ascii_case(CONFIG_LABEL).then(|| PathBuf::from(mount.path))
        })
    };
    if let Some(dir) = mounted() {
        return Some(dir);
    }
    let labelled = probe::devices().ok()?.into_iter().find(|(name, _)| {
        probe::reader(name)
            .and_then(|mut read| probe::filesystem(&mut read))
            .and_then(|fs| fs.label)
            .is_some_and(|label| label.eq_ignore_ascii_case(CONFIG_LABEL))
    });
    let (name, _) = labelled?;
    let start = Instant::now();
    while start.elapsed() < MOUNT_WAIT {
        if let Some(dir) = mounted() {
            return Some(dir);
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    say!("/devices/{name} is labelled {CONFIG_LABEL}, and nothing mounted it");
    None
}

/// Reads NoCloud's files in `dir`: `meta-data`, which it needs, and
/// `user-data` and `network-config`, if they are there.
fn read_nocloud(dir: &Path) -> Result<Found, Fail> {
    let read = |name: &str| match fs::read(dir.join(name)) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(util::failed(&dir.join(name).display(), err)),
    };
    let meta = read("meta-data")?
        .ok_or_else(|| util::fail(format!("{} has no meta-data file", dir.display())))?;
    let meta = MetaData::nocloud(&String::from_utf8_lossy(&meta)).map_err(util::fail)?;
    let network = read("network-config")?.map(|bytes| String::from_utf8_lossy(&bytes).into_owned());
    Ok(Found { origin: dir.display().to_string(), meta, user_data: read("user-data")?, network })
}

/// The clouds whose metadata services cloud-setup asks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Cloud {
    DigitalOcean,
    Aws,
}

impl Cloud {
    fn name(self) -> &'static str {
        match self {
            Cloud::DigitalOcean => "DigitalOcean",
            Cloud::Aws => "AWS",
        }
    }
}

/// The cloud the firmware's tables name, if one whose metadata service
/// cloud-setup asks: a machine elsewhere is not kept waiting for a
/// service that is not there.
fn cloud() -> Option<Cloud> {
    let read = |name: &str| fs::read_to_string(Path::new(DMI).join(name)).unwrap_or_default();
    cloud_of(&read("sys_vendor"), &read("bios_vendor"), &read("bios_version"))
}

/// The cloud of a machine whose maker, firmware's maker and firmware's
/// version are these: AWS's Nitro machines say `Amazon EC2`, its older
/// Xen ones have `amazon` in the firmware's version.
fn cloud_of(vendor: &str, bios_vendor: &str, bios_version: &str) -> Option<Cloud> {
    let (vendor, bios_vendor) = (vendor.trim(), bios_vendor.trim());
    if vendor == "DigitalOcean" {
        Some(Cloud::DigitalOcean)
    } else if vendor == "Amazon EC2"
        || bios_vendor == "Amazon EC2"
        || bios_version.to_ascii_lowercase().contains("amazon")
    {
        Some(Cloud::Aws)
    } else {
        None
    }
}

/// Waits for the metadata service to take a connection, as the network
/// cards may still be asking DHCP.
fn wait_for_service() -> bool {
    let address: SocketAddr = METADATA.parse().expect("the address is one");
    let start = Instant::now();
    let mut said = false;
    while start.elapsed() < NETWORK_WAIT {
        if TcpStream::connect_timeout(&address, Duration::from_secs(2)).is_ok() {
            return true;
        }
        if !said {
            say!("waiting for the network to reach {METADATA}");
            said = true;
        }
        std::thread::sleep(Duration::from_secs(1));
    }
    false
}

/// Asks the metadata service `method path` with `headers`: the answer's
/// status and body. HTTP/1.0, so that no answer comes in chunks.
fn http(method: &str, path: &str, headers: &[(&str, &str)]) -> io::Result<(u16, Vec<u8>)> {
    let address: SocketAddr = METADATA.parse().expect("the address is one");
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(5))?;
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    let mut request = format!("{method} {path} HTTP/1.0\r\nHost: 169.254.169.254\r\n");
    for (name, value) in headers {
        request.push_str(&format!("{name}: {value}\r\n"));
    }
    request.push_str("Content-Length: 0\r\n\r\n");
    stream.write_all(request.as_bytes())?;
    let mut answer = Vec::new();
    stream.read_to_end(&mut answer)?;
    parse_answer(&answer).ok_or_else(|| io::Error::other("not an HTTP answer"))
}

/// The status and body of an HTTP answer.
fn parse_answer(answer: &[u8]) -> Option<(u16, Vec<u8>)> {
    let end = answer.windows(4).position(|w| w == b"\r\n\r\n")?;
    let head = std::str::from_utf8(&answer[..end]).ok()?;
    let status = head.lines().next()?.split(' ').nth(1)?.parse().ok()?;
    let mut body = answer[end + 4..].to_vec();
    let length = head.lines().skip(1).find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.trim()
            .eq_ignore_ascii_case("content-length")
            .then(|| value.trim().parse::<usize>().ok())?
    });
    if let Some(length) = length {
        body.truncate(length);
    }
    Some((status, body))
}

/// A GET of `path` that must answer 200.
fn get(path: &str, headers: &[(&str, &str)]) -> Result<Vec<u8>, String> {
    match http("GET", path, headers) {
        Ok((200, body)) => Ok(body),
        Ok((status, _)) => Err(format!("the metadata service answered {status} for {path}")),
        Err(err) => Err(format!("asking the metadata service for {path}: {err}")),
    }
}

/// DigitalOcean's metadata, all in `/metadata/v1.json`.
fn digitalocean() -> Result<Found, String> {
    let (meta, user_data) = MetaData::digitalocean(&get("/metadata/v1.json", &[])?)?;
    Ok(Found { origin: "DigitalOcean's metadata service".into(), meta, user_data, network: None })
}

/// AWS's metadata, from its instance metadata service, version 2: a
/// token first, which each request then carries; version 1, with none,
/// if the service gives no token.
fn aws() -> Result<Found, String> {
    let token = match http(
        "PUT",
        "/latest/api/token",
        &[("X-aws-ec2-metadata-token-ttl-seconds", "300")],
    ) {
        Ok((200, token)) => String::from_utf8(token).ok().map(|t| t.trim().to_string()),
        _ => None,
    };
    let headers: Vec<(&str, &str)> =
        token.as_deref().map(|t| ("X-aws-ec2-metadata-token", t)).into_iter().collect();
    let text =
        |path: &str| get(path, &headers).map(|b| String::from_utf8_lossy(&b).trim().to_string());
    let instance = text("/latest/meta-data/instance-id")?;
    let hostname = text("/latest/meta-data/local-hostname").ok();
    // `public-keys/` lists `0=name` a line; each key is at N/openssh-key.
    let mut keys = Vec::new();
    for line in text("/latest/meta-data/public-keys/").unwrap_or_default().lines() {
        let Some((number, _)) = line.split_once('=') else { continue };
        if let Ok(key) = text(&format!("/latest/meta-data/public-keys/{number}/openssh-key")) {
            keys.extend(config::key_lines(&key));
        }
    }
    let user_data = match http("GET", "/latest/user-data", &headers) {
        Ok((200, body)) => Some(body),
        Ok((404, _)) => None,
        Ok((status, _)) => {
            return Err(format!("the metadata service answered {status} for the user data"));
        }
        Err(err) => return Err(format!("asking the metadata service for the user data: {err}")),
    };
    let meta = MetaData { instance, hostname, keys };
    Ok(Found { origin: "AWS's instance metadata service".into(), meta, user_data, network: None })
}

/// Runs `program` with `args`, its output into the log; whether it ended
/// well. With `dry_run`, says what it would run.
fn run(program: &str, args: &[&str], dry_run: bool) -> bool {
    let shown = std::iter::once(program).chain(args.iter().copied()).collect::<Vec<_>>().join(" ");
    if dry_run {
        say!("would run {shown}");
        return true;
    }
    match Program::new(program).args(args).status() {
        Ok(status) if status.success() => true,
        Ok(status) => {
            say!("{shown} ended with {status}");
            false
        }
        Err(err) => {
            say!("cannot run {shown}: {}", util::strip_os_error(&err));
            false
        }
    }
}

/// Grows the root's partition to its disk's end, and the root's Btrfs
/// over it, as the user data allows: both on its own disk only, as an
/// installed system's root is, not the live system's. Whether all went
/// well.
fn grow(config: Option<&Config>, dry_run: bool) -> bool {
    let (partition, filesystem) =
        config.map_or((true, true), |c| (c.grow_partition, c.grow_filesystem));
    if !partition && !filesystem {
        say!("the user data says not to grow the root");
        return true;
    }
    let source = proc::mounts()
        .ok()
        .and_then(|mounts| mounts.into_iter().find(|m| m.path == "/system").map(|m| m.source));
    let Some(source) = source.filter(|s| probe::block_text(s, "partition").is_some()) else {
        say!("the system does not run from a disk's partition: nothing to grow");
        return true;
    };
    let number = probe::block_text(&source, "partition").unwrap_or_default();
    let disk = probe::disk_of(&source).to_string();
    let mut ok = true;
    if partition {
        ok &= run(RDISK, &["--grow", &disk, &number], dry_run);
    }
    if filesystem && ok {
        ok &= run(DEVICE, &["resize", "max", "/system/settings"], dry_run);
    }
    ok
}

/// Sets the machine up as `found` and its user data say, once for the
/// instance. Whether all went well.
fn set_up(found: &Found, user_data: &UserData, dry_run: bool) -> bool {
    let mut ok = true;
    let empty = Config { users: vec![config::UserEntry::Default], ..Config::default() };
    let config = match user_data {
        UserData::Config(config) => {
            for note in &config.left_out {
                say!("left out {note}");
            }
            config
        }
        UserData::Other(what) => {
            say!("the user data is {what}, which cloud-setup does not read; left out");
            &empty
        }
        UserData::Empty | UserData::Script(_) => &empty,
    };
    if let Some(name) = config.hostname(&found.meta) {
        ok &= set_hostname(&name, dry_run);
    }
    if let Some(text) = &found.network {
        ok &= set_network(text, dry_run);
    }
    for account in config.accounts(&found.meta) {
        ok &= set_account(&account, dry_run);
    }
    for file in &config.files {
        ok &= write_file(file, dry_run);
    }
    for command in &config.commands {
        ok &= match command {
            Command::Shell(line) => run(SHELL, &["-c", line], dry_run),
            Command::Words(words) => {
                let args: Vec<&str> = words[1..].iter().map(String::as_str).collect();
                run(&words[0], &args, dry_run)
            }
        };
    }
    if let UserData::Script(script) = user_data {
        ok &= run_script(script, dry_run);
    }
    ok
}

/// Gives the machine the name `name` (hostname(5)).
fn set_hostname(name: &str, dry_run: bool) -> bool {
    say!("naming the machine {name}");
    if dry_run {
        return true;
    }
    // SAFETY: the pointer and length are the name's own bytes.
    let set = unsafe { libc::sethostname(name.as_ptr().cast(), name.len() as _) };
    if set != 0 {
        say!(
            "cannot name the machine {name}: {}",
            util::strip_os_error(&io::Error::last_os_error())
        );
    }
    set == 0
}

/// Writes the network(5) file for the configuration `text`, and has netd
/// read it.
fn set_network(text: &str, dry_run: bool) -> bool {
    let read = yaml::parse(text).map_err(|err| format!("network-config: {err}"));
    let made = read.and_then(|value| network::network_file(&value));
    let (file, left_out) = match made {
        Ok(made) => made,
        Err(why) => {
            say!("{why}; the network is left as DHCP sets it");
            return false;
        }
    };
    for note in left_out {
        say!("left out {note}");
    }
    let Some(file) = file else {
        say!("the network configuration has every card ask DHCP, as netd does");
        return true;
    };
    say!("setting the network cards up as the network configuration says");
    if dry_run {
        print!("{file}");
        return true;
    }
    if let Err(err) = fs::write(NETWORK_FILE, &file) {
        say!("cannot write {NETWORK_FILE}: {}", util::strip_os_error(&err));
        return false;
    }
    run(SERVICE, &["netd", "reload"], false)
}

/// Makes `account` if there is none of its name, through authd(8), and
/// gives it its keys.
fn set_account(account: &Account, dry_run: bool) -> bool {
    let present = user(&account.name).is_some();
    let keys = account.keys.len();
    let what = if account.admin { "an administrator's account" } else { "an account" };
    match (present, dry_run) {
        (true, _) => say!("{} is there: giving it {keys} SSH key(s)", account.name),
        (false, _) => say!("making {what}, {}, with {keys} SSH key(s)", account.name),
    }
    if dry_run {
        return true;
    }
    if !present {
        let Some(request) = add_request(account) else { return false };
        if !ask(&request) {
            return false;
        }
    }
    if account.without_password
        && !ask(&Request::Set {
            name: account.name.clone(),
            full_name: None,
            admin: None,
            without_password: Some(true),
        })
    {
        return false;
    }
    match user(&account.name) {
        Some(user) => add_keys(&user, &account.keys),
        None => {
            say!("authd made no account {}", account.name);
            false
        }
    }
}

/// The request that makes `account`: with its password, or with one no
/// one knows, which keeps the account to its SSH keys, as cloud-init's
/// `lock_passwd` does. None if there is no randomness for that one: the
/// account is not made, rather than made with a password one could guess.
fn add_request(account: &Account) -> Option<Request> {
    let password = match &account.password {
        Some(password) => password.as_bytes().to_vec(),
        None => {
            let mut random = [0u8; 32];
            let read =
                fs::File::open("/devices/urandom").and_then(|mut f| f.read_exact(&mut random));
            if let Err(err) = read {
                say!("cannot read /devices/urandom for {}'s password: {err}", account.name);
                return None;
            }
            use base64::Engine as _;
            base64::engine::general_purpose::STANDARD.encode(random).into_bytes()
        }
    };
    Some(Request::Add {
        name: account.name.clone(),
        full_name: account.full_name.clone(),
        admin: account.admin,
        password,
    })
}

/// Asks authd `request`; whether it was done.
fn ask(request: &Request) -> bool {
    match account::ask(request) {
        Ok(Answer::Done(_)) => true,
        Ok(Answer::Refused(why)) => {
            say!("authd refused: {why}");
            false
        }
        Err(fail) => {
            say!("{}", fail.message);
            false
        }
    }
}

/// The account `name`, from the passwd file.
fn user(name: &str) -> Option<accounts::User> {
    let text = fs::read_to_string(accounts::PASSWD).ok()?;
    accounts::users(&text).entries().find(|u| u.name == name).cloned()
}

/// Adds `keys` to the end of `user`'s `~/.ssh/authorized_keys`, those it
/// does not have yet, the folder and file its own and closed to others.
fn add_keys(user: &accounts::User, keys: &[String]) -> bool {
    if keys.is_empty() {
        return true;
    }
    let folder = Path::new(&user.home).join(".ssh");
    let file = folder.join("authorized_keys");
    let added = (|| -> io::Result<()> {
        fs::DirBuilder::new().mode(0o700).recursive(true).create(&folder)?;
        std::os::unix::fs::chown(&folder, Some(user.uid), Some(user.gid))?;
        let have = fs::read_to_string(&file).unwrap_or_default();
        let mut out = OpenOptions::new().create(true).append(true).mode(0o600).open(&file)?;
        if !have.is_empty() && !have.ends_with('\n') {
            out.write_all(b"\n")?;
        }
        for key in keys.iter().filter(|k| !have.lines().any(|line| line.trim() == k.as_str())) {
            writeln!(out, "{key}")?;
        }
        std::os::unix::fs::chown(&file, Some(user.uid), Some(user.gid))?;
        fs::set_permissions(&file, fs::Permissions::from_mode(0o600))
    })();
    if let Err(err) = added {
        say!("cannot write {}: {}", file.display(), util::strip_os_error(&err));
        return false;
    }
    true
}

/// Writes `file` of `write_files`, with its mode and owner.
fn write_file(file: &WriteFile, dry_run: bool) -> bool {
    let verb = if file.append { "adding to" } else { "writing" };
    say!("{verb} {} ({} bytes, mode {:o})", file.path, file.content.len(), file.mode);
    if dry_run {
        return true;
    }
    let owner = match file.owner.as_deref().map(owner_ids) {
        None => (0, 0),
        Some(Ok(ids)) => ids,
        Some(Err(why)) => {
            say!("{}: {why}", file.path);
            return false;
        }
    };
    let path = Path::new(&file.path);
    let written = (|| -> io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::DirBuilder::new().recursive(true).mode(0o755).create(parent)?;
        }
        let mut out = OpenOptions::new()
            .create(true)
            .write(true)
            .append(file.append)
            .truncate(!file.append)
            .mode(file.mode)
            .open(path)?;
        out.write_all(&file.content)?;
        std::os::unix::fs::chown(path, Some(owner.0), Some(owner.1))?;
        fs::set_permissions(path, fs::Permissions::from_mode(file.mode))
    })();
    if let Err(err) = written {
        say!("cannot write {}: {}", file.path, util::strip_os_error(&err));
        return false;
    }
    true
}

/// The user and group numbers of `owner`, `user` or `user:group`.
fn owner_ids(owner: &str) -> Result<(u32, u32), String> {
    let (name, group) = owner.split_once(':').map_or((owner, None), |(u, g)| (u, Some(g)));
    let user = if name == "root" {
        (0, 0)
    } else {
        let user = user(name).ok_or_else(|| format!("there is no account {name}"))?;
        (user.uid, user.gid)
    };
    let gid = match group {
        None => user.1,
        Some("root") => 0,
        Some(group) => {
            let text = fs::read_to_string(accounts::GROUP).unwrap_or_default();
            accounts::groups(&text)
                .entries()
                .find(|g| g.name == group)
                .map(|g| g.gid)
                .ok_or_else(|| format!("there is no group {group}"))?
        }
    };
    Ok((user.0, gid))
}

/// Runs user data that is a script, from [`STATE`], as cloud-init runs
/// one once for each instance.
fn run_script(script: &[u8], dry_run: bool) -> bool {
    let path = Path::new(STATE).join("user-data-script");
    say!("running the user data's script");
    if dry_run {
        return true;
    }
    let written = fs::DirBuilder::new()
        .recursive(true)
        .mode(0o755)
        .create(STATE)
        .and_then(|()| fs::write(&path, script))
        .and_then(|()| fs::set_permissions(&path, fs::Permissions::from_mode(0o700)));
    if let Err(err) = written {
        say!("cannot write {}: {}", path.display(), util::strip_os_error(&err));
        return false;
    }
    run(&path.to_string_lossy(), &[], false)
}

// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! dukovo-rescue(8): Rescue's screens (ADRs 0459 and 0460). Rescue is the
//! live system of a release, kept on the EFI system partition, which the
//! boot menu starts with `live rescue=<uuid>` after three starts in a row
//! that were not seen through, or when no slot can be read. Every console
//! runs this program in place of a sign-in, again whenever it ends.
//!
//! It mounts the Dukovo root the boot menu named, asks for an
//! administrator's name and password when the root's settings can be
//! read, and offers the repairs: reinstalling the system from Rescue's
//! own copy of the release, from the update server or from an update
//! folder, keeping the settings, apps and files; putting back the settings from before the
//! last update; checking the root's disk; copying the users' and shared
//! files to another disk; checking the hardware and the network, which
//! run `hardware check` and `netcheck` (ADRs 0488 and 0489); turning
//! networking on; a shell; naming the likely cause of what went wrong
//! (troubleshoot(8)); and starting the system normally. It also says
//! whether the change that broke the last start can be found, and how:
//! `changes bisect` runs in the installed system, not here (ADR 0475).
//! Nothing is formatted, and `@users`, `@shared` and `@applications` are
//! never written.
//!
//! When the installed system had SSH on, Rescue turns it on too, before
//! it asks at the screen for an administrator (ADR 0540): with the root's
//! network, firewall and blocking settings, so that the machine is where
//! its owner reached it before, and with authd(8) and sshd(8) signing in
//! the root's administrators alone, by its accounts, keys and host key.
//! authd starts this menu for each of them, as root, with `--ssh NAME`.
//! The choices that change something take a lock, so that one repair
//! runs at a time, and a second person is told who holds it, and from
//! where; the choices that only read run side by side.
//!
//! Each repair says what it did, or why it could not, in one line;
//! `/system/state/log` keeps Rescue's log until it restarts.

use std::fs::{self, File, OpenOptions};
use std::io::{Read as _, Write as _};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::Command;

use accounts::rescue::{ROOT, SSHD_COPIES, SSHD_FILES};
use accounts::{Group, Password, Table, User};
use manifest::service as service_file;
use slots::{Slot, State};
use system_release::list::Release;
use system_release::store::Store;

use crate::args::{Arg, Args};
use crate::changes;
use crate::ip;
use crate::prompt::{self, Answer};
use crate::sysupdate::bisect;
use crate::sysupdate::online::{self, Plan};
use crate::sysupdate::place::{self, BUILDING, Root};
use crate::sysupdate::stage::{self, Files, TRUSTED_ROOT};
use crate::sysupdate::starts::{self, Outcome};
use crate::util::{self, Fail, fail};
use crate::{authd, firewall, netd, power, probe, resolved, service, subvolume};

/// Where Rescue mounts the EFI system partition it reads its own copy
/// from.
const ESP: &str = "/tmp/rescue/esp";

/// Where Rescue mounts the disk files are copied off to.
const COPY_TO: &str = "/tmp/rescue/copy-to";

/// The subvolumes of the root whose files are copied off: the users' and
/// the shared folders.
const COPIED: [&str; 2] = ["@users", "@shared"];

/// The services that make up networking, in the order they start.
const NETWORKING: [&str; 4] = ["e1000d", "netd", "resolved", "timed"];

/// What the owner types to reinstall from the update server.
const FROM_SERVER: &str = "server";

/// How many times a wrong name or password may be typed before Rescue
/// waits and asks again.
const TRIES: u32 = 3;

/// How many seconds Rescue waits for a card to have an address before
/// it checks the network.
const ADDRESS_WAIT: u32 = 20;

/// The settings an administrator's name and password are checked
/// against, in the root's subvolumes.
const PASSWD: &str = "@settings/passwd";
const GROUP: &str = "@settings/group";
const PASSWORDS: &str = "@state/authd/passwords";

/// Rescue's folder in memory, root's alone: the link to the root that
/// authd reads ([`ROOT`]) and the lock a repair holds.
const LIVE: &str = "/system/live/rescue";

/// The lock a repair holds while it runs: the line [`holder_line`] writes
/// says who holds it, and from where.
const LOCK: &str = "/system/live/rescue/lock";

/// The root's settings Rescue takes in place of its own when it turns SSH
/// on, so that the machine is where its owner reached it before: its
/// cards' addresses, its DNS, its firewall's rules, and the addresses it
/// never blocks. Rescue's own stay where the root has none.
const NETWORK_SETTINGS: [&str; 4] =
    [netd::SETTINGS, resolved::SETTINGS, firewall::SETTINGS, authd::blocking::SETTINGS];

/// sshd's service file, among the root's settings and a slot's defaults.
const SSHD_SERVICE: &str = "spark/services/sshd.toml";

/// Who uses this screen of Rescue's, and from where.
struct Person {
    /// The administrator's name, once known: none when Rescue asked for
    /// no password.
    name: Option<String>,
    /// Where they are: `None` at the console, or the address they came
    /// over SSH from.
    over_ssh: Option<String>,
}

impl Person {
    /// Where they are, as the lock's line says it.
    fn place(&self) -> String {
        match &self.over_ssh {
            Some(from) => format!("over SSH from {from}"),
            None => "at the console".into(),
        }
    }
}

/// How the sign-in at the screen went.
enum SignIn {
    /// An administrator of the root's signed in.
    Admin(String),
    /// Rescue asked for no password: the root's accounts do not read, or
    /// name no administrator.
    NotAsked,
    /// No administrator's name and password were given.
    Refused,
}

/// The root's accounts.
struct Accounts {
    users: Table<User>,
    groups: Table<Group>,
    hashes: Table<Password>,
}

/// What Rescue knows of the root it rescues.
struct Rescued {
    /// The root's Btrfs UUID, as the boot menu named it.
    uuid: String,
    /// The root, mounted whole at `/volumes/<disk>`, or why it is not.
    root: Result<Root, String>,
}

/// dukovo-rescue(8).
pub fn main(args: &mut Args) -> Result<i32, Fail> {
    let mut ssh_name = None;
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Long(ref name) if name == "ssh" => {
                ssh_name = Some(args.value()?.to_string_lossy().into_owned());
            }
            other => return Err(args.unknown(&other)),
        }
    }
    let Some(uuid) = util::kernel_word("rescue=") else {
        return Err(fail(
            "this is not Rescue: the boot menu's Rescue entry starts it, with rescue= on the \
             kernel's command line"
                .into(),
        ));
    };
    // authd starts the menu, as root, for an administrator it signed in
    // over SSH; no one else may skip the sign-in.
    if ssh_name.is_some() && util::own_user() != 0 {
        return Err(fail(
            "--ssh is authd's, which starts Rescue's menu as root for an administrator it signed \
             in over SSH"
                .into(),
        ));
    }
    let over_ssh = ssh_name
        .is_some()
        .then(|| ssh_client(&std::env::var("SSH_CONNECTION").unwrap_or_default()).to_string());
    let mut me = Person { name: ssh_name, over_ssh };
    println!("\x1b[H\x1b[2J\x1b[?25hDukovo Rescue {}", env!("CARGO_PKG_VERSION"));
    println!("Rescue repairs this computer's Dukovo. Files, apps and settings stay as they are");
    println!("unless you choose otherwise.");
    println!();
    let rescued = Rescued { root: mount_root(&uuid), uuid };
    match &rescued.root {
        Ok(root) => println!("The Dukovo root is {}, on {}.", rescued.uuid, root.disk),
        Err(why) => println!("The Dukovo root {} cannot be used: {why}.", rescued.uuid),
    }
    match (&rescued.root, &me.name) {
        (_, Some(name)) => println!("{name} signed in {}.", me.place()),
        (Ok(root), None) => {
            println!("{}", ssh_line(root));
            match signed_in(root) {
                SignIn::Admin(name) => me.name = Some(name),
                SignIn::NotAsked => {}
                SignIn::Refused => {
                    // consoled starts Rescue again, which asks again.
                    std::thread::sleep(std::time::Duration::from_secs(10));
                    return Ok(1);
                }
            }
        }
        (Err(_), None) => println!("Networking is off until you turn it on."),
    }
    if let Ok(root) = &rescued.root {
        println!();
        for line in why_it_failed(root) {
            println!("{line}");
        }
    }
    loop {
        println!();
        let update = rescued.root.as_ref().ok().and_then(settings_to_put_back);
        let can =
            |ok: bool, text: &str| if ok { text.to_string() } else { format!("{text} (not here)") };
        let mounted = rescued.root.is_ok();
        let choices = [
            can(mounted, "Why the last start failed, and what changed"),
            "Find the likely cause".to_string(),
            can(mounted, "Find the change that broke the start"),
            can(mounted, "Reinstall Dukovo, keeping settings, apps and files"),
            can(update.is_some(), "Put back the settings from before the last update"),
            can(mounted, "Check the root's disk"),
            can(mounted, "Copy the users' and shared files to another disk"),
            "Check the hardware".to_string(),
            "Turn networking on".to_string(),
            "Check the network".to_string(),
            "Open a shell (type exit to come back)".to_string(),
            "Start Dukovo normally".to_string(),
            "Restart".to_string(),
            "Power off".to_string(),
        ];
        println!("What would you like to do?");
        for (number, text) in choices.iter().enumerate() {
            println!("{number:>4}  {text}");
        }
        let Some(choice) = ask(&format!("Choose 0 to {}: ", choices.len() - 1), true) else {
            return Ok(0);
        };
        // One repair at a time: its lock goes when it is done, or when this
        // program ends, however it ends.
        let held = match repair(choice.trim()).map(|doing| hold(&me, doing)).transpose() {
            Ok(held) => held,
            Err(why) => {
                println!("Rescue: {why}");
                continue;
            }
        };
        let done = match (choice.trim(), &rescued.root) {
            ("0", Ok(root)) => Ok(why_it_failed(root).join("\n")),
            ("1", _) => Ok(likely_cause(&rescued)),
            ("2", Ok(root)) => find_the_change(root),
            ("3", Ok(root)) => reinstall(root),
            ("4", Ok(root)) if update.is_some() => put_back_settings(root),
            ("5", Ok(root)) => check_disk(root),
            ("6", Ok(root)) => copy_off(root),
            ("7", _) => show("hardware", &["check"]),
            ("8", _) => networking_on(),
            ("9", _) => check_network(),
            ("10", _) => shell(),
            ("11", root) => start_normally(root.as_ref().ok()),
            ("12", _) => end("reboot"),
            ("13", _) => end("poweroff"),
            (other, _) => Err(format!("{other} is not one of the choices here")),
        };
        drop(held);
        match done {
            Ok(said) => println!("{said}"),
            Err(why) => println!("Rescue: {why}"),
        }
    }
}

/// Why the root's last start failed, from the record the health check
/// keeps of each start, and what changed since the last good start (ADR
/// 0501): how many starts in a row were not seen through, as the boot
/// menu counts them; the newest start that failed or never finished; or,
/// when the newest record is of a good start, that the starts the menu
/// counted after it stopped before the health check began them a record.
fn why_it_failed(root: &Root) -> Vec<String> {
    let records = starts::all_in(&root.path("@state/starts"));
    let Some(newest) = records.last() else {
        return vec!["No start of this Dukovo has been recorded: it is older than 0.15.0, or has never started.".into()];
    };
    let mut lines = Vec::new();
    let counted = place::partitions(root).map_or(0, |esps| place::rescue_starts(&esps));
    if counted > 0 && newest.outcome == Outcome::Good {
        let reached = root.path("@state/starts").join(starts::REACHED).exists();
        lines.push(format!(
            "The last {counted} starts were not seen through. They stopped before the health \
             check began, after start {}, the last recorded.",
            newest.number
        ));
        lines.push(stopped_where(reached).into());
    } else if counted > 0 {
        lines.push(format!("The last {counted} starts were not seen through."));
    }
    let Some(failed) = records.iter().rev().find(|start| start.outcome != Outcome::Good) else {
        lines.push(format!("Every start recorded was good, the last start {}.", newest.number));
        return lines;
    };
    lines.push(changes::start_line(failed));
    if failed.outcome == Outcome::Unfinished {
        lines.push(
            "Its services started, but its health check never finished: it stopped, hung or \
             lost power after that."
                .into(),
        );
    }
    lines.extend(changes::report(&records, None, failed));
    if let Some(id) = &failed.error {
        lines.push(format!("Help: {}", crate::troubleshoot::help_url(id)));
    }
    lines
}

/// What troubleshoot(8) says of the likely cause (ADR 0474): of the
/// root's last start that did not go well, from its records, or, when the
/// root cannot be used, of that, which Rescue knows no more of.
fn likely_cause(rescued: &Rescued) -> String {
    let (records, unusable, kept) = match &rescued.root {
        Ok(root) => {
            (starts::all_in(&root.path("@state/starts")), None, root.path("@state/troubleshoot"))
        }
        Err(why) => (Vec::new(), Some(why.as_str()), PathBuf::from("/system/state/troubleshoot")),
    };
    crate::troubleshoot::for_rescue(&records, unusable, &rescued.uuid, &kept).join("\n")
}

/// Whether the change that broke the last start can be looked for, by the
/// records the health check keeps (ADR 0475), and how. Rescue cannot run
/// `changes bisect` itself: its tries are starts of the installed system,
/// whose health check judges each against that system's own records of
/// starts and apps and keeps the search in its state, none of which
/// Rescue, running from memory, holds where the search reads it.
fn find_the_change(root: &Root) -> Result<String, String> {
    let records = starts::all_in(&root.path("@state/starts"));
    let (failed, good) = bisect::failed_and_good(&records).map_err(|why| why.to_string())?;
    Ok(format!(
        "Start {} failed its health check, and start {} before it was good. Rescue cannot try \
         the changes between them apart: each try is a start of the installed system, which \
         judges it itself. Start Dukovo normally, and once it is up, even with a service \
         failing, run `elevate changes bisect`: it restarts the machine by itself, a few times, \
         and names the change that broke the start. Nothing changes for good.",
        failed.number, good.number
    ))
}

/// Where starts that never began their health check stopped, by whether
/// one of them `reached` its services, as Spark marks it (ADR 0503).
fn stopped_where(reached: bool) -> &'static str {
    if reached {
        "At least one of them reached its services, but not its health check: a service, or \
         one an app or an administrator added, may be to blame. Troubleshooting on the start \
         menu offers a start with only the essentials."
    } else {
        "None of them reached its services: they stopped in the kernel or mounting the root. \
         Troubleshooting on the start menu offers a start with all of the kernel's messages."
    }
}

/// Asks `question`, the answer shown as typed if `shown`; `None` when
/// the console is gone or the question was stopped with Ctrl+C.
fn ask(question: &str, shown: bool) -> Option<String> {
    match prompt::ask(question, shown) {
        Ok(Answer::Line(line)) => Some(String::from_utf8_lossy(&line).into_owned()),
        _ => None,
    }
}

/// The UUID of the Btrfs on `/devices/<name>`, if it holds one.
fn btrfs_uuid(name: &str) -> Option<String> {
    let mut read = probe::reader(name)?;
    let found = probe::filesystem(&mut read)?;
    if found.kind == "btrfs" { found.uuid } else { None }
}

/// Finds the root of Btrfs UUID `uuid` and mounts it whole, compressed,
/// at `/volumes/<disk>`, unless it is there already, as it is when Rescue
/// starts again on a console. Rescue holds the whole tree and no Spark
/// has recorded the root, so it reads the disks for it.
fn mount_root(uuid: &str) -> Result<Root, String> {
    let devices = probe::devices().map_err(|err| format!("the disks cannot be listed: {err}"))?;
    let partitions: Vec<String> = devices
        .into_iter()
        .map(|(name, _)| name)
        .filter(|name| btrfs_uuid(name).as_deref() == Some(uuid))
        .collect();
    let disk = partitions.first().ok_or("no disk here holds it")?.clone();
    let top = Path::new("/volumes").join(&disk);
    if !top.join("@state").is_dir() {
        std::fs::create_dir_all(&top).map_err(|err| format!("{}: {err}", top.display()))?;
        let device = format!("/devices/{disk}");
        run("mount", &["-t", "btrfs", "-o", "compress", &device, &top.display().to_string()])
            .map_err(|why| format!("it will not mount: {why}"))?;
    }
    // The slot the state names as the default is the one a reinstall
    // writes; slot A when the state cannot be read.
    let mut root = Root { disk, uuid: uuid.to_string(), partitions, running: Slot::A };
    if let Ok((_, state)) = place::state(&root) {
        root.running = state.default;
    }
    Ok(root)
}

/// The root's accounts, if they read.
fn accounts_of(root: &Root) -> Option<Accounts> {
    let read = |name: &str| fs::read_to_string(root.path(name)).ok();
    Some(Accounts {
        users: accounts::users(&read(PASSWD)?),
        groups: accounts::groups(&read(GROUP)?),
        hashes: accounts::passwords(&read(PASSWORDS)?),
    })
}

/// The names of the root's administrators, as authd's Rescue mode takes
/// them: those in `admins`, root aside.
fn admins(accounts: &Accounts) -> Vec<String> {
    accounts
        .users
        .entries()
        .filter(|user| user.uid != 0 && accounts::is_admin(user, &accounts.groups))
        .map(|user| user.name.clone())
        .collect()
}

/// Asks for an administrator's name and password, checked against the
/// root's accounts, when its settings can be read.
fn signed_in(root: &Root) -> SignIn {
    let Some(accounts) = accounts_of(root) else {
        println!("The root's accounts cannot be read, so Rescue asks for no password.");
        return SignIn::NotAsked;
    };
    let admins = admins(&accounts);
    if admins.is_empty() {
        println!("This system has no administrator, so Rescue asks for no password.");
        return SignIn::NotAsked;
    }
    println!("Rescue needs one of this computer's administrators.");
    for _ in 0..TRIES {
        let Some(name) = ask("Name: ", true) else { return SignIn::Refused };
        let Some(password) = ask("Password: ", false) else { return SignIn::Refused };
        let mut password = password.into_bytes();
        let name = name.trim();
        let right = admins.iter().any(|admin| admin == name)
            && accounts.hashes.entries().find(|entry| entry.name == name).is_some_and(|entry| {
                !entry.is_disabled()
                    && if entry.hash.is_empty() {
                        password.is_empty()
                    } else {
                        accounts::hash::verify(&password, entry.own_hash()).unwrap_or(false)
                    }
            });
        accounts::wipe(&mut password);
        if right {
            println!("Rescue: {name} is an administrator of this computer.");
            return SignIn::Admin(name.to_string());
        }
        println!("That is not an administrator's name and password.");
    }
    SignIn::Refused
}

/// The one line Rescue says of SSH, having turned it on if the installed
/// system had it on (ADR 0540).
fn ssh_line(root: &Root) -> String {
    if let Some(why) = ssh_refused(root) {
        return format!("Networking is off until you turn it on, and SSH stays off: {why}.");
    }
    match start_ssh(root) {
        Ok(()) => "SSH is on, as it was on this computer: its administrators can reach Rescue \
                   over it."
            .into(),
        // Networking may be on by then, so the line says only what failed.
        Err(why) => format!("SSH did not come up, though this computer had it on: {why}."),
    }
}

/// Why SSH stays off in Rescue, if it does: unless the installed system
/// had sshd on, its accounts read and name an administrator, and its host
/// key is there, as sshd made it.
fn ssh_refused(root: &Root) -> Option<String> {
    match sshd_enabled(root) {
        Ok(true) => {}
        Ok(false) => return Some("this computer had it off".into()),
        Err(why) => return Some(why),
    }
    match accounts_of(root) {
        None => return Some("the root's accounts cannot be read".into()),
        Some(accounts) if admins(&accounts).is_empty() => {
            return Some("this computer has no administrator".into());
        }
        Some(_) => {}
    }
    let key = root.path("@state/sshd").join("ssh_host_ed25519_key");
    match File::open(&key) {
        Ok(_) => None,
        Err(err) => Some(format!("the root's host key cannot be read: {err}")),
    }
}

/// Whether the installed system starts sshd: its service file among the
/// root's settings, or the default slot's own when the settings have
/// none, with the settings' override set over it.
fn sshd_enabled(root: &Root) -> Result<bool, String> {
    let settings = root.path("@settings").join(SSHD_SERVICE);
    let defaults = root.slot(root.running).join("resources/default-settings").join(SSHD_SERVICE);
    let file = match read_if_there(&settings)? {
        Some(text) => text,
        None => read_if_there(&defaults)?.ok_or("this computer has no sshd")?,
    };
    let over_path = settings.with_file_name(service_file::OVERRIDES).join("sshd.toml");
    enabled_in(&file, read_if_there(&over_path)?.as_deref())
}

/// Whether the service file `file`, with the override `over` set over it,
/// starts its service at boot.
fn enabled_in(file: &str, over: Option<&str>) -> Result<bool, String> {
    let unread =
        |err: manifest::Error| format!("the root's sshd service file does not read: {err}");
    let over = over.map(service_file::Override::from_toml).transpose().map_err(unread)?;
    service_file::from_toml_over(file, over.as_ref()).map(|service| service.enabled).map_err(unread)
}

/// The text of the file at `path`; `None` when there is none.
fn read_if_there(path: &Path) -> Result<Option<String>, String> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(format!("{}: {err}", path.display())),
    }
}

/// Turns SSH on in Rescue's mode: links the root for authd, copies sshd
/// what it reads of the root, takes the root's network settings, turns
/// networking on, and starts authd and sshd. Nothing, when it is on
/// already, as it is when Rescue starts again on a console, or on another
/// console first.
fn start_ssh(root: &Root) -> Result<(), String> {
    if sshd_runs() {
        return Ok(());
    }
    make_live()?;
    link_root(root)?;
    copy_for_sshd(root)?;
    take_root_network(root)?;
    networking_on()?;
    start_sign_ins()
}

/// Takes the root's network settings in place of Rescue's own
/// ([`NETWORK_SETTINGS`]), so that the machine is on the network as the
/// installed system was, and loads the root's firewall rules.
fn take_root_network(root: &Root) -> Result<(), String> {
    for path in NETWORK_SETTINGS {
        let Some(from) = accounts::rescue::place(&root.path(""), path) else { continue };
        if from.is_file() {
            replace_with_copy(&from, Path::new(path), None)?;
        }
    }
    // Spark loads them through the firewall's own service, which holds the
    // right to change the rules; Rescue's screens do not hold it.
    let loaded = Command::new("/system/commands/service")
        .args(["firewall", "start"])
        .output()
        .map_err(|err| format!("service: {err}"))?;
    if !loaded.status.success() {
        return Err(format!(
            "the root's firewall rules did not load: {}",
            String::from_utf8_lossy(&loaded.stderr).trim()
        ));
    }
    Ok(())
}

/// Starts authd and sshd, for [`start_ssh`].
fn start_sign_ins() -> Result<(), String> {
    for name in ["authd", "sshd"] {
        let started = Command::new("/system/commands/service")
            .args([name, "start"])
            .output()
            .map_err(|err| format!("service: {err}"))?;
        if !started.status.success() {
            return Err(format!(
                "{name} did not start: {}",
                String::from_utf8_lossy(&started.stderr).trim()
            ));
        }
    }
    Ok(())
}

/// Whether Spark runs sshd, or is starting it.
fn sshd_runs() -> bool {
    runs("sshd")
}

/// Whether Spark runs service `name`, or is starting it.
fn runs(name: &str) -> bool {
    use spark_api::show::State;
    service::shown(Some(name)).is_ok_and(|show| {
        show.services.iter().any(|service| {
            service.name == name
                && matches!(service.state, State::Running | State::Starting | State::Pending)
        })
    })
}

/// Makes [`LIVE`], root's alone, unless it is there: and refuses one that
/// is not root's own folder, closed to others.
fn make_live() -> Result<(), String> {
    match fs::DirBuilder::new().mode(0o700).create(LIVE) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(err) => return Err(format!("{LIVE}: {err}")),
    }
    let made = fs::symlink_metadata(LIVE).map_err(|err| format!("{LIVE}: {err}"))?;
    if !made.is_dir() || made.uid() != 0 || made.mode() & 0o077 != 0 {
        return Err(format!("{LIVE} is not root's own folder"));
    }
    Ok(())
}

/// Links the root at [`ROOT`], for authd: in place of a link there, at
/// once, so that authd reads one root or the other, never none.
fn link_root(root: &Root) -> Result<(), String> {
    let new = format!("{LIVE}/.root.{}", std::process::id());
    let _ = fs::remove_file(&new);
    symlink(root.path(""), &new)
        .and_then(|()| fs::rename(&new, ROOT))
        .map_err(|err| format!("{ROOT}: {err}"))
}

/// Copies sshd the installed system's files it reads ([`SSHD_FILES`]),
/// which it cannot reach in the root, into its own folder, root's alone;
/// a file the root lacks has no copy, as sshd_config(5) when the system
/// keeps the defaults.
fn copy_for_sshd(root: &Root) -> Result<(), String> {
    for path in SSHD_FILES {
        let (Some(from), Some(to)) = (
            accounts::rescue::place(&root.path(""), path),
            accounts::rescue::place(Path::new(SSHD_COPIES), path),
        ) else {
            return Err(format!("{path} is in none of the root's subvolumes"));
        };
        if let Some(folder) = to.parent() {
            fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(folder)
                .map_err(|err| format!("{}: {err}", folder.display()))?;
        }
        if from.is_file() {
            replace_with_copy(&from, &to, Some(0o600))?;
        } else {
            let _ = fs::remove_file(&to);
        }
    }
    Ok(())
}

/// Puts a copy of the file at `from` at `to`, in place of what is there,
/// at once, with the mode `mode`, or `from`'s.
fn replace_with_copy(from: &Path, to: &Path, mode: Option<u32>) -> Result<(), String> {
    let new = to.with_file_name(format!(
        ".{}.{}",
        to.file_name().map_or_default(|name| name.to_string_lossy().into_owned()),
        std::process::id()
    ));
    let _ = fs::remove_file(&new);
    let copied = fs::copy(from, &new)
        .and_then(|_| match mode {
            Some(mode) => fs::set_permissions(&new, fs::Permissions::from_mode(mode)),
            None => Ok(()),
        })
        .and_then(|()| fs::rename(&new, to));
    copied.map_err(|err| {
        let _ = fs::remove_file(&new);
        format!("{} cannot be copied to {}: {err}", from.display(), to.display())
    })
}

/// The address an SSH client came from, the first of `SSH_CONNECTION`'s
/// words: `client-address client-port server-address server-port`.
fn ssh_client(connection: &str) -> &str {
    connection
        .split_whitespace()
        .next()
        .filter(|address| {
            !address.is_empty()
                && address.bytes().all(|b| b.is_ascii_hexdigit() || b".:".contains(&b))
        })
        .unwrap_or("an unknown address")
}

/// What choice `choice` does that changes something, as the lock's line
/// says it; `None` for a choice that only reads.
fn repair(choice: &str) -> Option<&'static str> {
    Some(match choice {
        "3" => "reinstalling Dukovo",
        "4" => "putting back the settings from before the last update",
        "5" => "checking the root's disk",
        "6" => "copying the users' and shared files off",
        "11" => "starting Dukovo normally",
        "12" => "restarting",
        "13" => "powering off",
        _ => return None,
    })
}

/// The lock a repair holds: the kernel lets it go when the file is
/// closed, as the repair ends or its program does, however it ends.
struct Held(File);

impl Drop for Held {
    fn drop(&mut self) {
        // No one holds it: the next to look reads no holder.
        let _ = self.0.set_len(0);
    }
}

/// Takes the lock for `me`, `doing` a repair; why not, when someone else
/// holds it.
fn hold(me: &Person, doing: &str) -> Result<Held, String> {
    make_live()?;
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(LOCK)
        .map_err(|err| format!("{LOCK}: {err}"))?;
    // SAFETY: flock takes a descriptor `file` owns; the lock goes when it
    // is closed.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        let mut line = String::new();
        let _ = file.read_to_string(&mut line);
        return Err(held_by(&line));
    }
    let line = holder_line(me, doing);
    file.set_len(0)
        .and_then(|()| file.write_all(line.as_bytes()))
        .map_err(|err| format!("{LOCK}: {err}"))?;
    Ok(Held(file))
}

/// The lock's line for `me`, `doing` a repair: name, what and where, a tab
/// between each.
fn holder_line(me: &Person, doing: &str) -> String {
    format!("{}\t{doing}\t{}\n", me.name.as_deref().unwrap_or("someone"), me.place())
}

/// What a second person is told of the lock's holder, by its `line`.
fn held_by(line: &str) -> String {
    let mut words = line.trim_end().splitn(3, '\t');
    match (words.next(), words.next(), words.next()) {
        (Some(name), Some(doing), Some(place)) if !name.is_empty() => {
            format!("{name} is {doing}, {place}: wait for it to finish, then choose again")
        }
        _ => "another repair is under way: wait for it to finish, then choose again".into(),
    }
}

/// What the last update was, when its settings from before it can still
/// be put back: they are kept, and no fallback put them back.
fn settings_to_put_back(root: &Root) -> Option<String> {
    let update = root.path(slots::restore::UPDATE);
    let before = update.join(slots::restore::BEFORE_SETTINGS);
    if !before.is_dir() || update.join(slots::restore::RESTORED).exists() {
        return None;
    }
    let about = std::fs::read_to_string(update.join(slots::restore::ABOUT)).unwrap_or_default();
    Some(
        about
            .lines()
            .find_map(|line| line.strip_prefix("to = "))
            .map_or_else(|| "the last update".into(), |to| format!("the update to {to}")),
    )
}

/// Reinstalls the system into the default slot from Rescue's own copy of
/// the release, from the update server, or from an update folder the
/// owner names, keeping the settings, the state and every user's files
/// and apps.
fn reinstall(root: &Root) -> Result<String, String> {
    // Each line fits 80 columns, so that a test finds the prompt whole.
    println!("Reinstall Dukovo from Rescue's own copy, from the update server, or from an");
    println!("update folder or file on a disk?");
    let from = ask(
        &format!("Press Enter for Rescue's copy, or type {FROM_SERVER}, or the folder or file: "),
        true,
    )
    .ok_or("nothing was reinstalled")?;
    let from = from.trim();
    let esps = place::partitions(root).map_err(|why| why.message)?;
    let release =
        if from == FROM_SERVER { from_server(root, &esps)? } else { from_disk(root, from, &esps)? };
    let state = State {
        generation: 0,
        default: root.running,
        previous: None,
        trying: None,
        tries: 0,
        boot: None,
    };
    place::write_both(root, state).map_err(|why| why.message)?;
    for why in place::reset_rescue(esps.iter().map(String::as_str)) {
        println!("Rescue: {why}");
    }
    Ok(format!(
        "Dukovo {} is back in slot {}, every file checked against its signed list; settings, \
         apps and files are as they were. Restart to start it.",
        release.version,
        root.running.letter().to_ascii_uppercase()
    ))
}

/// Asks before a reinstall of Dukovo `version` into the default slot,
/// saying where it comes from, `from`, when that is not Rescue's copy:
/// each on a line of its own, so that the question fits 80 columns.
fn confirm(root: &Root, version: &str, from: Option<&str>) -> Result<(), String> {
    let slot = root.running.letter().to_ascii_uppercase();
    if let Some(from) = from {
        println!("It comes from {from}.");
    }
    println!("Reinstall Dukovo {version} into slot {slot}?");
    println!("Settings, apps and every user's files stay as they are.");
    match ask("Type yes to reinstall: ", true) {
        Some(answer) if answer.trim() == "yes" => Ok(()),
        _ => Err("nothing was reinstalled".into()),
    }
}

/// Builds the default slot from Rescue's own copy of the release, when
/// `from` is empty, or from the update folder or file `from` names.
fn from_disk(root: &Root, from: &str, esps: &[String]) -> Result<Release, String> {
    let trusted = std::fs::read(TRUSTED_ROOT).map_err(|err| format!("{TRUSTED_ROOT}: {err}"))?;
    let mounted = if from.is_empty() { Some(mount_rescue(esps)?) } else { None };
    let mut archive = None;
    let opened = match &mounted {
        Some(esp) => system_release::rescue::open(&esp.join(system_release::rescue::DIR), &trusted),
        None if Path::new(from).is_file() => {
            system_release::updates::open_file(Path::new(from), &trusted, None).map(
                |(published, opened)| {
                    archive = Some(opened);
                    published
                },
            )
        }
        None => system_release::repo::open(Path::new(from), &trusted, None),
    };
    let result = opened.and_then(|published| {
        let release = published.release;
        confirm(root, &release.version, None)?;
        let rescue_dir = mounted.as_ref().map(|esp| esp.join(system_release::rescue::DIR));
        let files = match (&rescue_dir, archive.take()) {
            (Some(dir), _) => Files::Rescue { rescue: dir, system: Path::new("/system") },
            (None, Some(archive)) => Files::Archive(archive),
            (None, None) => Files::Store(Store::at(&Path::new(from).join("files"))),
        };
        build_slot(root, &release, &files, &|| Ok(())).map(|()| release)
    });
    if mounted.is_some() {
        let _ = run("umount", &[ESP]);
    }
    result
}

/// Builds the default slot from the update server the root's settings
/// name, with the newest release of the channel the root follows, or
/// the bridge the root's release needs first (ADR 0543). Networking is on
/// for the while, with the root's settings; trust starts from the
/// metadata the root keeps, or from the root Rescue's own release
/// trusts when it keeps none, and each newer root is signed by the one
/// before. Only the files the slot lacks are fetched, onto the root's
/// disk, and they go once the slot is built.
fn from_server(root: &Root, esps: &[String]) -> Result<Release, String> {
    let top = root.path("");
    let in_root = |path: &str| {
        accounts::rescue::place(&top, path)
            .ok_or_else(|| format!("{path} is in none of the root's subvolumes"))
    };
    let state = in_root(online::STATE)?;
    let settings = root_update_settings(&in_root(online::SETTINGS)?)?;
    let ran = root_version(root, esps)?;
    let _network = Networking::on(root)?;
    println!(
        "Rescue: reading the {} channel from {}, against what the root trusts",
        settings.channel, settings.server
    );
    let mut client = connect(settings, &state)?;
    let release = newest_for(&mut client, &ran)?;
    let server = client.settings.server.clone();
    let slot = root.slot(root.running);
    let downloads = online::downloads_in(&state);
    let needed = online::needed_in(slot.is_dir().then_some(slot.as_path()), &release, &downloads)
        .map_err(|why| why.message)?;
    println!(
        "Rescue: the slot lacks {} files of Dukovo {}, {}, which are fetched; it holds the rest",
        needed.fetch.len(),
        release.version,
        online::mib(needed.bytes)
    );
    confirm(root, &release.version, Some(&server))?;
    let fetched = online::fetch_into(&client, &needed, &downloads).map_err(|why| why.message)?;
    println!("Rescue: fetched {} files, {}", fetched.fetched, online::mib(fetched.bytes));
    let afresh = || -> Result<(), String> {
        let all = online::needed_in(None, &release, &downloads).map_err(|why| why.message)?;
        println!("Rescue: fetching the {} files the slot no longer gives", all.fetch.len());
        online::fetch_into(&client, &all, &downloads).map(|_| ()).map_err(|why| why.message)
    };
    build_slot(root, &release, &Files::Store(Store::at(&downloads)), &afresh)?;
    let _ = fs::remove_dir_all(&downloads);
    Ok(release)
}

/// The root's update settings, in the file at `path`, or the defaults
/// when it has none.
fn root_update_settings(path: &Path) -> Result<online::Settings, String> {
    match fs::read_to_string(path) {
        Ok(text) => {
            online::Settings::parse(&text).map_err(|why| format!("{}: {why}", path.display()))
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(online::Settings::default()),
        Err(err) => Err(format!("{}: {err}", path.display())),
    }
}

/// The release the root's default slot held, as the slot says, or else
/// the one of Rescue's own copy on the EFI system partitions `esps`,
/// which follows each release that proves itself: the server's plan
/// starts from it.
fn root_version(root: &Root, esps: &[String]) -> Result<String, String> {
    if let Some(about) = root.about(root.running) {
        return Ok(about.version);
    }
    let trusted = fs::read(TRUSTED_ROOT).map_err(|err| format!("{TRUSTED_ROOT}: {err}"));
    let copy = trusted.and_then(|trusted| {
        let esp = mount_rescue(esps)?;
        let opened = system_release::rescue::open(&esp.join(system_release::rescue::DIR), &trusted);
        let _ = run("umount", &[ESP]);
        opened.map(|published| published.release.version)
    });
    copy.map_err(|why| {
        format!(
            "the slot does not say which release it held, and Rescue's own copy does not either \
             ({why}), so the update server cannot say which release reaches it: reinstall from \
             an update folder or file"
        )
    })
}

/// A client of the update server that reads trust from, and keeps what
/// it reads in, the root's sysupdate state `state`, as the installed
/// system does. Metadata kept there that does not read is set aside, and
/// trust starts again from the root Rescue's own release trusts.
fn connect(settings: online::Settings, state: &Path) -> Result<online::Client, String> {
    let mut client = match online::Client::open_in(settings.clone(), state) {
        Ok(client) => client,
        Err(why) => {
            let kept = state.join("metadata");
            let aside = state.join(format!("metadata.unread-{}", util::now_ns() / 1_000_000_000));
            fs::rename(&kept, &aside).map_err(|err| format!("{}: {err}", kept.display()))?;
            println!(
                "Rescue: {}; it is kept as {}, and trust starts again from the root Rescue's \
                 release trusts",
                why.message,
                aside.display()
            );
            online::Client::open_in(settings, state).map_err(|why| why.message)?
        }
    };
    client.refresh().map_err(|why| why.message)?;
    Ok(client)
}

/// The release to reinstall on a root that ran Dukovo `ran`: the
/// channel's, when `ran` updates to it directly or is it, else the
/// bridge it needs first. Waves and holds are for updates, and the owner
/// asked for this one; a channel that offers an older release than `ran`
/// is refused, as a stage refuses one.
fn newest_for(client: &mut online::Client, ran: &str) -> Result<Release, String> {
    let channel = client.settings.channel.clone();
    let offered = client.channel().map_err(|why| why.message)?.version;
    let version = match online::plan(client, ran, Some(&offered)).map_err(|why| why.message)? {
        Plan::Update { offered, from, release } => {
            if release.version != offered {
                println!(
                    "Rescue: Dukovo {offered} updates only from {from} on, and the root ran \
                     Dukovo {ran}: Dukovo {} comes first, and the system updates on from there",
                    release.version
                );
            }
            return Ok(release);
        }
        Plan::Current { offered } if offered == ran => offered,
        Plan::Current { offered } => {
            return Err(format!(
                "the {channel} channel offers Dukovo {offered}, older than the Dukovo {ran} the \
                 root ran: reinstall from Rescue's own copy, or from an update folder"
            ));
        }
        Plan::Withdrawn { offered, reason } => {
            return Err(format!("Dukovo {offered} was withdrawn: {reason}"));
        }
        Plan::Waiting { offered, .. } | Plan::Held { offered, .. } => {
            return Err(format!("the update server does not offer Dukovo {offered} here yet"));
        }
    };
    let server = client.settings.server.clone();
    client
        .release(&version)
        .map_err(|why| why.message)?
        .ok_or_else(|| format!("{server} does not offer Dukovo {version}"))
}

/// Networking, on for a reinstall from the update server with the
/// root's settings, as SSH into Rescue has it (ADR 0540), and off again
/// when it goes, unless it was on before.
struct Networking {
    /// Whether it was off before.
    turned_on: bool,
}

impl Networking {
    /// Turns networking on, unless it is, and waits up to
    /// [`ADDRESS_WAIT`] seconds for a card to have an address.
    fn on(root: &Root) -> Result<Networking, String> {
        let networking = Networking { turned_on: !runs("netd") };
        if networking.turned_on {
            take_root_network(root)?;
            networking_on()?;
        }
        println!("Rescue: waiting up to {ADDRESS_WAIT} seconds for an address");
        for _ in 0..ADDRESS_WAIT {
            if has_address() {
                return Ok(networking);
            }
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
        Err(format!(
            "no network card has an address after {ADDRESS_WAIT} seconds: check the cable, or \
             choose Check the network"
        ))
    }
}

impl Drop for Networking {
    fn drop(&mut self) {
        if !self.turned_on {
            return;
        }
        for name in NETWORKING.iter().rev() {
            let _ = Command::new("/system/commands/service").args([name, "stop"]).output();
        }
        println!("Rescue: networking is off again");
    }
}

/// Mounts, read-only, the first of the EFI system partitions `esps` that
/// holds Rescue, at [`ESP`].
fn mount_rescue(esps: &[String]) -> Result<PathBuf, String> {
    std::fs::create_dir_all(ESP).map_err(|err| format!("{ESP}: {err}"))?;
    for name in esps {
        let device = format!("/devices/{name}");
        if run("mount", &["-t", "fat", "-o", "ro", &device, ESP]).is_err() {
            continue;
        }
        if Path::new(ESP).join(system_release::rescue::DIR).join("release").is_dir() {
            return Ok(PathBuf::from(ESP));
        }
        let _ = run("umount", &[ESP]);
    }
    Err("no EFI system partition of the root's disks holds Rescue's release".into())
}

/// Builds `release` into the slot `root.running` from `files`: from a
/// writable snapshot of what the slot holds, so that only what differs
/// is written, or afresh when that fails, `afresh` first making `files`
/// whole; then reads it back against the signed list and seals it
/// read-only in the slot's place.
fn build_slot(
    root: &Root,
    release: &Release,
    files: &Files<'_>,
    afresh: &dyn Fn() -> Result<(), String>,
) -> Result<(), String> {
    let building = root.path(BUILDING);
    let slot = root.slot(root.running);
    let fresh = |from: Option<&Path>| -> Result<usize, String> {
        if building.exists() {
            subvolume::remove(&building).map_err(|why| why.message)?;
        }
        match from {
            Some(slot) => subvolume::snapshot_to(slot, &building, false),
            None => subvolume::make(&building),
        }
        .map_err(|why| why.message)?;
        stage::build(&building, release, files, None).map_err(|why| why.message)
    };
    println!("Rescue: building the slot, checking every file; this takes a few minutes");
    let written = match fresh(slot.is_dir().then_some(slot.as_path())) {
        Ok(written) => written,
        Err(why) => {
            println!("Rescue: the slot could not be mended ({why}); building it afresh");
            afresh()?;
            fresh(None)?
        }
    };
    stage::check(&building, release).map_err(|why| why.message)?;
    if slot.exists() {
        subvolume::remove(&slot).map_err(|why| why.message)?;
    }
    subvolume::snapshot_to(&building, &slot, true).map_err(|why| why.message)?;
    subvolume::remove(&building).map_err(|why| why.message)?;
    println!("Rescue: wrote {written} files into {}", root.running.subvolume());
    Ok(())
}

/// Puts the settings from before the last update back, keeping the ones
/// there as `settings-failed`, as a fallback does (ADR 0454).
fn put_back_settings(root: &Root) -> Result<String, String> {
    use slots::restore::{self, Seen, Step};
    match ask("Put the settings from before the last update back? Type yes: ", true) {
        Some(answer) if answer.trim() == "yes" => {}
        _ => return Err("the settings are as they were".into()),
    }
    let update = |name: &str| root.path(restore::UPDATE).join(name);
    let settings = root.path(restore::SETTINGS);
    let mut put_back = false;
    for _ in 0..=restore::MOST_STEPS {
        let seen = Seen {
            settings: settings.is_dir(),
            before: update(restore::BEFORE_SETTINGS).is_dir(),
            failed: update(restore::FAILED_SETTINGS).is_dir(),
            restored: update(restore::RESTORED).exists(),
        };
        let step = if put_back { Step::Mark } else { restore::next(seen, true) };
        let done = match step {
            Step::KeepFailed => {
                subvolume::snapshot_to(&settings, &update(restore::FAILED_SETTINGS), true)
            }
            Step::RemoveSettings => subvolume::remove(&settings),
            Step::PutBack => {
                subvolume::snapshot_to(&update(restore::BEFORE_SETTINGS), &settings, false)
            }
            Step::Mark => {
                std::fs::write(update(restore::RESTORED), "settings put back by Rescue\n")
                    .map_err(|err| util::failed(&restore::RESTORED, err))
            }
            Step::MakeEmpty => subvolume::make(&settings),
            Step::Done => break,
        };
        done.map_err(|why| why.message)?;
        put_back = step == Step::PutBack;
    }
    Ok(format!(
        "The settings from before the last update are back; the ones they replace are kept in \
         {}/{}.",
        restore::UPDATE,
        restore::FAILED_SETTINGS
    ))
}

/// Checks the root's Btrfs, read-only, with it unmounted, and mounts it
/// again.
fn check_disk(root: &Root) -> Result<String, String> {
    let top = format!("/volumes/{}", root.disk);
    run("umount", &[&top]).map_err(|why| format!("the root cannot be checked: {why}"))?;
    let checked = Command::new("/system/commands/fsck-btrfs")
        .arg(&root.disk)
        .status()
        .map_err(|err| format!("fsck-btrfs: {err}"));
    let device = format!("/devices/{}", root.disk);
    let mounted = run("mount", &["-t", "btrfs", "-o", "compress", &device, &top]);
    let status = checked?;
    mounted.map_err(|why| format!("the root did not mount again: {why}"))?;
    if status.success() {
        Ok(format!("The Btrfs on {} is sound.", root.disk))
    } else {
        Err(format!(
            "the Btrfs on {} has problems, which fsck-btrfs(8) named above; copy the files off \
             before anything else",
            root.disk
        ))
    }
}

/// Copies the users' and shared files to a folder on another disk the
/// owner names, which Rescue mounts.
fn copy_off(root: &Root) -> Result<String, String> {
    println!("Disks here: {}", other_disks(root).join(", "));
    let disk = ask("Copy to which disk or partition (such as sdb1)? ", true)
        .ok_or("nothing was copied")?;
    let disk = disk.trim().trim_start_matches("/devices/");
    if disk.is_empty() || probe::disk_of(disk) == probe::disk_of(&root.disk) {
        return Err("name a disk other than the root's".into());
    }
    std::fs::create_dir_all(COPY_TO).map_err(|err| format!("{COPY_TO}: {err}"))?;
    run("mount", &[&format!("/devices/{disk}"), COPY_TO])
        .map_err(|why| format!("{disk} will not mount: {why}"))?;
    let folder =
        Path::new(COPY_TO).join(format!("dukovo-rescue-{}", util::now_ns() / 1_000_000_000));
    let copied = std::fs::create_dir(&folder)
        .map_err(|err| format!("{}: {err}", folder.display()))
        .and_then(|()| {
            COPIED.iter().try_for_each(|name| {
                let from = root.path(name).display().to_string();
                println!("Rescue: copying {name}");
                run("cp", &["-R", &from, &folder.display().to_string()])
            })
        });
    let unmounted = run("umount", &[COPY_TO]);
    copied?;
    unmounted?;
    let name = folder.file_name().map_or_default(|name| name.to_string_lossy().into_owned());
    Ok(format!("The users' and shared files are copied to {name} on {disk}."))
}

/// The disks and partitions other than the root's disk, for the owner to
/// copy to.
fn other_disks(root: &Root) -> Vec<String> {
    probe::devices()
        .unwrap_or_default()
        .into_iter()
        .map(|(name, _)| name)
        .filter(|name| probe::disk_of(name) != probe::disk_of(&root.disk))
        .collect()
}

/// Starts the services networking needs, the firewall already running.
/// A driver the kernel left none of the cards it drives, as e1000d on a
/// machine with no Intel card, does not start, and is off as it is meant
/// to be: that is no failure, and what the service command said of it is
/// not shown.
fn networking_on() -> Result<String, String> {
    for name in NETWORKING {
        let path = format!("/system/settings/spark/services/{name}.toml");
        if !Path::new(&path).exists() {
            continue;
        }
        let started = Command::new("/system/commands/service")
            .args([name, "start"])
            .output()
            .map_err(|err| format!("service: {err}"))?;
        if !started.status.success() && !off_as_meant(name) {
            eprint!("{}", String::from_utf8_lossy(&started.stderr));
            return Err(format!("service {name} start failed ({})", started.status));
        }
    }
    Ok("Networking is on, behind the firewall.".into())
}

/// Whether Spark says service `name` is off as it is meant to be. After a
/// request to start it, that is a driver the kernel left no cards.
fn off_as_meant(name: &str) -> bool {
    service::shown(Some(name))
        .is_ok_and(|show| show.services.iter().any(|s| s.name == name && s.off_as_meant))
}

/// Turns networking on as [`networking_on`] does, waits a little for
/// the cards to have addresses, which DHCP gives some seconds after
/// netd starts, then shows what `netcheck` finds.
fn check_network() -> Result<String, String> {
    networking_on()?;
    println!("Rescue: waiting up to {ADDRESS_WAIT} seconds for an address");
    for _ in 0..ADDRESS_WAIT {
        if has_address() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
    show("netcheck", &[])
}

/// Whether a network card has an IPv4 address.
fn has_address() -> bool {
    ip::read_interfaces(Path::new(ip::INTERFACES), Path::new(ip::ROUTES))
        .is_ok_and(|cards| cards.iter().any(|card| !card.loopback && !card.addresses.is_empty()))
}

/// Runs `program` with `args` from /system/commands, its results on the
/// screen, and waits for Enter before the choices come back over them.
/// What a check finds is in its output and its exit status, so no status
/// is a failure here.
fn show(program: &str, args: &[&str]) -> Result<String, String> {
    Command::new(format!("/system/commands/{program}"))
        .args(args)
        .status()
        .map_err(|err| format!("{program}: {err}"))?;
    // Any answer, or the console going away, ends the wait.
    let _ = ask("Press Enter to go back to the choices. ", true);
    Ok("Back in Rescue.".into())
}

/// Runs root's shell, as the running system's user database names it,
/// until it ends. `SHELL` is the console's sign-in, not a shell.
fn shell() -> Result<String, String> {
    let passwd = std::fs::read_to_string(accounts::PASSWD).unwrap_or_default();
    let shell = crate::elevate::root_account(accounts::users(&passwd).entries().cloned()).shell;
    Command::new(&shell).status().map_err(|err| format!("{shell}: {err}"))?;
    Ok("Back in Rescue.".into())
}

/// Sets the count of starts not seen through back to 0, so that the
/// system starts as usual, and restarts.
fn start_normally(root: Option<&Root>) -> Result<String, String> {
    if let Some(root) = root {
        let esps = place::partitions(root).map_err(|why| why.message)?;
        for why in place::reset_rescue(esps.iter().map(String::as_str)) {
            println!("Rescue: {why}");
        }
    }
    end("reboot")
}

/// Asks Spark to `request` the machine's end: `reboot` or `poweroff`.
fn end(request: &str) -> Result<String, String> {
    power::request(request).map_err(|why| why.message)?;
    // Spark stops the consoles; wait for it.
    loop {
        std::thread::sleep(std::time::Duration::from_secs(60));
    }
}

/// Runs the command `program` from /system/commands with `args`, its
/// output shown; why it failed.
fn run(program: &str, args: &[&str]) -> Result<(), String> {
    let status = Command::new(format!("/system/commands/{program}"))
        .args(args)
        .status()
        .map_err(|err| format!("{program}: {err}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{program} {} failed ({status})", args.join(" ")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_reinstall_from_the_server_reads_the_root_s_update_settings() {
        let dir = std::env::temp_dir().join(format!("rescue-settings-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("updates.toml");
        assert_eq!(root_update_settings(&file), Ok(online::Settings::default()));
        fs::write(&file, "channel = \"beta\"\nserver = \"http://10.0.2.2:8080/system/\"\n")
            .unwrap();
        let settings = root_update_settings(&file).unwrap();
        assert_eq!(
            (settings.channel.as_str(), settings.server.as_str()),
            ("beta", "http://10.0.2.2:8080/system/")
        );
        fs::write(&file, "channel = 7\n").unwrap();
        assert!(root_update_settings(&file).unwrap_err().starts_with(&file.display().to_string()));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_slot_that_does_not_say_its_release_needs_rescue_s_copy_to() {
        let root = Root {
            disk: format!("no-such-disk-{}", std::process::id()),
            uuid: String::new(),
            partitions: Vec::new(),
            running: Slot::A,
        };
        let why = root_version(&root, &[]).unwrap_err();
        assert!(why.starts_with("the slot does not say which release it held"), "{why}");
        assert!(why.ends_with("reinstall from an update folder or file"), "{why}");
    }

    #[test]
    fn rescue_has_ssh_on_only_when_the_installed_system_did() {
        let file = "service_version = 1\ndescription = \"Sign-in over SSH\"\n\
                    program = \"/system/commands/sshd\"\nenabled = false\n";
        assert_eq!(enabled_in(file, None), Ok(false));
        let on = file.replace("enabled = false", "enabled = true");
        assert_eq!(enabled_in(&on, None), Ok(true));
        // An administrator's override has the last word.
        assert_eq!(enabled_in(file, Some("enabled = true\n")), Ok(true));
        assert_eq!(enabled_in(&on, Some("enabled = false\n")), Ok(false));
        let unread = enabled_in("enabled = maybe\n", None).unwrap_err();
        assert!(unread.starts_with("the root's sshd service file does not read"), "{unread}");
    }

    #[test]
    fn the_shipped_sshd_file_leaves_ssh_off() {
        let shipped = include_str!("../../../defaults/spark/services/sshd.toml");
        assert_eq!(enabled_in(shipped, None), Ok(false));
    }

    #[test]
    fn administrators_are_those_in_admins_but_root() {
        let accounts = Accounts {
            users: accounts::users(
                "root:x:0:0:root:/users/root:/system/commands/nukesh\n\
                 alice:x:1000:1000:Alice:/users/alice:/system/commands/nukesh\n\
                 bob:x:1001:1001:Bob:/users/bob:/system/commands/nukesh\n\
                 carol:x:1002:10:Carol:/users/carol:/system/commands/nukesh\n",
            ),
            groups: accounts::groups("admins:x:10:root,alice\nbob:x:1001:\n"),
            hashes: accounts::passwords(""),
        };
        // carol is one by her own group, as authd counts her.
        assert_eq!(admins(&accounts), ["alice", "carol"]);
    }

    #[test]
    fn the_lock_says_who_holds_it_and_from_where() {
        let alice = Person { name: Some("alice".into()), over_ssh: Some("192.168.1.5".into()) };
        let line = holder_line(&alice, repair("3").unwrap());
        assert_eq!(
            held_by(&line),
            "alice is reinstalling Dukovo, over SSH from 192.168.1.5: wait for it to finish, \
             then choose again"
        );
        let console = Person { name: None, over_ssh: None };
        assert_eq!(
            held_by(&holder_line(&console, repair("13").unwrap())),
            "someone is powering off, at the console: wait for it to finish, then choose again"
        );
        // Taken, and not yet written: no holder to name.
        assert!(held_by("").starts_with("another repair is under way"));
    }

    #[test]
    fn only_the_choices_that_change_something_take_the_lock() {
        for choice in ["3", "4", "5", "6", "11", "12", "13"] {
            assert!(repair(choice).is_some(), "{choice}");
        }
        for choice in ["0", "1", "2", "7", "8", "9", "10", "", "x"] {
            assert!(repair(choice).is_none(), "{choice}");
        }
    }

    #[test]
    fn the_client_s_address_is_ssh_connection_s_first_word() {
        assert_eq!(ssh_client("192.168.1.5 50122 192.168.1.9 22"), "192.168.1.5");
        assert_eq!(ssh_client("fd00::5 50122 fd00::9 22"), "fd00::5");
        // Nothing else is shown as one.
        assert_eq!(ssh_client(""), "an unknown address");
        assert_eq!(ssh_client("\x1b[2J 1 2 3"), "an unknown address");
    }
}

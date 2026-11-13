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
//! own copy of the release or from an update folder, keeping the
//! settings, apps and files; putting back the settings from before the
//! last update; checking the root's disk; copying the users' and shared
//! files to another disk; checking the hardware and the network, which
//! run `hardware check` and `netcheck` (ADRs 0488 and 0489); turning
//! networking on; a shell; and starting the system normally. Nothing is
//! formatted, and `@users`, `@shared` and `@applications` are never
//! written.
//!
//! Each repair says what it did, or why it could not, in one line;
//! `/system/state/log` keeps Rescue's log until it restarts.

use std::path::{Path, PathBuf};
use std::process::Command;

use slots::{Slot, State};
use system_release::list::Release;
use system_release::store::Store;

use crate::args::Args;
use crate::changes;
use crate::ip;
use crate::prompt::{self, Answer};
use crate::sysupdate::place::{self, BUILDING, Root};
use crate::sysupdate::stage::{self, Files, TRUSTED_ROOT};
use crate::sysupdate::starts::{self, Outcome};
use crate::util::{self, Fail, fail};
use crate::{power, probe, subvolume};

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

/// What Rescue knows of the root it rescues.
struct Rescued {
    /// The root's Btrfs UUID, as the boot menu named it.
    uuid: String,
    /// The root, mounted whole at `/volumes/<disk>`, or why it is not.
    root: Result<Root, String>,
}

/// dukovo-rescue(8).
pub fn main(args: &mut Args) -> Result<i32, Fail> {
    if let Some(arg) = args.next()? {
        return Err(args.unknown(&arg));
    }
    let Some(uuid) = util::kernel_word("rescue=") else {
        return Err(fail(
            "this is not Rescue: the boot menu's Rescue entry starts it, with rescue= on the \
             kernel's command line"
                .into(),
        ));
    };
    println!("\x1b[H\x1b[2J\x1b[?25hDukovo Rescue {}", env!("CARGO_PKG_VERSION"));
    println!("Rescue repairs this computer's Dukovo. Files, apps and settings stay as they are");
    println!("unless you choose otherwise. Networking is off until you turn it on.");
    println!();
    let rescued = Rescued { root: mount_root(&uuid), uuid };
    match &rescued.root {
        Ok(root) => println!("The Dukovo root is {}, on {}.", rescued.uuid, root.disk),
        Err(why) => println!("The Dukovo root {} cannot be used: {why}.", rescued.uuid),
    }
    if let Ok(root) = &rescued.root
        && !signed_in(root)
    {
        // consoled starts Rescue again, which asks again.
        std::thread::sleep(std::time::Duration::from_secs(10));
        return Ok(1);
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
        let done = match (choice.trim(), &rescued.root) {
            ("0", Ok(root)) => Ok(why_it_failed(root).join("\n")),
            ("1", Ok(root)) => reinstall(root),
            ("2", Ok(root)) if update.is_some() => put_back_settings(root),
            ("3", Ok(root)) => check_disk(root),
            ("4", Ok(root)) => copy_off(root),
            ("5", _) => show("hardware", &["check"]),
            ("6", _) => networking_on(),
            ("7", _) => check_network(),
            ("8", _) => shell(),
            ("9", root) => start_normally(root.as_ref().ok()),
            ("10", _) => end("reboot"),
            ("11", _) => end("poweroff"),
            (other, _) => Err(format!("{other} is not one of the choices here")),
        };
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
        lines.push(format!(
            "Help: https://dukovo.org/support/{}/errors/{id}.html",
            env!("CARGO_PKG_VERSION")
        ));
    }
    lines
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

/// Finds the root of Btrfs UUID `uuid` and mounts it whole, compressed,
/// at `/volumes/<disk>`, unless it is there already, as it is when Rescue
/// starts again on a console.
fn mount_root(uuid: &str) -> Result<Root, String> {
    let devices = probe::devices().map_err(|err| format!("the disks cannot be listed: {err}"))?;
    let disk = devices
        .iter()
        .map(|(name, _)| name)
        .find(|name| place::btrfs_uuid(name).as_deref() == Some(uuid))
        .ok_or("no disk here holds it")?
        .clone();
    let top = Path::new("/volumes").join(&disk);
    if !top.join("@state").is_dir() {
        std::fs::create_dir_all(&top).map_err(|err| format!("{}: {err}", top.display()))?;
        let device = format!("/devices/{disk}");
        run("mount", &["-t", "btrfs", "-o", "compress", &device, &top.display().to_string()])
            .map_err(|why| format!("it will not mount: {why}"))?;
    }
    // The slot the state names as the default is the one a reinstall
    // writes; slot A when the state cannot be read.
    let mut root = Root { disk, uuid: uuid.to_string(), running: Slot::A };
    if let Ok((_, state)) = place::state(&root) {
        root.running = state.default;
    }
    Ok(root)
}

/// Asks for an administrator's name and password, checked against the
/// root's accounts, when its settings can be read; whether they were
/// right.
fn signed_in(root: &Root) -> bool {
    let read = |name: &str| std::fs::read_to_string(root.path(name));
    let (Ok(passwd), Ok(group), Ok(passwords)) = (read(PASSWD), read(GROUP), read(PASSWORDS))
    else {
        println!("The root's accounts cannot be read, so Rescue asks for no password.");
        return true;
    };
    let users = accounts::users(&passwd);
    let groups = accounts::groups(&group);
    let hashes = accounts::passwords(&passwords);
    let admins: Vec<String> = groups
        .entries()
        .filter(|g| g.name == accounts::ADMINS)
        .flat_map(|g| g.members.iter().cloned())
        .filter(|name| users.entries().any(|user| user.name == *name))
        .collect();
    if admins.is_empty() {
        println!("This system has no administrator, so Rescue asks for no password.");
        return true;
    }
    println!("Rescue needs one of this computer's administrators.");
    for _ in 0..TRIES {
        let Some(name) = ask("Name: ", true) else { return false };
        let Some(password) = ask("Password: ", false) else { return false };
        let mut password = password.into_bytes();
        let name = name.trim();
        let right = admins.iter().any(|admin| admin == name)
            && hashes.entries().find(|entry| entry.name == name).is_some_and(|entry| {
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
            return true;
        }
        println!("That is not an administrator's name and password.");
    }
    false
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
/// the release, or from an update folder the owner names, keeping the
/// settings, the state and every user's files and apps.
fn reinstall(root: &Root) -> Result<String, String> {
    println!("Reinstall from Rescue's own copy, or from an update folder or file on a disk?");
    let from = ask("Press Enter for Rescue's copy, or type the folder or file: ", true)
        .ok_or("nothing was reinstalled")?;
    let from = from.trim();
    let esps = place::partitions(root).map_err(|why| why.message)?;
    let trusted = std::fs::read(TRUSTED_ROOT).map_err(|err| format!("{TRUSTED_ROOT}: {err}"))?;
    let mounted = if from.is_empty() { Some(mount_rescue(&esps)?) } else { None };
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
        let slot = root.running.letter().to_ascii_uppercase();
        println!("Reinstall Dukovo {} into slot {slot}?", release.version);
        println!("Settings, apps and every user's files stay as they are.");
        match ask("Type yes to reinstall: ", true) {
            Some(answer) if answer.trim() == "yes" => {}
            _ => return Err("nothing was reinstalled".into()),
        }
        let rescue_dir = mounted.as_ref().map(|esp| esp.join(system_release::rescue::DIR));
        let files = match (&rescue_dir, archive.take()) {
            (Some(dir), _) => Files::Rescue { rescue: dir, system: Path::new("/system") },
            (None, Some(archive)) => Files::Archive(archive),
            (None, None) => Files::Store(Store::at(&Path::new(from).join("files"))),
        };
        build_slot(root, &release, &files).map(|()| release)
    });
    if mounted.is_some() {
        let _ = run("umount", &[ESP]);
    }
    let release = result?;
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
/// is written, or afresh when that fails; then reads it back against the
/// signed list and seals it read-only in the slot's place.
fn build_slot(root: &Root, release: &Release, files: &Files<'_>) -> Result<(), String> {
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
        stage::build(&building, release, files).map_err(|why| why.message)
    };
    println!("Rescue: building the slot, checking every file; this takes a few minutes");
    let written = match fresh(slot.is_dir().then_some(slot.as_path())) {
        Ok(written) => written,
        Err(why) => {
            println!("Rescue: the slot could not be mended ({why}); building it afresh");
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
fn networking_on() -> Result<String, String> {
    for name in NETWORKING {
        let path = format!("/system/settings/spark/services/{name}.toml");
        if Path::new(&path).exists() {
            run("service", &[name, "start"])?;
        }
    }
    Ok("Networking is on, behind the firewall.".into())
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

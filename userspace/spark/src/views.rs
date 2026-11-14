// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What a service holds (ADR 0217): its root is a view (ADR 0215) that
//! Spark builds as it starts the service, of exactly what its file
//! declares and what every service gets without asking.
//!
//! The view holds, each at its real path: the places [`ALWAYS`] lists,
//! read-only but for the devices; the places the file names under
//! `files`, read-only or not as it says, each of the second kind made if
//! it is not there yet; the service's program; the folder of its
//! secrets, read-only; the folder of its copies of the certificates it
//! names (`certificates`), read-only; a `/tmp` of its own, which is
//! `/system/live/spark/tmp/NAME`; and the links of the root and of
//! `/system`, so that `/bin/sh` and `/usr/bin/env` lead where they do. The
//! service acts for root without the bypass right, so the owners and modes
//! of its files narrow it too, and it reaches nothing past its view. A
//! place `files.mount` names is handed to it as a descriptor as well, from
//! 3 on, with the control right that mounting needs and a view never
//! hands on; `MOUNT_PLACES` names them, in that order.
//!
//! It sees its own processes at `/processes`: those of its resource
//! domain and the domains beneath it, from the kernel (ADR 0473), unless
//! its file names `/processes`, which holds them all. A file with
//! `files.disks` or `files.efi-system` gets a `/devices` of that class in
//! place of the four devices every service has: they are in it too.
//! Spark mounts each class once, at first need, at
//! `/system/live/spark/devices/<class>`. A file with
//! `files.root-subvolumes` gets the top of the root's Btrfs at
//! `/volumes/<disk>`, where Spark mounted it, with those names alone in
//! it: no other name there is found, listed, made, removed or renamed
//! through the view, `@users` and `@shared` among them. A socket the file
//! names, such as logd's `/system/live/log/query`, cannot be opened to be
//! attached, so its folder is attached in the same way, with only the
//! names the file gives in it.
//!
//! A file with `runs-with-your-access` gets Spark's own root instead, the
//! whole tree with the bypass right, as consoled needs to start root's
//! shells until sign-in (13b.5).
//!
//! A file with an `account`, a bundle's service's (ADR 0385), gets a view
//! that acts for that account: the places in it are opened as the account,
//! so that the owners and modes of their files narrow it as they would a
//! person, and what it makes is the account's. Its `/tmp`, the places it
//! may change that Spark makes, its secrets and its certificates' copies
//! are the account's.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use libradix::abi::capability::Rights;
use libradix::abi::directory::{FileKind, FilesystemKind, MountFlags};
use libradix::abi::errors::File as FileError;
use libradix::abi::names as names_abi;
use libradix::abi::resource_domain;
use manifest::service::{self, ALWAYS, PROCESSES, Place};
use radix_rt::fs::{Dir, File};
use radix_rt::{SYS, slots};
use spark_api::root::{self as root_record, Root as RootRecord};

use crate::println;
use crate::services::{self, Spec};

/// Where the services' own `/tmp` folders are.
pub const TMP: &str = "system/live/spark/tmp";

/// The folder whose links a view copies with the root's (`/bin`,
/// `/etc`): `/system`'s are `/system/bin` and the like, where `/usr/bin`
/// leads.
const SYSTEM: &str = "system";

/// The rights of what a service gets: to read and change, and to copy
/// and pass on, as `fork` and `exec` need.
const GIVEN: Rights = Rights::READ.union(Rights::WRITE).union(Rights::COPY).union(Rights::TRANSFER);

/// The folder of /system/live that holds a folder for each bundle's
/// service, beneath which each is named for the service's account.
const BUNDLE_SERVICES: &str = "services/";

/// Where Spark mounts the classes of `/devices` its services hold.
const CLASSES: &str = "system/live/spark/devices";

/// The read right, and to copy and pass on: what a place held read-only
/// gets.
const READ_ONLY: Rights = Rights::READ.union(Rights::COPY).union(Rights::TRANSFER);

/// A class of `/devices` a service's file asks for (ADR 0473).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceClass {
    /// Every disk and partition: `files.disks`.
    Disks,
    /// The EFI system partitions: `files.efi-system`.
    EfiSystem,
}

impl DeviceClass {
    /// The kind of filesystem the kernel makes of it.
    fn kind(self) -> FilesystemKind {
        match self {
            DeviceClass::Disks => FilesystemKind::DevDisks,
            DeviceClass::EfiSystem => FilesystemKind::DevEfiSystem,
        }
    }

    /// Its name, as its key in a service file has it, and its folder in
    /// [`CLASSES`].
    pub fn name(self) -> &'static str {
        match self {
            DeviceClass::Disks => "disks",
            DeviceClass::EfiSystem => "efi-system",
        }
    }
}

/// A service's root and what it mounts beneath.
pub struct View {
    /// Its root: the view, or Spark's root for a service that runs with
    /// Spark's access.
    pub root: Root,
    /// The places `files.mount` names, with the control right, in order.
    pub mounts: Vec<Dir>,
}

/// The account a service runs as, other than root's.
pub struct Account {
    /// Spark's root, acting for the account.
    pub root: Dir,
    /// Its user number.
    pub user: u32,
    /// Its group's number.
    pub group: u32,
    /// The `names` capability, which stamps what the view holds from
    /// elsewhere than the root with the account's name, as a view acting
    /// for it takes only that.
    pub names: u32,
}

/// A service's root.
pub enum Root {
    /// A view of its own.
    View(Dir),
    /// Spark's root.
    Spark,
}

/// Builds service `spec`'s view beneath Spark's `root`, acting for its
/// `account` if it has one, with the processes of its `domain`, or says
/// why not.
pub fn build(
    root: &Dir,
    account: Option<&Account>,
    spec: &Spec,
    domain: Option<u32>,
) -> Result<View, String> {
    if spec.whole_tree {
        return Ok(View { root: Root::Spark, mounts: Vec::new() });
    }
    // Without the bypass right: the view, and each place attached to it,
    // acts for root, or for the account, as the owners and modes of its
    // files let it.
    let base = account
        .map_or(root, |account| &account.root)
        .derive(GIVEN.union(Rights::CONTROL))
        .map_err(|err| format!("cannot narrow the root: {err}"))?;
    let view = base.make_view().map_err(|err| format!("cannot make its view: {err}"))?;
    let mut attached: Vec<String> = Vec::new();

    // A class of /devices holds the devices every service has.
    let always = ALWAYS
        .iter()
        .filter(|(path, _)| !spec.places.iter().any(|p| within(path, &p.path)))
        .filter(|(path, _)| spec.device_class.is_none() || !within(path, "/devices"))
        .map(|&(path, write)| (Place { path: path.into(), write }, true));
    let declared = spec.places.iter().map(|place| (place.clone(), false));
    let places: Vec<(Place, bool)> = always.chain(declared).collect();
    let folders = socket_folders(root, &places);
    for (place, optional) in places {
        let path = &place.path[1..];
        if folders.iter().any(|folder| folder.holds(path)) {
            continue;
        }
        if let Err(err) = root.stat(path) {
            match when_missing(path, place.write, optional) {
                Missing::Make => {
                    services::make_dirs(root, path)?;
                    if place.write {
                        own(root, account, path)?;
                    }
                }
                Missing::LeaveOut => {
                    if !optional {
                        println!(
                            "spark: {}: {} is not there yet; its view leaves it out",
                            spec.name, place.path
                        );
                    }
                    continue;
                }
                Missing::Refuse => return Err(format!("{}: {err}", place.path)),
            }
        }
        attach(&base, &view, path, place.write)?;
        attached.push(place.path);
    }
    for folder in &folders {
        let rights = if folder.write { GIVEN } else { READ_ONLY };
        let dir = base
            .open_dir(&folder.path, rights)
            .map_err(|err| format!("/{}: {err}", folder.path))?;
        let names: Vec<&str> = folder.names.iter().map(String::as_str).collect();
        view.attach_names(&folder.path, dir.slot(), &names).map_err(|err| {
            format!("cannot attach /{} with {}: {err}", folder.path, names.join(", "))
        })?;
        attached.extend(names.iter().map(|name| format!("/{}/{name}", folder.path)));
    }

    if !spec.places.iter().any(|place| place.path == PROCESSES) {
        attach_processes(&base, &view, domain, account)?;
    }
    if let Some((class, access)) = spec.device_class {
        let place = mount_class(root, class)?;
        attach_as(&base, &view, &place, "devices", access.write())?;
    }
    if !spec.root_subvolumes.is_empty() {
        attach_root_subvolumes(root, &base, &view, &spec.root_subvolumes)?;
    }

    // Its program, unless a place holds it already.
    if !attached.iter().any(|path| within(&spec.program, path)) {
        attach(&base, &view, &spec.program[1..], false)?;
    }

    if !spec.secrets.is_empty() {
        let dir = service::secrets_dir(&spec.name);
        for secret in &spec.secrets {
            if root.stat(&format!("{}/{secret}", &dir[1..])).is_err() {
                return Err(format!(
                    "its secret {secret} is not set: set it with `service secret set {} {secret}`",
                    spec.name
                ));
            }
        }
        // A view holds only what acts for its name: the secrets are the
        // account's, then, as systemd hands a service's credentials to its
        // user, read-only.
        if let Some(account) = account {
            let give = |path: &str, mode| {
                root.set_access(path, Some(account.user), Some(account.group), Some(mode))
                    .map_err(|err| format!("cannot give /{path} to its account: {err}"))
            };
            give(&dir[1..], 0o500)?;
            for secret in &spec.secrets {
                give(&format!("{}/{secret}", &dir[1..]), 0o400)?;
            }
        }
        attach(&base, &view, &dir[1..], false)?;
    }

    // Its copies of its certificates, which Spark made as it started,
    // read-only (ADR 0512).
    if !spec.certificates.is_empty() {
        attach(&base, &view, &service::certificates_dir(&spec.name)[1..], false)?;
    }

    let tmp = format!("{TMP}/{}", spec.name);
    services::make_dirs(root, &tmp)?;
    own(root, account, &tmp)?;
    let opened = base.open_dir(&tmp, GIVEN).map_err(|err| format!("/{tmp}: {err}"))?;
    view.attach("tmp", opened.slot()).map_err(|err| format!("cannot attach /tmp: {err}"))?;

    copy_links(root, &view, "");
    if let Ok(system) = root.open_dir(SYSTEM, Rights::READ) {
        copy_links(&system, &view, SYSTEM);
    }

    let mut mounts = Vec::new();
    for path in &spec.mounts {
        let place = base
            .open_dir(&path[1..], GIVEN.union(Rights::CONTROL))
            .map_err(|err| format!("{path}: {err}"))?;
        mounts.push(place);
    }
    // Without the control right: nothing more can be attached to it.
    let given = view.derive(GIVEN).map_err(|err| format!("cannot narrow its view: {err}"))?;
    Ok(View { root: Root::View(given), mounts })
}

/// Gives the folder at `path` beneath `root` to `account`, if the service
/// has one, for it alone.
fn own(root: &Dir, account: Option<&Account>, path: &str) -> Result<(), String> {
    let Some(account) = account else { return Ok(()) };
    root.set_access(path, Some(account.user), Some(account.group), Some(0o700))
        .map_err(|err| format!("cannot give /{path} to its account: {err}"))
}

/// Whether `inner` is `outer` or beneath it.
fn within(inner: &str, outer: &str) -> bool {
    inner.strip_prefix(outer).is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}

/// What becomes of a place of a view that is not there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Missing {
    /// Spark makes it, a folder.
    Make,
    /// The view goes without it.
    LeaveOut,
    /// The service does not start.
    Refuse,
}

/// What becomes of `path`, a place of a view held for writing if `write`
/// and one every service gets if `optional`, when it is not there.
///
/// A folder of /system/live, Spark's folder for what goes at shutdown, is
/// made, as notifyd's is for those that start before notifyd, and so is
/// a bundle's service's folder of /system/live/services, named for its
/// account, which its file names for it to change (ADR 0386); so is a
/// place the file lets it change elsewhere, as systemd's
/// StateDirectory= is. What lies deeper in /system/live is another
/// service's to make, a socket say, which a folder in its place would
/// break: the view goes without it until a later start, as it goes
/// without a place every service gets that this system lacks. So does
/// a place of /system/state held read-only: state another service keeps,
/// which it may not have written yet on a new system, as troubleshoot's
/// signatures before the first update. Any other place that is not there
/// is a mistake in the file, and the service does not start.
fn when_missing(path: &str, write: bool, optional: bool) -> Missing {
    if let Some(rest) = path.strip_prefix("system/live/") {
        let folder = rest.strip_prefix(BUNDLE_SERVICES).unwrap_or(rest);
        return if folder.contains('/') { Missing::LeaveOut } else { Missing::Make };
    }
    if optional {
        Missing::LeaveOut
    } else if write {
        Missing::Make
    } else if within(path, "system/state") {
        Missing::LeaveOut
    } else {
        Missing::Refuse
    }
}

/// Attaches its processes at `processes` in `view`: those of its
/// `domain` and beneath, or, for a service Spark made no domain for, as
/// where the kernel gave Spark none, every process, as `base` has them.
/// The domain's list is Spark's, so for a service that acts for an
/// `account` it is stamped with the account's name first: the kernel
/// attaches to a view only what acts for the view's name.
fn attach_processes(
    base: &Dir,
    view: &Dir,
    domain: Option<u32>,
    account: Option<&Account>,
) -> Result<(), String> {
    let path = &PROCESSES[1..];
    let Some(domain) = domain else {
        // Where it is: a system may start without the processes' files.
        return if base.stat(path).is_ok() { attach(base, view, path, false) } else { Ok(()) };
    };
    let slot = slots::take().ok_or("no free slot")?;
    if let Err(status) = resource_domain::processes(&SYS, domain, slot) {
        slots::give_back(slot);
        return Err(format!("cannot list its processes: {:#x}", status.0));
    }
    let mut processes = Dir::own(slot).map_err(|err| format!("its processes: {err}"))?;
    if let Some(account) = account {
        let slot = slots::take().ok_or("no free slot")?;
        let (user, group) = (account.user, account.group);
        if let Err(status) =
            names_abi::stamp(&SYS, account.names, processes.slot(), slot, user, group, &[])
        {
            slots::give_back(slot);
            return Err(format!("cannot list its processes as its account: {:#x}", status.0));
        }
        processes = Dir::own(slot).map_err(|err| format!("its processes: {err}"))?;
    }
    view.attach(path, processes.slot()).map_err(|err| format!("cannot attach /{path}: {err}"))
}

/// The folder in Spark's `root` where `class` is mounted, mounting it
/// there first if it is not yet.
fn mount_class(root: &Dir, class: DeviceClass) -> Result<String, String> {
    let place = format!("{CLASSES}/{}", class.name());
    services::make_dirs(root, &place)?;
    let mounted = root
        .open_dir(&place, Rights::READ)
        .and_then(|dir| dir.info())
        .is_ok_and(|info| info.kind == FilesystemKind::Dev);
    if !mounted {
        root.mount(&place, class.kind(), None, MountFlags::NONE)
            .map_err(|err| format!("cannot mount the {} on /{place}: {err}", class.name()))?;
    }
    Ok(place)
}

/// A folder that holds a socket a service's file names, with the names
/// of its places in it, and whether one of them that is no socket is
/// written.
struct SocketFolder {
    path: String,
    names: Vec<String>,
    write: bool,
}

impl SocketFolder {
    /// Whether `path` is one of its places.
    fn holds(&self, path: &str) -> bool {
        path.rsplit_once('/').is_some_and(|(folder, _)| folder == self.path)
    }
}

/// The folders of the sockets among `places` that are there, beneath
/// `root`. A socket cannot be opened to be attached as a file is, so a
/// service reaches one through its folder, attached with only the names
/// of the places in it that are there (ADR 0473): read-only unless one
/// that is no socket is written, since connecting writes nothing to the
/// folder. A server that makes its socket again, as logd does when it
/// restarts, is found at the same name.
fn socket_folders(root: &Dir, places: &[(Place, bool)]) -> Vec<SocketFolder> {
    let is_socket = |path: &str| root.stat(path).is_ok_and(|stat| stat.kind == FileKind::Socket);
    let mut folders: Vec<SocketFolder> = Vec::new();
    for (place, _) in places {
        let path = &place.path[1..];
        if let Some((folder, _)) = path.rsplit_once('/')
            && is_socket(path)
            && !folders.iter().any(|known| known.path == folder)
        {
            folders.push(SocketFolder { path: folder.into(), names: Vec::new(), write: false });
        }
    }
    for (place, _) in places {
        let path = &place.path[1..];
        let Some(folder) = folders.iter_mut().find(|folder| folder.holds(path)) else { continue };
        let Some((_, name)) = path.rsplit_once('/') else { continue };
        if root.stat(path).is_ok() && !folder.names.iter().any(|known| known == name) {
            folder.names.push(name.into());
            folder.write |= place.write && !is_socket(path);
        }
    }
    folders
}

/// Attaches the top of the root's Btrfs at `volumes/<disk>` in `view`,
/// read-write, with only `names` in it. A system with no root on a disk
/// (the live system, Rescue) has no such top, and the view none.
fn attach_root_subvolumes(
    root: &Dir,
    base: &Dir,
    view: &Dir,
    names: &[String],
) -> Result<(), String> {
    let Some(record) = root_record(root) else { return Ok(()) };
    let path = format!("volumes/{}", record.disk);
    let top = base.open_dir(&path, GIVEN).map_err(|err| format!("/{path}: {err}"))?;
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    view.attach_names(&path, top.slot(), &names)
        .map_err(|err| format!("cannot attach /{path} with {}: {err}", names.join(", ")))
}

/// The root Spark started, as it recorded it, if it started one.
pub fn root_record(root: &Dir) -> Option<RootRecord> {
    let text = root.read_file(&root_record::PATH[1..]).ok()?;
    RootRecord::parse(core::str::from_utf8(&text).ok()?)
}

/// Attaches the directory or file at `path` beneath `base` at the same
/// path in `view`, read-only unless `write`.
fn attach(base: &Dir, view: &Dir, path: &str, write: bool) -> Result<(), String> {
    attach_as(base, view, path, path, write)
}

/// Attaches the directory or file at `path` beneath `base` at `at` in
/// `view`, read-only unless `write`.
fn attach_as(base: &Dir, view: &Dir, path: &str, at: &str, write: bool) -> Result<(), String> {
    let rights = if write { GIVEN } else { READ_ONLY };
    let slot = match base.open_dir(path, rights) {
        Ok(dir) => Held::Dir(dir),
        Err(err) if err.is(FileError::NotDirectory) => {
            let file = base
                .open(path, libradix::abi::directory::OpenFlags::NONE, rights)
                .map_err(|err| format!("/{path}: {err}"))?;
            Held::File(file)
        }
        Err(err) => return Err(format!("/{path}: {err}")),
    };
    view.attach(at, slot.slot()).map_err(|err| {
        if path == at {
            format!("cannot attach /{path}: {err}")
        } else {
            format!("cannot attach /{path} at /{at}: {err}")
        }
    })
}

/// A directory or a file, held while it is attached.
enum Held {
    Dir(Dir),
    File(File),
}

impl Held {
    fn slot(&self) -> u32 {
        match self {
            Held::Dir(dir) => dir.slot(),
            Held::File(file) => file.slot(),
        }
    }
}

/// Copies the symbolic links of the directory `at`, which is `dir` of the
/// root, into the view, where nothing has their names yet.
fn copy_links(at: &Dir, view: &Dir, dir: &str) {
    let Ok(entries) = at.entries() else { return };
    for entry in entries.iter().filter(|e| e.kind == FileKind::Symlink) {
        let Ok(target) = at.read_link(&entry.name) else { continue };
        let Ok(target) = String::from_utf8(target) else { continue };
        let path =
            if dir.is_empty() { entry.name.clone() } else { format!("{dir}/{}", entry.name) };
        // A name the view has already keeps what it has, as /system/bin
        // where a file names /system whole.
        let _ = view.attach_link(&path, &target);
    }
}

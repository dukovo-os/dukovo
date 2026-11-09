// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! reach(1) (ADR 0443): who can reach a file, which `ls --grants` shows.
//!
//! Two answers for each file named:
//!
//! - **Accounts.** Each account that may read or change the file, by the
//!   owners, modes and access lists (ADR 0256) of the file and of every
//!   folder on the way to it, as the kernel checks them (radix-vfs's
//!   `Who::check`, mirrored in [`may`]): an account must be let through
//!   each folder above the file before the file's own permissions count.
//!   When an account with no tie to the file may, the answer says
//!   "everyone" once and names only the accounts that may do more.
//!   Administrators pass every check through `elevate`, so they are
//!   named on a line of their own.
//! - **Your apps.** Each app of the caller's, installed for them or for
//!   everyone, that would reach the file when they run it: by what its
//!   sandbox attaches (ADR 0225) from what they allowed it (ADR 0224),
//!   which bundled(8) tells them as it tells `bundle run`, and then by
//!   their own account's permissions below the folder attached, as the
//!   sandbox acts for them. A command that runs with their access
//!   reaches what they do.
//!
//! A file in a person's own `/tmp` (ADR 0255) is in no one else's view, so
//! only that person's account reaches it, whatever its modes say.
//!
//! It errs toward saying more can reach a file, never less: each program
//! of a bundle counts with every permission it may be given, and the
//! folders above a folder the sandbox attaches are not asked to let the
//! app through, as it never passes them. What it cannot know it
//! says it does not count: a program running now that was given a
//! permission once, other people's apps (which reach no more than their
//! accounts, and whose permissions are theirs to see), and the file's
//! other names, if it has hard links, in folders that may be open to
//! more.
//!
//! If an answer looks wrong, start with [`Way::can`] for accounts and
//! [`app_reach`] for apps.

use std::ffi::{CString, OsString};
use std::io::{self, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use accounts::{Group, Table, User};
use manifest::bundle::{Manifest, Need, Request as Asked, RunsWith};

use crate::args::{Arg, Args};
use crate::bundled::{self, APP_DATA};
use crate::bundles::grants::{self, Grant, Step, YOUR_ACCESS};
use crate::bundles::protocol::{Answer, Find, Found, Request};
use crate::bundles::registry::Place;
use crate::bundles::run::{self, Given};
use crate::bundles::{self, sheet};
use crate::signin;
use crate::util::{self, Fail};

/// The permission bits as the kernel's `want` gives them.
const READ: u32 = 4;
const WRITE: u32 = 2;
const SEARCH: u32 = 1;

/// The widest a name is shown, in characters.
const NAME_WIDTH: usize = 24;

/// What may be done to a file: read it, change it, or both. For a folder,
/// reading is listing its names, and changing is adding and removing
/// them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Can {
    read: bool,
    write: bool,
}

impl Can {
    const NOTHING: Can = Can { read: false, write: false };

    fn any(self) -> bool {
        self.read || self.write
    }

    fn and(self, other: Can) -> Can {
        Can { read: self.read && other.read, write: self.write && other.write }
    }

    /// Whether it lets do everything `other` does, and more.
    fn more_than(self, other: Can) -> bool {
        self != other && (self.read || !other.read) && (self.write || !other.write)
    }

    /// How many things it lets do, to choose the most.
    fn count(self) -> u8 {
        u8::from(self.read) + u8::from(self.write)
    }

    fn words(self) -> &'static str {
        match (self.read, self.write) {
            (true, true) => "read and write",
            (true, false) => "read",
            (false, true) => "write",
            (false, false) => "nothing",
        }
    }
}

/// A file or folder, as the kernel checks who may use it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Node {
    owner: u32,
    group: u32,
    /// Its permission bits.
    mode: u32,
    /// Its access list, if it has one.
    list: Option<acl::Acl>,
    folder: bool,
}

impl Node {
    fn read(path: &Path) -> io::Result<Node> {
        let meta = std::fs::symlink_metadata(path)?;
        let list =
            acl::read_attribute(path, acl::ACCESS, true)?.and_then(|v| acl::Acl::from_value(&v));
        Ok(Node {
            owner: meta.uid(),
            group: meta.gid(),
            mode: meta.mode() & 0o777,
            list,
            folder: meta.is_dir(),
        })
    }
}

/// Who is checked: an account's number, its group, and the other groups
/// it is in.
#[derive(Clone, Debug)]
struct Who {
    user: u32,
    group: u32,
    groups: Vec<u32>,
}

impl Who {
    /// The account `user`, in the groups `groups` says it is in, as its
    /// sign-in puts it (initgroups(3)).
    fn of(user: &User, groups: &Table<Group>) -> Who {
        Who {
            user: user.uid,
            group: user.gid,
            groups: accounts::other_groups(user, groups, usize::MAX),
        }
    }

    /// One with no tie to any file: no account's number or group's.
    fn anyone() -> Who {
        Who { user: u32::MAX - 1, group: u32::MAX - 1, groups: Vec::new() }
    }

    fn in_group(&self, group: u32) -> bool {
        self.group == group || self.groups.contains(&group)
    }
}

/// Whether `who` may do `want` (the [`READ`], [`WRITE`] and [`SEARCH`]
/// bits) to `node`, as the kernel decides it without the bypass:
/// radix-vfs's `Who::check`. The access list counts for one who does not
/// own the node, when the mode's group bits are not all clear; otherwise
/// the owner's bits for its owner, the group's for one in its group, and
/// the others' for the rest.
fn may(who: &Who, node: &Node, want: u32) -> bool {
    if node.owner != who.user
        && node.mode & 0o070 != 0
        && let Some(list) = &node.list
    {
        return permits(list, who, node, want);
    }
    let shift = if node.owner == who.user {
        6
    } else if who.in_group(node.group) {
        3
    } else {
        0
    };
    (node.mode >> shift) & want == want
}

/// Whether access list `list` of `node` lets `who` do `want`, as
/// radix-vfs's `Acl::permits` reads it, which is Linux's
/// `posix_acl_permission`: the first entry that names `who` decides,
/// limited by the mask, which the mode's group bits hold; one of `who`'s
/// groups that does not give all of `want` lets the next try, and
/// `others` counts only when no group named them.
fn permits(list: &acl::Acl, who: &Who, node: &Node, want: u32) -> bool {
    use acl::Tag;
    let want = want & 7;
    let mode = node.mode;
    let has_mask = list.entries.iter().any(|e| e.tag == Tag::Mask);
    let mask = if has_mask { (mode >> 3) & 7 } else { 7 };
    let mut in_a_group = false;
    for entry in &list.entries {
        let perm = u32::from(entry.perm);
        match entry.tag {
            Tag::UserObj if node.owner == who.user => return (mode >> 6) & want == want,
            Tag::User if entry.id == who.user => return perm & mask & want == want,
            Tag::GroupObj if who.in_group(node.group) => {
                in_a_group = true;
                // Without a mask, the group's entry is the mode's.
                let perm = if has_mask { perm } else { (mode >> 3) & 7 };
                if perm & want == want {
                    return perm & mask & want == want;
                }
            }
            Tag::Group if who.in_group(entry.id) => {
                in_a_group = true;
                if perm & want == want {
                    return perm & mask & want == want;
                }
            }
            Tag::Other => return !in_a_group && mode & want == want,
            _ => {}
        }
    }
    false
}

/// The way to a file: the root, each folder beneath it in turn, then the
/// file, with what is true of the file wherever it is reached from.
#[derive(Clone, Debug)]
struct Way {
    /// The file's real path, with no link or `..` in it.
    path: PathBuf,
    /// `/`, each folder on the way, then the file.
    nodes: Vec<Node>,
    /// Whether its filesystem or subvolume takes no changes (`EROFS`).
    read_only: bool,
    /// How many names it has.
    links: u64,
}

impl Way {
    /// The way to `path`, a real path.
    fn to(path: &Path) -> io::Result<Way> {
        let mut nodes = Vec::new();
        let mut at = PathBuf::new();
        for part in path.components() {
            at.push(part);
            nodes.push(Node::read(&at)?);
        }
        let links = std::fs::symlink_metadata(path)?.nlink();
        Ok(Way { path: path.to_path_buf(), nodes, read_only: refuses_writes(path), links })
    }

    /// What `who` may do to the file, reaching it from the folder at
    /// `from` in [`Way::nodes`] (0 for the root): each folder from there
    /// down must let them search it.
    fn can(&self, who: &Who, from: usize) -> Can {
        let Some((file, folders)) = self.nodes.split_last() else { return Can::NOTHING };
        if folders.iter().skip(from).any(|folder| !may(who, folder, SEARCH)) {
            return Can::NOTHING;
        }
        // Changing a folder's names needs searching it too.
        let change = if file.folder { WRITE | SEARCH } else { WRITE };
        Can { read: may(who, file, READ), write: !self.read_only && may(who, file, change) }
    }

    /// Where in [`Way::nodes`] `folder` is, if the file is it or is in it.
    fn depth_of(&self, folder: &Path) -> Option<usize> {
        self.path.starts_with(folder).then(|| folder.components().count().saturating_sub(1))
    }
}

/// Whether nothing at `path` may change, whoever asks: `access(2)` says
/// `EROFS` before it looks at who may write.
fn refuses_writes(path: &Path) -> bool {
    let Ok(c) = CString::new(path.as_os_str().as_bytes()) else { return false };
    // SAFETY: `c` is a C string.
    let status = unsafe { libc::access(c.as_ptr(), libc::W_OK) };
    status != 0 && io::Error::last_os_error().raw_os_error() == Some(libc::EROFS)
}

/// `text` safe to write to a terminal: a control character, or one that
/// turns the text's direction or takes no room, shows as `?`, so that no
/// file's name or app's name can move the cursor or make a line read as
/// another.
fn printable(text: &str) -> String {
    let unsafe_char = |c: char| {
        c.is_control()
            || matches!(c, '\u{061C}' | '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}')
            || matches!(c, '\u{2060}'..='\u{2069}' | '\u{FEFF}')
    };
    text.chars().map(|c| if unsafe_char(c) { '?' } else { c }).collect()
}

/// A line of the answer: who, what they may do, and how.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Line {
    who: String,
    can: Can,
    how: String,
}

/// Whose own `/tmp` the file is in, if it is in one (ADR 0255): each
/// person's sessions hold theirs, [`signin::TMP`]`/NUMBER`, at `/tmp`, and
/// no one else's sessions hold it. Gives the account whose it is, or none
/// for the system's `/tmp`, which root's sessions hold and no person's;
/// and where in [`Way::nodes`] the account reaches it from. None for a
/// file every session holds at its path.
fn own_tmp(way: &Way, caller: u32) -> Option<(Option<u32>, usize)> {
    if way.path.starts_with("/tmp") {
        // The caller's own /tmp, or root's, the system's.
        return Some(((caller != 0).then_some(caller), 1));
    }
    let rest = way.path.strip_prefix(signin::TMP).ok()?;
    let number = rest.components().next()?.as_os_str().to_str()?;
    let depth = way.depth_of(&Path::new(signin::TMP).join(number))?;
    Some((Some(number.parse().ok()?), depth))
}

/// The accounts that reach the file, as `caller` sees it, and what anyone
/// may do. The owner comes first, even when it may do nothing now, since
/// it may change that; then each account that may do more than anyone,
/// by number.
fn accounts_reaching(
    way: &Way,
    users: &Table<User>,
    groups: &Table<Group>,
    caller: u32,
) -> (Vec<Line>, Can) {
    // In a person's own /tmp, only that person reaches it, from there.
    let held = own_tmp(way, caller);
    let can = |who: &Who| match held {
        None => way.can(who, 0),
        Some((whose, from)) if whose == Some(who.user) => way.can(who, from),
        Some(_) => Can::NOTHING,
    };
    let everyone = can(&Who::anyone());
    let Some(file) = way.nodes.last() else { return (Vec::new(), everyone) };
    let mut users: Vec<&User> = users.entries().filter(|u| u.uid != 0).collect();
    users.sort_by_key(|u| (u.uid != file.owner, u.uid));
    let mut lines = Vec::new();
    for user in users {
        let who = Who::of(user, groups);
        let can = can(&who);
        let owner = user.uid == file.owner;
        if !owner && !can.more_than(everyone) {
            continue;
        }
        lines.push(Line { who: user.name.clone(), can, how: tie(file, &who, groups) });
    }
    (lines, everyone)
}

/// What ties `who` to `file`: owning it, being named in its access list,
/// or its group.
fn tie(file: &Node, who: &Who, groups: &Table<Group>) -> String {
    let named = |tag: acl::Tag, id: u32| {
        file.list.as_ref().is_some_and(|l| l.entries.iter().any(|e| e.tag == tag && e.id == id))
    };
    let group_name = |gid: u32| {
        groups.entries().find(|g| g.gid == gid).map_or_else(|| gid.to_string(), |g| g.name.clone())
    };
    if file.owner == who.user {
        "owner".into()
    } else if named(acl::Tag::User, who.user) {
        "its access list".into()
    } else if who.in_group(file.group) {
        format!("group {}", group_name(file.group))
    } else if who.groups.iter().chain([&who.group]).any(|&g| named(acl::Tag::Group, g)) {
        "its access list".into()
    } else {
        String::new()
    }
}

/// A folder an app's sandbox attaches: where, whether it may change what
/// is there, why, and the permission that gives it, if one does.
struct Attached {
    path: String,
    write: bool,
    why: String,
    permission: Option<&'static str>,
}

/// What a sandboxed program given `given` has attached for the person
/// whose home is `home`, besides what every app has ([`run::ALWAYS`]):
/// as the runner's `build_and_start` attaches it, which this follows
/// place for place. Its keyring's folder is left out: it holds a socket
/// the runner serves, and none of the person's files.
fn attached(given: &Given, home: &str, id: &str, bundle: &str) -> Vec<Attached> {
    let at = |path: String, write: bool, why: String, permission: Option<&'static str>| Attached {
        path,
        write,
        why,
        permission,
    };
    let mut all = Vec::new();
    if given.outbound || given.local {
        for path in run::NETWORK {
            all.push(at(path.to_string(), false, "as it connects out".into(), None));
        }
    }
    if given.local {
        all.push(at(run::DISCOVERY.into(), false, "as it finds what is nearby".into(), None));
    }
    if given.home {
        all.push(at(home.into(), true, "all of your files".into(), Some("files.home")));
    } else {
        for (folder, write) in &given.folders {
            let why = format!("your {folder} folder");
            all.push(at(format!("{home}/{folder}"), *write, why, Some("files.folder")));
        }
        for app in given.app_data.iter().filter(|app| *app != id) {
            let why = format!("{app}'s data");
            all.push(at(format!("{home}/{APP_DATA}/{app}"), false, why, Some("files.app-data")));
        }
        all.push(at(format!("{home}/{APP_DATA}/{id}"), true, "its own data".into(), None));
    }
    if given.volumes {
        all.push(at(run::VOLUMES.into(), true, "drives you plug in".into(), Some("files.volumes")));
    }
    for (folder, write) in &given.shared {
        let path = format!("{}/{folder}", run::SHARED);
        all.push(at(path.clone(), *write, path, Some("files.shared")));
    }
    if given.serial {
        for port in run::SERIAL {
            // Each serial port's terminal, by the start of its name.
            all.push(at(
                format!("/devices/{port}"),
                true,
                "serial ports".into(),
                Some("devices.serial"),
            ));
        }
    }
    all.push(at(bundle.into(), false, "its own files".into(), None));
    all.push(at(format!("/tmp/.app-{id}"), true, "its /tmp".into(), None));
    all
}

/// Whether `path` is or is in `folder`; for a serial port's start of a
/// name, whether it is such a port.
fn holds(folder: &str, path: &Path) -> bool {
    if let Some(start) = folder.strip_prefix("/devices/tty") {
        let name = path.strip_prefix("/devices").ok().and_then(|p| p.to_str());
        return name.is_some_and(|n| n.starts_with(&format!("tty{start}")) && !n.contains('/'));
    }
    path.starts_with(folder)
}

/// What one app reaches of the file, and why: the most any of its
/// programs reaches, by what the person said of its permissions
/// (`said`), checked as the person (`who`, whose home is `home`); none if
/// it reaches nothing.
fn app_reach(
    way: &Way,
    manifest: &Manifest,
    bundle: &str,
    said: &[Grant],
    home: &str,
    who: &Who,
) -> Option<(Can, String, Option<u64>)> {
    let id = manifest.full_id();
    let since =
        |permission: &str| said.iter().find(|g| g.permission == permission).map(|g| g.since);
    let mut best: Option<(Can, String, Option<u64>)> = None;
    let mut consider = |can: Can, why: String, when: Option<u64>| {
        if can.any() && best.as_ref().is_none_or(|(b, _, _)| can.count() > b.count()) {
            best = Some((can, why, when));
        }
    };

    // A command that runs with your access reaches what you do, once
    // you allowed it.
    if manifest.commands.iter().any(|c| c.runs_with == RunsWith::YourAccess) {
        let request = Asked {
            need: Need::Required,
            why: None,
            folders: Vec::new(),
            ports: Vec::new(),
            apps: Vec::new(),
        };
        let grant = said.iter().find(|g| g.permission == YOUR_ACCESS);
        if let Ok(Step::Give(_)) = grants::step(&id, YOUR_ACCESS, &request, grant) {
            consider(way.can(who, 0), "runs with your access".into(), since(YOUR_ACCESS));
        }
    }

    // Each program in its sandbox, with every permission it may be given
    // now: those given at install and those the person allowed.
    for (program, _) in manifest.programs.iter() {
        let program = program.to_string();
        let mut given = Given::default();
        for (name, request) in run::kept(manifest, &program) {
            let grant = said.iter().find(|g| g.permission == name);
            if let Ok(Step::Give(words)) = grants::step(&id, name, request, grant) {
                run::give(&mut given, name, request, &words);
            }
        }
        for folder in attached(&given, home, &id, bundle) {
            if !holds(&folder.path, &way.path) {
                continue;
            }
            // Attached as the folder itself, so the folders above it
            // are not searched; a serial port is attached by itself.
            let from = way.depth_of(Path::new(&folder.path)).unwrap_or(way.nodes.len() - 1);
            let view = Can { read: true, write: folder.write };
            let when = folder.permission.and_then(since);
            consider(view.and(way.can(who, from)), folder.why, when);
        }
    }
    best
}

/// What every app of the person's reaches of the file, from what each
/// one's sandbox has whatever it is given ([`run::ALWAYS`]).
fn every_app_reach(way: &Way, who: &Who) -> Option<(Can, String)> {
    run::ALWAYS.iter().find_map(|&(folder, write)| {
        let depth = way.depth_of(Path::new(folder))?;
        let can = Can { read: true, write }.and(way.can(who, depth));
        can.any().then(|| (can, format!("{folder}, which every app sees")))
    })
}

/// The person's apps that reach the file, each as a line; or why
/// bundled could not say.
fn apps_reaching(way: &Way, person: &User, who: &Who) -> Result<Vec<Line>, String> {
    let (_stream, mut input) = bundles::ask(&Request::List, None)?;
    let mut ids: Vec<(Place, String)> = Vec::new();
    loop {
        match bundles::answer(&mut input).map_err(|f| f.message)? {
            Answer::Listed(listed) => ids.push((listed.place, listed.id)),
            Answer::Ok(_) => break,
            Answer::Error(why) => return Err(why),
            _ => return Err("bundled answered out of turn".into()),
        }
    }
    // Their own first: an ID installed for them and for everyone runs as
    // theirs.
    ids.sort_by_key(|(place, id)| (id.clone(), *place == Place::Everyone));
    ids.dedup_by(|b, a| a.1 == b.1);

    let home = person.home.trim_end_matches('/');
    let mut lines = Vec::new();
    for (_, id) in ids {
        let Some((found, said)) = find(&id)? else { continue };
        // One that is not sealed does not run.
        let Ok(sealed) = run::open_sealed(&found) else { continue };
        let manifest = &sealed.manifest;
        if let Some((can, why, since)) = app_reach(way, manifest, &sealed.path, &said, home, who) {
            let how = match since {
                Some(when) => {
                    let day = util::time_text(i64::try_from(when).unwrap_or(0), "%Y-%m-%d", true);
                    format!("{why}, allowed {day}")
                }
                None => why,
            };
            let name = printable(&sheet::shown(&manifest.name.0, NAME_WIDTH));
            lines.push(Line { who: name, can, how: printable(&how) });
        }
    }
    Ok(lines)
}

/// The bundle `id` as bundled finds it to run, with what the person said
/// of its permissions; none if it has gone since it was listed.
fn find(id: &str) -> Result<Option<(Found, Vec<Grant>)>, String> {
    let (_stream, mut input) = bundles::ask(&Request::Run { find: Find::Id(id.into()) }, None)?;
    let mut found = None;
    let mut said = Vec::new();
    loop {
        match bundles::answer(&mut input).map_err(|f| f.message)? {
            Answer::Found(f) => found = Some(f),
            Answer::Granted(grant) => said.push(grant),
            Answer::Ok(_) => return Ok(found.map(|f| (f, said))),
            Answer::Error(_) => return Ok(None),
            _ => return Err("bundled answered out of turn".into()),
        }
    }
}

/// Writes `lines` as columns: who, what they may do, how.
fn write_lines(out: &mut impl Write, lines: &[Line], width: usize) -> io::Result<()> {
    for line in lines {
        let how = if line.how.is_empty() { String::new() } else { format!("  {}", line.how) };
        let what = format!("{:<14}", line.can.words());
        writeln!(
            out,
            "    {:<width$}  {}{how}",
            line.who,
            if how.is_empty() { what.trim_end() } else { &what }
        )?;
    }
    Ok(())
}

/// Writes who reaches the file `named`, whose real path is `way`'s.
fn answer(
    out: &mut impl Write,
    named: &OsString,
    way: &Way,
    users: &Table<User>,
    groups: &Table<Group>,
    caller: u32,
    person: Option<&User>,
) -> io::Result<()> {
    let shown = printable(&named.to_string_lossy());
    if Path::new(named) == way.path {
        writeln!(out, "{shown}")?;
    } else {
        writeln!(out, "{shown} ({})", printable(&way.path.to_string_lossy()))?;
    }

    let (mut accounts, everyone) = accounts_reaching(way, users, groups, caller);
    if everyone.any() {
        accounts.push(Line { who: "everyone".into(), can: everyone, how: String::new() });
    }
    let who = person.map(|p| Who::of(p, groups));
    let apps = match (person, &who) {
        (Some(person), Some(who)) => apps_reaching(way, person, who),
        _ => Err("you have no account".into()),
    };
    let every_app = who.as_ref().and_then(|who| every_app_reach(way, who));
    let mut app_lines = apps.as_ref().map_or_else(|_| Vec::new(), Clone::clone);
    if let Some((can, how)) = every_app {
        app_lines.insert(0, Line { who: "every app".into(), can, how });
    }
    let width = accounts.iter().chain(&app_lines).map(|l| l.who.chars().count()).max().unwrap_or(0);

    writeln!(out, "  Accounts:")?;
    write_lines(out, &accounts, width)?;
    let admins: Vec<&str> = users
        .entries()
        .filter(|u| u.uid != 0 && accounts::is_admin(u, groups))
        .map(|u| u.name.as_str())
        .collect();
    if admins.is_empty() {
        writeln!(out, "    root reaches every file.")?;
    } else {
        writeln!(out, "    Administrators reach every file with elevate: {}.", admins.join(", "))?;
    }

    writeln!(out, "  Your apps:")?;
    match &apps {
        Ok(_) if app_lines.is_empty() => writeln!(out, "    None of them reaches it.")?,
        Ok(_) => write_lines(out, &app_lines, width)?,
        Err(why) => {
            write_lines(out, &app_lines, width)?;
            writeln!(out, "    Not known: {why}.")?;
        }
    }

    match own_tmp(way, caller) {
        Some((Some(whose), _)) => {
            let name = users
                .entries()
                .find(|u| u.uid == whose)
                .map_or_else(|| whose.to_string(), |u| u.name.clone());
            writeln!(out, "  It is in {name}'s own /tmp, which no one else's sessions hold.")?;
        }
        Some((None, _)) => {
            writeln!(out, "  It is in the system's /tmp, which no person's sessions hold.")?;
        }
        None => {}
    }
    if way.read_only {
        writeln!(
            out,
            "  Nothing can change it: it is on a filesystem or subvolume kept read-only."
        )?;
    }
    if way.links > 1 && !way.nodes.last().is_some_and(|n| n.folder) {
        writeln!(out, "  It has {} names; another may be in a folder open to more.", way.links)?;
    }
    writeln!(out, "  An app reaches no more than the person who runs it; other people's apps are")?;
    writeln!(out, "  theirs to see.")
}

/// reach(1).
pub fn main(args: &mut Args) -> Result<i32, Fail> {
    let mut files = Vec::new();
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Operand(file) => files.push(file),
            other => return Err(args.unknown(&other)),
        }
    }
    if files.is_empty() {
        return Err(util::usage("name a file".into()));
    }
    let read = |path: &str| std::fs::read_to_string(path).map_err(|e| util::failed(&path, e));
    let users = accounts::users(&read(accounts::PASSWD)?);
    let groups = accounts::groups(&read(accounts::GROUP)?);
    // SAFETY: getuid has no failure and touches no memory.
    let uid = unsafe { libc::getuid() };
    let person = bundled::account(uid).ok();

    let mut out = io::stdout().lock();
    let mut code = 0;
    for (n, named) in files.iter().enumerate() {
        let way = std::fs::canonicalize(named).and_then(|path| Way::to(&path));
        match way {
            Ok(way) => {
                if n > 0 {
                    writeln!(out).map_err(|e| util::failed(&"the output", e))?;
                }
                answer(&mut out, named, &way, &users, &groups, uid, person.as_ref())
                    .map_err(|e| util::failed(&"the output", e))?;
            }
            Err(e) => {
                out.flush().map_err(|e| util::failed(&"the output", e))?;
                let shown = printable(&named.to_string_lossy());
                eprintln!("reach: {shown}: {}", util::strip_os_error(&e));
                code = 1;
            }
        }
    }
    Ok(code)
}

#[cfg(test)]
mod tests;

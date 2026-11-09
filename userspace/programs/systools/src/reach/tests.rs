// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! reach(1)'s rules, on ways and manifests made here: the kernel's
//! checks, the accounts it names and what an app reaches.

use super::*;
use crate::bundles::grants::Said;

/// A node of `owner` and `group` with `mode`, a folder or a file.
fn node(owner: u32, group: u32, mode: u32, folder: bool) -> Node {
    Node { owner, group, mode, list: None, folder }
}

fn who(user: u32, group: u32, groups: &[u32]) -> Who {
    Who { user, group, groups: groups.to_vec() }
}

/// The way to `path`: `/` and `/users` open to all, then `folders`,
/// then `file`.
fn way(path: &str, folders: &[Node], file: Node) -> Way {
    let mut nodes = vec![node(0, 0, 0o755, true), node(0, 0, 0o755, true)];
    nodes.extend_from_slice(folders);
    nodes.push(file);
    assert_eq!(nodes.len(), Path::new(path).components().count(), "{path}");
    Way { path: path.into(), nodes, read_only: false, links: 1 }
}

fn entry(tag: acl::Tag, id: u32, perm: u8) -> acl::Entry {
    acl::Entry { tag, id, perm }
}

#[test]
fn the_owner_group_and_others_have_their_bits() {
    let file = node(1000, 100, 0o640, false);
    assert!(may(&who(1000, 1000, &[]), &file, READ | WRITE));
    assert!(may(&who(1001, 100, &[]), &file, READ));
    assert!(!may(&who(1001, 100, &[]), &file, WRITE));
    // In the group as another of its groups.
    assert!(may(&who(1002, 1002, &[100]), &file, READ));
    assert!(!may(&who(1003, 1003, &[]), &file, READ));
    // The owner's bits decide for the owner, even when the others' give
    // more, as on Linux.
    let odd = node(1000, 100, 0o044, false);
    assert!(!may(&who(1000, 1000, &[]), &odd, READ));
}

#[test]
fn an_access_list_decides_as_the_kernels_does() {
    use acl::Tag;
    let list = acl::Acl {
        entries: vec![
            entry(Tag::UserObj, acl::NO_ID, 6),
            entry(Tag::User, 1001, 6),
            entry(Tag::GroupObj, acl::NO_ID, 4),
            entry(Tag::Group, 200, 6),
            // The mask, which the mode's group bits hold: read only.
            entry(Tag::Mask, acl::NO_ID, 4),
            entry(Tag::Other, acl::NO_ID, 0),
        ],
    };
    let file = Node { owner: 1000, group: 100, mode: 0o640, list: Some(list), folder: false };
    // Named: read and write, less the mask's write.
    assert!(may(&who(1001, 1001, &[]), &file, READ));
    assert!(!may(&who(1001, 1001, &[]), &file, WRITE));
    // A named group, the same.
    assert!(may(&who(1002, 1002, &[200]), &file, READ));
    assert!(!may(&who(1002, 1002, &[200]), &file, WRITE));
    // In a group that does not give it all: not the others' bits.
    assert!(may(&who(1003, 100, &[]), &file, READ));
    assert!(!may(&who(1004, 1004, &[]), &file, READ));
    // The owner has the mode's own bits.
    assert!(may(&who(1000, 1000, &[]), &file, READ | WRITE));

    // Without group bits the list is not read.
    let mut closed = file.clone();
    closed.mode = 0o600;
    assert!(!may(&who(1001, 1001, &[]), &closed, READ));
}

#[test]
fn every_folder_on_the_way_must_let_one_through() {
    let alice = who(1000, 1000, &[]);
    let bob = who(1001, 1001, &[]);
    // Her home is hers alone; the file in it is open to all.
    let home = node(1000, 1000, 0o700, true);
    let w =
        way("/users/alice/notes.txt", std::slice::from_ref(&home), node(1000, 1000, 0o644, false));
    assert_eq!(w.can(&alice, 0), Can { read: true, write: true });
    assert_eq!(w.can(&bob, 0), Can::NOTHING);
    assert_eq!(w.can(&Who::anyone(), 0), Can::NOTHING);
    // Her home attached as itself, as an app's view attaches a folder:
    // the home must still let one search it.
    assert_eq!(w.can(&bob, 2), Can::NOTHING);
    // The file attached as itself: only its own bits.
    assert_eq!(w.can(&bob, 3), Can { read: true, write: false });

    // A folder needs searching to change its names.
    let open = node(1000, 1000, 0o755, true);
    let folder = node(1000, 1000, 0o753, true);
    let w = way("/users/alice/drop", &[open], folder);
    assert_eq!(w.can(&Who::anyone(), 0), Can { read: false, write: true });
    let w = Way { read_only: true, ..w };
    assert_eq!(w.can(&alice, 0), Can { read: true, write: false });
}

#[test]
fn depth_is_by_whole_names() {
    let w = way("/users/alice/a", &[node(0, 0, 0o755, true)], node(0, 0, 0o644, false));
    assert_eq!(w.depth_of(Path::new("/")), Some(0));
    assert_eq!(w.depth_of(Path::new("/users/alice")), Some(2));
    assert_eq!(w.depth_of(Path::new("/users/alice/a")), Some(3));
    assert_eq!(w.depth_of(Path::new("/users/ali")), None);
    assert!(holds("/devices/ttyUSB", Path::new("/devices/ttyUSB0")));
    assert!(!holds("/devices/ttyUSB", Path::new("/devices/ttyS0")));
    assert!(!holds("/users/alice", Path::new("/users/alice2/x")));
}

fn tables() -> (Table<User>, Table<Group>) {
    let users = accounts::users(
        "root:x:0:0:root:/:/system/commands/nukesh\n\
         alice:x:1000:1000:Alice:/users/alice:/system/commands/nukesh\n\
         bob:x:1001:1001:Bob:/users/bob:/system/commands/nukesh\n\
         carol:x:1002:1002:Carol:/users/carol:/system/commands/nukesh\n\
         web:x:120:120:web service:/:/system/commands/false\n",
    );
    let groups = accounts::groups(
        "root:x:0:\nadmins:x:10:alice\nalice:x:1000:\nbob:x:1001:\ncarol:x:1002:\n\
         family:x:2000:bob\nweb:x:120:\n",
    );
    (users, groups)
}

#[test]
fn accounts_are_named_by_what_they_may_do_more_than_anyone() {
    let (users, groups) = tables();
    let home = node(1000, 1000, 0o711, true);
    // Hers; the family may read it, and everyone else nothing.
    let w = way("/users/alice/budget", std::slice::from_ref(&home), node(1000, 2000, 0o640, false));
    let (lines, everyone) = accounts_reaching(&w, &users, &groups, 1000);
    assert_eq!(everyone, Can::NOTHING);
    let named: Vec<(&str, &str, &str)> =
        lines.iter().map(|l| (l.who.as_str(), l.can.words(), l.how.as_str())).collect();
    assert_eq!(named, [("alice", "read and write", "owner"), ("bob", "read", "group family")]);

    // Open to all to read: everyone, and only her for more.
    let w = way("/users/alice/notes", std::slice::from_ref(&home), node(1000, 1000, 0o644, false));
    let (lines, everyone) = accounts_reaching(&w, &users, &groups, 1000);
    assert_eq!(everyone, Can { read: true, write: false });
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].who, "alice");

    // Root is not listed; the owner is, even with nothing.
    let w = way("/users/alice/locked", &[home], node(1000, 1000, 0o000, false));
    let (lines, _) = accounts_reaching(&w, &users, &groups, 1000);
    assert_eq!(lines.len(), 1);
    assert_eq!((lines[0].who.as_str(), lines[0].can), ("alice", Can::NOTHING));
}

#[test]
fn a_named_account_says_so() {
    use acl::Tag;
    let (users, groups) = tables();
    let list = acl::Acl {
        entries: vec![
            entry(Tag::UserObj, acl::NO_ID, 6),
            entry(Tag::User, 1002, 4),
            entry(Tag::GroupObj, acl::NO_ID, 0),
            entry(Tag::Mask, acl::NO_ID, 4),
            entry(Tag::Other, acl::NO_ID, 0),
        ],
    };
    let file = Node { owner: 1000, group: 1000, mode: 0o640, list: Some(list), folder: false };
    let w = way("/users/alice/shared", &[node(1000, 1000, 0o711, true)], file);
    let (lines, _) = accounts_reaching(&w, &users, &groups, 1000);
    let carol = lines.iter().find(|l| l.who == "carol").expect("carol is named");
    assert_eq!((carol.can.words(), carol.how.as_str()), ("read", "its access list"));
}

#[test]
fn a_persons_own_tmp_is_theirs_alone() {
    let (users, groups) = tables();
    // Open to all by its modes, in alice's own /tmp: only she reaches it.
    let sticky = node(1000, 1000, 0o1777, true);
    let note = Way {
        path: "/tmp/note".into(),
        nodes: vec![node(0, 0, 0o755, true), sticky.clone(), node(1000, 1000, 0o666, false)],
        read_only: false,
        links: 1,
    };
    let (lines, everyone) = accounts_reaching(&note, &users, &groups, 1000);
    assert_eq!(everyone, Can::NOTHING);
    let named: Vec<(&str, Can)> = lines.iter().map(|l| (l.who.as_str(), l.can)).collect();
    assert_eq!(named, [("alice", Can { read: true, write: true })]);
    // Root's /tmp is the system's: no person's sessions hold it.
    let (lines, everyone) = accounts_reaching(&note, &users, &groups, 0);
    assert_eq!(everyone, Can::NOTHING);
    assert!(lines.iter().all(|l| l.can == Can::NOTHING));

    // At its real path, as root sees it: bob's, through his /tmp, though
    // the folder above it is root's alone.
    let real = format!("{}/1001/note", signin::TMP);
    let mut nodes = vec![node(0, 0, 0o755, true); 4];
    nodes.push(node(0, 0, 0o700, true));
    nodes.push(sticky);
    nodes.push(node(1001, 1001, 0o644, false));
    let note = Way { path: real.into(), nodes, read_only: false, links: 1 };
    assert_eq!(own_tmp(&note, 0), Some((Some(1001), 5)));
    let (lines, everyone) = accounts_reaching(&note, &users, &groups, 0);
    assert_eq!(everyone, Can::NOTHING);
    let named: Vec<(&str, Can)> = lines.iter().map(|l| (l.who.as_str(), l.can)).collect();
    assert_eq!(named, [("bob", Can { read: true, write: true })]);
    // Elsewhere, every session holds it at its path.
    assert_eq!(own_tmp(&hers("/users/alice/x"), 1000), None);
}

fn manifest(permissions: &str, commands: &str) -> Manifest {
    let mut text = manifest::bundle::template("org.example.notes", "Notes", "Example", "MIT")
        .replace("\"permissions\": {}", &format!("\"permissions\": {permissions}"));
    if !commands.is_empty() {
        let at = text.find("\"commands\"").expect("the template has commands");
        let end = at + text[at..].find(']').expect("commands end") + 1;
        text.replace_range(at..end, &format!("\"commands\": {commands}"));
    }
    manifest::bundle::from_json(&text).unwrap_or_else(|e| panic!("{e:?}\n{text}"))
}

fn grant(permission: &str, said: Said, words: &[&str]) -> Grant {
    asked_and_given(permission, said, words, words)
}

/// A grant of what was `asked` for, which gives `given`.
fn asked_and_given(permission: &str, said: Said, asked: &[&str], given: &[&str]) -> Grant {
    let words = |w: &[&str]| w.iter().map(|w| (*w).to_string()).collect::<Vec<String>>();
    Grant {
        permission: permission.into(),
        said,
        since: 1_762_646_400,
        given: words(given),
        asked: words(asked),
    }
}

/// The way to a file of hers at `path` under her home, each folder and
/// the file hers and closed to others.
fn hers(path: &str) -> Way {
    let depth = Path::new(path).components().count();
    let folders: Vec<Node> = (2..depth - 1).map(|_| node(1000, 1000, 0o700, true)).collect();
    way(path, &folders, node(1000, 1000, 0o600, false))
}

const BUNDLE: &str = "/users/alice/applications/org.example.notes";

#[test]
fn an_app_reaches_the_folders_she_allowed() {
    let alice = who(1000, 1000, &[]);
    let asks = r#"{"files.folder": {"need": "required", "folders": {"documents": "read-write", "music": "read"}}}"#;
    let m = manifest(asks, "");
    let doc = hers("/users/alice/documents/taxes.pdf");
    // Not yet allowed: asked on first use, so nothing yet.
    assert_eq!(app_reach(&doc, &m, BUNDLE, &[], "/users/alice", &alice), None);
    // Allowed, with less than it asked for: documents to read.
    let asked = ["documents=read-write", "music=read"];
    let said = [asked_and_given("files.folder", Said::Allow, &asked, &["documents=read"])];
    let (can, why, since) = app_reach(&doc, &m, BUNDLE, &said, "/users/alice", &alice).unwrap();
    assert_eq!(
        (can, why.as_str(), since),
        (Can { read: true, write: false }, "your documents folder", Some(1_762_646_400))
    );
    // Not her music, which it was not given, nor another folder.
    let song = hers("/users/alice/music/song.flac");
    assert_eq!(app_reach(&song, &m, BUNDLE, &said, "/users/alice", &alice), None);
    let other = hers("/users/alice/pictures/cat.png");
    assert_eq!(app_reach(&other, &m, BUNDLE, &said, "/users/alice", &alice), None);
    // Not allowed at all: nothing.
    let never = [grant("files.folder", Said::Never, &["documents=read-write", "music=read"])];
    assert_eq!(app_reach(&doc, &m, BUNDLE, &never, "/users/alice", &alice), None);
}

#[test]
fn an_app_reaches_its_own_and_no_more_than_she_does() {
    let alice = who(1000, 1000, &[]);
    let m = manifest("{}", "");
    // Its own data and /tmp, to change; its own files, to read.
    let data = hers("/users/alice/app-data/org.example.notes/notes.db");
    let (can, why, _) = app_reach(&data, &m, BUNDLE, &[], "/users/alice", &alice).unwrap();
    assert_eq!((can, why.as_str()), (Can { read: true, write: true }, "its own data"));
    let other = hers("/users/alice/app-data/org.example.mail/inbox");
    assert_eq!(app_reach(&other, &m, BUNDLE, &[], "/users/alice", &alice), None);
    let tmp = way(
        "/tmp/.app-org.example.notes/x",
        &[node(1000, 1000, 0o700, true)],
        node(1000, 1000, 0o600, false),
    );
    assert_eq!(app_reach(&tmp, &m, BUNDLE, &[], "/users/alice", &alice).unwrap().1, "its /tmp");
    // Her file it may change, which she may only read: read.
    let read_only = way(
        "/users/alice/app-data/org.example.notes/kept",
        &[
            node(1000, 1000, 0o700, true),
            node(1000, 1000, 0o700, true),
            node(1000, 1000, 0o700, true),
        ],
        node(1000, 1000, 0o400, false),
    );
    let (can, _, _) = app_reach(&read_only, &m, BUNDLE, &[], "/users/alice", &alice).unwrap();
    assert_eq!(can, Can { read: true, write: false });
}

#[test]
fn all_of_your_files_and_your_access_reach_everything_of_yours() {
    let alice = who(1000, 1000, &[]);
    let doc = hers("/users/alice/documents/taxes.pdf");
    let home = manifest(r#"{"files.home": {"need": "required"}}"#, "");
    assert_eq!(app_reach(&doc, &home, BUNDLE, &[], "/users/alice", &alice), None);
    let said = [grant("files.home", Said::Allow, &[])];
    let (can, why, _) = app_reach(&doc, &home, BUNDLE, &said, "/users/alice", &alice).unwrap();
    assert_eq!((can, why.as_str()), (Can { read: true, write: true }, "all of your files"));

    // A command that runs with her access, once she allowed it.
    let commands = r#"[{"name": "notes", "program": "notes", "runs_with": "your-access"}]"#;
    let yours = manifest("{}", commands);
    assert_eq!(app_reach(&doc, &yours, BUNDLE, &[], "/users/alice", &alice), None);
    let said = [grant(YOUR_ACCESS, Said::Allow, &[])];
    let (can, why, _) = app_reach(&doc, &yours, BUNDLE, &said, "/users/alice", &alice).unwrap();
    assert_eq!((can, why.as_str()), (Can { read: true, write: true }, "runs with your access"));
}

#[test]
fn every_app_sees_the_systems_commands() {
    let alice = who(1000, 1000, &[]);
    let ls = way("/system/commands/ls", &[node(0, 0, 0o755, true)], node(0, 0, 0o555, false));
    // `/users` stands in for `/system` in [`way`]; the depth is the same.
    let ls = Way { path: "/system/commands/ls".into(), ..ls };
    let (can, why) = every_app_reach(&ls, &alice).unwrap();
    assert_eq!(
        (can, why.as_str()),
        (Can { read: true, write: false }, "/system/commands, which every app sees")
    );
    assert_eq!(every_app_reach(&hers("/users/alice/x"), &alice), None);
}

#[test]
fn names_cannot_write_to_the_terminal() {
    assert_eq!(printable("plan.txt"), "plan.txt");
    assert_eq!(printable("caf\u{e9} \u{1f4c4}"), "caf\u{e9} \u{1f4c4}");
    assert_eq!(printable("a\x1b[2Jb\nc"), "a?[2Jb?c");
    // Turning the text's direction, or taking no room.
    assert_eq!(printable("x\u{202e}txt.exe"), "x?txt.exe");
    assert_eq!(printable("No\u{200b}tes\u{2066}"), "No?tes?");
}

#[test]
fn the_answer_reads_as_columns() {
    let lines = [
        Line { who: "alice".into(), can: Can { read: true, write: true }, how: "owner".into() },
        Line { who: "everyone".into(), can: Can { read: true, write: false }, how: String::new() },
    ];
    let mut out = Vec::new();
    write_lines(&mut out, &lines, 8).unwrap();
    assert_eq!(
        String::from_utf8(out).unwrap(),
        "    alice     read and write  owner\n    everyone  read\n"
    );
}

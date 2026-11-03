// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

use super::request::{
    Answer, CONTINUE, Choice, Mode, PROMPT_SHOWN_MAX, Prompt, Prompted, Question, REQUEST_MAX,
    Relay, Request, SSH_COMMAND_MAX, SshProof, SshRun, ended, ended_status, signal, stopped,
    stopped_signal,
};
use super::*;

const PASSWD_TEXT: &str = "root:x:0:0:root:/:/system/commands/nukesh
# a note an administrator left
alice:x:1000:1000:Alice Liddell:/users/alice:/system/commands/nukesh
broken line
";

#[test]
fn files_keep_what_they_do_not_read() {
    let mut users = users(PASSWD_TEXT);
    assert_eq!(users.entries().count(), 2);
    let alice = users.entries().find(|u| u.name == "alice").unwrap();
    assert_eq!((alice.uid, alice.gid, alice.full_name.as_str()), (1000, 1000, "Alice Liddell"));
    assert!(alice.is_person() && !users.entries().next().unwrap().is_person());
    users.push(User {
        name: "bob".into(),
        uid: 1001,
        gid: 1001,
        full_name: String::new(),
        home: "/users/bob".into(),
        shell: SHELL.into(),
    });
    assert_eq!(
        users.text(),
        format!("{PASSWD_TEXT}bob:x:1001:1001::/users/bob:/system/commands/nukesh\n")
    );
}

#[test]
fn groups_and_administrators() {
    let mut groups = groups("root:x:0:\nadmins:x:10:alice,carol\nbob:x:1001:\n");
    let group = |name: &str| groups.entries().find(|g| g.name == name).cloned().unwrap();
    assert_eq!(group("admins").members, ["alice", "carol"]);
    assert!(group("bob").members.is_empty());
    let user = |name: &str, uid| User {
        name: name.into(),
        uid,
        gid: uid,
        full_name: String::new(),
        home: String::new(),
        shell: String::new(),
    };
    assert!(is_admin(&user("alice", 1000), &groups));
    assert!(is_admin(&user("root", 0), &groups));
    assert!(!is_admin(&user("bob", 1001), &groups));
    groups.entries_mut().find(|g| g.name == "admins").unwrap().members.push("bob".into());
    assert!(is_admin(&user("bob", 1001), &groups));
    assert!(groups.text().contains("admins:x:10:alice,carol,bob\n"));
}

#[test]
fn new_accounts_take_the_lowest_free_number() {
    let users = users("root:x:0:0::/:/bin/sh\na:x:1000:1000::/users/a:/bin/sh\n");
    let groups = groups("root:x:0:\na:x:1000:\nstaff:x:1001:\n");
    assert_eq!(free_number(&users, &groups, &[]), Some(1002));
    // A removed account's number is never given again.
    let retired = retired("1002:carol:2026-10-31\nbroken\n1003:dave:2026-10-31\n");
    assert_eq!(retired, [1002, 1003]);
    assert_eq!(free_number(&users, &groups, &retired), Some(1004));
}

#[test]
fn removing_accounts() {
    let mut users = users(
        "root:x:0:0::/:/bin/sh\nalice:x:1000:1000::/users/alice:/bin/sh\n\
         bob:x:1001:1001::/users/bob:/bin/sh\n",
    );
    let mut groups = groups("admins:x:10:alice,bob\nalice:x:1000:\nbob:x:1001:\nstaff:x:50:bob\n");
    let bob = remove(&mut users, &mut groups, "bob").unwrap();
    assert_eq!((bob.uid, bob.home.as_str()), (1001, "/users/bob"));
    assert_eq!(users.entries().map(|u| u.name.as_str()).collect::<Vec<_>>(), ["root", "alice"]);
    assert_eq!(groups.text(), "admins:x:10:alice\nalice:x:1000:\nstaff:x:50:\n");
    assert!(remove(&mut users, &mut groups, "bob").unwrap_err().contains("no account bob"));
    assert_eq!(retired_line(&bob, "2026-10-31"), "1001:bob:2026-10-31\n");
}

#[test]
fn renaming_accounts() {
    let mut users =
        users("alice:x:1000:1000:Alice:/users/alice:/bin/sh\nbob:x:1001:1001::/data/bob:/bin/sh\n");
    let mut groups = groups("admins:x:10:alice\nalice:x:1000:\nbob:x:1001:alice\n");
    let (before, after) = rename(&mut users, &mut groups, "alice", "ally").unwrap();
    assert_eq!((before.home.as_str(), after.home.as_str()), ("/users/alice", "/users/ally"));
    assert_eq!((after.uid, after.full_name.as_str()), (1000, "Alice"));
    assert_eq!(groups.text(), "admins:x:10:ally\nally:x:1000:\nbob:x:1001:ally\n");
    // A home elsewhere stays where it is.
    let (_, bob) = rename(&mut users, &mut groups, "bob", "robert").unwrap();
    assert_eq!(bob.home, "/data/bob");
    for (name, new, why) in [
        ("ally", "robert", "an account robert"),
        ("ally", "admins", "a group admins"),
        ("ally", "Ally", "cannot be a name"),
        ("carol", "caz", "no account carol"),
    ] {
        assert!(rename(&mut users, &mut groups, name, new).unwrap_err().contains(why), "{new}");
    }
}

#[test]
fn administrators_come_and_go() {
    let mut groups = groups("alice:x:1000:\n");
    set_admin(&mut groups, "alice", false).unwrap();
    assert_eq!(groups.text(), "alice:x:1000:\n");
    set_admin(&mut groups, "alice", true).unwrap();
    set_admin(&mut groups, "alice", true).unwrap();
    set_admin(&mut groups, "bob", true).unwrap();
    assert_eq!(groups.text(), "alice:x:1000:\nadmins:x:10:alice,bob\n");
    set_admin(&mut groups, "alice", false).unwrap();
    assert_eq!(groups.text(), "alice:x:1000:\nadmins:x:10:bob\n");
    let mut taken = self::groups("staff:x:10:\n");
    assert!(set_admin(&mut taken, "alice", true).unwrap_err().contains("another group"));
}

#[test]
fn adding_accounts() {
    let mut users = users("root:x:0:0:root:/:/system/commands/nukesh\n");
    let mut groups = groups("root:x:0:\n");
    let alice = add(&mut users, &mut groups, "alice", "Alice Liddell", true, &[]).unwrap();
    assert_eq!((alice.uid, alice.gid, alice.home.as_str()), (1000, 1000, "/users/alice"));
    let bob = add(&mut users, &mut groups, "bob", "", false, &[]).unwrap();
    assert_eq!(bob.uid, 1001);
    assert!(is_admin(&alice, &groups) && !is_admin(&bob, &groups));
    assert_eq!(groups.text(), "root:x:0:\nadmins:x:10:alice\nalice:x:1000:\nbob:x:1001:\n");
    assert!(users.text().ends_with(
        "alice:x:1000:1000:Alice Liddell:/users/alice:/system/commands/nukesh\n\
         bob:x:1001:1001::/users/bob:/system/commands/nukesh\n"
    ));
    for (name, full_name) in
        [("alice", ""), ("root", ""), ("admins", ""), ("Eve", ""), ("eve", "a:b")]
    {
        assert!(add(&mut users, &mut groups, name, full_name, false, &[]).is_err(), "{name}");
    }
    // The administrators' number taken by another group: no account.
    let mut groups = self::groups("staff:x:10:\n");
    assert!(
        add(&mut users, &mut groups, "carol", "", true, &[]).unwrap_err().contains("another group")
    );
}

#[test]
fn services_get_accounts_of_their_own() {
    let mut users = users("root:x:0:0:root:/:/system/commands/nukesh\n");
    let mut groups = groups("root:x:0:\nstaff:x:100:\n");
    let home = "/shared/services/org.example.web";
    // The lowest free number from 100, past a group's and a retired one.
    let web = add_service(&mut users, &mut groups, "web", "org.example.web", home, &[101]).unwrap();
    assert_eq!((web.uid, web.gid, web.shell.as_str()), (102, 102, NO_SHELL));
    assert!(web.is_service() && !web.is_person());
    assert!(users.text().ends_with(
        "web:x:102:102:org.example.web:/shared/services/org.example.web:/system/commands/false\n"
    ));
    // Again, the same: kept; another home, or a person's name: refused.
    let again = add_service(&mut users, &mut groups, "web", "org.example.web", home, &[]).unwrap();
    assert_eq!(again, web);
    assert!(add_service(&mut users, &mut groups, "web", "x", "/shared/services/x", &[]).is_err());
    add(&mut users, &mut groups, "alice", "", false, &[]).unwrap();
    assert!(add_service(&mut users, &mut groups, "alice", "x", home, &[]).is_err());
    assert_eq!(add(&mut users, &mut groups, "bob", "", false, &[]).unwrap().uid, 1001);
}

#[test]
fn password_lines() {
    let table = passwords("alice:$argon2id$v=19$x:\nbob::\ncarol:!:19000:0:99999:7:::\n");
    let hashes: Vec<(&str, &str)> =
        table.entries().map(|p| (p.name.as_str(), p.hash.as_str())).collect();
    assert_eq!(hashes, [("alice", "$argon2id$v=19$x"), ("bob", ""), ("carol", "!")]);
    // Fields after the hash, as /etc/shadow's, are not kept: authd
    // writes none.
    assert_eq!(table.text(), "alice:$argon2id$v=19$x:\nbob::\ncarol:!:\n");
    // A disabled account keeps its password behind the mark.
    let mut alice = table.entries().next().unwrap().clone();
    assert!(!alice.is_disabled());
    alice.set_disabled(true);
    alice.set_disabled(true);
    assert_eq!((alice.hash.as_str(), alice.own_hash()), ("!$argon2id$v=19$x", "$argon2id$v=19$x"));
    assert!(alice.is_disabled());
    alice.set_disabled(false);
    assert_eq!(alice.hash, "$argon2id$v=19$x");
    let carol = table.entries().nth(2).unwrap();
    assert!(carol.is_disabled() && carol.own_hash().is_empty());
}

#[test]
fn names() {
    for good in ["alice", "_build", "x", "mary-jane", "j.r", "user2"] {
        assert_eq!(check_name(good), Ok(()), "{good}");
    }
    for bad in ["", "Alice", "2fast", "-x", "a b", "a:b", "été", "a/b", &"a".repeat(33)] {
        assert!(check_name(bad).is_err(), "{bad}");
    }
    assert!(check_full_name("Alice Liddell, Room 2").is_ok());
    assert!(check_full_name("a:b").is_err() && check_full_name("a\nb").is_err());
}

#[test]
fn passwords_get_warnings_and_no_rules() {
    assert!(password_warning("alice", "").unwrap().contains("anyone at this machine"));
    assert!(password_warning("alice", "abc").unwrap().contains("3 characters"));
    assert_eq!(password_warning("alice", "correct horse"), None);
    // Characters, not bytes.
    assert!(password_warning("alice", "ééééééé").is_some());
    assert_eq!(password_warning("alice", "éééééééé"), None);
}

#[test]
fn requests_go_and_come_back() {
    let requests = [
        Request::Check { name: "alice".into(), password: b"s3cret".to_vec() },
        Request::Passwd { name: "alice".into(), current: Vec::new(), new: b"new".to_vec() },
        Request::Add {
            name: "bob".into(),
            full_name: "Bob Smith".into(),
            admin: true,
            password: b"\xffnot utf-8".to_vec(),
        },
        Request::NeedsOwner,
        Request::Owner { name: "chase".into(), full_name: String::new(), password: Vec::new() },
        Request::Reset { name: "alice".into(), password: b"queen of hearts".to_vec() },
        Request::Reset { name: "alice".into(), password: Vec::new() },
        Request::Session {
            name: "alice".into(),
            password: b"s3cret".to_vec(),
            term: "linux".into(),
        },
        Request::Elevate {
            mode: Mode::Command,
            password: None,
            ask: false,
            term: "linux".into(),
            directory: b"/users/alice".to_vec(),
            language: vec!["LANG=C.UTF-8".into()],
            command: vec![b"dmesg".to_vec(), b"--".to_vec(), b"\xff".to_vec()],
        },
        Request::Elevate {
            mode: Mode::Login,
            password: Some(Vec::new()),
            ask: false,
            term: String::new(),
            directory: b"/".to_vec(),
            language: Vec::new(),
            command: Vec::new(),
        },
        Request::Elevate {
            mode: Mode::Shell,
            password: None,
            ask: true,
            term: "linux".into(),
            directory: b"/".to_vec(),
            language: Vec::new(),
            command: Vec::new(),
        },
        Request::Unlock { password: Some(b"s3cret".to_vec()), ask: false },
        Request::Unlock { password: None, ask: false },
        Request::Unlock { password: None, ask: true },
        Request::Forget,
        Request::Prompts,
        Request::Prompted { id: 7, typed: Prompted::Password(b"s3cret".to_vec()) },
        Request::Prompted { id: 0, typed: Prompted::Password(Vec::new()) },
        Request::Prompted { id: u64::MAX, typed: Prompted::Cancelled },
        Request::Prompted { id: 8, typed: Prompted::Elsewhere },
        Request::Prompted { id: 9, typed: Prompted::Chose(Choice::Once) },
        Request::Prompted { id: 10, typed: Prompted::Chose(Choice::Always) },
        Request::Prompted { id: 11, typed: Prompted::Chose(Choice::Never) },
        Request::Choose {
            terminal: "pts/1".into(),
            name: "alice".into(),
            question: Question::Warning,
            text: "Notes asks to: use all of your files".into(),
        },
        Request::Disable { name: "bob".into() },
        Request::Enable { name: "bob".into() },
        Request::Remove { name: "bob".into() },
        Request::Service {
            name: "web".into(),
            whose: "org.example.web".into(),
            home: "/shared/services/org.example.web".into(),
        },
        Request::Rename { name: "bob".into(), new: "robert".into() },
        Request::Set { name: "bob".into(), full_name: Some("Bob Smith".into()), admin: None },
        Request::Set { name: "bob".into(), full_name: Some(String::new()), admin: Some(true) },
        Request::Set { name: "bob".into(), full_name: None, admin: Some(false) },
        Request::SharePassword {
            name: "alice".into(),
            current: b"s3cret".to_vec(),
            new: Some(b"for windows".to_vec()),
        },
        Request::SharePassword { name: "alice".into(), current: Vec::new(), new: None },
        Request::ShareSignIn {
            from: "192.0.2.7".into(),
            negotiate: b"NTLMSSP\0\x01".to_vec(),
            challenge: Vec::new(),
            authenticate: vec![0, 0xff, 0x80],
        },
        Request::EndShares,
        Request::SshKey { name: "alice".into(), key: b"\0\0\0\x0bssh-ed25519".to_vec() },
        Request::SshPassword {
            connection: "192.0.2.7 51234 192.0.2.1 22".into(),
            name: "alice".into(),
            password: b"s3cret with spaces".to_vec(),
        },
        Request::SshSession {
            connection: "192.0.2.7 51234 192.0.2.1 22".into(),
            name: "alice".into(),
            proof: SshProof::Key { signed: vec![0, 0, 0, 32, 0xff], signature: vec![1, 2] },
            term: "xterm-256color".into(),
            run: SshRun::Shell,
            language: vec!["LANG=C.UTF-8".into(), "LC_TIME=en_GB.UTF-8".into()],
        },
        Request::SshSession {
            connection: "::1 40000 ::1 22".into(),
            name: "alice".into(),
            proof: SshProof::Key { signed: Vec::new(), signature: Vec::new() },
            term: String::new(),
            run: SshRun::Command(b"ls -l 'my files'".to_vec()),
            language: Vec::new(),
        },
        Request::SshSession {
            connection: String::new(),
            name: "alice".into(),
            proof: SshProof::Ticket(vec![7; request::SSH_TICKET_LENGTH]),
            term: String::new(),
            run: SshRun::Command(Vec::new()),
            language: Vec::new(),
        },
        Request::SshSession {
            connection: "a 1 b 22".into(),
            name: "alice".into(),
            proof: SshProof::Ticket(vec![0xab; request::SSH_TICKET_LENGTH]),
            term: String::new(),
            run: SshRun::Sftp,
            language: Vec::new(),
        },
        Request::SshSession {
            connection: "a 1 b 22".into(),
            name: "alice".into(),
            proof: SshProof::Key { signed: vec![1], signature: vec![2] },
            term: String::new(),
            run: SshRun::Forward,
            language: Vec::new(),
        },
        Request::SshEnded { ticket: vec![0xcd; request::SSH_TICKET_LENGTH] },
    ];
    for request in requests {
        assert_eq!(Request::decode(&request.encode()), Ok(request));
    }
    assert_eq!(
        Request::Check { name: "a".into(), password: b"b".to_vec() }.encode(),
        b"check\0a\0b\0"
    );
    assert_eq!(Request::NeedsOwner.encode(), b"needs-owner\0");
    assert_eq!(
        Request::Reset { name: "alice".into(), password: b"pw".to_vec() }.encode(),
        b"reset\0alice\0pw\0"
    );
    let elevate = Request::Elevate {
        mode: Mode::Shell,
        password: Some(b"pw".to_vec()),
        ask: true,
        term: "linux".into(),
        directory: b"/x".to_vec(),
        language: vec!["LANG=C".into()],
        command: vec![b"id".to_vec()],
    };
    // A password typed is sent whatever `ask` says.
    assert_eq!(elevate.encode(), b"elevate\0shell\0password\0pw\0linux\0/x\0LANG=C\0--\0id\0");
    assert_eq!(Request::Unlock { password: None, ask: true }.encode(), b"unlock\0ask\0\0");
    assert_eq!(Request::Disable { name: "bob".into() }.encode(), b"disable\0bob\0");
    assert_eq!(
        Request::Rename { name: "bob".into(), new: "rob".into() }.encode(),
        b"rename\0bob\0rob\0"
    );
    assert_eq!(
        Request::Set { name: "bob".into(), full_name: Some("Bob".into()), admin: Some(false) }
            .encode(),
        b"set\0bob\0full-name\0Bob\0admin\0no\0"
    );
    assert_eq!(
        Request::Prompted { id: 12, typed: Prompted::Password(b"pw".to_vec()) }.encode(),
        b"prompted\x0012\0password\0pw\0"
    );
    assert_eq!(
        Request::Prompted { id: 3, typed: Prompted::Chose(Choice::Always) }.encode(),
        b"prompted\x003\0always\0\0"
    );
    let choose = Request::Choose {
        terminal: "pts/2".into(),
        name: "bob".into(),
        question: Question::Grant,
        text: "Give Notes".into(),
    };
    assert_eq!(choose.encode(), b"choose\0pts/2\0bob\0grant\0Give Notes\0");
    assert_eq!(
        Request::ShareSignIn {
            from: "::1".into(),
            negotiate: vec![0x4e, 0],
            challenge: vec![0xab],
            authenticate: Vec::new(),
        }
        .encode(),
        b"share-signin\0::1\x004e00\0ab\0\0"
    );
    assert_eq!(
        Request::SharePassword { name: "bob".into(), current: b"pw".to_vec(), new: None }.encode(),
        b"share-password\0bob\0pw\0clear\0\0"
    );
    assert_eq!(
        Request::SshKey { name: "bob".into(), key: vec![0, 0xab] }.encode(),
        b"ssh-key\0bob\x0000ab\0"
    );
    let ssh = Request::SshSession {
        connection: "a 1 b 22".into(),
        name: "bob".into(),
        proof: SshProof::Key { signed: vec![1], signature: vec![2] },
        term: "linux".into(),
        run: SshRun::Command(b"id".to_vec()),
        language: vec!["LANG=C".into()],
    };
    assert_eq!(
        ssh.encode(),
        b"ssh-session\0a 1 b 22\0bob\0key\x0001\x0002\0linux\0command\0id\0LANG=C\0"
    );
    let ticket = "ab".repeat(request::SSH_TICKET_LENGTH);
    let sftp = Request::SshSession {
        connection: "a 1 b 22".into(),
        name: "bob".into(),
        proof: SshProof::Ticket(vec![0xab; request::SSH_TICKET_LENGTH]),
        term: String::new(),
        run: SshRun::Sftp,
        language: Vec::new(),
    };
    assert_eq!(
        sftp.encode(),
        format!("ssh-session\0a 1 b 22\0bob\0ticket\0{ticket}\0\0\0subsystem\0sftp\0").as_bytes()
    );
    let forward = Request::SshSession {
        connection: "a 1 b 22".into(),
        name: "bob".into(),
        proof: SshProof::Ticket(vec![0xab; request::SSH_TICKET_LENGTH]),
        term: String::new(),
        run: SshRun::Forward,
        language: Vec::new(),
    };
    assert_eq!(
        forward.encode(),
        format!("ssh-session\0a 1 b 22\0bob\0ticket\0{ticket}\0\0\0forward\0\0").as_bytes()
    );
    assert_eq!(
        Request::SshPassword {
            connection: "a 1 b 22".into(),
            name: "bob".into(),
            password: b"pw".to_vec()
        }
        .encode(),
        b"ssh-password\0a 1 b 22\0bob\0pw\0"
    );
    // A password over SSH is not shown, nor a ticket.
    let password = Request::SshPassword {
        connection: "a 1 b 22".into(),
        name: "bob".into(),
        password: b"hunter2".to_vec(),
    };
    assert!(!format!("{password:?}").contains("hunter2"));
    assert!(!format!("{sftp:?}").contains("abab"));
    // A ticket of another length, a ticket with a second field, or a
    // subsystem other than SFTP is no request.
    let short = "ab".repeat(request::SSH_TICKET_LENGTH - 1);
    for bad in [
        format!("ssh-session\0a\0bob\0ticket\0{short}\0\0\0shell\0\0"),
        format!("ssh-session\0a\0bob\0ticket\0{ticket}\0ab\0\0shell\0\0"),
        format!("ssh-session\0a\0bob\0password\0{ticket}\0\0\0shell\0\0"),
        format!("ssh-session\0a\0bob\0ticket\0{ticket}\0\0\0subsystem\0netconf\0"),
        format!("ssh-session\0a\0bob\0ticket\0{ticket}\0\0\0subsystem\0\0"),
        format!("ssh-session\0a\0bob\0ticket\0{ticket}\0\0\0forward\0x\0"),
        format!("ssh-ended\0{short}\0"),
        format!("ssh-ended\0{ticket}\0x\0"),
        "ssh-password\0a\0bob\0".to_string(),
    ] {
        assert!(Request::decode(bad.as_bytes()).is_err(), "{bad:?}");
    }
    // A question with a line break, an empty field or a kind authd does
    // not know is no request.
    for bad in [
        &b"choose\0pts/2\0bob\0grant\0a\nb\0"[..],
        b"choose\0pts/2\0bob\0grant\0\0",
        b"choose\0\0bob\0grant\0x\0",
        b"choose\0pts/2\0bob\0maybe\0x\0",
        b"prompted\x003\0sometimes\0\0",
        b"prompted\x003\0always\0x\0",
        b"share-password\0bob\0pw\0clear\0x\0",
        b"share-password\0bob\0pw\0keep\0x\0",
        b"share-signin\0x\0abc\0\0\0",
        b"share-signin\0x\0zz\0\0\0",
        b"end-shares\0x\0",
        // A key or signature that is not hexadecimal; a shell with a
        // command, or a kind of session there is not; a language setting
        // that is not one; a command too long.
        b"ssh-key\0bob\0zz\0",
        b"ssh-key\0bob\0",
        b"ssh-session\0a\0bob\0key\0zz\0\0linux\0shell\0\0",
        b"ssh-session\0a\0bob\0key\0\0\0linux\0shell\0id\0",
        b"ssh-session\0a\0bob\0key\0\0\0linux\0exec\0id\0",
        b"ssh-session\0a\0bob\0key\0\0\0linux\0shell\0\0LANG\0",
        b"ssh-session\0a\0bob\0key\0\0\0linux\0",
        b"ssh-session\0a\0bob\0\0\0linux\0shell\0\0",
    ] {
        assert!(Request::decode(bad).is_err(), "{bad:?}");
    }
    for bad in [
        &b"check\0a\0"[..],
        b"check\0a\0b",
        b"check\0a\0b\0c\0",
        b"add\0a\0b\0root\0c\0",
        b"session\0a\0b\0",
        b"needs-owner\0x\0",
        b"",
        // A command, with nothing to run; no `--`; an unknown kind of
        // password; a password with `none`; a language setting that is
        // not one.
        b"elevate\0command\0none\0\0linux\0/\0--\0",
        b"elevate\0command\0none\0\0linux\0/\0id\0",
        b"elevate\0command\0maybe\0\0linux\0/\0--\0id\0",
        b"elevate\0command\0none\0x\0linux\0/\0--\0id\0",
        b"elevate\0command\0none\0\0linux\0/\0LANG\0--\0id\0",
        b"elevate\0root\0none\0\0linux\0/\0--\0id\0",
        b"unlock\0none\0",
        b"unlock\0ask\0x\0",
        b"forget\0now\0",
        // A prompt's number that is not one; an answer with words where
        // none go, or of a kind there is not.
        b"prompts\0x\0",
        b"prompted\0seven\0cancelled\0\0",
        b"prompted\0-1\0cancelled\0\0",
        b"prompted\x007\0cancelled\0x\0",
        b"prompted\x007\0elsewhere\0x\0",
        b"prompted\x007\0maybe\0\0",
        b"prompted\x007\0password\0",
        // An account's request with a field too many or too few; settings
        // with none, one twice, or a value there is not.
        b"disable\0",
        b"enable\0bob\0x\0",
        b"rename\0bob\0",
        b"set\0bob\0",
        b"set\0bob\0full-name\0",
        b"set\0bob\0admin\0maybe\0",
        b"set\0bob\0admin\0yes\0admin\0no\0",
        b"set\0bob\0shell\0/bin/sh\0",
    ] {
        assert!(Request::decode(bad).is_err(), "{bad:?}");
    }
    assert!(Request::decode(&vec![b'a'; REQUEST_MAX + 1]).is_err());
    let mut long = b"ssh-session\0a\0bob\0key\0\0\0\0command\0".to_vec();
    long.extend(vec![b'x'; SSH_COMMAND_MAX + 1]);
    long.push(0);
    assert!(Request::decode(&long).is_err());
}

#[test]
fn an_ssh_session_takes_language_settings_and_nothing_else() {
    for taken in ["LANG=de_DE.UTF-8", "LANGUAGE=de:en", "LC_ALL=C", "LC_TIME=en_DK.UTF-8", "LANG="]
    {
        assert!(request::ssh_language(taken), "{taken}");
    }
    let long = format!("LANG={}", "x".repeat(request::SSH_SETTING_MAX + 1));
    for passed_over in [
        "LANG",
        "LC_=C",
        "LC_all=C",
        "LD_PRELOAD=/tmp/x.so",
        "PATH=/tmp",
        "TERM=xterm",
        "LANG=a b",
        "LANG=a\nb",
        long.as_str(),
    ] {
        assert!(!request::ssh_language(passed_over), "{passed_over}");
    }
}

#[test]
fn debug_leaves_out_passwords() {
    let request = Request::Check { name: "alice".into(), password: b"hunter2".to_vec() };
    assert!(!format!("{request:?}").contains("hunter2"));
    let session = Request::Session {
        name: "alice".into(),
        password: b"hunter2".to_vec(),
        term: "linux".into(),
    };
    assert!(!format!("{session:?}").contains("hunter2"));
    let elevate = Request::Elevate {
        mode: Mode::Command,
        password: Some(b"hunter2".to_vec()),
        ask: false,
        term: "linux".into(),
        directory: b"/".to_vec(),
        language: Vec::new(),
        command: vec![b"id".to_vec()],
    };
    assert!(!format!("{elevate:?}").contains("hunter2"));
    assert!(
        !format!("{:?}", Request::Unlock { password: Some(b"hunter2".to_vec()), ask: false })
            .contains("hunter2")
    );
    let prompted = Request::Prompted { id: 3, typed: Prompted::Password(b"hunter2".to_vec()) };
    assert!(!format!("{prompted:?}").contains("hunter2"));
    let reset = Request::Reset { name: "alice".into(), password: b"hunter2".to_vec() };
    assert_eq!(format!("{reset:?}"), r#"Reset { name: "alice" }"#);
}

#[test]
fn prompt_lines() {
    let ask = Prompt::Ask {
        id: 4,
        terminal: "pts/2".into(),
        name: "alice".into(),
        shown: "/system/commands/user add bob".into(),
    };
    assert_eq!(ask.encode(), "ask 4 pts/2 alice /system/commands/user add bob\n");
    assert_eq!(Prompt::decode(&ask.encode()), Some(ask));
    // An unlock shows no command.
    let unlock =
        Prompt::Ask { id: 5, terminal: "pts/0".into(), name: "bob".into(), shown: String::new() };
    assert_eq!(unlock.encode(), "ask 5 pts/0 bob \n");
    assert_eq!(Prompt::decode(&unlock.encode()), Some(unlock));
    assert_eq!(Prompt::decode("done 4\n"), Some(Prompt::Done { id: 4 }));
    assert_eq!(Prompt::Done { id: 9 }.encode(), "done 9\n");
    // A control character in the command, or a space in a name, cannot
    // make a line of its own or move a field.
    let odd = Prompt::Ask {
        id: 6,
        terminal: "pts/1".into(),
        name: "a b".into(),
        shown: "echo\nask 7 pts/1 root x".into(),
    };
    assert_eq!(odd.encode(), "ask 6 pts/1 a?b echo?ask 7 pts/1 root x\n");
    let long = Prompt::Ask {
        id: 1,
        terminal: "pts/1".into(),
        name: "a".into(),
        shown: "x".repeat(PROMPT_SHOWN_MAX + 10),
    };
    assert_eq!(long.encode().len(), "ask 1 pts/1 a \n".len() + PROMPT_SHOWN_MAX);
    let choose = Prompt::Choose {
        id: 8,
        terminal: "pts/3".into(),
        name: "alice".into(),
        question: Question::Permission,
        text: "Notes asks to: use your documents folder".into(),
    };
    assert_eq!(
        choose.encode(),
        "choose 8 pts/3 alice permission Notes asks to: use your documents folder\n"
    );
    assert_eq!(Prompt::decode(&choose.encode()), Some(choose));
    let sneaky = Prompt::Choose {
        id: 9,
        terminal: "pts/3".into(),
        name: "alice".into(),
        question: Question::Grant,
        text: "x\ndone 8".into(),
    };
    assert_eq!(sneaky.encode(), "choose 9 pts/3 alice grant x?done 8\n");
    for bad in [
        "ask",
        "ask 1 pts/1",
        "ask x pts/1 a b",
        "ask 1  a b",
        "done",
        "done x",
        "wait 1",
        "",
        "choose 1 pts/1 a permission",
        "choose 1 pts/1 a maybe x",
        "choose 1 pts/1 a grant ",
    ] {
        assert_eq!(Prompt::decode(bad), None, "{bad}");
    }
}

#[test]
fn elevate_s_lines() {
    assert_eq!(stopped(20), "stopped 20\n");
    assert_eq!(stopped_signal(&stopped(20)), Some(20));
    assert_eq!(stopped_signal(&ended(0)), None);
    assert_eq!(Relay::decode(signal(2).trim_end()), Some(Relay::Signal(2)));
    assert_eq!(Relay::decode(CONTINUE.trim_end()), Some(Relay::Continue));
    for bad in ["signal 0", "signal 65", "signal -9", "signal", "go on", ""] {
        assert_eq!(Relay::decode(bad), None, "{bad}");
    }
}

#[test]
fn other_groups_are_those_that_list_the_user() {
    let groups =
        groups("admins:x:10:alice,bob\nalice:x:1000:\naudio:x:29:alice\nagain:x:10:alice\n");
    let users = users(PASSWD_TEXT);
    let alice = users.entries().find(|u| u.name == "alice").unwrap();
    assert_eq!(other_groups(alice, &groups, 32), [10, 29]);
    assert_eq!(other_groups(alice, &groups, 1), [10]);
}

#[test]
fn a_session_ends_with_its_status() {
    assert_eq!(ended(256), "ended 256\n");
    assert_eq!(ended_status(&ended(-1)), Some(-1));
    assert_eq!(ended_status("ok 41\n"), None);
}

#[test]
fn answers() {
    for answer in [
        Answer::Done(String::new()),
        Answer::Done("made bob".into()),
        Answer::Refused("wrong".into()),
    ] {
        assert_eq!(Answer::decode(&answer.encode()), answer);
    }
    assert_eq!(Answer::Refused("a\nb".into()).encode(), "no a b\n");
    assert!(matches!(Answer::decode("what"), Answer::Refused(_)));
}

#[test]
fn wiping_leaves_nothing() {
    let mut secret = b"hunter2".to_vec();
    wipe(&mut secret);
    assert!(secret.is_empty());
}

#[cfg(feature = "hash")]
mod hashes {
    use crate::hash::{Cost, hash, readable, verify};

    const SALT: [u8; 16] = *b"0123456789abcdef";

    #[test]
    fn verifies_what_it_hashed() {
        let phc = hash(b"correct horse", &SALT, Cost::TEST).unwrap();
        assert!(phc.starts_with("$argon2id$v=19$m=8192,t=1,p=1$MDEyMzQ1Njc4OWFiY2RlZg$"));
        assert_eq!(verify(b"correct horse", &phc), Ok(true));
        assert_eq!(verify(b"correct horsf", &phc), Ok(false));
        assert_eq!(verify(b"", &phc), Ok(false));
        let empty = hash(b"", &SALT, Cost::TEST).unwrap();
        assert_eq!(verify(b"", &empty), Ok(true));
    }

    /// RFC 9106's test vector needs a secret and associated data, which
    /// PHC strings have no room for; these are the reference
    /// implementation's, libargon2 20190702 as Python's argon2-cffi
    /// bundles it (`hash_secret(b"password", b"somesalt", time_cost=2,
    /// memory_cost=65536, parallelism=4, hash_len=32, type=Type.ID)`).
    #[test]
    fn agrees_with_the_reference_implementation() {
        let phc = "$argon2id$v=19$m=65536,t=2,p=4$c29tZXNhbHQ$\
                   GpZ3sK/oH9p7VIiV56G/64Zo/8GaUw434IimaPqxwCo";
        assert_eq!(verify(b"password", phc), Ok(true));
        assert_eq!(verify(b"Password", phc), Ok(false));
        assert_eq!(
            hash(b"correct horse", &SALT, Cost::TEST).unwrap(),
            "$argon2id$v=19$m=8192,t=1,p=1$MDEyMzQ1Njc4OWFiY2RlZg$\
             64zw4y6hGNSGW1HTKDajc74EIkJpCJ+AUBXDspyrFEU"
        );
    }

    #[test]
    fn refuses_what_it_cannot_read() {
        for bad in [
            "",
            "!",
            "$argon2i$v=19$m=8192,t=1,p=1$MDEyMzQ1Njc4OWFiY2RlZg$AAAAAAAAAAAAAAAAAAAAAA",
            "$argon2id$v=16$m=8192,t=1,p=1$MDEyMzQ1Njc4OWFiY2RlZg$AAAAAAAAAAAAAAAAAAAAAA",
            "$argon2id$v=19$m=8192,t=1,q=1$MDEyMzQ1Njc4OWFiY2RlZg$AAAAAAAAAAAAAAAAAAAAAA",
            "$argon2id$v=19$m=99999999,t=1,p=1$MDEyMzQ1Njc4OWFiY2RlZg$AAAAAAAAAAAAAAAAAAAAAA",
            "$6$salt$sha512crypt",
        ] {
            assert!(!readable(bad), "{bad}");
            assert!(verify(b"x", bad).is_err(), "{bad}");
        }
    }
}

// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;

use super::*;

/// The record authd's line `text` makes, written at Unix time `at`.
fn record(text: &str, at: u64) -> Record {
    Record {
        time: logd::rfc3339(at * 1_000_000_000),
        name: "authd".into(),
        from: "service".into(),
        audit: true,
        text: text.into(),
        ..Record::default()
    }
}

fn elevated(who: &str, command: &str, at: i64) -> Elevated {
    Elevated { who: who.into(), command: command.into(), at }
}

#[test]
fn an_elevated_command_at_a_terminal_is_read_from_authd_s_line() {
    let line = "chase elevated on pts/1 (password typed): process 412 runs vi \
                /system/settings/admin.toml, for process 398";
    let found = Elevated::of(&record(line, 1_793_000_000)).unwrap();
    assert_eq!(found, elevated("chase", "vi /system/settings/admin.toml", 1_793_000_000));
    let unlocked = "alice elevated on pts/0 (unlocked): process 7 runs service nginx restart, \
                    for process 3";
    assert_eq!(Elevated::of(&record(unlocked, 5)).unwrap().command, "service nginx restart");
    // A command with a comma in it is whole.
    let commas = "bob elevated on pts/2 (no password needed): process 9 runs sed s/a,b/c/ x, \
                  for process 4";
    assert_eq!(Elevated::of(&record(commas, 5)).unwrap().command, "sed s/a,b/c/ x");
}

#[test]
fn a_line_the_kernel_cut_short_keeps_its_command_whole() {
    let command = "/system/commands/sh -c sleep 45; echo \"# lab journal firefox\" >> \
                   /system/settings/spark/services/nfsd.toml";
    let line = format!("lab elevated on no terminal (password typed): process 3427 runs {command}");
    for tail in [", for process 3426", ", for process ", ", for process", ", for p", ", f", ",", ""]
    {
        let found = Elevated::of(&record(&format!("{line}{tail}"), 5)).unwrap();
        assert_eq!(found.command, command, "{tail:?}");
        assert!(found.names("/system/settings/spark/services/nfsd.toml"), "{tail:?}");
    }
}

#[test]
fn what_the_console_runs_and_other_lines_are_not_a_hand_at_a_terminal() {
    for text in [
        "chase elevated in the web console session 1a2b3c4d: process 5 runs service x restart",
        "chase elevated with the API token \"ci\", for services: process 5 runs service x stop",
        "chase unlocked the web console session 1a2b3c4d for 15 minutes (password)",
        "refused eve a command in the web console session: no",
        " elevated on pts/1 (x): process 1 runs vi y, for process 2",
        "not a name elevated on pts/1 (x): process 1 runs vi y, for process 2",
        "chase elevated on pts/1 (x): no process here",
        "",
    ] {
        assert_eq!(Elevated::of(&record(text, 5)), None, "{text:?}");
    }
    let mut no_time = record("a elevated on pts/1 (x): process 1 runs vi y, for process 2", 5);
    no_time.time = "yesterday".into();
    assert_eq!(Elevated::of(&no_time), None);
}

#[test]
fn a_command_names_a_file_by_a_word_that_is_its_path() {
    let command = elevated("chase", "vi /system/settings/admin.toml", 0);
    assert!(command.names("/system/settings/admin.toml"));
    for other in ["/system/settings/admin", "/system/settings/admin.toml.bak", "/system/settings"] {
        assert!(!command.names(other), "{other}");
    }
    assert!(!elevated("chase", "ls /system/settings", 0).names("/system/settings/admin.toml"));
}

#[test]
fn the_person_is_the_last_to_name_the_file_in_the_ten_minutes_before() {
    let path = "/system/settings/admin.toml";
    let commands = [
        elevated("old", &format!("vi {path}"), 1_000),
        elevated("alice", &format!("nano {path}"), 5_000),
        elevated("bob", &format!("vi {path}"), 5_100),
        elevated("carol", "vi /system/settings/other.toml", 5_200),
        elevated("dave", &format!("vi {path}"), 5_900),
    ];
    // Bob's command is the last before the edit; Carol's names another
    // file; Dave's came after.
    assert_eq!(who_edited(&commands, path, 5_300), Some("bob"));
    // At 5,050 Bob's command has not been run yet, and Alice's has.
    assert_eq!(who_edited(&commands, path, 5_050), Some("alice"));
    assert_eq!(who_edited(&commands, path, 5_600), Some("bob"));
    // Ten minutes exactly is in, a second more is out.
    assert_eq!(who_edited(&commands, path, 5_000 + WINDOW), Some("bob"));
    assert_eq!(who_edited(&commands[..2], path, 5_000 + WINDOW), Some("alice"));
    assert_eq!(who_edited(&commands[..2], path, 5_000 + WINDOW + 1), None);
    // No command named it before the edit.
    assert_eq!(who_edited(&commands, path, 900), None);
    assert_eq!(who_edited(&commands, "/system/settings/unnamed", 5_300), None);
}

#[test]
fn each_edit_is_given_its_own_person_or_none() {
    let commands = [elevated("alice", "vi /a", 100), elevated("bob", "vi /b", 100)];
    let edits = [("/a".to_string(), 150), ("/b".to_string(), 150), ("/c".to_string(), 150)];
    let who = who_edited_each(&commands, &edits);
    let name = |path: &str| who.get(path).map(|editor| (editor.name.as_str(), editor.note()));
    assert_eq!(name("/a"), Some(("alice", None)));
    assert_eq!(name("/b"), Some(("bob", None)));
    assert_eq!(name("/c"), None);
}

fn rooted(account: &str, started: i64, ended: Option<i64>) -> Rooted {
    Rooted { account: account.into(), started, ended }
}

#[test]
fn a_change_while_one_accounts_administrator_terminal_ran_is_that_accounts() {
    let panes = [rooted("alice", 100, Some(200)), rooted("alice", 150, None)];
    assert_eq!(who_in_terminal(&panes, 100), Some("alice"));
    assert_eq!(who_in_terminal(&panes, 5_000), Some("alice"));
    // An editor hung up as its pane ends may save a moment later.
    assert_eq!(who_in_terminal(&panes[..1], 202), Some("alice"));
    assert_eq!(who_in_terminal(&panes[..1], 203), None);
    assert_eq!(who_in_terminal(&panes, 99), None);
    // Two accounts' panes: the journal does not guess.
    let both = [rooted("alice", 100, None), rooted("bob", 120, Some(300))];
    assert_eq!(who_in_terminal(&both, 110), Some("alice"));
    assert_eq!(who_in_terminal(&both, 200), None);
    assert_eq!(who_in_terminal(&both, 400), Some("alice"));
    assert_eq!(who_in_terminal(&[], 400), None);
}

#[test]
fn an_elevated_command_names_its_person_before_a_terminal_does() {
    let commands = [elevated("bob", "vi /a", 100)];
    let edits = [("/a".to_string(), 150), ("/b".to_string(), 150), ("/c".to_string(), 50)];
    let mut who = who_edited_each(&commands, &edits);
    add_terminal_editors(&mut who, &[rooted("alice", 90, None)], &edits);
    assert_eq!(who["/a"], Editor { name: "bob".into(), in_terminal: false });
    assert_eq!(who["/b"], Editor { name: "alice".into(), in_terminal: true });
    assert_eq!(who["/b"].note(), Some(IN_TERMINAL));
    assert!(!who.contains_key("/c"));
}

/// A socket path of the test's own.
fn socket(what: &str) -> String {
    let path = std::env::temp_dir().join(format!("admind-audit-{}-{what}", std::process::id()));
    let _ = std::fs::remove_file(&path);
    path.to_string_lossy().into_owned()
}

/// A logd that is a thread: reads one request and writes `lines`; gives
/// the request it read.
fn fake_logd(path: &str, lines: Vec<String>) -> std::thread::JoinHandle<String> {
    let listener = UnixListener::bind(path).unwrap();
    std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut asked = String::new();
        BufReader::new(&stream).read_line(&mut asked).unwrap();
        let mut stream = &stream;
        for line in lines {
            stream.write_all(format!("{line}\n").as_bytes()).unwrap();
        }
        asked
    })
}

fn run<F: std::future::Future>(future: F) -> F::Output {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    runtime.block_on(future)
}

#[test]
fn logd_is_asked_for_authd_s_elevated_lines_in_the_window_and_what_it_says_is_read() {
    let path = socket("asks");
    let line = |text: &str, at| serde_json::to_string(&record(text, at)).unwrap();
    let server = fake_logd(
        &path,
        vec![
            line("chase elevated on pts/1 (unlocked): process 4 runs vi /a, for process 2", 1_000),
            line(
                "alice elevated in the web console session ab: process 5 runs service x stop",
                1_001,
            ),
            "not a record".to_string(),
        ],
    );
    let found = run(elevated_between(&path, 400, 1_200)).unwrap();
    assert_eq!(found, [elevated("chase", "vi /a", 1_000)]);
    let asked: serde_json::Value = serde_json::from_str(&server.join().unwrap()).unwrap();
    assert_eq!(asked["audit"], true);
    assert_eq!(asked["services"], serde_json::json!(["authd"]));
    assert_eq!(asked["since"], 400_000_000_000u64);
    assert_eq!(asked["until"], 1_200_000_000_000u64);
    assert_eq!(asked["grep"], "elevated on");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_logd_that_refuses_or_is_not_there_is_said_in_words() {
    let path = socket("refuses");
    let server =
        fake_logd(&path, vec![r#"{"error":"the audit log is for administrators"}"#.into()]);
    let refused = run(elevated_between(&path, 0, 10)).unwrap_err();
    assert_eq!(refused, "logd refused: the audit log is for administrators");
    server.join().unwrap();
    let _ = std::fs::remove_file(&path);
    let gone = run(elevated_between(&socket("nobody"), 0, 10)).unwrap_err();
    assert!(gone.starts_with("cannot ask logd at "), "{gone}");
}

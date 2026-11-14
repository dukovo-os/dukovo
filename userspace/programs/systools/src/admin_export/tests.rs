// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

use std::os::unix::fs::PermissionsExt;

use super::*;
use crate::cert::fixtures::Scratch;

const NOW: u64 = 1_794_643_200;

/// A machine's settings in a folder of the test's own, and a place to
/// export them to that is not made yet.
struct Machine {
    folder: Scratch,
}

impl Machine {
    fn new(what: &str) -> Machine {
        let folder = Scratch::new(what);
        fs::create_dir_all(folder.0.join("settings/spark/services")).unwrap();
        Machine { folder }
    }

    /// The settings folder.
    fn settings(&self) -> PathBuf {
        self.folder.0.join("settings")
    }

    /// Writes `text` to `name` in the settings, with `mode`.
    fn put(&self, name: &str, text: &str, mode: u32) {
        let path = self.settings().join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, text).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
    }

    /// Where the repository is.
    fn repo(&self) -> PathBuf {
        self.folder.0.join("repo")
    }

    /// Exports at `now`, and gives what it said.
    fn export(&self, now: u64) -> Result<String, Fail> {
        let mut out = Vec::new();
        export(&self.settings(), &self.repo(), "duke-nas", now, &mut out)?;
        Ok(String::from_utf8(out).unwrap())
    }

    /// What git says of the repository for `args`.
    fn git(&self, args: &[&str]) -> String {
        git(&self.repo(), "duke-nas", args).unwrap()
    }
}

#[test]
fn a_new_folder_is_made_a_repository_of_one_commit_with_the_files_where_they_are() {
    let machine = Machine::new("export-new");
    machine.put("admin.toml", "zone = \"Europe/Prague\"\n", 0o644);
    machine.put("spark/services/web.toml", "program = \"/bin/web\"\n", 0o644);
    let said = machine.export(NOW).unwrap();

    let repo = machine.repo();
    assert_eq!(
        fs::read_to_string(repo.join("system/settings/admin.toml")).unwrap(),
        "zone = \"Europe/Prague\"\n"
    );
    assert_eq!(
        fs::read_to_string(repo.join("system/settings/spark/services/web.toml")).unwrap(),
        "program = \"/bin/web\"\n"
    );
    assert_eq!(machine.git(&["rev-list", "--count", "HEAD"]).trim(), "1");
    assert_eq!(
        machine.git(&["log", "-1", "--format=%s"]).trim(),
        "Settings of duke-nas at 2026-11-14 08:00:00 UTC"
    );
    assert!(said.contains("2 files copied to "), "{said}");
    assert!(said.contains("committed "), "{said}");
    // Nothing is left out, and the file says so.
    let listed = fs::read_to_string(repo.join(LEFT_OUT)).unwrap();
    assert!(listed.contains("Nothing was left out"), "{listed}");
    assert_eq!(machine.git(&["status", "--porcelain"]), "");
}

#[test]
fn the_commits_are_the_machines_and_never_the_persons() {
    let machine = Machine::new("export-identity");
    machine.put("admin.toml", "a = 1\n", 0o644);
    // A repository that is there already, whose person has their own name,
    // address, signing and hooks.
    let repo = machine.repo();
    fs::create_dir(&repo).unwrap();
    machine.git(&["init", "-q"]);
    machine.git(&["config", "user.name", "A Person"]);
    machine.git(&["config", "user.email", "person@example.org"]);
    machine.git(&["config", "commit.gpgsign", "true"]);
    machine.git(&["config", "core.hooksPath", "hooks"]);
    fs::create_dir(repo.join("hooks")).unwrap();
    fs::write(repo.join("hooks/pre-commit"), "#!/bin/sh\nexit 1\n").unwrap();
    fs::set_permissions(repo.join("hooks/pre-commit"), fs::Permissions::from_mode(0o755)).unwrap();

    machine.export(NOW).unwrap();
    let made = machine.git(&["log", "-1", "--format=%an <%ae>|%cn <%ce>"]);
    assert_eq!(
        made.trim(),
        "Dukovo on duke-nas <settings@duke-nas.invalid>|Dukovo on duke-nas <settings@duke-nas.invalid>"
    );
}

#[test]
fn an_export_that_changes_nothing_makes_no_commit_and_a_change_makes_one() {
    let machine = Machine::new("export-again");
    machine.put("admin.toml", "a = 1\n", 0o644);
    machine.put("gone.toml", "b = 2\n", 0o644);
    machine.export(NOW).unwrap();

    let again = machine.export(NOW + 60).unwrap();
    assert!(again.contains("nothing has changed"), "{again}");
    assert!(!again.contains("committed"), "{again}");
    assert_eq!(machine.git(&["rev-list", "--count", "HEAD"]).trim(), "1");

    // A file changed, one removed, one made: one commit holds the three.
    machine.put("admin.toml", "a = 2\n", 0o644);
    fs::remove_file(machine.settings().join("gone.toml")).unwrap();
    machine.put("spark/new.toml", "c = 3\n", 0o644);
    let later = machine.export(NOW + 3600).unwrap();
    assert!(later.contains("committed "), "{later}");
    assert_eq!(machine.git(&["rev-list", "--count", "HEAD"]).trim(), "2");
    assert_eq!(
        machine.git(&["log", "-1", "--format=%s"]).trim(),
        "Settings of duke-nas at 2026-11-14 09:00:00 UTC"
    );
    let shown = machine.git(&["show", "--name-status", "--format=", "HEAD"]);
    let mut changed: Vec<&str> = shown.lines().collect();
    changed.sort_unstable();
    assert_eq!(
        changed,
        [
            "A\tsystem/settings/spark/new.toml",
            "D\tsystem/settings/gone.toml",
            "M\tsystem/settings/admin.toml"
        ]
    );
}

#[test]
fn a_file_not_everyone_may_read_is_left_out_by_name_and_never_copied() {
    let machine = Machine::new("export-secret");
    machine.put("admin.toml", "a = 1\n", 0o644);
    machine.put("ssl/key.pem", "SECRET KEY\n", 0o600);
    machine.put("shared.toml", "token = \"hunter2\"\n", 0o640);
    let said = machine.export(NOW).unwrap();

    let repo = machine.repo();
    assert!(!repo.join("system/settings/ssl/key.pem").exists());
    assert!(!repo.join("system/settings/shared.toml").exists());
    let listed = fs::read_to_string(repo.join(LEFT_OUT)).unwrap();
    let key = machine.settings().join("ssl/key.pem");
    let shared = machine.settings().join("shared.toml");
    assert!(listed.contains(&format!("- `{}`", key.display())), "{listed}");
    assert!(listed.contains(&format!("- `{}`", shared.display())), "{listed}");
    assert!(!listed.contains("SECRET KEY") && !listed.contains("hunter2"));
    assert!(said.contains("1 file copied"), "{said}");
    assert!(said.contains("2 secrets left out, since not everyone may read them"), "{said}");
    // Nowhere in the history either.
    assert_eq!(machine.git(&["log", "--all", "-S", "SECRET KEY", "--oneline"]), "");
    assert_eq!(machine.git(&["log", "--all", "-S", "hunter2", "--oneline"]), "");
}

#[test]
fn a_folder_that_cannot_be_opened_is_listed_where_it_is() {
    if util::own_user() == 0 {
        // Root opens any folder.
        return;
    }
    let machine = Machine::new("export-closed");
    machine.put("admin.toml", "a = 1\n", 0o644);
    machine.put("private/inner.toml", "b = 2\n", 0o644);
    let private = machine.settings().join("private");
    fs::set_permissions(&private, fs::Permissions::from_mode(0o000)).unwrap();
    let said = machine.export(NOW);
    fs::set_permissions(&private, fs::Permissions::from_mode(0o755)).unwrap();
    let said = said.unwrap();

    let listed = fs::read_to_string(machine.repo().join(LEFT_OUT)).unwrap();
    assert!(listed.contains(&format!("- `{}/`", private.display())), "{listed}");
    assert!(said.contains("1 could not be read by you and is left out"), "{said}");
    assert!(!machine.repo().join("system/settings/private").exists());
}

#[test]
fn a_folder_with_other_files_is_not_made_a_repository_and_nothing_is_written() {
    let machine = Machine::new("export-busy");
    machine.put("admin.toml", "a = 1\n", 0o644);
    fs::create_dir(machine.repo()).unwrap();
    fs::write(machine.repo().join("notes.txt"), "mine\n").unwrap();
    let refused = machine.export(NOW).unwrap_err();
    assert!(
        refused.message.contains("has files and is not a Git repository"),
        "{}",
        refused.message
    );
    assert!(!machine.repo().join(".git").exists());
    assert!(!machine.repo().join("system").exists());
    assert_eq!(fs::read_to_string(machine.repo().join("notes.txt")).unwrap(), "mine\n");

    // An empty folder is made one, and so is a folder that is a file.
    fs::remove_file(machine.repo().join("notes.txt")).unwrap();
    machine.export(NOW).unwrap();
    assert!(machine.repo().join(".git").exists());

    let file = machine.folder.0.join("plain");
    fs::write(&file, "x").unwrap();
    let mut out = Vec::new();
    let not_a_folder = export(&machine.settings(), &file, "duke-nas", NOW, &mut out).unwrap_err();
    assert!(not_a_folder.message.contains("not a folder"), "{}", not_a_folder.message);
}

#[test]
fn a_repository_inside_the_settings_is_refused() {
    let machine = Machine::new("export-inside");
    machine.put("admin.toml", "a = 1\n", 0o644);
    let mut out = Vec::new();
    for inside in [machine.settings(), machine.settings().join("new")] {
        let refused = export(&machine.settings(), &inside, "duke-nas", NOW, &mut out).unwrap_err();
        assert!(refused.message.contains("that is inside"), "{}", refused.message);
    }
    assert!(!machine.settings().join("new").exists());
}

#[test]
fn a_settings_folder_with_no_file_is_a_commit_of_what_was_left_out_alone() {
    let machine = Machine::new("export-empty");
    let said = machine.export(NOW).unwrap();
    assert!(said.contains("0 files copied"), "{said}");
    assert_eq!(machine.git(&["rev-list", "--count", "HEAD"]).trim(), "1");
    assert!(machine.repo().join(LEFT_OUT).exists());

    // A settings folder that is not there is no export.
    let missing = Machine::new("export-missing");
    fs::remove_dir_all(missing.settings()).unwrap();
    let refused = missing.export(NOW).unwrap_err();
    assert!(refused.message.contains("cannot be read"), "{}", refused.message);
}

#[test]
fn a_machine_name_makes_an_address_that_is_never_a_persons() {
    assert_eq!(
        identity("duke-nas"),
        ("Dukovo on duke-nas".to_string(), "settings@duke-nas.invalid".to_string())
    );
    assert_eq!(identity("a b@c").1, "settings@a-b-c.invalid");
    assert_eq!(identity("").1, "settings@localhost.invalid");
    assert_eq!(identity("@@").1, "settings@localhost.invalid");
}

#[test]
fn the_time_of_a_commit_is_in_utc_to_the_second() {
    assert_eq!(time_of(0), "1970-01-01 00:00:00 UTC");
    assert_eq!(time_of(NOW), "2026-11-14 08:00:00 UTC");
    assert_eq!(time_of(NOW + 3661), "2026-11-14 09:01:01 UTC");
}

#[test]
fn the_command_line_takes_one_folder() {
    let mut none = Args::new("admin export", Vec::new());
    assert!(main(&mut none).unwrap_err().usage);
    let mut two = Args::new("admin export", vec!["a".into(), "b".into()]);
    assert!(main(&mut two).unwrap_err().usage);
}

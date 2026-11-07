// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The Dukovo Lab's side of `ob` (ADR 0401): `ob image --lab`, the image
//! the lab tests, and `ob lab`, which runs the lab's harness on this
//! tree's tests in `lab/tests`.
//!
//! A lab image is an installed system's disk with three things added,
//! all outside the system's slot, so that no release (which is made from
//! the slot alone) can ever carry them:
//!
//! - the `lab` account, an administrator's whose password no one knows,
//!   so that it signs in only over SSH with the lab key;
//! - its home, the account's, holding the lab's public key in
//!   `~/.ssh/authorized_keys`;
//! - sshd turned on, through an override file, and a sign-in banner that
//!   says the image is a test image.
//!
//! The harness itself, dukovo-lab, lives in dukovo-os/lab; `ob lab` finds
//! it on PATH (or at `OB_LAB`), builds the lab image and passes it on.
//! When `ob lab run` fails before a test starts, run `dukovo-lab` with
//! the same arguments to see its own message.
//!
//! Plain `ob lab`, with no dukovo-lab command, is the one command a person
//! needs: it sets up what is missing (dukovo-lab, the lab key, the
//! lab-results clone that Send uses), checks QEMU, then builds the image
//! and opens the runner page. Each step is skipped once it is done.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::account::{self, Account};
use crate::image::{self, Overrides};
use crate::process::{self, step};
use crate::rootfs::Owned;
use crate::{Args, Context as _, Ctx, Error, Result, VERSION, bail, bench, doctor, qemu};

/// The lab's account.
pub const ACCOUNT: &str = "lab";

/// Its full name, as `user list` shows it.
const FULL_NAME: &str = "Dukovo Lab";

/// dukovo-lab's configuration folder, under the home folder, when its
/// `--config` does not name another.
const CONFIG: &str = ".config/dukovo-lab";

/// The lab key's public half in that folder (`dukovo-lab key` makes it).
const PUBLIC_KEY: &str = "lab_ed25519.pub";

/// What a lab image's console says before signing in, after the default
/// banner.
pub const BANNER: &str = "Test image of the Dukovo Lab: the lab account signs in over SSH \
                          with the lab's key. Never use it as a release.";

/// Where dukovo-lab comes from.
const LAB_REPO: &str = "https://github.com/dukovo-os/lab";

/// How dukovo-lab is installed when `ob lab` cannot find it.
const INSTALL: &str = "cargo install --locked --git https://github.com/dukovo-os/lab";

/// dukovo-lab's store, under the home folder, when its `--store` does not
/// name another.
const STORE: &str = ".local/share/dukovo-lab";

/// The private repository Send pushes findings to, over SSH and then
/// HTTPS: whichever way this PC's GitHub sign-in works.
const RESULTS_REPO: [&str; 2] =
    ["git@github.com:dukovo-os/lab-results.git", "https://github.com/dukovo-os/lab-results.git"];

/// What the lab runs on this PC, each with the Debian or Ubuntu package
/// that brings it.
const NEEDS: &[(&str, &str)] = &[
    ("qemu-system-x86_64", "qemu-system-x86"),
    ("qemu-img", "qemu-utils"),
    ("ssh", "openssh-client"),
    ("ssh-keygen", "openssh-client"),
    ("git", "git"),
    ("gzip", "gzip"),
];

/// The lab's public key, which a lab image trusts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Key {
    /// One `authorized_keys` line: `ssh-ed25519 <base64> <comment>`.
    line: String,
}

impl Key {
    /// Reads the key from `file` (`--lab-key`), or from dukovo-lab's
    /// configuration folder `config` (its `--config`), or from the folder
    /// it uses when given none.
    pub fn read(file: Option<&str>, config: Option<&str>) -> Result<Key> {
        let path = match (file, config) {
            (Some(path), _) => PathBuf::from(path),
            (None, Some(config)) => Path::new(config).join(PUBLIC_KEY),
            (None, None) => {
                let home = std::env::var_os("HOME")
                    .ok_or_else(|| Error::new("HOME is not set: give the key with --lab-key"))?;
                Path::new(&home).join(CONFIG).join(PUBLIC_KEY)
            }
        };
        let text = std::fs::read_to_string(&path).context(|| {
            format!(
                "reading the lab's public key {}: run `dukovo-lab key` once to make it, \
                 or give one with --lab-key",
                path.display()
            )
        })?;
        Key::parse(&text).map_err(|why| Error::new(format!("{}: {why}", path.display())))
    }

    /// Checks that `text` is one Ed25519 public key, as `ssh-keygen` writes
    /// it. sshd passes over lines with options, so none is allowed here.
    fn parse(text: &str) -> std::result::Result<Key, String> {
        // A copy made on Windows ends its line with \r\n.
        let line = text.trim();
        if line.contains(['\n', '\r']) {
            return Err("holds more than one line; the lab key is one".into());
        }
        let mut words = line.split(' ');
        if words.next() != Some("ssh-ed25519") {
            return Err("is not an Ed25519 public key (ssh-ed25519 ...)".into());
        }
        let key = words.next().unwrap_or("");
        let base64 = |c: char| c.is_ascii_alphanumeric() || "+/=".contains(c);
        if key.len() < 60 || !key.chars().all(base64) {
            return Err("does not hold a public key after ssh-ed25519".into());
        }
        Ok(Key { line: line.into() })
    }
}

/// Adds the lab's account, home, key, sshd and banner to a root being
/// staged in `staging`, whose subvolumes are named as `ob image` names
/// them; `system` is its staged system slot, which is not changed. Gives
/// the home, which must be the account's.
pub fn stage(staging: &Path, system: &Path, key: &Key) -> Result<Owned> {
    let settings = staging.join("@settings");
    let account = Account {
        name: ACCOUNT.into(),
        full_name: FULL_NAME.into(),
        hash: account::unknown_password_hash()?,
    };
    let (owner, group) = account::add(system, &settings, &staging.join("@state"), &account)?;

    // Its home, made as authd makes one (the folders a home starts with)
    // and with the key; given to the account as the root is written.
    let home = staging.join("@users").join(ACCOUNT);
    make_dir(&home, 0o700)?;
    for folder in accounts::HOME_FOLDERS {
        make_dir(&home.join(folder), if folder == "app-data" { 0o700 } else { 0o755 })?;
    }
    make_dir(&home.join(".ssh"), 0o700)?;
    write(&home.join(".ssh/authorized_keys"), &format!("{}\n", key.line), 0o600)?;

    // sshd is off by default (ADR 0347): an override turns it on, as
    // `service sshd enable` does, without changing the system's file.
    let overrides = settings.join("spark/services/settings.d");
    std::fs::create_dir_all(&overrides).context(|| format!("making {}", overrides.display()))?;
    write(
        &overrides.join("sshd.toml"),
        "# The lab image: the lab signs in over SSH (ADR 0401).\nenabled = true\n",
        0o644,
    )?;

    // The sign-in banner: the default's, then what this image is.
    let defaults = crate::userspace::DEFAULTS_DIR.strip_prefix("system/").expect("in the system");
    let issue = std::fs::read_to_string(system.join(defaults).join("issue")).unwrap_or_default();
    write(&settings.join("issue"), &format!("{issue}{BANNER}\n"), 0o644)?;

    Ok(Owned { path: Path::new("@users").join(ACCOUNT), owner, group })
}

fn make_dir(path: &Path, mode: u32) -> Result<()> {
    std::fs::create_dir_all(path).context(|| format!("making {}", path.display()))?;
    set_mode(path, mode)
}

fn write(path: &Path, text: &str, mode: u32) -> Result<()> {
    std::fs::write(path, text).context(|| format!("writing {}", path.display()))?;
    set_mode(path, mode)
}

fn set_mode(path: &Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
        .context(|| format!("setting the mode of {}", path.display()))
}

/// The dukovo-lab commands that test an image, and so need one built:
/// `run`, and `ui`, the runner page, which starts runs.
const NEEDS_IMAGE: &[&str] = &["run", "ui"];

/// dukovo-lab's options that take a value, which is not its command.
const VALUED: &[&str] = &[
    "--tests",
    "--config",
    "--store",
    "--image",
    "--machine",
    "--build",
    "--preset",
    "--port",
    "--limit",
    "--run",
    "--results",
    "--note",
];

/// `ob lab`: runs dukovo-lab on `lab/tests`, building the lab image first
/// for the commands that need one.
pub fn command(ctx: &Ctx, args: &mut Args) -> Result<()> {
    let no_build = args.flag("--no-build");
    let given_image = args.value("--image")?;
    let key_file = args.value("--lab-key")?;
    let profile = args.profile(&ctx.config)?;
    let mut rest = args.rest();
    rest.extend(args.take_passthrough());
    if wants_setup(&rest) {
        set_up(ctx, &rest)?;
        rest.push("ui".into());
    }
    let tool = find_tool()?;

    let mut lab = Command::new(&tool);
    lab.arg("--tests").arg(ctx.root.join("lab/tests"));
    if lab_command(&rest).is_some_and(|command| NEEDS_IMAGE.contains(&command)) {
        let image = match given_image {
            Some(image) => PathBuf::from(image),
            None if no_build => ctx.out.join(image::file_name(image::Kind::Lab)),
            None => {
                // The key dukovo-lab will sign in with is the one in the
                // folder it is given, so the image trusts that one.
                let key = Key::read(key_file.as_deref(), option_value(&rest, "--config"))?;
                let overrides = Overrides { lab: Some(key), ..Overrides::default() };
                image::build_release(ctx, profile, image::Kind::Lab, &overrides)?
            }
        };
        if !image.is_file() {
            bail!("no lab image at {}: leave out --no-build to build one", ctx.display(&image));
        }
        lab.arg("--image").arg(image);
        lab.args(["--build", &format!("{VERSION} {}", bench::commit(ctx))]);
    }
    lab.args(&rest);
    // dukovo-lab's own exit status says how the run went (0 passed, 1 a
    // test failed, 2 a bad request, 3 the lab broke): it is ob's too.
    let status = lab.status().context(|| format!("starting {}", tool.display()))?;
    match status.code() {
        Some(0) => Ok(()),
        Some(code) => std::process::exit(code),
        None => bail!("{} was stopped by a signal", tool.display()),
    }
}

/// Whether `args` hold no dukovo-lab command, nor ask for its help: plain
/// `ob lab`, which sets the lab up and opens the runner page.
fn wants_setup(args: &[String]) -> bool {
    let asks = |word: &String| matches!(word.as_str(), "--help" | "-h" | "--version" | "-V");
    lab_command(args).is_none() && !args.iter().any(asks)
}

/// Sets up what the lab needs and is not there yet, for plain `ob lab`:
/// the programs it runs, dukovo-lab itself (installed, or updated to the
/// newest), the lab key, and the lab-results clone that Send uses. `args`
/// are the options for dukovo-lab, whose `--config`, `--store` and
/// `--results` it follows.
fn set_up(ctx: &Ctx, args: &[String]) -> Result<()> {
    step("Checking what the lab needs on this PC");
    let missing = missing_needs();
    if !missing.is_empty() {
        let mut packages: Vec<&str> = Vec::new();
        for (_, package) in &missing {
            if !packages.contains(package) {
                packages.push(package);
            }
        }
        let programs: Vec<&str> = missing.iter().map(|(program, _)| *program).collect();
        return Err(Error::new(format!("the lab needs {} on this PC", programs.join(", "))).hint(
            format!(
                "on Linux Mint, Debian or Ubuntu: `sudo apt install {}`, then ./ob lab again",
                packages.join(" ")
            ),
        ));
    }
    if !qemu::kvm_available() {
        eprintln!(
            "    note: /dev/kvm is not usable, so tests run under QEMU's emulator, some four times slower"
        );
    }

    // dukovo-lab: the newest from its repository. cargo builds it only
    // when there is a newer commit, and an update that fails (offline,
    // say) leaves the one installed.
    if std::env::var_os("OB_LAB").is_none() {
        let installed = find_tool().is_ok();
        step(if installed {
            "Updating dukovo-lab, if there is a newer one"
        } else {
            "Installing dukovo-lab, the lab's harness"
        });
        // With this tree's pinned toolchain, which ob setup installed.
        let mut cargo = Command::new("cargo");
        cargo
            .args(["install", "--locked", "--git", LAB_REPO])
            .env("RUSTUP_TOOLCHAIN", doctor::toolchain_channel(ctx)?);
        if installed {
            // Up to date is the usual answer, and cargo's words for it
            // ("Ignored package ... use --force") read like a problem.
            cargo.arg("--quiet");
        }
        match process::run(ctx, &mut cargo) {
            Ok(()) => {}
            Err(err) if installed => {
                eprintln!("    note: keeping the dukovo-lab already installed: {err}");
            }
            Err(err) => return Err(err),
        }
    }
    let tool = find_tool()?;

    // The lab key, which lab images trust.
    let config = match option_value(args, "--config") {
        Some(config) => PathBuf::from(config),
        None => home()?.join(CONFIG),
    };
    if !config.join("lab_ed25519").is_file() {
        step("Making the lab key");
        let mut key = Command::new(&tool);
        key.arg("--config").arg(&config).arg("key");
        process::run(ctx, &mut key)?;
    }

    // The lab-results clone, without which Send is off. A clone that
    // fails is not the end: the page opens, and says Send needs it.
    let clone = match (option_value(args, "--results"), option_value(args, "--store")) {
        (Some(results), _) => PathBuf::from(results),
        (None, Some(store)) => Path::new(store).join("lab-results"),
        (None, None) => home()?.join(STORE).join("lab-results"),
    };
    if !clone.join(".git").exists() {
        step("Cloning lab-results, where Send puts findings");
        if let Some(parent) = clone.parent() {
            std::fs::create_dir_all(parent).context(|| format!("making {}", parent.display()))?;
        }
        // Over SSH first, quietly: no key for GitHub is no error, and
        // github.com's host key is taken on first use, as a person
        // answering yes would. Then over HTTPS, where git may ask for a
        // sign-in in the terminal.
        let mut ssh = Command::new("git");
        ssh.args(["clone", "-q", RESULTS_REPO[0]])
            .arg(&clone)
            .env("GIT_SSH_COMMAND", "ssh -o BatchMode=yes -o StrictHostKeyChecking=accept-new");
        let cloned = process::output(ctx, &mut ssh).is_ok() || {
            let mut https = Command::new("git");
            https.args(["clone", "-q", RESULTS_REPO[1]]).arg(&clone);
            process::run(ctx, &mut https).is_ok()
        };
        if !cloned {
            eprintln!(
                "    note: could not clone lab-results, so Send stays off. This PC's GitHub account \
                 needs push access to dukovo-os/lab-results, through an SSH key on GitHub or a \
                 sign-in git keeps (such as `gh auth login`). ./ob lab tries again next time."
            );
        }
    }
    // Send commits as the person on this PC, so git needs their name.
    let mut email = Command::new("git");
    email.args(["config", "user.email"]);
    if clone.join(".git").exists() && process::output(ctx, email.current_dir(&clone)).is_err() {
        eprintln!(
            "    note: git has no name and email to commit findings with, so Send will fail: \
             `git config --global user.name \"Your Name\"` and \
             `git config --global user.email you@example.org`"
        );
    }
    step("Building the lab image and opening the runner page");
    Ok(())
}

/// The programs in [`NEEDS`] not on PATH, and OVMF if it is not there.
fn missing_needs() -> Vec<(&'static str, &'static str)> {
    let mut missing: Vec<(&str, &str)> = NEEDS
        .iter()
        .copied()
        .filter(|(program, _)| process::find_program(program).is_none())
        .collect();
    if qemu::find_ovmf().is_none() {
        missing.push(("OVMF (UEFI firmware for QEMU)", "ovmf"));
    }
    missing
}

/// The home folder.
fn home() -> Result<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from).ok_or_else(|| {
        Error::new("HOME is not set: give dukovo-lab's folders with --config and --store")
    })
}

/// The value `args` give the dukovo-lab option `name`, if any; the last
/// one, as dukovo-lab takes the last.
fn option_value<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.windows(2).rev().find(|pair| pair[0] == name).map(|pair| pair[1].as_str())
}

/// The dukovo-lab command among `args`: the first word that is neither
/// an option nor an option's value.
fn lab_command(args: &[String]) -> Option<&str> {
    let mut words = args.iter();
    while let Some(word) = words.next() {
        if VALUED.contains(&word.as_str()) {
            words.next();
        } else if !word.starts_with('-') {
            return Some(word);
        }
    }
    None
}

/// dukovo-lab: `OB_LAB`, or the first on PATH, or where `cargo install`
/// puts it (which is not on every PC's PATH).
fn find_tool() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("OB_LAB") {
        return Ok(PathBuf::from(path));
    }
    let path = std::env::var_os("PATH").unwrap_or_default();
    let cargo_bin = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| Path::new(&home).join(".cargo")))
        .map(|cargo| cargo.join("bin"));
    std::env::split_paths(&path)
        .chain(cargo_bin)
        .map(|dir| dir.join("dukovo-lab"))
        .find(|candidate| candidate.is_file())
        .ok_or_else(|| {
            Error::new(format!(
                "dukovo-lab, the lab's harness, is not installed: run ./ob lab, which installs it, or `{INSTALL}`"
            ))
        })
}

/// Whether dukovo-lab is installed, for `ob doctor`.
pub fn tool_found() -> Option<PathBuf> {
    find_tool().ok()
}

/// What `ob doctor` says when dukovo-lab is missing.
pub const DOCTOR_NOTE: &str = INSTALL;

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIGv8yKkzUYtD3Q5jzN7mKpWv0bL3Vx2m5C9E4fQwZr8T dukovo-lab\n";

    #[test]
    fn takes_one_ed25519_key() {
        assert_eq!(Key::parse(KEY).unwrap().line, KEY.trim_end());
        let windows = KEY.replace('\n', "\r\n");
        assert_eq!(Key::parse(&windows).unwrap().line, KEY.trim_end());
        for bad in [
            "",
            "ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABAQ lab",
            "ssh-ed25519",
            "command=\"x\" ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIGv8yKkzUYtD3Q5jzN7mKpWv0bL3Vx2m5C9E4fQwZr8T",
            &format!("{KEY}{KEY}"),
        ] {
            assert!(Key::parse(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn plain_ob_lab_sets_up_and_opens_the_page() {
        let words = |line: &str| line.split_whitespace().map(String::from).collect::<Vec<_>>();
        assert!(wants_setup(&words("")));
        assert!(wants_setup(&words("--store /tmp/lab --results /tmp/clone")));
        assert!(!wants_setup(&words("list")));
        assert!(!wants_setup(&words("--store /tmp/lab ui")));
        assert!(!wants_setup(&words("--help")));
        assert!(!wants_setup(&words("findings --run 20261107-153012-4f2a")));
    }

    #[test]
    fn finds_the_command_past_options_and_their_values() {
        let words = |line: &str| line.split(' ').map(String::from).collect::<Vec<_>>();
        assert_eq!(lab_command(&words("run --all")), Some("run"));
        assert_eq!(lab_command(&words("--machine run --json run boot/")), Some("run"));
        assert_eq!(lab_command(&words("--json list")), Some("list"));
        assert_eq!(lab_command(&words("--machine qemu")), None);
        assert_eq!(lab_command(&words("--port 7421 ui")), Some("ui"));
        assert_eq!(lab_command(&words("--preset quick run")), Some("run"));
        assert_eq!(
            option_value(&words("run --config /a --all --config /b"), "--config"),
            Some("/b")
        );
        assert_eq!(option_value(&words("run --all"), "--config"), None);
    }

    #[test]
    fn stages_the_account_home_key_sshd_and_banner_beside_the_system() {
        let staging = std::env::temp_dir().join(format!("ob-lab-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&staging);
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let system = staging.join("@system.a");
        let defaults = system.join("resources/default-settings");
        for dir in [
            &defaults,
            &staging.join("@settings"),
            &staging.join("@state"),
            &staging.join("@users"),
        ] {
            std::fs::create_dir_all(dir).unwrap();
        }
        for name in ["passwd", "group", "issue"] {
            std::fs::copy(root.join("userspace/defaults").join(name), defaults.join(name)).unwrap();
        }
        let owned = stage(&staging, &system, &Key::parse(KEY).unwrap()).unwrap();
        assert_eq!(owned, Owned { path: "@users/lab".into(), owner: 1000, group: 1000 });

        let read = |path: &str| std::fs::read_to_string(staging.join(path)).unwrap();
        assert!(read("@settings/passwd").contains("lab:x:1000:1000:Dukovo Lab:/users/lab:"));
        assert!(read("@settings/group").contains("admins:x:10:lab"));
        let hash = read("@state/authd/passwords");
        assert!(hash.starts_with("lab:$argon2id$"), "a password no one knows, not an empty one");
        assert_eq!(read("@users/lab/.ssh/authorized_keys"), KEY);
        assert!(
            read("@settings/spark/services/settings.d/sshd.toml").contains("\nenabled = true\n")
        );
        assert!(read("@settings/issue").ends_with(&format!("{BANNER}\n")));
        assert!(std::fs::metadata(staging.join("@users/lab/documents")).unwrap().is_dir());
        let mode = |path: &str| {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::metadata(staging.join(path)).unwrap().permissions().mode() & 0o7777
        };
        assert_eq!(
            [mode("@users/lab"), mode("@users/lab/.ssh"), mode("@users/lab/.ssh/authorized_keys")],
            [0o700, 0o700, 0o600]
        );
        // The system slot, which releases are made from, is as it was.
        let mut staged: Vec<_> =
            std::fs::read_dir(&defaults).unwrap().map(|e| e.unwrap().file_name()).collect();
        staged.sort();
        assert_eq!(staged, ["group", "issue", "passwd"]);
        assert!(!read("@system.a/resources/default-settings/issue").contains(BANNER));
        std::fs::remove_dir_all(&staging).unwrap();
    }
}

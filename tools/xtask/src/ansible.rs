// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The host's side of the Ansible tests: `ob test boot --case ansible`
//! runs the playbooks of `src/test/ansible/` against a machine in QEMU, and
//! the lab's `system/ansible` the same ones from the lab PC.
//!
//! The playbooks run on ansible-core of one release, [`CORE`], which `ob`
//! installs once into a Python virtual environment in `.cache/ansible/` as
//! `ob test tuf` does its suite's, and the collection `dukovo.system` comes
//! from the tarball `src/test/ansible.lock` pins: a path or a URL, the git
//! commit of the collection's repository it was built from, and its SHA-256.
//! Until that repository exists the lock names the tarball by an
//! environment variable instead, [`LockSource::File`], and a test run
//! without it is skipped with a line that says why, so that CI, which has
//! no such file, does not fail. The environment, the controller's Python
//! (3.11 or later, with venv) and OpenSSH's client are all the host needs;
//! it only runs on the build host.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::process::step;

/// ansible-core, pinned to one release, which pip installs into the
/// environment. The lab's `system/ansible.sh` pins the same one, and a unit
/// test says so.
pub const CORE: &str = "2.19.15";

/// The oldest Python's minor version that runs ansible-core [`CORE`] on the
/// controller.
const PYTHON_MINOR: u32 = 11;

/// Where the environment and the collections installed from tarballs are.
const CACHE: &str = ".cache/ansible";

/// The lock, which the tests read where it lies in the tree.
pub const LOCK_FILE: &str = "tools/xtask/src/test/ansible.lock";

/// The text of the lock.
const LOCK: &str = include_str!("test/ansible.lock");

/// The folder of the playbooks, from the top of the tree.
pub const PLAYBOOKS: &str = "tools/xtask/src/test/ansible";

/// Where the interpreter of the machine is: the `org.python.python` bundle,
/// installed for everyone, run by its path, which asks no question as the
/// command `python3` does the first time at a console.
pub const PYTHON: &str = "/applications/org.python.python/bin/python3";

/// The host's name in the playbooks.
pub const HOST: &str = "dukovo";

/// Where the lock says the collection's tarball is.
#[derive(Debug, PartialEq, Eq)]
pub enum LockSource {
    /// A file on this computer, which the environment variable of this name
    /// gives.
    File(String),
    /// An `https` URL, which `ob` fetches once.
    Url(String),
}

/// What `ansible.lock` pins.
#[derive(Debug, PartialEq, Eq)]
pub struct Lock {
    pub source: LockSource,
    /// The git commit of the collection's repository the tarball is built
    /// from: 40 hexadecimal digits, or none while the source is a file.
    pub commit: Option<String>,
    /// The SHA-256 of the tarball: 64 hexadecimal digits, or none while the
    /// source is a file, which runs as it is.
    pub sha256: Option<String>,
}

/// Whether `text` is `length` lowercase hexadecimal digits.
fn hex(text: &str, length: usize) -> bool {
    text.len() == length && text.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

impl Lock {
    /// Reads the lock's lines: `key = value`, with `#` comments.
    pub fn parse(text: &str) -> Result<Lock, String> {
        let (mut source, mut commit, mut sha256) = (None, None, None);
        for line in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
            let (key, value) = line
                .split_once('=')
                .map(|(key, value)| (key.trim(), value.trim()))
                .ok_or_else(|| format!("ansible.lock: \"{line}\" is no `key = value`"))?;
            let slot = match key {
                "source" => &mut source,
                "commit" => &mut commit,
                "sha256" => &mut sha256,
                _ => return Err(format!("ansible.lock: {key} is no key of the lock")),
            };
            if slot.replace(value.to_string()).is_some() {
                return Err(format!("ansible.lock: {key} is given twice"));
            }
        }
        let say = |key: &str, value: Option<String>| {
            value.ok_or_else(|| format!("ansible.lock: {key} is missing"))
        };
        let source = say("source", source)?;
        let (commit, sha256) = (say("commit", commit)?, say("sha256", sha256)?);
        let source = match source.strip_prefix("env:") {
            Some(name)
                if !name.is_empty()
                    && name
                        .bytes()
                        .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_') =>
            {
                LockSource::File(name.to_string())
            }
            Some(name) => {
                return Err(format!(
                    "ansible.lock: \"{name}\" is no name of an environment variable"
                ));
            }
            None if source.starts_with("https://") && !source.contains(char::is_whitespace) => {
                LockSource::Url(source)
            }
            None => {
                return Err(format!(
                    "ansible.lock: source \"{source}\" is neither env:NAME nor an https URL"
                ));
            }
        };
        let pinned = |key: &str, value: String, length: usize| match (value.as_str(), &source) {
            ("-", LockSource::File(_)) => Ok(None),
            ("-", LockSource::Url(_)) => Err(format!("ansible.lock: a URL needs its {key} pinned")),
            (digits, _) if hex(digits, length) => Ok(Some(digits.to_string())),
            _ => {
                Err(format!("ansible.lock: {key} is {length} lowercase hexadecimal digits, or `-`"))
            }
        };
        Ok(Lock {
            commit: pinned("commit", commit, 40)?,
            sha256: pinned("sha256", sha256, 64)?,
            source,
        })
    }

    /// The lock of the tree.
    pub fn read() -> Result<Lock, String> {
        Lock::parse(LOCK)
    }
}

/// What a test needs from the host to run the playbooks.
pub struct Tools {
    /// The environment's `ansible-playbook`.
    playbook: PathBuf,
    /// Where the collection is installed, for `ANSIBLE_COLLECTIONS_PATH`.
    collections: PathBuf,
}

/// What `prepare` found.
pub enum Prepared {
    /// What the playbooks run on.
    Ready(Tools),
    /// Why the test is skipped, a line.
    Skip(String),
}

/// The line that says a test is skipped, because the lock names the
/// tarball by `variable`, which is not set.
pub fn skipped(variable: &str) -> String {
    format!(
        "ansible: skipped: {variable} is not set; it names the tarball of the collection \
         dukovo.system that {LOCK_FILE} pins (`ansible-galaxy collection build` makes one) until \
         the collection's repository exists"
    )
}

/// Runs `command` with the streams of `ob`, failing with `what` if it does
/// not end well.
fn run(command: &mut Command, what: &str) -> Result<(), String> {
    let status = command.status().map_err(|err| format!("{what}: {err}"))?;
    if status.success() { Ok(()) } else { Err(format!("{what} failed ({status})")) }
}

/// The SHA-256 of the file, in hexadecimal.
fn sha256(file: &Path) -> Result<String, String> {
    let out = Command::new("sha256sum")
        .arg(file)
        .output()
        .map_err(|err| format!("sha256sum {}: {err}", file.display()))?;
    String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .next()
        .filter(|sum| hex(sum, 64))
        .map(str::to_string)
        .ok_or_else(|| format!("sha256sum {} said nothing useful", file.display()))
}

/// The tarball the lock names, fetched if it is a URL and not here yet:
/// None, with the line that says so, when the lock names it by a variable
/// that is not set.
fn tarball(root: &Path, lock: &Lock) -> Result<Result<PathBuf, String>, String> {
    match (&lock.source, &lock.sha256) {
        (LockSource::File(variable), pinned) => {
            let Some(path) = std::env::var_os(variable).filter(|path| !path.is_empty()) else {
                return Ok(Err(skipped(variable)));
            };
            let path = PathBuf::from(path);
            if !path.is_file() {
                return Err(format!("{variable} is {}, which is no file", path.display()));
            }
            if let Some(pinned) = pinned {
                let sum = sha256(&path)?;
                if &sum != pinned {
                    return Err(format!(
                        "{} has SHA-256 {sum}, not the pinned {pinned}",
                        path.display()
                    ));
                }
            }
            Ok(Ok(path))
        }
        (LockSource::Url(url), Some(pinned)) => {
            let file = root.join(CACHE).join(format!("dukovo-system-{}.tar.gz", &pinned[..16]));
            if file.is_file() && &sha256(&file)? == pinned {
                return Ok(Ok(file));
            }
            step(&format!("Fetching the collection dukovo.system from {url}"));
            fs::create_dir_all(root.join(CACHE)).map_err(|err| format!("making {CACHE}: {err}"))?;
            let partial = file.with_extension("partial");
            let mut curl = Command::new("curl");
            curl.args(["--proto", "=https", "--tlsv1.2", "-fsSL", "--retry", "3", "-o"])
                .arg(&partial)
                .arg(url);
            run(&mut curl, "curl")?;
            let sum = sha256(&partial)?;
            if &sum != pinned {
                let _ = fs::remove_file(&partial);
                return Err(format!("{url} has SHA-256 {sum}, not the pinned {pinned}"));
            }
            fs::rename(&partial, &file)
                .map_err(|err| format!("moving it to {}: {err}", file.display()))?;
            Ok(Ok(file))
        }
        (LockSource::Url(_), None) => {
            Err("ansible.lock: a URL needs its sha256 pinned".to_string())
        }
    }
}

/// The virtual environment with ansible-core [`CORE`], made once and kept
/// in `.cache/ansible/`: its folder. A change to the pinned release makes
/// another environment.
fn venv(root: &Path) -> Result<PathBuf, String> {
    let requirement = format!("ansible-core=={CORE}");
    let key: u32 = requirement.bytes().fold(5381, |h, b| h.wrapping_mul(33) ^ u32::from(b));
    let dir = root.join(CACHE).join(format!("venv-{key:08x}"));
    let done = dir.join(".installed");
    if done.is_file() {
        return Ok(dir);
    }
    let install = "install Python 3.11 or later with venv (Debian, Ubuntu, Mint: `sudo apt install \
                   python3-venv`)";
    let check =
        format!("import sys; sys.exit(0 if sys.version_info >= (3, {PYTHON_MINOR}) else 1)");
    run(
        Command::new("python3").args(["-c", &check]),
        "python3, which ansible-core needs at 3.11 or later",
    )
    .map_err(|err| format!("{err}; {install}"))?;
    step(&format!("Installing {requirement}"));
    if dir.exists() {
        fs::remove_dir_all(&dir).map_err(|err| format!("removing {}: {err}", dir.display()))?;
    }
    run(Command::new("python3").args(["-m", "venv"]).arg(&dir), "python3 -m venv")
        .map_err(|err| format!("{err}; {install}"))?;
    let python = dir.join("bin/python");
    let mut pip = Command::new(&python);
    pip.args(["-m", "pip", "install", "--quiet", "--disable-pip-version-check", &requirement]);
    run(&mut pip, "pip install")?;
    fs::write(&done, &requirement).map_err(|err| format!("writing {}: {err}", done.display()))?;
    Ok(dir)
}

/// The collection's tarball installed into `.cache/ansible/`, once for each
/// tarball: the folder that `ANSIBLE_COLLECTIONS_PATH` takes.
fn install_collection(root: &Path, venv: &Path, tarball: &Path) -> Result<PathBuf, String> {
    let sum = sha256(tarball)?;
    let dir = root.join(CACHE).join(format!("collections-{}", &sum[..16]));
    let done = dir.join(".installed");
    if done.is_file() {
        return Ok(dir);
    }
    step(&format!("Installing the collection dukovo.system from {}", tarball.display()));
    if dir.exists() {
        fs::remove_dir_all(&dir).map_err(|err| format!("removing {}: {err}", dir.display()))?;
    }
    let mut galaxy = Command::new(venv.join("bin/ansible-galaxy"));
    // The folder is on the collections' path, so that galaxy does not warn
    // that a playbook would not find what it installs there.
    galaxy
        .args(["collection", "install", "--force", "-p"])
        .arg(&dir)
        .arg(tarball)
        .env("ANSIBLE_COLLECTIONS_PATH", &dir);
    run(&mut galaxy, "ansible-galaxy collection install")?;
    fs::write(&done, &sum).map_err(|err| format!("writing {}: {err}", done.display()))?;
    Ok(dir)
}

/// Gets what the playbooks run on: the environment, and the collection that
/// the lock pins. `root` is the top of the tree.
pub fn prepare(root: &Path) -> Result<Prepared, String> {
    let lock = Lock::read()?;
    let tarball = match tarball(root, &lock)? {
        Ok(tarball) => tarball,
        Err(line) => return Ok(Prepared::Skip(line)),
    };
    let venv = venv(root)?;
    let collections = install_collection(root, &venv, &tarball)?;
    Ok(Prepared::Ready(Tools { playbook: venv.join("bin/ansible-playbook"), collections }))
}

/// A machine to run the playbooks against, over SSH.
pub struct Machine<'a> {
    /// A folder of the test's own, where the inventory, the settings and
    /// what Ansible keeps go.
    pub dir: &'a Path,
    pub address: &'a str,
    pub port: u16,
    pub user: &'a str,
    /// The private key that signs the user in.
    pub key: &'a Path,
    /// The machine's host key as `ssh-keyscan` read it, a line of
    /// known_hosts, which is the only key the connection takes.
    pub known_hosts: &'a Path,
    /// The user's password, which elevate asks for when a task has `become`.
    pub password: &'a str,
}

/// `text` as a YAML string in single quotes.
fn quoted(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

impl Machine<'_> {
    /// The inventory: one host, [`HOST`], with the user, the key that signs
    /// in and the only host key that is taken, Python by its path and the
    /// password `become` gives elevate.
    pub fn inventory(&self) -> String {
        let ssh = format!(
            "-F /dev/null -o StrictHostKeyChecking=yes -o UserKnownHostsFile={} -o IdentitiesOnly=yes \
             -o BatchMode=yes -o LogLevel=ERROR -o ConnectTimeout=30",
            self.known_hosts.display()
        );
        let mut text = String::from("all:\n  hosts:\n");
        let _ = writeln!(text, "    {HOST}:");
        let _ = writeln!(text, "      ansible_host: {}", quoted(self.address));
        let _ = writeln!(text, "      ansible_port: {}", self.port);
        let _ = writeln!(text, "      ansible_user: {}", quoted(self.user));
        let _ = writeln!(
            text,
            "      ansible_ssh_private_key_file: {}",
            quoted(&self.key.display().to_string())
        );
        let _ = writeln!(text, "      ansible_ssh_common_args: {}", quoted(&ssh));
        let _ = writeln!(text, "      ansible_python_interpreter: {}", quoted(PYTHON));
        let _ = writeln!(text, "      ansible_become_password: {}", quoted(self.password));
        text
    }

    /// The settings: the inventory beside them, no colors, a connection for
    /// each task rather than one kept open, and a place of the test's own
    /// for what Ansible keeps on the host.
    pub fn settings(&self) -> String {
        format!(
            "[defaults]\ninventory = {dir}/inventory.yml\nhost_key_checking = True\nnocolor = True\n\
             forks = 1\ntimeout = 120\nlocal_tmp = {dir}/tmp\nhome = {dir}/home\n\
             deprecation_warnings = False\n\n[ssh_connection]\nssh_args = -o ControlMaster=no\n\
             retries = 3\n",
            dir = self.dir.display()
        )
    }

    /// Writes the inventory and the settings into the folder.
    pub fn write(&self) -> Result<(), String> {
        fs::create_dir_all(self.dir)
            .map_err(|err| format!("making {}: {err}", self.dir.display()))?;
        for (name, text) in [("inventory.yml", self.inventory()), ("ansible.cfg", self.settings())]
        {
            let file = self.dir.join(name);
            fs::write(&file, text).map_err(|err| format!("writing {}: {err}", file.display()))?;
        }
        Ok(())
    }
}

impl Tools {
    /// `ansible-playbook` of the environment, with the collection, for
    /// `machine`, running `playbook` with `args` after it. The machine's
    /// files must have been written.
    pub fn command(&self, machine: &Machine<'_>, playbook: &Path, args: &[&str]) -> Command {
        let mut command = Command::new(&self.playbook);
        command
            .arg(playbook)
            .args(args)
            .current_dir(machine.dir)
            .env("ANSIBLE_CONFIG", machine.dir.join("ansible.cfg"))
            .env("ANSIBLE_COLLECTIONS_PATH", &self.collections)
            // Setup's other subsets read Linux's /proc and /sys.
            .env("ANSIBLE_GATHER_SUBSET", "min")
            .env("PYTHONUNBUFFERED", "1");
        command
    }
}

/// How a playbook ended, from its recap.
#[derive(Debug, Default, PartialEq, Eq, Clone, Copy)]
pub struct Recap {
    pub ok: u32,
    pub changed: u32,
    pub unreachable: u32,
    pub failed: u32,
    pub skipped: u32,
    pub rescued: u32,
    pub ignored: u32,
}

impl Recap {
    /// The recap of [`HOST`] in what ansible-playbook printed.
    pub fn of(output: &str) -> Result<Recap, String> {
        let at = output.rfind("PLAY RECAP").ok_or("ansible-playbook printed no PLAY RECAP")?;
        let line = output[at..]
            .lines()
            .skip(1)
            .find(|line| line.trim_start().starts_with(HOST))
            .ok_or_else(|| format!("the PLAY RECAP has no line for {HOST}"))?;
        let mut recap = Recap::default();
        for (key, value) in line.split_whitespace().filter_map(|word| word.split_once('=')) {
            let count = value.parse().map_err(|_| format!("the PLAY RECAP says {key}={value}"))?;
            match key {
                "ok" => recap.ok = count,
                "changed" => recap.changed = count,
                "unreachable" => recap.unreachable = count,
                "failed" => recap.failed = count,
                "skipped" => recap.skipped = count,
                "rescued" => recap.rescued = count,
                "ignored" => recap.ignored = count,
                _ => {}
            }
        }
        Ok(recap)
    }

    /// Whether every task that ran did well.
    pub fn clean(&self) -> bool {
        self.unreachable == 0 && self.failed == 0 && self.rescued == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIGESTS: &str = "0123456789abcdef";

    #[test]
    fn the_lock_of_the_tree_reads() {
        let lock = Lock::read().expect("the lock reads");
        assert_eq!(
            lock,
            Lock {
                source: LockSource::File("DUKOVO_ANSIBLE_COLLECTION".into()),
                commit: None,
                sha256: None
            },
            "while the collection has no repository, the lock names its tarball by this variable"
        );
    }

    #[test]
    fn a_lock_that_names_a_url_pins_its_commit_and_its_tarball() {
        let commit = DIGESTS.repeat(2) + &DIGESTS.repeat(2)[..8];
        let sum = DIGESTS.repeat(4);
        let url = "https://example.org/dukovo-system-0.1.0.tar.gz";
        let text = format!("source = {url}\ncommit = {commit}\n# a comment\nsha256 = {sum}\n");
        assert_eq!(
            Lock::parse(&text),
            Ok(Lock {
                source: LockSource::Url(url.into()),
                commit: Some(commit.clone()),
                sha256: Some(sum.clone())
            })
        );
        for (text, wrong) in [
            (format!("source = {url}\ncommit = -\nsha256 = {sum}\n"), "needs its commit"),
            (format!("source = {url}\ncommit = {commit}\nsha256 = -\n"), "needs its sha256"),
            (
                format!("source = {url}\ncommit = {commit}\nsha256 = {}\n", sum.to_uppercase()),
                "lowercase",
            ),
            (format!("source = {url}\ncommit = abc\nsha256 = {sum}\n"), "40 lowercase"),
        ] {
            let err = Lock::parse(&text).expect_err("a lock that does not pin it");
            assert!(err.contains(wrong), "{err} does not say {wrong}");
        }
    }

    #[test]
    fn a_lock_that_is_not_one_is_refused() {
        for (text, wrong) in [
            ("source = env:X\ncommit = -\n", "sha256 is missing"),
            ("source = env:X\ncommit = -\nsha256 = -\nsha256 = -\n", "given twice"),
            ("source = env:X\ncommit = -\nsha256 = -\nbranch = main\n", "no key"),
            ("source = ftp://example.org/x\ncommit = -\nsha256 = -\n", "neither env:NAME"),
            ("source = http://example.org/x\ncommit = -\nsha256 = -\n", "neither env:NAME"),
            ("source = env:lower\ncommit = -\nsha256 = -\n", "no name of an environment variable"),
            ("source env:X\n", "no `key = value`"),
        ] {
            let err = Lock::parse(text).expect_err("a lock that is wrong");
            assert!(err.contains(wrong), "{err} does not say {wrong}");
        }
    }

    #[test]
    fn a_tarball_given_by_a_variable_is_skipped_when_the_variable_is_unset() {
        let lock = Lock {
            source: LockSource::File("DUKOVO_ANSIBLE_NO_SUCH_VARIABLE".into()),
            commit: None,
            sha256: None,
        };
        let line =
            tarball(Path::new("/nonexistent"), &lock).expect("not an error").expect_err("skipped");
        assert!(line.starts_with("ansible: skipped: DUKOVO_ANSIBLE_NO_SUCH_VARIABLE is not set"));
        assert!(line.contains(LOCK_FILE));
    }

    #[test]
    fn the_inventory_names_the_host_the_key_and_the_one_host_key_it_takes() {
        let dir = Path::new("/work/ansible");
        let machine = Machine {
            dir,
            address: "127.0.0.1",
            port: 2222,
            user: "alice",
            key: Path::new("/work/keys/alice"),
            known_hosts: Path::new("/work/keys/known_hosts"),
            password: "queen of 'hearts'",
        };
        let text = machine.inventory();
        for line in [
            "    dukovo:\n",
            "      ansible_host: '127.0.0.1'\n",
            "      ansible_port: 2222\n",
            "      ansible_user: 'alice'\n",
            "      ansible_ssh_private_key_file: '/work/keys/alice'\n",
            "UserKnownHostsFile=/work/keys/known_hosts",
            "StrictHostKeyChecking=yes",
            "      ansible_python_interpreter: '/applications/org.python.python/bin/python3'\n",
            "      ansible_become_password: 'queen of ''hearts'''\n",
        ] {
            assert!(text.contains(line), "the inventory lacks {line:?}:\n{text}");
        }
        let settings = machine.settings();
        assert!(settings.contains("inventory = /work/ansible/inventory.yml"));
        assert!(settings.contains("ControlMaster=no"));
    }

    #[test]
    fn the_recap_of_the_host_is_read() {
        let output = "TASK [x] ***\nok: [dukovo]\n\nPLAY RECAP *****\n\
                      dukovo                     : ok=12   changed=5    unreachable=0    failed=0    \
                      skipped=1    rescued=0    ignored=0   \n";
        let recap = Recap::of(output).expect("a recap");
        assert_eq!(
            recap,
            Recap {
                ok: 12,
                changed: 5,
                unreachable: 0,
                failed: 0,
                skipped: 1,
                rescued: 0,
                ignored: 0
            }
        );
        assert!(recap.clean());
        let failed = output.replace("failed=0", "failed=1");
        assert!(!Recap::of(&failed).expect("a recap").clean());
        assert!(Recap::of("no recap here").is_err());
        assert!(Recap::of("PLAY RECAP ***\nother : ok=1 changed=0\n").is_err());
    }

    /// The playbooks and the lab's script, which `ob test boot --case
    /// ansible` and `system/ansible` run.
    const SITE: &str = include_str!("test/ansible/site.yml");
    const PYTHON_YML: &str = include_str!("test/ansible/python.yml");
    const UPDATE: &str = include_str!("test/ansible/update.yml");
    const LAB_SCRIPT: &str = include_str!("../../../lab/tests/system/ansible.sh");
    const LAB_TEST: &str = include_str!("../../../lab/tests/system/ansible.toml");

    #[test]
    fn the_site_playbook_runs_each_module_once_and_checks_the_system_name() {
        for module in ["bundle", "feature", "service", "snapshot", "update", "facts"] {
            assert!(
                SITE.contains(&format!("dukovo.system.{module}:")),
                "site.yml does not run dukovo.system.{module}"
            );
        }
        assert!(SITE.contains("ansible_facts.distribution == \"Dukovo\""));
        assert!(SITE.contains("hosts: dukovo"), "the host of the inventory is {HOST}");
    }

    #[test]
    fn python_comes_by_raw_and_the_update_restarts_into_the_update() {
        assert!(PYTHON_YML.contains("ansible.builtin.raw:"));
        assert!(!PYTHON_YML.contains("dukovo.system."), "no module runs before Python is there");
        assert!(UPDATE.contains("dukovo.system.update:"));
        assert!(UPDATE.contains("state: applied") && UPDATE.contains("reboot: true"));
        assert!(UPDATE.contains("rebooted"), "update.yml must check that the machine restarted");
    }

    #[test]
    fn the_lab_runs_the_same_core_the_same_lock_and_the_same_playbooks() {
        assert!(
            LAB_SCRIPT.contains(&format!("ansible-core=={CORE}")),
            "lab/tests/system/ansible.sh pins another ansible-core than {CORE}"
        );
        assert!(LAB_SCRIPT.contains(LOCK_FILE), "the lab's script does not read {LOCK_FILE}");
        assert!(LAB_SCRIPT.contains(PLAYBOOKS), "the lab's script does not read {PLAYBOOKS}");
        for playbook in ["site.yml", "python.yml", "update.yml"] {
            assert!(
                LAB_TEST
                    .contains(&format!("ansible.sh {{machine}} {{machine-port:22}} {playbook}")),
                "the lab's test does not run {playbook}"
            );
        }
        assert!(LAB_SCRIPT.contains(PYTHON), "the lab's script names another Python than {PYTHON}");
    }
}

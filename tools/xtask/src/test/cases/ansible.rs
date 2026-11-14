// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Ansible manages an installed system with the collection dukovo.system,
//! over SSH through a port QEMU forwards, with ansible-core on the host
//! (src/ansible.rs). alice, the owner, puts a key of the test's in her
//! authorized_keys and turns sshd on, as the *help-session* test does. At
//! the console an administrator adds the test catalog, which lists
//! ripgrep, from the second disk, and turns discoverd off.
//!
//! The collection is the tarball that src/test/ansible.lock pins. While the
//! collection's repository does not exist, the lock names the tarball by
//! the environment variable DUKOVO_ANSIBLE_COLLECTION, and when that is
//! not set the test is skipped, with a line that says why, and passes: a
//! run without the tarball, as CI's, does not fail. The host also needs
//! Python 3.13 or later with venv, which makes ansible-core's environment
//! once, in .cache/ansible/, and the bundle files `ob` packs, as the
//! *python* test does. ansible-core is 2.22, the first release whose
//! modules run under the machine's Python 3.15, and it needs that Python.
//!
//! The first playbook, python.yml, installs Python on the machine, which
//! has none, with `ansible.builtin.raw`: the bundle org.python.python, from
//! its file on the first disk, for everyone; Ansible then runs its modules
//! with the interpreter at /applications/org.python.python/bin/python3,
//! by its path. The second, site.yml, makes `ansible.builtin.setup` say the
//! system is Dukovo, and runs each module of the collection as alice
//! through elevate: it gathers Dukovo's facts, adds the feature
//! file-sharing, turns discoverd on, installs ripgrep from the test catalog,
//! makes a snapshot of /users and sets the update channel. Run again it
//! reports changed=0. With the feature removed at the console, running it
//! with --check and --diff reports that the feature would be added, shows it
//! as the JSON of its state before and after, and leaves it not added.

use super::super::*;
use super::help_session::{
    PASSWORD, Session, as_root, free_port, keys, make_alice, scan_host_key, sshd_on,
};
use crate::ansible::{self, Machine, Prepared, Recap, Tools};

/// The keys', the known hosts' and Ansible's own folder, from the top of
/// the tree, which QEMU runs in.
const DIR: &str = "out/test/ansible";

/// The bundle files' disk label, and where Spark mounts the disk: the
/// first virtio disk after the installed system's.
const BUNDLES: &str = "/volumes/vda";

/// The test catalog's disk label, and where Spark mounts it: the second.
const CATALOG: &str = "/volumes/vdb";

/// The feature site.yml adds.
const FEATURE: &str = "file-sharing";

/// How long a playbook may take, in seconds: every module starts Python,
/// under emulation, and the first install copies a bundle.
const PLAYBOOK_SECONDS: u64 = 1800;

/// The bundle files, which hold Python's, and the test catalog.
const DISKS: &[Disk] =
    &[bundle_files_disk(128 * 1024, "BUNDLES"), catalog_site_disk(64 * 1024, "CATALOG")];

pub const CASE: Case = Case {
    name: "ansible",
    about: "Ansible's collection dukovo.system manages an installed system over SSH from the host (skipped, passing, while DUKOVO_ANSIBLE_COLLECTION, which src/test/ansible.lock names, is not set): Python is installed with ansible.builtin.raw and found by its path; site.yml has setup name the system Dukovo and runs facts, feature, service, bundle from the test catalog, snapshot and update's channel through elevate, and run again reports changed=0; with the feature removed at the console, --check --diff reports it would be added, as JSON before and after, and changes nothing",
    firmware: Firmware::Uefi,
    media: Media::Disk,
    cmdline: "",
    cpus: 2,
    // Each module starts a Python of its own.
    memory: "2G",
    reboot: false,
    image: TestImage::Installed,
    cpu_model: None,
    qemu: &["-netdev", "user,id=n0", "-device", "virtio-net-pci,netdev=n0"],
    script: ansible,
    disks: DISKS,
    second: None,
};

/// Runs `playbook` of src/test/ansible with `args` against the machine,
/// and gives what it printed, with its recap. Fails if a task failed or the
/// machine could not be reached.
fn play(
    tools: &Tools,
    machine: &Machine<'_>,
    playbook: &str,
    args: &[&str],
) -> Result<(String, Recap), String> {
    let root = std::env::current_dir().map_err(|err| err.to_string())?;
    let file = root.join(ansible::PLAYBOOKS).join(playbook);
    let what = format!("ansible-playbook {playbook} {}", args.join(" "));
    let mut session = Session::start(tools.command(machine, &file, args), &what)?;
    session.close_input();
    let status = session.finished(PLAYBOOK_SECONDS)?;
    let said = session.said();
    let recap =
        Recap::of(&said).map_err(|err| format!("{what}: {err}; it said: {}", tail(&said)))?;
    if status != Some(0) || !recap.clean() {
        return Err(format!("{what} ended with {status:?}, {recap:?}; it said: {}", tail(&said)));
    }
    Ok((said, recap))
}

/// The end of what a playbook printed, for an error: what it did last.
fn tail(said: &str) -> String {
    let from = said.char_indices().rev().nth(3000).map_or(0, |(at, _)| at);
    said[from..].trim().to_string()
}

/// The catalog on the second disk added by an administrator, and the
/// image's own turned off, so that nothing here reaches the network; and a
/// service that site.yml turns on, discoverd, turned off.
fn prepared(vm: &mut Vm) -> Result<(), String> {
    vm.expect_anywhere(&format!("spark: mounted the FAT filesystem of vda at {BUNDLES}"))?;
    vm.expect_anywhere(&format!("spark: mounted the FAT filesystem of vdb at {CATALOG}"))?;
    let add = format!("bundle catalog add test file://{CATALOG}/catalog {CATALOG}/root.json");
    answer_prompt(vm, "$", &format!("clear; {}", as_root(&add)))?;
    vm.expect_screen(&["added catalog test, which lists 1 bundle"])?;
    answer_prompt(vm, "$", &format!("clear; {}", as_root("bundle catalog remove dukovo")))?;
    vm.expect_screen(&["turned catalog dukovo off"])?;
    answer_prompt(
        vm,
        "$",
        &format!(
            "clear; {}; {} | grep discoverd",
            as_root("service discoverd disable"),
            as_root("service --status-all")
        ),
    )?;
    vm.expect_screen(&[" [ - ]  discoverd"])
}

fn ansible(vm: &mut Vm, _case: &Case) -> Result<(), String> {
    let root = std::env::current_dir().map_err(|err| err.to_string())?;
    let tools = match ansible::prepare(&root)? {
        Prepared::Ready(tools) => tools,
        Prepared::Skip(line) => {
            eprintln!("{line}");
            return Ok(());
        }
    };
    let dir = root.join(DIR);
    let alice_key = keys(&dir)?;
    make_alice(vm)?;
    let port = free_port()?;
    sshd_on(vm, &alice_key, port)?;
    let host_key = scan_host_key(port)?;
    let known_hosts = dir.join("known_hosts");
    std::fs::write(&known_hosts, format!("{host_key}\n"))
        .map_err(|err| format!("writing {}: {err}", known_hosts.display()))?;
    prepared(vm)?;

    let key = dir.join("alice");
    let work = dir.join("run");
    let machine = Machine {
        dir: &work,
        address: "127.0.0.1",
        port,
        user: "alice",
        key: &key,
        known_hosts: &known_hosts,
        password: PASSWORD,
    };
    machine.write()?;

    // Python, which the machine has none of, from its file.
    let from = format!("dukovo_python_from={BUNDLES}/org.python.python-*.dkv");
    let (_, installed) = play(&tools, &machine, "python.yml", &["-e", &from])?;
    if installed.changed != 1 {
        return Err(format!(
            "python.yml changed {} tasks, not the install alone",
            installed.changed
        ));
    }

    // Each module once.
    let (_, first) = play(&tools, &machine, "site.yml", &[])?;
    if first.changed < 5 {
        return Err(format!(
            "site.yml changed {} tasks: a feature, a service, a bundle, a snapshot and the channel \
             are five",
            first.changed
        ));
    }
    // And again: nothing changes.
    let (_, again) = play(&tools, &machine, "site.yml", &[])?;
    if again.changed != 0 {
        return Err(format!("site.yml changed {} tasks the second time, not none", again.changed));
    }

    // A change at the console: --check --diff says the feature would be
    // added, and the diff is its state before and after as JSON; the
    // machine is as it was.
    answer_prompt(vm, "$", &format!("clear; {}", as_root(&format!("feature remove {FEATURE}"))))?;
    vm.expect_screen(&["File sharing is removed."])?;
    let (said, checked) = play(&tools, &machine, "site.yml", &["--check", "--diff"])?;
    if checked.changed != 1 {
        return Err(format!(
            "site.yml --check changed {} tasks, not the feature alone",
            checked.changed
        ));
    }
    for needle in [format!("\"{FEATURE}\": \"not added\""), format!("\"{FEATURE}\": \"added\"")] {
        if !said.contains(&needle) {
            return Err(format!("the diff does not show {needle}; it said: {}", tail(&said)));
        }
    }
    answer_prompt(vm, "$", &format!("clear; feature list | grep {FEATURE}"))?;
    vm.expect_screen(&[&format!("{FEATURE}  not added  File sharing")])?;
    super::install_uefi::power_off(vm)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_disks_are_where_the_test_says_spark_mounts_them() {
        // The first virtio disk after the installed system's is vda, the
        // next vdb, in the order of DISKS.
        assert_eq!(DISKS.len(), 2);
        assert_eq!(BUNDLES, "/volumes/vda");
        assert_eq!(CATALOG, "/volumes/vdb");
    }

    #[test]
    fn the_playbooks_are_in_the_tree() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        for playbook in ["python.yml", "site.yml", "update.yml"] {
            let file = root.join(ansible::PLAYBOOKS).join(playbook);
            assert!(file.is_file(), "{} is not there", file.display());
        }
    }

    #[test]
    fn the_end_of_what_a_playbook_said_is_kept_whole_when_it_is_short() {
        assert_eq!(tail("  one\n  two\n"), "one\n  two");
        let long = "x".repeat(10_000);
        assert_eq!(tail(&long).len(), 3001);
    }
}

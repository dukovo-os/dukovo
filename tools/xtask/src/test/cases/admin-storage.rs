// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The Storage of the web console, Dukovo Admin: what it shows of the
//! disks and the Btrfs volumes, a disk's health, a scrub as a task, a disk
//! grown between boots, and the log's limits (ADR 0561). alice, the owner,
//! is an administrator; bob may use the console and is no administrator.
//!
//! The machine has five disks. `sda` is a SATA disk, which keeps a
//! record of its health; `vda` and `vdb` are the two disks of a Btrfs
//! RAID1 the host made, labelled LAB, which Spark mounts at `/volumes/vda`
//! as one volume; `vdc` is blank, and the first boot gives it a GPT with
//! one partition of 128 MiB and a Btrfs, labelled GROW. The boot stick is
//! the fifth.
//!
//! The first boot. `GET /api/1/storage` is what `storage show --json` prints
//! in alice's pane, for alice and for bob: the pair's profiles are all
//! raid1, its mount and its two disks are what the kernel says to anyone,
//! and its size, used and free agree with `df -B1`, and every disk's size
//! with `lsblk -b`. The GPT name, filesystem and label of `vdc1`, not
//! mounted, are told only to root, who opens the disk. `storage.health` is ADM-122 for
//! bob and ADM-106 for alice while locked; unlocked, `sda` gives its
//! attributes, `vda`, a virtio disk, gives none, and a disk that is not
//! there is DSK-101. `storage.scrub` of `/volumes/vda` is a task that ends
//! done and says in its log that it found no errors, and `storage show` has
//! the result for the volume. `storage.usage` is a list of series or, where
//! metricsd has no history yet, ADM-123, as the command says, and a range
//! that is none is ADM-125. `storage.log` is what `logtrim --show --json`
//! prints; `storage.log-set` changes the file's line and no other, says it
//! changed nothing when it is asked again, and its undo puts the file back
//! as it was; `storage.log-trim` is done.
//!
//! Between the boots the host makes `vdc` 64 MiB larger. The second boot
//! starts afresh from the stick, which keeps nothing of the first, so alice
//! makes the owner's account again, and bob's. In the second boot,
//! Spark has mounted the Btrfs of `vdc1`, and `storage show` says
//! how much room there is after the last partition. `storage.grow` of a
//! name that is no disk, of a partition and of a disk with no partition is
//! refused with what the command says, and for bob and for alice locked it
//! is ADM-122 and ADM-106; of `vdc` it makes the partition and the
//! filesystem larger, so that a file larger than they were fits, and a
//! second time it changes nothing.
//!
//! What the pane's commands print reaches the host through nc, to a port on
//! the host's loopback that QEMU's user network gives the guest at
//! 10.0.2.2.

use std::process::Command;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::super::console_client::{ALICE_PASSWORD, Browser, Console, Pane, Reply, elevated};
use super::super::*;

/// The password of bob, whom alice makes: he may use the console, and is
/// no administrator.
const BOB_PASSWORD: &str = "looking-glass";

/// Where Spark mounts the RAID1 of `vda` and `vdb`, and the Btrfs of `vdc1`.
const PAIR_MOUNT: &str = "/volumes/vda";
const GROWN_MOUNT: &str = "/volumes/vdc1";

/// The test.
pub const CASE: Case = Case {
    name: "admin-storage",
    about: "The web console's Storage: GET /api/1/storage is what storage show --json prints in alice's pane, and bob's the same, with the RAID1 pair's profiles raid1, its two disks as the kernel says them to anyone, and its mount, size, used and free as df -B1 gives them, every disk's size as lsblk -b does, and the GPT name, filesystem and label of a partition not mounted told to root alone; storage.health is ADM-122 for bob and ADM-106 for alice locked, the attributes of the SATA disk when unlocked, none for a virtio disk and DSK-101 for a disk that is none; storage.scrub is a task that ends done with no errors, and storage show has it kept for the volume; storage.usage is a list or ADM-123 as the command says; storage.log is logtrim --show --json, storage.log-set changes the one line of the settings file and changes nothing the second time, its undo puts the file back, and storage.log-trim is done; with a disk made 64 MiB larger between boots, storage show gives the room after its last partition, storage.grow refuses a name that is none, a partition and a disk with no partition, grows the partition and its mounted Btrfs so that a file larger than they were fits, and changes nothing the second time (ADR 0561)",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    // As a normal boot: Spark starts the services, admind among them.
    cmdline: "lockdep",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &["-netdev", "user,id=n0", "-device", "virtio-net-pci,netdev=n0"],
    script: first_boot,
    disks: DISKS,
    second: Some(SecondBoot { cmdline: "lockdep", before: grown, script: second_boot }),
};

/// The size of the disks of the RAID1, in KiB. The images are sparse.
const PAIR_KIB: u64 = 512 * 1024;

/// The disks, in the order they are named in: the SATA disk `sda`, the
/// RAID1's two virtio disks, `vda` and `vdb`, which the host makes whole,
/// and the blank virtio disk `vdc`, which the first boot gives a GPT and a
/// Btrfs and the host grows between the boots.
const DISKS: &[Disk] = &[
    raw_disk(Bus::Sata, raw_unchanged),
    Disk { kib: PAIR_KIB, label: "LAB", ..btrfs_disk(Bus::Virtio, 512, BTRFS_RAID, unchecked) },
    Disk { kib: PAIR_KIB, label: "LAB", ..btrfs_disk(Bus::Virtio, 512, BTRFS_RAID, unchecked) },
    Disk {
        kib: 192 * 1024,
        fat: 0,
        cluster: 0,
        label: "",
        table: Table::None,
        bus: Bus::Virtio,
        sector: 512,
        device: "",
        errors: None,
        read_only: false,
        check: grown_table,
    },
];

/// The disk `vdc` before the host grows it: 192 MiB, and after, 256 MiB.
const SMALL: u64 = 192 << 20;
const GROWN: u64 = 256 << 20;

/// The size of the partition the first boot makes on `vdc`.
const PARTITION: u64 = 128 << 20;

/// What a disk of this test needs no check of on the host.
fn unchecked(_disk: &DiskImage, _log: &str) -> Result<(), String> {
    Ok(())
}

/// A GPT with one partition of 128 MiB, named Data.
const LAYOUT: &str =
    r#"{"table": "gpt", "partitions": [{"type": "linux", "size": "128MiB", "name": "Data"}]}"#;

/// Checks on the host, after each boot, that sgdisk finds the table of
/// `vdc` sound, and that partition 1 ends at its first size's last sector
/// after the first boot and at the grown disk's last usable sector after
/// the second.
fn grown_table(disk: &DiskImage, _log: &str) -> Result<(), String> {
    let said = host_tool(Command::new(fat_tool("sgdisk")).arg("-v").arg(&disk.path))?;
    let said = String::from_utf8_lossy(&said);
    if !said.contains("No problems found") {
        return Err(format!("sgdisk -v found problems in the table:\n{said}"));
    }
    let bytes = std::fs::metadata(&disk.path)
        .map_err(|err| format!("{}: {err}", disk.path.display()))?
        .len();
    let last = if bytes == GROWN { "Last sector: 524254 " } else { "Last sector: 264191 " };
    let said = host_tool(Command::new(fat_tool("sgdisk")).arg("-i").arg("1").arg(&disk.path))?;
    let said = String::from_utf8_lossy(&said);
    if !said.contains(last) {
        return Err(format!("sgdisk does not give partition 1's \"{last}\":\n{said}"));
    }
    Ok(())
}

/// Between the boots: `vdc` grows to [`GROWN`], as a cloud's disk does.
fn grown(_case: &Case, disks: &[DiskImage]) -> Result<(), String> {
    let Some(disk) = disks.get(3) else { return Err("the case has no fourth disk".into()) };
    std::fs::File::options()
        .write(true)
        .open(&disk.path)
        .and_then(|file| file.set_len(GROWN))
        .map_err(|err| format!("growing {}: {err}", disk.path.display()))
}

/// Who asks the console.
#[derive(Clone, Copy)]
enum Who {
    Alice,
    Bob,
}

/// What the test keeps from one step to the next.
struct Test {
    console: Console,
    pane: Pane,
    alice: Browser,
    bob: Browser,
}

impl Test {
    /// Opens the console and the pane once the machine has its network and
    /// admind answers.
    fn open(vm: &mut Vm) -> Result<Test, String> {
        vm.expect_anywhere("netd: net0 has 10.0.2.15/24")?;
        vm.expect_anywhere("admind: answers at ")?;
        Ok(Test {
            console: Console::open(vm)?,
            pane: Pane::open()?,
            alice: Browser::default(),
            bob: Browser::default(),
        })
    }

    /// Signs alice and bob in, as the console's page does, each with a
    /// session of its own, locked.
    fn sign_in(&mut self) -> Result<(), String> {
        self.alice = Browser::default();
        self.console.sign_in(&mut self.alice, "alice", ALICE_PASSWORD)?;
        self.console.read_session(&mut self.alice)?;
        self.bob = Browser::default();
        self.console.sign_in(&mut self.bob, "bob", BOB_PASSWORD)?;
        self.console.read_session(&mut self.bob)?;
        Ok(())
    }

    /// A read of `path` for `who`, whatever the answer.
    fn get(&mut self, who: Who, path: &str) -> Result<Reply, String> {
        let console = &self.console;
        let browser = match who {
            Who::Alice => &mut self.alice,
            Who::Bob => &mut self.bob,
        };
        console.get(browser, path)
    }

    /// What `path` says to `who`, which must be 200.
    fn read(&mut self, who: Who, path: &str) -> Result<Value, String> {
        self.get(who, path)?.must(path, 200)?.json()
    }

    /// A change at `path` with `body`, for `who`, whatever the answer.
    fn post(&mut self, who: Who, path: &str, body: &Value) -> Result<Reply, String> {
        let console = &self.console;
        let browser = match who {
            Who::Alice => &mut self.alice,
            Who::Bob => &mut self.bob,
        };
        console.post(browser, path, body)
    }

    /// Unlocks alice's session with her password, if it is locked.
    fn unlocked(&mut self) -> Result<(), String> {
        let session =
            self.console.get(&mut self.alice, "/api/1/session")?.must("her session", 200)?;
        if session.json()?["unlock_seconds"].as_u64().is_some_and(|left| left > 120) {
            return Ok(());
        }
        let unlock = json!({"kind": "password", "password": ALICE_PASSWORD});
        self.console.post(&mut self.alice, "/api/1/unlock", &unlock)?.must("her unlocking", 200)?;
        Ok(())
    }

    /// Locks alice's session, as the page's Lock does.
    fn locked(&mut self) -> Result<(), String> {
        self.post(Who::Alice, "/api/1/lock", &json!({}))?.must("alice's lock", 200).map(drop)
    }

    /// A change that bob may not ask for, ADM-122, and alice may not while
    /// her session is locked, ADM-106: she is locked first, and the caller
    /// unlocks her again. `what` says which in the failure.
    fn refused_for_both(&mut self, what: &str, path: &str, body: &Value) -> Result<(), String> {
        self.locked()?;
        self.post(Who::Bob, path, body)?.refused(
            &format!("{what} asked by bob"),
            403,
            "ADM-122",
        )?;
        let locked = self.post(Who::Alice, path, body)?;
        locked.refused(&format!("{what} asked by alice, locked"), 403, "ADM-106")?;
        Ok(())
    }

    /// A read that runs as an administrator, which bob may not ask for,
    /// ADM-122, and alice may not while her session is locked, ADM-106.
    fn read_refused_for_both(&mut self, what: &str, path: &str) -> Result<(), String> {
        self.locked()?;
        self.get(Who::Bob, path)?.refused(&format!("{what} read by bob"), 403, "ADM-122")?;
        let locked = self.get(Who::Alice, path)?;
        locked.refused(&format!("{what} read by alice, locked"), 403, "ADM-106")?;
        Ok(())
    }

    /// The task `id` once `ready` is true of it, within `seconds`.
    fn task_where(
        &mut self,
        id: i64,
        seconds: u64,
        ready: impl Fn(&Value) -> bool,
    ) -> Result<Value, String> {
        let deadline = Instant::now() + Duration::from_secs(seconds);
        loop {
            let task = self.read(Who::Alice, &format!("/api/1/tasks/{id}"))?;
            if ready(&task) {
                return Ok(task);
            }
            if Instant::now() >= deadline {
                return Err(format!("task {id} is not as wanted after {seconds} s: {task}"));
            }
            std::thread::sleep(Duration::from_millis(500));
        }
    }

    /// What `storage show --json` prints in the pane, as JSON.
    fn typed_show(&self, vm: &mut Vm) -> Result<Value, String> {
        self.typed(vm, "storage show --json")
    }

    /// What `command`, which prints JSON, prints in the pane.
    fn typed(&self, vm: &mut Vm, command: &str) -> Result<Value, String> {
        let text = self.pane.output(vm, command)?;
        serde_json::from_str(text.trim())
            .map_err(|err| format!("{command} printed {text:?}: {err}"))
    }

    /// What `GET /api/1/storage` says to `who`, once it is what
    /// `storage show --json` prints in the pane at the same time, as far as
    /// numbers that move allow: the two are taken one after the other, up
    /// to four times.
    fn show_as_typed(&mut self, vm: &mut Vm, who: Who) -> Result<Value, String> {
        let mut last = String::new();
        for _ in 0..4 {
            let typed = self.typed_show(vm)?;
            let routed = self.read(who, "/api/1/storage")?;
            if steady(&typed) == steady(&routed) {
                return Ok(routed);
            }
            last = format!("the pane has {}, and the route {}", steady(&typed), steady(&routed));
        }
        Err(format!("GET /api/1/storage is not what storage show --json prints: {last}"))
    }

    /// What `df -B1` and `lsblk -b` say, in bytes, to compare with the
    /// answer: the size, space used and space available of the filesystem
    /// mounted at `mount`.
    fn df(&self, vm: &mut Vm, mount: &str) -> Result<[u64; 3], String> {
        let said = self.pane.output(vm, &format!("df -B1 {mount}"))?;
        let line = said.lines().rev().find(|line| line.contains(mount));
        let fields: Vec<u64> = line.map_or_default(|line| {
            line.split_whitespace().filter_map(|word| word.parse().ok()).collect()
        });
        match fields[..] {
            [size, used, free, ..] => Ok([size, used, free]),
            _ => Err(format!("df -B1 {mount} printed {said:?}")),
        }
    }

    /// The size in bytes that `lsblk -b` gives each of `names`, in order.
    /// lsblk lists a named disk's partitions after it, as vdc's once it
    /// has one; only the devices named count.
    fn lsblk(&self, vm: &mut Vm, names: &[&str]) -> Result<Vec<(String, u64)>, String> {
        let said =
            self.pane.output(vm, &format!("lsblk -b -l -n -o NAME,SIZE {}", names.join(" ")))?;
        let mut sizes = Vec::new();
        for line in said.lines().filter(|line| !line.trim().is_empty()) {
            let mut words = line.split_whitespace();
            match (words.next(), words.next().and_then(|size| size.parse().ok())) {
                (Some(name), Some(size)) if names.contains(&name) => {
                    sizes.push((name.to_string(), size));
                }
                (Some(_), Some(_)) => {}
                _ => return Err(format!("lsblk -b printed the line {line:?}")),
            }
        }
        Ok(sizes)
    }
}

/// `report` without what moves from one look to the next: the space used
/// and free of each volume and filesystem, and when the diskhealth service
/// last looked at a disk.
fn steady(report: &Value) -> Value {
    let mut report = report.clone();
    for key in ["volumes", "filesystems"] {
        for item in report[key].as_array_mut().into_iter().flatten() {
            for field in ["used", "free"] {
                item[field] = Value::Null;
            }
        }
    }
    for disk in report["disks"].as_array_mut().into_iter().flatten() {
        if disk["health"].is_object() {
            disk["health"]["checked_ms"] = Value::Null;
        }
    }
    report
}

/// [`steady`], and without what a person who may not open the disks
/// cannot be told, as the filesystem, label and ID on a partition and
/// where a volume is mounted, from which disks and how large.
fn without_what_devices_say(report: &Value) -> Value {
    let mut report = steady(report);
    for disk in report["disks"].as_array_mut().into_iter().flatten() {
        for partition in disk["partitions"].as_array_mut().into_iter().flatten() {
            for field in ["filesystem", "label", "uuid"] {
                partition[field] = Value::Null;
            }
        }
    }
    for volume in report["volumes"].as_array_mut().into_iter().flatten() {
        for field in ["mount", "size", "devices", "holds"] {
            volume[field] = Value::Null;
        }
    }
    report["filesystems"] = Value::Null;
    report
}

/// The volume of `report` labelled `label`.
fn volume<'a>(report: &'a Value, label: &str) -> Result<&'a Value, String> {
    let volumes = report["volumes"].as_array().ok_or("storage show has no volumes")?;
    volumes
        .iter()
        .find(|volume| volume["label"] == label)
        .ok_or_else(|| format!("storage show has no volume labelled {label}: {report}"))
}

/// The disk of `report` named `name`.
fn disk<'a>(report: &'a Value, name: &str) -> Result<&'a Value, String> {
    let disks = report["disks"].as_array().ok_or("storage show has no disks")?;
    disks
        .iter()
        .find(|disk| disk["name"] == name)
        .ok_or_else(|| format!("storage show has no disk {name}: {report}"))
}

/// A number of `value`, or the failure that says `what` is none.
fn number(value: &Value, what: &str) -> Result<u64, String> {
    value.as_u64().ok_or_else(|| format!("{what} is {value}, not a number"))
}

/// Makes bob, in group console and no administrator, once the owner alice
/// is signed in at the console, and starts the console's sessions for
/// alice and bob; alice's pane is elevated.
fn people(vm: &mut Vm) -> Result<Test, String> {
    let mut test = Test::open(vm)?;
    elevated(vm)?;
    answer_prompt(
        vm,
        "$",
        &format!(
            "clear; elevate sh -c 'echo {BOB_PASSWORD} | user add bob; user set --yes --console \
             bob'"
        ),
    )?;
    vm.expect("authd: changed the account bob")?;
    vm.expect_screen(&["Made bob"])?;
    test.sign_in()?;
    Ok(test)
}

/// The first boot: the machine as it is shown, health, a scrub, and the
/// log's limits, then `vdc` made ready to be grown.
fn first_boot(vm: &mut Vm, case: &Case) -> Result<(), String> {
    owner_signs_in(vm, case)?;
    let mut test = people(vm)?;

    a_disk_to_grow(vm, &test)?;
    the_machine_as_shown(vm, &mut test)?;
    health(&mut test)?;
    a_scrub(vm, &mut test)?;
    usage(vm, &mut test)?;
    the_log(vm, &mut test)?;

    vm.press_power_button()?;
    vm.expect("spark: every service has stopped")?;
    vm.expect("Power: `spark` asked to power off the machine; syncing the filesystems")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}

/// `vdc` gets its GPT with one partition of 128 MiB, and a Btrfs on it,
/// which Spark mounts at the next start. The layout goes to rdisk on its
/// standard input: alice's /tmp is her own (ADR 0255), and root's is not
/// hers.
fn a_disk_to_grow(vm: &mut Vm, test: &Test) -> Result<(), String> {
    let steps = [
        format!("echo '{LAYOUT}' | elevate rdisk --script - vdc"),
        "elevate mkfs-btrfs -q -L GROW vdc1".to_string(),
    ];
    for step in steps {
        let status = test.pane.status(vm, &step)?;
        if status != 0 {
            return Err(format!("`{step}` ended with status {status}"));
        }
    }
    Ok(())
}

/// Step 1: `GET /api/1/storage` for alice and for bob, and what the
/// machine says of the same in its own words.
fn the_machine_as_shown(vm: &mut Vm, test: &mut Test) -> Result<(), String> {
    let alice = test.show_as_typed(vm, Who::Alice)?;
    let bob = test.read(Who::Bob, "/api/1/storage")?;
    if alice["format"] != json!(1) {
        return Err(format!("storage show has the format {}, not 1", alice["format"]));
    }
    // What bob is told of what he cannot open may be less, and is no more
    // and no other than alice's.
    if without_what_devices_say(&bob) != without_what_devices_say(&alice) {
        return Err(format!("bob is shown {bob}, and alice {alice}"));
    }

    // The five disks, as `lsblk -b` has them: the SATA disk, the pair, the
    // grown one and the boot stick are all disks.
    let names: Vec<&str> = alice["disks"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|disk| disk["name"].as_str())
        .collect();
    for wanted in ["sda", "vda", "vdb", "vdc"] {
        if !names.contains(&wanted) {
            return Err(format!("storage show has no disk {wanted}, only {names:?}"));
        }
    }
    let sizes = test.lsblk(vm, &["sda", "vda", "vdb", "vdc"])?;
    if sizes.len() != 4 {
        return Err(format!("lsblk -b gave {sizes:?}, not four disks"));
    }
    for (name, size) in &sizes {
        let shown = number(&disk(&alice, name)?["size"], &format!("the size of {name}"))?;
        if shown != *size {
            return Err(format!("storage show gives {name} {shown} bytes, and lsblk -b {size}"));
        }
    }
    if disk(&alice, "vdc")?["size"] != json!(SMALL) {
        return Err(format!("vdc is not {SMALL} bytes before the host grows it"));
    }

    // The RAID1: the host made it, and Spark mounted it once.
    let pair = volume(&alice, "LAB")?;
    let copies = json!({"data": ["raid1"], "metadata": ["raid1"], "system": ["raid1"]});
    if pair["profiles"] != copies {
        return Err(format!("the pair's profiles are {}, not {copies}", pair["profiles"]));
    }
    let counts = (&pair["disks"], &pair["missing"], &pair["behind"]);
    if counts != (&json!(2), &json!(0), &json!(0)) {
        return Err(format!("the pair has {counts:?} of disks, missing and behind"));
    }
    if pair["mount"] != json!(PAIR_MOUNT) || pair["devices"] != json!(["vda", "vdb"]) {
        return Err(format!("the pair is at {} from {}", pair["mount"], pair["devices"]));
    }
    let [size, used, free] = test.df(vm, PAIR_MOUNT)?;
    let shown = [&pair["size"], &pair["used"], &pair["free"]];
    if shown != [&json!(size), &json!(used), &json!(free)] {
        return Err(format!("the pair is {shown:?} bytes, and df -B1 gives {size} {used} {free}"));
    }
    if !pair["scrub"].is_null() {
        return Err(format!("the pair has a scrub before any was run: {}", pair["scrub"]));
    }

    // vdc: the partition the first boot made, with the Btrfs not mounted
    // yet, and the room after it: the disk is 192 MiB, and the partition
    // 128 MiB from 1 MiB. A person is told where it is and its type; its
    // GPT name, filesystem and label only the disk says, which root opens.
    let opened = test.typed(vm, "elevate storage show --json")?;
    for (report, opens) in [(&alice, false), (&opened, true)] {
        let vdc = disk(report, "vdc")?;
        let partitions = vdc["partitions"].as_array().ok_or("vdc has no partitions")?;
        let [partition] = &partitions[..] else {
            return Err(format!("vdc has {} partitions, not one", partitions.len()));
        };
        let said = |value: &str| if opens { json!(value) } else { Value::Null };
        let wanted = json!({
            "name": "vdc1", "number": 1, "start": 1 << 20, "size": PARTITION, "type": "linux",
            "gpt_name": said("Data"), "filesystem": said("btrfs"), "label": said("GROW"),
            "mounts": []
        });
        let whose = if opens { "root's storage show" } else { "alice's" };
        for (key, value) in wanted.as_object().into_iter().flatten() {
            if &partition[key] != value {
                return Err(format!(
                    "in {whose}, vdc1's {key} is {}, not {value}: {partition}",
                    partition[key]
                ));
            }
        }
    }
    let vdc = disk(&alice, "vdc")?;
    let room = number(&vdc["free_after"], "the room after vdc1")?;
    if !(60 << 20..=64 << 20).contains(&room) {
        return Err(format!("the room after vdc1 is {room} bytes, about 63 MiB were left"));
    }
    Ok(())
}

/// Step 2: the health of `sda`, which keeps a record, of `vda`, which does
/// not, and of what is no disk.
fn health(test: &mut Test) -> Result<(), String> {
    test.read_refused_for_both("storage.health of sda", "/api/1/storage/disks/sda/health")?;
    test.unlocked()?;
    let sda = test.read(Who::Alice, "/api/1/storage/disks/sda/health")?;
    let [one] = &sda.as_array().ok_or("the health of sda is no list")?[..] else {
        return Err(format!("the health of sda is {sda}, not one disk's"));
    };
    let attributes = one["attributes"].as_array().ok_or("sda's health has no attributes")?;
    if one["disk"] != "sda" || one["verdict"] != "passed" || attributes.is_empty() {
        return Err(format!("the health of sda is {one}"));
    }
    if attributes.iter().any(|attribute| !attribute["name"].is_string()) {
        return Err(format!("an attribute of sda has no name: {attributes:?}"));
    }
    // A virtio disk keeps no record of its health: diskhealth leaves it out.
    let vda = test.read(Who::Alice, "/api/1/storage/disks/vda/health")?;
    if vda != json!([]) {
        return Err(format!("the health of the virtio disk vda is {vda}, not none"));
    }
    let nothing = test.get(Who::Alice, "/api/1/storage/disks/nosuch/health")?;
    nothing.refused("the health of a disk that is none", 422, "DSK-101")?;
    let spaced = test.get(Who::Alice, "/api/1/storage/disks/a%20b/health")?;
    let spaced = spaced.refused("the health of a name with a space", 400, "ADM-125")?;
    if spaced.json()?["error"]["field"] != json!("disk") {
        return Err("the refusal of a disk named `a b` does not name the field disk".into());
    }
    Ok(())
}

/// Step 3: a scrub of the pair, as a task that ends done, and what
/// `storage show` keeps of it.
fn a_scrub(vm: &mut Vm, test: &mut Test) -> Result<(), String> {
    let body = json!({"mount": PAIR_MOUNT});
    // The session is unlocked now, so these are refused for who they are,
    // and bob only.
    test.post(Who::Bob, "/api/1/storage/scrub", &body)?.refused(
        "the scrub asked by bob",
        403,
        "ADM-122",
    )?;
    let reply = test.post(Who::Alice, "/api/1/storage/scrub", &body)?;
    let id =
        reply.must("the scrub", 202)?.json()?["task"].as_i64().ok_or("the scrub has no task")?;
    let task =
        test.task_where(id, 400, |task| task["state"] != "running" && task["state"] != "waiting")?;
    let log: Vec<&str> =
        task["log"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
    let found_none = log.iter().any(|line| line.contains("no errors found"));
    if task["state"] != "done" || task["action"] != "storage.scrub" || !found_none {
        return Err(format!("the scrub task is {task}"));
    }
    if let Some(percent) = task["percent"].as_u64()
        && percent > 100
    {
        return Err(format!("the scrub task is {percent}% done"));
    }

    // What `storage show` keeps for the volume, as the pane has it too.
    let shown = test.show_as_typed(vm, Who::Alice)?;
    let kept = &volume(&shown, "LAB")?["scrub"];
    let errors = [&kept["errors"], &kept["fixed"], &kept["unfixable"]];
    if kept["result"] != "ok" || errors != [&json!(0), &json!(0), &json!(0)] {
        return Err(format!("the pair's kept scrub is {kept}"));
    }
    if number(&kept["bytes"], "the bytes scrubbed")? == 0 || number(&kept["at_ms"], "when")? == 0 {
        return Err(format!("the pair's kept scrub is {kept}"));
    }
    // A scrub of what is no mount is ADM-125 with its field.
    let relative = test.post(Who::Alice, "/api/1/storage/scrub", &json!({"mount": "volumes"}))?;
    let relative = relative.refused("a scrub of a path that is no mount point", 400, "ADM-125")?;
    if relative.json()?["error"]["field"] != json!("mount") {
        return Err("the refusal of a relative mount does not name the field mount".into());
    }
    Ok(())
}

/// The use of the disks over time: a list of series, or ADM-123 where
/// metricsd has none yet, and the command says the same.
fn usage(vm: &mut Vm, test: &mut Test) -> Result<(), String> {
    for who in [Who::Bob, Who::Alice] {
        let reply = test.get(who, "/api/1/storage/usage/1h")?;
        if reply.status == 200 {
            let series = reply.json()?;
            let well = series.as_array().is_some_and(|all| {
                all.iter().all(|one| one["series"].is_string() && one["points"].is_array())
            });
            if !well {
                return Err(format!("the use of the disks over an hour is {series}"));
            }
        } else {
            reply.refused("the use of the disks over an hour", 422, "ADM-123")?;
            let status = test
                .pane
                .status(vm, "metrics show disk.read disk.write --since -1h --every 1m --json")?;
            if status == 0 {
                return Err("the route had no history, and the command ended with status 0".into());
            }
        }
    }
    let bad = test.get(Who::Alice, "/api/1/storage/usage/90d")?;
    let bad = bad.refused("a range that is none", 400, "ADM-125")?;
    if bad.json()?["error"]["field"] != json!("range") {
        return Err("the refusal of the range 90d does not name the field range".into());
    }
    Ok(())
}

/// What alice reads of the log's settings file, as root: it is not for
/// everyone.
fn settings(test: &Test, vm: &mut Vm) -> Result<String, String> {
    test.pane.output(vm, "elevate cat /system/settings/log")
}

/// Step 5: the log's room and limits, a change of one limit, none the
/// second time, its undo, and a trim.
fn the_log(vm: &mut Vm, test: &mut Test) -> Result<(), String> {
    test.read_refused_for_both("storage.log", "/api/1/storage/log")?;
    test.unlocked()?;
    let route = test.read(Who::Alice, "/api/1/storage/log")?;
    let typed = test.pane.output(vm, "elevate logtrim --show --json")?;
    let typed: Value = serde_json::from_str(typed.trim())
        .map_err(|err| format!("logtrim --show --json printed {typed:?}: {err}"))?;
    // The log grows between the two, and its room with it.
    for key in ["limits", "cap", "filesystem_size", "wrong"] {
        if route[key] != typed[key] {
            return Err(format!(
                "the route's {key} is {}, the command's {}",
                route[key], typed[key]
            ));
        }
    }
    if number(&route["usage"]["bytes"], "the log's bytes")? == 0 {
        return Err(format!("the log takes no room: {}", route["usage"]));
    }
    if route["limits"]["keep_days"] != json!(90) {
        return Err(format!("the log keeps {} days before a change", route["limits"]["keep_days"]));
    }

    // The limits: refused for bob and for alice locked, and for what
    // cannot work, before any is changed.
    let path = "/api/1/storage/log/limits";
    let sixty = json!({"keep_days": 60});
    let before = settings(test, vm)?;
    test.refused_for_both("storage.log-set", path, &sixty)?;
    test.unlocked()?;
    for (body, field) in [
        (json!({}), "keep_days"),
        (json!({"keep_days": 3651}), "keep_days"),
        (json!({"max_percent": 101}), "max_percent"),
        (json!({"max_size": "lots"}), "max_size"),
    ] {
        let refused = test.post(Who::Alice, path, &body)?;
        let refused = refused.refused(&format!("the limits {body}"), 400, "ADM-125")?;
        if refused.json()?["error"]["field"] != json!(field) {
            return Err(format!("the refusal of {body} does not name the field {field}"));
        }
    }
    if settings(test, vm)? != before {
        return Err("a limit that was refused changed the settings file".into());
    }

    let set = test.post(Who::Alice, path, &sixty)?.must("the change to 60 days", 200)?.json()?;
    if set["done"] != json!(true) || set["changed"] != json!(true) {
        return Err(format!("the change to 60 days was answered {set}"));
    }
    let after = settings(test, vm)?;
    if line_of(&after, "keep-days") != Some("keep-days = 60") {
        return Err(format!(
            "the settings file has {:?} for keep-days:\n{after}",
            line_of(&after, "keep-days")
        ));
    }
    // No other line is changed, the comments among them.
    if without(&before, "keep-days") != without(&after, "keep-days") {
        return Err(format!("the settings file was not changed in that one line alone:\n{after}"));
    }
    let shown = test.read(Who::Alice, "/api/1/storage/log")?;
    if shown["limits"]["keep_days"] != json!(60) {
        return Err(format!(
            "the log keeps {} days after the change",
            shown["limits"]["keep_days"]
        ));
    }
    let again = test.post(Who::Alice, path, &sixty)?.must("the same change again", 200)?.json()?;
    if again["done"] != json!(true) || again["changed"] != json!(false) {
        return Err(format!("the same change again was answered {again}"));
    }

    // The undo the list of actions names puts the file back as it was.
    let list = test.read(Who::Alice, "/api/1/actions")?;
    let row = list
        .as_array()
        .and_then(|rows| rows.iter().find(|row| row["id"] == "storage.log-set"))
        .ok_or("the action list has no storage.log-set")?;
    let undo = &row["examples"]["undo"];
    if undo["action"] != "storage.log-set" {
        return Err(format!("the undo of storage.log-set is {undo}"));
    }
    let undone = test.post(Who::Alice, path, &undo["args"])?.must("the undo", 200)?.json()?;
    if undone["done"] != json!(true) || undone["changed"] != json!(true) {
        return Err(format!("the undo was answered {undone}"));
    }
    if settings(test, vm)? != before {
        return Err(format!(
            "the undo did not put the settings file back:\n{}",
            settings(test, vm)?
        ));
    }

    // A trim now, to the limits.
    test.refused_for_both("storage.log-trim", "/api/1/storage/log/trim", &json!({}))?;
    test.unlocked()?;
    let trimmed = test.post(Who::Alice, "/api/1/storage/log/trim", &json!({}))?;
    let trimmed = trimmed.must("the trim", 200)?.json()?;
    if trimmed["done"] != json!(true) {
        return Err(format!("the trim was answered {trimmed}"));
    }
    Ok(())
}

/// The line of `text` that sets `key`, the last one, without its
/// surrounding space.
fn line_of<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    text.lines().map(str::trim).rfind(|line| line.split([' ', '=']).next() == Some(key))
}

/// The lines of `text` that do not set `key`.
fn without(text: &str, key: &str) -> Vec<String> {
    text.lines()
        .filter(|line| line.trim().split([' ', '=']).next() != Some(key))
        .map(str::to_string)
        .collect()
}

/// The second boot: Spark has mounted the Btrfs of `vdc1`, and `vdc` is
/// 64 MiB larger than the first boot left it.
fn second_boot(vm: &mut Vm, case: &Case) -> Result<(), String> {
    owner_signs_in(vm, case)?;
    let mut test = people(vm)?;

    let before = room_after_the_last_partition(vm, &mut test)?;
    grow_is_refused(&mut test)?;
    grown_once(vm, &mut test, &before)?;
    grown_twice(vm, &mut test)?;

    vm.press_power_button()?;
    vm.expect("spark: every service has stopped")?;
    vm.expect("Power: `spark` asked to power off the machine; syncing the filesystems")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}

/// What `storage show` says of `vdc` and of the Btrfs on it before it is
/// grown.
struct Before {
    /// The partition's size.
    partition: u64,
    /// The filesystem's.
    filesystem: u64,
}

/// Step 4's beginning: `storage show` gives the room after the last
/// partition of `vdc`, now that it is larger, and the Btrfs of `vdc1`
/// mounted; the pair is mounted again.
fn room_after_the_last_partition(vm: &mut Vm, test: &mut Test) -> Result<Before, String> {
    let shown = test.show_as_typed(vm, Who::Alice)?;
    let pair = volume(&shown, "LAB")?;
    if pair["mount"] != json!(PAIR_MOUNT) {
        return Err(format!("the pair is at {}", pair["mount"]));
    }
    if pair["profiles"]["data"] != json!(["raid1"]) {
        return Err(format!("the pair's profiles are {}", pair["profiles"]));
    }
    let vdc = disk(&shown, "vdc")?;
    if vdc["size"] != json!(GROWN) {
        return Err(format!("vdc is {} bytes, not the {GROWN} the host made it", vdc["size"]));
    }
    // 256 MiB less the 129 MiB the partition ends at, and the GPT's end.
    let room = number(&vdc["free_after"], "the room after vdc1")?;
    let wanted = GROWN - (1 << 20) - PARTITION - 33 * 512;
    if room != wanted {
        return Err(format!("the room after vdc1 is {room} bytes, not {wanted}"));
    }
    let partition = &vdc["partitions"][0];
    let grown = volume(&shown, "GROW")?;
    if partition["mounts"] != json!([GROWN_MOUNT]) || grown["mount"] != json!(GROWN_MOUNT) {
        return Err(format!(
            "vdc1 is mounted at {}, the volume at {}",
            partition["mounts"], grown["mount"]
        ));
    }
    if grown["devices"] != json!(["vdc1"]) {
        return Err(format!("the volume GROW is made of {}", grown["devices"]));
    }
    Ok(Before {
        partition: number(&partition["size"], "the size of vdc1")?,
        filesystem: number(&grown["size"], "the size of the filesystem")?,
    })
}

/// A grow that cannot be done: asked by one who may not, for what is no
/// disk, for a partition and for a disk with none.
fn grow_is_refused(test: &mut Test) -> Result<(), String> {
    let path = |disk: &str| format!("/api/1/storage/disks/{disk}/grow");
    test.refused_for_both("storage.grow of vdc", &path("vdc"), &json!({}))?;
    test.unlocked()?;
    let nosuch = test.post(Who::Alice, &path("nosuch"), &json!({}))?;
    nosuch.refused("the grow of a disk that is none", 422, "DSK-101")?;
    let partition = test.post(Who::Alice, &path("vdc1"), &json!({}))?;
    partition.refused("the grow of a partition", 422, "DSK-101")?;
    // sda has no partition to grow.
    let none = test.post(Who::Alice, &path("sda"), &json!({}))?;
    let none = none.refused("the grow of a disk with no partition", 422, "ADM-123")?;
    if !none.detail().contains("no partition") {
        return Err(format!(
            "the grow of sda says {}, not that it has no partition",
            none.detail()
        ));
    }
    let spaced = test.post(Who::Alice, &path("a%20b"), &json!({}))?;
    let spaced = spaced.refused("the grow of a name with a space", 400, "ADM-125")?;
    if spaced.json()?["error"]["field"] != json!("disk") {
        return Err("the refusal of a disk named `a b` does not name the field disk".into());
    }
    Ok(())
}

/// The grow of `vdc`: the partition and the filesystem larger, and a file
/// that is larger than they were fits.
fn grown_once(vm: &mut Vm, test: &mut Test, before: &Before) -> Result<(), String> {
    let reply = test.post(Who::Alice, "/api/1/storage/disks/vdc/grow", &json!({}))?;
    let grown = reply.must("the grow of vdc", 200)?.json()?;
    if grown["done"] != json!(true) || grown["changed"] != json!(true) {
        return Err(format!("the grow of vdc was answered {grown}"));
    }
    let shown = test.show_as_typed(vm, Who::Alice)?;
    let vdc = disk(&shown, "vdc")?;
    let partition = number(&vdc["partitions"][0]["size"], "the size of vdc1 now")?;
    let filesystem = number(&volume(&shown, "GROW")?["size"], "the size of the filesystem now")?;
    // 256 MiB less the first MiB and the GPT's end: about 255 MiB.
    if partition < 250 << 20 || partition <= before.partition {
        return Err(format!("vdc1 is {partition} bytes, and was {}", before.partition));
    }
    if filesystem < 240 << 20 || filesystem <= before.filesystem {
        return Err(format!("the filesystem is {filesystem} bytes, and was {}", before.filesystem));
    }
    if number(&vdc["free_after"], "the room after vdc1 now")? >= 1 << 20 {
        return Err(format!("vdc has room after vdc1 still: {}", vdc["free_after"]));
    }
    // The sizes are the machine's too.
    let sizes = test.lsblk(vm, &["vdc1"])?;
    if sizes != [("vdc1".to_string(), partition)] {
        return Err(format!("lsblk -b gives {sizes:?}, and storage show {partition} bytes"));
    }
    let [size, ..] = test.df(vm, GROWN_MOUNT)?;
    if size != filesystem {
        return Err(format!("df -B1 gives {size} bytes, and storage show {filesystem}"));
    }
    // A file larger than the partition was fits.
    let status = test.pane.status(
        vm,
        &format!(
            "elevate sh -c 'head -c 150000000 /devices/zero > {GROWN_MOUNT}/big && sync && \
             test -s {GROWN_MOUNT}/big'"
        ),
    )?;
    if status != 0 {
        return Err(format!(
            "a file of 150 MB was not written to the grown Btrfs: status {status}"
        ));
    }
    Ok(())
}

/// A second grow has nothing to do: it is done and changes nothing.
fn grown_twice(vm: &mut Vm, test: &mut Test) -> Result<(), String> {
    let reply = test.post(Who::Alice, "/api/1/storage/disks/vdc/grow", &json!({}))?;
    let again = reply.must("the second grow of vdc", 200)?.json()?;
    if again["done"] != json!(true) || again["changed"] != json!(false) {
        return Err(format!("the second grow of vdc was answered {again}"));
    }
    // And the command says the same at the terminal.
    let said = test.pane.output(vm, "elevate storage grow vdc")?;
    if !said.contains("no room after its last partition") {
        return Err(format!("storage grow vdc a second time said {said:?}"));
    }
    Ok(())
}

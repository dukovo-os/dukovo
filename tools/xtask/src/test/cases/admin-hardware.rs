// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The Hardware of the web console, Dukovo Admin: what the machine is, the
//! memory modules the firmware lists, the last check of the hardware and
//! the checks that came before it, and a check as a task. alice, the
//! owner, is an administrator; bob may use the console and is no
//! administrator.
//!
//! What the machine is. `GET /api/1/hardware` is what the command `hardware
//! show --json` prints in alice's pane, for bob and for alice: the same
//! document, whose memory has no `ecc` and no `modules` and whose
//! processor has no `sockets` since a person is not told what only the
//! firmware's tables say, and which has no serial number or ID of the
//! machine anywhere. `GET /api/1/hardware/inventory` is ADM-122 for bob
//! and ADM-106 for alice while locked; unlocked, it is the same document
//! with the memory's slots and modules and the processor's one socket
//! filled in, from the tables of QEMU's SMBIOS, whose modules add up to at
//! least the memory the kernel counts.
//!
//! Before any check. `GET /api/1/hardware/check` is `null`, and so is what
//! the command `hardware last --json` prints, which ends with status 3;
//! the history, `GET /api/1/hardware/history` and the command `hardware
//! history --json`, is the empty list.
//!
//! A check. `POST /api/1/hardware/check` for the sensors and the CPUs is
//! ADM-122 for bob and ADM-106 for alice while locked, and for what cannot
//! work (an area that is none, one named twice, a disk with an area that is
//! not the disks) ADM-125 with its field, and none of these starts a
//! check. Asked for, it is a task that ends, each check counted as a step
//! of it up to the last, as many as the checks it keeps. Then bob reads the
//! result the check kept, which is the file of a check made by root that
//! everyone may read, the same as the command `hardware last --json` prints
//! in alice's pane, and the history has the one line of the check, as the
//! command `hardware history --json` prints it.
//!
//! The sensors, the highest temperatures and the devices. Each is a read
//! that bob and alice both make, the same document for both, and what the
//! command prints in alice's pane: `hardware sensors --json`, `metrics show
//! temp --since 30d --highest --json` and `hardware devices --json`, each
//! ending with status 0. QEMU gives the machine no hardware monitor and no
//! thermal zone, so the sensors and the zones are empty lists, and the
//! highest temperatures are a list of series with their greatest value.
//! The devices are the PCI functions of QEMU's q35 board and its USB
//! controller: the network card is virtio's, driven by radix-virtio and
//! working as net0, the board's SATA controller is the AHCI driver's, the
//! host bridge is hidden, and no member of the document is a serial number.
//! A read given a name it does not take is ADM-108.
//!
//! What the pane's commands print reaches the host through nc, to a port on
//! the host's loopback that QEMU's user network gives the guest at
//! 10.0.2.2.

use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::super::console_client::{ALICE_PASSWORD, Browser, Console, Pane, Reply, elevated};
use super::super::*;

/// The password of bob, whom alice makes: he may use the console, and is
/// no administrator.
const BOB_PASSWORD: &str = "looking-glass";

/// The areas the check of this test looks at: the ones that need no disk,
/// and take about a minute, which the processors under load do.
const AREAS: &str = "sensors cpu";

/// The most seconds the check may take: its minute of load, the other
/// checks and the time the machine needs to start it.
const CHECK_SECONDS: u64 = 420;

/// The test.
pub const CASE: Case = Case {
    name: "admin-hardware",
    about: "The web console's Hardware: GET /api/1/hardware is what hardware show --json prints in alice's pane, for bob and for alice, with no memory modules, no sockets and no serial number; hardware.inventory is ADM-122 for bob and ADM-106 for alice locked, and unlocked the same document with the memory's slots and modules and the one socket from the tables of QEMU's SMBIOS; before any check hardware.last is null and hardware last --json prints null and ends with status 3, and hardware.history is the empty list; hardware.check is ADM-122 for bob and ADM-106 for alice locked, an area that is none, an area named twice and a disk with another area are ADM-125 with their fields, none of them starts a check, and for the sensors and the processors it is a task that counts each check as a step, to the last; bob then reads the result it kept, as hardware last --json prints it in alice's pane, and the history has its one line, as hardware history --json prints it; hardware.sensors, hardware.highest, hardware.devices and hardware.errors are the same document for bob and for alice, as the commands print it in alice's pane with status 0: QEMU has no sensor, and its virtio network card is driven by radix-virtio and working; hardware.bmc-sensors, hardware.bmc-events and hardware.identify are ADM-122 for bob and ADM-106 for alice locked",
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
    script: admin_hardware,
    disks: &[],
    second: None,
};

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

    /// What `command`, which prints JSON, prints in the pane.
    fn typed(&self, vm: &mut Vm, command: &str) -> Result<Value, String> {
        let text = self.pane.output(vm, command)?;
        serde_json::from_str(text.trim())
            .map_err(|err| format!("{command} printed {text:?}: {err}"))
    }
}

/// A number of `value`, or the failure that says `what` is none.
fn number(value: &Value, what: &str) -> Result<u64, String> {
    value.as_u64().ok_or_else(|| format!("{what} is {value}, not a number"))
}

/// Whether `value` has a member called, or containing, `needle` anywhere
/// in it, in any case.
fn has_member(value: &Value, needle: &str) -> bool {
    match value {
        Value::Object(members) => members.iter().any(|(name, inside)| {
            name.to_lowercase().contains(needle) || has_member(inside, needle)
        }),
        Value::Array(items) => items.iter().any(|item| has_member(item, needle)),
        _ => false,
    }
}

/// `report` without what only the firmware's tables say of the memory and
/// the processor, which a person who may not read them is not told.
fn without_the_tables(report: &Value) -> Value {
    let mut report = report.clone();
    for member in ["ecc", "slots", "modules"] {
        report["memory"][member] = Value::Null;
    }
    report["processor"]["sockets"] = Value::Null;
    report
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

/// The test: the machine as shown, what is kept before any check, and a
/// check.
fn admin_hardware(vm: &mut Vm, case: &Case) -> Result<(), String> {
    owner_signs_in(vm, case)?;
    let mut test = people(vm)?;

    what_the_machine_is(vm, &mut test)?;
    nothing_kept_yet(vm, &mut test)?;
    a_check(vm, &mut test)?;
    sensors_and_devices(vm, &mut test)?;
    //
    // Step 5 goes after it, and ends the test in place of the power button
    // below: power.restart while the console trial runs, and the restart
    // of the machine that follows.

    vm.press_power_button()?;
    vm.expect("spark: every service has stopped")?;
    vm.expect("Power: `spark` asked to power off the machine; syncing the filesystems")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}

/// Step 1: `hardware.show` for bob and alice, which is what the pane
/// prints, and `hardware.inventory`, which only an administrator reads.
fn what_the_machine_is(vm: &mut Vm, test: &mut Test) -> Result<(), String> {
    let bob = test.read(Who::Bob, "/api/1/hardware")?;
    let alice = test.read(Who::Alice, "/api/1/hardware")?;
    if bob != alice {
        return Err(format!("bob is shown {bob}, and alice {alice}"));
    }
    let typed = test.typed(vm, "hardware show --json")?;
    if typed != bob {
        return Err(format!("the pane has {typed}, and the route {bob}"));
    }
    if bob["format"] != json!(1) {
        return Err(format!("hardware show has the format {}, not 1", bob["format"]));
    }
    for member in ["maker", "model", "processor", "memory", "firmware"] {
        if bob[member].is_null() {
            return Err(format!("hardware show has no {member}: {bob}"));
        }
    }
    // The processors are the two the machine was given; the memory the
    // kernel counts is no more than the 1 GiB it was given.
    if bob["processor"]["threads"] != json!(2) {
        return Err(format!("the machine has two CPUs, and hardware show {}", bob["processor"]));
    }
    let bytes = number(&bob["memory"]["bytes"], "the memory")?;
    if bytes == 0 || bytes > 1 << 30 {
        return Err(format!("the memory is {bytes} bytes, of the 1 GiB the machine was given"));
    }
    // What a person is not told, and what no one is.
    if !bob["memory"]["ecc"].is_null() || !bob["memory"]["modules"].is_null() {
        return Err(format!("a person is told what only the tables say: {}", bob["memory"]));
    }
    if !bob["processor"]["sockets"].is_null() {
        return Err(format!("a person is told the sockets the tables list: {}", bob["processor"]));
    }
    for needle in ["serial", "uuid"] {
        if has_member(&bob, needle) {
            return Err(format!("hardware show has a member with {needle} in its name: {bob}"));
        }
    }

    // The same, as an administrator, with the modules.
    test.read_refused_for_both("hardware.inventory", "/api/1/hardware/inventory")?;
    test.unlocked()?;
    let inventory = test.read(Who::Alice, "/api/1/hardware/inventory")?;
    if without_the_tables(&inventory) != without_the_tables(&bob) {
        return Err(format!("the inventory is {inventory}, and the show {bob}"));
    }
    // QEMU's tables list a socket for each the machine was given: one.
    if inventory["processor"]["sockets"] != json!(1) {
        return Err(format!("the inventory's processor is {}", inventory["processor"]));
    }
    let Some(modules) = inventory["memory"]["modules"].as_array() else {
        return Err(format!(
            "the inventory has no modules, so the command did not read the tables: {}",
            inventory["memory"]
        ));
    };
    if modules.is_empty() {
        return Err(format!("the tables of QEMU's SMBIOS list no module: {}", inventory["memory"]));
    }
    let mut installed = 0;
    for module in modules {
        let slot = module["slot"].as_str().unwrap_or_default();
        if slot.is_empty() || !module["kind"].is_string() {
            return Err(format!("a module has no slot or no kind: {module}"));
        }
        installed += number(&module["bytes"], &format!("the size of the module {slot}"))?;
    }
    if installed < bytes {
        return Err(format!("the modules are {installed} bytes, and the kernel counts {bytes}"));
    }
    let slots = number(&inventory["memory"]["slots"], "the slots")?;
    if slots < modules.len() as u64 {
        return Err(format!("{slots} slots hold {} modules", modules.len()));
    }
    Ok(())
}

/// Step 2: before any check, the last is `null` and the history is empty,
/// as the routes say them and as the commands do.
fn nothing_kept_yet(vm: &mut Vm, test: &mut Test) -> Result<(), String> {
    for who in [Who::Bob, Who::Alice] {
        let last = test.get(who, "/api/1/hardware/check")?.must("the last check", 200)?;
        if last.json()? != Value::Null {
            return Err(format!("the last check, before any, is {}", last.body));
        }
        let history = test.read(who, "/api/1/hardware/history")?;
        if history != json!([]) {
            return Err(format!("the history, before any check, is {history}"));
        }
    }
    let said = test.pane.output(vm, "hardware last --json")?;
    if said.trim() != "null" {
        return Err(format!("hardware last --json printed {said:?}, not null"));
    }
    let status = test.pane.status(vm, "hardware last --json")?;
    if status != 3 {
        return Err(format!("hardware last --json ended with status {status}, not 3"));
    }
    let history = test.typed(vm, "hardware history --json")?;
    if history != json!([]) {
        return Err(format!("hardware history --json printed {history}"));
    }
    Ok(())
}

/// Step 3: a check of the sensors and the processors, as a task, and what
/// it kept.
fn a_check(vm: &mut Vm, test: &mut Test) -> Result<(), String> {
    let path = "/api/1/hardware/check";
    let body = json!({"areas": AREAS});
    test.refused_for_both("hardware.check", path, &body)?;
    test.unlocked()?;
    for (bad, field) in [
        (json!({"areas": "gpu"}), "areas"),
        (json!({"areas": "memory memory"}), "areas"),
        (json!({"areas": "memory", "disk": "vda"}), "areas"),
        (json!({"disk": "vda"}), "areas"),
        (json!({"areas": "disks", "disk": "../x"}), "disk"),
    ] {
        let refused = test.post(Who::Alice, path, &bad)?;
        let refused = refused.refused(&format!("the check {bad}"), 400, "ADM-125")?;
        if refused.json()?["error"]["field"] != json!(field) {
            return Err(format!("the refusal of {bad} does not name the field {field}"));
        }
    }

    let reply = test.post(Who::Alice, path, &body)?;
    let id =
        reply.must("the check", 202)?.json()?["task"].as_i64().ok_or("the check has no task")?;
    let task = test.task_where(id, CHECK_SECONDS, |task| {
        task["state"] != "running" && task["state"] != "waiting"
    })?;
    let words = ["hardware", "check", "--json", "--progress", "sensors", "cpu"];
    if task["action"] != "hardware.check" || task["argv"] != json!(words) {
        return Err(format!("the check task is {task}"));
    }
    // A check that finds nothing ends with 0 and is done. One that finds a
    // warning or a failure ends with 2 or 1, which the task is failed for.
    let status = task["status"].as_i64().unwrap_or(-1);
    match (task["state"].as_str(), status) {
        (Some("done"), 0) | (Some("failed"), 1 | 2) => {}
        _ => return Err(format!("the check task ended as {task}")),
    }

    // Each check was a step, counted to the last, which names one of the
    // two areas: the steps are the task's progress, not lines of its log.
    let (step, steps) = (task["step"].as_u64(), task["steps"].as_u64());
    let area = task["words"].as_str().unwrap_or_default().split(' ').next().unwrap_or_default();
    if step.is_none_or(|step| step == 0)
        || step != steps
        || !AREAS.split(' ').any(|wanted| wanted == area)
    {
        return Err(format!("the check task did not count its checks as its steps: {task}"));
    }

    // What it kept, for bob and for alice, and what the pane prints.
    let last = test.read(Who::Bob, path)?;
    if test.read(Who::Alice, path)? != last {
        return Err("alice and bob are shown different checks".into());
    }
    let typed = test.typed(vm, "hardware last --json")?;
    if typed != last {
        return Err(format!("the pane has {typed}, and the route {last}"));
    }
    let ended = test.pane.status(vm, "hardware last --json")?;
    if last["status"] != json!(status) || i64::from(ended) != status {
        let kept = &last["status"];
        return Err(format!("the check ended with {status}, kept {kept} and said {ended}"));
    }
    if last["full"] != json!(false) || number(&last["time"], "the time of the check")? == 0 {
        return Err(format!("the kept check is {last}"));
    }
    let checks = last["checks"].as_array().ok_or("the kept check has no checks")?;
    if Some(checks.len() as u64) != steps {
        let kept = checks.len();
        return Err(format!("the kept check has {kept} checks, and the task {steps:?} steps"));
    }
    let mut counts = [0u64; 4];
    for check in checks {
        let area = check["area"].as_str().unwrap_or_default();
        let at = ["passed", "warning", "failed", "skipped"]
            .iter()
            .position(|result| check["result"] == *result)
            .ok_or_else(|| format!("a check has the result {}: {check}", check["result"]))?;
        counts[at] += 1;
        if !AREAS.split(' ').any(|wanted| wanted == area) || !check["check"].is_string() {
            return Err(format!("the kept check has a check that was not asked for: {check}"));
        }
    }
    for area in AREAS.split(' ') {
        if !checks.iter().any(|check| check["area"] == area) {
            return Err(format!("the kept check has nothing of the area {area}: {last}"));
        }
    }

    // The history has the one line of it, which says the same.
    let history = test.read(Who::Bob, "/api/1/hardware/history")?;
    let typed = test.typed(vm, "hardware history --json")?;
    if typed != history {
        return Err(format!("the pane has {typed}, and the route {history}"));
    }
    let [line] = &history.as_array().ok_or("the history is no list")?[..] else {
        return Err(format!("the history is {history}, not the one line of the check"));
    };
    let counted = json!({
        "passed": counts[0], "warnings": counts[1], "failed": counts[2], "skipped": counts[3]
    });
    let said = json!({
        "passed": line["passed"], "warnings": line["warnings"], "failed": line["failed"],
        "skipped": line["skipped"]
    });
    if said != counted || line["status"] != json!(status) || line["full"] != json!(false) {
        return Err(format!("the history's line is {line}, and the check kept {last}"));
    }
    if line["at_ms"] != json!(number(&last["time"], "the time")? * 1000) || !line["ids"].is_array()
    {
        return Err(format!("the history's line is {line}, and the check kept {last}"));
    }
    Ok(())
}

/// What `path` says to bob and to alice, which is one document, and what
/// `command` prints in alice's pane with status 0, which is that document
/// too once `settled` has taken out of each what the machine moves by itself.
fn read_as_printed(
    vm: &mut Vm,
    test: &mut Test,
    path: &str,
    command: &str,
    settled: fn(&Value) -> Value,
) -> Result<Value, String> {
    let bob = test.read(Who::Bob, path)?;
    let alice = test.read(Who::Alice, path)?;
    if bob != alice {
        return Err(format!("{path} is {bob} for bob, and {alice} for alice"));
    }
    let typed = test.typed(vm, command)?;
    if settled(&typed) != settled(&bob) {
        return Err(format!("`{command}` printed {typed}, and {path} said {bob}"));
    }
    let status = test.pane.status(vm, command)?;
    if status != 0 {
        return Err(format!("`{command}` ended with status {status}, not 0"));
    }
    Ok(bob)
}

/// A document as it is.
fn as_it_is(value: &Value) -> Value {
    value.clone()
}

/// The sensors' document without the times its disks were last read at,
/// which diskhealth moves on its own.
fn without_the_times(value: &Value) -> Value {
    let mut value = value.clone();
    if let Some(disks) = value["disks"].as_array_mut() {
        for disk in disks {
            disk["checked_ms"] = Value::Null;
        }
    }
    value
}

/// Whether the vendor and device numbers of the device `row` start with
/// `vendor`, as `8086:` does.
fn from_vendor(row: &Value, vendor: &str) -> bool {
    row["ids"].as_str().is_some_and(|ids| ids.starts_with(vendor))
}

/// Step 4: the sensors, the highest temperatures and the devices, which a
/// person reads, as the routes say them and as the commands print them.
fn sensors_and_devices(vm: &mut Vm, test: &mut Test) -> Result<(), String> {
    // QEMU has no hardware monitor and no thermal zone: nothing is listed,
    // and that is no failure.
    let path = "/api/1/hardware/sensors";
    let sensors = read_as_printed(vm, test, path, "hardware sensors --json", without_the_times)?;
    if sensors["format"] != json!(1) || !sensors["disks"].is_array() {
        return Err(format!("hardware sensors has the document {sensors}"));
    }
    for list in ["sensors", "zones"] {
        if sensors[list] != json!([]) {
            return Err(format!(
                "QEMU has no {list}, and hardware sensors lists {}",
                sensors[list]
            ));
        }
    }

    // The greatest value of each temperature series in 30 days: a list,
    // each with the series' name and the time and value of its highest, or
    // null for a series that has no point.
    let path = "/api/1/hardware/highest";
    let command = "metrics show temp --since 30d --highest --json";
    let highest = read_as_printed(vm, test, path, command, as_it_is)?;
    let Some(series) = highest.as_array() else {
        return Err(format!("the highest temperatures are {highest}, not a list"));
    };
    for one in series {
        let name = one["series"].as_str().unwrap_or_default();
        let point = &one["highest"];
        let shaped = point.is_null() || point.as_array().is_some_and(|point| point.len() == 2);
        if !name.starts_with("temp{") || !shaped {
            return Err(format!("a series of the highest temperatures is {one}"));
        }
    }

    // The hardware errors the kernel has heard of: a list and the counts,
    // empty or not, the same for both and as the command prints it.
    let path = "/api/1/hardware/errors";
    let errors = read_as_printed(vm, test, path, "hardware errors --json", as_it_is)?;
    if errors["format"] != json!(1) || !errors["errors"].is_array() || !errors["counts"].is_object()
    {
        return Err(format!("hardware errors has the document {errors}"));
    }

    // What a read is not given is refused, not ignored.
    for path in [
        "/api/1/hardware/errors?all=true",
        "/api/1/hardware/sensors?all=true",
        "/api/1/hardware/highest?since=1d",
        "/api/1/hardware/devices?all=true",
    ] {
        test.get(Who::Bob, path)?.refused(&format!("{path} read by bob"), 400, "ADM-108")?;
    }

    // The devices: QEMU's own functions, with what is told of each.
    let path = "/api/1/hardware/devices";
    let devices = read_as_printed(vm, test, path, "hardware devices --json", as_it_is)?;
    if devices["format"] != json!(1) {
        return Err(format!("hardware devices has the format {}, not 1", devices["format"]));
    }
    let Some(rows) = devices["devices"].as_array() else {
        return Err(format!("hardware devices has no list of devices: {devices}"));
    };
    if has_member(&devices, "serial") {
        return Err(format!("hardware devices has a member with serial in its name: {devices}"));
    }
    for row in rows {
        let ids = row["ids"].as_str().unwrap_or_default();
        let place = row["where"].as_str().unwrap_or_default();
        let group = row["group"].as_str().unwrap_or_default();
        let numbers = ids.split_once(':').is_some_and(|(vendor, device)| {
            [vendor, device]
                .iter()
                .all(|part| part.len() == 4 && part.bytes().all(|byte| byte.is_ascii_hexdigit()))
        });
        let named = row["name"].as_str().is_some_and(|name| !name.is_empty());
        let shaped = row["system"].is_array() && row["hidden"].is_boolean();
        if !numbers
            || !named
            || !shaped
            || !(place.starts_with("PCI ") || place.starts_with("USB "))
            || !["network", "storage", "usb", "display", "other"].contains(&group)
        {
            return Err(format!("a device is shown as {row}"));
        }
    }
    let find = |wanted: &dyn Fn(&Value) -> bool| rows.iter().find(|row| wanted(row));
    // The network card is virtio's, and the kernel says which driver took
    // it and that it works, as the interface netd started.
    let Some(card) = find(&|row| row["group"] == "network" && from_vendor(row, "1af4:")) else {
        return Err(format!("no virtio network card among the devices: {devices}"));
    };
    if card["driver"] != json!("radix-virtio")
        || card["driver_told"] != json!(true)
        || card["state"] != json!("working")
        || card["system"] != json!(["net0"])
        || card["hidden"] != json!(false)
    {
        return Err(format!("the virtio network card is shown as {card}"));
    }
    // QEMU's own USB controller, which the boot disk is on.
    if find(&|row| row["group"] == "usb" && from_vendor(row, "1b36:")).is_none() {
        return Err(format!("no USB controller of QEMU's among the devices: {devices}"));
    }
    // The board's SATA controller is the AHCI driver's, whole class, and its
    // bridges work and need nothing, so they are hidden.
    let ahci = |row: &Value| {
        row["group"] == "storage" && row["driver"] == "ahci" && row["driver_told"] == true
    };
    if find(&ahci).is_none() {
        return Err(format!("no SATA controller driven by ahci among the devices: {devices}"));
    }
    if find(&|row| row["hidden"] == true && from_vendor(row, "8086:")).is_none() {
        return Err(format!("the board's host bridge is not among the hidden devices: {devices}"));
    }

    // The BMC's sensors and event log, and the identify light, are an
    // administrator's: QEMU has no BMC here, so only who may ask is held.
    for (what, path) in [
        ("hardware.bmc-sensors", "/api/1/hardware/bmc/sensors"),
        ("hardware.bmc-events", "/api/1/hardware/bmc/events"),
    ] {
        test.read_refused_for_both(what, path)?;
    }
    test.refused_for_both("hardware.identify", "/api/1/hardware/bmc/identify", &json!({}))?;
    Ok(())
}

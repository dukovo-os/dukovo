// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The memory test at the next start, asked for from the web console,
//! Dukovo Admin: on an installed system's disk under UEFI, the kernel
//! tests the free memory once before it starts the first program, takes
//! any bad part out of use, and the system then starts as usual. alice,
//! the owner, is an administrator.
//!
//! Asking. Nothing is asked at first: `GET /api/1/hardware/memtest` is
//! the document the command `hardware memtest --status --json` prints in
//! alice's pane, with no `asked` and no `last`. `POST
//! /api/1/hardware/memtest/next-start` asks, and says the next start
//! tests the free memory first; the status then has who asked and when,
//! and the file that keeps it is there, for anyone to read. Asked again,
//! it is done again. While it is asked, `changes bisect` is refused with
//! HW-105. `POST /api/1/hardware/memtest/cancel` takes the ask back: the
//! file is gone and the status says nothing is asked, and cancelled again
//! it changes nothing. It is asked once more, and the machine is
//! restarted from the pane, as the words of the command say.
//!
//! The start. The start menu takes the try that holds the test, and the
//! kernel says in the serial log that it tests the free memory, how far it
//! has come at each tenth, and that it passed. The system then starts and
//! alice signs in again: the command line the kernel was given has the try
//! and the test, `changes --list` names the start a memory test at the
//! start, and once the health check has run the status has the result:
//! passed, complete, at least 900 MiB tested, with nothing asked any more.
//!
//! A second start, afresh from the image, is given `memtest=fault` on its
//! command line by hand: the kernel plants one error in the first block it
//! tests, and the result is an error in one range, which the status keeps,
//! the kernel counts as one page of memory gone bad, and alertd has as a
//! critical alert of the kind hardware.
//!
//! What the pane's commands print reaches the host through nc, to a port
//! on the host's loopback that QEMU's user network gives the guest at
//! 10.0.2.2.

use serde_json::{Value, json};

use super::super::console_client::{ALICE_PASSWORD, Browser, Console, Pane, Reply, elevated};
use super::super::*;
use super::problem_report::run;

/// The most the first start may take, from the restart to the result the
/// health check keeps, and the second's wait for the same, in seconds.
const RESULT_SECONDS: u64 = 300;

/// The least the test must have tested of the 1 GiB the machine has.
const TESTED_LEAST: u64 = 900 << 20;

/// The file that keeps what is asked, which anyone may read.
const ASKED_FILE: &str = "/system/state/hardware/memtest-asked.json";

/// The command whose output the status route has as its answer.
const STATUS: &str = "hardware memtest --status --json";

/// The routes that ask for the test and take the ask back.
const ASK: &str = "/api/1/hardware/memtest/next-start";
const CANCEL: &str = "/api/1/hardware/memtest/cancel";

/// What `hardware memtest --status --json` prints when nothing was asked
/// and no test has run.
const NOTHING: &str = r#"{"format":1,"asked":null,"last":null,"history":[]}"#;

/// The test.
pub const CASE: Case = Case {
    name: "memory-test-start",
    about: "The memory test at the next start, asked for from the web console: hardware.memtest-status is what hardware memtest --status --json prints, with nothing asked and no last test at first; hardware.memtest asks and says the next start tests the free memory first, the status says who asked, the file that keeps it is readable by anyone, and asked again it is done again; changes bisect is refused with HW-105 while it is asked; hardware.memtest-cancel takes the ask back and the file goes, and cancelled again it changes nothing; asked once more, a restart from the pane makes the start menu take the try, the kernel says in the serial log that it tests the free memory, its progress and that it passed, the system starts, the command line has the try and memtest=once, changes --list names the start a memory test at the start, and the status has the last test as passed and complete with at least 900 MiB tested and nothing asked; a second start with memtest=fault on its command line finds one error in one range, which the status keeps, /processes/meminfo counts as 4 KiB of corrupted memory and alertd has as a critical alert of the kind hardware",
    firmware: Firmware::Uefi,
    media: Media::Disk,
    cmdline: "lockdep",
    cpus: 2,
    memory: "1G",
    reboot: true,
    image: TestImage::Installed,
    cpu_model: None,
    qemu: &["-netdev", "user,id=n0", "-device", "virtio-net-pci,netdev=n0"],
    script: first_start,
    disks: &[],
    second: Some(SecondBoot {
        cmdline: "lockdep memtest=fault",
        before: nothing_between,
        script: second_start,
    }),
};

/// What the host does to the disks between the starts: nothing, as the
/// second start is made afresh from the image.
fn nothing_between(_case: &Case, _disks: &[DiskImage]) -> Result<(), String> {
    Ok(())
}

/// What the test keeps from one step to the next.
struct Test {
    console: Console,
    pane: Pane,
    alice: Browser,
}

impl Test {
    /// Signs alice in at the console, as the page does.
    fn sign_in(&mut self) -> Result<(), String> {
        self.alice = Browser::default();
        self.console.sign_in(&mut self.alice, "alice", ALICE_PASSWORD)?;
        self.console.read_session(&mut self.alice)?;
        Ok(())
    }

    /// What `path` says to alice, which must be 200.
    fn read(&mut self, path: &str) -> Result<Value, String> {
        self.console.get(&mut self.alice, path)?.must(path, 200)?.json()
    }

    /// A change at `path` for alice, whatever the answer.
    fn post(&mut self, path: &str) -> Result<Reply, String> {
        self.console.post(&mut self.alice, path, &json!({}))
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

    /// What the status says, which the route and the command agree on.
    fn status(&mut self, vm: &mut Vm) -> Result<Value, String> {
        let route = self.read("/api/1/hardware/memtest")?;
        let typed = printed(vm, &self.pane)?;
        if typed != route {
            return Err(format!("the pane has {typed}, and the route {route}"));
        }
        Ok(route)
    }

    /// A change of the test at `path`, which must be done: its answer.
    fn changed(&mut self, path: &str) -> Result<Value, String> {
        let answer = self.post(path)?.must(path, 200)?.json()?;
        if answer["done"] != json!(true) {
            return Err(format!("{path} does not say it is done: {answer}"));
        }
        Ok(answer)
    }
}

/// What `hardware memtest --status --json` prints in the pane.
fn printed(vm: &mut Vm, pane: &Pane) -> Result<Value, String> {
    let text = pane.output(vm, STATUS)?;
    serde_json::from_str(text.trim()).map_err(|err| format!("{STATUS} printed {text:?}: {err}"))
}

/// The first start: asked, refused to bisect, cancelled, asked, restarted
/// into the test.
fn first_start(vm: &mut Vm, _case: &Case) -> Result<(), String> {
    owner_made(vm)?;
    vm.expect_anywhere("netd: net0 has 10.0.2.15/24")?;
    vm.expect_anywhere("admind: answers at ")?;
    let mut test =
        Test { console: Console::open(vm)?, pane: Pane::open()?, alice: Browser::default() };
    elevated(vm)?;
    test.sign_in()?;

    nothing_is_asked(vm, &mut test)?;
    the_test_is_asked(vm, &mut test)?;
    a_search_is_refused(vm)?;
    the_ask_is_taken_back(vm, &mut test)?;
    restarted_into_the_test(vm)?;
    the_result_is_kept(vm, &test.pane)?;

    vm.press_power_button()?;
    vm.expect("spark: every service has stopped")?;
    vm.expect("Power: `spark` asked to power off the machine; syncing the filesystems")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}

/// Step 1: nothing is asked, and no test has run.
fn nothing_is_asked(vm: &mut Vm, test: &mut Test) -> Result<(), String> {
    let status = test.status(vm)?;
    let nothing: Value = serde_json::from_str(NOTHING).map_err(|err| err.to_string())?;
    if status != nothing {
        return Err(format!("the status at first is {status}, not {NOTHING}"));
    }
    Ok(())
}

/// Step 2: the test is asked for the next start, which the status and the
/// file say, and asked again it is done again.
fn the_test_is_asked(vm: &mut Vm, test: &mut Test) -> Result<(), String> {
    test.unlocked()?;
    let answer = test.changed(ASK)?;
    let said = answer["output"].to_string();
    if answer["changed"] != json!(true)
        || !said.contains("the next start tests the free memory first")
    {
        return Err(format!("asking for the test is answered {answer}"));
    }
    let status = test.status(vm)?;
    let asked = &status["asked"];
    if asked["by_name"] != json!("alice") || asked["by"] != json!(1000) {
        return Err(format!("the status does not say alice asked: {status}"));
    }
    if asked["at_ms"].as_u64().is_none_or(|at| at == 0) || !status["last"].is_null() {
        return Err(format!("the status after the ask is {status}"));
    }
    run(vm, &format!("echo \"asked $(stat -c %a {ASKED_FILE})\""), 30, &["asked 644"])?;
    test.changed(ASK)?;
    Ok(())
}

/// Step 3: while the test is asked, the next start is not a search's:
/// `changes bisect` is refused with the ID of the page that says so.
fn a_search_is_refused(vm: &mut Vm) -> Result<(), String> {
    run(
        vm,
        "elevate changes bisect --yes > /tmp/b.txt 2>&1; echo \"bisect $? $(grep -c 'first \
         (HW-10[5])' /tmp/b.txt)\"",
        60,
        &["bisect 1 1"],
    )
}

/// Step 4: the ask is taken back, the file goes, and it is taken back
/// again with nothing changed; then the test is asked once more.
fn the_ask_is_taken_back(vm: &mut Vm, test: &mut Test) -> Result<(), String> {
    let first = test.changed(CANCEL)?;
    if first["changed"] != json!(true) {
        return Err(format!("cancelling what was asked is answered {first}"));
    }
    let status = test.status(vm)?;
    if !status["asked"].is_null() {
        return Err(format!("the status after the cancel is {status}"));
    }
    run(vm, &format!("[ -e {ASKED_FILE} ] || echo \"gone $((6 * 7))\""), 30, &["gone 42"])?;
    let again = test.changed(CANCEL)?;
    if again["changed"] != json!(false) {
        return Err(format!("cancelling what was not asked is answered {again}"));
    }
    test.changed(ASK)?;
    let status = test.status(vm)?;
    if status["asked"]["by_name"] != json!("alice") {
        return Err(format!("the status after asking again is {status}"));
    }
    Ok(())
}

/// Step 5: the restart the command's words name. The start menu takes the
/// try, and the kernel tests the free memory before it starts anything,
/// saying so, how far it has come, and that it passed. alice signs in
/// again on tty1, and elevate asks her password again.
fn restarted_into_the_test(vm: &mut Vm) -> Result<(), String> {
    answer_prompt(vm, "$", "clear; elevate shutdown -r now")?;
    vm.expect("spark: asked to restart the machine")?;
    vm.expect("Memory test: testing ")?;
    vm.expect("Memory test: 50% of ")?;
    vm.expect("Memory test: passed: ")?;
    vm.expect("Boot complete.")?;
    vm.expect_anywhere("consoled: started /system/commands/login on tty1 as process ")?;
    answer_prompt(vm, "Name:", "alice")?;
    answer_prompt(vm, "Password:", ALICE_PASSWORD)?;
    elevated(vm)
}

/// Step 6: once the health check has kept the result, the status has it,
/// the start's command line has the try and the test, and `changes --list`
/// names the start.
fn the_result_is_kept(vm: &mut Vm, pane: &Pane) -> Result<(), String> {
    run(
        vm,
        "i=0; while [ $i -lt 150 ] && [ \"$(hardware memtest --status --json | grep -c \
         'result[^,]*passed')\" -lt 1 ]; do sleep 2; i=$((i + 1)); done; [ $i -lt 150 ] && echo \
         \"waited $((6 * 7))\"",
        RESULT_SECONDS,
        &["waited 42"],
    )?;
    let status = printed(vm, pane)?;
    let last = &status["last"];
    let tested = last["tested_bytes"].as_u64().unwrap_or(0);
    if last["result"] != json!("passed")
        || last["complete"] != json!(true)
        || last["errors"] != json!(0)
    {
        return Err(format!("the last test is not a complete pass: {status}"));
    }
    if tested < TESTED_LEAST || last["total_bytes"].as_u64().is_none_or(|total| total < tested) {
        return Err(format!("the test covered {tested} bytes, not {TESTED_LEAST}: {status}"));
    }
    if !status["asked"].is_null() || status["history"].as_array().map(Vec::len) != Some(1) {
        return Err(format!("the status after the test is {status}"));
    }
    run(
        vm,
        &format!(
            "echo \"cmdline $(grep -c 'trial=9[9]' /kernel/cmdline) $(grep -c 'memtest=onc[e]' \
             /kernel/cmdline)\"; echo \"listed $(elevate changes --list | grep -c 'memory test \
             at the star[t]')\"; [ -e {ASKED_FILE} ] || echo \"cleared $((6 * 7))\""
        ),
        60,
        &["cmdline 1 1", "listed 1", "cleared 42"],
    )
}

/// The second start, with `memtest=fault` given by hand: one error in one
/// range, which the status keeps, the kernel counts as corrupted memory,
/// and alertd has as a critical alert.
fn second_start(vm: &mut Vm, _case: &Case) -> Result<(), String> {
    vm.expect("Memory test: testing ")?;
    vm.expect("Memory test: 1 errors in ")?;
    owner_made(vm)?;
    vm.expect_anywhere("netd: net0 has 10.0.2.15/24")?;
    let pane = Pane::open()?;
    elevated(vm)?;
    run(
        vm,
        "i=0; while [ $i -lt 150 ] && [ \"$(elevate alerts list --json | grep -c \
         'memory-test.*critical')\" -lt 1 ]; do sleep 2; i=$((i + 1)); done; [ $i -lt 150 ] && \
         echo \"alerted $((6 * 7))\"",
        RESULT_SECONDS,
        &["alerted 42"],
    )?;
    run(
        vm,
        "echo \"kind $(elevate alerts list --json | grep 'memory-test' | grep -c \
         '\"about\":\"hardware\"')\"",
        60,
        &["kind 1"],
    )?;
    let status = printed(vm, &pane)?;
    let last = &status["last"];
    let bad = last["bad"].as_array().map_or(0, Vec::len);
    if last["result"] != json!("errors") || last["errors"] != json!(1) || bad != 1 {
        return Err(format!("the last test is not one error in one range: {status}"));
    }
    if last["retired"].as_u64().is_none_or(|pages| pages < 1) || !status["asked"].is_null() {
        return Err(format!("the status after the test is {status}"));
    }
    run(
        vm,
        "echo \"corrupted $(grep -c 'hardware_corrupted: 4 KiB' /processes/meminfo)\"",
        30,
        &["corrupted 1"],
    )?;

    vm.press_power_button()?;
    vm.expect("spark: every service has stopped")?;
    vm.expect("Power: `spark` asked to power off the machine; syncing the filesystems")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}

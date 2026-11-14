// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What a service's limits do (ADR 0476), at the console as an
//! administrator. A service with `memory = "64M"` runs a program that
//! takes memory until the domain's limit refuses a page: Spark says it
//! was stopped at its memory limit, `service status` says so and shows
//! the refused charges, and another service goes on running. A service
//! with `threads = 64` forks until fork fails with EAGAIN, and a shell
//! command still answers within 2 seconds. Both again at once, each
//! stopped by its own limit. A service with `open_files = 10000` starts a
//! shell whose `ulimit -n` and `ulimit -Hn` both say 10000.

use super::super::*;

/// The service files this test copies into place.
const TESTS: &str = "/system/tests/services/limits";

/// Where Spark reads service files.
const FILES: &str = "/system/settings/spark/services";

/// The test.
pub const CASE: Case = Case {
    name: "limits",
    about: "a service's limits (ADR 0476): a memory hog in a service with a 64 MiB limit is stopped at it, which Spark and service status say, while another service keeps running; a fork bomb in a service with 64 threads gets EAGAIN and a shell command still answers within 2 seconds; both at once; open_files = 10000 gives the program that soft and hard limit",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    cmdline: "console-sign-in=off",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &[],
    script: limits,
    disks: &[],
    second: None,
};

/// Waits until the log has said `needle` `times` times in all.
fn expect_times(vm: &mut Vm, needle: &str, times: usize) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(90);
    while vm.log().matches(needle).count() < times {
        if Instant::now() >= deadline {
            return Err(format!("the log never said \"{needle}\" {times} times"));
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    Ok(())
}

/// How many children the last fork bomb of the log says it made before
/// fork failed with EAGAIN.
fn children_made(vm: &Vm) -> Result<u32, String> {
    let log = vm.log();
    let said = "forkbomb: fork failed with EAGAIN after ";
    let at = log.rfind(said).ok_or("the fork bomb never said fork failed with EAGAIN")?;
    log[at + said.len()..]
        .split(' ')
        .next()
        .and_then(|count| count.parse().ok())
        .ok_or_else(|| "the fork bomb's count of children does not read".to_string())
}

fn limits(vm: &mut Vm, _case: &Case) -> Result<(), String> {
    vm.expect("spark: started consoled (/system/commands/consoled) as process ")?;
    vm.expect_screen(&[&format!("{RELEASE} {VERSION} (tty1)"), "# "])?;

    // The service files: each is read when its service is first started.
    answer_prompt(
        vm,
        "#",
        &format!(
            "clear; cp {TESTS}/*.toml {FILES}/; echo \"checked $(service check limits-hog \
             limits-steady limits-bomb limits-files | grep -c ': ok')\""
        ),
    )?;
    vm.expect_screen(&["checked 4"])?;
    answer_prompt(vm, "#", "clear; service limits-steady start -q; echo \"steady $?\"")?;
    vm.expect_screen(&["steady 0"])?;

    // The memory hog takes memory until the limit refuses a page: Spark
    // says it was stopped at its memory limit, not by the out-of-memory
    // policy, which only runs when the machine's memory is gone.
    answer_prompt(vm, "#", "clear; service limits-hog start -q; echo \"hog $?\"")?;
    vm.expect("spark: started limits-hog (/system/tests/posix/hog) as process ")?;
    vm.expect(" was ended by signal 9 (stopped at its memory limit)")?;
    answer_prompt(vm, "#", "clear; service limits-hog status | grep -e Limits: -e Use:")?;
    vm.expect_screen(&[
        "Limits: memory up to 64M; CPU weight 100; at most 4096 threads",
        "Use: memory ",
        "refused; stopped at its memory limit",
    ])?;
    // Another service has gone on running.
    answer_prompt(vm, "#", "clear; service limits-steady status | grep Active:")?;
    vm.expect_screen(&["Active: active (running)"])?;

    // The fork bomb makes fewer than 64 children, as its own thread is the
    // first of the 64, and fork fails with EAGAIN. A shell command, which
    // forks as it runs, answers within 2 seconds.
    answer_prompt(vm, "#", "clear; service limits-bomb start -q; echo \"bomb $?\"")?;
    vm.expect("limits-bomb: forkbomb: fork failed with EAGAIN after ")?;
    let made = children_made(vm)?;
    if !(1..64).contains(&made) {
        return Err(format!("the fork bomb made {made} children, not 1 to 63"));
    }
    answer_prompt(vm, "#", "clear; timeout 2 ls /system > /dev/null; echo \"answered $?\"")?;
    vm.expect_screen(&["answered 0"])?;
    // The threads are the second of the Use lines, which has no label.
    answer_prompt(
        vm,
        "#",
        "clear; service limits-bomb status | grep -e Limits: -e '^ *threads [0-9]* of'",
    )?;
    vm.expect_screen(&["at most 64 threads", " of 64"])?;

    // Both at once, started again: each is stopped by its own limit, and
    // the shell still answers.
    answer_prompt(
        vm,
        "#",
        "clear; service limits-hog restart --no-wait -q; service limits-bomb restart --no-wait \
         -q; echo \"both $?\"",
    )?;
    expect_times(vm, " was ended by signal 9 (stopped at its memory limit)", 2)?;
    expect_times(vm, "limits-bomb: forkbomb: fork failed with EAGAIN after ", 2)?;
    answer_prompt(vm, "#", "clear; timeout 2 ls /system > /dev/null; echo \"answered $?\"")?;
    vm.expect_screen(&["answered 0"])?;
    answer_prompt(vm, "#", "clear; service limits-steady status | grep Active:")?;
    vm.expect_screen(&["Active: active (running)"])?;

    // open_files: the shell Spark starts has the soft and the hard limit
    // set before it runs.
    answer_prompt(vm, "#", "clear; service limits-files start -q; echo \"files $?\"")?;
    vm.expect("limits-files: soft 10000 hard 10000")?;
    answer_prompt(vm, "#", "clear; service limits-files status | grep Limits:")?;
    vm.expect_screen(&["10000 open files"])?;

    vm.type_text("poweroff\n")?;
    vm.expect("spark: asked to power off the machine")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}

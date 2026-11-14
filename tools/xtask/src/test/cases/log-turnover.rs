// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A service's own log files, turned over (ADR 0478), at the console as
//! an administrator. turnlog (userspace/spark/tests/services/log-turnover)
//! writes a number to a file of its logs folder each second, opening the
//! file for each line, and its file names every `.log` file of `logs`,
//! with `keep = 3`. *service check* reads its `[logs]`. *logturn --now turnlog* runs five
//! times, with the service writing between the runs: three files are
//! kept next to the current one, the first of them as written and the
//! older two compressed with zstd, and none of a fourth. Read from the
//! oldest to the newest, with *zstd -d* for the compressed ones, the
//! numbers are in order and none is there twice: no line is lost
//! between the files kept, and nothing is cut. The power button ends it.

use super::super::*;

/// The service files this test copies into place.
const TESTS: &str = "/system/tests/services/log-turnover";

/// Where Spark reads service files.
const FILES: &str = "/system/settings/spark/services";

/// The folder of the service's log.
const LOGS: &str = "/system/state/turnlog/logs";

/// The test.
pub const CASE: Case = Case {
    name: "log-turnover",
    about: "a service's log files are turned over (ADR 0478): logturn --now renames the file named by the service's logs setting five times while the service writes on, keeps three turned files, the first plain and the older two compressed with zstd, and removes the fourth; read from the oldest to the newest, the numbers are in order and none is twice",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    cmdline: "console-sign-in=off",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &[],
    script: log_turnover,
    disks: &[],
    second: None,
};

fn log_turnover(vm: &mut Vm, _case: &Case) -> Result<(), String> {
    vm.expect("spark: started consoled (/system/commands/consoled) as process ")?;
    vm.expect_screen(&[&format!("{RELEASE} {VERSION} (tty1)"), "# "])?;

    answer_prompt(
        vm,
        "#",
        &format!(
            "clear; cp {TESTS}/*.toml {FILES}/; service check turnlog | grep -c ': ok'; service \
             turnlog enable; echo \"enable $?\""
        ),
    )?;
    vm.expect_screen(&["1", "enable 0"])?;
    vm.expect("spark: started turnlog (/system/commands/sh) as process ")?;

    // Five turns, the service writing between them.
    for turn in 1..=5 {
        answer_prompt(
            vm,
            "#",
            &format!(
                "clear; sleep 4; logturn --now turnlog; echo \"turn {turn} status $?\""
            ),
        )?;
        vm.expect_screen(&[&format!("turn {turn} status 0")])?;
    }

    // Three kept, none of a fourth, the older two compressed.
    answer_prompt(
        vm,
        "#",
        &format!(
            "clear; cd {LOGS}; sleep 2; ls x.log* | tr '\\n' ' '; echo; echo \"fourth $(ls | grep -c 'x.log.4')\""
        ),
    )?;
    vm.expect_screen(&["x.log x.log.1 x.log.2.zst x.log.3.zst", "fourth 0"])?;

    // Oldest to newest, in order and none twice.
    answer_prompt(
        vm,
        "#",
        "clear; (zstd -dc x.log.3.zst; zstd -dc x.log.2.zst; cat x.log.1 x.log) > /tmp/all; \
         sort -n -c /tmp/all; echo \"sorted $?\"; uniq -d /tmp/all | wc -l; wc -l < /tmp/all",
    )?;
    vm.expect_screen(&["sorted 0", "0"])?;

    answer_prompt(vm, "#", "poweroff")?;
    vm.expect("spark: asked to power off the machine")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}

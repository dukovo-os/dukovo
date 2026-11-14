// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Sockets Spark holds for a service (ADR 0477), at the console as an
//! administrator. listenfd (userspace/posix/listenfd.c) is a service
//! whose file names a TCP socket on port 8090 and a Unix socket and
//! starts at the first connection: enabled, it is not running, and
//! *service status* says it waits for a connection on port 8090. The
//! first `nc` starts it and is answered with the line of the socket it
//! reached, as is a connection to the Unix socket. *service stop* puts
//! it back to waiting, and a connection starts it again. While
//! connectloop (userspace/posix/connectloop.c) makes 500 connections one
//! after another, *service restart* stops and starts the service: none
//! is refused, none fails, since the connections wait in the socket's
//! backlog for the new process. *service check* refuses a file whose
//! socket is on a port that its network.listen does not name, at the
//! line of its port.

use super::super::*;

/// The service files this test copies into place.
const TESTS: &str = "/system/tests/services/sockets";

/// Where Spark reads service files.
const FILES: &str = "/system/settings/spark/services";

/// The test.
pub const CASE: Case = Case {
    name: "held-sockets",
    about: "sockets Spark holds for a service (ADR 0477): a service that starts at the first connection and has a TCP socket on port 8090 and a Unix socket is not running once enabled, and service status says it waits for a connection; the first nc starts it and is answered by the socket it reached, as is a connection to the Unix socket; service stop puts it back to waiting; service restart while 500 connections are made one after another refuses none and fails none; service check refuses a socket on a port network.listen does not name, at its line",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    cmdline: "console-sign-in=off",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &[],
    script: held_sockets,
    disks: &[],
    second: None,
};

/// How many times the log says Spark started the service.
fn starts(vm: &Vm) -> usize {
    vm.log().matches("spark: started sockets-echo (/system/tests/posix/listenfd) as process ").count()
}

fn held_sockets(vm: &mut Vm, _case: &Case) -> Result<(), String> {
    vm.expect("spark: started consoled (/system/commands/consoled) as process ")?;
    vm.expect_screen(&[&format!("{RELEASE} {VERSION} (tty1)"), "# "])?;

    // A file whose socket is on a port its permissions do not name is
    // refused at the line of the port.
    answer_prompt(
        vm,
        "#",
        &format!(
            "clear; cp {TESTS}/*.toml {FILES}/; service check sockets-wrong; echo \"wrong $?\""
        ),
    )?;
    vm.expect_screen(&[
        "sockets-wrong.toml:17:1",
        "port 8091 is none of network.listen's ports",
        "add 8091 to network.listen",
    ])?;
    answer_prompt(vm, "#", "clear; service check sockets-echo | grep -c ': ok'")?;
    vm.expect_screen(&["1"])?;

    // Enabled, it waits: Spark holds its sockets and has started nothing.
    answer_prompt(vm, "#", "clear; service sockets-echo enable; echo \"enable $?\"")?;
    vm.expect_screen(&["enable 0"])?;
    if starts(vm) != 0 {
        return Err("Spark started sockets-echo before any connection".into());
    }
    answer_prompt(
        vm,
        "#",
        "clear; service sockets-echo status | grep -c 'waits for a connection on port 8090'",
    )?;
    vm.expect_screen(&["1"])?;

    // The first connection starts it and is answered by the socket it
    // reached.
    answer_prompt(vm, "#", "clear; echo | nc -w 10 127.0.0.1 8090; echo \"nc $?\"")?;
    vm.expect_screen(&["hello from tcp", "nc 0"])?;
    vm.expect("spark: started sockets-echo (/system/tests/posix/listenfd) as process ")?;
    vm.expect_anywhere("sockets-echo: listenfd: serving 2 sockets")?;
    answer_prompt(
        vm,
        "#",
        "clear; echo | nc -w 10 -U /system/live/sockets-echo/greet; echo \"unix $?\"",
    )?;
    vm.expect_screen(&["hello from unix", "unix 0"])?;
    if starts(vm) != 1 {
        return Err("the second connection started the service again".into());
    }

    // Stopped by request, it waits again, and a connection starts it.
    answer_prompt(vm, "#", "clear; service sockets-echo stop; echo \"stop $?\"")?;
    vm.expect_screen(&["stop 0"])?;
    answer_prompt(
        vm,
        "#",
        "clear; service sockets-echo status | grep -c 'waits for a connection on port 8090'",
    )?;
    vm.expect_screen(&["1"])?;
    answer_prompt(vm, "#", "clear; echo | nc -w 10 127.0.0.1 8090; echo \"again $?\"")?;
    vm.expect_screen(&["hello from tcp", "again 0"])?;
    if starts(vm) != 2 {
        return Err(format!("Spark started sockets-echo {} times, not 2", starts(vm)));
    }

    // A restart while connections come one after another: the sockets
    // stay open, so none is refused.
    answer_prompt(
        vm,
        "#",
        "clear; /system/tests/posix/connectloop 127.0.0.1 8090 500 10 > /tmp/loop & sleep 1; \
         service sockets-echo restart; echo \"restart $?\"; wait",
    )?;
    answer_prompt_within(vm, "#", "clear; cat /tmp/loop", 120)?;
    vm.expect_screen(&["connections 500 refused 0 failed 0"])?;

    vm.type_text("poweroff\n")?;
    vm.expect("spark: asked to power off the machine")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}

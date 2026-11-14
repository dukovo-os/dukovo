// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The commands an administrator knows from other Linux servers (ADR
//! 0478), at the console as root: systemctl, journalctl, hostnamectl
//! and sendmail. systemctl says what Spark says of logd in systemd's
//! words and statuses (*is-active*, *is-enabled*, *status*,
//! *list-units*, and *show -p* with and without *--value*), restarts
//! notifyd and finds it active, answers a service Spark does not know
//! with systemd's words and status 5 (*reload*) or *inactive* and 3
//! (*is-active*), and says what to run for *mask*, *--user* and the
//! other verbs and options Dukovo does not have. journalctl reads the
//! log by service (*-u*), count (*-n*), level (*-p*), kernel (*-k*),
//! time (*--since*) and format (*-o json*), and says what to run for
//! *--rotate*. hostnamectl shows the machine's name, the release, the
//! kernel and the architecture, gives the machine another name, which
//! *hostname* and the settings file both say, refuses a name that is
//! none, and gives the first one back. sendmail refuses a message
//! without a recipient (64) and one when alerts.toml names no mail
//! server (78), and with a server on lo that nothing listens at, ends
//! with the status for a server to try again at (75) and says why. The
//! success of a message sent through a server is the unit tests' of
//! the mail module, which run a server on lo, and the monitoring test's,
//! whose mail server hears alertd's messages through the same code: this
//! test has no server of its own. The power button ends it.

use super::super::*;

/// The test.
pub const CASE: Case = Case {
    name: "admin-commands",
    about: "systemctl, journalctl, hostnamectl and sendmail work as an administrator used to Linux types them (ADR 0478): systemctl says logd's state in systemd's words and statuses, restarts notifyd, answers an unknown service as systemd does, and says what to run for mask and --user; journalctl reads the log by service, count, level, kernel, time and as JSON, and says what to run for --rotate; hostnamectl shows the machine, sets its name and gives it back, and refuses a bad name; sendmail refuses a message without a recipient, one without a mail server named, and one for a server that is not there, each with its status",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    cmdline: "console-sign-in=off",
    cpus: 2,
    memory: "512M",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &[],
    script: admin_commands,
    disks: &[],
    second: None,
};

/// Types `command` at the root prompt, on a clear screen, and waits for
/// the screen to show each of `needles`.
fn run(vm: &mut Vm, command: &str, needles: &[&str]) -> Result<(), String> {
    answer_prompt(vm, "#", &format!("clear; {command}"))?;
    vm.expect_screen(needles)
}

/// systemctl, as the commands a person types for Spark's services.
fn systemctl(vm: &mut Vm) -> Result<(), String> {
    run(vm, "s=$(systemctl is-active logd); r=$?; echo \"A $s $r\"", &["A active 0"])?;
    run(vm, "s=$(systemctl is-active nosuch); r=$?; echo \"B $s $r\"", &["B inactive 3"])?;
    run(vm, "s=$(systemctl is-enabled logd.service); r=$?; echo \"C $s $r\"", &["C enabled 0"])?;
    run(vm, "systemctl status logd | grep Active:", &["Active: active (running)"])?;
    run(vm, "systemctl list-units --type=service | grep logd", &["logd.service", "loaded"])?;
    run(vm, "systemctl list-units --failed --no-legend; echo \"D $?\"", &["D 0"])?;
    run(
        vm,
        "systemctl show -p ActiveState,SubState logd",
        &["ActiveState=active", "SubState=running"],
    )?;
    run(
        vm,
        "n=$(systemctl show -p MainPID --value logd); [ \"$n\" -gt 0 ]; echo \"E $?\"",
        &["E 0"],
    )?;
    run(vm, "systemctl show -p LoadState nosuch", &["LoadState=not-found"])?;
    run(vm, "systemctl cat logd | head -n 1", &["logd.toml"])?;
    // A restart waits until the service is back, so it is active at once.
    run(vm, "systemctl restart notifyd; echo \"F $?\"", &["F 0"])?;
    run(vm, "systemctl is-active -q notifyd; echo \"G $?\"", &["G 0"])?;
    run(vm, "systemctl reload nosuch; echo \"H $?\"", &["Unit nosuch.service not found.", "H 5"])?;
    run(vm, "systemctl mask logd; echo \"I $?\"", &["no masking", "I 1"])?;
    run(vm, "systemctl --user status logd; echo \"J $?\"", &["crontab", "J 1"])?;
    run(vm, "systemctl frobnicate; echo \"K $?\"", &["no command 'frobnicate'", "K 1"])
}

/// journalctl, as the log's reader.
fn journalctl(vm: &mut Vm) -> Result<(), String> {
    run(vm, "echo \"L $(journalctl -u logd -n 1 --no-pager | grep -c logd)\"", &["L 1"])?;
    run(vm, "journalctl -p err --no-pager > /dev/null; echo \"M $?\"", &["M 0"])?;
    run(vm, "echo \"N $(journalctl -k -n 2 --no-pager | grep -c .)\"", &["N 2"])?;
    run(vm, "echo \"O $(journalctl -u logd -n 1 -o json | grep -c text)\"", &["O 1"])?;
    run(vm, "echo \"P $(journalctl --since -1h -u logd -n 1 -o cat | grep -c .)\"", &["P 1"])?;
    run(vm, "journalctl --rotate; echo \"Q $?\"", &["elevate logtrim", "Q 1"])
}

/// hostnamectl, showing the machine's name and changing it and back.
fn hostnamectl(vm: &mut Vm) -> Result<(), String> {
    run(
        vm,
        "hostnamectl status | grep -e Static -e Operating -e Kernel -e Architecture",
        &["Static hostname:", "Operating System: Dukovo", "Kernel: Radix", "Architecture:"],
    )?;
    run(vm, "old=$(hostname); echo \"R $old $(hostnamectl hostname)\"", &["R dukovo dukovo"])?;
    run(vm, "hostnamectl set-hostname admintest; echo \"S $?\"", &["S 0"])?;
    run(
        vm,
        "echo \"T $(hostname) $(hostnamectl hostname) $(cat /system/settings/hostname)\"",
        &["T admintest admintest admintest"],
    )?;
    run(vm, "hostnamectl set-hostname bad_name; echo \"U $?\"", &["letters, digits", "U 1"])?;
    run(vm, "hostnamectl set-hostname $old; echo \"V $(hostname) $?\"", &["V dukovo 0"])
}

/// sendmail, in the ways it fails.
fn sendmail(vm: &mut Vm) -> Result<(), String> {
    let message = "printf 'Subject: test\\n\\nhello\\n'";
    run(vm, &format!("{message} | sendmail -t; echo \"W $?\""), &["no recipients", "W 64"])?;
    run(
        vm,
        &format!("{message} | sendmail ops@example.test; echo \"X $?\""),
        &["names no [email] server", "X 78"],
    )?;
    // A mail server named, and nothing listening at it.
    run(
        vm,
        "printf '[email]\\nto = [\"ops@example.test\"]\\nserver = \"127.0.0.1\"\\n' \
         > /system/settings/alerts.toml; chmod 600 /system/settings/alerts.toml; echo \"Y $?\"",
        &["Y 0"],
    )?;
    run(
        vm,
        &format!("{message} | sendmail ops@example.test; echo \"Z $?\""),
        &["cannot connect to 127.0.0.1:587", "Z 75"],
    )
}

fn admin_commands(vm: &mut Vm, _case: &Case) -> Result<(), String> {
    vm.expect("spark: started consoled (/system/commands/consoled) as process ")?;
    vm.expect_screen(&[&format!("{RELEASE} {VERSION} (tty1)"), "# "])?;
    systemctl(vm)?;
    journalctl(vm)?;
    hostnamectl(vm)?;
    sendmail(vm)?;
    vm.type_text("poweroff\n")?;
    vm.expect("spark: asked to power off the machine")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}

// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Alerts reach the owner's channels (ADR 0485): alertd(8) takes the
//! alerts that root's scripts and the services raise, tells each change
//! once, and sends it to the email address and the webhooks that
//! alerts.toml names. An installed system's disk boots under UEFI and
//! alice, the owner, signs in. *monitoring-test*, the test's own program
//! (userspace/programs/monitoring-test), runs as a service and listens on
//! 127.0.0.1 for a mail server on port 587, which offers STARTTLS and
//! takes one account only after it, and for a webhook receiver in HTTPS
//! on port 8443 and in plain HTTP on port 8080; they write what they are
//! sent to /shared/monitoring. Both use the certificate the *tls* test
//! holds for 127.0.0.1, whose authority an administrator adds to the
//! system's roots, as dukovo-certificates(7) says. alice reaches
//! administration through elevate, which asks for her password once.
//!
//! The first steps are the user's. alice is no root, so *alerts* refuses
//! her, and so does alertd, whose own refusal a request to its socket as
//! alice shows. As an administrator she then has *monitoring-test setup*
//! start the receivers and write alerts.toml at mode 600: the email, and
//! a webhook of each kind, ntfy, Slack, Discord and Matrix, with a token,
//! to the HTTPS receiver, and the json kind, with a token, to the plain
//! one. *alerts test* sends to each channel and says each was sent; the
//! mail server heard the test over TLS as the account it takes, to the
//! address named, and each webhook heard it as its kind sends it: the
//! tokens of Matrix and json as bearer tokens, and only json without TLS.
//!
//! Then she raises an alert, raises it again with another detail, clears
//! it, raises a critical one, and has Spark restart alertd, all within the
//! 30 seconds alertd waits before it sends. The changes were kept in the
//! files alertd saves, and after the restart one message per channel
//! tells of the first alert's raising and clearing, and of the second
//! alert's raising: each said once, by every channel, the second raising
//! of the first alert said by none because it changed nothing, and none
//! said again by the restart. The history holds three changes, the second
//! alert is active still and the first is not, the owner was notified of
//! each of the three, and the notice for the next sign-in names the
//! second alert alone.
//!
//! A wrong password for the mail server and an address the receiver
//! answers 403 for are said in the servers' own words by *alerts test*,
//! which sends to the other channels all the same and exits 1. An
//! alerts.toml that anyone may read is refused: no channel is set up,
//! *alerts status* says so, and the log says to chmod it. The power
//! button ends it.

use super::super::*;

pub const CASE: Case = Case {
    name: "monitoring",
    about: "alertd takes the alerts that root's scripts raise and tells each change once, on the screen and at the next sign-in and to the email and webhooks alerts.toml names (ADR 0485), as an installed system's owner finds: a mail server with STARTTLS and a webhook receiver of the test's own program hear alerts test over TLS, as the account and the tokens they take; an alert raised, raised again, cleared, and another raised across a restart of alertd, are each sent once by every channel; a user who is not root is refused by the command and by alertd; the servers' own refusals reach alerts test; an alerts.toml others can read is refused",
    firmware: Firmware::Uefi,
    media: Media::Disk,
    cmdline: "",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Installed,
    cpu_model: None,
    qemu: &[],
    script: monitoring,
    disks: &[],
    second: None,
};

/// alice's password, which elevate asks for once.
const PASSWORD: &str = "queen of hearts";

/// What the shell is told first, so that the rest is typed shorter: the
/// test's program, and the administrator's `alerts`.
const SHORTHAND: &str =
    "m=/system/tests/posix/monitoring-test; a=\"elevate alerts\"; echo \"set $((6 * 7))\"";

/// How long a command that waits for alertd's 30 seconds may take, with
/// the time to read the screen, in seconds.
const BATCH_SECONDS: u64 = 120;

/// The lines `alerts test` says when every channel took the test alert.
const ALL_SENT: [&str; 6] = [
    "email to ops@example.test: sent",
    "ntfy (127.0.0.1): sent",
    "slack (127.0.0.1): sent",
    "discord (127.0.0.1): sent",
    "matrix (127.0.0.1): sent",
    "json (127.0.0.1): sent",
];

/// Waits until the screen shows each of `needles`, for up to `seconds`.
fn shown_within(vm: &mut Vm, needles: &[&str], seconds: u64) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(seconds);
    loop {
        match vm.expect_screen(needles) {
            Err(_) if Instant::now() < deadline => {}
            done => return done,
        }
    }
}

/// Types `command` at alice's prompt, on a cleared screen, and checks that
/// the screen shows each of `shown` once it has run: up to `seconds`.
/// Each of `shown` is output, never in the command itself, so that the
/// command's own echo is not taken for its answer.
fn run(vm: &mut Vm, command: &str, seconds: u64, shown: &[&str]) -> Result<(), String> {
    answer_prompt(vm, "$", &format!("clear; {command}"))?;
    shown_within(vm, shown, seconds).map_err(|err| format!("`{command}`: {err}"))
}

fn monitoring(vm: &mut Vm, _case: &Case) -> Result<(), String> {
    owner_signs_in(vm)?;
    run(vm, SHORTHAND, 15, &["set 42"])?;
    a_user_is_refused(vm)?;
    receivers_and_settings(vm)?;
    the_test_alert_reaches_every_channel(vm)?;
    alerts_raised_and_cleared_across_a_restart(vm)?;
    refusals_are_said_in_the_servers_words(vm)?;
    an_open_settings_file_is_refused(vm)?;
    metrics(vm)?;

    vm.press_power_button()?;
    vm.expect("spark: every service has stopped")?;
    vm.expect("Power: `spark` asked to power off the machine; syncing the filesystems")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}

/// The machine has no account yet: the owner's comes first, as in the
/// crash-dump test, which also starts an installed system's console
/// sign-in. alertd is among the services Spark starts.
fn owner_signs_in(vm: &mut Vm) -> Result<(), String> {
    vm.expect("Boot complete.")?;
    vm.expect_anywhere("spark: started alertd (/system/commands/alertd) as process ")?;
    vm.expect_anywhere("authd: 0 accounts; answering at /system/live/authd/socket")?;
    vm.expect_anywhere("consoled: started /system/commands/login on tty1 as process ")?;
    vm.expect_screen(&[
        &format!("{RELEASE} {VERSION}"),
        "This computer has no account yet: make yours, the owner's,",
    ])?;
    answer_prompt(vm, "Name:", "alice")?;
    answer_prompt(vm, "Full name (may be empty):", "Alice Liddell")?;
    answer_prompt(vm, "New password:", PASSWORD)?;
    answer_prompt(vm, "The same again:", PASSWORD)?;
    vm.expect_anywhere("authd: alice signed in on pts/")?;
    vm.expect_screen(&["Made alice, number 1000, home /users/alice"])
}

/// alice is no root: the command says so before it asks alertd, and a
/// request to the socket as her is answered with alertd's own refusal,
/// which the kernel's word on who is asking decides.
fn a_user_is_refused(vm: &mut Vm) -> Result<(), String> {
    run(
        vm,
        "alerts list; echo \"list $?\"; $m ask '{\"ask\":\"list\"}'; echo \"ask $?\"",
        30,
        &[
            "only root reads and raises alerts; try elevate alerts",
            "list 1",
            "only root and the system's services raise and read alerts",
            "ask 0",
        ],
    )
}

/// As an administrator, through elevate: the first command, a short one so
/// that the whole of it shows in the prompt, has her type her password,
/// which unlocks her pane for the commands after it. Then the receivers
/// start and alertd reads alerts.toml, which names them.
fn receivers_and_settings(vm: &mut Vm) -> Result<(), String> {
    answer_prompt(vm, "$", "clear; elevate sleep 0; echo \"unlocked $?\"")?;
    answer_prompt(vm, "sleep 0:", PASSWORD)?;
    shown_within(vm, &["unlocked 0"], 30)?;
    run(
        vm,
        "elevate $m setup; echo \"setup $?\"",
        60,
        &["monitoring-test: the receivers listen, and alerts.toml names them", "setup 0"],
    )
}

/// The test alert goes to every channel, and what each receiver heard is
/// how its kind sends it.
fn the_test_alert_reaches_every_channel(vm: &mut Vm) -> Result<(), String> {
    let mut shown = ALL_SENT.to_vec();
    shown.extend([
        "test 0",
        "seen can read this: mail 1 ntfy 1 slack 1 discord 1 matrix 1 json 1",
        "heard mail tls=yes user=alerts@example.test to=ops@example.test",
        "heard ntfy tls=yes token=none",
        "heard slack tls=yes token=none",
        "heard discord tls=yes token=none",
        "heard matrix tls=yes token=matrix-token",
        "heard json tls=no token=json-token",
    ]);
    run(vm, "$a test; echo \"test $?\"; $m seen --details 'can read this'", 60, &shown)
}

/// The first alert is raised, raised again with another detail, and
/// cleared, and a critical one is raised, and alertd is restarted, all
/// within the 30 seconds before it sends. Every channel then gets one
/// message that tells of the first raising, the clearing and the second
/// alert, each detail said once: the one of the second raising of the
/// first alert is the one the clearing carries, and no more.
fn alerts_raised_and_cleared_across_a_restart(vm: &mut Vm) -> Result<(), String> {
    run(
        vm,
        "$a raise t1 --title \"Boot A\" --detail mark1; $a raise t1 --title \"Boot A\" --detail \
         mark2; $a clear t1; $a raise t2 --critical --title \"Boot B\" --detail mark3; elevate \
         service alertd restart; echo \"raised $?\"",
        30,
        &["raised 0"],
    )?;
    run(
        vm,
        "$m wait mark3 && $m seen mark1 mark2 mark3",
        BATCH_SECONDS,
        &[
            "seen mark1: mail 1 ntfy 1 slack 1 discord 1 matrix 1 json 1",
            "seen mark2: mail 1 ntfy 1 slack 1 discord 1 matrix 1 json 1",
            "seen mark3: mail 1 ntfy 1 slack 1 discord 1 matrix 1 json 1",
        ],
    )?;

    // What alertd kept: the three changes, the second alert active, the
    // owner told of each, and the next sign-in's notice.
    run(
        vm,
        "echo \"rows $($a history | grep -c Boot) active $($a list | grep -c Boot) told $(elevate \
         notifications | grep -c Boot)\"; elevate cat /system/live/sign-in/alerts",
        30,
        &["rows 3 active 1 told 3", "Alert: Boot B (since "],
    )?;
    vm.expect_screen_without("Alert: Boot A (")
}

/// A wrong password and an address the receiver refuses are each said in
/// the server's own words; the other four channels are sent to all the
/// same.
fn refusals_are_said_in_the_servers_words(vm: &mut Vm) -> Result<(), String> {
    run(
        vm,
        "elevate $m settings broken; $a test > b.txt; echo \"failed $?\"; echo \"refused $(grep \
         -c -e 535 -e 403 b.txt) sent $(grep -c sent b.txt)\"",
        60,
        &["failed 1", "refused 2 sent 4"],
    )
}

/// alerts.toml that others can read holds secrets: alertd sets up no
/// channel from it, and the log says what to do.
fn an_open_settings_file_is_refused(vm: &mut Vm) -> Result<(), String> {
    run(
        vm,
        "elevate $m settings loose; $a status; echo \"logged $(elevate log -s alertd -p warning | \
         grep -c 'read by others')\"",
        60,
        &["No channels are set up; notices go to the screen. See alerts.toml(5).", "logged 1"],
    )
}

/// The steps of the metrics half of this test (metricsd, ADR 0484) go
/// here, after the alerts and before the machine is powered off. They
/// run as alice, who has the shorthand `$a` for `elevate alerts` and
/// `$m` for the test's program, and use [`run`] to type a command and
/// check the screen:
///
/// - `metrics` lists cpu, memory and fs series after 5 seconds;
/// - `metrics show fs.used --since 1m` has rows;
/// - `curl http://127.0.0.1:9100/metrics` with `listen` set has
///   node_exporter's names, and a request from an address outside
///   `allow` is closed;
/// - service.cpu for metricsd stays under its budget.
///
/// A small test disk filled to 95 percent raises `fs-full`, which reaches
/// the mail server, and freeing it clears it: the monitoring-test
/// program's `wait` and `seen` read what the receivers heard.
fn metrics(_vm: &mut Vm) -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_channel_the_test_sends_to_has_a_line_in_alerts_test() {
        // The names alertd gives its channels: the kind and the address's
        // host, and the email by its recipient.
        for kind in ["ntfy", "slack", "discord", "matrix", "json"] {
            assert!(ALL_SENT.iter().any(|line| line.starts_with(&format!("{kind} (127.0.0.1)"))));
        }
        assert!(ALL_SENT[0].starts_with("email to ops@example.test"));
    }

    #[test]
    fn what_a_command_waits_for_is_not_in_the_command() {
        // The screen is read for output: a line that the typed command
        // holds would be found before the command had run.
        let command = "$a test; echo \"test $?\"; $m seen --details 'can read this'";
        for needle in ["test 0", "seen can read this: mail 1"] {
            assert!(!command.contains(needle), "{needle}");
        }
        assert!(!SHORTHAND.contains("set 42"));
    }
}

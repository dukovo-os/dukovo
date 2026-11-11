// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Every example the docs mark as a test, a block that opens with
//! *```console test* in the handbook, a guide, a tutorial or a runbook,
//! is typed at the console: each command must succeed, and the screen
//! must then show each line the example shows after it. So an example on
//! dukovo.org/support is one that worked on this build, and a change that
//! breaks it fails like any other test (ADR 0420).

use super::super::*;

pub const CASE: Case = Case {
    name: "docs-examples",
    about: "every example the handbook, the guides, the tutorials and the runbooks mark as a test, typed at the console: each command succeeds and the screen then shows what the example shows",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    cmdline: "console-sign-in=off",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &[],
    script: docs_examples,
    disks: &[],
    second: None,
};

/// The `docs-examples` test.
fn docs_examples(vm: &mut Vm, _case: &Case) -> Result<(), String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let examples = crate::site::check::tree_examples(&root)?;
    if examples.is_empty() {
        return Err("the docs mark no example as a test".into());
    }

    vm.expect("spark: started consoled (/system/commands/consoled) as process ")?;
    vm.expect_screen(&[&format!("{RELEASE} {VERSION} (tty1)"), "# "])?;

    let mut count = 0;
    for (file, example) in &examples {
        for step in &example.steps {
            count += 1;
            // A clear screen, so that what it shows is this command's, and
            // its exit status, as no command prints it: 1000 for success.
            // The arithmetic keeps what is typed from matching what is shown.
            vm.type_text(&format!(
                "clear; {}\necho \"example $(( {count} )) status $(( $? + 1000 ))\"\n",
                step.command
            ))?;
            let place = format!("{file}:{} `{}`", example.line, step.command);
            let done = format!("example {count} status ");
            if let Err(err) = vm.expect_screen(&[&format!("{done}1000")]) {
                let screen = vm.screen()?;
                let Some(status) =
                    screen.iter().find_map(|line| Some(line[line.find(&done)? + done.len()..].trim()))
                else {
                    return Err(format!("{place}: {err}"));
                };
                let code = status.parse::<i32>().map_or(status.to_string(), |s| (s - 1000).to_string());
                return Err(format!("{place}: exited with status {code}"));
            }
            let shows: Vec<&str> = step.shows.iter().map(String::as_str).collect();
            vm.expect_screen(&shows).map_err(|err| format!("{place}: {err}"))?;
        }
    }
    eprintln!("    {count} commands of {} examples ran as the docs show", examples.len());

    vm.type_text("poweroff\n")?;
    vm.expect("spark: asked to power off the machine")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}

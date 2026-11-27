// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The processor's speculative-execution flaws as the kernel reports them
//! (ADR 0790), at the console as an administrator. The directory
//! /kernel/devices/system/cpu/vulnerabilities has a file for each flaw
//! the kernel knows, named as Linux's are, and each holds one line that
//! starts with *Not affected*, *Vulnerable*, *Mitigation: * or *Unknown: *,
//! whichever processor QEMU emulates. *lscpu* ends with a *Vulnerability*
//! line for each file, and *dukovo-check* has a line for them under
//! *health*.

use super::super::*;

/// The test.
pub const CASE: Case = Case {
    name: "cpu-flaws",
    about: "the kernel's word on the processor's flaws (ADR 0790): /kernel/devices/system/cpu/vulnerabilities has a file for each of radix-cpuflaws's flaws and no other, each of one line that starts with Not affected, Vulnerable, Mitigation: or Unknown:; lscpu ends with a Vulnerability line for each file; dukovo-check has a processor flaws line",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    cmdline: "console-sign-in=off",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &[],
    script: cpu_flaws,
    disks: &[],
    second: None,
};

/// The directory of the files.
const DIRECTORY: &str = "/kernel/devices/system/cpu/vulnerabilities";

fn cpu_flaws(vm: &mut Vm, _case: &Case) -> Result<(), String> {
    vm.expect("spark: started consoled (/system/commands/consoled) as process ")?;
    vm.expect_screen(&[&format!("{RELEASE} {VERSION} (tty1)"), "# "])?;

    // Each flaw has its file, no other file is there, and each file is one
    // whole line of one of the four kinds. The loops run in the shell,
    // which prints only the totals.
    let names: Vec<&str> = radix_cpuflaws::Flaw::ALL.iter().map(|flaw| flaw.name()).collect();
    let check = format!(
        "d={DIRECTORY}; n=0; m=0; b=0; for f in $d/*; do n=$((n+1)); l=$(cat $f); case \"$l\" in \
         \"Not affected\"|Vulnerable*|\"Mitigation: \"*|\"Unknown: \"*) ;; *) b=$((b+1));; esac; \
         [ $(wc -l < $f) -eq 1 ] || b=$((b+1)); done; for f in {}; do [ -f $d/$f ] || \
         m=$((m+1)); done; echo \"flaws $n missing $m bad $b\"",
        names.join(" ")
    );
    answer_prompt(vm, "#", &format!("clear; {check}"))?;
    vm.expect_screen(&[&format!("flaws {} missing 0 bad 0", names.len())])?;

    // lscpu says each file's line after its other lines, so the number of
    // its Vulnerability lines is the number of files.
    answer_prompt(vm, "#", "clear; echo \"lscpu $(lscpu | grep -c '^Vulnerability ')\"")?;
    vm.expect_screen(&[&format!("lscpu {}", names.len())])?;

    // dukovo-check gives the flaws their own line, a warning or a pass.
    answer_prompt(
        vm,
        "#",
        "clear; echo \"line $(dukovo-check | grep -c '^health  *processor flaws  *\\(passed\\|warning\\)')\"",
    )?;
    vm.expect_screen(&["line 1"])?;
    Ok(())
}

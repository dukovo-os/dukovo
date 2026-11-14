// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! How services share a disk (ADR 0476), at the console as an
//! administrator. Three services read the same virtio disk raw, with
//! nothing cached, 16 threads each: one of the default I/O weight, 100,
//! one of 400, and one held to 10 MiB a second by `io_read = "10M"`. QEMU
//! throttles the disk to 64 MiB a second, so that it is the slowest part
//! and requests wait in Dukovo's queue, which decides whose go first.
//! From the counters of the services' domains, as `service status` shows
//! them, in a window of 10 seconds with each reading stamped by the
//! kernel's clock, the heavier service reads 3 to 5 times the bytes a
//! second the lighter one does, and the rated one 9 to 11 MiB a second.

use super::super::*;

/// The service files this test copies into place.
const TESTS: &str = "/system/tests/services/io-weight";

/// Where Spark reads service files.
const FILES: &str = "/system/settings/spark/services";

/// What QEMU lets the disk do each second in all, in bytes: less than
/// the three readers ask for, so that the disk's queue is full.
const DISK_RATE: u64 = 64 << 20;

/// The services, in the order the readings are taken.
const SERVICES: [&str; 3] = ["iow-light", "iow-heavy", "iow-rated"];

/// The disk, of 64 MiB the readers read round and round: `vda`, the
/// first virtio disk, as the boot is from USB.
const DISKS: &[Disk] = &[Disk { kib: 64 * 1024, ..raw_disk(Bus::Virtio, raw_unchanged) }];

/// The test.
pub const CASE: Case = Case {
    name: "io-weight",
    about: "how services share a disk (ADR 0476): three services read one throttled virtio disk raw at once, with I/O weights 100 and 400 and with io_read = 10M; by the counters of their domains, in a window of 10 seconds, the heavier reads 3 to 5 times the bytes a second of the lighter, and the rated one reads 9 to 11 MiB a second",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    cmdline: "console-sign-in=off",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &[],
    script: io_weight,
    disks: DISKS,
    second: None,
};

/// What the line typed takes: a function that says the kernel's clock
/// before and after it asks for a service's reading, and the bytes its
/// domain read; then two passes over the services, 10 seconds apart.
/// `-n 0` leaves the log's lines out of the answer, which asks logd.
const READINGS: &str = "clear; r() { a=$(cut -d ' ' -f 2 /processes/uptime); v=$(service $1 status -n 0 \
                        | grep -o 'disk read [0-9.]*[KMGT]*' | cut -d ' ' -f 3); b=$(cut -d ' ' -f 2 \
                        /processes/uptime); echo \"$1 $a $b $v\"; }; sleep 5; for s in iow-light \
                        iow-heavy iow-rated; do r $s; done; sleep 10; for s in iow-light iow-heavy \
                        iow-rated; do r $s; done; echo \"read $?\"";

/// A size `human` wrote, such as `5.0M` or `412M`, in bytes.
fn size(text: &str) -> Result<f64, String> {
    let digits = text.trim_end_matches(|c: char| c.is_ascii_alphabetic());
    let scale = match &text[digits.len()..] {
        "" => 1.0,
        "K" => 1024.0,
        "M" => 1024.0 * 1024.0,
        "G" => 1024.0 * 1024.0 * 1024.0,
        other => return Err(format!("`{text}` has the unit `{other}`")),
    };
    digits.parse::<f64>().map(|number| number * scale).map_err(|_| format!("`{text}` is no size"))
}

/// A reading: when the kernel's clock said it was, in seconds, as the
/// middle of the two times that stamped the asking, and what the domain
/// had read by then, in bytes.
struct Reading {
    seconds: f64,
    bytes: f64,
}

/// The readings of `service` on the screen, in the order they were taken.
fn readings(screen: &[String], service: &str) -> Result<Vec<Reading>, String> {
    screen
        .iter()
        .filter_map(|line| {
            let words: Vec<&str> = line.split_whitespace().collect();
            match words.as_slice() {
                [name, from, to, bytes] if *name == service => Some((*from, *to, *bytes)),
                _ => None,
            }
        })
        .map(|(from, to, bytes)| {
            let ns = |text: &str| text.parse::<f64>().map_err(|_| format!("`{text}` is no time"));
            Ok(Reading { seconds: (ns(from)? + ns(to)?) / 2e9, bytes: size(bytes)? })
        })
        .collect()
}

/// What `service` read a second between its two readings, in bytes.
fn rate(screen: &[String], service: &str) -> Result<f64, String> {
    let taken = readings(screen, service)?;
    let [before, after] = taken.as_slice() else {
        return Err(format!("the screen shows {} readings of {service}, not 2", taken.len()));
    };
    let window = after.seconds - before.seconds;
    if window < 8.0 {
        return Err(format!("{service}'s two readings are only {window:.1} s apart"));
    }
    Ok((after.bytes - before.bytes) / window)
}

fn io_weight(vm: &mut Vm, _case: &Case) -> Result<(), String> {
    vm.expect("spark: started consoled (/system/commands/consoled) as process ")?;
    vm.expect_screen(&[&format!("{RELEASE} {VERSION} (tty1)"), "# "])?;
    // The disk is slower than the readers: QEMU holds it to DISK_RATE, so
    // that the requests wait in Dukovo's queue and not in the disk's.
    vm.monitor(&format!("block_set_io_throttle disk0 {DISK_RATE} 0 0 0 0 0"))?;

    answer_prompt(
        vm,
        "#",
        &format!(
            "clear; cp {TESTS}/*.toml {FILES}/; echo \"checked $(service check iow-light \
             iow-heavy iow-rated | grep -c ': ok')\""
        ),
    )?;
    vm.expect_screen(&["checked 3"])?;
    // Together, so that they start within a second of one another.
    answer_prompt(
        vm,
        "#",
        "clear; for s in iow-light iow-heavy iow-rated; do service $s start -q; done; echo \
         \"started $?\"",
    )?;
    vm.expect_screen(&["started 0"])?;
    for service in SERVICES {
        vm.expect_anywhere(&format!(
            "spark: started {service} (/system/tests/posix/diskread) as process "
        ))?;
    }
    vm.expect_anywhere("diskread: 16 threads read 64 MiB of /devices/vda")?;

    // The Limits line says the weights and the rate; two readings of each
    // service's counters, 10 seconds apart, say what they read.
    answer_prompt(vm, "#", READINGS)?;
    answer_prompt_within(vm, "#", "echo finished", 120)?;
    vm.expect_screen(&["read 0", "finished"])?;
    let screen = vm.screen()?;
    let (light, heavy, rated) =
        (rate(&screen, SERVICES[0])?, rate(&screen, SERVICES[1])?, rate(&screen, SERVICES[2])?);
    let mib = |bytes: f64| bytes / f64::from(1 << 20);
    let said = format!(
        "light {:.1} MiB/s, heavy {:.1} MiB/s, rated {:.1} MiB/s",
        mib(light),
        mib(heavy),
        mib(rated)
    );
    if light <= 0.0 {
        return Err(format!("the lighter service read nothing: {said}"));
    }
    if !(3.0..=5.0).contains(&(heavy / light)) {
        return Err(format!("the weights 100 and 400 should give 3 to 5 times the bytes: {said}"));
    }
    if !(9.0..=11.0).contains(&mib(rated)) {
        return Err(format!("io_read = \"10M\" should give 9 to 11 MiB a second: {said}"));
    }

    // What status says of their limits.
    answer_prompt(
        vm,
        "#",
        "clear; for s in iow-light iow-heavy iow-rated; do service $s status -n 0 | grep -e \
         'disk weight'; done",
    )?;
    vm.expect_screen(&[
        "disk weight 100",
        "disk weight 400",
        "disk weight 100; reads up to 10M a second",
    ])?;

    vm.type_text("poweroff\n")?;
    vm.expect("spark: asked to power off the machine")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}

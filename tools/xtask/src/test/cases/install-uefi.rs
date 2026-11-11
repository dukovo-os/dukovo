// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The install stick boots under UEFI into the live system, beside a
//! blank virtio disk, and *dukovo-install --answers* installs Dukovo onto
//! that disk as the test's answers file says (ADR 0309): the guided
//! layout, the signed release copied and checked file by file into the
//! slot *@system.a*, GRUB for both firmwares, the host name, time zone
//! and keyboard, and the accounts of alice, the administrator, and bob.
//! On the host, the disk must hold that layout, an EFI system partition
//! that dosfstools' *fsck.fat* finds sound with UEFI's GRUB and a menu
//! naming the root by its UUID, and a root Linux's *btrfs check* finds
//! sound. The second boot is from that disk alone, without the stick:
//! alice signs in on the console with her password, and finds the
//! system, the settings and the saved answers the install left, without
//! the password's hash, and File sharing, which the answers' Server
//! profile added, running.

use std::process::Command;

use disk::{Scheme, types};

use super::super::*;

pub const CASE: Case = Case {
    name: "install-uefi",
    about: "The live system installs Dukovo onto a blank disk under UEFI from an answers file: guided layout, signed release copied and checked, GRUB, settings and accounts; the host's fsck.fat and btrfs check find the disk sound, and it then boots alone, where alice signs in and finds the install's settings",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    // The live menu entry has its own flags, `live` and
    // `console-sign-in=off`, before these.
    cmdline: "",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Live,
    cpu_model: None,
    qemu: &[],
    script: install,
    disks: INSTALL_DISKS,
    second: Some(SecondBoot { cmdline: "", before: nothing, script: installed }),
};

/// The blank disk Dukovo is installed onto, `vda`: 4 GiB, room for the
/// answers file's 1 GiB of swap, the system and more.
pub const INSTALL_DISKS: &[Disk] = &[Disk {
    kib: 4 * 1024 * 1024,
    fat: INSTALL_TARGET,
    cluster: 0,
    label: "",
    table: Table::None,
    bus: Bus::Virtio,
    sector: 512,
    device: "",
    errors: None,
    read_only: false,
    check: installed_disk,
}];

/// The answers file the tests install by, from
/// userspace/programs/installer/tests/install. Its administrator, alice,
/// has the password [`PASSWORD`], hashed at the tests' low cost.
pub const ANSWERS: &str = "/system/tests/install/answers.json";

/// alice's password.
pub const PASSWORD: &str = "queen of hearts";

/// How long the install may take, in seconds: it copies the whole system.
const INSTALL_SECONDS: u64 = 240;

/// Nothing to do on the host between the boots: the disk check has run.
pub fn nothing(_case: &Case, _disks: &[DiskImage]) -> Result<(), String> {
    Ok(())
}

/// The first boot, from the stick: the live system installs Dukovo onto
/// `vda` as the answers file says, then powers off. install-bios's first
/// boot is the same, with the answers file from the web.
pub fn install(vm: &mut Vm, _case: &Case) -> Result<(), String> {
    live_shell(vm)?;
    install_by(vm, ANSWERS, None)
}

/// Waits for root's shell on tty1 of the live system the stick started.
pub fn live_shell(vm: &mut Vm) -> Result<(), String> {
    vm.expect(
        "spark: the live system: /system is the boot archive, in memory, and no disk is the root",
    )?;
    vm.expect("spark: started consoled (/system/commands/consoled) as process ")?;
    vm.expect_screen(&[&format!("{RELEASE} {VERSION} (tty1)"), "# "])
}

/// Installs Dukovo onto `vda` from the live system's shell as the answers
/// file `from` says, a file or an address, checked against `sha256` if
/// given; checks that the log says where the answers came from, and that
/// the disk holds the install; then powers off.
pub fn install_by(vm: &mut Vm, from: &str, sha256: Option<&str>) -> Result<(), String> {
    install_from(vm, from, sha256, "stick")
}

/// As [`install_by`], from the live system on `medium`, the install
/// `stick` or `disc`, which dukovo-install says to remove at the end.
pub fn install_from(
    vm: &mut Vm,
    from: &str,
    sha256: Option<&str>,
    medium: &str,
) -> Result<(), String> {
    let check = sha256.map_or_default(|sha256| format!(" --sha256 {sha256}"));
    // The shell shows each command as it is typed, so the answer is a sum
    // the command line does not hold.
    vm.type_text(&format!(
        "dukovo-install --answers {from}{check} > /tmp/out 2>&1; echo \"install: $((100 + $?))\"\n"
    ))?;
    wait_for_screen(vm, "install: 1", INSTALL_SECONDS)?;
    if !vm.screen()?.iter().any(|line| line.trim() == "install: 100") {
        vm.type_text("clear; tail -n 8 /tmp/install.log\n")?;
        let _ = vm.expect_screen(&["problem: "]);
        return Err(format!("dukovo-install failed: {}", vm.screen()?.join(" | ")));
    }
    vm.type_text("clear; cat /tmp/out\n")?;
    vm.expect_screen(&[
        "dukovo-install: Checking the signatures of the system release",
        &format!("dukovo-install: Copying Dukovo {VERSION} into its slot, checking every file"),
        &format!("is installed on vda. Remove the install {medium}, then restart."),
    ])?;

    // The log names the answers file, with its SHA-256, checked when one
    // was given (ADR 0319). Each label is split, so the command line does
    // not hold it whole.
    let checked = match sha256 {
        Some(sha256) => format!("SHA-256 {sha256}, checked)"),
        None => "not checked: none was given)".to_string(),
    };
    vm.type_text(&format!(
        "clear; echo \"fr\"\"om: $(grep -c -F 'answers from {from} (' /tmp/install.log)\"; \
         echo \"che\"\"ck: $(grep -c -F '{checked}' /tmp/install.log)\"\n"
    ))?;
    vm.expect_screen(&["from: 1", "check: 1"])?;

    // The root holds the subvolumes, the slot among them and its copy
    // gone. `mount -o compress` finds the Btrfs without `-t`, as Linux's.
    vm.type_text(
        "clear; mkdir /tmp/look && mount -o compress vda4 /tmp/look && ls /tmp/look && \
         umount /tmp/look; echo \"looked: $((10 + $?))\"\n",
    )?;
    vm.expect_screen(&["@settings", "@state", "@system.a", "@users", "looked: 10"])?;
    vm.expect_screen_without("@system.new")?;

    vm.type_text("poweroff\n")?;
    vm.expect("spark: asked to power off the machine")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}

/// The second boot, from the installed disk alone, now a SATA disk: GRUB
/// finds the root by its UUID, Spark turns on its swap, alice signs in
/// with her password, and the system, its settings, the saved answers
/// and the feature they added are as the install left them.
/// install-bios's second boot is the same.
pub fn installed(vm: &mut Vm, _case: &Case) -> Result<(), String> {
    signed_in(vm)?;
    // The answers' Server profile added File sharing, so Spark started
    // its services at this first boot (ADR 0317), and the web console
    // they turned off is saved as off.
    vm.expect_anywhere("spark: started smbd (/system/commands/smbd) as process ")?;
    vm.expect_anywhere("spark: started nfsd (/system/commands/nfsd) as process ")?;
    answer_prompt(vm, "$", "clear; feature list | tail -1")?;
    vm.expect_screen(&["file-sharing  added      File sharing"])?;
    answer_prompt(
        vm,
        "$",
        "echo \"console\"\": $(grep -c '\"web_console\": false' /system/settings/install/answers.json)\"",
    )?;
    vm.expect_screen(&["console: 1"])?;
    power_off(vm)
}

/// The installed system started: alice signs in with her password, and
/// the system, its settings and the saved answers are as the answers
/// file had them installed.
pub fn signed_in(vm: &mut Vm) -> Result<(), String> {
    vm.expect("spark: /system is Dukovo's root on ")?;
    vm.expect("spark: turned on swap on ")?;
    vm.expect_anywhere("consoled: started /system/commands/login on tty1 as process ")?;
    vm.expect_screen(&[&format!("{RELEASE} {VERSION}")])?;
    answer_prompt(vm, "Name:", "alice")?;
    answer_prompt(vm, "Password:", PASSWORD)?;
    vm.expect_anywhere("authd: alice signed in on pts/")?;

    // Each answer has a label the command line does not hold whole.
    answer_prompt(vm, "$", "echo \"host\"\": $(cat /system/settings/hostname)\"")?;
    vm.expect_screen(&["host: installed"])?;
    answer_prompt(vm, "$", "echo \"zone\"\": $(readlink /system/settings/localtime)\"")?;
    vm.expect_screen(&["zone: /system/resources/zoneinfo/America/Denver"])?;
    answer_prompt(vm, "$", "echo \"keys\"\": $(cat /system/settings/keyboard)\"")?;
    vm.expect_screen(&["keys: layout = us"])?;
    answer_prompt(
        vm,
        "$",
        "echo \"people\"\": $(grep -c -e '^alice:x:1000:' -e '^bob:x:1001:' /system/settings/passwd)\"",
    )?;
    vm.expect_screen(&["people: 2"])?;
    answer_prompt(vm, "$", "echo \"admins\"\": $(grep '^admins:' /system/settings/group)\"")?;
    vm.expect_screen(&["admins: admins:x:10:alice"])?;

    // The system is the release's, in the slot GRUB started.
    answer_prompt(vm, "$", "ls /system/startup; echo \"slot\"\": $((40 + 2))\"")?;
    vm.expect_screen(&["archive", "radix", "slot: 42"])?;

    // Spark turned on the swap partition the answers asked for, the one
    // on the root's disk (ADR 0311).
    answer_prompt(vm, "$", "echo \"swap\"\": $(grep -c partition /processes/swaps)\"")?;
    vm.expect_screen(&["swap: 1"])?;

    // The answers are kept, without the password's hash, and the log.
    answer_prompt(
        vm,
        "$",
        "grep -q '\"hostname\": \"installed\"' /system/settings/install/answers.json && \
         ! grep -q argon2 /system/settings/install/answers.json; echo \"saved: $((10 + $?))\"",
    )?;
    vm.expect_screen(&["saved: 10"])?;
    answer_prompt(
        vm,
        "$",
        "grep -q 'installed: the log ends here' /system/state/log/install.log; \
         echo \"log: $((10 + $?))\"",
    )?;
    vm.expect_screen(&["log: 10"])
}

/// The power button stops every service and powers off.
pub fn power_off(vm: &mut Vm) -> Result<(), String> {
    vm.press_power_button()?;
    vm.expect("spark: every service has stopped")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}

/// Waits until a line of the screen shows `needle`, for up to `seconds`:
/// longer than [`Vm::expect_screen`] waits, for a command that takes a
/// while.
pub fn wait_for_screen(vm: &mut Vm, needle: &str, seconds: u64) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(seconds);
    loop {
        let screen = vm.screen()?;
        if screen.iter().any(|line| line.contains(needle)) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return screen_shows(&screen, &[needle])
                .map_err(|err| format!("after {seconds} s, {err}"));
        }
        std::thread::sleep(Duration::from_secs(1));
    }
}

/// The partitions of the guided layout, in order, with 1 GiB of swap.
const GUIDED: &[&str] = &["bios-boot", "efi", "swap", "dukovo-root"];

/// Where UEFI's GRUB and its menu are on the EFI system partition.
const EFI_GRUB: &str = "::/EFI/BOOT/BOOTX64.EFI";
const EFI_MENU: &str = "::/boot/grub/grub.cfg";

/// The host's side, after each boot: the disk holds the guided layout,
/// GRUB's `boot.img` in its first sector, an EFI system partition that
/// `fsck.fat` finds sound with UEFI's GRUB, a menu that names the root by
/// its UUID and both copies of the slot state, and a root Linux's `btrfs
/// check` finds sound.
pub fn installed_disk(disk: &DiskImage, log: &str) -> Result<(), String> {
    holds_install(disk, log, GUIDED)
}

/// The host's side of an install whose disk ends with the partitions of
/// `layout`, in order, by their rdisk aliases: GRUB's `boot.img` in its
/// first sector if it has a BIOS boot partition, its EFI system partition
/// sound with UEFI's GRUB, a menu that names the root by its UUID and
/// the slot state starting slot A, and the root sound.
pub fn holds_install(disk: &DiskImage, log: &str, layout: &[&str]) -> Result<(), String> {
    let shown = disk.path.display().to_string();
    let file = std::fs::File::open(&disk.path).map_err(|e| format!("opening {shown}: {e}"))?;
    let device = disk::Device::whole(file, disk.bytes, 512);
    let found = disk::read(&device).map_err(|e| format!("{shown}: {e}"))?;
    let table = found.table.ok_or_else(|| format!("{shown} has no partition table"))?;
    let kinds: Vec<_> = table.partitions.iter().map(|p| p.kind).collect();
    let wanted: Vec<_> =
        layout.iter().map(|name| types::parse(name, Scheme::Gpt).expect("a known type")).collect();
    if kinds != wanted {
        let names: Vec<_> = kinds.iter().map(|kind| kind.name()).collect();
        return Err(format!(
            "{shown} holds partitions of {}, not the layout's {}",
            names.join(", "),
            layout.join(", ")
        ));
    }
    let part = |n: usize| {
        let p = &table.partitions[n];
        DiskImage {
            path: disk.path.clone(),
            offset: p.first * 512,
            bytes: p.sectors() * 512,
            sector: 512,
            check: disk.check,
            btrfs: disk.btrfs.clone(),
        }
    };

    if layout.contains(&"bios-boot") {
        let first = device.read_sectors(0, 1).map_err(|e| format!("reading {shown}: {e}"))?;
        // boot.img starts with its jump over the parameter block.
        if first[..2] != [0xeb, 0x63] || first[510..512] != [0x55, 0xaa] {
            return Err(format!("{shown}'s first sector holds no GRUB boot.img"));
        }
    }

    let at = |alias: &str| layout.iter().position(|name| *name == alias).map(part);
    // Dukovo's EFI system partition is the last: next to Windows, Windows'
    // own comes first.
    let esp = layout
        .iter()
        .rposition(|name| *name == "efi")
        .map(part)
        .ok_or("the layout has no EFI system partition")?;
    let root = at("dukovo-root").ok_or("the layout has no Dukovo root")?;
    let mut uuid = [0u8; 16];
    // A Btrfs superblock is 64 KiB in, with its filesystem's UUID 32
    // bytes into it.
    device
        .read_at(root.offset + 0x10020, &mut uuid)
        .map_err(|e| format!("reading {shown}: {e}"))?;
    let uuid = image::uuid_text(uuid);
    let menu = host_tool(Command::new("mtype").args(["-i", &mtools_image(&esp), EFI_MENU]))?;
    let menu = String::from_utf8_lossy(&menu);
    let wanted = [format!("--set=system {uuid}\n"), format!("set dukovo_root={uuid}\n")];
    if menu.contains("@ROOT_UUID@") || !wanted.iter().all(|line| menu.contains(line.as_str())) {
        return Err(format!("the EFI system partition's GRUB menu does not name the root {uuid}"));
    }
    // Both copies of the slot state are whole, and the newer starts slot
    // A (ADR 0452): GRUB and the health check write them at each start.
    let copies = slots::state::COPIES.map(|copy| {
        let path = format!("::/{copy}");
        host_tool(Command::new("mtype").args(["-i", &mtools_image(&esp), &path]))
            .map(|bytes| slots::State::decode(&bytes))
    });
    let [Ok(Some(first)), Ok(Some(second))] = copies else {
        return Err(format!(
            "the EFI system partition's slot state is not two whole copies: {copies:?}"
        ));
    };
    match slots::state::newest(Some(first), Some(second)) {
        Some((state, _)) if state.default == slots::Slot::A && state.trying.is_none() => {}
        newest => {
            return Err(format!("the newer copy of the slot state is {newest:?}, not slot A's"));
        }
    }
    host_tool(Command::new("mdir").args(["-i", &mtools_image(&esp), EFI_GRUB]))
        .map_err(|err| format!("the EFI system partition holds no UEFI GRUB: {err}"))?;
    fat_checked(&esp)?;
    root_checked(&root, log)
}

/// Checks the FAT of the EFI system partition `esp` with dosfstools'
/// `fsck.fat`, on a copy of it beside the disk.
fn fat_checked(esp: &DiskImage) -> Result<(), String> {
    let copy = esp.path.with_extension("esp");
    let copied = (|| {
        let mut from = std::fs::File::open(&esp.path)?;
        let mut to = std::fs::File::create(&copy)?;
        to.set_len(esp.bytes)?;
        image::copy_sparse(&mut from, esp.offset, &mut to, 0, esp.bytes)
    })()
    .map_err(|err| format!("copying the EFI system partition to {}: {err}", copy.display()));
    let checked = copied.and_then(|()| {
        host_tool(Command::new(fat_tool("fsck.fat")).arg("-n").arg(&copy))
            .map(drop)
            .map_err(|err| format!("the EFI system partition fails fsck.fat: {err}"))
    });
    let _ = std::fs::remove_file(&copy);
    checked
}

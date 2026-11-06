// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Building the boot images, with the GRUB `ob` builds (ADR 0306).
//!
//! A disk image, for a USB stick or a hard disk, is a GPT that the disk
//! engine makes, as rdisk and mkfs-fat do: GRUB for a BIOS in a BIOS boot
//! partition and the disk's first sector, as rgrub-setup puts it, an EFI
//! system partition the kernel's FAT crate fills, with UEFI's GRUB, and
//! the Dukovo root, with the system on Btrfs (ADR 0137). An ISO, for
//! optical media, is what xorriso makes of the same files, with El
//! Torito's boot images for both firmwares.
//!
//! The stick and the ISO boot the kernel and the whole boot archive from
//! the EFI system partition, as the live system ([`LIVE_CFG`]), which
//! takes no disk's root; the stick also carries the signed release the
//! installer installs, on its DUKOVO_LIVE partition ([`make_live`], ADR
//! 0308). An installed system's disk (`ob image --installed`) boots from
//! its root instead: GRUB finds the root by its Btrfs UUID and loads the
//! kernel and the startup archive from the system's slot,
//! `@system.a/startup` (ADR 0307). When an image will not start, look
//! first at its grub.cfg, made from `image/grub/`, and at what
//! [`stage_system`] put in the slot.

use std::fmt::Write;
use std::io::{Read, Seek, SeekFrom, Write as _};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::account::{self, Account};
use crate::config::{self, Profile};
use crate::process::{self, step};
use disk::blockfile::BlockFile;

use crate::grub::{self, Grub};
use crate::{Context, Ctx, Error, Result, VERSION, kernel, licenses, release, rootfs, userspace};

/// What goes into an image and where it is written.
pub struct ImageSpec<'a> {
    /// Kernel to boot.
    pub kernel: &'a Path,
    /// GRUB menu timeout in seconds.
    pub timeout: u32,
    /// Kernel command line.
    pub cmdline: &'a str,
    /// Intel microcode updates to load as a boot module, if any.
    pub microcode: Option<Vec<u8>>,
    /// The user programs: the test image and the boot archive, each
    /// loaded as a boot module.
    pub programs: Option<&'a userspace::Programs>,
    /// The serial number of the disk for a disk check entry in the boot
    /// menu, with Linux's side of the check beside it ([`disk_check`]).
    pub disk_check: Option<&'a str>,
    /// A file holding the Dukovo root's filesystem ([`make_root`]), to put
    /// after the image's partitions ([`append_root`]).
    pub root: Option<&'a Path>,
    /// A file holding the install stick's DUKOVO_LIVE filesystem
    /// ([`make_live`]), to put after the image's partitions
    /// ([`append_live`]).
    pub live: Option<&'a Path>,
    /// The boot menu, and so how the image starts the system.
    pub menu: Menu,
    /// Staging directory, recreated on every build.
    pub staging: PathBuf,
    /// Output file.
    pub output: PathBuf,
    /// Whether it is an ISO for optical media, not a disk image.
    pub iso: bool,
}

/// Which boot menu an image has, and so how it starts the system.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Menu {
    /// The boot tests' and the disk check's (`image/grub/grub.cfg.in`):
    /// the whole system from the boot archive, or from the first disk
    /// that holds a Dukovo root.
    Tests,
    /// The install stick's and the ISO's ([`LIVE_CFG`]): the live system,
    /// from the boot archive, which takes no disk's root (ADR 0308), with
    /// this entry started when nobody chooses one.
    Live(LiveStart),
    /// An installed system's ([`INSTALLED_CFG`]): the kernel and the
    /// startup archive from the slot of the root with this Btrfs UUID
    /// (ADR 0307). Its EFI system partition holds GRUB and its menu, and
    /// the microcode, but no kernel or archive.
    Installed([u8; 16]),
}

/// Which of the live menu's entries starts when nobody chooses one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LiveStart {
    /// *Install Dukovo*: the installer's screens on tty1, as a release's
    /// stick and ISO start (ADR 0310).
    Install,
    /// *Try Dukovo*: a shell on every console, as the boot tests' live
    /// image starts. A case that tests the screens adds `install` to its
    /// own command line.
    Try,
}

impl LiveStart {
    /// The entry's `--id` in [`LIVE_CFG`].
    fn id(self) -> &'static str {
        match self {
            Self::Install => "install",
            Self::Try => "try",
        }
    }
}

/// Which image `ob image` makes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// The install stick: the live system, from the boot archive, and on
    /// its DUKOVO_LIVE partition the signed release the installer
    /// installs (ADR 0308).
    Stick,
    /// An ISO for optical media and virtual machines: the live system
    /// alone.
    Iso,
    /// The disk of an installed system: the layout dukovo-install leaves,
    /// less the swap, starting from the root's slot (ADR 0307).
    Installed,
}

/// File name of a release image.
pub fn file_name(kind: Kind) -> String {
    match kind {
        Kind::Stick => format!("dukovo-{VERSION}-x86_64.img"),
        Kind::Iso => format!("dukovo-{VERSION}-x86_64.iso"),
        Kind::Installed => format!("dukovo-{VERSION}-x86_64-installed.img"),
    }
}

/// Path of the microcode module inside the image.
pub const MICROCODE_PATH: &str = "/boot/firmware/intel-ucode.bin";

/// What `ob image` and `ob run` put into the image beyond dukovo.toml.
#[derive(Default)]
pub struct Overrides {
    /// Kernel flags instead of `boot.cmdline`.
    pub cmdline: Option<String>,
    /// Microcode file or directory instead of `image.microcode`.
    pub microcode: Option<String>,
    /// The serial number of a disk to add the disk check of (ADR 0122).
    pub disk_check: Option<String>,
    /// An account to make in the root (ADR 0218).
    pub account: Option<Account>,
}

/// Builds the kernel and the release image described by dukovo.toml.
pub fn build_release(
    ctx: &Ctx,
    profile: Profile,
    kind: Kind,
    overrides: &Overrides,
) -> Result<PathBuf> {
    let microcode_source = overrides.microcode.as_deref().unwrap_or(&ctx.config.image.microcode);
    let microcode = if microcode_source.is_empty() {
        None
    } else {
        Some(read_microcode(ctx, Path::new(microcode_source))?)
    };
    if overrides.account.is_some() && kind != Kind::Installed {
        crate::bail!(
            "an account goes into an installed system's root, which only --installed makes: \
             the stick and the ISO run the live system, which keeps no accounts"
        );
    }
    let kernel = kernel::build(ctx, profile)?;
    let programs = userspace::build(ctx, profile)?;
    let (root, live) = (ctx.out.join("root.btrfs"), ctx.out.join("live.btrfs"));
    let menu = match kind {
        Kind::Installed => {
            let staging = ctx.out.join("root-staging");
            let account = overrides.account.as_ref();
            Menu::Installed(make_root(ctx, &kernel, &programs, &staging, &root, account)?)
        }
        Kind::Stick => {
            make_live(ctx, &kernel, &programs, &ctx.out.join("live-staging"), &live)?;
            Menu::Live(LiveStart::Install)
        }
        Kind::Iso => Menu::Live(LiveStart::Install),
    };
    // The features' flags go on whichever command line the image boots.
    let cmdline = overrides.cmdline.as_deref().unwrap_or(&ctx.config.boot.cmdline);
    let cmdline = ctx.config.features.cmdline(cmdline);
    let built = build(
        ctx,
        &ImageSpec {
            kernel: &kernel,
            timeout: ctx.config.boot.timeout,
            cmdline: &cmdline,
            microcode,
            programs: Some(&programs),
            disk_check: overrides.disk_check.as_deref(),
            root: (kind == Kind::Installed).then_some(root.as_path()),
            live: (kind == Kind::Stick).then_some(live.as_path()),
            menu,
            staging: ctx.out.join("image-root"),
            output: ctx.out.join(file_name(kind)),
            iso: kind == Kind::Iso,
        },
    );
    let _ = std::fs::remove_file(&root);
    let _ = std::fs::remove_file(&live);
    built?;
    Ok(ctx.out.join(file_name(kind)))
}

/// Reads Intel microcode from a file of updates, or from every file in a
/// directory such as `/lib/firmware/intel-ucode`, and checks that it holds
/// at least one valid update.
pub fn read_microcode(ctx: &Ctx, path: &Path) -> Result<Vec<u8>> {
    let mut files = Vec::new();
    if path.is_dir() {
        for entry in std::fs::read_dir(path).context(|| format!("reading {}", path.display()))? {
            let entry = entry.context(|| format!("reading {}", path.display()))?.path();
            if entry.is_file() {
                files.push(entry);
            }
        }
        files.sort();
    } else {
        files.push(path.to_path_buf());
    }
    let mut bytes = Vec::new();
    let (mut valid, mut invalid) = (0, 0);
    for file in &files {
        let data =
            std::fs::read(file).context(|| format!("reading microcode {}", file.display()))?;
        for update in radix_microcode::updates(&data) {
            match update {
                Ok(update) => {
                    bytes.extend_from_slice(update.bytes());
                    valid += 1;
                }
                Err(_) => invalid += 1,
            }
        }
    }
    if valid == 0 {
        return Err(Error::new(format!("{} holds no Intel microcode updates", ctx.display(path)))
            .hint(
                "give the raw update files, such as /lib/firmware/intel-ucode from the \
             intel-microcode package, not an initramfs image",
            ));
    }
    step(&format!(
        "Adding {valid} microcode updates from {}{}",
        ctx.display(path),
        if invalid > 0 { format!(" (skipped {invalid} damaged ones)") } else { String::new() }
    ));
    Ok(bytes)
}

/// Fills `@VERSION@`, `@TIMEOUT@`, `@CMDLINE@`, `@MODULES@` and
/// `@ENTRIES@` in the grub.cfg template. `modules` are (path, command
/// line) pairs, and `entries` more menu entries.
pub fn render_grub_cfg(
    template: &str,
    timeout: u32,
    cmdline: &str,
    modules: &[(&str, &str)],
    entries: &str,
) -> Result<String> {
    config::validate_cmdline(cmdline)?;
    let lines = module_lines(modules);
    let rendered = template
        .replace("@VERSION@", VERSION)
        .replace("@TIMEOUT@", &timeout.to_string())
        .replace("@CMDLINE@", cmdline.trim())
        .replace("@MODULES@", &lines)
        .replace("@ENTRIES@", entries);
    if let Some(name) = find_placeholder(&rendered) {
        crate::bail!("image/grub/grub.cfg.in uses unknown placeholder {name}");
    }
    Ok(rendered)
}

/// A menu entry's `module2` lines for `modules`.
fn module_lines(modules: &[(&str, &str)]) -> String {
    let mut lines = String::new();
    for (path, args) in modules {
        let _ = writeln!(lines, "    module2 {path} {args}");
    }
    lines
}

/// The GRUB configuration of an installed system's EFI system partition,
/// whose `@ROOT_UUID@` the installer fills too (ADR 0307).
pub const INSTALLED_CFG: &str = "image/grub/installed.cfg.in";

/// The GRUB configuration of the install stick and the ISO, which start
/// the live system (ADR 0308).
pub const LIVE_CFG: &str = "image/grub/live.cfg.in";

/// The boot tests' and the disk check's GRUB configuration.
const TESTS_CFG: &str = "image/grub/grub.cfg.in";

/// [`INSTALLED_CFG`] with the root's UUID in it, the other placeholders
/// left for [`render_grub_cfg`].
fn installed_template(ctx: &Ctx, uuid: [u8; 16]) -> Result<String> {
    let path = ctx.root.join(INSTALLED_CFG);
    let template =
        std::fs::read_to_string(&path).context(|| format!("reading {}", path.display()))?;
    Ok(template.replace("@ROOT_UUID@", &uuid_text(uuid)))
}

/// The live menu's template with the entry `start` as its default.
fn live_template(template: &str, start: LiveStart) -> String {
    template.replace("@DEFAULT@", start.id())
}

/// The installed system's menu in the system's GRUB folder, which
/// dukovo-install(8) writes on the EFI system partition of the disk it
/// installs, with the root's UUID in it (ADR 0309).
pub const INSTALLER_MENU: &str = "installed.cfg";

/// Writes [`INSTALLER_MENU`] into `dir`: [`INSTALLED_CFG`] with this
/// build's timeout and command line, and `@ROOT_UUID@` left for the
/// installer. Gives its path.
pub fn installer_menu(ctx: &Ctx, dir: &Path) -> Result<PathBuf> {
    let path = ctx.root.join(INSTALLED_CFG);
    let template =
        std::fs::read_to_string(&path).context(|| format!("reading {}", path.display()))?;
    let boot = &ctx.config.boot;
    let menu = installer_menu_text(&template, boot.timeout, &boot.cmdline)?;
    process::create_dir(dir)?;
    let out = dir.join(INSTALLER_MENU);
    std::fs::write(&out, menu).context(|| format!("writing {}", out.display()))?;
    Ok(out)
}

/// The text of [`INSTALLER_MENU`]: [`render_grub_cfg`] refuses a
/// placeholder it does not fill, so the root's is held out of its sight
/// and put back after.
fn installer_menu_text(template: &str, timeout: u32, cmdline: &str) -> Result<String> {
    const HELD: &str = "\u{1}ROOT_UUID\u{1}";
    let held = template.replace("@ROOT_UUID@", HELD);
    Ok(render_grub_cfg(&held, timeout, cmdline, &[], "")?.replace(HELD, "@ROOT_UUID@"))
}

/// A UUID as GRUB's `search --fs-uuid`, Linux's `blkid` and the kernel's
/// command line write it: lowercase, in groups of 8, 4, 4, 4 and 12
/// digits.
pub fn uuid_text(uuid: [u8; 16]) -> String {
    let mut text = String::with_capacity(36);
    for (i, byte) in uuid.iter().enumerate() {
        if matches!(i, 4 | 6 | 8 | 10) {
            text.push('-');
        }
        let _ = write!(text, "{byte:02x}");
    }
    text
}

/// Where the disk check's program is in the boot archive.
const DISK_CHECK_PROGRAM: &str = "/system/tests/posix/disktest";

/// Where the POSIX benchmarks are in the boot archive.
const BENCH_PROGRAM: &str = "/system/tests/posix/bench";

/// The boot menu entry of the disk check of the disk with serial number
/// `serial` (ADR 0122), which runs the kernel's benchmarks and then its
/// program under Spark, with no disk mounted, which also runs the POSIX
/// benchmarks (ADR 0124);
/// and Linux's side of the check in the image's `crosscheck` directory:
/// the program built for Linux, linked against the live system's C
/// library so that the image carries none of it, the benchmarks built
/// for Linux as static programs, and `run.sh`.
fn disk_check(
    ctx: &Ctx,
    spec: &ImageSpec<'_>,
    serial: &str,
    modules: &[(&str, &str)],
) -> Result<String> {
    let valid = !serial.is_empty()
        && serial.len() <= 40
        && serial.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c));
    if !valid {
        crate::bail!(
            "--disk-check takes a disk's serial number, of up to 40 letters, digits, dots, \
             dashes and underscores, not `{serial}`"
        );
    }
    if spec.cmdline.split_whitespace().any(|word| word == "--") {
        crate::bail!(
            "--disk-check runs a program of its own under Spark, so the kernel command line \
             cannot have a `--` part"
        );
    }
    if spec.cmdline.split_whitespace().any(|word| word.starts_with("selftest=")) {
        crate::bail!(
            "--disk-check runs the kernel's benchmarks, so the kernel command line cannot \
             name another self-test"
        );
    }
    step(&format!("Adding the disk check of the disk with serial number {serial}"));
    let dir = spec.staging.join("crosscheck");
    process::create_dir(&dir)?;
    let compiler = crate::ports::find_clang()
        .map(|(clang, _)| clang)
        .or_else(|| process::find_program("cc"))
        .ok_or_else(|| {
            Error::new("no C compiler is installed, for Linux's side of the disk check")
                .hint("run `./ob doctor`")
        })?;
    let mut cmd = Command::new(compiler);
    cmd.args(["-std=c11", "-D_GNU_SOURCE", "-O2", "-Wall", "-o"])
        .arg(dir.join("disktest"))
        .arg(ctx.root.join("userspace/posix/disktest.c"));
    process::run(ctx, &mut cmd)?;
    crate::bench::posix_for_linux(ctx, &dir)?;
    process::copy(&crate::bench::linux_bench_static(ctx)?, &dir.join("linux-bench"))?;
    let template = ctx.root.join("image/crosscheck/run.sh.in");
    let script =
        std::fs::read_to_string(&template).context(|| format!("reading {}", template.display()))?;
    let run = dir.join("run.sh");
    std::fs::write(&run, script.replace("@SERIAL@", serial))
        .context(|| format!("writing {}", run.display()))?;
    let cmdline = spec.cmdline.trim();
    let space = if cmdline.is_empty() { "" } else { " " };
    Ok(format!(
        "\nmenuentry \"Dukovo disk check: erases the disk with serial {serial}\" {{\n    \
         multiboot2 /boot/radix {cmdline}{space}selftest=bench init-after-selftest -- \
         --no-volumes --posix {DISK_CHECK_PROGRAM} --yes-erase {serial} --bench \
         {BENCH_PROGRAM}\n{}    boot\n}}\n",
        module_lines(modules)
    ))
}

/// Returns the first `@NAME@` left in `text`.
fn find_placeholder(text: &str) -> Option<&str> {
    let mut rest = text;
    while let Some(start) = rest.find('@') {
        let after = &rest[start + 1..];
        if let Some(len) = after.find('@') {
            let name = &after[..len];
            if !name.is_empty() && name.chars().all(|c| c.is_ascii_uppercase() || c == '_') {
                return Some(&rest[start..start + len + 2]);
            }
        }
        rest = after;
    }
    None
}

/// Builds an image from `spec`: a disk image (a USB stick's), or an ISO
/// for optical media when [`ImageSpec::iso`] says so, each booting with
/// the GRUB `./ob` builds ([`grub::build`]) under a PC's BIOS and UEFI.
pub fn build(ctx: &Ctx, spec: &ImageSpec<'_>) -> Result<()> {
    if spec.iso && process::find_program("xorriso").is_none() {
        return Err(Error::new("xorriso is needed to write the ISO but is not installed")
            .hint("run `./ob doctor` to see what is missing"));
    }
    let grub = grub::build(ctx)?;

    step(&format!("Assembling the boot image {}", ctx.display(&spec.output)));
    for dir in [&spec.staging, &grub_work(spec)] {
        if dir.exists() {
            std::fs::remove_dir_all(dir).context(|| format!("removing {}", dir.display()))?;
        }
    }
    let installed = matches!(spec.menu, Menu::Installed(_));
    if installed && (spec.iso || spec.disk_check.is_some()) {
        crate::bail!(
            "an installed system's disk is neither an ISO nor a disk check's image: those boot \
             the system from memory"
        );
    }
    // An installed system's kernel is in its slot, with its startup archive.
    if !installed {
        kernel::copy_for_image(ctx, spec.kernel, &spec.staging.join("boot/radix"))?;
    }
    let manifest = licenses::install(
        ctx,
        &spec.staging,
        &licenses::Extras {
            grub: &grub,
            microcode: spec.microcode.is_some(),
            programs: spec.programs.map(|programs| &programs.ports),
        },
    )?;
    let mut modules = Vec::new();
    if let Some(programs) = spec.programs.filter(|_| !installed) {
        // The kernel finds each module by the word on its command line.
        for (from, path, word) in [
            (&programs.usertests, userspace::USERTESTS_PATH, userspace::USERTESTS_WORD),
            (&programs.bootfs, userspace::BOOTFS_PATH, userspace::BOOTFS_WORD),
        ] {
            process::copy(from, &spec.staging.join(path.trim_start_matches('/')))?;
            modules.push((path, word));
        }
    }
    if let Some(microcode) = &spec.microcode {
        let path = spec.staging.join(MICROCODE_PATH.trim_start_matches('/'));
        process::create_dir(path.parent().expect("the microcode path has a parent"))?;
        std::fs::write(&path, microcode).context(|| format!("writing {}", path.display()))?;
        // The kernel recognises the module by this word on its command line.
        modules.push((MICROCODE_PATH, "microcode"));
    }
    let read = |name: &str| {
        let path = ctx.root.join(name);
        std::fs::read_to_string(&path).context(|| format!("reading {}", path.display()))
    };
    let template = match spec.menu {
        Menu::Installed(uuid) => installed_template(ctx, uuid)?,
        Menu::Live(start) => live_template(&read(LIVE_CFG)?, start),
        Menu::Tests => read(TESTS_CFG)?,
    };
    let entries = match spec.disk_check {
        Some(serial) => disk_check(ctx, spec, serial, &modules)?,
        None => String::new(),
    };
    let grub_cfg = render_grub_cfg(&template, spec.timeout, spec.cmdline, &modules, &entries)?;
    let cfg_path = spec.staging.join("boot/grub/grub.cfg");
    process::create_dir(cfg_path.parent().expect("grub.cfg has a parent"))?;
    std::fs::write(&cfg_path, grub_cfg).context(|| format!("writing {}", cfg_path.display()))?;
    if let Some(parent) = spec.output.parent() {
        process::create_dir(parent)?;
    }

    let time = image_time(ctx);
    process::create_dir(&grub_work(spec))?;
    if spec.iso {
        make_iso(ctx, spec, &grub, time)?;
    } else {
        make_stick(ctx, spec, &grub, time)?;
    }
    let _ = std::fs::remove_dir_all(grub_work(spec));
    if let Some(root) = spec.root {
        step("Adding the Dukovo root partition");
        append_root(&spec.output, root)?;
    }
    if let Some(live) = spec.live {
        step("Adding the DUKOVO_LIVE partition, with the signed release");
        append_live(&spec.output, live)?;
    }
    // A copy of the manifest next to the image, for release pages and CI.
    let sidecar = spec.output.with_extension("spdx");
    std::fs::write(&sidecar, manifest).context(|| format!("writing {}", sidecar.display()))?;
    Ok(())
}

/// Where the GRUB images an image is made with are made: beside its
/// staging directory.
fn grub_work(spec: &ImageSpec<'_>) -> PathBuf {
    spec.staging.with_extension("grub")
}

/// When an image is made, in seconds since 1970: [`source_date_epoch`],
/// or now.
pub fn image_time(ctx: &Ctx) -> u64 {
    source_date_epoch(ctx).and_then(|epoch| epoch.parse().ok()).unwrap_or_else(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs())
    })
}

/// The size of a disk image's BIOS boot partition, which GRUB's
/// `core.img` is in for a PC's BIOS, in sectors: a MiB.
const BIOS_BOOT_SECTORS: u64 = 2048;

/// The label of an image's EFI system partition.
const ESP_LABEL: &str = "DUKOVO";

/// Makes the disk image `spec.output` from the staging directory (ADR
/// 0306): a GPT with GRUB's `core.img` in its first partition, a BIOS
/// boot partition, and an EFI system partition second, FAT32, holding
/// the staging tree and UEFI's GRUB as `EFI/BOOT/BOOTX64.EFI`, with
/// GRUB's `boot.img` in the disk's first sector for a PC's BIOS, which
/// rgrub-setup(8) puts there on Dukovo. A protective MBR covers the
/// disk, as the UEFI specification asks, so the image boots from a USB
/// stick or a disk under either firmware.
fn make_stick(ctx: &Ctx, spec: &ImageSpec<'_>, grub: &Grub, time: u64) -> Result<()> {
    let images = grub.disk_images(ctx, &grub_work(spec))?;
    process::copy(&images.efi, &spec.staging.join("EFI/BOOT/BOOTX64.EFI"))?;
    let read = |path: &Path| std::fs::read(path).context(|| format!("reading {}", path.display()));
    let (boot, core) = (read(&images.boot)?, read(&images.core)?);

    // Room for what the staging tree holds, a little more, and a
    // filesystem of at least 64 MiB, which FAT32's clusters need.
    let content = tree_bytes(&spec.staging)?;
    let esp_bytes = (content + content / 8 + (16 << 20)).max(64 << 20).next_multiple_of(1 << 20);
    let esp_sectors = esp_bytes / SECTOR as u64;
    // The table and a MiB's gap before the partitions, and a MiB after
    // them, which the backup table is at the end of.
    let sectors = 2048 + BIOS_BOOT_SECTORS + esp_sectors + 2048;
    let shown = spec.output.display().to_string();
    let disk = new_disk(&spec.output, sectors)?;
    let mut rng = disk::Rng::system();
    let failed = |err: disk::table::Error| Error::new(format!("{shown}: {}", err.0));
    let mut table =
        disk::Table::new(disk::Scheme::Gpt, SECTOR as u32, sectors, &mut rng).map_err(failed)?;
    let kind = |name| disk::types::parse(name, disk::Scheme::Gpt).expect("a known type");
    for (name, kind, end) in [
        ("BIOS boot", kind("bios-boot"), disk::End::Sectors(BIOS_BOOT_SECTORS)),
        ("EFI system", kind("efi"), disk::End::Sectors(esp_sectors)),
    ] {
        let new = disk::New { name: name.into(), ..disk::New::of(kind, end) };
        table.add(new, &mut rng).map_err(failed)?;
    }
    let writes = disk::plan(&disk, &table).context(|| format!("partitioning {shown}"))?;
    disk::apply(&disk, &writes).context(|| format!("partitioning {shown}"))?;
    let part = table.get(2).expect("the EFI system partition was added");
    let esp = disk.part(part.first, part.sectors()).context(|| format!("opening {shown}"))?;
    let options = disk::fat::Options {
        bits: Some(32),
        label: Some(ESP_LABEL.into()),
        hidden: Some(part.first),
        now: Some(time),
        ..disk::fat::Options::default()
    };
    disk::fat::format(&esp, &options, &mut rng)
        .map_err(|why| Error::new(format!("{shown}: making its EFI system partition: {why}")))?;
    fill_fat(esp, &spec.staging, time, &shown)?;
    disk::grub::setup_bios(&disk, &boot, &core)
        .map_err(|why| Error::new(format!("{shown}: putting GRUB on it for a BIOS: {why}")))?;
    Ok(())
}

/// A new file of `sectors` at `path`, as a disk.
fn new_disk(path: &Path, sectors: u64) -> Result<disk::Device> {
    let _ = std::fs::remove_file(path);
    let bytes = sectors * SECTOR as u64;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(path)
        .and_then(|file| file.set_len(bytes).map(|()| file))
        .context(|| format!("making {}", path.display()))?;
    Ok(disk::Device::whole(file, bytes, SECTOR as u32))
}

/// Writes the tree `from` into the FAT on `device` with the kernel's FAT
/// crate, as Radix writes one.
fn fill_fat(device: disk::Device, from: &Path, time: u64, shown: &str) -> Result<()> {
    rootfs::host_clock(time);
    let fs = radix_fat::FatFs::mount(BlockFile::new(device, false).into_ref(), false)
        .map_err(|err| rootfs::failed(shown, err))?;
    let root = radix_vfs::FileSystem::root(&*fs);
    rootfs::copy(from, &root)?;
    radix_vfs::FileSystem::sync(&*fs).map_err(|err| rootfs::failed(shown, err))
}

/// The bytes the files under `dir` take, in 4 KiB blocks, and a block
/// for each directory.
fn tree_bytes(dir: &Path) -> Result<u64> {
    let mut total = 4096;
    for entry in std::fs::read_dir(dir).context(|| format!("reading {}", dir.display()))? {
        let entry = entry.context(|| format!("reading {}", dir.display()))?;
        let meta = entry.metadata().context(|| format!("reading {}", entry.path().display()))?;
        total += if meta.is_dir() {
            tree_bytes(&entry.path())?
        } else {
            meta.len().next_multiple_of(4096)
        };
    }
    Ok(total)
}

/// Makes the ISO `spec.output` of the staging directory with xorriso
/// (ADR 0306), for optical media, which El Torito boots: GRUB's
/// `cdboot.img` and a `core.img` for a CD under a PC's BIOS, and under
/// UEFI a FAT image holding UEFI's GRUB, which finds the disc by the
/// UUID GRUB reads from its time of making.
fn make_iso(ctx: &Ctx, spec: &ImageSpec<'_>, grub: &Grub, time: u64) -> Result<()> {
    let work = grub_work(spec);
    let (date, uuid) = iso_date(time);
    // grub-mkimage puts `cdboot.img` before `core.img` for a CD.
    let eltorito = spec.staging.join(ELTORITO);
    process::create_dir(eltorito.parent().expect("the El Torito image has a parent"))?;
    grub.mkimage(ctx, "i386-pc-eltorito", "/boot/grub", None, &eltorito)?;

    let early = work.join("early.cfg");
    let config =
        format!("search --no-floppy --fs-uuid --set=root {uuid}\nset prefix=($root)/boot/grub\n");
    std::fs::write(&early, config).context(|| format!("writing {}", early.display()))?;
    let tree = work.join("efi");
    let app = tree.join("EFI/BOOT/BOOTX64.EFI");
    process::create_dir(app.parent().expect("BOOTX64.EFI has a parent"))?;
    grub.mkimage(ctx, "x86_64-efi", "/boot/grub", Some(&early), &app)?;
    let size = std::fs::metadata(&app).context(|| format!("reading {}", app.display()))?.len();
    let efi_img = spec.staging.join(EFI_IMAGE);
    let sectors = (size + (1 << 20)).next_multiple_of(1 << 20) / SECTOR as u64;
    let device = new_disk(&efi_img, sectors)?;
    let shown = efi_img.display().to_string();
    let options = disk::fat::Options {
        label: Some(ESP_LABEL.into()),
        now: Some(time),
        ..disk::fat::Options::default()
    };
    disk::fat::format(&device, &options, &mut disk::Rng::system())
        .map_err(|why| Error::new(format!("{shown}: {why}")))?;
    fill_fat(device, &tree, time, &shown)?;

    let mut cmd = Command::new("xorriso");
    cmd.args(["-as", "mkisofs", "-quiet", "-R", "-V", &volume_id()])
        .arg(format!("--modification-date={date}"))
        .args(["-b", ELTORITO, "-no-emul-boot", "-boot-load-size", "4", "-boot-info-table"])
        .args(["-eltorito-alt-boot", "-e", EFI_IMAGE, "-no-emul-boot"])
        .arg("-o")
        .arg(&spec.output)
        .arg(&spec.staging)
        .env("SOURCE_DATE_EPOCH", time.to_string());
    let output = process::output(ctx, &mut cmd)?;
    if ctx.verbose {
        eprint!("{}", String::from_utf8_lossy(&output.stderr));
    }
    Ok(())
}

/// Where an ISO's El Torito image for a PC's BIOS is.
const ELTORITO: &str = "boot/grub/i386-pc/eltorito.img";

/// Where an ISO's El Torito image for UEFI is, a FAT holding GRUB.
const EFI_IMAGE: &str = "boot/grub/efi.img";

/// The ISO's time of making for xorriso's `--modification-date`
/// (`YYYYMMDDhhmmss00`), and the UUID GRUB reads from it
/// (`YYYY-MM-DD-hh-mm-ss-00`), for `time` seconds since 1970 (UTC).
fn iso_date(time: u64) -> (String, String) {
    let days = time / 86_400;
    let secs = time % 86_400;
    let (year, month, day) = civil(days);
    let (h, m, s) = (secs / 3600, secs / 60 % 60, secs % 60);
    (
        format!("{year:04}{month:02}{day:02}{h:02}{m:02}{s:02}00"),
        format!("{year:04}-{month:02}-{day:02}-{h:02}-{m:02}-{s:02}-00"),
    )
}

/// The year, month and day of `days` since 1970-01-01, by Howard
/// Hinnant's `civil_from_days`.
fn civil(days: u64) -> (u64, u64, u64) {
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + u64::from(month <= 2);
    (year, month, day)
}

/// The size of the Dukovo root partition of an image (ADR 0137).
pub const ROOT_BYTES: u64 = 512 << 20;

/// The Dukovo root partition's GPT type, 90C9A303-3954-4174-8F8B-EE75A92A9EB0
/// (minted by ADR 0137), in its mixed-endian byte order.
pub const ROOT_TYPE: [u8; 16] = [
    0x03, 0xa3, 0xc9, 0x90, 0x54, 0x39, 0x74, 0x41, 0x8f, 0x8b, 0xee, 0x75, 0xa9, 0x2a, 0x9e, 0xb0,
];

/// The Dukovo root partition's GPT name.
const ROOT_NAME: &str = "Dukovo root";

/// The install stick's DUKOVO_LIVE partition's GPT type, "Dukovo live",
/// 5894C662-AF90-4B47-BAAF-2E3396443B25 (minted by ADR 0308), in its
/// mixed-endian byte order. Spark mounts a partition of this type
/// read-only and never takes a root from it.
pub const LIVE_TYPE: [u8; 16] = [
    0x62, 0xc6, 0x94, 0x58, 0x90, 0xaf, 0x47, 0x4b, 0xba, 0xaf, 0x2e, 0x33, 0x96, 0x44, 0x3b, 0x25,
];

/// The DUKOVO_LIVE partition's GPT name.
const LIVE_NAME: &str = "Dukovo live";

/// The DUKOVO_LIVE partition's Btrfs label.
pub const LIVE_LABEL: &str = "DUKOVO_LIVE";

/// The Dukovo root's Btrfs label.
pub const ROOT_LABEL: &str = "dukovo-root";

/// The subvolumes of the Dukovo root, the first read-only: those Spark
/// mounts (spark(8)), and `@snapshots`.
const SUBVOLUMES: [&str; 7] =
    ["@system.a", "@settings", "@state", "@applications", "@users", "@shared", "@snapshots"];

/// The folder of the system, in its slot, that holds what GRUB starts an
/// installed system with: the kernel, [`SLOT_KERNEL`], and the startup
/// archive, [`SLOT_ARCHIVE`] (ADR 0307). It is not under `/system/boot`,
/// where the EFI system partition is mounted.
pub const SLOT_STARTUP: &str = "startup";

/// The kernel's name in [`SLOT_STARTUP`].
pub const SLOT_KERNEL: &str = "radix";

/// The startup archive's name in [`SLOT_STARTUP`].
pub const SLOT_ARCHIVE: &str = "archive";

/// Makes the Dukovo root's Btrfs in the file `output`, of [`ROOT_BYTES`],
/// with the kernel's own Btrfs crate ([`rootfs::make`], ADR 0304):
/// `@system.a` holds the system's slot as [`stage_system`] stages it in
/// `staging` first; the other [`SUBVOLUMES`] are empty but for the files
/// of `account`, if there is one. Data is compressed with zstd, and every
/// file is root's. Returns the Btrfs's UUID.
pub fn make_root(
    ctx: &Ctx,
    kernel: &Path,
    programs: &userspace::Programs,
    staging: &Path,
    output: &Path,
    account: Option<&Account>,
) -> Result<[u8; 16]> {
    step(&format!("Making the Dukovo root's Btrfs {}", ctx.display(output)));
    fresh_dir(staging)?;
    for name in SUBVOLUMES {
        process::create_dir(&staging.join(name))?;
        set_mode(&staging.join(name), 0o755)?;
    }
    let system = staging.join(SUBVOLUMES[0]);
    stage_system(ctx, kernel, programs, &system)?;
    if let Some(account) = account {
        let subvolume = |name: &str| staging.join(name);
        account::add(&system, &subvolume(SUBVOLUMES[1]), &subvolume(SUBVOLUMES[2]), account)?;
    }
    let time = image_time(ctx);
    let made = rootfs::make(staging, output, ROOT_BYTES, ROOT_LABEL, &SUBVOLUMES, time);
    let _ = std::fs::remove_dir_all(staging);
    made
}

/// Makes the install stick's DUKOVO_LIVE filesystem in the file `output`
/// (ADR 0308): a Btrfs labelled [`LIVE_LABEL`] whose read-only subvolume
/// [`release::SUBVOLUME`] holds the signed release of the system's slot,
/// staged in `staging` first, as the update server will serve it. The
/// installer installs from it, after checking it against the root the
/// live system trusts.
pub fn make_live(
    ctx: &Ctx,
    kernel: &Path,
    programs: &userspace::Programs,
    staging: &Path,
    output: &Path,
) -> Result<()> {
    fresh_dir(staging)?;
    let system = staging.join("system");
    stage_system(ctx, kernel, programs, &system)?;
    let repository = staging.join("filesystem").join(release::SUBVOLUME);
    release::make(ctx, &system, &repository)?;
    step(&format!("Making the DUKOVO_LIVE Btrfs {}", ctx.display(output)));
    // Room for the release a quarter over, before zstd shrinks it, and
    // what Btrfs keeps for itself.
    let bytes = (tree_bytes(&repository)? * 5 / 4 + (64 << 20)).next_multiple_of(1 << 20);
    let made = rootfs::make(
        &staging.join("filesystem"),
        output,
        bytes,
        LIVE_LABEL,
        &[release::SUBVOLUME],
        image_time(ctx),
    );
    let _ = std::fs::remove_dir_all(staging);
    made.map(|_| ())
}

/// Stages the system's slot in `system`, a new directory: what the boot
/// archive of `programs` has in its `system` directory, with each file's
/// mode and time, and in [`SLOT_STARTUP`] the kernel at `kernel` and the
/// startup archive, which an installed system starts from (ADR 0307).
/// The same tree is a root's `@system.a` and the stick's release.
pub fn stage_system(
    ctx: &Ctx,
    kernel: &Path,
    programs: &userspace::Programs,
    system: &Path,
) -> Result<()> {
    let bootfs = &programs.bootfs;
    let bytes = std::fs::read(bootfs).context(|| format!("reading {}", bootfs.display()))?;
    let archive = radix_bootfs::open(&bytes)
        .map_err(|err| Error::new(format!("reading {}: {err:?}", bootfs.display())))?;
    fresh_dir(system)?;
    set_mode(system, 0o755)?;
    for entry in archive.entries() {
        let mut names = entry.path().components();
        if names.next() != Some("system") {
            continue;
        }
        let path = names.fold(system.to_path_buf(), |path, name| path.join(name));
        if entry.kind == radix_bootfs::Kind::Directory {
            process::create_dir(&path)?;
            set_mode(&path, entry.mode)?;
            continue;
        }
        process::create_dir(path.parent().expect("an entry is inside the system"))?;
        if entry.kind == radix_bootfs::Kind::Symlink {
            let target = std::str::from_utf8(entry.data)
                .map_err(|_| Error::new(format!("the link {} is not UTF-8", path.display())))?;
            std::os::unix::fs::symlink(target, &path)
                .context(|| format!("linking {}", path.display()))?;
            continue;
        }
        let mut file =
            std::fs::File::create(&path).context(|| format!("writing {}", path.display()))?;
        file.write_all(entry.data)
            .and_then(|()| {
                use std::os::unix::fs::PermissionsExt as _;
                file.set_permissions(std::fs::Permissions::from_mode(entry.mode & 0o7777))
            })
            .and_then(|()| {
                file.set_modified(
                    std::time::UNIX_EPOCH + std::time::Duration::from_secs(entry.mtime),
                )
            })
            .context(|| format!("writing {}", path.display()))?;
    }
    add_startup(ctx, kernel, &programs.startup, &system.join(SLOT_STARTUP), image_time(ctx))
}

/// Removes `dir` if it is there, and makes it again, empty.
fn fresh_dir(dir: &Path) -> Result<()> {
    if dir.exists() {
        std::fs::remove_dir_all(dir).context(|| format!("removing {}", dir.display()))?;
    }
    process::create_dir(dir)
}

/// Gives `path` the mode `mode`, whatever the umask ob runs with: a
/// user's is often 002.
fn set_mode(path: &Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode & 0o7777))
        .context(|| format!("setting the mode of {}", path.display()))
}

/// Puts the kernel at `kernel`, without its debug information, and the
/// startup archive at `archive` into the slot's folder `dir`, with mode
/// 0644 and dated `time` (seconds since 1970), as the boot archive's files
/// are, so the same build makes the same slot.
fn add_startup(ctx: &Ctx, kernel: &Path, archive: &Path, dir: &Path, time: u64) -> Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    process::create_dir(dir)?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o755))
        .context(|| format!("setting the mode of {}", dir.display()))?;
    let (to_kernel, to_archive) = (dir.join(SLOT_KERNEL), dir.join(SLOT_ARCHIVE));
    kernel::copy_for_image(ctx, kernel, &to_kernel)?;
    process::copy(archive, &to_archive)?;
    let date = std::time::UNIX_EPOCH + std::time::Duration::from_secs(time);
    for path in [&to_kernel, &to_archive] {
        std::fs::File::options()
            .write(true)
            .open(path)
            .and_then(|file| {
                file.set_permissions(std::fs::Permissions::from_mode(0o644))?;
                file.set_modified(date)
            })
            .context(|| format!("dating {}", path.display()))?;
    }
    Ok(())
}

/// Puts the Dukovo root partition, holding the filesystem in the file
/// `root`, after the partitions of the GPT disk image at `image` (ADR
/// 0137), as [`append_partition`] does. Returns its first sector.
pub fn append_root(image: &Path, root: &Path) -> Result<u64> {
    append_partition(image, root, ROOT_TYPE, ROOT_NAME)
}

/// Puts the install stick's DUKOVO_LIVE partition, holding the filesystem
/// in the file `live`, after the partitions of the GPT disk image at
/// `image` (ADR 0308), as [`append_partition`] does. Returns its first
/// sector.
pub fn append_live(image: &Path, live: &Path) -> Result<u64> {
    append_partition(image, live, LIVE_TYPE, LIVE_NAME)
}

/// Puts a partition of the type `kind` named `name`, holding the
/// filesystem in the file `filesystem`, after the partitions of the GPT
/// disk image at `image`, from the next whole MiB, in the table's first
/// empty entry. The backup table moves to the new end, both headers are
/// written again, and the protective MBR covers the disk. The
/// filesystem's zeros are left as holes. Returns the partition's first
/// sector.
fn append_partition(image: &Path, filesystem: &Path, kind: [u8; 16], name: &str) -> Result<u64> {
    let failed = |what: &str| {
        let what = what.to_string();
        move |err: std::io::Error| Error::new(format!("{what} {}: {err}", image.display()))
    };
    let mut disk = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(image)
        .map_err(failed("opening"))?;
    let mut head = vec![0u8; 2 * SECTOR];
    disk.read_exact(&mut head).map_err(failed("reading"))?;
    let word = |at: usize| u32::from_le_bytes(head[at..at + 4].try_into().expect("4 bytes"));
    let long = |at: usize| u64::from_le_bytes(head[at..at + 8].try_into().expect("8 bytes"));
    if &head[SECTOR..SECTOR + 8] != b"EFI PART"
        || word(SECTOR + 12) != 92
        || word(SECTOR + 84) != 128
    {
        crate::bail!("{} has no GPT of 128-byte entries", image.display());
    }
    let (old_backup, entries_at) = (long(SECTOR + 32), long(SECTOR + 72));
    let count = word(SECTOR + 80) as usize;
    let entry_sectors = (count * 128).div_ceil(SECTOR) as u64;
    let mut entries = vec![0u8; entry_sectors as usize * SECTOR];
    disk.seek(SeekFrom::Start(entries_at * SECTOR as u64))
        .and_then(|_| disk.read_exact(&mut entries))
        .map_err(failed("reading the GPT entries of"))?;
    let entry = |i: usize| &entries[i * 128..(i + 1) * 128];
    let free = (0..count)
        .find(|&i| entry(i)[..16] == [0; 16])
        .ok_or_else(|| Error::new(format!("the GPT of {} has no empty entry", image.display())))?;
    let used = (0..count)
        .filter(|&i| entry(i)[..16] != [0; 16])
        .map(|i| u64::from_le_bytes(entry(i)[40..48].try_into().expect("8 bytes")))
        .max()
        .unwrap_or(0)
        .max(old_backup);
    let size = std::fs::metadata(filesystem).map_err(failed("reading the filesystem for"))?.len();
    let first = (used + 1).div_ceil(2048) * 2048;
    let last = first + size / SECTOR as u64 - 1;
    let (backup_entries, backup) = (last + 1, last + 1 + entry_sectors);

    let mut unique = [0u8; 16];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut random| random.read_exact(&mut unique))
        .map_err(failed("making a GUID for"))?;
    // A random GUID: version 4, variant 1.
    unique[7] = unique[7] & 0x0f | 0x40;
    unique[8] = unique[8] & 0x3f | 0x80;
    let new = &mut entries[free * 128..(free + 1) * 128];
    new.fill(0);
    new[..16].copy_from_slice(&kind);
    new[16..32].copy_from_slice(&unique);
    new[32..40].copy_from_slice(&first.to_le_bytes());
    new[40..48].copy_from_slice(&last.to_le_bytes());
    for (i, c) in name.encode_utf16().enumerate() {
        new[56 + 2 * i..58 + 2 * i].copy_from_slice(&c.to_le_bytes());
    }
    let table = crc32(&entries[..count * 128]);
    let header = |at: u64, alternate: u64, entries_at: u64| {
        let mut h = head[SECTOR..2 * SECTOR].to_vec();
        h[16..20].fill(0);
        h[24..32].copy_from_slice(&at.to_le_bytes());
        h[32..40].copy_from_slice(&alternate.to_le_bytes());
        h[48..56].copy_from_slice(&last.to_le_bytes());
        h[72..80].copy_from_slice(&entries_at.to_le_bytes());
        h[88..92].copy_from_slice(&table.to_le_bytes());
        let crc = crc32(&h[..92]);
        h[16..20].copy_from_slice(&crc.to_le_bytes());
        h
    };
    // The protective MBR's one partition, from sector 1 to the end, with
    // the CHS of an end past what CHS can say.
    let mut mbr = head[..SECTOR].to_vec();
    if mbr[446 + 4] != 0xee {
        crate::bail!("{} has no protective MBR", image.display());
    }
    mbr[446 + 5..446 + 8].fill(0xff);
    let sectors = u32::try_from(backup).unwrap_or(u32::MAX);
    mbr[446 + 12..446 + 16].copy_from_slice(&sectors.to_le_bytes());

    let mut from = std::fs::File::open(filesystem).map_err(failed("opening the filesystem for"))?;
    copy_sparse(&mut from, 0, &mut disk, first * SECTOR as u64, size)
        .map_err(failed("writing the filesystem to"))?;
    let at = |sector: u64| SeekFrom::Start(sector * SECTOR as u64);
    let zeros = vec![0u8; (entry_sectors as usize + 1) * SECTOR];
    (|| {
        // The old backup table, in the gap before the partition.
        disk.seek(at(old_backup - entry_sectors))?;
        disk.write_all(&zeros)?;
        disk.seek(at(backup_entries))?;
        disk.write_all(&entries)?;
        disk.write_all(&header(backup, 1, backup_entries))?;
        disk.seek(at(0))?;
        disk.write_all(&mbr)?;
        disk.write_all(&header(1, backup, entries_at))?;
        disk.seek(at(entries_at))?;
        disk.write_all(&entries)?;
        disk.set_len((backup + 1) * SECTOR as u64)?;
        disk.sync_all()
    })()
    .map_err(failed("writing the partition table of"))?;
    Ok(first)
}

/// Copies `len` bytes from `from` at `from_at` to `to` at `to_at`,
/// skipping the pieces that are all zeros, which a file just made or
/// extended reads as anyway.
pub fn copy_sparse(
    from: &mut std::fs::File,
    from_at: u64,
    to: &mut std::fs::File,
    to_at: u64,
    len: u64,
) -> std::io::Result<()> {
    let mut piece = vec![0u8; 1 << 20];
    let mut done = 0;
    while done < len {
        let n = piece.len().min(usize::try_from(len - done).unwrap_or(usize::MAX));
        from.seek(SeekFrom::Start(from_at + done))?;
        from.read_exact(&mut piece[..n])?;
        if piece[..n].iter().any(|&b| b != 0) {
            to.seek(SeekFrom::Start(to_at + done))?;
            to.write_all(&piece[..n])?;
        }
        done += n as u64;
    }
    Ok(())
}

/// The CRC-32 of `bytes` that GPT uses (IEEE 802.3, reflected).
pub fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xedb8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

/// The size of a sector, which an image's GPT counts in.
const SECTOR: usize = 512;

/// ISO 9660 volume label, such as `DUKOVO_0_0_1`.
pub fn volume_id() -> String {
    format!("DUKOVO_{}", VERSION.replace(['.', '-'], "_")).to_ascii_uppercase()
}

/// Commit time of HEAD, which an image's files and filesystems have
/// instead of the clock. An existing `SOURCE_DATE_EPOCH` wins. Images
/// are not yet bit-for-bit reproducible: their GPT and filesystems have
/// random UUIDs.
pub fn source_date_epoch(ctx: &Ctx) -> Option<String> {
    if let Ok(epoch) = std::env::var("SOURCE_DATE_EPOCH") {
        return Some(epoch);
    }
    let output = Command::new("git")
        .args(["log", "-1", "--format=%ct"])
        .current_dir(&ctx.root)
        .output()
        .ok()?;
    let epoch = String::from_utf8(output.stdout).ok()?.trim().to_string();
    (output.status.success() && !epoch.is_empty()).then_some(epoch)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEMPLATE: &str = include_str!("../../../image/grub/grub.cfg.in");

    #[test]
    fn renders_the_repository_template() {
        let cfg = render_grub_cfg(TEMPLATE, 0, "qemu-exit", &[], "").unwrap();
        assert!(cfg.contains("set timeout=0"));
        assert!(cfg.contains("multiboot2 /boot/radix qemu-exit\n    boot\n"));
        assert!(cfg.contains(&format!("Dukovo Server {VERSION}: startup trouble")));
        assert!(cfg.contains("multiboot2 /boot/radix startup-trouble qemu-exit\n    boot\n"));
        assert!(cfg.contains(&format!("Dukovo Server {VERSION} (Radix)")));
        assert!(cfg.contains("set gfxpayload=keep"));
        assert!(!cfg.contains("@VERSION@"));
        assert!(!cfg.contains("module2"));
    }

    #[test]
    fn renders_modules() {
        let cfg = render_grub_cfg(TEMPLATE, 0, "", &[(MICROCODE_PATH, "microcode")], "").unwrap();
        assert!(cfg.contains(&format!("    module2 {MICROCODE_PATH} microcode\n    boot\n")));
    }

    #[test]
    fn rejects_unknown_placeholders_and_bad_cmdlines() {
        assert!(render_grub_cfg("set x=@NOPE@", 0, "", &[], "").is_err());
        assert!(render_grub_cfg(TEMPLATE, 0, "a; reboot", &[], "").is_err());
    }

    #[test]
    fn writes_uuids_as_grub_and_the_command_line_do() {
        let uuid = [
            0x90, 0xc9, 0xa3, 0x03, 0x39, 0x54, 0x41, 0x74, 0x8f, 0x8b, 0xee, 0x75, 0xa9, 0x2a,
            0x9e, 0xb0,
        ];
        assert_eq!(uuid_text(uuid), "90c9a303-3954-4174-8f8b-ee75a92a9eb0");
    }

    #[test]
    fn the_installed_menu_starts_the_slot_named_by_its_root() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../", "image/grub/installed.cfg.in");
        let template = std::fs::read_to_string(path).unwrap();
        let uuid = "90c9a303-3954-4174-8f8b-ee75a92a9eb0";
        let filled = template.replace("@ROOT_UUID@", uuid);
        let modules = [(MICROCODE_PATH, "microcode")];
        let cfg = render_grub_cfg(&filled, 0, "quiet", &modules, "").unwrap();
        assert!(cfg.contains(&format!("search --no-floppy --fs-uuid --set=system {uuid}\n")));
        let kernel = format!(
            "    multiboot2 ($system)/@system.a/{SLOT_STARTUP}/{SLOT_KERNEL} root=UUID={uuid} \
             panic=10 quiet\n"
        );
        let archive = format!(
            "    module2 ($system)/@system.a/{SLOT_STARTUP}/{SLOT_ARCHIVE} {}\n",
            userspace::BOOTFS_WORD
        );
        let microcode = format!("    module2 {MICROCODE_PATH} microcode\n    boot\n");
        assert!(cfg.contains(&format!("{kernel}{archive}{microcode}")), "{cfg}");
        // The startup-trouble entry names the same root, with its flag first.
        assert_eq!(cfg.matches(&format!("root=UUID={uuid} panic=10 quiet")).count(), 2);
        assert!(cfg.contains("radix startup-trouble root=UUID="));
    }

    #[test]
    fn the_installers_menu_leaves_the_root_to_the_installer() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../", "image/grub/installed.cfg.in");
        let template = std::fs::read_to_string(path).unwrap();
        let menu = installer_menu_text(&template, 5, "quiet").unwrap();
        assert!(menu.contains("search --no-floppy --fs-uuid --set=system @ROOT_UUID@\n"), "{menu}");
        assert!(menu.contains("root=UUID=@ROOT_UUID@ panic=10 quiet\n"), "{menu}");
        assert!(!menu.contains("@VERSION@") && !menu.contains("@CMDLINE@"));
        assert!(!menu.contains('\u{1}'));
    }

    #[test]
    fn the_live_menu_starts_the_live_system_or_the_first_disk() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../", "image/grub/live.cfg.in");
        let template = std::fs::read_to_string(path).unwrap();
        let modules = [("/boot/bootfs", userspace::BOOTFS_WORD)];
        let live = live_template(&template, LiveStart::Install);
        let cfg = render_grub_cfg(&live, 5, "quiet", &modules, "").unwrap();
        // A release starts the installer unless another entry is chosen.
        assert!(cfg.contains("set default=install\n"), "{cfg}");
        assert!(cfg.contains(&format!("menuentry \"Install Dukovo {VERSION}\" --id install {{")));
        let install = format!(
            "    multiboot2 /boot/radix live install console-sign-in=off quiet\n    module2 /boot/bootfs {}\n    boot\n",
            userspace::BOOTFS_WORD
        );
        assert!(cfg.contains(&install), "{cfg}");
        assert!(
            cfg.contains(&format!("Try Dukovo {VERSION} (live console)\" --id try {{")),
            "{cfg}"
        );
        // Spark's flag comes first, so that a --cmdline of the user's
        // cannot push it past a `--`.
        let entry = format!(
            "    multiboot2 /boot/radix live console-sign-in=off quiet\n    module2 /boot/bootfs {}\n    boot\n",
            userspace::BOOTFS_WORD
        );
        assert!(cfg.contains(&entry), "{cfg}");
        // Boot from first disk works under both firmwares.
        assert!(cfg.contains("Boot from first disk"));
        assert!(cfg.contains("chainloader \"${part}/EFI/BOOT/BOOTX64.EFI\""));
        assert!(cfg.contains("drivemap -s (hd0) (hd1)"));
        assert!(cfg.contains("chainloader +1"));
        // The modules those need are built into GRUB.
        assert!(crate::grub::MODULES.contains(&"chain"));
        assert!(crate::grub::PC_MODULES.contains(&"drivemap"));
        // The boot tests' live image tries the system instead.
        let tests = live_template(&template, LiveStart::Try);
        let cfg = render_grub_cfg(&tests, 0, "quiet", &modules, "").unwrap();
        assert!(cfg.contains("set default=try\n"), "{cfg}");
    }

    #[test]
    fn names_follow_the_version() {
        assert_eq!(file_name(Kind::Stick), format!("dukovo-{VERSION}-x86_64.img"));
        assert_eq!(file_name(Kind::Iso), format!("dukovo-{VERSION}-x86_64.iso"));
        assert_eq!(file_name(Kind::Installed), format!("dukovo-{VERSION}-x86_64-installed.img"));
        let label = volume_id();
        assert!(label.len() <= 32);
        assert!(label.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_'));
    }
}

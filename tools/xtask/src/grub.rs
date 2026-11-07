// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! GRUB, built from its pinned source for the images (ADR 0306), for a
//! PC's BIOS and for UEFI, with only the modules [`MODULES`] names in the
//! images `grub-mkimage` makes of it: not the build machine's GRUB, which
//! grub-mkrescue copied before, with every module its distribution built.
//!
//! The release's tarball is fetched once, checked against its SHA-256,
//! and built with the build machine's C compiler into
//! `.cache/grub/<version>`, which holds what `make install` puts there
//! for each platform; [`RECIPE`] names what the build depends on, so a
//! change to it builds GRUB again.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::process::{self, step};
use crate::{Context, Ctx, Error, Result};

/// The release built.
pub const VERSION: &str = "2.12";

/// Its tarball's SHA-256, as the GNU project published it.
pub const SHA256: &str = "f3c97391f7c4eaa677a78e090c7e97e6dc47b16f655f04683ebd37bef7fe0faa";

/// Where the GNU project publishes the tarball.
pub const URL: &str = "https://ftp.gnu.org/gnu/grub/grub-2.12.tar.xz";

/// Where the tarball is fetched from: the GNU project's server, then
/// Ubuntu's copy of the same file (its `orig` tarball), which [`SHA256`]
/// checks is the same.
const URLS: &[&str] =
    &[URL, "https://archive.ubuntu.com/ubuntu/pool/main/g/grub2/grub2_2.12.orig.tar.xz"];

/// What `configure` is given for every platform: no translations,
/// fonts, themes or the tools that need libraries the images do not.
const CONFIGURE: &[&str] = &[
    "--disable-werror",
    "--disable-nls",
    "--disable-grub-mkfont",
    "--disable-grub-themes",
    "--disable-grub-mount",
    "--disable-device-mapper",
    "--disable-libzfs",
    "--disable-liblzma",
    "--disable-efiemu",
];

/// The file the 2.12 tarball leaves out, which its build reads: the
/// symbols `bli` and `part_gpt` give other modules, as GRUB's own
/// repository has it after the release.
const EXTRA_DEPS: &str = "depends bli part_gpt\n";

/// What a build depends on, kept beside it: when this changes, GRUB is
/// built again.
const RECIPE: &str =
    "grub 2.12, f3c97391, extra_deps bli part_gpt, configure flags of grub.rs, recipe 1";

/// The platforms built: GRUB's name for each, and what `configure` is
/// told for it.
const PLATFORMS: [(&str, &str, &str); 2] =
    [("i386-pc", "pc", "i386"), ("x86_64-efi", "efi", "x86_64")];

/// The modules every image's GRUB holds (ADR 0306): partition tables,
/// the filesystems it reads (the EFI system partition's FAT, the root's
/// Btrfs, a disc's ISO 9660), Multiboot2 to load Radix, the environment
/// block, `search`, the terminals and the menu, plain or themed.
pub const MODULES: &[&str] = &[
    "part_gpt",
    "part_msdos",
    "fat",
    "btrfs",
    "iso9660",
    "multiboot2",
    "loadenv",
    "search",
    "search_fs_uuid",
    "search_label",
    "search_fs_file",
    "gfxterm",
    // The install stick's menu in Dukovo's colours: GRUB's themed menu
    // and the pictures its theme draws with (ADR 0402).
    "gfxmenu",
    "png",
    "serial",
    "terminal",
    "normal",
    "test",
    "echo",
    "configfile",
    "video",
    "boot",
    // The install stick's "Boot from first disk" (ADR 0308).
    "chain",
];

/// The modules a PC's BIOS needs more: its disks, VESA's video, and the
/// BIOS's disk numbers swapped, for the install stick's "Boot from first
/// disk".
pub const PC_MODULES: &[&str] = &["biosdisk", "vbe", "drivemap"];

/// The modules UEFI needs more: the firmware's video.
pub const EFI_MODULES: &[&str] = &["efi_gop", "efi_uga"];

/// Where a disk's GRUB finds its files under a PC's BIOS: the directory
/// `boot/grub` of the disk's second partition, its EFI system partition,
/// on the disk the BIOS booted. UEFI's GRUB finds them on the partition
/// it was loaded from.
pub const BIOS_PREFIX: &str = "(,gpt2)/boot/grub";

/// Where the system keeps GRUB's images for a disk, for rgrub-setup(8).
pub const SYSTEM_DIR: &str = "system/boot/grub";

/// GRUB's images for a disk ([`Grub::disk_images`]).
pub struct DiskImages {
    /// `boot.img`, for the disk's first sector under a PC's BIOS.
    pub boot: PathBuf,
    /// `core.img`, for its BIOS boot partition, which finds GRUB's files
    /// at [`BIOS_PREFIX`].
    pub core: PathBuf,
    /// `BOOTX64.EFI`, UEFI's GRUB, for its EFI system partition.
    pub efi: PathBuf,
}

/// GRUB, built.
pub struct Grub {
    /// `grub-mkimage`.
    pub mkimage: PathBuf,
    /// The directory of the PC's modules and images (`boot.img`).
    pub pc: PathBuf,
    /// The directory of UEFI's modules.
    pub efi: PathBuf,
    /// GRUB's licence, its source's `COPYING`.
    pub copying: PathBuf,
}

/// The directory GRUB is built into: `.cache/grub/<version>`.
fn cache(ctx: &Ctx) -> PathBuf {
    ctx.root.join(".cache/grub").join(VERSION)
}

/// GRUB, built for both platforms: fetched and built the first time, and
/// when [`RECIPE`] changes.
///
/// # Errors
///
/// If it cannot be fetched or built: the tools GRUB's build needs (a C
/// compiler, make, bison, flex, gawk) are missing, say.
pub fn build(ctx: &Ctx) -> Result<Grub> {
    let dir = cache(ctx);
    let built = Grub {
        mkimage: dir.join("i386-pc/bin/grub-mkimage"),
        pc: dir.join("i386-pc/lib/grub/i386-pc"),
        efi: dir.join("x86_64-efi/lib/grub/x86_64-efi"),
        copying: dir.join("COPYING"),
    };
    let stamp = dir.join("recipe");
    if std::fs::read_to_string(&stamp).is_ok_and(|text| text == RECIPE)
        && built.mkimage.exists()
        && built.pc.join("boot.img").exists()
        && built.efi.join("kernel.img").exists()
        && built.copying.exists()
    {
        return Ok(built);
    }
    for (tool, purpose) in [
        ("make", "build GRUB"),
        ("bison", "build GRUB's parser"),
        ("flex", "build GRUB's lexer"),
        ("gawk", "list GRUB's modules"),
        ("tar", "unpack GRUB"),
    ] {
        if process::find_program(tool).is_none() {
            return Err(Error::new(format!("{tool} is needed to {purpose} but is not installed"))
                .hint("run `./ob doctor` to see what is missing"));
        }
    }
    let tarball = ctx.root.join(".cache/grub").join(format!("grub-{VERSION}.tar.xz"));
    process::create_dir(tarball.parent().expect("the cache has a parent"))?;
    if !tarball.exists() || process::sha256(ctx, &tarball)? != SHA256 {
        fetch(ctx, &tarball)?;
    }
    let work = ctx.target.join("grub");
    if work.exists() {
        std::fs::remove_dir_all(&work).context(|| format!("removing {}", work.display()))?;
    }
    process::create_dir(&work)?;
    step(&format!("Unpacking GRUB {VERSION}"));
    process::run(ctx, Command::new("tar").arg("-xJf").arg(&tarball).arg("-C").arg(&work))?;
    let source = work.join(format!("grub-{VERSION}"));
    std::fs::write(source.join("grub-core/extra_deps.lst"), EXTRA_DEPS)
        .context(|| format!("writing {}", source.display()))?;
    if dir.exists() {
        std::fs::remove_dir_all(&dir).context(|| format!("removing {}", dir.display()))?;
    }
    let jobs = std::thread::available_parallelism().map_or(1, std::num::NonZero::get);
    for (name, platform, target) in PLATFORMS {
        step(&format!("Building GRUB {VERSION} for {name}, with {jobs} jobs"));
        let build = work.join(format!("build-{name}"));
        process::create_dir(&build)?;
        let prefix = dir.join(name);
        let mut configure = Command::new(source.join("configure"));
        configure
            .arg(format!("--prefix={}", prefix.display()))
            .arg(format!("--with-platform={platform}"))
            .arg(format!("--target={target}"))
            .args(CONFIGURE)
            .arg("AWK=gawk")
            .current_dir(&build);
        logged(ctx, &mut configure, &build.join("configure.log"))?;
        let mut make = Command::new("make");
        make.arg(format!("-j{jobs}")).current_dir(&build);
        logged(ctx, &mut make, &build.join("make.log"))?;
        let mut install = Command::new("make");
        install.arg("install").current_dir(&build);
        logged(ctx, &mut install, &build.join("install.log"))?;
    }
    process::copy(&source.join("COPYING"), &built.copying)?;
    std::fs::write(&stamp, RECIPE).context(|| format!("writing {}", stamp.display()))?;
    let _ = std::fs::remove_dir_all(&work);
    Ok(built)
}

/// Runs `cmd` with its output in `log`, whose end a failure shows.
fn logged(ctx: &Ctx, cmd: &mut Command, log: &Path) -> Result<()> {
    let file = std::fs::File::create(log).context(|| format!("writing {}", log.display()))?;
    let err = file.try_clone().context(|| format!("writing {}", log.display()))?;
    cmd.stdin(Stdio::null()).stdout(file).stderr(err);
    let status = process::status(ctx, cmd)?;
    if status.success() {
        return Ok(());
    }
    let text = std::fs::read_to_string(log).unwrap_or_default();
    let tail: Vec<&str> = text.lines().rev().take(20).collect();
    let tail: Vec<&str> = tail.into_iter().rev().collect();
    Err(Error::new(format!(
        "building GRUB failed ({status}); the end of {}:\n{}",
        log.display(),
        tail.join("\n")
    )))
}

/// Downloads the pinned tarball to `path`, from the first of [`URLS`]
/// that serves it whole.
fn fetch(ctx: &Ctx, path: &Path) -> Result<()> {
    let partial = path.with_extension("partial");
    let mut failed = Vec::new();
    for url in URLS {
        step(&format!("Fetching GRUB {VERSION} from {url}"));
        let mut cmd = Command::new("curl");
        cmd.args(["--proto", "=https", "--tlsv1.2", "-fsSL", "--retry", "3", "-o"])
            .arg(&partial)
            .arg(url);
        if let Err(err) = process::run(ctx, &mut cmd) {
            failed.push(format!("{url}: {err}"));
            continue;
        }
        let sum = process::sha256(ctx, &partial)?;
        if sum == SHA256 {
            return std::fs::rename(&partial, path)
                .context(|| format!("moving it to {}", path.display()));
        }
        failed.push(format!("{url} has SHA-256 {sum}, not the pinned {SHA256}"));
    }
    let _ = std::fs::remove_file(&partial);
    Err(Error::new(format!("no copy of GRUB {VERSION} would do: {}", failed.join("; "))))
}

impl Grub {
    /// Makes GRUB's images for a disk in `dir`, which rgrub-setup(8)
    /// puts on one, as `./ob image` does.
    ///
    /// # Errors
    ///
    /// If `grub-mkimage` fails, or `dir` cannot be written.
    pub fn disk_images(&self, ctx: &Ctx, dir: &Path) -> Result<DiskImages> {
        process::create_dir(dir)?;
        let images = DiskImages {
            boot: dir.join("boot.img"),
            core: dir.join("core.img"),
            efi: dir.join("BOOTX64.EFI"),
        };
        process::copy(&self.pc.join("boot.img"), &images.boot)?;
        self.mkimage(ctx, "i386-pc", BIOS_PREFIX, None, &images.core)?;
        self.mkimage(ctx, "x86_64-efi", "/boot/grub", None, &images.efi)?;
        Ok(images)
    }

    /// Makes the image `output` of `format` (`grub-mkimage -O`), for
    /// which `prefix` is where GRUB's files are, with `config` run first if
    /// given, holding [`MODULES`] and the platform's.
    ///
    /// # Errors
    ///
    /// If `grub-mkimage` fails.
    pub fn mkimage(
        &self,
        ctx: &Ctx,
        format: &str,
        prefix: &str,
        config: Option<&Path>,
        output: &Path,
    ) -> Result<()> {
        let (dir, more) = if format.starts_with("i386-pc") {
            (&self.pc, PC_MODULES)
        } else {
            (&self.efi, EFI_MODULES)
        };
        let mut cmd = Command::new(&self.mkimage);
        cmd.arg("-O").arg(format).arg("-d").arg(dir).arg("-p").arg(prefix).arg("-o").arg(output);
        if let Some(config) = config {
            cmd.arg("-c").arg(config);
        }
        cmd.args(MODULES).args(more);
        process::run(ctx, &mut cmd)
    }
}

// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The red-team kit the lab's attack tests boot as the other machine on
//! the private segment (`ob lab redteam-kit`, `--redteam`): a Linux guest
//! that carries the tools those tests drive against the machine under
//! test, so that no attack tool ever runs on the lab PC itself. It is
//! built the way [`crate::lab::linux_kit`] builds the performance kit, but
//! from Alpine rather than from a bare kernel: Alpine's own packages give
//! the kernel, its virtio and e1000 drivers, and most of the tools without
//! fetching and checking each one by hand, and `apk.static`'s signature
//! check, run against the keys the checked minirootfs tarball carries,
//! means nothing lands in the kit unsigned.
//!
//! Everything is fetched once into `.cache/lab-redteam` under the
//! repository root, by a pinned version and a SHA-256 checked against
//! what was fetched when these constants were last raised, so a rebuild
//! with nothing changed touches the network only to re-check a kernel
//! package index; `ob lab --no-build` skips the fetch and the build
//! outright. apk's own install needs to `chroot` and to set file
//! ownership as root; this PC has no root and no setuid helpers for a
//! full subuid range, so the install runs under `unshare --user
//! --map-root-user`, which gives it a root of its own only for that one
//! user. That root cannot set ownership outside the one uid and gid it
//! was mapped, so a handful of apk's own bookkeeping files fail that one
//! `chown`; the packages they belong to still land correctly, and the
//! packed image gives every file owner 0 regardless of what the real
//! filesystem recorded, so the failed `chown`s cost nothing.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::process::{self, step};
use crate::{Context as _, Ctx, Error, Result};

/// The kit's folder under `out/`, for dukovo-lab's `--redteam`.
const DIR: &str = "lab-redteam";

/// The kit's `kit.toml`, matching [`crate::lab::linux_kit`]'s file.
const KIT_FILE: &str = "kit.toml";

/// What the kit's `/init` prints once sshd is listening.
const READY: &str = "redteam-kit: ready";

/// The private segment's NIC, by the MAC the kit contract gives it: the
/// one `lab.addr` configures, never the first NIC, which takes an
/// address over DHCP instead.
const LAB_NIC_MAC: &str = "52:54:00:4c:a1:02";

/// Alpine's branch and release: the minirootfs, and every package
/// [`PACKAGES`] names, come from here. Raising it means raising every
/// hash below from the new release's own index.
const ALPINE_BRANCH: &str = "v3.20";
const ALPINE_RELEASE: &str = "3.20.10";
const ALPINE_MIRROR: &str = "https://dl-cdn.alpinelinux.org/alpine";

/// The minirootfs tarball's SHA-256, from its own `.sha256` at
/// `{ALPINE_MIRROR}/{ALPINE_BRANCH}/releases/x86_64/`.
const MINIROOTFS_SHA256: &str = "0d44a414245f043ae56872a23762e6c9bee585da2cbe4aa81cbd0331eaf53223";

/// The packages `apk` installs into the kit's rootfs: the shell and
/// its applets, SSH's server, the attack tools the design names (sqlmap
/// is not packaged for this branch, so it comes from [`SQLMAP`] instead),
/// and `linux-virt`, whose kernel has the virtio and e1000 NIC drivers as
/// modules rather than built in.
const PACKAGES: &[&str] = &[
    "alpine-baselayout",
    "alpine-baselayout-data",
    "busybox",
    "busybox-extras",
    "openssh-server",
    "nmap",
    "nmap-scripts",
    "nmap-nselibs",
    "hydra",
    "nikto",
    "curl",
    "openssl",
    "iproute2",
    "python3",
    "linux-virt",
];

/// A release at least two weeks old (ADR: the lab never runs a tool so new
/// its own bugs could be mistaken for the machine's).
struct Pinned {
    url: &'static str,
    sha256: &'static str,
}

/// ffuf's Linux amd64 release tarball.
const FFUF: Pinned = Pinned {
    url: "https://github.com/ffuf/ffuf/releases/download/v2.3.0/ffuf_2.3.0_linux_amd64.tar.gz",
    sha256: "b2a3c725fcb9da175159682f54d6e9149f2905b00d84d155e7efc5d599975ceb",
};

/// nuclei's Linux amd64 release zip.
const NUCLEI: Pinned = Pinned {
    url: "https://github.com/projectdiscovery/nuclei/releases/download/v3.11.1/\
          nuclei_3.11.1_linux_amd64.zip",
    sha256: "ea63d4ae232808cd7c6bc00d0142428e231fab59dae01042246097d195835ab6",
};

/// nuclei's own templates, checked the same way.
const NUCLEI_TEMPLATES: Pinned = Pinned {
    url: "https://github.com/projectdiscovery/nuclei-templates/archive/refs/tags/\
          v10.4.9.tar.gz",
    sha256: "d7cd989935f9a84943cba8a193f567db37626dbf4e526ff57ba5b1f24badd5d6",
};

/// sqlmap's source tarball: not packaged for Alpine's `v3.20`, so the
/// design's fallback, a pinned release run with the kit's own Python.
const SQLMAP: Pinned = Pinned {
    url: "https://github.com/sqlmapproject/sqlmap/archive/refs/tags/1.10.tar.gz",
    sha256: "aea3bf921d9a2466bc0e33cf43bc941f527eabf7981ec1de104220def3a1cba4",
};

/// Builds the red-team kit, without root, into `out/lab-redteam`, and
/// returns its folder, for dukovo-lab's `--redteam`. A kit already built
/// from these same pins is not fetched or packed again.
pub fn kit(ctx: &Ctx) -> Result<PathBuf> {
    let dir = ctx.out.join(DIR);
    let stamp = dir.join(".built-from");
    let pin = format!(
        "{ALPINE_RELEASE} {} {} {} {}",
        FFUF.sha256, NUCLEI.sha256, NUCLEI_TEMPLATES.sha256, SQLMAP.sha256
    );
    if dir.join(KIT_FILE).is_file()
        && std::fs::read_to_string(&stamp).ok().as_deref() == Some(pin.as_str())
    {
        return Ok(dir);
    }
    for program in ["unshare", "tar", "unzip", "curl"] {
        if process::find_program(program).is_none() {
            return Err(Error::new(format!(
                "building the red-team kit takes `{program}`, which this PC does not have"
            ))
            .hint("install util-linux, tar, unzip and curl"));
        }
    }
    step("Building the red-team kit that the attack tests boot on the private segment");
    let cache = ctx.root.join(".cache/lab-redteam");
    process::create_dir(&cache)?;
    let seed = extract_seed(ctx, &cache)?;

    let partial = dir.with_extension("partial");
    if partial.exists() {
        std::fs::remove_dir_all(&partial)
            .map_err(|err| Error::new(format!("removing {}: {err}", partial.display())))?;
    }
    let rootfs = partial.join("rootfs");
    install_packages(ctx, &seed, &rootfs)?;
    fetch_tools(ctx, &cache, &rootfs)?;
    let kernel = write_init(&rootfs)?;

    step("Packing the red-team kit's initramfs");
    let cpio = partial.join("initramfs-redteam.cpio");
    std::fs::write(&cpio, pack_initramfs(&rootfs)?)
        .context(|| format!("writing {}", cpio.display()))?;
    process::run(ctx, Command::new("gzip").arg("-9").arg("-n").arg("-f").arg(&cpio))?;
    let kernel_copy = partial.join("vmlinuz");
    process::copy(&kernel, &kernel_copy)?;
    std::fs::write(partial.join(KIT_FILE), KIT_TOML)
        .context(|| format!("writing {}", partial.join(KIT_FILE).display()))?;
    std::fs::write(partial.join(".built-from"), &pin)
        .context(|| format!("writing {}", partial.join(".built-from").display()))?;

    if dir.exists() {
        std::fs::remove_dir_all(&dir)
            .map_err(|err| Error::new(format!("removing {}: {err}", dir.display())))?;
    }
    std::fs::rename(&partial, &dir)
        .map_err(|err| Error::new(format!("moving it to {}: {err}", dir.display())))?;
    Ok(dir)
}

/// The kit's `kit.toml`, in the form the kit contract gives: the kernel
/// and initramfs beside it, the command line (the serial console, and
/// `lab.key`/`lab.addr`, which dukovo-lab appends), the account the key
/// signs in as, the memory its initramfs unpacks into (about 480 MB
/// unpacked, which a 1 GB guest cannot hold), and the line that says sshd
/// is up.
const KIT_TOML: &str = "\
# The red-team kit: `ob lab redteam-kit` makes this folder, and dukovo-lab\n\
# boots it with --redteam for the lab's attack tests.\n\
kernel = \"vmlinuz\"\n\
initrd = \"initramfs-redteam.cpio.gz\"\n\
cmdline = \"console=ttyS0\"\n\
user = \"root\"\n\
memory = \"3G\"\n\
ready = \"redteam-kit: ready\"\n";

/// Fetches Alpine's minirootfs, once, and returns its extracted folder:
/// `apk`, the keys it trusts, and the musl loader that runs it, all
/// native x86-64 binaries, so running them here takes no chroot.
fn extract_seed(ctx: &Ctx, cache: &Path) -> Result<PathBuf> {
    let seed = cache.join(format!("minirootfs-{ALPINE_RELEASE}"));
    if seed.join("sbin/apk").is_file() {
        return Ok(seed);
    }
    let tarball = cache.join(format!("alpine-minirootfs-{ALPINE_RELEASE}-x86_64.tar.gz"));
    let url = format!(
        "{ALPINE_MIRROR}/{ALPINE_BRANCH}/releases/x86_64/alpine-minirootfs-{ALPINE_RELEASE}-x86_64.tar.gz"
    );
    fetch_checked(ctx, &url, &tarball, MINIROOTFS_SHA256, "Alpine's minirootfs")?;
    process::create_dir(&seed)?;
    process::run(ctx, Command::new("tar").arg("-xzf").arg(&tarball).arg("-C").arg(&seed))?;
    Ok(seed)
}

/// Downloads `url` to `dest` unless it is already there with `sha256`;
/// a mismatch after a fresh download is the download's own error, not a
/// stale cache's, since `dest` is never kept with the wrong hash.
fn fetch_checked(ctx: &Ctx, url: &str, dest: &Path, sha256: &str, what: &str) -> Result<()> {
    if dest.is_file() && process::sha256(ctx, dest)? == sha256 {
        return Ok(());
    }
    step(&format!("Fetching {what}"));
    let partial = dest.with_extension("partial");
    let mut cmd = Command::new("curl");
    cmd.args(["--proto", "=https", "--tlsv1.2", "-fsSL", "--retry", "3", "-o"])
        .arg(&partial)
        .arg(url);
    process::run(ctx, &mut cmd)?;
    let sum = process::sha256(ctx, &partial)?;
    if sum != sha256 {
        let _ = std::fs::remove_file(&partial);
        return Err(Error::new(format!("{url} has SHA-256 {sum}, not the pinned {sha256}")));
    }
    std::fs::rename(&partial, dest)
        .map_err(|err| Error::new(format!("moving it to {}: {err}", dest.display())))
}

/// Runs `apk`, from the seed's musl loader and libraries, inside a fresh
/// user namespace mapped to a root of its own (`unshare --map-root-user`),
/// which is the rootless way to let `apk` `chroot` for its packages'
/// install scripts and set ownership within that one mapped uid and gid.
fn apk_command(seed: &Path, rootfs: &Path, args: &[&str]) -> Command {
    let mut cmd = Command::new("unshare");
    cmd.args(["--user", "--map-root-user", "--mount", "--"])
        .arg(seed.join("lib/ld-musl-x86_64.so.1"))
        .arg("--library-path")
        .arg(format!("{}:{}", seed.join("lib").display(), seed.join("usr/lib").display()))
        .arg(seed.join("sbin/apk"))
        .arg("--root")
        .arg(rootfs)
        .args(args);
    cmd
}

/// Installs [`PACKAGES`] into `rootfs`, fresh, with `apk`'s own signature
/// check against the keys the checked minirootfs carries (never
/// `--allow-untrusted`). The one mapped uid cannot take the ownership a
/// handful of apk's bookkeeping files ask for, which apk reports as a
/// non-zero exit over otherwise-clean output; that failure is accepted
/// only when the summary line says every package still went in.
fn install_packages(ctx: &Ctx, seed: &Path, rootfs: &Path) -> Result<()> {
    step("Installing the red-team kit's packages with apk, under a user namespace of their own");
    let keys = rootfs.join("etc/apk/keys");
    process::create_dir(&keys)?;
    for key in std::fs::read_dir(seed.join("etc/apk/keys"))
        .context(|| format!("reading {}", seed.join("etc/apk/keys").display()))?
    {
        let key = key.context(|| "reading a key".to_string())?.path();
        process::copy(&key, &keys.join(key.file_name().unwrap_or_default()))?;
    }
    std::fs::create_dir_all(rootfs.join("etc/apk"))
        .context(|| format!("making {}", rootfs.join("etc/apk").display()))?;
    std::fs::write(
        rootfs.join("etc/apk/repositories"),
        format!(
            "{ALPINE_MIRROR}/{ALPINE_BRANCH}/main\n{ALPINE_MIRROR}/{ALPINE_BRANCH}/community\n"
        ),
    )
    .context(|| format!("writing {}", rootfs.join("etc/apk/repositories").display()))?;

    let mut add = apk_command(seed, rootfs, &["--initdb", "add"]);
    add.args(PACKAGES);
    let out = add.output().map_err(|err| process::spawn_error("unshare", &err))?;
    // apk's summary line ("N MiB in M packages") goes to standard output
    // even when it also reports an error on standard error; only that
    // line says whether every package actually went in.
    let printed = String::from_utf8_lossy(&out.stdout);
    let said = String::from_utf8_lossy(&out.stderr);
    if !out.status.success() && !printed.lines().any(|line| line.contains(" packages")) {
        return Err(Error::new(format!("apk add failed: {}", said.trim_end())));
    }
    if ctx.verbose && !out.status.success() {
        eprintln!(
            "    note: apk reported errors setting ownership outside its namespace: {}",
            said.trim_end()
        );
    }
    Ok(())
}

/// Fetches ffuf, nuclei, nuclei's templates and sqlmap, each pinned and
/// checked, and places them in `rootfs`: ffuf and nuclei as static
/// binaries in `/usr/local/bin`, the templates at `/opt/nuclei-templates`,
/// and sqlmap at `/opt/sqlmap`, run through a wrapper that calls it with
/// the kit's own Python since Alpine does not package it.
fn fetch_tools(ctx: &Ctx, cache: &Path, rootfs: &Path) -> Result<()> {
    let bin = rootfs.join("usr/local/bin");
    process::create_dir(&bin)?;

    let ffuf_archive = cache.join("ffuf.tar.gz");
    fetch_checked(ctx, FFUF.url, &ffuf_archive, FFUF.sha256, "ffuf")?;
    process::run(
        ctx,
        Command::new("tar").arg("-xzf").arg(&ffuf_archive).arg("-C").arg(&bin).arg("ffuf"),
    )?;

    let nuclei_archive = cache.join("nuclei.zip");
    fetch_checked(ctx, NUCLEI.url, &nuclei_archive, NUCLEI.sha256, "nuclei")?;
    process::run(
        ctx,
        Command::new("unzip")
            .args(["-o", "-q"])
            .arg(&nuclei_archive)
            .arg("nuclei")
            .arg("-d")
            .arg(&bin),
    )?;
    for tool in ["ffuf", "nuclei"] {
        set_executable(&bin.join(tool))?;
    }

    let templates = rootfs.join("opt/nuclei-templates");
    process::create_dir(&templates)?;
    let templates_archive = cache.join("nuclei-templates.tar.gz");
    fetch_checked(
        ctx,
        NUCLEI_TEMPLATES.url,
        &templates_archive,
        NUCLEI_TEMPLATES.sha256,
        "nuclei's templates",
    )?;
    process::run(
        ctx,
        Command::new("tar")
            .arg("-xzf")
            .arg(&templates_archive)
            .arg("-C")
            .arg(&templates)
            .arg("--strip-components=1"),
    )?;

    let sqlmap = rootfs.join("opt/sqlmap");
    process::create_dir(&sqlmap)?;
    let sqlmap_archive = cache.join("sqlmap.tar.gz");
    fetch_checked(ctx, SQLMAP.url, &sqlmap_archive, SQLMAP.sha256, "sqlmap")?;
    process::run(
        ctx,
        Command::new("tar")
            .arg("-xzf")
            .arg(&sqlmap_archive)
            .arg("-C")
            .arg(&sqlmap)
            .arg("--strip-components=1"),
    )?;
    std::fs::write(bin.join("sqlmap"), "#!/bin/sh\nexec python3 /opt/sqlmap/sqlmap.py \"$@\"\n")
        .context(|| format!("writing {}", bin.join("sqlmap").display()))?;
    set_executable(&bin.join("sqlmap"))?;
    Ok(())
}

/// Sets `path`'s mode to `0o755`, for a binary a download does not mark
/// executable.
fn set_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
        .map_err(|err| Error::new(format!("setting {} executable: {err}", path.display())))
}

/// Writes the kit's `/init`, which brings up both NICs, installs the
/// lab's key, starts sshd keys-only and prints [`READY`], and sshd's own
/// override that turns password sign-in off; returns the kernel apk
/// installed, `rootfs/boot/vmlinuz-virt`.
fn write_init(rootfs: &Path) -> Result<PathBuf> {
    let kernel = std::fs::read_dir(rootfs.join("boot"))
        .context(|| format!("reading {}", rootfs.join("boot").display()))?
        .filter_map(std::result::Result::ok)
        .map(|entry| entry.path())
        .find(|path| {
            path.file_name().is_some_and(|name| name.to_string_lossy().starts_with("vmlinuz"))
        })
        .ok_or_else(|| Error::new(format!("{} holds no vmlinuz", rootfs.join("boot").display())))?;

    let overrides = rootfs.join("etc/ssh/sshd_config.d");
    process::create_dir(&overrides)?;
    std::fs::write(
        overrides.join("redteam.conf"),
        // Alpine's openssh-server is built without PAM, so UsePAM is not
        // one of its options; sshd refuses to start with an option it
        // does not have.
        "# The red-team kit signs in with the lab's key alone.\n\
         PermitRootLogin prohibit-password\n\
         PubkeyAuthentication yes\n\
         PasswordAuthentication no\n\
         KbdInteractiveAuthentication no\n\
         ChallengeResponseAuthentication no\n",
    )
    .context(|| format!("writing {}", overrides.join("redteam.conf").display()))?;

    let init = rootfs.join("init");
    std::fs::write(&init, init_script()).context(|| format!("writing {}", init.display()))?;
    set_executable(&init)?;
    Ok(kernel)
}

/// The kit's `/init`: a POSIX shell script, since the kit carries no
/// other interpreter it can rely on before `/opt/sqlmap`'s Python is
/// reachable over the network it has yet to bring up. It brings up both
/// NICs (DHCP on the one that is not [`LAB_NIC_MAC`], `lab.addr` on the
/// one that is), installs `lab.key` as root's key, starts sshd keys-only,
/// and prints [`READY`] once sshd is listening.
fn init_script() -> String {
    format!(
        r#"#!/bin/sh
mount -t proc proc /proc
mount -t sysfs sysfs /sys
mount -t devtmpfs devtmpfs /dev
mkdir -p /dev/pts /dev/shm
mount -t devpts devpts /dev/pts
mount -t tmpfs tmpfs /dev/shm

# virtio-net and e1000 are modules on this kernel; modprobe pulls in
# whichever of the two the NIC QEMU gave this guest needs, from the
# modules.dep apk's depmod trigger already wrote.
modprobe virtio_net 2>/dev/null
modprobe e1000 2>/dev/null

lab_if=
dhcp_if=
for n in /sys/class/net/*; do
    name=${{n##*/}}
    [ "$name" = lo ] && continue
    mac=$(cat "$n/address" 2>/dev/null)
    if [ "$mac" = "{LAB_NIC_MAC}" ]; then
        lab_if=$name
    else
        dhcp_if=$name
    fi
done

ip link set lo up
if [ -n "$dhcp_if" ]; then
    ip link set "$dhcp_if" up
    udhcpc -i "$dhcp_if" -n -q -t 3 -T 3 2>/dev/null
fi

lab_addr=
lab_key=
for word in $(cat /proc/cmdline); do
    case $word in
        lab.addr=*) lab_addr=${{word#lab.addr=}} ;;
        lab.key=*) lab_key=${{word#lab.key=}} ;;
    esac
done

if [ -n "$lab_if" ] && [ -n "$lab_addr" ]; then
    ip link set "$lab_if" up
    ip addr add "$lab_addr" dev "$lab_if"
fi

mkdir -p /root/.ssh
if [ -n "$lab_key" ]; then
    echo "$lab_key" | base64 -d > /root/.ssh/authorized_keys
fi
chmod 700 /root/.ssh
[ -f /root/.ssh/authorized_keys ] && chmod 600 /root/.ssh/authorized_keys

ssh-keygen -A >/dev/null 2>&1
mkdir -p /run/sshd
/usr/sbin/sshd

# sshd daemonizes itself; the ready line waits for it to be listening,
# up to 30s, rather than a fixed sleep that a slow boot could outrun.
i=0
while [ $i -lt 30 ]; do
    grep -q ':0016 ' /proc/net/tcp 2>/dev/null && break
    sleep 1
    i=$((i + 1))
done
echo "{READY}"

while true; do
    sleep 3600
done
"#
    )
}

/// Packs `rootfs` into a gzip-compressed cpio "newc" archive, the way
/// Linux's initramfs reads one ([`crate::bench::initramfs`]): every file
/// gets owner `(0, 0)` regardless of what the real filesystem recorded,
/// since the rootless install could not always set that itself (see the
/// module's own doc comment).
fn pack_initramfs(rootfs: &Path) -> Result<Vec<u8>> {
    let mut entries = Vec::new();
    walk(rootfs, "", &mut entries)?;
    let mut out = Vec::new();
    for (ino, (name, mode, data)) in (1u32..).zip(entries) {
        crate::bench::newc_entry(&mut out, ino, &name, mode, (0, 0), &data);
    }
    crate::bench::newc_entry(&mut out, 0, "TRAILER!!!", 0, (0, 0), &[]);
    Ok(out)
}

/// Walks `dir` (the rootfs root when `prefix` is empty), adding every
/// regular file, symlink and directory under it to `entries` as
/// `(cpio name, mode, data)`, sorted so the archive is the same byte for
/// byte on every build. `boot/` and `var/cache/apk/` are left out: the
/// kernel they hold goes to QEMU on its own, and the apk cache is only
/// ever useful on this PC.
fn walk(dir: &Path, prefix: &str, entries: &mut Vec<(String, u32, Vec<u8>)>) -> Result<()> {
    if !prefix.is_empty() {
        entries.push((prefix.trim_end_matches('/').to_string(), 0o040_755, Vec::new()));
    }
    let mut names: Vec<_> = std::fs::read_dir(dir)
        .context(|| format!("reading {}", dir.display()))?
        .filter_map(std::result::Result::ok)
        .map(|entry| entry.file_name())
        .collect();
    names.sort();
    for name in names {
        let name = name.to_string_lossy().into_owned();
        let rel = format!("{prefix}{name}");
        if rel == "boot" || rel == "var/cache/apk" {
            continue;
        }
        let path = dir.join(&name);
        let meta = std::fs::symlink_metadata(&path)
            .context(|| format!("reading the metadata of {}", path.display()))?;
        if meta.is_dir() {
            walk(&path, &format!("{rel}/"), entries)?;
        } else if meta.file_type().is_symlink() {
            let target = std::fs::read_link(&path)
                .context(|| format!("reading the symlink {}", path.display()))?;
            entries.push((rel, 0o120_777, target.to_string_lossy().into_owned().into_bytes()));
        } else {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = 0o100_000 | (meta.permissions().mode() & 0o777);
            let data = std::fs::read(&path).context(|| format!("reading {}", path.display()))?;
            entries.push((rel, mode, data));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pinned_hashes_are_sha256() {
        for sha256 in
            [MINIROOTFS_SHA256, FFUF.sha256, NUCLEI.sha256, NUCLEI_TEMPLATES.sha256, SQLMAP.sha256]
        {
            assert_eq!(sha256.len(), 64);
            assert!(sha256.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()));
        }
    }

    #[test]
    fn the_init_script_names_the_lab_nic_and_the_ready_line() {
        let script = init_script();
        assert!(script.contains(LAB_NIC_MAC));
        assert!(script.contains(READY));
    }
}

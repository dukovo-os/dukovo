// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `ob fixture btrfs-send`: streams Linux's `btrfs send` made.
//!
//! A Btrfs disk `mkfs.btrfs` makes is mounted through a loop device, a
//! subvolume on it filled with what a send carries (folders deep and
//! renamed, small, large, sparse and cloned files, hard and symbolic
//! links, a named pipe and a device, extended attributes, owners,
//! permissions and times), snapshotted read-only, changed (folders
//! swapped and moved, files cut, grown, written over, renamed over each
//! other and removed, a name taken by another kind of file) and
//! snapshotted again. `btrfs send` writes the first snapshot whole and the
//! second from the first, and a list of each snapshot's files says what a
//! receiver must make of them. The Btrfs host tests receive both streams
//! and check the copies against the lists (ADR 0462). Loop mounts take
//! root; `setfattr` and `getfattr` come with the `attr` package.

use std::fmt::Write as _;
use std::fs::{self, File};
use std::os::unix::fs::{MetadataExt, PermissionsExt, chown, symlink};
use std::path::Path;
use std::process::Command;
use std::time::{Duration, SystemTime};

use super::FIXTURES;
use crate::process::{self, step};
use crate::{Context, Ctx, Result};

/// The disk's size.
const DISK_BYTES: u64 = 256 << 20;

/// `ob fixture btrfs-send`.
pub fn btrfs_send(ctx: &Ctx) -> Result<()> {
    for tool in ["mount", "umount", "setfattr", "getfattr", "mknod", "cp", "sha256sum"] {
        if process::find_program(tool).is_none() {
            crate::bail!("{tool} is missing: install util-linux, coreutils and attr");
        }
    }
    let btrfs = crate::btrfs::tool(ctx)?;
    let dir = ctx.out.join("fixture/btrfs-send");
    let _ = fs::remove_dir_all(&dir);
    process::create_dir(&dir)?;
    let image = dir.join("send.img");
    let file = File::create(&image).context(|| format!("making {}", image.display()))?;
    file.set_len(DISK_BYTES).context(|| format!("sizing {}", image.display()))?;
    drop(file);
    let mut mkfs = Command::new(crate::btrfs::mkfs(ctx)?);
    mkfs.args(["-q", "-f", "-L", "SEND"]).arg(&image);
    process::run(ctx, &mut mkfs)?;
    let mount = dir.join("mnt");
    process::create_dir(&mount)?;
    let mut mounting = Command::new("mount");
    mounting.args(["-o", "loop"]).arg(&image).arg(&mount);
    if process::run(ctx, &mut mounting).is_err() {
        crate::bail!(
            "mounting {} failed: loop mounts take root (sudo ./ob fixture btrfs-send)",
            image.display()
        );
    }
    let made = make(ctx, &btrfs, &mount, &dir);
    let mut unmount = Command::new("umount");
    unmount.arg(&mount);
    let unmounted = process::run(ctx, &mut unmount);
    made?;
    unmounted?;
    for name in [
        "btrfs-send-full.stream",
        "btrfs-send-inc.stream",
        "btrfs-send-s1.txt",
        "btrfs-send-s2.txt",
    ] {
        process::copy(&dir.join(name), &Path::new(FIXTURES).join(name))?;
    }
    step(&format!("Wrote the streams and their lists to {FIXTURES}"));
    Ok(())
}

/// `ob fixture btrfs-send --check <dir>`: Linux's `btrfs receive` takes
/// the streams Radix sent that the Btrfs host tests kept in `dir`
/// (`SEND_STREAMS_TO`, ADR 0462), each seed's rounds in order into a
/// folder of their own on a new Btrfs, and each copy is listed as the
/// list beside its stream says the snapshot was.
pub fn check(ctx: &Ctx, streams: &Path) -> Result<()> {
    let mut found = Vec::new();
    for entry in fs::read_dir(streams).context(|| format!("listing {}", streams.display()))? {
        let name = entry.context(|| format!("listing {}", streams.display()))?.file_name();
        let name = name.to_string_lossy();
        let Some((seed, round)) = name.strip_suffix(".stream").and_then(|n| n.split_once('-'))
        else {
            continue;
        };
        if let (Ok(seed), Ok(round)) = (u64::from_str_radix(seed, 16), round.parse::<u32>()) {
            found.push((seed, round));
        }
    }
    found.sort_unstable();
    if found.is_empty() {
        crate::bail!(
            "{} holds no streams: run the Btrfs host tests with SEND_STREAMS_TO",
            streams.display()
        );
    }
    let btrfs = crate::btrfs::tool(ctx)?;
    let dir = ctx.out.join("fixture/btrfs-send-check");
    let _ = fs::remove_dir_all(&dir);
    process::create_dir(&dir)?;
    let image = dir.join("receive.img");
    let file = File::create(&image).context(|| format!("making {}", image.display()))?;
    file.set_len(4 * DISK_BYTES).context(|| format!("sizing {}", image.display()))?;
    drop(file);
    let mut mkfs = Command::new(crate::btrfs::mkfs(ctx)?);
    mkfs.args(["-q", "-f", "-L", "RECEIVE"]).arg(&image);
    process::run(ctx, &mut mkfs)?;
    let mount = dir.join("mnt");
    process::create_dir(&mount)?;
    let mut mounting = Command::new("mount");
    mounting.args(["-o", "loop"]).arg(&image).arg(&mount);
    if process::run(ctx, &mut mounting).is_err() {
        crate::bail!(
            "mounting {} failed: loop mounts take root (sudo ./ob fixture btrfs-send --check)",
            image.display()
        );
    }
    let mut differ = Vec::new();
    let received = (|| -> Result<()> {
        for &(seed, round) in &found {
            let into = mount.join(format!("{seed:x}"));
            if round == 0 {
                process::create_dir(&into)?;
            }
            let stream = streams.join(format!("{seed:x}-{round}.stream"));
            let mut receive = Command::new(&btrfs);
            receive.args(["-q", "receive", "-f"]).arg(&stream).arg(&into);
            process::run(ctx, &mut receive)?;
            let list = streams.join(format!("{seed:x}-{round}.txt"));
            let want =
                fs::read_to_string(&list).context(|| format!("reading {}", list.display()))?;
            if listing(ctx, &into.join(format!("s{round}")))? != want {
                differ.push(format!("{seed:x}-{round}"));
            }
        }
        Ok(())
    })();
    let mut unmount = Command::new("umount");
    unmount.arg(&mount);
    let unmounted = process::run(ctx, &mut unmount);
    received?;
    unmounted?;
    if !differ.is_empty() {
        crate::bail!("Linux's copies differ from the snapshots sent for {}", differ.join(", "));
    }
    step(&format!("Linux received all {} streams, each copy as its snapshot was", found.len()));
    Ok(())
}

/// Fills, snapshots, changes, snapshots and sends, on the disk at `mount`.
fn make(ctx: &Ctx, btrfs: &Path, mount: &Path, dir: &Path) -> Result<()> {
    let run = |args: &[&str]| -> Result<()> {
        let mut cmd = Command::new(btrfs);
        cmd.args(args);
        process::run(ctx, &mut cmd)
    };
    let at = |name: &str| mount.join(name).to_string_lossy().into_owned();
    run(&["-q", "subvolume", "create", &at("src")])?;
    let src = mount.join("src");
    step("Filling the subvolume");
    fill(ctx, &src)?;
    run(&["-q", "subvolume", "snapshot", "-r", &at("src"), &at("s1")])?;
    step("Changing it");
    change(ctx, &src)?;
    run(&["-q", "subvolume", "snapshot", "-r", &at("src"), &at("s2")])?;
    let out = |name: &str| dir.join(name).to_string_lossy().into_owned();
    run(&["-q", "send", "--proto", "1", "-f", &out("btrfs-send-full.stream"), &at("s1")])?;
    run(&[
        "-q",
        "send",
        "--proto",
        "1",
        "-p",
        &at("s1"),
        "-f",
        &out("btrfs-send-inc.stream"),
        &at("s2"),
    ])?;
    for name in ["s1", "s2"] {
        let list = listing(ctx, &mount.join(name))?;
        let path = dir.join(format!("btrfs-send-{name}.txt"));
        fs::write(&path, list).context(|| format!("writing {}", path.display()))?;
    }
    Ok(())
}

/// Bytes that differ from offset to offset and file to file.
fn pattern(seed: u32, len: usize) -> Vec<u8> {
    super::ntfs::pattern(seed, len)
}

fn write(path: &Path, data: &[u8]) -> Result<()> {
    fs::write(path, data).context(|| format!("writing {}", path.display()))
}

/// Writes `data` at `offset` of the file `path`.
fn write_at(path: &Path, offset: u64, data: &[u8]) -> Result<()> {
    use std::os::unix::fs::FileExt;
    let file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .context(|| format!("opening {}", path.display()))?;
    file.write_all_at(data, offset).context(|| format!("writing {}", path.display()))
}

/// Sets when `path` was last read and changed, `secs` after 1970 and a
/// few nanoseconds.
fn times(path: &Path, secs: u64) -> Result<()> {
    let when = SystemTime::UNIX_EPOCH + Duration::new(secs, 123_456_789);
    let file = File::open(path).context(|| format!("opening {}", path.display()))?;
    let set = fs::FileTimes::new().set_accessed(when).set_modified(when);
    file.set_times(set).context(|| format!("setting the times of {}", path.display()))
}

fn mode(path: &Path, mode: u32) -> Result<()> {
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .context(|| format!("setting the mode of {}", path.display()))
}

fn mkdir(path: &Path) -> Result<()> {
    fs::create_dir(path).context(|| format!("making {}", path.display()))
}

fn rename(from: &Path, to: &Path) -> Result<()> {
    fs::rename(from, to).context(|| format!("renaming {} to {}", from.display(), to.display()))
}

fn tool(ctx: &Ctx, program: &str, args: &[&str], path: &Path) -> Result<()> {
    let mut cmd = Command::new(program);
    cmd.args(args).arg(path);
    process::run(ctx, &mut cmd)
}

/// The first snapshot's files.
fn fill(ctx: &Ctx, src: &Path) -> Result<()> {
    for d in ["a", "a/b", "a/b/c", "x", "y", "with space", "é", "dir to file"] {
        mkdir(&src.join(d))?;
    }
    write(&src.join("a/small"), &pattern(1, 100))?;
    write(&src.join("a/b/medium"), &pattern(2, 70_000))?;
    write(&src.join("a/b/c/deep"), b"deep down\n")?;
    write(&src.join("big"), &pattern(3, 200_000))?;
    write(&src.join("empty"), b"")?;
    write(&src.join("one"), b"one\n")?;
    write(&src.join("two"), b"two\n")?;
    write(&src.join("with space/file"), &pattern(4, 5000))?;
    write(&src.join("x/in x"), b"x\n")?;
    write(&src.join("y/in y"), b"y\n")?;
    write(&src.join("é/ü"), &pattern(5, 9000))?;
    // A sparse file: data, a hole of a mebibyte, data.
    write_at(&src.join("sparse"), 0, &pattern(6, 4096))?;
    write_at(&src.join("sparse"), 1 << 20, &pattern(7, 4096))?;
    fs::hard_link(src.join("a/b/medium"), src.join("a/link to medium"))
        .context(|| "linking a/b/medium".to_string())?;
    symlink("a/b/medium", src.join("sym")).context(|| "making sym".to_string())?;
    symlink("../nowhere", src.join("dangling")).context(|| "making dangling".to_string())?;
    let mut fifo = Command::new("mknod");
    fifo.args(["-m", "600"]).arg(src.join("fifo")).arg("p");
    process::run(ctx, &mut fifo)?;
    let mut device = Command::new("mknod");
    device.arg(src.join("null")).args(["c", "1", "3"]);
    process::run(ctx, &mut device)?;
    // A clone: Linux sends it as one.
    let mut cp = Command::new("cp");
    cp.arg("--reflink=always").arg(src.join("big")).arg(src.join("a/clone of big"));
    process::run(ctx, &mut cp)?;
    tool(ctx, "setfattr", &["-n", "user.color", "-v", "blue"], &src.join("a/small"))?;
    tool(ctx, "setfattr", &["-n", "user.bin", "-v", "0x00ff10"], &src.join("x"))?;
    tool(ctx, "setfattr", &["-n", "user.gone", "-v", "soon"], &src.join("one"))?;
    mode(&src.join("a/small"), 0o4755)?;
    mode(&src.join("y"), 0o700)?;
    chown(src.join("two"), Some(1000), Some(100)).context(|| "chown two".to_string())?;
    for (n, path) in ["a/small", "big", "x/in x", "sparse"].iter().enumerate() {
        times(&src.join(path), 1_700_000_000 + n as u64 * 86_400)?;
    }
    Ok(())
}

/// The changes the second snapshot has.
fn change(ctx: &Ctx, src: &Path) -> Result<()> {
    // x and y swap their names; a/b moves under é, and c out of it.
    rename(&src.join("x"), &src.join("t"))?;
    rename(&src.join("y"), &src.join("x"))?;
    rename(&src.join("t"), &src.join("y"))?;
    rename(&src.join("a/b"), &src.join("é/b"))?;
    rename(&src.join("é/b/c"), &src.join("c"))?;
    // A folder and what it holds go.
    fs::remove_file(src.join("with space/file")).context(|| "removing".to_string())?;
    fs::remove_dir(src.join("with space")).context(|| "removing".to_string())?;
    // Written over, cut and grown.
    write_at(&src.join("big"), 50_000, &pattern(8, 8192))?;
    let medium = fs::OpenOptions::new().write(true).open(src.join("é/b/medium"));
    medium.and_then(|f| f.set_len(30_000)).context(|| "cutting medium".to_string())?;
    write_at(&src.join("a/small"), 100, b" and more")?;
    write_at(&src.join("sparse"), 3 << 20, &pattern(9, 100))?;
    // Names: one goes over two, a link goes, another comes.
    rename(&src.join("one"), &src.join("two"))?;
    fs::remove_file(src.join("a/link to medium")).context(|| "unlinking".to_string())?;
    fs::hard_link(src.join("é/b/medium"), src.join("y/medium again"))
        .context(|| "linking".to_string())?;
    // A pipe's name taken by a folder, and a folder's by a file.
    fs::remove_file(src.join("fifo")).context(|| "removing fifo".to_string())?;
    mkdir(&src.join("fifo"))?;
    write(&src.join("fifo/inside"), b"inside\n")?;
    fs::remove_dir(src.join("dir to file")).context(|| "removing".to_string())?;
    write(&src.join("dir to file"), b"a file now\n")?;
    tool(ctx, "setfattr", &["-x", "user.gone"], &src.join("two"))?;
    tool(ctx, "setfattr", &["-n", "user.new", "-v", "value"], &src.join("c/deep"))?;
    mode(&src.join("big"), 0o640)?;
    chown(src.join("c/deep"), Some(2000), Some(200)).context(|| "chown deep".to_string())?;
    times(&src.join("big"), 1_710_000_000)?;
    Ok(())
}

/// The list of every file under `top`: a line each, sorted by path, of
/// tab-separated fields: the path (`.` for the top), its kind, its
/// permission bits in octal, its owner and group, when it was last read
/// and changed in nanoseconds since 1970, what it holds (a file's
/// SHA-256, a link's target, a device's major:minor, else `-`), its
/// extended attributes as `name=hex` joined by commas (`-` for none), and
/// the first of its paths, for files with more than one.
fn listing(ctx: &Ctx, top: &Path) -> Result<String> {
    let mut paths = Vec::new();
    collect(top, Path::new(""), &mut paths)?;
    paths.sort();
    let mut first = std::collections::BTreeMap::new();
    let mut out = String::new();
    for rel in &paths {
        let path = if rel.is_empty() { top.to_path_buf() } else { top.join(rel) };
        let meta = fs::symlink_metadata(&path).context(|| format!("reading {}", path.display()))?;
        let kind = meta.file_type();
        use std::os::unix::fs::FileTypeExt;
        let (name, what) = if kind.is_dir() {
            ("dir", "-".to_string())
        } else if kind.is_symlink() {
            let target = fs::read_link(&path).context(|| format!("reading {}", path.display()))?;
            ("link", target.to_string_lossy().into_owned())
        } else if kind.is_file() {
            ("file", process::sha256(ctx, &path)?)
        } else if kind.is_fifo() {
            ("fifo", "-".to_string())
        } else if kind.is_socket() {
            ("sock", "-".to_string())
        } else {
            let rdev = meta.rdev();
            let (major, minor) = ((rdev >> 8) & 0xfff, (rdev & 0xff) | ((rdev >> 12) & !0xff));
            (if kind.is_block_device() { "blk" } else { "chr" }, format!("{major}:{minor}"))
        };
        let shown = if rel.is_empty() { ".".to_string() } else { rel.clone() };
        let first = first.entry(meta.ino()).or_insert_with(|| shown.clone()).clone();
        let ns = |secs: i64, nanos: i64| secs as i128 * 1_000_000_000 + nanos as i128;
        let _ = writeln!(
            out,
            "{shown}\t{name}\t{:o}\t{}\t{}\t{}\t{}\t{what}\t{}\t{first}",
            meta.mode() & 0o7777,
            meta.uid(),
            meta.gid(),
            ns(meta.atime(), meta.atime_nsec()),
            ns(meta.mtime(), meta.mtime_nsec()),
            xattrs(ctx, &path)?,
        );
    }
    Ok(out)
}

fn collect(top: &Path, rel: &Path, out: &mut Vec<String>) -> Result<()> {
    out.push(rel.to_string_lossy().into_owned());
    let path = top.join(rel);
    if !fs::symlink_metadata(&path).context(|| format!("reading {}", path.display()))?.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(&path).context(|| format!("listing {}", path.display()))? {
        let entry = entry.context(|| format!("listing {}", path.display()))?;
        collect(top, &rel.join(entry.file_name()), out)?;
    }
    Ok(())
}

/// The extended attributes of `path`, as `name=hex` joined by commas.
fn xattrs(ctx: &Ctx, path: &Path) -> Result<String> {
    let mut cmd = Command::new("getfattr");
    cmd.args(["-h", "-d", "-m", "-", "-e", "hex", "--absolute-names"]).arg(path);
    let out = process::output(ctx, &mut cmd)?;
    let text = String::from_utf8_lossy(&out.stdout);
    let mut found: Vec<String> = text
        .lines()
        .filter_map(|line| line.split_once('='))
        .map(|(name, value)| format!("{name}={}", value.trim_start_matches("0x")))
        .collect();
    found.sort();
    Ok(if found.is_empty() { "-".into() } else { found.join(",") })
}

// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The Dukovo root's Btrfs (ADR 0137), made and filled by the kernel's
//! own Btrfs crate (ADR 0304): made as Dukovo's mkfs-btrfs makes one,
//! then mounted by the crate's writer, as Radix mounts it, which makes
//! the subvolumes and writes the staging tree into them, compressed. No
//! btrfs-progs is needed, and every file is root's (ADR 0213) but the
//! trees an [`Owned`] names: the lab account's home in a lab image (ADR
//! 0400), whose SSH key sshd reads only from a home that is the account's
//! or root's and that authd, finding it there, leaves as it is.

use std::io::Read as _;
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::path::Path;
use std::sync::OnceLock;

use disk::btrfs::{BlockFile, Options};
use radix_btrfs::BtrfsFs;
use radix_btrfs::node::BtrfsNode;
use radix_vfs::{Access, Kind, NodeRef, Slice};

use crate::{Context, Error, Result};

/// The time the image is made at, in seconds since 1970: what the clock
/// the crate reads says, so that every new inode has it.
static TIME: OnceLock<u64> = OnceLock::new();

/// The clock the crate reads: [`TIME`], in nanoseconds.
fn clock() -> u64 {
    TIME.get().copied().unwrap_or(0).saturating_mul(1_000_000_000)
}

/// The random bytes the crate makes subvolumes' UUIDs of: the system's.
fn random(out: &mut [u8]) {
    let read = std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(out));
    if read.is_err() {
        disk::Rng::system().fill(out);
    }
}

/// Gives the kernel's filesystem crates on the host [`clock`] and
/// [`random`], which read `time` (seconds since 1970), for the files they
/// make: the first time given is kept.
pub fn host_clock(time: u64) {
    let _ = TIME.set(time);
    radix_vfs::set_clock(clock);
    radix_vfs::set_random(random);
}

/// What a filesystem error says, for `what`.
pub fn failed(what: &str, err: radix_vfs::Error) -> Error {
    Error::new(format!("{what}: {}", disk::btrfs::describe(err)))
}

/// A tree of the staging folder that is an account's, not root's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Owned {
    /// Its path, relative to the staging folder: `@users/lab`.
    pub path: std::path::PathBuf,
    pub owner: u32,
    pub group: u32,
}

/// Makes a Btrfs labelled `label` in the file `output`, of `bytes`, at
/// `time` (seconds since 1970): the subvolumes `subvolumes` in its top
/// directory, each holding what the directory of its name in `staging`
/// holds, and the first made read-only. Returns its UUID, which the
/// boot menu of an installed system finds it by (ADR 0307). Files are
/// root's but those in the trees `owned` names.
///
/// # Errors
///
/// If `output` cannot be written, or `staging` read.
pub fn make(
    staging: &Path,
    output: &Path,
    bytes: u64,
    label: &str,
    subvolumes: &[&str],
    time: u64,
    owned: &[Owned],
) -> Result<[u8; 16]> {
    host_clock(time);
    let _ = std::fs::remove_file(output);
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(output)
        .context(|| format!("making {}", output.display()))?;
    file.set_len(bytes).context(|| format!("making {}", output.display()))?;
    let device = disk::Device::whole(file, bytes, 512);
    let mut rng = disk::Rng::system();
    let options = Options {
        fsid: disk::btrfs::new_uuid(&mut rng),
        dev_uuid: disk::btrfs::new_uuid(&mut rng),
        chunk_tree_uuid: disk::btrfs::new_uuid(&mut rng),
        top_uuid: disk::btrfs::new_uuid(&mut rng),
        ..Options::new(label.as_bytes(), time)
    };
    let shown = output.display().to_string();
    disk::btrfs::make(&device, &options).map_err(|why| Error::new(format!("{shown}: {why}")))?;
    let part = device.part(0, device.sectors()).context(|| format!("opening {shown}"))?;
    let fs = BtrfsFs::mount_writable(BlockFile::new(part, false).into_ref())
        .map_err(|err| failed(&shown, err))?;
    fs.set_compress(true);
    let top = radix_vfs::FileSystem::root(&*fs);
    for name in subvolumes {
        top.make_subvolume(name).map_err(|err| failed(&format!("making {name}"), err))?;
        let subvolume = top.lookup(name).map_err(|err| failed(name, err))?;
        copy_owned(&staging.join(name), &subvolume, Path::new(name), owned)?;
    }
    if let Some(first) = subvolumes.first() {
        let node = top.lookup(first).map_err(|err| failed(first, err))?;
        let btrfs = radix_vfs::downcast::<BtrfsNode>(&*node)
            .ok_or_else(|| Error::new(format!("{first} is not a Btrfs directory")))?;
        btrfs.make_read_only().map_err(|err| failed(&format!("making {first} read-only"), err))?;
    }
    radix_vfs::FileSystem::sync(&*fs).map_err(|err| failed(&format!("writing {shown}"), err))?;
    Ok(options.fsid)
}

/// The access of `meta` for the file at `path`, relative to the staging
/// folder: root's unless it is in a tree `owned` names.
fn access_of(meta: &std::fs::Metadata, path: &Path, owned: &[Owned]) -> Access {
    let (owner, group) = owned
        .iter()
        .find(|tree| path.starts_with(&tree.path))
        .map_or((0, 0), |tree| (tree.owner, tree.group));
    Access { owner, group, mode: (meta.permissions().mode() & 0o7777) as u16 }
}

/// The nanoseconds since 1970 of `meta`'s last change of contents.
fn modified(meta: &std::fs::Metadata) -> u64 {
    let (secs, nanos) = (meta.mtime().max(0) as u64, meta.mtime_nsec().max(0) as u64);
    secs.saturating_mul(1_000_000_000).saturating_add(nanos)
}

/// Writes what the directory `from` holds into `to`, in the order of its
/// names, so the same tree is the same filesystem: the Dukovo root's
/// Btrfs, or an image's EFI system partition (FAT). Every file is root's.
pub fn copy(from: &Path, to: &NodeRef) -> Result<()> {
    copy_owned(from, to, Path::new(""), &[])
}

/// As [`copy`], `from` being `at` in the staging folder, with the trees
/// `owned` names given to their accounts.
fn copy_owned(from: &Path, to: &NodeRef, at: &Path, owned: &[Owned]) -> Result<()> {
    let mut names: Vec<_> = std::fs::read_dir(from)
        .context(|| format!("reading {}", from.display()))?
        .map(|entry| entry.map(|e| e.file_name()))
        .collect::<std::io::Result<_>>()
        .context(|| format!("reading {}", from.display()))?;
    names.sort();
    for name in names {
        let path = from.join(&name);
        let staged = at.join(&name);
        let name = name
            .to_str()
            .ok_or_else(|| Error::new(format!("the name of {} is not UTF-8", path.display())))?;
        let shown = path.display().to_string();
        let meta = std::fs::symlink_metadata(&path).context(|| format!("reading {shown}"))?;
        let kind = meta.file_type();
        if kind.is_symlink() {
            let target = std::fs::read_link(&path).context(|| format!("reading {shown}"))?;
            let target = target
                .to_str()
                .ok_or_else(|| Error::new(format!("the link {shown} is not UTF-8")))?;
            let link = Access::default_for(Kind::Symlink);
            to.make_link(name, target, link).map_err(|err| failed(&shown, err))?;
        } else if kind.is_dir() {
            let dir = to
                .create(name, Kind::Directory, access_of(&meta, &staged, owned))
                .map_err(|err| failed(&shown, err))?;
            copy_owned(&path, &dir, &staged, owned)?;
            dir.set_times(None, Some(modified(&meta))).map_err(|err| failed(&shown, err))?;
        } else if kind.is_file() {
            let data = std::fs::read(&path).context(|| format!("reading {shown}"))?;
            let access = access_of(&meta, &staged, owned);
            let file = to.create(name, Kind::File, access).map_err(|err| failed(&shown, err))?;
            let written = file.write(0, &Slice(&data)).map_err(|err| failed(&shown, err))?;
            if written != data.len() {
                return Err(Error::new(format!(
                    "{shown}: {written} of {} bytes written",
                    data.len()
                )));
            }
            let when = modified(&meta);
            file.set_times(Some(when), Some(when)).map_err(|err| failed(&shown, err))?;
        } else {
            return Err(Error::new(format!("{shown} is neither a file, a directory nor a link")));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::process::{Command, Stdio};

    use radix_vfs::{FileSystem as _, SliceMut};

    use super::*;

    /// The `btrfs` of btrfs-progs that `ob test` points `RADIX_BTRFS_PROGS`
    /// at, or the system's, if there is one.
    fn btrfs_tool() -> Option<PathBuf> {
        let pinned = std::env::var_os("RADIX_BTRFS_PROGS")
            .map(|sbin| PathBuf::from(sbin).join("../bin/btrfs"));
        pinned
            .into_iter()
            .chain(["/usr/bin/btrfs", "/bin/btrfs", "/usr/sbin/btrfs"].map(PathBuf::from))
            .find(|p| p.exists())
    }

    fn read_all(node: &NodeRef) -> Vec<u8> {
        let mut out = vec![0u8; node.stat().size as usize];
        let n = node.read(0, &mut SliceMut(&mut out)).unwrap();
        out.truncate(n);
        out
    }

    #[test]
    fn the_root_holds_the_staging_tree_whole_and_roots() {
        let dir = std::env::temp_dir().join(format!("ob-rootfs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let staging = dir.join("staging");
        let deep = staging.join("@a/commands/deep");
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::create_dir_all(staging.join("@b")).unwrap();
        // 4 MiB of text, which zstd makes a few KiB of.
        let text = "every file is root's\n".repeat(200_000);
        std::fs::write(staging.join("@a/commands/text"), &text).unwrap();
        let tool = staging.join("@a/commands/tool");
        std::fs::write(&tool, b"\x7fELF and the rest").unwrap();
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::os::unix::fs::symlink("../tool", deep.join("link")).unwrap();
        std::fs::write(staging.join("@b/passwd"), "root:x:0:0::/:/bin/sh\n").unwrap();

        let output = dir.join("root.btrfs");
        std::fs::create_dir_all(staging.join("@b/home/.ssh")).unwrap();
        std::fs::write(staging.join("@b/home/.ssh/authorized_keys"), "ssh-ed25519 AAAA lab\n")
            .unwrap();
        let owned = [Owned { path: "@b/home".into(), owner: 1001, group: 1002 }];
        make(&staging, &output, 160 << 20, "test-root", &["@a", "@b"], 1_793_592_777, &owned)
            .unwrap();

        if let Some(btrfs) = btrfs_tool() {
            let out = Command::new(btrfs)
                .args(["check", "--check-data-csum"])
                .arg(&output)
                .stdin(Stdio::null())
                .output()
                .unwrap();
            assert!(out.status.success(), "btrfs check:\n{}", String::from_utf8_lossy(&out.stderr));
        }
        let file = std::fs::OpenOptions::new().read(true).open(&output).unwrap();
        let device = disk::Device::whole(file, 160 << 20, 512);
        let checking = disk::btrfs::Checking::open(&device).unwrap();
        assert_eq!(checking.label().as_deref(), Some("test-root"));
        let report = checking.check(true, &mut |_, _| {}).unwrap();
        assert!(report.problems.is_empty(), "{:#?}", report.problems);
        assert_eq!(report.subvolumes, 3);
        assert!(report.bytes_used < 1 << 20, "not compressed: {} bytes used", report.bytes_used);

        let fs = BtrfsFs::mount(BlockFile::new(device, true).into_ref()).unwrap();
        let top = fs.root();
        let a = top.lookup("@a").unwrap();
        assert_eq!(
            a.create("new", Kind::File, Access::default_for(Kind::File)).err(),
            Some(radix_vfs::Error::ReadOnly)
        );
        let commands = a.lookup("commands").unwrap();
        let text_node = commands.lookup("text").unwrap();
        assert_eq!(read_all(&text_node), text.as_bytes());
        let tool_node = commands.lookup("tool").unwrap();
        assert_eq!(tool_node.access(), Access { owner: 0, group: 0, mode: 0o755 });
        assert_eq!(read_all(&tool_node), b"\x7fELF and the rest");
        let link = commands.lookup("deep").unwrap().lookup("link").unwrap();
        assert_eq!(link.kind(), Kind::Symlink);
        let passwd = top.lookup("@b").unwrap().lookup("passwd").unwrap();
        assert_eq!(passwd.access().owner, 0);
        assert!(passwd.create("x", Kind::File, Access::default_for(Kind::File)).is_err());
        // The owned tree is its account's, the folder and what is in it.
        let home = top.lookup("@b").unwrap().lookup("home").unwrap();
        let keys = home.lookup(".ssh").unwrap().lookup("authorized_keys").unwrap();
        for node in [&home, &keys] {
            assert_eq!((node.access().owner, node.access().group), (1001, 1002));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}

// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The client against nfsd(8), served in the tests' process from a
//! folder of their own: files made, written, read back and committed,
//! folders listed, names renamed and linked, attributes set, errors as
//! errnos, and opens made again after the server lost the client.

use std::ffi::OsStr;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use fuser::FileType;

use crate::attrs::{Set, When};
use crate::fs::{Made, NfsFs, ROOT};
use crate::options::{Options, Target};
use crate::rpc::Cred;

/// A fresh folder for a test.
fn tempdir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("nfs-test-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// A test's mount: the filesystem, the shared folder, and who calls.
struct Mounted {
    fs: NfsFs,
    dir: PathBuf,
    cred: Cred,
}

/// nfsd sharing a fresh folder as Music, mounted with `options`.
fn mounted_with(name: &str, add: &str) -> Mounted {
    let dir = tempdir(name);
    let addr = nfsd::testing::serve(&[("Music", dir.clone())]).unwrap();
    let mut options = Options::default();
    options.add(&format!("soft,timeo=50,port={}", addr.port())).unwrap();
    options.add(add).unwrap();
    let target = Target::parse("127.0.0.1:/Music").unwrap();
    // SAFETY: getuid and getgid only read the process's IDs.
    let (uid, gid) = unsafe { (libc::getuid(), libc::getgid()) };
    let cred = Cred { uid, gid, gids: Vec::new() };
    let (client, root) = crate::connect(&target, &options, &cred).unwrap();
    let fs = NfsFs::new(Arc::new(Mutex::new(client)), &root, options.read_only);
    Mounted { fs, dir, cred }
}

fn mounted(name: &str) -> Mounted {
    mounted_with(name, "")
}

fn name(text: &str) -> &OsStr {
    OsStr::new(text)
}

#[test]
fn files_are_made_written_and_read_back() {
    let Mounted { mut fs, dir, cred } = mounted("files");
    let (attr, handle) =
        fs.create(&cred, ROOT, name("song.txt"), 0o640, libc::O_WRONLY | libc::O_EXCL).unwrap();
    assert_eq!((attr.kind, attr.perm, attr.size), (FileType::RegularFile, 0o640, 0));
    assert_eq!(fs.write(handle, 0, b"hello, ").unwrap(), 7);
    assert_eq!(fs.write(handle, 7, b"world").unwrap(), 5);
    fs.commit(handle).unwrap();
    fs.release(handle).unwrap();
    assert_eq!(fs::read(dir.join("song.txt")).unwrap(), b"hello, world");

    let found = fs.look_up(&cred, ROOT, name("song.txt")).unwrap();
    assert_eq!((found.ino, found.size), (attr.ino, 12));
    let handle = fs.open(&cred, found.ino, libc::O_RDONLY).unwrap();
    assert_eq!(fs.read(handle, 0, 100).unwrap(), b"hello, world");
    assert_eq!(fs.read(handle, 7, 3).unwrap(), b"wor");
    assert_eq!(fs.read(handle, 50, 10).unwrap(), b"");
    fs.release(handle).unwrap();

    // Made again, exclusively: it is there.
    let again = fs.create(&cred, ROOT, name("song.txt"), 0o640, libc::O_WRONLY | libc::O_EXCL);
    assert_eq!(again.err(), Some(libc::EEXIST));
    // Made again, cut short.
    let (attr, handle) =
        fs.create(&cred, ROOT, name("song.txt"), 0o640, libc::O_WRONLY | libc::O_TRUNC).unwrap();
    assert_eq!(attr.size, 0);
    fs.release(handle).unwrap();
    assert_eq!(fs.look_up(&cred, ROOT, name("gone")).err(), Some(libc::ENOENT));
}

#[test]
fn writes_larger_than_the_server_takes_are_split() {
    let Mounted { mut fs, dir, cred } = mounted_with("split", "wsize=4096,rsize=4096");
    let data: Vec<u8> = (0..20_000u32).map(|i| (i % 251) as u8).collect();
    let (_, handle) = fs.create(&cred, ROOT, name("big"), 0o600, libc::O_RDWR).unwrap();
    assert_eq!(fs.write(handle, 0, &data).unwrap(), 20_000);
    assert_eq!(fs.read(handle, 0, 20_000).unwrap(), data);
    fs.release(handle).unwrap();
    assert_eq!(fs::read(dir.join("big")).unwrap(), data);
}

#[test]
fn folders_names_and_links() {
    // Pages of listings 4 KiB long, which 300 names fill several of.
    let Mounted { mut fs, dir, cred } = mounted_with("names", "rsize=4096");
    let folder = fs.make(&cred, ROOT, name("album"), Made::Folder, 0o750).unwrap();
    assert_eq!((folder.kind, folder.perm), (FileType::Directory, 0o750));
    let (file, handle) = fs.create(&cred, folder.ino, name("a"), 0o644, libc::O_WRONLY).unwrap();
    fs.write(handle, 0, b"track").unwrap();
    fs.release(handle).unwrap();

    let link = fs.make(&cred, ROOT, name("link"), Made::Link(b"album/a"), 0o777).unwrap();
    assert_eq!(link.kind, FileType::Symlink);
    assert_eq!(fs.read_link(&cred, link.ino).unwrap(), b"album/a");
    let hard = fs.link(&cred, file.ino, ROOT, name("hard")).unwrap();
    assert_eq!((hard.ino, hard.nlink), (file.ino, 2));

    fs.rename(&cred, folder.ino, name("a"), ROOT, name("b")).unwrap();
    assert_eq!(fs::read(dir.join("b")).unwrap(), b"track");
    // The node is the file's still, by its handle.
    assert_eq!(fs.attributes(&cred, file.ino).unwrap().size, 5);

    let listing = fs.list(&cred, ROOT).unwrap();
    let mut names = fs.names(listing).unwrap();
    names.sort_by(|a, b| a.0.cmp(&b.0));
    let names: Vec<(&str, FileType)> =
        names.iter().map(|(n, k)| (n.to_str().unwrap(), *k)).collect();
    assert_eq!(
        names,
        [
            ("album", FileType::Directory),
            ("b", FileType::RegularFile),
            ("hard", FileType::RegularFile),
            ("link", FileType::Symlink)
        ]
    );

    fs.remove(&cred, ROOT, name("hard")).unwrap();
    let full = fs.make(&cred, folder.ino, name("inner"), Made::Folder, 0o700).unwrap();
    assert_eq!(fs.remove(&cred, ROOT, name("album")).err(), Some(libc::ENOTEMPTY));
    fs.remove(&cred, folder.ino, name("inner")).unwrap();
    fs.let_go(full.ino, 1);
    fs.remove(&cred, ROOT, name("album")).unwrap();
    assert!(!dir.join("album").exists());
    // A long listing, of several pages.
    for i in 0..300 {
        fs.make(&cred, ROOT, name(&format!("folder-with-a-long-name-{i:04}")), Made::Folder, 0o700)
            .unwrap();
    }
    let listing = fs.list(&cred, ROOT).unwrap();
    assert_eq!(fs.names(listing).unwrap().len(), 302);
}

#[test]
fn attributes_are_set() {
    let Mounted { mut fs, dir, cred } = mounted("attributes");
    let (file, handle) = fs.create(&cred, ROOT, name("f"), 0o644, libc::O_RDWR).unwrap();
    fs.write(handle, 0, b"0123456789").unwrap();
    let cut = Set { size: Some(4), ..Set::default() };
    assert_eq!(fs.set(&cred, file.ino, &cut, Some(handle)).unwrap().size, 4);
    fs.release(handle).unwrap();
    let at = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000_000);
    let set = Set { mode: Some(0o600), mtime: Some(When::At(at)), ..Set::default() };
    let after = fs.set(&cred, file.ino, &set, None).unwrap();
    assert_eq!((after.perm, after.mtime, after.size), (0o600, at, 4));
    // Without an open: the anonymous stateid.
    let grow = Set { size: Some(8), ..Set::default() };
    assert_eq!(fs.set(&cred, file.ino, &grow, None).unwrap().size, 8);
    assert_eq!(fs::read(dir.join("f")).unwrap(), b"0123\0\0\0\0");
    let space = fs.space(&cred).unwrap();
    assert!(space.blocks > 0 && space.max_name > 0, "{space:?}");
}

#[test]
fn opens_are_made_again_after_the_server_lost_the_client() {
    let Mounted { mut fs, dir, cred } = mounted("lost");
    let (_, handle) = fs.create(&cred, ROOT, name("f"), 0o644, libc::O_RDWR).unwrap();
    fs.write(handle, 0, b"before").unwrap();
    // As a client that restarted: a new verifier, so the server drops the
    // old client's state, the open among it.
    fs.forget_client([9; 8]);
    fs.write(handle, 6, b" and after").unwrap();
    assert_eq!(fs.read(handle, 0, 64).unwrap(), b"before and after");
    fs.release(handle).unwrap();
    assert_eq!(fs::read(dir.join("f")).unwrap(), b"before and after");
}

#[test]
fn read_only_mounts_refuse_changes() {
    let Mounted { mut fs, cred, .. } = mounted_with("ro", "ro");
    assert_eq!(fs.make(&cred, ROOT, name("x"), Made::Folder, 0o700).err(), Some(libc::EROFS));
    assert_eq!(fs.open(&cred, ROOT, libc::O_WRONLY).err(), Some(libc::EROFS));
}

// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The release copied into the new system slot, every file checked
//! against the signed list as it is copied: its size and SHA-256 are
//! counted from the bytes written, so that what lands on the disk is what
//! the release's signers listed, whatever the stick holds (ADR 0308).

use std::fs::{self, File};
use std::io::{Read as _, Write as _};
use std::os::unix::fs::{PermissionsExt as _, symlink};
use std::path::Path;

use ring::digest::{Context, SHA256};
use system_release::list::{Kind, Release, hex};
use system_release::store::Store;

/// What was copied.
#[derive(Debug, Default)]
pub struct Copied {
    /// Files.
    pub files: usize,
    /// Their bytes.
    pub bytes: u64,
}

/// Copies every directory, file and link of `release` from the store in
/// the repository `repo` into `slot`, an empty directory. Directories get
/// their modes last, so that one without write access is filled first.
///
/// # Errors
///
/// The first file that is missing or not what the list says, by its path
/// in the release, or a write that failed.
pub fn release(repo: &Path, release: &Release, slot: &Path) -> Result<Copied, String> {
    let store = Store::at(&repo.join("files"));
    let mut copied = Copied::default();
    let mut directories = Vec::new();
    for entry in &release.files {
        let to = slot.join(&entry.path);
        let shown = &entry.path;
        match entry.kind {
            Kind::Directory => {
                fs::create_dir(&to).map_err(|err| format!("{shown}: making it: {err}"))?;
                directories.push((to, entry.mode_bits()));
            }
            Kind::File => {
                let (sha256, size) =
                    entry.file().ok_or_else(|| format!("{shown}: listed without its hash"))?;
                copy_checked(&store.path(sha256), &to, sha256, size)
                    .map_err(|why| format!("{shown}: {why}"))?;
                set_mode(&to, entry.mode_bits()).map_err(|why| format!("{shown}: {why}"))?;
                copied.files += 1;
                copied.bytes += size;
            }
            Kind::Link => {
                let target = entry
                    .target
                    .as_deref()
                    .ok_or_else(|| format!("{shown}: listed without its target"))?;
                symlink(target, &to).map_err(|err| format!("{shown}: making the link: {err}"))?;
            }
        }
    }
    for (directory, mode) in directories.iter().rev() {
        set_mode(directory, *mode)?;
    }
    Ok(copied)
}

/// Copies the file `from` to `to`, a new file, and checks that what was
/// written is `size` bytes whose SHA-256 is `sha256`. A file that is not
/// is removed again.
fn copy_checked(from: &Path, to: &Path, sha256: &str, size: u64) -> Result<(), String> {
    let mut input = File::open(from).map_err(|err| format!("reading it from the stick: {err}"))?;
    let mut output = File::create_new(to).map_err(|err| format!("writing it: {err}"))?;
    let mut hash = Context::new(&SHA256);
    let mut buffer = vec![0u8; 256 * 1024];
    let mut written = 0u64;
    loop {
        let n =
            input.read(&mut buffer).map_err(|err| format!("reading it from the stick: {err}"))?;
        if n == 0 {
            break;
        }
        written += n as u64;
        // A file longer than listed is refused before more is written.
        if written > size {
            let _ = fs::remove_file(to);
            return Err(format!(
                "the stick's copy is longer than the {size} bytes the signed list gives"
            ));
        }
        hash.update(&buffer[..n]);
        output.write_all(&buffer[..n]).map_err(|err| format!("writing it: {err}"))?;
    }
    let got = hex(hash.finish().as_ref());
    if written != size || got != sha256 {
        let _ = fs::remove_file(to);
        return Err(format!(
            "the stick's copy is {written} bytes with SHA-256 {got}, not the signed list's {size} bytes \
             with {sha256}: the stick is damaged; write it again"
        ));
    }
    Ok(())
}

/// Sets the mode of `path`.
fn set_mode(path: &Path, mode: u32) -> Result<(), String> {
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .map_err(|err| format!("setting the mode of {}: {err}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A store holding one file, and a release listing it under `bin`.
    fn repo(dir: &Path, content: &[u8]) -> Release {
        let sha256 = hex(ring::digest::digest(&SHA256, content).as_ref());
        fs::create_dir_all(dir.join("files")).unwrap();
        fs::write(dir.join("files").join(&sha256), content).unwrap();
        let list = format!(
            r#"{{"format":1,"version":"0.14.0","commit":"c","toolchain":"t","built":"2026-11-06T00:00:00Z",
            "updates_from":"0.14.0","files":[
            {{"path":"bin","kind":"directory","mode":"0555"}},
            {{"path":"bin/tool","kind":"file","mode":"0755","size":{},"sha256":"{sha256}"}},
            {{"path":"tool","kind":"link","mode":"0777","target":"bin/tool"}}]}}"#,
            content.len()
        );
        Release::parse(list.as_bytes()).unwrap()
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("installer-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn copies_a_release_with_its_modes_and_links() {
        let dir = scratch("copy");
        let listed = repo(&dir.join("repo"), b"hello");
        fs::create_dir(dir.join("slot")).unwrap();
        let copied = release(&dir.join("repo"), &listed, &dir.join("slot")).unwrap();
        assert_eq!((copied.files, copied.bytes), (1, 5));
        assert_eq!(fs::read(dir.join("slot/tool")).unwrap(), b"hello");
        let mode = |p: &str| fs::metadata(dir.join(p)).unwrap().permissions().mode() & 0o7777;
        assert_eq!((mode("slot/bin"), mode("slot/bin/tool")), (0o555, 0o755));
        set_mode(&dir.join("slot/bin"), 0o755).unwrap();
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn refuses_a_changed_file_and_removes_it() {
        let dir = scratch("changed");
        let listed = repo(&dir.join("repo"), b"hello");
        let (sha256, _) = listed.files[1].file().unwrap();
        fs::write(dir.join("repo/files").join(sha256), b"hellO").unwrap();
        fs::create_dir(dir.join("slot")).unwrap();
        let why = release(&dir.join("repo"), &listed, &dir.join("slot")).unwrap_err();
        assert!(why.starts_with("bin/tool: the stick's copy is 5 bytes"), "{why}");
        assert!(!dir.join("slot/bin/tool").exists());
        fs::write(dir.join("repo/files").join(sha256), b"hello!").unwrap();
        fs::create_dir(dir.join("slot-2")).unwrap();
        let why = release(&dir.join("repo"), &listed, &dir.join("slot-2")).unwrap_err();
        assert!(why.contains("longer than"), "{why}");
        fs::remove_dir_all(&dir).unwrap();
    }
}

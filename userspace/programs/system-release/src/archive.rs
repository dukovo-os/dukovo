// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The update file (ADR 0466): a release's repository in one file, for a
//! machine with no network, or a mirror. It is a POSIX tar archive
//! (ustar), uncompressed, so that it is read in place, each file at its
//! offset, with nothing unpacked beside it, and so that Linux's `tar`
//! lists or unpacks it. Only folders and regular files are written or
//! read, every name relative and without `..`; anything else is
//! refused.
//!
//! Nothing in the archive is trusted for being there: its metadata is
//! read by the TUF client from the root the computer already has, and
//! every file is checked against the signed list as it is copied.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;

use crate::list::hex;

/// The size of a tar block.
const BLOCK: u64 = 512;

/// The longest name a header holds in its name field; a longer one is
/// split at a slash into the prefix field (155 bytes) and the name.
const NAME: usize = 100;
const PREFIX: usize = 155;

/// The most entries an archive may have: a release's files and its
/// metadata, many times over.
const MOST_ENTRIES: usize = 200_000;

/// Writes a tar archive, an entry at a time, every header the same but
/// for its name, size and mode, so that one repository always makes the
/// same bytes.
pub struct Writer<W: Write> {
    out: W,
    mtime: u64,
}

impl<W: Write> Writer<W> {
    /// An archive written to `out`, every entry dated `mtime`, in seconds
    /// since 1970.
    pub fn new(out: W, mtime: u64) -> Writer<W> {
        Writer { out, mtime }
    }

    /// Adds a folder.
    ///
    /// # Errors
    ///
    /// If the name does not fit a header, or the write fails.
    pub fn folder(&mut self, name: &str) -> Result<(), String> {
        let header =
            header(&format!("{}/", name.trim_end_matches('/')), 0, 0o755, b'5', self.mtime)?;
        self.out.write_all(&header).map_err(|err| err.to_string())
    }

    /// Adds a file `size` bytes long, read from `data`.
    ///
    /// # Errors
    ///
    /// If the name does not fit a header, `data` is not `size` bytes, or
    /// a read or write fails.
    pub fn file(&mut self, name: &str, size: u64, data: &mut dyn Read) -> Result<(), String> {
        let header = header(name, size, 0o644, b'0', self.mtime)?;
        self.out.write_all(&header).map_err(|err| err.to_string())?;
        let copied =
            io::copy(&mut data.take(size), &mut self.out).map_err(|err| err.to_string())?;
        if copied != size {
            return Err(format!("{name}: {copied} bytes, not {size}"));
        }
        let pad = padding(size);
        self.out.write_all(&vec![0; pad as usize]).map_err(|err| err.to_string())
    }

    /// Ends the archive with its two empty blocks.
    ///
    /// # Errors
    ///
    /// If the write fails.
    pub fn finish(mut self) -> Result<W, String> {
        self.out.write_all(&[0; 2 * BLOCK as usize]).map_err(|err| err.to_string())?;
        self.out.flush().map_err(|err| err.to_string())?;
        Ok(self.out)
    }
}

/// The bytes after a file's data to the end of its last block.
fn padding(size: u64) -> u64 {
    (BLOCK - size % BLOCK) % BLOCK
}

/// A ustar header for `name`.
fn header(name: &str, size: u64, mode: u32, kind: u8, mtime: u64) -> Result<[u8; 512], String> {
    let mut block = [0u8; 512];
    let (prefix, rest) = split_name(name)?;
    block[..rest.len()].copy_from_slice(rest.as_bytes());
    octal(&mut block[100..108], u64::from(mode));
    octal(&mut block[108..116], 0);
    octal(&mut block[116..124], 0);
    octal(&mut block[124..136], size);
    octal(&mut block[136..148], mtime);
    block[156] = kind;
    block[257..263].copy_from_slice(b"ustar\0");
    block[263..265].copy_from_slice(b"00");
    block[345..345 + prefix.len()].copy_from_slice(prefix.as_bytes());
    // The checksum is of the header with its own field as spaces.
    block[148..156].copy_from_slice(b"        ");
    let sum: u64 = block.iter().map(|&b| u64::from(b)).sum();
    octal(&mut block[148..155], sum);
    block[155] = b' ';
    Ok(block)
}

/// `name` as a header holds it: in the name field alone, or split at a
/// slash into the prefix and the name.
fn split_name(name: &str) -> Result<(&str, &str), String> {
    if name.len() <= NAME {
        return Ok(("", name));
    }
    for (at, _) in name.match_indices('/') {
        let (prefix, rest) = (&name[..at], &name[at + 1..]);
        if prefix.len() <= PREFIX && rest.len() <= NAME && !rest.is_empty() {
            return Ok((prefix, rest));
        }
    }
    Err(format!("{name}: too long a name for a tar header"))
}

/// Writes `value` in octal into `field`, zero-filled, ending in NUL.
fn octal(field: &mut [u8], value: u64) {
    let width = field.len() - 1;
    let digits = format!("{value:0width$o}");
    field[..width].copy_from_slice(&digits.as_bytes()[digits.len() - width..]);
    field[width] = 0;
}

/// Reads an octal field: digits, maybe after spaces, up to a NUL or a
/// space.
fn read_octal(field: &[u8]) -> Option<u64> {
    let text: Vec<u8> = field
        .iter()
        .copied()
        .skip_while(|&b| b == b' ')
        .take_while(|&b| b != 0 && b != b' ')
        .collect();
    if text.is_empty() {
        return Some(0);
    }
    let text = std::str::from_utf8(&text).ok()?;
    u64::from_str_radix(text, 8).ok()
}

/// A NUL-ended text field.
fn text(field: &[u8]) -> Result<&str, String> {
    let end = field.iter().position(|&b| b == 0).unwrap_or(field.len());
    std::str::from_utf8(&field[..end]).map_err(|_| "a name that is not UTF-8".to_string())
}

/// An archive opened for reading: where each file's bytes are in it.
pub struct Archive {
    file: File,
    /// Each regular file by its name: its data's offset and length.
    entries: BTreeMap<String, (u64, u64)>,
}

impl Archive {
    /// Opens the archive at `path` and reads its headers.
    ///
    /// # Errors
    ///
    /// If it cannot be read, or is not a ustar archive of folders and
    /// files with relative names, each once.
    pub fn open(path: &Path) -> Result<Archive, String> {
        let at = |why: String| format!("{}: {why}", path.display());
        let mut file = File::open(path).map_err(|err| at(err.to_string()))?;
        let length = file.metadata().map_err(|err| at(err.to_string()))?.len();
        let mut entries = BTreeMap::new();
        let mut offset = 0;
        loop {
            let mut block = [0u8; 512];
            file.seek(SeekFrom::Start(offset)).map_err(|err| at(err.to_string()))?;
            file.read_exact(&mut block)
                .map_err(|_| at("it ends before the archive's end".into()))?;
            offset += BLOCK;
            if block.iter().all(|&b| b == 0) {
                break;
            }
            if &block[257..262] != b"ustar" {
                return Err(at("it is not a tar archive (ustar)".into()));
            }
            let stored = read_octal(&block[148..156]).ok_or_else(|| at("a bad header".into()))?;
            let mut summed = block;
            summed[148..156].copy_from_slice(b"        ");
            if summed.iter().map(|&b| u64::from(b)).sum::<u64>() != stored {
                return Err(at("a header's checksum does not match: it is damaged".into()));
            }
            let size = read_octal(&block[124..136]).ok_or_else(|| at("a bad size".into()))?;
            let (prefix, name) =
                (text(&block[345..500]).map_err(at)?, text(&block[..100]).map_err(at)?);
            let name =
                if prefix.is_empty() { name.to_string() } else { format!("{prefix}/{name}") };
            let name = name.trim_end_matches('/').to_string();
            if name.is_empty()
                || name.starts_with('/')
                || name.split('/').any(|part| part.is_empty() || part == "." || part == "..")
            {
                return Err(at(format!("it holds {name:?}, which is not a name inside it")));
            }
            match block[156] {
                b'0' | 0 => {
                    if offset + size > length {
                        return Err(at(format!("{name} goes past its end: it is cut short")));
                    }
                    if entries.insert(name.clone(), (offset, size)).is_some() {
                        return Err(at(format!("it holds {name} twice")));
                    }
                    if entries.len() > MOST_ENTRIES {
                        return Err(at(format!("it holds more than {MOST_ENTRIES} files")));
                    }
                }
                b'5' => {}
                _ => return Err(at(format!("{name} is neither a file nor a folder"))),
            }
            offset += size + padding(size);
        }
        Ok(Archive { file, entries })
    }

    /// The names of its files, in order.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.entries.keys().map(String::as_str)
    }

    /// The length of the file `name`, if it holds one.
    #[must_use]
    pub fn size(&self, name: &str) -> Option<u64> {
        self.entries.get(name).map(|&(_, size)| size)
    }

    /// The file `name`'s bytes, to read once.
    ///
    /// # Errors
    ///
    /// If it holds no such file, or the seek fails.
    pub fn reader(&self, name: &str) -> Result<impl Read + '_, String> {
        let &(offset, size) = self.entries.get(name).ok_or(format!("it holds no {name}"))?;
        let mut file = &self.file;
        file.seek(SeekFrom::Start(offset)).map_err(|err| err.to_string())?;
        Ok(file.take(size))
    }

    /// Copies the file `name` to `to`.
    ///
    /// # Errors
    ///
    /// If it holds no such file, or a read or write fails.
    pub fn copy_to(&self, name: &str, to: &Path) -> Result<(), String> {
        let mut reader = self.reader(name)?;
        let mut out = File::create(to).map_err(|err| format!("{}: {err}", to.display()))?;
        io::copy(&mut reader, &mut out).map_err(|err| format!("{}: {err}", to.display()))?;
        Ok(())
    }

    /// The SHA-256 of the file `name`, in hex.
    ///
    /// # Errors
    ///
    /// If it holds no such file, or a read fails.
    pub fn sha256(&self, name: &str) -> Result<String, String> {
        let mut reader = self.reader(name)?;
        let mut context = ring::digest::Context::new(&ring::digest::SHA256);
        let mut buffer = vec![0; 256 * 1024];
        loop {
            let n = reader.read(&mut buffer).map_err(|err| format!("{name}: {err}"))?;
            if n == 0 {
                break;
            }
            context.update(&buffer[..n]);
        }
        Ok(hex(context.finish().as_ref()))
    }

    /// Writes every file under `folder` into `out`, by its name there.
    ///
    /// # Errors
    ///
    /// If a read or write fails.
    pub fn unpack(&self, folder: &str, out: &Path) -> Result<usize, String> {
        let mut count = 0;
        let prefix = format!("{folder}/");
        for name in self.entries.keys().filter(|name| name.starts_with(&prefix)) {
            let to = out.join(name);
            if let Some(parent) = to.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|err| format!("{}: {err}", parent.display()))?;
            }
            self.copy_to(name, &to)?;
            count += 1;
        }
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::list::tests::scratch;

    fn archive(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut writer = Writer::new(Vec::new(), 1_793_923_200);
        for (name, data) in entries {
            if data.is_empty() && name.ends_with('/') {
                writer.folder(name).unwrap();
            } else {
                writer.file(name, data.len() as u64, &mut &data[..]).unwrap();
            }
        }
        writer.finish().unwrap()
    }

    #[test]
    fn what_is_written_reads_back_and_linux_tar_agrees() {
        let dir = scratch("archive");
        let long = format!("targets/system/{}.0.15.0-test.year.json", "ab".repeat(32));
        let bytes = archive(&[
            ("metadata/", b""),
            ("metadata/1.root.json", b"{\"root\":1}"),
            (&long, &[7u8; 1300]),
            ("files/empty", b""),
        ]);
        assert_eq!(bytes.len() % 512, 0);
        let path = dir.join("a.update");
        std::fs::write(&path, &bytes).unwrap();
        let opened = Archive::open(&path).unwrap();
        let names: Vec<&str> = opened.names().collect();
        assert_eq!(names, ["files/empty", "metadata/1.root.json", long.as_str()]);
        assert_eq!(opened.size(&long), Some(1300));
        let mut data = Vec::new();
        opened.reader("metadata/1.root.json").unwrap().read_to_end(&mut data).unwrap();
        assert_eq!(data, b"{\"root\":1}");
        assert_eq!(
            opened.sha256("files/empty").unwrap(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(opened.unpack("metadata", &dir.join("out")).unwrap(), 1);
        assert_eq!(std::fs::read(dir.join("out/metadata/1.root.json")).unwrap(), b"{\"root\":1}");
        // The same names from Linux's tar, where the host has it.
        if let Ok(listed) = std::process::Command::new("tar").arg("tf").arg(&path).output() {
            let text = String::from_utf8_lossy(&listed.stdout).into_owned();
            assert!(text.contains(&long), "{text}");
            assert!(text.contains("metadata/1.root.json"), "{text}");
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn damaged_or_strange_archives_are_refused() {
        let dir = scratch("archive-bad");
        let good = archive(&[("files/a", b"aaaa")]);
        let path = dir.join("a.update");
        let refused = |bytes: &[u8]| {
            std::fs::write(&path, bytes).unwrap();
            Archive::open(&path).err().unwrap()
        };
        let mut flipped = good.clone();
        flipped[0] ^= 1;
        assert!(refused(&flipped).contains("checksum"));
        assert!(refused(&good[..514]).contains("cut short"));
        assert!(refused(&good[..1024]).contains("ends before"));
        assert!(refused(&archive(&[("../a", b"x")])).contains("not a name inside it"));
        assert!(refused(&archive(&[("/a", b"x")])).contains("not a name inside it"));
        assert!(refused(&archive(&[("a", b"x"), ("a", b"y")])).contains("twice"));
        let mut link = archive(&[("a", b"")]);
        link[156] = b'2';
        let sum: u64 = {
            let mut block = [0u8; 512];
            block.copy_from_slice(&link[..512]);
            block[148..156].copy_from_slice(b"        ");
            block.iter().map(|&b| u64::from(b)).sum()
        };
        octal(&mut link[148..155], sum);
        assert!(refused(&link).contains("neither a file nor a folder"));
        assert!(refused(&[1u8; 1024]).contains("not a tar archive"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn names_too_long_for_a_header_are_refused() {
        assert_eq!(split_name("a/b").unwrap(), ("", "a/b"));
        let long = format!("{}/{}", "p".repeat(150), "n".repeat(99));
        assert_eq!(split_name(&long).unwrap(), ("p".repeat(150).as_str(), "n".repeat(99).as_str()));
        assert!(split_name(&"x".repeat(101)).is_err());
    }
}

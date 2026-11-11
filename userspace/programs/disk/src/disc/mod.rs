// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Disc images made of a folder (ADR 0449), for mkiso(1) and burn(1): ISO
//! 9660 with Rock Ridge's POSIX names, modes, owners, times and symbolic
//! links, Joliet's Unicode names for Windows, and a UDF 2.01 bridge for
//! DVDs and Blu-ray discs. The three trees point at one copy of each
//! file's bytes.
//!
//! Written from ECMA-119 (the free text of ISO 9660), Microsoft's Joliet
//! specification, IEEE P1281 and P1282 (SUSP and RRIP 1.10), and ECMA-167
//! 3rd edition with OSTA's UDF 2.01, and checked against the kernel's own
//! readers (radix-iso9660 and radix-udf) and against isoinfo and udfinfo.
//! xorriso and libisofs, with which Linux makes discs, are GPL-licensed
//! and were not used.
//!
//! Start at [`Node::scan`], which reads a folder into a tree. [`Image::new`]
//! lays the tree out, sector by sector, and builds everything but the
//! files' bytes; [`Image::write`] writes the image, reading each file as it
//! comes, so that a disc is burned from a folder with no image on a disk
//! first. [`iso`], [`rock`] and [`udf`] build each format's structures.
//!
//! What a disc does not hold is left out and said: sockets, pipes and
//! devices. A directory deeper than ISO 9660's eight levels is written
//! where it is, without Rock Ridge's relocation, as `mkisofs -D` does;
//! every reader that reads Rock Ridge or Joliet reads it.

pub mod iso;
pub mod rock;
pub mod udf;

#[cfg(test)]
mod tests;

use std::collections::{HashMap, VecDeque};
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// A disc's sector: 2,048 bytes of data, on a CD, a DVD and a Blu-ray
/// disc alike.
pub const SECTOR: u64 = 2048;
/// The zeros after the last file: 150 sectors, two seconds of a CD, which
/// a CD drive that reads ahead past the end of a track written at once
/// would otherwise fail on (mkisofs pads the same).
pub const PADDING: u64 = 150;
/// Where UDF's first anchor is, and so where its partition starts after.
const ANCHOR: u64 = 256;

/// Where a file's bytes come from.
#[derive(Clone, Debug)]
pub enum Source {
    /// A file, read as the image is written.
    Path(PathBuf),
    /// Bytes in memory.
    Bytes(Arc<[u8]>),
}

/// What a node is.
#[derive(Clone, Debug)]
pub enum Kind {
    /// A file.
    File {
        /// Its size in bytes.
        size: u64,
        /// Where its bytes come from.
        source: Source,
        /// The device and inode of a file with several names, which share
        /// one copy of its bytes on the disc.
        link: Option<(u64, u64)>,
    },
    /// A directory and what it holds.
    Dir(Vec<Node>),
    /// A symbolic link to the path it holds.
    Symlink(Vec<u8>),
}

/// A file, directory or symbolic link to put on a disc.
#[derive(Clone, Debug)]
pub struct Node {
    /// Its name, as bytes: UTF-8 is expected, as Joliet and UDF name it in
    /// Unicode. The root's is empty.
    pub name: Vec<u8>,
    /// What it is.
    pub kind: Kind,
    /// Its permission bits (`0o7777` at most).
    pub mode: u32,
    /// Its owner's number.
    pub uid: u32,
    /// Its group's number.
    pub gid: u32,
    /// When it was last modified, in seconds since 1970.
    pub modified: i64,
    /// When it was last read.
    pub accessed: i64,
    /// When its status last changed.
    pub changed: i64,
}

impl Node {
    /// A node of `kind` named `name`, owned by root, as a disc made of
    /// nothing but memory has it: mode 0o644 (0o755 for a directory),
    /// times `time`.
    pub fn new(name: &str, kind: Kind, time: i64) -> Node {
        let mode = if matches!(kind, Kind::Dir(_)) { 0o755 } else { 0o644 };
        let name = name.as_bytes().to_vec();
        Node { name, kind, mode, uid: 0, gid: 0, modified: time, accessed: time, changed: time }
    }

    /// Reads the folder `path` into a tree whose root it is: its files,
    /// folders and symbolic links, each folder's in the order of their
    /// names. Returns the tree, and the paths of what was left out:
    /// sockets, pipes and devices, which a disc does not hold.
    pub fn scan(path: &Path) -> io::Result<(Node, Vec<PathBuf>)> {
        let mut left = Vec::new();
        let meta = std::fs::metadata(path)?;
        let not_folder = || io::Error::new(io::ErrorKind::InvalidInput, "not a folder");
        if !meta.is_dir() {
            return Err(not_folder());
        }
        let mut root = scan_one(path, &meta, &mut left)?.ok_or_else(not_folder)?;
        root.name.clear();
        Ok((root, left))
    }
}

/// The node of `path`, whose metadata is `meta` (not followed through a
/// link), or `None` for one a disc does not hold, put in `left`.
fn scan_one(
    path: &Path,
    meta: &std::fs::Metadata,
    left: &mut Vec<PathBuf>,
) -> io::Result<Option<Node>> {
    let kind = meta.file_type();
    let kind = if kind.is_dir() {
        let mut names: Vec<_> =
            std::fs::read_dir(path)?.map(|e| e.map(|e| e.path())).collect::<io::Result<_>>()?;
        names.sort();
        let mut children = Vec::with_capacity(names.len());
        for child in names {
            let meta = std::fs::symlink_metadata(&child)?;
            if let Some(node) = scan_one(&child, &meta, left)? {
                children.push(node);
            }
        }
        Kind::Dir(children)
    } else if kind.is_file() {
        let link = (meta.nlink() > 1).then(|| (meta.dev(), meta.ino()));
        Kind::File { size: meta.len(), source: Source::Path(path.to_path_buf()), link }
    } else if kind.is_symlink() {
        use std::os::unix::ffi::OsStrExt;
        Kind::Symlink(std::fs::read_link(path)?.as_os_str().as_bytes().to_vec())
    } else {
        left.push(path.to_path_buf());
        return Ok(None);
    };
    use std::os::unix::ffi::OsStrExt;
    let name = path.file_name().map_or_else(Vec::new, |n| n.as_bytes().to_vec());
    Ok(Some(Node {
        name,
        kind,
        mode: meta.mode() & 0o7777,
        uid: meta.uid(),
        gid: meta.gid(),
        modified: meta.mtime(),
        accessed: meta.atime(),
        changed: meta.ctime(),
    }))
}

/// How an image is made.
#[derive(Clone, Debug)]
pub struct Options {
    /// The volume's label, as given: ISO 9660's own is in upper case and
    /// at most 32 characters, Joliet's and UDF's as given, cut to 16 and
    /// 30.
    pub label: String,
    /// Rock Ridge's POSIX names, modes, owners, times and links.
    pub rock: bool,
    /// Joliet's Unicode names, for Windows.
    pub joliet: bool,
    /// A UDF 2.01 tree beside ISO 9660's, for DVDs and Blu-ray discs.
    pub udf: bool,
    /// [`PADDING`] sectors of zeros at the end.
    pub pad: bool,
    /// When the image was made, in seconds since 1970.
    pub time: i64,
}

impl Options {
    /// Rock Ridge and Joliet, padded, without UDF, labelled `label` and
    /// made at `time`.
    pub fn new(label: &str, time: i64) -> Options {
        Options { label: label.into(), rock: true, joliet: true, udf: false, pad: true, time }
    }
}

/// Why an image cannot be made.
#[derive(Debug)]
pub enum Error {
    /// A name, a link's path or a file too long for the formats.
    TooLong(String),
    /// The image would be larger than a disc's 2^32 sectors.
    TooLarge,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::TooLong(what) => write!(f, "{what} is too long for a disc"),
            Error::TooLarge => f.write_str("the image would hold more than 2^32 sectors"),
        }
    }
}

impl std::error::Error for Error {}

/// A node as the image lays it out.
#[derive(Debug)]
pub(crate) struct Entry {
    pub name: Vec<u8>,
    pub kind: EntryKind,
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub modified: i64,
    pub accessed: i64,
    pub changed: i64,
    /// Its directory's entry; the root's is itself.
    pub parent: usize,
    /// How many names it has: for a directory, 2 and one for each
    /// directory in it; for a file, its names on the disc.
    pub links: u32,
    /// Its ISO 9660 name (`NAME.EXT;1`, or `NAME` for a directory), and
    /// its Joliet name, in UCS-2.
    pub iso: Vec<u8>,
    pub joliet: Vec<u16>,
    /// UDF's unique ID of it: 0 for the root, then 16 on.
    pub unique: u64,
}

#[derive(Debug)]
pub(crate) enum EntryKind {
    /// A file: its size and source, and its data's index in
    /// [`Image::data`].
    File {
        size: u64,
        data: usize,
    },
    /// A directory: its entries, in ISO 9660's order and in Joliet's.
    Dir {
        iso: Vec<usize>,
        joliet: Vec<usize>,
    },
    Symlink(Vec<u8>),
}

/// Where each of the image's parts is, in sectors.
#[derive(Debug, Default)]
pub(crate) struct Layout {
    pub pvd: u64,
    pub svd: Option<u64>,
    pub terminator: u64,
    /// UDF's recognition sequence, its main and reserve volume descriptor
    /// sequences, its integrity sequence, and its partition's start.
    pub vrs: u64,
    pub main_vds: u64,
    pub reserve_vds: u64,
    pub integrity: u64,
    pub partition: u64,
    /// The path tables, little- and big-endian, of the primary tree and
    /// of Joliet's; each table's size in bytes.
    pub iso_tables: (u64, u64, u64),
    pub joliet_tables: (u64, u64, u64),
    /// Each directory's extent and size in sectors, by entry; and the
    /// directories in the path tables' order.
    pub iso_dirs: Vec<(u64, u64)>,
    pub joliet_dirs: Vec<(u64, u64)>,
    pub iso_order: Vec<usize>,
    pub joliet_order: Vec<usize>,
    /// Rock Ridge's continuation areas.
    pub continuation: (u64, u64),
    /// UDF's file set descriptor, each entry's file entry, and each
    /// directory's identifiers' extent and size in bytes.
    pub fsd: u64,
    pub file_entries: Vec<u64>,
    pub udf_dirs: Vec<(u64, u64)>,
    /// The first sector of the files' bytes, and the sector after them.
    pub data: (u64, u64),
    /// The image's sectors.
    pub total: u64,
}

/// A file's bytes as the image holds them.
#[derive(Debug)]
pub(crate) struct Data {
    pub source: Source,
    pub size: u64,
    /// Its first sector.
    pub at: u64,
}

/// A disc image of a tree, laid out: [`Image::write`] writes it.
#[derive(Debug)]
pub struct Image {
    pub(crate) options: Options,
    pub(crate) entries: Vec<Entry>,
    pub(crate) data: Vec<Data>,
    pub(crate) layout: Layout,
    /// Every sector before the files' bytes.
    head: Vec<u8>,
    /// Every sector after them.
    tail: Vec<u8>,
}

impl Image {
    /// Lays `root` out as `options` say, and builds the image's sectors but
    /// the files' bytes.
    pub fn new(root: Node, options: Options) -> Result<Image, Error> {
        let (entries, data) = flatten(root)?;
        let mut image = Image {
            options,
            entries,
            data,
            layout: Layout::default(),
            head: Vec::new(),
            tail: Vec::new(),
        };
        iso::name(&mut image)?;
        image.lay_out()?;
        image.build()?;
        Ok(image)
    }

    /// The image's size in sectors.
    pub fn sectors(&self) -> u64 {
        self.layout.total
    }

    /// The image's size in bytes.
    pub fn bytes(&self) -> u64 {
        self.layout.total * SECTOR
    }

    /// Writes the image to `out`, each file's bytes read as they come.
    /// Fails if a file is not the size it was when scanned.
    pub fn write(&self, out: &mut dyn Write) -> io::Result<()> {
        out.write_all(&self.head)?;
        let mut at = self.layout.data.0;
        let mut buffer = vec![0u8; 1 << 16];
        for data in &self.data {
            debug_assert_eq!(data.at, at);
            match &data.source {
                Source::Bytes(bytes) => out.write_all(bytes)?,
                Source::Path(path) => copy(path, data.size, out, &mut buffer)?,
            }
            let sectors = data.size.div_ceil(SECTOR);
            let pad = (sectors * SECTOR - data.size) as usize;
            out.write_all(buffer_zeros(&mut buffer, pad))?;
            at += sectors;
        }
        out.write_all(&self.tail)
    }

    /// Places each part, in the order the image holds them.
    fn lay_out(&mut self) -> Result<(), Error> {
        let options = self.options.clone();
        let mut l = Layout { pvd: 16, ..Layout::default() };
        let mut at = 17;
        if options.joliet {
            l.svd = Some(at);
            at += 1;
        }
        l.terminator = at;
        at += 1;
        if options.udf {
            // The recognition sequence follows ISO 9660's descriptors;
            // UDF's own descriptors go in the sectors before its anchor,
            // and its partition after it.
            l.vrs = at;
            l.main_vds = 32;
            l.reserve_vds = 48;
            l.integrity = 64;
            l.partition = ANCHOR + 1;
            at = l.partition;
        }
        let iso_order = iso::dir_order(&self.entries, false);
        let joliet_order = iso::dir_order(&self.entries, true);
        let table = iso::path_table_size(&self.entries, &iso_order, false);
        l.iso_tables = (at, at + table.div_ceil(SECTOR), table);
        at += 2 * table.div_ceil(SECTOR);
        if options.joliet {
            let table = iso::path_table_size(&self.entries, &joliet_order, true);
            l.joliet_tables = (at, at + table.div_ceil(SECTOR), table);
            at += 2 * table.div_ceil(SECTOR);
        }
        // The directories' sizes, and the continuation areas', do not
        // depend on where anything is: measured first with nothing
        // placed.
        l.iso_dirs = vec![(0, 0); self.entries.len()];
        l.joliet_dirs = vec![(0, 0); self.entries.len()];
        let mut measure = rock::Areas::new(0);
        for &dir in &iso_order {
            let bytes = iso::directory(self, &l, dir, false, &mut measure)?;
            l.iso_dirs[dir] = (at, bytes.len() as u64 / SECTOR);
            at += bytes.len() as u64 / SECTOR;
        }
        l.continuation = (at, measure.sectors());
        at += measure.sectors();
        if options.joliet {
            for &dir in &joliet_order {
                let mut none = rock::Areas::new(0);
                let bytes = iso::directory(self, &l, dir, true, &mut none)?;
                l.joliet_dirs[dir] = (at, bytes.len() as u64 / SECTOR);
                at += bytes.len() as u64 / SECTOR;
            }
        }
        l.iso_order = iso_order;
        l.joliet_order = joliet_order;
        if options.udf {
            // The file set descriptor and its terminator, a file entry for
            // each entry, then each directory's identifiers.
            l.fsd = at;
            at += 2;
            l.file_entries = (0..self.entries.len() as u64).map(|i| at + i).collect();
            at += self.entries.len() as u64;
            l.udf_dirs = vec![(0, 0); self.entries.len()];
            for (i, entry) in self.entries.iter().enumerate() {
                if matches!(entry.kind, EntryKind::Dir { .. }) {
                    let bytes = udf::identifiers_size(self, i)?;
                    l.udf_dirs[i] = (at, bytes);
                    at += bytes.div_ceil(SECTOR);
                }
            }
        }
        let first = at;
        for data in &mut self.data {
            data.at = at;
            at += data.size.div_ceil(SECTOR);
        }
        l.data = (first, at);
        if options.pad {
            at += PADDING;
        }
        if options.udf {
            // The last anchor, in the image's last sector.
            at = at.max(ANCHOR + 2) + 1;
        }
        if at > u64::from(u32::MAX) {
            return Err(Error::TooLarge);
        }
        l.total = at;
        self.layout = l;
        Ok(())
    }

    /// Builds the sectors before the files' bytes and after them.
    fn build(&mut self) -> Result<(), Error> {
        let l = &self.layout;
        let mut head = vec![0u8; (l.data.0 * SECTOR) as usize];
        let mut put = |sector: u64, bytes: &[u8]| {
            let at = (sector * SECTOR) as usize;
            head[at..at + bytes.len()].copy_from_slice(bytes);
        };
        put(l.pvd, &iso::volume_descriptor(self, false));
        if let Some(svd) = l.svd {
            put(svd, &iso::volume_descriptor(self, true));
        }
        put(l.terminator, &iso::terminator());
        let (little, big) = iso::path_tables(self, false);
        put(l.iso_tables.0, &little);
        put(l.iso_tables.1, &big);
        if self.options.joliet {
            let (little, big) = iso::path_tables(self, true);
            put(l.joliet_tables.0, &little);
            put(l.joliet_tables.1, &big);
        }
        let mut areas = rock::Areas::new(l.continuation.0);
        for &dir in &l.iso_order {
            put(l.iso_dirs[dir].0, &iso::directory(self, l, dir, false, &mut areas)?);
        }
        put(l.continuation.0, &areas.into_bytes());
        if self.options.joliet {
            for &dir in &l.joliet_order {
                let mut none = rock::Areas::new(0);
                put(l.joliet_dirs[dir].0, &iso::directory(self, l, dir, true, &mut none)?);
            }
        }
        let mut tail = vec![0u8; ((l.total - l.data.1) * SECTOR) as usize];
        if self.options.udf {
            for (sector, bytes) in udf::head(self)? {
                put(sector, &bytes);
            }
            let anchor = udf::anchor(self, l.total - 1);
            let at = tail.len() - SECTOR as usize;
            tail[at..at + anchor.len()].copy_from_slice(&anchor);
        }
        self.head = head;
        self.tail = tail;
        Ok(())
    }
}

/// The tree under `root` as entries, the root first, and the files'
/// bytes, one for each file with names that share them.
fn flatten(root: Node) -> Result<(Vec<Entry>, Vec<Data>), Error> {
    let mut entries = Vec::new();
    let mut data: Vec<Data> = Vec::new();
    let mut links: HashMap<(u64, u64), usize> = HashMap::default();
    // Each node, and its directory's entry.
    let mut queue = VecDeque::from([(root, 0usize)]);
    while let Some((node, parent)) = queue.pop_front() {
        let index = entries.len();
        let (kind, children) = match node.kind {
            Kind::File { size, source, link } => {
                let shared = link.and_then(|key| links.get(&key).copied());
                let at = match shared {
                    Some(at) => at,
                    None => {
                        data.push(Data { source, size, at: 0 });
                        if let Some(key) = link {
                            links.insert(key, data.len() - 1);
                        }
                        data.len() - 1
                    }
                };
                (EntryKind::File { size, data: at }, Vec::new())
            }
            Kind::Dir(children) => {
                (EntryKind::Dir { iso: Vec::new(), joliet: Vec::new() }, children)
            }
            Kind::Symlink(target) => {
                if target.len() > 1024 {
                    return Err(Error::TooLong(format!(
                        "the link {}",
                        String::from_utf8_lossy(&node.name)
                    )));
                }
                (EntryKind::Symlink(target), Vec::new())
            }
        };
        if node.name.len() > 255 {
            return Err(Error::TooLong(format!(
                "the name {}",
                String::from_utf8_lossy(&node.name)
            )));
        }
        entries.push(Entry {
            name: node.name,
            kind,
            mode: node.mode & 0o7777,
            uid: node.uid,
            gid: node.gid,
            modified: node.modified,
            accessed: node.accessed,
            changed: node.changed,
            parent,
            links: 1,
            iso: Vec::new(),
            joliet: Vec::new(),
            unique: if index == 0 { 0 } else { 15 + index as u64 },
        });
        if let EntryKind::Dir { iso, .. } = &mut entries[index].kind {
            // Filled in name order by iso::name; their indices first.
            let first = index + 1 + queue.len();
            iso.extend(first..first + children.len());
        }
        for child in children {
            queue.push_back((child, index));
        }
    }
    // Each directory's links: itself, its "." and each directory's "..".
    let mut links = vec![0u32; entries.len()];
    for (i, entry) in entries.iter().enumerate() {
        match entry.kind {
            EntryKind::Dir { .. } => {
                links[i] += 2;
                if i != 0 {
                    links[entry.parent] += 1;
                }
            }
            // A file's are counted below, by the names of its bytes.
            EntryKind::File { .. } => {}
            EntryKind::Symlink(_) => links[i] = 1,
        }
    }
    // A file's names: those that share its bytes.
    let mut names = vec![0u32; data.len()];
    for entry in &entries {
        if let EntryKind::File { data, .. } = entry.kind {
            names[data] += 1;
        }
    }
    for (i, entry) in entries.iter_mut().enumerate() {
        entry.links = match entry.kind {
            EntryKind::File { data, .. } => names[data],
            _ => links[i],
        };
    }
    Ok((entries, data))
}

/// Copies `size` bytes of the file at `path` to `out`.
fn copy(path: &Path, size: u64, out: &mut dyn Write, buffer: &mut [u8]) -> io::Result<()> {
    let changed = || {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{} changed while the disc was made", path.display()),
        )
    };
    let mut file = File::open(path)?;
    let mut left = size;
    while left > 0 {
        let want = left.min(buffer.len() as u64) as usize;
        let got = file.read(&mut buffer[..want])?;
        if got == 0 {
            return Err(changed());
        }
        out.write_all(&buffer[..got])?;
        left -= got as u64;
    }
    if file.read(&mut buffer[..1])? != 0 {
        return Err(changed());
    }
    Ok(())
}

/// `len` zeros, from `buffer`.
fn buffer_zeros(buffer: &mut [u8], len: usize) -> &[u8] {
    buffer[..len].fill(0);
    &buffer[..len]
}

/// The year, month, day, hour, minute and second of `time`, in UTC.
pub(crate) fn date(time: i64) -> (i64, u32, u32, u32, u32, u32) {
    let (days, secs) = (time.div_euclid(86_400), time.rem_euclid(86_400) as u32);
    let (year, month, day) = crate::fat::civil(days);
    (year, month, day, secs / 3600, secs / 60 % 60, secs % 60)
}

/// Writes `value` at `at` in both byte orders, little-endian first, as
/// ISO 9660 records its numbers.
pub(crate) fn both16(out: &mut [u8], at: usize, value: u16) {
    out[at..at + 2].copy_from_slice(&value.to_le_bytes());
    out[at + 2..at + 4].copy_from_slice(&value.to_be_bytes());
}

pub(crate) fn both32(out: &mut [u8], at: usize, value: u32) {
    out[at..at + 4].copy_from_slice(&value.to_le_bytes());
    out[at + 4..at + 8].copy_from_slice(&value.to_be_bytes());
}

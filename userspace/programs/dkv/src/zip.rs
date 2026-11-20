// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The zip a `.dkv` file is (ADR 0222): written plainly, and read as
//! something anyone could have made.
//!
//! Writing: each file stored, or deflated or zstd-packed ([`Method`]),
//! whichever is smaller, with its Unix mode, the time 1980-01-01 00:00 and
//! no data descriptors, in the order given, so that the same folder packs
//! to the same bytes. An archive that fits a classic zip's fields has no
//! zip64 in it; zip64's fields and records are written where a size, an
//! offset or the count does not fit, and only there (ADR 0590). A file of
//! more than [`LARGE`] bytes is given to [`Writer::add_streamed`], which
//! reads it twice instead of holding it.
//!
//! Reading takes the central directory as what the archive holds and
//! checks it against each local header. It takes zip64's records where a
//! size, an offset or the count does not fit the classic fields, and has
//! each value from one place only: a classic field either says that zip64
//! holds it or says the same (ADR 0590). It refuses what a bundle never
//! needs and attacks use: encryption, methods other than stored, deflate
//! and zstd, archives over several disks, entries that overlap or hide
//! between others, paths that leave the folder, links, device files, pipes
//! and sockets, set-ID bits, a name twice, more than [`ENTRIES_MAX`] files,
//! a zstd window over [`WINDOW_MAX`], and more unpacked bytes than
//! [`RATIO`] times the archive. Each file's bytes are checked against its
//! size and CRC-32 as they are unpacked, and unpacking stops at the size
//! the archive says.

use std::fs::File;
use std::io::{self, Write};
use std::os::unix::fs::FileExt;

use miniz_oxide::inflate::stream::{InflateState, inflate};
use miniz_oxide::{DataFormat, MZError, MZFlush, MZStatus};
use ruzstd::decoding::errors::FrameDecoderError;
use ruzstd::decoding::{BlockDecodingStrategy, FrameDecoder};

use crate::sums;

const LOCAL: u32 = 0x0403_4b50;
const CENTRAL: u32 = 0x0201_4b50;
const END: u32 = 0x0605_4b50;
const END64: u32 = 0x0606_4b50;
/// Zip64's end locator, which says where its end record is.
const LOCATOR: u32 = 0x0706_4b50;
/// The signature a data descriptor may start with.
const DESCRIPTOR: u32 = 0x0807_4b50;
/// The extra field of zip64's sizes and offset.
const ZIP64_EXTRA: u16 = 0x0001;
/// Version 2.0, the first with deflate and folders, made on Unix.
const NEEDED: u16 = 20;
const MADE_BY: u16 = 3 << 8 | NEEDED;
/// Bit 11: the name is UTF-8.
const UTF8: u16 = 1 << 11;
const STORED: u16 = 0;
const DEFLATED: u16 = 8;
const ZSTD: u16 = 93;
/// 1980-01-01 00:00, the earliest time a zip holds.
const DOS_DATE: u16 = 1 << 5 | 1;
/// The most an archive's unpacked files may take, against its own size:
/// text and programs deflate to a third or a tenth, and a zip bomb to a
/// thousandth.
pub const RATIO: u64 = 100;
/// The least an archive may always unpack to, whatever its size.
const RATIO_FLOOR: u64 = 16 << 20;
/// The most files an archive may hold: a large app's tree of small files
/// fits, and its directory's entries stay a bounded amount of memory.
pub const ENTRIES_MAX: usize = 1 << 20;
/// The most bytes an archive's directory may take: a million entries
/// whose paths are 200 bytes long.
const DIRECTORY_MAX: u64 = 256 << 20;
/// The most memory a zstd frame's window may take as it unpacks. zstd's
/// own levels use 8 MiB at most, short of `--long`, which no bundle needs.
pub const WINDOW_MAX: u64 = 32 << 20;

const S_IFMT: u32 = 0o170_000;
const S_IFREG: u32 = 0o100_000;
const S_IFDIR: u32 = 0o040_000;
const S_IFLNK: u32 = 0o120_000;

/// The CRC-32 of zip and gzip (IEEE 802.3), as bytes come.
#[derive(Clone, Copy)]
pub struct Crc(u32);

const fn crc_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 { 0xedb8_8320 ^ (c >> 1) } else { c >> 1 };
            k += 1;
        }
        table[i] = c;
        i += 1;
    }
    table
}

static CRC_TABLE: [u32; 256] = crc_table();

impl Default for Crc {
    fn default() -> Crc {
        Crc::new()
    }
}

impl Crc {
    /// A CRC-32 of no bytes yet.
    pub fn new() -> Crc {
        Crc(!0)
    }

    /// Adds `bytes`.
    pub fn update(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 = CRC_TABLE[((self.0 ^ u32::from(b)) & 0xff) as usize] ^ (self.0 >> 8);
        }
    }

    /// The CRC-32 of everything added.
    pub fn value(self) -> u32 {
        !self.0
    }
}

/// A file to write into an archive.
pub struct Input<'a> {
    /// Its path in the archive.
    pub path: &'a str,
    /// Whether it may be run.
    pub executable: bool,
    /// Its bytes.
    pub data: &'a [u8],
}

/// How a file is packed when that makes it smaller; it is stored when it
/// does not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    /// Deflate, which every unzip reads.
    Deflate,
    /// zstd (ADR 0590), which unpacks faster than deflate does.
    Zstd,
}

/// The files over this many bytes that packing does not hold in memory: it
/// reads them twice, through [`Writer::add_streamed`].
pub const LARGE: u64 = 256 << 20;

/// What a file's bytes are given by, as many times as needed: it hands each
/// piece, in order, to the function it is called with.
pub type Contents<'a> =
    dyn Fn(&mut dyn FnMut(&[u8]) -> io::Result<()>) -> io::Result<()> + Sync + 'a;

/// Writes archives a file at a time.
pub struct Writer<W: Write> {
    out: W,
    method: Method,
    offset: u64,
    central: Vec<u8>,
    entries: usize,
    /// The least value a classic field may not hold: a size, an offset or
    /// a directory's size that reaches it goes in a zip64 field. Only the
    /// tests lower it, to reach zip64 without writing 4 GiB.
    wide_from: u64,
}

fn too_big() -> io::Error {
    io::Error::other(format!(
        "the bundle is bigger than a .dkv file holds: {ENTRIES_MAX} files, a directory of \
         256 MiB, a path of 65535 bytes"
    ))
}

fn changed() -> io::Error {
    io::Error::other("a file changed while it was packed")
}

/// A file's entry in the archive.
struct Header<'a> {
    name: &'a str,
    executable: bool,
    method: u16,
    crc: u32,
    size: u64,
    packed: u64,
}

/// A writer that counts the bytes it passes on. Used as a [`Write`], as
/// the zstd encoder does, it never fails: after a failure it takes and
/// drops what it is given and keeps the error, since ruzstd's encoder
/// unwraps the result of each write.
struct Counted<'a> {
    inner: &'a mut dyn Write,
    count: u64,
    failure: Option<io::Error>,
}

impl Counted<'_> {
    /// Passes `bytes` on.
    fn put(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.inner.write_all(bytes)?;
        self.count += bytes.len() as u64;
        Ok(())
    }
}

impl Write for Counted<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.failure.is_none() {
            match self.inner.write_all(bytes) {
                Ok(()) => self.count += bytes.len() as u64,
                Err(error) => self.failure = Some(error),
            }
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Hands `contents` to `each` and gives the CRC-32 and the number of bytes.
fn feed(
    contents: &Contents<'_>,
    mut each: impl FnMut(&[u8]) -> io::Result<()>,
) -> io::Result<(u32, u64)> {
    let mut crc = Crc::new();
    let mut total = 0u64;
    contents(&mut |piece| {
        crc.update(piece);
        total += piece.len() as u64;
        each(piece)
    })?;
    Ok((crc.value(), total))
}

/// Writes `contents` to `sink` as it is, and gives its CRC-32 and length.
fn store_pass(contents: &Contents<'_>, sink: &mut Counted<'_>) -> io::Result<(u32, u64)> {
    feed(contents, |piece| sink.put(piece))
}

/// Deflates `contents` into `sink`, and gives its CRC-32 and length.
fn deflate_pass(contents: &Contents<'_>, sink: &mut Counted<'_>) -> io::Result<(u32, u64)> {
    use miniz_oxide::deflate::core::{CompressorOxide, create_comp_flags_from_zip_params};
    use miniz_oxide::deflate::stream::deflate;
    // Window bits 0 is raw deflate, with no zlib header.
    let mut compressor = Box::new(CompressorOxide::new(create_comp_flags_from_zip_params(6, 0, 0)));
    let mut output = vec![0u8; 64 << 10];
    let (crc, total) = feed(contents, |piece| {
        let mut rest = piece;
        while !rest.is_empty() {
            let step = deflate(&mut compressor, rest, &mut output, MZFlush::None);
            step.status.map_err(|e| io::Error::other(format!("deflate failed: {e:?}")))?;
            rest = &rest[step.bytes_consumed..];
            sink.put(&output[..step.bytes_written])?;
        }
        Ok(())
    })?;
    loop {
        let step = deflate(&mut compressor, &[], &mut output, MZFlush::Finish);
        let status = step.status.map_err(|e| io::Error::other(format!("deflate failed: {e:?}")))?;
        sink.put(&output[..step.bytes_written])?;
        if status == MZStatus::StreamEnd {
            return Ok((crc, total));
        }
    }
}

/// The reading end of the pieces that [`zstd_pass`] sends to its encoder.
struct Pieces {
    queue: std::sync::mpsc::Receiver<Vec<u8>>,
    piece: Vec<u8>,
    at: usize,
}

impl io::Read for Pieces {
    /// Never fails: ruzstd's encoder unwraps what it reads. A sender that
    /// stopped, after a failure of its own, reads as the end.
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.at == self.piece.len() {
            match self.queue.recv() {
                Ok(piece) => (self.piece, self.at) = (piece, 0),
                Err(_) => return Ok(0),
            }
        }
        let n = buf.len().min(self.piece.len() - self.at);
        buf[..n].copy_from_slice(&self.piece[self.at..self.at + n]);
        self.at += n;
        Ok(n)
    }
}

/// Packs `contents` into `sink` as one zstd frame, and gives its CRC-32 and
/// length. ruzstd's encoder pulls its bytes, so another thread calls
/// `contents` and hands each piece over.
fn zstd_pass(contents: &Contents<'_>, sink: &mut Counted<'_>) -> io::Result<(u32, u64)> {
    use ruzstd::encoding::{CompressionLevel, FrameCompressor};
    let (sender, queue) = std::sync::mpsc::sync_channel(4);
    std::thread::scope(|scope| {
        let feeding = scope.spawn(move || {
            feed(contents, |piece| {
                sender.send(piece.to_vec()).map_err(|_| io::Error::other("the encoder stopped"))
            })
        });
        let mut encoder = FrameCompressor::new(CompressionLevel::Fastest);
        encoder.set_source(Pieces { queue, piece: Vec::new(), at: 0 });
        encoder.set_drain(&mut *sink);
        encoder.compress();
        drop(encoder);
        let fed = feeding.join().map_err(|_| io::Error::other("reading a file failed"))??;
        match sink.failure.take() {
            None => Ok(fed),
            Some(error) => Err(error),
        }
    })
}

/// Runs one pass over `contents` packed as `method` into `sink`, and gives
/// its CRC-32. `size` is what the file is said to hold.
fn pass(
    method: u16,
    size: u64,
    contents: &Contents<'_>,
    sink: &mut Counted<'_>,
) -> io::Result<u32> {
    let (crc, total) = match method {
        DEFLATED => deflate_pass(contents, sink)?,
        ZSTD => zstd_pass(contents, sink)?,
        _ => store_pass(contents, sink)?,
    };
    if total != size {
        return Err(changed());
    }
    Ok(crc)
}

impl<W: Write> Writer<W> {
    /// A writer of an archive to `out`, deflating its files.
    pub fn new(out: W) -> Writer<W> {
        Writer::with_method(out, Method::Deflate)
    }

    /// A writer of an archive to `out`, packing its files with `method`.
    pub fn with_method(out: W, method: Method) -> Writer<W> {
        Writer {
            out,
            method,
            offset: 0,
            central: Vec::new(),
            entries: 0,
            wide_from: u64::from(u32::MAX),
        }
    }

    fn method_code(&self) -> u16 {
        match self.method {
            Method::Deflate => DEFLATED,
            Method::Zstd => ZSTD,
        }
    }

    /// Adds `input`, packed if that makes it smaller.
    ///
    /// # Errors
    ///
    /// Fails when `out` does, or when the archive would hold more than
    /// [`ENTRIES_MAX`] files.
    pub fn add(&mut self, input: &Input<'_>) -> io::Result<()> {
        let packed = match self.method {
            Method::Deflate => miniz_oxide::deflate::compress_to_vec(input.data, 6),
            // A slice cannot fail to read, which ruzstd's encoder needs.
            Method::Zstd => ruzstd::encoding::compress_to_vec(
                input.data,
                ruzstd::encoding::CompressionLevel::Fastest,
            ),
        };
        let (method, stored) = if packed.len() < input.data.len() {
            (self.method_code(), &packed[..])
        } else {
            (STORED, input.data)
        };
        let mut crc = Crc::new();
        crc.update(input.data);
        let header = Header {
            name: input.path,
            executable: input.executable,
            method,
            crc: crc.value(),
            size: input.data.len() as u64,
            packed: stored.len() as u64,
        };
        self.begin(&header)?;
        self.out.write_all(stored)
    }

    /// Adds a file of `size` bytes that `contents` gives, without holding it
    /// in memory: it is read twice, the first time to learn its CRC-32 and
    /// its packed size, which come before it, and the second to write it.
    ///
    /// # Errors
    ///
    /// Fails when `out` or `contents` does, when `contents` does not give
    /// `size` bytes, and when the second reading is not the first's bytes.
    pub fn add_streamed(
        &mut self,
        path: &str,
        executable: bool,
        size: u64,
        contents: &Contents<'_>,
    ) -> io::Result<()> {
        let mut method = self.method_code();
        let mut counter = Counted { inner: &mut io::sink(), count: 0, failure: None };
        let crc = pass(method, size, contents, &mut counter)?;
        let mut packed = counter.count;
        if packed >= size {
            (method, packed) = (STORED, size);
        }
        self.begin(&Header { name: path, executable, method, crc, size, packed })?;
        let mut sink = Counted { inner: &mut self.out, count: 0, failure: None };
        let second = pass(method, size, contents, &mut sink)?;
        if second != crc || sink.count != packed {
            return Err(changed());
        }
        Ok(())
    }

    /// Writes `header`'s local header, and its directory entry for
    /// [`Writer::finish`]. The file's `packed` bytes follow.
    fn begin(&mut self, header: &Header<'_>) -> io::Result<()> {
        if self.entries == ENTRIES_MAX {
            return Err(too_big());
        }
        let name = header.name.as_bytes();
        let name_len = u16::try_from(name.len()).map_err(|_| too_big())?;
        let (size, packed, offset) = (header.size, header.packed, self.offset);
        let marked = [size, packed, offset].map(|value| value >= self.wide_from);
        let wide = marked.contains(&true);
        let needed = match header.method {
            ZSTD => 63,
            _ if wide => 45,
            _ => NEEDED,
        };
        // A classic field holds its value, or 0xFFFFFFFF where zip64's extra
        // field does. A local header always has both sizes there, or none.
        let classic = |value: u64, marked: bool| -> u32 {
            // An unmarked value is below `wide_from`, so below 4 GiB.
            if marked { u32::MAX } else { value as u32 }
        };
        let [size_marked, packed_marked, offset_marked] = marked;
        let mut local = Vec::with_capacity(30 + name.len() + 20);
        for field in [&LOCAL.to_le_bytes()[..], &needed.to_le_bytes(), &UTF8.to_le_bytes()] {
            local.extend_from_slice(field);
        }
        let local_extra_len: u16 = if wide { 20 } else { 0 };
        let common = |extra_len: u16, packed: u32, size: u32| {
            [
                header.method.to_le_bytes().to_vec(),
                0u16.to_le_bytes().to_vec(),
                DOS_DATE.to_le_bytes().to_vec(),
                header.crc.to_le_bytes().to_vec(),
                packed.to_le_bytes().to_vec(),
                size.to_le_bytes().to_vec(),
                name_len.to_le_bytes().to_vec(),
                extra_len.to_le_bytes().to_vec(),
            ]
            .concat()
        };
        local.extend_from_slice(&common(
            local_extra_len,
            classic(packed, wide),
            classic(size, wide),
        ));
        local.extend_from_slice(name);
        if wide {
            local.extend_from_slice(&ZIP64_EXTRA.to_le_bytes());
            local.extend_from_slice(&16u16.to_le_bytes());
            local.extend_from_slice(&size.to_le_bytes());
            local.extend_from_slice(&packed.to_le_bytes());
        }
        self.out.write_all(&local)?;
        let values = [(size, size_marked), (packed, packed_marked), (offset, offset_marked)];
        let wide_values: Vec<u8> = values
            .iter()
            .filter(|(_, marked)| *marked)
            .flat_map(|(v, _)| v.to_le_bytes())
            .collect();
        let mut extra = Vec::new();
        if !wide_values.is_empty() {
            extra.extend_from_slice(&ZIP64_EXTRA.to_le_bytes());
            extra.extend_from_slice(&(wide_values.len() as u16).to_le_bytes());
            extra.extend_from_slice(&wide_values);
        }
        let mode: u32 = S_IFREG | if header.executable { 0o755 } else { 0o644 };
        for field in [&CENTRAL.to_le_bytes()[..], &MADE_BY.to_le_bytes(), &needed.to_le_bytes()] {
            self.central.extend_from_slice(field);
        }
        self.central.extend_from_slice(&UTF8.to_le_bytes());
        self.central.extend_from_slice(&common(
            extra.len() as u16,
            classic(packed, packed_marked),
            classic(size, size_marked),
        ));
        for field in [
            &0u16.to_le_bytes()[..],
            &0u16.to_le_bytes(),
            &0u16.to_le_bytes(),
            &(mode << 16).to_le_bytes(),
            &classic(offset, offset_marked).to_le_bytes(),
        ] {
            self.central.extend_from_slice(field);
        }
        self.central.extend_from_slice(name);
        self.central.extend_from_slice(&extra);
        self.offset += local.len() as u64 + packed;
        self.entries += 1;
        Ok(())
    }

    /// Writes the central directory and the end records, and gives back
    /// `out`.
    ///
    /// # Errors
    ///
    /// Fails when `out` does, or when the directory takes more than the
    /// 256 MiB a reader holds.
    pub fn finish(mut self) -> io::Result<W> {
        let size = self.central.len() as u64;
        if size > DIRECTORY_MAX {
            return Err(too_big());
        }
        let (offset, count) = (self.offset, self.entries as u64);
        self.out.write_all(&self.central)?;
        let wide = count >= 0xffff || size >= self.wide_from || offset >= self.wide_from;
        let mut end = Vec::with_capacity(98);
        if wide {
            end.extend_from_slice(&END64.to_le_bytes());
            end.extend_from_slice(&44u64.to_le_bytes());
            end.extend_from_slice(&MADE_BY.to_le_bytes());
            end.extend_from_slice(&45u16.to_le_bytes());
            end.extend_from_slice(&[0; 8]);
            for value in [count, count, size, offset] {
                end.extend_from_slice(&value.to_le_bytes());
            }
            end.extend_from_slice(&LOCATOR.to_le_bytes());
            end.extend_from_slice(&0u32.to_le_bytes());
            end.extend_from_slice(&(offset + size).to_le_bytes());
            end.extend_from_slice(&1u32.to_le_bytes());
        }
        // The classic record holds each field that fits, and the marker for
        // each that zip64's record holds.
        let count = if wide { 0xffff } else { count as u16 };
        let size = if wide { u32::MAX } else { size as u32 };
        let offset = if wide { u32::MAX } else { offset as u32 };
        for field in [
            &END.to_le_bytes()[..],
            &0u16.to_le_bytes(),
            &0u16.to_le_bytes(),
            &count.to_le_bytes(),
            &count.to_le_bytes(),
            &size.to_le_bytes(),
            &offset.to_le_bytes(),
            &0u16.to_le_bytes(),
        ] {
            end.extend_from_slice(field);
        }
        self.out.write_all(&end)?;
        Ok(self.out)
    }
}

/// A file or folder an archive holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// Its path, without the `/` a folder's ends in.
    pub path: String,
    /// Whether it is a folder.
    pub folder: bool,
    /// Whether it may be run.
    pub executable: bool,
    /// How it is stored: 0, as it is; 8, deflated; or 93, packed with zstd.
    pub method: u16,
    /// The CRC-32 of its bytes.
    pub crc: u32,
    /// How many bytes it takes in the archive.
    pub packed: u64,
    /// How many bytes it is.
    pub size: u64,
    /// Where its local header starts, and where its bytes do.
    header: u64,
    data: u64,
}

/// Where an archive's bytes come from: a file, or bytes in memory.
pub trait Source {
    /// How many bytes it holds.
    fn len(&self) -> io::Result<u64>;

    /// Whether it holds no bytes.
    fn is_empty(&self) -> io::Result<bool> {
        self.len().map(|len| len == 0)
    }

    /// Reads into `buf` from `offset`, as many bytes as are there.
    fn read_at(&self, buf: &mut [u8], offset: u64) -> io::Result<usize>;

    /// Fills `buf` from `offset`, or fails.
    fn read_exact_at(&self, buf: &mut [u8], mut offset: u64) -> io::Result<()> {
        let mut done = 0;
        while done < buf.len() {
            match self.read_at(&mut buf[done..], offset)? {
                0 => return Err(io::ErrorKind::UnexpectedEof.into()),
                n => {
                    done += n;
                    offset += n as u64;
                }
            }
        }
        Ok(())
    }
}

impl Source for File {
    fn len(&self) -> io::Result<u64> {
        Ok(self.metadata()?.len())
    }

    fn read_at(&self, buf: &mut [u8], offset: u64) -> io::Result<usize> {
        FileExt::read_at(self, buf, offset)
    }
}

impl Source for [u8] {
    fn len(&self) -> io::Result<u64> {
        Ok(<[u8]>::len(self) as u64)
    }

    fn read_at(&self, buf: &mut [u8], offset: u64) -> io::Result<usize> {
        let start = usize::try_from(offset).unwrap_or(usize::MAX).min(<[u8]>::len(self));
        let n = buf.len().min(<[u8]>::len(self) - start);
        buf[..n].copy_from_slice(&self[start..start + n]);
        Ok(n)
    }
}

fn u16_at(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([bytes[at], bytes[at + 1]])
}

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

fn u64_at(bytes: &[u8], at: usize) -> u64 {
    let mut field = [0u8; 8];
    field.copy_from_slice(&bytes[at..at + 8]);
    u64::from_le_bytes(field)
}

/// An archive's directory, read and checked.
pub struct Archive<'a, S: Source + ?Sized> {
    source: &'a S,
    /// Its entries, in the central directory's order.
    pub entries: Vec<Entry>,
    /// The places in `entries` of its files, not folders, by path.
    files: Vec<usize>,
}

#[allow(clippy::needless_pass_by_value)] // As `map_err` passes it.
fn io_error(e: io::Error) -> String {
    format!("cannot read it: {e}")
}

/// Where an archive's central directory is, as its end records say.
struct Directory {
    count: u64,
    size: u64,
    offset: u64,
    /// Where the directory must end: at the zip64 end record if there is
    /// one, else at the classic end record.
    end: u64,
}

impl<'a, S: Source + ?Sized> Archive<'a, S> {
    /// Reads and checks the archive's directory.
    pub fn read(source: &'a S) -> Result<Archive<'a, S>, String> {
        let size = source.len().map_err(io_error)?;
        let directory = directory(source, size)?;
        if directory.count > ENTRIES_MAX as u64 {
            return Err(format!(
                "it holds {} files, more than the {ENTRIES_MAX} a bundle may",
                directory.count
            ));
        }
        if directory.size > DIRECTORY_MAX {
            return Err(format!(
                "its directory takes {} bytes, more than the {DIRECTORY_MAX} a bundle's may",
                directory.size
            ));
        }
        if directory.offset.checked_add(directory.size) != Some(directory.end) {
            return Err("damaged: its directory is not where its end record says".into());
        }
        // Both fit in a usize: they are under ENTRIES_MAX and DIRECTORY_MAX.
        let mut bytes = vec![0u8; directory.size as usize];
        source.read_exact_at(&mut bytes, directory.offset).map_err(io_error)?;
        // An entry takes 46 bytes at least, so a count the directory cannot
        // hold allocates no more than the directory does.
        let room = (directory.count as usize).min(bytes.len() / 46);
        let mut entries: Vec<Entry> = Vec::with_capacity(room);
        let mut at = 0usize;
        for _ in 0..directory.count {
            let (entry, next) = central_entry(&bytes, at)?;
            at = next;
            entries.push(entry);
        }
        if at != bytes.len() {
            return Err("damaged: its directory has more than its entries".into());
        }
        let mut archive = Archive { source, entries, files: Vec::new() };
        archive.check_locals(directory.offset)?;
        archive.check_names()?;
        let total = archive.entries.iter().try_fold(0u64, |total, e| total.checked_add(e.size));
        let most = size.saturating_mul(RATIO).max(RATIO_FLOOR);
        match total {
            Some(total) if total <= most => Ok(archive),
            _ => Err(format!(
                "its files unpack to more than {RATIO} times its own size: an archive made to \
                 fill the disk"
            )),
        }
    }

    /// Checks each entry's local header against the directory, and that no
    /// two entries' bytes overlap or leave a gap another could hide in.
    fn check_locals(&mut self, cd_offset: u64) -> Result<(), String> {
        let mut order: Vec<usize> = (0..self.entries.len()).collect();
        order.sort_by_key(|&i| self.entries[i].header);
        let past = |path: &str| format!("{path}: damaged: its bytes run into the directory");
        let mut next = 0u64;
        for i in order {
            let entry = &mut self.entries[i];
            if entry.header != next {
                return Err(format!(
                    "{}: its bytes overlap another file's, or leave a gap before it",
                    entry.path
                ));
            }
            let mut header = [0u8; 30];
            self.source.read_exact_at(&mut header, entry.header).map_err(io_error)?;
            let name_len = usize::from(u16_at(&header, 26));
            let extra_len = usize::from(u16_at(&header, 28));
            if u32_at(&header, 0) != LOCAL || u16_at(&header, 8) != entry.method {
                return Err(format!("{}: damaged: its header does not match", entry.path));
            }
            let mut name_and_extra = vec![0u8; name_len + extra_len];
            self.source.read_exact_at(&mut name_and_extra, entry.header + 30).map_err(io_error)?;
            let (name, extra) = name_and_extra.split_at(name_len);
            let expected =
                if entry.folder { format!("{}/", entry.path) } else { entry.path.clone() };
            if name != expected.as_bytes() {
                return Err(format!("{}: damaged: its header names another file", entry.path));
            }
            let wide = extra_field(extra, ZIP64_EXTRA)
                .map_err(|why| format!("{}: damaged: its header's {why}", entry.path))?
                .is_some();
            let header_len = 30 + (name_len + extra_len) as u64;
            entry.data = entry.header.checked_add(header_len).ok_or_else(|| past(&entry.path))?;
            next = entry.data.checked_add(entry.packed).ok_or_else(|| past(&entry.path))?;
            // A data descriptor may follow, as streaming writers leave one.
            if u16_at(&header, 6) & 1 << 3 != 0 {
                next = descriptor_end(self.source, entry, next, wide)?;
            }
            if next > cd_offset {
                return Err(past(&entry.path));
            }
        }
        if next != cd_offset {
            return Err("damaged: bytes between its last file and its directory".into());
        }
        Ok(())
    }

    /// Checks the paths: each once, none a file and a folder at once. Keeps
    /// the files' places by path, for [`Archive::file`].
    fn check_names(&mut self) -> Result<(), String> {
        let entries = &self.entries;
        let mut order: Vec<usize> = (0..entries.len()).collect();
        order.sort_unstable_by(|&a, &b| entries[a].path.cmp(&entries[b].path));
        for pair in order.windows(2) {
            if entries[pair[0]].path == entries[pair[1]].path {
                return Err(format!("{}: in the archive twice", entries[pair[0]].path));
            }
        }
        // The paths inside a folder `p` sort together from `p/`, though not
        // just after `p`: `p-x` sorts between them.
        for &i in order.iter().filter(|&&i| !entries[i].folder) {
            let inside = format!("{}/", entries[i].path);
            let at = order.partition_point(|&j| entries[j].path.as_str() < inside.as_str());
            if order.get(at).is_some_and(|&j| entries[j].path.starts_with(&inside)) {
                return Err(format!("{}: a file, and a folder of others", entries[i].path));
            }
        }
        order.retain(|&i| !entries[i].folder);
        self.files = order;
        Ok(())
    }

    /// The entry at `path`, if the archive holds that file.
    pub fn file(&self, path: &str) -> Option<&Entry> {
        let at = self.files.binary_search_by(|&i| self.entries[i].path.as_str().cmp(path)).ok()?;
        Some(&self.entries[self.files[at]])
    }

    /// Unpacks `entry`, handing its bytes to `out` as they come, and checks
    /// its size and CRC-32. It stops at the size the directory says.
    pub fn unpack(
        &self,
        entry: &Entry,
        out: &mut dyn FnMut(&[u8]) -> Result<(), String>,
    ) -> Result<(), String> {
        let mut crc = Crc::new();
        let mut written = 0u64;
        let mut sink = |bytes: &[u8]| -> Result<(), String> {
            written += bytes.len() as u64;
            if written > entry.size {
                return Err(format!("{}: damaged: more bytes than it says", entry.path));
            }
            crc.update(bytes);
            out(bytes)
        };
        let mut packed = Packed { source: self.source, at: entry.data, left: entry.packed };
        match entry.method {
            STORED => copy(&mut packed, &mut sink)?,
            DEFLATED => inflate_all(&mut packed, &mut sink, &entry.path)?,
            // `central_entry` lets no other method through.
            _ => unzstd(&mut packed, &mut sink, &entry.path)?,
        }
        if written != entry.size {
            return Err(format!("{}: damaged: fewer bytes than it says", entry.path));
        }
        if crc.value() != entry.crc {
            return Err(format!("{}: damaged: its CRC-32 does not match", entry.path));
        }
        Ok(())
    }

    /// Unpacks a small file, such as the list or the signature, whole.
    pub fn read_small(&self, entry: &Entry, max: u64) -> Result<Vec<u8>, String> {
        if entry.size > max {
            return Err(format!("{}: bigger than it may be, {max} bytes", entry.path));
        }
        let mut bytes = Vec::with_capacity(entry.size as usize);
        self.unpack(entry, &mut |chunk| {
            bytes.extend_from_slice(chunk);
            Ok(())
        })?;
        Ok(bytes)
    }
}

/// The end records of an archive of `size` bytes: where its directory is,
/// and how many entries it holds.
fn directory<S: Source + ?Sized>(source: &S, size: u64) -> Result<Directory, String> {
    if size < 22 {
        return Err("not a zip: it is too short".into());
    }
    // The end record is in the last 22 bytes and its comment.
    let tail_len = size.min(22 + 0xffff);
    let mut tail = vec![0u8; tail_len as usize];
    source.read_exact_at(&mut tail, size - tail_len).map_err(io_error)?;
    let at = (0..=tail.len() - 22)
        .rev()
        .find(|&i| {
            u32_at(&tail, i) == END && i + 22 + usize::from(u16_at(&tail, i + 20)) == tail.len()
        })
        .ok_or("not a zip: it has no end record")?;
    let end = &tail[at..at + 22];
    let end_at = size - tail_len + at as u64;
    let classic = [
        u64::from(u16_at(end, 4)),
        u64::from(u16_at(end, 6)),
        u64::from(u16_at(end, 8)),
        u64::from(u16_at(end, 10)),
        u64::from(u32_at(end, 12)),
        u64::from(u32_at(end, 16)),
    ];
    let markers = [0xffff, 0xffff, 0xffff, 0xffff, 0xffff_ffff, 0xffff_ffff];
    let mut locator = [0u8; 20];
    if end_at >= 20 {
        source.read_exact_at(&mut locator, end_at - 20).map_err(io_error)?;
    }
    let zip64 = u32_at(&locator, 0) == LOCATOR;
    let [disk, cd_disk, here, count, cd_size, cd_offset] = if zip64 {
        let wide = zip64_end(source, &locator, end_at - 20)?;
        // Each classic field says that zip64 holds it, or says the same, so
        // that no reader can take one value where another takes another.
        for ((field, marker), value) in classic.iter().zip(markers).zip(wide) {
            if *field != marker && *field != value {
                return Err("damaged: its end records do not agree".into());
            }
        }
        wide
    } else {
        if classic.iter().zip(markers).any(|(field, marker)| *field == marker) {
            return Err("damaged: its end record says zip64 holds a field, and it has no zip64 \
                        end record"
                .into());
        }
        classic
    };
    if disk != 0 || cd_disk != 0 || here != count {
        return Err("an archive split over several disks, which a bundle is not".into());
    }
    let end = if zip64 { end_at - 20 - 56 } else { end_at };
    Ok(Directory { count, size: cd_size, offset: cd_offset, end })
}

/// Reads the zip64 end record that `locator`, read at `locator_at`, points
/// to: its disk, the directory's disk, the entries on this disk and in
/// all, and the directory's size and offset, in the classic record's order.
fn zip64_end<S: Source + ?Sized>(
    source: &S,
    locator: &[u8; 20],
    locator_at: u64,
) -> Result<[u64; 6], String> {
    let damaged = || -> String { "damaged: its zip64 end records do not agree".into() };
    let (disk, record_at, disks) = (u32_at(locator, 4), u64_at(locator, 8), u32_at(locator, 16));
    if disk != 0 || disks != 1 {
        return Err("an archive split over several disks, which a bundle is not".into());
    }
    // The record is 56 bytes with no extensible data, just before the
    // locator: anything between them would be bytes no entry accounts for.
    if record_at.checked_add(56) != Some(locator_at) {
        return Err(damaged());
    }
    let mut record = [0u8; 56];
    source.read_exact_at(&mut record, record_at).map_err(io_error)?;
    if u32_at(&record, 0) != END64 || u64_at(&record, 4) != 44 {
        return Err(damaged());
    }
    Ok([
        u64::from(u32_at(&record, 16)),
        u64::from(u32_at(&record, 20)),
        u64_at(&record, 24),
        u64_at(&record, 32),
        u64_at(&record, 40),
        u64_at(&record, 48),
    ])
}

/// The data of the extra field `id` in `extra`, a header's extra fields,
/// if it is there. Fails if the fields do not fill `extra` exactly or `id`
/// is there twice.
fn extra_field(extra: &[u8], id: u16) -> Result<Option<&[u8]>, String> {
    let mut found = None;
    let mut at = 0;
    while at < extra.len() {
        let head = extra.get(at..at + 4).ok_or("extra fields are cut short")?;
        let len = usize::from(u16_at(head, 2));
        let data = extra.get(at + 4..at + 4 + len).ok_or("extra fields are cut short")?;
        if u16_at(head, 0) == id {
            if found.is_some() {
                return Err(format!("extra field {id:#06x} is there twice"));
            }
            found = Some(data);
        }
        at += 4 + len;
    }
    Ok(found)
}

/// An entry's packed bytes, read in order, up to where they end.
struct Packed<'a, S: Source + ?Sized> {
    source: &'a S,
    at: u64,
    left: u64,
}

impl<S: Source + ?Sized> io::Read for Packed<'_, S> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = buf.len().min(usize::try_from(self.left).unwrap_or(usize::MAX));
        if n == 0 {
            return Ok(0);
        }
        self.source.read_exact_at(&mut buf[..n], self.at)?;
        self.at += n as u64;
        self.left -= n as u64;
        Ok(n)
    }
}

/// Hands a stored entry's bytes to `sink`.
fn copy<S: Source + ?Sized>(
    packed: &mut Packed<'_, S>,
    sink: &mut dyn FnMut(&[u8]) -> Result<(), String>,
) -> Result<(), String> {
    let mut buffer = vec![0u8; 64 << 10];
    loop {
        match io::Read::read(packed, &mut buffer).map_err(io_error)? {
            0 => return Ok(()),
            n => sink(&buffer[..n])?,
        }
    }
}

/// Inflates a deflated entry's bytes into `sink`.
fn inflate_all<S: Source + ?Sized>(
    packed: &mut Packed<'_, S>,
    sink: &mut dyn FnMut(&[u8]) -> Result<(), String>,
    path: &str,
) -> Result<(), String> {
    let mut state = InflateState::new_boxed(DataFormat::Raw);
    let mut input = vec![0u8; 64 << 10];
    let mut output = vec![0u8; 64 << 10];
    let mut have = 0usize;
    let mut start = 0usize;
    loop {
        if start == have && packed.left > 0 {
            have = io::Read::read(packed, &mut input).map_err(io_error)?;
            start = 0;
        }
        // Never MZFlush::Finish: miniz_oxide takes a first call with Finish
        // to mean that the output buffer holds it all, and fails a file
        // whose deflated bytes fit in the input buffer but whose own do not
        // fit in the output's. StreamEnd ends the stream all the same.
        let result = inflate(&mut state, &input[start..have], &mut output, MZFlush::None);
        start += result.bytes_consumed;
        sink(&output[..result.bytes_written])?;
        let all_read = packed.left == 0 && start == have;
        match result.status {
            Ok(MZStatus::StreamEnd) => break,
            Ok(_) => {}
            Err(MZError::Buf) if all_read => {
                return Err(format!("{path}: damaged: its bytes end too soon"));
            }
            Err(MZError::Buf) => {}
            Err(_) => return Err(format!("{path}: damaged: it does not inflate")),
        }
        if result.bytes_consumed == 0 && result.bytes_written == 0 && all_read {
            return Err(format!("{path}: damaged: its bytes end too soon"));
        }
    }
    if start != have || packed.left != 0 {
        return Err(format!("{path}: damaged: bytes after its end"));
    }
    Ok(())
}

/// Unpacks a zstd entry's bytes, one frame with no dictionary and a window
/// of at most [`WINDOW_MAX`], into `sink`.
fn unzstd<S: Source + ?Sized>(
    packed: &mut Packed<'_, S>,
    sink: &mut dyn FnMut(&[u8]) -> Result<(), String>,
    path: &str,
) -> Result<(), String> {
    let failed = |e: FrameDecoderError| match e {
        FrameDecoderError::WindowSizeTooBig { requested, .. } => format!(
            "{path}: packed with a window of {requested} bytes, more than the {WINDOW_MAX} a \
             bundle's files may use"
        ),
        e => format!("{path}: damaged: it does not unpack: {e}"),
    };
    let mut decoder = FrameDecoder::new();
    decoder.set_max_window_size(WINDOW_MAX);
    decoder.reset(&mut *packed).map_err(failed)?;
    loop {
        // A block unpacks to 128 KiB at most, so each turn holds little
        // more than the window.
        let done = decoder
            .decode_blocks(&mut *packed, BlockDecodingStrategy::UptoBytes(64 << 10))
            .map_err(failed)?;
        if let Some(bytes) = decoder.collect() {
            sink(&bytes)?;
        }
        if done {
            break;
        }
    }
    if packed.left != 0 {
        return Err(format!("{path}: damaged: bytes after its end"));
    }
    Ok(())
}

/// Where the data descriptor after `entry`'s bytes, at `at`, ends: its CRC
/// and sizes, with or without its signature before them. Its sizes take 8
/// bytes each when `wide`, as zip64 has them.
fn descriptor_end<S: Source + ?Sized>(
    source: &S,
    entry: &Entry,
    at: u64,
    wide: bool,
) -> Result<u64, String> {
    let mut signature = [0u8; 4];
    source.read_exact_at(&mut signature, at).map_err(io_error)?;
    let start = if u32_at(&signature, 0) == DESCRIPTOR { at + 4 } else { at };
    let mut fields = [0u8; 20];
    let len = if wide { 20 } else { 12 };
    source.read_exact_at(&mut fields[..len], start).map_err(io_error)?;
    let (packed, size) = if wide {
        (u64_at(&fields, 4), u64_at(&fields, 12))
    } else {
        (u64::from(u32_at(&fields, 4)), u64::from(u32_at(&fields, 8)))
    };
    if u32_at(&fields, 0) != entry.crc || packed != entry.packed || size != entry.size {
        return Err(format!("{}: damaged: its data descriptor does not match", entry.path));
    }
    Ok(start + len as u64)
}

/// Reads and checks the directory entry at `at`; returns it and where the
/// next one starts.
fn central_entry(directory: &[u8], at: usize) -> Result<(Entry, usize), String> {
    let damaged = || "damaged: its directory is cut short".to_string();
    let fixed = directory.get(at..at + 46).ok_or_else(damaged)?;
    if u32_at(fixed, 0) != CENTRAL {
        return Err("damaged: its directory has something that is not an entry".into());
    }
    let made_by = u16_at(fixed, 4);
    let flags = u16_at(fixed, 8);
    let method = u16_at(fixed, 10);
    let crc = u32_at(fixed, 16);
    let (name_len, extra_len, comment_len) = (
        usize::from(u16_at(fixed, 28)),
        usize::from(u16_at(fixed, 30)),
        usize::from(u16_at(fixed, 32)),
    );
    let disk = u16_at(fixed, 34);
    let attributes = u32_at(fixed, 38);
    let next = at + 46 + name_len + extra_len + comment_len;
    let name = directory.get(at + 46..at + 46 + name_len).ok_or_else(damaged)?;
    let extra = directory.get(at + 46 + name_len..at + 46 + name_len + extra_len);
    let extra = extra.ok_or_else(damaged)?;
    directory.get(at..next).ok_or_else(damaged)?;
    let name = std::str::from_utf8(name).map_err(|_| {
        format!("`{}`: a name that is not UTF-8", String::from_utf8_lossy(name).escape_debug())
    })?;
    let shown = name.escape_debug().to_string();
    if disk != 0 {
        return Err(format!("{shown}: on another disk, which a bundle's files are not"));
    }
    let [size, packed, header] = sizes(fixed, extra).map_err(|why| format!("{shown}: {why}"))?;
    if flags & (1 | 1 << 6 | 1 << 13) != 0 {
        return Err(format!("{shown}: encrypted, which a bundle is not"));
    }
    if ![STORED, DEFLATED, ZSTD].contains(&method) {
        return Err(format!("{shown}: packed a way other than stored, deflated or zstd"));
    }
    let folder = name.ends_with('/');
    let path = name.strip_suffix('/').unwrap_or(name);
    sums::check_path(path).map_err(|why| format!("{shown}: {why}"))?;
    let unix = made_by >> 8 == 3;
    let mode = if unix { attributes >> 16 } else { 0 };
    match mode & S_IFMT {
        0 => {}
        S_IFREG if !folder => {}
        S_IFDIR if folder => {}
        S_IFLNK => return Err(format!("{path}: a symbolic link, which a bundle may not hold")),
        S_IFREG | S_IFDIR => return Err(format!("{path}: a file and a folder at once")),
        _ => {
            return Err(format!(
                "{path}: a device file, pipe or socket, which a bundle may not hold"
            ));
        }
    }
    if mode & 0o6000 != 0 {
        return Err(format!("{path}: it has set-ID bits, which a bundle's files may not have"));
    }
    // MS-DOS's folder bit, from archives made elsewhere.
    if !unix && attributes & 0x10 != 0 && !folder {
        return Err(format!("{path}: a folder whose name does not end in /"));
    }
    if method == STORED && packed != size {
        return Err(format!("{path}: damaged: stored, but its sizes differ"));
    }
    if folder && (size != 0 || packed != 0) {
        return Err(format!("{path}: a folder with bytes in it"));
    }
    let entry = Entry {
        path: path.into(),
        folder,
        executable: mode & 0o111 != 0,
        method,
        crc,
        packed,
        size,
        header,
        data: 0,
    };
    Ok((entry, next))
}

/// A directory entry's size, packed size and local header's offset, from
/// its `fixed` fields, or from the zip64 field in its `extra` fields for
/// each that holds 0xFFFFFFFF. That field holds those values, in that
/// order, and nothing else, so that each value is had from one place only.
fn sizes(fixed: &[u8], extra: &[u8]) -> Result<[u64; 3], String> {
    let mut values = [u32_at(fixed, 24), u32_at(fixed, 20), u32_at(fixed, 42)].map(u64::from);
    let marked = values.iter().filter(|&&v| v == 0xffff_ffff).count();
    let wide = extra_field(extra, ZIP64_EXTRA).map_err(|why| format!("damaged: its {why}"))?;
    match wide {
        None if marked == 0 => Ok(values),
        Some(wide) if wide.len() == 8 * marked => {
            let marked = values.iter_mut().filter(|v| **v == 0xffff_ffff);
            for (value, field) in marked.zip(wide.as_chunks::<8>().0) {
                *value = u64::from_le_bytes(*field);
            }
            Ok(values)
        }
        _ => Err("damaged: its zip64 field does not hold just the sizes it says it does".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pack(files: &[(&str, bool, &[u8])]) -> Vec<u8> {
        let mut writer = Writer::new(Vec::new());
        for (path, executable, data) in files {
            writer.add(&Input { path, executable: *executable, data }).unwrap();
        }
        writer.finish().unwrap()
    }

    /// What the writer made of [`pinned_files`] before zip64 and zstd: the
    /// signed catalog bundles were packed with these bytes, so they must not
    /// change.
    #[rustfmt::skip]
    const PINNED: &[u8] = &[
        0x50, 0x4b, 0x03, 0x04, 0x14, 0x00, 0x00, 0x08, 0x00, 0x00, 0x00, 0x00, 0x21, 0x00, 0x06, 0xb0, 0xa1, 0xdd, 0x03, 0x00,
        0x00, 0x00, 0x03, 0x00, 0x00, 0x00, 0x0d, 0x00, 0x00, 0x00, 0x6d, 0x61, 0x6e, 0x69, 0x66, 0x65, 0x73, 0x74, 0x2e, 0x6a,
        0x73, 0x6f, 0x6e, 0x7b, 0x7d, 0x0a, 0x50, 0x4b, 0x03, 0x04, 0x14, 0x00, 0x00, 0x08, 0x00, 0x00, 0x00, 0x00, 0x21, 0x00,
        0x33, 0xa0, 0x0b, 0x62, 0x12, 0x00, 0x00, 0x00, 0x12, 0x00, 0x00, 0x00, 0x07, 0x00, 0x00, 0x00, 0x62, 0x69, 0x6e, 0x2f,
        0x61, 0x70, 0x70, 0x7f, 0x45, 0x4c, 0x46, 0x00, 0x00, 0x00, 0x00, 0x20, 0x61, 0x20, 0x70, 0x72, 0x6f, 0x67, 0x72, 0x61,
        0x6d, 0x50, 0x4b, 0x03, 0x04, 0x14, 0x00, 0x00, 0x08, 0x08, 0x00, 0x00, 0x00, 0x21, 0x00, 0xcf, 0x2c, 0xf1, 0x52, 0x1e,
        0x00, 0x00, 0x00, 0xc0, 0x00, 0x00, 0x00, 0x0a, 0x00, 0x00, 0x00, 0x73, 0x68, 0x61, 0x72, 0x65, 0x2f, 0x74, 0x65, 0x78,
        0x74, 0xd5, 0xc8, 0x21, 0x0d, 0x00, 0x00, 0x0c, 0x03, 0x41, 0x3e, 0x15, 0x33, 0xb7, 0xe4, 0x41, 0x93, 0xf9, 0x67, 0xe5,
        0x75, 0x50, 0x72, 0xe0, 0x38, 0xe9, 0x97, 0x74, 0x28, 0x79, 0x03, 0x50, 0x4b, 0x03, 0x04, 0x14, 0x00, 0x00, 0x08, 0x00,
        0x00, 0x00, 0x00, 0x21, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x05, 0x00, 0x00,
        0x00, 0x65, 0x6d, 0x70, 0x74, 0x79, 0x50, 0x4b, 0x01, 0x02, 0x14, 0x03, 0x14, 0x00, 0x00, 0x08, 0x00, 0x00, 0x00, 0x00,
        0x21, 0x00, 0x06, 0xb0, 0xa1, 0xdd, 0x03, 0x00, 0x00, 0x00, 0x03, 0x00, 0x00, 0x00, 0x0d, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xa4, 0x81, 0x00, 0x00, 0x00, 0x00, 0x6d, 0x61, 0x6e, 0x69, 0x66, 0x65, 0x73, 0x74,
        0x2e, 0x6a, 0x73, 0x6f, 0x6e, 0x50, 0x4b, 0x01, 0x02, 0x14, 0x03, 0x14, 0x00, 0x00, 0x08, 0x00, 0x00, 0x00, 0x00, 0x21,
        0x00, 0x33, 0xa0, 0x0b, 0x62, 0x12, 0x00, 0x00, 0x00, 0x12, 0x00, 0x00, 0x00, 0x07, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0xed, 0x81, 0x2e, 0x00, 0x00, 0x00, 0x62, 0x69, 0x6e, 0x2f, 0x61, 0x70, 0x70, 0x50, 0x4b,
        0x01, 0x02, 0x14, 0x03, 0x14, 0x00, 0x00, 0x08, 0x08, 0x00, 0x00, 0x00, 0x21, 0x00, 0xcf, 0x2c, 0xf1, 0x52, 0x1e, 0x00,
        0x00, 0x00, 0xc0, 0x00, 0x00, 0x00, 0x0a, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xa4, 0x81,
        0x65, 0x00, 0x00, 0x00, 0x73, 0x68, 0x61, 0x72, 0x65, 0x2f, 0x74, 0x65, 0x78, 0x74, 0x50, 0x4b, 0x01, 0x02, 0x14, 0x03,
        0x14, 0x00, 0x00, 0x08, 0x00, 0x00, 0x00, 0x00, 0x21, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x05, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xa4, 0x81, 0xab, 0x00, 0x00, 0x00,
        0x65, 0x6d, 0x70, 0x74, 0x79, 0x50, 0x4b, 0x05, 0x06, 0x00, 0x00, 0x00, 0x00, 0x04, 0x00, 0x04, 0x00, 0xdb, 0x00, 0x00,
        0x00, 0xce, 0x00, 0x00, 0x00, 0x00, 0x00,
    ];

    fn pinned_files() -> Vec<(&'static str, bool, Vec<u8>)> {
        vec![
            ("manifest.json", false, b"{}\n".to_vec()),
            ("bin/app", true, b"\x7fELF\0\0\0\0 a program".to_vec()),
            ("share/text", false, b"hello hello hello hello\n".repeat(8)),
            ("empty", false, Vec::new()),
        ]
    }

    #[test]
    fn an_archive_of_small_files_packs_to_the_bytes_it_always_did() {
        let files = pinned_files();
        let files: Vec<(&str, bool, &[u8])> =
            files.iter().map(|(path, exe, data)| (*path, *exe, &data[..])).collect();
        assert_eq!(pack(&files), PINNED);
    }

    fn unpacked(archive: &Archive<'_, [u8]>, path: &str) -> Vec<u8> {
        archive.read_small(archive.file(path).unwrap(), u64::MAX).unwrap()
    }

    /// `size` bytes, handed over in pieces of uneven length: text that
    /// packs well, or noise that does not.
    fn generated(size: u64, noise: bool) -> Box<Contents<'static>> {
        Box::new(move |give| {
            let mut state = 0x2545_f491_4f6c_dd1du64;
            let (mut made, mut turn) = (0u64, 0u64);
            while made < size {
                let piece: Vec<u8> = if noise {
                    (0..70_001)
                        .map(|_| {
                            state ^= state << 13;
                            state ^= state >> 7;
                            state ^= state << 17;
                            (state >> 32) as u8
                        })
                        .collect()
                } else {
                    format!("line {turn} of a generated file, line {turn}\n")
                        .repeat(1 + usize::try_from(turn % 3000).unwrap())
                        .into_bytes()
                };
                let piece = &piece[..piece.len().min(usize::try_from(size - made).unwrap())];
                give(piece)?;
                made += piece.len() as u64;
                turn += 1;
            }
            Ok(())
        })
    }

    fn collected(contents: &Contents<'_>) -> Vec<u8> {
        let mut bytes = Vec::new();
        contents(&mut |piece| {
            bytes.extend_from_slice(piece);
            Ok(())
        })
        .unwrap();
        bytes
    }

    fn zstd_pack(files: &[(&str, bool, &[u8])]) -> Vec<u8> {
        let mut writer = Writer::with_method(Vec::new(), Method::Zstd);
        for (path, executable, data) in files {
            writer.add(&Input { path, executable: *executable, data }).unwrap();
        }
        writer.finish().unwrap()
    }

    #[test]
    fn zstd_files_read_back_what_was_written() {
        let text = b"hello hello hello hello hello hello hello hello\n".repeat(2000);
        let noise = collected(&generated(5000, true));
        let files: [(&str, bool, &[u8]); 4] = [
            ("manifest.json", false, b"{}\n"),
            ("bin/app", true, &noise),
            ("share/text", false, &text),
            ("empty", false, b""),
        ];
        let bytes = zstd_pack(&files);
        let archive = Archive::read(&bytes[..]).unwrap();
        for (path, executable, data) in files {
            assert_eq!(unpacked(&archive, path), data, "{path}");
            assert_eq!(archive.file(path).unwrap().executable, executable, "{path}");
        }
        let packed = archive.file("share/text").unwrap();
        assert!(packed.method == ZSTD && packed.packed < 1000);
        assert_eq!(archive.file("bin/app").unwrap().method, STORED);
        // The header asks for zstd's version, 6.3, and a stored file's asks
        // for no more than the classic 2.0.
        let at = |path: &str| usize::try_from(archive.file(path).unwrap().header).unwrap();
        assert_eq!(u16_at(&bytes, at("share/text") + 4), 63);
        assert_eq!(u16_at(&bytes, at("bin/app") + 4), NEEDED);
        assert_eq!(zstd_pack(&files), bytes);
    }

    /// Entries whose size, packed size or offset reach `wide_from` have
    /// zip64's fields, each marked field in the directory and both sizes in
    /// the local header, and the rest do not.
    #[test]
    fn zip64_entries_read_back() {
        let text = b"some text, some text, some text\n".repeat(50);
        let noise = collected(&generated(300, true));
        for method in [Method::Deflate, Method::Zstd] {
            let mut writer = Writer::with_method(Vec::new(), method);
            writer.wide_from = 200;
            let files: [(&str, &[u8]); 4] =
                [("a", b"small"), ("b", &noise), ("c", &text), ("d", b"after")];
            for (path, data) in files {
                writer.add(&Input { path, executable: false, data }).unwrap();
            }
            let bytes = writer.finish().unwrap();
            let archive = Archive::read(&bytes[..]).unwrap();
            for (path, data) in files {
                assert_eq!(unpacked(&archive, path), data, "{method:?} {path}");
            }
            let at = |path: &str| usize::try_from(archive.file(path).unwrap().header).unwrap();
            let version = |path: &str| u16_at(&bytes, at(path) + 4);
            let wide = if method == Method::Zstd { 63 } else { 45 };
            assert_eq!(version("a"), NEEDED);
            assert_eq!(version("b"), 45);
            // Only its size is past the limit: its packed size is not.
            assert_eq!(version("c"), wide);
            assert_eq!(version("d"), 45);
            // The last entry is past the limit only by its offset, and its
            // directory entry marks that alone.
            let central = find(&bytes, &CENTRAL.to_le_bytes(), 3);
            assert_eq!(u32_at(&bytes, central + 20), 5);
            assert_eq!(u32_at(&bytes, central + 24), 5);
            assert_eq!(u32_at(&bytes, central + 42), u32::MAX);
            assert_eq!(u16_at(&bytes, central + 30), 12);
            // The directory starts past the limit, so zip64 has an end too.
            assert!(find(&bytes, &END64.to_le_bytes(), 0) > central);
        }
    }

    #[test]
    fn more_files_than_a_classic_zip_holds_are_written_and_read() {
        let names: Vec<String> = (0..70_000).map(|i| format!("files/{i:05}")).collect();
        let mut writer = Writer::new(Vec::new());
        for (i, name) in names.iter().enumerate() {
            let data = format!("file {i}");
            writer.add(&Input { path: name, executable: false, data: data.as_bytes() }).unwrap();
        }
        let bytes = writer.finish().unwrap();
        // The classic count is its marker, and zip64's record is there.
        assert_eq!(u16_at(&bytes, bytes.len() - 22 + 10), 0xffff);
        assert_eq!(u32_at(&bytes, bytes.len() - 22 - 20), LOCATOR);
        let archive = Archive::read(&bytes[..]).unwrap();
        assert_eq!(archive.entries.len(), 70_000);
        assert_eq!(unpacked(&archive, "files/69999"), b"file 69999");
        assert_eq!(unpacked(&archive, "files/65535"), b"file 65535");
    }

    #[test]
    fn a_small_archive_has_no_zip64_in_it() {
        let bytes = pack(&[("a", false, b"a"), ("b", false, &b"b".repeat(1000))]);
        assert!(!bytes.windows(4).any(|w| w == END64.to_le_bytes() || w == LOCATOR.to_le_bytes()));
        assert!(!bytes.windows(4).any(|w| w == [1, 0, 16, 0]));
    }

    #[test]
    fn an_archive_holds_no_more_files_than_the_reader_takes() {
        let mut writer = Writer::new(Vec::new());
        writer.entries = ENTRIES_MAX;
        let error = writer.add(&Input { path: "x", executable: false, data: b"x" }).unwrap_err();
        assert!(error.to_string().contains("1048576 files"), "{error}");
    }

    #[test]
    fn streamed_files_read_back() {
        for method in [Method::Deflate, Method::Zstd] {
            for (size, noise) in [(3 << 20, false), (3 << 20, true), (0, false), (1, true)] {
                let contents = generated(size, noise);
                let mut writer = Writer::with_method(Vec::new(), method);
                writer.add(&Input { path: "before", executable: false, data: b"before" }).unwrap();
                writer.add_streamed("big", true, size, &contents).unwrap();
                writer.add(&Input { path: "after", executable: false, data: b"after" }).unwrap();
                let bytes = writer.finish().unwrap();
                let archive = Archive::read(&bytes[..]).unwrap();
                let big = archive.file("big").unwrap();
                assert_eq!(unpacked(&archive, "big"), collected(&contents));
                assert_eq!(unpacked(&archive, "after"), b"after");
                assert!(big.executable);
                let packs = !noise && size > 0;
                let expected = if packs { method_code(method) } else { STORED };
                assert_eq!(big.method, expected, "{method:?} {size} {noise}");
                assert!(!packs || big.packed < big.size / 5);
            }
        }
    }

    fn method_code(method: Method) -> u16 {
        Writer::with_method(Vec::new(), method).method_code()
    }

    #[test]
    fn streamed_files_may_need_zip64_too() {
        let contents = generated(1 << 20, false);
        for method in [Method::Deflate, Method::Zstd] {
            let mut writer = Writer::with_method(Vec::new(), method);
            writer.wide_from = 1 << 19;
            writer.add_streamed("big", false, 1 << 20, &contents).unwrap();
            writer.add(&Input { path: "after", executable: false, data: b"after" }).unwrap();
            let bytes = writer.finish().unwrap();
            let archive = Archive::read(&bytes[..]).unwrap();
            assert_eq!(unpacked(&archive, "big"), collected(&contents));
            assert_eq!(unpacked(&archive, "after"), b"after");
            assert_eq!(u16_at(&bytes, 4), if method == Method::Zstd { 63 } else { 45 });
        }
    }

    #[test]
    fn streamed_files_that_change_or_fail_are_refused() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        for method in [Method::Deflate, Method::Zstd] {
            let calls = AtomicUsize::new(0);
            let first = generated(1 << 20, false);
            let second = generated((1 << 20) + 1, false);
            let changing = |give: &mut dyn FnMut(&[u8]) -> io::Result<()>| {
                if calls.fetch_add(1, Ordering::Relaxed) == 0 { first(give) } else { second(give) }
            };
            let mut writer = Writer::with_method(Vec::new(), method);
            let error = writer.add_streamed("big", false, 1 << 20, &changing).unwrap_err();
            assert!(error.to_string().contains("changed while it was packed"), "{error}");
            // Same length, other bytes.
            let calls = AtomicUsize::new(0);
            let noise = generated(1 << 20, true);
            let swapped = |give: &mut dyn FnMut(&[u8]) -> io::Result<()>| {
                if calls.fetch_add(1, Ordering::Relaxed) == 0 { first(give) } else { noise(give) }
            };
            let mut writer = Writer::with_method(Vec::new(), method);
            let error = writer.add_streamed("big", false, 1 << 20, &swapped).unwrap_err();
            assert!(error.to_string().contains("changed while it was packed"), "{error}");
            // A file that is not the size it is said to be.
            let mut writer = Writer::with_method(Vec::new(), method);
            let error = writer.add_streamed("big", false, 100, &first).unwrap_err();
            assert!(error.to_string().contains("changed while it was packed"), "{error}");
            // A file that cannot be read, in either pass.
            let unreadable = |_: &mut dyn FnMut(&[u8]) -> io::Result<()>| {
                Err(io::Error::other("the disk is gone"))
            };
            let mut writer = Writer::with_method(Vec::new(), method);
            let error = writer.add_streamed("big", false, 100, &unreadable).unwrap_err();
            assert_eq!(error.to_string(), "the disk is gone");
        }
    }

    /// A writer that fails after `room` bytes.
    struct Full {
        room: usize,
    }

    impl Write for Full {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if bytes.len() > self.room {
                return Err(io::Error::other("the disk is full"));
            }
            self.room -= bytes.len();
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn a_full_disk_fails_a_streamed_file_without_a_panic() {
        let contents = generated(3 << 20, false);
        for method in [Method::Deflate, Method::Zstd] {
            let mut writer = Writer::with_method(Full { room: 400 }, method);
            let error = writer.add_streamed("big", false, 3 << 20, &contents).unwrap_err();
            assert_eq!(error.to_string(), "the disk is full");
        }
    }

    #[test]
    fn crc_32_is_zip_s() {
        let mut crc = Crc::new();
        crc.update(b"123456789");
        assert_eq!(crc.value(), 0xcbf4_3926);
    }

    #[test]
    fn archives_read_back_what_was_written() {
        let text = b"hello hello hello hello hello hello hello hello\n".repeat(100);
        let mut state = 0x2545_f491_4f6c_dd1du64;
        let random: Vec<u8> = (0..5000)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                (state >> 32) as u8
            })
            .collect();
        let bytes = pack(&[
            ("manifest.json", false, b"{}\n"),
            ("bin/app", true, &random),
            ("share/text", false, &text),
            ("empty", false, b""),
        ]);
        let archive = Archive::read(&bytes[..]).unwrap();
        assert_eq!(archive.entries.len(), 4);
        assert_eq!(unpacked(&archive, "manifest.json"), b"{}\n");
        assert_eq!(unpacked(&archive, "bin/app"), random);
        assert_eq!(unpacked(&archive, "share/text"), text);
        assert_eq!(unpacked(&archive, "empty"), b"");
        let app = archive.file("bin/app").unwrap();
        assert!(app.executable && app.method == STORED);
        let shared = archive.file("share/text").unwrap();
        assert!(!shared.executable && shared.method == DEFLATED && shared.packed < 200);
        // The same files give the same bytes.
        let again = pack(&[
            ("manifest.json", false, b"{}\n"),
            ("bin/app", true, &random),
            ("share/text", false, &text),
            ("empty", false, b""),
        ]);
        assert_eq!(again, bytes);
    }

    /// A file whose deflated bytes fit in one read of the input but which
    /// inflates to more than one buffer of output, as a manual page does.
    #[test]
    fn small_deflated_files_inflate_to_more_than_a_buffer() {
        let text: Vec<u8> =
            (0..6000).flat_map(|i| format!("line {i} of a manual page\n").into_bytes()).collect();
        assert!(text.len() > 64 << 10);
        let bytes = pack(&[("man/man1/x.1", false, &text)]);
        assert!(bytes.len() < 64 << 10);
        let archive = Archive::read(&bytes[..]).unwrap();
        assert_eq!(unpacked(&archive, "man/man1/x.1"), text);
    }

    /// An archive Info-ZIP's zip 3.0 made of a folder (`zip -r -X`), with a
    /// folder entry, read as it is.
    #[test]
    fn reads_what_info_zip_makes() {
        for bytes in [INFO_ZIP, STREAMED, FORCED_ZIP64] {
            let archive = Archive::read(bytes).unwrap();
            let names: Vec<(&str, bool)> =
                archive.entries.iter().map(|e| (e.path.as_str(), e.folder)).collect();
            assert_eq!(names, [("b", true), ("b/hello.txt", false)]);
            assert_eq!(unpacked(&archive, "b/hello.txt"), b"hello\n");
        }
        assert_eq!(Archive::read(STREAMED).unwrap().entries[1].method, DEFLATED);
    }

    #[rustfmt::skip]
    const INFO_ZIP: &[u8] = &[
        80, 75, 3, 4, 10, 0, 0, 0, 0, 0, 0, 96, 95, 93, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 2, 0, 0, 0, 98, 47, 80, 75, 3, 4, 10, 0, 0, 0,
        0, 0, 0, 96, 95, 93, 32, 48, 58, 54, 6, 0, 0, 0, 6, 0, 0, 0, 11, 0,
        0, 0, 98, 47, 104, 101, 108, 108, 111, 46, 116, 120, 116, 104, 101, 108, 108, 111, 10, 80,
        75, 1, 2, 30, 3, 10, 0, 0, 0, 0, 0, 0, 96, 95, 93, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 16, 0, 237,
        65, 0, 0, 0, 0, 98, 47, 80, 75, 1, 2, 30, 3, 10, 0, 0, 0, 0, 0, 0,
        96, 95, 93, 32, 48, 58, 54, 6, 0, 0, 0, 6, 0, 0, 0, 11, 0, 0, 0, 0,
        0, 0, 0, 1, 0, 0, 0, 164, 129, 32, 0, 0, 0, 98, 47, 104, 101, 108, 108, 111,
        46, 116, 120, 116, 80, 75, 5, 6, 0, 0, 0, 0, 2, 0, 2, 0, 105, 0, 0, 0,
        79, 0, 0, 0, 0, 0,
    ];

    /// The same, streamed through a pipe (`zip -r -X -fz- - b | cat`): the
    /// file deflated, with a data descriptor after it.
    #[rustfmt::skip]
    const STREAMED: &[u8] = &[
        80, 75, 3, 4, 10, 0, 0, 0, 0, 0, 0, 96, 95, 93, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 2, 0, 0, 0, 98, 47, 80, 75, 3, 4, 20, 0, 8, 0,
        8, 0, 0, 96, 95, 93, 0, 0, 0, 0, 0, 0, 0, 0, 6, 0, 0, 0, 11, 0,
        0, 0, 98, 47, 104, 101, 108, 108, 111, 46, 116, 120, 116, 203, 72, 205, 201, 201, 231, 2,
        0, 80, 75, 7, 8, 32, 48, 58, 54, 8, 0, 0, 0, 6, 0, 0, 0, 80, 75, 1,
        2, 30, 3, 10, 0, 0, 0, 0, 0, 0, 96, 95, 93, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 16, 0, 237, 65, 0,
        0, 0, 0, 98, 47, 80, 75, 1, 2, 30, 3, 20, 0, 8, 0, 8, 0, 0, 96, 95,
        93, 32, 48, 58, 54, 8, 0, 0, 0, 6, 0, 0, 0, 11, 0, 0, 0, 0, 0, 0,
        0, 1, 0, 0, 0, 164, 129, 32, 0, 0, 0, 98, 47, 104, 101, 108, 108, 111, 46, 116,
        120, 116, 80, 75, 5, 6, 0, 0, 0, 0, 2, 0, 2, 0, 105, 0, 0, 0, 97, 0,
        0, 0, 0, 0,
    ];

    /// The same folder, with zip64's records forced (`zip -r -X -fz`):
    /// each local header holds zip64's sizes, the directory's entries hold
    /// those of their fields that say so, and the end record says that
    /// zip64 holds the directory's offset.
    #[rustfmt::skip]
    const FORCED_ZIP64: &[u8] = &[
        80, 75, 3, 4, 45, 0, 0, 0, 0, 0, 0, 0, 33, 92, 0, 0, 0, 0, 255, 255,
        255, 255, 255, 255, 255, 255, 2, 0, 20, 0, 98, 47, 1, 0, 16, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 80, 75, 3, 4, 45, 0, 0, 0, 0, 0,
        0, 0, 33, 92, 32, 48, 58, 54, 255, 255, 255, 255, 255, 255, 255, 255, 11, 0, 20, 0,
        98, 47, 104, 101, 108, 108, 111, 46, 116, 120, 116, 1, 0, 16, 0, 6, 0, 0, 0, 0,
        0, 0, 0, 6, 0, 0, 0, 0, 0, 0, 0, 104, 101, 108, 108, 111, 10, 80, 75, 1,
        2, 30, 3, 10, 0, 0, 0, 0, 0, 0, 0, 33, 92, 0, 0, 0, 0, 0, 0, 0,
        0, 255, 255, 255, 255, 2, 0, 12, 0, 0, 0, 0, 0, 0, 0, 16, 0, 237, 65, 0,
        0, 0, 0, 98, 47, 1, 0, 8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 80, 75, 1,
        2, 30, 3, 45, 0, 0, 0, 0, 0, 0, 0, 33, 92, 32, 48, 58, 54, 6, 0, 0,
        0, 255, 255, 255, 255, 11, 0, 12, 0, 0, 0, 0, 0, 1, 0, 0, 0, 164, 129, 52,
        0, 0, 0, 98, 47, 104, 101, 108, 108, 111, 46, 116, 120, 116, 1, 0, 8, 0, 6, 0,
        0, 0, 0, 0, 0, 0, 80, 75, 6, 6, 44, 0, 0, 0, 0, 0, 0, 0, 30, 3,
        45, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 2, 0,
        0, 0, 0, 0, 0, 0, 129, 0, 0, 0, 0, 0, 0, 0, 119, 0, 0, 0, 0, 0,
        0, 0, 80, 75, 6, 7, 0, 0, 0, 0, 248, 0, 0, 0, 0, 0, 0, 0, 1, 0,
        0, 0, 80, 75, 5, 6, 0, 0, 0, 0, 2, 0, 2, 0, 129, 0, 0, 0, 255, 255,
        255, 255, 0, 0,
    ];

    /// Where `pattern` is in `bytes`, the `nth` time from 0.
    fn find(bytes: &[u8], pattern: &[u8], nth: usize) -> usize {
        bytes.windows(pattern.len()).enumerate().filter(|(_, w)| *w == pattern).nth(nth).unwrap().0
    }

    #[test]
    fn zip64_fields_that_disagree_are_refused() {
        let hello = find(FORCED_ZIP64, &CENTRAL.to_le_bytes(), 1);
        let hello_extra = hello + 46 + "b/hello.txt".len();
        let end = find(FORCED_ZIP64, &END.to_le_bytes(), 0);
        let end64 = find(FORCED_ZIP64, &END64.to_le_bytes(), 0);
        let locator = find(FORCED_ZIP64, &LOCATOR.to_le_bytes(), 0);
        // Bytes to put at places in the archive, and why it is refused.
        type Changes<'a> = &'a [(usize, &'a [u8])];
        let cases: &[(Changes<'_>, &str)] = &[
            // The size no longer says zip64 holds it, while zip64 does.
            (&[(hello + 24, &[6, 0, 0, 0])], "zip64 field does not hold just"),
            (&[(hello_extra, &[1, 0, 0, 0, 1, 0, 4, 0, 6, 0, 0, 0])], "twice"),
            (&[(hello_extra + 2, &[9])], "cut short"),
            (&[(end + 10, &[3])], "end records do not agree"),
            (&[(locator, &[0])], "no zip64 end record"),
            (&[(end64 + 4, &[45])], "zip64 end records do not agree"),
            (&[(end64 + 48, &[130])], "not where its end record says"),
            (&[(end + 6, &[255, 255]), (end64 + 20, &[1])], "several disks"),
            (&[(locator + 16, &[2])], "several disks"),
            (&[(hello + 34, &[1])], "another disk"),
            // A million and one files, said in both records.
            (
                &[
                    (end + 8, &[255, 255, 255, 255]),
                    (end64 + 24, &[1, 0, 16]),
                    (end64 + 32, &[1, 0, 16]),
                ],
                "more than the 1048576",
            ),
        ];
        for (changes, why) in cases {
            let mut bytes = FORCED_ZIP64.to_vec();
            for (at, new) in *changes {
                bytes[*at..*at + new.len()].copy_from_slice(new);
            }
            let error = Archive::read(&bytes[..]).err().unwrap_or_default();
            assert!(error.contains(why), "{changes:?}: {error}");
        }
    }

    /// Bytes of an archive made by hand: some held, some zeros that are
    /// never held, so that an archive may pass 4 GiB.
    enum Piece {
        Bytes(Vec<u8>),
        Zeros(u64),
    }

    impl Piece {
        fn len(&self) -> u64 {
            match self {
                Piece::Bytes(bytes) => bytes.len() as u64,
                Piece::Zeros(n) => *n,
            }
        }
    }

    struct Pieces(Vec<Piece>);

    impl Pieces {
        /// Adds `piece`, as part of the last piece when both are bytes, so
        /// that reads find their piece quickly.
        fn push(&mut self, piece: Piece) {
            match (self.0.last_mut(), piece) {
                (Some(Piece::Bytes(last)), Piece::Bytes(bytes)) => last.extend_from_slice(&bytes),
                (_, piece) => self.0.push(piece),
            }
        }

        /// The last piece: the directory and the end records.
        fn tail(&mut self) -> &mut Vec<u8> {
            let Some(Piece::Bytes(tail)) = self.0.last_mut() else { unreachable!() };
            tail
        }
    }

    impl Source for Pieces {
        fn len(&self) -> io::Result<u64> {
            Ok(self.0.iter().map(Piece::len).sum())
        }

        fn read_at(&self, buf: &mut [u8], offset: u64) -> io::Result<usize> {
            let mut start = 0u64;
            for piece in &self.0 {
                if offset < start + piece.len() {
                    let within = offset - start;
                    let n = buf.len().min(usize::try_from(piece.len() - within).unwrap());
                    match piece {
                        Piece::Bytes(bytes) => {
                            let within = within as usize;
                            buf[..n].copy_from_slice(&bytes[within..within + n]);
                        }
                        Piece::Zeros(_) => buf[..n].fill(0),
                    }
                    return Ok(n);
                }
                start += piece.len();
            }
            Ok(0)
        }
    }

    /// An entry written by hand into a zip64 archive: name, method, its
    /// bytes as stored, and the CRC-32 and size it says it has.
    type Wide<'a> = (&'a str, u16, Piece, u32, u64);

    fn crc_of(bytes: &[u8]) -> u32 {
        let mut crc = Crc::new();
        crc.update(bytes);
        crc.value()
    }

    /// Writes a zip64 archive by hand, with every size and offset in
    /// zip64's fields and every classic field saying so. With `streamed`,
    /// each entry's CRC and sizes come after its bytes, in a data
    /// descriptor with zip64's 8-byte sizes.
    fn handmade64(entries: Vec<Wide<'_>>, streamed: bool) -> Pieces {
        let mut pieces = Pieces(Vec::new());
        let mut central = Vec::new();
        let mut offset = 0u64;
        let count = entries.len() as u64;
        let flags: u16 = if streamed { 1 << 3 } else { 0 };
        for (name, method, data, crc, size) in entries {
            let packed = data.len();
            let fields = [&method.to_le_bytes()[..], &[0, 0, 0x21, 0][..]];
            let mut local = Vec::new();
            for field in [&LOCAL.to_le_bytes()[..], &45u16.to_le_bytes(), &flags.to_le_bytes()] {
                local.extend_from_slice(field);
            }
            fields.iter().for_each(|f| local.extend_from_slice(f));
            let (crc_here, size_here, packed_here) =
                if streamed { (0, 0, 0) } else { (crc, size, packed) };
            local.extend_from_slice(&crc_here.to_le_bytes());
            local.extend_from_slice(&[0xff; 8]);
            local.extend_from_slice(&(name.len() as u16).to_le_bytes());
            local.extend_from_slice(&20u16.to_le_bytes());
            local.extend_from_slice(name.as_bytes());
            local.extend_from_slice(&[1, 0, 16, 0]);
            local.extend_from_slice(&size_here.to_le_bytes());
            local.extend_from_slice(&packed_here.to_le_bytes());
            let mut written = local.len() as u64 + packed;
            pieces.push(Piece::Bytes(local));
            pieces.push(data);
            if streamed {
                let mut descriptor = DESCRIPTOR.to_le_bytes().to_vec();
                descriptor.extend_from_slice(&crc.to_le_bytes());
                descriptor.extend_from_slice(&packed.to_le_bytes());
                descriptor.extend_from_slice(&size.to_le_bytes());
                written += descriptor.len() as u64;
                pieces.push(Piece::Bytes(descriptor));
            }
            for field in [&CENTRAL.to_le_bytes()[..], &MADE_BY.to_le_bytes(), &45u16.to_le_bytes()]
            {
                central.extend_from_slice(field);
            }
            central.extend_from_slice(&flags.to_le_bytes());
            fields.iter().for_each(|f| central.extend_from_slice(f));
            central.extend_from_slice(&crc.to_le_bytes());
            central.extend_from_slice(&[0xff; 8]);
            central.extend_from_slice(&(name.len() as u16).to_le_bytes());
            central.extend_from_slice(&28u16.to_le_bytes());
            central.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
            central.extend_from_slice(&(0o100_644u32 << 16).to_le_bytes());
            central.extend_from_slice(&[0xff; 4]);
            central.extend_from_slice(name.as_bytes());
            central.extend_from_slice(&[1, 0, 24, 0]);
            for value in [size, packed, offset] {
                central.extend_from_slice(&value.to_le_bytes());
            }
            offset += written;
        }
        let (cd_offset, cd_size) = (offset, central.len() as u64);
        let mut tail = central;
        tail.extend_from_slice(&END64.to_le_bytes());
        tail.extend_from_slice(&44u64.to_le_bytes());
        tail.extend_from_slice(&MADE_BY.to_le_bytes());
        tail.extend_from_slice(&45u16.to_le_bytes());
        tail.extend_from_slice(&[0; 8]);
        for value in [count, count, cd_size, cd_offset] {
            tail.extend_from_slice(&value.to_le_bytes());
        }
        tail.extend_from_slice(&LOCATOR.to_le_bytes());
        tail.extend_from_slice(&0u32.to_le_bytes());
        tail.extend_from_slice(&(cd_offset + cd_size).to_le_bytes());
        tail.extend_from_slice(&1u32.to_le_bytes());
        tail.extend_from_slice(&END.to_le_bytes());
        tail.extend_from_slice(&[0, 0, 0, 0, 0xff, 0xff, 0xff, 0xff]);
        tail.extend_from_slice(&[0xff; 8]);
        tail.extend_from_slice(&[0, 0]);
        // Its own piece, which `Pieces::tail` changes.
        pieces.0.push(Piece::Bytes(tail));
        pieces
    }

    fn unpacked_from(archive: &Archive<'_, Pieces>, path: &str) -> Vec<u8> {
        archive.read_small(archive.file(path).unwrap(), u64::MAX).unwrap()
    }

    #[test]
    fn zip64_archives_read_back() {
        let text = b"some text, some text, some text\n".repeat(50);
        let deflated = miniz_oxide::deflate::compress_to_vec(&text, 6);
        let zstd_text = b"zstd packs this line well. ".repeat(40);
        for streamed in [false, true] {
            let entries = vec![
                ("a", STORED, Piece::Bytes(b"hello".to_vec()), crc_of(b"hello"), 5),
                ("b", DEFLATED, Piece::Bytes(deflated.clone()), crc_of(&text), text.len() as u64),
                ("c", ZSTD, Piece::Bytes(ZSTD_FRAME.to_vec()), crc_of(&zstd_text), 1080),
            ];
            let pieces = handmade64(entries, streamed);
            let archive = Archive::read(&pieces).unwrap();
            assert_eq!(unpacked_from(&archive, "a"), b"hello");
            assert_eq!(unpacked_from(&archive, "b"), text);
            assert_eq!(unpacked_from(&archive, "c"), zstd_text);
        }
    }

    /// A file of 5 GiB, and one after it, whose header is past 4 GiB.
    #[test]
    fn an_archive_past_4_gib_reads() {
        let entries = vec![
            ("big", STORED, Piece::Zeros(5 << 30), 0, 5 << 30),
            ("after", STORED, Piece::Bytes(b"after".to_vec()), crc_of(b"after"), 5),
        ];
        let pieces = handmade64(entries, false);
        let archive = Archive::read(&pieces).unwrap();
        assert_eq!(archive.file("big").unwrap().size, 5 << 30);
        assert!(archive.file("after").unwrap().header > u64::from(u32::MAX));
        assert_eq!(unpacked_from(&archive, "after"), b"after");
    }

    #[test]
    fn more_files_than_a_classic_zip_holds() {
        let names: Vec<String> = (0..70_000).map(|i| format!("files/{i:05}")).collect();
        let entries =
            names.iter().map(|name| (name.as_str(), STORED, Piece::Bytes(Vec::new()), 0, 0));
        let entries: Vec<Wide<'_>> = entries.collect();
        let pieces = handmade64(entries, false);
        let archive = Archive::read(&pieces).unwrap();
        assert_eq!(archive.entries.len(), 70_000);
        assert_eq!(archive.file("files/69999").unwrap().path, "files/69999");
        assert!(archive.file("files/70000").is_none());
    }

    #[test]
    fn zip64_sizes_that_cannot_be_are_refused() {
        let text = b"some text, some text, some text\n".repeat(50);
        let deflated = miniz_oxide::deflate::compress_to_vec(&text, 6);
        let entry = |name| (name, DEFLATED, Piece::Bytes(deflated.clone()), crc_of(&text), 0);
        // Two files that say they unpack to more than a u64 holds.
        let mut pieces = handmade64(vec![entry("a"), entry("b")], false);
        let second = find(pieces.tail(), &CENTRAL.to_le_bytes(), 1);
        for at in [46 + 1 + 4, second + 46 + 1 + 4] {
            pieces.tail()[at..at + 8].copy_from_slice(&(u64::MAX / 2 + 1).to_le_bytes());
        }
        let error = Archive::read(&pieces).err().unwrap_or_default();
        assert!(error.contains("fill the disk"), "{error}");
        // Bytes that would run past the end of a u64.
        let mut pieces = handmade64(vec![entry("a")], false);
        pieces.tail()[46 + 1 + 12..46 + 1 + 20].copy_from_slice(&(u64::MAX - 10).to_le_bytes());
        let error = Archive::read(&pieces).err().unwrap_or_default();
        assert!(error.contains("run into the directory"), "{error}");
        // A data descriptor whose size is not the directory's.
        let entries =
            vec![("a", DEFLATED, Piece::Bytes(deflated.clone()), crc_of(&text), text.len() as u64)];
        let mut pieces = handmade64(entries, true);
        pieces.tail()[46 + 1 + 4] ^= 1;
        let error = Archive::read(&pieces).err().unwrap_or_default();
        assert!(error.contains("data descriptor does not match"), "{error}");
    }

    /// 1080 bytes, packed by libzstd 1.5.7 at level 19, with its checksum.
    #[rustfmt::skip]
    const ZSTD_FRAME: &[u8] = &[
        40, 181, 47, 253, 100, 56, 3, 29, 1, 0, 216, 122, 115, 116, 100, 32, 112, 97, 99, 107,
        115, 32, 116, 104, 105, 115, 32, 108, 105, 110, 101, 32, 119, 101, 108, 108, 46, 32, 1, 0,
        107, 224, 255, 28, 3, 180, 198, 194, 141,
    ];

    /// The same bytes, packed by libzstd with a window of 64 MiB.
    #[rustfmt::skip]
    const ZSTD_WIDE_WINDOW: &[u8] = &[
        40, 181, 47, 253, 0, 128, 29, 1, 0, 216, 122, 115, 116, 100, 32, 112, 97, 99, 107, 115,
        32, 116, 104, 105, 115, 32, 108, 105, 110, 101, 32, 119, 101, 108, 108, 46, 32, 1, 0, 107,
        224, 255, 28, 3,
    ];

    /// An archive of one file, `frame` packed with zstd, which unpacks to
    /// `content`.
    fn zstd_archive(frame: &[u8], content: &[u8]) -> Vec<u8> {
        let size = u32::try_from(content.len()).unwrap();
        let mut bytes = handmade(&[("z", MADE_BY, 0o100_644 << 16, 0, ZSTD, frame, size)]);
        // `handmade` gives the CRC-32 of the bytes as stored.
        let central = find(&bytes, &CENTRAL.to_le_bytes(), 0);
        for at in [14, central + 16] {
            bytes[at..at + 4].copy_from_slice(&crc_of(content).to_le_bytes());
        }
        bytes
    }

    #[test]
    fn zstd_files_unpack() {
        let text = b"zstd packs this line well. ".repeat(40);
        let bytes = zstd_archive(ZSTD_FRAME, &text);
        let archive = Archive::read(&bytes[..]).unwrap();
        assert_eq!(unpacked(&archive, "z"), text);
        // A file of many blocks, as ruzstd packs it, unpacks in turns.
        let long: Vec<u8> =
            (0..40_000).flat_map(|i| format!("line {i} of a long file\n").into_bytes()).collect();
        let packed = ruzstd::encoding::compress_to_vec(
            &long[..],
            ruzstd::encoding::CompressionLevel::Fastest,
        );
        let bytes = zstd_archive(&packed, &long);
        let archive = Archive::read(&bytes[..]).unwrap();
        assert_eq!(unpacked(&archive, "z"), long);
    }

    #[test]
    fn zstd_frames_a_bundle_does_not_hold_are_refused() {
        let text = b"zstd packs this line well. ".repeat(40);
        let twice = [ZSTD_FRAME, ZSTD_FRAME].concat();
        let skippable = [&[0x50, 0x2a, 0x4d, 0x18, 1, 0, 0, 0, 0][..], ZSTD_FRAME].concat();
        let cases: &[(&[u8], &str)] = &[
            (ZSTD_WIDE_WINDOW, "a window of 67108864 bytes"),
            (&twice, "bytes after its end"),
            (&skippable, "does not unpack"),
            (&ZSTD_FRAME[..30], "does not unpack"),
        ];
        for (frame, why) in cases {
            let bytes = zstd_archive(frame, &text);
            let archive = Archive::read(&bytes[..]).unwrap();
            let error = archive.read_small(&archive.entries[0], 1 << 20).unwrap_err();
            assert!(error.contains(why), "{error}");
        }
    }

    /// An entry written by hand: name, made by, mode or attributes,
    /// flags, method, data as stored, and the size it says it unpacks to.
    type Handmade<'a> = (&'a str, u16, u32, u16, u16, &'a [u8], u32);

    /// Writes an archive by hand, entry by entry.
    fn handmade(entries: &[Handmade<'_>]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut central = Vec::new();
        for (name, made_by, attributes, flags, method, data, size) in entries {
            let offset = out.len() as u32;
            let mut crc = Crc::new();
            crc.update(data);
            let common = |v: &mut Vec<u8>| {
                v.extend_from_slice(&flags.to_le_bytes());
                v.extend_from_slice(&method.to_le_bytes());
                v.extend_from_slice(&[0, 0, 0x21, 0]);
                v.extend_from_slice(&crc.value().to_le_bytes());
                v.extend_from_slice(&(data.len() as u32).to_le_bytes());
                v.extend_from_slice(&size.to_le_bytes());
                v.extend_from_slice(&(name.len() as u16).to_le_bytes());
                v.extend_from_slice(&0u16.to_le_bytes());
            };
            out.extend_from_slice(&LOCAL.to_le_bytes());
            out.extend_from_slice(&20u16.to_le_bytes());
            common(&mut out);
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(data);
            central.extend_from_slice(&CENTRAL.to_le_bytes());
            central.extend_from_slice(&made_by.to_le_bytes());
            central.extend_from_slice(&20u16.to_le_bytes());
            common(&mut central);
            central.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
            central.extend_from_slice(&attributes.to_le_bytes());
            central.extend_from_slice(&offset.to_le_bytes());
            central.extend_from_slice(name.as_bytes());
        }
        let (cd_offset, cd_size) = (out.len() as u32, central.len() as u32);
        out.extend_from_slice(&central);
        out.extend_from_slice(&END.to_le_bytes());
        out.extend_from_slice(&[0, 0, 0, 0]);
        let n = entries.len() as u16;
        out.extend_from_slice(&n.to_le_bytes());
        out.extend_from_slice(&n.to_le_bytes());
        out.extend_from_slice(&cd_size.to_le_bytes());
        out.extend_from_slice(&cd_offset.to_le_bytes());
        out.extend_from_slice(&[0, 0]);
        out
    }

    #[test]
    fn what_attacks_use_is_refused() {
        let file = |mode: u32| mode << 16;
        let unix = MADE_BY;
        let cases: &[(&[Handmade<'_>], &str)] = &[
            (&[("../evil", unix, file(0o100644), 0, 0, b"x", 1)], "not a path in the bundle"),
            (&[("/etc/passwd", unix, file(0o100644), 0, 0, b"x", 1)], "not a path in the bundle"),
            (&[("a\\..\\b", unix, file(0o100644), 0, 0, b"x", 1)], "not a path in the bundle"),
            (&[("link", unix, file(0o120777), 0, 0, b"/etc", 4)], "symbolic link"),
            (&[("dev", unix, file(0o020644), 0, 0, b"", 0)], "device file"),
            (&[("suid", unix, file(0o104755), 0, 0, b"x", 1)], "set-ID"),
            (&[("sgid", unix, file(0o102755), 0, 0, b"x", 1)], "set-ID"),
            (&[("secret", unix, file(0o100644), 1, 0, b"x", 1)], "encrypted"),
            (&[("bz", unix, file(0o100644), 0, 12, b"x", 1)], "stored, deflated or zstd"),
            (
                &[
                    ("a", unix, file(0o100644), 0, 0, b"x", 1),
                    ("a", unix, file(0o100644), 0, 0, b"y", 1),
                ],
                "twice",
            ),
            (
                &[
                    ("a", unix, file(0o100644), 0, 0, b"x", 1),
                    ("a/b", unix, file(0o100644), 0, 0, b"y", 1),
                ],
                "a folder of others",
            ),
            (&[("big", unix, file(0o100644), 0, 0, b"x", u32::MAX)], "zip64 field"),
        ];
        for (entries, why) in cases {
            let bytes = handmade(entries);
            let error = Archive::read(&bytes[..]).err().unwrap_or_default();
            assert!(error.contains(why), "{entries:?}: {error}");
        }
        // A stored file that says it is bigger than its bytes, and a
        // deflated one whose bytes unpack to less than it says.
        let bytes = handmade(&[("short", unix, file(0o100644), 0, 0, b"abc", 10)]);
        assert!(Archive::read(&bytes[..]).err().unwrap_or_default().contains("sizes differ"));
        let deflated = miniz_oxide::deflate::compress_to_vec(b"abc", 6);
        let bytes = handmade(&[("short", unix, file(0o100644), 0, DEFLATED, &deflated, 10)]);
        let archive = Archive::read(&bytes[..]).unwrap();
        let error = archive.read_small(&archive.entries[0], 100).unwrap_err();
        assert!(error.contains("fewer bytes") || error.contains("CRC"), "{error}");
    }

    #[test]
    fn a_zip_bomb_is_refused_before_it_unpacks() {
        let zeros = vec![0u8; 40 << 20];
        let bytes = pack(&[("zeros", false, &zeros)]);
        assert!(bytes.len() < 100_000);
        let error = Archive::read(&bytes[..]).err().unwrap();
        assert!(error.contains("fill the disk"), "{error}");
    }

    /// Changes each byte of `bytes` in turn: every change is found, by the
    /// directory's checks or by unpacking, or leaves the files as `files`,
    /// and none panics.
    fn damage_each_byte(bytes: &[u8], files: &[Vec<u8>]) {
        for i in 0..bytes.len() {
            let mut damaged = bytes.to_vec();
            damaged[i] ^= 0x41;
            let Ok(archive) = Archive::read(&damaged[..]) else { continue };
            let read: Result<Vec<Vec<u8>>, String> =
                archive.entries.iter().map(|e| archive.read_small(e, 1 << 20)).collect();
            if let Ok(read) = read {
                // Only what no check covers may change: the version, the
                // time, and the sizes in a local header, which nothing
                // reads.
                assert_eq!(read, files, "byte {i}");
            }
        }
        for cut in [0, 10, 21, bytes.len() / 2, bytes.len() - 1] {
            assert!(Archive::read(&bytes[..cut]).is_err(), "cut at {cut}");
        }
    }

    #[test]
    fn damage_is_found() {
        let text = b"some text, some text, some text\n".repeat(50);
        let bytes = pack(&[("a", false, &text), ("b", false, b"plain")]);
        damage_each_byte(&bytes, &[text.clone(), b"plain".to_vec()]);
        damage_each_byte(FORCED_ZIP64, &[Vec::new(), b"hello\n".to_vec()]);
        let text = b"zstd packs this line well. ".repeat(40);
        damage_each_byte(&zstd_archive(ZSTD_FRAME, &text), &[text]);
    }
}

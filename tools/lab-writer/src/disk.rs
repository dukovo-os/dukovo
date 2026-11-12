// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The disk the image goes to, found by its serial number alone, and the
//! write itself: zeros where the image has none of its data, the data
//! where it has, the disk's last mebibyte zeroed, its cache flushed, and
//! all of it read back from the disk past the page cache and hashed.

use std::fs::{File, OpenOptions};
use std::io::Read;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{FileExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

use crate::plan::Plan;

/// Where the kernel lists whole disks.
const DISKS: &str = "/sys/block";

/// A mebibyte.
pub const MIB: u64 = 1 << 20;

/// How much is moved at a time.
const CHUNK: usize = 4 << 20;

/// What reads past the page cache are aligned to: the largest logical
/// block size of a disk.
const ALIGN: usize = 4096;

/// Linux's BLKZEROOUT: zero a byte range of a block device, given as a
/// `[u64; 2]` of start and length, both multiples of 512.
const BLKZEROOUT: libc::c_ulong = 0x127f;

/// Linux's BLKFLSBUF: write a block device's dirty pages and drop its
/// page cache.
const BLKFLSBUF: libc::c_ulong = 0x1261;

/// The disk found.
#[derive(Debug, PartialEq, Eq)]
pub struct Disk {
    /// Its device, such as `/dev/sda`.
    pub device: PathBuf,
    /// Its size in bytes.
    pub bytes: u64,
}

/// A serial number as `lsblk` shows it: without the spaces around it,
/// and with each run of spaces inside it one underscore, as udev writes
/// `ID_SERIAL_SHORT`.
pub fn normalize(serial: &str) -> String {
    serial
        .trim_matches(|c: char| c.is_whitespace() || c == '\0')
        .split(|c: char| c.is_whitespace() || c == '\0')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("_")
}

/// The serial number in a SCSI Unit Serial Number VPD page (0x80), which
/// Linux gives as `device/vpd_pg80`: libata answers it with an ATA disk's
/// serial number. Bytes 2 and 3 are the length of the number after them.
pub fn vpd_serial(page: &[u8]) -> Option<String> {
    let length = usize::from(u16::from_be_bytes([*page.get(2)?, *page.get(3)?]));
    let number = page.get(4..4 + length)?;
    let serial = normalize(&String::from_utf8_lossy(number));
    (!serial.is_empty()).then_some(serial)
}

/// The serial number of the whole disk `name` under `disks` (`/sys/block`),
/// where the kernel gives one: NVMe's controller's `serial`, virtio's
/// `serial`, else the VPD page of a SCSI or ATA disk.
fn serial_of(disks: &Path, name: &str) -> Option<String> {
    let dir = disks.join(name);
    for file in ["device/serial", "serial"] {
        if let Ok(text) = std::fs::read_to_string(dir.join(file)) {
            let serial = normalize(&text);
            if !serial.is_empty() {
                return Some(serial);
            }
        }
    }
    vpd_serial(&std::fs::read(dir.join("device/vpd_pg80")).ok()?)
}

/// Each whole disk under `disks`, with its serial number if it has one.
fn listed(disks: &Path) -> Vec<(String, Option<String>)> {
    let Ok(entries) = std::fs::read_dir(disks) else { return Vec::new() };
    let mut found: Vec<(String, Option<String>)> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .map(|name| {
            let serial = serial_of(disks, &name);
            (name, serial)
        })
        .collect();
    found.sort();
    found
}

/// The one disk under `disks` whose serial number is `serial`: none, or
/// more than one, is an error that lists what there is.
fn one_with(disks: &Path, serial: &str) -> Result<Option<String>, String> {
    let wanted = normalize(serial);
    let all = listed(disks);
    let matching: Vec<&String> = all
        .iter()
        .filter(|(_, found)| found.as_deref().is_some_and(|found| found == wanted))
        .map(|(name, _)| name)
        .collect();
    match matching.as_slice() {
        [] => Ok(None),
        [name] => Ok(Some((*name).clone())),
        names => Err(format!(
            "{} disks have the serial number {wanted}, so none is written: {}",
            names.len(),
            names.iter().map(|name| name.as_str()).collect::<Vec<_>>().join(", ")
        )),
    }
}

/// Each disk with its serial number, for a message.
fn describe(disks: &Path) -> String {
    let all: Vec<String> = listed(disks)
        .into_iter()
        .map(|(name, serial)| {
            format!("{name} ({})", serial.as_deref().unwrap_or("no serial number"))
        })
        .collect();
    if all.is_empty() { "none".into() } else { all.join(", ") }
}

/// Finds the disk with serial number `serial`, waiting `wait` for the
/// kernel to find it, and checks that it holds `size` bytes and a
/// mebibyte more.
pub fn find(serial: &str, size: u64, wait: Duration) -> Result<Disk, String> {
    let disks = Path::new(DISKS);
    let deadline = Instant::now() + wait;
    let name = loop {
        if let Some(name) = one_with(disks, serial)? {
            let device = Path::new("/dev").join(&name);
            if device.exists() {
                break name;
            }
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "no disk has the serial number {}; the disks are: {}",
                normalize(serial),
                describe(disks)
            ));
        }
        std::thread::sleep(Duration::from_millis(250));
    };
    let read = |file: &str| std::fs::read_to_string(disks.join(&name).join(file));
    if read("ro").is_ok_and(|ro| ro.trim() == "1") {
        return Err(format!("{name} is read-only"));
    }
    let sectors = read("size")
        .ok()
        .and_then(|text| text.trim().parse::<u64>().ok())
        .ok_or_else(|| format!("the kernel gives no size for {name}"))?;
    let bytes = sectors * 512;
    if bytes < size + MIB {
        return Err(format!(
            "{name} holds {bytes} bytes, and the image needs {size} and a mebibyte more"
        ));
    }
    Ok(Disk { device: Path::new("/dev").join(name), bytes })
}

/// What the write reports as it goes.
pub trait Progress {
    /// `done` of the data's `total` bytes are on the disk.
    fn wrote(&mut self, done: u64, total: u64);
}

/// Writes the image of `plan`, whose data `data` streams, to the disk at
/// `device` of `bytes` bytes, and returns the SHA-256 of the first
/// `plan.size` bytes read back from it.
pub fn write(
    device: &Path,
    bytes: u64,
    plan: &Plan,
    data: &mut impl Read,
    progress: &mut impl Progress,
) -> Result<String, String> {
    let disk = OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_EXCL)
        .open(device)
        .map_err(|err| format!("opening {}: {err}", device.display()))?;
    // Zeros first, so no partial page the zeroing drops from the cache
    // holds data not yet written.
    for (offset, length) in plan.gaps() {
        zero(&disk, offset, length)?;
    }
    zero(&disk, bytes - MIB, MIB)?;
    let total = plan.data_bytes();
    let mut buffer = vec![0u8; CHUNK];
    let mut done = 0;
    for &(offset, length) in &plan.extents {
        let mut at = 0;
        while at < length {
            let want = usize::try_from((length - at).min(CHUNK as u64)).unwrap_or(CHUNK);
            data.read_exact(&mut buffer[..want])
                .map_err(|err| format!("reading the image's data from the lab PC: {err}"))?;
            disk.write_all_at(&buffer[..want], offset + at)
                .map_err(|err| format!("writing {}: {err}", device.display()))?;
            at += want as u64;
            done += want as u64;
            progress.wrote(done, total);
        }
    }
    // fsync on a block device writes its dirty pages and flushes the
    // disk's own write cache.
    disk.sync_all().map_err(|err| format!("flushing {}: {err}", device.display()))?;
    // SAFETY: BLKFLSBUF takes no argument; on a file that is not a block
    // device (in tests) it fails, and the read back below still works.
    unsafe { libc::ioctl(disk.as_raw_fd(), BLKFLSBUF, 0) };
    drop(disk);
    hash_back(device, plan.size)
}

/// Zeroes `length` bytes at `offset`: the 512-byte sectors inside with
/// BLKZEROOUT, which the disk may do itself, and any part sector at
/// either end, or all of it where BLKZEROOUT is not had, by writing
/// zeros.
fn zero(disk: &File, offset: u64, length: u64) -> Result<(), String> {
    let end = offset + length;
    let (first, last) = (offset.next_multiple_of(512), end / 512 * 512);
    if last > first {
        let range = [first, last - first];
        // SAFETY: BLKZEROOUT reads a [u64; 2], which lives through the
        // call.
        let result = unsafe { libc::ioctl(disk.as_raw_fd(), BLKZEROOUT, &raw const range) };
        if result == 0 {
            write_zeros(disk, offset, first - offset)?;
            return write_zeros(disk, last, end - last);
        }
    }
    write_zeros(disk, offset, length)
}

/// Writes `length` zeros at `offset`.
fn write_zeros(disk: &File, offset: u64, length: u64) -> Result<(), String> {
    let zeros = vec![0u8; CHUNK];
    let mut at = 0;
    while at < length {
        let want = usize::try_from((length - at).min(CHUNK as u64)).unwrap_or(CHUNK);
        disk.write_all_at(&zeros[..want], offset + at)
            .map_err(|err| format!("zeroing at {}: {err}", offset + at))?;
        at += want as u64;
    }
    Ok(())
}

/// The SHA-256 of the first `size` bytes of `device`, read with O_DIRECT
/// so that they come from the disk, not the page cache: in aligned
/// blocks, the last one read whole (the disk holds a mebibyte more).
/// Where O_DIRECT is refused (a file in tests), the cache was dropped
/// already and a plain read does.
fn hash_back(device: &Path, size: u64) -> Result<String, String> {
    let direct = OpenOptions::new().read(true).custom_flags(libc::O_DIRECT).open(device);
    let (file, direct) = match direct {
        Ok(file) => (file, true),
        Err(_) => (
            File::open(device).map_err(|err| format!("opening {}: {err}", device.display()))?,
            false,
        ),
    };
    let mut space = vec![0u8; CHUNK + ALIGN];
    let start = space.as_ptr().align_offset(ALIGN);
    let buffer = &mut space[start..start + CHUNK];
    let mut sha = Sha256::new();
    let mut at = 0u64;
    while at < size {
        let wanted = (size - at).min(CHUNK as u64);
        let read_length = if direct { wanted.next_multiple_of(ALIGN as u64) } else { wanted };
        let read_length = usize::try_from(read_length).unwrap_or(CHUNK);
        let mut filled = 0;
        while filled < read_length {
            match file.read_at(&mut buffer[filled..read_length], at + filled as u64) {
                Ok(0) => {
                    return Err(format!("{} ended at {}", device.display(), at + filled as u64));
                }
                Ok(read) => filled += read,
                Err(err) if err.kind() == std::io::ErrorKind::Interrupted => {}
                Err(err) => return Err(format!("reading {} back: {err}", device.display())),
            }
        }
        let wanted = usize::try_from(wanted).unwrap_or(CHUNK);
        sha.update(&buffer[..wanted]);
        at += wanted as u64;
    }
    Ok(hex(&sha.finalize()))
}

/// `bytes` in lower-case hexadecimal.
pub fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        text.push(char::from(DIGITS[usize::from(byte >> 4)]));
        text.push(char::from(DIGITS[usize::from(byte & 0xf)]));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Quiet;
    impl Progress for Quiet {
        fn wrote(&mut self, _: u64, _: u64) {}
    }

    fn temp(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lab-writer-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn serial_numbers_read_as_lsblk_shows_them() {
        assert_eq!(normalize("  S3Z5NB0K123456A \n"), "S3Z5NB0K123456A");
        assert_eq!(normalize("WD-WX1 2AB\0\0"), "WD-WX1_2AB");
        // ATA's serial number is 20 characters, padded with spaces in
        // front; the page may hold more than its length says.
        let mut page = vec![0x00, 0x80, 0x00, 0x14];
        page.extend_from_slice(b"     S3Z5NB0K123456A\0\0");
        assert_eq!(vpd_serial(&page).as_deref(), Some("S3Z5NB0K123456A"));
        assert_eq!(vpd_serial(&[0, 0x80, 0, 9, b'A']), None);
        assert_eq!(vpd_serial(&[0, 0x80, 0, 2, b' ', b' ']), None);
    }

    #[test]
    fn finds_one_disk_by_its_serial_number() {
        let dir = temp("disks");
        let disk = |name: &str, file: &str, bytes: &[u8]| {
            let path = dir.join(name).join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, bytes).unwrap();
        };
        disk("nvme0n1", "device/serial", b"S3Z5NB0K123456A     \n");
        disk("sda", "device/vpd_pg80", b"\x00\x80\x00\x08 LAB-SSD");
        disk("vda", "serial", b"VIRT\n");
        disk("loop0", "size", b"0\n");
        assert_eq!(one_with(&dir, "S3Z5NB0K123456A").unwrap().as_deref(), Some("nvme0n1"));
        assert_eq!(one_with(&dir, " LAB-SSD").unwrap().as_deref(), Some("sda"));
        assert_eq!(one_with(&dir, "VIRT").unwrap().as_deref(), Some("vda"));
        assert_eq!(one_with(&dir, "OTHER").unwrap(), None);
        assert_eq!(
            describe(&dir),
            "loop0 (no serial number), nvme0n1 (S3Z5NB0K123456A), sda (LAB-SSD), vda (VIRT)"
        );
        disk("sdb", "device/vpd_pg80", b"\x00\x80\x00\x07LAB-SSD");
        let err = one_with(&dir, "LAB-SSD").unwrap_err();
        assert!(err.contains("2 disks") && err.contains("sda, sdb"), "{err}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn writes_the_image_zeroes_the_rest_and_hashes_it_back() {
        let dir = temp("write");
        let device = dir.join("disk");
        // A disk full of an earlier test's bytes, 3 MiB and a sector, and
        // an image whose size is not a whole number of sectors.
        let bytes = 3 * MIB + 512;
        std::fs::write(&device, vec![0xAAu8; usize::try_from(bytes).unwrap()]).unwrap();
        let size = MIB + 1000;
        let mut image = vec![0u8; usize::try_from(size).unwrap()];
        image[..700].fill(1);
        image[5000..5003].copy_from_slice(b"abc");
        image[usize::try_from(size).unwrap() - 10..].fill(9);
        let plan = Plan {
            disk: "D".into(),
            size,
            sha256: String::new(),
            extents: vec![(0, 700), (5000, 3), (size - 10, 10)],
        };
        let data: Vec<u8> = plan
            .extents
            .iter()
            .flat_map(|&(o, l)| image[o as usize..(o + l) as usize].to_vec())
            .collect();
        let sum = write(&device, bytes, &plan, &mut data.as_slice(), &mut Quiet).unwrap();
        let written = std::fs::read(&device).unwrap();
        assert!(written[..image.len()] == image[..], "the image is on the disk");
        let between = &written[image.len()..usize::try_from(bytes - MIB).unwrap()];
        assert!(between.iter().all(|&b| b == 0xAA), "what lies past the image is left");
        let last = &written[usize::try_from(bytes - MIB).unwrap()..];
        assert!(last.iter().all(|&b| b == 0), "the last mebibyte is zeroed");
        let expected: String = hex(&Sha256::digest(&image));
        assert_eq!(sum, expected);
        // Data that breaks off fails the write.
        let err = write(&device, bytes, &plan, &mut &data[..10], &mut Quiet).unwrap_err();
        assert!(err.contains("reading the image's data"), "{err}");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

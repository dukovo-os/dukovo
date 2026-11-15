// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The identifiers an image and its filesystems need, made from what the
//! build is and never from chance, so that two builds of one commit write
//! the same bytes (ADR 0535).
//!
//! A GPT's disk and partition GUIDs, a Btrfs's filesystem, device and
//! subvolume UUIDs and a FAT's volume serial are each the HMAC-SHA-256,
//! under a key naming this program's format, of four things: Dukovo's
//! version, `SOURCE_DATE_EPOCH`, the image's kind and the identifier's own
//! purpose, such as `gpt-disk` or `btrfs-subvol:@system.a`. Two builds of
//! the same kind and version at the same time agree; a stick and an
//! installed disk, a root and a live partition, or the first partition and
//! the second do not.
//!
//! These identifiers are for telling disks apart, not secrets. An
//! installed system gets fresh ones from the installer, whose rdisk,
//! mkfs-fat and mkfs-btrfs draw on the system's random numbers
//! (`disk::Rng::system`), which this module does not touch.

use std::cell::RefCell;
use std::fmt::Write as _;

use sha2::{Digest, Sha256};

/// The HMAC's key, which also names the format of the message: a change
/// to how identifiers are made changes every one of them.
const KEY: &[u8] = b"dukovo reproducible identifiers 1";

/// HMAC-SHA-256 of `message` under `key` (RFC 2104).
fn hmac(key: &[u8], message: &[u8]) -> [u8; 32] {
    const BLOCK: usize = 64;
    let mut padded = [0u8; BLOCK];
    if key.len() > BLOCK {
        padded[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        padded[..key.len()].copy_from_slice(key);
    }
    let pad = |byte: u8| padded.map(|k| k ^ byte);
    let mut inner = Sha256::new();
    inner.update(pad(0x36));
    inner.update(message);
    let mut outer = Sha256::new();
    outer.update(pad(0x5c));
    outer.update(inner.finalize());
    outer.finalize().into()
}

/// The identifiers of one kind of image of one build.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ids {
    version: String,
    epoch: u64,
    kind: String,
}

impl Ids {
    /// The identifiers of an image of `kind` (`stick`, `installed`, ...)
    /// of Dukovo `version`, made at `epoch` seconds since 1970, which is
    /// `SOURCE_DATE_EPOCH`.
    pub fn new(version: &str, epoch: u64, kind: &str) -> Ids {
        Ids { version: version.into(), epoch, kind: kind.into() }
    }

    /// The identifiers of what is added to a disk that is made already,
    /// `disk_guid` being the GPT's own bytes: that GUID is a product of
    /// the version, the time and the kind of the image that made it, and
    /// stands for them here, so that a partition's GUID depends on all of
    /// them, and on which disk it is put.
    pub fn of_disk(disk_guid: &[u8]) -> Ids {
        Ids::new("", 0, &format!("disk {}", hex(disk_guid)))
    }

    /// The time the image is made at, in seconds since 1970.
    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    /// Thirty-two bytes for `purpose`: the HMAC of the four inputs, each
    /// with its length in front, so that no two choices of inputs make
    /// the same message.
    pub fn bytes(&self, purpose: &str) -> [u8; 32] {
        let mut message = Vec::new();
        for field in [self.version.as_str(), &self.epoch.to_string(), &self.kind, purpose] {
            message.extend_from_slice(&(field.len() as u64).to_le_bytes());
            message.extend_from_slice(field.as_bytes());
        }
        hmac(KEY, &message)
    }

    /// A version 4 UUID for `purpose`, variant 1 (RFC 9562), in the order
    /// of its text, as Btrfs keeps one and `blkid` prints it.
    pub fn uuid(&self, purpose: &str) -> [u8; 16] {
        let mut uuid = [0u8; 16];
        uuid.copy_from_slice(&self.bytes(purpose)[..16]);
        uuid[6] = uuid[6] & 0x0f | 0x40;
        uuid[8] = uuid[8] & 0x3f | 0x80;
        uuid
    }

    /// A GPT GUID for `purpose`: [`Ids::uuid`], in the byte order a GPT
    /// writes it.
    pub fn guid(&self, purpose: &str) -> disk::Guid {
        let text = crate::image::uuid_text(self.uuid(purpose));
        disk::Guid::from_text(&text).expect("a UUID's text is a GUID")
    }

    /// A FAT's 32-bit volume serial number for `purpose`.
    pub fn serial(&self, purpose: &str) -> u32 {
        let bytes = self.bytes(purpose);
        u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
    }

    /// A generator for code that takes one, seeded from `purpose`'s
    /// bytes. Where an identifier is asked for by name it comes from the
    /// methods above; this is for the draws that code makes and nothing
    /// reads back.
    pub fn rng(&self, purpose: &str) -> disk::Rng {
        let bytes = self.bytes(purpose);
        disk::Rng::seeded(u64::from_le_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        ]))
    }
}

/// The identifier a filesystem crate asks for next: the purpose it is for,
/// and how many it has asked for under it.
struct Source {
    ids: Ids,
    purpose: String,
    count: u64,
}

thread_local! {
    /// What [`fill`] makes the kernel's filesystem crates' UUIDs of: the
    /// crates call a plain function, which holds no state of its own. A
    /// host build makes a filesystem on one thread, and a test that makes
    /// one has its own.
    static SOURCE: RefCell<Option<Source>> = const { RefCell::new(None) };
}

/// Has the filesystem crates' random bytes, [`fill`], come from `ids`,
/// under the purpose `purpose` until it is set again. Each request under
/// one purpose is a number further on, so that two UUIDs a filesystem
/// makes while one purpose holds are different.
pub fn for_crates(ids: &Ids, purpose: &str) {
    SOURCE.with_borrow_mut(|source| {
        *source = Some(Source { ids: ids.clone(), purpose: purpose.into(), count: 0 });
    });
}

/// Fills `out` as the kernel's `radix_vfs::random` is asked to, from the
/// identifiers set by [`for_crates`]: never from the system's random
/// numbers. With none set, from an identifier of the purpose `unset`.
pub fn fill(out: &mut [u8]) {
    SOURCE.with_borrow_mut(|source| {
        let source = source.get_or_insert_with(|| Source {
            ids: Ids::new("", 0, "unset"),
            purpose: "unset".into(),
            count: 0,
        });
        source.count += 1;
        let purpose = format!("{}#{}", source.purpose, source.count);
        for (n, chunk) in out.chunks_mut(32).enumerate() {
            let bytes = source.ids.bytes(&format!("{purpose}/{n}"));
            chunk.copy_from_slice(&bytes[..chunk.len()]);
        }
    });
}

/// Lowercase hexadecimal.
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut text, byte| {
        let _ = write!(text, "{byte:02x}");
        text
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 4231's test cases 1, 2 and 6, which cover a short key, a key
    /// that is text, and a key longer than a block.
    #[test]
    fn hmac_is_sha_256_s() {
        assert_eq!(
            hex(&hmac(&[0x0b; 20], b"Hi There")),
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        );
        assert_eq!(
            hex(&hmac(b"Jefe", b"what do ya want for nothing?")),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
        assert_eq!(
            hex(&hmac(&[0xaa; 131], b"Test Using Larger Than Block-Size Key - Hash Key First")),
            "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
        );
    }

    #[test]
    fn the_same_inputs_make_the_same_identifiers() {
        let a = Ids::new("0.15.0", 1_793_592_777, "stick");
        let b = Ids::new("0.15.0", 1_793_592_777, "stick");
        for purpose in ["gpt-disk", "gpt-part-2", "btrfs-fsid", "btrfs-subvol:@system.a"] {
            assert_eq!(a.bytes(purpose), b.bytes(purpose));
            assert_eq!(a.uuid(purpose), b.uuid(purpose));
            assert_eq!(a.guid(purpose), b.guid(purpose));
            assert_eq!(a.serial(purpose), b.serial(purpose));
        }
    }

    #[test]
    fn a_different_purpose_makes_a_different_identifier() {
        let ids = Ids::new("0.15.0", 1_793_592_777, "stick");
        let purposes = ["gpt-disk", "gpt-part-1", "gpt-part-2", "btrfs-fsid", "btrfs-dev"];
        for (i, one) in purposes.iter().enumerate() {
            for other in &purposes[i + 1..] {
                assert_ne!(ids.uuid(one), ids.uuid(other), "{one} and {other}");
                assert_ne!(ids.guid(one), ids.guid(other), "{one} and {other}");
            }
        }
        // A purpose that is another's with something added, and the same
        // letters cut at another place, are not the same either.
        assert_ne!(ids.bytes("gpt-part-1"), ids.bytes("gpt-part-10"));
        let (a, b) = (Ids::new("0.1", 12, "x"), Ids::new("0.1", 1, "2x"));
        assert_ne!(a.bytes("p"), b.bytes("p"));
    }

    #[test]
    fn each_input_changes_every_identifier() {
        let base = Ids::new("0.15.0", 1_793_592_777, "stick");
        for other in [
            Ids::new("0.15.1", 1_793_592_777, "stick"),
            Ids::new("0.15.0", 1_793_592_778, "stick"),
            Ids::new("0.15.0", 1_793_592_777, "iso"),
        ] {
            assert_ne!(base.uuid("gpt-disk"), other.uuid("gpt-disk"));
            assert_ne!(base.serial("fat-esp"), other.serial("fat-esp"));
        }
    }

    #[test]
    fn a_partition_added_to_a_disk_has_a_guid_of_the_disk_and_its_place() {
        let stick = Ids::new("0.15.0", 1_793_592_777, "stick").guid("gpt-disk");
        let installed = Ids::new("0.15.0", 1_793_592_777, "installed").guid("gpt-disk");
        let on = |disk: &disk::Guid, purpose: &str| Ids::of_disk(&disk.0).guid(purpose);
        assert_eq!(on(&stick, "gpt-part-3:root"), on(&stick, "gpt-part-3:root"));
        assert_ne!(on(&stick, "gpt-part-3:root"), on(&installed, "gpt-part-3:root"));
        assert_ne!(on(&stick, "gpt-part-3:root"), on(&stick, "gpt-part-4:live"));
    }

    #[test]
    fn a_uuid_is_version_4_and_variant_1() {
        let ids = Ids::new("0.15.0", 1_793_592_777, "installed");
        for n in 0..200 {
            let uuid = ids.uuid(&format!("purpose {n}"));
            assert_eq!(uuid[6] >> 4, 4, "version");
            assert_eq!(uuid[8] >> 6, 0b10, "variant 1");
            let text = crate::image::uuid_text(uuid);
            assert_eq!(text.as_bytes()[14], b'4', "{text}");
            assert!(matches!(text.as_bytes()[19], b'8' | b'9' | b'a' | b'b'), "{text}");
        }
    }

    /// A GPT writes the first three fields of a GUID little-endian, which
    /// is where the version and the variant move to in the disk's bytes.
    #[test]
    fn a_guid_has_the_same_bits_in_the_order_a_gpt_writes_them() {
        let ids = Ids::new("0.15.0", 1_793_592_777, "stick");
        let uuid = ids.uuid("gpt-disk");
        let guid = ids.guid("gpt-disk");
        assert_eq!(guid.0[..4], [uuid[3], uuid[2], uuid[1], uuid[0]]);
        assert_eq!(guid.0[4..6], [uuid[5], uuid[4]]);
        assert_eq!(guid.0[6..8], [uuid[7], uuid[6]]);
        assert_eq!(guid.0[8..], uuid[8..]);
        assert_eq!(guid.0[7] >> 4, 4, "version, in the byte a GPT puts it in");
        assert_eq!(guid.0[8] >> 6, 0b10, "variant 1");
        assert_eq!(guid.to_string().to_ascii_lowercase(), crate::image::uuid_text(uuid));
    }

    #[test]
    fn the_filesystem_crates_get_a_new_value_each_time_and_the_same_ones_each_build() {
        let ids = Ids::new("0.15.0", 1_793_592_777, "installed");
        let draw = |purpose: &str| {
            for_crates(&ids, purpose);
            let (mut a, mut b, mut long) = ([0u8; 16], [0u8; 16], [0u8; 70]);
            fill(&mut a);
            fill(&mut b);
            fill(&mut long);
            (a, b, long)
        };
        let (a, b, long) = draw("btrfs-subvol:@system.a");
        assert_ne!(a, b);
        assert_ne!(long[..16], a);
        assert!(long.iter().any(|&byte| byte != 0));
        assert_eq!((a, b, long), draw("btrfs-subvol:@system.a"));
        assert_ne!(a, draw("btrfs-subvol:@system.b").0);
    }
}

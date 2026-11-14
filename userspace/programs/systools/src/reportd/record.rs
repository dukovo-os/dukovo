// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A crash record as `/devices/crashes` gives it (ADR 0530,
//! radix-crashes(4)): read field by field at the offsets the ABI names, and
//! checked against itself, so that a short or garbled record is refused
//! rather than read past its end.

use radix_abi::crash::{
    DOMAIN_MAX, Header, MAGIC, MAPPINGS_MAX, Mapping as Wire, PATH_CUT, PATH_MAX, PATH_UNKNOWN,
    STACK_MAX, VERSION,
};

/// A part of a file the process had mapped to run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mapping {
    pub start: u64,
    pub end: u64,
    /// Where `start` is in the file.
    pub offset: u64,
    /// The file's path, if the kernel knew it whole.
    pub path: Option<String>,
}

/// A crash record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    /// How many crashes the kernel did not keep before this one.
    pub dropped: u32,
    pub signal: u32,
    pub code: i32,
    /// A fault's address.
    pub address: u64,
    pub process: u64,
    pub thread: u64,
    pub user: u32,
    pub group: u32,
    /// The process's parent, its parent's and so on, nearest first.
    pub ancestors: Vec<u64>,
    /// When, in nanoseconds since 1970, if the kernel knew.
    pub wall_ns: Option<u64>,
    pub thread_name: String,
    pub pc: u64,
    pub sp: u64,
    pub fp: u64,
    /// The program's path, if the kernel knew it whole.
    pub path: Option<String>,
    /// The resource domain it ran in: a service's processes run in one
    /// named after the service.
    pub domain: String,
    pub mappings: Vec<Mapping>,
    /// The stack's bytes from `sp` up.
    pub stack: Vec<u8>,
}

/// Why a record was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Malformed(pub &'static str);

impl Record {
    /// Reads `bytes`, one whole record.
    ///
    /// # Errors
    ///
    /// [`Malformed`], naming what does not fit.
    pub fn read(bytes: &[u8]) -> Result<Record, Malformed> {
        if bytes.len() < Header::SIZE {
            return Err(Malformed("shorter than its header"));
        }
        if u64_at(bytes, Header::OFFSET_MAGIC) != MAGIC {
            return Err(Malformed("not a crash record"));
        }
        if u32_at(bytes, Header::OFFSET_VERSION) != VERSION {
            return Err(Malformed("of another version"));
        }
        if u32_at(bytes, Header::OFFSET_SIZE) as usize != bytes.len() {
            return Err(Malformed("not the size its header says"));
        }
        let count = u32_at(bytes, Header::OFFSET_MAPPINGS) as usize;
        let stack_len = u32_at(bytes, Header::OFFSET_STACK_LEN) as usize;
        if count > MAPPINGS_MAX as usize || stack_len > STACK_MAX as usize {
            return Err(Malformed("larger than a record may be"));
        }
        let stack_at = Header::SIZE + count * Wire::SIZE;
        if stack_at + stack_len != bytes.len() {
            return Err(Malformed("its parts do not add up to its size"));
        }
        let mappings = (0..count)
            .map(|i| {
                let at = Header::SIZE + i * Wire::SIZE;
                let wire = &bytes[at..at + Wire::SIZE];
                Mapping {
                    start: u64_at(wire, Wire::OFFSET_START),
                    end: u64_at(wire, Wire::OFFSET_END),
                    offset: u64_at(wire, Wire::OFFSET_OFFSET),
                    path: path_at(
                        wire,
                        Wire::OFFSET_PATH_FLAGS,
                        Wire::OFFSET_PATH_LEN,
                        Wire::OFFSET_PATH,
                    ),
                }
            })
            .collect();
        let ancestors = (0..4)
            .map(|i| u64_at(bytes, Header::OFFSET_ANCESTORS + i * 8))
            .take_while(|&n| n != 0)
            .collect();
        let name = &bytes[Header::OFFSET_THREAD_NAME..Header::OFFSET_THREAD_NAME + 16];
        let name = name.split(|&b| b == 0).next().unwrap_or_default();
        let wall_ns = u64_at(bytes, Header::OFFSET_WALL_NS);
        let domain_len =
            (u32_at(bytes, Header::OFFSET_DOMAIN_LEN) as usize).min(DOMAIN_MAX as usize);
        let domain = &bytes[Header::OFFSET_DOMAIN..Header::OFFSET_DOMAIN + domain_len];
        Ok(Record {
            dropped: u32_at(bytes, Header::OFFSET_DROPPED),
            signal: u32_at(bytes, Header::OFFSET_SIGNAL),
            code: u32_at(bytes, Header::OFFSET_CODE) as i32,
            address: u64_at(bytes, Header::OFFSET_ADDRESS),
            process: u64_at(bytes, Header::OFFSET_PROCESS),
            thread: u64_at(bytes, Header::OFFSET_THREAD),
            user: u32_at(bytes, Header::OFFSET_USER),
            group: u32_at(bytes, Header::OFFSET_GROUP),
            ancestors,
            wall_ns: (wall_ns != 0).then_some(wall_ns),
            thread_name: String::from_utf8_lossy(name).into_owned(),
            pc: u64_at(bytes, Header::OFFSET_PC),
            sp: u64_at(bytes, Header::OFFSET_SP),
            fp: u64_at(bytes, Header::OFFSET_FP),
            path: path_at(
                bytes,
                Header::OFFSET_PATH_FLAGS,
                Header::OFFSET_PATH_LEN,
                Header::OFFSET_PATH,
            ),
            domain: String::from_utf8_lossy(domain).into_owned(),
            mappings,
            stack: bytes[stack_at..].to_vec(),
        })
    }

    /// The mapping `address` is in, if any.
    pub fn mapping_of(&self, address: u64) -> Option<&Mapping> {
        self.mappings.iter().find(|m| m.start <= address && address < m.end)
    }

    /// The 8 bytes of the stack at `address`, if the record holds them.
    pub fn stack_word(&self, address: u64) -> Option<u64> {
        let at = usize::try_from(address.checked_sub(self.sp)?).ok()?;
        let bytes = self.stack.get(at..at.checked_add(8)?)?;
        Some(u64::from_le_bytes(bytes.try_into().ok()?))
    }

    /// The program's file name: the end of its path, or the thread's name.
    pub fn program(&self) -> String {
        match &self.path {
            Some(path) => path.rsplit('/').next().unwrap_or(path).to_string(),
            None => self.thread_name.clone(),
        }
    }
}

fn u64_at(bytes: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap_or_default())
}

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap_or_default())
}

/// The path at `path_at`, if it is known and whole, and UTF-8.
fn path_at(bytes: &[u8], flags_at: usize, len_at: usize, path_at: usize) -> Option<String> {
    let flags = u32_at(bytes, flags_at);
    let len = (u32_at(bytes, len_at) as usize).min(PATH_MAX as usize);
    if flags & (PATH_CUT | PATH_UNKNOWN) != 0 || len == 0 {
        return None;
    }
    String::from_utf8(bytes[path_at..path_at + len].to_vec()).ok()
}

/// A record's bytes, as the kernel lays them out, for tests here and in
/// the unwinder's.
#[cfg(test)]
pub fn encode(record: &Record) -> Vec<u8> {
    let size = Header::SIZE + record.mappings.len() * Wire::SIZE + record.stack.len();
    let mut bytes = vec![0u8; size];
    let mut put = |at: usize, value: &[u8]| bytes[at..at + value.len()].copy_from_slice(value);
    put(Header::OFFSET_MAGIC, &MAGIC.to_le_bytes());
    put(Header::OFFSET_VERSION, &VERSION.to_le_bytes());
    put(Header::OFFSET_SIZE, &(size as u32).to_le_bytes());
    put(Header::OFFSET_DROPPED, &record.dropped.to_le_bytes());
    put(Header::OFFSET_SIGNAL, &record.signal.to_le_bytes());
    put(Header::OFFSET_CODE, &record.code.to_le_bytes());
    put(Header::OFFSET_MAPPINGS, &(record.mappings.len() as u32).to_le_bytes());
    put(Header::OFFSET_ADDRESS, &record.address.to_le_bytes());
    put(Header::OFFSET_PROCESS, &record.process.to_le_bytes());
    put(Header::OFFSET_THREAD, &record.thread.to_le_bytes());
    put(Header::OFFSET_USER, &record.user.to_le_bytes());
    put(Header::OFFSET_GROUP, &record.group.to_le_bytes());
    for (i, ancestor) in record.ancestors.iter().enumerate() {
        put(Header::OFFSET_ANCESTORS + i * 8, &ancestor.to_le_bytes());
    }
    put(Header::OFFSET_WALL_NS, &record.wall_ns.unwrap_or(0).to_le_bytes());
    put(Header::OFFSET_THREAD_NAME, record.thread_name.as_bytes());
    put(Header::OFFSET_PC, &record.pc.to_le_bytes());
    put(Header::OFFSET_SP, &record.sp.to_le_bytes());
    put(Header::OFFSET_FP, &record.fp.to_le_bytes());
    put(Header::OFFSET_STACK_ADDRESS, &record.sp.to_le_bytes());
    put(Header::OFFSET_STACK_LEN, &(record.stack.len() as u32).to_le_bytes());
    let path_of = |path: &Option<String>| match path {
        Some(path) => (0u32, path.as_bytes().to_vec()),
        None => (PATH_UNKNOWN, Vec::new()),
    };
    let (flags, path) = path_of(&record.path);
    put(Header::OFFSET_PATH_FLAGS, &flags.to_le_bytes());
    put(Header::OFFSET_PATH_LEN, &(path.len() as u32).to_le_bytes());
    put(Header::OFFSET_PATH, &path);
    put(Header::OFFSET_DOMAIN_LEN, &(record.domain.len() as u32).to_le_bytes());
    put(Header::OFFSET_DOMAIN, record.domain.as_bytes());
    for (i, mapping) in record.mappings.iter().enumerate() {
        let at = Header::SIZE + i * Wire::SIZE;
        put(at + Wire::OFFSET_START, &mapping.start.to_le_bytes());
        put(at + Wire::OFFSET_END, &mapping.end.to_le_bytes());
        put(at + Wire::OFFSET_OFFSET, &mapping.offset.to_le_bytes());
        let (flags, path) = path_of(&mapping.path);
        put(at + Wire::OFFSET_PATH_FLAGS, &flags.to_le_bytes());
        put(at + Wire::OFFSET_PATH_LEN, &(path.len() as u32).to_le_bytes());
        put(at + Wire::OFFSET_PATH, &path);
    }
    let stack_at = Header::SIZE + record.mappings.len() * Wire::SIZE;
    put(stack_at, &record.stack);
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Record {
        Record {
            dropped: 2,
            signal: 11,
            code: 1,
            address: 0,
            process: 4012,
            thread: 4013,
            user: 1000,
            group: 1000,
            ancestors: vec![1],
            wall_ns: Some(1_763_027_000_000_000_000),
            thread_name: "crash-test".into(),
            pc: 0x10_1234,
            sp: 0x7fff_0000,
            fp: 0x7fff_0010,
            path: Some("/system/tests/report/crash-test".into()),
            domain: "crash-test".into(),
            mappings: vec![Mapping {
                start: 0x10_0000,
                end: 0x20_0000,
                offset: 0x1000,
                path: Some("/system/tests/report/crash-test".into()),
            }],
            stack: (0u8..64).collect(),
        }
    }

    #[test]
    fn a_record_reads_back_as_it_was_written() {
        let record = sample();
        assert_eq!(Record::read(&encode(&record)), Ok(record.clone()));
        assert_eq!(record.program(), "crash-test");
        assert_eq!(record.mapping_of(0x10_1234).map(|m| m.offset), Some(0x1000));
        assert_eq!(record.mapping_of(0x20_0000), None);
        assert_eq!(
            record.stack_word(0x7fff_0008),
            Some(u64::from_le_bytes([8, 9, 10, 11, 12, 13, 14, 15]))
        );
        assert_eq!(record.stack_word(0x7fff_0039), None);
        assert_eq!(record.stack_word(0x7ffe_fff8), None);
    }

    #[test]
    fn a_record_that_does_not_add_up_is_refused() {
        let bytes = encode(&sample());
        assert_eq!(Record::read(&bytes[..100]), Err(Malformed("shorter than its header")));
        assert_eq!(
            Record::read(&bytes[..bytes.len() - 1]),
            Err(Malformed("not the size its header says"))
        );
        let mut wrong = bytes.clone();
        wrong[Header::OFFSET_MAPPINGS] = 2;
        assert_eq!(Record::read(&wrong), Err(Malformed("its parts do not add up to its size")));
        let mut wrong = bytes;
        wrong[0] = b'X';
        assert_eq!(Record::read(&wrong), Err(Malformed("not a crash record")));
    }

    #[test]
    fn a_path_that_was_cut_or_is_unknown_is_none() {
        let mut record = sample();
        record.path = None;
        record.mappings[0].path = None;
        let read = Record::read(&encode(&record)).unwrap();
        assert_eq!(read.path, None);
        assert_eq!(read.program(), "crash-test");
        let mut bytes = encode(&sample());
        bytes[Header::OFFSET_PATH_FLAGS] = PATH_CUT as u8;
        assert_eq!(Record::read(&bytes).unwrap().path, None);
    }
}

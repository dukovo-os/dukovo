// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What the filesystem reads of the machine: the kernel's text files
//! under `/processes`, `/kernel` and `/system`, the processor's CPUID,
//! and random bytes. The filesystem works through [`Kernel`], so that
//! the host's tests run it against [`Fake`], a machine in memory.

use std::io;

/// The machine the filesystem describes.
pub trait Kernel: Send + Sync {
    /// The bytes of the file at `path`, which is absolute.
    fn read(&self, path: &str) -> io::Result<Vec<u8>>;

    /// The names in the directory at `path`, which is absolute.
    fn names(&self, path: &str) -> io::Result<Vec<String>>;

    /// CPUID leaf `leaf`, subleaf `sub`, of the CPU linuxd runs on: EAX,
    /// EBX, ECX and EDX.
    fn cpuid(&self, leaf: u32, sub: u32) -> [u32; 4];

    /// Fills `buffer` from the kernel's random number generator.
    fn random(&self, buffer: &mut [u8]) -> io::Result<()>;
}

/// The errno of a failed read, as the filesystem answers it.
pub fn errno_of(error: &io::Error) -> i32 {
    error.raw_os_error().unwrap_or(libc::EIO)
}

/// The machine linuxd runs on.
pub struct Machine;

impl Kernel for Machine {
    fn read(&self, path: &str) -> io::Result<Vec<u8>> {
        std::fs::read(path)
    }

    fn names(&self, path: &str) -> io::Result<Vec<String>> {
        let mut names = Vec::new();
        for entry in std::fs::read_dir(path)? {
            // A name that is not text is not one of the kernel's.
            if let Ok(name) = entry?.file_name().into_string() {
                names.push(name);
            }
        }
        Ok(names)
    }

    #[cfg(target_arch = "x86_64")]
    fn cpuid(&self, leaf: u32, sub: u32) -> [u32; 4] {
        let registers = core::arch::x86_64::__cpuid_count(leaf, sub);
        [registers.eax, registers.ebx, registers.ecx, registers.edx]
    }

    #[cfg(not(target_arch = "x86_64"))]
    fn cpuid(&self, _leaf: u32, _sub: u32) -> [u32; 4] {
        [0; 4]
    }

    fn random(&self, buffer: &mut [u8]) -> io::Result<()> {
        use std::io::Read;
        std::fs::File::open("/devices/random")?.read_exact(buffer)
    }
}

/// A machine in memory, for the host's tests: its files, its CPUID and
/// the bytes its random number generator gives.
#[cfg(test)]
pub struct Fake {
    files: std::collections::BTreeMap<String, Vec<u8>>,
    cpuid: fn(u32, u32) -> [u32; 4],
    random: u8,
}

#[cfg(test)]
impl Fake {
    /// A machine with no files, whose CPUID is all zeros and whose
    /// random bytes are all 0x5a.
    pub fn new() -> Fake {
        Fake { files: std::collections::BTreeMap::new(), cpuid: |_, _| [0; 4], random: 0x5a }
    }

    /// Adds the file `path` with `text`.
    #[must_use]
    pub fn with(mut self, path: &str, text: &str) -> Fake {
        self.files.insert(path.to_string(), text.as_bytes().to_vec());
        self
    }

    /// Gives the machine the CPUID `cpuid` answers.
    #[must_use]
    pub fn with_cpuid(mut self, cpuid: fn(u32, u32) -> [u32; 4]) -> Fake {
        self.cpuid = cpuid;
        self
    }

    /// Removes the file `path`.
    pub fn remove(&mut self, path: &str) {
        self.files.remove(path);
    }
}

#[cfg(test)]
impl Kernel for Fake {
    fn read(&self, path: &str) -> io::Result<Vec<u8>> {
        self.files.get(path).cloned().ok_or_else(|| io::Error::from_raw_os_error(libc::ENOENT))
    }

    fn names(&self, path: &str) -> io::Result<Vec<String>> {
        let prefix = format!("{}/", path.trim_end_matches('/'));
        let mut names: Vec<String> = self
            .files
            .keys()
            .filter_map(|file| file.strip_prefix(&prefix))
            .map(|rest| rest.split_once('/').map_or(rest, |(name, _)| name).to_string())
            .collect();
        names.dedup();
        if names.is_empty() {
            return Err(io::Error::from_raw_os_error(libc::ENOENT));
        }
        Ok(names)
    }

    fn cpuid(&self, leaf: u32, sub: u32) -> [u32; 4] {
        (self.cpuid)(leaf, sub)
    }

    fn random(&self, buffer: &mut [u8]) -> io::Result<()> {
        buffer.fill(self.random);
        Ok(())
    }
}

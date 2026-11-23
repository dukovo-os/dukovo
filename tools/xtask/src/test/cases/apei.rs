// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The hardware errors the firmware reports through ACPI's APEI (ADR 0662).
//! QEMU has no APEI on x86, so the test brings its own tables: a HEST with
//! one polled GHESv2 source (every 100 ms, an error status block of 4096
//! bytes, a read acknowledge register), and a BERT. QEMU's `-acpitable`
//! loads both, and its `loader` devices put the records in memory: the
//! source's error status address register holds the address of a block with
//! one corrected memory error that names the memory device with SMBIOS
//! handle 0x1100 (QEMU's `-smbios` file gives the machine that one memory
//! device, "DIMM 0"), and the BERT's region holds a fatal PCIe error of
//! function 0000:3b:00.0. `memreserve=` keeps the kernel off the pages the
//! records are in.
//!
//! The kernel logs the error from before this boot and does not panic for
//! it, and records it in `/kernel/ras/errors` as `bert fatal pcie`. It reads
//! the block, records the corrected error as `ghes3 corrected memory` of
//! the part DIMM 0, counts it for that memory device in Linux's EDAC files, writes
//! zero to the block's status, and acknowledges it by writing 1 to the read
//! acknowledge register (QEMU's monitor reads both). The block is not read
//! again: after two more seconds, 20 polls, the record still has the one
//! line.

use super::super::*;

/// The error source's identifier in the HEST.
const SOURCE_ID: u16 = 3;
/// The SMBIOS handle of the memory device the corrected error names:
/// `ddr4_device`'s.
const DIMM_HANDLE: u16 = 0x1100;

/// The error status address register: the 8 bytes that hold the address of
/// the error status block.
const STATUS_REGISTER: u64 = 0x3000_0000;
/// The read acknowledge register, which starts as 0.
const ACK_REGISTER: u64 = 0x3000_1000;
/// The error status block: 4096 bytes, the HEST's length for it.
const BLOCK: u64 = 0x3000_2000;
/// The BERT's boot error region: 4096 bytes.
const BOOT_REGION: u64 = 0x3000_3000;
/// The bytes of the block and of the region.
const BLOCK_BYTES: u32 = 4096;
/// The address of the memory error in the corrected record.
const ERROR_ADDRESS: u64 = 0x2000_0000;

/// The HEST, as `-acpitable` takes it, and the argument `run_case` replaces
/// with the file it writes it to.
const HEST_TABLE: &str = "file=hest.acpi";
/// The BERT, likewise.
const BERT_TABLE: &str = "file=bert.acpi";
/// The `loader` devices' arguments, each replaced by a device that loads one
/// file at its address ([`file_for`]).
const LOAD_STATUS_REGISTER: &str = "<apei status register>";
const LOAD_ACK_REGISTER: &str = "<apei acknowledge register>";
const LOAD_BLOCK: &str = "<apei block>";
const LOAD_BOOT_REGION: &str = "<apei boot region>";

/// The test.
pub const CASE: Case = Case {
    name: "apei",
    about: "the firmware's hardware error records (APEI): the BERT's fatal PCIe error of the last boot is logged and recorded without a panic; a polled GHESv2's block with a corrected memory error is read, recorded for DIMM 0, counted in the EDAC files, cleared and acknowledged, and not read again",
    firmware: Firmware::Bios,
    media: Media::Usb,
    cmdline: "console-sign-in=off memreserve=0x30000000,0x4000",
    cpus: 1,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &[
        // QEMU's memory device in its own place, with a handle that is known.
        "-smbios",
        DDR4_DEVICE,
        "-acpitable",
        HEST_TABLE,
        "-acpitable",
        BERT_TABLE,
        "-device",
        LOAD_STATUS_REGISTER,
        "-device",
        LOAD_ACK_REGISTER,
        "-device",
        LOAD_BLOCK,
        "-device",
        LOAD_BOOT_REGION,
    ],
    script: apei,
    disks: &[],
    second: None,
};

/// What `run_case` writes for the placeholder argument `arg`, if it is this
/// case's: the file's name, its bytes, and the physical address a `loader`
/// device puts them at (none for a table).
pub fn file_for(arg: &str) -> Option<(&'static str, Vec<u8>, Option<u64>)> {
    Some(match arg {
        HEST_TABLE => ("hest.acpi", hest(), None),
        BERT_TABLE => ("bert.acpi", bert(), None),
        LOAD_STATUS_REGISTER => {
            ("apei-status.bin", BLOCK.to_le_bytes().to_vec(), Some(STATUS_REGISTER))
        }
        LOAD_ACK_REGISTER => ("apei-ack.bin", vec![0; 8], Some(ACK_REGISTER)),
        LOAD_BLOCK => ("apei-block.bin", corrected_block(), Some(BLOCK)),
        LOAD_BOOT_REGION => ("apei-region.bin", fatal_block(), Some(BOOT_REGION)),
        _ => return None,
    })
}

fn put16(bytes: &mut [u8], at: usize, value: u16) {
    bytes[at..at + 2].copy_from_slice(&value.to_le_bytes());
}

fn put32(bytes: &mut [u8], at: usize, value: u32) {
    bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
}

fn put64(bytes: &mut [u8], at: usize, value: u64) {
    bytes[at..at + 8].copy_from_slice(&value.to_le_bytes());
}

/// A table with its header (36 bytes) and a correct checksum.
fn table(signature: [u8; 4], body: &[u8]) -> Vec<u8> {
    let mut table = Vec::with_capacity(36 + body.len());
    table.extend_from_slice(&signature);
    table.extend_from_slice(&((36 + body.len()) as u32).to_le_bytes());
    // The revision, the checksum, which is filled in below, and the OEM
    // fields as the other test tables have them.
    table.extend_from_slice(&[1, 0]);
    table.extend_from_slice(b"DUKOVO");
    table.extend_from_slice(b"APEI\0\0\0\0");
    table.extend_from_slice(&1u32.to_le_bytes());
    table.extend_from_slice(b"DKVO");
    table.extend_from_slice(&1u32.to_le_bytes());
    table.extend_from_slice(body);
    let sum = table.iter().fold(0u8, |sum, &byte| sum.wrapping_add(byte));
    table[9] = 0u8.wrapping_sub(sum);
    table
}

/// A Generic Address Structure (ACPI 6.5 5.2.3.2, 12 bytes): system memory,
/// 64 bits wide, at offset 0, qword accesses.
fn memory_register(address: u64) -> [u8; 12] {
    let mut gas = [0; 12];
    gas[1] = 64;
    gas[3] = 4;
    put64(&mut gas, 4, address);
    gas
}

/// The HEST: one error source, a Generic Hardware Error Source version 2
/// (type 10, 92 bytes; ACPI 6.5 18.3.2.8). The table's count is at 36 and
/// the source from 40.
fn hest() -> Vec<u8> {
    let mut source = vec![0u8; 92];
    // The type, the source's identifier, no related source (0xffff), the
    // flags, enabled, the records to pre-allocate, the most sections of a
    // record and the longest raw data.
    put16(&mut source, 0, 10);
    put16(&mut source, 2, SOURCE_ID);
    put16(&mut source, 4, 0xffff);
    source[7] = 1;
    put32(&mut source, 8, 1);
    put32(&mut source, 12, 1);
    put32(&mut source, 16, 0);
    // The error status address register at 20, and the Notification
    // Structure at 32: type 0 (polled), 28 bytes long, and the poll
    // interval at 36, in milliseconds.
    source[20..32].copy_from_slice(&memory_register(STATUS_REGISTER));
    source[33] = 28;
    put32(&mut source, 36, 100);
    // The error status block's length at 60, the read acknowledge register
    // at 64, and its preserve and write masks at 76 and 84.
    put32(&mut source, 60, BLOCK_BYTES);
    source[64..76].copy_from_slice(&memory_register(ACK_REGISTER));
    put64(&mut source, 76, 0);
    put64(&mut source, 84, 1);
    let mut body = 1u32.to_le_bytes().to_vec();
    body.extend_from_slice(&source);
    table(*b"HEST", &body)
}

/// The BERT (ACPI 6.5 18.3.1): the region's length at 36 and its address
/// at 40.
fn bert() -> Vec<u8> {
    let mut body = BLOCK_BYTES.to_le_bytes().to_vec();
    body.extend_from_slice(&BOOT_REGION.to_le_bytes());
    table(*b"BERT", &body)
}

/// A GUID as the firmware stores it: its first three fields little-endian.
fn guid(a: u32, b: u16, c: u16, d: [u8; 8]) -> [u8; 16] {
    let mut bytes = [0; 16];
    bytes[..4].copy_from_slice(&a.to_le_bytes());
    bytes[4..6].copy_from_slice(&b.to_le_bytes());
    bytes[6..8].copy_from_slice(&c.to_le_bytes());
    bytes[8..].copy_from_slice(&d);
    bytes
}

/// Section type: Platform Memory Error (UEFI 2.10, appendix N).
fn memory_error_type() -> [u8; 16] {
    guid(0xa5bc_1114, 0x6f64, 0x4ede, [0xb8, 0x63, 0x3e, 0x83, 0xed, 0x7c, 0x83, 0xb1])
}

/// Section type: PCI Express Error.
fn pcie_error_type() -> [u8; 16] {
    guid(0xd995_e954, 0xbbc1, 0x430f, [0xad, 0x91, 0xb4, 0x4d, 0xcb, 0x3c, 0x6f, 0x35])
}

/// A Generic Error Data Entry of revision 3.0 (ACPI 6.5 18.3.2.7.1, 72
/// bytes and the section): the section type at 0, the severity at 16, the
/// revision at 20, the validation bits at 22, the flags at 23 and the
/// section's length at 24.
fn entry(section_type: [u8; 16], severity: u32, section: &[u8]) -> Vec<u8> {
    let mut entry = vec![0u8; 72];
    entry[..16].copy_from_slice(&section_type);
    put32(&mut entry, 16, severity);
    put16(&mut entry, 20, 0x0300);
    put32(&mut entry, 24, section.len() as u32);
    entry.extend_from_slice(section);
    entry
}

/// A Generic Error Status Block with one entry (ACPI 6.5 18.3.2.7): the
/// block status at 0, the raw data's offset at 4 and length at 8, the
/// entries' length at 12 and the severity at 16, then the entry. The status
/// has the count of entries in bits 13:4, and bit 0 (an uncorrectable
/// error) or bit 1 (a correctable one).
fn status_block(severity: u32, entry: &[u8]) -> Vec<u8> {
    let kind = if severity == 2 { 1 << 1 } else { 1 };
    let mut block = vec![0u8; 20];
    put32(&mut block, 0, kind | 1 << 4);
    put32(&mut block, 12, entry.len() as u32);
    put32(&mut block, 16, severity);
    block.extend_from_slice(entry);
    block
}

/// The Memory Error Section (UEFI 2.10 N.2.5, 80 bytes): a single-bit ECC
/// error (type 2, bit 14 of the validation bits at 0, byte 72) at
/// [`ERROR_ADDRESS`] (bit 1, at 16) in the memory device with handle
/// [`DIMM_HANDLE`] (bit 17, at 78).
fn memory_section() -> Vec<u8> {
    let mut section = vec![0u8; 80];
    put64(&mut section, 0, 1 << 1 | 1 << 14 | 1 << 17);
    put64(&mut section, 16, ERROR_ADDRESS);
    section[72] = 2;
    put16(&mut section, 78, DIMM_HANDLE);
    section
}

/// The PCI Express Error Section (UEFI 2.10 N.2.7, 208 bytes) of the
/// function 0000:3b:00.0, whose device ID is valid (bit 3 of the validation
/// bits at 0): the device ID is at 24, with the vendor at 0, the device at
/// 2, the class code at 4, the function at 7, the device at 8, the segment
/// at 9 and the primary bus at 11.
fn pcie_section() -> Vec<u8> {
    let mut section = vec![0u8; 208];
    put64(&mut section, 0, 1 << 3);
    put16(&mut section, 24, 0x8086);
    put16(&mut section, 26, 0x1533);
    section[28..31].copy_from_slice(&[0x00, 0x00, 0x02]);
    section[35] = 0x3b;
    section
}

/// The block of the GHES: one corrected memory error.
fn corrected_block() -> Vec<u8> {
    status_block(2, &entry(memory_error_type(), 2, &memory_section()))
}

/// The BERT's region: one fatal PCI Express error.
fn fatal_block() -> Vec<u8> {
    status_block(1, &entry(pcie_error_type(), 1, &pcie_section()))
}

/// What QEMU's monitor says is at physical `address`: `size` is `w` for a
/// 32-bit word or `g` for a 64-bit one.
fn memory_at(vm: &mut Vm, address: u64, size: char) -> Result<u64, String> {
    let command = format!("xp /1{size}x {address:#x}");
    let text = vm.monitor_output(&command, 0)?;
    text.rsplit(':')
        .next()
        .and_then(|value| u64::from_str_radix(value.trim().trim_start_matches("0x"), 16).ok())
        .ok_or_else(|| format!("QEMU's monitor, asked to `{command}`: {}", text.trim()))
}

/// The `apei` case.
fn apei(vm: &mut Vm, case: &Case) -> Result<(), String> {
    boot_report(vm, case)?;
    vm.expect("Boot complete.")?;
    // Said before the first program starts: the range is kept out of use,
    // and the one source is read.
    if !vm.log().contains("Memory: 0x30000000-0x30004000 kept out of use (memreserve)") {
        return Err("the kernel did not keep the memory of the records out of use".into());
    }
    vm.expect_anywhere("APEI: 1 error sources read: 1 polled, 0 by SCI, 0 by NMI, 0 every second")?;
    vm.expect_anywhere("Hardware error before this boot: fatal, PCIe 0000:3b:00.0: PCIe error")?;
    vm.expect_anywhere(
        "Hardware error: corrected, from the firmware's error source 3, DIMM 0: memory error: ",
    )?;
    vm.expect("spark: started consoled (/system/commands/consoled) as process ")?;
    vm.expect_screen(&[&format!("{RELEASE} {VERSION} (tty1)"), "# "])?;
    // The brackets keep what each pattern says out of the typed line.
    vm.type_text(
        "clear; echo \"bert is $(grep -c 'bert fatal pci[e] x1 - PCIe 0000:3b:00.0: ' \
         /kernel/ras/errors) ghes is $(grep -c 'ghes3 corrected memory x1 dimm=0x1100 DIMM 0: \
         memory erro[r]' /kernel/ras/errors)\"\n",
    )?;
    vm.expect_screen(&["bert is 1 ghes is 1"])?;
    // The block is read once: 20 polls later its line is still the one.
    vm.type_text(
        "clear; sleep 2; echo \"ghes again is $(grep -c 'ghes[0-9]' /kernel/ras/errors)\"\n",
    )?;
    vm.expect_screen(&["ghes again is 1"])?;
    // The kernel cleared the block's status and acknowledged the record.
    let status = memory_at(vm, BLOCK, 'w')?;
    if status != 0 {
        return Err(format!("the block's status is {status:#x} after the kernel read it"));
    }
    let ack = memory_at(vm, ACK_REGISTER, 'g')?;
    if ack != 1 {
        return Err(format!("the read acknowledge register is {ack:#x}, not 1"));
    }
    // The corrected error is counted for the memory device it names.
    vm.type_text(
        "clear; cd /kernel/devices/system/edac/mc/mc0/dimm0; echo \"edac is $(cat dimm_ce_count) \
         $(cat dimm_label)\"\n",
    )?;
    vm.expect_screen(&["edac is 1 DIMM 0"])?;
    if vm.log().contains("Radix panic") {
        return Err("the kernel panicked".into());
    }
    vm.type_text("poweroff\n")?;
    vm.expect("spark: asked to power off the machine")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at16(bytes: &[u8], at: usize) -> u16 {
        u16::from_le_bytes(bytes[at..at + 2].try_into().unwrap())
    }

    fn at32(bytes: &[u8], at: usize) -> u32 {
        u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
    }

    fn at64(bytes: &[u8], at: usize) -> u64 {
        u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap())
    }

    /// A table's length and checksum are right.
    fn check_header(bytes: &[u8], signature: [u8; 4]) {
        assert_eq!(bytes[..4], signature);
        assert_eq!(at32(bytes, 4) as usize, bytes.len());
        assert_eq!(bytes.iter().fold(0u8, |sum, &byte| sum.wrapping_add(byte)), 0);
    }

    #[test]
    fn the_hest_has_one_polled_ghes_version_2() {
        let hest = hest();
        check_header(&hest, *b"HEST");
        assert_eq!(hest.len(), 36 + 4 + 92);
        assert_eq!(at32(&hest, 36), 1, "one error source");
        let source = &hest[40..];
        assert_eq!(at16(source, 0), 10, "a GHESv2");
        assert_eq!(at16(source, 2), SOURCE_ID);
        assert_eq!(source[7], 1, "enabled");
        // The error status address register: memory, 64 bits, qwords.
        assert_eq!(source[20..24], [0, 64, 0, 4]);
        assert_eq!(at64(source, 24), STATUS_REGISTER);
        // The notification: polled, 28 bytes, every 100 ms.
        assert_eq!(source[32], 0);
        assert_eq!(source[33], 28);
        assert_eq!(at32(source, 36), 100);
        assert_eq!(at32(source, 60), 4096);
        // The read acknowledge register, preserve 0, write 1.
        assert_eq!(source[64..68], [0, 64, 0, 4]);
        assert_eq!(at64(source, 68), ACK_REGISTER);
        assert_eq!(at64(source, 76), 0);
        assert_eq!(at64(source, 84), 1);
    }

    #[test]
    fn the_bert_names_the_boot_region() {
        let bert = bert();
        check_header(&bert, *b"BERT");
        assert_eq!(bert.len(), 48);
        assert_eq!(at32(&bert, 36), 4096);
        assert_eq!(at64(&bert, 40), BOOT_REGION);
    }

    #[test]
    fn the_corrected_block_has_one_memory_entry_of_the_dimm() {
        let block = corrected_block();
        assert_eq!(at32(&block, 0), 0x12, "one correctable error");
        assert_eq!(at32(&block, 12) as usize, block.len() - 20);
        assert_eq!(at32(&block, 16), 2, "corrected");
        assert_eq!(block[20..36], memory_error_type());
        assert_eq!(at32(&block, 36), 2);
        assert_eq!(at16(&block, 40), 0x0300);
        assert_eq!(at32(&block, 44), 80);
        let section = &block[20 + 72..];
        assert_eq!(section.len(), 80);
        // Validation bits: the address (1), the type (14), the module
        // handle (17).
        assert_eq!(at64(section, 0), 1 << 1 | 1 << 14 | 1 << 17);
        assert_eq!(at64(section, 16), ERROR_ADDRESS);
        assert_eq!(section[72], 2);
        assert_eq!(at16(section, 78), DIMM_HANDLE);
        assert!(block.len() <= BLOCK_BYTES as usize);
    }

    #[test]
    fn the_fatal_block_has_one_pcie_entry_of_the_function() {
        let block = fatal_block();
        assert_eq!(at32(&block, 0), 0x11, "one uncorrectable error");
        assert_eq!(at32(&block, 12) as usize, block.len() - 20);
        assert_eq!(at32(&block, 16), 1, "fatal");
        assert_eq!(block[20..36], pcie_error_type());
        assert_eq!(at32(&block, 36), 1);
        assert_eq!(at32(&block, 44), 208);
        let section = &block[20 + 72..];
        assert_eq!(section.len(), 208);
        assert_eq!(at64(section, 0), 1 << 3, "the device ID is valid");
        let id = &section[24..40];
        // Function 0, device 0, segment 0, bus 0x3b.
        assert_eq!((id[7], id[8], at16(id, 9), id[11]), (0, 0, 0, 0x3b));
        assert!(block.len() <= BLOCK_BYTES as usize);
    }

    #[test]
    fn the_guids_are_stored_as_uefi_does() {
        assert_eq!(
            memory_error_type(),
            [
                0x14, 0x11, 0xbc, 0xa5, 0x64, 0x6f, 0xde, 0x4e, 0xb8, 0x63, 0x3e, 0x83, 0xed, 0x7c,
                0x83, 0xb1
            ]
        );
        assert_eq!(pcie_error_type()[..4], [0x54, 0xe9, 0x95, 0xd9]);
    }

    #[test]
    fn the_records_do_not_overlap_and_are_kept_out_of_use() {
        let places = [
            (STATUS_REGISTER, 8),
            (ACK_REGISTER, 8),
            (BLOCK, u64::from(BLOCK_BYTES)),
            (BOOT_REGION, u64::from(BLOCK_BYTES)),
        ];
        for (i, (start, len)) in places.iter().enumerate() {
            for (other, other_len) in &places[i + 1..] {
                assert!(start + len <= *other || other + other_len <= *start);
            }
            assert!(start + len <= 1 << 30, "inside the 1 GiB of RAM");
        }
        // `memreserve=` covers every one of them, and the DIMM's handle is
        // the one `ddr4_device` gives.
        assert_eq!(CASE.cmdline, "console-sign-in=off memreserve=0x30000000,0x4000");
        let kept = 0x3000_0000..0x3000_0000 + 0x4000;
        assert!(places.iter().all(|(start, len)| kept.contains(start) && start + len <= kept.end));
        assert_eq!(ddr4_device()[2..4], DIMM_HANDLE.to_le_bytes());
    }

    #[test]
    fn every_placeholder_has_a_file() {
        for arg in CASE.qemu.iter().filter(|arg| arg.starts_with('<') || arg.starts_with("file=")) {
            if *arg != DDR4_DEVICE {
                assert!(file_for(arg).is_some(), "{arg}");
            }
        }
        let (name, bytes, address) = file_for(LOAD_STATUS_REGISTER).unwrap();
        assert_eq!((name, address), ("apei-status.bin", Some(STATUS_REGISTER)));
        assert_eq!(at64(&bytes, 0), BLOCK);
        assert!(file_for("file=power.aml").is_none());
    }
}

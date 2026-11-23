// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The BMC of a server (ADR 0661), as QEMU's ipmi-bmc-sim plays it on an
//! isa-ipmi-kcs interface. The test gives the BMC a sensor data
//! repository of one temperature sensor, *Test Inlet*, whose conversion
//! is known (a raw reading x is (5 x - 200) / 10 degrees, so that its
//! nominal, normal minimum and normal maximum raw readings of 80, 60 and
//! 100 are 20, 10 and 30 degrees), and a FRU whose board area names its
//! maker *Dukovo Test Works*, both made by the test. The kernel finds the
//! BMC through ACPI's IPI0001 and logs the line that says where its KCS
//! registers are, adds /devices/ipmi0, and names the BMC's watchdog as
//! the machine's. alice, the owner, signs in; as an administrator,
//! through elevate, ipmitool(1) answers *mc info*, *sdr list* and *sdr
//! get* with the converted nominal readings, and *fru print 0* with the
//! board's maker, which proves Linux's ioctls on /devices/ipmi0 against
//! the tool administrators use; bmc(8) shows the BMC, its sensor and its
//! FRU, and asks for the identify light for five seconds. The power
//! button ends it.

use super::super::*;

pub const CASE: Case = Case {
    name: "ipmi",
    about: "a BMC through QEMU's ipmi-bmc-sim on a KCS interface: the kernel logs where it is, adds /devices/ipmi0 and names the BMC's watchdog; ipmitool answers mc info, sdr list, an sdr get whose nominal readings are converted as the test's sensor record says, and fru print with the board's maker; bmc shows the BMC, its sensor and its FRU, and asks for the identify light",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    cmdline: "lockdep",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &["-device", IPMI_BMC, "-device", "isa-ipmi-kcs,bmc=bmc0"],
    script: ipmi,
    disks: &[],
    second: None,
};

/// The name of the test's sensor.
const SENSOR_NAME: &str = "Test Inlet";

/// The board area's fields, in the order the FRU specification gives them
/// after the manufacturing date: maker, product name, serial number, part
/// number and FRU file ID.
const BOARD_FIELDS: [&str; 5] = ["Dukovo Test Works", "Test Board 7", "SN-0042", "PN-19", "fru-1"];

/// alice's password.
const PASSWORD: &str = "queen of hearts";

/// The sensor data repository's one record, a full sensor record (IPMI
/// 2.0 specification, table 43-1) of a temperature sensor the BMC itself
/// owns. The comments give the table's byte numbers, which count from 1.
pub(crate) fn sdr_file() -> Vec<u8> {
    let mut record = vec![0u8; 48];
    // Bytes 1 and 2: the record ID, which the BMC may number again.
    record[0] = 1;
    // Byte 3: the SDR version, 51h. Byte 4: the record type, 01h.
    record[2] = 0x51;
    record[3] = 0x01;
    // Byte 6: the sensor owner, the BMC at slave address 20h. Byte 7: its
    // channel 0 and LUN 0. Byte 8: the sensor number.
    record[5] = 0x20;
    record[7] = 1;
    // Byte 9: the entity, 37h (55), the air inlet. Byte 10: its instance 1.
    record[8] = 0x37;
    record[9] = 1;
    // Byte 11: scanning and event generation enabled at start. Byte 12:
    // the sensor re-arms by itself.
    record[10] = 0x03;
    record[11] = 0x40;
    // Byte 13: the sensor type, 01h temperature. Byte 14: the event/reading
    // type code, 01h, a threshold sensor.
    record[12] = 0x01;
    record[13] = 0x01;
    // Byte 21: the units, unsigned and not a rate, a modifier or a
    // percentage. Byte 22: the base unit, 1, degrees Celsius.
    record[20] = 0x00;
    record[21] = 0x01;
    // Byte 24: the linearization, 0, linear. Byte 25: M's low 8 bits, 5.
    record[23] = 0;
    record[24] = 5;
    // Bytes 27 and 28: B is -20, 3ECh in ten bits, so its low 8 bits are
    // ECh and its top two bits, 11b, are the top of byte 28.
    record[26] = 0xEC;
    record[27] = 0xC0;
    // Byte 30: the result exponent -1 (Fh) in the high nibble and B's
    // exponent 1 in the low one: the reading is (5 x + (-20) * 10) * 10^-1.
    record[29] = 0xF1;
    // Byte 31: the nominal reading, the normal maximum and the normal
    // minimum are given. Bytes 32 to 34: their raw readings, 20, 30 and 10
    // degrees. Bytes 35 and 36: the sensor's own maximum and minimum.
    record[30] = 0x07;
    record[31] = 80;
    record[32] = 100;
    record[33] = 60;
    record[34] = 0xFF;
    record[35] = 0x00;
    // Byte 48: the ID string is 8-bit ASCII (11b in the top two bits) and
    // its length is in the low five.
    let length = u8::try_from(SENSOR_NAME.len()).expect("the name fits a byte");
    record[47] = 0xC0 | length;
    record.extend_from_slice(SENSOR_NAME.as_bytes());
    // Byte 5: the bytes that follow it.
    record[4] = u8::try_from(record.len() - 5).expect("the record fits a byte");
    record
}

/// The FRU's data (Platform Management FRU Information Storage
/// Definition 1.0): the common header (section 8), whose only area is the
/// board info area (section 11) at offset 8, with the board's fields as
/// 8-bit ASCII (section 13).
pub(crate) fn fru_file() -> Vec<u8> {
    // The format version, offsets of the internal use, chassis, board,
    // product and multirecord areas in units of 8 bytes, a pad, and the
    // checksum that makes the header's bytes sum to zero.
    let mut header = vec![1, 0, 0, 1, 0, 0, 0];
    header.push(checksum(&header));
    // The format version, the area's length in units of 8 bytes (filled in
    // below), the language, 0 for English, and the manufacturing date, 0
    // for unspecified.
    let mut board = vec![1, 0, 0, 0, 0, 0];
    for field in BOARD_FIELDS {
        let length = u8::try_from(field.len()).expect("the field fits a type/length byte");
        board.push(0xC0 | length);
        board.extend_from_slice(field.as_bytes());
    }
    // The end of the fields, then padding to a whole number of units with
    // the checksum last.
    board.push(0xC1);
    while (board.len() + 1) % 8 != 0 {
        board.push(0);
    }
    board.push(0);
    board[1] = u8::try_from(board.len() / 8).expect("the area fits a byte");
    let last = board.len() - 1;
    board[last] = checksum(&board[..last]);
    header.extend(board);
    header
}

/// The byte that makes `bytes` and itself sum to zero modulo 256.
fn checksum(bytes: &[u8]) -> u8 {
    bytes.iter().fold(0u8, |sum, byte| sum.wrapping_add(*byte)).wrapping_neg()
}

/// `command` as root: alice, the owner, is an administrator, and elevate
/// reads her password from its standard input with `-S`. Its prompt is
/// empty, as it goes to the standard error, which the output files hold:
/// elevate's own `[elevate] password for alice: ` would begin the
/// command's first line.
fn as_root(command: &str) -> String {
    format!("echo '{PASSWORD}' | elevate -S -p '' {command}")
}

/// Runs `command` as root with its output in `/tmp/<name>` and its exit
/// status in `/tmp/<name>.exit`, then, when the prompt is back, checks that
/// it ended with status 0 and that each pattern (a label and an extended
/// regular expression) matches exactly one line of the output. A line that
/// says `<label> 1` comes from the count the typed command makes, not from
/// the command's text, which holds no such words. The BMC answers one byte
/// at a time under emulation, so the command may take a while.
fn run_and_find(
    vm: &mut Vm,
    name: &str,
    command: &str,
    finds: &[(&str, &str)],
) -> Result<(), String> {
    answer_prompt(
        vm,
        "$",
        &format!("clear; {} > /tmp/{name} 2>&1; echo $? > /tmp/{name}.exit", as_root(command)),
    )?;
    let mut counts = String::new();
    for (label, pattern) in finds {
        counts.push_str(&format!("; echo \"{label} $(grep -c -E '{pattern}' /tmp/{name})\""));
    }
    answer_prompt_within(
        vm,
        "$",
        &format!("clear; echo \"{name} exit $(cat /tmp/{name}.exit)\"{counts}"),
        300,
    )?;
    let mut expected = vec![format!("{name} exit 0")];
    expected.extend(finds.iter().map(|(label, _)| format!("{label} 1")));
    let expected: Vec<&str> = expected.iter().map(String::as_str).collect();
    vm.expect_screen(&expected)
        .map_err(|err| format!("{command}: {err}; its output is in /tmp/{name} on the machine"))
}

fn ipmi(vm: &mut Vm, case: &Case) -> Result<(), String> {
    owner_signs_in(vm, case)?;
    // The kernel's line says which interface, from the ACPI device QEMU
    // describes, and the BMC's watchdog is the one it names.
    vm.expect_anywhere("IPMI: BMC ")?;
    vm.expect_anywhere(", KCS at I/O 0xCA2 (spacing 1)")?;
    vm.expect_anywhere("Watchdog: the BMC's watchdog (IPMI), with timeouts from 1 to 6553 s")?;

    // The device is there for root, and the watchdog is the BMC's.
    answer_prompt(
        vm,
        "$",
        "clear; echo \"found $(ls /devices/ipmi0)\"; \
         echo \"identity $(grep -c 'BMC.s watchdog (IPMI)' /kernel/class/watchdog/watchdog0/identity)\"",
    )?;
    vm.expect_screen(&["found /devices/ipmi0", "identity 1"])?;

    // ipmitool, through Linux's ioctls.
    run_and_find(
        vm,
        "mcinfo",
        "ipmitool mc info",
        &[("device", "^Device ID +: "), ("firmware", "^Firmware Revision +: ")],
    )?;
    run_and_find(
        vm,
        "sdrlist",
        "ipmitool sdr list",
        &[("sensor", &format!("^{SENSOR_NAME} +\\|"))],
    )?;
    // The nominal, normal minimum and normal maximum are converted from the
    // record's raw 80, 60 and 100 by M = 5, B = -200 and a result exponent
    // of -1; ipmitool prints three decimals.
    run_and_find(
        vm,
        "sdrget",
        &format!("ipmitool sdr get \"{SENSOR_NAME}\""),
        &[
            ("nominal", "Nominal Reading +: 20\\.000$"),
            ("minimum", "Normal Minimum +: 10\\.000$"),
            ("maximum", "Normal Maximum +: 30\\.000$"),
        ],
    )?;
    // FRU 0 by its number: QEMU's BMC answers Get Device ID with a sensor
    // device, an SDR repository and a SEL but not FRU inventory, and
    // ipmitool's `fru print` with no number reads the BMC's own FRU only
    // when that bit is set, as a server's BMC sets it.
    run_and_find(
        vm,
        "fruprint",
        "ipmitool fru print 0",
        &[("maker", &format!("Board Mfg +: {}$", BOARD_FIELDS[0]))],
    )?;

    // bmc(8), through the same device.
    run_and_find(vm, "bmc", "bmc", &[("maker", "^Maker +"), ("firmware", "^Firmware +[0-9]")])?;
    run_and_find(vm, "bmcsensors", "bmc sensors", &[("sensor", &format!("^{SENSOR_NAME} +"))])?;
    run_and_find(vm, "bmcfru", "bmc fru", &[("maker", &format!("Maker +{}$", BOARD_FIELDS[0]))])?;
    // QEMU's simulated BMC may not have Chassis Identify: then the command
    // still goes to the BMC and back, and `bmc` says it was refused.
    answer_prompt(
        vm,
        "$",
        &format!("clear; {} > /tmp/bmcidentify 2>&1", as_root("bmc identify 5")),
    )?;
    answer_prompt(
        vm,
        "$",
        "clear; echo \"identify answered $(grep -c -E 'is on for 5 seconds|invalid command \\(0xC1\\)' /tmp/bmcidentify)\"",
    )?;
    vm.expect_screen(&["identify answered 1"])
        .map_err(|err| format!("bmc identify 5: {err}; its output is in /tmp/bmcidentify"))?;

    vm.press_power_button()?;
    vm.expect("spark: every service has stopped")?;
    vm.expect("Power: `spark` asked to power off the machine; syncing the filesystems")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sensor_record_says_how_long_it_is() {
        let record = sdr_file();
        assert_eq!(usize::from(record[4]), record.len() - 5);
        assert_eq!(record.len(), 48 + SENSOR_NAME.len());
        assert_eq!(&record[48..], SENSOR_NAME.as_bytes());
        assert_eq!(record[47] & 0x1F, u8::try_from(SENSOR_NAME.len()).unwrap());
    }

    #[test]
    fn the_sensor_converts_raw_readings_as_the_comment_says() {
        let record = sdr_file();
        let m = i32::from(record[24]);
        // B is a signed ten-bit number: the low byte and the top two bits
        // of the next.
        let b = i32::from(record[26]) | (i32::from(record[27] >> 6) << 8);
        let b = if b >= 512 { b - 1024 } else { b };
        let result_exponent = i32::from(record[29] >> 4) - 16;
        let b_exponent = i32::from(record[29] & 0x0F);
        let convert = |raw: u8| {
            f64::from(m * i32::from(raw) + b * 10i32.pow(b_exponent.unsigned_abs()))
                * 10f64.powi(result_exponent)
        };
        assert!((convert(record[31]) - 20.0).abs() < 1e-9);
        assert!((convert(record[33]) - 10.0).abs() < 1e-9);
        assert!((convert(record[32]) - 30.0).abs() < 1e-9);
    }

    #[test]
    fn the_fru_areas_sum_to_zero_and_hold_the_board_fields() {
        let fru = fru_file();
        let sum = |bytes: &[u8]| bytes.iter().fold(0u8, |sum, byte| sum.wrapping_add(*byte));
        assert_eq!(sum(&fru[..8]), 0);
        assert_eq!(fru[3], 1);
        let board = &fru[8..];
        assert_eq!(board.len(), usize::from(board[1]) * 8);
        assert_eq!(sum(board), 0);
        let text = String::from_utf8_lossy(board);
        for field in BOARD_FIELDS {
            assert!(text.contains(field));
        }
    }
}

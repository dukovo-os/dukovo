// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Tests of the PCI Express error texts and register decoding, against
//! the exact output of Linux's `drivers/pci/pcie/aer.c`.

use std::string::{String, ToString};

use super::aer::{self, Counters, Names};
use super::dpc::{self, Reason};
use super::osc::{self, Granted};
use super::{PortType, express};

fn names(bits: u32, correctable: bool) -> String {
    Names { bits, correctable }.to_string()
}

#[test]
fn port_types_follow_the_capabilities_register() {
    let cases = [
        (0x0, PortType::Endpoint),
        (0x1, PortType::LegacyEndpoint),
        (0x4, PortType::RootPort),
        (0x5, PortType::Upstream),
        (0x6, PortType::Downstream),
        (0x7, PortType::PcieToPci),
        (0x8, PortType::PciToPcie),
        (0x9, PortType::Integrated),
        (0xa, PortType::EventCollector),
        (0x2, PortType::Other(2)),
        (0xf, PortType::Other(0xf)),
    ];
    for (code, expected) in cases {
        // The version in the low bits and a slot flag above are ignored.
        assert_eq!(PortType::of(code << 4 | 0x2 | 1 << 8), expected);
    }
    assert!(PortType::RootPort.is_root() && PortType::EventCollector.is_root());
    assert!(!PortType::Downstream.is_root() && !PortType::Integrated.is_root());
    assert!(PortType::RootPort.is_downstream() && PortType::Downstream.is_downstream());
    assert!(!PortType::EventCollector.is_downstream() && !PortType::Upstream.is_downstream());
}

#[test]
fn interrupt_messages_come_from_their_bits() {
    assert_eq!(express::interrupt_message(0b11_1110_0000_0010), 0x1f);
    assert_eq!(express::interrupt_message(0x0002), 0);
    assert_eq!(aer::root::interrupt_message(0xf800_007f), 0x1f);
    assert_eq!(aer::root::interrupt_message(0x1000_0000), 2);
    assert_eq!(dpc::interrupt_message(0xffe1), 1);
    assert_eq!(aer::first_error(0xffff_ffe5), 5);
}

#[test]
fn names_text_lists_bits_in_order() {
    assert_eq!(names(1 << 0, true), "RxErr");
    assert_eq!(names(1 << 6 | 1 << 12, true), "BadTLP, Timeout");
    assert_eq!(names(1 << 3, true), "bit 3");
    assert_eq!(names(1 << 3 | 1 << 7 | 1 << 15, true), "bit 3, BadDLLP, HeaderOF");
    assert_eq!(names(0, true), "none");
    assert_eq!(names(0, false), "none");
    assert_eq!(names(1 << 14, false), "CmpltTO");
    assert_eq!(names(1 << 1 | 1 << 20 | 1 << 31, false), "bit 1, UnsupReq, TLPXlatBlocked");
    assert_eq!(names(1 << 16, true), "none", "bit 16 is not a correctable bit");
}

#[test]
fn every_name_is_linuxs() {
    let correctable = [
        (0, "RxErr"),
        (6, "BadTLP"),
        (7, "BadDLLP"),
        (8, "Rollover"),
        (12, "Timeout"),
        (13, "NonFatalErr"),
        (14, "CorrIntErr"),
        (15, "HeaderOF"),
    ];
    for bit in 0..16 {
        let expected = correctable.iter().find(|(b, _)| *b == bit).map(|(_, n)| *n);
        assert_eq!(aer::CORRECTABLE_NAMES[bit], expected, "correctable bit {bit}");
    }
    let uncorrectable = [
        "Undefined",
        "",
        "",
        "",
        "DLP",
        "SDES",
        "",
        "",
        "",
        "",
        "",
        "",
        "TLP",
        "FCP",
        "CmpltTO",
        "CmpltAbrt",
        "UnxCmplt",
        "RxOF",
        "MalfTLP",
        "ECRC",
        "UnsupReq",
        "ACSViol",
        "UncorrIntErr",
        "BlockedTLP",
        "AtomicOpBlocked",
        "TLPBlockedErr",
        "PoisonTLPBlocked",
        "DMWrReqBlocked",
        "IDECheck",
        "MisIDETLP",
        "PCRC_CHECK",
        "TLPXlatBlocked",
    ];
    for (bit, expected) in uncorrectable.iter().enumerate() {
        let expected = (!expected.is_empty()).then_some(*expected);
        assert_eq!(aer::UNCORRECTABLE_NAMES[bit], expected, "uncorrectable bit {bit}");
    }
}

#[test]
fn the_header_log_holds_a_tlp_for_linuxs_bits() {
    let bits = [12, 15, 16, 18, 19, 20, 21, 23, 24, 25, 26, 27, 28, 29, 30, 31];
    let expected = bits.iter().fold(0u32, |mask, bit| mask | 1 << bit);
    assert_eq!(aer::HEADER_LOGGED, expected);
}

fn text(write: impl Fn(&mut String) -> core::fmt::Result) -> String {
    let mut out = String::new();
    write(&mut out).unwrap();
    out
}

#[test]
fn a_correctable_report_is_counted_as_linux_prints_it() {
    let mut counters = Counters::new();
    counters.add_correctable(1 << 0 | 1 << 6);
    let expected = "RxErr 1\nBadTLP 1\nBadDLLP 0\nRollover 0\nTimeout 0\nNonFatalErr 0\n\
                    CorrIntErr 0\nHeaderOF 0\nTOTAL_ERR_COR 1\n";
    assert_eq!(text(|out| counters.write_correctable(out)), expected);
    assert_eq!(counters.totals(), (1, 0, 0));
}

#[test]
fn an_unnamed_correctable_bit_prints_only_with_a_count() {
    let mut counters = Counters::new();
    assert!(!text(|out| counters.write_correctable(out)).contains("dev_cor_errs_bit"));
    counters.add_correctable(1 << 3 | 1 << 20);
    counters.add_correctable(1 << 3);
    let out = text(|out| counters.write_correctable(out));
    assert!(out.starts_with("RxErr 0\ndev_cor_errs_bit[3] 2\nBadTLP 0\n"), "{out}");
    assert!(out.ends_with("HeaderOF 0\nTOTAL_ERR_COR 2\n"), "{out}");
    assert_eq!(out.matches("dev_cor_errs_bit").count(), 1, "bit 20 is ignored");
}

#[test]
fn a_nonfatal_report_is_counted_as_linux_prints_it() {
    let mut counters = Counters::new();
    counters.add_uncorrectable(1 << 14, false);
    let expected = "Undefined 0\nDLP 0\nSDES 0\nTLP 0\nFCP 0\nCmpltTO 1\nCmpltAbrt 0\n\
                    UnxCmplt 0\nRxOF 0\nMalfTLP 0\nECRC 0\nUnsupReq 0\nACSViol 0\n\
                    UncorrIntErr 0\nBlockedTLP 0\nAtomicOpBlocked 0\nTLPBlockedErr 0\n\
                    PoisonTLPBlocked 0\nDMWrReqBlocked 0\nIDECheck 0\nMisIDETLP 0\n\
                    PCRC_CHECK 0\nTLPXlatBlocked 0\nTOTAL_ERR_NONFATAL 1\n";
    assert_eq!(text(|out| counters.write_nonfatal(out)), expected);
    assert_eq!(counters.totals(), (0, 1, 0));
    assert!(
        text(|out| counters.write_fatal(out)).ends_with("TLPXlatBlocked 0\nTOTAL_ERR_FATAL 0\n")
    );
}

#[test]
fn a_fatal_report_and_an_unnamed_bit_print_under_their_own_labels() {
    let mut counters = Counters::new();
    counters.add_uncorrectable(1 << 1 | 1 << 17, true);
    let out = text(|out| counters.write_fatal(out));
    assert!(out.starts_with("Undefined 0\ndev_fatal_errs_bit[1] 1\nDLP 0\n"), "{out}");
    assert!(out.contains("\nRxOF 1\n"), "{out}");
    assert!(out.ends_with("TLPXlatBlocked 0\nTOTAL_ERR_FATAL 1\n"), "{out}");
    assert!(text(|out| counters.write_nonfatal(out)).ends_with("TOTAL_ERR_NONFATAL 0\n"));
    counters.add_uncorrectable(1 << 2, false);
    let out = text(|out| counters.write_nonfatal(out));
    assert!(out.contains("\ndev_nonfatal_errs_bit[2] 1\n"), "{out}");
    assert_eq!(counters.totals(), (0, 1, 1));
}

#[test]
fn counts_stop_at_the_largest_value() {
    let mut counters = Counters::new();
    for _ in 0..3 {
        counters.add_correctable(1);
    }
    assert_eq!(counters.totals().0, 3);
    let mut full = counters;
    full.correctable[0] = u64::MAX;
    full.total_correctable = u64::MAX;
    full.add_correctable(1);
    assert_eq!(full.correctable[0], u64::MAX);
    assert_eq!(full.totals().0, u64::MAX);
    assert_eq!(counters, counters.clone());
    assert_eq!(Counters::new(), Counters::default());
}

#[test]
fn cper_registers_are_read_without_their_masked_bits() {
    let mut bytes = [0u8; 96];
    let mut put = |offset: usize, value: u32| {
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    };
    put(aer::UNCORRECTABLE_STATUS, 0x0010_4001);
    put(aer::UNCORRECTABLE_MASK, 0x0000_0001);
    put(aer::UNCORRECTABLE_SEVERITY, 0x0044_6030);
    put(aer::CORRECTABLE_STATUS, 0x0000_10c1);
    put(aer::CORRECTABLE_MASK, 0x0000_1000);
    assert_eq!(aer::from_cper(&bytes), (0x0000_00c1, 0x0010_4000, 0x0044_6030));
}

#[test]
fn dpc_control_triggers_on_fatal_and_keeps_other_bits() {
    assert_eq!(dpc::control_for_fatal(0), 0x9);
    assert_eq!(dpc::control_for_fatal(0xffff), 0xffad);
    // Non-fatal trigger (bit 1), ERR_COR (4) and software (6) go off.
    assert_eq!(dpc::control_for_fatal(0b101_0010), 0x9);
    // Completion control (2), poisoned TLP blocking (5) and bit 7 stay.
    assert_eq!(dpc::control_for_fatal(0b1010_0100), 0b1010_1101);
}

#[test]
fn dpc_reasons_are_named() {
    let cases = [
        (0b000_0000, Reason::Unmasked, "an unmasked uncorrectable error"),
        (0b000_0010, Reason::Nonfatal, "an ERR_NONFATAL message"),
        (0b000_0100, Reason::Fatal, "an ERR_FATAL message"),
        (0b000_0110, Reason::RpPio, "a root port I/O error"),
        (0b010_0110, Reason::Software, "a software trigger"),
        (0b100_0110, Reason::Other, "a reason the port does not name"),
        (0b110_0110, Reason::Other, "a reason the port does not name"),
    ];
    for (status, reason, text) in cases {
        // The triggered bit does not change the reason.
        for status in [status, status | dpc::TRIGGERED] {
            assert_eq!(dpc::reason(status), reason, "status {status:#b}");
            assert_eq!(dpc::reason(status).to_string(), text);
        }
    }
}

#[test]
fn osc_calls_carry_the_support_and_control_dwords() {
    assert_eq!(osc::SUPPORT, 0b11001);
    assert_eq!(osc::ASKED, 0x98);
    assert_eq!(osc::dwords(true, osc::ASKED), [1, 0b11001, 0x98]);
    assert_eq!(osc::dwords(false, 0x18), [0, 0b11001, 0x18]);
    assert_eq!(
        osc::UUID,
        [
            0x5b, 0x4d, 0xdb, 0x33, 0xf7, 0x1f, 0x1c, 0x40, 0x96, 0x57, 0x74, 0x41, 0xc0, 0x3d,
            0xd7, 0x66
        ]
    );
}

#[test]
fn osc_takes_only_what_it_can_use() {
    let all = osc::AER | osc::CAPABILITY | osc::DPC;
    // Only what the kernel asks for.
    assert_eq!(osc::to_take(0xffff_ffff), all);
    assert_eq!(osc::to_take(osc::PME | osc::HOTPLUG | osc::LTR), 0);
    // AER is dropped without the PCIe capability, and DPC with it.
    assert_eq!(osc::to_take(osc::AER | osc::DPC), 0);
    assert_eq!(osc::to_take(osc::AER), 0);
    assert_eq!(osc::to_take(osc::AER | osc::CAPABILITY), osc::AER | osc::CAPABILITY);
    // DPC is dropped without AER.
    assert_eq!(osc::to_take(osc::DPC | osc::CAPABILITY), osc::CAPABILITY);
    assert_eq!(osc::to_take(osc::CAPABILITY), osc::CAPABILITY);
    assert_eq!(osc::to_take(0), 0);
}

#[test]
fn granted_controls_read_as_a_list() {
    let text = |bits| Granted(bits).to_string();
    assert_eq!(text(0), "nothing");
    assert_eq!(text(osc::AER), "AER");
    assert_eq!(text(osc::AER | osc::CAPABILITY), "AER and the PCIe capability");
    assert_eq!(text(osc::AER | osc::CAPABILITY | osc::DPC), "AER, DPC and the PCIe capability");
    assert_eq!(
        text(0xff),
        "AER, DPC, the PCIe capability, PME, LTR, native hot-plug and SHPC hot-plug"
    );
    assert_eq!(text(osc::LTR | osc::PME), "PME and LTR");
}

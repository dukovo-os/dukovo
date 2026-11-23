// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The interface against a simulated BMC that keeps to the state
//! transitions of section 9 of the IPMI 2.0 specification, and the
//! parsers for where firmware says it is.

use std::cell::RefCell;
use std::rc::Rc;
use std::vec::Vec;

use super::*;
use crate::locate::{from_io_resources, from_smbios};

const ECHO_NETFN: u8 = 0x30;
const ECHO_CMD: u8 = 0x10;

#[derive(Default)]
struct State {
    data_port: u16,
    command_port: u16,
    state: u8,
    obf: bool,
    command_written: bool,
    data_out: u8,
    request: Vec<u8>,
    ending: bool,
    aborting: bool,
    response: Vec<u8>,
    sent: usize,
    /// Status reads that show IBF after each write.
    busy: u32,
    ibf_left: u32,
    /// Whether IBF never clears.
    hung: bool,
    /// Whether the status register reads all ones, as at a port nothing
    /// decodes.
    absent: bool,
    /// Goes to the error state at this many request bytes, once.
    fail_once_at: Option<usize>,
    /// Goes to the error state at every message's first byte.
    fail_always: bool,
    /// Answers with a network function two above the request's.
    wrong_netfn: bool,
    waited_us: u64,
    aborts: u32,
    messages: Vec<Vec<u8>>,
    /// What the host did that the specification forbids.
    violations: Vec<&'static str>,
}

#[derive(Clone)]
struct Simulated(Rc<RefCell<State>>);

impl Simulated {
    fn new(interface: Interface) -> Simulated {
        let state = State {
            data_port: interface.data(),
            command_port: interface.command(),
            busy: 2,
            ..State::default()
        };
        Simulated(Rc::new(RefCell::new(state)))
    }

    fn bmc(&self, interface: Interface) -> Bmc<Simulated> {
        Bmc::new(self.clone(), interface)
    }
}

const KCS_IDLE: u8 = 0;
const KCS_READ: u8 = 1;
const KCS_WRITE: u8 = 2;
const KCS_ERROR: u8 = 3;

impl State {
    fn status(&mut self) -> u8 {
        if self.absent {
            return 0xff;
        }
        let ibf = self.hung || self.ibf_left > 0;
        self.ibf_left = self.ibf_left.saturating_sub(1);
        u8::from(self.obf)
            | u8::from(ibf) << 1
            | u8::from(self.command_written) << 3
            | self.state << 6
    }

    fn write_command(&mut self, value: u8) {
        self.command_written = true;
        match (value, self.state) {
            (GET_STATUS_ABORT_CODE, _) => {
                self.aborts += 1;
                self.aborting = true;
                self.ending = false;
                self.request.clear();
                self.state = KCS_WRITE;
            }
            (_, KCS_ERROR) => {}
            (WRITE_START_CODE, KCS_IDLE) => {
                self.request.clear();
                self.ending = false;
                self.state = KCS_WRITE;
                self.obf = true;
                if self.fail_always {
                    self.state = KCS_ERROR;
                }
            }
            (WRITE_END_CODE, KCS_WRITE) => {
                self.ending = true;
                self.obf = true;
            }
            _ => self.state = KCS_ERROR,
        }
    }

    fn write_data(&mut self, value: u8) {
        self.command_written = false;
        match self.state {
            KCS_WRITE if self.aborting => {
                self.aborting = false;
                self.response = std::vec![0x01];
                self.sent = 0;
                self.data_out = 0x01;
                self.obf = true;
                self.state = KCS_READ;
            }
            KCS_WRITE => {
                if self.obf {
                    self.violations.push("a data byte written before OBF was cleared");
                    self.state = KCS_ERROR;
                    return;
                }
                self.request.push(value);
                if self.ending {
                    self.answer();
                } else if self.fail_once_at == Some(self.request.len()) {
                    self.fail_once_at = None;
                    self.state = KCS_ERROR;
                } else {
                    self.obf = true;
                }
            }
            KCS_READ if value == READ_CODE => {
                if self.obf {
                    self.violations.push("READ written before the byte was read");
                }
                self.sent += 1;
                if let Some(&next) = self.response.get(self.sent) {
                    self.data_out = next;
                } else {
                    self.data_out = 0;
                    self.state = KCS_IDLE;
                }
                self.obf = true;
            }
            _ => self.state = KCS_ERROR,
        }
    }

    fn answer(&mut self) {
        let request = core::mem::take(&mut self.request);
        let (netfn, cmd) = (request[0] >> 2, request[1]);
        let body = &request[2..];
        let reply: Vec<u8> = match (netfn, cmd) {
            (NETFN_APP, CMD_GET_DEVICE_ID) => {
                // Device 0x20 revision 1 with SDRs, firmware 3.40, IPMI
                // 2.0, manufacturer 674, product 0x1234.
                std::vec![0, 0x20, 0x81, 0x03, 0x40, 0x02, 0x8f, 0xa2, 0x02, 0, 0x34, 0x12]
            }
            (ECHO_NETFN, ECHO_CMD) => [&[0][..], body].concat(),
            _ => std::vec![0xc1],
        };
        let skew = if self.wrong_netfn { 2 } else { 1 };
        self.response = [&[(netfn + skew) << 2, cmd][..], &reply].concat();
        self.messages.push(request);
        self.sent = 0;
        self.data_out = self.response[0];
        self.obf = true;
        self.state = KCS_READ;
    }
}

const GET_STATUS_ABORT_CODE: u8 = 0x60;
const WRITE_START_CODE: u8 = 0x61;
const WRITE_END_CODE: u8 = 0x62;
const READ_CODE: u8 = 0x68;

impl Platform for Simulated {
    fn in8(&self, port: u16) -> u8 {
        let mut s = self.0.borrow_mut();
        if port == s.command_port {
            s.status()
        } else if port == s.data_port {
            s.obf = false;
            s.data_out
        } else {
            0xff
        }
    }

    fn out8(&self, port: u16, value: u8) {
        let mut s = self.0.borrow_mut();
        if s.hung || s.ibf_left > 0 {
            s.violations.push("a write while IBF was set");
        }
        if port == s.command_port {
            s.write_command(value);
        } else if port == s.data_port {
            s.write_data(value);
        }
        s.ibf_left = s.busy;
    }

    fn wait_us(&self, us: u64) {
        self.0.borrow_mut().waited_us += us;
    }
}

const SPACING_1: Interface = Interface { base: 0xca2, spacing: 1 };
const SPACING_4: Interface = Interface { base: 0xca8, spacing: 4 };

#[test]
fn get_device_id_is_parsed() {
    for interface in [SPACING_1, SPACING_4] {
        let sim = Simulated::new(interface);
        let id = sim.bmc(interface).probe().unwrap();
        assert_eq!(
            id,
            DeviceId {
                device_id: 0x20,
                revision: 1,
                provides_sdrs: true,
                firmware_major: 3,
                firmware_minor: 40,
                updating: false,
                ipmi_version: (2, 0),
                manufacturer: 674,
                product: 0x1234,
            }
        );
        assert_eq!(manufacturer_name(id.manufacturer), Some("Dell"));
        let s = sim.0.borrow();
        assert_eq!(s.messages, [std::vec![NETFN_APP << 2, CMD_GET_DEVICE_ID]]);
        assert!(s.violations.is_empty(), "{:?}", s.violations);
    }
}

#[test]
fn an_echo_returns_what_was_sent() {
    let sim = Simulated::new(SPACING_4);
    let mut bmc = sim.bmc(SPACING_4);
    let sent: Vec<u8> = (0..=200).collect();
    let mut response = [0; 256];
    let len = bmc.request(ECHO_NETFN, ECHO_CMD, &sent, &mut response).unwrap();
    assert_eq!(len, 1 + sent.len());
    assert_eq!(response[0], 0);
    assert_eq!(response[1..len], sent[..]);
    let s = sim.0.borrow();
    assert_eq!(s.messages[0][2..], sent[..]);
    assert!(s.violations.is_empty(), "{:?}", s.violations);
    assert_eq!(s.state, KCS_IDLE);
}

#[test]
fn a_completion_code_that_is_not_success_is_the_callers() {
    let sim = Simulated::new(SPACING_1);
    let mut response = [0; 8];
    let len = sim.bmc(SPACING_1).request(ECHO_NETFN, 0x77, &[], &mut response).unwrap();
    assert_eq!(&response[..len], [0xc1]);
}

#[test]
fn a_bmc_whose_ibf_never_clears_times_out() {
    let sim = Simulated::new(SPACING_1);
    sim.0.borrow_mut().hung = true;
    let err = sim.bmc(SPACING_1).probe().unwrap_err();
    assert_eq!(err, Error::Timeout);
    // The wait is the 5 seconds a message has, and no more.
    assert_eq!(sim.0.borrow().waited_us, 5_000_000);
}

#[test]
fn a_bmc_that_goes_busy_halfway_times_out_too() {
    let sim = Simulated::new(SPACING_1);
    let mut bmc = sim.bmc(SPACING_1);
    bmc.probe().unwrap();
    sim.0.borrow_mut().hung = true;
    assert_eq!(bmc.probe().unwrap_err(), Error::Timeout);
}

#[test]
fn an_error_state_is_left_by_the_abort() {
    let sim = Simulated::new(SPACING_1);
    sim.0.borrow_mut().fail_once_at = Some(1);
    let id = sim.bmc(SPACING_1).probe().unwrap();
    assert_eq!(id.firmware_major, 3);
    let s = sim.0.borrow();
    assert_eq!(s.aborts, 1);
    assert_eq!(s.messages.len(), 1);
    assert!(s.violations.is_empty(), "{:?}", s.violations);
}

#[test]
fn an_interface_that_stays_in_error_is_given_up_after_three_tries() {
    let sim = Simulated::new(SPACING_1);
    sim.0.borrow_mut().fail_always = true;
    assert_eq!(sim.bmc(SPACING_1).probe().unwrap_err(), Error::Interface);
    let s = sim.0.borrow();
    assert_eq!(s.aborts, 3);
    assert!(s.messages.is_empty());
}

#[test]
fn a_state_left_over_from_before_is_aborted_first() {
    let sim = Simulated::new(SPACING_4);
    sim.0.borrow_mut().state = KCS_ERROR;
    sim.bmc(SPACING_4).probe().unwrap();
    assert_eq!(sim.0.borrow().aborts, 1);
}

#[test]
fn a_response_to_another_network_function_is_refused() {
    let sim = Simulated::new(SPACING_1);
    sim.0.borrow_mut().wrong_netfn = true;
    let mut bmc = sim.bmc(SPACING_1);
    assert_eq!(bmc.probe().unwrap_err(), Error::WrongResponse);
    // The exchange was read to its end, so the next one works.
    sim.0.borrow_mut().wrong_netfn = false;
    bmc.probe().unwrap();
}

#[test]
fn a_port_nothing_decodes_is_no_device_at_once() {
    let sim = Simulated::new(SPACING_1);
    sim.0.borrow_mut().absent = true;
    assert_eq!(sim.bmc(SPACING_1).probe().unwrap_err(), Error::NoDevice);
    assert_eq!(sim.0.borrow().waited_us, 0);
}

#[test]
fn a_response_too_big_for_its_buffer_is_read_to_its_end() {
    let sim = Simulated::new(SPACING_1);
    let mut bmc = sim.bmc(SPACING_1);
    let mut small = [0; 8];
    let err = bmc.request(ECHO_NETFN, ECHO_CMD, &[7; 40], &mut small).unwrap_err();
    assert_eq!(err, Error::ResponseTooBig(41));
    bmc.probe().unwrap();
}

#[test]
fn requests_that_cannot_be_sent_are_refused() {
    let sim = Simulated::new(SPACING_1);
    let mut bmc = sim.bmc(SPACING_1);
    let mut response = [0; 4];
    let big = [0; MAX_MESSAGE - 1];
    assert_eq!(bmc.request(ECHO_NETFN, 1, &big, &mut response), Err(Error::TooBig));
    assert_eq!(bmc.request(0x07, 1, &[], &mut response), Err(Error::BadNetfn(7)));
    assert_eq!(bmc.request(0x3f, 1, &[], &mut response), Err(Error::BadNetfn(0x3f)));
    assert!(sim.0.borrow().messages.is_empty());
}

#[test]
fn the_firmware_minor_is_two_bcd_digits() {
    // Firmware 12.09 being updated, IPMI 1.5.
    let bytes = [0x20, 0x01, 0x80 | 0x0c, 0x09, 0x51, 0x02, 0, 0, 0, 0, 0, 0];
    let id = DeviceId::parse(&bytes).unwrap();
    assert_eq!((id.firmware_major, id.firmware_minor, id.updating), (12, 9, true));
    assert_eq!(id.ipmi_version, (1, 5));
    assert!(DeviceId::parse(&bytes[..10]).is_none());
}

#[test]
fn errors_read_as_plain_sentences() {
    use std::string::ToString;
    assert_eq!(Error::Timeout.to_string(), "the BMC stayed busy");
    assert_eq!(Error::Completion(0xc1).to_string(), "the BMC does not take the command");
    assert_eq!(Error::Completion(0xd5).to_string(), "the BMC answered completion code 0xd5");
}

/// A type 38 structure for a KCS interface at `address`, with the
/// modifier byte `modifier` (absent if None).
fn type_38(interface_type: u8, address: u64, modifier: Option<u8>) -> Vec<u8> {
    let mut bytes = std::vec![38, 0x10, 0x34, 0x00, interface_type, 0x20, 0x20, 0xff];
    bytes.extend_from_slice(&address.to_le_bytes());
    if let Some(modifier) = modifier {
        bytes.extend_from_slice(&[modifier, 0]);
        bytes[1] = 0x12;
    }
    bytes
}

#[test]
fn type_38_gives_the_ports_and_the_spacing() {
    // The I/O bit set in the address, and the spacing in bits 7:6.
    let one = type_38(1, 0xca3, Some(0x00));
    assert_eq!(from_smbios(&one), Some(Interface { base: 0xca2, spacing: 1 }));
    let four = type_38(1, 0xca9, Some(0x40));
    assert_eq!(from_smbios(&four), Some(Interface { base: 0xca8, spacing: 4 }));
    let sixteen = type_38(1, 0xca3, Some(0x80));
    assert_eq!(from_smbios(&sixteen), Some(Interface { base: 0xca2, spacing: 16 }));
    // Before version 2.3 there is no modifier: successive bytes.
    assert_eq!(from_smbios(&type_38(1, 0xca3, None)), Some(Interface { base: 0xca2, spacing: 1 }));
}

#[test]
fn type_38_gives_the_lowest_address_bit_from_the_modifier() {
    let odd = type_38(1, 0xca3, Some(0x10));
    assert_eq!(from_smbios(&odd), Some(Interface { base: 0xca3, spacing: 1 }));
}

#[test]
fn type_38_for_other_interfaces_or_memory_gives_nothing() {
    assert_eq!(from_smbios(&type_38(2, 0xca3, Some(0))), None);
    assert_eq!(from_smbios(&type_38(3, 0xe4, Some(0))), None);
    assert_eq!(from_smbios(&type_38(4, 0x20, Some(0))), None);
    assert_eq!(from_smbios(&type_38(1, 0xfed0_0000, Some(0))), None);
    assert_eq!(from_smbios(&type_38(1, 0x1000_0ca3, Some(0))), None);
    assert_eq!(from_smbios(&[38, 4, 0, 0]), None);
}

#[test]
fn a_crs_with_two_ports_gives_the_spacing_between_them() {
    assert_eq!(
        from_io_resources(&[(0xca2, 1), (0xca3, 1)]),
        Some(Interface { base: 0xca2, spacing: 1 })
    );
    assert_eq!(
        from_io_resources(&[(0xca8, 1), (0xcac, 1)]),
        Some(Interface { base: 0xca8, spacing: 4 })
    );
    assert_eq!(from_io_resources(&[(0xca8, 1), (0xca9, 1), (0xcaa, 1)]), None);
    assert_eq!(from_io_resources(&[(0xcac, 1), (0xca8, 1)]), None);
    assert_eq!(from_io_resources(&[(0xca2, 1), (0xca2, 1)]), None);
}

#[test]
fn a_crs_with_one_port_range_gives_the_spacing_by_its_length() {
    assert_eq!(from_io_resources(&[(0xca2, 2)]), Some(Interface { base: 0xca2, spacing: 1 }));
    assert_eq!(from_io_resources(&[(0xca8, 5)]), Some(Interface { base: 0xca8, spacing: 4 }));
    assert_eq!(from_io_resources(&[(0xca0, 17)]), Some(Interface { base: 0xca0, spacing: 16 }));
    assert_eq!(from_io_resources(&[(0xca2, 3)]), None);
    assert_eq!(from_io_resources(&[(0xca2, 0)]), None);
    assert_eq!(from_io_resources(&[]), None);
}

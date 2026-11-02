// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The methods of the firewall capability (Phase 12, radix-firewall(2),
//! ADR 0334): giving the packet filter its rules (`crate::net::firewall`),
//! and reading them back with what listens. A change is logged, with who
//! made it.

use alloc::vec::Vec;

use radix_abi::errors::Kernel;
use radix_abi::firewall::{
    self as abi, Action as AbiAction, Listener as AbiListener, ListenerFlags, ListenersArgs,
    ListenersResults, Match as AbiMatch, RULES_MAX, Rule as AbiRule, RulesArgs, RulesResults,
    SetRulesArgs, StateArgs, StateResults,
};
use radix_cap::{Capability, ObjectKind, Rights};
use radix_hal::UserMode;
use radix_klog::info;
use radix_mm::{Flags, HEAP, Kmalloc};
use radix_net::{IpAddress, Ipv4Address, Ipv6Address};

use super::methods::{Answer, answer};
use super::{Result, UserRef, cap_error, error};
use crate::Arch;
use crate::net::firewall::{self, Action, LISTENERS_MAX, Match, Program, Rule};
use crate::net::ports::{Protocol, Versions};

/// Calls the method `label` on `cap`, a firewall capability, for `user`:
/// kept out of line, so that [`super::methods::call`]'s frame does not
/// carry its own.
#[inline(never)]
pub fn method(user: &UserRef, cap: &Capability, label: u32, args: &[u64; 8]) -> Result<Answer> {
    let check = |rights| cap.check(ObjectKind::Firewall, rights).map_err(cap_error);
    match label {
        abi::SET_RULES => {
            let a = SetRulesArgs::decode(args)?;
            check(Rights::CONTROL)?;
            let size = AbiRule::SIZE as u64;
            if a.rules.len % size != 0 {
                return Err(error(Kernel::InvalidArgument));
            }
            let count = a.rules.len / size;
            if count > RULES_MAX {
                return Err(error(Kernel::OutOfRange));
            }
            if !radix_ipc::range::user_bytes(a.rules.address, a.rules.len) {
                return Err(error(Kernel::BadAddress));
            }
            set_rules(a.rules.address, count as usize)?;
            info!("Firewall: {} gave it {count} rules", user.process.name().as_str());
            answer(label, 0, [0; 8])
        }
        abi::STATE => {
            StateArgs::decode(args)?;
            check(Rights::READ)?;
            let (rules, dropped) = firewall::counts();
            // At most RULES_MAX.
            let results = StateResults { rules: rules as u32, dropped };
            answer(label, StateResults::WORDS, results.encode())
        }
        abi::RULES => {
            let a = RulesArgs::decode(args)?;
            check(Rights::READ)?;
            let room = out_room(a.buffer.address, a.buffer.len, AbiRule::SIZE)?;
            let mut count = 0;
            for at in 0..firewall::RULES_MAX {
                let Some(rule) = firewall::rule(at) else { break };
                if count < room {
                    put(a.buffer.address, count, &encode_rule(&rule))?;
                }
                count += 1;
            }
            // At most RULES_MAX.
            let results = RulesResults { count: count as u32 };
            answer(label, RulesResults::WORDS, results.encode())
        }
        abi::LISTENERS => {
            let a = ListenersArgs::decode(args)?;
            check(Rights::READ)?;
            let room = out_room(a.buffer.address, a.buffer.len, AbiListener::SIZE)?;
            let mut count = 0;
            for at in 0..LISTENERS_MAX {
                let Some(listener) = firewall::listener(at) else { continue };
                if count < room {
                    put(a.buffer.address, count, &encode_listener(&listener))?;
                }
                count += 1;
            }
            // At most LISTENERS_MAX.
            let results = ListenersResults { count: count as u32 };
            answer(label, ListenersResults::WORDS, results.encode())
        }
        _ => Err(error(Kernel::NoSuchMethod)),
    }
}

/// How many structures of `size` bytes fit in the caller's `len` bytes at
/// `address`.
fn out_room(address: u64, len: u64, size: usize) -> Result<usize> {
    if !radix_ipc::range::user_bytes(address, len) {
        return Err(error(Kernel::BadAddress));
    }
    Ok(usize::try_from(len).unwrap_or(usize::MAX) / size)
}

/// Copies `bytes`, the `at`th structure of its size, to the caller's
/// buffer at `address`.
fn put(address: u64, at: usize, bytes: &[u8]) -> Result<()> {
    let to = address + (at * bytes.len()) as u64;
    if Arch::copy_to_user(to, bytes) { Ok(()) } else { Err(error(Kernel::BadAddress)) }
}

/// Reads `count` Rule structures from the caller's `address`, checks each
/// and gives them to the firewall: a rule at a time, as the rules are too
/// many for a stack frame.
fn set_rules(address: u64, count: usize) -> Result<()> {
    let mut rules: Vec<Rule, Kmalloc> = Vec::new_in(HEAP.with(Flags::MAY_SLEEP));
    rules.try_reserve_exact(count).map_err(|_| error(Kernel::NoMemory))?;
    for at in 0..count {
        let mut bytes = [0u8; AbiRule::SIZE];
        if !Arch::copy_from_user(&mut bytes, address + (at * AbiRule::SIZE) as u64) {
            return Err(error(Kernel::BadAddress));
        }
        let rule = decode_rule(&bytes).ok_or(error(Kernel::InvalidArgument))?;
        // Room was reserved for them all.
        let _ = rules.push_within_capacity(rule);
    }
    firewall::set_rules(&rules).map_err(|()| error(Kernel::OutOfRange))
}

fn u16_at(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([bytes[at], bytes[at + 1]])
}

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

fn u64_at(bytes: &[u8], at: usize) -> u64 {
    let mut word = [0u8; 8];
    word.copy_from_slice(&bytes[at..at + 8]);
    u64::from_le_bytes(word)
}

/// What a Rule structure says, if it is one.
fn decode_rule(bytes: &[u8; AbiRule::SIZE]) -> Option<Rule> {
    let action = match AbiAction::from_raw(u32_at(bytes, AbiRule::OFFSET_ACTION))? {
        AbiAction::Allow => Action::Allow,
        AbiAction::Deny => Action::Deny,
    };
    let protocol = match AbiMatch::from_raw(u32_at(bytes, AbiRule::OFFSET_PROTOCOL))? {
        AbiMatch::Any => Match::Any,
        AbiMatch::Tcp => Match::Tcp,
        AbiMatch::Udp => Match::Udp,
        AbiMatch::Ping => Match::Ping,
    };
    let ports =
        (u16_at(bytes, AbiRule::OFFSET_FIRST_PORT), u16_at(bytes, AbiRule::OFFSET_LAST_PORT));
    if ports.0 > ports.1 {
        return None;
    }
    let mut ip = [0u8; 16];
    ip.copy_from_slice(&bytes[AbiRule::OFFSET_FROM..AbiRule::OFFSET_FROM + 16]);
    let prefix = bytes[AbiRule::OFFSET_PREFIX];
    let from = match u32_at(bytes, AbiRule::OFFSET_FAMILY) {
        0 => None,
        4 if prefix <= 32 => {
            Some((IpAddress::Ipv4(Ipv4Address::new(ip[0], ip[1], ip[2], ip[3])), prefix))
        }
        6 if prefix <= 128 => Some((IpAddress::Ipv6(Ipv6Address::from(ip)), prefix)),
        _ => return None,
    };
    let interface = match u32_at(bytes, AbiRule::OFFSET_INTERFACE) {
        0 => None,
        // Linux's number, from 1: the kernel's are from 0.
        index => Some(index as usize - 1),
    };
    let program = Program {
        mount: u64_at(bytes, AbiRule::OFFSET_MOUNT),
        id: u64_at(bytes, AbiRule::OFFSET_FILE),
    };
    let program = (program != Program::NONE).then_some(program);
    Some(Rule { action, protocol, ports, from, interface, program })
}

/// `rule` as a Rule structure.
fn encode_rule(rule: &Rule) -> [u8; AbiRule::SIZE] {
    let mut bytes = [0u8; AbiRule::SIZE];
    let action = match rule.action {
        Action::Allow => AbiAction::Allow,
        Action::Deny => AbiAction::Deny,
    };
    let protocol = match rule.protocol {
        Match::Any => AbiMatch::Any,
        Match::Tcp => AbiMatch::Tcp,
        Match::Udp => AbiMatch::Udp,
        Match::Ping => AbiMatch::Ping,
    };
    let mut put32 = |at: usize, value: u32| bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
    put32(AbiRule::OFFSET_ACTION, action.raw());
    put32(AbiRule::OFFSET_PROTOCOL, protocol.raw());
    let interface = rule.interface.map_or(0, |number| number as u32 + 1);
    put32(AbiRule::OFFSET_INTERFACE, interface);
    let family = match rule.from {
        None => 0,
        Some((IpAddress::Ipv4(_), _)) => 4,
        Some((IpAddress::Ipv6(_), _)) => 6,
    };
    put32(AbiRule::OFFSET_FAMILY, family);
    bytes[AbiRule::OFFSET_FIRST_PORT..AbiRule::OFFSET_FIRST_PORT + 2]
        .copy_from_slice(&rule.ports.0.to_le_bytes());
    bytes[AbiRule::OFFSET_LAST_PORT..AbiRule::OFFSET_LAST_PORT + 2]
        .copy_from_slice(&rule.ports.1.to_le_bytes());
    if let Some((from, prefix)) = rule.from {
        let at = AbiRule::OFFSET_FROM;
        match from {
            IpAddress::Ipv4(ip) => bytes[at..at + 4].copy_from_slice(&ip.octets()),
            IpAddress::Ipv6(ip) => bytes[at..at + 16].copy_from_slice(&ip.octets()),
        }
        bytes[AbiRule::OFFSET_PREFIX] = prefix;
    }
    let program = rule.program.unwrap_or(Program::NONE);
    bytes[AbiRule::OFFSET_MOUNT..AbiRule::OFFSET_MOUNT + 8]
        .copy_from_slice(&program.mount.to_le_bytes());
    bytes[AbiRule::OFFSET_FILE..AbiRule::OFFSET_FILE + 8]
        .copy_from_slice(&program.id.to_le_bytes());
    bytes
}

/// `listener` as a Listener structure.
fn encode_listener(listener: &firewall::Listener) -> [u8; AbiListener::SIZE] {
    let mut bytes = [0u8; AbiListener::SIZE];
    let protocol = match listener.protocol {
        Protocol::Tcp => AbiMatch::Tcp,
        Protocol::Udp => AbiMatch::Udp,
    };
    let mut flags = match listener.versions {
        Versions::Ipv4 => ListenerFlags::IPV4,
        Versions::Ipv6 => ListenerFlags::IPV6,
        Versions::Both => ListenerFlags::IPV4.union(ListenerFlags::IPV6),
    };
    if listener.inbound {
        flags = flags.union(ListenerFlags::INBOUND);
    }
    let mut put32 = |at: usize, value: u32| bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
    put32(AbiListener::OFFSET_PROTOCOL, protocol.raw());
    put32(AbiListener::OFFSET_FLAGS, flags.bits());
    put32(AbiListener::OFFSET_INTERFACE, listener.device);
    bytes[AbiListener::OFFSET_PORT..AbiListener::OFFSET_PORT + 2]
        .copy_from_slice(&listener.port.to_le_bytes());
    for (at, value) in [
        (AbiListener::OFFSET_PROCESS, listener.owner.process),
        (AbiListener::OFFSET_MOUNT, listener.owner.program.mount),
        (AbiListener::OFFSET_FILE, listener.owner.program.id),
    ] {
        bytes[at..at + 8].copy_from_slice(&value.to_le_bytes());
    }
    bytes
}

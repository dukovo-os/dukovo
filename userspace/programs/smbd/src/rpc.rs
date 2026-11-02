// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The pipes of IPC$: DCE/RPC (C706, MS-RPCE) over SMB. The srvsvc pipe
//! has enough of the Server Service (MS-SRVS) for a PC to list the
//! shares, as `smbclient -L` and Windows' Explorer do: NetrShareEnum,
//! NetrShareGetInfo and NetrServerGetInfo, in NDR. The lsarpc pipe names
//! SIDs ([`crate::lsa`]).
//!
//! A pipe takes whole PDUs written to it and gives back its answers, a
//! PDU at a time, cut to the size the client binds with.

use crate::lsa::{self, Names, Policies};
use crate::sd::Sid;
use crate::wire::{self, Out};

/// srvsvc's interface: 4b324fc8-1670-01d3-1278-5a47bf6ee188, version 3.0.
const SRVSVC: [u8; 16] = [
    0xc8, 0x4f, 0x32, 0x4b, 0x70, 0x16, 0xd3, 0x01, 0x12, 0x78, 0x5a, 0x47, 0xbf, 0x6e, 0xe1, 0x88,
];
/// lsarpc's interface: 12345778-1234-abcd-ef00-0123456789ab, version 0.0.
const LSARPC: [u8; 16] = [
    0x78, 0x57, 0x34, 0x12, 0x34, 0x12, 0xcd, 0xab, 0xef, 0x00, 0x01, 0x23, 0x45, 0x67, 0x89, 0xab,
];
/// NDR's transfer syntax: 8a885d04-1ceb-11c9-9fe8-08002b104860, version 2.
const NDR: [u8; 16] = [
    0x04, 0x5d, 0x88, 0x8a, 0xeb, 0x1c, 0xc9, 0x11, 0x9f, 0xe8, 0x08, 0x00, 0x2b, 0x10, 0x48, 0x60,
];

/// The PDU types.
const REQUEST: u8 = 0;
const RESPONSE: u8 = 2;
const FAULT: u8 = 3;
const BIND: u8 = 11;
const BIND_ACK: u8 = 12;
const FIRST: u8 = 0x01;
const LAST: u8 = 0x02;

/// The operations.
const NETR_SHARE_ENUM: u16 = 15;
const NETR_SHARE_GET_INFO: u16 = 16;
const NETR_SERVER_GET_INFO: u16 = 21;

/// WERRORs.
const WERR_OK: u32 = 0;
const WERR_UNKNOWN_LEVEL: u32 = 0x7c;
const WERR_NET_NAME_NOT_FOUND: u32 = 0x906;
/// The fault of an operation the server does not have.
pub const NCA_OP_RNG_ERROR: u32 = 0x1c01_0002;
/// The fault of a request whose stub does not read.
pub const BAD_STUB_DATA: u32 = 0x0000_06f7;
/// The longest stub a request may have in all its fragments.
const STUB_MAX: usize = 1 << 20;

/// The interfaces of the pipes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Interface {
    /// The Server Service's: the shares.
    #[default]
    Srvsvc,
    /// The Local Security Authority's: names and SIDs.
    Lsarpc,
}

impl Interface {
    /// The interface of the pipe called `name`, if smbd has it.
    pub fn of_pipe(name: &str) -> Option<Interface> {
        let name = name.trim_start_matches('\\');
        if name.eq_ignore_ascii_case("srvsvc") {
            Some(Interface::Srvsvc)
        } else if name.eq_ignore_ascii_case("lsarpc") || name.eq_ignore_ascii_case("lsass") {
            Some(Interface::Lsarpc)
        } else {
            None
        }
    }

    /// Its pipe's name, as an open of it is called.
    pub fn name(self) -> &'static str {
        match self {
            Interface::Srvsvc => "\\srvsvc",
            Interface::Lsarpc => "\\lsarpc",
        }
    }

    fn uuid(self) -> [u8; 16] {
        match self {
            Interface::Srvsvc => SRVSVC,
            Interface::Lsarpc => LSARPC,
        }
    }

    /// The address a BIND_ACK gives.
    fn address(self) -> &'static [u8] {
        match self {
            Interface::Srvsvc => b"\\PIPE\\srvsvc\0",
            Interface::Lsarpc => b"\\PIPE\\lsass\0",
        }
    }
}

/// A share, as the pipe lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Listed {
    /// Its name.
    pub name: String,
    /// STYPE_DISKTREE (0), or STYPE_IPC with STYPE_SPECIAL.
    pub kind: u32,
    /// Its comment.
    pub comment: String,
}

/// A disk share's type, and IPC$'s.
pub const DISK: u32 = 0;
/// IPC$'s.
pub const IPC: u32 = 0x8000_0003;

/// What the pipes answer about.
pub struct Server {
    /// The server's name.
    pub name: String,
    /// Its shares, IPC$ among them.
    pub shares: Vec<Listed>,
    /// The names of its users and groups.
    pub names: Box<dyn Names>,
}

/// One end of a pipe.
#[derive(Debug, Default)]
pub struct Pipe {
    /// Its interface.
    interface: Interface,
    /// lsarpc's open policy handles.
    policies: Policies,
    /// What was written and not yet a whole PDU.
    input: Vec<u8>,
    /// The answers waiting to be read, a PDU each.
    output: std::collections::VecDeque<Vec<u8>>,
    /// The longest PDU the client takes, from its BIND.
    max_fragment: usize,
    /// The context the client bound srvsvc to.
    context: Option<u16>,
    /// A request's stub so far, when it comes in fragments.
    stub: Vec<u8>,
}

impl Pipe {
    /// A pipe of `interface`.
    pub fn new(interface: Interface) -> Pipe {
        Pipe { interface, ..Pipe::default() }
    }

    /// Takes what a client wrote, and answers each whole PDU in it.
    pub fn write(&mut self, bytes: &[u8], server: &Server) {
        self.input.extend_from_slice(bytes);
        while self.input.len() >= 16 {
            let length = u16::from_le_bytes([self.input[8], self.input[9]]) as usize;
            if length < 16 {
                // Not DCE/RPC: dropped whole.
                self.input.clear();
                return;
            }
            if self.input.len() < length {
                return;
            }
            let pdu: Vec<u8> = self.input.drain(..length).collect();
            self.answer(&pdu, server);
        }
    }

    /// The next answer, cut to `max` bytes: what is left of it stays for
    /// the next read. Whether more of it is left.
    pub fn read(&mut self, max: usize) -> Option<(Vec<u8>, bool)> {
        let front = self.output.front_mut()?;
        if front.len() <= max {
            return self.output.pop_front().map(|pdu| (pdu, false));
        }
        let rest = front.split_off(max);
        let first = std::mem::replace(front, rest);
        Some((first, true))
    }

    /// Answers one PDU.
    fn answer(&mut self, pdu: &[u8], server: &Server) {
        let kind = pdu[2];
        let flags = pdu[3];
        let call = wire::u32_at(pdu, 12).unwrap_or(0);
        // Little-endian, ASCII, IEEE floats: what every client sends.
        if pdu[0] != 5 || pdu[4] != 0x10 {
            return;
        }
        match kind {
            BIND => self.bind(pdu, call),
            REQUEST => {
                let (Ok(context), Ok(operation)) = (wire::u16_at(pdu, 20), wire::u16_at(pdu, 22))
                else {
                    return;
                };
                if flags & FIRST != 0 {
                    self.stub.clear();
                }
                let part = pdu.get(24..).unwrap_or_default();
                if self.stub.len() + part.len() > STUB_MAX {
                    self.stub = Vec::new();
                    return self.fault(call, context, BAD_STUB_DATA);
                }
                self.stub.extend_from_slice(part);
                if flags & LAST == 0 {
                    return;
                }
                let stub = std::mem::take(&mut self.stub);
                if self.context != Some(context) {
                    return self.fault(call, context, NCA_OP_RNG_ERROR);
                }
                let answer = match self.interface {
                    Interface::Srvsvc => operate(operation, &stub, server).ok_or(NCA_OP_RNG_ERROR),
                    Interface::Lsarpc => lsa::operate(operation, &stub, server, &mut self.policies),
                };
                match answer {
                    Ok(answer) => self.respond(call, context, &answer),
                    Err(fault) => self.fault(call, context, fault),
                }
            }
            _ => {}
        }
    }

    /// Answers a BIND: the pipe's interface in NDR is accepted, and
    /// nothing else.
    fn bind(&mut self, pdu: &[u8], call: u32) {
        let max_receive = wire::u16_at(pdu, 18).unwrap_or(4280) as usize;
        self.max_fragment = max_receive.clamp(1024, 65_535);
        let count = wire::u8_at(pdu, 24).unwrap_or(0) as usize;
        let mut at = 28;
        let mut results = Out::new();
        for _ in 0..count {
            let (Ok(id), Ok(syntaxes)) = (wire::u16_at(pdu, at), wire::u8_at(pdu, at + 2)) else {
                return;
            };
            let abstract_syntax = pdu.get(at + 4..at + 24).unwrap_or_default();
            let mut accepted = false;
            for k in 0..syntaxes as usize {
                let syntax = pdu.get(at + 24 + 20 * k..at + 44 + 20 * k).unwrap_or_default();
                if abstract_syntax.get(..16) == Some(&self.interface.uuid())
                    && syntax.get(..16) == Some(&NDR)
                    && syntax.get(16..20) == Some(&[2, 0, 0, 0])
                {
                    accepted = true;
                }
            }
            if accepted && self.context.is_none() {
                self.context = Some(id);
                results.u16(0).u16(0).bytes(&NDR).u32(2);
            } else {
                // A provider rejection: transfer syntaxes not supported.
                results.u16(2).u16(2).zeros(20);
            }
            at += 24 + 20 * syntaxes as usize;
        }
        let address = self.interface.address();
        let mut body = Out::new();
        body.u16(self.max_fragment as u16)
            .u16(self.max_fragment as u16)
            .u32(0x0001_2345)
            .u16(address.len() as u16)
            .bytes(address);
        // The results start on 4 bytes, from the PDU's start.
        while !(16 + body.len()).is_multiple_of(4) {
            body.u8(0);
        }
        body.u8(count as u8).u8(0).u16(0).bytes(&results.0);
        self.output.push_back(pdu_of(BIND_ACK, FIRST | LAST, call, &body.0));
    }

    /// Queues a RESPONSE with `stub`, in fragments the client takes.
    fn respond(&mut self, call: u32, context: u16, stub: &[u8]) {
        let room = self.max_fragment.max(1024) - 24;
        let chunks: Vec<&[u8]> =
            if stub.is_empty() { vec![&[]] } else { stub.chunks(room).collect() };
        let count = chunks.len();
        for (i, chunk) in chunks.into_iter().enumerate() {
            let mut flags = 0;
            if i == 0 {
                flags |= FIRST;
            }
            if i + 1 == count {
                flags |= LAST;
            }
            let mut body = Out::new();
            body.u32(stub.len() as u32).u16(context).u8(0).u8(0).bytes(chunk);
            self.output.push_back(pdu_of(RESPONSE, flags, call, &body.0));
        }
    }

    /// Queues a FAULT.
    fn fault(&mut self, call: u32, context: u16, code: u32) {
        let mut body = Out::new();
        body.u32(0).u16(context).u8(0).u8(0).u32(code).u32(0);
        self.output.push_back(pdu_of(FAULT, FIRST | LAST, call, &body.0));
    }
}

/// A PDU: the common header, then `body`.
fn pdu_of(kind: u8, flags: u8, call: u32, body: &[u8]) -> Vec<u8> {
    let mut out = Out::new();
    out.u8(5).u8(0).u8(kind).u8(flags).bytes(&[0x10, 0, 0, 0]);
    out.u16((16 + body.len()) as u16).u16(0).u32(call).bytes(body);
    out.0
}

/// NDR's reading of a request's stub.
pub struct Ndr<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Ndr<'a> {
    /// A reader of `bytes`, from their start.
    pub fn new(bytes: &'a [u8]) -> Ndr<'a> {
        Ndr { bytes, at: 0 }
    }

    pub fn u32(&mut self) -> Option<u32> {
        self.at = self.at.next_multiple_of(4);
        let value = wire::u32_at(self.bytes, self.at).ok()?;
        self.at += 4;
        Some(value)
    }

    /// `n` bytes, as they are.
    pub fn bytes(&mut self, n: usize) -> Option<&'a [u8]> {
        let bytes = wire::slice_at(self.bytes, self.at, n).ok()?;
        self.at += n;
        Some(bytes)
    }

    /// A context handle: 20 bytes.
    pub fn handle(&mut self) -> Option<[u8; 20]> {
        self.at = self.at.next_multiple_of(4);
        self.bytes(20)?.try_into().ok()
    }

    /// An RPC_SID, a conformant structure: its count, then the SID.
    pub fn sid(&mut self) -> Option<Sid> {
        let count = self.u32()? as usize;
        let bytes = self.bytes(8 + 4 * count.min(16))?;
        let (sid, _) = Sid::parse(bytes)?;
        (sid.subs.len() == count).then_some(sid)
    }

    /// A conformant varying array of UTF-16, as text.
    pub fn units(&mut self) -> Option<String> {
        let _max = self.u32()?;
        let _offset = self.u32()?;
        let actual = self.u32()? as usize;
        let bytes = self.bytes(actual.checked_mul(2)?)?;
        wire::utf16(bytes).ok()
    }

    /// A conformant varying string of UTF-16, after its pointer if it is
    /// `unique`.
    fn string(&mut self, unique: bool) -> Option<Option<String>> {
        if unique && self.u32()? == 0 {
            return Some(None);
        }
        let text = self.units()?;
        Some(Some(text.trim_end_matches('\0').to_string()))
    }
}

/// NDR's writing of a response's stub, with the deferred strings of
/// pointers written after what holds them.
#[derive(Default)]
pub struct Writer {
    pub out: Out,
    next_referent: u32,
}

impl Writer {
    pub fn u16(&mut self, value: u16) {
        self.out.align(2);
        self.out.u16(value);
    }

    pub fn u32(&mut self, value: u32) {
        self.out.align(4);
        self.out.u32(value);
    }

    /// A pointer's referent, not null.
    pub fn pointer(&mut self) {
        self.next_referent += 4;
        let referent = 0x0002_0000 + self.next_referent;
        self.u32(referent);
    }

    /// An RPC_UNICODE_STRING's own part: its lengths in bytes and its
    /// buffer's pointer, null for an empty one. [`Writer::units`] writes
    /// the buffer, where the pointer's referent goes.
    pub fn unicode(&mut self, text: &str) {
        let bytes = (text.encode_utf16().count() * 2) as u16;
        self.out.align(4);
        self.u16(bytes);
        self.u16(bytes);
        if text.is_empty() {
            self.u32(0);
        } else {
            self.pointer();
        }
    }

    /// The buffer of an RPC_UNICODE_STRING that is not empty: a
    /// conformant varying array of UTF-16 without a NUL.
    pub fn units(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        let units: Vec<u16> = text.encode_utf16().collect();
        self.u32(units.len() as u32);
        self.u32(0);
        self.u32(units.len() as u32);
        for unit in units {
            self.out.u16(unit);
        }
    }

    /// An RPC_SID: its count, then the SID.
    pub fn sid(&mut self, sid: &Sid) {
        self.u32(sid.subs.len() as u32);
        self.out.bytes(&sid.bytes());
    }

    /// A conformant varying string, with its NUL.
    fn string(&mut self, text: &str) {
        let mut units: Vec<u16> = text.encode_utf16().collect();
        units.push(0);
        self.u32(units.len() as u32);
        self.u32(0);
        self.u32(units.len() as u32);
        for unit in units {
            self.out.u16(unit);
        }
    }
}

/// The answer to operation `operation` with `stub`, or none for an
/// operation the pipe does not have.
fn operate(operation: u16, stub: &[u8], server: &Server) -> Option<Vec<u8>> {
    let mut ndr = Ndr::new(stub);
    let mut w = Writer::default();
    match operation {
        NETR_SHARE_ENUM => {
            ndr.string(true)?;
            let level = ndr.u32()?;
            let shares = &server.shares;
            if level != 0 && level != 1 {
                w.u32(level);
                w.u32(level);
                w.u32(0);
                w.u32(0);
                w.u32(0);
                w.u32(0);
                w.u32(WERR_UNKNOWN_LEVEL);
                return Some(w.out.0);
            }
            // SHARE_ENUM_STRUCT: the level, the union's, the container's
            // pointer; the container: its count and its array's pointer.
            w.u32(level);
            w.u32(level);
            w.pointer();
            w.u32(shares.len() as u32);
            w.pointer();
            w.u32(shares.len() as u32);
            for share in shares {
                w.pointer();
                if level == 1 {
                    w.u32(share.kind);
                    w.pointer();
                }
            }
            for share in shares {
                w.string(&share.name);
                if level == 1 {
                    w.string(&share.comment);
                }
            }
            // The total, the resume handle, the status.
            w.u32(shares.len() as u32);
            w.pointer();
            w.u32(0);
            w.u32(WERR_OK);
        }
        NETR_SHARE_GET_INFO => {
            ndr.string(true)?;
            let name = ndr.string(false)??;
            let level = ndr.u32()?;
            let share = server.shares.iter().find(|s| manifest::shares::same_name(&s.name, &name));
            match (share, level) {
                (None, _) => {
                    w.u32(level);
                    w.u32(0);
                    w.u32(WERR_NET_NAME_NOT_FOUND);
                }
                (Some(share), 0 | 1) => {
                    w.u32(level);
                    w.pointer();
                    w.pointer();
                    if level == 1 {
                        w.u32(share.kind);
                        w.pointer();
                    }
                    w.string(&share.name);
                    if level == 1 {
                        w.string(&share.comment);
                    }
                    w.u32(WERR_OK);
                }
                (Some(_), 1005) => {
                    // SHARE_INFO_1005: no offline caching.
                    w.u32(level);
                    w.pointer();
                    w.u32(0x0000_0030);
                    w.u32(WERR_OK);
                }
                (Some(_), _) => {
                    w.u32(level);
                    w.u32(0);
                    w.u32(WERR_UNKNOWN_LEVEL);
                }
            }
        }
        NETR_SERVER_GET_INFO => {
            ndr.string(true)?;
            let level = ndr.u32()?;
            if level != 100 && level != 101 {
                w.u32(level);
                w.u32(0);
                w.u32(WERR_UNKNOWN_LEVEL);
                return Some(w.out.0);
            }
            w.u32(level);
            w.pointer();
            // PLATFORM_ID_NT.
            w.u32(500);
            w.pointer();
            if level == 101 {
                // Version 6.1; a workstation and a server, of NT's kind.
                w.u32(6);
                w.u32(1);
                w.u32(0x0000_9003);
                w.pointer();
            }
            w.string(&server.name);
            if level == 101 {
                w.string("Dukovo");
            }
            w.u32(WERR_OK);
        }
        _ => return None,
    }
    Some(w.out.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server() -> Server {
        Server {
            name: "BOX".into(),
            shares: vec![
                Listed { name: "Music".into(), kind: DISK, comment: "tunes".into() },
                Listed { name: "IPC$".into(), kind: IPC, comment: "Remote procedure calls".into() },
            ],
            names: Box::new(lsa::tests::Fake),
        }
    }

    /// Reads every answer the pipe has, joined.
    fn drain(pipe: &mut Pipe) -> Vec<u8> {
        let mut all = Vec::new();
        while let Some((bytes, _)) = pipe.read(1 << 20) {
            all.extend(bytes);
        }
        all
    }

    #[test]
    fn smb2s_client_lists_the_shares() {
        let mut pipe = Pipe::default();
        pipe.write(&smb2::rpc::build_srvsvc_bind(1), &server());
        smb2::rpc::parse_bind_ack(&drain(&mut pipe)).unwrap();
        pipe.write(&smb2::rpc::srvsvc::build_net_share_enum_all(2, "BOX"), &server());
        let response = drain(&mut pipe);
        let shares = smb2::rpc::srvsvc::parse_net_share_enum_all_response(&response).unwrap();
        let names: Vec<_> = shares.iter().map(|s| (s.name.as_str(), s.comment.as_str())).collect();
        assert_eq!(names, [("Music", "tunes"), ("IPC$", "Remote procedure calls")]);
    }

    #[test]
    fn a_long_answer_comes_in_fragments() {
        let mut big = server();
        for i in 0..200 {
            big.shares.push(Listed {
                name: format!("share {i}"),
                kind: DISK,
                comment: "x".repeat(40),
            });
        }
        let mut pipe = Pipe::default();
        pipe.write(&smb2::rpc::build_srvsvc_bind(1), &big);
        drain(&mut pipe);
        pipe.write(&smb2::rpc::srvsvc::build_net_share_enum_all(2, "BOX"), &big);
        let mut stub = Vec::new();
        let mut fragments = 0;
        while let Some((pdu, more)) = pipe.read(1 << 20) {
            assert!(!more);
            assert!(pdu.len() <= 4280);
            let (part, _last) = smb2::rpc::parse_response_fragment(&pdu).unwrap();
            stub.extend_from_slice(part);
            fragments += 1;
        }
        assert!(fragments > 1);
        // The stub whole, in one PDU as the client's reader takes it.
        let mut body = Out::new();
        body.u32(stub.len() as u32).u16(0).u8(0).u8(0).bytes(&stub);
        let whole = pdu_of(RESPONSE, FIRST | LAST, 2, &body.0);
        let shares = smb2::rpc::srvsvc::parse_net_share_enum_all_response(&whole).unwrap();
        assert_eq!(shares.len(), 202);
    }

    #[test]
    fn reads_cut_an_answer_and_unknown_operations_fault() {
        let mut pipe = Pipe::default();
        pipe.write(&smb2::rpc::build_srvsvc_bind(1), &server());
        let (first, more) = pipe.read(10).unwrap();
        assert_eq!((first.len(), more), (10, true));
        let (_, more) = pipe.read(1 << 20).unwrap();
        assert!(!more);
        pipe.write(&smb2::rpc::build_request(3, 99, &[]), &server());
        assert_eq!(drain(&mut pipe)[2], FAULT);
        // Garbage, and every cut of a request, is answered or dropped.
        let request = smb2::rpc::srvsvc::build_net_share_enum_all(4, "BOX");
        for end in 0..request.len() {
            let mut pipe = Pipe::default();
            pipe.write(&smb2::rpc::build_srvsvc_bind(1), &server());
            pipe.write(&request[..end], &server());
        }
    }
}

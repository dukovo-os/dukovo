// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Mutation fuzzing, as unit tests, of what smbd reads from the network
//! (ADR 0229). Valid messages are bent at random, from a fixed seed so
//! that a failure is found again, and fed to the sign-in, to the worker
//! (sealed with the session's key, so that they reach its commands), to
//! the pipes, to the security descriptors' reader and to the NTLM and
//! SPNEGO readers. Nothing may panic, and every answer must be one the
//! caller can send.
//!
//! `SMBD_FUZZ=N` runs N rounds of each instead of the default, and
//! `SMBD_FUZZ_SEED=S` starts from another seed; a failure names its round
//! and input.

use std::fmt::Write;
use std::fs;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;

use acl::{Acl, Entry, NO_ID, Tag};
use manifest::shares::{Share, Shares};
use smb2::crypto::encryption::Cipher;

use crate::crypto::{Keys, Sealer, Signing, dialect};
use crate::listener::{SignIn, Step};
use crate::lsa;
use crate::negotiate::Negotiated;
use crate::rpc::{Interface, Listed, Pipe, Server};
use crate::sd::{self, Security};
use crate::state::Handoff;
use crate::wire::{self, HEADER, Header, Out, command, flags};
use crate::worker::Worker;
use crate::{auth, rpc};

/// xorshift64*: enough to bend bytes, and the same every run.
struct Rng(u64);

impl Rng {
    fn new(target: u64) -> Rng {
        let seed =
            std::env::var("SMBD_FUZZ_SEED").ok().and_then(|s| s.parse().ok()).unwrap_or(0x5eed);
        Rng((seed ^ target.wrapping_mul(0x9e37_79b9_7f4a_7c15)) | 1)
    }

    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    /// A number below `n`, or 0.
    fn below(&mut self, n: usize) -> usize {
        if n == 0 { 0 } else { (self.next() % n as u64) as usize }
    }
}

/// The rounds of each target.
fn rounds() -> usize {
    std::env::var("SMBD_FUZZ").ok().and_then(|n| n.parse().ok()).unwrap_or(4000)
}

/// Numbers that find the edges of lengths and offsets.
const EDGES: [u32; 12] = [0, 1, 2, 7, 8, 0x40, 0x7f, 0x80, 0xff, 0xffff, 0x7fff_ffff, 0xffff_ffff];

/// `seed`, bent one to four times: a bit flipped, a byte, a 16-bit or
/// 32-bit field set to an edge, the end cut, bytes put in, repeated or
/// taken out.
fn mutate(rng: &mut Rng, seed: &[u8]) -> Vec<u8> {
    let mut data = seed.to_vec();
    for _ in 0..=rng.below(4) {
        let at = rng.below(data.len());
        let edge = EDGES[rng.below(EDGES.len())];
        match rng.below(8) {
            0 if at < data.len() => data[at] ^= 1 << rng.below(8),
            1 if at < data.len() => data[at] = edge as u8,
            2 if at + 2 <= data.len() => {
                data[at..at + 2].copy_from_slice(&(edge as u16).to_le_bytes());
            }
            3 if at + 4 <= data.len() => data[at..at + 4].copy_from_slice(&edge.to_le_bytes()),
            4 => data.truncate(at),
            5 => {
                let more: Vec<u8> = (0..=rng.below(32)).map(|_| rng.next() as u8).collect();
                data.splice(at..at, more);
            }
            6 if at < data.len() => {
                let end = (at + 1 + rng.below(64)).min(data.len());
                let copy = data[at..end].to_vec();
                data.splice(at..at, copy);
            }
            _ if at < data.len() => {
                let end = (at + 1 + rng.below(16)).min(data.len());
                data.drain(at..end);
            }
            _ => data.push(rng.next() as u8),
        }
    }
    data
}

/// Runs `round` of a target on `input`, naming them if it panics.
fn guard(target: &str, round: usize, input: &[u8], run: impl FnOnce()) {
    if let Err(panic) = catch_unwind(AssertUnwindSafe(run)) {
        let why = panic
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| panic.downcast_ref::<&str>().map(|s| (*s).to_string()))
            .unwrap_or_default();
        let hex = input.iter().fold(String::new(), |mut hex, b| {
            let _ = write!(hex, "{b:02x}");
            hex
        });
        panic!("{target}, round {round}: {why}\ninput: {hex}");
    }
}

/// UTF-16, as SMB carries names.
fn utf16(text: &str) -> Vec<u8> {
    wire::to_utf16(text)
}

#[test]
fn descriptors() {
    let named = Acl {
        entries: vec![
            Entry { tag: Tag::UserObj, id: NO_ID, perm: 7 },
            Entry { tag: Tag::User, id: 1001, perm: 5 },
            Entry { tag: Tag::GroupObj, id: NO_ID, perm: 4 },
            Entry { tag: Tag::Group, id: 200, perm: 6 },
            Entry { tag: Tag::Mask, id: NO_ID, perm: 7 },
            Entry { tag: Tag::Other, id: NO_ID, perm: 0 },
        ],
    };
    let seeds = [
        sd::descriptor(
            &Security { uid: 1000, gid: 100, access: Acl::from_mode(0o640), default: None },
            7,
        ),
        sd::descriptor(
            &Security { uid: 0, gid: 0, access: named.clone(), default: Some(named) },
            7,
        ),
        sd::descriptor(
            &Security { uid: 5, gid: 5, access: Acl::from_mode(0o755), default: None },
            1,
        ),
    ];
    let mut rng = Rng::new(1);
    for round in 0..rounds() {
        let input = mutate(&mut rng, &seeds[round % seeds.len()]);
        guard("descriptors", round, &input, || {
            let Ok(given) = sd::parse(&input) else { return };
            let _ = sd::owner_of(&given);
            let _ = sd::group_of(&given);
            if let Some(aces) = &given.dacl
                && let Ok((access, default)) = sd::lists(aces, 1000, 100, round % 2 == 0)
            {
                // What is set is a whole list, with the mask it needs.
                assert_eq!(access.missing(), None);
                assert_eq!(access.mask().is_some(), access.has_named());
                if let Some(default) = default {
                    assert_eq!(default.missing(), None);
                }
            }
        });
    }
}

/// A BIND of lsarpc: srvsvc's, with lsarpc's interface and version.
fn lsarpc_bind() -> Vec<u8> {
    let mut bind = smb2::rpc::build_srvsvc_bind(1);
    let srvsvc = [0xc8, 0x4f, 0x32, 0x4b, 0x70, 0x16, 0xd3, 0x01];
    let at = bind.windows(8).position(|w| w == srvsvc).unwrap();
    bind[at..at + 16].copy_from_slice(&[
        0x78, 0x57, 0x34, 0x12, 0x34, 0x12, 0xcd, 0xab, 0xef, 0x00, 0x01, 0x23, 0x45, 0x67, 0x89,
        0xab,
    ]);
    bind[at + 16..at + 20].copy_from_slice(&[0; 4]);
    bind
}

/// lsarpc's requests: open a policy, look up SIDs and names with the
/// handle the first open gives, and close it.
fn lsarpc_requests() -> Vec<Vec<u8>> {
    let mut handle = [0u8; 20];
    handle[4] = 1;
    handle[8..].copy_from_slice(b"dukovo-lsarp");
    let mut open = rpc::Writer::default();
    open.u32(0x0002_0000);
    open.u16(u16::from(b'\\'));
    open.u32(24);
    open.out.zeros(20);
    open.u32(0x0800);
    let sids = [sd::Who::User(1000).sid(), sd::Who::Everyone.sid(), sd::Sid::new(5, &[18])];
    let mut lookup = rpc::Writer::default();
    lookup.out.bytes(&handle);
    lookup.u32(sids.len() as u32);
    lookup.pointer();
    lookup.u32(sids.len() as u32);
    for _ in &sids {
        lookup.pointer();
    }
    for sid in &sids {
        lookup.sid(sid);
    }
    lookup.u32(0);
    lookup.u32(0);
    lookup.u16(1);
    lookup.u32(0);
    let names = ["alice", "Unix Group\\staff", "Everyone", "nobody here"];
    let mut by_name = rpc::Writer::default();
    by_name.out.bytes(&handle);
    by_name.u32(names.len() as u32);
    by_name.u32(names.len() as u32);
    for name in names {
        by_name.unicode(name);
    }
    for name in names {
        by_name.units(name);
    }
    by_name.u32(0);
    by_name.u32(0);
    by_name.u16(1);
    by_name.u32(0);
    by_name.u32(0);
    by_name.u32(2);
    vec![
        smb2::rpc::build_request(2, 44, &open.out.0),
        smb2::rpc::build_request(3, 15, &lookup.out.0),
        smb2::rpc::build_request(4, 57, &[&lookup.out.0[..], &[0; 8]].concat()),
        smb2::rpc::build_request(5, 14, &by_name.out.0),
        smb2::rpc::build_request(6, 68, &by_name.out.0),
        smb2::rpc::build_request(7, 0, &handle),
    ]
}

#[test]
fn pipes() {
    let server = Server {
        name: "BOX".into(),
        shares: vec![
            Listed { name: "Music".into(), kind: rpc::DISK, comment: "tunes".into() },
            Listed { name: "IPC$".into(), kind: rpc::IPC, comment: "IPC".into() },
        ],
        names: Box::new(lsa::tests::Fake),
    };
    let mut get_info = rpc::Writer::default();
    get_info.u32(0);
    for text in ["Music\0"] {
        let units: Vec<u16> = text.encode_utf16().collect();
        get_info.u32(units.len() as u32);
        get_info.u32(0);
        get_info.u32(units.len() as u32);
        for unit in units {
            get_info.out.u16(unit);
        }
    }
    get_info.u32(1);
    let srvsvc = vec![
        smb2::rpc::srvsvc::build_net_share_enum_all(2, "BOX"),
        smb2::rpc::build_request(3, 16, &get_info.out.0),
        smb2::rpc::build_request(4, 21, &[0, 0, 0, 0, 101, 0, 0, 0]),
    ];
    let lsarpc = lsarpc_requests();
    let mut rng = Rng::new(2);
    for round in 0..rounds() {
        let (interface, bind, requests) = if round % 2 == 0 {
            (Interface::Srvsvc, smb2::rpc::build_srvsvc_bind(1), &srvsvc)
        } else {
            (Interface::Lsarpc, lsarpc_bind(), &lsarpc)
        };
        let bent = rng.below(requests.len() + 1);
        let mut input = Vec::new();
        let mut pipe = Pipe::new(interface);
        guard("pipes", round, &input.clone(), || {
            let bind = if bent == requests.len() { mutate(&mut rng, &bind) } else { bind };
            input = bind.clone();
            pipe.write(&bind, &server);
            for (i, request) in requests.iter().enumerate() {
                let request = if i == bent { mutate(&mut rng, request) } else { request.clone() };
                input.clone_from(&request);
                pipe.write(&request, &server);
                while let Some((pdu, _)) = pipe.read(16 + rng.below(4096)) {
                    assert!(!pdu.is_empty());
                }
            }
        });
    }
}

#[test]
fn ntlm_and_spnego() {
    use smb2::auth::ntlm::{NtlmAuthenticator, NtlmCredentials};
    let credentials =
        NtlmCredentials { username: "alice".into(), password: "pw".into(), domain: String::new() };
    let mut client = NtlmAuthenticator::new(credentials);
    let negotiate = client.negotiate();
    let names = auth::Names::of("box");
    let challenge = auth::challenge(&negotiate, [1; 8], 133_000_000_000_000_000, &names);
    let authenticate = client.authenticate(&challenge).unwrap();
    let hash = accounts::ntlm::nt_hash(b"pw").unwrap();
    let init = smb2::auth::spnego::wrap_neg_token_init(&[auth::OID_NTLMSSP], &negotiate);
    let resp = smb2::auth::spnego::wrap_neg_token_resp(&authenticate);
    let seeds = [init, resp, negotiate.clone(), challenge.clone(), authenticate.clone()];
    let mut rng = Rng::new(3);
    for round in 0..rounds() {
        let which = round % seeds.len();
        let input = mutate(&mut rng, &seeds[which]);
        guard("ntlm_and_spnego", round, &input, || match which {
            0 | 1 => {
                let _ = auth::read_token(&input);
            }
            2 => {
                let made = auth::challenge(&input, [1; 8], 0, &names);
                let _ = accounts::ntlm::check(&hash, &input, &made, &authenticate);
            }
            3 => {
                let _ = accounts::ntlm::check(&hash, &negotiate, &input, &authenticate);
            }
            _ => {
                let _ = auth::authenticate_flags(&input);
                let _ = accounts::ntlm::user_of(&input);
                // A bent AUTHENTICATE never signs in.
                if input != authenticate {
                    assert!(accounts::ntlm::check(&hash, &negotiate, &challenge, &input).is_err());
                }
            }
        });
    }
}

/// A SESSION_SETUP request carrying `token`.
fn session_setup(message_id: u64, session_id: u64, token: &[u8]) -> Vec<u8> {
    let header = Header {
        command: command::SESSION_SETUP,
        credits: 8,
        message_id,
        session_id,
        ..Header::default()
    };
    let mut body = Out::new();
    body.u16(25).u8(0).u8(1).u32(0).u32(0).u16((HEADER + 24) as u16).u16(token.len() as u16);
    body.u64(0).bytes(token);
    wire::message(&header, &body.0)
}

/// A sign-in's three frames, made against a listener with smb2's NTLM
/// client: NEGOTIATE and the two SESSION_SETUPs.
fn whole_sign_in() -> Vec<Vec<u8>> {
    use smb2::auth::ntlm::{NtlmAuthenticator, NtlmCredentials};
    let credentials =
        NtlmCredentials { username: "alice".into(), password: "pw".into(), domain: String::new() };
    let mut client = NtlmAuthenticator::new(credentials);
    let mut sign_in = new_sign_in();
    let negotiate = crate::negotiate::tests::request(
        &[dialect::SMB_3_0_2, dialect::SMB_3_1_1],
        &[2, 1],
        &[1],
        0x7f,
    );
    let Step::Answer(_) = sign_in.frame(&negotiate) else { panic!("NEGOTIATE refused") };
    let init = smb2::auth::spnego::wrap_neg_token_init(&[auth::OID_NTLMSSP], &client.negotiate());
    let first = session_setup(1, 0, &init);
    let Step::Answer(response) = sign_in.frame(&first) else { panic!("SESSION_SETUP refused") };
    let session_id = wire::u64_at(&response, 40).unwrap();
    let offset = usize::from(wire::u16_at(&response, HEADER + 4).unwrap());
    let length = usize::from(wire::u16_at(&response, HEADER + 6).unwrap());
    let token =
        smb2::auth::spnego::parse_neg_token_resp(&response[offset..offset + length]).unwrap();
    let authenticate = client.authenticate(&token.response_token.unwrap()).unwrap();
    let second =
        session_setup(2, session_id, &smb2::auth::spnego::wrap_neg_token_resp(&authenticate));
    let Step::Check(_) = sign_in.frame(&second) else { panic!("AUTHENTICATE not checked") };
    vec![negotiate, first, second]
}

fn new_sign_in() -> SignIn {
    SignIn::new("192.0.2.1".into(), [7; 16], "box", [1; 32], 42, vec![[2; 8], [3; 8], [4; 8]])
}

#[test]
fn sign_in() {
    let frames = whole_sign_in();
    let mut rng = Rng::new(4);
    for round in 0..rounds() {
        let bent = rng.below(frames.len());
        let input = mutate(&mut rng, &frames[bent]);
        guard("sign_in", round, &input, || {
            let mut sign_in = new_sign_in();
            for (i, frame) in frames.iter().enumerate() {
                let frame = if i == bent { &input } else { frame };
                match sign_in.frame(frame) {
                    Step::Answer(_) => {}
                    Step::End(..) => return,
                    Step::Check(pending) => {
                        let _ = sign_in.finish(&pending, Err("not checked here".into()));
                        return;
                    }
                }
            }
        });
    }
}

/// The session the worker's target uses.
const SESSION: u64 = 0x0000_4000_0000_0042;
/// The file of the last CREATE in a compound.
const CHAINED: [u8; 16] = [0xff; 16];

/// A request, with its tree and its place in a compound.
fn request(command: u16, message_id: u64, tree: u32, related: bool, body: &[u8]) -> Vec<u8> {
    let header = Header {
        command,
        credit_charge: 1,
        credits: 8,
        flags: if related { flags::RELATED_OPERATIONS } else { 0 },
        message_id,
        async_id: u64::from(tree) << 32,
        session_id: SESSION,
        ..Header::default()
    };
    wire::message(&header, body)
}

/// Requests made one compound: each but the last padded to 8 bytes and
/// pointing at the next.
fn compound(messages: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::new();
    for (i, message) in messages.iter().enumerate() {
        let mut message = message.clone();
        if i + 1 < messages.len() {
            message.resize(message.len().next_multiple_of(8), 0);
            let next = message.len() as u32;
            message[20..24].copy_from_slice(&next.to_le_bytes());
        }
        out.extend(message);
    }
    out
}

fn tree_connect(path: &str) -> Vec<u8> {
    let path = utf16(path);
    let mut body = Out::new();
    body.u16(9).u16(0).u16((HEADER + 8) as u16).u16(path.len() as u16).bytes(&path);
    body.0
}

fn create(name: &str, access: u32, disposition: u32, options: u32) -> Vec<u8> {
    let name = utf16(name);
    let mut body = Out::new();
    body.u16(57).u8(0).u8(0).u32(2).u64(0).u64(0).u32(access).u32(0x80).u32(7);
    body.u32(disposition).u32(options).u16((HEADER + 56) as u16).u16(name.len() as u16);
    body.u32(0).u32(0).bytes(&name);
    if name.is_empty() {
        body.u8(0);
    }
    body.0
}

fn query_info(kind: u8, class: u8, additional: u32, file: [u8; 16]) -> Vec<u8> {
    let mut body = Out::new();
    body.u16(41).u8(kind).u8(class).u32(65_536).u16(0).u16(0).u32(0).u32(additional).u32(0);
    body.bytes(&file);
    body.0
}

fn read(file: [u8; 16], length: u32) -> Vec<u8> {
    let mut body = Out::new();
    body.u16(49).u8(0).u8(0).u32(length).u64(0).bytes(&file).u32(0).u32(0).u32(0);
    body.u16(0).u16(0).u8(0);
    body.0
}

fn write(file: [u8; 16], data: &[u8]) -> Vec<u8> {
    let mut body = Out::new();
    body.u16(49).u16((HEADER + 48) as u16).u32(data.len() as u32).u64(0).bytes(&file);
    body.u32(0).u32(0).u16(0).u16(0).u32(0).bytes(data);
    body.0
}

fn query_directory(file: [u8; 16], pattern: &str) -> Vec<u8> {
    let pattern = utf16(pattern);
    let mut body = Out::new();
    body.u16(33).u8(0x25).u8(0).u32(0).bytes(&file).u16((HEADER + 32) as u16);
    body.u16(pattern.len() as u16).u32(65_536).bytes(&pattern);
    body.0
}

fn ioctl(code: u32, file: [u8; 16], input: &[u8]) -> Vec<u8> {
    let mut body = Out::new();
    body.u16(57).u16(0).u32(code).bytes(&file).u32((HEADER + 56) as u32).u32(input.len() as u32);
    body.u32(0).u32(0).u32(0).u32(65_536).u32(1).u32(0).bytes(input);
    body.0
}

fn set_info(kind: u8, class: u8, additional: u32, file: [u8; 16], buffer: &[u8]) -> Vec<u8> {
    let mut body = Out::new();
    body.u16(33).u8(kind).u8(class).u32(buffer.len() as u32).u16((HEADER + 32) as u16).u16(0);
    body.u32(additional).bytes(&file).bytes(buffer);
    body.0
}

fn close(file: [u8; 16]) -> Vec<u8> {
    let mut body = Out::new();
    body.u16(24).u16(1).u32(0).bytes(&file);
    body.0
}

/// A folder of the worker's target's, with a file and a folder in it.
fn folder() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("smbd-fuzz-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("sub")).unwrap();
    fs::write(dir.join("a.txt"), b"fuzzed, never changed\n").unwrap();
    fs::canonicalize(&dir).unwrap()
}

/// A worker for a session with one key both ways, and its share.
fn worker(root: &std::path::Path) -> (Worker, Keys) {
    let key = vec![0x11; 16];
    let keys = || Keys { signing: [0x22; 16], decrypt: key.clone(), encrypt: key.clone() };
    let negotiated = Negotiated {
        dialect: dialect::SMB_3_1_1,
        cipher: Cipher::Aes128Gcm,
        signing: Signing::Cmac,
        client_guid: [5; 16],
        client_security_mode: 1,
        client_capabilities: 0x7f,
        client_dialects: vec![dialect::SMB_3_1_1],
        server_guid: [6; 16],
        server_capabilities: 0x6c,
        preauth: [0; 64],
    };
    let handoff = Handoff {
        negotiated,
        session_id: SESSION,
        next_message: 3,
        keys: keys(),
        user: "alice".into(),
        from: "fuzz".into(),
    };
    let mut worker = Worker::new(handoff);
    // Read-only: a bent name that got out of the share could only read.
    worker.catalog.shares = Shares {
        shares: vec![Share {
            name: "Docs".into(),
            folder: root.to_string_lossy().into_owned(),
            comment: String::new(),
            read_only: true,
        }],
    };
    (worker, keys())
}

/// The worker's target's seeds: compounds on the share and on IPC$,
/// after message 3 connects tree 1 to IPC$ and message 4 tree 2 to the
/// share.
fn worker_seeds() -> Vec<Vec<u8>> {
    let read_data = 0x0012_0089;
    let descriptor = sd::descriptor(
        &Security { uid: 1000, gid: 100, access: Acl::from_mode(0o644), default: None },
        7,
    );
    vec![
        compound(&[
            request(command::CREATE, 5, 2, false, &create("a.txt", read_data, 1, 0x40)),
            request(command::QUERY_INFO, 6, 2, true, &query_info(1, 0x12, 0, CHAINED)),
            request(command::READ, 7, 2, true, &read(CHAINED, 4096)),
            request(command::QUERY_INFO, 8, 2, true, &query_info(3, 0, 7, CHAINED)),
            request(command::CLOSE, 9, 2, true, &close(CHAINED)),
        ]),
        compound(&[
            request(command::CREATE, 5, 2, false, &create("", read_data, 1, 1)),
            request(command::QUERY_DIRECTORY, 6, 2, true, &query_directory(CHAINED, "*")),
            request(command::QUERY_INFO, 7, 2, true, &query_info(2, 7, 0, CHAINED)),
            request(command::SET_INFO, 8, 2, true, &set_info(3, 0, 4, CHAINED, &descriptor)),
            request(command::CLOSE, 9, 2, true, &close(CHAINED)),
        ]),
        compound(&[
            request(command::CREATE, 5, 1, false, &create("lsarpc", 0x0012_019f, 1, 0)),
            request(command::WRITE, 6, 1, true, &write(CHAINED, &lsarpc_bind())),
            request(command::READ, 7, 1, true, &read(CHAINED, 4280)),
            request(
                command::IOCTL,
                8,
                1,
                true,
                &ioctl(0x0011_c017, CHAINED, &lsarpc_requests()[0]),
            ),
        ]),
        request(command::CREATE, 5, 2, false, &create("sub\\..\\..\\a.txt", read_data, 1, 0)),
    ]
}

/// A worker with trees 1 and 2 connected, and a sealer of what is sent
/// to it.
fn connected(root: &std::path::Path) -> (Worker, Keys, Sealer) {
    let (mut worker, keys) = worker(root);
    let mut sealer = Sealer { cipher: Cipher::Aes128Gcm, count: 1 << 40 };
    let connect = compound(&[
        request(command::TREE_CONNECT, 3, 0, false, &tree_connect("\\\\BOX\\IPC$")),
        request(command::TREE_CONNECT, 4, 0, false, &tree_connect("\\\\BOX\\Docs")),
    ]);
    let answers = worker.frame(&sealer.seal(&keys, SESSION, &connect)).unwrap();
    assert_eq!(statuses(&answers, &keys), [0, 0]);
    (worker, keys, sealer)
}

/// The statuses of the responses in sealed `answers`.
fn statuses(answers: &[Vec<u8>], keys: &Keys) -> Vec<u32> {
    let mut all = Vec::new();
    for answer in answers {
        let plain = crate::crypto::unseal(answer, keys, Cipher::Aes128Gcm).unwrap();
        let mut at = 0;
        loop {
            let header = Header::parse(&plain[at..]).unwrap();
            all.push(header.status);
            if header.next_command == 0 {
                break;
            }
            at += header.next_command as usize;
        }
    }
    all
}

#[test]
fn the_worker_seeds_reach_the_commands() {
    let root = folder().join("..").join(format!("smbd-seeds-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("sub")).unwrap();
    fs::write(root.join("a.txt"), b"seed\n").unwrap();
    let denied = wire::status::ACCESS_DENIED;
    let expected: [&[u32]; 4] = [
        &[0, 0, 0, 0, 0],
        // A read-only share's descriptors are not set, and the CLOSE
        // related to that fails as it did (MS-SMB2 3.3.5.2.7.2).
        &[0, 0, 0, denied, denied],
        &[0, 0, 0, 0],
        // A name that would leave the share is not one.
        &[wire::status::OBJECT_NAME_INVALID],
    ];
    for (seed, expected) in worker_seeds().iter().zip(expected) {
        let (mut worker, keys, mut sealer) = connected(&root);
        let answers = worker.frame(&sealer.seal(&keys, SESSION, seed)).unwrap();
        assert_eq!(statuses(&answers, &keys), expected);
    }
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn worker_requests() {
    let root = folder();
    let seeds = worker_seeds();
    let mut rng = Rng::new(5);
    for round in 0..rounds() {
        let (mut worker, keys, mut sealer) = connected(&root);
        let input = mutate(&mut rng, &seeds[round % seeds.len()]);
        guard("worker_requests", round, &input, || {
            if let Ok(answers) = worker.frame(&sealer.seal(&keys, SESSION, &input)) {
                for answer in answers {
                    assert!(answer.starts_with(&wire::PROTOCOL_ENCRYPTED));
                }
            }
        });
    }
    assert_eq!(fs::read(root.join("a.txt")).unwrap(), b"fuzzed, never changed\n");
    fs::remove_dir_all(&root).unwrap();
}

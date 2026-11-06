// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! nfsd with Kerberos: shares that take only the flavors they list, what
//! SECINFO says of them, and RPCSEC_GSS's calls through the listener,
//! from setting a context up to its end.

use std::net::IpAddr;
use std::path::Path;
use std::sync::Mutex;

use kerberos::crypto::Enctype;
use kerberos::gss::{Context as Gss, Initiator, flags, testing};

use super::{CLIENT, Local, Session, after_sequence, cx, dirs, lookup, next, op, putfh, results};
use crate::contexts::{Accounts, Contexts, Keys};
use crate::gss::{self, Cred, Flavor, InitResult, RPCSEC_GSS, Service, proc};
use crate::listener::answer;
use crate::nfs::err;
use crate::rpc;
use crate::server::{Context, Server};
use crate::shares;
use crate::worker::Worker;
use crate::xdr::{Reader, Writer};

/// A server sharing `music` as Music, to AUTH_SYS alone, and `docs` as
/// Secret, to krb5p and krb5i, both to 10.0.2.0/24.
fn server(music: &Path, docs: &Path) -> Server<Local> {
    let text = format!(
        "[[share]]\nname = \"Music\"\nfolder = \"{}\"\nnfs = [\"10.0.2.0/24\"]\n\
         [[share]]\nname = \"Secret\"\nfolder = \"{}\"\nnfs = [\"10.0.2.0/24\"]\n\
         nfs-sec = [\"krb5p\", \"krb5i\"]\n",
        music.display(),
        docs.display(),
    );
    let shares = shares::of_toml(&text).unwrap();
    let worker = Worker::new(&[music.to_path_buf(), docs.to_path_buf()]);
    Server::new(Local { worker, people: Vec::new() }, 7, [3; 8], "test").with_shares(shares)
}

/// Calls as `uid`, with `flavor`.
fn as_flavor(uid: u32, flavor: Flavor) -> Context {
    Context { peer: CLIENT.parse().unwrap(), uid, flavor }
}

/// The flavors of a SECINFO result.
fn flavors(r: &mut Reader) -> Vec<Flavor> {
    let n = r.u32().unwrap();
    (0..n)
        .map(|_| match r.u32().unwrap() {
            rpc::AUTH_SYS => Flavor::Sys,
            RPCSEC_GSS => {
                assert_eq!(r.opaque(64).unwrap(), gss::krb5_oid());
                assert_eq!(r.u32(), Ok(0));
                Flavor::Krb5(Service::from_number(r.u32().unwrap()).unwrap())
            }
            other => panic!("flavor {other}"),
        })
        .collect()
}

#[test]
fn shares_take_only_the_flavors_they_list() {
    let (music, docs) = dirs("flavors");
    let mut server = server(&music, &docs);
    let mut s = Session::new(&mut server, b"client");
    let sys = cx(1000);
    let krb5p = as_flavor(1000, Flavor::Krb5(Service::Privacy));
    let root = || op(op::PUTROOTFH, |_| {});
    let no_name = || {
        op(op::SECINFO_NO_NAME, |w| {
            w.u32(0);
        })
    };
    let secret = [Flavor::Krb5(Service::Privacy), Flavor::Krb5(Service::Integrity)];

    // AUTH_SYS is turned away at Secret, and told what it takes.
    let answer = s.call_as(&mut server, &sys, &[root(), lookup("Secret")]);
    assert_eq!(after_sequence(&answer).0, err::WRONGSEC);
    let named = op(op::SECINFO, |w| {
        w.opaque(b"Secret");
    });
    let answer = s.call_as(&mut server, &sys, &[root(), named]);
    let (status, mut r) = after_sequence(&answer);
    assert_eq!(status, err::OK);
    next(&mut r);
    assert_eq!(next(&mut r), (op::SECINFO, err::OK));
    assert_eq!(flavors(&mut r), secret);
    // The root takes what any share takes.
    let answer = s.call_as(&mut server, &sys, &[root(), no_name()]);
    let (_, mut r) = after_sequence(&answer);
    next(&mut r);
    next(&mut r);
    assert_eq!(flavors(&mut r), [&[Flavor::Sys][..], &secret].concat());

    // krb5p reaches Secret, and not Music.
    let answer = s.call_as(&mut server, &krb5p, &[root(), lookup("Secret"), op(op::GETFH, |_| {})]);
    let (status, mut r) = after_sequence(&answer);
    assert_eq!(status, err::OK);
    next(&mut r);
    next(&mut r);
    next(&mut r);
    let fh = r.opaque(128).unwrap().to_vec();
    let answer = s.call_as(&mut server, &krb5p, &[root(), lookup("Music")]);
    assert_eq!(after_sequence(&answer).0, err::WRONGSEC);

    // With its handle, AUTH_SYS is turned away at the PUTFH when an
    // operation on the file follows, and not when a SECINFO does, or
    // nothing.
    let attrs = op(op::GETATTR, |w| {
        w.u32(0);
    });
    let answer = s.call_as(&mut server, &sys, &[putfh(&fh), attrs]);
    let (status, mut r) = after_sequence(&answer);
    assert_eq!((status, next(&mut r)), (err::WRONGSEC, (op::PUTFH, err::WRONGSEC)));
    let answer = s.call_as(&mut server, &sys, &[putfh(&fh), no_name()]);
    let (status, mut r) = after_sequence(&answer);
    assert_eq!(status, err::OK);
    next(&mut r);
    next(&mut r);
    assert_eq!(flavors(&mut r), secret);
    let answer = s.call_as(&mut server, &sys, &[putfh(&fh)]);
    assert_eq!(after_sequence(&answer).0, err::OK);
}

/// An NFS call of `procedure` with RPCSEC_GSS's `cred`, its header
/// signed under `context` when there is one, and `args`.
fn gss_call(
    xid: u32,
    procedure: u32,
    cred: &Cred,
    context: Option<&mut Gss>,
    args: &[u8],
) -> Vec<u8> {
    let mut w = Writer::new();
    w.u32(xid).u32(0).u32(2).u32(rpc::NFS_PROGRAM).u32(4).u32(procedure);
    w.u32(RPCSEC_GSS).opaque(&cred.encode());
    match context {
        Some(context) => {
            let mic = context.get_mic(&w.bytes);
            w.u32(RPCSEC_GSS).opaque(&mic);
        }
        None => {
            w.u32(rpc::AUTH_NONE).u32(0);
        }
    }
    w.bytes.extend_from_slice(args);
    w.bytes
}

/// An accepted reply's verifier and status, and where its results
/// start.
fn accepted(reply: &[u8]) -> (Vec<u8>, u32, usize) {
    let mut r = Reader::new(reply);
    r.u32().unwrap();
    assert_eq!((r.u32(), r.u32()), (Ok(1), Ok(0)), "an accepted reply");
    assert_eq!(r.u32(), Ok(RPCSEC_GSS));
    let verifier = r.opaque(400).unwrap().to_vec();
    let status = r.u32().unwrap();
    (verifier, status, r.position())
}

#[test]
fn kerberos_calls_are_served_as_their_principals_account() {
    let (music, docs) = dirs("rpcsec-gss");
    let mut plain = server(&music, &docs);
    let (_, keytab) = testing::issue("alice@EXAMPLE.ORG", testing::SERVICE, Enctype::Aes256Sha384);
    let credential = testing::ticket("alice@EXAMPLE.ORG", &keytab[1], Enctype::Aes256Sha384);
    let accounts = Accounts::Given(vec![("alice".into(), 1000)]);
    plain.contexts = Contexts::new([5; 4], Keys::Given(keytab), accounts);
    let mut s = Session::new(&mut plain, b"client");
    let server = Mutex::new(plain);
    let peer: IpAddr = CLIENT.parse().unwrap();

    // The context is set up with NULL, and the window signed.
    let (initiator, token) = Initiator::start(&credential, flags::MUTUAL).unwrap();
    let init = Cred { proc: proc::INIT, seq: 0, service: Service::Privacy, handle: Vec::new() };
    let reply = answer(&server, peer, &gss_call(1, 0, &init, None, &gss::init_args(&token)));
    let reply = reply.unwrap();
    let (mic, status, at) = accepted(&reply);
    assert_eq!(status, rpc::SUCCESS);
    let result = InitResult::decode(&reply[at..]).unwrap();
    let mut context = initiator.finish(Some(&result.token)).unwrap();
    context.verify_mic(&result.window.to_be_bytes(), &mic).unwrap();

    // A sealed COMPOUND is served as alice's account, and its results
    // come back sealed.
    let data = |seq| Cred {
        proc: proc::DATA,
        seq,
        service: Service::Privacy,
        handle: result.handle.clone(),
    };
    let mut args = Writer::new();
    args.opaque(b"").u32(2).u32(3);
    for o in [s.sequence(false), op(op::PUTROOTFH, |_| {}), lookup("Secret")] {
        args.bytes.extend_from_slice(&o);
    }
    let sealed = gss::protect(&mut context, Service::Privacy, 1, &args.bytes).unwrap();
    let call = gss_call(2, 1, &data(1), Some(&mut context), &sealed);
    let reply = answer(&server, peer, &call).unwrap();
    let (mic, status, at) = accepted(&reply);
    assert_eq!(status, rpc::SUCCESS);
    gss::check_reply_verifier(&context, 1, &mic).unwrap();
    let opened = gss::unprotect(&context, Service::Privacy, 1, &reply[at..]).unwrap();
    assert_eq!(results(&opened).0, err::OK);
    let people = server.lock().unwrap().backend.people.clone();
    assert!(!people.is_empty() && people.iter().all(|&p| p == 1000), "{people:?}");

    // The same call again is a replay, and is not answered; arguments
    // sealed for another call are garbage.
    assert!(answer(&server, peer, &call).is_none());
    let moved = gss_call(3, 1, &data(2), Some(&mut context), &sealed);
    assert_eq!(accepted(&answer(&server, peer, &moved).unwrap()).1, rpc::GARBAGE_ARGS);

    // After DESTROY, the context is no more.
    let destroy = Cred { proc: proc::DESTROY, ..data(3) };
    let reply = answer(&server, peer, &gss_call(4, 0, &destroy, Some(&mut context), &[])).unwrap();
    assert_eq!(accepted(&reply).1, rpc::SUCCESS);
    let reply = answer(&server, peer, &gss_call(5, 1, &data(4), Some(&mut context), &sealed));
    // Denied, AUTH_ERROR, CREDPROBLEM.
    assert_eq!(reply.unwrap()[8..], [0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, gss::CREDPROBLEM as u8]);
}

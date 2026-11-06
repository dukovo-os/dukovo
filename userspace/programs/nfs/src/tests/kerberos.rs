// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The client with Kerberos against nfsd in the tests' process: a mount
//! with each of `sec=krb5`, `krb5i` and `krb5p` writes and reads back,
//! and the refusals of a share that takes another flavor and of a
//! ticket the server has no key for.

use std::sync::{Arc, Mutex};

use kerberos::creds::Credential;
use kerberos::crypto::Enctype;
use kerberos::gss::testing;
use kerberos::keytab;
use kerberos::messages::Principal;

use super::{name, tempdir};
use crate::fs::{NfsFs, ROOT};
use crate::options::{Options, Target};
use crate::rpc::Cred;
use crate::security::{Kerberos, Tickets};

/// Tickets as a KDC gave them: the same one for everyone.
struct Given(Credential);

impl Tickets for Given {
    fn ticket(&mut self, _uid: u32, _server: &Principal) -> Result<Credential, String> {
        Ok(self.0.clone())
    }
}

/// A share of a fresh folder that takes `sec`, served under a keytab,
/// mounted with `options`, alice's ticket being under `key`: the mount
/// or why it failed, and the folder.
fn mount(
    test: &str,
    sec: &str,
    options: &str,
    key: impl FnOnce(&keytab::Entry) -> keytab::Entry,
) -> (Result<(NfsFs, Cred), String>, std::path::PathBuf) {
    let dir = tempdir(test);
    let (_, keytab) = testing::issue("alice@EXAMPLE.ORG", testing::SERVICE, Enctype::Aes256Sha1);
    let ticket = testing::ticket("alice@EXAMPLE.ORG", &key(&keytab[1]), Enctype::Aes128Sha256);
    let shares = [("Secret", dir.clone(), sec)];
    let addr = nfsd::testing::serve_kerberos(&shares, keytab).unwrap();
    let mut given = Options::default();
    given.add(&format!("soft,timeo=50,port={},{options}", addr.port())).unwrap();
    let target = Target::parse("127.0.0.1:/Secret").unwrap();
    let kerberos = given.sec.map(|service| {
        let server = Principal::parse(testing::SERVICE, None).unwrap();
        Kerberos::new(service, server, Box::new(Given(ticket)))
    });
    // SAFETY: getuid and getgid only read the process's IDs.
    let (uid, gid) = unsafe { (libc::getuid(), libc::getgid()) };
    let cred = Cred { uid, gid, gids: Vec::new() };
    let mounted = crate::connect_with(&target, &given, &cred, kerberos).map(|(client, root)| {
        (NfsFs::new(Arc::new(Mutex::new(client)), &root, given.read_only), cred)
    });
    (mounted, dir)
}

#[test]
fn each_kerberos_service_writes_and_reads_back() {
    for sec in ["krb5", "krb5i", "krb5p"] {
        let (mounted, dir) = mount(sec, "krb5:krb5i:krb5p", &format!("sec={sec}"), Clone::clone);
        let (mut fs, cred) = mounted.unwrap();
        let data: Vec<u8> = (0..100_000u32).map(|i| (i % 253) as u8).collect();
        let (_, handle) = fs.create(&cred, ROOT, name("song"), 0o600, libc::O_RDWR).unwrap();
        assert_eq!(fs.write(handle, 0, &data).unwrap(), 100_000);
        fs.commit(handle).unwrap();
        assert_eq!(fs.read(handle, 0, 100_000).unwrap(), data, "{sec}");
        fs.release(handle).unwrap();
        assert_eq!(std::fs::read(dir.join("song")).unwrap(), data);
    }
}

#[test]
fn shares_and_servers_that_refuse_say_why() {
    // A share for Kerberos alone, mounted with AUTH_SYS.
    let (mounted, _) = mount("sys-refused", "krb5p", "sec=sys", Clone::clone);
    let err = mounted.err().unwrap();
    assert!(err.contains("sec="), "{err}");
    // A ticket under a key the server's keytab does not hold.
    let other = |entry: &keytab::Entry| {
        let key = kerberos::crypto::Key::random(Enctype::Aes256Sha1).unwrap();
        keytab::Entry { key, ..entry.clone() }
    };
    let (mounted, _) = mount("no-key", "krb5p", "sec=krb5p", other);
    let err = mounted.err().unwrap();
    assert!(err.contains("refused alice@EXAMPLE.ORG's ticket"), "{err}");
}

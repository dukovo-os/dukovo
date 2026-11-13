// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `cert add`: the checks a certificate from elsewhere passes before certd
//! keeps it (ADR 0512).
//!
//! A service that names a certificate takes whatever is in it: a wrong
//! key, an expired certificate or a chain a browser refuses would show
//! only when a client connects. So each is refused here, in this order,
//! with a message of its own: the name, the files, the key, the first
//! certificate, and the chain against the system's roots. What is
//! stored is re-encoded from what was read, so that the files Spark copies
//! hold the certificates and the key and nothing else.

use std::fs::File;
use std::io::Read;
use std::path::Path;

use manifest::service;
use rustls::RootCertStore;
use rustls::pki_types::CertificateDer;

use super::inspect::{self, Details, Trust};
use super::machine;
use super::store::{Kind, Store};
use crate::util;

/// The most bytes of a file `cert add` takes: Spark copies no more.
pub const FILE_MAX: u64 = 64 * 1024;

/// The most certificates in a chain file.
const CHAIN_MAX: usize = 10;

/// What a person asked `cert add` to add.
pub struct Request<'a> {
    pub name: &'a str,
    pub chain: &'a Path,
    pub key: &'a Path,
    pub replace: bool,
    /// Take a chain the system's roots do not lead to, for an authority
    /// the clients trust that this machine does not.
    pub untrusted: bool,
}

/// A certificate that passed, as certd keeps it.
#[derive(Debug)]
pub struct Checked {
    /// The `fullchain.pem` and `key.pem` to write.
    pub chain: String,
    pub key: String,
    /// What the first certificate says of itself.
    pub details: Details,
    /// What the person should know, one line each.
    pub notes: Vec<String>,
}

/// A file's text, and the path it is called in messages.
pub struct Text {
    pub shown: String,
    pub text: String,
}

/// Checks `request` against `store`'s certificates and the files, at time
/// `now`.
///
/// # Errors
///
/// With the first thing that is wrong, in words that say what to do.
pub fn check(
    store: &Store,
    request: &Request<'_>,
    roots: &RootCertStore,
    now: i64,
) -> Result<Checked, String> {
    check_name(store, request.name, request.replace)?;
    let chain = read_limited(request.chain)?;
    let key = read_limited(request.key)?;
    check_texts(&chain, &key, request.untrusted, roots, now)
}

/// Step 1: the name is a name, is not certd's own, and is free, or is an
/// added certificate when it is to be replaced.
fn check_name(store: &Store, name: &str, replace: bool) -> Result<(), String> {
    service::check_certificate(name)?;
    if name == machine::NAME {
        return Err(format!(
            "{name} is the machine's own certificate, which certd makes: `cert renew {name}` \
             makes it anew"
        ));
    }
    let existing = store.generation(name)?.map(|_| store.get(name)).transpose()?;
    match (existing, replace) {
        (None, false) => Ok(()),
        (None, true) => {
            Err(format!("there is no certificate {name} to replace: leave out --replace"))
        }
        (Some(_), false) => Err(format!(
            "the certificate {name} exists: add its new version with `cert add {name} --replace`, \
             or choose another name"
        )),
        (Some(stored), true) if stored.kind != Kind::Added => Err(format!(
            "{name} is a {} certificate, which certd made: only one added by hand is replaced",
            stored.kind.word()
        )),
        (Some(_), true) => Ok(()),
    }
}

/// Reads `path`, which may hold [`FILE_MAX`] bytes at most, as text.
fn read_limited(path: &Path) -> Result<Text, String> {
    let shown = path.display().to_string();
    let file =
        File::open(path).map_err(|err| format!("{shown}: {}", util::strip_os_error(&err)))?;
    let mut bytes = Vec::new();
    file.take(FILE_MAX + 1)
        .read_to_end(&mut bytes)
        .map_err(|err| format!("{shown}: {}", util::strip_os_error(&err)))?;
    if bytes.len() as u64 > FILE_MAX {
        return Err(format!(
            "{shown} is larger than {} KiB, which is the most certd keeps",
            FILE_MAX / 1024
        ));
    }
    let text = String::from_utf8(bytes).map_err(|_| format!("{shown} is not PEM text"))?;
    Ok(Text { shown, text })
}

/// Steps 2 to 5, on the chain file and the key file.
pub(super) fn check_texts(
    chain: &Text,
    key: &Text,
    untrusted: bool,
    roots: &RootCertStore,
    now: i64,
) -> Result<Checked, String> {
    let certificates =
        inspect::chain_of(&chain.text).map_err(|why| format!("{}: {why}", chain.shown))?;
    if certificates.is_empty() || certificates.len() > CHAIN_MAX {
        return Err(format!(
            "{} holds {} certificates: it holds 1 to {CHAIN_MAX}, the certificate first",
            chain.shown,
            certificates.len()
        ));
    }
    let blocks = inspect::pem_blocks(&key.text).map_err(|why| format!("{}: {why}", key.shown))?;
    let [block] = blocks.as_slice() else {
        return Err(format!(
            "{} holds {} PEM blocks: it holds the private key alone",
            key.shown,
            blocks.len()
        ));
    };
    let private = inspect::private_key(block).map_err(|why| format!("{}: {why}", key.shown))?;
    inspect::key_fits(&certificates, &private)?;
    let details = check_first(&certificates[0], &chain.shown, now)?;
    let mut notes = Vec::new();
    match inspect::trust(&certificates, roots, now) {
        Trust::Trusted => {}
        Trust::SelfSigned => {
            notes.push(String::from("self-signed: browsers warn unless told to trust it"));
        }
        Trust::UnknownIssuer(issuer) if untrusted => notes.push(format!(
            "not trusted here: {issuer} is no authority Dukovo trusts; clients that do not trust \
             it will warn"
        )),
        Trust::UnknownIssuer(issuer) => {
            return Err(format!(
                "the chain stops at {issuer}, which no authority Dukovo trusts signed: the file \
                 lacks {issuer}'s intermediate certificate, which your authority gives with the \
                 certificate (put it after the certificate in {}); a private authority that the \
                 clients trust and this machine does not is taken with --untrusted",
                chain.shown
            ));
        }
        Trust::Refused(why) => return Err(why),
    }
    Ok(Checked {
        chain: certificates
            .iter()
            .map(|der| inspect::pem_encode(inspect::CERTIFICATE, der.as_ref()))
            .collect(),
        key: inspect::pem_encode(&block.label, &block.der),
        details,
        notes,
    })
}

/// Step 4: the first certificate is valid now, names a host, and is no
/// authority's.
fn check_first(
    first: &CertificateDer<'static>,
    chain_file: &str,
    now: i64,
) -> Result<Details, String> {
    let details = inspect::details(first).map_err(|why| format!("{chain_file}: {why}"))?;
    if details.not_after <= now {
        let date = util::local_time(details.not_after, "%F");
        return Err(format!("the certificate ran out on {date}: add one that is valid now"));
    }
    if details.not_before > now {
        let date = util::local_time(details.not_before, "%F");
        return Err(format!("the certificate starts on {date}, which is not yet: add it then"));
    }
    if details.names.is_empty() {
        return Err(String::from(
            "the certificate names no host: it has no DNS or IP name among its subject \
             alternative names, which is where clients look",
        ));
    }
    if details.authority {
        return Err(format!(
            "the first certificate is a certificate authority's, not a server's: put the \
             server's certificate first in {chain_file}"
        ));
    }
    Ok(details)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::Duration;

    use super::*;
    use crate::cert::fixtures::{self, Authority, DAY, Leaf, NOW, Scratch, Server};
    use crate::cert::store::Files;

    /// An intermediate and the roots a system trusts, which are its root.
    struct Pki {
        middle: Authority,
        roots: RootCertStore,
    }

    fn pki() -> Pki {
        let root = fixtures::root("Test Root");
        let middle = fixtures::intermediate(&root, "Test Intermediate");
        let roots = fixtures::roots(&[&root]);
        Pki { middle, roots }
    }

    fn text(shown: &str, text: String) -> Text {
        Text { shown: shown.to_string(), text }
    }

    fn run(
        chain: &str,
        key: &str,
        untrusted: bool,
        roots: &RootCertStore,
    ) -> Result<Checked, String> {
        check_texts(
            &text("chain.pem", chain.into()),
            &text("key.pem", key.into()),
            untrusted,
            roots,
            NOW,
        )
    }

    fn full(leaf: &Leaf, pki: &Pki) -> String {
        fixtures::chain_file(&[&leaf.der, pki.middle.der()])
    }

    #[test]
    fn a_chain_to_a_trusted_root_is_added_as_it_stands() {
        let pki = pki();
        let leaf = fixtures::server(&pki.middle, &Server::default());
        let checked = run(&full(&leaf, &pki), &leaf.key, false, &pki.roots).unwrap();
        assert_eq!(checked.details.names, ["nas.example"]);
        assert!(checked.notes.is_empty());
        assert_eq!(inspect::chain_of(&checked.chain).unwrap().len(), 2);
        assert_eq!(checked.key, leaf.key);
    }

    #[test]
    fn what_is_kept_is_re_encoded_without_carriage_returns() {
        let pki = pki();
        let leaf = fixtures::server(&pki.middle, &Server::default());
        let windows = full(&leaf, &pki).replace('\n', "\r\n");
        let checked = run(&windows, &leaf.key, false, &pki.roots).unwrap();
        assert_eq!(checked.chain, full(&leaf, &pki));
    }

    #[test]
    fn a_chain_without_its_intermediate_is_refused_naming_the_issuer_and_the_file() {
        let pki = pki();
        let leaf = fixtures::server(&pki.middle, &Server::default());
        let why =
            run(&fixtures::chain_file(&[&leaf.der]), &leaf.key, false, &pki.roots).unwrap_err();
        assert!(why.contains("the chain stops at CN=Test Intermediate"), "{why}");
        assert!(why.contains("lacks CN=Test Intermediate's intermediate certificate"), "{why}");
        assert!(why.contains("after the certificate in chain.pem"), "{why}");
        assert!(why.contains("--untrusted"), "{why}");
    }

    #[test]
    fn untrusted_takes_a_chain_the_roots_do_not_lead_to_and_says_so() {
        let pki = pki();
        let leaf = fixtures::server(&pki.middle, &Server::default());
        let checked =
            run(&fixtures::chain_file(&[&leaf.der]), &leaf.key, true, &pki.roots).unwrap();
        assert!(
            checked.notes[0].starts_with("not trusted here: CN=Test Intermediate"),
            "{:?}",
            checked.notes
        );
    }

    #[test]
    fn a_certificate_that_signed_itself_is_added_with_a_warning() {
        let own = fixtures::self_signed(&["nas.example"]);
        let checked =
            run(&fixtures::chain_file(&[&own.der]), &own.key, false, &fixtures::roots(&[]))
                .unwrap();
        assert_eq!(checked.notes, ["self-signed: browsers warn unless told to trust it"]);
    }

    #[test]
    fn a_key_that_is_another_certificates_is_refused() {
        let pki = pki();
        let leaf = fixtures::server(&pki.middle, &Server::default());
        let other = fixtures::server(&pki.middle, &Server::default());
        let why = run(&full(&leaf, &pki), &other.key, false, &pki.roots).unwrap_err();
        assert_eq!(why, "the key is not the certificate's");
    }

    #[test]
    fn a_key_the_system_cannot_sign_with_is_refused() {
        let pki = pki();
        let leaf = fixtures::server(&pki.middle, &Server::default());
        let nonsense = inspect::pem_encode("PRIVATE KEY", &[1, 2, 3]);
        let why = run(&full(&leaf, &pki), &nonsense, false, &pki.roots).unwrap_err();
        assert!(why.contains("not one the system can sign with"), "{why}");
    }

    #[test]
    fn an_encrypted_key_is_refused_with_what_to_do() {
        let pki = pki();
        let leaf = fixtures::server(&pki.middle, &Server::default());
        let encrypted = inspect::pem_encode("ENCRYPTED PRIVATE KEY", b"x");
        let why = run(&full(&leaf, &pki), &encrypted, false, &pki.roots).unwrap_err();
        assert!(why.starts_with("key.pem: the key is encrypted: take its passphrase off"), "{why}");
    }

    #[test]
    fn a_key_file_with_more_than_the_key_is_refused() {
        let pki = pki();
        let leaf = fixtures::server(&pki.middle, &Server::default());
        let both = format!("{}{}", leaf.key, leaf.key);
        let why = run(&full(&leaf, &pki), &both, false, &pki.roots).unwrap_err();
        assert!(why.contains("holds 2 PEM blocks"), "{why}");
        let why = run(&full(&leaf, &pki), "", false, &pki.roots).unwrap_err();
        assert!(why.contains("holds 0 PEM blocks"), "{why}");
    }

    #[test]
    fn an_expired_certificate_is_refused_with_the_day_it_ran_out() {
        let pki = pki();
        let leaf = fixtures::server(
            &pki.middle,
            &Server { not_after: NOW - 2 * DAY, ..Server::default() },
        );
        let why = run(&full(&leaf, &pki), &leaf.key, false, &pki.roots).unwrap_err();
        assert_eq!(why, "the certificate ran out on 2027-01-13: add one that is valid now");
    }

    #[test]
    fn a_certificate_that_starts_later_is_refused_with_the_day_it_starts() {
        let pki = pki();
        let leaf = fixtures::server(
            &pki.middle,
            &Server { not_before: NOW + 3 * DAY, ..Server::default() },
        );
        let why = run(&full(&leaf, &pki), &leaf.key, false, &pki.roots).unwrap_err();
        assert_eq!(why, "the certificate starts on 2027-01-18, which is not yet: add it then");
    }

    #[test]
    fn a_certificate_that_names_no_host_is_refused() {
        let pki = pki();
        let leaf =
            fixtures::server(&pki.middle, &Server { names: Vec::new(), ..Server::default() });
        let why = run(&full(&leaf, &pki), &leaf.key, false, &pki.roots).unwrap_err();
        assert!(why.starts_with("the certificate names no host"), "{why}");
    }

    #[test]
    fn an_authority_in_a_server_s_place_is_refused() {
        let pki = pki();
        let leaf = fixtures::server(&pki.middle, &Server { authority: true, ..Server::default() });
        let why = run(&full(&leaf, &pki), &leaf.key, false, &pki.roots).unwrap_err();
        assert!(why.contains("a certificate authority's, not a server's"), "{why}");
        assert!(why.contains("first in chain.pem"), "{why}");
    }

    #[test]
    fn a_chain_of_eleven_certificates_is_refused_and_ten_pass_this_check() {
        let pki = pki();
        let leaf = fixtures::server(&pki.middle, &Server::default());
        let many = |count: usize| fixtures::chain_file(&vec![&leaf.der; count]);
        let why = run(&many(11), &leaf.key, false, &pki.roots).unwrap_err();
        assert_eq!(why, "chain.pem holds 11 certificates: it holds 1 to 10, the certificate first");
        let why = run("", &leaf.key, false, &pki.roots).unwrap_err();
        assert!(why.contains("holds 0 certificates"), "{why}");
        // Ten pass the count, and fail for the want of the issuer's, not it.
        assert!(!run(&many(10), &leaf.key, false, &pki.roots).unwrap_err().contains("holds"));
    }

    #[test]
    fn junk_after_the_certificates_is_refused() {
        let pki = pki();
        let leaf = fixtures::server(&pki.middle, &Server::default());
        let junk = format!("{}this is not PEM\n", full(&leaf, &pki));
        let why = run(&junk, &leaf.key, false, &pki.roots).unwrap_err();
        assert!(why.starts_with("chain.pem: line "), "{why}");
    }

    #[test]
    fn a_key_in_the_chain_file_is_refused() {
        let pki = pki();
        let leaf = fixtures::server(&pki.middle, &Server::default());
        let why = run(&format!("{}{}", leaf.key, full(&leaf, &pki)), &leaf.key, false, &pki.roots)
            .unwrap_err();
        assert!(why.contains("PRIVATE KEY block"), "{why}");
    }

    fn store(scratch: &Scratch) -> Store {
        Store::at(&scratch.0.join("certificates"), Duration::from_secs(5))
    }

    fn put(store: &Store, name: &str, kind: Kind) {
        let lock = store.lock().unwrap();
        store
            .install(
                &lock,
                name,
                &Files { chain: "chain", key: "key", kind, made: NOW, acme: None },
            )
            .unwrap();
    }

    #[test]
    fn a_name_that_is_no_name_or_the_machines_own_is_refused() {
        let scratch = Scratch::new("add-names");
        let store = store(&scratch);
        for bad in ["", "A", "a b", "a..b", ".x", "é"] {
            assert!(
                check_name(&store, bad, false).unwrap_err().contains("is not a certificate's name"),
                "{bad}"
            );
        }
        let why = check_name(&store, "this-machine", false).unwrap_err();
        assert!(why.contains("the machine's own certificate"), "{why}");
        assert!(check_name(&store, "this-machine", true).is_err());
    }

    #[test]
    fn a_name_in_use_needs_replace_and_replace_needs_a_name_in_use() {
        let scratch = Scratch::new("add-replace");
        let store = store(&scratch);
        assert_eq!(check_name(&store, "example.com", false), Ok(()));
        assert!(
            check_name(&store, "example.com", true).unwrap_err().contains("leave out --replace")
        );
        put(&store, "example.com", Kind::Added);
        assert!(check_name(&store, "example.com", false).unwrap_err().contains("--replace"));
        assert_eq!(check_name(&store, "example.com", true), Ok(()));
    }

    #[test]
    fn only_a_certificate_added_by_hand_is_replaced() {
        let scratch = Scratch::new("add-replace-kind");
        let store = store(&scratch);
        put(&store, "other.example", Kind::SelfSigned);
        let why = check_name(&store, "other.example", true).unwrap_err();
        assert!(why.contains("self-signed certificate, which certd made"), "{why}");
    }

    #[test]
    fn a_file_larger_than_64_kib_is_refused_before_it_is_read_whole() {
        let scratch = Scratch::new("add-large");
        let big = scratch.0.join("big.pem");
        fs::write(&big, vec![b'#'; 65 * 1024]).unwrap();
        let why = read_limited(&big).err().unwrap();
        assert!(why.ends_with("is larger than 64 KiB, which is the most certd keeps"), "{why}");
        let fits = scratch.0.join("fits.pem");
        fs::write(&fits, vec![b' '; 64 * 1024]).unwrap();
        assert!(read_limited(&fits).is_ok());
        assert!(
            read_limited(&scratch.0.join("missing.pem")).err().unwrap().contains("missing.pem")
        );
    }

    #[test]
    fn the_whole_check_reads_the_files_and_the_store() {
        let scratch = Scratch::new("add-whole");
        let pki = pki();
        let leaf = fixtures::server(&pki.middle, &Server::default());
        let (chain, key) = (scratch.0.join("chain.pem"), scratch.0.join("key.pem"));
        fs::write(&chain, full(&leaf, &pki)).unwrap();
        fs::write(&key, &leaf.key).unwrap();
        let store = store(&scratch);
        let request = Request {
            name: "nas.example",
            chain: &chain,
            key: &key,
            replace: false,
            untrusted: false,
        };
        let checked = check(&store, &request, &pki.roots, NOW).unwrap();
        let lock = store.lock().unwrap();
        let files = Files {
            chain: &checked.chain,
            key: &checked.key,
            kind: Kind::Added,
            made: NOW,
            acme: None,
        };
        store.install(&lock, "nas.example", &files).unwrap();
        drop(lock);
        let again = check(&store, &request, &pki.roots, NOW).unwrap_err();
        assert!(again.contains("--replace"), "{again}");
        let replace = Request { replace: true, ..request };
        assert!(check(&store, &replace, &pki.roots, NOW).is_ok());
    }
}

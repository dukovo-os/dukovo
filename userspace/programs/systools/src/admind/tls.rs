// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! admind's certificate and what it is served with (ADR 0514). certd(8)
//! keeps the machine's certificates and Spark copies the one the service
//! file names into the folder `CERTIFICATES_DIRECTORY` names, each in a
//! folder of its name with `fullchain.pem` and `key.pem` (ADR 0512).
//! admind reads the one `admin.toml` picks, and again when Spark sends
//! SIGHUP because it changed, for the connections that come after.
//!
//! The server speaks TLS 1.3 and 1.2, and in 1.2 only the suites that
//! exchange keys with ECDHE and encrypt with AES-GCM or ChaCha20, which
//! are all the suites rustls has: it implements nothing older, and a test
//! here says what each suite is, so that a provider that gains one is
//! noticed. The browser is not asked for a certificate, and there is no
//! early data. A client that offers `h2` is answered `http/1.1`.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rustls::ServerConfig;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};

use crate::cert::inspect;
use crate::{tls, util};

/// The certificate admind serves, and what it says of itself.
pub struct Certificate {
    /// What each connection's handshake is made with.
    pub config: Arc<ServerConfig>,
    /// The leaf's SHA-256, as `cert` prints it and a browser shows it.
    pub fingerprint: String,
    /// The DNS names and addresses the leaf is valid for.
    pub names: Vec<String>,
}

/// The folder Spark keeps admind's copy of certificate `name` in.
///
/// # Errors
///
/// If `CERTIFICATES_DIRECTORY` is not set, which is so when admind is not
/// started by Spark with `certificates` in its service file.
pub fn folder(name: &str) -> Result<PathBuf, String> {
    let directory = std::env::var_os("CERTIFICATES_DIRECTORY").ok_or(
        "CERTIFICATES_DIRECTORY is not set: Spark gives it to a service whose file has \
         `certificates`",
    )?;
    Ok(Path::new(&directory).join(name))
}

/// Reads `fullchain.pem` and `key.pem` in `folder`.
///
/// # Errors
///
/// In plain words: a file that is missing or does not read, a key that
/// is not the certificate's, or one the system cannot sign with.
pub fn load(folder: &Path) -> Result<Certificate, String> {
    let read = |file: &str| {
        let path = folder.join(file);
        fs::read_to_string(&path).map_err(|err| {
            format!("cannot read {}: {}", path.display(), util::strip_os_error(&err))
        })
    };
    let chain = inspect::chain_of(&read("fullchain.pem")?)
        .map_err(|why| format!("{}/fullchain.pem: {why}", folder.display()))?;
    let leaf = chain
        .first()
        .ok_or_else(|| format!("{}/fullchain.pem holds no certificate", folder.display()))?;
    let key = inspect::pem_blocks(&read("key.pem")?)
        .and_then(|blocks| {
            let block = blocks.first().ok_or("it holds no key")?;
            inspect::private_key(block)
        })
        .map_err(|why| format!("{}/key.pem: {why}", folder.display()))?;
    inspect::key_fits(&chain, &key)?;
    let details = inspect::details(leaf)?;
    Ok(Certificate {
        fingerprint: details.fingerprint_colons(),
        names: details.names,
        config: server_config(chain, key)?,
    })
}

/// The TLS settings a connection is accepted with.
fn server_config(
    chain: Vec<CertificateDer<'static>>,
    key: PrivateKeyDer<'static>,
) -> Result<Arc<ServerConfig>, String> {
    let versions = [&rustls::version::TLS13, &rustls::version::TLS12];
    let mut config = ServerConfig::builder_with_provider(tls::provider())
        .with_protocol_versions(&versions)
        .map_err(|err| err.to_string())?
        .with_no_client_auth()
        .with_single_cert(chain, key)
        .map_err(|err| format!("the certificate and key do not go together: {err}"))?;
    // HTTP/2 first, which carries every tab's requests and streams on one
    // connection (ADR 0519); HTTP/1.1 for the Terminal's WebSocket, which a
    // browser opens on a connection of its own, and for any client
    // without HTTP/2.
    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    config.ticketer = rustls::crypto::ring::Ticketer::new()
        .map_err(|err| format!("cannot make the keys of session tickets: {err}"))?;
    Ok(Arc::new(config))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cert::fixtures::{self, DAY, Scratch, Server};
    use rustls::pki_types::ServerName;
    use rustls::{ClientConfig, SupportedCipherSuite};
    use tokio::net::UnixStream;
    use tokio_rustls::{TlsAcceptor, TlsConnector};

    /// A folder holding a certificate for `nas.example` from an authority
    /// of its own, and what trusts that authority.
    fn certificate(what: &str) -> (Scratch, rustls::RootCertStore) {
        let authority = fixtures::root("Test Root");
        // Valid whatever day the tests run: a client checks it against the
        // clock, which the fixtures' own time does not stand in for.
        let now = crate::admind::shared::unix_now();
        let valid =
            Server { not_before: now - DAY, not_after: now + 90 * DAY, ..Server::default() };
        let leaf = fixtures::server(&authority, &valid);
        let scratch = Scratch::new(what);
        fs::write(scratch.0.join("fullchain.pem"), fixtures::chain_file(&[&leaf.der])).unwrap();
        fs::write(scratch.0.join("key.pem"), &leaf.key).unwrap();
        (scratch, fixtures::roots(&[&authority]))
    }

    /// What a client that speaks only `versions` negotiates with
    /// `served`: the protocol and the suite's name, and the protocol
    /// the connection agreed on.
    fn connect(
        served: &Certificate,
        roots: rustls::RootCertStore,
        versions: &[&'static rustls::SupportedProtocolVersion],
        offer: &[&[u8]],
    ) -> (String, String, Option<Vec<u8>>) {
        let mut client = ClientConfig::builder_with_provider(tls::provider())
            .with_protocol_versions(versions)
            .unwrap()
            .with_root_certificates(roots)
            .with_no_client_auth();
        client.alpn_protocols = offer.iter().map(|protocol| protocol.to_vec()).collect();
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        runtime.block_on(async {
            let (ours, theirs) = UnixStream::pair().unwrap();
            let acceptor = TlsAcceptor::from(Arc::clone(&served.config));
            let server = tokio::spawn(async move { acceptor.accept(theirs).await.map(drop) });
            let name = ServerName::try_from("nas.example").unwrap();
            let stream = TlsConnector::from(Arc::new(client)).connect(name, ours).await.unwrap();
            server.await.unwrap().unwrap();
            let (_, connection) = stream.get_ref();
            let version = format!("{:?}", connection.protocol_version().unwrap());
            let suite = format!("{:?}", connection.negotiated_cipher_suite().unwrap().suite());
            (version, suite, connection.alpn_protocol().map(<[u8]>::to_vec))
        })
    }

    #[test]
    fn a_certificate_is_read_with_its_fingerprint_and_names() {
        let (folder, _) = certificate("reads");
        let served = load(&folder.0).unwrap();
        assert_eq!(served.names, ["nas.example"]);
        let chain = fs::read_to_string(folder.0.join("fullchain.pem")).unwrap();
        let der = &inspect::chain_of(&chain).unwrap()[0];
        assert_eq!(served.fingerprint, inspect::details(der).unwrap().fingerprint_colons());
        assert_eq!(served.fingerprint.len(), 32 * 3 - 1);
    }

    #[test]
    fn a_folder_that_does_not_read_is_said_in_words() {
        let (folder, _) = certificate("broken");
        let path = |file: &str| folder.0.join(file);
        let why = |folder: &Path| load(folder).err().unwrap();
        assert!(why(&path("none")).starts_with("cannot read "));
        // Another certificate's key.
        let other = fixtures::server(&fixtures::root("Other"), &Server::default());
        fs::write(path("key.pem"), &other.key).unwrap();
        assert_eq!(why(&folder.0), "the key is not the certificate's");
        fs::write(path("key.pem"), "not a key").unwrap();
        assert!(why(&folder.0).contains("key.pem: line 1 is not PEM"), "{}", why(&folder.0));
        fs::write(path("key.pem"), "").unwrap();
        assert!(why(&folder.0).ends_with("key.pem: it holds no key"), "{}", why(&folder.0));
        fs::write(path("fullchain.pem"), "").unwrap();
        assert!(why(&folder.0).ends_with("holds no certificate"), "{}", why(&folder.0));
    }

    #[test]
    fn tls_1_3_and_1_2_both_work_with_http_2_first_and_http_1_1_for_who_asks_it() {
        let (folder, roots) = certificate("versions");
        let served = load(&folder.0).unwrap();
        let (version, suite, protocol) =
            connect(&served, roots.clone(), &[&rustls::version::TLS13], &[b"http/1.1", b"h2"]);
        assert_eq!(version, "TLSv1_3");
        assert!(suite.starts_with("TLS13_"), "{suite}");
        assert_eq!(protocol.as_deref(), Some(&b"h2"[..]));
        // A browser's WebSocket asks for HTTP/1.1 alone.
        let (_, _, protocol) =
            connect(&served, roots.clone(), &[&rustls::version::TLS13], &[b"http/1.1"]);
        assert_eq!(protocol.as_deref(), Some(&b"http/1.1"[..]));
        // HTTP/2 over TLS 1.2 needs ECDHE and an AEAD (RFC 9113 9.2.2),
        // which every suite of 1.2 here has.
        let (version, suite, protocol) =
            connect(&served, roots.clone(), &[&rustls::version::TLS12], &[b"h2", b"http/1.1"]);
        assert_eq!(version, "TLSv1_2");
        assert!(suite.contains("ECDHE"), "{suite}");
        assert_eq!(protocol.as_deref(), Some(&b"h2"[..]));
        // A client that offers nothing speaks HTTP/1.1, as before.
        let (_, _, protocol) = connect(&served, roots, &[&rustls::version::TLS13], &[]);
        assert_eq!(protocol, None);
    }

    #[test]
    fn the_suites_of_tls_1_2_exchange_with_ecdhe_and_encrypt_with_gcm_or_chacha20() {
        let mut seen = 0;
        for suite in &tls::provider().cipher_suites {
            if let SupportedCipherSuite::Tls12(suite) = suite {
                let name = format!("{:?}", suite.common.suite);
                assert!(name.contains("_ECDHE_"), "{name}");
                assert!(name.contains("_GCM_") || name.contains("CHACHA20"), "{name}");
                seen += 1;
            }
        }
        assert!(seen >= 4, "{seen}");
    }
}

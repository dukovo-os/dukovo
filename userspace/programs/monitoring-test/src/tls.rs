// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The receivers' certificate: tls-test's `rsa-2048`, for tls.test and
//! 127.0.0.1, from its test authority, which the boot test makes the
//! system trust.

use std::sync::Arc;

use rustls::ServerConfig;
use rustls::crypto::ring;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};

/// A file of tls-test's testdata.
macro_rules! testdata {
    ($name:literal) => {
        include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../tls-test/testdata/", $name))
            .as_slice()
    };
}

/// The test authority's certificate, in PEM.
pub fn authority() -> &'static [u8] {
    testdata!("ca.pem")
}

/// What a TLS server needs: the certificate and key.
///
/// # Errors
///
/// Why the certificate or the key could not be used, which means the
/// build is wrong.
pub fn server_config() -> Result<Arc<ServerConfig>, String> {
    let chain = CertificateDer::pem_slice_iter(testdata!("rsa-2048.pem"))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| format!("the receivers' certificate: {err}"))?;
    let key = PrivateKeyDer::from_pem_slice(testdata!("rsa-2048.key"))
        .map_err(|err| format!("the receivers' key: {err}"))?;
    let config = ServerConfig::builder_with_provider(Arc::new(ring::default_provider()))
        .with_safe_default_protocol_versions()
        .map_err(|err| err.to_string())?
        .with_no_client_auth()
        .with_single_cert(chain, key)
        .map_err(|err| err.to_string())?;
    Ok(Arc::new(config))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_certificate_and_key_make_a_server() {
        assert!(server_config().is_ok());
        assert!(String::from_utf8_lossy(authority()).contains("BEGIN CERTIFICATE"));
    }
}

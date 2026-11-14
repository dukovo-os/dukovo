// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What the unit tests of `cert` build on: certificates made with rcgen as
//! the people who add them make theirs (an authority, an intermediate and
//! a server's certificate), a few made once with openssl whose values the
//! tests state, and a scratch folder for the store.

use std::path::PathBuf;

use rcgen::{
    BasicConstraints, CertificateParams, CertifiedIssuer, DistinguishedName, DnType,
    ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair, KeyUsagePurpose, PKCS_ECDSA_P256_SHA256,
};
use rustls::RootCertStore;
use rustls::pki_types::CertificateDer;
use time::OffsetDateTime;

use super::inspect;

/// The time the tests run at: 2027-01-15 08:00 UTC.
pub const NOW: i64 = 1_800_000_000;

/// A day, in seconds.
pub const DAY: i64 = 86_400;

/// A folder of the test's own, empty, that the test removes when it ends.
pub struct Scratch(pub PathBuf);

impl Scratch {
    pub fn new(what: &str) -> Scratch {
        let path = std::env::temp_dir().join(format!("cert-{}-{what}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("a folder in the temporary folder");
        Scratch(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// An authority's certificate and key.
pub type Authority = CertifiedIssuer<'static, KeyPair>;

fn named(common_name: &str) -> DistinguishedName {
    let mut name = DistinguishedName::new();
    name.push(DnType::CommonName, common_name);
    name
}

fn at(seconds: i64) -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(seconds).expect("a time rcgen takes")
}

fn authority_params(common_name: &str) -> CertificateParams {
    let mut params = CertificateParams::default();
    params.distinguished_name = named(common_name);
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    params.not_before = at(NOW - 365 * DAY);
    params.not_after = at(NOW + 3650 * DAY);
    params
}

/// A root authority called `common_name`.
pub fn root(common_name: &str) -> Authority {
    let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).expect("a key");
    CertifiedIssuer::self_signed(authority_params(common_name), key).expect("a root certificate")
}

/// An intermediate authority called `common_name`, which `parent` signs.
pub fn intermediate(parent: &Authority, common_name: &str) -> Authority {
    let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).expect("a key");
    let parent: &Issuer<'_, KeyPair> = parent;
    CertifiedIssuer::signed_by(authority_params(common_name), key, parent)
        .expect("an intermediate certificate")
}

/// What a server's certificate says, for the tests that change one thing.
pub struct Server {
    pub names: Vec<String>,
    pub not_before: i64,
    pub not_after: i64,
    pub authority: bool,
}

impl Default for Server {
    fn default() -> Server {
        Server {
            names: vec![String::from("nas.example")],
            not_before: NOW - DAY,
            not_after: NOW + 90 * DAY,
            authority: false,
        }
    }
}

/// A server's certificate and its key as PKCS#8 PEM.
pub struct Leaf {
    pub der: CertificateDer<'static>,
    pub key: String,
}

/// A certificate as `server` describes it, signed by `issuer`.
pub fn server(issuer: &Authority, server: &Server) -> Leaf {
    let mut params = CertificateParams::new(server.names.clone()).expect("names");
    params.distinguished_name = named("nas.example");
    params.not_before = at(server.not_before);
    params.not_after = at(server.not_after);
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    params.is_ca = if server.authority {
        IsCa::Ca(BasicConstraints::Unconstrained)
    } else {
        IsCa::ExplicitNoCa
    };
    let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).expect("a key");
    let issuer: &Issuer<'_, KeyPair> = issuer;
    let certificate = params.signed_by(&key, issuer).expect("a certificate");
    Leaf {
        der: certificate.der().clone(),
        key: inspect::pem_encode("PRIVATE KEY", &key.serialize_der()),
    }
}

/// A certificate that signed itself, for `names`, as a person makes one
/// who needs none from an authority.
pub fn self_signed(names: &[&str]) -> Leaf {
    let names: Vec<String> = names.iter().map(|name| (*name).to_string()).collect();
    let mut params = CertificateParams::new(names).expect("names");
    params.distinguished_name = named("self.example");
    params.not_before = at(NOW - DAY);
    params.not_after = at(NOW + 90 * DAY);
    params.is_ca = IsCa::ExplicitNoCa;
    let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).expect("a key");
    let certificate = params.self_signed(&key).expect("a certificate");
    Leaf {
        der: certificate.der().clone(),
        key: inspect::pem_encode("PRIVATE KEY", &key.serialize_der()),
    }
}

/// A chain file: `certificates` as PEM, one after the other.
pub fn chain_file(certificates: &[&CertificateDer<'static>]) -> String {
    certificates.iter().map(|der| inspect::pem_encode(inspect::CERTIFICATE, der.as_ref())).collect()
}

/// The roots a system trusts, which are `authorities` alone.
pub fn roots(authorities: &[&Authority]) -> RootCertStore {
    let mut store = RootCertStore::empty();
    for authority in authorities {
        store.add(authority.der().clone()).expect("a root the store takes");
    }
    store
}

/// A certificate made with openssl, and what `openssl x509` says of it.
pub struct Known {
    pub pem: &'static str,
    pub fingerprint: &'static str,
    pub serial: &'static str,
    pub key: &'static str,
}

/// `openssl req -x509 -newkey rsa:2048`, the names rsa.example,
/// www.rsa.example, 192.0.2.7 and 2001:db8::1, valid from 2026-11-13
/// 18:22:33 UTC for 36500 days.
pub const RSA: Known = Known {
    pem: "\
-----BEGIN CERTIFICATE-----\n\
MIIDTzCCAjegAwIBAgIUYVbY6hVN6q4KvBOimHWXHMmhWbIwDQYJKoZIhvcNAQEL\n\
BQAwFjEUMBIGA1UEAwwLcnNhLmV4YW1wbGUwIBcNMjYxMTEzMTgyMjMzWhgPMjEy\n\
NjEwMjAxODIyMzNaMBYxFDASBgNVBAMMC3JzYS5leGFtcGxlMIIBIjANBgkqhkiG\n\
9w0BAQEFAAOCAQ8AMIIBCgKCAQEAzBqWjbYa7XDaRNnGfmg/zFcLOXKt0uzOdBkI\n\
pf4ff+TeFpGTaQiKEKR2x9jMHIG0s1HqLIjICeeiwzwl3MpLnMig7eVN6FmVuVnC\n\
sAnvBTwoKj0cgddukXxFLMa/726/l5wtMg0PwmMkl7KGNJJxV5deh2zIVUHVQEpL\n\
BSCYgh4vQ/YWwnXcOji6Oe/tejdV+/XuGdXIVVeMigmJo0r4/Qu8H+bdcwZsNHWH\n\
IiR3SZNrjdI8dByV8/I5m5JA31YjZLF0N7LR9a86vpvjHG2YjcbEUW7JdNTQ8U6b\n\
gvH2oXMsPuHVJxJeVhknp1PZ56wqiimAkFR76JmpMX+8W9fQswIDAQABo4GSMIGP\n\
MB0GA1UdDgQWBBToGbgpR5qZe//bG8mBWG5soaJhbTAfBgNVHSMEGDAWgBToGbgp\n\
R5qZe//bG8mBWG5soaJhbTA/BgNVHREEODA2ggtyc2EuZXhhbXBsZYIPd3d3LnJz\n\
YS5leGFtcGxlhwTAAAIHhxAgAQ24AAAAAAAAAAAAAAABMAwGA1UdEwEB/wQCMAAw\n\
DQYJKoZIhvcNAQELBQADggEBALEqKn2hCvSPp2doz8UmwOhIqitPZekbMuEtVLR0\n\
s3/T+z6ShbG9JOL6x1YnE652DAteMANhTNEK/d+CrLva6AxKKpm2rt9jozs28xNV\n\
Us+giTfQZ/5udjtwKuASRf9FXrbASH+FRvC3GD09TWc5pxkx8eOT3yErdO6z42pK\n\
fYlGw2yEhzA1rX/Yo2VXhzlwltUUQmJoDnt6U1cCUusCt9t4LMPqWyG5p+a3FNik\n\
ScWyAYhv7ApDw55kRgWD77vgtOD4VDstW/FRogS+vs+hC71kbYi2Z2Kx4BBsXus/\n\
2mkeMKAm4Q/sarC35uxHhNEmmbhMPJMXDfD1jZYnff7ikcw=\n\
-----END CERTIFICATE-----\n\
",
    fingerprint: "56:36:31:B4:14:B3:7B:11:59:10:5B:7B:74:DC:82:8F:61:C5:07:9A:1D:E5:38:59:88:5B:63:F1:22:4F:F7:5E",
    serial: "61:56:D8:EA:15:4D:EA:AE:0A:BC:13:A2:98:75:97:1C:C9:A1:59:B2",
    key: "RSA 2048",
};

/// The same with `-newkey ec -pkeyopt ec_paramgen_curve:P-256`.
pub const ECDSA: Known = Known {
    pem: "\
-----BEGIN CERTIFICATE-----\n\
MIIBvjCCAWWgAwIBAgIUZUTFrcgyVT/vtTSwFdBTnS8BCg4wCgYIKoZIzj0EAwIw\n\
FTETMBEGA1UEAwwKZWMuZXhhbXBsZTAgFw0yNjExMTMxODIyMzNaGA8yMTI2MTAy\n\
MDE4MjIzM1owFTETMBEGA1UEAwwKZWMuZXhhbXBsZTBZMBMGByqGSM49AgEGCCqG\n\
SM49AwEHA0IABKpnVwW3xANtq7vUJ2rr5P0T4+ba1Ka9Oa0wiaQ7TMs32wmp3A+W\n\
FqdIvQC34TYI1mM711mysLCnfhkoGnREDTGjgZAwgY0wHQYDVR0OBBYEFC4JuTij\n\
MU4dizOFEILpm4s3wTzNMB8GA1UdIwQYMBaAFC4JuTijMU4dizOFEILpm4s3wTzN\n\
MD0GA1UdEQQ2MDSCCmVjLmV4YW1wbGWCDnd3dy5lYy5leGFtcGxlhwTAAAIHhxAg\n\
AQ24AAAAAAAAAAAAAAABMAwGA1UdEwEB/wQCMAAwCgYIKoZIzj0EAwIDRwAwRAIg\n\
Y0NSKGsn7nqjtgk/Pn5hZwMUmgYdO6Jm+TgZPo3+/PACIGnaVnN8Uj0egv++Tcos\n\
whSpSSNG/Go1O/dQewl+kKDf\n\
-----END CERTIFICATE-----\n\
",
    fingerprint: "29:90:12:54:FE:8C:CB:1B:C7:24:CA:CE:BA:6D:FD:7C:1D:FD:A9:3B:45:A4:72:A8:B6:EE:5D:9C:0E:4D:9B:E7",
    serial: "65:44:C5:AD:C8:32:55:3F:EF:B5:34:B0:15:D0:53:9D:2F:01:0A:0E",
    key: "ECDSA P-256",
};

/// The same with `-newkey ed25519`.
pub const ED25519: Known = Known {
    pem: "\
-----BEGIN CERTIFICATE-----\n\
MIIBfzCCATGgAwIBAgIUcZiFS7DSk7dhKufZfZT0DGhXEO0wBQYDK2VwMBUxEzAR\n\
BgNVBAMMCmVkLmV4YW1wbGUwIBcNMjYxMTEzMTgyMjMzWhgPMjEyNjEwMjAxODIy\n\
MzNaMBUxEzARBgNVBAMMCmVkLmV4YW1wbGUwKjAFBgMrZXADIQCp8Hr9pxg+lSBS\n\
IriqXGx7w9YWE3ijInOPs9re9SizlaOBkDCBjTAdBgNVHQ4EFgQUuw1Xm/f9rmJp\n\
CL52Ropz94EfdJ0wHwYDVR0jBBgwFoAUuw1Xm/f9rmJpCL52Ropz94EfdJ0wPQYD\n\
VR0RBDYwNIIKZWQuZXhhbXBsZYIOd3d3LmVkLmV4YW1wbGWHBMAAAgeHECABDbgA\n\
AAAAAAAAAAAAAAEwDAYDVR0TAQH/BAIwADAFBgMrZXADQQBNdjU9zHYims4JXGpB\n\
x/ja4bsrrqfLmOCwe0OdrHlPXKVVMI3f3fC4ZkpIMlAx4RBmpyNOMwHv2WhVKVYy\n\
FhMF\n\
-----END CERTIFICATE-----\n\
",
    fingerprint: "E3:EE:C1:27:D6:98:35:DA:70:81:15:22:5D:5C:6A:10:78:32:23:C1:6E:27:B4:42:2A:B0:92:11:9D:02:98:24",
    serial: "71:98:85:4B:B0:D2:93:B7:61:2A:E7:D9:7D:94:F4:0C:68:57:10:ED",
    key: "Ed25519",
};

/// When the three above start and end, in seconds since 1970.
pub const KNOWN_START: i64 = 1_794_594_153;
pub const KNOWN_END: i64 = 4_948_194_153;

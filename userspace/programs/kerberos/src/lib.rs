// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Kerberos 5 for Dukovo (ADR 0397): a client of a realm's KDC, whether
//! MIT's, Heimdal's, Samba's or Active Directory's, for kinit(1),
//! klist(1) and kdestroy(1), and for the network file systems' `sec=krb5`.
//!
//! How it fits together, from the bottom:
//!
//! - [`der`] writes and reads the DER that every message is in;
//! - [`crypto`] holds the AES encryption types and their keys;
//! - [`messages`] holds RFC 4120's messages;
//! - [`config`] reads krb5.conf and [`kdc`] reaches the KDCs it names;
//! - [`client`] runs the AS and TGS exchanges, which make credentials;
//! - [`creds`] keeps credentials in the person's keyring, and [`keytab`]
//!   reads a service's keys.
//!
//! Debugging starts with the error, which says what the KDC refused or
//! what did not read; then with the KDC's own log, which records each
//! request.

pub mod client;
pub mod config;
pub mod creds;
pub mod crypto;
pub mod der;
pub mod kdc;
pub mod keytab;
pub mod messages;
pub mod time;

/// What went wrong.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// A message that is not what Kerberos sends: what did not read.
    Malformed(String),
    /// A ciphertext whose integrity check failed: the wrong key, or a
    /// message changed on the way.
    Integrity,
    /// The KDC refused: its error code, and what it means.
    Kdc {
        /// RFC 4120's code.
        code: i32,
        /// What it means, and what to do.
        text: String,
    },
    /// Something else: what failed, and what to do.
    Other(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Malformed(why) => write!(f, "a Kerberos message did not read: {why}"),
            Error::Integrity => write!(
                f,
                "a message did not pass its integrity check: the wrong key, or it was changed on the way"
            ),
            Error::Kdc { text, .. } | Error::Other(text) => f.write_str(text),
        }
    }
}

impl std::error::Error for Error {}

/// Kerberos's results.
pub type Result<T> = std::result::Result<T, Error>;

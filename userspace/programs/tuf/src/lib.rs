// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The Update Framework's client for Dukovo (ADR 0380): what a catalog,
//! an update feed or a bundle's own feed says is checked against the
//! keys its root trusts before anything is installed from it.
//!
//! It follows TUF's specification (version 1.0) and its client workflow
//! as the reference client, python-tuf's `ngclient`, runs it, and the
//! TUF conformance suite checks it against both (`ob test tuf`). It
//! reads metadata in canonical JSON, checks signatures with ring, and
//! fetches over http, https or file URLs.

pub mod fetch;
pub mod http;
pub mod json;
pub mod keys;
pub mod metadata;
pub mod time;
pub mod trusted;
pub mod updater;
pub mod url;

pub use updater::{Error, Limits, Updater};
pub use url::Url;

/// Makes `dir` trust `root`, a root's metadata that came by another way
/// than the repository: with the system, or checked by its person. It is
/// checked when an [`Updater`] starts from it.
///
/// # Errors
///
/// If `dir` cannot be made or written.
pub fn init(dir: &std::path::Path, root: &[u8]) -> Result<(), Error> {
    std::fs::create_dir_all(dir).map_err(|err| Error::Io(dir.to_path_buf(), err))?;
    let partial = dir.join(".root.json.partial");
    std::fs::write(&partial, root).map_err(|err| Error::Io(partial.clone(), err))?;
    std::fs::rename(&partial, dir.join("root.json")).map_err(|err| Error::Io(partial, err))
}

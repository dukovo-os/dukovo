// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The shares NFS serves: those of shares.toml(5) that name networks
//! (ADR 0341), read again whenever the file changes.

use std::fs;
use std::net::IpAddr;
use std::path::PathBuf;
use std::time::SystemTime;

use manifest::shares::{self, Share};

/// A share NFS serves.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NfsShare {
    /// Its name, which a client mounts it by: `computer:/name`.
    pub name: String,
    /// Its folder, as shares.toml names it.
    pub folder: PathBuf,
    /// Whether nothing in it can be changed through it.
    pub read_only: bool,
    /// The networks it may be mounted from.
    share: Share,
    /// The number its filehandles carry: a hash of its name, in lower
    /// case, which is the same after a restart.
    pub key: u64,
}

impl NfsShare {
    /// Whether a client at `address` may reach it.
    pub fn allows(&self, address: IpAddr) -> bool {
        self.share.nfs_allows(address)
    }
}

/// A share's key, from its name.
pub use manifest::shares::nfs_key as key_of;

/// The shares of `text`, shares.toml's, that name networks.
pub fn of_toml(text: &str) -> Result<Vec<NfsShare>, String> {
    let all = shares::from_toml(text).map_err(|err| err.to_string())?;
    Ok(all
        .shares
        .into_iter()
        .filter(|share| !share.nfs.is_empty())
        .map(|share| NfsShare {
            name: share.name.clone(),
            folder: PathBuf::from(&share.folder),
            read_only: share.read_only,
            key: key_of(&share.name),
            share,
        })
        .collect())
}

/// The shares NFS serves now: none if shares.toml is missing or broken,
/// which is said once each time it is read.
pub fn load() -> Vec<NfsShare> {
    match fs::read_to_string(shares::PATH) {
        Ok(text) => of_toml(&text).unwrap_or_else(|err| {
            eprintln!("nfsd: {}: {err}; no shares until it is mended", shares::PATH);
            Vec::new()
        }),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(err) => {
            eprintln!("nfsd: cannot read {}: {err}", shares::PATH);
            Vec::new()
        }
    }
}

/// When shares.toml last changed, to read it again when it does.
pub fn changed() -> Option<SystemTime> {
    fs::metadata(shares::PATH).and_then(|meta| meta.modified()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_shares_with_networks_are_served() {
        let text = "[[share]]\nname = \"Music\"\nfolder = \"/m\"\nnfs = [\"10.0.2.0/24\"]\n\
                    [[share]]\nname = \"Private\"\nfolder = \"/p\"\n";
        let served = of_toml(text).unwrap();
        assert_eq!(served.len(), 1);
        assert_eq!(served[0].name, "Music");
        assert_eq!(served[0].key, key_of("MUSIC"));
        assert!(served[0].allows("10.0.2.2".parse().unwrap()));
        assert!(!served[0].allows("10.0.3.2".parse().unwrap()));
    }
}

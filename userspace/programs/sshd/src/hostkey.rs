// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The host's key, which tells clients this machine is the one they
//! signed in to before: Ed25519, made from the kernel's random bytes the
//! first time sshd starts, and kept in OpenSSH's format, root's alone,
//! with its public half beside it for anyone to read, as Linux's sshd
//! keeps `/etc/ssh/ssh_host_ed25519_key`.

use std::fs::{self, DirBuilder, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};

use russh::keys::PrivateKey;
use russh::keys::ssh_key::private::{Ed25519Keypair, KeypairData};
use russh::keys::ssh_key::{HashAlg, LineEnding};

use crate::sys;

/// The folder sshd keeps it in.
const FOLDER: &str = "/system/state/sshd";

/// Where the key is.
pub const KEY: &str = "/system/state/sshd/ssh_host_ed25519_key";

/// Where its public half is.
pub const PUBLIC: &str = "/system/state/sshd/ssh_host_ed25519_key.pub";

/// The key, read from [`KEY`].
pub fn load() -> Result<PrivateKey, String> {
    let text = fs::read(KEY).map_err(|err| format!("cannot read the host key {KEY}: {err}"))?;
    let key = PrivateKey::from_openssh(&text)
        .map_err(|err| format!("the host key {KEY} is not one: {err}"))?;
    if key.is_encrypted() {
        return Err(format!("the host key {KEY} is encrypted"));
    }
    Ok(key)
}

/// The key, made first if there is none, and whether it was made.
pub fn load_or_make() -> Result<(PrivateKey, bool), String> {
    match fs::symlink_metadata(KEY) {
        Ok(_) => {
            let key = load()?;
            // The public half, if it went, is written again.
            let gone =
                fs::symlink_metadata(PUBLIC).is_err_and(|e| e.kind() == io::ErrorKind::NotFound);
            if gone {
                write(PUBLIC, public(&key)?.as_bytes(), 0o644)?;
            }
            return Ok((key, false));
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => {}
        Err(err) => return Err(format!("cannot read the host key {KEY}: {err}")),
    }
    DirBuilder::new()
        .recursive(true)
        .mode(0o755)
        .create(FOLDER)
        .map_err(|err| format!("cannot make {FOLDER}: {err}"))?;
    let seed = sys::random::<32>().map_err(|err| format!("cannot read random bytes: {err}"))?;
    let pair = KeypairData::Ed25519(Ed25519Keypair::from_seed(&seed));
    let comment = format!("root@{}", sys::hostname());
    let key = PrivateKey::new(pair, comment.as_str())
        .map_err(|err| format!("cannot make the host key: {err}"))?;
    let private = key
        .to_openssh(LineEnding::LF)
        .map_err(|err| format!("cannot write the host key: {err}"))?;
    write(PUBLIC, public(&key)?.as_bytes(), 0o644)?;
    write(KEY, private.as_bytes(), 0o600)?;
    Ok((key, true))
}

/// The key's public half, as a line of OpenSSH's.
fn public(key: &PrivateKey) -> Result<String, String> {
    let public = key
        .public_key()
        .to_openssh()
        .map_err(|err| format!("cannot write the host key's public half: {err}"))?;
    Ok(format!("{public}\n"))
}

/// The key's fingerprint, as `ssh-keygen -l` shows it.
pub fn fingerprint(key: &PrivateKey) -> String {
    key.public_key().fingerprint(HashAlg::Sha256).to_string()
}

/// Writes `bytes` to `path`, mode `mode`, by way of a new file beside it
/// that takes its place once whole.
fn write(path: &str, bytes: &[u8], mode: u32) -> Result<(), String> {
    let failed = |err: io::Error| format!("cannot write {path}: {err}");
    let new = format!("{path}.new");
    let _ = fs::remove_file(&new);
    let mut file =
        OpenOptions::new().write(true).create_new(true).mode(mode).open(&new).map_err(failed)?;
    file.write_all(bytes).and_then(|()| file.sync_all()).map_err(failed)?;
    fs::set_permissions(&new, fs::Permissions::from_mode(mode)).map_err(failed)?;
    fs::rename(&new, path).map_err(failed)
}

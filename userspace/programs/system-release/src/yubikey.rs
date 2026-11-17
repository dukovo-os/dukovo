// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Signing with a YubiKey's PIV key, through `tools/yubikey-piv.py`
//! (ADRs 0381 and 0538): the catalog's keys are in each YubiKey's slot
//! 9c, the system releases' in slot 95, so that a signature made for one
//! repository is never one the other's keys made. The helper asks for the
//! PIN and the touch itself; the secret half never leaves the YubiKey.
//! When signing fails, run the helper by hand: `tools/yubikey-piv.py list`
//! names the YubiKeys it sees, and `public SERIAL SLOT` the key a slot
//! holds.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use tuf::keys::Key;
use tuf::repo::{self, Signer};

/// The PIV slot of the catalog's keys, Digital Signature.
pub const CATALOG_SLOT: &str = "9c";

/// The PIV slot of the system releases' keys, the last of the twenty
/// retired key management slots: age-plugin-yubikey takes the first free
/// one from 82 upward, so 95 stays clear of it.
pub const RELEASE_SLOT: &str = "95";

/// A key held by a YubiKey, in one of its PIV slots, which signs through
/// the helper.
pub struct YubiKey {
    helper: PathBuf,
    serial: u64,
    slot: &'static str,
    key: Key,
}

impl YubiKey {
    /// The key `key` of YubiKey `serial`'s `slot`, signing through
    /// `helper`.
    #[must_use]
    pub fn new(helper: &Path, serial: u64, slot: &'static str, key: Key) -> YubiKey {
        YubiKey { helper: helper.to_path_buf(), serial, slot, key }
    }
}

impl Signer for YubiKey {
    fn key(&self) -> &Key {
        &self.key
    }

    fn sign(&self, message: &[u8]) -> Result<Vec<u8>, String> {
        let hex = run_helper(&self.helper, &helper_args("sign", self.serial, self.slot), message)?;
        let hex = hex.trim();
        if hex.len() % 2 != 0 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(format!("YubiKey {}'s helper printed no signature", self.serial));
        }
        (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).map_err(|e| e.to_string()))
            .collect()
    }
}

/// The helper's words for `command` on YubiKey `serial`'s `slot`. The
/// catalog's slot is the helper's default and is left out, so that a
/// helper of before slots were named still serves the catalog.
fn helper_args(command: &str, serial: u64, slot: &str) -> Vec<String> {
    let mut args = vec![command.to_string(), serial.to_string()];
    if slot != CATALOG_SLOT {
        args.push(slot.to_string());
    }
    args
}

/// Runs the YubiKey helper with `args`, `input` on its standard input,
/// and returns what it printed. Its prompts go to the terminal.
fn run_helper<S: AsRef<str>>(path: &Path, args: &[S], input: &[u8]) -> Result<String, String> {
    let words: Vec<&str> = args.iter().map(AsRef::as_ref).collect();
    let mut child = Command::new(path)
        .args(&words)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| format!("{}: {e}", path.display()))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(input).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    let out = child.wait_with_output().map_err(|e| format!("{}: {e}", path.display()))?;
    if !out.status.success() {
        return Err(format!("{} {} failed", path.display(), words.join(" ")));
    }
    String::from_utf8(out.stdout)
        .map_err(|_| format!("{} printed what is not text", path.display()))
}

/// The serial numbers of the YubiKeys plugged in.
///
/// # Errors
///
/// If the helper fails or prints something else.
pub fn plugged(helper: &Path) -> Result<Vec<u64>, String> {
    let text = run_helper::<&str>(helper, &["list"], &[])?;
    text.lines()
        .map(|line| {
            line.trim().parse().map_err(|_| format!("{} listed {line:?}", helper.display()))
        })
        .collect()
}

/// The YubiKey `serial`, or the only one plugged in.
///
/// # Errors
///
/// If there is no YubiKey, or more than one and no serial.
pub fn one(helper: &Path, serial: Option<u64>) -> Result<u64, String> {
    if let Some(serial) = serial {
        return Ok(serial);
    }
    match plugged(helper)?.as_slice() {
        [one] => Ok(*one),
        [] => Err("no YubiKey is plugged in".into()),
        _ => Err("more than one YubiKey is plugged in: name one with --serial".into()),
    }
}

/// The public half of the P-256 key in YubiKey `serial`'s `slot`.
///
/// # Errors
///
/// If the helper fails: no such YubiKey, or the slot holds no P-256 key
/// made on the YubiKey.
pub fn public_key(helper: &Path, serial: u64, slot: &str) -> Result<Key, String> {
    let pem = run_helper(helper, &helper_args("public", serial, slot), &[])?;
    repo::p256_key(&pem).map_err(|e| format!("YubiKey {serial}, slot {slot}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_catalogs_slot_is_left_to_the_helpers_default() {
        assert_eq!(helper_args("sign", 7, CATALOG_SLOT), ["sign", "7"]);
        assert_eq!(helper_args("public", 7, RELEASE_SLOT), ["public", "7", "95"]);
    }
}

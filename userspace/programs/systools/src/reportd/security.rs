// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The security box (ADR 0531): a report of a security problem is
//! encrypted with age to the security team's X25519 keys before it is
//! queued, so that the queue, the server and anyone between hold only
//! what the team alone can read. The keys are a file of recipients, one
//! `age1…` a line, `#` starting a comment: the system's own, or the one
//! report.toml names.

use std::io::Write;
use std::str::FromStr;

/// Where to send a security problem when the box has no keys.
pub const ADDRESS: &str = "security@dukovo.org";

/// The recipients in `text`.
///
/// # Errors
///
/// A line that is not a recipient, or none at all.
pub fn recipients(text: &str) -> Result<Vec<age::x25519::Recipient>, String> {
    let mut keys = Vec::new();
    for (number, line) in text.lines().enumerate() {
        let line = line.split('#').next().unwrap_or_default().trim();
        if line.is_empty() {
            continue;
        }
        let key = age::x25519::Recipient::from_str(line)
            .map_err(|_| format!("line {} is not an age recipient", number + 1))?;
        keys.push(key);
    }
    if keys.is_empty() {
        return Err(format!("it holds no key; write to {ADDRESS} instead"));
    }
    Ok(keys)
}

/// The recipients in the file `path`.
///
/// # Errors
///
/// The file cannot be read, or holds no recipient.
pub fn recipients_in(path: &str) -> Result<Vec<age::x25519::Recipient>, String> {
    let text = std::fs::read_to_string(path).map_err(|err| {
        format!(
            "the security box has no keys ({path}: {}); write to {ADDRESS} instead",
            crate::util::strip_os_error(&err)
        )
    })?;
    recipients(&text).map_err(|why| format!("{path}: {why}"))
}

/// `bytes` encrypted to `keys`, in age's binary format.
///
/// # Errors
///
/// If age fails, which it does not for keys it parsed.
pub fn seal(bytes: &[u8], keys: &[age::x25519::Recipient]) -> Result<Vec<u8>, String> {
    let encryptor = age::Encryptor::with_recipients(keys.iter().map(|k| k as &dyn age::Recipient))
        .map_err(|err| format!("encrypting: {err}"))?;
    let mut sealed = Vec::with_capacity(bytes.len() + 256);
    let mut writer =
        encryptor.wrap_output(&mut sealed).map_err(|err| format!("encrypting: {err}"))?;
    writer.write_all(bytes).map_err(|err| format!("encrypting: {err}"))?;
    writer.finish().map_err(|err| format!("encrypting: {err}"))?;
    Ok(sealed)
}

#[cfg(test)]
mod tests {
    use std::io::Read;

    use age::secrecy::ExposeSecret;

    use super::*;

    #[test]
    fn a_sealed_report_opens_with_the_teams_key_alone() {
        let team = age::x25519::Identity::generate();
        let other = age::x25519::Identity::generate();
        let text = format!("# the security team\n{}  # 2026 key\n\n", team.to_public());
        let keys = recipients(&text).unwrap();
        let sealed = seal(b"Dukovo problem report\nthe secret part\n", &keys).unwrap();
        assert!(sealed.starts_with(b"age-encryption.org/v1\n"));
        assert!(!sealed.windows(6).any(|w| w == b"secret"));
        let open = |identity: &age::x25519::Identity| -> Option<Vec<u8>> {
            let decryptor = age::Decryptor::new(&sealed[..]).ok()?;
            let mut reader =
                decryptor.decrypt(std::iter::once(identity as &dyn age::Identity)).ok()?;
            let mut out = Vec::new();
            reader.read_to_end(&mut out).ok()?;
            Some(out)
        };
        assert_eq!(open(&team).unwrap(), b"Dukovo problem report\nthe secret part\n");
        assert!(open(&other).is_none());
        // The identity's secret is never what is written.
        assert!(!text.contains(team.to_string().expose_secret()));
    }

    #[test]
    fn a_file_without_a_key_names_the_address() {
        assert!(recipients("# none yet\n").unwrap_err().contains(ADDRESS));
        assert!(recipients("age1nope\n").unwrap_err().contains("line 1"));
    }
}

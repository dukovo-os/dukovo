// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What authd(8) checks before it signs someone in over SSH (ADR 0347):
//! that a public key is in the person's `~/.ssh/authorized_keys`, and
//! that the client signed its sign-in request with it (RFC 4252 7).
//!
//! sshd(8) checks the signature too, as its SSH library does, but authd
//! does not take sshd's word for it: sshd reads what the network sends
//! before anyone has signed in, and a flaw there must not let it start a
//! session as anyone whose public key is known. What was signed holds
//! the session's identifier, which sshd and the client made together in
//! their key exchange, so a signature is good for that one connection.
//!
//! The keys are those the C library's OpenSSH client makes and the
//! SSH library sshd is built on checks, and that ring verifies: Ed25519,
//! and ECDSA on P-256 and P-384. A line with options (`command=`,
//! `from=`, `restrict` and the rest) is passed over, with a word in the
//! log, rather than taken without them: none is carried out yet.

use std::ffi::CString;
use std::fs::File;
use std::io::{self, Read};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use ring::signature::{self, UnparsedPublicKey};

use accounts::User;

/// The key algorithms whose signatures authd checks.
pub const ALGORITHMS: &[&str] = accounts::request::SSH_KEY_ALGORITHMS;

/// The longest `authorized_keys` read, in bytes.
pub const FILE_MAX: u64 = 1 << 20;

/// The message a sign-in request is (RFC 4252 5).
const USERAUTH_REQUEST: u8 = 50;

/// The longest session identifier: SHA-512's.
const SESSION_ID_MAX: usize = 64;

/// SSH's encoding (RFC 4251 5), read.
struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn byte(&mut self) -> Option<u8> {
        let (&first, rest) = self.0.split_first()?;
        self.0 = rest;
        Some(first)
    }

    fn string(&mut self) -> Option<&'a [u8]> {
        let length = u32::from_be_bytes(self.0.get(..4)?.try_into().ok()?) as usize;
        let string = self.0.get(4..4usize.checked_add(length)?)?;
        self.0 = &self.0[4 + length..];
        Some(string)
    }

    fn text(&mut self) -> Option<&'a str> {
        std::str::from_utf8(self.string()?).ok()
    }

    fn end(&self) -> Option<()> {
        self.0.is_empty().then_some(())
    }
}

/// The algorithm's name a key, as SSH carries it, starts with.
pub fn algorithm_of(key: &[u8]) -> Option<&str> {
    Reader(key).text()
}

/// The fingerprint OpenSSH shows for a key: `SHA256:` and the base64,
/// without padding, of its SHA-256.
pub fn fingerprint(key: &[u8]) -> String {
    let digest = ring::digest::digest(&ring::digest::SHA256, key);
    let text = STANDARD.encode(digest.as_ref());
    format!("SHA256:{}", text.trim_end_matches('='))
}

/// A sign-in request with a public key, as its client signed it.
#[derive(Debug, PartialEq, Eq)]
pub struct Signed<'a> {
    /// The session's identifier.
    pub session: &'a [u8],
    /// The account it signs in to.
    pub user: &'a str,
    /// The key, as SSH carries it.
    pub key: &'a [u8],
}

/// Reads what a client signed to sign in with a key (RFC 4252 7): the
/// session's identifier, then a `publickey` request for SSH's
/// connection service with its signature, and nothing more.
pub fn read_signed(signed: &[u8]) -> Result<Signed<'_>, &'static str> {
    let bad = "the signed sign-in request is not one";
    let mut r = Reader(signed);
    let session = r.string().filter(|s| (1..=SESSION_ID_MAX).contains(&s.len())).ok_or(bad)?;
    if r.byte() != Some(USERAUTH_REQUEST) {
        return Err(bad);
    }
    let user = r.text().ok_or(bad)?;
    if r.string() != Some(b"ssh-connection") || r.string() != Some(b"publickey") {
        return Err(bad);
    }
    // TRUE: the request carries a signature.
    if r.byte().is_none_or(|b| b == 0) {
        return Err(bad);
    }
    let algorithm = r.string().ok_or(bad)?;
    let key = r.string().ok_or(bad)?;
    r.end().ok_or(bad)?;
    // The key's own algorithm is the request's, for the keys checked.
    if algorithm_of(key).map(str::as_bytes) != Some(algorithm) {
        return Err("the sign-in request names another algorithm than its key's");
    }
    Ok(Signed { session, user, key })
}

/// Checks that `signature`, as SSH carries it, is the key's in `signed`
/// over all of `signed`, which [`read_signed`] reads. The request, if
/// it is.
pub fn verify<'a>(signed: &'a [u8], signature: &[u8]) -> Result<Signed<'a>, &'static str> {
    let request = read_signed(signed)?;
    let wrong = "the signature is not the key's";
    let mut key = Reader(request.key);
    let algorithm = key.text().ok_or(wrong)?;
    let mut sig = Reader(signature);
    if sig.text() != Some(algorithm) {
        return Err("the signature is of another algorithm than the key's");
    }
    let blob = sig.string().ok_or(wrong)?;
    sig.end().ok_or(wrong)?;
    let checked = match algorithm {
        "ssh-ed25519" => {
            let point = key.string().filter(|p| p.len() == 32).ok_or(wrong)?;
            key.end().ok_or(wrong)?;
            if blob.len() != 64 {
                return Err(wrong);
            }
            UnparsedPublicKey::new(&signature::ED25519, point).verify(signed, blob)
        }
        "ecdsa-sha2-nistp256" | "ecdsa-sha2-nistp384" => {
            let (curve, size, check): (
                &[u8],
                usize,
                &'static signature::EcdsaVerificationAlgorithm,
            ) = if algorithm == "ecdsa-sha2-nistp256" {
                (b"nistp256", 32, &signature::ECDSA_P256_SHA256_FIXED)
            } else {
                (b"nistp384", 48, &signature::ECDSA_P384_SHA384_FIXED)
            };
            if key.string() != Some(curve) {
                return Err(wrong);
            }
            let point = key.string().ok_or(wrong)?;
            key.end().ok_or(wrong)?;
            // r and s, each an mpint (RFC 5656 3.1.2), made the fixed
            // width ring takes.
            let mut numbers = Reader(blob);
            let mut fixed = Vec::with_capacity(2 * size);
            for _ in 0..2 {
                let number = mpint(numbers.string().ok_or(wrong)?, size).ok_or(wrong)?;
                fixed.extend_from_slice(&number);
            }
            numbers.end().ok_or(wrong)?;
            UnparsedPublicKey::new(check, point).verify(signed, &fixed)
        }
        _ => return Err("authd does not check signatures of this kind of key"),
    };
    checked.map_err(|_| wrong)?;
    Ok(request)
}

/// A positive mpint (RFC 4251 5) as a number `size` bytes wide, big
/// end first, if it is one and fits.
fn mpint(bytes: &[u8], size: usize) -> Option<Vec<u8>> {
    // Positive: no sign bit, and a leading zero only before one.
    if bytes.first().is_some_and(|&b| b & 0x80 != 0) {
        return None;
    }
    if bytes.len() > 1 && bytes[0] == 0 && bytes[1] & 0x80 == 0 {
        return None;
    }
    let digits = bytes.strip_prefix(&[0]).unwrap_or(bytes);
    if digits.len() > size {
        return None;
    }
    let mut fixed = vec![0; size - digits.len()];
    fixed.extend_from_slice(digits);
    Some(fixed)
}

/// Whether an `authorized_keys` file's `text` names `key`. Each line is
/// a key's algorithm, its base64 and a comment; blank lines and lines
/// starting with `#` are none. A line with options before its key is
/// passed over, and `skipped` called with its number.
pub fn names(text: &str, key: &[u8], mut skipped: impl FnMut(usize)) -> bool {
    let Some(algorithm) = algorithm_of(key) else { return false };
    let mut found = false;
    for (number, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut words = line.split_ascii_whitespace();
        let Some(first) = words.next() else { continue };
        if !is_key_type(first) {
            skipped(number + 1);
            continue;
        }
        let Some(decoded) = words.next().and_then(|text| STANDARD.decode(text).ok()) else {
            continue;
        };
        // A line's key is of the kind it says.
        if first == algorithm && algorithm_of(&decoded) == Some(first) && decoded == key {
            found = true;
        }
    }
    found
}

/// Whether `word` is a key's algorithm, as an `authorized_keys` line
/// starts with, rather than options.
fn is_key_type(word: &str) -> bool {
    word.starts_with("ssh-")
        || word.starts_with("ecdsa-sha2-")
        || word.starts_with("sk-")
        || word.starts_with("rsa-sha2-")
}

/// Opens `name` in the folder `at`, or the path `name` itself without
/// `at`, without following a link at its last step.
fn open_at(at: Option<&OwnedFd>, name: &Path, directory: bool) -> io::Result<OwnedFd> {
    let path = CString::new(name.as_os_str().as_bytes())
        .map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
    let mut flags = libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK;
    if directory {
        flags |= libc::O_DIRECTORY;
    }
    let dir = at.map_or(libc::AT_FDCWD, AsRawFd::as_raw_fd);
    // SAFETY: openat reads the NUL-ended path, which lives until it
    // returns.
    let fd = unsafe { libc::openat(dir, path.as_ptr(), flags) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: openat made the descriptor, which is ours alone.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

/// Checks that what `fd` is open on is `user`'s or root's, and that no
/// one else may change it, as OpenSSH's StrictModes checks.
fn safe(fd: &OwnedFd, user: &User, what: &str) -> Result<std::fs::Metadata, String> {
    let file = File::from(fd.try_clone().map_err(|err| format!("{what}: {err}"))?);
    let meta = file.metadata().map_err(|err| format!("{what}: {err}"))?;
    if meta.uid() != user.uid && meta.uid() != 0 {
        return Err(format!("{what} is user {}'s, not {}'s", meta.uid(), user.name));
    }
    if meta.mode() & 0o022 != 0 {
        return Err(format!("{what} may be changed by others (mode {:o})", meta.mode() & 0o7777));
    }
    Ok(meta)
}

/// The text of `user`'s `~/.ssh/authorized_keys`, if the file and the
/// folders it is in are safe to take keys from (each theirs or root's,
/// and no one else's to change), reached without following a link from
/// the home on. `None` when there is no such file: no key signs in.
pub fn read_authorized(user: &User) -> Result<Option<String>, String> {
    let absent = |err: &io::Error| err.kind() == io::ErrorKind::NotFound;
    let home = Path::new(&user.home);
    let home = match open_at(None, home, true) {
        Ok(fd) => fd,
        Err(err) if absent(&err) => return Ok(None),
        Err(err) => return Err(format!("{}: {err}", user.home)),
    };
    safe(&home, user, &user.home)?;
    let folder = match open_at(Some(&home), Path::new(".ssh"), true) {
        Ok(fd) => fd,
        Err(err) if absent(&err) => return Ok(None),
        Err(err) => return Err(format!("{}/.ssh: {err}", user.home)),
    };
    let shown = format!("{}/.ssh/authorized_keys", user.home.trim_end_matches('/'));
    safe(&folder, user, &format!("{}/.ssh", user.home.trim_end_matches('/')))?;
    let fd = match open_at(Some(&folder), Path::new("authorized_keys"), false) {
        Ok(fd) => fd,
        Err(err) if absent(&err) => return Ok(None),
        Err(err) => return Err(format!("{shown}: {err}")),
    };
    let meta = safe(&fd, user, &shown)?;
    if !meta.is_file() {
        return Err(format!("{shown} is not a file"));
    }
    let mut text = String::new();
    File::from(fd)
        .take(FILE_MAX)
        .read_to_string(&mut text)
        .map_err(|err| format!("{shown}: {err}"))?;
    Ok(Some(text))
}

#[cfg(test)]
mod tests {
    use super::*;

    use ring::rand::SystemRandom;
    use ring::signature::{EcdsaKeyPair, Ed25519KeyPair, KeyPair};

    fn string(out: &mut Vec<u8>, bytes: &[u8]) {
        out.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
        out.extend_from_slice(bytes);
    }

    /// What a client signs to sign `user` in with `key` in session
    /// `session`.
    fn request(session: &[u8], user: &str, algorithm: &str, key: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        string(&mut out, session);
        out.push(USERAUTH_REQUEST);
        string(&mut out, user.as_bytes());
        string(&mut out, b"ssh-connection");
        string(&mut out, b"publickey");
        out.push(1);
        string(&mut out, algorithm.as_bytes());
        string(&mut out, key);
        out
    }

    fn ed25519(seed: u8) -> (Ed25519KeyPair, Vec<u8>) {
        let pair = Ed25519KeyPair::from_seed_unchecked(&[seed; 32]).unwrap();
        let mut key = Vec::new();
        string(&mut key, b"ssh-ed25519");
        string(&mut key, pair.public_key().as_ref());
        (pair, key)
    }

    fn ed25519_signature(pair: &Ed25519KeyPair, signed: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        string(&mut out, b"ssh-ed25519");
        string(&mut out, pair.sign(signed).as_ref());
        out
    }

    /// r or s as an mpint: its leading zeros gone, and a zero before a
    /// top bit.
    fn as_mpint(fixed: &[u8]) -> Vec<u8> {
        let start = fixed.iter().position(|&b| b != 0).unwrap_or(fixed.len());
        let mut digits = fixed[start..].to_vec();
        if digits.first().is_some_and(|&b| b & 0x80 != 0) {
            digits.insert(0, 0);
        }
        digits
    }

    #[test]
    fn ed25519_signatures_are_checked() {
        let (pair, key) = ed25519(7);
        let signed = request(&[9; 32], "alice", "ssh-ed25519", &key);
        let signature = ed25519_signature(&pair, &signed);
        let request = verify(&signed, &signature).unwrap();
        assert_eq!(request, Signed { session: &[9; 32], user: "alice", key: &key });

        // Another key's signature, a changed request or session, and a
        // signature cut short are refused.
        let (other, _) = ed25519(8);
        assert!(verify(&signed, &ed25519_signature(&other, &signed)).is_err());
        let mut changed = signed.clone();
        changed[10] ^= 1;
        assert!(verify(&changed, &signature).is_err());
        let bob = request_for(&key, "bob");
        assert!(verify(&bob, &signature).is_err());
        assert!(verify(&signed, &signature[..signature.len() - 1]).is_err());
        assert!(verify(&signed, &[]).is_err());
    }

    fn request_for(key: &[u8], user: &str) -> Vec<u8> {
        request(&[9; 32], user, "ssh-ed25519", key)
    }

    #[test]
    fn ecdsa_signatures_are_checked() {
        let random = SystemRandom::new();
        for (name, curve, signing, size) in [
            (
                "ecdsa-sha2-nistp256",
                &b"nistp256"[..],
                &signature::ECDSA_P256_SHA256_FIXED_SIGNING,
                32,
            ),
            ("ecdsa-sha2-nistp384", b"nistp384", &signature::ECDSA_P384_SHA384_FIXED_SIGNING, 48),
        ] {
            let pkcs8 = EcdsaKeyPair::generate_pkcs8(signing, &random).unwrap();
            let pair = EcdsaKeyPair::from_pkcs8(signing, pkcs8.as_ref(), &random).unwrap();
            let mut key = Vec::new();
            string(&mut key, name.as_bytes());
            string(&mut key, curve);
            string(&mut key, pair.public_key().as_ref());
            let signed = request(&[3; 48], "alice", name, &key);
            // Many signatures, so that some r or s has its top bit set,
            // or a leading zero byte.
            for _ in 0..16 {
                let fixed = pair.sign(&random, &signed).unwrap();
                let (r, s) = fixed.as_ref().split_at(size);
                let mut numbers = Vec::new();
                string(&mut numbers, &as_mpint(r));
                string(&mut numbers, &as_mpint(s));
                let mut signature = Vec::new();
                string(&mut signature, name.as_bytes());
                string(&mut signature, &numbers);
                assert!(verify(&signed, &signature).is_ok(), "{name}");
                let mut wrong = signature.clone();
                let last = wrong.len() - 1;
                wrong[last] ^= 1;
                assert!(verify(&signed, &wrong).is_err(), "{name}");
            }
        }
    }

    #[test]
    fn requests_are_read_strictly() {
        let (_, key) = ed25519(1);
        let good = request(&[1; 32], "alice", "ssh-ed25519", &key);
        assert!(read_signed(&good).is_ok());
        // Another algorithm than the key's, another service, no session,
        // a request with no signature, and bytes after it.
        assert!(read_signed(&request(&[1; 32], "alice", "ecdsa-sha2-nistp256", &key)).is_err());
        let mut other = Vec::new();
        string(&mut other, &[1; 32]);
        other.push(USERAUTH_REQUEST);
        string(&mut other, b"alice");
        string(&mut other, b"ssh-userauth");
        assert!(read_signed(&other).is_err());
        assert!(read_signed(&request(&[], "alice", "ssh-ed25519", &key)).is_err());
        assert!(read_signed(&request(&[1; 65], "alice", "ssh-ed25519", &key)).is_err());
        let mut probe = good.clone();
        let at = 4 + 32 + 1 + 4 + 5 + 4 + 14 + 4 + 9;
        probe[at] = 0;
        assert!(read_signed(&probe).is_err());
        let mut longer = good;
        longer.push(0);
        assert!(read_signed(&longer).is_err());
        assert!(read_signed(&[0, 0, 0, 200]).is_err());
    }

    #[test]
    fn mpints_take_the_fixed_width() {
        assert_eq!(mpint(&[1, 2], 4), Some(vec![0, 0, 1, 2]));
        assert_eq!(mpint(&[0, 0x80], 2), Some(vec![0, 0x80]));
        assert_eq!(mpint(&[], 2), Some(vec![0, 0]));
        // Negative, a needless zero, and too wide.
        assert_eq!(mpint(&[0x80], 2), None);
        assert_eq!(mpint(&[0, 1], 2), None);
        assert_eq!(mpint(&[1, 2, 3], 2), None);
    }

    #[test]
    fn authorized_keys_lines_are_read() {
        let (_, key) = ed25519(5);
        let (_, other) = ed25519(6);
        let text = STANDARD.encode(&key);
        let file = format!(
            "# alice's keys\n\n  ssh-ed25519 {text} alice@laptop\n\
             command=\"/bin/date\" ssh-ed25519 {} restricted\n",
            STANDARD.encode(&other)
        );
        let mut skipped = Vec::new();
        assert!(names(&file, &key, |n| skipped.push(n)));
        assert_eq!(skipped, [4]);
        // The key with options is not taken without them.
        assert!(!names(&file, &other, |_| {}));
        // A line whose type is not its key's, a key cut short, and no key.
        assert!(!names(&format!("ecdsa-sha2-nistp256 {text}\n"), &key, |_| {}));
        let cut = STANDARD.encode(&key[..key.len() - 1]);
        assert!(!names(&format!("ssh-ed25519 {cut}\n"), &key, |_| {}));
        assert!(!names("ssh-ed25519\n", &key, |_| {}));
        assert!(!names("", &key, |_| {}));
    }

    #[test]
    fn fingerprints_are_openssh_s() {
        // An Ed25519 key of the points 0 to 31, and its fingerprint as
        // `ssh-keygen -l` shows it, worked out apart from this code.
        let line = "AAAAC3NzaC1lZDI1NTE5AAAAIAABAgMEBQYHCAkKCwwNDg8QERITFBUWFxgZGhscHR4f";
        let key = STANDARD.decode(line).unwrap();
        assert_eq!(fingerprint(&key), "SHA256:ZkAslGjFiUHdGf/WUL8rQvkib4PTvQatUV0OUQSncCA");
    }

    #[test]
    fn unsafe_files_give_no_keys() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("sshkeys-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let ssh = dir.join(".ssh");
        std::fs::create_dir_all(&ssh).unwrap();
        // SAFETY: getuid reads this process's user and cannot fail.
        let me = unsafe { libc::getuid() };
        let user = User {
            name: "me".into(),
            uid: me,
            gid: me,
            full_name: String::new(),
            home: dir.display().to_string(),
            shell: String::new(),
        };
        let mode = |path: &Path, mode: u32| {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
        };
        mode(&dir, 0o700);
        mode(&ssh, 0o700);
        assert_eq!(read_authorized(&user), Ok(None));
        let file = ssh.join("authorized_keys");
        std::fs::write(&file, "ssh-ed25519 AAAA\n").unwrap();
        mode(&file, 0o600);
        assert_eq!(read_authorized(&user), Ok(Some("ssh-ed25519 AAAA\n".into())));
        // Others may write the file, or the folder; the file is a link.
        mode(&file, 0o622);
        assert!(read_authorized(&user).unwrap_err().contains("changed by others"));
        mode(&file, 0o600);
        mode(&ssh, 0o777);
        assert!(read_authorized(&user).is_err());
        mode(&ssh, 0o700);
        std::fs::rename(&file, ssh.join("real")).unwrap();
        std::os::unix::fs::symlink("real", &file).unwrap();
        assert!(read_authorized(&user).is_err());
        // Another's home.
        let theirs = User { uid: me + 1, ..user.clone() };
        if me != 0 {
            assert!(read_authorized(&theirs).unwrap_err().contains("not me's"));
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The install images' checksums, signed on a YubiKey (ADR 0779): a
//! `SHA256SUMS` file as `sha256sum` writes and checks it, and its
//! signature in DER by the release key of a YubiKey the newest root names
//! for targets, which `openssl dgst -sha256 -verify` checks with that
//! key's PEM in `releases/keys/`. Anyone can check a download with these
//! two common tools before writing it to a stick, without Dukovo.
//!
//! A release key's signature of a `SHA256SUMS` cannot pass for one of TUF
//! metadata: TUF's are of canonical JSON, which starts with `{`, and every
//! line here starts with a hex digit.

use std::io::Read as _;
use std::path::{Path, PathBuf};

use crate::list::hex;
use crate::repo::Keys;
use crate::yubikey::YubiKey;

/// The checksums' file name.
pub const SUMS: &str = "SHA256SUMS";

/// The name of the signature of [`SUMS`] by YubiKey `serial`'s release
/// key, so that whoever checks it knows which key's PEM to check it with.
#[must_use]
pub fn signature_name(serial: u64) -> String {
    format!("{SUMS}.yubikey-{serial}.sig")
}

/// One image the checksums name.
#[derive(Debug, PartialEq, Eq)]
pub struct Image {
    /// Its file name.
    pub name: String,
    /// Its size in bytes.
    pub bytes: u64,
    /// Its SHA-256, in hex.
    pub sha256: String,
}

/// The checksums of the files `images`, by name, and the text of
/// [`SUMS`]: a line for each, its SHA-256 in hex, two spaces and its name.
///
/// # Errors
///
/// If a file cannot be read, two have the same name, or a name is not
/// plain: `sha256sum` writes a name with a backslash or a new line in
/// another form, and one with a space or a slash is not an image's.
pub fn sums(images: &[PathBuf]) -> Result<(Vec<Image>, Vec<u8>), String> {
    let mut named = Vec::new();
    for path in images {
        let name = path.file_name().and_then(|name| name.to_str()).unwrap_or_default();
        let plain = !name.is_empty()
            && !name.starts_with('.')
            && name.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-+".contains(&b));
        if !plain {
            return Err(format!("{}: not an image's plain file name", path.display()));
        }
        let mut file =
            std::fs::File::open(path).map_err(|err| format!("{}: {err}", path.display()))?;
        let mut context = ring::digest::Context::new(&ring::digest::SHA256);
        let mut buffer = vec![0; 1 << 20];
        let mut bytes = 0_u64;
        loop {
            let read =
                file.read(&mut buffer).map_err(|err| format!("{}: {err}", path.display()))?;
            if read == 0 {
                break;
            }
            context.update(&buffer[..read]);
            bytes += read as u64;
        }
        named.push(Image { name: name.to_string(), bytes, sha256: hex(context.finish().as_ref()) });
    }
    named.sort_by(|a, b| a.name.cmp(&b.name));
    if let Some(pair) = named.windows(2).find(|pair| pair[0].name == pair[1].name) {
        return Err(format!("two images are called {}", pair[0].name));
    }
    let mut text = String::new();
    for image in &named {
        text.push_str(&image.sha256);
        text.push_str("  ");
        text.push_str(&image.name);
        text.push('\n');
    }
    Ok((named, text.into_bytes()))
}

/// Signs `sums` on `yubikey` with its release key, which the root `root`
/// must name for targets, once `confirm` has passed, and checks the
/// signature it gives, in DER. No touch is asked for a YubiKey the root
/// does not name.
///
/// # Errors
///
/// If the root does not name the key, `confirm` refuses, the YubiKey does
/// not sign, or its signature does not check.
pub fn sign(
    sums: &[u8],
    root: &[u8],
    yubikey: &YubiKey,
    confirm: impl FnOnce() -> Result<(), String>,
) -> Result<Vec<u8>, String> {
    let keys = Keys::hardware(vec![root.to_vec()], yubikey, &["targets"])?;
    let signer = keys.signer("targets")?;
    confirm()?;
    let signature = signer.sign(sums)?;
    if !signer.key().verifies(sums, &hex(&signature)) {
        return Err(format!("YubiKey {}'s signature does not check", yubikey.serial()));
    }
    Ok(signature)
}

/// Writes [`SUMS`] and its signature by YubiKey `serial` into `out`, and
/// gives their paths.
///
/// # Errors
///
/// If one cannot be written.
pub fn write(
    out: &Path,
    sums: &[u8],
    signature: &[u8],
    serial: u64,
) -> Result<[PathBuf; 2], String> {
    std::fs::create_dir_all(out).map_err(|err| format!("{}: {err}", out.display()))?;
    let paths = [out.join(SUMS), out.join(signature_name(serial))];
    for (path, bytes) in paths.iter().zip([sums, signature]) {
        std::fs::write(path, bytes).map_err(|err| format!("{}: {err}", path.display()))?;
    }
    Ok(paths)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::list::tests::scratch;
    use crate::updates::tests::ceremony;
    use std::process::Command;
    use tuf::repo::Signer;

    /// Two images, small, and their checksums signed on YubiKey 1001.
    #[test]
    fn the_checksums_are_signed_on_a_release_key_and_check_with_common_tools() {
        let dir = scratch("sums");
        let (helper, root, _) = ceremony(&dir);
        let images = dir.join("images");
        std::fs::create_dir_all(&images).unwrap();
        let iso = images.join("dukovo-1.0.0-alpha.1-x86_64.iso");
        let stick = images.join("dukovo-1.0.0-alpha.1-x86_64.img");
        std::fs::write(&iso, b"an ISO").unwrap();
        std::fs::write(&stick, vec![7; 3 << 20]).unwrap();
        let (named, text) = sums(&[iso.clone(), stick.clone()]).unwrap();
        assert_eq!(
            named.iter().map(|i| i.name.as_str()).collect::<Vec<_>>(),
            ["dukovo-1.0.0-alpha.1-x86_64.img", "dukovo-1.0.0-alpha.1-x86_64.iso"]
        );
        assert_eq!(named[0].bytes, 3 << 20);
        let yubikey = crate::yubikey::release_key(&helper, Some(1001)).unwrap();
        let signature = sign(&text, &root, &yubikey, || Ok(())).unwrap();
        let [sums_file, sig_file] = write(&images, &text, &signature, 1001).unwrap();
        assert_eq!(sig_file.file_name().unwrap(), "SHA256SUMS.yubikey-1001.sig");
        let pem = dir.join("releases/keys/yubikey-1001.pem");
        let run = |program: &str, args: &[&std::ffi::OsStr]| {
            let out = Command::new(program).args(args).current_dir(&images).output().unwrap();
            assert!(out.status.success(), "{program}: {}", String::from_utf8_lossy(&out.stderr));
        };
        run("sha256sum", &["-c".as_ref(), sums_file.as_os_str()]);
        let verify = |key: &Path| {
            Command::new("openssl")
                .args(["dgst", "-sha256", "-verify"])
                .arg(key)
                .arg("-signature")
                .arg(&sig_file)
                .arg(&sums_file)
                .output()
                .unwrap()
                .status
                .success()
        };
        assert!(verify(&pem));
        // The other YubiKey's key did not sign it.
        assert!(!verify(&dir.join("releases/keys/yubikey-1002.pem")));
        let entry = crate::rekor::entry_for(&text, &signature, Signer::key(&yubikey)).unwrap();
        assert!(String::from_utf8(entry).unwrap().contains("hashedrekord"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn nothing_is_signed_by_a_key_the_root_does_not_name_or_unconfirmed() {
        let dir = scratch("sums-refused");
        let (helper, root, _) = ceremony(&dir);
        let stranger = Signer::key(&crate::ceremony::tests::P256::new()).clone();
        let stranger = YubiKey::new(&helper, 1003, "95", stranger);
        let asked = || -> Result<(), String> { panic!("nothing is to be confirmed") };
        let error = sign(b"sums", &root, &stranger, asked).unwrap_err();
        assert!(error.contains("does not name YubiKey 1003's key"), "{error}");
        let yubikey = crate::yubikey::release_key(&helper, Some(1002)).unwrap();
        let refused = sign(b"sums", &root, &yubikey, || Err("not typed".into()));
        assert_eq!(refused.unwrap_err(), "not typed");
        for name in ["with space.iso", ".hidden.iso", "back\\slash.iso"] {
            let path = dir.join(name);
            std::fs::write(&path, b"x").unwrap();
            assert!(sums(&[path]).unwrap_err().contains("plain file name"), "{name}");
        }
        let one = dir.join("one.iso");
        std::fs::write(&one, b"x").unwrap();
        assert!(sums(&[one.clone(), one]).unwrap_err().contains("two images are called"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

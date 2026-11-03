// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The catalog's keys: the public half of each in `keys/`, and what signs
//! for it, a YubiKey through `tools/yubikey-piv.py` or a secret key file.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{Value, json};
use tuf::keys::Key;
use tuf::repo::{self, Signer};

/// A key of the catalog, as its file in `keys/` describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyFile {
    /// Its name, which is its file's.
    pub name: String,
    /// The roles it signs for: `root`, `targets`, `snapshot`, `timestamp`.
    pub roles: Vec<String>,
    /// The serial number of the YubiKey that holds it, if one does.
    pub yubikey: Option<u64>,
    /// The key.
    pub key: Key,
}

impl KeyFile {
    /// Whether it signs for `role`.
    #[must_use]
    pub fn signs_for(&self, role: &str) -> bool {
        self.roles.iter().any(|r| r == role)
    }

    fn to_json(&self) -> Value {
        let mut value = json!({ "name": self.name, "roles": self.roles, "key": self.key });
        if let Some(serial) = self.yubikey {
            value["yubikey"] = serial.into();
        }
        value
    }

    fn from_json(value: &Value) -> Result<KeyFile, String> {
        let name = value["name"].as_str().ok_or("no name")?.to_string();
        let roles = value["roles"]
            .as_array()
            .ok_or("no roles")?
            .iter()
            .map(|r| r.as_str().map(str::to_string).ok_or("a role is not a name"))
            .collect::<Result<_, _>>()?;
        let key = serde_json::from_value(value["key"].clone()).map_err(|e| e.to_string())?;
        Ok(KeyFile { name, roles, yubikey: value["yubikey"].as_u64(), key })
    }

    /// Writes it to `dir`, as `NAME.json`.
    ///
    /// # Errors
    ///
    /// If a key of that name is there already, or it cannot be written.
    pub fn write(&self, dir: &Path) -> Result<PathBuf, String> {
        check_name(&self.name)?;
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let path = dir.join(format!("{}.json", self.name));
        if path.exists() {
            return Err(format!("{} is there already", path.display()));
        }
        let mut text = serde_json::to_string_pretty(&self.to_json()).map_err(|e| e.to_string())?;
        text.push('\n');
        std::fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(path)
    }
}

/// Refuses a name that is not one file's: letters, digits, `-`, `_`, `.`.
fn check_name(name: &str) -> Result<(), String> {
    let ok = !name.is_empty()
        && !name.starts_with('.')
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b));
    if ok { Ok(()) } else { Err(format!("{name:?} is not a key's name")) }
}

/// Every key in `dir`, by name.
///
/// # Errors
///
/// If one cannot be read.
pub fn read_all(dir: &Path) -> Result<Vec<KeyFile>, String> {
    let mut keys = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else { return Ok(keys) };
    for entry in entries {
        let path = entry.map_err(|e| e.to_string())?.path();
        if path.extension().is_none_or(|e| e != "json") {
            continue;
        }
        let text =
            std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let value: Value =
            serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        keys.push(KeyFile::from_json(&value).map_err(|e| format!("{}: {e}", path.display()))?);
    }
    keys.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(keys)
}

/// A key held by a YubiKey, which signs through the helper.
pub struct YubiKey {
    helper: PathBuf,
    serial: u64,
    key: Key,
}

impl Signer for YubiKey {
    fn key(&self) -> &Key {
        &self.key
    }

    fn sign(&self, message: &[u8]) -> Result<Vec<u8>, String> {
        let hex = run_helper(&self.helper, &["sign", &self.serial.to_string()], message)?;
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

/// Runs the YubiKey helper with `args`, `input` on its standard input,
/// and returns what it printed. Its prompts go to the terminal.
fn run_helper(path: &Path, args: &[&str], input: &[u8]) -> Result<String, String> {
    let mut child = Command::new(path)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| format!("{}: {e}", path.display()))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(input).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    let out = child.wait_with_output().map_err(|e| format!("{}: {e}", path.display()))?;
    if !out.status.success() {
        return Err(format!("{} {} failed", path.display(), args.join(" ")));
    }
    String::from_utf8(out.stdout)
        .map_err(|_| format!("{} printed what is not text", path.display()))
}

/// What can sign: the YubiKeys plugged in, through `helper`, and the
/// secret key files given.
#[derive(Default)]
pub struct Signers {
    helper: Option<PathBuf>,
    plugged: Vec<u64>,
    files: Vec<repo::Ed25519>,
}

impl Signers {
    /// The YubiKeys `helper` finds plugged in, if it is given, and the
    /// keys in `files`.
    ///
    /// # Errors
    ///
    /// If the helper fails, or a file is not a secret key.
    pub fn new(helper: Option<&Path>, files: &[PathBuf]) -> Result<Signers, String> {
        let plugged = match helper {
            Some(path) => helper_list(path)?,
            None => Vec::new(),
        };
        let mut keys = Vec::new();
        for file in files {
            let der = std::fs::read(file).map_err(|e| format!("{}: {e}", file.display()))?;
            keys.push(
                repo::Ed25519::from_pkcs8(&der).map_err(|e| format!("{}: {e}", file.display()))?,
            );
        }
        Ok(Signers { helper: helper.map(Path::to_path_buf), plugged, files: keys })
    }

    /// What signs for `key`, if anything here can.
    #[must_use]
    pub fn for_key(&self, key: &KeyFile) -> Option<Box<dyn Signer + '_>> {
        if let (Some(serial), Some(helper)) = (key.yubikey, &self.helper)
            && self.plugged.contains(&serial)
        {
            return Some(Box::new(YubiKey {
                helper: helper.clone(),
                serial,
                key: key.key.clone(),
            }));
        }
        let file = self.files.iter().find(|f| f.key() == &key.key)?;
        Some(Box::new(FileSigner(file)))
    }
}

struct FileSigner<'a>(&'a repo::Ed25519);

impl Signer for FileSigner<'_> {
    fn key(&self) -> &Key {
        self.0.key()
    }

    fn sign(&self, message: &[u8]) -> Result<Vec<u8>, String> {
        self.0.sign(message)
    }
}

fn helper_list(path: &Path) -> Result<Vec<u64>, String> {
    let text = run_helper(path, &["list"], &[])?;
    text.lines()
        .map(|line| line.trim().parse().map_err(|_| format!("{} listed {line:?}", path.display())))
        .collect()
}

/// The key of the YubiKey `serial`, or of the only one plugged in.
///
/// # Errors
///
/// If there is no such YubiKey, more than one and no serial, or its slot
/// holds no P-256 key.
pub fn from_yubikey(helper: &Path, serial: Option<u64>) -> Result<KeyFile, String> {
    let serial = match serial {
        Some(serial) => serial,
        None => match helper_list(helper)?.as_slice() {
            [one] => *one,
            [] => return Err("no YubiKey is plugged in".into()),
            _ => return Err("more than one YubiKey is plugged in: name one with --serial".into()),
        },
    };
    let pem = run_helper(helper, &["public", &serial.to_string()], &[])?;
    let key = repo::p256_key(&pem).map_err(|e| format!("YubiKey {serial}: {e}"))?;
    Ok(KeyFile {
        name: format!("yubikey-{serial}"),
        roles: vec!["root".into(), "targets".into()],
        yubikey: Some(serial),
        key,
    })
}

/// A new Ed25519 key, its secret half written to `secret`, readable by
/// its owner alone.
///
/// # Errors
///
/// If `secret` is there already or cannot be written.
pub fn new_file_key(name: &str, roles: Vec<String>, secret: &Path) -> Result<KeyFile, String> {
    use std::os::unix::fs::OpenOptionsExt as _;
    check_name(name)?;
    let (key, der) = repo::Ed25519::generate()?;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(secret)
        .map_err(|e| format!("{}: {e}", secret.display()))?;
    file.write_all(&der).map_err(|e| format!("{}: {e}", secret.display()))?;
    Ok(KeyFile { name: name.into(), roles, yubikey: None, key: key.key().clone() })
}

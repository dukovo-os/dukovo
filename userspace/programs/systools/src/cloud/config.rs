// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What cloud-setup(8) reads (ADR 0328): the cloud's meta-data, which
//! names the machine and holds the SSH keys its owner chose, and the
//! user data its owner wrote, in the part of cloud-init's `#cloud-config`
//! format that cloud-config(5) lists, or a script.
//!
//! Reading changes nothing: [`Config`] says what is to be done, and what
//! of the file is left out and why, for the log; `cloud` does it.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;

use super::yaml::{self, Value};

/// The name of the account a cloud machine makes for its owner when the
/// user data names none, as Ubuntu's images make `ubuntu`: an
/// administrator who elevates without a password, signing in with the
/// cloud's SSH keys.
pub const DEFAULT_ACCOUNT: &str = "dukovo";

/// The groups that make an account an administrator, as cloud-init's
/// files name them for Linux distributions.
const ADMIN_GROUPS: [&str; 4] = ["sudo", "wheel", "admin", "admins"];

/// What the cloud says of the machine: its instance, its name and its
/// owner's SSH keys.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MetaData {
    /// The instance's ID: the setup is done once for each.
    pub instance: String,
    /// The name the cloud gives the machine, perhaps with its domain.
    pub hostname: Option<String>,
    /// The SSH keys of the machine's owner.
    pub keys: Vec<String>,
}

impl MetaData {
    /// Reads NoCloud's `meta-data` file: YAML, or JSON, which it reads too.
    pub fn nocloud(text: &str) -> Result<MetaData, String> {
        let value = yaml::parse(text).map_err(|err| format!("meta-data: {err}"))?;
        let instance = value
            .get("instance-id")
            .and_then(Value::text)
            .ok_or("meta-data has no instance-id")?
            .to_string();
        let hostname = ["local-hostname", "hostname"]
            .iter()
            .find_map(|key| value.get(key).and_then(Value::text))
            .map(str::to_string);
        Ok(MetaData { instance, hostname, keys: public_keys(value.get("public-keys")) })
    }

    /// Reads DigitalOcean's `/metadata/v1.json`: the meta-data, and the
    /// user data if the droplet has any.
    pub fn digitalocean(json: &[u8]) -> Result<(MetaData, Option<Vec<u8>>), String> {
        let value: serde_json::Value = serde_json::from_slice(json)
            .map_err(|err| format!("DigitalOcean's metadata: {err}"))?;
        let instance = match value.get("droplet_id") {
            Some(serde_json::Value::Number(id)) => id.to_string(),
            Some(serde_json::Value::String(id)) => id.clone(),
            _ => return Err("DigitalOcean's metadata has no droplet_id".into()),
        };
        let hostname = value.get("hostname").and_then(|h| h.as_str()).map(str::to_string);
        let keys = value
            .get("public_keys")
            .and_then(|keys| keys.as_array())
            .map_or_else(Vec::new, |keys| {
                keys.iter().filter_map(|k| k.as_str()).flat_map(key_lines).collect()
            });
        let user_data =
            value.get("user_data").and_then(|u| u.as_str()).map(|u| u.as_bytes().to_vec());
        Ok((MetaData { instance, hostname, keys }, user_data))
    }
}

/// The SSH keys of a `public-keys` value: a list, a mapping of names to
/// keys or lists of them, or one key, as NoCloud and OpenStack write it.
fn public_keys(value: Option<&Value>) -> Vec<String> {
    let mut keys = Vec::new();
    let mut add = |value: &Value| {
        for text in value.texts() {
            keys.extend(key_lines(text));
        }
    };
    match value {
        Some(Value::Map(entries)) => entries.iter().for_each(|(_, value)| add(value)),
        Some(value) => add(value),
        None => {}
    }
    keys
}

/// The keys of `text`, one a line, without blank lines and comments.
pub fn key_lines(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_string)
        .collect()
}

/// What the user data is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UserData {
    /// None, or only blank lines.
    Empty,
    /// `#cloud-config`, read.
    Config(Config),
    /// A script, `#!` and its interpreter first, run once.
    Script(Vec<u8>),
    /// A kind cloud-setup does not read, and what it is.
    Other(&'static str),
}

/// Reads user data, by its first line, as cloud-init tells its kinds
/// apart.
pub fn user_data(bytes: &[u8]) -> Result<UserData, String> {
    if bytes.iter().all(u8::is_ascii_whitespace) {
        return Ok(UserData::Empty);
    }
    if bytes.starts_with(&[0x1f, 0x8b]) {
        return Ok(UserData::Other("compressed with gzip"));
    }
    if bytes.starts_with(b"#!") {
        return Ok(UserData::Script(bytes.to_vec()));
    }
    let text = std::str::from_utf8(bytes).map_err(|_| "the user data is not UTF-8".to_string())?;
    let first = text.lines().next().unwrap_or("").trim_end();
    if first == "#cloud-config" {
        let value = yaml::parse(text).map_err(|err| format!("user data: {err}"))?;
        return Config::read(&value).map(UserData::Config);
    }
    Ok(UserData::Other(match first {
        f if f.starts_with("#include") => "an #include of other files",
        f if f.starts_with("#cloud-boothook") => "a boot hook",
        f if f.starts_with("## template: jinja") => "a Jinja template",
        f if f.to_ascii_lowercase().starts_with("content-type: multipart") => "a MIME multipart",
        _ => "neither #cloud-config nor a script",
    }))
}

/// One entry of the user data's `users`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UserEntry {
    /// `default`: the [`DEFAULT_ACCOUNT`], with the cloud's keys.
    Default,
    /// An account named in full.
    Named(Account),
}

/// An account to make, or to add keys to if it is there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Account {
    /// Its name.
    pub name: String,
    /// Its full name, which may be empty: cloud-init's `gecos`.
    pub full_name: String,
    /// Whether it is an administrator's.
    pub admin: bool,
    /// Whether it elevates without a password: a `sudo` rule with
    /// `NOPASSWD`.
    pub without_password: bool,
    /// Its SSH keys.
    pub keys: Vec<String>,
    /// Its password, when the user data gives one and does not lock it;
    /// without one, the account has a password no one knows.
    pub password: Option<String>,
}

/// A file to write, from `write_files`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WriteFile {
    /// Its absolute path.
    pub path: String,
    /// What to write.
    pub content: Vec<u8>,
    /// Its mode.
    pub mode: u32,
    /// Its owner, `user` or `user:group`; root's if none.
    pub owner: Option<String>,
    /// Whether to add to its end rather than replace it.
    pub append: bool,
}

/// A command of `runcmd`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    /// A line for the shell.
    Shell(String),
    /// A program and its arguments.
    Words(Vec<String>),
}

/// `#cloud-config` user data, as cloud-setup reads it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Config {
    /// `hostname`.
    pub hostname: Option<String>,
    /// `fqdn`.
    pub fqdn: Option<String>,
    /// `preserve_hostname`: leave the machine's name as it is.
    pub preserve_hostname: bool,
    /// `users`; the default account alone if the data has no `users`.
    pub users: Vec<UserEntry>,
    /// `user`: the default account's name instead of
    /// [`DEFAULT_ACCOUNT`], as Proxmox's cloud-init drive gives it.
    pub default_name: Option<String>,
    /// `ssh_authorized_keys`: more keys of the default account.
    pub keys: Vec<String>,
    /// `password`: the default account's password.
    pub password: Option<String>,
    /// `write_files`.
    pub files: Vec<WriteFile>,
    /// `runcmd`.
    pub commands: Vec<Command>,
    /// Whether the root's partition grows to the disk's end (`growpart`).
    pub grow_partition: bool,
    /// Whether the root's Btrfs grows to its partition's end
    /// (`resize_rootfs`).
    pub grow_filesystem: bool,
    /// What of the data is left out, and why, for the log.
    pub left_out: Vec<String>,
}

/// Top-level keys left out without a word: they say how cloud-init itself
/// runs, or what Dukovo does anyway.
const QUIET: [&str; 6] = [
    "chpasswd",
    "disable_root",
    "final_message",
    "manage_etc_hosts",
    "prefer_fqdn_over_hostname",
    "datasource",
];

impl Config {
    /// Reads `#cloud-config` user data, `value` the file read.
    pub fn read(value: &Value) -> Result<Config, String> {
        let entries = match value {
            Value::Map(entries) => entries.as_slice(),
            Value::Null => &[],
            _ => return Err("user data: #cloud-config holds a mapping of keys".into()),
        };
        let mut config =
            Config { grow_partition: true, grow_filesystem: true, ..Config::default() };
        let mut users = None;
        for (key, value) in entries {
            if *value == Value::Null {
                continue;
            }
            match key.as_str() {
                "hostname" => config.hostname = value.text().map(str::to_string),
                "fqdn" => config.fqdn = value.text().map(str::to_string),
                "preserve_hostname" => config.preserve_hostname = value.truth() == Some(true),
                "users" => users = Some(read_users(value, &mut config.left_out)?),
                "user" => config.default_name = read_default_name(value, &mut config.left_out)?,
                "ssh_authorized_keys" => {
                    config.keys = value.texts().into_iter().flat_map(key_lines).collect();
                }
                // Proxmox gives the password as a crypt(3) hash, as
                // cloud-init's chpasswd takes one.
                "password" => match value.text() {
                    Some(hash) if is_crypt_hash(hash) => config.left_out.push(
                        "password: a crypt(3) hash, which Dukovo's Argon2id passwords cannot take; \
                         set one with passwd(1)"
                            .into(),
                    ),
                    text => config.password = text.map(str::to_string),
                },
                "write_files" => {
                    for (i, item) in value.items().into_iter().enumerate() {
                        config.files.push(read_file(item, i)?);
                    }
                }
                "runcmd" => config.commands = read_commands(value)?,
                "growpart" => {
                    let mode = value.get("mode");
                    let off = mode.is_some_and(|m| m.truth() == Some(false) || m.text() == Some("off"));
                    config.grow_partition = !off;
                }
                "resize_rootfs" => config.grow_filesystem = value.truth() != Some(false),
                "ssh_pwauth" if value.truth() == Some(true) => config.left_out.push(
                    "ssh_pwauth: sshd takes passwords only as sshd_config(5) says; turn them on there"
                        .into(),
                ),
                "ssh_pwauth" => {}
                "packages" | "package_update" | "package_upgrade" | "apt" | "yum_repos" => {
                    config.left_out.push(format!("{key}: Dukovo installs apps with bundle(1)"));
                }
                key if QUIET.contains(&key) => {}
                key => config.left_out.push(format!("{key}: cloud-setup does not read it")),
            }
        }
        config.users = users.unwrap_or_else(|| vec![UserEntry::Default]);
        Ok(config)
    }

    /// The accounts to make, the default one with `meta`'s keys and the
    /// data's own.
    pub fn accounts(&self, meta: &MetaData) -> Vec<Account> {
        self.users
            .iter()
            .map(|entry| match entry {
                UserEntry::Default => Account {
                    name: self.default_name.clone().unwrap_or_else(|| DEFAULT_ACCOUNT.into()),
                    full_name: String::new(),
                    admin: true,
                    without_password: true,
                    keys: unique(meta.keys.iter().chain(&self.keys)),
                    password: self.password.clone(),
                },
                UserEntry::Named(account) => account.clone(),
            })
            .collect()
    }

    /// The machine's name: its short name, from the data's `hostname` or
    /// `fqdn`, or else the cloud's; none with `preserve_hostname`.
    pub fn hostname(&self, meta: &MetaData) -> Option<String> {
        if self.preserve_hostname {
            return None;
        }
        let name = self.hostname.as_ref().or(self.fqdn.as_ref()).or(meta.hostname.as_ref())?;
        short_name(name)
    }
}

/// The short name of `name`, which may have its domain after it.
pub fn short_name(name: &str) -> Option<String> {
    let short = name.trim().split('.').next().unwrap_or("");
    (!short.is_empty()).then(|| short.to_string())
}

/// `keys` once each, in order.
fn unique<'a>(keys: impl Iterator<Item = &'a String>) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    for key in keys {
        if !seen.contains(key) {
            seen.push(key.clone());
        }
    }
    seen
}

/// Reads `users`: a list of `default`, names, and accounts in full.
fn read_users(value: &Value, left_out: &mut Vec<String>) -> Result<Vec<UserEntry>, String> {
    let mut users = Vec::new();
    for (i, item) in value.items().into_iter().enumerate() {
        match item {
            Value::Text(name) if name == "default" => users.push(UserEntry::Default),
            Value::Text(names) => {
                // A list of names in one string, as cloud-init takes too.
                for name in names.split(',').map(str::trim).filter(|n| !n.is_empty()) {
                    users.push(UserEntry::Named(plain_account(name)));
                }
            }
            Value::Map(_) => users.push(UserEntry::Named(read_account(item, i, left_out)?)),
            _ => return Err(format!("users: entry {} is not a name or an account", i + 1)),
        }
    }
    Ok(users)
}

/// An account of `name` alone: no administrator, no keys.
fn plain_account(name: &str) -> Account {
    Account {
        name: name.into(),
        full_name: String::new(),
        admin: false,
        without_password: false,
        keys: Vec::new(),
        password: None,
    }
}

/// Reads an account of `users`, its `i`th entry.
fn read_account(value: &Value, i: usize, left_out: &mut Vec<String>) -> Result<Account, String> {
    let name = value
        .get("name")
        .and_then(Value::text)
        .ok_or_else(|| format!("users: entry {} has no name", i + 1))?;
    let mut account = plain_account(name);
    let Value::Map(entries) = value else { return Ok(account) };
    let mut locked = true;
    for (key, value) in entries {
        match key.as_str() {
            "name" => {}
            "gecos" => account.full_name = value.text().unwrap_or("").to_string(),
            "groups" => {
                let groups: Vec<&str> =
                    value.texts().into_iter().flat_map(|g| g.split(',')).map(str::trim).collect();
                account.admin |= groups.iter().any(|g| ADMIN_GROUPS.contains(g));
            }
            "sudo" => {
                // A rule, or a list of them; false or null for none.
                let rules = value.texts();
                if value.truth() != Some(false) && !rules.is_empty() {
                    account.admin = true;
                    account.without_password |= rules.iter().any(|r| r.contains("NOPASSWD"));
                }
            }
            "ssh_authorized_keys" | "ssh-authorized-keys" => {
                account.keys = value.texts().into_iter().flat_map(key_lines).collect();
            }
            "lock_passwd" => locked = value.truth() != Some(false),
            "plain_text_passwd" => account.password = value.text().map(str::to_string),
            "passwd" | "hashed_passwd" => left_out.push(format!(
                "users: {name}'s {key}: a crypt(3) hash, which Dukovo's Argon2id passwords cannot \
                 take; give plain_text_passwd, or set one with passwd"
            )),
            "primary_group" | "shell" | "homedir" | "uid" | "system" | "no_create_home"
            | "no_user_group" | "inactive" | "expiredate" | "create_groups" | "selinux_user"
            | "snapuser" | "ssh_import_id" | "ssh_redirect_user" | "doas" => {
                left_out.push(format!("users: {name}'s {key}: cloud-setup does not read it"));
            }
            other => left_out.push(format!("users: {name}'s {other}: not a key cloud-init knows")),
        }
    }
    if locked {
        account.password = None;
    }
    Ok(account)
}

/// Reads an entry of `write_files`, its `i`th.
fn read_file(value: &Value, i: usize) -> Result<WriteFile, String> {
    let what = |what: &str| format!("write_files: entry {} {what}", i + 1);
    let path = value.get("path").and_then(Value::text).ok_or_else(|| what("has no path"))?;
    if !path.starts_with('/') {
        return Err(what(&format!("has a path that is not absolute: {path}")));
    }
    let text = value.get("content").and_then(Value::text).unwrap_or("");
    let content = match value.get("encoding").and_then(Value::text).unwrap_or("text/plain") {
        "text/plain" => text.as_bytes().to_vec(),
        "b64" | "base64" => {
            let joined: String = text.split_whitespace().collect();
            STANDARD.decode(joined).map_err(|err| what(&format!("is not base64: {err}")))?
        }
        other => return Err(what(&format!("has an encoding cloud-setup does not read: {other}"))),
    };
    let mode = match value.get("permissions").and_then(Value::text) {
        Some(text) => {
            let digits = text.trim_start_matches("0o");
            u32::from_str_radix(digits, 8)
                .ok()
                .filter(|mode| *mode <= 0o7777)
                .ok_or_else(|| what(&format!("has permissions that are not octal: {text}")))?
        }
        None => 0o644,
    };
    Ok(WriteFile {
        path: path.into(),
        content,
        mode,
        owner: value.get("owner").and_then(Value::text).map(str::to_string),
        append: value.get("append").and_then(Value::truth) == Some(true),
    })
}

/// Whether `text` is a crypt(3) hash, by its `$id$` start: MD5's,
/// bcrypt's, SHA-256's, SHA-512's or yescrypt's.
fn is_crypt_hash(text: &str) -> bool {
    ["$1$", "$2a$", "$2b$", "$2y$", "$5$", "$6$", "$y$", "$gy$", "$7$"]
        .iter()
        .any(|id| text.starts_with(id))
}

/// Reads `user`, the default account's name: a name, or a mapping of
/// which only its `name` is read.
fn read_default_name(value: &Value, left_out: &mut Vec<String>) -> Result<Option<String>, String> {
    match value {
        Value::Text(name) => Ok(Some(name.clone())),
        Value::Map(entries) => {
            let name =
                value.get("name").and_then(Value::text).ok_or("user: the mapping has no name")?;
            for (key, _) in entries.iter().filter(|(key, _)| key != "name") {
                left_out.push(format!("user: {key}: only the default account's name is read"));
            }
            Ok(Some(name.to_string()))
        }
        _ => Err("user: give the default account's name".into()),
    }
}

/// Reads `runcmd`: lines for the shell, and lists of a program's words.
fn read_commands(value: &Value) -> Result<Vec<Command>, String> {
    let mut commands = Vec::new();
    for (i, item) in value.items().into_iter().enumerate() {
        commands.push(match item {
            Value::Text(line) => Command::Shell(line.clone()),
            Value::List(words) => {
                let words: Option<Vec<String>> =
                    words.iter().map(|w| w.text().map(str::to_string)).collect();
                match words {
                    Some(words) if !words.is_empty() => Command::Words(words),
                    _ => return Err(format!("runcmd: entry {} is not a list of words", i + 1)),
                }
            }
            // YAML reads `- echo a: b` as a mapping, as cloud-init does:
            // the line wants quotes.
            Value::Map(_) => {
                return Err(format!(
                    "runcmd: entry {} is a mapping, not a command: quote a line with \": \" in it",
                    i + 1
                ));
            }
            _ => return Err(format!("runcmd: entry {} is not a command", i + 1)),
        });
    }
    Ok(commands)
}

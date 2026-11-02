// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What `smb` is told: where a share is, as whom to sign in, and how its
//! files are shown, in the words of Linux's mount.cifs(8), so that a line
//! that mounts a share there mounts it here.

/// SMB's port.
pub const PORT: u16 = 445;

/// A server, a share on it and a folder of the share, from
/// `//server[:port]/share/folder`, `\\server\share\folder` or
/// `smb://server/share/folder`.
#[derive(Debug, PartialEq, Eq)]
pub struct Target {
    /// The server's name or address; an IPv6 address without brackets.
    pub host: String,
    /// The port, if the target names one.
    pub port: Option<u16>,
    /// The share, empty if the target names only the server.
    pub share: String,
    /// The folder of the share, `/`-separated, empty for its top.
    pub path: String,
}

impl Target {
    /// Reads a target.
    pub fn parse(text: &str) -> Result<Target, String> {
        let rest = text
            .strip_prefix("smb://")
            .or_else(|| text.strip_prefix("//"))
            .or_else(|| text.strip_prefix("\\\\"))
            .ok_or_else(|| format!("{text}: not a share: give //server/share"))?;
        let rest = rest.replace('\\', "/");
        let mut parts = rest.split('/').filter(|part| !part.is_empty());
        let server = parts.next().ok_or_else(|| format!("{text}: no server named"))?;
        let (host, port) = server_port(server).ok_or_else(|| format!("{text}: a bad server"))?;
        let share = parts.next().unwrap_or_default().to_string();
        let path: Vec<&str> = parts.collect();
        if path.iter().any(|part| *part == "." || *part == "..") {
            return Err(format!("{text}: a folder of a share is named without . and .."));
        }
        Ok(Target { host, port, share, path: path.join("/") })
    }

    /// The server's address to connect to, with `port` if the options
    /// give one, or else the target's, or else SMB's.
    pub fn addr(&self, port: Option<u16>) -> String {
        let port = port.or(self.port).unwrap_or(PORT);
        if self.host.contains(':') {
            format!("[{}]:{port}", self.host)
        } else {
            format!("{}:{port}", self.host)
        }
    }

    /// The target as the mounts list shows it: `//server/share/folder`.
    pub fn source(&self) -> String {
        let host =
            if self.host.contains(':') { format!("[{}]", self.host) } else { self.host.clone() };
        let mut source = format!("//{host}/{}", self.share);
        if !self.path.is_empty() {
            source.push('/');
            source.push_str(&self.path);
        }
        source
    }
}

/// `host`, `host:port`, `[v6]` or `[v6]:port`, split.
fn server_port(server: &str) -> Option<(String, Option<u16>)> {
    if let Some(rest) = server.strip_prefix('[') {
        let (host, after) = rest.split_once(']')?;
        let port = match after {
            "" => None,
            after => Some(after.strip_prefix(':')?.parse().ok()?),
        };
        return (!host.is_empty()).then(|| (host.to_string(), port));
    }
    match server.matches(':').count() {
        0 => Some((server.to_string(), None)),
        // Two or more are an IPv6 address without a port.
        1 => {
            let (host, port) = server.split_once(':')?;
            if host.is_empty() {
                return None;
            }
            Some((host.to_string(), Some(port.parse().ok()?)))
        }
        _ => Some((server.to_string(), None)),
    }
}

/// The options, as `-o` gives them and a credentials file adds to.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Options {
    pub user: Option<String>,
    pub password: Option<String>,
    pub domain: Option<String>,
    /// A file of `username=`, `password=` and `domain=` lines.
    pub credentials: Option<String>,
    pub port: Option<u16>,
    pub uid: Option<u32>,
    pub gid: Option<u32>,
    pub file_mode: Option<u16>,
    pub dir_mode: Option<u16>,
    pub read_only: bool,
    /// Signs in as a guest, with no account or password.
    pub guest: bool,
}

/// Options that Linux's cifs takes and that change nothing here: the
/// protocol's version and security are what the server and smb agree
/// on, the rest is for the mounts table of Linux's boot or is how every
/// mount here is.
const IGNORED: &[&str] = &[
    "vers",
    "sec",
    "iocharset",
    "_netdev",
    "nofail",
    "defaults",
    "auto",
    "noauto",
    "nosuid",
    "nodev",
    "nounix",
    "noserverino",
    "serverino",
    "noperm",
    "soft",
    "hard",
    "cache",
];

impl Options {
    /// Adds the comma-separated options of `text`, as `-o` gives them; a
    /// doubled comma is a comma of a value, as mount.cifs reads a password.
    pub fn add(&mut self, text: &str) -> Result<(), String> {
        for option in split(text) {
            let (key, value) = match option.split_once('=') {
                Some((key, value)) => (key, Some(value)),
                None => (option.as_str(), None),
            };
            let given = || value.ok_or_else(|| format!("{key}: needs a value ({key}=...)"));
            match key {
                "user" | "username" => self.add_user(given()?),
                "password" | "pass" => self.password = Some(given()?.to_string()),
                "domain" | "dom" | "workgroup" => self.domain = Some(given()?.to_string()),
                "credentials" | "cred" => self.credentials = Some(given()?.to_string()),
                "port" => self.port = Some(number(key, given()?)?),
                "uid" => self.uid = Some(number(key, given()?)?),
                "gid" => self.gid = Some(number(key, given()?)?),
                "file_mode" => self.file_mode = Some(mode(key, given()?)?),
                "dir_mode" => self.dir_mode = Some(mode(key, given()?)?),
                "ro" if value.is_none() => self.read_only = true,
                "rw" if value.is_none() => self.read_only = false,
                "guest" if value.is_none() => self.guest = true,
                "" => {}
                key if IGNORED.contains(&key) => {}
                // The key alone: a mistyped password= says no password.
                _ => return Err(format!("{key}: not an option smb knows")),
            }
        }
        Ok(())
    }

    /// `user=`'s value: a name, `domain/name` or `domain\name`, and
    /// `%password` after it, as mount.cifs reads it.
    fn add_user(&mut self, value: &str) {
        let (who, password) = match value.split_once('%') {
            Some((who, password)) => (who, Some(password)),
            None => (value, None),
        };
        let (domain, name) = match who.split_once(['/', '\\']) {
            Some((domain, name)) => (Some(domain), name),
            None => (None, who),
        };
        self.user = Some(name.to_string());
        if let Some(domain) = domain {
            self.domain = Some(domain.to_string());
        }
        if let Some(password) = password {
            self.password = Some(password.to_string());
        }
    }

    /// Adds what a credentials file says, where the options do not say
    /// it: lines of `username=`, `password=` and `domain=`, as mount.cifs
    /// reads them, with blank lines and `#` comments.
    pub fn add_credentials(&mut self, text: &str) -> Result<(), String> {
        for (number, line) in text.lines().enumerate() {
            let line = line.strip_suffix('\r').unwrap_or(line);
            if line.trim().is_empty() || line.trim_start().starts_with('#') {
                continue;
            }
            // Its number, not the line, which may be a password.
            let Some((key, value)) = line.split_once('=') else {
                return Err(format!(
                    "line {}: not a username=, password= or domain= line",
                    number + 1
                ));
            };
            match key.trim() {
                "username" | "user" => {
                    if self.user.is_none() {
                        let password = self.password.take();
                        self.add_user(value);
                        if password.is_some() {
                            self.password = password;
                        }
                    }
                }
                "password" | "pass" => {
                    self.password.get_or_insert_with(|| value.to_string());
                }
                "domain" | "dom" | "workgroup" => {
                    self.domain.get_or_insert_with(|| value.to_string());
                }
                other => {
                    return Err(format!("{other}: not a username=, password= or domain= line"));
                }
            }
        }
        Ok(())
    }
}

/// `text` split at its single commas, a doubled comma kept as one.
fn split(text: &str) -> Vec<String> {
    let mut options = Vec::new();
    let mut option = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != ',' {
            option.push(c);
        } else if chars.peek() == Some(&',') {
            chars.next();
            option.push(',');
        } else {
            options.push(std::mem::take(&mut option));
        }
    }
    options.push(option);
    options
}

fn number<T: std::str::FromStr>(key: &str, value: &str) -> Result<T, String> {
    value.parse().map_err(|_| format!("{key}={value}: not a number"))
}

/// A mode in octal, permissions alone.
fn mode(key: &str, value: &str) -> Result<u16, String> {
    let mode = u16::from_str_radix(value, 8).map_err(|_| format!("{key}={value}: not a mode"))?;
    if mode > 0o777 {
        return Err(format!("{key}={value}: only permissions (at most 0777)"));
    }
    Ok(mode)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(host: &str, port: Option<u16>, share: &str, path: &str) -> Target {
        Target { host: host.into(), port, share: share.into(), path: path.into() }
    }

    #[test]
    fn targets_in_each_spelling() {
        assert_eq!(Target::parse("//nas/media").unwrap(), target("nas", None, "media", ""));
        assert_eq!(
            Target::parse("\\\\nas\\media\\tv").unwrap(),
            target("nas", None, "media", "tv")
        );
        assert_eq!(
            Target::parse("smb://nas:4445/media/a/b/").unwrap(),
            target("nas", Some(4445), "media", "a/b")
        );
        assert_eq!(
            Target::parse("//10.0.2.2:4445/share").unwrap(),
            target("10.0.2.2", Some(4445), "share", "")
        );
        assert_eq!(
            Target::parse("//[fe80::1]:139/s").unwrap(),
            target("fe80::1", Some(139), "s", "")
        );
        assert_eq!(Target::parse("//fe80::1/s").unwrap(), target("fe80::1", None, "s", ""));
        assert_eq!(Target::parse("//nas").unwrap(), target("nas", None, "", ""));
        for bad in ["nas/media", "//", "//:445/s", "//nas:x/s", "//[::1/s", "//nas/s/../t"] {
            assert!(Target::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn addresses_and_sources() {
        let t = target("fe80::1", Some(139), "s", "a/b");
        assert_eq!(t.addr(None), "[fe80::1]:139");
        assert_eq!(t.addr(Some(445)), "[fe80::1]:445");
        assert_eq!(t.source(), "//[fe80::1]/s/a/b");
        assert_eq!(target("nas", None, "media", "").addr(None), "nas:445");
        assert_eq!(target("nas", None, "media", "").source(), "//nas/media");
    }

    #[test]
    fn options_as_mount_cifs_takes_them() {
        let mut o = Options::default();
        o.add(
            "username=WORK/ann%se,,cret,uid=1000,gid=100,file_mode=0644,dir_mode=755,ro,vers=3.1.1",
        )
        .unwrap();
        assert_eq!(o.user.as_deref(), Some("ann"));
        assert_eq!(o.domain.as_deref(), Some("WORK"));
        assert_eq!(o.password.as_deref(), Some("se,cret"));
        assert_eq!(
            (o.uid, o.gid, o.file_mode, o.dir_mode),
            (Some(1000), Some(100), Some(0o644), Some(0o755))
        );
        assert!(o.read_only);
        o.add("rw,port=4445,guest,pass=x").unwrap();
        assert!(!o.read_only && o.guest);
        assert_eq!((o.port, o.password.as_deref()), (Some(4445), Some("x")));
        for bad in ["uid=ann", "file_mode=1777", "port", "colour=red", "ro=1", "dir_mode=9"] {
            assert!(Options::default().add(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn credentials_fill_in_what_the_options_leave() {
        let mut o = Options::default();
        o.add("user=bob").unwrap();
        o.add_credentials("# saved\r\nusername=ann\r\npassword= two words\n\ndomain=HOME\n")
            .unwrap();
        assert_eq!(o.user.as_deref(), Some("bob"));
        assert_eq!(o.password.as_deref(), Some(" two words"));
        assert_eq!(o.domain.as_deref(), Some("HOME"));
        let mut o = Options::default();
        o.add("password=given").unwrap();
        o.add_credentials("username=WORK\\ann%p\n").unwrap();
        assert_eq!((o.user.as_deref(), o.domain.as_deref()), (Some("ann"), Some("WORK")));
        assert_eq!(o.password.as_deref(), Some("given"));
        assert!(Options::default().add_credentials("token=x\n").is_err());
        assert!(Options::default().add_credentials("just words\n").is_err());
    }
}

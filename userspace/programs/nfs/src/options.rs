// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What to mount, `server:/path`, and how: mount.nfs(8)'s options, those
//! that mean something here, and the rest taken and left alone, so that
//! a line written for Linux mounts.

use std::net::IpAddr;
use std::time::Duration;

use nfsd::gss::Service;

/// NFS's port.
pub const PORT: u16 = 2049;

/// A folder of a server: `server:/path`, `[v6]:/path`, or
/// `nfs://server[:port]/path`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub host: String,
    pub port: Option<u16>,
    /// Its path's names, from the server's root.
    pub names: Vec<String>,
}

impl Target {
    /// Reads `source`.
    pub fn parse(source: &str) -> Result<Target, String> {
        let bad = || format!("{source}: not a server's folder (SERVER:/PATH)");
        let (host, port, path) = if let Some(rest) = source.strip_prefix("nfs://") {
            let (server, path) = rest.split_once('/').map_or((rest, ""), |(s, p)| (s, p));
            let (host, port) = match server.strip_prefix('[') {
                Some(v6) => {
                    let (host, after) = v6.split_once(']').ok_or_else(bad)?;
                    (host, after.strip_prefix(':'))
                }
                None => match server.rsplit_once(':') {
                    Some((host, port)) => (host, Some(port)),
                    None => (server, None),
                },
            };
            let port = port.map(|p| p.parse().map_err(|_| bad())).transpose()?;
            (host, port, path)
        } else if let Some(v6) = source.strip_prefix('[') {
            let (host, after) = v6.split_once(']').ok_or_else(bad)?;
            (host, None, after.strip_prefix(':').ok_or_else(bad)?)
        } else {
            let (host, path) = source.split_once(':').ok_or_else(bad)?;
            (host, None, path)
        };
        if host.is_empty() || host.contains('/') {
            return Err(bad());
        }
        let mut names = Vec::new();
        for name in path.split('/') {
            match name {
                "" | "." => {}
                ".." => return Err(format!("{source}: no `..` in the path")),
                name => names.push(name.to_string()),
            }
        }
        Ok(Target { host: host.to_string(), port, names })
    }

    /// The target as the mounts list shows it: `server:/path`.
    pub fn source(&self) -> String {
        let host =
            if self.host.contains(':') { format!("[{}]", self.host) } else { self.host.clone() };
        format!("{host}:/{}", self.names.join("/"))
    }
}

/// The options given.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Options {
    /// The minor version of NFS 4 asked for: 4.2 then 4.1 if `None`.
    pub minor: Option<u32>,
    pub port: Option<u16>,
    /// The server's address, which its name is not looked up for.
    pub addr: Option<IpAddr>,
    /// Whether calls are sent again for as long as the server is away
    /// (`hard`, as Linux's), or fail after `retrans` (`soft`).
    pub hard: bool,
    /// How long a reply is waited for, in tenths of a second.
    pub timeo: u32,
    pub retrans: u32,
    pub rsize: Option<u32>,
    pub wsize: Option<u32>,
    pub read_only: bool,
    /// How calls say who calls: AUTH_SYS's numbers (`sec=sys`), or
    /// Kerberos with one of RPCSEC_GSS's services (`sec=krb5`, `krb5i`,
    /// `krb5p`).
    pub sec: Option<Service>,
}

impl Default for Options {
    fn default() -> Options {
        Options {
            minor: None,
            port: None,
            addr: None,
            hard: true,
            timeo: 600,
            retrans: 2,
            rsize: None,
            wsize: None,
            read_only: false,
            sec: None,
        }
    }
}

/// Options that change nothing here: the kernel's caching and mount
/// flags, and those of NFS versions, transports and helpers this has
/// not.
const LEFT_ALONE: &[&str] = &[
    "ac",
    "acdirmax",
    "acdirmin",
    "acregmax",
    "acregmin",
    "actimeo",
    "async",
    "atime",
    "auto",
    "bg",
    "clientaddr",
    "cto",
    "defaults",
    "dev",
    "diratime",
    "exec",
    "fg",
    "fsc",
    "intr",
    "local_lock",
    "lock",
    "lookupcache",
    "max_connect",
    "mountaddr",
    "mounthost",
    "mountport",
    "mountproto",
    "mountvers",
    "namlen",
    "nconnect",
    "noac",
    "noatime",
    "noauto",
    "nocto",
    "nodev",
    "nodiratime",
    "noexec",
    "nofail",
    "nofsc",
    "nointr",
    "nolock",
    "norelatime",
    "noresvport",
    "nosharecache",
    "nosuid",
    "relatime",
    "resvport",
    "retry",
    "sharecache",
    "strictatime",
    "suid",
    "sync",
    "user",
    "users",
    "_netdev",
];

impl Options {
    /// Adds the comma-separated options of `text`.
    pub fn add(&mut self, text: &str) -> Result<(), String> {
        for option in text.split(',').filter(|o| !o.is_empty()) {
            let (key, value) = option.split_once('=').map_or((option, None), |(k, v)| (k, Some(v)));
            let number = || -> Result<u32, String> {
                value
                    .and_then(|v| v.parse().ok())
                    .ok_or_else(|| format!("{option}: {key} takes a number"))
            };
            match key {
                "vers" | "nfsvers" => {
                    self.minor = match value {
                        Some("4") => None,
                        Some("4.1") => Some(1),
                        Some("4.2") => Some(2),
                        Some("4.0") => {
                            return Err(format!(
                                "{option}: NFS 4.0 is not spoken, 4.1 and 4.2 are"
                            ));
                        }
                        _ => {
                            return Err(format!(
                                "{option}: NFS 4.1 and 4.2 are spoken, not 2 or 3"
                            ));
                        }
                    };
                }
                "minorversion" => {
                    self.minor = match number()? {
                        1 => Some(1),
                        2 => Some(2),
                        _ => return Err(format!("{option}: 4.1 and 4.2 are spoken")),
                    };
                }
                "port" => {
                    let port = number()?;
                    self.port =
                        Some(u16::try_from(port).map_err(|_| format!("{option}: no such port"))?)
                            .filter(|&p| p != 0);
                }
                "addr" => {
                    let addr = value.unwrap_or("").trim_start_matches('[').trim_end_matches(']');
                    self.addr =
                        Some(addr.parse().map_err(|_| format!("{option}: not an address"))?);
                }
                "proto" => match value {
                    Some("tcp" | "tcp6") => {}
                    _ => return Err(format!("{option}: NFS 4 goes over TCP alone here")),
                },
                "sec" => match value {
                    Some("sys") => self.sec = None,
                    Some(name) => {
                        let service = Service::from_name(name).ok_or_else(|| {
                            format!("{option}: sec=sys, krb5, krb5i or krb5p, one of them")
                        })?;
                        self.sec = Some(service);
                    }
                    None => return Err(format!("{option}: sec=sys, krb5, krb5i or krb5p")),
                },
                "hard" => self.hard = true,
                "soft" | "softerr" => self.hard = false,
                "timeo" => self.timeo = number()?.max(1),
                "retrans" => self.retrans = number()?,
                "rsize" => self.rsize = Some(number()?),
                "wsize" => self.wsize = Some(number()?),
                "ro" => self.read_only = true,
                "rw" => self.read_only = false,
                key if LEFT_ALONE.contains(&key) || key.starts_with("x-") => {}
                _ => return Err(format!("{option}: not an option of nfs")),
            }
        }
        Ok(())
    }

    /// How long a reply is waited for.
    pub fn timeout(&self) -> Duration {
        Duration::from_millis(u64::from(self.timeo) * 100)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(target: &Target) -> Vec<&str> {
        target.names.iter().map(String::as_str).collect()
    }

    #[test]
    fn sources() {
        let t = Target::parse("nas:/srv/music/").unwrap();
        assert_eq!((t.host.as_str(), t.port, names(&t)), ("nas", None, vec!["srv", "music"]));
        assert_eq!(t.source(), "nas:/srv/music");
        let t = Target::parse("[fe80::1]:/").unwrap();
        assert_eq!((t.host.as_str(), names(&t).len()), ("fe80::1", 0));
        assert_eq!(t.source(), "[fe80::1]:/");
        let t = Target::parse("nfs://nas:2050/a/./b").unwrap();
        assert_eq!((t.host.as_str(), t.port, names(&t)), ("nas", Some(2050), vec!["a", "b"]));
        assert!(Target::parse("nas").is_err());
        assert!(Target::parse(":/a").is_err());
        assert!(Target::parse("nas:/a/../b").is_err());
        assert!(Target::parse("//nas/share").is_err());
    }

    #[test]
    fn options_as_mount_nfs_takes_them() {
        let mut o = Options::default();
        o.add("vers=4.2,port=2050,addr=10.0.2.2,soft,timeo=100,retrans=3,rsize=65536,ro").unwrap();
        o.add("noatime,nolock,x-systemd.automount,proto=tcp,sec=sys,clientaddr=10.0.2.15").unwrap();
        assert_eq!(o.minor, Some(2));
        assert_eq!(o.port, Some(2050));
        assert_eq!(o.addr, Some("10.0.2.2".parse().unwrap()));
        assert!(!o.hard && o.read_only);
        assert_eq!((o.timeout(), o.retrans, o.rsize), (Duration::from_secs(10), 3, Some(65536)));
        o.add("vers=4,hard,rw").unwrap();
        assert!(o.minor.is_none() && o.hard && !o.read_only);
        o.add("sec=krb5p").unwrap();
        assert_eq!(o.sec, Some(Service::Privacy));
        o.add("sec=sys").unwrap();
        assert_eq!(o.sec, None);
        let refused =
            ["vers=3", "vers=4.0", "proto=udp", "sec=lkey", "sec=krb5:sys", "port=x", "frobnicate"];
        for refused in refused {
            assert!(Options::default().add(refused).is_err(), "{refused}");
        }
    }
}

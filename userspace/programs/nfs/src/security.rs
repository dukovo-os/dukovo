// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Kerberos for a mount with `sec=krb5`, `krb5i` or `krb5p` (ADR 0399):
//! each person's calls go under a context of their own, set up with the
//! server from their own ticket, as Linux's rpc.gssd sets them up from
//! each person's ticket cache.
//!
//! - A person's ticket for the server's `nfs/HOST` comes from their
//!   keyring, through authd ([`AuthdTickets`]): their ticket-granting
//!   ticket, which kinit(1) kept there, gets one from the KDC.
//! - Root's calls, as the mount's own walk to its folder, go under the
//!   machine's key for `host/HOST` or `nfs/HOST` in the keytab, as on
//!   Linux, or else under the tickets of the person who mounted through
//!   elevate(1).
//! - The calls that keep the client's state with the server (EXCHANGE_ID,
//!   CREATE_SESSION, a SEQUENCE alone) are no one's, and go with
//!   AUTH_SYS's numbers, as RFC 8881's SP4_NONE lets them.
//!
//! Where to start when a person's calls are refused: `keyring list` shows
//! whether their keyring holds `krb5` tickets, `klist` whether they are
//! still good, and `kvno nfs/HOST` whether the KDC knows the server.

use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;

use accounts::request::{self, Answer, Request};
use kerberos::client::{self, Options, Secret};
use kerberos::config::Config;
use kerberos::creds::{Credential, MemoryKeyring};
use kerberos::gss::{Initiator, flags};
use kerberos::keytab;
use kerberos::messages::Principal;
use nfsd::gss::Service;

use crate::client::Fail;
use crate::rpc::{Conn, GssSession};

/// Where a person's tickets come from.
pub trait Tickets {
    /// A ticket for `server` for the person numbered `uid`.
    ///
    /// # Errors
    ///
    /// What is missing, and what to do: no ticket, or the KDC's refusal.
    fn ticket(&mut self, uid: u32, server: &Principal) -> Result<Credential, String>;
}

/// The contexts of a mount's callers, one each.
pub struct Kerberos {
    /// What the contexts protect.
    pub service: Service,
    /// The server, `nfs/HOST@REALM`.
    server: Principal,
    tickets: Box<dyn Tickets + Send>,
    sessions: HashMap<u32, GssSession>,
}

impl Kerberos {
    /// Contexts for `service` with `server`, from `tickets`.
    pub fn new(service: Service, server: Principal, tickets: Box<dyn Tickets + Send>) -> Kerberos {
        Kerberos { service, server, tickets, sessions: HashMap::new() }
    }

    /// The context of the person numbered `uid`, set up over `conn` if
    /// there is none yet or theirs is spent.
    ///
    /// # Errors
    ///
    /// [`Fail::Denied`] when they have no ticket or the server refuses
    /// it; [`Fail::Io`] when the connection fails on the way.
    pub fn session(&mut self, uid: u32, conn: &mut Conn) -> Result<&mut GssSession, Fail> {
        let (now, _) = kerberos::time::now();
        if self.sessions.get(&uid).is_some_and(|session| session.spent(now)) {
            self.sessions.remove(&uid);
        }
        if !self.sessions.contains_key(&uid) {
            let session = self.set_up(uid, conn)?;
            self.sessions.insert(uid, session);
        }
        self.sessions.get_mut(&uid).ok_or_else(|| Fail::Denied("no Kerberos context".into()))
    }

    /// Forgets the context of the person numbered `uid`, which the server
    /// no longer holds.
    pub fn forget(&mut self, uid: u32) {
        self.sessions.remove(&uid);
    }

    /// Sets up a context for the person numbered `uid` (RFC 2203 5.2).
    fn set_up(&mut self, uid: u32, conn: &mut Conn) -> Result<GssSession, Fail> {
        let credential = self.tickets.ticket(uid, &self.server).map_err(Fail::Denied)?;
        let denied = |why: kerberos::Error| {
            Fail::Denied(format!(
                "{}'s Kerberos context with {}: {why}",
                credential.client, self.server
            ))
        };
        let (initiator, token) = Initiator::start(&credential, flags::MUTUAL).map_err(denied)?;
        let (result, verifier) = conn.init(self.service, &token).map_err(|err| {
            if err.kind() == io::ErrorKind::PermissionDenied {
                Fail::Denied(format!("the server refused RPCSEC_GSS: {err}"))
            } else {
                Fail::Io(err)
            }
        })?;
        if result.major != 0 {
            return Err(Fail::Denied(format!(
                "the server refused {}'s ticket (GSS-API status {:#x}): its log says why, most \
                 often a keytab without the key of {} that the KDC has",
                credential.client, result.major, self.server
            )));
        }
        let context = initiator.finish(Some(&result.token)).map_err(denied)?;
        // The reply's verifier signs the window: the server holds the
        // context too.
        context.verify_mic(&result.window.to_be_bytes(), &verifier).map_err(denied)?;
        Ok(GssSession::new(context, result.handle, self.service))
    }
}

/// The principal of the NFS server named `host`: `nfs/HOST@REALM`, with
/// the realm krb5.conf gives the host.
///
/// # Errors
///
/// A server given by its address, whose name Kerberos needs.
pub fn server_principal(host: &str, config: &Config) -> Result<Principal, String> {
    if host.parse::<std::net::IpAddr>().is_ok() {
        return Err(format!(
            "{host}: Kerberos names the server: mount it by its name, with addr={host} if the \
             name does not resolve"
        ));
    }
    let realm = config.realm_of(host).ok_or_else(|| {
        format!(
            "{host}: krb5.conf names no realm for it: set default_realm, or a [domain_realm] line"
        )
    })?;
    Ok(Principal::service("nfs", host, &realm))
}

/// Tickets from the callers' keyrings, through authd, and the machine's
/// keytab for root.
pub struct AuthdTickets {
    config: Config,
    /// This machine's name, for its keytab's principals.
    machine: String,
    /// The person who mounted through elevate, whose tickets root's calls
    /// go under when the keytab has no key of the machine's.
    mounter: Option<u32>,
}

impl AuthdTickets {
    /// Tickets under `config`, for the machine named `machine`.
    pub fn new(config: Config, machine: &str) -> AuthdTickets {
        // elevate(1) names the person it ran for.
        let mounter = std::env::var("SUDO_UID").ok().and_then(|uid| uid.parse().ok());
        AuthdTickets { config, machine: machine.to_lowercase(), mounter }
    }

    /// A ticket for `server` under the machine's own key, if the keytab
    /// holds one.
    fn machine_ticket(&self, server: &Principal) -> Option<Result<Credential, String>> {
        let entries = keytab::read(&keytab::path()).ok()?;
        let realm = self.config.realm_of(&self.machine)?;
        let (principal, keys) = ["host", "nfs"].iter().find_map(|service| {
            let principal = Principal::service(service, &self.machine, &realm);
            let keys = keytab::keys_for(&entries, &principal, &self.config.enctypes);
            (!keys.is_empty()).then_some((principal, keys))
        })?;
        let tgt = client::initial(
            &self.config,
            &principal,
            &Secret::Keys(&keys),
            &Options::from(&self.config),
        );
        Some(
            tgt.and_then(|tgt| client::service(&self.config, &tgt, server))
                .map_err(|err| format!("the machine's key for {principal}: {err}")),
        )
    }
}

impl Tickets for AuthdTickets {
    fn ticket(&mut self, uid: u32, server: &Principal) -> Result<Credential, String> {
        if uid == 0
            && let Some(ticket) = self.machine_ticket(server)
        {
            return ticket;
        }
        let owner = match (uid, self.mounter) {
            (0, Some(mounter)) => mounter,
            (0, None) => {
                return Err(format!(
                    "root has no Kerberos identity for {server}: give the machine a keytab with \
                     host/{} in it, or mount through elevate after kinit",
                    self.machine
                ));
            }
            (uid, _) => uid,
        };
        let mut keyring = MemoryKeyring::default();
        for line in ask(&Request::KerberosTickets { uid: owner })?.lines() {
            let (name, hexed) = line.split_once('\t').ok_or("authd's answer does not read")?;
            let secret = request::unhex(hexed).ok_or("authd's answer does not read")?;
            keyring.0.insert(name.to_string(), secret);
        }
        client::ticket_for(&self.config, &mut keyring, server).map_err(|err| err.to_string())
    }
}

/// Asks authd `request`: its answer's text.
fn ask(request: &Request) -> Result<String, String> {
    let unreachable = |err: io::Error| format!("cannot reach authd at {}: {err}", accounts::SOCKET);
    let mut stream = UnixStream::connect(accounts::SOCKET).map_err(unreachable)?;
    stream.write_all(&request.encode()).map_err(unreachable)?;
    stream.shutdown(Shutdown::Write).map_err(unreachable)?;
    let mut answer = String::new();
    stream.read_to_string(&mut answer).map_err(unreachable)?;
    match Answer::decode(&answer) {
        Answer::Done(text) => Ok(text),
        Answer::Refused(why) => Err(why),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn servers_are_named_by_their_host() {
        let config = Config::parse("[libdefaults]\n default_realm = EXAMPLE.ORG\n").unwrap();
        let nfs = server_principal("NAS.example.org", &config).unwrap();
        assert_eq!(nfs.to_string(), "nfs/nas.example.org@EXAMPLE.ORG");
        let err = server_principal("10.0.2.2", &config).unwrap_err();
        assert!(err.contains("addr=10.0.2.2"), "{err}");
        let none = Config::parse("").unwrap();
        assert!(server_principal("nas", &none).is_err());
    }
}

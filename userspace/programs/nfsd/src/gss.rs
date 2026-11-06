// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! RPCSEC_GSS (RFC 2203), as NFS's `sec=krb5`, `krb5i` and `krb5p` use
//! it with Kerberos (ADR 0399): the credential each call carries, and
//! how a call's arguments and a reply's results are signed or sealed.
//! Both sides use these: nfs(1) as the client, nfsd(8) as the server.
//!
//! A client first sets up a context with the server: it calls NULL with
//! a credential that says INIT and GSS-API's first token as the
//! argument, and the server answers with a handle for the context. Each
//! call after names the handle and a sequence number, and its verifier
//! is a GSS-API MIC of the call's header. Then, by the service:
//!
//! - `krb5` ([`Service::None`]): the arguments and results go as they
//!   are; only the header is signed.
//! - `krb5i` ([`Service::Integrity`]): each body is the sequence number
//!   and the arguments, followed by a MIC of them.
//! - `krb5p` ([`Service::Privacy`]): each body is the sequence number and
//!   the arguments, sealed in a GSS-API Wrap token.
//!
//! The reply's verifier is a MIC of the call's sequence number, which is
//! how the client knows the server holds the context.
//!
//! Where to start when a mount with Kerberos fails: the server's log says
//! why it refused a context (nfsd's says `nfsd: Kerberos: ...`); a call
//! refused with CREDPROBLEM or CTXPROBLEM is a context the server no
//! longer holds, which the client sets up again.

use kerberos::gss::Context;

use crate::xdr::{self, Reader, Writer};

/// RPCSEC_GSS's credential flavor.
pub const RPCSEC_GSS: u32 = 6;

/// The version of RPCSEC_GSS's credential.
const VERSION: u32 = 1;

/// What a call with RPCSEC_GSS's credential is for (rpc_gss_proc_t).
pub mod proc {
    /// An ordinary call, under a context.
    pub const DATA: u32 = 0;
    /// The first call of a context.
    pub const INIT: u32 = 1;
    /// A later call to set up a context, for mechanisms that take more
    /// than one round; Kerberos takes one.
    pub const CONTINUE_INIT: u32 = 2;
    /// The end of a context.
    pub const DESTROY: u32 = 3;
}

/// auth_stat: the server holds no such context, or its credential is
/// bad (RFC 2203 5.3.3.3).
pub const CREDPROBLEM: u32 = 13;
/// auth_stat: the context is there, but a check under it failed.
pub const CTXPROBLEM: u32 = 14;

/// The highest sequence number a context may use; past it, the client
/// sets up another.
pub const MAXSEQ: u32 = 0x8000_0000;

/// GSS-API's major status for a failure, which an INIT's result carries
/// when the server would not accept the context.
pub const GSS_S_FAILURE: u32 = 13 << 16;

/// What a context protects: the three services, as the `sec=` options
/// name them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Service {
    /// `krb5`: who calls, and the header, are checked.
    None,
    /// `krb5i`: the arguments and results are signed too.
    Integrity,
    /// `krb5p`: and sealed.
    Privacy,
}

impl Service {
    /// The service numbered `number` in a credential.
    pub fn from_number(number: u32) -> Option<Service> {
        match number {
            1 => Some(Service::None),
            2 => Some(Service::Integrity),
            3 => Some(Service::Privacy),
            _ => None,
        }
    }

    /// Its number in a credential (rpc_gss_service_t).
    pub fn number(self) -> u32 {
        match self {
            Service::None => 1,
            Service::Integrity => 2,
            Service::Privacy => 3,
        }
    }

    /// The `sec=` option's name of it.
    pub fn name(self) -> &'static str {
        match self {
            Service::None => "krb5",
            Service::Integrity => "krb5i",
            Service::Privacy => "krb5p",
        }
    }

    /// The service a `sec=` name names.
    pub fn from_name(name: &str) -> Option<Service> {
        [Service::None, Service::Integrity, Service::Privacy]
            .into_iter()
            .find(|service| service.name() == name)
    }
}

/// How a call says who calls, as a share's `nfs-sec` lists them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Flavor {
    /// AUTH_SYS's numbers, or AUTH_NONE's no one: `sys`.
    Sys,
    /// RPCSEC_GSS with Kerberos, under a service.
    Krb5(Service),
}

impl Flavor {
    /// The flavor `nfs-sec` names `name`.
    pub fn from_name(name: &str) -> Option<Flavor> {
        if name == "sys" { Some(Flavor::Sys) } else { Service::from_name(name).map(Flavor::Krb5) }
    }

    /// Writes it as SECINFO's secinfo4 gives it (RFC 8881 18.29): the
    /// flavor, and for RPCSEC_GSS the mechanism, its quality of
    /// protection and the service.
    pub fn write_secinfo(self, w: &mut Writer) {
        match self {
            Flavor::Sys => {
                w.u32(crate::rpc::AUTH_SYS);
            }
            Flavor::Krb5(service) => {
                w.u32(RPCSEC_GSS).opaque(krb5_oid()).u32(0).u32(service.number());
            }
        }
    }
}

/// The credential of an RPCSEC_GSS call (rpc_gss_cred_vers_1_t).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cred {
    /// What the call is for: [`proc`].
    pub proc: u32,
    /// Its sequence number, under the context.
    pub seq: u32,
    /// The service.
    pub service: Service,
    /// The context's handle, which the server gave; empty for INIT.
    pub handle: Vec<u8>,
}

impl Cred {
    /// The credential's body, as a call carries it.
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.u32(VERSION).u32(self.proc).u32(self.seq).u32(self.service.number());
        w.opaque(&self.handle);
        w.bytes
    }

    /// The credential whose body is `body`.
    ///
    /// # Errors
    ///
    /// A body that does not read, of another version, or of a service
    /// that is none of the three.
    pub fn decode(body: &[u8]) -> xdr::Result<Cred> {
        let mut r = Reader::new(body);
        if r.u32()? != VERSION {
            return Err(xdr::Bad);
        }
        let proc = r.u32()?;
        let seq = r.u32()?;
        let service = Service::from_number(r.u32()?).ok_or(xdr::Bad)?;
        let handle = r.opaque(64)?.to_vec();
        Ok(Cred { proc, seq, service, handle })
    }
}

/// What went wrong with a protected body.
#[derive(Debug, PartialEq, Eq)]
pub enum BodyError {
    /// It does not read as the service's body.
    Garbage,
    /// Its signature or seal does not check under the context.
    Integrity,
}

/// A call's arguments or a reply's results, `body`, protected for
/// `service` under `context` with sequence number `seq`.
///
/// # Errors
///
/// Sealing's, which only a failure to read random bytes causes.
pub fn protect(
    context: &mut Context,
    service: Service,
    seq: u32,
    body: &[u8],
) -> kerberos::Result<Vec<u8>> {
    if service == Service::None {
        return Ok(body.to_vec());
    }
    // The sequence number goes inside, so that a body cannot be moved
    // to another call.
    let mut data = Vec::with_capacity(body.len() + 4);
    data.extend_from_slice(&seq.to_be_bytes());
    data.extend_from_slice(body);
    let mut w = Writer::new();
    if service == Service::Integrity {
        let mic = context.get_mic(&data);
        w.opaque(&data).opaque(&mic);
    } else {
        w.opaque(&context.wrap(&data, true)?);
    }
    Ok(w.bytes)
}

/// The body that `data` protects for `service` under `context`, which
/// must carry sequence number `seq`.
///
/// # Errors
///
/// [`BodyError::Garbage`] for data that does not read, or carries
/// another sequence number; [`BodyError::Integrity`] for a signature or
/// seal that does not check.
pub fn unprotect(
    context: &Context,
    service: Service,
    seq: u32,
    data: &[u8],
) -> Result<Vec<u8>, BodyError> {
    let inner = match service {
        Service::None => return Ok(data.to_vec()),
        Service::Integrity => {
            let mut r = Reader::new(data);
            let signed = r.opaque(data.len()).map_err(|_| BodyError::Garbage)?;
            let mic = r.opaque(1024).map_err(|_| BodyError::Garbage)?;
            context.verify_mic(signed, mic).map_err(|_| BodyError::Integrity)?;
            signed.to_vec()
        }
        Service::Privacy => {
            let mut r = Reader::new(data);
            let sealed = r.opaque(data.len()).map_err(|_| BodyError::Garbage)?;
            let (plain, was_sealed) = context.unwrap(sealed).map_err(|_| BodyError::Integrity)?;
            if !was_sealed {
                return Err(BodyError::Integrity);
            }
            plain
        }
    };
    match inner.split_first_chunk::<4>() {
        Some((number, body)) if u32::from_be_bytes(*number) == seq => Ok(body.to_vec()),
        _ => Err(BodyError::Garbage),
    }
}

/// The verifier a reply carries for the call numbered `seq`: a MIC of
/// the number.
pub fn reply_verifier(context: &mut Context, seq: u32) -> Vec<u8> {
    context.get_mic(&seq.to_be_bytes())
}

/// Checks the verifier `mic` of a reply to the call numbered `seq`.
///
/// # Errors
///
/// The MIC's, when it does not check: a reply from someone without the
/// context.
pub fn check_reply_verifier(context: &Context, seq: u32, mic: &[u8]) -> kerberos::Result<()> {
    context.verify_mic(&seq.to_be_bytes(), mic)
}

/// The arguments of an INIT call: GSS-API's token (rpc_gss_init_arg).
pub fn init_args(token: &[u8]) -> Vec<u8> {
    let mut w = Writer::new();
    w.opaque(token);
    w.bytes
}

/// The results of an INIT call (rpc_gss_init_res).
#[derive(Debug, PartialEq, Eq)]
pub struct InitResult {
    /// The context's handle.
    pub handle: Vec<u8>,
    /// GSS-API's major status: 0 when the context is set up.
    pub major: u32,
    /// The mechanism's minor status.
    pub minor: u32,
    /// How many sequence numbers ahead of the highest seen the server
    /// takes.
    pub window: u32,
    /// GSS-API's answering token, if any.
    pub token: Vec<u8>,
}

impl InitResult {
    /// The results, as a reply carries them.
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.opaque(&self.handle).u32(self.major).u32(self.minor).u32(self.window);
        w.opaque(&self.token);
        w.bytes
    }

    /// The results `data`.
    ///
    /// # Errors
    ///
    /// Results that do not read.
    pub fn decode(data: &[u8]) -> xdr::Result<InitResult> {
        let mut r = Reader::new(data);
        let handle = r.opaque(64)?.to_vec();
        let major = r.u32()?;
        let minor = r.u32()?;
        let window = r.u32()?;
        let token = r.opaque(64 * 1024)?.to_vec();
        Ok(InitResult { handle, major, minor, window, token })
    }
}

/// The Kerberos 5 mechanism's OID, in the form SECINFO gives it.
pub fn krb5_oid() -> &'static [u8] {
    &kerberos::gss::KRB5_OID
}

#[cfg(test)]
mod tests {
    use super::*;
    use kerberos::gss::flags;

    /// Both sides of a context, set up with a made-up KDC's ticket.
    fn pair() -> (Context, Context) {
        kerberos::gss::testing::pair(flags::MUTUAL)
    }

    #[test]
    fn credentials_read_back() {
        let cred =
            Cred { proc: proc::DATA, seq: 7, service: Service::Privacy, handle: vec![1, 2, 3] };
        assert_eq!(Cred::decode(&cred.encode()), Ok(cred));
        let mut bad = Writer::new();
        bad.u32(2).u32(0).u32(0).u32(1).opaque(b"");
        assert!(Cred::decode(&bad.bytes).is_err());
    }

    #[test]
    fn bodies_go_both_ways_for_each_service() {
        let (mut client, mut server) = pair();
        for service in [Service::None, Service::Integrity, Service::Privacy] {
            let data = protect(&mut client, service, 42, b"COMPOUND args").unwrap();
            assert_eq!(data.windows(8).any(|w| w == b"COMPOUND"), service != Service::Privacy);
            assert_eq!(unprotect(&server, service, 42, &data).unwrap(), b"COMPOUND args");
            let back = protect(&mut server, service, 42, b"results").unwrap();
            assert_eq!(unprotect(&client, service, 42, &back).unwrap(), b"results");
        }
    }

    #[test]
    fn a_body_moved_to_another_call_or_changed_is_refused() {
        let (mut client, server) = pair();
        for service in [Service::Integrity, Service::Privacy] {
            let data = protect(&mut client, service, 42, b"args").unwrap();
            assert_eq!(unprotect(&server, service, 43, &data), Err(BodyError::Garbage));
            let mut changed = data.clone();
            changed[10] ^= 1;
            assert!(unprotect(&server, service, 42, &changed).is_err());
        }
    }

    #[test]
    fn reply_verifiers_check() {
        let (client, mut server) = pair();
        let mic = reply_verifier(&mut server, 9);
        check_reply_verifier(&client, 9, &mic).unwrap();
        assert!(check_reply_verifier(&client, 10, &mic).is_err());
    }

    #[test]
    fn init_results_read_back() {
        let init =
            InitResult { handle: vec![9; 8], major: 0, minor: 0, window: 128, token: vec![1; 99] };
        assert_eq!(InitResult::decode(&init.encode()), Ok(init));
    }

    #[test]
    fn flavors_are_given_as_secinfo_gives_them() {
        let mut w = Writer::new();
        Flavor::from_name("sys").unwrap().write_secinfo(&mut w);
        Flavor::from_name("krb5p").unwrap().write_secinfo(&mut w);
        let mut r = Reader::new(&w.bytes);
        assert_eq!(r.u32(), Ok(crate::rpc::AUTH_SYS));
        assert_eq!(r.u32(), Ok(RPCSEC_GSS));
        assert_eq!(r.opaque(16), Ok(&kerberos::gss::KRB5_OID[..]));
        assert_eq!((r.u32(), r.u32()), (Ok(0), Ok(3)));
        assert_eq!(Flavor::from_name("lkey"), None);
    }

    #[test]
    fn services_are_named_as_the_sec_options_name_them() {
        for name in ["krb5", "krb5i", "krb5p"] {
            assert_eq!(Service::from_name(name).unwrap().name(), name);
        }
        assert_eq!(Service::from_name("sys"), None);
    }
}

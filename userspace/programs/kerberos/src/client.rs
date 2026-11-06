// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A client's two exchanges with its KDC (RFC 4120 3.1 and 3.3): the AS
//! exchange, which turns a password or a keytab's key into a
//! ticket-granting ticket, and the TGS exchange, which turns that ticket
//! into one for a service, as `nfs/server@REALM`.
//!
//! The AS exchange asks first without proof. A KDC that wants proof, as
//! MIT's does by default, refuses with PREAUTH_REQUIRED and says which
//! salt and rounds make the client's key (ETYPE-INFO2). The client then
//! sends the time encrypted under that key (PA-ENC-TIMESTAMP) and opens
//! the reply with it. A wrong password shows as PREAUTH_FAILED here; a
//! clock too far from the KDC's as SKEW, which the error explains.
//!
//! To see an exchange, run the KDC with logging (MIT's krb5kdc logs each
//! request with its result) or capture port 88: every field is in the
//! clear except the encrypted parts.

use crate::config::Config;
use crate::creds::{self, Credential, Keyring};
use crate::crypto::{self, Enctype, Key};
use crate::messages::{
    self, Answer, Authenticator, Encrypted, EtypeInfo, KdcError, PaData, Principal, Reply,
    ReplyPart, RequestBody, codes, flags,
};
use crate::{Error, Result, kdc, time};

/// Key usage: PA-ENC-TIMESTAMP (RFC 4120 7.5.1).
const USAGE_TIMESTAMP: u32 = 1;
/// Key usage: an AS reply's part, under the client's key.
const USAGE_AS_REP: u32 = 3;
/// Key usage: the checksum of a TGS request's body.
const USAGE_TGS_CHECKSUM: u32 = 6;
/// Key usage: a TGS request's authenticator.
const USAGE_TGS_AUTHENTICATOR: u32 = 7;
/// Key usage: a TGS reply's part, under the session key.
const USAGE_TGS_REP: u32 = 8;

/// How the client proves itself in the AS exchange.
pub enum Secret<'a> {
    /// A password, made into the key the KDC says.
    Password(&'a [u8]),
    /// Keys from a keytab, with their versions.
    Keys(&'a [(u32, Key)]),
}

/// What a ticket is asked for with.
#[derive(Clone, Debug)]
pub struct Options {
    /// Its lifetime, in seconds.
    pub lifetime: i64,
    /// How long it may be renewed for, if at all.
    pub renewable: Option<i64>,
    /// Whether it may be forwarded.
    pub forwardable: bool,
    /// The service of the AS exchange: the realm's ticket-granting
    /// service unless set.
    pub service: Option<Principal>,
}

impl Options {
    /// What krb5.conf says.
    pub fn from(config: &Config) -> Options {
        Options {
            lifetime: config.lifetime,
            renewable: config.renew_lifetime,
            forwardable: config.forwardable,
            service: None,
        }
    }
}

/// A random nonce, kept to 31 bits as MIT's are, which some KDCs need.
fn nonce() -> Result<u32> {
    let bytes = crypto::random(4)?;
    Ok(u32::from_be_bytes(bytes.try_into().expect("4 bytes")) & 0x7fff_ffff)
}

/// The KDC options and times `options` ask for, from `now`.
fn body_for(
    options: &Options,
    client: Option<Principal>,
    server: Principal,
    enctypes: &[Enctype],
    now: i64,
) -> Result<RequestBody> {
    let mut kdc_options = flags::CANONICALIZE;
    if options.forwardable {
        kdc_options |= flags::FORWARDABLE;
    }
    let rtime = options.renewable.map(|r| now + r);
    if rtime.is_some() {
        kdc_options |= flags::RENEWABLE;
    }
    Ok(RequestBody {
        options: kdc_options,
        client,
        server,
        till: now + options.lifetime,
        rtime,
        nonce: nonce()?,
        etypes: enctypes.to_vec(),
    })
}

/// A ticket-granting ticket for `client`, proved with `secret`: the AS
/// exchange.
pub fn initial(
    config: &Config,
    client: &Principal,
    secret: &Secret<'_>,
    options: &Options,
) -> Result<Credential> {
    let kdcs = config.kdcs(&client.realm)?;
    let server = options.service.clone().unwrap_or_else(|| Principal::tgs(&client.realm));
    // Offer the types there is a key for, from a keytab.
    let enctypes: Vec<Enctype> = match secret {
        Secret::Password(_) => config.enctypes.clone(),
        Secret::Keys(keys) => config
            .enctypes
            .iter()
            .copied()
            .filter(|e| keys.iter().any(|(_, k)| k.enctype == *e))
            .collect(),
    };
    // The type to fall back on when the KDC names none of these.
    let Some(&fallback) = enctypes.first() else {
        return Err(Error::Other(format!(
            "the keytab holds no key of {client} of a type krb5.conf allows"
        )));
    };
    let (now, _) = time::now();
    let body = body_for(options, Some(client.clone()), server.clone(), &enctypes, now)?;
    let encoded = body.encode();

    // First without proof, which tells what proof the KDC wants.
    let answer = messages::decode_answer(
        &kdc::send(&kdcs, &messages::encode_request(true, &[], &encoded))?,
        true,
    )?;
    let (reply, key) = match answer {
        Answer::Reply(reply) => {
            let info = etype_info(&reply.padata)?;
            let key = client_key(client, secret, reply.enc_part.etype, &info)?;
            (reply, key)
        }
        Answer::Error(error) if error.code == codes::PREAUTH_REQUIRED => {
            let methods =
                error.data.as_deref().map(PaData::decode_list).transpose()?.unwrap_or_default();
            let info = etype_info(&methods)?;
            let chosen = info
                .iter()
                .find_map(|i| Enctype::from_number(i.etype).filter(|e| enctypes.contains(e)))
                .unwrap_or(fallback);
            let key = client_key(client, secret, chosen.number(), &info)?;
            let (now, usec) = time::now();
            let stamp = Encrypted::seal(
                &key,
                USAGE_TIMESTAMP,
                &messages::encode_timestamp(now, usec),
                None,
            )?;
            let padata = [PaData { kind: messages::PA_ENC_TIMESTAMP, value: stamp.encode() }];
            let request = messages::encode_request(true, &padata, &encoded);
            match messages::decode_answer(&kdc::send(&kdcs, &request)?, true)? {
                Answer::Reply(reply) => (reply, key),
                Answer::Error(error) => return Err(kdc_error(&error, client)),
            }
        }
        Answer::Error(error) => return Err(kdc_error(&error, client)),
    };
    let plain = reply.enc_part.open(&key, USAGE_AS_REP).map_err(|err| match err {
        Error::Integrity => Error::Other(format!(
            "the password or key of {client} is wrong: the KDC's reply does not open with it"
        )),
        other => other,
    })?;
    finish(*reply, &plain, body.nonce)
}

/// The salts and rounds of `padata`'s ETYPE-INFO2, if it holds one.
fn etype_info(padata: &[PaData]) -> Result<Vec<EtypeInfo>> {
    match padata.iter().find(|p| p.kind == messages::PA_ETYPE_INFO2) {
        Some(found) => EtypeInfo::decode_list(&found.value),
        None => Ok(Vec::new()),
    }
}

/// The client's key of type `etype`: made from the password with the
/// KDC's salt and rounds, or found among the keytab's.
fn client_key(
    client: &Principal,
    secret: &Secret<'_>,
    etype: i32,
    info: &[EtypeInfo],
) -> Result<Key> {
    let enctype = Enctype::from_number(etype).ok_or_else(|| {
        Error::Other(format!("the KDC chose encryption type {etype}, which Dukovo does not speak"))
    })?;
    match secret {
        Secret::Password(password) => {
            let entry = info.iter().find(|i| i.etype == etype);
            let salt = match entry.and_then(|i| i.salt.as_ref()) {
                Some(salt) => salt.as_bytes().to_vec(),
                None => client.salt(),
            };
            let params = entry.and_then(|i| i.params.as_deref());
            Key::from_password(enctype, password, &salt, params)
        }
        Secret::Keys(keys) => keys
            .iter()
            .find(|(_, key)| key.enctype == enctype)
            .map(|(_, key)| key.clone())
            .ok_or_else(|| {
                Error::Other(format!("the keytab holds no {} key of {client}", enctype.name()))
            }),
    }
}

/// The error a KDC's refusal makes, naming whom it was about.
fn kdc_error(error: &KdcError, about: &Principal) -> Error {
    Error::Kdc { code: error.code, text: format!("{about}: {}", error.explain()) }
}

/// The credential of a reply whose part opened as `plain`, once it
/// repeats the request's nonce.
fn finish(reply: Reply, plain: &[u8], nonce: u32) -> Result<Credential> {
    let part = ReplyPart::decode(plain)?;
    if part.nonce != nonce {
        return Err(Error::Other(
            "the KDC's reply does not repeat the request's nonce: it may be a replay; try again"
                .into(),
        ));
    }
    Ok(Credential {
        client: reply.client,
        server: part.server,
        key: part.key,
        authtime: part.authtime,
        starttime: part.starttime.unwrap_or(part.authtime),
        endtime: part.endtime,
        renew_till: part.renew_till.unwrap_or(0),
        flags: part.flags,
        ticket: reply.ticket.der,
    })
}

/// A ticket for `server`, from the ticket-granting ticket `tgt`: the TGS
/// exchange.
pub fn service(config: &Config, tgt: &Credential, server: &Principal) -> Result<Credential> {
    if server.realm != tgt.server.realm {
        return Err(Error::Other(format!(
            "{server} is in another realm than {}: Dukovo does not ask for tickets across realms",
            tgt.client
        )));
    }
    let kdcs = config.kdcs(&server.realm)?;
    let (now, usec) = time::now();
    let options = Options {
        lifetime: (tgt.endtime - now).max(60),
        renewable: None,
        forwardable: false,
        service: None,
    };
    let mut body = body_for(&options, None, server.clone(), &config.enctypes, now)?;
    // The service ticket ends with the TGT, which the KDC would cut it to.
    body.till = tgt.endtime;
    let encoded = body.encode();
    let checksum = tgt.key.checksum(USAGE_TGS_CHECKSUM, &encoded);
    let authenticator = Authenticator {
        client: tgt.client.clone(),
        checksum: Some((tgt.key.enctype.checksum_type(), checksum)),
        cusec: usec,
        ctime: now,
        subkey: None,
        seq_number: None,
    };
    let sealed = Encrypted::seal(&tgt.key, USAGE_TGS_AUTHENTICATOR, &authenticator.encode(), None)?;
    let ap_req = messages::encode_ap_request(0, &tgt.ticket, &sealed);
    let padata = [PaData { kind: messages::PA_TGS_REQ, value: ap_req }];
    let answer = messages::decode_answer(
        &kdc::send(&kdcs, &messages::encode_request(false, &padata, &encoded))?,
        false,
    )?;
    let reply = match answer {
        Answer::Reply(reply) => reply,
        Answer::Error(error) => return Err(kdc_error(&error, server)),
    };
    let plain = reply.enc_part.open(&tgt.key, USAGE_TGS_REP)?;
    finish(*reply, &plain, body.nonce)
}

/// A good ticket for `server`: the keyring's, or a new one from its
/// ticket-granting ticket, which is then kept. What the file systems
/// and GSS-API ask for.
pub fn ticket_for(
    config: &Config,
    keyring: &mut dyn Keyring,
    server: &Principal,
) -> Result<Credential> {
    let (now, _) = time::now();
    let client = creds::client(keyring)?.ok_or_else(|| {
        Error::Other("your keyring holds no Kerberos tickets: run kinit first".into())
    })?;
    if let Some(kept) = creds::find(keyring, &client, server)?
        && kept.valid_at(now)
    {
        return Ok(kept);
    }
    let tgs = Principal::tgs(&client.realm);
    let tgt =
        creds::find(keyring, &client, &tgs)?.filter(|tgt| tgt.valid_at(now)).ok_or_else(|| {
            Error::Other(format!("the tickets of {client} have expired: run kinit again"))
        })?;
    let fresh = service(config, &tgt, server)?;
    creds::add(keyring, &fresh)?;
    Ok(fresh)
}

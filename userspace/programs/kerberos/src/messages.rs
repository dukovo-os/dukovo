// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Kerberos's messages (RFC 4120 5): principals, tickets, the requests
//! to a KDC and its replies and errors, and the authenticator that proves
//! a ticket is its holder's. Each type writes itself with `encode` and
//! reads itself with `decode`, field by field as RFC 4120's ASN.1 has
//! them, so the two can be read side by side.
//!
//! Only what a client and a service need is here: no KDC.

use crate::crypto::{Enctype, Key};
use crate::der;
use crate::{Error, Result};

/// A principal's name type: a person (NT-PRINCIPAL).
pub const NT_PRINCIPAL: i32 = 1;
/// A principal's name type: a service and its instance, as `krbtgt`
/// (NT-SRV-INST).
pub const NT_SRV_INST: i32 = 2;
/// A principal's name type: a service on a host, as `nfs/server`
/// (NT-SRV-HST).
pub const NT_SRV_HST: i32 = 3;

/// A principal: a person or a service in a realm, as
/// `alice@EXAMPLE.ORG` or `nfs/server.example.org@EXAMPLE.ORG`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Principal {
    /// Its name type, one of the `NT_` numbers.
    pub name_type: i32,
    /// Its name's parts: `alice`, or `nfs` and the host.
    pub parts: Vec<String>,
    /// Its realm.
    pub realm: String,
}

impl Principal {
    /// The principal `text` names: `name[/instance...][@REALM]`, in the
    /// default realm when it names none. `\` escapes a `/`, `@` or `\`.
    pub fn parse(text: &str, default_realm: Option<&str>) -> Result<Principal> {
        // The parts before the one being read, which is `part`.
        let mut parts = Vec::new();
        let mut part = String::new();
        let mut realm: Option<String> = None;
        let mut chars = text.chars();
        while let Some(c) = chars.next() {
            let in_name = realm.is_none();
            let c = match c {
                '/' if in_name => {
                    parts.push(std::mem::take(&mut part));
                    continue;
                }
                '@' if in_name => {
                    realm = Some(String::new());
                    continue;
                }
                '\\' => chars.next().ok_or_else(|| bad_name(text))?,
                c => c,
            };
            match realm.as_mut() {
                Some(realm) => realm.push(c),
                None => part.push(c),
            }
        }
        parts.push(part);
        let realm = match realm {
            Some(realm) => realm,
            None => default_realm
                .ok_or_else(|| {
                    Error::Other(format!(
                        "{text} names no realm and krb5.conf gives no default_realm: write \
                         {text}@REALM, or set default_realm in krb5.conf's [libdefaults]"
                    ))
                })?
                .to_string(),
        };
        if realm.is_empty() || parts.iter().any(String::is_empty) {
            return Err(bad_name(text));
        }
        let name_type = if parts.len() > 1 { NT_SRV_HST } else { NT_PRINCIPAL };
        Ok(Principal { name_type, parts, realm })
    }

    /// The ticket-granting service of `realm`: `krbtgt/REALM@REALM`.
    pub fn tgs(realm: &str) -> Principal {
        Principal {
            name_type: NT_SRV_INST,
            parts: vec!["krbtgt".into(), realm.into()],
            realm: realm.into(),
        }
    }

    /// The service `service` on `host` in `realm`, as `nfs/host@REALM`.
    pub fn service(service: &str, host: &str, realm: &str) -> Principal {
        Principal {
            name_type: NT_SRV_HST,
            parts: vec![service.into(), host.to_ascii_lowercase()],
            realm: realm.into(),
        }
    }

    /// Whether it is a realm's ticket-granting service.
    pub fn is_tgs(&self) -> bool {
        self.parts.len() == 2 && self.parts[0] == "krbtgt"
    }

    /// The default salt of its keys (RFC 4120 4): the realm, then each
    /// part, run together.
    pub fn salt(&self) -> Vec<u8> {
        let mut salt = self.realm.as_bytes().to_vec();
        for part in &self.parts {
            salt.extend_from_slice(part.as_bytes());
        }
        salt
    }

    /// Whether `other` names the same principal, whatever its name type.
    pub fn same(&self, other: &Principal) -> bool {
        self.parts == other.parts && self.realm == other.realm
    }

    /// The PrincipalName of its name and parts, without the realm.
    pub fn encode_name(&self) -> Vec<u8> {
        let parts: Vec<Vec<u8>> = self.parts.iter().map(|part| der::string(part)).collect();
        der::sequence(&[
            der::field(0, &der::integer(self.name_type.into())),
            der::field(1, &der::sequence(&parts)),
        ])
    }

    /// The principal of the PrincipalName `value` in `realm`.
    pub fn decode_name(value: &[u8], realm: &str) -> Result<Principal> {
        let mut seq = der::read_sequence(value, "PrincipalName")?;
        let name_type = der::read_int32(seq.field(0)?, "name-type")?;
        let mut strings = der::read_sequence(seq.field(1)?, "name-string")?;
        let mut parts = Vec::new();
        while !strings.is_empty() {
            let (_, _, whole) = strings.any()?;
            parts.push(der::read_string(whole, "a name's part")?);
        }
        seq.skip_rest();
        Ok(Principal { name_type, parts, realm: realm.into() })
    }
}

impl std::fmt::Display for Principal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let escape =
            |text: &str| text.replace('\\', "\\\\").replace('/', "\\/").replace('@', "\\@");
        let parts: Vec<String> = self.parts.iter().map(|part| escape(part)).collect();
        write!(f, "{}@{}", parts.join("/"), self.realm.replace('\\', "\\\\").replace('@', "\\@"))
    }
}

/// An error for a principal's name that does not read.
fn bad_name(text: &str) -> Error {
    Error::Other(format!(
        "{text} is not a principal's name: write name@REALM, or service/host@REALM"
    ))
}

/// Ciphertext and its type (EncryptedData).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Encrypted {
    /// The encryption type's number.
    pub etype: i32,
    /// The key's version, where the key has versions, as a service's
    /// keys in a keytab do.
    pub kvno: Option<u32>,
    /// The ciphertext.
    pub cipher: Vec<u8>,
}

impl Encrypted {
    /// `plain` encrypted under `key` for `usage`.
    pub fn seal(key: &Key, usage: u32, plain: &[u8], kvno: Option<u32>) -> Result<Encrypted> {
        Ok(Encrypted { etype: key.enctype.number(), kvno, cipher: key.encrypt(usage, plain)? })
    }

    /// The plaintext, under `key` for `usage`.
    pub fn open(&self, key: &Key, usage: u32) -> Result<Vec<u8>> {
        if self.etype != key.enctype.number() {
            return Err(Error::Other(format!(
                "a message encrypted with type {}, where the key is {}",
                self.etype,
                key.enctype.name()
            )));
        }
        key.decrypt(usage, &self.cipher)
    }

    /// The EncryptedData.
    pub fn encode(&self) -> Vec<u8> {
        let mut fields = vec![der::field(0, &der::integer(self.etype.into()))];
        if let Some(kvno) = self.kvno {
            fields.push(der::field(1, &der::integer(kvno.into())));
        }
        fields.push(der::field(2, &der::octets(&self.cipher)));
        der::sequence(&fields)
    }

    /// The EncryptedData `value`.
    pub fn decode(value: &[u8]) -> Result<Encrypted> {
        let mut seq = der::read_sequence(value, "EncryptedData")?;
        let etype = der::read_int32(seq.field(0)?, "etype")?;
        let kvno = seq.optional(1)?.map(|v| der::read_uint32(v, "kvno")).transpose()?;
        let cipher = der::read_octets(seq.field(2)?, "cipher")?.to_vec();
        Ok(Encrypted { etype, kvno, cipher })
    }
}

/// An EncryptionKey: a key and its type's number.
pub fn encode_key(key: &Key) -> Vec<u8> {
    der::sequence(&[
        der::field(0, &der::integer(key.enctype.number().into())),
        der::field(1, &der::octets(key.bytes())),
    ])
}

/// The key of the EncryptionKey `value`, of a type Dukovo speaks.
pub fn decode_key(value: &[u8]) -> Result<Key> {
    let mut seq = der::read_sequence(value, "EncryptionKey")?;
    let number = der::read_int32(seq.field(0)?, "keytype")?;
    let bytes = der::read_octets(seq.field(1)?, "keyvalue")?;
    let enctype = Enctype::from_number(number).ok_or_else(|| {
        Error::Other(format!("a key of encryption type {number}, which Dukovo does not speak"))
    })?;
    Key::new(enctype, bytes)
}

/// A ticket (RFC 4120 5.3): its service, and its part encrypted under
/// the service's key, which a client keeps whole to send again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ticket {
    /// The service's realm.
    pub realm: String,
    /// The service.
    pub server: Principal,
    /// The part the service opens.
    pub enc_part: Encrypted,
    /// The whole Ticket, as the KDC sent it.
    pub der: Vec<u8>,
}

impl Ticket {
    /// The Ticket `value`.
    pub fn decode(value: &[u8]) -> Result<Ticket> {
        let inner = der::read_application(value, 1, "Ticket")?;
        let mut seq = der::read_sequence(inner, "Ticket")?;
        let vno = der::read_integer(seq.field(0)?, "tkt-vno")?;
        if vno != 5 {
            return Err(Error::Malformed(format!("a ticket of Kerberos version {vno}")));
        }
        let realm = der::read_string(seq.field(1)?, "realm")?;
        let server = Principal::decode_name(seq.field(2)?, &realm)?;
        let enc_part = Encrypted::decode(seq.field(3)?)?;
        Ok(Ticket { realm, server, enc_part, der: value.to_vec() })
    }
}

/// A piece of pre-authentication data (PA-DATA): its type and value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaData {
    /// Its type: one of the `PA_` numbers.
    pub kind: i32,
    /// Its value.
    pub value: Vec<u8>,
}

/// PA-DATA: a TGS request's AP-REQ (PA-TGS-REQ).
pub const PA_TGS_REQ: i32 = 1;
/// PA-DATA: the time encrypted under the client's key (PA-ENC-TIMESTAMP).
pub const PA_ENC_TIMESTAMP: i32 = 2;
/// PA-DATA: the client's keys' salts and rounds (PA-ETYPE-INFO2).
pub const PA_ETYPE_INFO2: i32 = 19;

impl PaData {
    /// The PA-DATA.
    pub fn encode(&self) -> Vec<u8> {
        der::sequence(&[
            der::field(1, &der::integer(self.kind.into())),
            der::field(2, &der::octets(&self.value)),
        ])
    }

    /// The SEQUENCE OF PA-DATA `value`.
    pub fn decode_list(value: &[u8]) -> Result<Vec<PaData>> {
        let mut list = der::read_sequence(value, "PA-DATA list")?;
        let mut out = Vec::new();
        while !list.is_empty() {
            let (_, _, whole) = list.any()?;
            let mut seq = der::read_sequence(whole, "PA-DATA")?;
            let kind = der::read_int32(seq.field(1)?, "padata-type")?;
            let value = der::read_octets(seq.field(2)?, "padata-value")?.to_vec();
            out.push(PaData { kind, value });
        }
        Ok(out)
    }
}

/// How a client's key is made from its password, for one type
/// (ETYPE-INFO2-ENTRY).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EtypeInfo {
    /// The encryption type's number.
    pub etype: i32,
    /// The salt, when it is not the principal's default.
    pub salt: Option<String>,
    /// The rounds, as 4 bytes, when they are not the type's default.
    pub params: Option<Vec<u8>>,
}

impl EtypeInfo {
    /// The ETYPE-INFO2 `value`, the KDC's first choice first.
    pub fn decode_list(value: &[u8]) -> Result<Vec<EtypeInfo>> {
        let mut list = der::read_sequence(value, "ETYPE-INFO2")?;
        let mut out = Vec::new();
        while !list.is_empty() {
            let (_, _, whole) = list.any()?;
            let mut seq = der::read_sequence(whole, "ETYPE-INFO2-ENTRY")?;
            let etype = der::read_int32(seq.field(0)?, "etype")?;
            let salt = seq.optional(1)?.map(|v| der::read_string(v, "salt")).transpose()?;
            let params = seq.optional(2)?.map(|v| der::read_octets(v, "s2kparams")).transpose()?;
            out.push(EtypeInfo { etype, salt, params: params.map(<[u8]>::to_vec) });
        }
        Ok(out)
    }
}

/// KDCOptions and TicketFlags bits, numbered from the top as RFC 4120
/// numbers them.
pub mod flags {
    /// The bit numbered `n`.
    const fn bit(n: u32) -> u32 {
        1 << (31 - n)
    }
    /// A ticket that may be forwarded to another host.
    pub const FORWARDABLE: u32 = bit(1);
    /// A ticket that was forwarded.
    pub const FORWARDED: u32 = bit(2);
    /// A ticket that may be proxied.
    pub const PROXIABLE: u32 = bit(3);
    /// A ticket that may be renewed until its renew-till.
    pub const RENEWABLE: u32 = bit(8);
    /// A ticket from an AS exchange, not a TGS one.
    pub const INITIAL: u32 = bit(9);
    /// A ticket whose client proved itself before the KDC replied.
    pub const PRE_AUTHENT: u32 = bit(10);
    /// The request may have its names changed to their canonical ones.
    pub const CANONICALIZE: u32 = bit(15);
    /// The request takes a renewable ticket if one for the lifetime
    /// asked cannot be had.
    pub const RENEWABLE_OK: u32 = bit(27);

    /// The flags' letters, as klist writes them: `F` forwardable, `f`
    /// forwarded, `P` proxiable, `R` renewable, `I` initial, `A`
    /// pre-authenticated.
    pub fn letters(flags: u32) -> String {
        [
            (FORWARDABLE, 'F'),
            (FORWARDED, 'f'),
            (PROXIABLE, 'P'),
            (RENEWABLE, 'R'),
            (INITIAL, 'I'),
            (PRE_AUTHENT, 'A'),
        ]
        .iter()
        .filter(|(bit, _)| flags & bit != 0)
        .map(|&(_, letter)| letter)
        .collect()
    }
}

/// The body of a request to a KDC (KDC-REQ-BODY).
#[derive(Clone, Debug)]
pub struct RequestBody {
    /// KDCOptions.
    pub options: u32,
    /// The client, in an AS request.
    pub client: Option<Principal>,
    /// The service asked for, whose realm is the request's.
    pub server: Principal,
    /// The end asked for, in seconds since 1970.
    pub till: i64,
    /// The renewable end asked for, if any.
    pub rtime: Option<i64>,
    /// A number the reply must repeat.
    pub nonce: u32,
    /// The encryption types the client takes, its first choice first.
    pub etypes: Vec<Enctype>,
}

impl RequestBody {
    /// The KDC-REQ-BODY.
    pub fn encode(&self) -> Vec<u8> {
        let mut fields = vec![der::field(0, &der::bits(self.options))];
        if let Some(client) = &self.client {
            fields.push(der::field(1, &client.encode_name()));
        }
        fields.push(der::field(2, &der::string(&self.server.realm)));
        fields.push(der::field(3, &self.server.encode_name()));
        fields.push(der::field(5, &der::kerberos_time(self.till)));
        if let Some(rtime) = self.rtime {
            fields.push(der::field(6, &der::kerberos_time(rtime)));
        }
        fields.push(der::field(7, &der::integer(self.nonce.into())));
        let etypes: Vec<Vec<u8>> =
            self.etypes.iter().map(|e| der::integer(e.number().into())).collect();
        fields.push(der::field(8, &der::sequence(&etypes)));
        der::sequence(&fields)
    }
}

/// An AS-REQ (`as_request` true) or TGS-REQ: `padata`, then the body,
/// already encoded, since a TGS request's checksum covers its bytes.
pub fn encode_request(as_request: bool, padata: &[PaData], body: &[u8]) -> Vec<u8> {
    let (msg_type, tag) = if as_request { (10, 10) } else { (12, 12) };
    let mut fields = vec![der::field(1, &der::integer(5)), der::field(2, &der::integer(msg_type))];
    if !padata.is_empty() {
        let list: Vec<Vec<u8>> = padata.iter().map(PaData::encode).collect();
        fields.push(der::field(3, &der::sequence(&list)));
    }
    fields.push(der::field(4, body));
    der::application(tag, &der::sequence(&fields))
}

/// A KDC's reply (KDC-REP), before its encrypted part is opened.
#[derive(Clone, Debug)]
pub struct Reply {
    /// Pre-authentication data, as the salt the client's key used.
    pub padata: Vec<PaData>,
    /// The client, as the KDC names it.
    pub client: Principal,
    /// The ticket.
    pub ticket: Ticket,
    /// The part for the client, under its key or session key.
    pub enc_part: Encrypted,
}

/// The part of a reply for the client (EncKDCRepPart).
#[derive(Debug)]
pub struct ReplyPart {
    /// The session key, which the ticket holds too.
    pub key: Key,
    /// The request's nonce, repeated.
    pub nonce: u32,
    /// TicketFlags.
    pub flags: u32,
    /// When the client proved itself, in seconds since 1970.
    pub authtime: i64,
    /// When the ticket starts, if not at `authtime`.
    pub starttime: Option<i64>,
    /// When the ticket ends.
    pub endtime: i64,
    /// Until when it may be renewed, if it may.
    pub renew_till: Option<i64>,
    /// The service, as the KDC names it.
    pub server: Principal,
}

/// The message types of an AS-REP and a TGS-REP.
const AS_REP: u8 = 11;
/// A TGS-REP's.
const TGS_REP: u8 = 13;
/// A KRB-ERROR's.
const KRB_ERROR: u8 = 30;
/// EncTicketPart's application tag.
const ENC_TICKET_PART: u8 = 3;
/// AP-REQ's application tag and message type.
const AP_REQ: u8 = 14;
/// AP-REP's.
const AP_REP: u8 = 15;
/// EncAPRepPart's application tag.
const ENC_AP_REP_PART: u8 = 27;

/// What a KDC answered: a reply, or an error.
#[derive(Debug)]
pub enum Answer {
    /// A reply, boxed: it is far larger than an error.
    Reply(Box<Reply>),
    /// An error.
    Error(KdcError),
}

/// The KDC's answer `value` to an AS (`as_request` true) or TGS request.
pub fn decode_answer(value: &[u8], as_request: bool) -> Result<Answer> {
    let expected = if as_request { AS_REP } else { TGS_REP };
    match value.first().map(|tag| tag & 0x1f) {
        Some(KRB_ERROR) => KdcError::decode(value).map(Answer::Error),
        Some(tag) if tag == expected => {
            decode_reply(value, expected).map(|reply| Answer::Reply(Box::new(reply)))
        }
        _ => Err(Error::Malformed("the KDC answered with neither a reply nor an error".into())),
    }
}

/// The KDC-REP `value` under `[APPLICATION tag]`.
fn decode_reply(value: &[u8], tag: u8) -> Result<Reply> {
    let inner = der::read_application(value, tag, "KDC-REP")?;
    let mut seq = der::read_sequence(inner, "KDC-REP")?;
    let _pvno = seq.field(0)?;
    let _msg_type = seq.field(1)?;
    let padata = seq.optional(2)?.map(PaData::decode_list).transpose()?.unwrap_or_default();
    let realm = der::read_string(seq.field(3)?, "crealm")?;
    let client = Principal::decode_name(seq.field(4)?, &realm)?;
    let ticket = Ticket::decode(seq.field(5)?)?;
    let enc_part = Encrypted::decode(seq.field(6)?)?;
    Ok(Reply { padata, client, ticket, enc_part })
}

impl ReplyPart {
    /// The EncKDCRepPart `plain`, opened from a reply. RFC 4120 lets a
    /// KDC tag an AS reply's part as a TGS reply's, as some do.
    pub fn decode(plain: &[u8]) -> Result<ReplyPart> {
        let tag = plain.first().map(|tag| tag & 0x1f);
        let number = match tag {
            Some(25) => 25,
            Some(26) => 26,
            _ => return Err(Error::Malformed("the reply's part is not an EncKDCRepPart".into())),
        };
        let inner = der::read_application(plain, number, "EncKDCRepPart")?;
        let mut seq = der::read_sequence(inner, "EncKDCRepPart")?;
        let key = decode_key(seq.field(0)?)?;
        let _last_req = seq.field(1)?;
        let nonce = der::read_uint32(seq.field(2)?, "nonce")?;
        let _key_expiration = seq.optional(3)?;
        let flags = der::read_bits(seq.field(4)?, "flags")?;
        let authtime = der::read_time(seq.field(5)?, "authtime")?;
        let starttime = seq.optional(6)?.map(|v| der::read_time(v, "starttime")).transpose()?;
        let endtime = der::read_time(seq.field(7)?, "endtime")?;
        let renew_till = seq.optional(8)?.map(|v| der::read_time(v, "renew-till")).transpose()?;
        let realm = der::read_string(seq.field(9)?, "srealm")?;
        let server = Principal::decode_name(seq.field(10)?, &realm)?;
        seq.skip_rest();
        Ok(ReplyPart { key, nonce, flags, authtime, starttime, endtime, renew_till, server })
    }
}

/// A KDC's error (KRB-ERROR).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KdcError {
    /// Its code: one of RFC 4120 7.5.9's.
    pub code: i32,
    /// The KDC's own words, if it sent any.
    pub text: Option<String>,
    /// Data for the client, as a PREAUTH_REQUIRED's METHOD-DATA.
    pub data: Option<Vec<u8>>,
}

/// Errors' codes that the client acts on.
pub mod codes {
    /// The KDC knows no such client.
    pub const C_PRINCIPAL_UNKNOWN: i32 = 6;
    /// The KDC knows no such service.
    pub const S_PRINCIPAL_UNKNOWN: i32 = 7;
    /// The KDC takes none of the encryption types offered.
    pub const ETYPE_NOSUPP: i32 = 14;
    /// The client's account is locked or disabled.
    pub const CLIENT_REVOKED: i32 = 18;
    /// The client's password has expired.
    pub const KEY_EXPIRED: i32 = 23;
    /// The pre-authentication failed: the wrong password.
    pub const PREAUTH_FAILED: i32 = 24;
    /// The KDC wants pre-authentication first.
    pub const PREAUTH_REQUIRED: i32 = 25;
    /// A message's integrity check failed: the wrong key.
    pub const BAD_INTEGRITY: i32 = 31;
    /// The ticket has expired.
    pub const TKT_EXPIRED: i32 = 32;
    /// The clocks differ by more than the KDC allows.
    pub const SKEW: i32 = 37;
    /// The reply is too big for UDP: ask again over TCP.
    pub const RESPONSE_TOO_BIG: i32 = 52;
}

impl KdcError {
    /// The KRB-ERROR `value`.
    pub fn decode(value: &[u8]) -> Result<KdcError> {
        let inner = der::read_application(value, KRB_ERROR, "KRB-ERROR")?;
        let mut seq = der::read_sequence(inner, "KRB-ERROR")?;
        let _pvno = seq.field(0)?;
        let _msg_type = seq.field(1)?;
        let _ctime = seq.optional(2)?;
        let _cusec = seq.optional(3)?;
        let _stime = seq.field(4)?;
        let _susec = seq.field(5)?;
        let code = der::read_int32(seq.field(6)?, "error-code")?;
        let _crealm = seq.optional(7)?;
        let _cname = seq.optional(8)?;
        let _realm = seq.field(9)?;
        let _sname = seq.field(10)?;
        let text = seq.optional(11)?.map(|v| der::read_string(v, "e-text")).transpose()?;
        let data = seq.optional(12)?.map(|v| der::read_octets(v, "e-data")).transpose()?;
        Ok(KdcError { code, text, data: data.map(<[u8]>::to_vec) })
    }

    /// What the error means, and what to do about it.
    pub fn explain(&self) -> String {
        let said = self.text.as_deref().filter(|text| !text.is_empty());
        let meaning = match self.code {
            codes::C_PRINCIPAL_UNKNOWN => {
                "the KDC knows no such principal: check the name and realm"
            }
            codes::S_PRINCIPAL_UNKNOWN => {
                "the KDC knows no such service: its administrator adds it with kadmin's addprinc"
            }
            codes::ETYPE_NOSUPP => {
                "the KDC takes none of the AES encryption types Dukovo offers: its administrator \
                 gives the principal AES keys"
            }
            codes::CLIENT_REVOKED => "the account is locked or disabled at the KDC",
            codes::KEY_EXPIRED => {
                "the password has expired: change it with kpasswd on a host that has it"
            }
            codes::PREAUTH_FAILED | codes::BAD_INTEGRITY => "the password or key is wrong",
            codes::TKT_EXPIRED => "the ticket has expired: run kinit again",
            codes::SKEW => {
                "this computer's clock is more than five minutes from the KDC's: `timedatectl` \
                 shows it, and timed sets it"
            }
            _ => "",
        };
        match (meaning.is_empty(), said) {
            (false, _) => meaning.to_string(),
            (true, Some(said)) => format!("the KDC said: {said} (error {})", self.code),
            (true, None) => format!("the KDC refused with error {}", self.code),
        }
    }
}

/// An authenticator (RFC 4120 5.5.1), which proves to a service that the
/// client holds a ticket's session key.
#[derive(Clone, Debug)]
pub struct Authenticator {
    /// The client.
    pub client: Principal,
    /// A checksum of what the authenticator vouches for: a TGS request's
    /// body, or GSS-API's channel bindings and flags.
    pub checksum: Option<(i32, Vec<u8>)>,
    /// The microseconds of the client's time.
    pub cusec: u32,
    /// The client's time, in seconds since 1970.
    pub ctime: i64,
    /// A key for what follows, if the client chose one.
    pub subkey: Option<Key>,
    /// The first sequence number of what follows, if any.
    pub seq_number: Option<u32>,
}

impl Authenticator {
    /// The Authenticator.
    pub fn encode(&self) -> Vec<u8> {
        let mut fields = vec![
            der::field(0, &der::integer(5)),
            der::field(1, &der::string(&self.client.realm)),
            der::field(2, &self.client.encode_name()),
        ];
        if let Some((kind, sum)) = &self.checksum {
            fields.push(der::field(3, &encode_checksum(*kind, sum)));
        }
        fields.push(der::field(4, &der::integer(self.cusec.into())));
        fields.push(der::field(5, &der::kerberos_time(self.ctime)));
        if let Some(subkey) = &self.subkey {
            fields.push(der::field(6, &encode_key(subkey)));
        }
        if let Some(seq) = self.seq_number {
            fields.push(der::field(7, &der::integer(seq.into())));
        }
        der::application(2, &der::sequence(&fields))
    }

    /// The Authenticator `plain`, opened from an AP-REQ.
    pub fn decode(plain: &[u8]) -> Result<Authenticator> {
        let inner = der::read_application(plain, 2, "Authenticator")?;
        let mut seq = der::read_sequence(inner, "Authenticator")?;
        let _vno = seq.field(0)?;
        let realm = der::read_string(seq.field(1)?, "crealm")?;
        let client = Principal::decode_name(seq.field(2)?, &realm)?;
        let checksum = seq.optional(3)?.map(decode_checksum).transpose()?;
        let cusec = der::read_uint32(seq.field(4)?, "cusec")?;
        let ctime = der::read_time(seq.field(5)?, "ctime")?;
        let subkey = seq.optional(6)?.map(decode_key).transpose()?;
        let seq_number = seq.optional(7)?.map(|v| der::read_uint32(v, "seq-number")).transpose()?;
        seq.skip_rest();
        Ok(Authenticator { client, checksum, cusec, ctime, subkey, seq_number })
    }
}

/// A Checksum: its type and bytes.
pub fn encode_checksum(kind: i32, sum: &[u8]) -> Vec<u8> {
    der::sequence(&[der::field(0, &der::integer(kind.into())), der::field(1, &der::octets(sum))])
}

/// The Checksum `value`.
fn decode_checksum(value: &[u8]) -> Result<(i32, Vec<u8>)> {
    let mut seq = der::read_sequence(value, "Checksum")?;
    let kind = der::read_int32(seq.field(0)?, "cksumtype")?;
    let sum = der::read_octets(seq.field(1)?, "checksum")?.to_vec();
    Ok((kind, sum))
}

/// An AP-REQ (RFC 4120 5.5.1): `ticket`, and the authenticator sealed
/// under its session key. `options` are APOptions, as mutual-required.
pub fn encode_ap_request(options: u32, ticket: &[u8], authenticator: &Encrypted) -> Vec<u8> {
    der::application(
        AP_REQ,
        &der::sequence(&[
            der::field(0, &der::integer(5)),
            der::field(1, &der::integer(AP_REQ.into())),
            der::field(2, &der::bits(options)),
            der::field(3, ticket),
            der::field(4, &authenticator.encode()),
        ]),
    )
}

/// APOptions' mutual-required bit: the client asks the service to prove
/// itself with an AP-REP.
pub const AP_MUTUAL_REQUIRED: u32 = 1 << (31 - 2);

/// An AP-REQ, read (RFC 4120 5.5.1): what a service gets from a client.
#[derive(Clone, Debug)]
pub struct ApRequest {
    /// APOptions, as [`AP_MUTUAL_REQUIRED`].
    pub options: u32,
    /// The ticket, which the service opens with its own key.
    pub ticket: Ticket,
    /// The authenticator, under the ticket's session key.
    pub authenticator: Encrypted,
}

impl ApRequest {
    /// The AP-REQ `value`.
    pub fn decode(value: &[u8]) -> Result<ApRequest> {
        let inner = der::read_application(value, AP_REQ, "AP-REQ")?;
        let mut seq = der::read_sequence(inner, "AP-REQ")?;
        let _pvno = seq.field(0)?;
        let kind = der::read_integer(seq.field(1)?, "msg-type")?;
        if kind != i64::from(AP_REQ) {
            return Err(Error::Malformed(format!("an AP-REQ of message type {kind}")));
        }
        let options = der::read_bits(seq.field(2)?, "ap-options")?;
        let ticket = Ticket::decode(seq.field(3)?)?;
        let authenticator = Encrypted::decode(seq.field(4)?)?;
        Ok(ApRequest { options, ticket, authenticator })
    }
}

/// The part of a ticket only its service opens (EncTicketPart, RFC 4120
/// 5.3): who the client is, the session key, and when it is good.
#[derive(Clone, Debug)]
pub struct TicketPart {
    /// TicketFlags.
    pub flags: u32,
    /// The session key the client was given with the ticket.
    pub key: Key,
    /// The client.
    pub client: Principal,
    /// When the client proved itself, in seconds since 1970.
    pub authtime: i64,
    /// When the ticket starts, if not at `authtime`.
    pub starttime: Option<i64>,
    /// When it ends.
    pub endtime: i64,
}

impl TicketPart {
    /// The EncTicketPart `plain`, opened from a ticket.
    pub fn decode(plain: &[u8]) -> Result<TicketPart> {
        let inner = der::read_application(plain, ENC_TICKET_PART, "EncTicketPart")?;
        let mut seq = der::read_sequence(inner, "EncTicketPart")?;
        let flags = der::read_bits(seq.field(0)?, "flags")?;
        let key = decode_key(seq.field(1)?)?;
        let realm = der::read_string(seq.field(2)?, "crealm")?;
        let client = Principal::decode_name(seq.field(3)?, &realm)?;
        let _transited = seq.field(4)?;
        let authtime = der::read_time(seq.field(5)?, "authtime")?;
        let starttime = seq.optional(6)?.map(|v| der::read_time(v, "starttime")).transpose()?;
        let endtime = der::read_time(seq.field(7)?, "endtime")?;
        // renew-till, the client's addresses and authorization data,
        // which a service that maps principals to users does not need.
        seq.skip_rest();
        Ok(TicketPart { flags, key, client, authtime, starttime, endtime })
    }

    /// The EncTicketPart, as a KDC writes it: for tests, which play the
    /// KDC.
    #[cfg(test)]
    pub(crate) fn encode(&self) -> Vec<u8> {
        let transited =
            der::sequence(&[der::field(0, &der::integer(1)), der::field(1, &der::octets(b""))]);
        let mut fields = vec![
            der::field(0, &der::bits(self.flags)),
            der::field(1, &encode_key(&self.key)),
            der::field(2, &der::string(&self.client.realm)),
            der::field(3, &self.client.encode_name()),
            der::field(4, &transited),
            der::field(5, &der::kerberos_time(self.authtime)),
        ];
        if let Some(start) = self.starttime {
            fields.push(der::field(6, &der::kerberos_time(start)));
        }
        fields.push(der::field(7, &der::kerberos_time(self.endtime)));
        der::application(ENC_TICKET_PART, &der::sequence(&fields))
    }
}

/// A Ticket for `server`, its part sealed under `key`: for tests, which
/// play the KDC.
#[cfg(test)]
pub(crate) fn encode_ticket(
    server: &Principal,
    key: &Key,
    kvno: u32,
    part: &TicketPart,
) -> Vec<u8> {
    const USAGE_TICKET: u32 = 2;
    let sealed = Encrypted::seal(key, USAGE_TICKET, &part.encode(), Some(kvno)).unwrap();
    der::application(
        1,
        &der::sequence(&[
            der::field(0, &der::integer(5)),
            der::field(1, &der::string(&server.realm)),
            der::field(2, &server.encode_name()),
            der::field(3, &sealed.encode()),
        ]),
    )
}

/// The part of an AP-REP its client opens (EncAPRepPart, RFC 4120
/// 5.5.2): the client's own time back, which proves the service opened
/// the authenticator, and the service's choice of key and sequence.
#[derive(Clone, Debug)]
pub struct ApReplyPart {
    /// The authenticator's time, in seconds since 1970.
    pub ctime: i64,
    /// Its microseconds.
    pub cusec: u32,
    /// The service's key for what follows, if it chose one.
    pub subkey: Option<Key>,
    /// The service's first sequence number, if any.
    pub seq_number: Option<u32>,
}

impl ApReplyPart {
    /// The whole AP-REP, with this part sealed under `key` for `usage`.
    pub fn seal(&self, key: &Key, usage: u32) -> Result<Vec<u8>> {
        let mut fields = vec![
            der::field(0, &der::kerberos_time(self.ctime)),
            der::field(1, &der::integer(self.cusec.into())),
        ];
        if let Some(subkey) = &self.subkey {
            fields.push(der::field(2, &encode_key(subkey)));
        }
        if let Some(seq) = self.seq_number {
            fields.push(der::field(3, &der::integer(seq.into())));
        }
        let part = der::application(ENC_AP_REP_PART, &der::sequence(&fields));
        let sealed = Encrypted::seal(key, usage, &part, None)?;
        Ok(der::application(
            AP_REP,
            &der::sequence(&[
                der::field(0, &der::integer(5)),
                der::field(1, &der::integer(AP_REP.into())),
                der::field(2, &sealed.encode()),
            ]),
        ))
    }

    /// The part of the AP-REP `value`, opened with `key` for `usage`.
    pub fn open(value: &[u8], key: &Key, usage: u32) -> Result<ApReplyPart> {
        let inner = der::read_application(value, AP_REP, "AP-REP")?;
        let mut seq = der::read_sequence(inner, "AP-REP")?;
        let _pvno = seq.field(0)?;
        let _msg_type = seq.field(1)?;
        let plain = Encrypted::decode(seq.field(2)?)?.open(key, usage)?;
        let inner = der::read_application(&plain, ENC_AP_REP_PART, "EncAPRepPart")?;
        let mut seq = der::read_sequence(inner, "EncAPRepPart")?;
        let ctime = der::read_time(seq.field(0)?, "ctime")?;
        let cusec = der::read_uint32(seq.field(1)?, "cusec")?;
        let subkey = seq.optional(2)?.map(decode_key).transpose()?;
        let seq_number = seq.optional(3)?.map(|v| der::read_uint32(v, "seq-number")).transpose()?;
        Ok(ApReplyPart { ctime, cusec, subkey, seq_number })
    }
}

/// The PA-ENC-TS-ENC of `seconds` and `usec`, the time a client proves
/// it knows its key with.
pub fn encode_timestamp(seconds: i64, usec: u32) -> Vec<u8> {
    der::sequence(&[
        der::field(0, &der::kerberos_time(seconds)),
        der::field(1, &der::integer(usec.into())),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn principals_read_and_write_as_kerberos_names_them() {
        let alice = Principal::parse("alice", Some("EXAMPLE.ORG")).unwrap();
        assert_eq!(alice.to_string(), "alice@EXAMPLE.ORG");
        assert_eq!(alice.name_type, NT_PRINCIPAL);
        let nfs = Principal::parse("nfs/server.example.org@EXAMPLE.ORG", None).unwrap();
        assert_eq!(nfs.parts, ["nfs", "server.example.org"]);
        assert_eq!(nfs.name_type, NT_SRV_HST);
        assert_eq!(nfs.salt(), b"EXAMPLE.ORGnfsserver.example.org");
        let odd = Principal::parse(r"a\/b\@c@R", None).unwrap();
        assert_eq!(odd.parts, ["a/b@c"]);
        assert_eq!(odd.to_string(), r"a\/b\@c@R");
        assert!(Principal::parse("alice", None).is_err());
        assert!(Principal::parse("alice@", None).is_err());
        assert!(Principal::parse("/x@R", None).is_err());
        assert!(Principal::tgs("EXAMPLE.ORG").is_tgs());
    }

    #[test]
    fn principal_names_read_back() {
        let nfs = Principal::service("nfs", "Server.Example.Org", "EXAMPLE.ORG");
        assert_eq!(nfs.parts[1], "server.example.org");
        let back = Principal::decode_name(&nfs.encode_name(), "EXAMPLE.ORG").unwrap();
        assert_eq!(back, nfs);
    }

    #[test]
    fn encrypted_data_and_keys_read_back() {
        let key = Key::random(Enctype::Aes256Sha1).unwrap();
        let sealed = Encrypted::seal(&key, 1, b"time", Some(3)).unwrap();
        let back = Encrypted::decode(&sealed.encode()).unwrap();
        assert_eq!(back, sealed);
        assert_eq!(back.open(&key, 1).unwrap(), b"time");
        assert_eq!(decode_key(&encode_key(&key)).unwrap(), key);
    }

    #[test]
    fn authenticators_read_back() {
        let authenticator = Authenticator {
            client: Principal::parse("alice@EXAMPLE.ORG", None).unwrap(),
            checksum: Some((16, vec![1; 12])),
            cusec: 999_999,
            ctime: 1_762_400_000,
            subkey: Some(Key::random(Enctype::Aes128Sha256).unwrap()),
            seq_number: Some(0x7fff_ffff),
        };
        let back = Authenticator::decode(&authenticator.encode()).unwrap();
        assert_eq!(back.client, authenticator.client);
        assert_eq!(back.checksum, authenticator.checksum);
        assert_eq!((back.cusec, back.ctime), (999_999, 1_762_400_000));
        assert_eq!(back.subkey, authenticator.subkey);
        assert_eq!(back.seq_number, Some(0x7fff_ffff));
    }

    #[test]
    fn flags_read_as_klist_writes_them() {
        let f = flags::FORWARDABLE | flags::RENEWABLE | flags::INITIAL | flags::PRE_AUTHENT;
        assert_eq!(flags::letters(f), "FRIA");
    }
}

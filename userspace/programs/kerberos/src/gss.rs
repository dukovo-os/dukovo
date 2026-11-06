// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! GSS-API's Kerberos mechanism (RFC 4121, with RFC 2743's token
//! framing): how a client and a service set up a security context from a
//! service ticket, and the tokens that sign or seal what each sends the
//! other after.
//!
//! It is what RPCSEC_GSS (RFC 2203) runs on, so what `sec=krb5`,
//! `sec=krb5i` and `sec=krb5p` mean for NFS:
//!
//! - the client, the initiator, sends [`Initiator::start`]'s token and
//!   reads the service's answer with [`Initiator::finish`];
//! - the service, the acceptor, answers with [`accept`], which opens the
//!   ticket with a key from its keytab;
//! - each then signs with [`Context::get_mic`] and checks with
//!   [`Context::verify_mic`] (krb5 and krb5i), or seals with
//!   [`Context::wrap`] and opens with [`Context::unwrap`] (krb5p).
//!
//! Only the AES encryption types are spoken (RFC 4121's "CFX" tokens), as
//! in the rest of the crate; RFC 1964's DES and RC4 tokens are not.
//!
//! Debugging: a context that will not set up is nearly always a key that
//! does not match. The service's keytab holds another version of its key
//! than the KDC's (`klist -k` against `kvno`), or the ticket names a host
//! other than the one the service runs on. A token that fails its check
//! after setup is a key-choice mismatch: both sides must agree on whether
//! the acceptor chose a subkey ([`Context::uses_acceptor_subkey`]).

use crate::creds::Credential;
use crate::crypto::{self, Key};
use crate::keytab;
use crate::messages::{
    AP_MUTUAL_REQUIRED, ApReplyPart, ApRequest, Authenticator, Encrypted, KdcError, Principal,
    TicketPart, encode_ap_request,
};
use crate::{Error, Result, der, time};

/// The Kerberos 5 mechanism's object identifier, 1.2.840.113554.1.2.2,
/// in DER.
pub const KRB5_OID: [u8; 9] = [0x2a, 0x86, 0x48, 0x86, 0xf7, 0x12, 0x01, 0x02, 0x02];

/// GSS-API's context flags (RFC 2744), as the authenticator's checksum
/// carries them.
pub mod flags {
    /// The client's credentials may be passed on.
    pub const DELEG: u32 = 1;
    /// The service proves itself to the client too.
    pub const MUTUAL: u32 = 2;
    /// Replayed tokens are detected.
    pub const REPLAY: u32 = 4;
    /// Tokens out of order are detected.
    pub const SEQUENCE: u32 = 8;
    /// Tokens may be sealed.
    pub const CONF: u32 = 16;
    /// Tokens may be signed.
    pub const INTEG: u32 = 32;
}

/// How far apart the two sides' clocks may be, as krb5.conf's default
/// `clockskew`: five minutes.
pub const CLOCK_SKEW: i64 = 300;

/// The token IDs of RFC 4121 4.1 and 4.2.6.
const TOK_AP_REQ: [u8; 2] = [0x01, 0x00];
const TOK_AP_REP: [u8; 2] = [0x02, 0x00];
const TOK_ERROR: [u8; 2] = [0x03, 0x00];
const TOK_MIC: [u8; 2] = [0x04, 0x04];
const TOK_WRAP: [u8; 2] = [0x05, 0x04];

/// The checksum type of GSS-API's authenticator checksum (RFC 4121
/// 4.1.1), which carries channel bindings and flags rather than a hash.
const GSS_CHECKSUM: i32 = 0x8003;

/// Key usages (RFC 4120 7.5.1 and RFC 4121 2).
const USAGE_TICKET: u32 = 2;
const USAGE_AP_REQ_AUTHENTICATOR: u32 = 11;
const USAGE_AP_REP: u32 = 12;
const USAGE_ACCEPTOR_SEAL: u32 = 22;
const USAGE_ACCEPTOR_SIGN: u32 = 23;
const USAGE_INITIATOR_SEAL: u32 = 24;
const USAGE_INITIATOR_SIGN: u32 = 25;

/// The bits of a token's Flags byte (RFC 4121 4.2.2).
const SENT_BY_ACCEPTOR: u8 = 0x01;
const SEALED: u8 = 0x02;
const ACCEPTOR_SUBKEY: u8 = 0x04;

/// The length of a MIC or Wrap token's header.
const HEADER: usize = 16;

/// A client's half-made context: what it sent, to check the service's
/// answer against.
pub struct Initiator {
    /// The context, once the answer checks out.
    context: Context,
    /// The ticket's session key, which the service's answer is under.
    session: Key,
    /// The authenticator's time, which the service sends back.
    ctime: i64,
    /// Its microseconds.
    cusec: u32,
}

impl Initiator {
    /// Starts a context with the service `credential` is a ticket for,
    /// asking for `wanted` ([`flags`]): the token to send it.
    ///
    /// With [`flags::MUTUAL`] the service answers with a token for
    /// [`Initiator::finish`]; without it the context is ready at once,
    /// from [`Initiator::finish`] with no token.
    pub fn start(credential: &Credential, wanted: u32) -> Result<(Initiator, Vec<u8>)> {
        let (ctime, cusec) = time::now();
        let subkey = Key::random(credential.key.enctype)?;
        // RFC 4121 leaves the first number to the sender; MIT's starts
        // below 2^30, which keeps old 32-bit peers from wrapping.
        let seq = u32::from_be_bytes(random4()?) & 0x3fff_ffff;
        let authenticator = Authenticator {
            client: credential.client.clone(),
            checksum: Some((GSS_CHECKSUM, gss_checksum(wanted))),
            cusec,
            ctime,
            subkey: Some(subkey.clone()),
            seq_number: Some(seq),
        };
        let sealed = Encrypted::seal(
            &credential.key,
            USAGE_AP_REQ_AUTHENTICATOR,
            &authenticator.encode(),
            None,
        )?;
        let options = if wanted & flags::MUTUAL != 0 { AP_MUTUAL_REQUIRED } else { 0 };
        let request = encode_ap_request(options, &credential.ticket, &sealed);
        let context = Context {
            key: subkey,
            initiator: true,
            acceptor_subkey: false,
            send_seq: seq.into(),
            peer: credential.server.clone(),
            service: credential.server.clone(),
            flags: wanted | flags::CONF | flags::INTEG,
            expires: credential.endtime,
        };
        let session = credential.key.clone();
        Ok((Initiator { context, session, ctime, cusec }, frame(TOK_AP_REQ, &request)))
    }

    /// The context, after the service's answer `token`, which proves it
    /// opened the ticket. `None` when no answer was asked for.
    pub fn finish(self, token: Option<&[u8]>) -> Result<Context> {
        let mut context = self.context;
        let Some(token) = token else {
            if context.flags & flags::MUTUAL != 0 {
                return Err(Error::Other(
                    "the service did not prove itself, though the client asked it to".into(),
                ));
            }
            return Ok(context);
        };
        let (id, inner) = unframe(token)?;
        if id == TOK_ERROR {
            let error = KdcError::decode(inner)?;
            return Err(Error::Kdc {
                code: error.code,
                text: format!("the service refused: {}", error.explain()),
            });
        }
        if id != TOK_AP_REP {
            return Err(Error::Malformed(format!(
                "a GSS-API token {id:02x?} where an AP-REP belongs"
            )));
        }
        let part = ApReplyPart::open(inner, &self.session, USAGE_AP_REP).map_err(|err| {
            match err {
            Error::Integrity => Error::Other(
                "the service's answer is not under the ticket's session key: it is not the service \
                 the ticket is for"
                    .into(),
            ),
            other => other,
        }
        })?;
        if (part.ctime, part.cusec) != (self.ctime, self.cusec) {
            return Err(Error::Other(
                "the service's answer is not to this client's authenticator: it may be a replay"
                    .into(),
            ));
        }
        if let Some(subkey) = part.subkey {
            context.key = subkey;
            context.acceptor_subkey = true;
        }
        Ok(context)
    }
}

/// Accepts a client's first token, `token`, with the keys in `keytab`:
/// the context, and the token to answer with, if the client asked for
/// one.
///
/// Any principal in `keytab` is accepted; the caller checks
/// [`Context::service`] if only one of them should be.
///
/// There is no replay cache: a replayed token sets up a context whose
/// keys only the true client knows, since the client's subkey travels
/// inside the authenticator and the service's inside its answer, so
/// whoever replays it can neither sign nor read with it.
pub fn accept(keytab: &[keytab::Entry], token: &[u8]) -> Result<(Context, Option<Vec<u8>>)> {
    let (id, inner) = unframe(token)?;
    if id != TOK_AP_REQ {
        return Err(Error::Malformed(format!("a GSS-API token {id:02x?} where an AP-REQ belongs")));
    }
    let request = ApRequest::decode(inner)?;
    let server = &request.ticket.server;
    let key = service_key(keytab, server, &request.ticket.enc_part)?;
    let ticket = TicketPart::decode(&request.ticket.enc_part.open(&key, USAGE_TICKET).map_err(|_| {
        Error::Other(format!(
            "the ticket for {server} does not open with the keytab's key: the keytab is older than \
             the KDC's key, so extract it again with kadmin's ktadd"
        ))
    })?)?;
    let (now, _) = time::now();
    if ticket.endtime + CLOCK_SKEW < now {
        return Err(Error::Other(format!(
            "{}'s ticket for {server} has expired: kinit again",
            ticket.client
        )));
    }
    if ticket.starttime.unwrap_or(ticket.authtime) - CLOCK_SKEW > now {
        return Err(Error::Other(format!(
            "{}'s ticket for {server} is not good yet: the clocks differ, so check both sides' time",
            ticket.client
        )));
    }
    let authenticator = Authenticator::decode(
        &request.authenticator.open(&ticket.key, USAGE_AP_REQ_AUTHENTICATOR)?,
    )?;
    if !authenticator.client.same(&ticket.client) {
        return Err(Error::Other(format!(
            "the authenticator is {}'s, the ticket {}'s",
            authenticator.client, ticket.client
        )));
    }
    if (authenticator.ctime - now).abs() > CLOCK_SKEW {
        return Err(Error::Other(format!(
            "{}'s clock and this machine's are more than five minutes apart: set both by NTP",
            ticket.client
        )));
    }
    let wanted = match &authenticator.checksum {
        Some((GSS_CHECKSUM, sum)) => read_gss_checksum(sum)?,
        _ => return Err(Error::Malformed("an authenticator without GSS-API's checksum".into())),
    };
    let mutual = wanted & flags::MUTUAL != 0 || request.options & AP_MUTUAL_REQUIRED != 0;
    // The key for what follows: the client's subkey, or the session key
    // if it chose none, unless this side chooses one in its answer.
    let mut context = Context {
        key: authenticator.subkey.clone().unwrap_or_else(|| ticket.key.clone()),
        initiator: false,
        acceptor_subkey: false,
        send_seq: 0,
        peer: ticket.client.clone(),
        service: server.clone(),
        flags: wanted | flags::CONF | flags::INTEG,
        expires: ticket.endtime,
    };
    if !mutual {
        return Ok((context, None));
    }
    // An acceptor subkey, as MIT's acceptor chooses: it keeps what
    // follows apart from the session key, which the KDC also knows.
    let subkey = Key::random(context.key.enctype)?;
    let seq = u32::from_be_bytes(random4()?) & 0x3fff_ffff;
    let reply = ApReplyPart {
        ctime: authenticator.ctime,
        cusec: authenticator.cusec,
        subkey: Some(subkey.clone()),
        seq_number: Some(seq),
    }
    .seal(&ticket.key, USAGE_AP_REP)?;
    context.key = subkey;
    context.acceptor_subkey = true;
    context.send_seq = seq.into();
    Ok((context, Some(frame(TOK_AP_REP, &reply))))
}

/// The key in `keytab` that opens `sealed`, a ticket's part for `server`.
fn service_key(keytab: &[keytab::Entry], server: &Principal, sealed: &Encrypted) -> Result<Key> {
    let mine: Vec<&keytab::Entry> = keytab
        .iter()
        .filter(|entry| entry.principal.same(server) && entry.key.enctype.number() == sealed.etype)
        .collect();
    let found = match sealed.kvno {
        Some(kvno) => mine.iter().find(|entry| entry.kvno == kvno),
        None => mine.iter().max_by_key(|entry| entry.kvno),
    };
    match (found, mine.is_empty()) {
        (Some(entry), _) => Ok(entry.key.clone()),
        (None, true) => Err(Error::Other(format!(
            "the keytab holds no key of encryption type {} for {server}: add one with kadmin's ktadd",
            sealed.etype
        ))),
        (None, false) => Err(Error::Other(format!(
            "the keytab holds another version of {server}'s key than the ticket's ({}): extract it \
             again with kadmin's ktadd",
            sealed.kvno.unwrap_or_default()
        ))),
    }
}

/// A security context: the key both sides use, and which side this is.
pub struct Context {
    /// The key the tokens are signed and sealed under.
    key: Key,
    /// Whether this side started the context.
    initiator: bool,
    /// Whether `key` is a subkey the acceptor chose.
    acceptor_subkey: bool,
    /// The next sequence number this side sends.
    send_seq: u64,
    /// The other side: the service for an initiator, the client for an
    /// acceptor.
    peer: Principal,
    /// The service the context is with, which the ticket names.
    service: Principal,
    /// The context's flags ([`flags`]).
    flags: u32,
    /// When the ticket under it ends, in seconds since 1970.
    expires: i64,
}

impl Context {
    /// The other side: the service, to a client; the client, to a
    /// service, which maps it to a user.
    pub fn peer(&self) -> &Principal {
        &self.peer
    }

    /// The service the context is with, as the ticket names it: an
    /// acceptor with several principals in its keytab checks it.
    pub fn service(&self) -> &Principal {
        &self.service
    }

    /// The context's flags ([`flags`]).
    pub fn flags(&self) -> u32 {
        self.flags
    }

    /// When the ticket under the context ends, in seconds since 1970:
    /// after it, the client sets up another.
    pub fn expires(&self) -> i64 {
        self.expires
    }

    /// Whether the acceptor chose the key, which both sides' tokens say.
    pub fn uses_acceptor_subkey(&self) -> bool {
        self.acceptor_subkey
    }

    /// The usages this side signs and seals with, and the other side's.
    fn usages(&self) -> ((u32, u32), (u32, u32)) {
        let initiator = (USAGE_INITIATOR_SIGN, USAGE_INITIATOR_SEAL);
        let acceptor = (USAGE_ACCEPTOR_SIGN, USAGE_ACCEPTOR_SEAL);
        if self.initiator { (initiator, acceptor) } else { (acceptor, initiator) }
    }

    /// The Flags byte of a token this side sends.
    fn sent_flags(&self) -> u8 {
        let mut bits = if self.initiator { 0 } else { SENT_BY_ACCEPTOR };
        if self.acceptor_subkey {
            bits |= ACCEPTOR_SUBKEY;
        }
        bits
    }

    /// Checks the Flags byte of a token the other side sent.
    fn check_flags(&self, bits: u8) -> Result<()> {
        let from_acceptor = bits & SENT_BY_ACCEPTOR != 0;
        if from_acceptor != self.initiator {
            return Err(Error::Other(
                "a token this side sent came back: it is reflected, not the other side's".into(),
            ));
        }
        if (bits & ACCEPTOR_SUBKEY != 0) != self.acceptor_subkey {
            return Err(Error::Other(
                "the other side signs with another key: the two sides disagree on the acceptor's \
                 subkey"
                    .into(),
            ));
        }
        Ok(())
    }

    /// The next sequence number to send.
    fn next_seq(&mut self) -> u64 {
        let seq = self.send_seq;
        self.send_seq = self.send_seq.wrapping_add(1);
        seq
    }

    /// A MIC token for `message` (RFC 4121 4.2.6.1): its signature.
    pub fn get_mic(&mut self, message: &[u8]) -> Vec<u8> {
        let ((sign, _), _) = self.usages();
        let mut token = Vec::with_capacity(HEADER + self.key.enctype.mac_len());
        token.extend_from_slice(&TOK_MIC);
        token.push(self.sent_flags());
        token.extend_from_slice(&[0xff; 5]);
        token.extend_from_slice(&self.next_seq().to_be_bytes());
        let mut signed = message.to_vec();
        signed.extend_from_slice(&token);
        token.extend_from_slice(&self.key.checksum(sign, &signed));
        token
    }

    /// Checks that `token` is the other side's MIC token for `message`.
    pub fn verify_mic(&self, message: &[u8], token: &[u8]) -> Result<()> {
        let (_, (sign, _)) = self.usages();
        if token.len() != HEADER + self.key.enctype.mac_len() || token[..2] != TOK_MIC {
            return Err(Error::Malformed("a MIC token of the wrong shape".into()));
        }
        if token[3..8] != [0xff; 5] {
            return Err(Error::Malformed("a MIC token whose filler is not 0xff".into()));
        }
        self.check_flags(token[2])?;
        let (header, mac) = token.split_at(HEADER);
        let mut signed = message.to_vec();
        signed.extend_from_slice(header);
        if !self.key.verify(sign, &signed, mac) {
            return Err(Error::Integrity);
        }
        Ok(())
    }

    /// A Wrap token for `message` (RFC 4121 4.2.6.2): sealed, if
    /// `seal`, or signed only.
    pub fn wrap(&mut self, message: &[u8], seal: bool) -> Result<Vec<u8>> {
        // A Wrap token's checksum, sealed or not, is under the SEAL usage,
        // as MIT's and Linux's are; only MIC tokens use SIGN.
        let ((_, usage), _) = self.usages();
        let mut header = [0u8; HEADER];
        header[..2].copy_from_slice(&TOK_WRAP);
        header[2] = self.sent_flags() | if seal { SEALED } else { 0 };
        header[3] = 0xff;
        // EC and RRC stay 0: AES needs no filler, and this side never
        // rotates (RRC), though it undoes the other side's rotation.
        header[8..].copy_from_slice(&self.next_seq().to_be_bytes());
        let mut token = header.to_vec();
        if seal {
            // The header goes inside too, so a changed header fails the
            // integrity check.
            let mut plain = Vec::with_capacity(message.len() + HEADER);
            plain.extend_from_slice(message);
            plain.extend_from_slice(&header);
            let sealed = self.key.encrypt(usage, &plain);
            crypto::wipe(&mut plain);
            token.extend_from_slice(&sealed?);
        } else {
            let mut signed = message.to_vec();
            signed.extend_from_slice(&header);
            let mac = self.key.checksum(usage, &signed);
            // EC counts the checksum's bytes after the message.
            token[4..6].copy_from_slice(&(mac.len() as u16).to_be_bytes());
            token.extend_from_slice(message);
            token.extend_from_slice(&mac);
        }
        Ok(token)
    }

    /// The message in the other side's Wrap token `token`, and whether it
    /// was sealed.
    pub fn unwrap(&self, token: &[u8]) -> Result<(Vec<u8>, bool)> {
        let (_, (_, usage)) = self.usages();
        if token.len() < HEADER || token[..2] != TOK_WRAP || token[3] != 0xff {
            return Err(Error::Malformed("a Wrap token of the wrong shape".into()));
        }
        self.check_flags(token[2])?;
        let sealed = token[2] & SEALED != 0;
        let ec = u16::from_be_bytes([token[4], token[5]]) as usize;
        let rrc = u16::from_be_bytes([token[6], token[7]]) as usize;
        let mut header = [0u8; HEADER];
        header.copy_from_slice(&token[..HEADER]);
        // The sender may have rotated what follows the header right by
        // RRC bytes (RFC 4121 4.2.5); turn it back.
        let mut body = token[HEADER..].to_vec();
        let len = body.len();
        if len > 0 {
            body.rotate_left(rrc % len);
        }
        if sealed {
            let mut plain = self.key.decrypt(usage, &body)?;
            if plain.len() < ec + HEADER {
                return Err(Error::Malformed("a sealed Wrap token shorter than its filler".into()));
            }
            // The header inside has RRC 0; the rest matches the outside.
            let inner = &plain[plain.len() - HEADER..];
            header[6..8].fill(0);
            if inner != header {
                crypto::wipe(&mut plain);
                return Err(Error::Integrity);
            }
            plain.truncate(plain.len() - HEADER - ec);
            return Ok((plain, true));
        }
        let mac_len = self.key.enctype.mac_len();
        if ec != mac_len || body.len() < mac_len {
            return Err(Error::Malformed(
                "a signed Wrap token whose checksum is the wrong size".into(),
            ));
        }
        let (message, mac) = body.split_at(body.len() - mac_len);
        // The checksum covers the header with EC and RRC as 0.
        header[4..8].fill(0);
        let mut signed = message.to_vec();
        signed.extend_from_slice(&header);
        if !self.key.verify(usage, &signed, mac) {
            return Err(Error::Integrity);
        }
        Ok((message.to_vec(), false))
    }
}

/// The checksum GSS-API puts in the authenticator (RFC 4121 4.1.1):
/// Lgth, then Bnd, the channel bindings' hash (none here, so zeros), then
/// the flags, all little-endian.
fn gss_checksum(wanted: u32) -> Vec<u8> {
    let mut sum = Vec::with_capacity(24);
    sum.extend_from_slice(&16u32.to_le_bytes());
    sum.extend_from_slice(&[0; 16]);
    sum.extend_from_slice(&wanted.to_le_bytes());
    sum
}

/// The flags in a client's GSS-API checksum `sum`. Channel bindings are
/// not checked, as a service that has none of its own may (RFC 4121
/// 4.1.1.2); delegated credentials after the flags are not taken.
fn read_gss_checksum(sum: &[u8]) -> Result<u32> {
    if sum.len() < 24 || sum[..4] != 16u32.to_le_bytes() {
        return Err(Error::Malformed("GSS-API's authenticator checksum is the wrong size".into()));
    }
    Ok(u32::from_le_bytes([sum[20], sum[21], sum[22], sum[23]]))
}

/// `message` in RFC 2743 3.1's framing: the mechanism's OID, then the
/// token ID, inside one APPLICATION 0 value.
fn frame(id: [u8; 2], message: &[u8]) -> Vec<u8> {
    let mut inner = der::tlv(0x06, &KRB5_OID);
    inner.extend_from_slice(&id);
    inner.extend_from_slice(message);
    der::application(0, &inner)
}

/// The token ID and message in the framed token `token`.
fn unframe(token: &[u8]) -> Result<([u8; 2], &[u8])> {
    let inner = der::read_application(token, 0, "GSS-API token")?;
    let oid = der::tlv(0x06, &KRB5_OID);
    match inner.strip_prefix(oid.as_slice()) {
        Some([a, b, rest @ ..]) => Ok(([*a, *b], rest)),
        Some(_) => Err(Error::Malformed("a GSS-API token that ends after its OID".into())),
        None => Err(Error::Malformed(
            "a GSS-API token for another mechanism than Kerberos 5 (SPNEGO is not spoken)".into(),
        )),
    }
}

/// Four random bytes, for a first sequence number.
fn random4() -> Result<[u8; 4]> {
    let bytes = crypto::random(4)?;
    Ok([bytes[0], bytes[1], bytes[2], bytes[3]])
}

/// Contexts without a KDC, for the tests of what runs on GSS-API: this
/// crate's, and nfs's and nfsd's, which turn on the `testing` feature.
#[cfg(any(test, feature = "testing"))]
pub mod testing {
    use super::{Context, Initiator, accept};
    use crate::creds::Credential;
    use crate::crypto::{Enctype, Key};
    use crate::keytab;
    use crate::messages::{Principal, TicketPart, encode_ticket};
    use crate::time;

    /// The service the tests' tickets are for.
    pub const SERVICE: &str = "nfs/server.example.org@EXAMPLE.ORG";

    /// The version of the service's key the tickets are under.
    pub const KVNO: u32 = 3;

    /// A ticket for `client` to `service`, with a session key of
    /// `enctype`, as a KDC would issue it: the client's credential, and
    /// the service's keytab, which also holds an older key.
    ///
    /// # Panics
    ///
    /// On a name that does not parse, or no random bytes.
    pub fn issue(
        client: &str,
        service: &str,
        enctype: Enctype,
    ) -> (Credential, Vec<keytab::Entry>) {
        let server = Principal::parse(service, None).expect("a service's name");
        let service_key = Key::random(Enctype::Aes256Sha1).expect("random bytes");
        let older = Key::random(Enctype::Aes256Sha1).expect("random bytes");
        let entry =
            |kvno, key| keytab::Entry { principal: server.clone(), timestamp: 0, kvno, key };
        let keytab = vec![entry(KVNO - 1, older), entry(KVNO, service_key)];
        (ticket(client, &keytab[1], enctype), keytab)
    }

    /// A ticket for `client` under the service's key `entry`, with a
    /// session key of `enctype`: another client's, for a keytab that
    /// [`issue`] made.
    ///
    /// # Panics
    ///
    /// On a name that does not parse, or no random bytes.
    pub fn ticket(client: &str, entry: &keytab::Entry, enctype: Enctype) -> Credential {
        let client = Principal::parse(client, None).expect("a client's name");
        let session = Key::random(enctype).expect("random bytes");
        let (now, _) = time::now();
        let part = TicketPart {
            flags: 0,
            key: session.clone(),
            client: client.clone(),
            authtime: now,
            starttime: None,
            endtime: now + 3600,
        };
        Credential {
            client,
            server: entry.principal.clone(),
            key: session,
            authtime: now,
            starttime: now,
            endtime: now + 3600,
            renew_till: 0,
            flags: 0,
            ticket: encode_ticket(&entry.principal, &entry.key, entry.kvno, &part),
        }
    }

    /// Both sides of a context for alice to [`SERVICE`], set up as
    /// `wanted` asks: the client's, then the service's.
    ///
    /// # Panics
    ///
    /// When the context does not set up, which is a bug.
    pub fn pair(wanted: u32) -> (Context, Context) {
        let (credential, keytab) = issue("alice@EXAMPLE.ORG", SERVICE, Enctype::Aes256Sha1);
        let (initiator, token) = Initiator::start(&credential, wanted).expect("a context starts");
        let (service, answer) = accept(&keytab, &token).expect("the service accepts it");
        let client = initiator.finish(answer.as_deref()).expect("the client takes the answer");
        (client, service)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::Enctype;

    use super::testing::{SERVICE, issue};

    /// Both sides' contexts, set up as `wanted` asks.
    fn contexts(enctype: Enctype, wanted: u32) -> (Context, Context) {
        let (credential, keytab) = issue("alice@EXAMPLE.ORG", SERVICE, enctype);
        let (initiator, token) = Initiator::start(&credential, wanted).unwrap();
        let (acceptor, answer) = accept(&keytab, &token).unwrap();
        assert_eq!(acceptor.peer().to_string(), "alice@EXAMPLE.ORG");
        let client = initiator.finish(answer.as_deref()).unwrap();
        assert_eq!(client.peer().to_string(), SERVICE);
        (client, acceptor)
    }

    #[test]
    fn contexts_set_up_with_and_without_mutual_authentication() {
        for enctype in crypto::ALL {
            let (client, service) = contexts(enctype, flags::MUTUAL);
            assert!(client.uses_acceptor_subkey() && service.uses_acceptor_subkey());
            let (client, service) = contexts(enctype, 0);
            assert!(!client.uses_acceptor_subkey() && !service.uses_acceptor_subkey());
        }
    }

    #[test]
    fn mics_check_both_ways_and_catch_changes() {
        for wanted in [0, flags::MUTUAL] {
            let (mut client, mut service) = contexts(Enctype::Aes128Sha256, wanted);
            let mic = client.get_mic(b"call 1");
            service.verify_mic(b"call 1", &mic).unwrap();
            assert_eq!(service.verify_mic(b"call 2", &mic), Err(Error::Integrity));
            let back = service.get_mic(b"reply 1");
            client.verify_mic(b"reply 1", &back).unwrap();
            // A token sent back to its own side is refused.
            assert!(client.verify_mic(b"call 1", &mic).is_err());
        }
    }

    #[test]
    fn wraps_open_both_ways_sealed_and_signed() {
        for enctype in crypto::ALL {
            let (mut client, mut service) = contexts(enctype, flags::MUTUAL);
            for seal in [true, false] {
                let token = client.wrap(b"write these bytes", seal).unwrap();
                assert_eq!(token.windows(5).any(|w| w == b"write"), !seal);
                assert_eq!(service.unwrap(&token).unwrap(), (b"write these bytes".to_vec(), seal));
                let reply = service.wrap(b"", seal).unwrap();
                assert_eq!(client.unwrap(&reply).unwrap(), (Vec::new(), seal));
                let mut changed = token.clone();
                *changed.last_mut().unwrap() ^= 1;
                assert!(service.unwrap(&changed).is_err());
            }
        }
    }

    #[test]
    fn rotated_wraps_open() {
        let (mut client, service) = contexts(Enctype::Aes256Sha1, flags::MUTUAL);
        for seal in [true, false] {
            let mut token = client.wrap(b"rotated by the sender", seal).unwrap();
            // Rotate right by 28, as Windows does, and say so in RRC.
            token[6..8].copy_from_slice(&28u16.to_be_bytes());
            token[HEADER..].rotate_right(28);
            assert_eq!(service.unwrap(&token).unwrap().0, b"rotated by the sender");
        }
    }

    #[test]
    fn a_keytab_without_the_tickets_key_is_named() {
        let (credential, mut keytab) = issue("alice@EXAMPLE.ORG", SERVICE, Enctype::Aes256Sha1);
        let (_, token) = Initiator::start(&credential, flags::MUTUAL).unwrap();
        keytab.retain(|entry| entry.kvno != testing::KVNO);
        let err = accept(&keytab, &token).err().unwrap().to_string();
        assert!(err.contains("another version"), "{err}");
        keytab.clear();
        let err = accept(&keytab, &token).err().unwrap().to_string();
        assert!(err.contains("no key"), "{err}");
    }

    #[test]
    fn another_services_answer_is_refused() {
        let (credential, _) = issue("alice@EXAMPLE.ORG", SERVICE, Enctype::Aes256Sha1);
        let (initiator, _) = Initiator::start(&credential, flags::MUTUAL).unwrap();
        // An answer from a service that never opened the ticket: it does
        // not know the session key.
        let other = Key::random(Enctype::Aes256Sha1).unwrap();
        let part = ApReplyPart {
            ctime: initiator.ctime,
            cusec: initiator.cusec,
            subkey: None,
            seq_number: None,
        };
        let answer = frame(TOK_AP_REP, &part.seal(&other, USAGE_AP_REP).unwrap());
        let err = initiator.finish(Some(&answer)).err().unwrap().to_string();
        assert!(err.contains("not the service"), "{err}");
    }

    #[test]
    fn a_missing_answer_is_refused_when_one_was_asked_for() {
        let (credential, _) = issue("alice@EXAMPLE.ORG", SERVICE, Enctype::Aes256Sha1);
        let (initiator, _) = Initiator::start(&credential, flags::MUTUAL).unwrap();
        assert!(initiator.finish(None).is_err());
    }

    #[test]
    fn tokens_are_framed_as_rfc_2743_says() {
        let token = frame(TOK_AP_REQ, b"x");
        assert_eq!(token[..4], [0x60, 14, 0x06, 9]);
        assert_eq!(unframe(&token).unwrap(), (TOK_AP_REQ, &b"x"[..]));
        let mut spnego = token.clone();
        spnego[4] = 0x2b;
        assert!(unframe(&spnego).is_err());
    }
}

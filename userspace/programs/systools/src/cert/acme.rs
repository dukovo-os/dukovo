// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Certificates from an ACME authority (ADR 0513): Let's Encrypt, or any
//! authority that speaks RFC 8555.
//!
//! `cert get` asks for a certificate for names, shows the authority that
//! this machine holds them, and installs what it gets in the store; `cert
//! renew` and certd ask again with what was asked the first time; `cert
//! revoke` tells the authority the certificate is not to be trusted.
//!
//! - [`http`] is the client the library sends its requests through, tuf's;
//! - [`problems`] says the authority's refusals in plain words;
//! - [`preflight`] checks, before the authority is asked, what a person can
//!   fix at once: the clock, the names' addresses and the firewall;
//! - [`answer`] is the responder that answers the authority on port 80;
//! - [`dns`] is DNS-01 by hand.
//!
//! The answers to HTTP-01 are served in one of two ways, by what holds port
//! 80: when it is free, `acme-answer` binds it for the length of the order;
//! when nginx holds it, the answers are files in a folder nginx serves for
//! every site. DNS-01 needs a person: certd cannot renew what it proves.
//!
//! When an order fails, the words say what to fix; `log certd` shows what
//! certd did, and `cert show NAME` what a certificate holds.

pub mod answer;
pub mod dns;
pub mod http;
pub mod preflight;
pub mod problems;

use std::fs;
use std::io::{self, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::PathBuf;
use std::time::Duration;

use instant_acme::{
    Account, AccountCredentials, AuthorizationStatus, CertificateIdentifier, ChallengeType,
    Identifier, NewAccount, NewOrder, OrderStatus, RetryPolicy, RevocationReason,
    RevocationRequest,
};
use rcgen::{
    CertificateParams, DistinguishedName, DnType, KeyPair, PKCS_ECDSA_P256_SHA256,
    PKCS_ECDSA_P384_SHA384,
};
use ring::rand::{SecureRandom, SystemRandom};
use rustls::pki_types::{CertificateDer, Der};
use serde::{Deserialize, Serialize};
use tuf::url::Url;
use x509_cert::Certificate;
use x509_cert::der::Decode;
use x509_cert::ext::pkix::AuthorityKeyIdentifier;

use self::answer::Responder;
use self::http::TufClient;
use super::add::{self, Text};
use super::inspect::{self, Details};
use super::store::{Acme, Challenge, Files, KeyKind, Kind, Lock, Store, Stored};
use super::{now, spark_warning, tell_spark};
use crate::power;
use crate::util::{self, Fail, fail};

/// Let's Encrypt's directory, which is the default.
pub const LETS_ENCRYPT: &str = "https://acme-v02.api.letsencrypt.org/directory";

/// Its staging directory, whose certificates browsers do not trust, and whose
/// rate limits are far higher.
pub const LETS_ENCRYPT_STAGING: &str = "https://acme-staging-v02.api.letsencrypt.org/directory";

/// The most names in a certificate: Let's Encrypt's limit.
pub const NAMES_MAX: usize = 100;

/// Where the answers go for nginx to serve (acme-challenge.conf in nginx's
/// folder of configuration), and the folder certd may write them in.
pub const ANSWERS: &str = "/system/live/certd/acme-challenge";

/// Waiting for the authority: the first wait 1 second, each longer by half,
/// for 2 minutes in all.
const POLL: RetryPolicy = RetryPolicy::new()
    .initial_delay(Duration::from_secs(1))
    .backoff(1.5)
    .timeout(Duration::from_secs(120));

/// Renewal of a certificate by what its lifetime is: after two thirds of
/// it, as the authority has not said otherwise.
const RENEW_AFTER_THIRDS: i64 = 2;

/// When the authority is asked again about a window at the soonest and at
/// the latest, whatever it says (seconds).
pub const LOOK_AGAIN: (i64, i64) = (3_600, 86_400);

/// The days a certificate must have left for a person to be asked before
/// it is replaced.
const REPLACE_ASKS_DAYS: i64 = 30;

/// How far ahead of this machine's clock a new certificate may start and
/// still be taken (seconds): some authorities date a certificate from the
/// minute they sign it.
pub const CLOCK_SKEW: i64 = 300;

/// How the answers to the authority are given.
pub enum Serving {
    /// `acme-answer` serves these sockets, which are port 80.
    Responder(Vec<TcpListener>),
    /// Files in this folder, which nginx serves.
    Files(PathBuf),
    /// A person adds the TXT records; with `enter`, it waits for them to
    /// say so, and without, which a script does, it goes on to wait for
    /// the records to show.
    ByHand { enter: bool },
}

impl Serving {
    /// The way it is told in a line.
    fn words(&self) -> &'static str {
        match self {
            Serving::Responder(_) => "answering the authority on port 80",
            Serving::Files(_) => "answering the authority through nginx",
            Serving::ByHand { .. } => "waiting for the TXT records to be added",
        }
    }
}

/// What port 80 is, found by binding it.
pub enum Port80 {
    /// Free: bound here, on IPv4 and, where the machine has it, IPv6.
    Free(Vec<TcpListener>),
    /// Held by a program.
    Taken,
}

/// Binds `port` on every IPv4 address and, as its own socket, on every IPv6
/// address, to find out whether it is free.
///
/// # Errors
///
/// With the reason when it cannot be bound for another cause than being
/// taken: no right to, say.
pub fn bind(port: u16) -> Result<Port80, String> {
    let mut listeners = Vec::new();
    for ip in [IpAddr::V4(Ipv4Addr::UNSPECIFIED), IpAddr::V6(Ipv6Addr::UNSPECIFIED)] {
        match listen(SocketAddr::new(ip, port)) {
            Ok(listener) => listeners.push(listener),
            Err(error) if error.kind() == io::ErrorKind::AddrInUse => return Ok(Port80::Taken),
            // No IPv6 on this machine: the IPv4 socket does.
            Err(_) if ip.is_ipv6() && !listeners.is_empty() => {}
            Err(error) => {
                return Err(format!(
                    "cannot listen on port {port}: {}",
                    util::strip_os_error(&error)
                ));
            }
        }
    }
    Ok(Port80::Free(listeners))
}

/// A listening socket on `address`, an IPv6 one for IPv6 alone.
fn listen(address: SocketAddr) -> io::Result<TcpListener> {
    use socket2::{Domain, Socket, Type};
    let domain = if address.is_ipv6() { Domain::IPV6 } else { Domain::IPV4 };
    let socket = Socket::new(domain, Type::STREAM, None)?;
    if address.is_ipv6() {
        socket.set_only_v6(true)?;
    }
    socket.set_reuse_address(true)?;
    socket.bind(&address.into())?;
    socket.listen(128)?;
    Ok(socket.into())
}

/// Whether service `name` runs, by Spark's `status` text.
pub fn service_running(status: &str, name: &str) -> bool {
    status.lines().any(|line| {
        let mut fields = line.split('\t');
        fields.next() == Some(name) && fields.next() == Some("running")
    })
}

// ---------------------------------------------------------------------
// What is asked

/// What a person asks `cert get` for, as the command line says it.
pub struct Get {
    pub name: String,
    pub names: Vec<String>,
    pub dns: bool,
    pub staging: bool,
    pub dry_run: bool,
    pub directory: Option<String>,
    pub email: Option<String>,
    pub agree_terms: bool,
    pub key: Option<KeyKind>,
    pub profile: Option<String>,
    pub yes: bool,
}

/// Whether `directory` is Let's Encrypt's own, whose certificates the
/// system's roots lead to.
pub fn is_lets_encrypt(directory: &str) -> bool {
    directory == LETS_ENCRYPT
}

/// Checks a name for a certificate: lowercase letters, digits and hyphens
/// in labels of up to 63, dots between them, 253 in all, a dot at least,
/// and `*.` first for a wildcard, which `by_hand` allows alone.
///
/// # Errors
///
/// With what is wrong with `name`.
pub fn check_dns_name(name: &str, by_hand: bool) -> Result<(), String> {
    let (wildcard, host) = match name.strip_prefix("*.") {
        Some(rest) => (true, rest),
        None => (false, name),
    };
    if wildcard && !by_hand {
        return Err(format!(
            "{name} is a wildcard, which the authority proves only through DNS: add --dns"
        ));
    }
    if host.parse::<IpAddr>().is_ok() {
        return Err(format!(
            "{name} is an IP address: authorities issue to names here (certificates for IP \
             addresses come with a later release)"
        ));
    }
    let labels: Vec<&str> = host.split('.').collect();
    let label_fits = |label: &&str| {
        (1..=63).contains(&label.len())
            && label
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
            && !label.starts_with('-')
            && !label.ends_with('-')
    };
    if host.len() > 253 || labels.len() < 2 || !labels.iter().all(label_fits) {
        return Err(format!(
            "`{}` is not a name an authority issues for: it has lowercase letters, digits and `-` \
             in labels of up to 63 characters, with a dot between them; write a name that is not \
             ASCII in punycode, as xn--...",
            name.escape_debug()
        ));
    }
    Ok(())
}

/// Checks the names of an order: 1 to [`NAMES_MAX`], each fit, none twice.
///
/// # Errors
///
/// With what is wrong with them.
pub fn check_names(names: &[String], by_hand: bool) -> Result<(), String> {
    if names.is_empty() || names.len() > NAMES_MAX {
        return Err(format!("a certificate has 1 to {NAMES_MAX} names, not {}", names.len()));
    }
    for (number, name) in names.iter().enumerate() {
        check_dns_name(name, by_hand)?;
        if names[..number].contains(name) {
            return Err(format!("{name} is named twice"));
        }
    }
    Ok(())
}

/// Checks a directory's address: https, or http for an authority on a
/// private address (a lab's), where `resolve` says what its host leads to.
///
/// # Errors
///
/// With what is wrong with it.
pub fn check_directory(
    directory: &str,
    resolve: &dyn Fn(&str) -> Vec<IpAddr>,
) -> Result<Url, String> {
    let url = Url::parse(directory).map_err(|why| format!("{directory}: {why}"))?;
    match url.scheme() {
        "https" => {}
        "http" => {
            let host = url.host().trim_start_matches('[').trim_end_matches(']');
            let addresses = match host.parse::<IpAddr>() {
                Ok(address) => vec![address],
                Err(_) => resolve(host),
            };
            if addresses.is_empty()
                || !addresses.iter().all(|address| preflight::private_address(*address))
            {
                return Err(format!(
                    "{directory} is an http address: an authority on the internet is reached \
                     over https, and only one on a private network may be reached over http"
                ));
            }
        }
        other => {
            return Err(format!(
                "{directory}: `{other}` is not an address of an authority: use https://"
            ));
        }
    }
    Ok(url)
}

/// The name of the account file for `directory`: its host, with `-PORT`
/// when the address names a port, and `.json`.
///
/// # Errors
///
/// If the host is not one a file can be named for.
pub fn account_file(directory: &Url) -> Result<String, String> {
    let host = directory.host().trim_start_matches('[').trim_end_matches(']');
    let fits = !host.is_empty()
        && host.len() <= 253
        && host.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"-.:".contains(&byte))
        && !host.starts_with('.')
        && !host.contains("..");
    if !fits {
        return Err(format!("`{}` is not a host an account can be kept for", host.escape_debug()));
    }
    let host = host.replace(':', "_");
    Ok(match directory.port() {
        Some(port) => format!("{host}-{port}.json"),
        None => format!("{host}.json"),
    })
}

// ---------------------------------------------------------------------
// Accounts

/// An account's file: the library's credentials, and when it was made and
/// the contact it was made with.
#[derive(Serialize, Deserialize)]
struct AccountFile {
    #[serde(flatten)]
    credentials: AccountCredentials,
    created: String,
    contact: Option<String>,
}

/// What it takes to make an account where there is none.
pub struct Creation {
    pub email: Option<String>,
    pub agree_terms: bool,
    /// A person is there to ask.
    pub terminal: bool,
}

/// The terms of service the directory at `url` shows, if it shows any.
fn terms_of_service(url: &Url) -> Result<Option<String>, String> {
    let mut body = tuf::http::get(url).map_err(|why| {
        format!(
            "cannot reach the authority at {}: {}",
            tuf::http::host_header(url),
            tuf_words(&why)
        )
    })?;
    let mut bytes = Vec::new();
    io::Read::read_to_end(&mut io::Read::take(&mut body, http::ANSWER_MAX), &mut bytes)
        .map_err(|why| format!("cannot read the authority's directory: {why}"))?;
    terms_in(&bytes)
}

/// `meta.termsOfService` of a directory's JSON.
fn terms_in(json: &[u8]) -> Result<Option<String>, String> {
    let value: serde_json::Value = serde_json::from_slice(json)
        .map_err(|why| format!("the authority's directory is not JSON: {why}"))?;
    Ok(value["meta"]["termsOfService"].as_str().map(str::to_string))
}

/// A fetch's failure in plain words.
fn tuf_words(why: &tuf::http::Error) -> String {
    match why {
        tuf::http::Error::NotFound(code) | tuf::http::Error::Status(code) => {
            format!("it answered {code}")
        }
        tuf::http::Error::Url(why) => why.clone(),
        tuf::http::Error::Redirects => String::from("too many redirects"),
        tuf::http::Error::Io(why) => util::strip_os_error(why),
    }
}

/// Opens the account kept for `directory`, or makes one, as `creation`
/// says, which a command that is not for a person's eyes leaves out.
///
/// # Errors
///
/// In plain words.
pub async fn open_account(
    store: &Store,
    client: &TufClient,
    directory: &Url,
    creation: Option<&Creation>,
    now: i64,
    say: &mut dyn FnMut(&str),
) -> Result<Account, String> {
    let file = account_file(directory)?;
    let text = directory.to_string();
    if let Some(bytes) = store.account(&file)? {
        let kept: AccountFile = serde_json::from_slice(&bytes)
            .map_err(|why| format!("the account for {text} does not read ({file}): {why}"))?;
        let account = Account::builder_with_http(Box::new(client.clone()))
            .from_credentials(kept.credentials)
            .await
            .map_err(|why| problems::explain(&why, &[], client.retry_after().as_deref(), now))?;
        say(&format!("using the account at {}", directory.host()));
        return Ok(account);
    }
    let Some(creation) = creation else {
        return Err(format!(
            "there is no account at {text}: get the certificate once with `elevate cert get`, \
             which makes one"
        ));
    };
    let terms = terms_of_service(directory)?;
    if !creation.agree_terms {
        match (&terms, creation.terminal) {
            (Some(terms), true) => {
                println!("The authority's terms of service: {terms}");
                if !crate::prompt::confirm("Do you agree to them?") {
                    return Err(String::from("the terms were not agreed to: no account was made"));
                }
            }
            (Some(terms), false) => {
                return Err(format!(
                    "agree to the authority's terms with --agree-terms after reading {terms}"
                ));
            }
            (None, _) => {}
        }
    }
    let contact = creation.email.as_ref().map(|email| format!("mailto:{email}"));
    let contacts: Vec<&str> = contact.iter().map(String::as_str).collect();
    say(&format!("making an account at {}", directory.host()));
    let (account, credentials) = Account::builder_with_http(Box::new(client.clone()))
        .create(
            &NewAccount {
                contact: &contacts,
                terms_of_service_agreed: true,
                only_return_existing: false,
            },
            text.clone(),
            None,
        )
        .await
        .map_err(|why| problems::explain(&why, &[], client.retry_after().as_deref(), now))?;
    let kept = AccountFile { credentials, created: util::time_text(now, "%FT%TZ", true), contact };
    let json = serde_json::to_string_pretty(&kept).map_err(|why| why.to_string())?;
    let lock = store.lock()?;
    if !store.add_account(&lock, &file, &json)? {
        eprintln!(
            "cert: another account was kept for {text} while this one was made: this run uses the new one"
        );
    }
    Ok(account)
}

// ---------------------------------------------------------------------
// Orders

/// What an order is for.
#[derive(Clone, Debug)]
pub struct Wanted {
    pub names: Vec<String>,
    pub key: KeyKind,
    pub profile: Option<String>,
}

/// A certificate and its key, as PEM, which the authority has issued.
pub struct Issued {
    pub chain: String,
    pub key: String,
}

/// Makes a key and a request for `names`, the first the common name: the
/// key as PEM and the request as DER.
fn make_request(names: &[String], key: KeyKind) -> Result<(String, Vec<u8>), String> {
    let algorithm = match key {
        KeyKind::EcdsaP256 => &PKCS_ECDSA_P256_SHA256,
        KeyKind::EcdsaP384 => &PKCS_ECDSA_P384_SHA384,
    };
    let pair =
        KeyPair::generate_for(algorithm).map_err(|why| format!("cannot make a key: {why}"))?;
    let mut params = CertificateParams::new(names.to_vec())
        .map_err(|why| format!("the names do not make a request: {why}"))?;
    let mut subject = DistinguishedName::new();
    subject.push(DnType::CommonName, names[0].as_str());
    params.distinguished_name = subject;
    let request =
        params.serialize_request(&pair).map_err(|why| format!("cannot make the request: {why}"))?;
    Ok((inspect::pem_encode("PRIVATE KEY", &pair.serialize_der()), request.der().as_ref().to_vec()))
}

/// What serving the answers holds while an order is checked: the responder,
/// or the files, which are taken away when it ends.
struct Guard {
    responder: Option<Responder>,
    files: Vec<PathBuf>,
}

impl Drop for Guard {
    fn drop(&mut self) {
        if let Some(responder) = self.responder.take() {
            responder.stop();
        }
        for file in &self.files {
            let _ = fs::remove_file(file);
        }
    }
}

/// Puts the answers where the authority will look.
fn serve(serving: &Serving, answers: &[(String, String)]) -> Result<Guard, String> {
    match serving {
        Serving::Responder(listeners) => {
            let sockets: Vec<&TcpListener> = listeners.iter().collect();
            let responder = Responder::start(&sockets, answers)?;
            Ok(Guard { responder: Some(responder), files: Vec::new() })
        }
        Serving::Files(folder) => {
            fs::DirBuilder::new()
                .recursive(true)
                .mode(0o755)
                .create(folder)
                .map_err(|why| format!("{}: {}", folder.display(), util::strip_os_error(&why)))?;
            let mut guard = Guard { responder: None, files: Vec::new() };
            for (token, keyauth) in answers {
                // The token is part of a file's name.
                if !answer::is_token(token) {
                    return Err(format!(
                        "the authority gave `{}` as a token, which is not one",
                        token.escape_debug()
                    ));
                }
                let path = folder.join(token);
                let mut file = fs::OpenOptions::new()
                    .write(true)
                    .create(true)
                    .truncate(true)
                    .mode(0o644)
                    .open(&path)
                    .map_err(|why| format!("{}: {}", path.display(), util::strip_os_error(&why)))?;
                guard.files.push(path.clone());
                file.write_all(keyauth.as_bytes())
                    .map_err(|why| format!("{}: {}", path.display(), util::strip_os_error(&why)))?;
            }
            Ok(guard)
        }
        Serving::ByHand { .. } => Ok(Guard { responder: None, files: Vec::new() }),
    }
}

/// What an order needs of its surroundings.
pub struct Setup<'a> {
    pub client: &'a TufClient,
    pub now: i64,
}

/// Runs an order for `wanted` on `account`: the answers, the wait, the
/// key and the request, and the certificate. `replaces` is the certificate
/// this one is to replace, for an authority with renewal information.
///
/// # Errors
///
/// In plain words, with the authority's own detail after them.
pub async fn order(
    setup: &Setup<'_>,
    account: &Account,
    wanted: &Wanted,
    serving: &Serving,
    replaces: Option<CertificateIdentifier<'static>>,
    say: &mut dyn FnMut(&str),
) -> Result<Issued, String> {
    let explain = |error: instant_acme::Error| {
        problems::explain(&error, &wanted.names, setup.client.retry_after().as_deref(), setup.now)
    };
    let identifiers: Vec<Identifier> =
        wanted.names.iter().map(|name| Identifier::Dns(name.clone())).collect();
    let mut new = NewOrder::new(&identifiers);
    if let Some(identifier) = replaces {
        new = new.replaces(identifier);
    }
    if let Some(profile) = &wanted.profile {
        new = new.profile(profile);
    }
    say(&format!("asking for a certificate for {}", wanted.names.join(", ")));
    let mut ordered = account.new_order(&new).await.map_err(explain)?;
    let kind = match serving {
        Serving::ByHand { .. } => ChallengeType::Dns01,
        _ => ChallengeType::Http01,
    };

    // What each name must show.
    let mut answers: Vec<(String, String)> = Vec::new();
    let mut records: Vec<dns::Record> = Vec::new();
    {
        let mut authorizations = ordered.authorizations();
        while let Some(next) = authorizations.next().await {
            let mut authorization = next.map_err(explain)?;
            match authorization.status {
                AuthorizationStatus::Pending => {}
                AuthorizationStatus::Valid => continue,
                other => {
                    return Err(format!(
                        "the authority's authorization is {other:?}: ask again in a few minutes"
                    ));
                }
            }
            let name = authorization.identifier().to_string();
            let Some(challenge) = authorization.challenge(kind.clone()) else {
                return Err(format!(
                    "the authority offers no way to show {name} by {}: {}",
                    if matches!(kind, ChallengeType::Dns01) { "DNS" } else { "HTTP" },
                    if matches!(kind, ChallengeType::Dns01) {
                        "try without --dns"
                    } else {
                        "try with --dns"
                    }
                ));
            };
            let keyauth = challenge.key_authorization();
            match kind {
                ChallengeType::Dns01 => {
                    records.push(dns::Record { name, value: keyauth.dns_value() });
                }
                _ => answers.push((challenge.token.clone(), keyauth.as_str().to_string())),
            }
        }
    }

    say(serving.words());
    let guard = serve(serving, &answers)?;
    if let Serving::ByHand { enter } = serving {
        by_hand(&records, *enter, say)?;
    }
    {
        let mut authorizations = ordered.authorizations();
        while let Some(next) = authorizations.next().await {
            let mut authorization = next.map_err(explain)?;
            if authorization.status != AuthorizationStatus::Pending {
                continue;
            }
            let Some(mut challenge) = authorization.challenge(kind.clone()) else { continue };
            challenge.set_ready().await.map_err(explain)?;
        }
    }
    say("waiting for the authority to check the names");
    let status = ordered.poll_ready(&POLL).await.map_err(explain)?;
    drop(guard);
    if status != OrderStatus::Ready {
        return Err(why_invalid(&mut ordered, setup, &wanted.names).await);
    }
    say("the names are proven");

    say(&format!("making a key ({})", wanted.key.word()));
    let (key, request) = make_request(&wanted.names, wanted.key)?;
    ordered.finalize_csr(&request).await.map_err(explain)?;
    say("waiting for the certificate");
    let chain = ordered.poll_certificate(&POLL).await.map_err(explain)?;
    Ok(Issued { chain, key })
}

/// Shows the records to add, waits for Enter, then for the resolver to show
/// each.
fn by_hand(records: &[dns::Record], enter: bool, say: &mut dyn FnMut(&str)) -> Result<(), String> {
    print!("{}", dns::instructions(records, enter));
    let _ = io::stdout().flush();
    if enter {
        match crate::prompt::ask("", true) {
            Ok(crate::prompt::Answer::Line(_)) => {}
            _ => return Err(String::from("stopped before the records were added")),
        }
    }
    dns::wait_for(
        records,
        dns::EVERY,
        dns::UP_TO,
        &mut |at| dns::lookup(at),
        &mut std::thread::sleep,
        say,
    )
}

/// Why an order that ended invalid did: what the authority said of each
/// challenge it could not check.
async fn why_invalid(
    ordered: &mut instant_acme::Order,
    setup: &Setup<'_>,
    names: &[String],
) -> String {
    let after = setup.client.retry_after();
    let mut found: Vec<String> = Vec::new();
    let mut authorizations = ordered.authorizations();
    while let Some(Ok(mut authorization)) = authorizations.next().await {
        let Ok(state) = authorization.refresh().await else { continue };
        let name = state.identifier().to_string();
        for challenge in &state.challenges {
            if let Some(problem) = &challenge.error {
                let words = problems::explain(
                    &instant_acme::Error::Api(problem.clone()),
                    std::slice::from_ref(&name),
                    after.as_deref(),
                    setup.now,
                );
                if !found.contains(&words) {
                    found.push(words);
                }
            }
        }
    }
    if found.is_empty() {
        format!("the authority could not check {}: ask again in a few minutes", names.join(", "))
    } else {
        found.join("; ")
    }
}

/// A current-thread runtime with time, which runs one thing at a time.
///
/// # Errors
///
/// If the runtime cannot be made.
pub fn runtime() -> Result<tokio::runtime::Runtime, String> {
    tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .map_err(|why| format!("cannot start the runtime: {why}"))
}

// ---------------------------------------------------------------------
// Installing

/// Checks what the authority gave as `cert add` checks a pair, and that it
/// names what was asked for, then installs it as certificate `name` under
/// the lock: its details and what to tell the person.
///
/// # Errors
///
/// In plain words, with nothing installed.
pub fn install(
    store: &Store,
    lock: &Lock,
    name: &str,
    issued: &Issued,
    acme: &Acme,
    now: i64,
) -> Result<(Details, Vec<String>), String> {
    let roots = crate::tls::roots(&[], "cert");
    let chain = Text { shown: String::from("the authority's chain"), text: issued.chain.clone() };
    let key = Text { shown: String::from("the new key"), text: issued.key.clone() };
    // The clock may be a little behind the authority's, which dates a
    // certificate from the minute it signs it, and the order is not to be
    // lost to that: a certificate that starts within the skew is judged at
    // the time it starts.
    let starts = inspect::chain_of(&issued.chain)
        .ok()
        .and_then(|chain| chain.first().and_then(|first| inspect::details(first).ok()))
        .map(|details| details.not_before);
    let judged_at = match starts {
        Some(starts) if starts > now && starts <= now + CLOCK_SKEW => starts,
        _ => now,
    };
    let checked =
        add::check_texts(&chain, &key, !is_lets_encrypt(&acme.directory), &roots, judged_at)?;
    for wanted in &acme.names {
        if !checked.details.names.contains(wanted) {
            return Err(format!(
                "the authority's certificate does not name {wanted}: it names {}",
                checked.details.names.join(", ")
            ));
        }
    }
    let files = Files {
        chain: &checked.chain,
        key: &checked.key,
        kind: Kind::Acme,
        made: now,
        acme: Some(acme),
    };
    store.install(lock, name, &files)?;
    Ok((checked.details, checked.notes))
}

// ---------------------------------------------------------------------
// `cert get`, `cert renew`, `cert revoke`

/// How the answers are given for a renewal that no person watches or
/// that an administrator runs: port 80 when it is free, nginx's folder when
/// it is taken; `nginx_runs` says nginx is the one holding it, where that
/// is known.
///
/// # Errors
///
/// With why neither is possible.
pub fn serving_for_http(nginx_runs: Option<bool>) -> Result<Serving, String> {
    match bind(80)? {
        Port80::Free(listeners) => Ok(Serving::Responder(listeners)),
        Port80::Taken if nginx_runs != Some(false) => Ok(Serving::Files(PathBuf::from(ANSWERS))),
        Port80::Taken => {
            Err(String::from("port 80 is taken by another program: stop it, or use --dns"))
        }
    }
}

/// Whether nginx runs, by asking Spark; none when Spark cannot be asked.
fn nginx_runs() -> Option<bool> {
    let status = power::request("status").ok()?;
    Some(service_running(&status, "nginx"))
}

/// Reads the key type a command line names.
pub fn key_kind(word: &str) -> Result<KeyKind, String> {
    match word {
        "ecdsa-p256" => Ok(KeyKind::EcdsaP256),
        "ecdsa-p384" => Ok(KeyKind::EcdsaP384),
        other => Err(format!("`{other}` is not a key type: ecdsa-p256 or ecdsa-p384")),
    }
}

/// The days a stored certificate has left.
fn days_left(stored: &Stored, now: i64) -> Result<(Details, i64), String> {
    let text = String::from_utf8_lossy(&stored.chain).into_owned();
    let chain = inspect::chain_of(&text)?;
    let first = chain.first().ok_or_else(|| String::from("it holds no certificate"))?;
    let details = inspect::details(first)?;
    let days = details.days_left(now);
    Ok((details, days))
}

/// `cert get`.
pub fn get(options: &Get) -> Result<i32, Fail> {
    super::need_root()?;
    let now = now();
    super::need_clock(now)?;
    let name = options.name.clone();
    tuf_check_name(&name)?;
    let store = Store::system();
    let existing = {
        let lock = store.lock().map_err(fail)?;
        let found = match store.generation(&name).map_err(fail)? {
            Some(_) => Some(store.get(&name).map_err(fail)?),
            None => None,
        };
        drop(lock);
        found
    };
    let kept = match &existing {
        Some(stored) if stored.kind == Kind::Acme => stored.acme.clone(),
        Some(_) => {
            return Err(fail(format!(
                "{name} was added by hand: remove it first, or choose another name"
            )));
        }
        None => None,
    };

    // What is asked: what the command line says, else what the certificate
    // already holds, else the defaults.
    let names = if !options.names.is_empty() {
        options.names.clone()
    } else if let Some(kept) = &kept {
        kept.names.clone()
    } else if name.contains('.') {
        vec![name.clone()]
    } else {
        return Err(fail(format!(
            "{name} is not a domain: name the domains with --name, as in `cert get {name} --name \
             example.com`"
        )));
    };
    check_names(&names, options.dns).map_err(fail)?;
    let directory = if let Some(directory) = &options.directory {
        directory.clone()
    } else if options.staging || options.dry_run {
        LETS_ENCRYPT_STAGING.to_string()
    } else if let Some(kept) = &kept {
        kept.directory.clone()
    } else {
        LETS_ENCRYPT.to_string()
    };
    let directory = check_directory(&directory, &preflight::lookup).map_err(fail)?;
    let wanted = Wanted {
        names,
        key: options.key.or(kept.as_ref().map(|kept| kept.key)).unwrap_or(KeyKind::EcdsaP256),
        profile: options.profile.clone().or(kept.as_ref().and_then(|kept| kept.profile.clone())),
    };
    let terminal = crate::prompt::interactive();
    if options.dns && !terminal && !options.yes {
        return Err(fail(String::from(
            "--dns needs a person to add the TXT records: run it from a terminal, or add --yes \
             to have it wait for them to show without asking",
        )));
    }

    // A certificate with time left is not replaced by accident.
    if let Some(stored) = existing.as_ref().filter(|_| !options.dry_run)
        && let Ok((_, days)) = days_left(stored, now)
        && days > REPLACE_ASKS_DAYS
        && terminal
        && !options.yes
        && !crate::prompt::confirm(&format!("{name} has {days} days left: get a new one now?"))
    {
        return Err(fail(String::from("nothing was changed")));
    }

    // Before the authority is asked.
    let firewall = preflight::firewall_settings();
    let looked: Vec<(String, Vec<IpAddr>)> = if options.dns {
        Vec::new()
    } else {
        wanted.names.iter().map(|name| (name.clone(), preflight::lookup(name))).collect()
    };
    // Who holds port 80 decides what the firewall must say: nginx, a service
    // that names the port, has the inbound right and only a rule that denies
    // closes it; this command has none, and a rule must allow it.
    let serving = if options.dns {
        Ok(Serving::ByHand { enter: terminal })
    } else {
        serving_for_http(nginx_runs())
    };
    let with_inbound = matches!(serving, Ok(Serving::Files(_)));
    let facts =
        preflight::gather(now, &looked, !options.dns, directory.host(), &firewall, with_inbound);
    let verdict = preflight::check(&facts);
    for warning in &verdict.warnings {
        eprintln!("cert: {warning}");
    }
    if !verdict.failures.is_empty() {
        return Err(fail(verdict.failures.join("\n")));
    }
    let serving = serving.map_err(fail)?;
    println!("the names and this machine are ready");

    let client = TufClient::default();
    let setup = Setup { client: &client, now };
    let creation =
        Creation { email: options.email.clone(), agree_terms: options.agree_terms, terminal };
    let runtime = runtime().map_err(fail)?;
    let mut say = |line: &str| println!("{line}");
    let issued = runtime
        .block_on(async {
            let account =
                open_account(&store, &client, &directory, Some(&creation), now, &mut say).await?;
            let replaces = match &existing {
                Some(stored) if !options.dry_run => replacing(&account, stored).await,
                _ => None,
            };
            order(&setup, &account, &wanted, &serving, replaces, &mut say).await
        })
        .map_err(fail)?;
    drop(serving);
    if options.dry_run {
        println!("the dry run worked: nothing was installed, as it asked");
        return Ok(0);
    }
    let acme = Acme {
        directory: directory.to_string(),
        names: wanted.names.clone(),
        challenge: if options.dns { Challenge::DnsManual } else { Challenge::Http01 },
        key: wanted.key,
        profile: wanted.profile.clone(),
        renew_at: None,
        revoked: false,
    };
    let lock = store.lock().map_err(fail)?;
    let (details, notes) = install(&store, &lock, &name, &issued, &acme, now).map_err(fail)?;
    drop(lock);
    println!(
        "{name} installed: {}, runs out {}",
        details.names.join(", "),
        super::runs_out(&details, now)
    );
    println!("issued by {}", details.issuer);
    if options.dns {
        println!(
            "certd does not renew a certificate proved by DNS by hand: renew it with `elevate cert get {name} --dns` before it runs out"
        );
    } else {
        println!("certd renews it before it runs out");
    }
    for note in &notes {
        eprintln!("cert: {note}");
    }
    if let Err(why) = tell_spark(power::CONTROL, &name) {
        eprintln!("cert: {}", spark_warning(&why));
    }
    Ok(0)
}

/// Checks the certificate's own name.
fn tuf_check_name(name: &str) -> Result<(), Fail> {
    manifest::service::check_certificate(name).map_err(fail)?;
    if name == super::machine::NAME {
        return Err(fail(format!(
            "{name} is the machine's own certificate, which certd makes: choose another name"
        )));
    }
    Ok(())
}

/// The identifier of the certificate `stored` holds, if the authority
/// takes it as one that a new order replaces.
async fn replacing(account: &Account, stored: &Stored) -> Option<CertificateIdentifier<'static>> {
    let text = String::from_utf8_lossy(&stored.chain).into_owned();
    let identifier = identifier_of(&text).ok()?;
    account.renewal_info(&identifier).await.ok()?;
    Some(identifier)
}

/// The renewal-information identifier of the first certificate of `chain`
/// (RFC 9773): its authority key identifier and its serial number.
///
/// # Errors
///
/// If the certificate does not read or lacks an authority key identifier.
pub fn identifier_of(chain: &str) -> Result<CertificateIdentifier<'static>, String> {
    let certificates = inspect::chain_of(chain)?;
    let first = certificates.first().ok_or_else(|| String::from("it holds no certificate"))?;
    let certificate = Certificate::from_der(first.as_ref())
        .map_err(|why| format!("a certificate in it does not read: {why}"))?;
    let tbs = certificate.tbs_certificate();
    let key = match tbs.get_extension::<AuthorityKeyIdentifier>() {
        Ok(Some((_, identifier))) => identifier
            .key_identifier
            .ok_or_else(|| String::from("it has no authority key identifier"))?,
        Ok(None) => return Err(String::from("it has no authority key identifier")),
        Err(why) => return Err(format!("its authority key identifier does not read: {why}")),
    };
    let serial = tbs.serial_number().as_bytes();
    Ok(CertificateIdentifier::new(Der::from_slice(key.as_bytes()), Der::from_slice(serial))
        .into_owned())
}

/// A window the authority suggests a renewal in, in seconds since 1970,
/// and the seconds to wait before asking again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Window {
    pub start: i64,
    pub end: i64,
    pub ask_again: i64,
}

/// Asks the authority when to renew `chain`: none when it gives no renewal
/// information.
///
/// # Errors
///
/// When the authority cannot be reached or does not answer as it should.
pub async fn renewal_window(
    account: &Account,
    chain: &str,
    setup: &Setup<'_>,
) -> Result<Option<Window>, String> {
    let identifier = identifier_of(chain)?;
    match account.renewal_info(&identifier).await {
        Ok((info, wait)) => Ok(Some(Window {
            start: info.suggested_window.start.unix_timestamp(),
            end: info.suggested_window.end.unix_timestamp(),
            ask_again: i64::try_from(wait.as_secs()).unwrap_or(i64::MAX),
        })),
        Err(instant_acme::Error::Unsupported(_) | instant_acme::Error::Str(_)) => Ok(None),
        Err(why) => {
            Err(problems::explain(&why, &[], setup.client.retry_after().as_deref(), setup.now))
        }
    }
}

/// Asks the authority at `directory`, through the account kept for it, when
/// to renew `chain`; none when the authority gives no renewal information.
///
/// # Errors
///
/// In plain words, when the account is missing or the authority cannot be
/// reached or does not answer as it should.
pub fn look_at_window(
    store: &Store,
    directory: &str,
    chain: &str,
    now: i64,
) -> Result<Option<Window>, String> {
    let directory = check_directory(directory, &preflight::lookup)?;
    let client = TufClient::default();
    let setup = Setup { client: &client, now };
    let runtime = runtime()?;
    runtime.block_on(async {
        let account = open_account(store, &client, &directory, None, now, &mut |_| {}).await?;
        renewal_window(&account, chain, &setup).await
    })
}

/// When to renew a certificate valid from `not_before` to `not_after`
/// (seconds since 1970) when the authority says nothing: the time two
/// thirds of its life have gone, which is 30 days before a 90-day
/// certificate ends and 15 before a 45-day one does.
pub fn third_rule(not_before: i64, not_after: i64) -> i64 {
    let life = (not_after - not_before).max(0);
    not_before + life / 3 * RENEW_AFTER_THIRDS
}

/// The time to renew a certificate at, by what the authority's `window`
/// says (none: it says nothing) and what is `kept` from an earlier look,
/// at time `now`; `random` is a number from 0 up to but not including
/// 2^64, which picks the time inside the window.
///
/// A window that has begun is shortened to start now. A kept time inside
/// the window stays, so that the time does not wander from look to look; one
/// outside it, as when the authority wants certificates replaced sooner,
/// gives way to a new pick.
pub fn renew_time(
    kept: Option<i64>,
    window: Option<(i64, i64)>,
    life: (i64, i64),
    now: i64,
    random: u64,
) -> i64 {
    let Some((start, end)) = window else { return third_rule(life.0, life.1) };
    let (start, end) = (start.min(end), start.max(end));
    if let Some(kept) = kept.filter(|kept| (start..=end).contains(kept)) {
        return kept;
    }
    let from = start.max(now).min(end.max(now));
    let to = end.max(from);
    let span = u128::try_from(to - from).unwrap_or(0);
    from + i64::try_from((span * u128::from(random)) >> 64).unwrap_or(0)
}

/// A random number for [`renew_time`].
pub fn random() -> u64 {
    let mut bytes = [0u8; 8];
    // Without random numbers the window's start: late is better than none.
    let _ = SystemRandom::new().fill(&mut bytes);
    u64::from_le_bytes(bytes)
}

/// How the certificate `name` is renewed.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Who {
    /// An administrator ran `cert renew`: a command, with no inbound right
    /// until the firewall's rules give it, and Spark's status to ask.
    Administrator,
    /// certd, a service that listens on port 80 with the inbound right.
    Certd,
}

/// Renews certificate `name` now, with what it was got with, as `who`
/// asks: the new certificate's details.
///
/// # Errors
///
/// In plain words, with what is kept unchanged.
pub fn renew(
    store: &Store,
    name: &str,
    who: Who,
    now: i64,
    say: &mut dyn FnMut(&str),
) -> Result<(Details, Vec<String>), String> {
    let stored = store.get(name)?;
    let Some(kept) = stored.acme.clone().filter(|_| stored.kind == Kind::Acme) else {
        return Err(format!("{name} is not a certificate from an authority"));
    };
    if kept.challenge == Challenge::DnsManual {
        return Err(format!(
            "{name} was proved by DNS by hand, which certd cannot repeat: renew it with `elevate \
             cert get {name} --dns`"
        ));
    }
    let directory = check_directory(&kept.directory, &preflight::lookup)?;
    let wanted = Wanted { names: kept.names.clone(), key: kept.key, profile: kept.profile.clone() };
    let firewall = preflight::firewall_settings();
    let looked: Vec<(String, Vec<IpAddr>)> =
        wanted.names.iter().map(|name| (name.clone(), preflight::lookup(name))).collect();
    // As `cert get` does: nginx holding port 80 has the inbound right a
    // command of an administrator has not; certd, a service, has it itself.
    let serving = serving_for_http(match who {
        Who::Administrator => nginx_runs(),
        Who::Certd => None,
    });
    let with_inbound = who == Who::Certd || matches!(serving, Ok(Serving::Files(_)));
    let facts = preflight::gather(now, &looked, true, directory.host(), &firewall, with_inbound);
    let verdict = preflight::check(&facts);
    if !verdict.failures.is_empty() {
        return Err(verdict.failures.join("; "));
    }
    let serving = serving?;
    let client = TufClient::default();
    let setup = Setup { client: &client, now };
    let runtime = runtime()?;
    let issued = runtime.block_on(async {
        let account = open_account(store, &client, &directory, None, now, say).await?;
        let replaces = replacing(&account, &stored).await;
        order(&setup, &account, &wanted, &serving, replaces, say).await
    })?;
    drop(serving);
    let fresh = Acme { renew_at: None, revoked: false, ..kept };
    let lock = store.lock()?;
    install(store, &lock, name, &issued, &fresh, now)
}

/// The reasons `cert revoke` takes, by name.
pub fn revocation_reason(word: &str) -> Result<RevocationReason, String> {
    match word {
        "unspecified" => Ok(RevocationReason::Unspecified),
        "key-compromise" => Ok(RevocationReason::KeyCompromise),
        "superseded" => Ok(RevocationReason::Superseded),
        "cessation-of-operation" => Ok(RevocationReason::CessationOfOperation),
        other => Err(format!(
            "`{other}` is not a reason: unspecified, key-compromise, superseded or \
             cessation-of-operation"
        )),
    }
}

/// `cert revoke NAME [--reason REASON]`.
pub fn revoke(name: &str, reason: Option<&str>) -> Result<i32, Fail> {
    super::need_root()?;
    let now = now();
    super::need_clock(now)?;
    manifest::service::check_certificate(name).map_err(fail)?;
    let reason = reason.map(revocation_reason).transpose().map_err(fail)?;
    let store = Store::system();
    let (stored, kept) = {
        let _lock = store.lock().map_err(fail)?;
        if store.generation(name).map_err(fail)?.is_none() {
            return Err(fail(format!("there is no certificate {name}")));
        }
        let stored = store.get(name).map_err(fail)?;
        let kept = stored.acme.clone().filter(|_| stored.kind == Kind::Acme);
        (stored, kept)
    };
    let Some(mut kept) = kept else {
        return Err(fail(String::from("only a certificate from an authority can be revoked")));
    };
    let text = String::from_utf8_lossy(&stored.chain).into_owned();
    let chain = inspect::chain_of(&text).map_err(fail)?;
    let first: CertificateDer<'static> =
        chain.first().cloned().ok_or_else(|| fail(String::from("it holds no certificate")))?;
    let directory = check_directory(&kept.directory, &preflight::lookup).map_err(fail)?;
    let client = TufClient::default();
    let runtime = runtime().map_err(fail)?;
    let mut say = |line: &str| println!("{line}");
    runtime
        .block_on(async {
            let account = open_account(&store, &client, &directory, None, now, &mut say).await?;
            account.revoke(&RevocationRequest { certificate: &first, reason }).await.map_err(
                |why| problems::explain(&why, &kept.names, client.retry_after().as_deref(), now),
            )
        })
        .map_err(fail)?;
    kept.revoked = true;
    let lock = store.lock().map_err(fail)?;
    store.set_acme(&lock, name, &kept).map_err(fail)?;
    drop(lock);
    println!(
        "{name} revoked: the services keep using it until it is replaced: get a new one with \
         `elevate cert get {name}`"
    );
    Ok(0)
}

#[cfg(test)]
mod tests;

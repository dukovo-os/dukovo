// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The checks on each request, and the headers on each answer (ADR
//! 0514): the `Host` must name this machine, so that no other name's DNS
//! can point a browser at it (DNS rebinding); a request that changes
//! anything must pass three checks against cross-site requests (CSRF):
//! its `Origin` is the console's own, `Sec-Fetch-Site` is absent or
//! `same-origin`, and `X-Dukovo-CSRF` holds the session's token; and the
//! cookies are `__Host-` cookies, `Secure`, `HttpOnly` and
//! `SameSite=Strict`, which no other site's page sends.

use std::net::IpAddr;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;

use crate::authd::proofs;

/// The session's cookie, and the cookie of a browser trusted for 30
/// days.
pub const SESSION_COOKIE: &str = "__Host-dukovo";
pub const TRUST_COOKIE: &str = "__Host-dukovo-trust";

/// The header that carries the session's CSRF token.
pub const CSRF_HEADER: &str = "x-dukovo-csrf";

/// The value of cookie `name` in the `Cookie` headers `headers`, if it is
/// there once: a cookie set twice, as another site's page on a sibling
/// name could add one, is no cookie.
pub fn cookie<'a>(headers: impl IntoIterator<Item = &'a str>, name: &str) -> Option<&'a str> {
    let mut found = None;
    for header in headers {
        for pair in header.split(';') {
            let Some((key, value)) = pair.trim().split_once('=') else { continue };
            if key == name {
                if found.is_some() {
                    return None;
                }
                found = Some(value);
            }
        }
    }
    found
}

/// A `Set-Cookie` value: `value` for cookie `name`, kept `max_age`
/// seconds, or for the browser's session without one.
pub fn set_cookie(name: &str, value: &str, max_age: Option<u64>) -> String {
    let age = max_age.map_or_else(String::new, |seconds| format!("; Max-Age={seconds}"));
    format!("{name}={value}; Path=/; Secure; HttpOnly; SameSite=Strict{age}")
}

/// A `Set-Cookie` value that removes cookie `name`.
pub fn clear_cookie(name: &str) -> String {
    set_cookie(name, "", Some(0))
}

/// The host a request's `Host` header names, without its port, and the
/// port it asked for (443 when it names none), if the host is this
/// machine: one of its `names` (its host name, that name with `.local`,
/// `localhost`, and its certificate's names), or one of its `addresses`,
/// loopback included. Names are compared without case.
///
/// Any port is taken. A router's forwarded port or an SSH tunnel reaches
/// admind from another, and the browser writes that one; what keeps
/// another name's DNS from pointing a browser here is the name, and the
/// origin a change must come from is made of both (`origin`).
pub fn host<'a>(header: &'a str, names: &[String], addresses: &[IpAddr]) -> Option<(&'a str, u16)> {
    let (host, given) = if let Some(rest) = header.strip_prefix('[') {
        let (inside, after) = rest.split_once(']')?;
        (inside, if after.is_empty() { None } else { Some(after.strip_prefix(':')?) })
    } else {
        match header.rsplit_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (header, None),
        }
    };
    let port = match given {
        // Digits alone: Rust would read `+80` as 80, which no browser
        // writes and so no origin could match.
        Some(given) if given.bytes().all(|b| b.is_ascii_digit()) => {
            given.parse::<u16>().ok().filter(|port| *port != 0)?
        }
        Some(_) => return None,
        None => 443,
    };
    if host.is_empty() {
        return None;
    }
    let known = match host.parse::<IpAddr>() {
        Ok(ip) => ip.is_loopback() || addresses.contains(&super::reach::plain(ip)),
        Err(_) => {
            host.eq_ignore_ascii_case("localhost")
                || names.iter().any(|name| name.eq_ignore_ascii_case(host))
        }
    };
    known.then_some((host, port))
}

/// The console's origin for a request to `host` on `port`, as a browser
/// writes it in `Origin`.
pub fn origin(host: &str, port: u16) -> String {
    let host = if host.contains(':') { format!("[{host}]") } else { host.to_ascii_lowercase() };
    if port == 443 { format!("https://{host}") } else { format!("https://{host}:{port}") }
}

/// Why a request of `method` is refused as a cross-site request, if it
/// is: one that changes anything (not `GET` or `HEAD`) must come from
/// `expected`, the console's origin, with `Sec-Fetch-Site` absent or
/// `same-origin`, and carry the session's token `csrf`.
pub fn csrf_refusal(
    method: &str,
    origin: Option<&str>,
    fetch_site: Option<&str>,
    token: Option<&str>,
    expected: &str,
    csrf: &[u8; 32],
) -> Option<&'static str> {
    if matches!(method, "GET" | "HEAD") {
        return None;
    }
    page_refusal(origin, fetch_site, token, expected, csrf)
}

/// Why a request is refused as not the console's page's, if it is: it
/// must come from `expected`, the console's origin, with `Sec-Fetch-Site`
/// absent or `same-origin`, and carry the session's token `csrf`. The
/// Terminal's socket, a `GET`, is held to these as a change is, with the
/// token among its protocols.
pub fn page_refusal(
    origin: Option<&str>,
    fetch_site: Option<&str>,
    token: Option<&str>,
    expected: &str,
    csrf: &[u8; 32],
) -> Option<&'static str> {
    if origin.is_none_or(|origin| !origin.eq_ignore_ascii_case(expected)) {
        return Some("the request did not come from the console's own page");
    }
    if fetch_site.is_some_and(|site| site != "same-origin") {
        return Some("the request came from another site");
    }
    let given = token.and_then(|token| URL_SAFE_NO_PAD.decode(token).ok());
    if !given.is_some_and(|given| proofs::same(&given, csrf)) {
        return Some("the request did not carry the session's token: reload the page");
    }
    None
}

/// Why a request that is not signed in yet, and changes anything, is
/// refused, if it is: the sign-in routes have no session token, so their
/// `Origin` and `Sec-Fetch-Site` are what keep another site from signing
/// a browser in as someone else.
pub fn sign_in_refusal(
    origin: Option<&str>,
    fetch_site: Option<&str>,
    expected: &str,
) -> Option<&'static str> {
    if origin.is_none_or(|origin| !origin.eq_ignore_ascii_case(expected)) {
        return Some("the request did not come from the console's own page");
    }
    if fetch_site.is_some_and(|site| site != "same-origin") {
        return Some("the request came from another site");
    }
    None
}

/// The content security policy: the console's own files alone, no
/// inline script, no frames around it. The service worker that shows an
/// alert's notice and the manifest of the home-screen entry are files of
/// the console's own too.
pub const POLICY: &str = "default-src 'self'; script-src 'self'; worker-src 'self'; \
                          manifest-src 'self'; style-src 'self'; img-src 'self' data:; \
                          connect-src 'self'; font-src 'self'; frame-ancestors 'none'; \
                          base-uri 'none'; form-action 'self'; object-src 'none'";

/// The headers every answer carries. `authority` says whether admind
/// serves an authority's certificate: only then does it ask browsers to
/// come back over HTTPS alone (HSTS), which would stop the one-time
/// warning for the machine's own certificate from being passed.
pub fn headers(authority: bool) -> Vec<(&'static str, &'static str)> {
    let mut headers = vec![
        ("content-security-policy", POLICY),
        ("x-content-type-options", "nosniff"),
        ("referrer-policy", "no-referrer"),
        ("cross-origin-opener-policy", "same-origin"),
        ("cross-origin-resource-policy", "same-origin"),
        (
            "permissions-policy",
            "camera=(), microphone=(), geolocation=(), publickey-credentials-get=(self), \
             publickey-credentials-create=(self)",
        ),
    ];
    if authority {
        headers.push(("strict-transport-security", "max-age=31536000"));
    }
    headers
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cookie_counts_once() {
        let headers = ["a=1; __Host-dukovo=abc", "b=2"];
        assert_eq!(cookie(headers, SESSION_COOKIE), Some("abc"));
        assert_eq!(cookie(["__Host-dukovo=abc; __Host-dukovo=def"], SESSION_COOKIE), None);
        assert_eq!(cookie(["__Host-dukovo=abc", "__Host-dukovo=abc"], SESSION_COOKIE), None);
        assert_eq!(cookie(["__Host-dukovo-trust=t"], SESSION_COOKIE), None);
        assert_eq!(cookie(["__Host-dukovo-trust=t"], TRUST_COOKIE), Some("t"));
        assert_eq!(cookie([], SESSION_COOKIE), None);
    }

    #[test]
    fn cookies_are_host_only_secure_and_strict() {
        assert_eq!(
            set_cookie(SESSION_COOKIE, "abc", None),
            "__Host-dukovo=abc; Path=/; Secure; HttpOnly; SameSite=Strict"
        );
        assert_eq!(
            set_cookie(TRUST_COOKIE, "t", Some(2_592_000)),
            "__Host-dukovo-trust=t; Path=/; Secure; HttpOnly; SameSite=Strict; Max-Age=2592000"
        );
        assert!(clear_cookie(SESSION_COOKIE).ends_with("Max-Age=0"));
    }

    #[test]
    fn the_host_must_name_this_machine() {
        let names =
            vec!["duke-nas".to_string(), "duke-nas.local".into(), "admin.example.org".into()];
        let addresses: Vec<IpAddr> =
            vec!["192.168.1.10".parse().unwrap(), "fd00::10".parse().unwrap()];
        let host = |header: &'static str| host(header, &names, &addresses);
        assert_eq!(host("duke-nas.local:9090"), Some(("duke-nas.local", 9090)));
        assert_eq!(host("DUKE-NAS.LOCAL:9090"), Some(("DUKE-NAS.LOCAL", 9090)));
        assert_eq!(host("admin.example.org:9090"), Some(("admin.example.org", 9090)));
        assert_eq!(host("192.168.1.10:9090"), Some(("192.168.1.10", 9090)));
        assert_eq!(host("[fd00::10]:9090"), Some(("fd00::10", 9090)));
        assert_eq!(host("localhost:9090"), Some(("localhost", 9090)));
        assert_eq!(host("127.0.0.1:9090"), Some(("127.0.0.1", 9090)));
        assert_eq!(host("[::1]:9090"), Some(("::1", 9090)));
        // Through a forwarded port or a tunnel, the port is another, and
        // with none it is HTTPS's.
        assert_eq!(host("localhost:8443"), Some(("localhost", 8443)));
        assert_eq!(host("duke-nas.local"), Some(("duke-nas.local", 443)));
        assert_eq!(host("[fd00::10]"), Some(("fd00::10", 443)));
        for wrong in [
            "evil.example:9090",
            "duke-nas.local.evil.example:9090",
            "duke-nas.local:90900",
            "duke-nas.local:0",
            "duke-nas.local:+9090",
            "duke-nas.local:",
            "10.9.9.9:9090",
            "[fd00::11]:9090",
            "[fd00::10]9090",
            ":9090",
            "",
        ] {
            assert_eq!(host(wrong), None, "{wrong}");
        }
    }

    #[test]
    fn origins_are_as_a_browser_writes_them() {
        assert_eq!(origin("Duke-NAS.local", 9090), "https://duke-nas.local:9090");
        assert_eq!(origin("duke-nas.local", 443), "https://duke-nas.local");
        assert_eq!(origin("fd00::10", 9090), "https://[fd00::10]:9090");
    }

    #[test]
    fn a_change_needs_the_origin_the_fetch_site_and_the_token() {
        let csrf = [9u8; 32];
        let token = URL_SAFE_NO_PAD.encode(csrf);
        let expected = "https://duke-nas.local:9090";
        let check = |method, origin, site, token: Option<&str>| {
            csrf_refusal(method, origin, site, token, expected, &csrf)
        };
        assert_eq!(check("POST", Some(expected), Some("same-origin"), Some(&token)), None);
        assert_eq!(check("POST", Some(expected), None, Some(&token)), None);
        assert_eq!(check("GET", None, Some("cross-site"), None), None);
        assert!(check("POST", None, None, Some(&token)).is_some());
        assert!(check("POST", Some("https://evil.example"), None, Some(&token)).is_some());
        assert!(check("POST", Some("http://duke-nas.local:9090"), None, Some(&token)).is_some());
        assert!(check("POST", Some(expected), Some("cross-site"), Some(&token)).is_some());
        assert!(check("POST", Some(expected), Some("same-site"), Some(&token)).is_some());
        assert!(check("POST", Some(expected), Some("none"), Some(&token)).is_some());
        assert!(check("POST", Some(expected), None, None).is_some());
        assert!(check("POST", Some(expected), None, Some("AAAA")).is_some());
        assert!(
            check("DELETE", Some(expected), None, Some(&URL_SAFE_NO_PAD.encode([8u8; 32])))
                .is_some()
        );
        assert!(sign_in_refusal(Some(expected), None, expected).is_none());
        assert!(sign_in_refusal(None, None, expected).is_some());
        assert!(sign_in_refusal(Some(expected), Some("cross-site"), expected).is_some());
    }

    #[test]
    fn every_answer_carries_the_headers_and_hsts_only_for_an_authoritys_certificate() {
        let names = |authority| headers(authority).into_iter().map(|(n, _)| n).collect::<Vec<_>>();
        assert!(names(false).contains(&"content-security-policy"));
        assert!(!names(false).contains(&"strict-transport-security"));
        assert!(names(true).contains(&"strict-transport-security"));
        assert!(!POLICY.contains("unsafe"));
    }

    #[test]
    fn the_policy_lets_the_console_start_its_own_worker_and_read_its_own_manifest_and_nothing_else()
    {
        let directives: Vec<&str> = POLICY.split("; ").collect();
        for wanted in ["worker-src 'self'", "manifest-src 'self'", "script-src 'self'"] {
            assert_eq!(directives.iter().filter(|each| **each == wanted).count(), 1, "{wanted}");
        }
        // Each directive is named once, and a worker has no wider source than a script.
        let mut names: Vec<&str> =
            directives.iter().map(|each| each.split(' ').next().unwrap_or_default()).collect();
        let all = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), all, "{POLICY}");
        assert!(!POLICY.contains('*') && !POLICY.contains("http"), "{POLICY}");
    }
}

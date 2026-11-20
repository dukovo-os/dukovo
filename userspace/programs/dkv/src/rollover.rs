// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Moving a bundle to a new key (ADR 0592). A bundle's ID is bound to the
//! key that signed its first install; a publisher that must change keys
//! puts a `rollover` file in the bundle, beside its list, and signs the
//! bundle with the new key. Each link of the file says, under the old
//! key's signature, that the ID moves from that key to the next, and when;
//! bundled follows the links from the key the ID is bound to, to the key
//! that signed the bundle, and binds the ID to that key. A link is never
//! followed backwards: once an ID is bound to the new key, a file the old
//! key signs is refused like any other.
//!
//! A link is five lines and the signature minisign writes over them, four
//! more; links are in the order of their times, an empty line between
//! them:
//!
//! ```text
//! dukovo-rollover 1
//! id org.example.app
//! from RW...
//! to RW...
//! since 1795000000
//! untrusted comment: ...
//! (the signature, its trusted comment, and the global signature)
//! ```

use crate::minisign::{PublicKey, SecretKey, Signature};

/// The rollover file's name in a bundle's folder.
pub const FILE: &str = "rollover";
/// The most bytes a rollover file may hold: its most links, each well
/// under 1 KiB.
pub const FILE_MAX: u64 = 16 << 10;
/// The most links a rollover file holds.
pub const LINKS_MAX: usize = 8;
/// A link's first line, which names this format and its version.
const HEADER: &str = "dukovo-rollover 1";

/// A link: the ID moves from one key to the next, from a time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Link {
    /// The base ID of the bundle, without a line.
    pub id: String,
    /// The key it moves from, which signed the link.
    pub from: PublicKey,
    /// The key it moves to.
    pub to: PublicKey,
    /// When, in seconds since 1970.
    pub since: i64,
}

/// The five lines a link's signature covers.
pub fn statement(id: &str, from: &PublicKey, to: &PublicKey, since: i64) -> String {
    format!("{HEADER}\nid {id}\nfrom {}\nto {}\nsince {since}\n", from.encoded(), to.encoded())
}

/// A whole link, moving `id` from `secret`'s key to `to` at `since`, for
/// `bundle key rollover` to write.
pub fn link(secret: &SecretKey, id: &str, to: &PublicKey, since: i64) -> String {
    let text = statement(id, &secret.public(), to, since);
    let untrusted = format!("{id} moves from key {} to key {}", secret.id, to.id);
    let signature = secret.sign(text.as_bytes(), &untrusted, HEADER);
    format!("{text}{}", signature.file())
}

/// Reads a rollover file, checking each link's signature by the key it
/// moves from.
///
/// # Errors
///
/// If a link is not written as one is, its signature does not match, or
/// there are more than [`LINKS_MAX`].
pub fn read(text: &str) -> Result<Vec<Link>, String> {
    let body = text.strip_suffix('\n').ok_or("it does not end with a whole line")?;
    let lines: Vec<&str> = body.split('\n').collect();
    let mut links = Vec::new();
    for (n, block) in lines.split(|line| line.is_empty()).enumerate() {
        if n == LINKS_MAX {
            return Err(format!("it has more than {LINKS_MAX} links"));
        }
        links.push(read_link(block).map_err(|why| format!("link {}: {why}", n + 1))?);
    }
    Ok(links)
}

/// Reads and checks one link's nine lines.
fn read_link(lines: &[&str]) -> Result<Link, String> {
    let [header, id, from, to, since, signature @ ..] = lines else {
        return Err("it is cut short".into());
    };
    let field = |line: &'_ str, name: &str| -> Result<String, String> {
        let value = line.strip_prefix(name).and_then(|rest| rest.strip_prefix(' '));
        value.map(String::from).ok_or_else(|| format!("its line {name} is missing"))
    };
    if *header != HEADER {
        return Err(format!("it does not start with {HEADER}"));
    }
    let id = field(id, "id")?;
    let from = PublicKey::decode(&field(from, "from")?)?;
    let to = PublicKey::decode(&field(to, "to")?)?;
    let since = field(since, "since")?;
    if since.is_empty() || !since.bytes().all(|b| b.is_ascii_digit()) {
        return Err(format!("{since:?} is not a time"));
    }
    let since: i64 = since.parse().map_err(|_| format!("{since} is not a time"))?;
    let text = statement(&id, &from, &to, since);
    // Only the form `statement` writes is signed, so that no two texts
    // say the same link.
    if text.strip_suffix('\n') != Some(&lines[..5].join("\n")) {
        return Err("it is not written as a link is".into());
    }
    if signature.len() != 4 {
        return Err("its signature is not four lines".into());
    }
    let signature = Signature::read(&format!("{}\n", signature.join("\n")))?;
    from.verify(text.as_bytes(), &signature)
        .map_err(|why| format!("the key it moves from did not sign it: {why}"))?;
    Ok(Link { id, from, to, since })
}

/// Follows `links` from `bound`, the key `id` is bound to, to `signer`,
/// the key that signed the bundle at `signed_at` if its signature says
/// when. The links must be one chain, in the order of their times, with no
/// key twice, ending at `signer`; `bound` may be any key along it, as a
/// publisher keeps its older links. Returns when the ID moved to `signer`.
///
/// # Errors
///
/// Why the links do not lead from `bound` to `signer`.
pub fn chain(
    links: &[Link],
    id: &str,
    bound: &PublicKey,
    signer: &PublicKey,
    signed_at: Option<i64>,
) -> Result<i64, String> {
    let Some(last) = links.last() else { return Err("it has no links".into()) };
    let mut seen = vec![links[0].from];
    for (n, link) in links.iter().enumerate() {
        if link.id != id {
            return Err(format!("link {} moves {}, not {id}", n + 1, link.id));
        }
        if n > 0 && (link.from != links[n - 1].to || link.since <= links[n - 1].since) {
            return Err(format!("link {} does not follow on from the one before it", n + 1));
        }
        if seen.contains(&link.to) {
            return Err(format!("link {} moves back to a key it moved from", n + 1));
        }
        seen.push(link.to);
    }
    if last.to != *signer {
        return Err("its last link is not to the key that signed the bundle".into());
    }
    if signed_at.is_some_and(|at| last.since > at) {
        return Err("its last link is from after the bundle was signed".into());
    }
    if !links.iter().any(|link| link.from == *bound) {
        return Err(format!("no link moves {id} from key {}, which it is bound to", bound.id));
    }
    Ok(last.since)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A key made from `seed`; the seed never makes the 32 zero bytes
    /// `generate` refuses, as these tests' seeds are under 0x5a.
    fn key(seed: u8) -> SecretKey {
        SecretKey::generate(&[seed ^ 0x5a; 40]).unwrap()
    }

    const ID: &str = "org.example.app";

    #[test]
    fn a_chain_of_links_reads_and_leads_on() {
        let (a, b, c) = (key(1), key(2), key(3));
        let text =
            format!("{}\n{}", link(&a, ID, &b.public(), 1_000), link(&b, ID, &c.public(), 2_000));
        let links = read(&text).unwrap();
        assert_eq!(links.len(), 2);
        assert_eq!((links[0].from, links[1].to, links[1].since), (a.public(), c.public(), 2_000));
        // From the first key or the middle one, to the signer.
        for bound in [a.public(), b.public()] {
            assert_eq!(chain(&links, ID, &bound, &c.public(), Some(3_000)), Ok(2_000));
        }
        // Not from the signer itself, a key the links do not name, or to
        // another key, and not before the last link.
        let refused = [
            (c.public(), c.public(), Some(3_000), "no link moves"),
            (key(9).public(), c.public(), Some(3_000), "no link moves"),
            (a.public(), b.public(), Some(3_000), "last link is not to the key"),
            (a.public(), c.public(), Some(1_500), "after the bundle was signed"),
        ];
        for (bound, signer, at, why) in refused {
            let error = chain(&links, ID, &bound, &signer, at).unwrap_err();
            assert!(error.contains(why), "{error}");
        }
        assert!(chain(&links, "org.example.other", &a.public(), &c.public(), None).is_err());
    }

    #[test]
    fn links_that_do_not_hold_together_are_refused() {
        let (a, b, c) = (key(1), key(2), key(3));
        let ab = link(&a, ID, &b.public(), 1_000);
        let chains = [
            // Out of order in time, a gap between keys, and a loop back.
            vec![ab.clone(), link(&b, ID, &c.public(), 500)],
            vec![ab.clone(), link(&a, ID, &c.public(), 2_000)],
            vec![ab.clone(), link(&b, ID, &a.public(), 2_000)],
            vec![ab.clone(), link(&b, "org.example.other", &c.public(), 2_000)],
        ];
        for texts in chains {
            let links = read(&texts.join("\n")).unwrap();
            assert!(chain(&links, ID, &a.public(), &c.public(), None).is_err(), "{texts:?}");
        }
        let nine: Vec<String> =
            (0..9).map(|n| link(&key(n), ID, &key(n + 1).public(), i64::from(n))).collect();
        assert!(read(&nine.join("\n")).unwrap_err().contains("more than 8 links"));
    }

    #[test]
    fn a_link_signed_by_another_key_or_changed_is_refused() {
        let (a, b, c) = (key(1), key(2), key(3));
        let good = link(&a, ID, &b.public(), 1_000);
        // Signed by the key it moves to, not from.
        let text = statement(ID, &a.public(), &b.public(), 1_000);
        let forged = b.sign(text.as_bytes(), "", HEADER);
        let forged = format!("{text}{}", forged.file());
        let cases = [
            (forged, "did not sign it"),
            (good.replace("since 1000", "since 1001"), "did not sign it"),
            (good.replace(&b.public().encoded(), &c.public().encoded()), "did not sign it"),
            (good.replace("since 1000", "since +1000"), "not a time"),
            (good.replace("since 1000", "since 01000"), "not written as a link is"),
            (good.replace(HEADER, "dukovo-rollover 2"), "does not start with"),
            (good.trim_end().to_string(), "whole line"),
            (format!("{}\n", good.lines().take(7).collect::<Vec<_>>().join("\n")), "four lines"),
            (format!("{good}\n\n{good}"), "cut short"),
        ];
        for (text, why) in cases {
            let error = read(&text).unwrap_err();
            assert!(error.contains(why), "{why}: {error}");
        }
    }

    #[test]
    fn no_text_makes_it_panic() {
        let (a, b) = (key(1), key(2));
        let good = format!("{}\n{}", link(&a, ID, &b.public(), 1), link(&b, ID, &a.public(), 2));
        let bytes = good.as_bytes();
        for i in 0..bytes.len() {
            for flip in [0x01, 0x20, 0x0a ^ bytes[i]] {
                let mut changed = bytes.to_vec();
                changed[i] ^= flip;
                if let Ok(text) = std::str::from_utf8(&changed) {
                    let _ = read(text);
                }
            }
        }
        for cut in 0..bytes.len() {
            let _ = read(&good[..cut]);
        }
    }
}

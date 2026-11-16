// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `user steps reset NAME` and `user token remove NAME TOKEN`: what an
//! administrator does for a person who lost the phone or key of their
//! second step, and for an API token that should stop working (ADR 0562).
//! authd does both, as root's to ask, and keeps the audit lines.

use accounts::request::Request;

use super::{Accounts, ask, done, operands};
use crate::args::Args;
use crate::util::{Fail, usage};

/// `user steps reset NAME`: removes NAME's authenticator app, passkeys,
/// recovery codes and trusted browsers, and says what went. The person
/// signs in with their password alone, and is told why at their next
/// sign-in.
pub(super) fn steps(args: &mut Args) -> Result<i32, Fail> {
    let help = "user steps reset name";
    match args.subcommand().as_deref().and_then(|word| word.to_str()) {
        Some("reset") => {}
        _ => return Err(usage(format!("which? reset\nUsage: {help}"))),
    }
    let (names, _) = operands(args, "user steps reset needs a name", 1, |_, _| Ok(false))?;
    let name = &names[0];
    Accounts::read()?.user(name)?;
    let answer = ask(&Request::StepsReset { name: name.clone() })?;
    done(answer, |went| println!("{}", reset_words(name, went)))
}

/// What `user steps reset` says of authd's answer `went`: how many of the
/// app, passkeys, recovery codes and trusted browsers went.
fn reset_words(name: &str, went: &str) -> String {
    let counts: Vec<usize> = went.split(' ').filter_map(|n| n.parse().ok()).collect();
    let [app, passkeys, codes, trusted] = counts[..] else {
        return format!("Removed the second steps of {name}.");
    };
    let counted = |n: usize, one: &str, many: &str| match n {
        1 => format!("1 {one}"),
        n => format!("{n} {many}"),
    };
    let mut parts = Vec::new();
    if app > 0 {
        parts.push("the authenticator app".to_string());
    }
    if passkeys > 0 {
        parts.push(counted(passkeys, "passkey", "passkeys"));
    }
    if codes > 0 {
        parts.push(counted(codes, "recovery code", "recovery codes"));
    }
    if trusted > 0 {
        parts.push(counted(trusted, "trusted browser", "trusted browsers"));
    }
    let went = match parts.as_slice() {
        [] => return format!("{name} had no second steps: nothing was removed."),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    };
    format!(
        "Removed {went} of {name}. They sign in with their password, and add a new second step \
         in Your account."
    )
}

/// `user token remove NAME TOKEN`: removes NAME's API token TOKEN, and
/// whatever uses it stops working.
pub(super) fn token(args: &mut Args) -> Result<i32, Fail> {
    let help = "user token remove name token";
    match args.subcommand().as_deref().and_then(|word| word.to_str()) {
        Some("remove") => {}
        _ => return Err(usage(format!("which? remove\nUsage: {help}"))),
    }
    let (names, _) =
        operands(args, "user token remove needs an account and a token", 2, |_, _| Ok(false))?;
    let (name, token) = (&names[0], &names[1]);
    Accounts::read()?.user(name)?;
    let answer = ask(&Request::TokenRemoveOf { name: name.clone(), token: token.clone() })?;
    done(answer, |_| {
        println!("Removed the API token {token} of {name}: whatever uses it stops working.");
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reset_says_what_went() {
        assert_eq!(
            reset_words("bob", "1 2 8 1"),
            "Removed the authenticator app, 2 passkeys, 8 recovery codes and 1 trusted browser \
             of bob. They sign in with their password, and add a new second step in Your \
             account."
        );
        assert_eq!(
            reset_words("bob", "0 1 0 0"),
            "Removed 1 passkey of bob. They sign in with their password, and add a new second \
             step in Your account."
        );
        assert_eq!(reset_words("bob", "0 0 0 0"), "bob had no second steps: nothing was removed.");
        assert_eq!(reset_words("bob", "odd"), "Removed the second steps of bob.");
    }
}

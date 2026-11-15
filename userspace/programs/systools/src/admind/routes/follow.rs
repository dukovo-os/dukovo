// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `GET /api/1/logs/follow` (ADR 0518): the Logs page's follow of the log,
//! as server-sent events ([`crate::admind::follow`]).
//!
//! It takes `service`, `kernel`, `priority` and `grep` as `logs.read`
//! does, checked by the same code, and `all=true` for the whole log, which
//! needs an administrator's unlocked session (ADM-122, ADM-106). As an
//! elevated read, it must come from the console's own page (ADM-109). A
//! ninth follow is ADM-134, with a wait of 30 seconds. A refusal is a JSON
//! answer before the stream begins; once it begins, its `ended` event says
//! why it ends.

use std::rc::Rc;

use accounts::console::Console;
use http::StatusCode;

use super::{Content, Context, Live, Outcome, Reply, unlocked};
use crate::admind::actions::{self, Params, logs, show};
use crate::admind::errors::{Failure, Kind};
use crate::admind::follow::{self, Asked, FOLLOWS_MAX, RETRY_AFTER};
use crate::admind::shared::Shared;
use crate::admind::who::{self, Account};
use crate::log;

/// The follow a request asks for, checked: its arguments, and whether the
/// session may read the whole log.
///
/// # Errors
///
/// ADM-108 for a query that does not read, ADM-125 for a filter that
/// cannot work, ADM-122 and ADM-106 for the whole log.
fn asked(shared: &Rc<Shared>, query: &str, live: &Live) -> Result<logs::Follow, Failure> {
    let args = actions::query_arguments(Params::new(), query, &logs::Follow::FIELDS)?;
    let follow: logs::Follow = serde_json::from_value(args)
        .map_err(|err| Failure::new(Kind::BadRequest, format!("the follow's arguments: {err}")))?;
    follow.check().map_err(|bad| Failure::bad_value(bad.field, bad.why))?;
    if follow.all {
        let who = who::read(&shared.accounts, Account::Name(&live.name))
            .map_err(|why| Failure::new(Kind::Internal, why))?;
        let unlocked = unlocked(shared, &live.cookie);
        actions::session_may_read_elevated(&live.name, who.admin, unlocked, logs::AREA)?;
    }
    Ok(follow)
}

/// The command `words` as a person types it: with `elevate` for the whole
/// log.
fn shown(words: &[String], all: bool) -> String {
    let line: Vec<String> = words.iter().map(|word| show::shell_word(word)).collect();
    let line = line.join(" ");
    if all { format!("elevate {line}") } else { line }
}

/// The refusal for a ninth follow.
fn too_many() -> Failure {
    let why = format!("the console follows {FOLLOWS_MAX} logs already");
    Failure { retry_after: Some(RETRY_AFTER), ..Failure::new(Kind::TooManyFollows, why) }
}

/// `GET /api/1/logs/follow`: checks the follow, has authd start it, and
/// answers its stream.
///
/// # Errors
///
/// As [`asked`]; ADM-109 for a request from another page; ADM-134 for a
/// ninth follow; authd's refusal.
///
/// # Panics
///
/// Outside a `LocalSet`.
pub async fn open(cx: &Context<'_>, live: &Live) -> Outcome {
    super::act::from_the_console(cx)?;
    let follow = asked(cx.shared, &cx.query, live)?;
    let filter = follow.filter();
    let words = log::console_follow(&filter).map_err(|why| Failure::new(Kind::BadValue, why))?;
    let (id, stop) =
        cx.shared.follows.borrow_mut().begin(&live.cookie, follow.all).ok_or_else(too_many)?;
    let asked = Asked {
        cookie: live.cookie.clone(),
        authd: live.authd.clone(),
        all: follow.all,
        request: Console::Follow { session: live.authd.clone(), all: follow.all, filter },
        shown: shown(&words, follow.all),
    };
    let body = follow::start(cx.shared, asked, id, stop).await.map_err(|failure| {
        if failure.kind == Kind::TooManyFollows {
            Failure { retry_after: Some(RETRY_AFTER), ..failure }
        } else {
            failure
        }
    })?;
    Ok(Reply {
        status: StatusCode::OK,
        content: Content::Follow(body),
        cookies: Vec::new(),
        retry_after: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::admind::errors::{Attempt, refusal};

    #[test]
    fn the_command_is_shown_as_typed_with_elevate_for_the_whole_log() {
        let words: Vec<String> = ["log", "--grep", "timed out"].map(String::from).to_vec();
        assert_eq!(shown(&words, false), "log --grep 'timed out'");
        assert_eq!(shown(&words, true), "elevate log --grep 'timed out'");
    }

    #[test]
    fn a_ninth_follow_waits_thirty_seconds() {
        let refused = too_many();
        assert_eq!((refused.kind, refused.retry_after), (Kind::TooManyFollows, Some(30)));
        assert_eq!(refused.detail, "the console follows 8 logs already");
    }

    #[test]
    fn refusals_of_authd_keep_their_ids() {
        let refused = refusal(Attempt::Other, "ADM-122: dave is not an administrator, and reads");
        assert_eq!(refused.kind, Kind::NotAdmin);
        let locked = refusal(Attempt::Other, "ADM-106: the session is locked: unlock it");
        assert_eq!(locked.kind, Kind::Locked);
    }
}

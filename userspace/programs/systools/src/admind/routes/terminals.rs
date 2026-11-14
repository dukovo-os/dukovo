// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The Terminal's routes (ADR 0517): the account's terminals, a new one or
//! a new pane, a layout, a name, an end, and the socket that shows them.
//!
//! They are routes, not actions: they act on live terminals, have no
//! command to show and no Ansible task. Each is for a signed-in session
//! that may do all that is a person's; an API token is refused (ADM-124),
//! since a terminal is a person's, and scripts have SSH. Each needs
//! admin.toml's `[terminal] allowed` to let the account, and an account
//! with a shell (ADM-132). An administrator's pane also needs an
//! administrator whose session is unlocked now; authd checks it all
//! again as it starts the shell.

use std::rc::Rc;

use hyper::upgrade::OnUpgrade;
use hyper_util::rt::TokioIo;
use serde::Deserialize;
use serde_json::json;

use super::{Context, Live, Outcome, Reply, single, unlocked};
use crate::admind::errors::{Failure, Kind};
use crate::admind::guard;
use crate::admind::log;
use crate::admind::shared::{Shared, unix_now};
use crate::admind::terminal::shells::{self, Asked};
use crate::admind::terminal::{
    KEEP_DAYS, KEEP_DAYS_MAX, Layout, NAME_MAX, PANES_PER_TERMINAL, SIZES, SIZES_MAX, Terminal,
    no_terminal, protocol, socket,
};
use crate::admind::websocket;
use crate::admind::who::{self, Account};

/// Whether the account of `live` may use the console's terminals, and
/// whether it is an administrator.
///
/// # Errors
///
/// ADM-132 when admin.toml does not let it, or it has no shell.
fn may_use(cx: &Context<'_>, live: &Live) -> Result<bool, Failure> {
    let who = who::read(&cx.shared.accounts, Account::Name(&live.name))
        .map_err(|why| Failure::new(Kind::Internal, why))?;
    let allowed = cx.shared.settings.borrow().terminal.allowed;
    if !who.shell {
        let why = format!("{} has no shell to start", live.name);
        return Err(Failure::new(Kind::TerminalClosed, why));
    }
    if !allowed.lets(who.admin) {
        let why = format!(
            "the console's settings do not let this account open a terminal (allowed = \"{}\" in \
             admin.toml)",
            allowed.word()
        );
        return Err(Failure::new(Kind::TerminalClosed, why));
    }
    Ok(who.admin)
}

/// Whether the account of `live` may open a terminal, for the session's
/// answer: the navigation shows the Terminal then.
pub fn offered(shared: &Shared, name: &str) -> bool {
    let Ok(who) = who::read(&shared.accounts, Account::Name(name)) else { return false };
    who.shell && shared.settings.borrow().terminal.allowed.lets(who.admin)
}

/// The most terminals an account may keep.
fn most(shared: &Shared) -> u32 {
    shared.settings.borrow().terminal.sessions_per_account
}

/// The number the address gives as `{name}`.
fn number(cx: &Context<'_>, name: &str) -> Result<u32, Failure> {
    cx.params
        .get(name)
        .and_then(serde_json::Value::as_str)
        .and_then(|id| id.parse::<u32>().ok())
        .ok_or_else(no_terminal)
}

/// The failure for a value out of its range.
fn bad(field: &'static str, why: impl Into<String>) -> Failure {
    Failure::bad_value(field, why)
}

/// A pane's size, checked.
fn size(cols: u16, rows: u16) -> Result<(u16, u16), Failure> {
    if !protocol::COLUMNS.contains(&cols) {
        return Err(bad("cols", "a terminal has 2 to 500 columns"));
    }
    if !protocol::ROWS.contains(&rows) {
        return Err(bad("rows", "a terminal has 1 to 300 rows"));
    }
    Ok((cols, rows))
}

/// A terminal's name, checked: 1 to [`NAME_MAX`] characters, none a
/// control character.
fn name(name: &str) -> Result<String, Failure> {
    let name = name.trim();
    let count = name.chars().count();
    if count == 0 || count > NAME_MAX || name.chars().any(char::is_control) {
        return Err(bad("name", "a name is 1 to 40 characters, with no control character"));
    }
    Ok(name.to_string())
}

/// Whether an administrator's pane may be asked for by `live`: an
/// administrator, unlocked now.
fn may_be_root(cx: &Context<'_>, live: &Live, admin: bool) -> Result<(), Failure> {
    if !admin {
        let why = format!("{} is not an administrator", live.name);
        return Err(Failure::new(Kind::NotAdmin, why));
    }
    if !unlocked(cx.shared, &live.cookie) {
        let why = "the session is locked: unlock it for an administrator's terminal";
        return Err(Failure::new(Kind::Locked, why));
    }
    Ok(())
}

/// `GET /api/1/terminals`.
pub fn list(cx: &Context<'_>, live: &Live) -> Outcome {
    may_use(cx, live)?;
    let terminals = cx.shared.terminals.borrow();
    Ok(Reply::json(terminals.list(&live.name, &live.authd, most(cx.shared))))
}

/// `POST /api/1/terminals`: a new terminal, with its one pane.
pub async fn add(cx: &Context<'_>, live: &Live) -> Outcome {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Body {
        name: Option<String>,
        cols: u16,
        rows: u16,
        #[serde(default)]
        admin: bool,
        keep_days: Option<u32>,
    }
    let is_admin = may_use(cx, live)?;
    let body: Body = cx.json()?;
    let (cols, rows) = size(body.cols, body.rows)?;
    let keep_days = body.keep_days.unwrap_or(KEEP_DAYS);
    if !(1..=KEEP_DAYS_MAX).contains(&keep_days) {
        return Err(bad("keep_days", "a terminal is kept 1 to 30 days"));
    }
    let host = cx.shared.identity.borrow().host.clone();
    let name = match &body.name {
        Some(given) => name(given)?,
        None if body.admin => format!("root@{host}"),
        None => format!("{}@{host}", live.name),
    };
    if body.admin {
        may_be_root(cx, live, is_admin)?;
    }
    cx.shared.terminals.borrow().room_for_terminal(&live.name, most(cx.shared))?;
    let (terminal_id, pane_id) = cx.shared.terminals.borrow_mut().next_ids();
    let asked = Asked {
        id: pane_id,
        terminal: terminal_id,
        session: &live.authd,
        admin: body.admin,
        cols,
        rows,
    };
    let (pane, reader, writer) = shells::start(cx.shared, &asked).await?.into_parts();
    let tty = pane.tty().to_string();
    let now = unix_now();
    {
        let mut terminals = cx.shared.terminals.borrow_mut();
        // Others may have taken the room while authd started the shell:
        // this one ends again, as the pane drops.
        terminals.room_for_terminal(&live.name, most(cx.shared))?;
        let terminal = Terminal {
            id: terminal_id,
            account: live.name.clone(),
            name,
            made: now,
            used: now,
            from: cx.peer,
            keep_days,
            layout: Layout::One,
            sizes: Vec::new(),
            panes: Vec::new(),
        };
        terminals.add_terminal(terminal, pane);
    }
    shells::run(cx.shared, pane_id, reader, writer);
    log(
        6,
        &format!(
            "terminal {terminal_id} pane {pane_id} opened for {}{} from {} ({tty})",
            live.name,
            if body.admin { " as an administrator" } else { "" },
            cx.peer
        ),
    );
    cx.shared.terminals.borrow_mut().tell_changed(&live.name, terminal_id);
    let terminal = cx.shared.terminals.borrow().json(terminal_id, &live.authd);
    Ok(Reply::json(json!({ "terminal": terminal })))
}

/// `POST /api/1/terminals/{id}/panes`: one more pane in a terminal.
pub async fn add_pane(cx: &Context<'_>, live: &Live) -> Outcome {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Body {
        cols: u16,
        rows: u16,
        #[serde(default)]
        admin: bool,
        layout: Option<String>,
    }
    let is_admin = may_use(cx, live)?;
    let id = number(cx, "id")?;
    let body: Body = cx.json()?;
    let (cols, rows) = size(body.cols, body.rows)?;
    let layout = match &body.layout {
        Some(word) => Some(Layout::of(word).ok_or_else(|| bad("layout", "no such layout"))?),
        None => None,
    };
    let full = || {
        let why = format!("a terminal has {PANES_PER_TERMINAL} panes at most");
        Failure::new(Kind::TooManyTerminals, why)
    };
    {
        let terminals = cx.shared.terminals.borrow();
        if terminals.terminal_of(&live.name, id)?.panes.len() >= PANES_PER_TERMINAL {
            return Err(full());
        }
        terminals.room_for_pane()?;
    }
    if body.admin {
        may_be_root(cx, live, is_admin)?;
    }
    let pane_id = cx.shared.terminals.borrow_mut().next_pane_id();
    let asked =
        Asked { id: pane_id, terminal: id, session: &live.authd, admin: body.admin, cols, rows };
    let (pane, reader, writer) = shells::start(cx.shared, &asked).await?.into_parts();
    let tty = pane.tty().to_string();
    {
        // Others may have taken the room, or ended the terminal, while
        // authd started the shell: this one ends again, as the pane drops.
        let mut terminals = cx.shared.terminals.borrow_mut();
        terminals.room_for_pane()?;
        let terminal = terminals.terminal_of_mut(&live.name, id)?;
        if terminal.panes.len() >= PANES_PER_TERMINAL {
            return Err(full());
        }
        terminal.layout = match (layout, terminal.layout) {
            (Some(layout), _) => layout,
            (None, Layout::One) => Layout::Columns,
            (None, layout) => layout,
        };
        terminals.add_pane(pane);
    }
    shells::run(cx.shared, pane_id, reader, writer);
    log(
        6,
        &format!(
            "terminal {id} pane {pane_id} opened for {}{} from {} ({tty})",
            live.name,
            if body.admin { " as an administrator" } else { "" },
            cx.peer
        ),
    );
    cx.shared.terminals.borrow_mut().tell_changed(&live.name, id);
    let terminal = cx.shared.terminals.borrow().json(id, &live.authd);
    Ok(Reply::json(json!({ "terminal": terminal })))
}

/// `POST /api/1/terminals/{id}/layout`.
pub fn layout(cx: &Context<'_>, live: &Live) -> Outcome {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Body {
        layout: String,
        #[serde(default)]
        sizes: Vec<f64>,
    }
    may_use(cx, live)?;
    let id = number(cx, "id")?;
    let body: Body = cx.json()?;
    let layout = Layout::of(&body.layout).ok_or_else(|| bad("layout", "no such layout"))?;
    if body.sizes.len() > SIZES_MAX || body.sizes.iter().any(|size| !SIZES.contains(size)) {
        return Err(bad("sizes", "a layout has at most 3 sizes, each 0.2 to 0.8"));
    }
    let mut terminals = cx.shared.terminals.borrow_mut();
    let terminal = terminals.terminal_of_mut(&live.name, id)?;
    terminal.layout = layout;
    terminal.sizes = body.sizes;
    terminals.tell_changed(&live.name, id);
    Ok(Reply::json(json!({ "terminal": terminals.json(id, &live.authd) })))
}

/// `POST /api/1/terminals/{id}/rename`.
pub fn rename(cx: &Context<'_>, live: &Live) -> Outcome {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Body {
        name: String,
    }
    may_use(cx, live)?;
    let id = number(cx, "id")?;
    let body: Body = cx.json()?;
    let name = name(&body.name)?;
    let mut terminals = cx.shared.terminals.borrow_mut();
    terminals.terminal_of_mut(&live.name, id)?.name = name;
    terminals.tell_changed(&live.name, id);
    Ok(Reply::json(json!({ "terminal": terminals.json(id, &live.authd) })))
}

/// `POST /api/1/terminals/{id}/end`: each of its panes ends.
pub fn end(cx: &Context<'_>, live: &Live) -> Outcome {
    may_use(cx, live)?;
    let id = number(cx, "id")?;
    let mut terminals = cx.shared.terminals.borrow_mut();
    terminals.terminal_of(&live.name, id)?;
    terminals.end_terminal(id);
    Ok(Reply::json(json!({ "done": true })))
}

/// `POST /api/1/terminals/panes/{pane}/end`.
pub fn end_pane(cx: &Context<'_>, live: &Live) -> Outcome {
    may_use(cx, live)?;
    let pane = number(cx, "pane")?;
    let mut terminals = cx.shared.terminals.borrow_mut();
    let terminal = terminals.terminal_of_pane(pane).ok_or_else(no_terminal)?;
    terminals.terminal_of(&live.name, terminal)?;
    terminals.end_pane(pane, protocol::Why::Ended, None);
    Ok(Reply::json(json!({ "done": true })))
}

/// `GET /api/1/terminal`: the socket. The request must open a WebSocket
/// as `websocket::opening` says, come from the console's own page
/// (`Origin`, and `Sec-Fetch-Site` when sent), and carry the session's
/// token among its protocols; the session may have
/// four open. The answer is 101, and the
/// socket is served once the connection is handed over.
pub fn open(cx: &Context<'_>, live: &Live) -> Outcome {
    may_use(cx, live)?;
    let headers = &cx.headers;
    let opening = websocket::opening(headers).map_err(|why| Failure::new(Kind::BadRequest, why))?;
    let origin = single(headers, &http::header::ORIGIN);
    let fetch_site = single(headers, &http::HeaderName::from_static("sec-fetch-site"));
    if let Some(why) =
        guard::page_refusal(origin, fetch_site, Some(&opening.csrf), &cx.origin, &live.csrf)
    {
        return Err(Failure::new(Kind::ForeignRequest, why));
    }
    let Some(upgrade) = cx.upgrade.borrow_mut().take() else {
        return Err(Failure::new(Kind::BadRequest, "this connection cannot be handed over"));
    };
    let id = cx.shared.terminals.borrow_mut().open_socket(&live.name, &live.authd)?;
    let slot = cx.held.as_ref().and_then(|held| held.take());
    tokio::task::spawn_local(serve(Rc::clone(cx.shared), id, upgrade, slot));
    log(6, &format!("a terminal socket opened for {} from {}", live.name, cx.peer));
    Ok(Reply::socket(opening.accept))
}

/// Serves socket `id` once `upgrade` hands the connection over.
async fn serve(
    shared: Rc<Shared>,
    id: u64,
    upgrade: OnUpgrade,
    slot: Option<crate::admind::limits::Slot>,
) {
    match upgrade.await {
        Ok(upgraded) => socket::serve(Rc::clone(&shared), id, TokioIo::new(upgraded), slot).await,
        Err(err) => {
            log(6, &format!("a terminal socket was not handed over: {err}"));
            shared.terminals.borrow_mut().close_socket(id, std::time::Instant::now());
        }
    }
}

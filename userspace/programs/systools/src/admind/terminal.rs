// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The console's terminals (ADR 0517), in admind's memory alone: nothing
//! of one is written to disk, and an admind that stops ends them all.
//!
//! - **A terminal** is a tab of the Terminal page: its account, its name,
//!   when it was made and last opened, the address it was made from, its
//!   layout, and its panes, one to four.
//! - **A pane** is a pseudo-terminal whose master admind keeps and whose
//!   shell authd started (`pty`, authd's console terminal request): the
//!   person's own login shell, or root's for an administrator whose
//!   console session was unlocked, which ends with the unlock. Its output
//!   is kept in a ring of 64 KiB (`ring`), and the modes its program set
//!   are followed (`modes`), so that a page that comes back sees it again.
//! - **A socket** is a Terminal page's WebSocket (`socket`), which shows
//!   up to 16 panes, each from the offset it asks for.
//!
//! Ids are numbers from counters, never used twice while admind runs.
//! Every request and message checks the account: a pane of another
//! account, and one that is not there, are the same ADM-131. An
//! administrator's pane is shown only on the console session that opened
//! it.
//!
//! **Holding back a flood.** A socket acknowledges a pane's output once
//! its page has taken it in. While a socket has more than 256 KiB of a
//! pane's output not acknowledged, the pane's master is not read, so its
//! program waits as on a slow line; reading goes on once every socket is
//! under 64 KiB. A socket over for 30 seconds is no longer shown the pane
//! (`detached`, `slow`), so that one stalled browser does not hold up the
//! others. Typing waits, 64 KiB at most a pane, for the master to take it;
//! past that, the socket is not read.

mod modes;
pub(crate) mod protocol;
mod pty;
mod ring;
pub(crate) mod shells;
pub(crate) mod socket;

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::net::IpAddr;
use std::os::fd::OwnedFd;
use std::rc::Rc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tokio::sync::Notify;
use tokio::task::AbortHandle;

use self::modes::Modes;
use self::protocol::{Attached, CHUNK_MAX, Why};
use self::ring::Ring;
use super::errors::{Failure, Kind};
use super::log;
use super::shared::unix_now;
use super::websocket::{self, Code, Kind as FrameKind};

/// The most panes a terminal has.
pub const PANES_PER_TERMINAL: usize = 4;

/// The most panes admind holds, of everyone's.
pub const PANES_MAX: usize = 256;

/// The most sockets a console session has open.
pub const SOCKETS_PER_SESSION: usize = 4;

/// The most panes one socket shows.
pub const ATTACHED_MAX: usize = 16;

/// Output not acknowledged past which a pane's master is not read, and
/// under which it is read again.
pub const PAUSE_AT: u64 = 256 << 10;
pub const RESUME_AT: u64 = 64 << 10;

/// How long a socket may stay past [`PAUSE_AT`] before it is no longer
/// shown the pane.
pub const SLOW_FOR: Duration = Duration::from_secs(30);

/// The most typing that waits for a pane's master.
pub const TYPING_MAX: usize = 64 << 10;

/// The most characters of a terminal's name.
pub const NAME_MAX: usize = 40;

/// How many days a terminal no page has open is kept, unless its request
/// says, and the most it may say.
pub const KEEP_DAYS: u32 = 7;
pub const KEEP_DAYS_MAX: u32 = 30;

/// The most bytes that wait to be sent on a socket: far more than its
/// panes' acknowledgements let wait, so only a fault reaches it.
const OUTBOX_MAX: usize = 8 << 20;

/// How a terminal's panes are laid out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    One,
    Columns,
    Rows,
    MainLeft,
    Grid,
}

impl Layout {
    /// The word the page uses.
    pub fn word(self) -> &'static str {
        match self {
            Layout::One => "one",
            Layout::Columns => "columns",
            Layout::Rows => "rows",
            Layout::MainLeft => "main-left",
            Layout::Grid => "grid",
        }
    }

    /// The layout `word` names.
    pub fn of(word: &str) -> Option<Layout> {
        [Layout::One, Layout::Columns, Layout::Rows, Layout::MainLeft, Layout::Grid]
            .into_iter()
            .find(|layout| layout.word() == word)
    }
}

/// The share of the page a split gives its first side: 20 to 80 per cent.
pub const SIZES: std::ops::RangeInclusive<f64> = 0.2..=0.8;

/// The most splits a layout has.
pub const SIZES_MAX: usize = 3;

/// A tab of the Terminal page.
#[derive(Debug)]
pub struct Terminal {
    pub id: u32,
    pub account: String,
    pub name: String,
    /// When it was made and last opened, in Unix seconds.
    pub made: i64,
    pub used: i64,
    /// The browser's address it was made from.
    pub from: IpAddr,
    pub keep_days: u32,
    pub layout: Layout,
    pub sizes: Vec<f64>,
    pub panes: Vec<u32>,
}

/// What admind holds of a pane's shell.
#[derive(Debug)]
pub struct Shell {
    /// Its master.
    pub master: OwnedFd,
    /// Its first process, the session's leader.
    pub process: i32,
    /// Its terminal's name, as `pts/7`.
    pub tty: String,
}

/// A pane: a shell on a pseudo-terminal.
#[derive(Debug)]
pub struct Pane {
    pub id: u32,
    pub terminal: u32,
    /// For an administrator's pane, the console session that opened it,
    /// as authd names it.
    pub admin: Option<String>,
    shell: Shell,
    cols: u16,
    rows: u16,
    /// When it started, in Unix seconds.
    started: i64,
    ring: Ring,
    modes: Modes,
    /// What was typed and waits for the master, and its bytes.
    typing: VecDeque<Vec<u8>>,
    typed: usize,
    /// Whether its master is not read now.
    paused: bool,
    /// The sockets that show it.
    attached: BTreeSet<u64>,
    /// Woken when reading goes on, when typing is queued, and when typing
    /// was taken.
    reading: Rc<Notify>,
    writing: Rc<Notify>,
    room: Rc<Notify>,
    /// Its tasks: the reader of the master and the writer to it.
    tasks: Vec<AbortHandle>,
}

impl Pane {
    /// A new pane of `terminal` with `shell`, of `cols` by `rows`.
    pub fn new(
        id: u32,
        terminal: u32,
        admin: Option<String>,
        shell: Shell,
        cols: u16,
        rows: u16,
        started: i64,
    ) -> Pane {
        Pane {
            id,
            terminal,
            admin,
            shell,
            cols,
            rows,
            started,
            ring: Ring::default(),
            modes: Modes::default(),
            typing: VecDeque::new(),
            typed: 0,
            paused: false,
            attached: BTreeSet::new(),
            reading: Rc::new(Notify::new()),
            writing: Rc::new(Notify::new()),
            room: Rc::new(Notify::new()),
            tasks: Vec::new(),
        }
    }

    /// Its terminal's name, as `pts/7`.
    pub fn tty(&self) -> &str {
        &self.shell.tty
    }

    /// Whether a program other than the shell has the terminal now, or
    /// `None` when the kernel does not say.
    fn busy(&self) -> Option<bool> {
        pty::foreground(&self.shell.master).map(|group| group != self.shell.process)
    }

    /// Whether the console session `session` may show it.
    fn shown_to(&self, session: &str) -> bool {
        self.admin.as_deref().is_none_or(|opener| opener == session)
    }
}

/// What a socket has been sent of a pane's output, and acknowledged.
#[derive(Debug)]
struct Flow {
    /// The offset just past what was sent.
    sent: u64,
    /// The offset up to which the page has taken it in.
    acked: u64,
    /// Since when more than [`PAUSE_AT`] waits.
    over_since: Option<Instant>,
}

impl Flow {
    fn waiting(&self) -> u64 {
        self.sent - self.acked
    }
}

/// A Terminal page's socket.
#[derive(Debug)]
pub struct Socket {
    account: String,
    /// Its console session, as authd names it.
    session: String,
    /// The panes it shows.
    flows: BTreeMap<u32, Flow>,
    /// The frames that wait to be written, and their bytes.
    outbox: VecDeque<Vec<u8>>,
    queued: usize,
    /// Why it is closing, once it is: its close frame is the last.
    closing: Option<Code>,
    /// Woken when a frame waits, and when it closes: the writer.
    wake: Rc<Notify>,
    /// Woken when it closes: the reader.
    stop: Rc<Notify>,
}

/// What a socket's writer is to do next.
#[derive(Debug, PartialEq, Eq)]
pub enum Next {
    /// Write this frame.
    Frame(Vec<u8>),
    /// Wait for one.
    Wait,
    /// Stop: the socket closed, and its close frame was the last written.
    Stop,
}

/// What typing in a pane came to.
#[derive(Debug)]
pub enum Typed {
    /// It waits for the master.
    Queued,
    /// The pane has [`TYPING_MAX`] waiting: wait for room, then try again.
    Full(Rc<Notify>),
    /// The pane is not shown on this socket, or ended: the typing is let
    /// go.
    Dropped,
}

/// An administrator's pane, from its start to its end, for the journal,
/// which names who changed a file by hand (`audit::who_in_terminal`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rooted {
    /// The account whose pane it is.
    pub account: String,
    /// When it started and ended, as Unix times: one that runs has no end.
    pub started: i64,
    pub ended: Option<i64>,
}

/// How long an administrator's pane that ended is remembered: the
/// journal names who changed a file in the 10 minutes before it looks, and
/// looks every 30 seconds.
const ROOTED_KEPT: i64 = crate::admind::audit::WINDOW + 60;

/// The terminals, panes and sockets.
#[derive(Debug, Default)]
pub struct Terminals {
    next_terminal: u32,
    next_pane: u32,
    next_socket: u64,
    terminals: BTreeMap<u32, Terminal>,
    panes: BTreeMap<u32, Pane>,
    sockets: BTreeMap<u64, Socket>,
    /// The administrators' panes that ended in the last [`ROOTED_KEPT`]
    /// seconds, oldest first.
    rooted: VecDeque<Rooted>,
}

/// The failure for a terminal or pane that is not there, or not the
/// asker's: the same, so that one cannot tell.
pub fn no_terminal() -> Failure {
    Failure::new(Kind::NoTerminal, "there is no such terminal, or it has ended")
}

impl Terminals {
    /// The next id of a terminal and of a pane.
    pub fn next_ids(&mut self) -> (u32, u32) {
        self.next_terminal += 1;
        self.next_pane += 1;
        (self.next_terminal, self.next_pane)
    }

    /// The next id of a pane.
    pub fn next_pane_id(&mut self) -> u32 {
        self.next_pane += 1;
        self.next_pane
    }

    /// Whether `account` may make one more terminal, with `most` its
    /// limit, and admind one more pane.
    ///
    /// # Errors
    ///
    /// ADM-130, saying which limit.
    pub fn room_for_terminal(&self, account: &str, most: u32) -> Result<(), Failure> {
        let theirs = self.terminals.values().filter(|t| t.account == account).count();
        if theirs >= most as usize {
            let why = format!("you have {theirs} terminals: end one on the Sessions page");
            return Err(Failure::new(Kind::TooManyTerminals, why));
        }
        self.room_for_pane()
    }

    /// Whether admind may hold one more pane.
    ///
    /// # Errors
    ///
    /// ADM-130.
    pub fn room_for_pane(&self) -> Result<(), Failure> {
        if self.panes.len() >= PANES_MAX {
            let why = "the console holds as many terminals as it can";
            return Err(Failure::new(Kind::TooManyTerminals, why));
        }
        Ok(())
    }

    /// The terminal `id` of `account`'s.
    ///
    /// # Errors
    ///
    /// ADM-131 for one that is not there or not theirs.
    pub fn terminal_of(&self, account: &str, id: u32) -> Result<&Terminal, Failure> {
        self.terminals.get(&id).filter(|t| t.account == account).ok_or_else(no_terminal)
    }

    /// The same, to change.
    ///
    /// # Errors
    ///
    /// ADM-131.
    pub fn terminal_of_mut(&mut self, account: &str, id: u32) -> Result<&mut Terminal, Failure> {
        self.terminals.get_mut(&id).filter(|t| t.account == account).ok_or_else(no_terminal)
    }

    /// Terminal `id`, whoever's it is: for the Accounts page, which an
    /// administrator ends another account's terminal from (ADR 0562).
    pub fn terminal(&self, id: u32) -> Option<&Terminal> {
        self.terminals.get(&id)
    }

    /// The terminal of pane `pane`, if it is there.
    pub fn terminal_of_pane(&self, pane: u32) -> Option<u32> {
        self.panes.get(&pane).map(|pane| pane.terminal)
    }

    /// Keeps `terminal`, with its first pane `pane`.
    pub fn add_terminal(&mut self, mut terminal: Terminal, pane: Pane) {
        terminal.panes = vec![pane.id];
        self.terminals.insert(terminal.id, terminal);
        self.panes.insert(pane.id, pane);
    }

    /// Adds `pane` to its terminal, which is there.
    pub fn add_pane(&mut self, pane: Pane) {
        if let Some(terminal) = self.terminals.get_mut(&pane.terminal) {
            terminal.panes.push(pane.id);
            self.panes.insert(pane.id, pane);
        }
    }

    /// Gives pane `id` the tasks that read and write its master.
    pub fn set_tasks(&mut self, id: u32, tasks: Vec<AbortHandle>) {
        match self.panes.get_mut(&id) {
            Some(pane) => pane.tasks = tasks,
            // It ended before its tasks were given.
            None => tasks.iter().for_each(AbortHandle::abort),
        }
    }

    /// What a pane's tasks wait on: reading going on, typing queued.
    pub fn notifies(&self, id: u32) -> Option<(Rc<Notify>, Rc<Notify>)> {
        self.panes.get(&id).map(|pane| (Rc::clone(&pane.reading), Rc::clone(&pane.writing)))
    }

    /// The JSON of terminal `id` as the console session `session` sees it:
    /// the panes it may show.
    pub fn json(&self, id: u32, session: &str) -> Value {
        let Some(terminal) = self.terminals.get(&id) else { return Value::Null };
        let panes: Vec<Value> = terminal
            .panes
            .iter()
            .filter_map(|pane| self.panes.get(pane))
            .filter(|pane| pane.shown_to(session))
            .map(|pane| {
                json!({
                    "id": pane.id,
                    "admin": pane.admin.is_some(),
                    "cols": pane.cols,
                    "rows": pane.rows,
                    "busy": pane.busy(),
                    "started": pane.started,
                    "attached": pane.attached.len(),
                })
            })
            .collect();
        json!({
            "id": terminal.id,
            "name": terminal.name,
            "made": terminal.made,
            "used": terminal.used,
            "from": terminal.from.to_string(),
            "keep_days": terminal.keep_days,
            "layout": terminal.layout.word(),
            "sizes": terminal.sizes,
            "panes": panes,
        })
    }

    /// The JSON of `GET /api/1/terminals` for `account` on the console
    /// session `session`: its terminals with a pane the session may show,
    /// the most it may keep, and the session's sockets.
    pub fn list(&self, account: &str, session: &str, most: u32) -> Value {
        let terminals: Vec<Value> = self
            .terminals
            .values()
            .filter(|t| t.account == account)
            .map(|t| self.json(t.id, session))
            .filter(|t| t["panes"].as_array().is_some_and(|panes| !panes.is_empty()))
            .collect();
        let open = self.sockets.values().filter(|s| s.session == session).count();
        json!({
            "terminals": terminals,
            "most": most,
            "sockets": { "open": open, "most": SOCKETS_PER_SESSION },
        })
    }

    /// The terminals of `account`, as the Accounts page lists them for an
    /// administrator or the account (ADR 0562): each with its name, how
    /// many panes it has, whether one is an administrator's, when it was
    /// made, whether a page shows it now, and its panes' pseudo-terminals
    /// as `pts/7`, which `user show` lists as sessions at the machine and
    /// the page leaves out of those. Nothing of what it shows.
    pub fn of_account(&self, account: &str) -> Vec<Value> {
        self.terminals
            .values()
            .filter(|terminal| terminal.account == account)
            .map(|terminal| {
                let panes: Vec<&Pane> =
                    terminal.panes.iter().filter_map(|pane| self.panes.get(pane)).collect();
                json!({
                    "id": terminal.id,
                    "title": terminal.name,
                    "panes": panes.len(),
                    "admin": panes.iter().any(|pane| pane.admin.is_some()),
                    "since_ms": terminal.made.saturating_mul(1000),
                    "attached": panes.iter().any(|pane| !pane.attached.is_empty()),
                    "ttys": panes.iter().map(|pane| pane.tty()).collect::<Vec<_>>(),
                })
            })
            .collect()
    }

    /// Opens a socket for `account` on the console session `session`.
    ///
    /// # Errors
    ///
    /// ADM-130 when the session has [`SOCKETS_PER_SESSION`] open.
    pub fn open_socket(&mut self, account: &str, session: &str) -> Result<u64, Failure> {
        let open = self.sockets.values().filter(|s| s.session == session).count();
        if open >= SOCKETS_PER_SESSION {
            let why = format!(
                "this browser has the Terminal open in {open} tabs, as many as it may: close one"
            );
            return Err(Failure::new(Kind::TooManyTerminals, why));
        }
        self.next_socket += 1;
        let socket = Socket {
            account: account.to_string(),
            session: session.to_string(),
            flows: BTreeMap::new(),
            outbox: VecDeque::new(),
            queued: 0,
            closing: None,
            wake: Rc::new(Notify::new()),
            stop: Rc::new(Notify::new()),
        };
        self.sockets.insert(self.next_socket, socket);
        Ok(self.next_socket)
    }

    /// What socket `id`'s writer waits on.
    pub fn socket_wake(&self, id: u64) -> Option<Rc<Notify>> {
        self.sockets.get(&id).map(|socket| Rc::clone(&socket.wake))
    }

    /// What socket `id`'s reader waits on besides the page.
    pub fn socket_stop(&self, id: u64) -> Option<Rc<Notify>> {
        self.sockets.get(&id).map(|socket| Rc::clone(&socket.stop))
    }

    /// Whether socket `id` is closing, or gone.
    pub fn closing(&self, id: u64) -> bool {
        self.sockets.get(&id).is_none_or(|socket| socket.closing.is_some())
    }

    /// Forgets socket `id`: the panes it showed are not held back by it.
    pub fn close_socket(&mut self, id: u64, now: Instant) {
        let Some(socket) = self.sockets.remove(&id) else { return };
        for pane in socket.flows.keys() {
            if let Some(pane) = self.panes.get_mut(pane) {
                pane.attached.remove(&id);
            }
            self.recheck(*pane, now);
        }
    }

    /// Queues `frame` on socket `id`. A socket with more waiting than it
    /// could ever need is closed.
    fn send(&mut self, id: u64, frame: Vec<u8>) {
        let Some(socket) = self.sockets.get_mut(&id) else { return };
        if socket.closing.is_some() {
            return;
        }
        if socket.queued + frame.len() > OUTBOX_MAX {
            log(4, "a terminal's socket had more waiting than it may: closed");
            self.close(id, Code::Failed);
            return;
        }
        socket.queued += frame.len();
        socket.outbox.push_back(frame);
        socket.wake.notify_one();
    }

    /// Queues the text message `text` on socket `id`.
    pub fn send_text(&mut self, id: u64, text: &str) {
        self.send(id, websocket::frame(FrameKind::Text, text.as_bytes()));
    }

    /// Queues the pong for a ping with `body` on socket `id`.
    pub fn pong(&mut self, id: u64, body: &[u8]) {
        self.send(id, websocket::frame(FrameKind::Pong, body));
    }

    /// Queues a ping on socket `id`.
    pub fn ping(&mut self, id: u64) {
        self.send(id, websocket::frame(FrameKind::Ping, b""));
    }

    /// Closes socket `id` with `code`: what waits is let go, the close
    /// frame is written, and the writer stops. Its panes are no longer
    /// held back by it.
    pub fn close(&mut self, id: u64, code: Code) {
        self.close_after(id, code, false);
    }

    /// Closes socket `id` with `code`, as [`Self::close`] does, but first
    /// sends what waits if `keep`.
    fn close_after(&mut self, id: u64, code: Code, keep: bool) {
        let Some(socket) = self.sockets.get_mut(&id) else { return };
        if socket.closing.is_some() {
            return;
        }
        socket.closing = Some(code);
        if !keep {
            socket.outbox.clear();
            socket.queued = 0;
        }
        socket.outbox.push_back(websocket::close(code));
        socket.wake.notify_one();
        socket.stop.notify_one();
        let panes: Vec<u32> = std::mem::take(&mut socket.flows).into_keys().collect();
        for pane in panes {
            if let Some(shown) = self.panes.get_mut(&pane) {
                shown.attached.remove(&id);
            }
            self.recheck(pane, Instant::now());
        }
    }

    /// What socket `id`'s writer is to do next.
    pub fn next_frame(&mut self, id: u64) -> Next {
        let Some(socket) = self.sockets.get_mut(&id) else { return Next::Stop };
        match socket.outbox.pop_front() {
            Some(frame) => {
                socket.queued = socket.queued.saturating_sub(frame.len());
                Next::Frame(frame)
            }
            None if socket.closing.is_some() => Next::Stop,
            None => Next::Wait,
        }
    }

    /// Closes every socket of the console session `session` with `code`.
    pub fn close_session(&mut self, session: &str, code: Code) {
        let theirs: Vec<u64> =
            self.sockets.iter().filter(|(_, s)| s.session == session).map(|(id, _)| *id).collect();
        for id in theirs {
            self.close(id, code);
        }
    }

    /// Shows pane `pane` on socket `id`, of `cols` by `rows` if no other
    /// socket shows it, from `since` when the ring still holds what
    /// follows it, else as a replay. Whether it was a replay, after which
    /// the pane's program is nudged to draw itself again.
    ///
    /// # Errors
    ///
    /// ADM-131 for a pane the socket's session may not show, ADM-130 for a
    /// socket that shows [`ATTACHED_MAX`] panes.
    pub fn attach(
        &mut self,
        id: u64,
        pane_id: u32,
        size: (u16, u16),
        since: Option<u64>,
        now: (Instant, i64),
    ) -> Result<bool, Failure> {
        let socket = self.sockets.get(&id).ok_or_else(no_terminal)?;
        let pane = self.panes.get(&pane_id).ok_or_else(no_terminal)?;
        let terminal = self.terminals.get(&pane.terminal).ok_or_else(no_terminal)?;
        if terminal.account != socket.account || !pane.shown_to(&socket.session) {
            return Err(no_terminal());
        }
        if !socket.flows.contains_key(&pane_id) && socket.flows.len() >= ATTACHED_MAX {
            let why = format!("a page shows {ATTACHED_MAX} panes at most");
            return Err(Failure::new(Kind::TooManyTerminals, why));
        }
        let pane = self.panes.get_mut(&pane_id).ok_or_else(no_terminal)?;
        pane.attached.remove(&id);
        if pane.attached.is_empty() && (pane.cols, pane.rows) != size {
            (pane.cols, pane.rows) = size;
            pty::resize(&pane.shell.master, pane.cols, pane.rows);
        }
        let (offset, output, replay) =
            match since.and_then(|since| Some((since, pane.ring.since(since)?))) {
                Some((since, output)) => (since, output, false),
                None => {
                    let from = pane.ring.replay_from();
                    (from, pane.ring.since(from).unwrap_or_default(), true)
                }
            };
        pane.attached.insert(id);
        let sent = pane.ring.end();
        let (cols, rows, admin) = (pane.cols, pane.rows, pane.admin.is_some());
        let modes = pane.modes.sequences();
        let terminal = self.terminals.get_mut(&pane.terminal).ok_or_else(no_terminal)?;
        terminal.used = now.1;
        let message = protocol::attached(&Attached {
            pane: pane_id,
            terminal: terminal.id,
            name: &terminal.name,
            admin,
            cols,
            rows,
            offset,
            live: sent,
            replay,
            modes,
        });
        if let Some(socket) = self.sockets.get_mut(&id) {
            socket.flows.insert(pane_id, Flow { sent, acked: offset, over_since: None });
        }
        self.send_text(id, &message);
        for (at, chunk) in output.chunks(CHUNK_MAX).enumerate() {
            let start = offset + (at * CHUNK_MAX) as u64;
            self.send(
                id,
                websocket::frame(FrameKind::Binary, &protocol::output(pane_id, start, chunk)),
            );
        }
        self.recheck(pane_id, now.0);
        Ok(replay)
    }

    /// Stops showing pane `pane` on socket `id`.
    pub fn detach(&mut self, id: u64, pane: u32, now: Instant) {
        let Some(socket) = self.sockets.get_mut(&id) else { return };
        if socket.flows.remove(&pane).is_some() {
            if let Some(shown) = self.panes.get_mut(&pane) {
                shown.attached.remove(&id);
            }
            self.recheck(pane, now);
        }
    }

    /// Gives pane `pane`, shown on socket `id`, the size `cols` by `rows`.
    ///
    /// # Errors
    ///
    /// ADM-131 when the socket does not show it.
    pub fn resize(&mut self, id: u64, pane: u32, cols: u16, rows: u16) -> Result<(), Failure> {
        let shown = self.sockets.get(&id).is_some_and(|socket| socket.flows.contains_key(&pane));
        let pane = self.panes.get_mut(&pane).filter(|_| shown).ok_or_else(no_terminal)?;
        if (pane.cols, pane.rows) != (cols, rows) {
            (pane.cols, pane.rows) = (cols, rows);
            pty::resize(&pane.shell.master, cols, rows);
        }
        Ok(())
    }

    /// Gives pane `pane` the size it has, after one row less: two
    /// SIGWINCH, so that a program that draws the whole screen draws it
    /// again for a page that came back.
    pub fn nudge(&self, pane: u32, step: bool) {
        if let Some(pane) = self.panes.get(&pane) {
            let rows = if step { pane.rows.saturating_sub(1).max(1) } else { pane.rows };
            pty::resize(&pane.shell.master, pane.cols, rows);
        }
    }

    /// Notes that socket `id`'s page took in pane `pane`'s output up to
    /// `offset`.
    pub fn ack(&mut self, id: u64, pane: u32, offset: u64, now: Instant) {
        let Some(flow) = self.sockets.get_mut(&id).and_then(|socket| socket.flows.get_mut(&pane))
        else {
            return;
        };
        flow.acked = flow.acked.max(offset.min(flow.sent));
        self.recheck(pane, now);
    }

    /// Takes `output` read from pane `pane`'s master: the ring keeps it,
    /// the modes are followed, and each socket that shows the pane is sent
    /// it.
    pub fn output(&mut self, pane_id: u32, output: &[u8], now: Instant) {
        let Some(pane) = self.panes.get_mut(&pane_id) else { return };
        let offset = pane.ring.end();
        pane.ring.push(output);
        pane.modes.scan(output);
        let shown: Vec<u64> = pane.attached.iter().copied().collect();
        for id in shown {
            if let Some(flow) =
                self.sockets.get_mut(&id).and_then(|socket| socket.flows.get_mut(&pane_id))
            {
                flow.sent = offset + output.len() as u64;
            }
            for (at, chunk) in output.chunks(CHUNK_MAX).enumerate() {
                let start = offset + (at * CHUNK_MAX) as u64;
                let message = protocol::output(pane_id, start, chunk);
                self.send(id, websocket::frame(FrameKind::Binary, &message));
            }
        }
        self.recheck(pane_id, now);
    }

    /// Whether pane `pane`'s master is read now, by what its sockets have
    /// not acknowledged: not past [`PAUSE_AT`] on any, and once not read,
    /// again when all are under [`RESUME_AT`].
    fn recheck(&mut self, pane_id: u32, now: Instant) {
        let Some(pane) = self.panes.get_mut(&pane_id) else { return };
        let mut most = 0;
        for id in &pane.attached {
            let Some(flow) = self.sockets.get_mut(id).and_then(|s| s.flows.get_mut(&pane_id))
            else {
                continue;
            };
            let waiting = flow.waiting();
            most = most.max(waiting);
            if waiting > PAUSE_AT {
                flow.over_since.get_or_insert(now);
            } else if waiting <= RESUME_AT {
                flow.over_since = None;
            }
        }
        if pane.paused && most <= RESUME_AT {
            pane.paused = false;
            pane.reading.notify_one();
        } else if !pane.paused && most > PAUSE_AT {
            pane.paused = true;
        }
    }

    /// Whether pane `pane`'s master is not to be read now; `None` once it
    /// is gone.
    pub fn paused(&self, pane: u32) -> Option<bool> {
        self.panes.get(&pane).map(|pane| pane.paused)
    }

    /// Queues `typed`, from socket `id`, for pane `pane`'s master.
    pub fn type_in(&mut self, id: u64, pane_id: u32, typed: &[u8]) -> Typed {
        let shown = self.sockets.get(&id).is_some_and(|socket| socket.flows.contains_key(&pane_id));
        let Some(pane) = self.panes.get_mut(&pane_id).filter(|_| shown) else {
            return Typed::Dropped;
        };
        if typed.is_empty() {
            return Typed::Queued;
        }
        if pane.typed + typed.len() > TYPING_MAX {
            return Typed::Full(Rc::clone(&pane.room));
        }
        pane.typed += typed.len();
        pane.typing.push_back(typed.to_vec());
        pane.writing.notify_one();
        Typed::Queued
    }

    /// Queues `answer`, from socket `id`, for pane `pane`'s master, when
    /// that socket is the one that answers for the pane: the first opened
    /// of the sockets that show it. Every page that shows a pane reads the
    /// same questions in its output, and a program that asks once must be
    /// answered once: a second answer would be read as the answer to its
    /// next question, or as typing.
    pub fn answer(&mut self, id: u64, pane_id: u32, answer: &[u8]) -> Typed {
        let answers =
            self.panes.get(&pane_id).is_some_and(|pane| pane.attached.first() == Some(&id));
        if answers { self.type_in(id, pane_id, answer) } else { Typed::Dropped }
    }

    /// The next typing for pane `pane`'s master; `Err` once the pane is
    /// gone.
    pub fn next_typing(&mut self, pane: u32) -> Result<Option<Vec<u8>>, ()> {
        let pane = self.panes.get_mut(&pane).ok_or(())?;
        Ok(pane.typing.pop_front())
    }

    /// Notes that `bytes` of pane `pane`'s typing were written: room for
    /// as much more.
    pub fn typing_taken(&mut self, pane: u32, bytes: usize) {
        if let Some(pane) = self.panes.get_mut(&pane) {
            pane.typed = pane.typed.saturating_sub(bytes);
            pane.room.notify_waiters();
        }
    }

    /// Ends pane `pane` for `why`, with the shell's `status` when known:
    /// its tasks stop and its master closes, which hangs its terminal up;
    /// the sockets that showed it are told, and the account's that its
    /// terminal changed. A terminal left with no pane ends too.
    pub fn end_pane(&mut self, pane_id: u32, why: Why, status: Option<i32>) {
        let Some(pane) = self.panes.remove(&pane_id) else { return };
        for task in &pane.tasks {
            task.abort();
        }
        pane.room.notify_waiters();
        let message = protocol::ended(pane_id, why, status);
        for id in &pane.attached {
            if let Some(socket) = self.sockets.get_mut(id) {
                socket.flows.remove(&pane_id);
            }
            self.send_text(*id, &message);
        }
        let Some(terminal) = self.terminals.get_mut(&pane.terminal) else { return };
        terminal.panes.retain(|id| *id != pane_id);
        let status = status.map_or_else(String::new, |status| format!(" {status}"));
        log(
            6,
            &format!(
                "terminal {} pane {pane_id} ({}) ended: {}{status}",
                terminal.id,
                pane.shell.tty,
                why.word()
            ),
        );
        let (account, terminal_id) = (terminal.account.clone(), terminal.id);
        if terminal.panes.is_empty() {
            self.terminals.remove(&terminal_id);
        }
        if pane.admin.is_some() {
            let now = unix_now();
            self.rooted.retain(|rooted| rooted.ended.is_some_and(|at| now - at <= ROOTED_KEPT));
            let account = account.clone();
            self.rooted.push_back(Rooted { account, started: pane.started, ended: Some(now) });
        }
        self.tell_changed(&account, terminal_id);
        // The master closes as `pane` goes.
    }

    /// The administrators' panes that run at Unix time `now`, and those
    /// that ended in the [`ROOTED_KEPT`] seconds before.
    pub fn rooted(&self, now: i64) -> Vec<Rooted> {
        let running = self.panes.values().filter(|pane| pane.admin.is_some()).filter_map(|pane| {
            let account = self.terminals.get(&pane.terminal)?.account.clone();
            Some(Rooted { account, started: pane.started, ended: None })
        });
        let ended =
            self.rooted.iter().filter(|r| r.ended.is_some_and(|at| now - at <= ROOTED_KEPT));
        running.chain(ended.cloned()).collect()
    }

    /// Ends every pane `which` picks, for `why`.
    pub fn end_where(&mut self, which: impl Fn(&Pane) -> bool, why: Why) {
        let ended: Vec<u32> =
            self.panes.values().filter(|pane| which(pane)).map(|pane| pane.id).collect();
        for pane in ended {
            self.end_pane(pane, why, None);
        }
    }

    /// Ends the panes of terminal `id`, and so the terminal.
    pub fn end_terminal(&mut self, id: u32) {
        self.end_where(|pane| pane.terminal == id, Why::Ended);
    }

    /// The console session whose administrator's pane `id` is, if it is
    /// one.
    pub fn admin_of(&self, id: u32) -> Option<String> {
        self.panes.get(&id).and_then(|pane| pane.admin.clone())
    }

    /// Ends the administrators' panes of the console session `session`,
    /// for `why`.
    pub fn end_admin_panes(&mut self, session: &str, why: Why) {
        self.end_where(|pane| pane.admin.as_deref() == Some(session), why);
    }

    /// Tells each socket of `account` that terminal `terminal` changed.
    pub fn tell_changed(&mut self, account: &str, terminal: u32) {
        let theirs: Vec<u64> =
            self.sockets.iter().filter(|(_, s)| s.account == account).map(|(id, _)| *id).collect();
        let message = protocol::changed(terminal);
        for id in theirs {
            self.send_text(id, &message);
        }
    }

    /// What is looked at every second: the administrators' panes whose
    /// session is not unlocked now, as `unlocked` says, end; a socket that
    /// has been past [`PAUSE_AT`] of a pane for [`SLOW_FOR`] no longer
    /// shows it; and a terminal no page shows, not opened for its days,
    /// ends.
    pub fn tick(&mut self, now: (Instant, i64), unlocked: impl Fn(&str) -> bool) {
        let locked: Vec<String> = self
            .panes
            .values()
            .filter_map(|pane| pane.admin.clone())
            .filter(|session| !unlocked(session))
            .collect();
        for session in locked {
            self.end_admin_panes(&session, Why::Locked);
        }
        let mut slow = Vec::new();
        for (id, socket) in &self.sockets {
            for (pane, flow) in &socket.flows {
                if flow.over_since.is_some_and(|at| now.0.saturating_duration_since(at) >= SLOW_FOR)
                {
                    slow.push((*id, *pane));
                }
            }
        }
        for (id, pane) in slow {
            self.detach(id, pane, now.0);
            self.send_text(id, &protocol::detached_slow(pane));
            log(
                6,
                &format!("pane {pane} detached from a socket that took in its output too slowly"),
            );
        }
        let old: Vec<u32> = self
            .terminals
            .values()
            .filter(|terminal| {
                let shown = terminal
                    .panes
                    .iter()
                    .any(|pane| self.panes.get(pane).is_some_and(|p| !p.attached.is_empty()));
                let kept = i64::from(terminal.keep_days) * 86_400;
                !shown && now.1.saturating_sub(terminal.used) >= kept
            })
            .map(|terminal| terminal.id)
            .collect();
        for id in old {
            log(6, &format!("terminal {id} was not opened for its days: ended"));
            self.end_where(|pane| pane.terminal == id, Why::Idle);
        }
    }

    /// Ends everything, as admind stops: each pane for `restart`, and each
    /// socket with 1001.
    pub fn end_all(&mut self) {
        // The output that waits is let go, so that it does not hold admind's
        // stop up; each page is then told why its panes ended, before the
        // close.
        for socket in self.sockets.values_mut() {
            socket.outbox.clear();
            socket.queued = 0;
        }
        self.end_where(|_| true, Why::Restart);
        let sockets: Vec<u64> = self.sockets.keys().copied().collect();
        for id in sockets {
            self.close_after(id, Code::Stopping, true);
        }
    }
}

#[cfg(test)]
mod tests;

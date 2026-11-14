// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Tasks (ADR 0516): the actions that take longer than a request waits.
//! Such an action answers 202 `{"task": ID}` at once, and admind runs its
//! command as an administrator, through authd's `console-elevate` as any
//! elevated command, but without the 30 seconds that [`run::RUN_TIME`]
//! gives a short one, and keeps the task in its store, so that a task is
//! there to read after the browser that asked has gone, from another
//! browser, and after admind has restarted.
//!
//! **States.** A task is `waiting` until it has one of the 32 places for
//! elevated commands that run at once (the short commands share them), in
//! the order the tasks were asked for; `running` while its command runs;
//! and then `done` (status 0), `failed`, or `stopped`.
//!
//! **Progress.** The command's standard output is read a line at a time.
//! `step N of M: WORDS` says the task is at step N of M and what the step
//! is; `progress P%` says how far the step has come. Every other line, and
//! each line of the error output, is the log, of which the last 2,000
//! lines are kept, as at most 256 KiB, each line cut at 1,024 bytes, so
//! that 32 tasks that write a lot cannot fill admind's memory.
//!
//! **Time.** A task may run 12 hours. No action is long yet; the longest
//! that are planned are a backup of a whole machine and an update over a
//! slow line, which end in hours on a machine that works. A limit a day
//! long would let a command that hangs hold one of the 32 places through
//! a night and the working day that follows; a task the person wants to
//! end sooner is stopped by them. At the limit the command is sent
//! SIGTERM and, 30 seconds later, hung up, as a stop is.
//!
//! **Stop.** `POST /api/1/tasks/{id}/stop` has the command sent SIGTERM
//! through the relay, the pipe that authd reads a terminal's signals from,
//! and the command puts back what it did, as it does for ^C. A task that
//! still waits for a place just leaves the queue. The command that does
//! not end 30 seconds after SIGTERM is hung up (the relay is closed,
//! which authd takes as the terminal going), and one that does not end 10
//! seconds after that is let go and the task fails.
//!
//! **Restarts.** When admind starts it ends every task it finds `waiting`
//! or `running` as `stopped`, "the console restarted": authd hangs up the
//! commands of an admind that has gone. A task is kept 30 days after it
//! ended, and then forgotten; the journal keeps its change.
//!
//! **Events.** `event: task` goes to each tab's stream for a change of
//! state, step, words or progress, at most four a second for each task:
//! [`Told`] holds back what comes sooner and sends the latest when the
//! time is up. A tab that opens is told where each task is.
//!
//! The row of the task in the store is saved when its state changes and,
//! while it runs, every 5 seconds when its log, step or progress changed;
//! what is read before that is the live one in memory.
//!
//! When a task is not as it should be, `log admind` has a line when it
//! starts and one when it ends, and the task's own log has what its
//! command wrote.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::io::Write;
use std::pin::pin;
use std::rc::Rc;
use std::task::Poll;
use std::time::Duration;

use accounts::console::Elevator;
use accounts::request;
use bytes::Bytes;
use tokio::io::AsyncReadExt;
use tokio::net::unix::pipe::Receiver;
use tokio::sync::{Notify, mpsc, watch};
use tokio::time::Instant;

use super::actions::Prepared;
use super::errors::{Failure, Kind};
use super::journal::Ended;
use super::log;
use super::run::{self, Place, Started};
use super::shared::{Shared, unix_now};
use super::store::tasks::{State, Task};
use super::store::{self, Store};

/// How long a task's command may run.
pub const TASK_TIME: Duration = Duration::from_hours(12);

/// How long a command that was sent SIGTERM has to end before it is hung
/// up.
pub const STOP_GRACE: Duration = Duration::from_secs(30);

/// How long a command that was hung up has to end before it is let go.
const HANG_UP_GRACE: Duration = Duration::from_secs(10);

/// How long what a command wrote is read for after its end was told.
const DRAIN_TIME: Duration = Duration::from_secs(2);

/// How long a running task's changes go unsaved.
const SAVE_EVERY: Duration = Duration::from_secs(5);

/// The least time between two events of one task: four a second.
pub const EVENT_SPACE: Duration = Duration::from_millis(250);

/// How long a task that ended is kept in memory for the streams to tell.
const KEEP_ENDED: Duration = Duration::from_secs(60);

/// The most tasks that wait for a place.
pub const WAITING_MAX: usize = 64;

/// The longest line of a task's log, in bytes.
pub const LINE_MAX: usize = 1024;

/// The most bytes a task's log holds, in all its lines.
pub const LOG_BYTES_MAX: usize = 256 << 10;

/// The longest words of a step, in characters.
const WORDS_MAX: usize = 200;

/// How long a task may run, and how long it has to end after SIGTERM:
/// what the tests shorten.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// How long its command may run.
    pub time: Duration,
    /// How long it has after SIGTERM, before it is hung up.
    pub grace: Duration,
}

/// The tasks that wait or run, or ended lately, as admind holds them.
pub struct Tasks {
    /// The tasks admind knows of, by number.
    live: RefCell<HashMap<i64, Entry>>,
    /// The tasks that wait for a place, in the order of their asking.
    queue: RefCell<VecDeque<i64>>,
    /// Woken when a place is given back, a task leaves the queue, or a
    /// stop is asked for.
    wake: Notify,
    /// Sent to when a task changes in a way the streams tell.
    changed: watch::Sender<()>,
    /// What the tests shorten.
    limits: Cell<Limits>,
    /// The last revision given to a change.
    revision: Cell<u64>,
    /// Whether the store's failure to save was said, so that the log says
    /// it once and not every 5 seconds.
    complained: Cell<bool>,
}

/// A task as admind holds it while it is live.
struct Entry {
    /// What the task is. Its log is [`Entry::log`], not this.
    task: Task,
    /// The last lines of the log.
    log: VecDeque<String>,
    /// How many bytes `log` holds.
    log_bytes: usize,
    /// Counts the changes the streams tell: a stream that has told this
    /// one need not tell it again.
    rev: u64,
    /// Who asked for the task to stop.
    stop: Option<String>,
    /// Whether the store lacks something the task has.
    unsaved: bool,
    /// When it was last saved.
    saved: Instant,
    /// When it ended.
    over: Option<Instant>,
}

impl Entry {
    /// The task, with its log.
    fn full(&self) -> Task {
        Task { log: self.log.iter().cloned().collect(), ..self.task.clone() }
    }
}

impl Default for Tasks {
    fn default() -> Tasks {
        Tasks::new()
    }
}

impl Tasks {
    /// No task.
    pub fn new() -> Tasks {
        Tasks {
            live: RefCell::new(HashMap::new()),
            queue: RefCell::new(VecDeque::new()),
            wake: Notify::new(),
            changed: watch::channel(()).0,
            limits: Cell::new(Limits { time: TASK_TIME, grace: STOP_GRACE }),
            revision: Cell::new(0),
            complained: Cell::new(false),
        }
    }

    /// Changes how long a task runs and has after SIGTERM, for the tests.
    #[cfg(test)]
    pub fn set_limits(&self, limits: Limits) {
        self.limits.set(limits);
    }

    /// A receiver that is told when a task changes in a way the streams
    /// tell.
    pub fn subscribe(&self) -> watch::Receiver<()> {
        self.changed.subscribe()
    }

    /// Wakes the tasks that wait for a place, as one has been given back.
    pub fn place_given_back(&self) {
        self.wake.notify_waiters();
    }

    /// How many tasks wait for a place.
    pub fn waiting(&self) -> usize {
        self.queue.borrow().len()
    }

    /// The task `id`, as it is now: the live one if there is one, or the
    /// store's.
    ///
    /// # Errors
    ///
    /// If the store fails.
    pub fn get(&self, store: &Store, id: i64) -> store::Result<Option<Task>> {
        if let Some(entry) = self.live.borrow().get(&id) {
            return Ok(Some(entry.full()));
        }
        store.task(id)
    }

    /// The tasks that wait or run, and those that ended at Unix time
    /// `since` or later, newest first, at most `limit`, as they are now.
    ///
    /// # Errors
    ///
    /// If the store fails.
    pub fn list(&self, store: &Store, since: i64, limit: usize) -> store::Result<Vec<Task>> {
        let live = self.live.borrow();
        let mut tasks = store.tasks(since, limit)?;
        for task in &mut tasks {
            if let Some(entry) = live.get(&task.id) {
                *task = entry.full();
            }
        }
        Ok(tasks)
    }

    /// Asks the task `id` to stop, as `by` asked: it is sent SIGTERM, or
    /// leaves the queue.
    ///
    /// # Errors
    ///
    /// ADM-128 when the task is not one that waits or runs.
    pub fn stop(&self, id: i64, by: &str) -> Result<(), Failure> {
        let mut live = self.live.borrow_mut();
        match live.get_mut(&id) {
            Some(entry) if !entry.task.state.ended() => {
                entry.stop.get_or_insert_with(|| by.to_string());
                drop(live);
                self.wake.notify_waiters();
                Ok(())
            }
            _ => Err(Failure::new(
                Kind::Cannot,
                "the task has ended: only a task that waits or runs can be stopped",
            )),
        }
    }

    /// Who asked for the task `id` to stop, if anyone did.
    fn stop_asked(&self, id: i64) -> Option<String> {
        self.live.borrow().get(&id).and_then(|entry| entry.stop.clone())
    }

    /// Adds the task as it waits, in the queue's last place.
    fn add(&self, task: Task) {
        let id = task.id;
        let entry = Entry {
            task,
            log: VecDeque::new(),
            log_bytes: 0,
            rev: 0,
            stop: None,
            unsaved: false,
            saved: Instant::now(),
            over: None,
        };
        self.live.borrow_mut().insert(id, entry);
        self.queue.borrow_mut().push_back(id);
        self.touch(id);
    }

    /// Notes that the task `id` changed in a way the streams tell.
    fn touch(&self, id: i64) {
        let revision = self.revision.get() + 1;
        self.revision.set(revision);
        if let Some(entry) = self.live.borrow_mut().get_mut(&id) {
            entry.rev = revision;
        }
        self.changed.send_replace(());
    }

    /// Whether the task `id` is the first to wait.
    fn at_front(&self, id: i64) -> bool {
        self.queue.borrow().front() == Some(&id)
    }

    /// Takes the task `id` out of the queue, and wakes those behind it.
    fn leave_queue(&self, id: i64) {
        self.queue.borrow_mut().retain(|queued| *queued != id);
        self.wake.notify_waiters();
    }

    /// Forgets the tasks that ended more than a minute before `now`.
    pub fn prune(&self, now: Instant) {
        self.live
            .borrow_mut()
            .retain(|_, entry| entry.over.is_none_or(|over| now < over + KEEP_ENDED));
    }

    /// Changes the live task `id` by `edit`, which says whether what it
    /// changed is what the streams tell.
    fn edit(&self, id: i64, edit: impl FnOnce(&mut Entry) -> bool) {
        let told = self.live.borrow_mut().get_mut(&id).is_some_and(edit);
        if told {
            self.touch(id);
        }
    }

    /// The task `id` is at the start of its command.
    fn running(&self, store: &Store, id: i64) {
        self.edit(id, |entry| {
            entry.task.state = State::Running;
            true
        });
        self.save(store, id);
    }

    /// Takes a line the command wrote into the task `id`: a step or a
    /// progress if the output says one, else a line of the log.
    fn line(&self, id: i64, stream: Stream, text: &str) {
        let progress = (stream == Stream::Output).then(|| progress(text)).flatten();
        self.edit(id, |entry| {
            entry.unsaved = true;
            match progress {
                Some(Progress::Step { step, steps, words }) => {
                    entry.task.step = step;
                    entry.task.steps = steps;
                    entry.task.words = words;
                    entry.task.percent = None;
                    true
                }
                Some(Progress::Percent(percent)) => {
                    entry.task.percent = Some(percent);
                    true
                }
                None => {
                    let line = clean(text);
                    if line.is_empty() {
                        return false;
                    }
                    entry.log_bytes += line.len();
                    entry.log.push_back(line);
                    while entry.log.len() > store::tasks::LOG_MAX || entry.log_bytes > LOG_BYTES_MAX
                    {
                        let Some(old) = entry.log.pop_front() else { break };
                        entry.log_bytes -= old.len();
                    }
                    false
                }
            }
        });
    }

    /// Saves the task `id` if it changed and was not saved for 5 seconds.
    fn save_if_due(&self, store: &Store, id: i64, now: Instant) {
        let due = self
            .live
            .borrow()
            .get(&id)
            .is_some_and(|entry| entry.unsaved && now >= entry.saved + SAVE_EVERY);
        if due {
            self.save(store, id);
        }
    }

    /// When the task `id` is next due to be saved, if it has something
    /// unsaved.
    fn save_due(&self, id: i64) -> Option<Instant> {
        self.live
            .borrow()
            .get(&id)
            .and_then(|entry| entry.unsaved.then(|| entry.saved + SAVE_EVERY))
    }

    /// Writes the task `id` to the store, and says in the log, once, when
    /// it cannot.
    fn save(&self, store: &Store, id: i64) {
        let Some(task) = self.live.borrow().get(&id).map(Entry::full) else { return };
        match store.save_task(&task) {
            Ok(()) => {
                self.complained.set(false);
                if let Some(entry) = self.live.borrow_mut().get_mut(&id) {
                    entry.unsaved = false;
                    entry.saved = Instant::now();
                }
            }
            Err(why) => {
                if !self.complained.replace(true) {
                    log(4, &format!("task {id} could not be saved: {why}"));
                }
            }
        }
    }

    /// Ends the task `id` as `end` says, and saves it.
    fn end(&self, store: &Store, id: i64, end: &End) {
        self.edit(id, |entry| {
            entry.task.state = end.state;
            entry.task.status = end.status;
            entry.task.note.clone_from(&end.note);
            entry.task.ended = Some(unix_now());
            entry.over = Some(Instant::now());
            entry.unsaved = true;
            true
        });
        self.save(store, id);
    }

    /// The events due for a stream that has `told`, at `now`, and how long
    /// to wait before the next is due, if one is held back. See [`Told`].
    pub fn events(&self, told: &mut Told, now: Instant) -> (Vec<Bytes>, Option<Duration>) {
        let live = self.live.borrow();
        told.sent.retain(|id, _| live.contains_key(id));
        let mut ids: Vec<i64> = live.keys().copied().collect();
        ids.sort_unstable();
        let (mut due, mut wait) = (Vec::new(), None::<Duration>);
        for id in ids {
            let Some(entry) = live.get(&id) else { continue };
            match told.sent.get(&id) {
                Some((rev, _)) if *rev == entry.rev => continue,
                Some((_, at)) if now < *at + EVENT_SPACE => {
                    let left = *at + EVENT_SPACE - now;
                    wait = Some(wait.map_or(left, |least| least.min(left)));
                    continue;
                }
                _ => {}
            }
            told.sent.insert(id, (entry.rev, now));
            due.push(event(&entry.task));
        }
        (due, wait)
    }
}

/// What a stream has told of each task: the change it was at, and when.
#[derive(Default)]
pub struct Told {
    sent: HashMap<i64, (u64, Instant)>,
}

/// The event for `task`: its state, step, words and progress, but not its
/// log, which the task's address gives.
fn event(task: &Task) -> Bytes {
    Bytes::from(format!("event: task\ndata: {}\n\n", task.event()))
}

/// The two streams a command writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stream {
    /// The standard output, whose lines may say how far the task is.
    Output,
    /// The error output, whose lines are the log's.
    Errors,
}

/// What a line of a command's output says of how far it is.
#[derive(Debug, PartialEq, Eq)]
pub enum Progress {
    /// `step N of M: WORDS`.
    Step {
        /// The step the command is at, from 1.
        step: u32,
        /// How many there are.
        steps: u32,
        /// What the step is.
        words: String,
    },
    /// `progress P%`: how far the step is, 0 to 100.
    Percent(u8),
}

/// What `line` says of progress, if it is one of the two lines that do:
/// `step N of M: WORDS`, with 1 <= N <= M and words, and `progress P%`,
/// with P from 0 to 100. Any other line is not progress, whatever it
/// looks like.
pub fn progress(line: &str) -> Option<Progress> {
    let line = line.trim_end();
    if let Some(rest) = line.strip_prefix("step ") {
        let (numbers, words) = rest.split_once(": ")?;
        let (step, steps) = numbers.split_once(" of ")?;
        let (step, steps) = (number(step)?, number(steps)?);
        let words = words.trim();
        if step == 0 || step > steps || words.is_empty() {
            return None;
        }
        let words = clean(words).chars().take(WORDS_MAX).collect();
        return Some(Progress::Step { step, steps, words });
    }
    let percent = line.strip_prefix("progress ")?.strip_suffix('%')?;
    u8::try_from(number(percent)?).ok().filter(|p| *p <= 100).map(Progress::Percent)
}

/// A whole number of plain digits.
fn number(text: &str) -> Option<u32> {
    if text.is_empty() || text.len() > 9 || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

/// `line` with each control character, but a tab, as the replacement
/// character, and its end trimmed: a terminal's escapes are not shown as
/// they would act.
fn clean(line: &str) -> String {
    let line = line.trim_end();
    line.chars().map(|c| if c.is_control() && c != '\t' { '\u{fffd}' } else { c }).collect()
}

/// Cuts a command's bytes into lines, keeping the first [`LINE_MAX`] bytes
/// of a line that is longer and dropping the rest of it.
#[derive(Default)]
pub struct LineSplitter {
    line: Vec<u8>,
    dropping: bool,
}

impl LineSplitter {
    /// The lines `bytes` ends, after what came before.
    pub fn push(&mut self, bytes: &[u8]) -> Vec<String> {
        let mut lines = Vec::new();
        for &byte in bytes {
            if byte == b'\n' {
                lines.push(String::from_utf8_lossy(&std::mem::take(&mut self.line)).into_owned());
                self.dropping = false;
            } else if self.line.len() < LINE_MAX {
                self.line.push(byte);
            } else {
                self.dropping = true;
            }
        }
        lines
    }

    /// The line a command left unended when its output ended, if it did.
    pub fn finish(&mut self) -> Option<String> {
        let line = std::mem::take(&mut self.line);
        (!line.is_empty()).then(|| String::from_utf8_lossy(&line).into_owned())
    }
}

/// How a task ended.
#[derive(Debug, PartialEq, Eq)]
struct End {
    state: State,
    status: Option<i32>,
    note: Option<String>,
}

/// What a task is made of, besides its number.
struct Job {
    prepared: Prepared,
    who: String,
    how: String,
    by: Elevator,
}

/// Asks for `prepared` to run as a task of `who`, who asked `how`
/// (`console`, or `token NAME`), by `by`'s right, and gives its number.
/// The task waits for a place and runs on its own, whatever becomes of the
/// request that asked.
///
/// # Errors
///
/// ADM-126 when the console has no store, which keeps the tasks; ADM-105
/// when 64 tasks wait already.
///
/// # Panics
///
/// Outside a `LocalSet`.
pub fn start(
    shared: &Rc<Shared>,
    prepared: Prepared,
    who: &str,
    how: &str,
    by: Elevator,
) -> Result<i64, Failure> {
    let store = shared.store.as_ref().ok_or_else(|| {
        Failure::new(Kind::NoStore, "the console's store did not open, and keeps no tasks")
    })?;
    if shared.tasks.waiting() >= WAITING_MAX {
        return Err(Failure::wait("too many tasks wait for a place already", 30));
    }
    let started = unix_now();
    let action = prepared.action.id();
    let id = store
        .add_task(action, &prepared.args, &prepared.argv, who, how, started)
        .map_err(|why| Failure::new(Kind::Internal, why.to_string()))?;
    shared.tasks.add(Task {
        id,
        action: action.to_string(),
        args: prepared.args.clone(),
        argv: prepared.argv.clone(),
        who: who.to_string(),
        how: how.to_string(),
        started,
        state: State::Waiting,
        step: 0,
        steps: 0,
        words: String::new(),
        percent: None,
        ended: None,
        status: None,
        note: None,
        log: Vec::new(),
    });
    let job = Job { prepared, who: who.to_string(), how: how.to_string(), by };
    tokio::task::spawn_local(run(Rc::clone(shared), id, job));
    Ok(id)
}

/// Runs the task `id`: waits for its place, runs its command, and ends it
/// and the journal's row of it.
async fn run(shared: Rc<Shared>, id: i64, job: Job) {
    let Some(store) = shared.store.as_ref() else { return };
    let tasks = &shared.tasks;
    let Some(place) = wait_for_place(&shared, id).await else {
        let by = tasks.stop_asked(id).unwrap_or_default();
        let end = End {
            state: State::Stopped,
            status: None,
            note: Some(format!("stopped by {by} before it began")),
        };
        tasks.end(store, id, &end);
        log(6, &format!("task {id} was stopped by {by} before it began"));
        return;
    };
    let argv = &job.prepared.argv;
    let begun = shared.journal.begin(Some(store), &job.prepared.files, unix_now());
    tasks.running(store, id);
    let started = run::start(&shared, job.by, argv).await;
    let (end, row) = match started {
        Ok(started) => {
            let done = follow(&shared, id, started).await;
            let program = argv.first().map_or("the command", String::as_str);
            let end = conclude(&done, program, tasks.limits.get());
            let row = (done.status, end.note.clone());
            (end, Some(row))
        }
        Err(failure) => {
            let row = run::ending_failure(&failure)
                .map(|(status, note)| (status, note.map(str::to_string)));
            let end = End { state: State::Failed, status: None, note: Some(failure.detail) };
            (end, row)
        }
    };
    match row {
        Some((status, note)) => {
            let ended = Ended {
                who: &job.who,
                how: &job.how,
                action: job.prepared.action.id(),
                argv,
                status,
                task: Some(id),
                note: note.as_deref(),
            };
            begun.end(Some(store), &ended, unix_now());
        }
        None => begun.cancel(),
    }
    tasks.end(store, id, &end);
    drop(place);
    let how = match end.status {
        Some(status) => format!("ended with status {status}"),
        None => "ended".to_string(),
    };
    log(
        6,
        &format!(
            "task {id} of {} ({}) {how}: {}",
            job.who,
            job.prepared.action.id(),
            end.state.word()
        ),
    );
}

/// Waits for the task `id` to be first in the queue and a place to be
/// free, and takes it. `None` when someone stopped the task while it
/// waited.
async fn wait_for_place(shared: &Shared, id: i64) -> Option<Place<'_>> {
    let tasks = &shared.tasks;
    loop {
        let woken = tasks.wake.notified();
        let mut woken = pin!(woken);
        woken.as_mut().enable();
        if tasks.stop_asked(id).is_some() {
            tasks.leave_queue(id);
            return None;
        }
        if tasks.at_front(id)
            && let Some(place) = Place::take(shared)
        {
            tasks.leave_queue(id);
            return Some(place);
        }
        woken.await;
    }
}

/// What came of a command that was watched to its end.
#[derive(Debug, Default, PartialEq, Eq)]
struct Done {
    /// The exit code, if its end was seen.
    status: Option<i32>,
    /// Who stopped it, if it was stopped.
    stopped_by: Option<String>,
    /// Whether it was sent SIGTERM for running out of time.
    out_of_time: bool,
    /// The last line it wrote to its error output.
    last_error: Option<String>,
}

/// How a task whose command came to `done` ends. A command that was sent
/// SIGTERM to stop is `stopped`, whatever its status: one that puts back
/// what it did ends well. A command that ended before the signal was sent
/// is not stopped, and is `done` or `failed` as its status says.
fn conclude(done: &Done, program: &str, limits: Limits) -> End {
    let end = |state, status, note: Option<String>| End { state, status, note };
    match (done.status, &done.stopped_by) {
        (Some(code), Some(by)) => end(State::Stopped, Some(code), Some(format!("stopped by {by}"))),
        (None, Some(by)) => end(
            State::Stopped,
            None,
            Some(format!("stopped by {by}, and the command did not end after it was hung up")),
        ),
        // Sent SIGTERM for running out of time: it did not finish its work
        // whatever its status says.
        (status, None) if done.out_of_time => end(
            State::Failed,
            status,
            Some(format!("it did not end in {}, and was stopped", lasting(limits.time))),
        ),
        (Some(0), None) => end(State::Done, Some(0), None),
        (Some(code), None) => {
            let note = done
                .last_error
                .clone()
                .unwrap_or_else(|| format!("{program} ended with status {code}"));
            end(State::Failed, Some(code), Some(note))
        }
        (None, None) => end(
            State::Failed,
            None,
            Some("the command did not end after it was hung up".to_string()),
        ),
    }
}

/// A time in words: hours, minutes or seconds, whichever is whole.
fn lasting(time: Duration) -> String {
    let seconds = time.as_secs();
    let (number, unit) = if seconds >= 3600 && seconds.is_multiple_of(3600) {
        (seconds / 3600, "hour")
    } else if seconds >= 60 && seconds.is_multiple_of(60) {
        (seconds / 60, "minute")
    } else {
        (seconds, "second")
    };
    format!("{number} {unit}{}", if number == 1 { "" } else { "s" })
}

/// What a command's readers tell the task that watches it.
enum Piece {
    /// A line of what it wrote.
    Line(Stream, String),
    /// One of its two outputs ended.
    Eof,
    /// authd said how it ended: its exit code.
    Ended(i32),
}

/// Reads `pipe` a line at a time into `tell`, then says it ended.
async fn read_lines(mut pipe: Receiver, stream: Stream, tell: mpsc::Sender<Piece>) {
    let (mut splitter, mut buffer) = (LineSplitter::default(), [0u8; 4096]);
    loop {
        match pipe.read(&mut buffer).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                for line in splitter.push(&buffer[..n]) {
                    // A task that has stopped watching has no more to hear.
                    if tell.send(Piece::Line(stream, line)).await.is_err() {
                        return;
                    }
                }
            }
        }
    }
    if let Some(line) = splitter.finish() {
        let _ = tell.send(Piece::Line(stream, line)).await;
    }
    let _ = tell.send(Piece::Eof).await;
}

/// Reads authd's connection until the command's end, and tells its exit
/// code.
async fn read_end(mut asked: super::authd::Asked, tell: mpsc::Sender<Piece>) {
    let status = run::ended_status(&mut asked).await.unwrap_or(1 << 8);
    let _ = tell.send(Piece::Ended(run::exit_code(status))).await;
}

/// What a watcher wakes for.
enum Next {
    /// A reader said something, or all have ended.
    Piece(Option<Piece>),
    /// A place was given back or a stop was asked for.
    Woken,
    /// The time it waited until has come.
    Time,
}

/// Waits for the next of a piece from the readers, `woken`, and `until`.
async fn next(
    pieces: &mut mpsc::Receiver<Piece>,
    mut woken: std::pin::Pin<&mut tokio::sync::futures::Notified<'_>>,
    until: Instant,
) -> Next {
    let mut piece = pin!(pieces.recv());
    let mut time = pin!(tokio::time::sleep_until(until));
    std::future::poll_fn(|cx| {
        // What the command wrote comes before what is done about it, so
        // that its last words are not lost to a stop.
        if let Poll::Ready(piece) = piece.as_mut().poll(cx) {
            return Poll::Ready(Next::Piece(piece));
        }
        if woken.as_mut().poll(cx).is_ready() {
            return Poll::Ready(Next::Woken);
        }
        if time.as_mut().poll(cx).is_ready() {
            return Poll::Ready(Next::Time);
        }
        Poll::Pending
    })
    .await
}

/// Watches the command of the task `id` until it ends: takes what it
/// writes into the task, and sends it SIGTERM when it is to stop or has
/// run out of time, and hangs it up if that does not end it.
async fn follow(shared: &Rc<Shared>, id: i64, started: Started) -> Done {
    let Some(store) = shared.store.as_ref() else { return Done::default() };
    let tasks = &shared.tasks;
    let limits = tasks.limits.get();
    let Started { output, errors, asked, relay } = started;
    let (tell, mut pieces) = mpsc::channel(256);
    // A command that leaves a child holding its pipes open must not keep
    // the readers, so they are ended with the watching.
    let readers = [
        tokio::task::spawn_local(read_lines(output, Stream::Output, tell.clone())),
        tokio::task::spawn_local(read_lines(errors, Stream::Errors, tell.clone())),
        tokio::task::spawn_local(read_end(asked, tell)),
    ];
    let mut relay = Some(relay);
    let mut done = Done::default();
    let (mut ended, mut eofs) = (None::<(i32, Instant)>, 0);
    let (mut termed, mut hung_up) = (None::<Instant>, None::<Instant>);
    let limit_at = Instant::now() + limits.time;
    loop {
        let woken = tasks.wake.notified();
        let mut woken = pin!(woken);
        woken.as_mut().enable();
        let now = Instant::now();
        if termed.is_none() && ended.is_none() {
            if let Some(by) = tasks.stop_asked(id) {
                signal_term(relay.as_ref());
                done.stopped_by = Some(by);
                termed = Some(now);
            } else if now >= limit_at {
                signal_term(relay.as_ref());
                done.out_of_time = true;
                termed = Some(now);
            }
        }
        if let Some(at) = termed
            && hung_up.is_none()
            && now >= at + limits.grace
        {
            relay = None;
            hung_up = Some(now);
        }
        if hung_up.is_some_and(|at| now >= at + HANG_UP_GRACE) {
            break;
        }
        if let Some((code, at)) = ended
            && (eofs == 2 || now >= at + DRAIN_TIME)
        {
            done.status = Some(code);
            break;
        }
        let mut until = if termed.is_none() { limit_at } else { now + limits.time };
        let soonest = [
            termed.filter(|_| hung_up.is_none()).map(|at| at + limits.grace),
            hung_up.map(|at| at + HANG_UP_GRACE),
            ended.map(|(_, at)| at + DRAIN_TIME),
            tasks.save_due(id),
        ];
        for at in soonest.into_iter().flatten() {
            until = until.min(at);
        }
        match next(&mut pieces, woken, until).await {
            Next::Piece(Some(Piece::Line(stream, text))) => {
                if stream == Stream::Errors && !text.trim().is_empty() {
                    done.last_error = Some(clean(&text));
                }
                tasks.line(id, stream, &text);
            }
            Next::Piece(Some(Piece::Eof)) => eofs += 1,
            Next::Piece(Some(Piece::Ended(code))) => ended = Some((code, Instant::now())),
            Next::Piece(None) => {
                // Every reader has gone without authd saying how it ended.
                if ended.is_none() {
                    ended = Some((1, Instant::now()));
                }
                eofs = 2;
            }
            Next::Woken | Next::Time => {}
        }
        tasks.save_if_due(store, id, Instant::now());
    }
    for reader in readers {
        reader.abort();
    }
    done
}

/// Sends the command SIGTERM through the relay, as a terminal's ^C would.
/// A relay that authd has stopped reading is no failure: the command is
/// then ending already.
fn signal_term(relay: Option<&std::io::PipeWriter>) {
    if let Some(mut relay) = relay {
        let _ = relay.write_all(request::signal(libc::SIGTERM).as_bytes());
    }
}

/// Ends the tasks an admind that ended left as `waiting` or `running`, as
/// `stopped`, "the console restarted", and forgets the tasks kept more
/// than 30 days. Says in the log what it did.
pub fn restarted(store: &Store) {
    match store.stop_unfinished(unix_now(), "the console restarted") {
        Ok(0) => {}
        Ok(count) => log(
            6,
            &format!("{count} task{} left by the last admind ended as stopped", plural(count)),
        ),
        Err(why) => log(4, &format!("tasks the last admind left could not be ended: {why}")),
    }
    forget_old(store);
}

/// Forgets the tasks that ended more than 30 days ago.
pub fn forget_old(store: &Store) {
    match store.forget_old_tasks(unix_now()) {
        Ok(0) => {}
        Ok(count) => log(6, &format!("{count} task{} kept 30 days were forgotten", plural(count))),
        Err(why) => log(4, &format!("old tasks could not be forgotten: {why}")),
    }
}

/// "s" for a count that is not one.
fn plural(count: usize) -> &'static str {
    if count == 1 { "" } else { "s" }
}

#[cfg(test)]
mod tests;

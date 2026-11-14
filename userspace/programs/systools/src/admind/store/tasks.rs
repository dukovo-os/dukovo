// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The tasks' rows (ADR 0516): the long actions admind runs, with the
//! state they are in, how far they have come, and the last lines of what
//! their commands wrote.

use rusqlite::{OptionalExtension, Row, params};
use serde::Serialize;
use serde_json::{Value, json};

use super::changes::{from_json, to_json};
use super::{Result, Store};

/// How many lines of a task's log are kept: the last.
pub const LOG_MAX: usize = 2000;

/// How many days a finished task is kept.
pub const KEPT_DAYS: i64 = 30;

/// Where a task is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum State {
    /// Asked for, and waiting for one of the 32 places for elevated
    /// commands.
    Waiting,
    /// Its command runs.
    Running,
    /// Its command ended with status 0.
    Done,
    /// It did not work: the command ended with another status, or did not
    /// start.
    Failed,
    /// Someone stopped it, or the console restarted.
    Stopped,
}

impl State {
    /// The word the table and the API use.
    pub fn word(self) -> &'static str {
        match self {
            State::Waiting => "waiting",
            State::Running => "running",
            State::Done => "done",
            State::Failed => "failed",
            State::Stopped => "stopped",
        }
    }

    /// The state a word names.
    fn of(word: &str) -> Option<State> {
        [State::Waiting, State::Running, State::Done, State::Failed, State::Stopped]
            .into_iter()
            .find(|state| state.word() == word)
    }

    /// Whether the task is over.
    pub fn ended(self) -> bool {
        !matches!(self, State::Waiting | State::Running)
    }
}

/// A task, as the table keeps it.
#[derive(Clone, Debug, PartialEq)]
pub struct Task {
    /// Its number.
    pub id: i64,
    /// The action's ID.
    pub action: String,
    /// Its checked arguments.
    pub args: Value,
    /// The command it runs.
    pub argv: Vec<String>,
    /// Whose it is: the person.
    pub who: String,
    /// `console` or `token NAME`.
    pub how: String,
    /// The Unix time it was asked for.
    pub started: i64,
    /// Where it is.
    pub state: State,
    /// The step the command is at, from 1, and how many it says there
    /// are: 0 and 0 until it says.
    pub step: u32,
    /// How many steps there are.
    pub steps: u32,
    /// What the step is, in the command's words.
    pub words: String,
    /// How far the step is, 0 to 100, if the command says.
    pub percent: Option<u8>,
    /// The Unix time it ended.
    pub ended: Option<i64>,
    /// The command's exit status, when it ran to its end.
    pub status: Option<i32>,
    /// Why it is as it is, when the status does not say.
    pub note: Option<String>,
    /// The last lines of what the command wrote.
    pub log: Vec<String>,
}

impl Task {
    /// The task as the list gives it: everything but the log.
    pub fn summary(&self) -> Value {
        json!({
            "id": self.id,
            "action": self.action,
            "args": self.args,
            "argv": self.argv,
            "who": self.who,
            "how": self.how,
            "started": self.started,
            "state": self.state,
            "step": self.step,
            "steps": self.steps,
            "words": self.words,
            "percent": self.percent,
            "ended": self.ended,
            "status": self.status,
            "note": self.note,
        })
    }

    /// What an `event: task` says of the task: where it is and how far,
    /// and not its command, its arguments or its log.
    pub fn event(&self) -> Value {
        json!({
            "id": self.id,
            "action": self.action,
            "state": self.state,
            "step": self.step,
            "steps": self.steps,
            "words": self.words,
            "percent": self.percent,
            "ended": self.ended,
            "status": self.status,
            "note": self.note,
        })
    }

    /// The task with its log, as one task's address gives it.
    pub fn detail(&self) -> Value {
        let mut value = self.summary();
        value["log"] = json!(self.log);
        value
    }

    /// The row `row` of a query of [`COLUMNS`].
    fn read(row: &Row<'_>) -> rusqlite::Result<Task> {
        let word: String = row.get(7)?;
        let state = State::of(&word).ok_or_else(|| {
            rusqlite::Error::FromSqlConversionFailure(
                7,
                rusqlite::types::Type::Text,
                format!("{word} is not a state of a task").into(),
            )
        })?;
        let log: String = row.get(15)?;
        Ok(Task {
            id: row.get(0)?,
            action: row.get(1)?,
            args: from_json(2, &row.get::<_, String>(2)?)?,
            argv: from_json(3, &row.get::<_, String>(3)?)?,
            who: row.get(4)?,
            how: row.get(5)?,
            started: row.get(6)?,
            state,
            step: row.get(8)?,
            steps: row.get(9)?,
            words: row.get(10)?,
            percent: row.get(11)?,
            ended: row.get(12)?,
            status: row.get(13)?,
            note: row.get(14)?,
            log: if log.is_empty() {
                Vec::new()
            } else {
                log.split('\n').map(String::from).collect()
            },
        })
    }
}

/// The columns of a row, in the order [`Task::read`] takes them.
const COLUMNS: &str = "id, action, args, argv, who, how, started, state, step, steps, words, \
                       percent, ended, status, note, log";

impl Store {
    /// Adds a task for the action `action`, waiting, and gives its
    /// number.
    ///
    /// # Errors
    ///
    /// If the store fails.
    pub fn add_task(
        &self,
        action: &str,
        args: &Value,
        argv: &[String],
        who: &str,
        how: &str,
        started: i64,
    ) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO tasks (action, args, argv, who, how, started, state)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'waiting')",
            params![action, to_json(args)?, to_json(argv)?, who, how, started],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Writes what `task` is now: its state, step, progress, end and
    /// the last [`LOG_MAX`] lines of its log.
    ///
    /// # Errors
    ///
    /// If the store fails.
    pub fn save_task(&self, task: &Task) -> Result<()> {
        let skip = task.log.len().saturating_sub(LOG_MAX);
        self.conn.execute(
            "UPDATE tasks SET state = ?2, step = ?3, steps = ?4, words = ?5, percent = ?6,
                 ended = ?7, status = ?8, note = ?9, log = ?10 WHERE id = ?1",
            params![
                task.id,
                task.state.word(),
                task.step,
                task.steps,
                task.words,
                task.percent,
                task.ended,
                task.status,
                task.note,
                task.log[skip..].join("\n"),
            ],
        )?;
        Ok(())
    }

    /// The task numbered `id`.
    ///
    /// # Errors
    ///
    /// If the store fails.
    pub fn task(&self, id: i64) -> Result<Option<Task>> {
        Ok(self
            .conn
            .query_row(&format!("SELECT {COLUMNS} FROM tasks WHERE id = ?1"), [id], Task::read)
            .optional()?)
    }

    /// The tasks that wait or run, and those that ended at Unix time
    /// `since` or later, newest first, at most `limit` of them.
    ///
    /// # Errors
    ///
    /// If the store fails.
    pub fn tasks(&self, since: i64, limit: usize) -> Result<Vec<Task>> {
        let mut query = self.conn.prepare(&format!(
            "SELECT {COLUMNS} FROM tasks
             WHERE state IN ('waiting', 'running') OR ended >= ?1 ORDER BY id DESC LIMIT ?2"
        ))?;
        let rows = query.query_map(params![since, limit as i64], Task::read)?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// Ends every task that waits or runs as stopped at Unix time `at`,
    /// saying why in `note`: they were left so by an admind that ended.
    /// Gives how many.
    ///
    /// # Errors
    ///
    /// If the store fails.
    pub fn stop_unfinished(&self, at: i64, note: &str) -> Result<usize> {
        Ok(self.conn.execute(
            "UPDATE tasks SET state = 'stopped', ended = ?1, note = ?2
             WHERE state IN ('waiting', 'running')",
            params![at, note],
        )?)
    }

    /// Forgets the tasks that ended more than [`KEPT_DAYS`] days before
    /// Unix time `now`. Gives how many.
    ///
    /// # Errors
    ///
    /// If the store fails.
    pub fn forget_old_tasks(&self, now: i64) -> Result<usize> {
        Ok(self.conn.execute(
            "DELETE FROM tasks WHERE ended IS NOT NULL AND ended < ?1",
            [now - KEPT_DAYS * 86_400],
        )?)
    }
}

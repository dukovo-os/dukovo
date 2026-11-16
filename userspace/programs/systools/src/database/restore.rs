// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `database restore`: a copy that `pg_dump -Fc` made, put back into a
//! database of its own.
//!
//! The copy goes into a new database, and nothing is written over. With
//! `--as` the new database has a name of its own, and the database the
//! copy is of is not touched. With `--over` it is the database itself that
//! is replaced: the database as it is stays, renamed, and the copy goes
//! into a new database with the old name, which its owner cannot connect
//! to until it is whole, so that no program finds it empty. Every step
//! that was made is taken back when a later one fails.
//!
//! The copy is opened once, here, without following a link, and what is
//! checked of it is checked of that open file, which `pg_restore` then
//! reads as its standard input: the file that was checked is the file that
//! is put back, whatever is done to its name meanwhile. `pg_restore` is the
//! bundle's own command, run as root.
//!
//! A copy holds what the database's owner wrote, and putting it back runs
//! some of it: a check or a default that calls a function of the owner's
//! runs as each row goes in. So it is put back with the owner's rights and
//! no more. `pg_restore` does not sign in as the superuser, whose session a
//! `RESET ROLE` in such a function would go back to: it signs in as
//! [`RESTORER`], a role made for the restore, which the server lets in by
//! the socket as the account of the same name (`local all all peer`). That
//! role holds no right of its own, inherits nothing, and may become the
//! owner and no one else, which `pg_restore` does at once: what the copy
//! does is done as the owner, and a way back out of the owner's role lands
//! in a role that can do nothing. It may connect to the new database alone,
//! and it is removed when the restore ends, well or not. A copy of a
//! database that a role with every right owns is not put back here.

use std::fs::{File, OpenOptions};
use std::os::unix::fs::{FileExt, OpenOptionsExt};
use std::path::Path;

use serde::Serialize;

use super::add::{built, found, no_database, ready, refused, run, sessions};
use super::server::{Clock, Host, SOCKET_DIR};
use super::sql::{self, Like, Sorting};
use super::wire::Error;
use super::{errors, names};
use crate::cert::acme::steps::Steps;
use crate::util::{Fail, fail};

/// The role `pg_restore` signs in as. It is called as the account the
/// command runs as, root, since the server lets an account in by the
/// socket as the role of its own name.
pub const RESTORER: &str = "root";

/// The longest copy that is put back: 64 GiB.
pub const COPY_MAX: u64 = 64 << 30;

/// What a copy made with `pg_dump -Fc` starts with.
pub const MAGIC: &[u8; 5] = b"PGDMP";

/// The steps a restore as a new database tells with `--progress`.
pub const NEW_STEPS: usize = 3;

/// The steps a restore over the database itself tells.
pub const OVER_STEPS: usize = 5;

/// How many names are tried for a new database before it is given up.
const NAMES_MAX: u32 = 99;

/// Where the copy goes.
#[derive(Debug, PartialEq, Eq)]
pub enum Target {
    /// Into a new database, called what was asked or, when it was not,
    /// `NAME_restored_` and the day.
    New(Option<String>),
    /// Over the database itself, which is kept as `NAME_before_` and the
    /// time; `force` ends the sessions open to it.
    Over {
        /// Whether to end the sessions that are open.
        force: bool,
    },
}

/// What `restore` is asked to do.
#[derive(Debug, PartialEq, Eq)]
pub struct Restore {
    /// The database the copy is of.
    pub name: String,
    /// The copy: the full path of the file.
    pub from: String,
    /// Where it goes.
    pub target: Target,
}

/// How many steps `restore` tells for `target`.
#[must_use]
pub fn steps_of(target: &Target) -> usize {
    match target {
        Target::New(_) => NEW_STEPS,
        Target::Over { .. } => OVER_STEPS,
    }
}

/// What a restore did: the document `--json` prints after its `format`.
#[derive(Debug, Serialize)]
pub struct Restored {
    /// The database the copy is of.
    pub database: String,
    /// The copy.
    pub from: String,
    /// The database the copy was put into.
    pub restored_as: String,
    /// Where the database as it was is kept, for `--over`.
    pub kept_as: Option<String>,
    /// How long it took, in milliseconds.
    pub took_ms: u64,
}

impl Restored {
    /// What `restore` says to a person.
    #[must_use]
    pub fn words(&self) -> Vec<String> {
        match &self.kept_as {
            None => vec![format!(
                "put the copy into the new database {}; {} was not changed",
                self.restored_as, self.database
            )],
            Some(kept) => vec![
                format!(
                    "put the copy into {}; the database as it was is kept as {kept}",
                    self.restored_as
                ),
                format!("`database remove {kept}` removes it when it is not needed"),
            ],
        }
    }
}

/// How a copy is judged, from what is known of the file it was opened as:
/// why it will not do, or nothing.
fn judge(is_file: bool, length: u64, start: Option<[u8; 5]>) -> Result<(), &'static str> {
    if !is_file {
        return Err("it is not a file");
    }
    if length > COPY_MAX {
        return Err("it is over 64 GiB");
    }
    if start.as_ref() != Some(MAGIC) {
        return Err("it does not start as a copy made with pg_dump -Fc does");
    }
    Ok(())
}

/// Opens the copy at `path`, never through a link, and checks it: a
/// regular file of at most [`COPY_MAX`] bytes that starts with [`MAGIC`].
/// The file is left to be read from its start.
///
/// # Errors
///
/// DB-109, saying what is wrong with it.
pub fn open_copy(path: &str) -> Result<File, Fail> {
    let flags = libc::O_NOFOLLOW | libc::O_NONBLOCK;
    let file = OpenOptions::new().read(true).custom_flags(flags).open(path).map_err(|error| {
        let link = std::fs::symlink_metadata(Path::new(path))
            .is_ok_and(|meta| meta.file_type().is_symlink());
        let why = if link {
            "it is a link, not a file".to_string()
        } else if error.kind() == std::io::ErrorKind::NotFound {
            "it is not there".to_string()
        } else {
            format!("it cannot be read: {error}")
        };
        errors::bad_copy(path, &why)
    })?;
    let meta = file.metadata().map_err(|error| errors::bad_copy(path, &error.to_string()))?;
    let mut start = [0u8; 5];
    // The offset of the file is not moved: the program that reads it
    // starts at the first byte.
    let start = file.read_exact_at(&mut start, 0).ok().map(|()| start);
    judge(meta.is_file(), meta.len(), start).map_err(|why| errors::bad_copy(path, why))?;
    Ok(file)
}

/// The database a copy is put back for, as it is: whose it is, how it
/// stores and sorts text, and its comment.
struct Original {
    /// The role that owns it, which has not every right.
    owner: String,
    /// How a database like it is made.
    like: Like,
    /// Its comment.
    comment: Option<String>,
}

/// The database `name` as it is.
fn original(host: &mut dyn Host, name: &str) -> Result<Original, Fail> {
    let answers = run(host, &built(sql::the_original(name))?)?;
    let Some(answer) = answers.first().filter(|answer| !answer.rows.is_empty()) else {
        return Err(no_database(name));
    };
    let owner = answer.first("owner").unwrap_or_default().to_string();
    if answer.first("owner_super") != Some("f") {
        return Err(fail(format!(
            "{name} is owned by {owner}, a role with every right: database puts a copy back \
             with no more than the rights of the database's owner, so it does not put back a \
             copy of a database that such a role owns"
        )));
    }
    let locale = answer.first("locale");
    let sorting = match answer.first("provider") {
        Some("c") => {
            let collate = answer.first("collate").or(locale).unwrap_or_default();
            let ctype = answer.first("ctype").unwrap_or(collate);
            Sorting::Libc { collate: collate.into(), ctype: ctype.into() }
        }
        Some("b") => Sorting::Builtin { locale: locale.unwrap_or_default().into() },
        _ => {
            return Err(fail(format!(
                "{name} sorts text with ICU, which database does not make a database with: its \
                 copy cannot be put back by it"
            )));
        }
    };
    Ok(Original {
        owner,
        like: Like { encoding: answer.first("encoding").unwrap_or("UTF8").into(), sorting },
        comment: answer.first("comment").map(String::from),
    })
}

/// A name for a new database that is free: `NAME_WORD_STAMP`, and with
/// `_2`, `_3` and on when that is taken. `hint` says what to do when no
/// name that database makes is good.
fn free_name(
    host: &mut dyn Host,
    name: &str,
    word: &str,
    stamp: &str,
    hint: &str,
) -> Result<String, Fail> {
    let base = format!("{name}_{word}_{stamp}");
    for number in 1..=NAMES_MAX {
        let candidate = if number == 1 { base.clone() } else { format!("{base}_{number}") };
        names::check_name(&candidate).map_err(|why| {
            fail(format!(
                "the name {candidate}, which database would give it, will not do: {why}; {hint}"
            ))
        })?;
        if !found(host, &built(sql::database_exists(&candidate))?)? {
            return Ok(candidate);
        }
    }
    Err(fail(format!("the names {base} and {base}_2 to _{NAMES_MAX} are all taken; {hint}")))
}

/// What a restore has made so far, which taking it back undoes.
struct Done<'a> {
    /// The database the copy is of.
    name: &'a str,
    /// The role that owns it, and the new one.
    owner: &'a str,
    /// The database the copy goes into.
    new: &'a str,
    /// Where `name` was renamed to, for `--over`.
    kept: Option<&'a str>,
    /// Whether `new` was made.
    created: bool,
    /// Whether `name` was renamed to `kept`.
    renamed: bool,
    /// The database that takes no connection at all, by the name it has
    /// now.
    shut: Option<String>,
    /// Whether [`RESTORER`] was made.
    restorer: bool,
}

impl<'a> Done<'a> {
    /// Nothing made yet.
    fn new(name: &'a str, owner: &'a str, new: &'a str, kept: Option<&'a str>) -> Done<'a> {
        Done { name, owner, new, kept, created: false, renamed: false, shut: None, restorer: false }
    }

    /// Makes [`RESTORER`]: a role that may sign in, holds nothing, may
    /// become the owner alone, and may connect to the new database alone.
    fn let_restorer_in(&mut self, host: &mut dyn Host) -> Result<(), Fail> {
        let made = built(sql::create_restorer(RESTORER))?;
        host.sql(&made).map_err(|error| refused(&error, "role", RESTORER))?;
        self.restorer = true;
        run(host, &built(sql::may_become(RESTORER, self.owner))?)?;
        run(host, &built(sql::connect_right(self.new, RESTORER, true))?)?;
        Ok(())
    }

    /// Removes [`RESTORER`] when it was made. A role that cannot be removed
    /// is left so that it signs in no more and may become no one, and the
    /// words say so; none when it is gone.
    fn restorer_gone(&mut self, host: &mut dyn Host) -> Option<String> {
        if !std::mem::take(&mut self.restorer) {
            return None;
        }
        let mut take = |statement: Result<String, String>| {
            built(statement).and_then(|statement| run(host, &statement)).map(|_| ())
        };
        // The right to connect is a part of the role that would keep it.
        let _ = take(sql::connect_right(self.new, RESTORER, false));
        let why = match take(sql::drop_role(RESTORER)) {
            Ok(()) => return None,
            Err(why) => why.message,
        };
        let _ = take(sql::no_login(RESTORER));
        let _ = take(sql::may_not_become(RESTORER, self.owner));
        Some(format!(
            "the role {RESTORER}, which put the copy back, stays and signs in no more ({why})"
        ))
    }

    /// Takes back what was made, last first, and says how that went, in
    /// words that follow `the copy was not put into NEW: `. Empty when
    /// nothing had been made.
    fn undo(&mut self, host: &mut dyn Host) -> String {
        let mut said = Vec::new();
        said.extend(self.restorer_gone(host));
        let mut stuck = false;
        if self.created {
            let dropped = sql::drop_database(self.new, false)
                .map_err(fail)
                .and_then(|statement| run(host, &statement));
            match dropped {
                Ok(_) => said.push(format!("the database {} was removed again", self.new)),
                Err(why) => {
                    stuck = true;
                    said.push(format!("the database {} stays ({})", self.new, why.message));
                }
            }
        }
        if let (true, Some(kept)) = (self.renamed, self.kept) {
            if stuck {
                said.push(format!("{} as it was stays as {kept}", self.name));
            } else {
                let back = sql::rename_database(kept, self.name)
                    .map_err(fail)
                    .and_then(|statement| run(host, &statement));
                match back {
                    Ok(_) => {
                        said.push(format!("{kept} is {} again", self.name));
                        if self.shut.as_deref() == Some(kept) {
                            self.shut = Some(self.name.to_string());
                        }
                    }
                    Err(why) => {
                        said.push(format!("{} stays as {kept} ({})", self.name, why.message));
                    }
                }
            }
        }
        if let Some(shut) = self.shut.take() {
            let opened = sql::allow_connections(&shut, true)
                .map_err(fail)
                .and_then(|statement| run(host, &statement));
            if let Err(why) = opened {
                said.push(format!("{shut} stays shut to connections ({})", why.message));
            }
        }
        said.join(", ")
    }

    /// `failure`, with how taking back went, when something was made.
    fn failed(&mut self, host: &mut dyn Host, mut failure: Fail) -> Fail {
        let back = self.undo(host);
        if !back.is_empty() {
            failure.message = format!("{}; {back}", failure.message);
        }
        failure
    }
}

/// `database restore`: puts the copy into a new database, or over the
/// database itself. Gives what was done.
///
/// # Errors
///
/// DB-101, DB-102 for a new name that is taken, DB-103 for open sessions
/// when `--over` is not forced, DB-109 for a copy that will not do, DB-110
/// when `pg_restore` did not put the copy in, and PostgreSQL's words for
/// anything else. Whatever was made is taken back.
pub fn restore(host: &mut dyn Host, ask: &Restore, steps: &Steps) -> Result<Restored, Fail> {
    let began = host.now();
    steps.next("checking the copy and the database");
    ready(host)?;
    let copy = open_copy(&ask.from)?;
    let original = original(host, &ask.name)?;
    if found(host, &built(sql::role_exists(RESTORER))?)? {
        return Err(fail(format!(
            "there is a role called {RESTORER} already: database restore signs in as a role of \
             that name, which it makes for the restore and removes after it; one left by a \
             restore that was stopped is removed with `elevate psql -U postgres -c 'DROP ROLE \
             {RESTORER}'`, and then the restore can run"
        )));
    }
    let (new, kept) = match &ask.target {
        Target::New(given) => (new_name(host, &ask.name, given.as_deref(), &began)?, None),
        Target::Over { force } => {
            if !force {
                let open = sessions(host, &ask.name)?;
                if !open.is_empty() {
                    return Err(errors::sessions(&ask.name, &open));
                }
            }
            let hint = "restore into a new database instead, with --as NEW";
            let kept = free_name(host, &ask.name, "before", &began.minute(), hint)?;
            (ask.name.clone(), Some(kept))
        }
    };
    let mut done = Done::new(&ask.name, &original.owner, &new, kept.as_deref());
    if let (Target::Over { force }, Some(kept)) = (&ask.target, &kept) {
        steps.next(&format!("keeping {} as {kept}", ask.name));
        keep(host, &mut done, *force)?;
    }
    steps.next(&format!("making the database {new}"));
    let made =
        make(host, &mut done, &original, kept.is_some()).and_then(|()| done.let_restorer_in(host));
    if let Err(failure) = made {
        return Err(done.failed(host, failure));
    }
    steps.next(&format!("putting the copy into {new}"));
    let argv = pg_restore_argv(&original.owner, &new);
    if let Err(said) = host.pg_restore(&argv, copy) {
        let back = done.undo(host);
        return Err(errors::not_put_back(&new, &said, &back_words(&back)));
    }
    if let Some(stays) = done.restorer_gone(host) {
        host.log(&format!("restoring {}: {stays}", ask.name));
    }
    if kept.is_some() {
        steps.next(&format!("letting programs connect to {new} again"));
        let opened = built(sql::connect_right(&new, &original.owner, true))
            .and_then(|statement| run(host, &statement));
        if let Err(failure) = opened {
            return Err(done.failed(host, failure));
        }
    }
    let ended = host.now();
    let restored = Restored {
        database: ask.name.clone(),
        from: ask.from.clone(),
        restored_as: new,
        kept_as: kept,
        took_ms: ended.ms.saturating_sub(began.ms),
    };
    host.log(&match &restored.kept_as {
        Some(kept) => format!(
            "restored {} over itself from {}; the database as it was is kept as {kept}",
            restored.database, restored.from
        ),
        None => format!(
            "restored {} from {} into the new database {}",
            restored.database, restored.from, restored.restored_as
        ),
    });
    Ok(restored)
}

/// How taking back went, as the last part of DB-110's words.
fn back_words(back: &str) -> String {
    if back.is_empty() { "nothing was changed".to_string() } else { back.to_string() }
}

/// The name of the new database for a restore `--as`: the name asked for,
/// which must be free, or, when none was, one that is made of the name of
/// the database and the day.
fn new_name(
    host: &mut dyn Host,
    name: &str,
    given: Option<&str>,
    now: &Clock,
) -> Result<String, Fail> {
    match given {
        Some(new) => {
            if found(host, &built(sql::database_exists(new))?)? {
                return Err(errors::taken("database", new));
            }
            Ok(new.to_string())
        }
        None => free_name(host, name, "restored", &now.day(), "name it with --as NEW"),
    }
}

/// `--over`: the database is renamed to where it is kept. With `force` its
/// sessions are ended first, and no new one is let in meanwhile.
fn keep(host: &mut dyn Host, done: &mut Done<'_>, force: bool) -> Result<(), Fail> {
    let (Some(kept), name) = (done.kept, done.name) else { return Ok(()) };
    if force {
        run(host, &built(sql::allow_connections(name, false))?)?;
        done.shut = Some(name.to_string());
        if let Err(failure) = run(host, &built(sql::end_sessions_of(name))?) {
            return Err(done.failed(host, failure));
        }
    }
    let renamed = built(sql::rename_database(name, kept)).and_then(|statement| {
        host.sql(&statement).map_err(|error| match &error {
            // A session came in, or is not gone yet.
            Error::Server(pg) if errors::is_in_use(pg) => {
                errors::sessions(name, &sessions(host, name).unwrap_or_default())
            }
            other => errors::said(other),
        })
    });
    if let Err(failure) = renamed {
        return Err(done.failed(host, failure));
    }
    done.renamed = true;
    if force {
        done.shut = Some(kept.to_string());
        let opened = built(sql::allow_connections(kept, true)).and_then(|s| run(host, &s));
        match opened {
            Ok(_) => done.shut = None,
            Err(failure) => return Err(done.failed(host, failure)),
        }
    }
    Ok(())
}

/// Makes the new database like the original: its owner, its sorting, no
/// connection for anyone but the owner, and, when it takes the place of the
/// original (`closed`), its comment. A database that takes the place of
/// another takes no connection of its owner's either, until the copy is in
/// it.
fn make(
    host: &mut dyn Host,
    done: &mut Done<'_>,
    original: &Original,
    closed: bool,
) -> Result<(), Fail> {
    let new = done.new;
    let statement = built(sql::create_like(new, &original.owner, &original.like))?;
    host.sql(&statement).map_err(|error| refused(&error, "database", new))?;
    done.created = true;
    run(host, &built(sql::revoke_connect(new))?)?;
    if closed {
        run(host, &built(sql::connect_right(new, &original.owner, false))?)?;
        if let Some(comment) = &original.comment {
            run(host, &built(sql::comment_on_database_as(new, comment))?)?;
        }
    }
    Ok(())
}

/// The words of `pg_restore`: signed in over the server's socket as
/// [`RESTORER`], as the owner of the database from its first statement,
/// into the database `new`, and ending at the first thing it cannot do.
/// A subscription of the copy, which would connect to another server, is
/// not made. The copy is read from its standard input.
#[must_use]
pub fn pg_restore_argv(owner: &str, new: &str) -> Vec<String> {
    [
        "--no-owner",
        &format!("--role={owner}"),
        "--exit-on-error",
        "--no-subscriptions",
        "-h",
        SOCKET_DIR,
        "-U",
        RESTORER,
        "-d",
        new,
    ]
    .map(String::from)
    .to_vec()
}

#[cfg(test)]
mod tests {
    use super::super::fake::{Fake, START_MS};
    use super::*;
    use crate::json;

    /// A fake with the database `shop` and its role, and a copy of it at
    /// `copy`, which the tests put back.
    fn shop() -> (Fake, String) {
        let fake = Fake::new()
            .with_database("shop", "shop")
            .made_by_database("shop", "shop")
            .with_site("shop", "shop.example.org")
            .with_copy("shop", 4096, START_MS);
        let copy = fake.copies_folder().join("shop.dump").to_string_lossy().into_owned();
        (fake, copy)
    }

    fn ask(from: &str, target: Target) -> Restore {
        Restore { name: "shop".into(), from: from.into(), target }
    }

    fn run_it(fake: &mut Fake, ask: &Restore) -> Result<Restored, Fail> {
        restore(fake, ask, &Steps::off())
    }

    /// The statements of the fake that change something, as strings.
    fn changes(fake: &Fake) -> Vec<String> {
        fake.changes()
    }

    #[test]
    fn a_copy_is_a_regular_file_that_starts_as_pg_dump_makes_it() {
        let (fake, copy) = shop();
        assert!(open_copy(&copy).is_ok());
        let folder = fake.copies_folder();
        let words = |path: &str| open_copy(path).unwrap_err().message;
        // Not there.
        let gone = folder.join("none.dump").to_string_lossy().into_owned();
        assert_eq!(
            words(&gone),
            format!("{gone} is not a copy that can be put back: it is not there (DB-109)")
        );
        // A link is not followed, to a copy that is good.
        let link = folder.join("link.dump");
        std::os::unix::fs::symlink(&copy, &link).unwrap();
        let link = link.to_string_lossy().into_owned();
        assert!(words(&link).contains("it is a link, not a file (DB-109)"), "{}", words(&link));
        // A folder is no file.
        let dir = folder.to_string_lossy().into_owned();
        assert!(words(&dir).contains("DB-109"), "{}", words(&dir));
        // Not a copy: another start, a text, and a file too short.
        for (name, content) in [
            ("sql.dump", &b"-- PostgreSQL database dump\n"[..]),
            ("tar.dump", b"toc.dat\0\0\0\0"),
            ("short.dump", b"PGDM"),
            ("empty.dump", b""),
        ] {
            let path = folder.join(name);
            std::fs::write(&path, content).unwrap();
            let path = path.to_string_lossy().into_owned();
            assert!(
                words(&path).contains("it does not start as a copy made with pg_dump -Fc does"),
                "{name}: {}",
                words(&path)
            );
        }
    }

    #[test]
    fn a_copy_is_judged_by_what_the_open_file_is() {
        let magic = Some(*MAGIC);
        assert_eq!(judge(true, 1, magic), Ok(()));
        assert_eq!(judge(true, COPY_MAX, magic), Ok(()));
        assert_eq!(judge(true, COPY_MAX + 1, magic), Err("it is over 64 GiB"));
        assert_eq!(judge(false, 1, magic), Err("it is not a file"));
        assert!(judge(true, 5, None).is_err());
        assert!(judge(true, 5, Some(*b"PGDMX")).is_err());
        assert_eq!(COPY_MAX, 68_719_476_736);
    }

    #[test]
    fn the_copy_is_read_from_its_first_byte_after_the_check() {
        use std::io::Seek;
        let (_fake, copy) = shop();
        let mut file = open_copy(&copy).unwrap();
        assert_eq!(file.stream_position().unwrap(), 0);
    }

    #[test]
    fn the_new_database_is_called_by_the_name_the_day_and_a_number_when_it_is_taken() {
        let (mut fake, copy) = shop();
        let restored = run_it(&mut fake, &ask(&copy, Target::New(None))).unwrap();
        assert_eq!(restored.restored_as, "shop_restored_20261116");
        assert!(fake.has_database("shop_restored_20261116"));
        let restored = run_it(&mut fake, &ask(&copy, Target::New(None))).unwrap();
        assert_eq!(restored.restored_as, "shop_restored_20261116_2");
        let restored = run_it(&mut fake, &ask(&copy, Target::New(None))).unwrap();
        assert_eq!(restored.restored_as, "shop_restored_20261116_3");
    }

    #[test]
    fn a_name_that_would_not_do_is_said_and_nothing_is_made() {
        // 41 characters and the 18 of the rest make 59; 46 make 64.
        let long = "a".repeat(46);
        let mut fake = Fake::new().with_database(&long, &long).with_copy(&long, 100, START_MS);
        let path = fake.copies_folder().join(format!("{long}.dump")).to_string_lossy().into_owned();
        let failure = restore(
            &mut fake,
            &Restore { name: long.clone(), from: path.clone(), target: Target::New(None) },
            &Steps::off(),
        )
        .unwrap_err();
        assert!(failure.message.contains("will not do"), "{}", failure.message);
        assert!(failure.message.ends_with("name it with --as NEW"), "{}", failure.message);
        assert!(changes(&fake).is_empty());
        // Names that start with pg_ are not made either.
        let mut fake = Fake::new().with_database("pg", "pg").with_copy("pg", 100, START_MS);
        let path = fake.copies_folder().join("pg.dump").to_string_lossy().into_owned();
        let kept = restore(
            &mut fake,
            &Restore {
                name: "pg".into(),
                from: path.clone(),
                target: Target::Over { force: false },
            },
            &Steps::off(),
        )
        .unwrap_err();
        assert!(kept.message.contains("pg_before_20261116_0200"), "{}", kept.message);
        assert!(kept.message.contains("restore into a new database instead"), "{}", kept.message);
        assert!(changes(&fake).is_empty());
    }

    #[test]
    fn a_name_that_is_asked_for_must_be_free() {
        let (fake, copy) = shop();
        let mut fake = fake.with_database("shop_old", "shop");
        let taken =
            run_it(&mut fake, &ask(&copy, Target::New(Some("shop_old".into())))).unwrap_err();
        assert!(taken.message.ends_with("(DB-102)"), "{}", taken.message);
        assert!(changes(&fake).is_empty());
        let restored =
            run_it(&mut fake, &ask(&copy, Target::New(Some("shop_new".into())))).unwrap();
        assert_eq!(restored.restored_as, "shop_new");
        // The database itself is taken too.
        let failure = run_it(&mut fake, &ask(&copy, Target::New(Some("shop".into())))).unwrap_err();
        assert!(failure.message.ends_with("(DB-102)"), "{}", failure.message);
    }

    #[test]
    fn a_database_that_is_not_there_is_said() {
        let (mut fake, copy) = shop();
        let failure = restore(
            &mut fake,
            &Restore { name: "blog".into(), from: copy, target: Target::New(None) },
            &Steps::off(),
        )
        .unwrap_err();
        assert!(failure.message.starts_with("there is no database called blog"));
        assert!(changes(&fake).is_empty());
    }

    #[test]
    fn a_server_that_is_not_there_is_db_101_and_a_copy_that_will_not_do_is_db_109() {
        let (fake, copy) = shop();
        let mut fake = fake.off();
        let failure = run_it(&mut fake, &ask(&copy, Target::New(None))).unwrap_err();
        assert!(failure.message.ends_with("(DB-101)"), "{}", failure.message);
        let mut fake = Fake::new().with_database("shop", "shop");
        let failure = run_it(&mut fake, &ask("/nowhere/x.dump", Target::New(None))).unwrap_err();
        assert!(failure.message.ends_with("(DB-109)"), "{}", failure.message);
        assert!(changes(&fake).is_empty());
    }

    #[test]
    fn as_makes_a_database_like_the_old_one_and_puts_the_copy_in_it() {
        let (mut fake, copy) = shop();
        let restored =
            run_it(&mut fake, &ask(&copy, Target::New(Some("shop_old".into())))).unwrap();
        assert_eq!(
            changes(&fake),
            [
                "CREATE DATABASE \"shop_old\" OWNER \"shop\" TEMPLATE template0 ENCODING 'UTF8' \
                 LOCALE_PROVIDER libc LC_COLLATE 'en_US.UTF-8' LC_CTYPE 'en_US.UTF-8'",
                "REVOKE ALL ON DATABASE \"shop_old\" FROM PUBLIC",
                "CREATE ROLE \"root\" LOGIN NOINHERIT",
                "GRANT \"shop\" TO \"root\" WITH INHERIT FALSE, SET TRUE",
                "GRANT CONNECT ON DATABASE \"shop_old\" TO \"root\"",
                "REVOKE CONNECT ON DATABASE \"shop_old\" FROM \"root\"",
                "DROP ROLE \"root\"",
            ]
        );
        assert_eq!(restored.kept_as, None);
        // The owner connects to it, and the role that put the copy in is gone.
        assert_eq!(fake.connects("shop_old"), Some((true, vec![])));
        assert!(!fake.has_role("root"));
        // The old one is as it was, and the new one has the copy.
        assert!(fake.has_database("shop"));
        assert_eq!(fake.restored("shop_old"), Some(4096));
        assert_eq!(fake.takes("shop_old"), Some((true, -1)));
        // The new one has no site: the old one is the website's.
        assert_eq!(fake.comment_of("shop_old"), None);
        assert!(fake.comment_of("shop").is_some());
        assert_eq!(
            restored.words(),
            ["put the copy into the new database shop_old; shop was not changed"]
        );
        assert!(
            fake.logged().last().unwrap().contains("restored shop from"),
            "{:?}",
            fake.logged()
        );
    }

    #[test]
    fn pg_restore_signs_in_as_a_role_held_to_the_owner_and_reads_the_copy_from_its_start() {
        let (mut fake, copy) = shop();
        run_it(&mut fake, &ask(&copy, Target::New(Some("shop_old".into())))).unwrap();
        let calls = fake.restores();
        assert_eq!(calls.len(), 1);
        assert_eq!(
            calls[0].argv,
            [
                "--no-owner",
                "--role=shop",
                "--exit-on-error",
                "--no-subscriptions",
                "-h",
                "/system/live/services/postgres",
                "-U",
                "root",
                "-d",
                "shop_old"
            ]
        );
        assert_eq!((calls[0].length, calls[0].offset, calls[0].closed), (4096, 0, false));
        // It signed in as a role that holds nothing, may become the owner
        // alone and may connect to the new database alone: never as the
        // superuser, whose session a copy could go back to.
        assert!(calls[0].held_to_the_owner);
        // No word of it is the copy's path, which is not named to it.
        assert!(calls[0].argv.iter().all(|word| !word.contains(".dump")));
        // The owner is the owner of the database, whoever it is.
        assert_eq!(
            pg_restore_argv("web dev", "x")[1],
            "--role=web dev",
            "a role is one word, whatever it is called"
        );
    }

    #[test]
    fn over_keeps_the_old_database_renamed_and_makes_the_new_one_that_takes_no_connection() {
        let (mut fake, copy) = shop();
        let restored = run_it(&mut fake, &ask(&copy, Target::Over { force: false })).unwrap();
        assert_eq!(restored.restored_as, "shop");
        assert_eq!(restored.kept_as.as_deref(), Some("shop_before_20261116_0200"));
        assert_eq!(
            changes(&fake),
            [
                "ALTER DATABASE \"shop\" RENAME TO \"shop_before_20261116_0200\"",
                "CREATE DATABASE \"shop\" OWNER \"shop\" TEMPLATE template0 ENCODING 'UTF8' \
                 LOCALE_PROVIDER libc LC_COLLATE 'en_US.UTF-8' LC_CTYPE 'en_US.UTF-8'",
                "REVOKE ALL ON DATABASE \"shop\" FROM PUBLIC",
                "REVOKE CONNECT ON DATABASE \"shop\" FROM \"shop\"",
                "COMMENT ON DATABASE \"shop\" IS 'database(8): site shop.example.org'",
                "CREATE ROLE \"root\" LOGIN NOINHERIT",
                "GRANT \"shop\" TO \"root\" WITH INHERIT FALSE, SET TRUE",
                "GRANT CONNECT ON DATABASE \"shop\" TO \"root\"",
                "REVOKE CONNECT ON DATABASE \"shop\" FROM \"root\"",
                "DROP ROLE \"root\"",
                "GRANT CONNECT ON DATABASE \"shop\" TO \"shop\"",
            ]
        );
        // The copy was put into the new one while its owner could not
        // connect, by the role held to the owner.
        let calls = fake.restores();
        assert_eq!(calls.len(), 1);
        assert!(calls[0].closed);
        assert!(calls[0].held_to_the_owner);
        assert_eq!(fake.connects("shop"), Some((true, vec![])));
        assert_eq!(calls[0].argv.last().map(String::as_str), Some("shop"));
        assert_eq!(fake.restored("shop"), Some(4096));
        assert_eq!(fake.restored("shop_before_20261116_0200"), None);
        assert_eq!(fake.takes("shop"), Some((true, -1)));
        assert!(fake.has_database("shop_before_20261116_0200"));
        assert_eq!(
            restored.words(),
            [
                "put the copy into shop; the database as it was is kept as \
                 shop_before_20261116_0200",
                "`database remove shop_before_20261116_0200` removes it when it is not needed"
            ]
        );
    }

    #[test]
    fn the_rename_comes_before_anything_is_made_and_the_old_name_is_taken_once() {
        let (fake, copy) = shop();
        let mut fake = fake.with_database("shop_before_20261116_0200", "shop");
        let restored = run_it(&mut fake, &ask(&copy, Target::Over { force: false })).unwrap();
        assert_eq!(restored.kept_as.as_deref(), Some("shop_before_20261116_0200_2"));
        let changes = changes(&fake);
        assert!(changes[0].starts_with("ALTER DATABASE \"shop\" RENAME TO"), "{changes:?}");
        assert!(changes[1].starts_with("CREATE DATABASE \"shop\""), "{changes:?}");
    }

    #[test]
    fn over_is_refused_while_sessions_are_open_unless_they_are_forced_to_end() {
        let (fake, copy) = shop();
        let mut fake = fake.with_sessions("shop", "php", 2);
        let failure = run_it(&mut fake, &ask(&copy, Target::Over { force: false })).unwrap_err();
        assert!(failure.message.ends_with("(DB-103)"), "{}", failure.message);
        assert!(failure.message.contains("php (2)"), "{}", failure.message);
        assert!(changes(&fake).is_empty());
        assert!(fake.restores().is_empty());
        // As --as never needs them gone.
        run_it(&mut fake, &ask(&copy, Target::New(None))).unwrap();
        // Forced, they are ended and no other comes in while it is renamed.
        let restored = run_it(&mut fake, &ask(&copy, Target::Over { force: true })).unwrap();
        assert_eq!(fake.open_sessions("shop"), 0);
        let sent: Vec<&str> = fake
            .sent()
            .iter()
            .map(String::as_str)
            .skip_while(|s| !s.starts_with("ALTER DATABASE \"shop\" WITH ALLOW"))
            .collect();
        assert_eq!(sent[0], "ALTER DATABASE \"shop\" WITH ALLOW_CONNECTIONS false");
        assert!(sent[1].starts_with("SELECT pg_catalog.pg_terminate_backend(pid)"), "{sent:?}");
        assert_eq!(sent[2], "ALTER DATABASE \"shop\" RENAME TO \"shop_before_20261116_0200\"");
        assert_eq!(
            sent[3],
            "ALTER DATABASE \"shop_before_20261116_0200\" WITH ALLOW_CONNECTIONS true"
        );
        assert!(sent[4].starts_with("CREATE DATABASE \"shop\""), "{sent:?}");
        // The old one is open again, and the new one is whole.
        assert_eq!(fake.takes(restored.kept_as.as_deref().unwrap()), Some((true, -1)));
        assert_eq!(fake.takes("shop"), Some((true, -1)));
    }

    #[test]
    fn a_session_that_comes_in_before_the_rename_is_db_103_and_the_database_is_open_again() {
        let (fake, copy) = shop();
        let mut fake = fake.refusing(
            "ALTER DATABASE \"shop\" RENAME",
            "55006",
            "database \"shop\" is being accessed by other users",
        );
        let failure = run_it(&mut fake, &ask(&copy, Target::Over { force: true })).unwrap_err();
        assert!(failure.message.ends_with("(DB-103)"), "{}", failure.message);
        assert_eq!(fake.takes("shop"), Some((true, -1)), "it takes connections again");
        assert!(fake.restores().is_empty());
        assert!(fake.has_database("shop") && !fake.has_database("shop_before_20261116_0200"));
    }

    #[test]
    fn a_restore_that_fails_removes_the_new_database_and_says_what_pg_restore_said() {
        let (fake, copy) = shop();
        let mut fake = fake.failing_restore(
            "pg_restore: error: could not execute query: ERROR:  permission denied\nCommand was: x",
        );
        let failure =
            run_it(&mut fake, &ask(&copy, Target::New(Some("shop_old".into())))).unwrap_err();
        let lines: Vec<&str> = failure.message.lines().collect();
        assert_eq!(lines.len(), 3, "{}", failure.message);
        assert!(lines[0].starts_with("pg_restore: error: could not execute query"));
        assert_eq!(
            lines[2],
            "the copy was not put into shop_old: the database shop_old was removed again (DB-110)"
        );
        assert!(!fake.has_database("shop_old"));
        assert!(fake.has_database("shop"));
        assert_eq!(changes(&fake).last().map(String::as_str), Some("DROP DATABASE \"shop_old\""));
        assert_eq!(failure.code, 1);
    }

    #[test]
    fn a_restore_over_that_fails_removes_the_new_database_and_renames_the_old_one_back() {
        let (fake, copy) = shop();
        let mut fake = fake.failing_restore("pg_restore: error: bad");
        let failure = run_it(&mut fake, &ask(&copy, Target::Over { force: false })).unwrap_err();
        assert_eq!(
            failure.message,
            "pg_restore: error: bad\nthe copy was not put into shop: the database shop was \
             removed again, shop_before_20261116_0200 is shop again (DB-110)"
        );
        let changes = changes(&fake);
        assert_eq!(
            &changes[changes.len() - 2..],
            [
                "DROP DATABASE \"shop\"",
                "ALTER DATABASE \"shop_before_20261116_0200\" RENAME TO \"shop\""
            ]
        );
        assert!(fake.has_database("shop") && !fake.has_database("shop_before_20261116_0200"));
        // The database that was is the one that is there, with its comment.
        assert!(fake.comment_of("shop").is_some());
        assert_eq!(fake.takes("shop"), Some((true, -1)));
    }

    #[test]
    fn a_forced_restore_that_fails_leaves_the_old_database_open_under_its_own_name() {
        let (fake, copy) = shop();
        let mut fake =
            fake.with_sessions("shop", "php", 1).failing_restore("pg_restore: error: bad");
        let failure = run_it(&mut fake, &ask(&copy, Target::Over { force: true })).unwrap_err();
        assert!(failure.message.ends_with("(DB-110)"), "{}", failure.message);
        assert!(fake.has_database("shop") && !fake.has_database("shop_before_20261116_0200"));
        assert_eq!(fake.takes("shop"), Some((true, -1)));
    }

    #[test]
    fn what_cannot_be_taken_back_is_said_and_what_there_is_to_keep_is_kept() {
        let (fake, copy) = shop();
        let mut fake = fake.failing_restore("pg_restore: error: bad").refusing(
            "DROP DATABASE",
            "55006",
            "database is being accessed by other users",
        );
        let failure = run_it(&mut fake, &ask(&copy, Target::Over { force: false })).unwrap_err();
        assert!(failure.message.ends_with("(DB-110)"), "{}", failure.message);
        let last = failure.message.lines().last().unwrap();
        assert!(last.contains("the database shop stays (PostgreSQL said: "), "{last}");
        assert!(last.contains("shop as it was stays as shop_before_20261116_0200"), "{last}");
        // Nothing was renamed back over a database that stays.
        assert!(fake.has_database("shop") && fake.has_database("shop_before_20261116_0200"));
    }

    #[test]
    fn a_statement_that_is_refused_after_the_database_is_made_takes_it_back_too() {
        let (fake, copy) = shop();
        let mut fake = fake.refusing("REVOKE ", "42501", "permission denied");
        let failure =
            run_it(&mut fake, &ask(&copy, Target::New(Some("shop_old".into())))).unwrap_err();
        assert!(
            failure
                .message
                .contains("permission denied (42501); the database shop_old was removed again"),
            "{}",
            failure.message
        );
        assert!(!fake.has_database("shop_old"));
        assert!(fake.restores().is_empty());
        // Over: and the old one is put back by its name.
        let (fake, copy) = shop();
        let mut fake = fake.refusing("REVOKE ", "42501", "permission denied");
        let failure = run_it(&mut fake, &ask(&copy, Target::Over { force: false })).unwrap_err();
        assert!(
            failure.message.contains("shop_before_20261116_0200 is shop again"),
            "{}",
            failure.message
        );
        assert!(fake.has_database("shop") && !fake.has_database("shop_before_20261116_0200"));
        assert!(fake.comment_of("shop").is_some());
    }

    #[test]
    fn a_database_that_a_role_with_every_right_owns_is_not_put_back() {
        let mut fake =
            Fake::new().with_database("shop", "postgres").with_copy("shop", 100, START_MS);
        let copy = fake.copies_folder().join("shop.dump").to_string_lossy().into_owned();
        let failure = run_it(&mut fake, &ask(&copy, Target::New(None))).unwrap_err();
        assert!(
            failure.message.starts_with("shop is owned by postgres, a role with every right"),
            "{}",
            failure.message
        );
        assert!(changes(&fake).is_empty());
        assert!(fake.restores().is_empty());
    }

    #[test]
    fn a_role_called_root_that_is_there_already_stops_the_restore_before_anything_is_made() {
        let (fake, copy) = shop();
        let mut fake = fake.with_role("root");
        let failure = run_it(&mut fake, &ask(&copy, Target::Over { force: true })).unwrap_err();
        assert!(
            failure.message.starts_with("there is a role called root already"),
            "{}",
            failure.message
        );
        assert!(failure.message.contains("DROP ROLE root"), "{}", failure.message);
        assert!(changes(&fake).is_empty());
        assert!(fake.restores().is_empty());
        // The role that was there is as it was.
        assert_eq!(fake.role_of("root"), Some((true, None)));
    }

    #[test]
    fn a_role_that_cannot_be_removed_after_the_restore_signs_in_no_more_and_is_logged() {
        let (fake, copy) = shop();
        let mut fake = fake.refusing(
            "DROP ROLE",
            "2BP01",
            "role \"root\" cannot be dropped because some objects depend on it",
        );
        let restored =
            run_it(&mut fake, &ask(&copy, Target::New(Some("shop_old".into())))).unwrap();
        assert_eq!(restored.restored_as, "shop_old");
        let changes = changes(&fake);
        assert_eq!(
            &changes[changes.len() - 2..],
            ["ALTER ROLE \"root\" NOLOGIN", "REVOKE \"shop\" FROM \"root\""]
        );
        assert_eq!(fake.role_of("root"), Some((false, None)));
        assert_eq!(fake.connects("shop_old"), Some((true, vec![])));
        let logged = fake.logged().join("\n");
        assert!(
            logged.contains(
                "restoring shop: the role root, which put the copy back, stays and \
                 signs in no more (PostgreSQL said: "
            ),
            "{logged}"
        );
    }

    #[test]
    fn a_failure_while_the_role_is_made_takes_the_role_and_the_database_back() {
        let (fake, copy) = shop();
        let mut fake = fake.refusing("GRANT \"shop\" TO", "42501", "permission denied");
        let failure =
            run_it(&mut fake, &ask(&copy, Target::New(Some("shop_old".into())))).unwrap_err();
        assert!(
            failure
                .message
                .contains("permission denied (42501); the database shop_old was removed again"),
            "{}",
            failure.message
        );
        assert!(!fake.has_role("root"));
        assert!(!fake.has_database("shop_old"));
        assert!(fake.restores().is_empty());
        // A role of that name that a rival made meanwhile is not removed.
        let (fake, copy) = shop();
        let mut fake = fake.refusing("CREATE ROLE", "42710", "role \"root\" already exists");
        let failure = run_it(&mut fake, &ask(&copy, Target::Over { force: false })).unwrap_err();
        assert!(failure.message.contains("(DB-102)"), "{}", failure.message);
        assert!(!changes(&fake).iter().any(|change| change.starts_with("DROP ROLE")));
        assert!(fake.has_database("shop") && !fake.has_database("shop_before_20261116_0200"));
    }

    #[test]
    fn a_failed_restore_takes_the_role_away_before_the_database() {
        let (fake, copy) = shop();
        let mut fake = fake.failing_restore("pg_restore: error: bad");
        run_it(&mut fake, &ask(&copy, Target::Over { force: false })).unwrap_err();
        let changes = changes(&fake);
        let at = |what: &str| changes.iter().position(|change| change.starts_with(what)).unwrap();
        assert!(at("REVOKE CONNECT ON DATABASE \"shop\" FROM \"root\"") < at("DROP ROLE"));
        assert!(at("DROP ROLE") < at("DROP DATABASE"));
        assert!(!fake.has_role("root"));
        assert!(!fake.restores()[0].argv.contains(&"postgres".to_string()));
    }

    #[test]
    fn a_new_name_that_a_rival_took_meanwhile_is_db_102() {
        let (fake, copy) = shop();
        let mut fake = fake.refusing("CREATE DATABASE", "42P04", "database already exists");
        let failure =
            run_it(&mut fake, &ask(&copy, Target::New(Some("shop_old".into())))).unwrap_err();
        assert!(failure.message.ends_with("(DB-102)"), "{}", failure.message);
    }

    #[test]
    fn the_steps_are_told_one_by_one_for_each_way() {
        assert_eq!(steps_of(&Target::New(None)), NEW_STEPS);
        assert_eq!(steps_of(&Target::Over { force: true }), OVER_STEPS);
        assert_eq!((NEW_STEPS, OVER_STEPS), (3, 5));
    }

    #[test]
    fn the_document_has_the_members_the_pages_read() {
        let (fake, copy) = shop();
        let mut fake = fake.at((2026, 11, 16, 2, 0), START_MS, 1500);
        let restored = run_it(&mut fake, &ask(&copy, Target::New(None))).unwrap();
        assert_eq!(restored.took_ms, 1500);
        let line = json::line(&restored).unwrap();
        let golden = include_str!("../../tests/json/database-restore.json").replace("COPY", &copy);
        json::assert_golden(&line, &golden);
        let over = run_it(&mut fake, &ask(&copy, Target::Over { force: false })).unwrap();
        let line = json::line(&over).unwrap();
        let golden =
            include_str!("../../tests/json/database-restore-over.json").replace("COPY", &copy);
        json::assert_golden(&line, &golden);
    }

    #[test]
    fn what_pg_restore_says_is_never_a_password_and_the_error_has_one_id() {
        // The words that come first are pg_restore's; the ID is on the last line only.
        let failure = errors::not_put_back("x", "a\nb", "back");
        assert_eq!(failure.message.matches("(DB-110)").count(), 1);
        assert!(failure.message.ends_with("(DB-110)"));
    }
}

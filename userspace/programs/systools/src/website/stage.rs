// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The new set of sites, put where nginx can check it before it is live
//! (ADR 0569), and the lock that lets one change at a time run.
//!
//! nginx's check runs as the account nginx, in nginx's own view, which
//! holds its data folder and not `/system/state`. So the set is staged in
//! nginx's `conf` folder: the files in `.website-next/`, and a copy of
//! `nginx.conf` that includes that folder where the original includes
//! the sites' folder, beside it, because nginx finds the other files its
//! settings name from the folder of the file it reads.
//!
//! The `conf` folder is nginx's: the account that runs the web server
//! may put anything in it, a link among it. So it is held open and every
//! name is looked up beneath it without following a link, the staged
//! files are made new and root's, and they are removed whatever happens,
//! when [`Staged`] is dropped.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::fd::AsFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use super::conf::FILE_MAX;
use super::host::{Paths, STATE};
use super::plan::{LOGS, Main, NGINX_HOME, SETTINGS};
use crate::bundles::folder::Dir;
use crate::util::strip_os_error;

/// nginx's settings folder, in nginx's view.
pub const CONF: &str = "/shared/services/org.nginx.nginx/nginx/conf";

/// The folder of the staged files, in [`CONF`].
const NEXT: &str = ".website-next";

/// The copy of `nginx.conf` that includes the staged files, in [`CONF`].
const CHECK: &str = ".website-check.conf";

/// The most bytes of a site's file that are staged.
const SITE_MAX: u64 = 1024 * 1024;

/// How long a change waits for another that runs.
const LOCK_WAIT: Duration = Duration::from_secs(60);

/// How often the lock is tried while it is taken.
const LOCK_POLL: Duration = Duration::from_millis(100);

/// The path of `nginx.conf` in nginx's view.
pub fn conf_view() -> String {
    format!("{CONF}/nginx.conf")
}

/// Reads nginx's `nginx.conf`, not through a link: none when nginx has
/// never started and so has no settings of its own yet.
///
/// # Errors
///
/// When it cannot be read or holds more than the reader takes.
pub fn read_conf(paths: &Paths) -> Result<Option<String>, String> {
    let words = |error: io::Error| format!("{}: {}", conf_view(), strip_os_error(&error));
    let dir = match Dir::open(&paths.text(CONF)) {
        Ok(dir) => dir,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(words(error)),
    };
    match dir.read("nginx.conf", FILE_MAX as u64) {
        Ok((bytes, _)) => {
            String::from_utf8(bytes).map(Some).map_err(|_| format!("{} is not text", conf_view()))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(words(error)),
    }
}

/// Gives the descriptor `fd` to the owner and group `uid` and `gid`.
fn chown(fd: impl AsFd, uid: libc::uid_t, gid: libc::gid_t) -> io::Result<()> {
    std::os::unix::fs::fchown(fd, Some(uid), Some(gid))
}

/// Opens the folder of the sites' logs, `logs/sites` in nginx's data
/// folder, one part at a time and not through a link. When a part is not
/// there it is made if `make` says so, for the owner of its folder, which
/// is nginx, so that nginx can write what it logs there; else none.
///
/// # Errors
///
/// When a part is a link or not a folder, or cannot be opened or made.
pub fn logs_dir(paths: &Paths, make: bool) -> io::Result<Option<Dir>> {
    let mut at = match Dir::open(&paths.text(NGINX_HOME)) {
        Ok(home) => home,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    for name in ["logs", "sites"] {
        let inner = match at.dir_at(name) {
            Ok(inner) => inner,
            Err(error) if make && error.kind() == io::ErrorKind::NotFound => {
                let owner = at.stat()?;
                at.make(name, 0o755)?;
                let inner = at.dir_at(name)?;
                chown(&inner, owner.st_uid, owner.st_gid)?;
                inner
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        at = inner;
    }
    Ok(Some(at))
}

/// A file of the settings folder, whole.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Raw {
    /// Its name, with `.conf`.
    pub file: String,
    /// What it holds.
    pub bytes: Vec<u8>,
}

/// The `*.conf` files of the sites' settings folder, by name: those nginx
/// includes.
///
/// # Errors
///
/// When the folder or a file in it cannot be read, or one is too large to
/// stage.
pub fn read_settings(paths: &Paths) -> Result<Vec<Raw>, String> {
    let folder = paths.real(SETTINGS);
    let entries = match fs::read_dir(&folder) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("{SETTINGS}: {}", strip_os_error(&error))),
    };
    let mut files = Vec::new();
    for entry in entries.flatten() {
        let Ok(file) = entry.file_name().into_string() else { continue };
        // nginx's glob `*.conf` leaves out names that start with a dot.
        if Path::new(&file).extension().is_none_or(|ext| ext != "conf") || file.starts_with('.') {
            continue;
        }
        let path = entry.path();
        let words = |error: io::Error| format!("{SETTINGS}/{file}: {}", strip_os_error(&error));
        let kind = entry.file_type().map_err(words)?;
        if !kind.is_file() && !kind.is_symlink() {
            continue;
        }
        let bytes = fs::read(&path).map_err(words)?;
        if bytes.len() as u64 > SITE_MAX {
            return Err(format!("{SETTINGS}/{file} is over {} KiB", SITE_MAX / 1024));
        }
        files.push(Raw { file, bytes });
    }
    files.sort_by(|a, b| a.file.cmp(&b.file));
    Ok(files)
}

/// Changes the mode of what `fd` holds, whatever the mask says.
fn chmod(fd: impl AsFd, mode: libc::mode_t) -> io::Result<()> {
    sysapi::fs::set_mode(fd, mode)
}

/// Makes the file `name` in `dir`, new, mode 0644, holding `bytes`.
fn put(dir: &Dir, name: &str, bytes: &[u8]) -> io::Result<()> {
    let fd = dir.create_new(name, 0o644)?;
    chmod(&fd, 0o644)?;
    File::from(fd).write_all(bytes)
}

/// The new set of sites in nginx's `conf` folder, until it is dropped.
pub struct Staged {
    conf: Dir,
}

impl Staged {
    /// Stages `files`, and the copy of `nginx.conf`, whose text is `text`
    /// and which `main` was read from, that includes them.
    ///
    /// # Errors
    ///
    /// When the folder is not nginx's `conf` folder as it should be, or a
    /// file cannot be made.
    pub fn make(paths: &Paths, files: &[Raw], text: &str, main: &Main) -> Result<Staged, String> {
        let words = |error: io::Error| format!("{CONF}: {}", strip_os_error(&error));
        let conf = Dir::open(&paths.text(CONF)).map_err(words)?;
        // nginx's check opens the file of each site's log, which needs the
        // folder it is in.
        logs_dir(paths, true).map_err(|error| format!("{LOGS}: {}", strip_os_error(&error)))?;
        // Whatever an earlier change that was cut short left.
        let staged = Staged { conf };
        staged.remove_all();
        staged.conf.make(NEXT, 0o755).map_err(words)?;
        let next = staged.conf.dir_at(NEXT).map_err(words)?;
        chmod(&next, 0o755).map_err(words)?;
        for file in files {
            put(&next, &file.file, &file.bytes).map_err(words)?;
        }
        let copy = main.with_include(text, &format!("{CONF}/{NEXT}/*.conf"))?;
        put(&staged.conf, CHECK, copy.as_bytes()).map_err(words)?;
        Ok(staged)
    }

    /// The copy of `nginx.conf` that nginx checks, in nginx's view.
    // A method, so that it reads `staged.conf()`: the copy is there to name.
    #[allow(clippy::unused_self)]
    pub fn conf(&self) -> String {
        format!("{CONF}/{CHECK}")
    }

    /// Removes the staged files and the copy.
    fn remove_all(&self) {
        let _ = self.conf.remove_tree(NEXT, 0);
        let _ = self.conf.unlink(CHECK);
    }
}

impl Drop for Staged {
    fn drop(&mut self) {
        self.remove_all();
    }
}

/// `line`, a line of nginx's check about the staged set, with the staged
/// paths shown as the files they will be.
pub fn as_live(line: &str) -> String {
    line.replace(&format!("{CONF}/{NEXT}/"), &format!("{SETTINGS}/"))
        .replace(&format!("{CONF}/{CHECK}"), &conf_view())
}

/// The lock on the websites, held while this lives: one change runs at a
/// time.
pub struct Lock {
    _file: File,
}

impl Lock {
    /// Takes the lock, waiting for whoever holds it.
    ///
    /// # Errors
    ///
    /// When it stays taken for a minute, or its file cannot be made.
    pub fn take(paths: &Paths) -> Result<Lock, String> {
        Lock::take_for(paths, LOCK_WAIT)
    }

    /// [`Lock::take`], waiting `wait` at most.
    fn take_for(paths: &Paths, wait: Duration) -> Result<Lock, String> {
        let folder = paths.real(STATE);
        let words = |error: io::Error| format!("{STATE}/lock: {}", strip_os_error(&error));
        fs::create_dir_all(&folder).map_err(words)?;
        let file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(folder.join("lock"))
            .map_err(words)?;
        let until = Instant::now() + wait;
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(Lock { _file: file }),
                Err(fs::TryLockError::WouldBlock) if Instant::now() < until => {
                    thread::sleep(LOCK_POLL);
                }
                Err(fs::TryLockError::WouldBlock) => {
                    return Err("another change of the websites is running: try again in a \
                                minute"
                        .to_string());
                }
                Err(fs::TryLockError::Error(error)) => return Err(words(error)),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::{PermissionsExt, symlink};

    use super::super::plan::read_main;
    use super::*;

    struct Scratch(std::path::PathBuf);

    impl Scratch {
        fn new(tag: &str) -> Scratch {
            let path =
                std::env::temp_dir().join(format!("website-stage-{tag}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(path.join(CONF.trim_start_matches('/'))).unwrap();
            fs::create_dir_all(path.join(SETTINGS.trim_start_matches('/'))).unwrap();
            Scratch(path)
        }

        fn paths(&self) -> Paths {
            Paths::under(&self.0)
        }

        fn at(&self, view: &str) -> std::path::PathBuf {
            self.0.join(view.trim_start_matches('/'))
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    const CONF_TEXT: &str =
        "http {\n    include mime.types;\n    include /system/settings/websites/*.conf;\n}\n";

    fn raw(file: &str, text: &str) -> Raw {
        Raw { file: file.into(), bytes: text.as_bytes().to_vec() }
    }

    #[test]
    fn the_set_and_the_copy_are_staged_in_nginx_s_conf_folder_and_gone_when_dropped() {
        let tree = Scratch::new("stage");
        let main = read_main(CONF_TEXT).unwrap();
        let files = [raw("a.conf", "server { }\n"), raw("_default.conf", "server { }\n")];
        {
            let staged = Staged::make(&tree.paths(), &files, CONF_TEXT, &main).unwrap();
            assert_eq!(
                staged.conf(),
                "/shared/services/org.nginx.nginx/nginx/conf/.website-check.conf"
            );
            let next = tree.at(CONF).join(NEXT);
            assert_eq!(fs::metadata(&next).unwrap().permissions().mode() & 0o777, 0o755);
            assert_eq!(fs::read_to_string(next.join("a.conf")).unwrap(), "server { }\n");
            assert_eq!(
                fs::metadata(next.join("a.conf")).unwrap().permissions().mode() & 0o777,
                0o644
            );
            let copy = fs::read_to_string(tree.at(CONF).join(CHECK)).unwrap();
            assert_eq!(
                copy,
                "http {\n    include mime.types;\n    include /shared/services/org.nginx.nginx/nginx/conf/.website-next/*.conf;\n}\n"
            );
            assert_eq!(
                fs::metadata(tree.at(CONF).join(CHECK)).unwrap().permissions().mode() & 0o777,
                0o644
            );
        }
        assert!(!tree.at(CONF).join(NEXT).exists() && !tree.at(CONF).join(CHECK).exists());
    }

    #[test]
    fn an_old_staging_is_replaced_and_a_link_nginx_planted_is_not_followed() {
        let tree = Scratch::new("planted");
        let main = read_main(CONF_TEXT).unwrap();
        let secret = tree.at("/secret");
        fs::create_dir_all(&secret).unwrap();
        fs::write(secret.join("keep.conf"), "mine").unwrap();
        // The staging folder is a link to a folder of root's, and the copy
        // a link to a file of root's.
        symlink(&secret, tree.at(CONF).join(NEXT)).unwrap();
        fs::write(tree.at("/target"), "root's own").unwrap();
        symlink(tree.at("/target"), tree.at(CONF).join(CHECK)).unwrap();
        let staged =
            Staged::make(&tree.paths(), &[raw("a.conf", "server { }\n")], CONF_TEXT, &main)
                .unwrap();
        assert_eq!(fs::read_to_string(secret.join("keep.conf")).unwrap(), "mine");
        assert!(!secret.join("a.conf").exists(), "nothing was staged through the link");
        assert_eq!(fs::read_to_string(tree.at("/target")).unwrap(), "root's own");
        assert!(tree.at(CONF).join(NEXT).join("a.conf").is_file());
        drop(staged);
        assert!(secret.join("keep.conf").exists(), "what the link led to is not removed");
        assert!(!tree.at(CONF).join(NEXT).exists());
        // The conf folder itself is a link.
        let tree = Scratch::new("planted2");
        let moved = tree.at("/moved");
        fs::rename(tree.at(CONF), &moved).unwrap();
        symlink(&moved, tree.at(CONF)).unwrap();
        assert!(Staged::make(&tree.paths(), &[], CONF_TEXT, &main).is_err());
        assert!(!moved.join(NEXT).exists());
    }

    #[test]
    fn a_copy_cannot_be_made_of_settings_that_do_not_include_the_sites() {
        let tree = Scratch::new("noinclude");
        let main = read_main("http { }").unwrap();
        let why = Staged::make(&tree.paths(), &[], "http { }", &main).err().unwrap();
        assert!(why.contains("does not include"), "{why}");
        assert!(!tree.at(CONF).join(NEXT).exists(), "what was made is gone");
    }

    #[test]
    fn the_folder_of_the_sites_logs_is_made_for_nginx_in_its_data_folder_and_never_through_a_link()
    {
        use std::os::unix::fs::MetadataExt;
        let tree = Scratch::new("logs");
        assert!(logs_dir(&tree.paths(), false).unwrap().is_none());
        assert!(!tree.at(LOGS).exists());
        let made = logs_dir(&tree.paths(), true).unwrap().expect("it is made");
        let home = fs::metadata(tree.at(NGINX_HOME)).unwrap();
        let own = fs::metadata(tree.at(LOGS)).unwrap();
        assert!(own.is_dir() && (own.uid(), own.gid()) == (home.uid(), home.gid()));
        assert_eq!(own.permissions().mode() & 0o777, 0o755);
        assert_eq!(made.stat().unwrap().st_ino, own.ino());
        // Made again, it is the same folder, with what is in it.
        fs::write(tree.at(LOGS).join("a.log"), "x").unwrap();
        logs_dir(&tree.paths(), true).unwrap().expect("it is there");
        assert!(tree.at(LOGS).join("a.log").is_file());
        assert!(logs_dir(&tree.paths(), false).unwrap().is_some());

        // nginx's account may make any part a link, which is refused, made or not.
        for part in ["logs", "logs/sites"] {
            let tree = Scratch::new("logs-link");
            logs_dir(&tree.paths(), true).unwrap();
            let place = tree.at(NGINX_HOME).join(part);
            fs::remove_dir_all(&place).unwrap();
            fs::create_dir_all(tree.at("/elsewhere")).unwrap();
            symlink(tree.at("/elsewhere"), &place).unwrap();
            assert!(logs_dir(&tree.paths(), false).is_err(), "{part}");
            assert!(logs_dir(&tree.paths(), true).is_err(), "{part}");
            assert!(fs::read_dir(tree.at("/elsewhere")).unwrap().next().is_none());
        }
        let none = Scratch::new("logs-none");
        fs::remove_dir_all(none.at(NGINX_HOME)).unwrap();
        assert!(logs_dir(&none.paths(), true).unwrap().is_none());
    }

    #[test]
    fn staging_makes_the_folder_nginx_s_check_needs_for_the_sites_logs() {
        let tree = Scratch::new("logs-stage");
        let main = read_main(CONF_TEXT).unwrap();
        assert!(!tree.at(LOGS).exists());
        let _staged = Staged::make(&tree.paths(), &[], CONF_TEXT, &main).unwrap();
        assert!(tree.at(LOGS).is_dir());
    }

    #[test]
    fn nginx_s_line_names_the_files_the_sites_will_be() {
        let line = "nginx: [emerg] unknown directive \"x\" in /shared/services/org.nginx.nginx/nginx/conf/.website-next/a.example.org.conf:3";
        assert_eq!(
            as_live(line),
            "nginx: [emerg] unknown directive \"x\" in /system/settings/websites/a.example.org.conf:3"
        );
        let line = "nginx: [emerg] duplicate in /shared/services/org.nginx.nginx/nginx/conf/.website-check.conf:9";
        assert_eq!(
            as_live(line),
            "nginx: [emerg] duplicate in /shared/services/org.nginx.nginx/nginx/conf/nginx.conf:9"
        );
    }

    #[test]
    fn nginx_conf_is_read_without_following_a_link_and_none_means_it_never_started() {
        let tree = Scratch::new("read");
        assert_eq!(read_conf(&tree.paths()).unwrap(), None);
        fs::write(tree.at(CONF).join("nginx.conf"), CONF_TEXT).unwrap();
        assert_eq!(read_conf(&tree.paths()).unwrap().as_deref(), Some(CONF_TEXT));
        fs::remove_file(tree.at(CONF).join("nginx.conf")).unwrap();
        fs::write(tree.at("/etc-shadow"), "root:x").unwrap();
        symlink(tree.at("/etc-shadow"), tree.at(CONF).join("nginx.conf")).unwrap();
        assert!(read_conf(&tree.paths()).is_err());
        fs::remove_file(tree.at(CONF).join("nginx.conf")).unwrap();
        fs::write(tree.at(CONF).join("nginx.conf"), "x".repeat(FILE_MAX + 1)).unwrap();
        assert!(read_conf(&tree.paths()).unwrap_err().contains("too large"));
        let none = Scratch::new("read2");
        fs::remove_dir_all(none.at(CONF)).unwrap();
        assert_eq!(read_conf(&none.paths()).unwrap(), None);
    }

    #[test]
    fn the_settings_files_are_the_conf_files_nginx_includes_by_name() {
        let tree = Scratch::new("settings");
        let folder = tree.at(SETTINGS);
        fs::write(folder.join("b.example.org.conf"), "b").unwrap();
        fs::write(folder.join("a.example.org.conf"), "a").unwrap();
        fs::write(folder.join("_default.conf"), "d").unwrap();
        fs::write(folder.join("notes.txt"), "x").unwrap();
        fs::write(folder.join(".hidden.conf"), "x").unwrap();
        fs::create_dir(folder.join("dir.conf")).unwrap();
        let files = read_settings(&tree.paths()).unwrap();
        let names: Vec<&str> = files.iter().map(|f| f.file.as_str()).collect();
        assert_eq!(names, ["_default.conf", "a.example.org.conf", "b.example.org.conf"]);
        fs::write(folder.join("big.conf"), vec![b'x'; 1024 * 1024 + 1]).unwrap();
        assert!(read_settings(&tree.paths()).unwrap_err().contains("big.conf is over 1024 KiB"));
        fs::remove_dir_all(&folder).unwrap();
        assert!(read_settings(&tree.paths()).unwrap().is_empty());
    }

    #[test]
    fn one_change_runs_at_a_time_and_a_second_waits_then_says_so() {
        let tree = Scratch::new("lock");
        let first = Lock::take(&tree.paths()).unwrap();
        let started = Instant::now();
        let why = Lock::take_for(&tree.paths(), Duration::from_millis(300)).err().unwrap();
        assert!(started.elapsed() >= Duration::from_millis(300));
        assert!(why.contains("another change of the websites is running"), "{why}");
        drop(first);
        assert!(Lock::take_for(&tree.paths(), Duration::ZERO).is_ok());
        assert_eq!(
            fs::metadata(tree.at(STATE).join("lock")).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}

// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Where backups are kept (ADR 0463): a folder, as on a USB disk, or a
//! folder on a server reached with OpenSSH's client. Either holds only
//! files, under `dukovo-backup/<machine>`; the server runs `mkdir`,
//! `cat`, `mv`, `rm` and `find` alone, so any POSIX server does.
//!
//! A backup's stream is written as parts of at most [`PART`] bytes, each
//! `.partial` until the whole stream is written; then its tag (ADR 0464)
//! and they are renamed, the tag first and the parts from the last to the
//! first, so that a backup is there once its first part, or its one file,
//! has its name, and has its tag by then.
//!
//! A restic repository (ADR 0465) is kept in the same folder, through
//! the same stores, and S3-compatible storage's ([`super::s3`]).

use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use std::cell::Cell;
use std::rc::Rc;

use super::layout::{self, PARTIAL};
use super::tag::Tagging;
use crate::util::{self, Fail};

/// The size of a part: FAT holds no file of 4 GiB.
pub const PART: u64 = 1 << 30;

/// The folder backups go in, at a destination.
pub const FOLDER: &str = "dukovo-backup";

/// OpenSSH's client.
const SSH: &str = "/system/commands/ssh";

/// A destination's files.
pub trait Store: Send {
    /// Where they are, for messages.
    fn place(&self) -> String;
    /// Every file's path under the machine's folder; none if it is not
    /// there yet.
    fn list(&mut self) -> Result<Vec<String>, Fail>;
    /// Every file's path and size under the folder `folder` of the
    /// machine's, at any depth; none if it is not there.
    fn list_sizes(&mut self, folder: &str) -> Result<Vec<(String, u64)>, Fail>;
    /// The size of the file `path`, if it is there.
    fn size(&mut self, path: &str) -> Result<Option<u64>, Fail>;
    /// Reads the file `path`.
    fn read(&mut self, path: &str) -> Result<Box<dyn Read>, Fail>;
    /// Reads `length` bytes of the file `path` from byte `start`; fails
    /// if it has fewer.
    fn read_range(&mut self, path: &str, start: u64, length: u64) -> Result<Vec<u8>, Fail>;
    /// Makes the file `path`, and the folders it is in.
    fn create(&mut self, path: &str) -> Result<Box<dyn Sink>, Fail>;
    /// Writes the file `path` whole: no one sees it until it is, and a
    /// file there before is replaced.
    fn put_whole(&mut self, path: &str, data: &[u8]) -> Result<(), Fail> {
        let partial = format!("{path}{PARTIAL}");
        let mut sink = self.create(&partial)?;
        sink.write_all(data)
            .map_err(|e| util::failed(&format!("{}/{partial}", self.place()), e))?;
        sink.finish()?;
        self.rename_all(&[(partial, path.to_string())])
    }
    /// Makes the folders `folders`, where a store has folders.
    fn make_folders(&mut self, folders: &[String]) -> Result<(), Fail>;
    /// Renames each file of `pairs` from its first path to its second, in
    /// order.
    fn rename_all(&mut self, pairs: &[(String, String)]) -> Result<(), Fail>;
    /// Removes the files and folders `paths`, with what the folders hold.
    fn remove(&mut self, paths: &[String]) -> Result<(), Fail>;
}

/// Reads exactly `length` bytes of `reader`, or says what `at` lacked.
fn read_exactly(reader: impl Read, length: u64, at: &str) -> Result<Vec<u8>, Fail> {
    let mut data = Vec::with_capacity(usize::try_from(length).unwrap_or(0).min(64 << 20));
    reader.take(length).read_to_end(&mut data).map_err(|e| util::failed(&at, e))?;
    if (data.len() as u64) < length {
        return Err(util::fail(format!("{at}: shorter than it should be")));
    }
    Ok(data)
}

/// A file being written.
pub trait Sink: Write {
    /// Writes what is left, and makes it last.
    fn finish(self: Box<Self>) -> Result<(), Fail>;
}

/// A folder of this machine's, as a USB disk's in `/volumes`.
pub struct Folder {
    /// The machine's folder in it.
    root: PathBuf,
}

impl Folder {
    /// The machine `machine`'s folder in the folder `to`, which must be
    /// there: a disk not plugged in is not. With no machine, `to` is the
    /// machine's folder itself.
    pub fn new(to: &Path, machine: Option<&str>) -> Result<Folder, Fail> {
        if !to.is_dir() {
            return Err(util::fail(format!(
                "{}: not there; is the disk plugged in?",
                to.display()
            )));
        }
        let root = match machine {
            Some(machine) => to.join(FOLDER).join(machine),
            None => to.to_path_buf(),
        };
        Ok(Folder { root })
    }
}

/// Makes what `path` names last, as its folder's sync does for a rename.
fn sync_folder(path: &Path) -> Result<(), Fail> {
    let file = File::open(path).map_err(|e| util::failed(&path.display(), e))?;
    file.sync_all().map_err(|e| util::failed(&path.display(), e))
}

struct FolderSink {
    file: File,
    path: PathBuf,
}

impl Write for FolderSink {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.file.write(data)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

impl Sink for FolderSink {
    fn finish(self: Box<Self>) -> Result<(), Fail> {
        self.file.sync_all().map_err(|e| util::failed(&self.path.display(), e))
    }
}

impl Store for Folder {
    fn place(&self) -> String {
        self.root.display().to_string()
    }

    fn list(&mut self) -> Result<Vec<String>, Fail> {
        let mut found = Vec::new();
        let mut folders = vec![(self.root.clone(), String::new(), 0)];
        while let Some((folder, prefix, depth)) = folders.pop() {
            let entries = match fs::read_dir(&folder) {
                Ok(entries) => entries,
                Err(err) if err.kind() == io::ErrorKind::NotFound && depth == 0 => break,
                Err(err) => return Err(util::failed(&folder.display(), err)),
            };
            for entry in entries {
                let entry = entry.map_err(|e| util::failed(&folder.display(), e))?;
                let Some(name) = entry.file_name().to_str().map(String::from) else { continue };
                let kind = entry.file_type().map_err(|e| util::failed(&folder.display(), e))?;
                let path = format!("{prefix}{name}");
                if kind.is_dir() && depth < 2 {
                    folders.push((entry.path(), format!("{path}/"), depth + 1));
                } else if kind.is_file() {
                    found.push(path);
                }
            }
        }
        found.sort();
        Ok(found)
    }

    fn list_sizes(&mut self, folder: &str) -> Result<Vec<(String, u64)>, Fail> {
        let mut found = Vec::new();
        let start = if folder.is_empty() { String::new() } else { format!("{folder}/") };
        let mut folders = vec![(self.root.join(folder), start, true)];
        while let Some((at, prefix, top)) = folders.pop() {
            let entries = match fs::read_dir(&at) {
                Ok(entries) => entries,
                Err(err) if err.kind() == io::ErrorKind::NotFound && top => break,
                Err(err) => return Err(util::failed(&at.display(), err)),
            };
            for entry in entries {
                let entry = entry.map_err(|e| util::failed(&at.display(), e))?;
                let Some(name) = entry.file_name().to_str().map(String::from) else { continue };
                let meta = entry.metadata().map_err(|e| util::failed(&at.display(), e))?;
                let path = format!("{prefix}{name}");
                if meta.is_dir() {
                    folders.push((entry.path(), format!("{path}/"), false));
                } else if meta.is_file() {
                    found.push((path, meta.len()));
                }
            }
        }
        found.sort();
        Ok(found)
    }

    fn size(&mut self, path: &str) -> Result<Option<u64>, Fail> {
        let at = self.root.join(path);
        match fs::metadata(&at) {
            Ok(meta) if meta.is_file() => Ok(Some(meta.len())),
            Ok(_) => Ok(None),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(util::failed(&at.display(), err)),
        }
    }

    fn read(&mut self, path: &str) -> Result<Box<dyn Read>, Fail> {
        let path = self.root.join(path);
        Ok(Box::new(File::open(&path).map_err(|e| util::failed(&path.display(), e))?))
    }

    fn read_range(&mut self, path: &str, start: u64, length: u64) -> Result<Vec<u8>, Fail> {
        use std::io::{Seek, SeekFrom};
        let at = self.root.join(path);
        let mut file = File::open(&at).map_err(|e| util::failed(&at.display(), e))?;
        file.seek(SeekFrom::Start(start)).map_err(|e| util::failed(&at.display(), e))?;
        read_exactly(file, length, &at.display().to_string())
    }

    fn make_folders(&mut self, folders: &[String]) -> Result<(), Fail> {
        for folder in folders {
            let at = self.root.join(folder);
            fs::create_dir_all(&at).map_err(|e| util::failed(&at.display(), e))?;
        }
        Ok(())
    }

    fn create(&mut self, path: &str) -> Result<Box<dyn Sink>, Fail> {
        let path = self.root.join(path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| util::failed(&parent.display(), e))?;
        }
        let file = File::options()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&path)
            .map_err(|e| util::failed(&path.display(), e))?;
        Ok(Box::new(FolderSink { file, path }))
    }

    fn rename_all(&mut self, pairs: &[(String, String)]) -> Result<(), Fail> {
        for (from, to) in pairs {
            let (from, to) = (self.root.join(from), self.root.join(to));
            fs::rename(&from, &to).map_err(|e| util::failed(&from.display(), e))?;
            if let Some(parent) = to.parent() {
                sync_folder(parent)?;
            }
        }
        Ok(())
    }

    fn remove(&mut self, paths: &[String]) -> Result<(), Fail> {
        for path in paths {
            let path = self.root.join(path);
            let removed = match fs::symlink_metadata(&path) {
                Ok(meta) if meta.is_dir() => fs::remove_dir_all(&path),
                Ok(_) => fs::remove_file(&path),
                Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
                Err(err) => Err(err),
            };
            removed.map_err(|e| util::failed(&path.display(), e))?;
        }
        Ok(())
    }
}

/// An address `ssh://user@host[:port]/path`: the path is the server's,
/// from its root, or from the account's home if it starts with `~/`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Address {
    pub user: String,
    pub host: String,
    pub port: u16,
    pub path: String,
}

impl Address {
    /// The address `text` names, if it is an SSH one.
    pub fn parse(text: &str) -> Result<Option<Address>, String> {
        let Some(rest) = text.strip_prefix("ssh://") else { return Ok(None) };
        let bad = |why: &str| format!("{text}: {why}; write ssh://user@host[:port]/path");
        let (authority, path) = rest.split_once('/').ok_or_else(|| bad("no path"))?;
        let (user, host) = authority.split_once('@').ok_or_else(|| bad("no user"))?;
        let (host, port) = match host.strip_prefix('[') {
            // An IPv6 address, in brackets as in a URL.
            Some(v6) => {
                let (inside, after) = v6.split_once(']').ok_or_else(|| bad("no ']'"))?;
                match after.strip_prefix(':') {
                    Some(port) => (inside, port.parse().map_err(|_| bad("not a port"))?),
                    None if after.is_empty() => (inside, 22),
                    None => return Err(bad("text after ']'")),
                }
            }
            None => match host.split_once(':') {
                Some((host, port)) => (host, port.parse().map_err(|_| bad("not a port"))?),
                None => (host, 22),
            },
        };
        let fine = |part: &str| {
            !part.is_empty()
                && !part.starts_with('-')
                && part.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.:".contains(&b))
        };
        if !fine(user) || !fine(host) {
            return Err(bad("a user or host of letters, digits, '-', '_', '.' and ':' alone"));
        }
        let path = match path.strip_prefix("~/") {
            Some(home) => format!("~/{home}"),
            None => format!("/{path}"),
        };
        if path.split('/').any(|name| name == "..") || path.contains('\0') {
            return Err(bad("a path without '..'"));
        }
        Ok(Some(Address { user: user.into(), host: host.into(), port, path }))
    }
}

/// `text` as one word of a POSIX shell's command, quoted.
pub fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

/// A folder on a server, through OpenSSH's client.
pub struct Ssh {
    address: Address,
    /// The machine's folder there, as the server's shell takes it.
    root: String,
    /// The machine's folder there, as people read it.
    shown: String,
    /// The key the machine signs in with.
    key: PathBuf,
    /// The server's host key, as recorded.
    known_hosts: PathBuf,
    /// Whether a host key not yet recorded is taken and recorded, as
    /// `backup add` does on first contact.
    first_contact: bool,
}

impl Ssh {
    /// The machine `machine`'s folder at `address`, or with no machine,
    /// the folder `address` names.
    pub fn new(address: Address, machine: Option<&str>, key: PathBuf, known_hosts: PathBuf) -> Ssh {
        let base = match address.path.strip_prefix("~/") {
            Some(home) => format!("~/{}", quote(home)),
            None => quote(&address.path),
        };
        let (root, shown) = match machine {
            Some(machine) => (
                format!("{base}/{FOLDER}/{}", quote(machine)),
                format!("{}/{FOLDER}/{machine}", address.path),
            ),
            None => (base, address.path.clone()),
        };
        Ssh { address, root, shown, key, known_hosts, first_contact: false }
    }

    /// Takes the server's host key if none is recorded yet.
    pub fn trust_on_first_contact(&mut self) {
        self.first_contact = true;
    }

    /// The path `path` under the machine's folder, quoted for the shell.
    fn at(&self, path: &str) -> String {
        format!("{}/{}", self.root, quote(path))
    }

    /// The client's command that runs `script` on the server.
    fn command(&self, script: &str) -> Command {
        let mut command = Command::new(SSH);
        let checking = if self.first_contact { "accept-new" } else { "yes" };
        command
            .args(["-q", "-T", "-o", "BatchMode=yes", "-o", "IdentitiesOnly=yes"])
            .arg("-o")
            .arg(format!("StrictHostKeyChecking={checking}"))
            .arg("-o")
            .arg(format!("UserKnownHostsFile={}", self.known_hosts.display()))
            .args(["-o", "ServerAliveInterval=30", "-o", "ServerAliveCountMax=4"])
            .arg("-i")
            .arg(&self.key)
            .arg("-p")
            .arg(self.address.port.to_string())
            .arg("--")
            .arg(format!("{}@{}", self.address.user, self.address.host))
            .arg(script)
            .stdin(Stdio::null())
            .stderr(Stdio::piped());
        command
    }

    /// Runs `script` on the server; its output.
    fn run(&self, script: &str) -> Result<Vec<u8>, Fail> {
        let output = self
            .command(script)
            .stdout(Stdio::piped())
            .output()
            .map_err(|e| util::failed(&SSH, e))?;
        if !output.status.success() {
            return Err(self.failure(&output.stderr, output.status.code()));
        }
        Ok(output.stdout)
    }

    /// Why a command on the server failed, from what it or ssh said.
    fn failure(&self, said: &[u8], code: Option<i32>) -> Fail {
        let said = String::from_utf8_lossy(said);
        let said = said.lines().rfind(|l| !l.trim().is_empty()).unwrap_or("").trim();
        let what = match (said.is_empty(), code) {
            (false, _) => said.to_string(),
            (true, Some(255)) => "could not sign in".into(),
            (true, Some(code)) => format!("ended with status {code}"),
            (true, None) => "ended by a signal".into(),
        };
        util::fail(format!("{}: {what}", self.place()))
    }
}

/// What ssh writes to a file on the server.
struct SshSink {
    child: Child,
    input: Option<ChildStdin>,
    place: String,
}

impl Write for SshSink {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.input.as_mut().ok_or(io::ErrorKind::BrokenPipe)?.write(data)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.input.as_mut().map_or(Ok(()), Write::flush)
    }
}

impl Sink for SshSink {
    fn finish(self: Box<Self>) -> Result<(), Fail> {
        let SshSink { child, input, place } = *self;
        drop(input);
        let output = child.wait_with_output().map_err(|e| util::failed(&SSH, e))?;
        if output.status.success() {
            return Ok(());
        }
        let said = String::from_utf8_lossy(&output.stderr);
        let said = said.trim();
        let why = if said.is_empty() { "the server did not take the file" } else { said };
        Err(util::fail(format!("{place}: {why}")))
    }
}

/// What ssh reads from a file on the server: an error at the end if the
/// server's `cat` failed.
struct SshReader {
    child: Child,
    output: ChildStdout,
    place: String,
}

impl Read for SshReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let read = self.output.read(buffer)?;
        if read == 0 && !buffer.is_empty() {
            let mut said = String::new();
            if let Some(mut errors) = self.child.stderr.take() {
                let _ = errors.read_to_string(&mut said);
            }
            let status = self.child.wait()?;
            if !status.success() {
                return Err(io::Error::other(format!("{}: {}", self.place, said.trim())));
            }
        }
        Ok(read)
    }
}

impl Store for Ssh {
    fn place(&self) -> String {
        let Address { user, host, port, .. } = &self.address;
        let host = if host.contains(':') { format!("[{host}]") } else { host.clone() };
        let port = if *port == 22 { String::new() } else { format!(":{port}") };
        let path = &self.shown;
        let path = path.strip_prefix('~').map_or(path.clone(), |home| format!("/~{home}"));
        format!("ssh://{user}@{host}{port}{path}")
    }

    fn list(&mut self) -> Result<Vec<String>, Fail> {
        let script = format!("cd {} 2>/dev/null || exit 0; find . -type f -print", self.root);
        let output = self.run(&script)?;
        let mut found: Vec<String> = BufReader::new(output.as_slice())
            .lines()
            .map_while(Result::ok)
            .map(|line| line.strip_prefix("./").unwrap_or(&line).to_string())
            .collect();
        found.sort();
        Ok(found)
    }

    fn list_sizes(&mut self, folder: &str) -> Result<Vec<(String, u64)>, Fail> {
        // `ls -ln` gives each file's size in its fifth column; the names
        // listed are restic's, without spaces.
        let script = format!(
            "cd {} 2>/dev/null || exit 0; find . -type f -exec ls -ln {{}} +",
            if folder.is_empty() { self.root.clone() } else { self.at(folder) }
        );
        let output = self.run(&script)?;
        let prefix = if folder.is_empty() { String::new() } else { format!("{folder}/") };
        let mut found: Vec<(String, u64)> = BufReader::new(output.as_slice())
            .lines()
            .map_while(Result::ok)
            .filter_map(|line| {
                let fields: Vec<&str> = line.split_whitespace().collect();
                let size = fields.get(4)?.parse().ok()?;
                let name = fields.last()?.strip_prefix("./")?;
                (fields.len() >= 9).then(|| (format!("{prefix}{name}"), size))
            })
            .collect();
        found.sort();
        Ok(found)
    }

    fn size(&mut self, path: &str) -> Result<Option<u64>, Fail> {
        let file = self.at(path);
        let output = self.run(&format!("if [ -f {file} ]; then wc -c < {file}; fi"))?;
        Ok(String::from_utf8_lossy(&output).trim().parse().ok())
    }

    fn read_range(&mut self, path: &str, start: u64, length: u64) -> Result<Vec<u8>, Fail> {
        if length == 0 {
            return Ok(Vec::new());
        }
        // POSIX dd: the first skips `start` bytes, by a seek on a file,
        // and the second reads the range in one read.
        let skip = if start == 0 {
            String::new()
        } else {
            format!("dd bs={start} skip=1 count=0 2>/dev/null; ")
        };
        let script = format!("{{ {skip}dd bs={length} count=1 2>/dev/null; }} < {}", self.at(path));
        let output = self.run(&script)?;
        read_exactly(output.as_slice(), length, &format!("{}: {path}", self.place()))
    }

    fn make_folders(&mut self, folders: &[String]) -> Result<(), Fail> {
        if folders.is_empty() {
            return Ok(());
        }
        let quoted: Vec<String> = folders.iter().map(|f| self.at(f)).collect();
        self.run(&format!("umask 077 && mkdir -p {}", quoted.join(" "))).map(drop)
    }

    fn read(&mut self, path: &str) -> Result<Box<dyn Read>, Fail> {
        let mut child = self
            .command(&format!("cat {}", self.at(path)))
            .stdout(Stdio::piped())
            .spawn()
            .map_err(|e| util::failed(&SSH, e))?;
        let output = child.stdout.take().ok_or_else(|| util::fail("ssh gave no output".into()))?;
        Ok(Box::new(SshReader { child, output, place: format!("{}: {path}", self.place()) }))
    }

    fn create(&mut self, path: &str) -> Result<Box<dyn Sink>, Fail> {
        let folder = Path::new(path).parent().and_then(Path::to_str).unwrap_or("");
        let script =
            format!("umask 077 && mkdir -p {} && cat > {}", self.at(folder), self.at(path));
        let mut child = self
            .command(&script)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .map_err(|e| util::failed(&SSH, e))?;
        let input = child.stdin.take();
        Ok(Box::new(SshSink { child, input, place: format!("{}: {path}", self.place()) }))
    }

    fn rename_all(&mut self, pairs: &[(String, String)]) -> Result<(), Fail> {
        if pairs.is_empty() {
            return Ok(());
        }
        let moves: Vec<String> = pairs
            .iter()
            .map(|(from, to)| format!("mv -f {} {}", self.at(from), self.at(to)))
            .collect();
        self.run(&moves.join(" && ")).map(drop)
    }

    fn remove(&mut self, paths: &[String]) -> Result<(), Fail> {
        if paths.is_empty() {
            return Ok(());
        }
        let quoted: Vec<String> = paths.iter().map(|p| self.at(p)).collect();
        self.run(&format!("rm -rf {}", quoted.join(" "))).map(drop)
    }
}

/// A backup's stream, written as parts of at most `part` bytes under
/// `folder` (the place's and chain's), each `.partial` until
/// [`Parts::finish`], and tagged as it is written.
pub struct Parts<'a> {
    store: &'a mut dyn Store,
    folder: String,
    time: String,
    part: u64,
    current: Option<Box<dyn Sink>>,
    /// How many parts were started.
    count: usize,
    /// What the current part holds.
    filled: u64,
    /// What every part holds.
    pub written: u64,
    /// The tag, made over every byte written.
    tagging: Tagging,
}

impl<'a> Parts<'a> {
    /// The parts of the backup made at `time` in `folder`, tagged with
    /// `tagging`, which [`layout::tagged_path`] of its first file started.
    pub fn new(
        store: &'a mut dyn Store,
        folder: &str,
        time: &str,
        part: u64,
        tagging: Tagging,
    ) -> Parts<'a> {
        Parts {
            store,
            folder: folder.into(),
            time: time.into(),
            part: part.max(1),
            current: None,
            count: 0,
            filled: 0,
            written: 0,
            tagging,
        }
    }

    fn partial(&self, index: usize) -> String {
        format!("{}/{}{PARTIAL}", self.folder, layout::part_name(&self.time, index))
    }

    /// Opens the next part.
    fn next(&mut self) -> io::Result<()> {
        if let Some(sink) = self.current.take() {
            sink.finish().map_err(|fail| io::Error::other(fail.message))?;
        }
        let path = self.partial(self.count);
        let sink = self.store.create(&path).map_err(|fail| io::Error::other(fail.message))?;
        self.current = Some(sink);
        self.count += 1;
        self.filled = 0;
        Ok(())
    }

    /// Ends the last part, and names the parts for what they are, from the
    /// last to the first: one file `<time>.age`, or `<time>.age.000` and
    /// on.
    pub fn finish(mut self) -> Result<u64, Fail> {
        if self.count == 0 {
            self.next().map_err(|e| util::failed(&self.store.place(), e))?;
        }
        if let Some(sink) = self.current.take() {
            sink.finish()?;
        }
        let tag = format!("{}/{}", self.folder, layout::tag_name(&self.time));
        let tag_partial = format!("{tag}{PARTIAL}");
        let mut pairs = vec![(tag_partial.clone(), tag)];
        if self.count == 1 {
            pairs.push((
                self.partial(0),
                format!("{}/{}", self.folder, layout::whole_name(&self.time)),
            ));
        } else {
            pairs.extend((0..self.count).rev().map(|i| {
                (self.partial(i), format!("{}/{}", self.folder, layout::part_name(&self.time, i)))
            }));
        }
        let (store, written) = (self.store, self.written);
        let mut sink = store.create(&tag_partial)?;
        sink.write_all(self.tagging.text().as_bytes())
            .map_err(|e| util::failed(&store.place(), e))?;
        sink.finish()?;
        store.rename_all(&pairs)?;
        Ok(written)
    }
}

impl Write for Parts<'_> {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        if data.is_empty() {
            return Ok(0);
        }
        if self.current.is_none() || self.filled >= self.part {
            self.next()?;
        }
        let room = usize::try_from(self.part - self.filled).unwrap_or(usize::MAX);
        let sink = self.current.as_mut().ok_or(io::ErrorKind::BrokenPipe)?;
        let wrote = sink.write(&data[..data.len().min(room)])?;
        self.tagging.update(&data[..wrote]);
        self.filled += wrote as u64;
        self.written += wrote as u64;
        Ok(wrote)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.current.as_mut().map_or(Ok(()), Write::flush)
    }
}

/// A backup's parts read as one stream, each opened when the one before
/// it ends, and its tag checked at the end.
pub struct Joined<'a> {
    store: &'a mut dyn Store,
    parts: std::vec::IntoIter<String>,
    current: Option<Box<dyn Read>>,
    /// The tag being made over what is read, and the one the backup's tag
    /// file holds; none to read it unchecked.
    check: Option<(Tagging, String)>,
    /// Set once the end was read and the tag matched, or without a check.
    checked: Rc<Cell<bool>>,
}

impl<'a> Joined<'a> {
    /// The stream of `parts`, checked against `check`: the tagging their
    /// path started and the text of their tag's file. Its [`Joined::checked`]
    /// says when the end was read and found to match.
    pub fn new(
        store: &'a mut dyn Store,
        parts: &[String],
        check: Option<(Tagging, String)>,
    ) -> Joined<'a> {
        let checked = Rc::new(Cell::new(check.is_none()));
        Joined { store, parts: Vec::from(parts).into_iter(), current: None, check, checked }
    }

    /// What says that the stream was read to its end, its tag matching.
    pub fn checked(&self) -> Rc<Cell<bool>> {
        Rc::clone(&self.checked)
    }
}

impl Read for Joined<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        loop {
            if self.current.is_none() {
                let Some(path) = self.parts.next() else {
                    if let Some((tagging, tag)) = self.check.take() {
                        if !tagging.matches(&tag) {
                            return Err(io::Error::new(
                                io::ErrorKind::InvalidData,
                                "its tag does not match: not written with this destination's key",
                            ));
                        }
                        self.checked.set(true);
                    }
                    return Ok(0);
                };
                let reader =
                    self.store.read(&path).map_err(|fail| io::Error::other(fail.message))?;
                self.current = Some(reader);
            }
            let reader = self.current.as_mut().ok_or(io::ErrorKind::UnexpectedEof)?;
            let read = reader.read(buffer)?;
            if let Some((tagging, _)) = self.check.as_mut() {
                tagging.update(&buffer[..read]);
            }
            if read > 0 || buffer.is_empty() {
                return Ok(read);
            }
            self.current = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::tag::TagKey;
    use super::*;

    fn folder() -> (tempdir::Dir, Folder) {
        let dir = tempdir::Dir::new();
        let store = Folder::new(dir.path(), Some("machine")).unwrap();
        (dir, store)
    }

    /// A folder of the test's own, removed after it.
    mod tempdir {
        use std::path::{Path, PathBuf};

        pub struct Dir(PathBuf);

        impl Dir {
            pub fn new() -> Dir {
                static COUNT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
                let n = COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let path =
                    std::env::temp_dir().join(format!("backup-store-{}-{n}", std::process::id()));
                std::fs::create_dir_all(&path).unwrap();
                Dir(path)
            }

            pub fn path(&self) -> &Path {
                &self.0
            }
        }

        impl Drop for Dir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
    }

    #[test]
    fn a_disk_not_plugged_in_is_not_there() {
        let fail = Folder::new(Path::new("/volumes/no-such-disk"), Some("m")).err().unwrap();
        assert!(fail.message.contains("plugged in"), "{}", fail.message);
    }

    fn key() -> TagKey {
        TagKey::of(&age::x25519::Identity::generate())
    }

    /// The text of the tag file at `path`.
    fn tag_text(store: &mut Folder, path: &str) -> String {
        let mut text = String::new();
        store.read(path).unwrap().read_to_string(&mut text).unwrap();
        text
    }

    #[test]
    fn parts_are_named_once_whole_and_read_back_as_one() {
        let (_dir, mut store) = folder();
        let key = key();
        let data: Vec<u8> = (0..2500u32).map(|i| (i * 7) as u8).collect();
        let tagging = key.start("users/2026-11-11-030000/2026-11-11-030000");
        let mut parts =
            Parts::new(&mut store, "users/2026-11-11-030000", "2026-11-11-030000", 1000, tagging);
        parts.write_all(&data).unwrap();
        // Nothing has its name until the end.
        assert!(store_list(&mut parts).iter().all(|p| p.ends_with(PARTIAL)));
        assert_eq!(parts.finish().unwrap(), 2500);
        let listed = store.list().unwrap();
        assert_eq!(listed.len(), 4, "{listed:?}");
        let found = layout::Listing::read(listed.iter().map(String::as_str));
        assert!(found.stray.is_empty(), "{:?}", found.stray);
        let backup = &found.chains("users")[0].backups[0];
        assert_eq!(backup.parts.len(), 3);
        let tag = tag_text(&mut store, backup.tag.as_deref().unwrap());
        let check = Some((key.start(&layout::tagged_path(&backup.parts[0])), tag.clone()));
        let mut joined = Joined::new(&mut store, &backup.parts, check);
        let checked = joined.checked();
        assert!(!checked.get());
        let mut back = Vec::new();
        joined.read_to_end(&mut back).unwrap();
        assert_eq!(back, data);
        assert!(checked.get());
        // Another key's tag, or the tag of another time, is refused at the
        // end.
        let check = Some((self::key().start(&layout::tagged_path(&backup.parts[0])), tag.clone()));
        let err = Joined::new(&mut store, &backup.parts, check).read_to_end(&mut back).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        let check = Some((key.start("users/2026-11-11-030000/2026-11-12-030000"), tag));
        assert!(Joined::new(&mut store, &backup.parts, check).read_to_end(&mut back).is_err());
        // Unchecked, it reads all the same.
        back.clear();
        Joined::new(&mut store, &backup.parts, None).read_to_end(&mut back).unwrap();
        assert_eq!(back, data);
    }

    fn store_list(parts: &mut Parts<'_>) -> Vec<String> {
        parts.store.list().unwrap()
    }

    #[test]
    fn a_small_stream_is_one_file() {
        let (_dir, mut store) = folder();
        let tagging = key().start("users/2026-11-11-030000/2026-11-11-030000");
        let mut parts =
            Parts::new(&mut store, "users/2026-11-11-030000", "2026-11-11-030000", PART, tagging);
        parts.write_all(b"stream").unwrap();
        parts.finish().unwrap();
        let mut listed = store.list().unwrap();
        listed.sort();
        assert_eq!(
            listed,
            [
                "users/2026-11-11-030000/2026-11-11-030000.age".to_string(),
                "users/2026-11-11-030000/2026-11-11-030000.tag".to_string()
            ]
        );
        store.remove(&["users".into()]).unwrap();
        assert!(store.list().unwrap().is_empty());
    }

    #[test]
    fn ssh_addresses_are_read_strictly() {
        let address = Address::parse("ssh://backup@nas.local:2222/srv/backups").unwrap().unwrap();
        assert_eq!(
            address,
            Address {
                user: "backup".into(),
                host: "nas.local".into(),
                port: 2222,
                path: "/srv/backups".into()
            }
        );
        let home = Address::parse("ssh://me@10.0.2.2/~/backups").unwrap().unwrap();
        assert_eq!((home.port, home.path.as_str()), (22, "~/backups"));
        let v6 = Address::parse("ssh://me@[fd00::1]:2200/b").unwrap().unwrap();
        assert_eq!((v6.host.as_str(), v6.port), ("fd00::1", 2200));
        assert_eq!(Address::parse("ssh://me@[fd00::1]/b").unwrap().unwrap().port, 22);
        assert_eq!(Address::parse("/volumes/STICK").unwrap(), None);
        for bad in [
            "ssh://host/path",
            "ssh://me@host",
            "ssh://-oProxyCommand=x@host/p",
            "ssh://me@host:port/p",
            "ssh://me@ho st/p",
            "ssh://me@host/a/../b",
        ] {
            assert!(Address::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_server_gets_every_name_quoted() {
        assert_eq!(quote("a b"), "'a b'");
        assert_eq!(quote("it's"), "'it'\\''s'");
        let address = Address::parse("ssh://me@host/~/my backups").unwrap().unwrap();
        let ssh = Ssh::new(address, Some("laptop"), "k".into(), "h".into());
        assert_eq!(ssh.at("users/x.age"), "~/'my backups'/dukovo-backup/'laptop'/'users/x.age'");
    }
}

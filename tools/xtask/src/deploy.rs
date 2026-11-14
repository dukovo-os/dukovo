// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What a deploy sends to the update server, and how (ADRs 0382 and
//! 0467). The server has ssh, sh and tar and nothing more: an Ubuntu one
//! has rsync too, a Dukovo one does not, and one way works for both.
//!
//! Metadata, targets and keys are small and change at every deploy, so
//! they go whole, as one tar stream that replaces the folders the server
//! held: nothing the deploy did not send stays, which is what rsync's
//! `--delete` did. Files and bundles are never changed and never taken
//! away, so the server is asked which names it already holds, and only
//! the others go, again as one stream.
//!
//! Every script runs as `sh -c`, whatever the login shell is: on Dukovo
//! it is Nukesh, which runs `sh -c` in POSIX mode. What the server's
//! tar unpacks is made group-writable and closed to others by a `chmod`
//! after it, as rsync's `--chmod=D2770,F0660` made it, and lands in a
//! folder the deploy's login owns whose group is the one that publishes:
//! the folders the server's setup makes are `setgid`, so what is made in
//! them takes that group.

use std::collections::HashSet;
use std::ffi::OsString;
use std::io::Write as _;
use std::path::Path;
use std::process::{Command, Stdio};

use crate::process::{self, render, step};
use crate::{Context, Ctx, Error, Result, bail};

/// A server `ob` deploys to: how ssh and scp reach it.
#[derive(Debug, Clone)]
pub struct Server {
    dest: String,
    options: Vec<OsString>,
}

impl Server {
    /// The server `$OB_UPDATE_SSH` names, such as `user@updates.dukovo.org`
    /// or a name in ~/.ssh/config, or the update server's own name.
    pub fn from_environment() -> Self {
        let dest = std::env::var("OB_UPDATE_SSH")
            .ok()
            .filter(|dest| !dest.is_empty())
            .unwrap_or_else(|| crate::catalog::HOST.into());
        Self { dest, options: Vec::new() }
    }

    /// The server `dest` names, reached with `options` more for ssh and
    /// scp, each as one of both's `-o` words: the boot tests' machine, a
    /// port of QEMU's away.
    pub fn with_options(dest: &str, options: Vec<OsString>) -> Self {
        Self { dest: dest.into(), options }
    }

    /// Where ssh reaches it, for messages.
    pub fn dest(&self) -> &str {
        &self.dest
    }

    /// `ssh` to the server, running `script` with `sh -c`.
    pub fn ssh(&self, script: &str) -> Command {
        let mut cmd = Command::new("ssh");
        cmd.args(&self.options).args(["--", &self.dest, &format!("sh -c {}", quote(script))]);
        cmd
    }

    /// [`Server::ssh`] with a terminal, for the password that sudo or
    /// elevate asks for.
    pub fn ssh_terminal(&self, script: &str) -> Command {
        let mut cmd = Command::new("ssh");
        cmd.args(&self.options).args(["-t", "--", &self.dest, &format!("sh -c {}", quote(script))]);
        cmd
    }

    /// `scp` of the folder `from` to `to` in the login's home on the
    /// server.
    pub fn scp(&self, from: &Path, to: &str) -> Command {
        let mut cmd = Command::new("scp");
        cmd.args(["-q", "-r", "-p"]).args(&self.options).arg("--").arg(from);
        cmd.arg(format!("{}:{to}", self.dest));
        cmd
    }
}

/// Quotes `word` for the server's shell.
pub fn quote(word: &str) -> String {
    format!("'{}'", word.replace('\'', r"'\''"))
}

/// Whether `name` is a SHA-256 as hex, as the files of a release are
/// named.
pub fn is_sha256_name(name: &str) -> bool {
    name.len() == 64 && name.bytes().all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

/// Whether `name` is a bundle's file, `ID-VERSION.dkv`: letters, digits,
/// dots, dashes and underscores, not starting with a dot.
pub fn is_bundle_name(name: &str) -> bool {
    let Some(stem) = name.strip_suffix(&format!(".{}", dkv::EXTENSION)) else {
        return false;
    };
    !stem.is_empty()
        && !stem.starts_with('.')
        && stem.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
}

/// The names in the server's answer to [`list_script`] that `is_name`
/// accepts. Anything else in it, such as a message of `ls` or the name of
/// a leftover, is ignored: it can only make the deploy send a file again,
/// never skip one.
pub fn held(answer: &str, is_name: fn(&str) -> bool) -> HashSet<&str> {
    answer.lines().map(str::trim_end).filter(|name| is_name(name)).collect()
}

/// The names in `local` that `held` does not hold, in order.
pub fn missing(local: &[String], held: &HashSet<&str>) -> Vec<String> {
    local.iter().filter(|name| !held.contains(name.as_str())).cloned().collect()
}

/// The script that prints the names in `folders` of the server, one a
/// line: a folder that is not there, or that the login may not read, adds
/// none.
pub fn list_script(folders: &[&str]) -> String {
    format!("for folder in {}; do ls -1 -- \"$folder\" 2>/dev/null; done; true", folders.join(" "))
}

/// What every script starts with: names the folders' group and keeps
/// what it makes closed to others, and stops the deploy with the reason
/// if the server was never set up.
fn start(incoming: &str) -> String {
    format!(
        "set -e\numask 007\n[ -d {incoming} ] || {{ echo 'the server has no {incoming}: set \
         it up first with `./ob release server setup` or `./ob catalog server setup`' >&2; \
         exit 1; }}\n"
    )
}

/// The script that unpacks the stream on its standard input into a new
/// folder `unpacked`, which only a whole stream gets as far as: one that
/// is cut leaves the folder the server had as it was.
fn unpack(unpacked: &str) -> String {
    format!(
        "rm -rf {unpacked}\nmkdir {unpacked}\ntar -x --no-same-owner -f - -C {unpacked}\n\
         chmod -R u=rwX,g=rwX,o= {unpacked}\n"
    )
}

/// The script that replaces the folders `names` of `folder` on the
/// server with those of the stream on its standard input, as whole
/// folders: what it did not send is gone.
pub fn replace_script(incoming: &str, folder: &str, names: &[&str]) -> String {
    let fresh = format!("{folder}/.fresh");
    let mut script = start(incoming);
    script += &format!("mkdir -p {folder}\n");
    script += &unpack(&fresh);
    // Every folder is there before the first is replaced, so that a
    // stream that lacks one leaves the server as it was.
    for name in names {
        script += &format!(
            "[ -d {fresh}/{name} ] || {{ echo 'the stream has no {name}' >&2; exit 1; }}\n"
        );
    }
    for name in names {
        script += &format!(
            "rm -rf {folder}/{name}.old\n[ ! -e {folder}/{name} ] || mv {folder}/{name} \
             {folder}/{name}.old\nmv {fresh}/{name} {folder}/{name}\nrm -rf {folder}/{name}.old\n"
        );
    }
    script + &format!("rmdir {fresh}\n")
}

/// The script that adds the files of the stream on its standard input to
/// `into` on the server, whose files stay. A file is there whole or not
/// at all, as the next deploy takes the names it finds for files it need
/// not send.
pub fn add_script(incoming: &str, into: &str) -> String {
    let unpacked = format!("{into}.partial");
    let mut script = start(incoming);
    script += &format!("mkdir -p {into}\n");
    script += &unpack(&unpacked);
    // A folder with nothing in it is replaced by one rename, which is
    // every deploy but one that follows a failed publish.
    script += &format!(
        "rmdir {into} 2>/dev/null || true\nif [ -e {into} ]; then\n\tfor file in {unpacked}/*; \
         do mv -f \"$file\" {into}/; done\n\trmdir {unpacked}\nelse\n\tmv {unpacked} {into}\nfi\n"
    );
    script
}

/// The names of the files in `folder`, sorted.
fn local_names(folder: &Path) -> Result<Vec<String>> {
    let mut names = Vec::new();
    let entries = std::fs::read_dir(folder).context(|| format!("reading {}", folder.display()))?;
    for entry in entries {
        let entry = entry.context(|| format!("reading {}", folder.display()))?;
        if entry.file_type().context(|| format!("reading {}", entry.path().display()))?.is_file() {
            names.push(entry.file_name().to_string_lossy().into_owned());
        }
    }
    names.sort();
    Ok(names)
}

/// Packs `names` of `from` with the local tar and runs `script` on the
/// server with the stream as its input.
fn send(ctx: &Ctx, server: &Server, from: &Path, names: &[String], script: &str) -> Result<()> {
    let mut packer = Command::new("tar");
    packer.args(["-c", "-f", "-", "--null", "-C"]).arg(from).args(["-T", "-"]);
    packer.stdin(Stdio::piped()).stdout(Stdio::piped());
    let mut receiver = server.ssh(script);
    if ctx.verbose {
        eprintln!("+ {} | {}", render(&packer), render(&receiver));
    }
    let mut packing = packer.spawn().map_err(|why| {
        Error::new(format!("cannot run `tar`: {why}"))
            .hint("run `./ob doctor` to see what is missing")
    })?;
    let mut list = packing.stdin.take().ok_or_else(|| Error::new("tar has no input"))?;
    let packed = packing.stdout.take().ok_or_else(|| Error::new("tar has no output"))?;
    // Written beside the stream, so that a long list never waits for a
    // pipe that is full.
    let listed: Vec<u8> = names.iter().flat_map(|name| name.bytes().chain([0])).collect();
    let writer = std::thread::spawn(move || list.write_all(&listed));
    let received = receiver.stdin(Stdio::from(packed)).status();
    let packing = packing.wait();
    // A cut stream ends the list's writer with an error too; ssh's own
    // failure is the one to report.
    let _ = writer.join();
    let received = received.map_err(|why| Error::new(format!("cannot run `ssh`: {why}")))?;
    if !received.success() {
        return Err(Error::new(format!(
            "{} did not take what was sent ({received})",
            server.dest()
        ))
        .hint("the output above says why; add --verbose to see every command"));
    }
    match packing {
        Ok(status) if status.success() => Ok(()),
        _ => bail!("tar could not pack the files of {}", from.display()),
    }
}

/// Sends the folders `names` of `from` whole, and has the server replace
/// the same folders of `folder`, which is in `incoming`, with them.
///
/// # Errors
///
/// If tar cannot pack them or the server does not take them.
pub fn replace(
    ctx: &Ctx,
    server: &Server,
    from: &Path,
    names: &[&str],
    incoming: &str,
    folder: &str,
) -> Result<()> {
    step(&format!("Sending {} to {}", names.join(" and "), server.dest()));
    let packed: Vec<String> = names.iter().map(|name| (*name).to_string()).collect();
    send(ctx, server, from, &packed, &replace_script(incoming, folder, names))
}

/// Sends the files of `from` that the server does not hold yet, into
/// `into` in `incoming`: it is asked what each of `known` and `into`
/// holds, as `is_name` names them, and only the others are sent.
///
/// # Errors
///
/// If `from` cannot be read, the server does not answer, or tar cannot
/// pack the files or the server does not take them.
pub fn add_missing(
    ctx: &Ctx,
    server: &Server,
    from: &Path,
    is_name: fn(&str) -> bool,
    incoming: &str,
    into: &str,
    known: &[&str],
) -> Result<()> {
    let local = local_names(from)?;
    let mut folders = known.to_vec();
    folders.push(into);
    let answer = process::output(ctx, &mut server.ssh(&list_script(&folders)))?;
    let answer = String::from_utf8_lossy(&answer.stdout);
    let to_send = missing(&local, &held(&answer, is_name));
    step(&format!(
        "{} of {} files are on {} already: sending {}",
        local.len() - to_send.len(),
        local.len(),
        server.dest(),
        to_send.len()
    ));
    if to_send.is_empty() {
        return Ok(());
    }
    send(ctx, server, from, &to_send, &add_script(incoming, into))
}

/// Asks the server to publish what a deploy copied into `incoming`, by
/// putting `ready` there, and waits up to `wait_seconds` for the
/// answer in `result`, whose first line is `published` or `failed` and
/// the rest what happened. `what` is what was published and `job` the
/// server's job that does it, for the messages.
///
/// # Errors
///
/// If the server does not answer in time or could not publish.
pub fn publish(
    ctx: &Ctx,
    server: &Server,
    incoming: &str,
    wait_seconds: u32,
    what: &str,
    job: &str,
) -> Result<()> {
    step("Publishing it on the server");
    let script = format!(
        "rm -f {incoming}/result && touch {incoming}/ready && \
         i=0; while [ ! -e {incoming}/result ] && [ $i -lt {wait_seconds} ]; do \
         sleep 1; i=$((i + 1)); done; \
         if [ -e {incoming}/result ]; then cat {incoming}/result; else echo waiting; fi"
    );
    let output = process::output(ctx, &mut server.ssh(&script))?;
    let text = String::from_utf8_lossy(&output.stdout);
    let (status, rest) = text.split_once('\n').unwrap_or((text.trim_end(), ""));
    if !rest.trim().is_empty() {
        println!("{}", rest.trim_end());
    }
    match status {
        "published" => Ok(()),
        "waiting" => bail!(
            "the server did not publish in {wait_seconds} seconds: `systemctl status {job}` on \
             an Ubuntu server, or `service {job} status` and `service list --timers` on a Dukovo \
             one, says why"
        ),
        _ => bail!(
            "the server could not publish the {what}, as the output above says; it serves what it \
             did before"
        ),
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt as _;
    use std::path::PathBuf;

    use super::*;

    const HASH: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    #[test]
    fn a_sha256_name_is_sixty_four_lowercase_hex_digits() {
        assert!(is_sha256_name(HASH));
        assert!(!is_sha256_name(&HASH[1..]));
        assert!(!is_sha256_name(&format!("{HASH}0")));
        assert!(!is_sha256_name(&HASH.to_uppercase()));
        assert!(!is_sha256_name(&format!("g{}", &HASH[1..])));
        assert!(!is_sha256_name(""));
    }

    #[test]
    fn a_bundle_name_is_an_id_and_a_version_and_dkv() {
        assert!(is_bundle_name("org.dukovo.ripgrep-14.1.1.dkv"));
        assert!(is_bundle_name("org.nginx.nginx-1.28.0_2.dkv"));
        assert!(!is_bundle_name(".dkv"));
        assert!(!is_bundle_name(".hidden.dkv"));
        assert!(!is_bundle_name("org.dukovo.ripgrep-14.1.1.dkv.partial"));
        assert!(!is_bundle_name("a b.dkv"));
        assert!(!is_bundle_name("../a.dkv"));
        assert!(!is_bundle_name("*.dkv"));
        assert!(!is_bundle_name("a.dkv\r"));
    }

    #[test]
    fn the_names_a_server_holds_are_the_lines_that_are_names() {
        let answer = format!(
            "{HASH}\nls: cannot access 'x': No such file\n\n{HASH}.partial\n../{HASH}\n \
             {HASH}\n{}\n",
            HASH.replace('0', "1")
        );
        let names = held(&answer, is_sha256_name);
        assert_eq!(names.len(), 2);
        assert!(names.contains(HASH));
        assert!(names.contains(HASH.replace('0', "1").as_str()));
    }

    #[test]
    fn a_line_ending_of_a_carriage_return_still_names_a_file() {
        let answer = format!("{HASH}\r\n");
        assert!(held(&answer, is_sha256_name).contains(HASH));
    }

    #[test]
    fn a_strange_answer_never_makes_a_file_be_skipped() {
        let local = vec![HASH.to_string(), "not-a-hash".to_string()];
        // The server says it holds a file by a name that is not a hash,
        // such as the name a local file that is not one has.
        let names = held("not-a-hash\n*\n", is_sha256_name);
        assert_eq!(missing(&local, &names), local);
    }

    #[test]
    fn only_the_files_the_server_lacks_are_sent() {
        let one = "1".repeat(64);
        let two = "2".repeat(64);
        let three = "3".repeat(64);
        let local = vec![one.clone(), two.clone(), three.clone()];
        let answer = format!("{two}\n");
        assert_eq!(missing(&local, &held(&answer, is_sha256_name)), [one, three]);
        assert!(missing(&local, &held(&local.join("\n"), is_sha256_name)).is_empty());
    }

    #[test]
    fn the_list_script_reads_each_folder_and_never_fails() {
        let script = list_script(&["/a/files", "/b/files"]);
        assert!(script.contains("for folder in /a/files /b/files;"));
        assert!(script.ends_with("true"));
        // Run here, folders that are not there add nothing.
        let output = Command::new("sh").args(["-c", &script]).output().unwrap();
        assert!(output.status.success());
        assert!(output.stdout.is_empty());
    }

    #[test]
    fn a_word_is_quoted_for_the_servers_shell() {
        assert_eq!(quote("a b"), "'a b'");
        assert_eq!(quote("it's"), r"'it'\''s'");
    }

    #[test]
    fn a_script_runs_with_sh_whatever_the_login_shell_is() {
        let server = Server {
            dest: "deploy@example.org".into(),
            options: vec!["-o".into(), "Port=22".into()],
        };
        let command = server.ssh("echo 'hi'");
        let args: Vec<_> =
            command.get_args().map(|arg| arg.to_string_lossy().into_owned()).collect();
        assert_eq!(args, ["-o", "Port=22", "--", "deploy@example.org", r"sh -c 'echo '\''hi'\'''"]);
    }

    /// A folder of its own under the temporary folder, empty.
    fn folder(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("ob-deploy-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    /// The stream the deploy sends for the `names` of `from`.
    fn pack(from: &Path, names: &[&str]) -> Vec<u8> {
        let output = Command::new("tar")
            .args(["-c", "-f", "-", "-C"])
            .arg(from)
            .args(names)
            .output()
            .unwrap();
        assert!(output.status.success());
        output.stdout
    }

    /// Runs `script` as the server would, with `stream` as its input, and
    /// tells whether it succeeded.
    fn run_stream(script: &str, stream: &[u8]) -> bool {
        let mut receiver =
            Command::new("sh").args(["-c", script]).stdin(Stdio::piped()).spawn().unwrap();
        // The script may end before it has read all of it.
        let _ = receiver.stdin.take().unwrap().write_all(stream);
        receiver.wait().unwrap().success()
    }

    /// Runs `script` with the `names` of `from` packed as its input.
    fn run(script: &str, from: &Path, names: &[&str]) -> bool {
        run_stream(script, &pack(from, names))
    }

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn mode(path: &Path) -> u32 {
        std::fs::metadata(path).unwrap().permissions().mode() & 0o7777
    }

    #[test]
    fn replacing_leaves_nothing_the_deploy_did_not_send() {
        let source = folder("replace-source");
        write(&source.join("metadata/2.root.json"), "new root");
        write(&source.join("targets/system/a.json"), "target");
        let incoming = folder("replace-incoming");
        write(&incoming.join("metadata/1.root.json"), "old root");
        write(&incoming.join("metadata/stale.json"), "stale");
        write(&incoming.join("targets/old/b.json"), "old target");
        write(&incoming.join("files/kept"), "kept");
        write(&incoming.join("ready"), "");

        let incoming_name = incoming.to_str().unwrap();
        let script = replace_script(incoming_name, incoming_name, &["metadata", "targets"]);
        assert!(run(&script, &source, &["metadata", "targets"]));

        assert_eq!(
            std::fs::read_to_string(incoming.join("metadata/2.root.json")).unwrap(),
            "new root"
        );
        assert!(!incoming.join("metadata/1.root.json").exists());
        assert!(!incoming.join("metadata/stale.json").exists());
        assert!(!incoming.join("targets/old").exists());
        assert!(incoming.join("targets/system/a.json").is_file());
        // What is not a folder of the deploy is as it was, and nothing
        // is left over.
        assert!(incoming.join("files/kept").is_file());
        assert!(incoming.join("ready").is_file());
        let mut left: Vec<String> = std::fs::read_dir(&incoming)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(left, ["files", "metadata", "ready", "targets"]);
    }

    #[test]
    fn what_is_replaced_is_open_to_the_group_and_closed_to_others() {
        let source = folder("modes-source");
        write(&source.join("metadata/1.root.json"), "root");
        write(&source.join("targets/system/a.json"), "target");
        std::fs::set_permissions(
            source.join("targets/system/a.json"),
            std::fs::Permissions::from_mode(0o444),
        )
        .unwrap();
        let incoming = folder("modes-incoming");
        let incoming_name = incoming.to_str().unwrap();
        let script = replace_script(incoming_name, incoming_name, &["metadata", "targets"]);
        assert!(run(&script, &source, &["metadata", "targets"]));
        assert_eq!(mode(&incoming.join("metadata/1.root.json")), 0o660);
        assert_eq!(mode(&incoming.join("targets/system/a.json")), 0o660);
        assert_eq!(mode(&incoming.join("targets/system")) & 0o777, 0o770);
        assert_eq!(mode(&incoming.join("metadata")) & 0o777, 0o770);
    }

    #[test]
    fn a_stream_that_is_cut_leaves_the_folders_the_server_had() {
        let source = folder("cut-source");
        write(&source.join("metadata/2.root.json"), &"x".repeat(200_000));
        let stream = pack(&source, &["metadata"]);
        let incoming = folder("cut-incoming");
        write(&incoming.join("metadata/1.root.json"), "old root");
        let incoming_name = incoming.to_str().unwrap();
        let script = replace_script(incoming_name, incoming_name, &["metadata"]);
        assert!(!run_stream(&script, &stream[..stream.len() / 2]));
        assert_eq!(
            std::fs::read_to_string(incoming.join("metadata/1.root.json")).unwrap(),
            "old root"
        );
        assert!(!incoming.join("metadata/2.root.json").exists());
        // The next deploy is not troubled by what the cut one left.
        assert!(run_stream(&script, &stream));
        assert!(incoming.join("metadata/2.root.json").is_file());
        assert!(!incoming.join("metadata/1.root.json").exists());
    }

    #[test]
    fn a_stream_without_a_folder_leaves_the_folders_the_server_had() {
        let incoming = folder("lacking-incoming");
        write(&incoming.join("metadata/1.root.json"), "old root");
        write(&incoming.join("targets/old.json"), "old target");
        let source = folder("lacking-source");
        write(&source.join("metadata/2.root.json"), "new root");
        let incoming_name = incoming.to_str().unwrap();
        let script = replace_script(incoming_name, incoming_name, &["metadata", "targets"]);
        assert!(!run(&script, &source, &["metadata"]));
        assert!(incoming.join("metadata/1.root.json").is_file());
        assert!(incoming.join("targets/old.json").is_file());
    }

    #[test]
    fn a_server_that_was_never_set_up_is_told_to_set_it_up() {
        let source = folder("unset-source");
        write(&source.join("metadata/1.root.json"), "root");
        let missing_folder =
            std::env::temp_dir().join(format!("ob-deploy-{}-none", std::process::id()));
        let name = missing_folder.to_str().unwrap();
        let script = replace_script(name, name, &["metadata"]);
        assert!(script.contains("set it up first"));
        assert!(!run(&script, &source, &["metadata"]));
        assert!(!missing_folder.exists());
    }

    #[test]
    fn adding_keeps_the_files_the_server_has() {
        let source = folder("add-source");
        write(&source.join(HASH), "new");
        let incoming = folder("add-incoming");
        let into = incoming.join("files");
        write(&into.join("old"), "old");
        let script = add_script(incoming.to_str().unwrap(), into.to_str().unwrap());
        assert!(run(&script, &source, &[HASH]));
        assert_eq!(std::fs::read_to_string(into.join(HASH)).unwrap(), "new");
        assert_eq!(std::fs::read_to_string(into.join("old")).unwrap(), "old");
        assert_eq!(mode(&into.join(HASH)), 0o660);
        assert!(!incoming.join("files.partial").exists());
    }

    #[test]
    fn adding_into_a_folder_that_is_not_there_makes_it() {
        let source = folder("fresh-source");
        write(&source.join(HASH), "new");
        let incoming = folder("fresh-incoming");
        let into = incoming.join("store/bundles");
        let script = add_script(incoming.to_str().unwrap(), into.to_str().unwrap());
        assert!(run(&script, &source, &[HASH]));
        assert_eq!(std::fs::read_to_string(into.join(HASH)).unwrap(), "new");
        assert_eq!(mode(&into) & 0o777, 0o770);
    }

    #[test]
    fn a_file_cut_short_is_never_taken_for_one_the_server_holds() {
        let source = folder("partial-source");
        write(&source.join(HASH), &"x".repeat(200_000));
        let stream = pack(&source, &[HASH]);
        let incoming = folder("partial-incoming");
        let into = incoming.join("files");
        write(&into.join("old"), "old");
        let script = add_script(incoming.to_str().unwrap(), into.to_str().unwrap());
        assert!(!run_stream(&script, &stream[..stream.len() / 2]));
        // What the server holds is what it held, so the next deploy
        // sends the file again.
        assert!(!into.join(HASH).exists());
        assert!(into.join("old").is_file());
        assert!(run_stream(&script, &stream));
        assert_eq!(std::fs::read(into.join(HASH)).unwrap().len(), 200_000);
        assert!(!incoming.join("files.partial").exists());
    }
}

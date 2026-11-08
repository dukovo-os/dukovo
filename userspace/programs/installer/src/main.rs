// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! dukovo-install's command line (dukovo-install(8)): with no arguments
//! it shows the installer's screens, `--answers FILE` installs as the
//! answers file says, a file or an `https://` address, checked against
//! `--sha256` when given (ADR 0319), and `--hash-password` makes the
//! password hash an answers file gives.

use std::io::BufRead as _;
use std::path::Path;
use std::process::ExitCode;

use dukovo_install::answers::Answers;
use dukovo_install::download;
use dukovo_install::install;
use dukovo_install::log::{self, Log};
use dukovo_install::messages::{self, Arg};
use dukovo_install::screens;

const USAGE: &str = "\
usage: dukovo-install [--release DIR]
       dukovo-install --answers FILE|https://ADDRESS [--sha256 HEX] [--release DIR]
       dukovo-install --hash-password [--test-cost] < FILE";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    let status = match words.as_slice() {
        [] => screens::run(None),
        ["--release", dir] => screens::run(Some(Path::new(dir))),
        ["--answers", from, options @ ..] => match answers_options(options) {
            Some((sha256, release)) => from_answers(from, sha256, release),
            None => {
                eprintln!("{USAGE}");
                2
            }
        },
        ["--hash-password"] => hash_password(accounts::hash::Cost::DEFAULT),
        ["--hash-password", "--test-cost"] => hash_password(accounts::hash::Cost::TEST),
        ["--help" | "-h"] => {
            println!("{USAGE}");
            0
        }
        _ => {
            eprintln!("{USAGE}");
            2
        }
    };
    ExitCode::from(status)
}

/// The options after `--answers FROM`: `--sha256 HEX` and `--release
/// DIR`, each once, in that order. None if they are something else.
fn answers_options<'a>(options: &[&'a str]) -> Option<(Option<&'a str>, Option<&'a Path>)> {
    match options {
        [] => Some((None, None)),
        ["--sha256", hex] => Some((Some(*hex), None)),
        ["--release", dir] => Some((None, Some(Path::new(*dir)))),
        ["--sha256", hex, "--release", dir] => Some((Some(*hex), Some(Path::new(*dir)))),
        _ => None,
    }
}

/// Installs as the answers file `from` says, a file or an `https://`
/// address, checked against `sha256` when given, from the release in
/// `release` or on the install stick.
fn from_answers(from: &str, sha256: Option<&str>, release: Option<&Path>) -> u8 {
    let (answers, source) = match read_answers(from, sha256) {
        Ok(read) => read,
        Err(problems) => {
            for problem in problems {
                eprintln!("dukovo-install: {from}: {problem}");
            }
            return 1;
        }
    };
    let mut log = match Log::create(Path::new(log::LIVE_PATH), true) {
        Ok(log) => log,
        Err(why) => {
            eprintln!("dukovo-install: {why}");
            return 1;
        }
    };
    log.note(&format!("answers from {source}:\n{}", String::from_utf8_lossy(&answers.to_saved())));
    let done = install::find_release(release, &mut log)
        .and_then(|source| install::install(&answers, &source, &mut log).map(|()| source));
    match done {
        Ok(source) => {
            log.finish(&messages::with(
                "installed-line",
                &[
                    ("version", Arg::Text(&source.release.version)),
                    ("disk", Arg::Text(&answers.disks())),
                ],
            ));
            0
        }
        Err(why) => {
            log.problem(&why);
            log.problem(&format!("the install did not finish; the log is {}", log::LIVE_PATH));
            1
        }
    }
}

/// Reads the answers file `from`, a file or an `https://` address,
/// checks it against `sha256` when given, and checks its answers. Gives
/// them, and where they came from for the log: the file or address, with
/// the file's SHA-256 and whether it was checked.
fn read_answers(from: &str, sha256: Option<&str>) -> Result<(Answers, String), Vec<String>> {
    let expected = download::sha256(sha256.unwrap_or_default()).map_err(|why| vec![why])?;
    // An address is anything with a scheme; only https:// is fetched.
    let file = if from.contains("://") {
        let url = download::address(from).map_err(|why| vec![why])?;
        download::fetch(&url, expected)
    } else {
        let bytes = std::fs::read(from).map_err(|err| vec![err.to_string()])?;
        download::checked(bytes, expected)
    }
    .map_err(|why| vec![why])?;
    let answers = Answers::parse(&file.bytes).map_err(|why| vec![why])?;
    answers.check()?;
    Ok((answers, file.source(from)))
}

/// Prints the Argon2id hash, at `cost`, of the password on the first line
/// of standard input, for an answers file's `password_hash`.
fn hash_password(cost: accounts::hash::Cost) -> u8 {
    let mut line = Vec::new();
    if let Err(err) = std::io::stdin().lock().read_until(b'\n', &mut line) {
        eprintln!("dukovo-install: reading the password: {err}");
        return 1;
    }
    let password = line.strip_suffix(b"\n").unwrap_or(&line);
    let password = password.strip_suffix(b"\r").unwrap_or(password);
    let hashed = dukovo_install::answers::hash_password(password, cost);
    // The password goes no further than this function.
    accounts::wipe(&mut line);
    match hashed {
        Ok(hash) => {
            println!("{hash}");
            0
        }
        Err(why) => {
            eprintln!("dukovo-install: {why}");
            1
        }
    }
}

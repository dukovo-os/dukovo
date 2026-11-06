// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! dukovo-install's command line (dukovo-install(8)): `--answers FILE`
//! installs as the answers file says, and `--hash-password` makes the
//! password hash an answers file gives.

use std::io::BufRead as _;
use std::path::Path;
use std::process::ExitCode;

use dukovo_install::answers::Answers;
use dukovo_install::install;
use dukovo_install::log::{self, Log};

const USAGE: &str = "\
usage: dukovo-install --answers FILE [--release DIR]
       dukovo-install --hash-password [--test-cost] < FILE";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    let status = match words.as_slice() {
        ["--answers", file] => from_answers(Path::new(file), None),
        ["--answers", file, "--release", dir] => {
            from_answers(Path::new(file), Some(Path::new(dir)))
        }
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

/// Installs as the answers file at `file` says, from the release in
/// `release` or on the install stick.
fn from_answers(file: &Path, release: Option<&Path>) -> u8 {
    let answers = match read_answers(file) {
        Ok(answers) => answers,
        Err(problems) => {
            for problem in problems {
                eprintln!("dukovo-install: {}: {problem}", file.display());
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
    log.note(&format!(
        "answers from {}:\n{}",
        file.display(),
        String::from_utf8_lossy(&answers.to_saved())
    ));
    let done = install::find_release(release, &mut log)
        .and_then(|source| install::install(&answers, &source, &mut log).map(|()| source));
    match done {
        Ok(source) => {
            log.step(&format!(
                "Dukovo {} is installed on {}. Remove the install stick, then restart.",
                source.release.version,
                answers.target()
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

/// Reads and checks the answers file.
fn read_answers(file: &Path) -> Result<Answers, Vec<String>> {
    let bytes = std::fs::read(file).map_err(|err| vec![err.to_string()])?;
    let answers = Answers::parse(&bytes).map_err(|why| vec![why])?;
    answers.check()?;
    Ok(answers)
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
    let mut salt = [0u8; accounts::hash::SALT];
    if ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut salt).is_err() {
        eprintln!("dukovo-install: the system gave no random numbers for the salt");
        return 1;
    }
    let hashed = accounts::hash::hash(password, &salt, cost);
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

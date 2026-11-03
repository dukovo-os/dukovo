// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The client the TUF conformance suite tests (ADR 0380), by its CLI
//! protocol (tuf-conformance's CLIENT-CLI.md), on the host: `ob test
//! tuf` runs the suite with it.
//!
//! ```text
//! tuf-conformance --metadata-dir DIR init ROOT
//! tuf-conformance --metadata-dir DIR --metadata-url URL refresh
//! tuf-conformance --metadata-dir DIR --metadata-url URL --target-name NAME...
//!     --target-base-url URL --target-dir DIR download
//! ```
//!
//! It exits 0 when the command did all it was asked, and 1 otherwise.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use tuf::{Updater, Url};

const USAGE: &str = "usage: tuf-conformance --metadata-dir DIR [--metadata-url URL] \
                     [--target-name NAME]... [--target-base-url URL] [--target-dir DIR] \
                     (init ROOT | refresh | download)";

/// The command line.
#[derive(Debug, Default)]
struct Args {
    metadata_dir: Option<PathBuf>,
    metadata_url: Option<Url>,
    target_names: Vec<String>,
    target_url: Option<Url>,
    target_dir: Option<PathBuf>,
    command: Vec<String>,
}

fn parse(mut argv: impl Iterator<Item = String>) -> Result<Args, String> {
    let mut args = Args::default();
    while let Some(arg) = argv.next() {
        let (name, inline) = match arg.split_once('=') {
            Some((name, value)) if name.starts_with("--") => (name.to_string(), Some(value.into())),
            _ => (arg.clone(), None),
        };
        if !name.starts_with("--") {
            args.command.push(arg);
            continue;
        }
        let value =
            inline.or_else(|| argv.next()).ok_or_else(|| format!("{name} needs a value"))?;
        let url = |value: &str| Url::parse(value).map_err(|err| format!("{name} {value}: {err}"));
        match name.as_str() {
            "--metadata-dir" => args.metadata_dir = Some(value.into()),
            "--metadata-url" => args.metadata_url = Some(url(&value)?),
            "--target-name" => args.target_names.push(value),
            "--target-base-url" => args.target_url = Some(url(&value)?),
            "--target-dir" => args.target_dir = Some(value.into()),
            _ => return Err(format!("unknown option {name}")),
        }
    }
    Ok(args)
}

fn updater(args: &Args, dir: &Path) -> Result<Updater, String> {
    let metadata = args.metadata_url.as_ref().ok_or("--metadata-url is needed")?;
    Updater::new(dir, metadata, args.target_url.as_ref()).map_err(|err| err.to_string())
}

fn run(args: &Args) -> Result<(), String> {
    let dir = args.metadata_dir.as_deref().ok_or("--metadata-dir is needed")?;
    let command: Vec<&str> = args.command.iter().map(String::as_str).collect();
    match command.as_slice() {
        ["init", root] => {
            let bytes = std::fs::read(root).map_err(|err| format!("{root}: {err}"))?;
            tuf::init(dir, &bytes).map_err(|err| err.to_string())
        }
        ["refresh"] => updater(args, dir)?.refresh().map_err(|err| err.to_string()),
        ["download"] => {
            let target_dir = args.target_dir.as_deref().ok_or("--target-dir is needed")?;
            if args.target_names.is_empty() {
                return Err("--target-name is needed".into());
            }
            let mut updater = updater(args, dir)?;
            for name in &args.target_names {
                let target = updater
                    .target(name)
                    .map_err(|err| format!("{name}: {err}"))?
                    .ok_or_else(|| format!("{name}: the repository has no such target"))?;
                let file = Updater::target_path(target_dir, name);
                if !Updater::cached(&file, &target) {
                    updater
                        .download_target(name, &target, &file)
                        .map_err(|err| format!("{name}: {err}"))?;
                }
                println!("{name}: {}", file.display());
            }
            Ok(())
        }
        _ => Err(USAGE.into()),
    }
}

fn main() -> ExitCode {
    let args = match parse(std::env::args().skip(1)) {
        Ok(args) => args,
        Err(err) => {
            eprintln!("tuf-conformance: {err}\n{USAGE}");
            return ExitCode::FAILURE;
        }
    };
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("tuf-conformance: {err}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(line: &str) -> Result<Args, String> {
        parse(line.split(' ').map(String::from))
    }

    #[test]
    fn the_protocols_arguments_parse() {
        let parsed = args(
            "--metadata-dir /m --metadata-url http://h/m --target-name a/b.txt \
             --target-name=c.txt --target-base-url http://h/t --target-dir /t download",
        )
        .unwrap();
        assert_eq!(parsed.metadata_dir, Some(PathBuf::from("/m")));
        assert_eq!(parsed.target_names, ["a/b.txt", "c.txt"]);
        assert_eq!(parsed.target_url.unwrap().to_string(), "http://h/t");
        assert_eq!(parsed.command, ["download"]);
        assert_eq!(
            args("--metadata-dir /m init ./root.json").unwrap().command,
            ["init", "./root.json"]
        );
        assert!(args("--metadata-dir").is_err());
        assert!(args("--colour red refresh").is_err());
        assert!(args("--metadata-url nonsense refresh").is_err());
    }
}

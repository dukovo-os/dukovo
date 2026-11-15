// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Crate identities that do not depend on where the checkout is
//! (ADR 0535).
//!
//! Cargo names each crate it builds to rustc with `-C metadata`, a hash
//! of the crate's package, features, profile and dependencies. rustc mixes
//! it into the crate's own hash and into every symbol of the crate. For a
//! path dependency outside the workspace that uses it, such as kernel/abi
//! in the user programs' workspace or tools/ridlc in the kernel's, Cargo
//! hashes the dependency's absolute path. Two checkouts in two folders
//! then name the same crate differently, and every crate that depends on
//! it differs too, as its symbols name it.
//!
//! `ob` has Cargo run rustc through `ob` itself (`RUSTC_WRAPPER`), which
//! gives rustc another `-C metadata`: a hash of what the compilation is.
//! That is:
//!
//! - rustc's arguments, with each directory of [`crate::repro::maps`]
//!   written as its name;
//! - the package's name, version and folder;
//! - for each crate given with `--extern`, that crate's own hash, which
//!   the wrapper keeps beside the crate's output as
//!   `.repro<extra-filename>`.
//!
//! What only says where things are is left out: where the outputs go,
//! where to look for libraries, and the maps of paths to names. Cargo
//! names those folders by its own hashes. The file names Cargo chose
//! (`-C extra-filename`) stay, so Cargo finds what it built where it
//! expects it.
//!
//! The wrapper also has rustc write a build script's output folder,
//! `OUT_DIR`, under a name of its own. Code that includes what a build
//! script generated names that folder in its panics and debug
//! information, and Cargo's hash in the folder's path differs between
//! checkouts as the crate's did.
//!
//! An rlib's object files are named after its file, Cargo's hash
//! included (`nix-<hash>.nix.<cgu>.rcgu.o`), and rustc's ThinLTO takes
//! the crates of a program in the order of those names. Two crates of
//! one name, such as two versions of nix, or the libc crate of a program
//! and that of Rust's standard library, then come in an order that
//! depends on where the checkout is, and the program's symbol table with
//! them. Once rustc has written an rlib, the wrapper names its object
//! files by the crate's identity instead, a name of the same length.

use std::fmt::Write as _;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use sha2::{Digest, Sha256};

use crate::Ctx;

/// The variable that tells the wrapper the directories to write as
/// names, a `<path>\t<name>` line each. It is set, and the first argument
/// is a compiler, when `ob` runs as the wrapper.
pub const MAPS: &str = "OB_REPRO_MAPS";

/// The wrapper Cargo would have run had `ob` not set its own, such as
/// sccache, which `ob`'s runs in turn.
const NEXT: &str = "OB_REPRO_NEXT_WRAPPER";

/// What the identity's format is called. A change to what goes into an
/// identity changes this, and with it every hash.
pub const FORMAT: &str = "dukovo rustc identity 2";

/// The name a build script's output folder is written under, followed
/// by the package's name.
const OUT_DIR_NAME: &str = "/dukovo-out";

/// Has `cmd`, a Cargo command, run rustc through `ob`.
pub fn configure(ctx: &Ctx, cmd: &mut Command) {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    if let Some(next) =
        std::env::var_os("RUSTC_WRAPPER").filter(|next| !next.is_empty() && Path::new(next) != exe)
    {
        cmd.env(NEXT, next);
    }
    cmd.env("RUSTC_WRAPPER", exe).env(MAPS, encode(&crate::repro::maps(ctx)));
}

/// Undoes [`configure`] for `cmd`, a Cargo command whose compiler is not
/// rustc, such as Kani's `kani-compiler`, which the wrapper does not
/// run: what it checks is no part of a release. The wrapper Cargo would
/// have run before, if any, runs again.
pub fn unconfigure(cmd: &mut Command) {
    cmd.env_remove(MAPS).env_remove(NEXT);
    let exe = std::env::current_exe().ok();
    match std::env::var_os("RUSTC_WRAPPER")
        .filter(|next| !next.is_empty() && exe.as_deref() != Some(Path::new(next)))
    {
        Some(next) => cmd.env("RUSTC_WRAPPER", next),
        None => cmd.env_remove("RUSTC_WRAPPER"),
    };
}

/// The maps of [`crate::repro::maps`] as [`MAPS`] holds them.
fn encode(maps: &[(PathBuf, &str)]) -> String {
    let mut text = String::new();
    for (from, to) in maps {
        let _ = writeln!(text, "{}\t{to}", from.display());
    }
    text
}

/// The maps [`MAPS`] holds, the longest path first, so that a folder
/// inside another is written as its own name.
fn decode(text: &str) -> Vec<(String, String)> {
    let mut maps: Vec<(String, String)> = text
        .lines()
        .filter_map(|line| line.split_once('\t'))
        .filter(|(from, _)| !from.is_empty())
        .map(|(from, to)| (from.to_string(), to.to_string()))
        .collect();
    maps.sort_by(|a, b| b.0.len().cmp(&a.0.len()).then_with(|| a.0.cmp(&b.0)));
    maps
}

/// `ob` as Cargo's rustc wrapper: `argv` being the compiler and its
/// arguments, runs the compiler with a `-C metadata` of its own. None
/// when `ob` is not run as the wrapper.
pub fn internal(argv: &[String]) -> Option<ExitCode> {
    let maps = std::env::var(MAPS).ok()?;
    let (compiler, args) = argv.split_first()?;
    if !is_compiler(compiler) {
        return None;
    }
    let mut args = args.to_vec();
    let rlib = match rewrite(&mut args, &decode(&maps), &|key| std::env::var(key).ok()) {
        Ok(rlib) => rlib,
        Err(err) => {
            eprintln!("ob: rustc wrapper: {err}");
            return Some(ExitCode::FAILURE);
        }
    };
    let mut cmd = match std::env::var_os(NEXT) {
        Some(next) => {
            let mut cmd = Command::new(next);
            cmd.arg(compiler);
            cmd
        }
        None => Command::new(compiler),
    };
    cmd.args(&args);
    let Some(rlib) = rlib else {
        let err = cmd.exec();
        eprintln!("ob: rustc wrapper: cannot run {compiler}: {err}");
        return Some(ExitCode::from(127));
    };
    let status = match cmd.status() {
        Ok(status) => status,
        Err(err) => {
            eprintln!("ob: rustc wrapper: cannot run {compiler}: {err}");
            return Some(ExitCode::from(127));
        }
    };
    if !status.success() {
        let code = status.code().and_then(|code| u8::try_from(code).ok()).unwrap_or(1);
        return Some(ExitCode::from(code.max(1)));
    }
    if let Err(err) = rlib.name_members() {
        eprintln!("ob: rustc wrapper: {err}");
        return Some(ExitCode::FAILURE);
    }
    Some(ExitCode::SUCCESS)
}

/// An rlib rustc is to write, whose object files the wrapper names by the
/// crate's identity once it has.
#[derive(Debug, PartialEq, Eq)]
struct Rlib {
    /// The rlib, as Cargo named it.
    path: PathBuf,
    /// What follows the crate's name in its object files' names: Cargo's
    /// `-C extra-filename`, such as `-0123456789abcdef`.
    cargo: String,
    /// What follows it instead: `-` and the crate's identity.
    ours: String,
}

impl Rlib {
    /// Names the rlib's object files by the crate's identity, if rustc
    /// wrote it (a check writes only the metadata).
    fn name_members(&self) -> Result<(), String> {
        let Ok(mut data) = std::fs::read(&self.path) else {
            return Ok(());
        };
        let renamed = rename_members(&mut data, &self.cargo, &self.ours)
            .map_err(|err| format!("{}: {err}", self.path.display()))?;
        if !renamed {
            return Ok(());
        }
        let mut partial = self.path.as_os_str().to_owned();
        partial.push(format!(".{}", std::process::id()));
        let partial = PathBuf::from(partial);
        std::fs::write(&partial, &data)
            .and_then(|()| std::fs::rename(&partial, &self.path))
            .map_err(|err| format!("naming the object files of {}: {err}", self.path.display()))
    }
}

/// Writes `ours` for `cargo` in the names of the members of `data`, an ar
/// archive, where it stands between a crate's name and the rest of an
/// object file's name: `<crate><cargo>.<cgu>.rcgu.o`. The names are in
/// the archive's table of long names (`//`), and `ours` is as long as
/// `cargo`, so that every offset in the archive stays where it is.
/// Whether a name changed.
fn rename_members(data: &mut [u8], cargo: &str, ours: &str) -> Result<bool, String> {
    const MAGIC: &[u8] = b"!<arch>\n";
    const HEADER: usize = 60;
    if cargo.len() != ours.len() || cargo == ours {
        return Ok(false);
    }
    if !data.starts_with(MAGIC) {
        return Err("not an ar archive".into());
    }
    let (from, to) = (format!("{cargo}."), format!("{ours}."));
    let mut renamed = false;
    let mut at = MAGIC.len();
    while at + HEADER <= data.len() {
        let header = &data[at..at + HEADER];
        if &header[58..60] != b"`\n" {
            return Err(format!("no member header at byte {at}"));
        }
        let size = std::str::from_utf8(&header[48..58])
            .ok()
            .and_then(|size| size.trim().parse::<usize>().ok())
            .ok_or_else(|| format!("the member at byte {at} has no size"))?;
        let start = at + HEADER;
        let end = start.checked_add(size).filter(|&end| end <= data.len());
        let end = end.ok_or_else(|| format!("the member at byte {at} runs past the end"))?;
        if header[..16].trim_ascii_end() == b"//" {
            let table = &mut data[start..end];
            let mut i = 0;
            while let Some(found) = find(&table[i..], from.as_bytes()) {
                let place = i + found;
                table[place..place + to.len()].copy_from_slice(to.as_bytes());
                renamed = true;
                i = place + to.len();
            }
        }
        at = end + (size & 1);
    }
    Ok(renamed)
}

/// Where `needle` first is in `haystack`.
fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|window| window == needle)
}

/// Whether `path` is what Cargo runs as rustc: rustc itself, or Clippy's
/// driver, which Cargo puts in front of it for a workspace's crates.
fn is_compiler(path: &str) -> bool {
    let name = Path::new(path).file_name().and_then(|name| name.to_str()).unwrap_or_default();
    name.starts_with("rustc") || name.starts_with("clippy-driver")
}

/// Gives `args`, rustc's arguments, the `-C metadata` of what they
/// compile, keeps it beside the crate's output for the crates that depend
/// on it, and names a build script's output folder. Arguments that do not
/// compile a crate of Cargo's, such as `-vV`, stay as they are. The rlib
/// they write, if any, whose object files are to be named once it is
/// written.
fn rewrite(
    args: &mut Vec<String>,
    maps: &[(String, String)],
    env: &dyn Fn(&str) -> Option<String>,
) -> Result<Option<Rlib>, String> {
    let given = args.clone();
    let file = expand_argument_files(args)?;
    let metadata = codegen_values(args, "metadata");
    if metadata.is_empty() {
        *args = given;
        return Ok(None);
    }
    let hash = hash(&identity(args, maps, env));
    for (at, start) in metadata {
        args[at].replace_range(start.., &hash);
    }
    let extra = codegen_values(args, "extra-filename")
        .first()
        .map(|&(at, start)| args[at][start..].to_string());
    let mut rlib = None;
    if let (Some(extra), Some(out_dir)) = (extra, flag_value(args, "--out-dir")) {
        keep(Path::new(&out_dir), &extra, &hash)?;
        if let Some(name) = flag_value(args, "--crate-name").filter(|_| writes_rlib(args)) {
            let path = Path::new(&out_dir).join(format!("lib{name}{extra}.rlib"));
            rlib = Some(Rlib { path, cargo: extra, ours: format!("-{hash}") });
        }
    }
    if let Some(out_dir) = env("OUT_DIR") {
        let package = env("CARGO_PKG_NAME").unwrap_or_default();
        args.push(format!("--remap-path-prefix={out_dir}={OUT_DIR_NAME}/{package}"));
    }
    if let Some(file) = file {
        write_argument_file(args, &file)?;
    }
    Ok(rlib)
}

/// Whether `args` have rustc write an rlib: a library crate, linked (a
/// check emits only its metadata).
fn writes_rlib(args: &[String]) -> bool {
    let values = |flag: &str| -> Vec<String> {
        let joined = format!("{flag}=");
        let mut values = Vec::new();
        for (i, arg) in args.iter().enumerate() {
            if arg == flag {
                values.extend(args.get(i + 1).cloned());
            } else if let Some(value) = arg.strip_prefix(&joined) {
                values.push(value.to_string());
            }
        }
        values
    };
    let types = values("--crate-type");
    let library =
        types.iter().flat_map(|types| types.split(',')).any(|t| t == "lib" || t == "rlib");
    let emits = values("--emit");
    let linked = emits.is_empty()
        || emits
            .iter()
            .flat_map(|emit| emit.split(','))
            .any(|emit| emit.split('=').next() == Some("link"));
    library && linked
}

/// Puts the arguments of each `@file` among `args` in its place, one a
/// line as rustc reads them: the first such file, if there was one.
/// Cargo writes one when a command line is too long to run.
fn expand_argument_files(args: &mut Vec<String>) -> Result<Option<PathBuf>, String> {
    let mut first = None;
    let mut expanded = Vec::with_capacity(args.len());
    for arg in args.drain(..) {
        match arg.strip_prefix('@').filter(|path| !path.starts_with("shell:")) {
            Some(path) => {
                let text = std::fs::read_to_string(path)
                    .map_err(|err| format!("reading the argument file {path}: {err}"))?;
                expanded.extend(text.lines().map(String::from));
                first.get_or_insert_with(|| PathBuf::from(path));
            }
            None => expanded.push(arg),
        }
    }
    *args = expanded;
    Ok(first)
}

/// Writes `args` to a new argument file beside `file`, and leaves it
/// alone in `args`, as Cargo gave its file.
fn write_argument_file(args: &mut Vec<String>, file: &Path) -> Result<(), String> {
    let mut name = file.as_os_str().to_owned();
    name.push(".ob");
    let ours = PathBuf::from(name);
    let mut text = String::new();
    for arg in args.iter() {
        let _ = writeln!(text, "{arg}");
    }
    std::fs::write(&ours, text)
        .map_err(|err| format!("writing the argument file {}: {err}", ours.display()))?;
    *args = vec![format!("@{}", ours.display())];
    Ok(())
}

/// Where each `-C <name>=` is among `args`: the argument that holds
/// `<name>=` and where its value starts in it, whether rustc was given
/// `-C <name>=value`, `-C<name>=value` or `--codegen=<name>=value`.
fn codegen_values(args: &[String], name: &str) -> Vec<(usize, usize)> {
    let key = format!("{name}=");
    let mut found = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if arg == "-C" || arg == "--codegen" {
            if args.get(i + 1).is_some_and(|next| next.starts_with(&key)) {
                found.push((i + 1, key.len()));
            }
            i += 2;
            continue;
        }
        let option = arg.strip_prefix("--codegen=").or_else(|| arg.strip_prefix("-C"));
        if let Some(option) = option.filter(|option| option.starts_with(&key)) {
            found.push((i, arg.len() - option.len() + key.len()));
        }
        i += 1;
    }
    found
}

/// The value of `flag` among `args`, given as `flag value` or
/// `flag=value`.
fn flag_value(args: &[String], flag: &str) -> Option<String> {
    let joined = format!("{flag}=");
    args.iter().enumerate().find_map(|(i, arg)| {
        if arg == flag {
            args.get(i + 1).cloned()
        } else {
            arg.strip_prefix(&joined).map(String::from)
        }
    })
}

/// What a compilation is, as text: the format, the package, and rustc's
/// arguments with paths written as names, each crate it is given by its
/// identity, and what only says where things are left out.
fn identity(
    args: &[String],
    maps: &[(String, String)],
    env: &dyn Fn(&str) -> Option<String>,
) -> String {
    let mut parts = vec![FORMAT.to_string()];
    for key in ["CARGO_PKG_NAME", "CARGO_PKG_VERSION", "CARGO_MANIFEST_DIR"] {
        parts.push(format!("{key}={}", normalize(&env(key).unwrap_or_default(), maps)));
    }
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        let next = args.get(i + 1).map(String::as_str);
        // Flags whose value is the next argument.
        let (flag, value, taken) = match arg {
            "-C" | "--codegen" | "--extern" | "-L" | "--out-dir" | "-o" | "--remap-path-prefix" => {
                (arg, next.unwrap_or_default(), 2)
            }
            _ => match arg.split_once('=') {
                Some((
                    flag @ ("--codegen" | "--extern" | "--out-dir" | "--remap-path-prefix"),
                    value,
                )) => (flag, value, 1),
                _ if arg.starts_with("-C") && arg.len() > 2 => ("-C", &arg[2..], 1),
                _ if arg.starts_with("-L") && arg.len() > 2 => ("-L", &arg[2..], 1),
                _ => ("", arg, 1),
            },
        };
        i += taken;
        match flag {
            // Where the outputs go and where libraries are looked for:
            // folders Cargo names by its own hashes.
            "-L" | "--out-dir" | "-o" | "--remap-path-prefix" => {}
            "-C" | "--codegen" => {
                let skipped = ["metadata=", "extra-filename=", "incremental="];
                if !skipped.iter().any(|option| value.starts_with(option)) {
                    parts.push(format!("-C {}", normalize(value, maps)));
                }
            }
            "--extern" => parts.push(match value.split_once('=') {
                Some((name, path)) => format!("--extern {name}={}", dependency(path, maps)),
                None => format!("--extern {value}"),
            }),
            _ => parts.push(normalize(value, maps)),
        }
    }
    parts.join("\n")
}

/// The identity of the crate at `path`, an `--extern`'s file: the hash
/// the wrapper kept beside it when it compiled it, or else its path with
/// the directories written as names.
fn dependency(path: &str, maps: &[(String, String)]) -> String {
    kept_beside(Path::new(path))
        .and_then(|kept| std::fs::read_to_string(kept).ok())
        .map(|hash| hash.trim().to_string())
        .filter(|hash| !hash.is_empty())
        .unwrap_or_else(|| normalize(path, maps))
}

/// Where the hash of the crate in `file` is kept: `.repro-<hash>` beside
/// it, `<hash>` being what Cargo put at the end of its name
/// (`libname-<hash>.rlib`).
fn kept_beside(file: &Path) -> Option<PathBuf> {
    let stem = file.file_stem()?.to_str()?;
    let (_, hash) = stem.rsplit_once('-')?;
    if hash.is_empty() || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    Some(file.with_file_name(format!(".repro-{hash}")))
}

/// Keeps `hash` beside the output in `out_dir` whose names end in
/// `extra`, for the crates that depend on it, whole or not at all.
fn keep(out_dir: &Path, extra: &str, hash: &str) -> Result<(), String> {
    let path = out_dir.join(format!(".repro{extra}"));
    let partial = out_dir.join(format!(".repro{extra}.{}", std::process::id()));
    std::fs::create_dir_all(out_dir)
        .and_then(|()| std::fs::write(&partial, hash))
        .and_then(|()| std::fs::rename(&partial, &path))
        .map_err(|err| format!("keeping the crate's hash in {}: {err}", path.display()))
}

/// The first 16 hexadecimal digits of the SHA-256 of `text`, as long as
/// Cargo's own.
fn hash(text: &str) -> String {
    let mut hex = String::with_capacity(16);
    for byte in &Sha256::digest(text.as_bytes())[..8] {
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

/// `text` with every directory of `maps` (the longest first) written as
/// its name, in one pass, so that a name is never read again as a path.
fn normalize(text: &str, maps: &[(String, String)]) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    'text: while let Some(c) = rest.chars().next() {
        for (from, to) in maps {
            if let Some(after) = rest.strip_prefix(from.as_str()) {
                out.push_str(to);
                rest = after;
                continue 'text;
            }
        }
        out.push(c);
        rest = &rest[c.len_utf8()..];
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// A checkout at `root` with Cargo's home at `home`, as the wrapper's
    /// maps have them.
    fn maps(root: &str, home: &str) -> Vec<(String, String)> {
        decode(&format!("{root}\t/dukovo\n{root}/target\t/dukovo-target\n{home}\t/cargo\n"))
    }

    fn strings(list: &[&str]) -> Vec<String> {
        list.iter().map(ToString::to_string).collect()
    }

    /// rustc's arguments for crate `name` of the checkout at `root`, as
    /// Cargo gives them, Cargo's hash being `cargo_hash` and the
    /// dependency `dep` at `dep_file`.
    fn compile(root: &str, cargo_hash: &str, dep_file: &str, feature: &str) -> Vec<String> {
        let out = format!("{root}/target/release/build/app/{cargo_hash}/out");
        vec![
            "--crate-name".into(),
            "app".into(),
            "--edition=2024".into(),
            format!("{root}/kernel/abi/src/lib.rs"),
            "--crate-type".into(),
            "lib".into(),
            "--cfg".into(),
            format!("feature=\"{feature}\""),
            "-C".into(),
            format!("metadata={cargo_hash}"),
            "-C".into(),
            format!("extra-filename=-{cargo_hash}"),
            "--out-dir".into(),
            out.clone(),
            "-L".into(),
            format!("dependency={out}"),
            "--extern".into(),
            format!("dep={dep_file}"),
            format!("--remap-path-prefix={root}=/dukovo"),
            "-Copt-level=3".into(),
        ]
    }

    fn env(root: &str) -> impl Fn(&str) -> Option<String> {
        let vars: HashMap<&str, String> = HashMap::from([
            ("CARGO_PKG_NAME", "radix-abi".to_string()),
            ("CARGO_PKG_VERSION", "0.15.0".to_string()),
            ("CARGO_MANIFEST_DIR", format!("{root}/kernel/abi")),
        ]);
        move |key| vars.get(key).cloned()
    }

    fn temp(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("ob-rustc-wrapper-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn metadata(args: &[String]) -> String {
        let (at, start) = codegen_values(args, "metadata")[0];
        args[at][start..].to_string()
    }

    #[test]
    fn a_crate_has_one_identity_wherever_the_checkout_is() {
        let base = temp("wherever");
        let (a, b) = (base.join("a/dukovo"), base.join("second-place"));
        let (a, b) = (a.to_str().unwrap(), b.to_str().unwrap());
        let mut first = compile(a, "1111222233334444", "/nowhere/libdep-aaaa.rlib", "std");
        let mut second = compile(b, "5555666677778888", "/nowhere/libdep-bbbb.rlib", "std");
        // The dependency was built by the wrapper in each, and kept the
        // same identity.
        for (root, args) in [(a, &mut first), (b, &mut second)] {
            let deps = PathBuf::from(root).join("deps");
            std::fs::create_dir_all(&deps).unwrap();
            let hash = if root == a { "aaaa" } else { "bbbb" };
            std::fs::write(deps.join(format!(".repro-{hash}")), "0123456789abcdef").unwrap();
            let file = deps.join(format!("libdep-{hash}.rlib"));
            let at = args.iter().position(|arg| arg.starts_with("dep=")).unwrap();
            args[at] = format!("dep={}", file.display());
        }
        rewrite(&mut first, &maps(a, "/home/runner/.cargo"), &env(a)).unwrap();
        rewrite(&mut second, &maps(b, "/home/builder/.cargo"), &env(b)).unwrap();
        let _ = std::fs::remove_dir_all(&base);
        assert_eq!(metadata(&first), metadata(&second));
        assert_ne!(metadata(&first), "1111222233334444");
        // Cargo still finds what it built under the names it chose.
        assert!(first.contains(&"extra-filename=-1111222233334444".to_string()));
        assert!(second.contains(&"extra-filename=-5555666677778888".to_string()));
    }

    #[test]
    fn what_a_crate_is_built_with_changes_its_identity() {
        let base = temp("identity");
        let root = base.join("dukovo").display().to_string();
        let maps = maps(&root, "/home/x/.cargo");
        let identity_of = |feature: &str, dep: &str| {
            let mut args = compile(&root, "1111222233334444", dep, feature);
            rewrite(&mut args, &maps, &env(&root)).unwrap();
            metadata(&args)
        };
        let plain = identity_of("std", "/nowhere/libdep-aaaa.rlib");
        let other_feature = identity_of("alloc", "/nowhere/libdep-aaaa.rlib");
        let other_dep = identity_of("std", "/nowhere/libother-aaaa.rlib");
        let again = identity_of("std", "/nowhere/libdep-aaaa.rlib");
        let _ = std::fs::remove_dir_all(&base);
        assert_ne!(plain, other_feature);
        assert_ne!(plain, other_dep);
        assert_eq!(plain, again);
    }

    #[test]
    fn the_hash_is_kept_beside_the_output_for_the_crates_that_depend_on_it() {
        let dir = temp("kept");
        let out = dir.join("out");
        let mut args = strings(&["--crate-name", "dep", "src/lib.rs", "-C", "metadata=ffff"]);
        args.extend(strings(&["-C", "extra-filename=-0a0b0c0d0e0f1011", "--out-dir"]));
        args.push(out.display().to_string());
        rewrite(&mut args, &[], &|_| None).unwrap();
        let rlib = out.join("libdep-0a0b0c0d0e0f1011.rlib");
        let kept = std::fs::read_to_string(kept_beside(&rlib).unwrap()).unwrap();
        let read = dependency(rlib.to_str().unwrap(), &[]);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(kept, metadata(&args));
        assert_eq!(read, metadata(&args));
    }

    #[test]
    fn a_command_that_builds_no_crate_of_cargos_is_left_alone() {
        for list in [&["-vV"][..], &["-", "--crate-name", "___", "--print=file-names"]] {
            let mut args = strings(list);
            rewrite(&mut args, &[], &|_| Some("/somewhere".into())).unwrap();
            assert_eq!(args, strings(list));
        }
    }

    #[test]
    fn each_way_of_giving_a_codegen_option_is_found() {
        let args = strings(&[
            "-C",
            "metadata=aa",
            "-Cmetadata=bb",
            "--codegen=metadata=cc",
            "-Copt-level=3",
        ]);
        let found: Vec<&str> = codegen_values(&args, "metadata")
            .iter()
            .map(|&(at, start)| &args[at][start..])
            .collect();
        assert_eq!(found, ["aa", "bb", "cc"]);
    }

    #[test]
    fn a_build_scripts_output_folder_is_written_under_a_name() {
        let mut args = strings(&["--crate-name", "abi", "-C", "metadata=aa"]);
        let env = |key: &str| match key {
            "OUT_DIR" => Some("/w/target/build/radix-abi/0123/out".to_string()),
            "CARGO_PKG_NAME" => Some("radix-abi".to_string()),
            _ => None,
        };
        rewrite(&mut args, &[], &env).unwrap();
        assert_eq!(
            args.last().unwrap(),
            "--remap-path-prefix=/w/target/build/radix-abi/0123/out=/dukovo-out/radix-abi"
        );
    }

    #[test]
    fn a_folder_inside_another_is_written_as_its_own_name_and_names_are_not_read_again() {
        let maps = decode("/d\t/dukovo\n/d/target\t/dukovo-target\n");
        assert_eq!(normalize("/d/target/x /d/src", &maps), "/dukovo-target/x /dukovo/src");
        // "/dukovo" holds "/d", which must not be read as the checkout.
        assert_eq!(normalize("/d/a", &maps), "/dukovo/a");
    }

    #[test]
    fn an_argument_file_is_read_and_written_anew() {
        let dir = temp("argfile");
        let file = dir.join("args");
        std::fs::write(&file, "--crate-name\nx\n-C\nmetadata=aa\n").unwrap();
        let mut args = vec![format!("@{}", file.display())];
        rewrite(&mut args, &[], &|_| None).unwrap();
        let ours = args[0].strip_prefix('@').unwrap().to_string();
        let text = std::fs::read_to_string(&ours).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(ours.ends_with("args.ob"));
        assert!(text.starts_with("--crate-name\nx\n-C\nmetadata="));
        assert!(!text.contains("metadata=aa\n"));
    }

    #[test]
    fn only_a_compiler_makes_ob_the_wrapper() {
        assert!(is_compiler("/root/.rustup/toolchains/nightly/bin/rustc"));
        assert!(is_compiler("/root/.rustup/toolchains/nightly/bin/clippy-driver"));
        assert!(!is_compiler("build"));
        assert!(!is_compiler("corpus-run"));
    }

    #[test]
    fn an_unconfigured_command_runs_the_wrapper_from_before_and_not_ob() {
        let mut cmd = Command::new("cargo");
        cmd.env("RUSTC_WRAPPER", "/checkout/target/debug/xtask")
            .env(MAPS, "/checkout\t/dukovo\n")
            .env(NEXT, "sccache");
        unconfigure(&mut cmd);
        let envs: Vec<_> = cmd.get_envs().collect();
        for name in [MAPS, NEXT] {
            assert!(envs.contains(&(std::ffi::OsStr::new(name), None)), "{name}");
        }
        let wrapper =
            envs.iter().find(|(name, _)| *name == "RUSTC_WRAPPER").map(|(_, value)| *value);
        let before = std::env::var_os("RUSTC_WRAPPER").filter(|next| !next.is_empty());
        assert_eq!(wrapper, Some(before.as_deref()));
    }

    /// An rlib as rustc writes one: a symbol table, the long names of its
    /// object files, the metadata, and the object files.
    fn rlib(names: &[&str]) -> Vec<u8> {
        fn member(name: &str, data: &[u8]) -> Vec<u8> {
            let mut out =
                format!("{name:<16}{:<12}{:<6}{:<6}{:<8}{:<10}`\n", 0, 0, 0, 644, data.len())
                    .into_bytes();
            out.extend_from_slice(data);
            if data.len() % 2 == 1 {
                out.push(b'\n');
            }
            out
        }
        let mut table = String::new();
        let mut offsets = Vec::new();
        for name in names {
            offsets.push(table.len());
            table.push_str(name);
            table.push_str("/\n");
        }
        let mut out = b"!<arch>\n".to_vec();
        out.extend(member("/", b"\0\0\0\x01\0\0\0\x88symbol\0"));
        out.extend(member("//", table.as_bytes()));
        out.extend(member("lib.rmeta/", b"rust metadata"));
        for offset in offsets {
            out.extend(member(&format!("/{offset}"), b"\x7fELF object"));
        }
        out
    }

    #[test]
    fn an_rlibs_object_files_are_named_by_the_crates_identity() {
        let names = [
            "nix-3e13aca479cc71b7.nix.26fc3ad416bc1ae0-cgu.0.rcgu.o",
            "nix-3e13aca479cc71b7.nix.26fc3ad416bc1ae0-cgu.1.rcgu.o",
        ];
        let before = rlib(&names);
        let mut after = before.clone();
        assert!(rename_members(&mut after, "-3e13aca479cc71b7", "-0123456789abcdef").unwrap());
        let renamed = names.map(|name| name.replace("3e13aca479cc71b7", "0123456789abcdef"));
        assert_eq!(after, rlib(&renamed.each_ref().map(String::as_str)));
        assert_eq!(after.len(), before.len());
        // A second time finds nothing to name.
        assert!(!rename_members(&mut after, "-3e13aca479cc71b7", "-0123456789abcdef").unwrap());
    }

    #[test]
    fn a_name_of_another_length_or_no_archive_is_left_alone() {
        let names = ["libc-28a9ed642aede354.libc.74b7a02a2394718a-cgu.0.rcgu.o"];
        let mut data = rlib(&names);
        assert!(!rename_members(&mut data, "-28a9ed642aede354", "-0123").unwrap());
        assert_eq!(data, rlib(&names));
        assert!(rename_members(&mut b"not an archive".to_vec(), "-aa", "-bb").is_err());
        let mut cut = rlib(&names);
        cut.truncate(cut.len() - 4);
        assert!(rename_members(&mut cut, "-28a9ed642aede354", "-0123456789abcdef").is_err());
    }

    #[test]
    fn a_library_built_whole_is_named_after_rustc_writes_it_and_a_check_or_program_is_not() {
        let base = temp("library");
        let root = base.join("dukovo");
        let root = root.to_str().unwrap();
        let args_with = |extra: &[&str]| {
            let mut args = compile(root, "1111222233334444", "/nowhere/libdep-aaaa.rlib", "std");
            args.extend(strings(extra));
            args
        };
        let mut args = args_with(&[]);
        let rlib = rewrite(&mut args, &maps(root, "/home/runner/.cargo"), &env(root)).unwrap();
        let wrote = |extra: &[&str]| rewrite(&mut args_with(extra), &[], &|_| None).unwrap();
        let linked = wrote(&["--emit=dep-info,metadata,link"]);
        let linked_to = wrote(&["--emit", "link=/elsewhere/out.rlib"]);
        let checked = wrote(&["--emit=dep-info,metadata"]);
        let mut program = args_with(&[]);
        let at = program.iter().position(|arg| arg == "lib").unwrap();
        program[at] = "bin".into();
        let program = rewrite(&mut program, &[], &|_| None).unwrap();
        let _ = std::fs::remove_dir_all(&base);

        let out = format!("{root}/target/release/build/app/1111222233334444/out");
        let rlib = rlib.expect("a library is an rlib");
        assert_eq!(rlib.path, PathBuf::from(format!("{out}/libapp-1111222233334444.rlib")));
        assert_eq!(rlib.cargo, "-1111222233334444");
        assert_eq!(rlib.ours, format!("-{}", metadata(&args)));
        assert!(linked.is_some());
        assert!(linked_to.is_some());
        assert_eq!(checked, None);
        assert_eq!(program, None);
    }

    #[test]
    fn naming_an_rlib_rewrites_it_whole_and_a_missing_one_is_no_error() {
        let dir = temp("rlib");
        let path = dir.join("libnix-3e13aca479cc71b7.rlib");
        let names = ["nix-3e13aca479cc71b7.nix.26fc3ad416bc1ae0-cgu.0.rcgu.o"];
        std::fs::write(&path, rlib(&names)).unwrap();
        let rlib_file = Rlib {
            path: path.clone(),
            cargo: "-3e13aca479cc71b7".into(),
            ours: "-0123456789abcdef".into(),
        };
        rlib_file.name_members().unwrap();
        let written = std::fs::read(&path).unwrap();
        let missing =
            Rlib { path: dir.join("libnone-aa.rlib"), cargo: "-aa".into(), ours: "-bb".into() };
        let missing = missing.name_members();
        let left: Vec<_> =
            std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name()).collect();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(written, rlib(&["nix-0123456789abcdef.nix.26fc3ad416bc1ae0-cgu.0.rcgu.o"]));
        assert!(missing.is_ok());
        assert_eq!(left.len(), 1, "no partial file is left: {left:?}");
    }
}

// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The paths a build keeps out of what it makes (ADR 0535).
//!
//! A compiler writes down where it read each source: in a panic's
//! location, in `__FILE__`, in an assertion's message and in debug
//! information. Where the checkout is, where Cargo keeps the crates and
//! where rustup keeps the standard library's sources differ from one
//! machine to the next, so a binary built on two machines would differ
//! in those strings alone. Every compiler `ob` runs is told to write the
//! same name for each of those directories, and the strings then agree.

use std::path::PathBuf;
use std::sync::OnceLock;

use crate::{Ctx, doctor};

/// What the checkout is called in what a build writes down.
pub const ROOT: &str = "/dukovo";

/// What a build directory outside the checkout is called, when
/// `OB_TARGET_DIR` puts it there.
pub const TARGET: &str = "/dukovo-target";

/// What Cargo's home, where it unpacks the crates it fetched, is called.
pub const CARGO_HOME: &str = "/cargo";

/// What the pinned toolchain's copy of Rust's sources, which `-Zbuild-std`
/// builds the core and alloc libraries from, is called.
pub const RUST_SRC: &str = "/rustc";

/// The directories a build reads that are not the same on another
/// machine, and the name each is written down under: shortest path
/// first, so that where one is inside another the more particular name
/// is the one a compiler takes (both rustc and GCC and Clang take the
/// last map that matches). A directory that is reached through a link is
/// named by the path it was given and by the one the link leads to, as
/// Cargo and the compilers write down either.
pub fn maps(ctx: &Ctx) -> Vec<(PathBuf, &'static str)> {
    let mut named = vec![(ctx.root.clone(), ROOT)];
    if !ctx.target.starts_with(&ctx.root) {
        named.push((ctx.target.clone(), TARGET));
    }
    if let Some(home) = cargo_home() {
        named.push((home, CARGO_HOME));
    }
    if let Some(src) = rust_src(ctx) {
        named.push((src, RUST_SRC));
    }
    let mut maps: Vec<(PathBuf, &'static str)> = Vec::new();
    for (dir, name) in named {
        let real = dir.canonicalize().ok().filter(|real| *real != dir);
        for dir in std::iter::once(dir).chain(real) {
            if !maps.iter().any(|(seen, _)| *seen == dir) {
                maps.push((dir, name));
            }
        }
    }
    maps.sort_by_key(|(from, _)| from.as_os_str().len());
    maps
}

/// Cargo's home: `$CARGO_HOME`, or `.cargo` in the home folder.
fn cargo_home() -> Option<PathBuf> {
    std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cargo")))
}

/// The pinned toolchain's copy of Rust's sources, if it has one: asked of
/// rustup once.
fn rust_src(ctx: &Ctx) -> Option<PathBuf> {
    static SOURCES: OnceLock<Option<PathBuf>> = OnceLock::new();
    SOURCES
        .get_or_init(|| doctor::sysroot(ctx).map(|sysroot| sysroot.join("lib/rustlib/src/rust")))
        .clone()
}

/// The flags that make `rustc` write [`maps`]' names: one
/// `--remap-path-prefix` each.
pub fn rustc_flags(ctx: &Ctx) -> Vec<String> {
    maps(ctx)
        .into_iter()
        .map(|(from, to)| format!("--remap-path-prefix={}={to}", from.display()))
        .collect()
}

/// The flags that make a C compiler write the names of the checkout and
/// the build directory in the place of their paths: one
/// `-ffile-prefix-map` each. Clang and GCC take it for `__FILE__`,
/// assertions and debug information alike. The crates' and the Rust
/// sources' directories have no C in them.
pub fn cc_flags(ctx: &Ctx) -> Vec<String> {
    maps(ctx)
        .into_iter()
        .filter(|(_, to)| matches!(*to, ROOT | TARGET))
        .map(|(from, to)| format!("-ffile-prefix-map={}={to}", from.display()))
        .collect()
}

/// `items` as a TOML array of strings, for a `--config` argument.
pub fn toml_array(items: &[String]) -> String {
    let quoted: Vec<String> = items
        .iter()
        .map(|item| format!("\"{}\"", item.replace('\\', "\\\\").replace('"', "\\\"")))
        .collect();
    format!("[{}]", quoted.join(","))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(root: &str, target: &str) -> Ctx {
        Ctx {
            root: root.into(),
            out: format!("{root}/out").into(),
            target: target.into(),
            config: crate::config::Config::default(),
            verbose: false,
        }
    }

    #[test]
    fn the_checkout_is_written_down_under_one_name_wherever_it_is() {
        let (a, b) = (
            ctx("/home/runner/work/dukovo/dukovo", "/home/runner/work/dukovo/dukovo/target"),
            ctx("/tmp/other/dukovo", "/tmp/other/dukovo/target"),
        );
        let names = |ctx: &Ctx| -> Vec<String> {
            cc_flags(ctx).iter().map(|flag| flag.rsplit_once('=').unwrap().1.to_string()).collect()
        };
        assert_eq!(names(&a), [ROOT]);
        assert_eq!(names(&a), names(&b));
        assert_eq!(
            cc_flags(&a),
            ["-ffile-prefix-map=/home/runner/work/dukovo/dukovo=/dukovo".to_string()]
        );
        let flags = rustc_flags(&b);
        assert!(flags.contains(&"--remap-path-prefix=/tmp/other/dukovo=/dukovo".to_string()));
    }

    #[test]
    fn a_build_directory_outside_the_checkout_has_a_name_of_its_own() {
        let ctx = ctx("/work/dukovo", "/scratch/target");
        let flags = cc_flags(&ctx);
        assert_eq!(flags.len(), 2);
        assert!(flags.contains(&"-ffile-prefix-map=/scratch/target=/dukovo-target".to_string()));
        assert!(flags.contains(&"-ffile-prefix-map=/work/dukovo=/dukovo".to_string()));
    }

    #[test]
    fn the_more_particular_directory_comes_last() {
        let ctx = ctx("/work/dukovo", "/work/dukovo/target");
        let maps = maps(&ctx);
        let lengths: Vec<usize> = maps.iter().map(|(from, _)| from.as_os_str().len()).collect();
        assert!(lengths.windows(2).all(|pair| pair[0] <= pair[1]), "{maps:?}");
    }

    #[test]
    fn a_directory_reached_through_a_link_is_named_under_both_paths() {
        let dir = std::env::temp_dir().join(format!("ob-repro-link-{}", std::process::id()));
        let real = dir.join("real");
        std::fs::create_dir_all(&real).unwrap();
        let link = dir.join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let ctx = ctx(link.to_str().unwrap(), link.join("target").to_str().unwrap());
        let flags = cc_flags(&ctx);
        let real = real.canonicalize().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(flags.contains(&format!("-ffile-prefix-map={}=/dukovo", link.display())));
        assert!(flags.contains(&format!("-ffile-prefix-map={}=/dukovo", real.display())));
    }

    #[test]
    fn an_array_for_cargo_escapes_what_toml_does() {
        let items = ["--cfg".to_string(), "a\"b\\c".to_string()];
        assert_eq!(toml_array(&items), r#"["--cfg","a\"b\\c"]"#);
        let parsed: toml::Value = toml::from_str(&format!("x = {}", toml_array(&items))).unwrap();
        assert_eq!(parsed["x"][1].as_str(), Some("a\"b\\c"));
    }
}

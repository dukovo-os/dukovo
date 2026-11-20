// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Programs in Go for Radix, built with the Go toolchain the `go` port
//! installs into `out/toolchain/go` (ob(1), PORTS).
//!
//! [`go_command`] gives the `go` command with the environment every build
//! of ob uses and nothing else from the build machine's: Go's compiler
//! reads a dozen settings that change what it builds, from the target to
//! the flags to the proxy a module comes from, and none of them is left to
//! the person who runs `./ob`. The build cache and the module folders are
//! in `target/go`, so that the standard library for Radix is compiled once
//! for every program built after it.
//!
//! [`build_tests`] builds what the `go-runtime` boot test runs: the Go test
//! program of `userspace/go`, and the test binaries of 22 packages of
//! Go's standard library, each in a folder with the package's files and test
//! data, which its tests read.
//! `userspace/go/skips.txt` names the tests of those packages that Radix cannot
//! pass yet, each with the feature it lacks ([`parse_skips`]).

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::ports::{self, Toolchain};
use crate::process::{self, step};
use crate::{Context, Ctx, Error, Result};

/// The module of the Go test programs, in the repository.
const TEST_MODULE: &str = "userspace/go";

/// The program of that module the `go-runtime` boot test runs, which is
/// also the name of its binary.
pub const RUNTIME_TEST: &str = "go-runtime-test";

/// The packages of Go's standard library whose tests the `go-runtime` boot
/// test runs, as import paths: the runtime and what it carries, the
/// operating system's side of the library, packages whose tests work files,
/// text and numbers hard, and a hash and an encoding.
pub const TEST_PACKAGES: [&str; 22] = [
    "runtime",
    "sync",
    "sync/atomic",
    "time",
    "os",
    "os/exec",
    "os/signal",
    "os/user",
    "syscall",
    "internal/poll",
    "io",
    "io/fs",
    "path/filepath",
    "strconv",
    "strings",
    "bytes",
    "sort",
    "errors",
    "context",
    "math/big",
    "crypto/sha256",
    "encoding/json",
];

/// The folder of the toolchain's Go tree.
fn go_root(toolchain: &Toolchain) -> PathBuf {
    toolchain.dir.join("go")
}

/// The `go` command of the toolchain, for building programs for Radix,
/// with the environment of ob's Go builds and nothing inherited from the
/// build machine's. Callers choose the folder it runs in.
pub fn go_command(ctx: &Ctx, toolchain: &Toolchain) -> Command {
    go_in(&toolchain.dir, &ctx.target)
}

/// The `go` command of the toolchain in `toolchain`, with its build cache
/// and module folders in `go` of `target`.
fn go_in(toolchain: &Path, target: &Path) -> Command {
    let root = toolchain.join("go");
    command(&root.join("bin/go"), &root, &target.join("go"))
}

/// `program` with the environment of a Go build in the Go tree `root`,
/// keeping its caches in `caches`.
fn command(program: &Path, root: &Path, caches: &Path) -> Command {
    let mut cmd = Command::new(program);
    cmd.env_clear().envs(environment(root, caches));
    cmd
}

/// The environment of a build: for Radix, on the amd64 baseline, without
/// cgo, with the toolchain that is there and no other, nothing sent or
/// fetched, nothing read from the build machine's configuration and no
/// path or time of the build left in what it makes.
fn environment(root: &Path, caches: &Path) -> Vec<(&'static str, OsString)> {
    let path = |name: &str| caches.join(name).into_os_string();
    vec![
        ("GOOS", "radix".into()),
        ("GOARCH", "amd64".into()),
        ("GOAMD64", "v1".into()),
        ("CGO_ENABLED", "0".into()),
        ("GOROOT", root.as_os_str().to_owned()),
        ("GOTOOLCHAIN", "local".into()),
        ("GOTELEMETRY", "off".into()),
        ("GOENV", "off".into()),
        ("GOFLAGS", "-trimpath -buildvcs=false".into()),
        ("GOPROXY", "off".into()),
        ("GOSUMDB", "off".into()),
        ("GOWORK", "off".into()),
        ("GOCACHE", path("cache")),
        ("GOMODCACHE", path("mod")),
        ("GOPATH", path("path")),
    ]
}

/// Builds the programs the `go-runtime` boot test runs into `dir`, which is
/// made afresh and laid out as a Go tree is: `go-runtime-test` in it, for
/// each of [`TEST_PACKAGES`] a folder `src/<path>` with its test binary,
/// `<last name>.test`, the package's own files and its `testdata`, where the
/// binary reads them from, and Go's time zone database in `lib/time`, which
/// the time package's tests read from there.
pub fn build_tests(ctx: &Ctx, toolchain: &Toolchain, dir: &Path) -> Result<()> {
    let root = go_root(toolchain);
    if !root.join("bin/go").is_file() {
        return Err(Error::new(format!("the Go toolchain is not in {}", ctx.display(&root)))
            .hint("run `./ob ports`, which builds it"));
    }
    if dir.exists() {
        std::fs::remove_dir_all(dir).context(|| format!("removing {}", dir.display()))?;
    }
    process::create_dir(dir)?;
    step("Building the Go test programs");
    let module = ctx.root.join(TEST_MODULE);
    let mut build = go_command(ctx, toolchain);
    build
        .current_dir(&module)
        .args(["build", "-o"])
        .arg(dir.join(RUNTIME_TEST))
        .arg(format!("./cmd/{RUNTIME_TEST}"));
    process::run(ctx, &mut build)?;
    for package in TEST_PACKAGES {
        let folder = dir.join("src").join(package);
        process::create_dir(&folder)?;
        let name = package.rsplit('/').next().unwrap_or(package);
        // From the test module's folder, as that is a module; the package
        // is the standard library's.
        let mut test = go_command(ctx, toolchain);
        test.current_dir(&module)
            .args(["test", "-c", "-o"])
            .arg(folder.join(format!("{name}.test")))
            .arg(package);
        process::run(ctx, &mut test)?;
        // A test runs in its package's folder, as `go test` runs it, and
        // some read their own source files there: the folder holds the
        // package's files, and its test data.
        let source = root.join("src").join(package);
        let files =
            std::fs::read_dir(&source).context(|| format!("reading {}", source.display()))?;
        for entry in files {
            let path = entry.context(|| format!("reading {}", source.display()))?.path();
            if path.is_file() {
                process::copy(&path, &folder.join(path.file_name().unwrap_or_default()))?;
            }
        }
        let testdata = source.join("testdata");
        if testdata.is_dir() {
            ports::copy_tree(ctx, &testdata, &folder.join("testdata"))?;
        }
    }
    let zones = dir.join("lib/time");
    process::create_dir(&zones)?;
    process::copy(&root.join("lib/time/zoneinfo.zip"), &zones.join("zoneinfo.zip"))
}

/// A test the `go-runtime` boot test does not run, with the reason.
#[derive(Debug, PartialEq, Eq)]
pub struct Skip {
    /// The package's import path, one of [`TEST_PACKAGES`].
    pub package: String,
    /// The test's name, or the path of one of its subtests, `Test/sub`, as
    /// `go test -run` takes it. A name, at any level, that ends in `*` is
    /// every name that starts with what comes before it.
    pub test: String,
    /// The feature Radix lacks, and the change that brings it.
    pub reason: String,
}

/// Reads the skip file, `userspace/go/skips.txt`.
pub fn skips(ctx: &Ctx) -> Result<Vec<Skip>> {
    let file = ctx.root.join("userspace/go/skips.txt");
    let text = std::fs::read_to_string(&file).context(|| format!("reading {}", file.display()))?;
    parse_skips(&text).map_err(|err| Error::new(format!("{}: {err}", ctx.display(&file))))
}

/// Reads the text of a skip file: a line each of the package, the test and
/// the reason, set apart by spaces, and blank lines and `#` comments.
/// Nothing is skipped without a reason, in a package the boot test does not
/// run, or twice.
pub fn parse_skips(text: &str) -> std::result::Result<Vec<Skip>, String> {
    let mut skips: Vec<Skip> = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let (at, line) = (i + 1, line.trim());
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (package, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
        let rest = rest.trim_start();
        let (test, reason) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
        let reason = reason.trim();
        if test.is_empty() || reason.is_empty() {
            return Err(format!(
                "line {at}: a skip is a package, a test and the reason, which names the feature \
                 Radix lacks and the change that brings it"
            ));
        }
        if !TEST_PACKAGES.contains(&package) {
            return Err(format!("line {at}: {package} is not a package the boot test runs"));
        }
        if !test.split('/').all(is_test_name) {
            return Err(format!("line {at}: `{test}` is not the name of a test or a subtest"));
        }
        if skips.iter().any(|skip| skip.package == package && skip.test == test) {
            return Err(format!("line {at}: {package} {test} is skipped twice"));
        }
        skips.push(Skip { package: package.into(), test: test.into(), reason: reason.into() });
    }
    Ok(skips)
}

/// Whether `level`, a test's name or a subtest's, is one a skip may name:
/// letters, digits, `_` and `-`, as Go writes a subtest's name with `_` for
/// each space, and `*` for any run of them, but not only `*`.
fn is_test_name(level: &str) -> bool {
    level.bytes().any(|b| b != b'*')
        && level.bytes().all(|b| b.is_ascii_alphanumeric() || b"_-*".contains(&b))
}

/// The pattern for `-test.skip` that skips the tests `skips` names in
/// `package`, if it has any. Each skip is an alternative, `^Test$` or
/// `^Test$/^sub$`, with `.*` for each `*`. Go's testing package takes the
/// first alternative that matches a test, and an alternative with more
/// levels than the test only matches it in part, which does not skip it;
/// the alternatives with fewer levels come first, so that a test one skip
/// names whole is not kept by another that names its subtests.
pub fn skip_pattern(skips: &[Skip], package: &str) -> Option<String> {
    let mut tests: Vec<&str> = skips
        .iter()
        .filter(|skip| skip.package == package)
        .map(|skip| skip.test.as_str())
        .collect();
    tests.sort_by_key(|test| test.split('/').count());
    let alternative = |test: &str| -> String {
        let levels = test.split('/').map(|level| format!("^{}$", level.replace('*', ".*")));
        levels.collect::<Vec<_>>().join("/")
    };
    (!tests.is_empty())
        .then(|| tests.iter().map(|test| alternative(test)).collect::<Vec<_>>().join("|"))
}

/// The skip file as the boot test's script reads it from the test disk: a
/// line of the package and its pattern for each package that has one.
pub fn skip_list(skips: &[Skip]) -> String {
    TEST_PACKAGES
        .iter()
        .filter_map(|package| Some(format!("{package} {}\n", skip_pattern(skips, package)?)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(output: &[u8]) -> Vec<String> {
        let mut lines: Vec<String> =
            String::from_utf8_lossy(output).lines().map(str::to_string).collect();
        lines.sort();
        lines
    }

    #[test]
    fn a_go_build_has_the_environment_ob_gives_and_nothing_else() {
        // `env` prints what it was given. This test runs with the build
        // machine's variables, PATH and HOME among them, and none of them
        // is there.
        assert!(std::env::var_os("PATH").is_some() && std::env::var_os("HOME").is_some());
        let printed = command(Path::new("env"), Path::new("/o/toolchain/go"), Path::new("/t/go"))
            .output()
            .unwrap();
        assert!(printed.status.success());
        assert_eq!(
            lines(&printed.stdout),
            [
                "CGO_ENABLED=0",
                "GOAMD64=v1",
                "GOARCH=amd64",
                "GOCACHE=/t/go/cache",
                "GOENV=off",
                "GOFLAGS=-trimpath -buildvcs=false",
                "GOMODCACHE=/t/go/mod",
                "GOOS=radix",
                "GOPATH=/t/go/path",
                "GOPROXY=off",
                "GOROOT=/o/toolchain/go",
                "GOSUMDB=off",
                "GOTELEMETRY=off",
                "GOTOOLCHAIN=local",
                "GOWORK=off",
            ]
        );
    }

    #[test]
    fn the_go_command_is_the_toolchains_and_keeps_its_caches_in_target() {
        let cmd = go_in(Path::new("/repo/out/toolchain"), Path::new("/repo/target"));
        assert_eq!(cmd.get_program(), "/repo/out/toolchain/go/bin/go");
        let env = |name: &str| {
            cmd.get_envs().find(|(var, _)| *var == name).and_then(|(_, value)| value).unwrap()
        };
        assert_eq!(env("GOROOT"), "/repo/out/toolchain/go");
        assert_eq!(env("GOCACHE"), "/repo/target/go/cache");
        assert_eq!(env("GOMODCACHE"), "/repo/target/go/mod");
        assert_eq!(env("GOPATH"), "/repo/target/go/path");
    }

    #[test]
    fn the_test_packages_are_standard_ones_and_each_is_listed_once() {
        let mut sorted = TEST_PACKAGES;
        sorted.sort_unstable();
        assert!(sorted.windows(2).all(|pair| pair[0] != pair[1]), "a package is listed twice");
        for package in TEST_PACKAGES {
            // Import paths of the standard library: the first element has
            // no dot, and nothing leaves the folder it is joined to.
            assert!(!package.split('/').next().unwrap().contains('.'), "{package}");
            assert!(!package.starts_with('/') && !package.contains(".."), "{package}");
            assert!(!package.is_empty() && !package.ends_with('/'), "{package}");
        }
    }

    #[test]
    fn skips_are_read_with_their_reasons() {
        let text = "# A comment.\n\n\
                    os TestExecutable   no /proc/self/exe yet (PR 2)\n\
                    \tos/exec TestA the kernel has no pidfd (PR 4)\n\
                    os TestSetuid needs setuid\n";
        let skips = parse_skips(text).unwrap();
        assert_eq!(skips.len(), 3);
        assert_eq!(
            skips[0],
            Skip {
                package: "os".into(),
                test: "TestExecutable".into(),
                reason: "no /proc/self/exe yet (PR 2)".into()
            }
        );
        assert_eq!(skips[1].package, "os/exec");
        assert_eq!(skip_pattern(&skips, "os").as_deref(), Some("^TestExecutable$|^TestSetuid$"));
        assert_eq!(skip_pattern(&skips, "os/exec").as_deref(), Some("^TestA$"));
        assert_eq!(skip_pattern(&skips, "strings"), None);
        assert_eq!(skip_list(&skips), "os ^TestExecutable$|^TestSetuid$\nos/exec ^TestA$\n");
        assert_eq!(parse_skips("# nothing\n\n").unwrap(), []);
        assert_eq!(skip_list(&[]), "");
    }

    #[test]
    fn a_skip_needs_a_reason_a_package_that_runs_and_a_test_that_is_named() {
        for (text, expected) in [
            ("os TestExecutable\n", "line 1: a skip is a package, a test and the reason"),
            ("os\n", "line 1: a skip is a package, a test and the reason"),
            ("os TestExecutable   \n", "line 1: a skip is a package, a test and the reason"),
            ("# c\nnet TestDial no network\n", "line 2: net is not a package the boot test runs"),
            ("os Test.* all of them\n", "line 1: `Test.*` is not the name of a test"),
            ("os Test|Other why\n", "line 1: `Test|Other` is not the name of a test"),
            ("os TestA/ why\n", "line 1: `TestA/` is not the name of a test"),
            ("os * all of them\n", "line 1: `*` is not the name of a test"),
            ("os TestA/** every subtest\n", "line 1: `TestA/**` is not the name of a test"),
            ("os TestA/b.c why\n", "line 1: `TestA/b.c` is not the name of a test"),
            ("os TestA why\nos TestA why again\n", "line 2: os TestA is skipped twice"),
        ] {
            let err = parse_skips(text).unwrap_err();
            assert!(err.contains(expected), "`{text}` gave `{err}`, not `{expected}`");
        }
    }

    #[test]
    fn a_skip_names_subtests_and_names_that_start_alike() {
        let text = "os TestRoot*/file_slash a slash after a file's name\n\
                    os TestRoot*/*-to_dir* a slash after a link's name\n\
                    os TestA no A\n\
                    os Test*B no B\n";
        let skips = parse_skips(text).unwrap();
        assert_eq!(skips[1].test, "TestRoot*/*-to_dir*");
        // The skips of whole tests come first, then the subtests', each in
        // the order the file has them.
        assert_eq!(
            skip_pattern(&skips, "os").as_deref(),
            Some("^TestA$|^Test.*B$|^TestRoot.*$/^file_slash$|^TestRoot.*$/^.*-to_dir.*$")
        );
        let err = parse_skips("os TestA/b why\nos TestA/b again\n").unwrap_err();
        assert!(err.contains("line 2: os TestA/b is skipped twice"), "{err}");
    }

    #[test]
    fn the_skip_file_of_the_repository_is_read() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let ctx = Ctx {
            out: root.join("out"),
            target: root.join("target"),
            root,
            config: crate::config::Config::default(),
            verbose: false,
        };
        skips(&ctx).unwrap();
    }
}

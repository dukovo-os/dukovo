// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The servers of the Linux kit: nginx, PHP, Valkey and PostgreSQL built
//! for Linux, for the server benchmarks that hold Dukovo to Linux
//! (`serverbench`, userspace/posix/serverbench.c).
//!
//! Each is the release and commit of its port's recipe, fetched with
//! [`ports::fetch`], built as upstream builds it for Linux with the host's
//! Clang at `-O2`, without the port's patches (they adapt it to Radix) and
//! with only what the benchmarks use. The prefix is the bundle's, so the
//! paths the benchmarks use are the same on both systems. A server's build
//! is kept in `target/bench/linux-servers/<bundle id>` with a stamp of
//! what it was built from, and is not made again while the stamp holds.
//!
//! The programs are dynamically linked: PostgreSQL loads PL/pgSQL with
//! `dlopen`. The tree they are put in therefore holds the host's C
//! library and whatever else `ldd` lists for any of its files, at the
//! same paths, a shell, and the account files that `initdb` needs. The
//! servers' own files are stripped of the symbols nothing links against,
//! as distributions ship them. The tree becomes an initramfs part that
//! follows the one of [`bench::initramfs`] ([`linux_servers`]);
//! linux-bench runs `/server-bench` from it.
//!
//! When a build fails, read the output above the error: it is the
//! server's own `configure` or `make`. To try a tree by hand, run
//! `chroot target/bench/linux-servers/tree /applications/<id>/bin/<program> --version`
//! as root, with `/tmp` made first.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::ports::{self, Recipe};
use crate::process::{self, step};
use crate::{Context as _, Ctx, Error, Result, bail, bench, kernel};

/// Bumped when a server's flags change, so that a build kept from the
/// flags before is made again.
const SERVER_KIT_VERSION: u32 = 1;

/// The program that runs the benchmarks, in the posix tests' folder: the
/// image holds it too, built for Radix.
const SERVERBENCH_SOURCE: &str = "userspace/posix/serverbench.c";

/// The inode number of the tree's first entry. The initramfs part that
/// precedes the tree's numbers its entries from 1 to well under this.
const FIRST_INO: u32 = 1000;

/// The account files of the tree: root and `bench` with uid and gid 1000,
/// which `initdb` looks up because it refuses to run as root, and the
/// name service switch that reads them from these files alone.
const ETC_FILES: [(&str, &str); 3] = [
    ("etc/passwd", "root:x:0:0:root:/root:/bin/sh\nbench:x:1000:1000:bench:/home/bench:/bin/sh\n"),
    ("etc/group", "root:x:0:\nbench:x:1000:\n"),
    ("etc/nsswitch.conf", "passwd: files\ngroup: files\n"),
];

/// What makes one server: it builds from a [`Build`] and puts its
/// programs in the stage.
type Builder = fn(&Build) -> Result<()>;

/// What one server's build is given.
struct Build<'a> {
    ctx: &'a Ctx,
    compiler: &'a Path,
    recipe: &'a Recipe,
    /// The recipe's extra sources, fetched, in its order.
    extras: &'a [PathBuf],
    /// A copy of the source, which the build works in.
    src: &'a Path,
    /// Where the files of the installation are put, at their final paths
    /// (`<stage>/applications/<id>/bin/<program>`).
    stage: &'a Path,
    jobs: usize,
}

impl Build<'_> {
    /// The folder the bundle is installed in, which the build is given as
    /// its prefix.
    fn prefix(&self) -> String {
        format!("/applications/{}", self.bundle_id())
    }

    fn bundle_id(&self) -> &str {
        self.recipe.bundle.as_ref().map_or(self.recipe.name.as_str(), |bundle| bundle.id.as_str())
    }

    /// Runs `command` in the copy of the source.
    fn run(&self, command: &mut Command) -> Result<()> {
        process::run(self.ctx, command.current_dir(self.src))
    }

    /// Runs `make` in `folder`, beneath the copy of the source, with
    /// `arguments` and as many jobs as the machine has CPUs.
    fn make(&self, folder: &str, arguments: &[&str]) -> Result<()> {
        let mut command = Command::new("make");
        command.arg("-C").arg(self.src.join(folder)).arg(format!("-j{}", self.jobs));
        self.run(command.args(arguments))
    }

    /// Puts `file` at `to` beneath the bundle's prefix in the stage, with
    /// its permissions and whatever it is a link to.
    fn install(&self, file: &Path, to: &str) -> Result<()> {
        let at = self.stage.join(self.prefix().trim_start_matches('/')).join(to);
        process::copy(file, &at)
    }
}

/// Builds the four servers for Linux, as [`built`] keeps them, and gives
/// an initramfs part, written to `servers.cpio` in `dir`: a "newc" cpio
/// archive, which Linux unpacks after the first, holding them under
/// `/applications`, the libraries they load, `/bin/sh`, `/etc`'s account
/// files, and `/server-bench`, the benchmark program built for Linux.
///
/// # Errors
///
/// Fails when a server cannot be fetched or built, when no C compiler is
/// installed, or when a library its programs need cannot be found.
pub fn linux_servers(ctx: &Ctx, dir: &Path) -> Result<PathBuf> {
    let compiler = ports::find_clang()
        .map(|(clang, _)| clang)
        .or_else(|| process::find_program("cc"))
        .ok_or_else(|| {
            Error::new("no C compiler is installed, for the servers on Linux")
                .hint("run `./ob doctor`")
        })?;
    let work = ctx.target.join("bench/linux-servers");
    let tree = work.join("tree");
    if tree.exists() {
        std::fs::remove_dir_all(&tree).context(|| format!("removing {}", tree.display()))?;
    }
    process::create_dir(&tree)?;
    let builds: [(&str, Builder); 4] =
        [("nginx", nginx), ("valkey", valkey), ("postgresql", postgresql), ("php", php)];
    for (port, build) in builds {
        let stage = built(ctx, &compiler, port, build)?;
        ports::copy_tree(ctx, &stage, &tree)?;
    }
    strip_elf_files(ctx, &tree)?;
    for (name, text) in ETC_FILES {
        let file = tree.join(name);
        process::create_dir(file.parent().unwrap_or(&tree))?;
        std::fs::write(&file, text).context(|| format!("writing {}", file.display()))?;
    }
    // initdb finds and starts postgres through popen, which runs /bin/sh.
    put_host_file(&tree, Path::new("/bin/sh"))?;
    add_host_libraries(ctx, &tree)?;
    serverbench_for_linux(ctx, &compiler, &tree.join("server-bench"))?;
    let part = dir.join("servers.cpio");
    std::fs::write(&part, bench::newc_tree(&tree, FIRST_INO)?)
        .context(|| format!("writing {}", part.display()))?;
    Ok(part)
}

/// The installation of the port `port` built for Linux, as a stage folder
/// (see [`Build::stage`]): the one kept from before when its stamp names
/// the commits and [`SERVER_KIT_VERSION`] that this build has.
fn built(ctx: &Ctx, compiler: &Path, port: &str, build_it: Builder) -> Result<PathBuf> {
    let Some((recipe, _)) =
        ports::recipes(ctx)?.into_iter().find(|(recipe, _)| recipe.name == port)
    else {
        bail!("there is no port {port}, which the Linux kit builds a server from");
    };
    let source = &recipe.source;
    let tree = ports::fetch(ctx, &recipe.name, &source.git, &source.commit, &source.paths)?;
    let mut extras = Vec::new();
    let mut stamp = format!("{SERVER_KIT_VERSION}\n{}\n{}\n", source.commit, compiler.display());
    for extra in &recipe.extra {
        let name = extra.path.rsplit('/').next().unwrap_or(&extra.path);
        extras.push(ports::fetch(ctx, name, &extra.git, &extra.commit, &[])?);
        stamp.push_str(&format!("{}\n", extra.commit));
    }
    let folder = ctx.target.join("bench/linux-servers").join(&recipe.name);
    let (src, stage) = (folder.join("src"), folder.join("stage"));
    let stamp_file = folder.join("built-from");
    if stage.is_dir() && std::fs::read_to_string(&stamp_file).ok().as_deref() == Some(&stamp) {
        return Ok(stage);
    }
    step(&format!("Building {} for Linux, for the server benchmarks", recipe.name));
    if folder.exists() {
        std::fs::remove_dir_all(&folder).context(|| format!("removing {}", folder.display()))?;
    }
    ports::copy_tree(ctx, &tree, &src)?;
    let jobs = std::thread::available_parallelism().map_or(2, std::num::NonZero::get);
    let build =
        Build { ctx, compiler, recipe: &recipe, extras: &extras, src: &src, stage: &stage, jobs };
    build_it(&build)?;
    std::fs::write(&stamp_file, &stamp).context(|| format!("writing {}", stamp_file.display()))?;
    Ok(stage)
}

/// nginx without OpenSSL, PCRE and zlib, which the benchmark's
/// configuration does not use (the port has them).
fn nginx(build: &Build) -> Result<()> {
    let mut configure = Command::new("auto/configure");
    configure
        .arg(format!("--prefix={}", build.prefix()))
        .arg(format!("--with-cc={}", build.compiler.display()))
        .args(["--with-cc-opt=-O2 -D_GNU_SOURCE", "--without-http_rewrite_module"])
        .args(["--without-http_gzip_module", "--without-pcre"]);
    build.run(&mut configure)?;
    build.make(".", &[])?;
    build.install(&build.src.join("objs/nginx"), "bin/nginx")
}

/// Valkey with the C library's allocator, as the port has it, and no TLS.
fn valkey(build: &Build) -> Result<()> {
    let compiler = format!("CC={}", build.compiler.display());
    let programs = ["valkey-server", "valkey-benchmark"];
    let mut arguments =
        vec!["MALLOC=libc", "BUILD_TLS=no", "USE_SYSTEMD=no", "OPTIMIZATION=-O2", &compiler];
    arguments.extend(programs);
    build.make("src", &arguments)?;
    for program in programs {
        build.install(&build.src.join("src").join(program), &format!("bin/{program}"))?;
    }
    Ok(())
}

/// PostgreSQL with none of the optional libraries: the server, `initdb`,
/// `pgbench`, `pg_isready` and what the server loads (PL/pgSQL and the
/// encoding conversions), with its share files.
fn postgresql(build: &Build) -> Result<()> {
    let mut configure = Command::new("./configure");
    configure
        .arg(format!("--prefix={}", build.prefix()))
        .args(["--without-icu", "--without-readline", "--without-zlib", "--without-openssl"])
        .args(["--without-lz4", "--without-zstd", "--without-libxml", "--without-llvm"])
        .args(["--without-perl", "--without-python", "--without-tcl", "--disable-nls"])
        .arg(format!("CC={}", build.compiler.display()))
        .arg("CFLAGS=-O2");
    build.run(&mut configure)?;
    build.make(".", &[])?;
    let installed = build.src.join("installed");
    build.make(".", &[&format!("DESTDIR={}", installed.display()), "install"])?;
    let from = installed.join(build.prefix().trim_start_matches('/'));
    for program in ["initdb", "postgres", "pgbench", "pg_isready"] {
        build.install(&from.join("bin").join(program), &format!("bin/{program}"))?;
    }
    // The client library of pgbench and pg_isready, by the name they load.
    build.install(&from.join("lib/libpq.so.5"), "lib/libpq.so.5")?;
    // The modules the server loads sit beside the libraries, not in a
    // folder of their own: PostgreSQL adds `postgresql` to its folders'
    // names only when the prefix lacks it, and the bundle's has it.
    let libraries = from.join("lib");
    for entry in
        std::fs::read_dir(&libraries).context(|| format!("reading {}", libraries.display()))?
    {
        let path = entry.context(|| format!("reading {}", libraries.display()))?.path();
        let name = path.file_name().map_or_default(|name| name.to_string_lossy());
        if name.ends_with(".so") && path.is_file() && !path.is_symlink() {
            build.install(&path, &format!("lib/{name}"))?;
        }
    }
    let share = build.stage.join(build.prefix().trim_start_matches('/')).join("share");
    ports::copy_tree(build.ctx, &from.join("share"), &share)?;
    Ok(())
}

/// PHP's command-line program and PHP-FPM with no extension beyond what
/// `--disable-all` leaves, which hold what the benchmarks run. PHP's
/// source is a git tree, so its scanners are made by re2c, built from the
/// recipe's extra source for this machine as the port's build.sh builds
/// it, and its configure by `buildconf`.
fn php(build: &Build) -> Result<()> {
    let re2c_source =
        build.extras.first().ok_or_else(|| Error::new("the php recipe names no re2c"))?;
    let re2c_build = build.src.join("re2c-build");
    let mut cmake = Command::new("cmake");
    cmake
        .arg("-S")
        .arg(re2c_source)
        .arg("-B")
        .arg(&re2c_build)
        .args(["-DCMAKE_BUILD_TYPE=Release", "-DRE2C_BUILD_LIBS=OFF", "-DRE2C_BUILD_TESTS=OFF"])
        .args(["-DRE2C_BUILD_BENCHMARKS=OFF"])
        .args(RE2C_OTHER_LANGUAGES.map(|language| format!("-DRE2C_BUILD_{language}=OFF")))
        .env("CC", "cc")
        .env("CXX", "c++");
    build.run(&mut cmake)?;
    let mut compile = Command::new("cmake");
    compile
        .arg("--build")
        .arg(&re2c_build)
        .arg(format!("-j{}", build.jobs))
        .args(["--target", "re2c"]);
    build.run(&mut compile)?;
    build.run(Command::new("./buildconf").arg("--force"))?;
    let mut configure = Command::new("./configure");
    configure
        .arg(format!("--prefix={}", build.prefix()))
        .args(["--disable-all", "--enable-cli", "--enable-fpm", "--disable-cgi"])
        .args(["--disable-phpdbg", "--without-pear", "--disable-debug"])
        .args(["--without-pcre-jit", "--disable-opcache-jit"])
        .arg(format!("CC={}", build.compiler.display()))
        .arg("CFLAGS=-O2")
        .arg(format!("RE2C={}", re2c_build.join("re2c").display()));
    build.run(&mut configure)?;
    build.make(".", &["cli", "fpm"])?;
    build.install(&build.src.join("sapi/cli/php"), "bin/php")?;
    build.install(&build.src.join("sapi/fpm/php-fpm"), "bin/php-fpm")
}

/// re2c's other languages' programs, which PHP's scanners do not need.
const RE2C_OTHER_LANGUAGES: [&str; 11] = [
    "RE2D", "RE2GO", "RE2HS", "RE2JAVA", "RE2JS", "RE2OCAML", "RE2PY", "RE2RUST", "RE2SWIFT",
    "RE2V", "RE2ZIG",
];

/// Puts into `tree`, at the paths `ldd` gives for them, the host's
/// libraries that any ELF file of the tree loads, and the dynamic loader
/// its programs name: each a regular file with the bytes of the library
/// the path leads to, since the tree holds no links.
///
/// A library the tree holds already is not copied. The libraries of a
/// server's own folders are found through `LD_LIBRARY_PATH`, since the
/// `RPATH` of its programs names a folder that exists only inside the
/// tree. `ldd` may run the file it is given, so it is given only files
/// that the builds made.
fn add_host_libraries(ctx: &Ctx, tree: &Path) -> Result<()> {
    let mut elf_files = Vec::new();
    find_elf_files(tree, &mut elf_files)?;
    let mut search: Vec<&Path> = elf_files.iter().filter_map(|file| file.parent()).collect();
    search.dedup();
    let search = std::env::join_paths(search)
        .map_err(|err| Error::new(format!("the tree's folders cannot be searched: {err}")))?;
    for file in &elf_files {
        let mut ldd = Command::new("ldd");
        ldd.arg(file).env("LD_LIBRARY_PATH", &search);
        if ctx.verbose {
            eprintln!("+ {}", process::render(&ldd));
        }
        let output = ldd.output().map_err(|err| process::spawn_error("ldd", &err))?;
        let (listing, complaint) =
            (String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        if !output.status.success() && complaint.contains("not a dynamic executable") {
            continue;
        }
        if !output.status.success() {
            bail!("`ldd {}` failed ({}): {}", file.display(), output.status, complaint.trim_end());
        }
        for library in ldd_paths(&listing).map_err(|why| {
            Error::new(format!("{} needs {why}", file.display())).hint("install the library")
        })? {
            if library.starts_with(tree) {
                continue;
            }
            put_host_file(tree, &library)?;
        }
    }
    Ok(())
}

/// Takes the symbols that nothing links against out of the ELF files in
/// `tree`, with the pinned toolchain's `llvm-objcopy`. That is a fifth of
/// the servers (14 of 67 MB), which every boot of the Linux twin unpacks
/// into memory. A symbolic link is left alone: writing through it would
/// put a copy of its file in its place.
fn strip_elf_files(ctx: &Ctx, tree: &Path) -> Result<()> {
    let objcopy = kernel::objcopy(ctx)?;
    let mut elf_files = Vec::new();
    find_elf_files(tree, &mut elf_files)?;
    for file in elf_files {
        let kind = file.symlink_metadata().context(|| format!("reading {}", file.display()))?;
        if kind.file_type().is_symlink() {
            continue;
        }
        process::run(ctx, Command::new(&objcopy).arg("--strip-unneeded").arg(&file))?;
    }
    Ok(())
}

/// The ELF files beneath `folder`, in the order of their paths.
fn find_elf_files(folder: &Path, found: &mut Vec<PathBuf>) -> Result<()> {
    use std::io::Read as _;
    let mut paths = Vec::new();
    for entry in std::fs::read_dir(folder).context(|| format!("reading {}", folder.display()))? {
        paths.push(entry.context(|| format!("reading {}", folder.display()))?.path());
    }
    paths.sort();
    for path in paths {
        if path.is_dir() {
            find_elf_files(&path, found)?;
            continue;
        }
        let mut magic = [0u8; 4];
        let opened =
            std::fs::File::open(&path).context(|| format!("reading {}", path.display()))?;
        if (&opened).read_exact(&mut magic).is_ok() && &magic == b"\x7fELF" {
            found.push(path);
        }
    }
    Ok(())
}

/// The files `ldd` lists in `output`, as paths: each library that has an
/// `=>` and the loader, which is a line of its own. The kernel's
/// `linux-vdso.so.1`, which is no file, is left out.
///
/// # Errors
///
/// Names the library that `ldd` says it cannot find.
fn ldd_paths(output: &str) -> std::result::Result<Vec<PathBuf>, String> {
    let mut paths = Vec::new();
    for line in output.lines().map(str::trim) {
        let path = if let Some((name, found)) = line.split_once(" => ") {
            if name.starts_with("linux-vdso") {
                continue;
            }
            match found.split_whitespace().next() {
                Some(path) if path.starts_with('/') => path,
                _ => return Err(format!("{name}, which is not found")),
            }
        } else if line.starts_with('/') {
            line.split_whitespace().next().unwrap_or(line)
        } else {
            continue;
        };
        paths.push(PathBuf::from(path));
    }
    Ok(paths)
}

/// Copies the host file at `path`, and what it is a link to, to the same
/// path in `tree`, unless it is there.
fn put_host_file(tree: &Path, path: &Path) -> Result<()> {
    let to = tree.join(path.strip_prefix("/").unwrap_or(path));
    if to.exists() {
        return Ok(());
    }
    let real = path.canonicalize().context(|| format!("finding {}", path.display()))?;
    process::copy(&real, &to)?;
    std::fs::set_permissions(&to, std::fs::Permissions::from_mode(0o755))
        .context(|| format!("setting the mode of {}", to.display()))
}

/// Builds the serverbench program for Linux as `posix_for_linux` builds
/// bench.c: static, with this machine's C library and the same compiler,
/// at the same optimisation, at `program`.
fn serverbench_for_linux(ctx: &Ctx, compiler: &Path, program: &Path) -> Result<()> {
    let mut cmd = Command::new(compiler);
    cmd.args(["-std=c11", "-D_GNU_SOURCE", "-O2", "-Wall", "-static", "-pthread", "-o"])
        .arg(program)
        .arg(ctx.root.join(SERVERBENCH_SOURCE));
    process::run(ctx, &mut cmd)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ldd_lists_the_libraries_with_a_path_and_the_loader() {
        let output = "\tlinux-vdso.so.1 (0x00007ffc6a7f5000)\n\
                      \tlibm.so.6 => /lib/x86_64-linux-gnu/libm.so.6 (0x00007f3a1c000000)\n\
                      \tlibpq.so.5 => /build/lib/libpq.so.5 (0x00007f3a1bf00000)\n\
                      \t/lib64/ld-linux-x86-64.so.2 (0x00007f3a1c200000)\n";
        assert_eq!(
            ldd_paths(output).unwrap(),
            [
                PathBuf::from("/lib/x86_64-linux-gnu/libm.so.6"),
                PathBuf::from("/build/lib/libpq.so.5"),
                PathBuf::from("/lib64/ld-linux-x86-64.so.2"),
            ]
        );
    }

    #[test]
    fn ldd_of_a_program_with_no_libraries_lists_none() {
        assert_eq!(ldd_paths("\tstatically linked\n").unwrap(), Vec::<PathBuf>::new());
    }

    #[test]
    fn a_library_that_ldd_cannot_find_is_named() {
        let output = "\tlibicuuc.so.74 => not found\n";
        let why = ldd_paths(output).unwrap_err();
        assert!(why.contains("libicuuc.so.74"), "{why}");
    }
}

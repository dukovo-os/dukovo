// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Ports: software from elsewhere, built for Radix (ob(1), PORTS; ADR 0069).
//!
//! A port is a directory `ports/<name>/` with:
//!
//! - `recipe.toml`: the port's version, license and home page, the git
//!   commit or the release archive's SHA-256 its source is pinned to (or,
//!   for Rust's standard library, the pinned Rust toolchain's rust-src, or,
//!   for libraries taken from a distribution's container image, the layers
//!   of the image, each pinned by its digest and size),
//!   for a release archive the project's git repository and the tag of
//!   the release (`[upstream]`), any other sources unpacked into its tree
//!   and crates from crates.io, each pinned by its SHA-256, the ports it
//!   needs built first, and the files it adds to the boot archive;
//! - `patches/*.patch`, applied in name order with `patch -p1`;
//! - `build.sh`, which builds the port in a fresh copy of its patched
//!   source and installs it into the sysroot (`out/sysroot`), or, for a
//!   port of the toolchain, into `out/toolchain`.
//!
//! A port that only a release of its own language builds, as Go's compiler
//! is built by Go, names the oldest release that will do and the release
//! ob downloads when the host has none (`[bootstrap]`). Its build.sh gets
//! the tree of one in `$GOROOT_BOOTSTRAP`: the host's, which the
//! variable of that name points ob at, or the download, which is checked
//! against the size and SHA-256 the recipe pins and unpacked once into
//! `target/ports/sources`.
//!
//! Sources are fetched once into `target/ports`, with git by commit, as
//! release archives by checksum (from their home, or from a mirror when
//! that cannot be reached), as the blobs of an image's layers by digest
//! from its registry and from crates.io by checksum, and checked to
//! be what the recipe names, so a build uses nothing else. Of a large
//! repository, such as LLVM's, a recipe can name the directories it
//! needs, and only their files are fetched. A port is
//! built again only when something that went into its last build
//! changed: its directory, the toolchain (and Rust's, for a port of
//! rust-src), or a port it needs.
//!
//! The toolchain is the host's Clang and LLD, [`CLANG_MIN`] or later,
//! made to target Radix by the wrapper scripts and Meson cross files ob
//! writes into `out/toolchain`: C for Radix against the C library in the
//! sysroot, linked into static position-independent programs, or, for
//! bundles that ship libraries of their own, into programs and libraries
//! on the shared C library and its dynamic linker (ADR 0071); C++ the same
//! way, on LLVM's libc++ once the llvm-runtimes port has built it; and a
//! CMake toolchain file for ports that build with CMake (ADR 0090).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::Deserialize;

use crate::process::{self, step};
use crate::{Context, Ctx, Error, Result};

/// The oldest Clang the toolchain takes: mlibc's C++23 needs Clang 19's
/// concepts, and CI and the build container use 20.
pub const CLANG_MIN: u32 = 20;

/// The prefix of the toolchain's programs: `x86_64-radix-cc` and so on.
pub const TRIPLE: &str = "x86_64-radix";

/// One port's `recipe.toml`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Recipe {
    /// The port's name: its directory's.
    pub name: String,
    pub version: String,
    /// What it is, in a few words.
    pub summary: String,
    /// SPDX license expression of what the port installs.
    pub license: String,
    pub homepage: String,
    /// Ports built before this one.
    #[serde(default)]
    pub depends: Vec<String>,
    /// Whether it installs into the toolchain rather than the sysroot:
    /// what it installs is used to build, and never reaches an image.
    #[serde(default)]
    pub toolchain: bool,
    /// Its source; a port of crates or of extra sources alone has none.
    #[serde(default)]
    pub source: Source,
    /// For a source that is a release archive: where the project keeps
    /// its git repository, which the archive does not say.
    #[serde(default)]
    pub upstream: Option<Upstream>,
    /// The release series the port follows, such as `3.5` for OpenSSL's
    /// long-term release: `ob ports outdated` offers only releases whose
    /// leading numbers are these. Without it, any newer release counts.
    #[serde(default)]
    pub series: Option<String>,
    /// For a port that takes only data from its source, such as locale
    /// tables, and builds none of the project's code: why, in one line.
    /// `ob security scan` does not ask about the project's flaws, which are
    /// in code the port does not build, and lists the port as not asked
    /// about, with this reason.
    #[serde(default)]
    pub data_only: Option<String>,
    /// More sources, unpacked inside the port's source tree.
    #[serde(default)]
    pub extra: Vec<Extra>,
    /// Crates from crates.io, unpacked inside the port's source tree.
    #[serde(default, rename = "crate")]
    pub crates: Vec<Crate>,
    /// Go modules from Go's module proxy, unpacked inside the port's
    /// source tree.
    #[serde(default, rename = "module")]
    pub modules: Vec<GoModule>,
    /// For a port in Go: the license of a module its programs link that
    /// the module's own files do not state in words `licenses::go_license`
    /// reads, as an SPDX expression by the module's path (the replacing
    /// module's, for one go.mod replaces). The recipe says in a comment
    /// above each line where the license is stated. A path no program
    /// links is an error, so that an entry does not outlive its module.
    #[serde(default, rename = "go-licenses")]
    pub go_licenses: BTreeMap<String, String>,
    /// Files of the sysroot the boot archive holds, as paths beneath it;
    /// one that ends in `/` is a directory, with everything in it.
    #[serde(default)]
    pub image: Vec<String>,
    /// The files of the source that hold its copyright and license
    /// notice, for a source without a LICENSE or COPYING file.
    #[serde(default)]
    pub notices: Vec<String>,
    /// Code from other projects that the port keeps in its own
    /// directory, each a package of its own in the license manifest.
    #[serde(default)]
    pub bundled: Vec<Bundled>,
    /// For a port the catalog carries and the image does not: the bundle
    /// ob packs from what its build.sh installs into `$PORT_BUNDLE`
    /// (ADR 0370).
    #[serde(default)]
    pub bundle: Option<BundleRecipe>,
    /// For a port that only a release of its own language builds, as Go's
    /// compiler is built by Go: the releases that will do, and the one ob
    /// downloads when the host has none.
    #[serde(default)]
    pub bootstrap: Option<Bootstrap>,
}

/// The toolchain a port builds itself with, which is not the one it
/// builds: the release of Go that builds Go. ob hands build.sh the tree of
/// one in `$GOROOT_BOOTSTRAP`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bootstrap {
    /// The oldest release that will do, such as `1.24.6`: the host's, which
    /// `$GOROOT_BOOTSTRAP` names, is refused when older.
    pub minimum: String,
    /// The release ob downloads when `$GOROOT_BOOTSTRAP` is not set: the
    /// zip of Go's toolchain module, which the Go proxy serves.
    pub url: String,
    /// The zip's SHA-256, which pins it.
    pub sha256: String,
    /// The zip's size in bytes, which a download is refused beyond.
    pub size: u64,
    /// The folder of the zip that holds the toolchain's tree, with `bin`
    /// and `VERSION` in it.
    pub root: String,
}

/// What a port that is a catalog bundle says of its bundle; the rest of
/// its manifest comes from the recipe (ADR 0370).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BundleRecipe {
    /// The bundle's ID, reverse DNS of its upstream's home.
    pub id: String,
    /// Its name as people see it.
    pub name: String,
    /// More about it than the recipe's summary says, a paragraph for the
    /// manifest's `description`: what it leaves out, say.
    #[serde(default)]
    pub description: Option<String>,
    /// The commands it gives, each a program `bin/<command>` that runs
    /// with the access of whoever runs it, as command-line tools do
    /// (manifest.json(5)); the install sheet says so.
    pub commands: Vec<String>,
    /// Its commands that run another of its programs by their own name,
    /// each the program's: `valkey-check-aof = "valkey-server"`, say,
    /// whose program tells what to do by the name it is run by.
    #[serde(default)]
    pub aliases: BTreeMap<String, String>,
    /// Its programs besides its commands, each `bin/<name>`: those its
    /// services run, say.
    #[serde(default)]
    pub programs: Vec<String>,
    /// The ports whose code its programs carry, linked in, as a C
    /// program carries mlibc's: each goes in its SBOM, with its licenses
    /// and notices, and so does the compiler-rt builtins' code.
    #[serde(default)]
    pub links: Vec<String>,
    /// Its services, each a table of the fields manifest.json(5) gives a
    /// service, written into its manifest as they are.
    #[serde(default)]
    pub services: Vec<toml::Table>,
    /// The permissions it asks for, by name, each a table of
    /// manifest.json(5)'s fields, written into its manifest as they are.
    #[serde(default)]
    pub permissions: toml::Table,
    /// Which of its folders it keeps (manifest.json(5)'s `data`).
    #[serde(default)]
    pub data: Option<toml::Table>,
    /// What its programs are written for: `linux` for x86-64 Linux
    /// programs, which its manifest says with version 2's `runtime` (ADR
    /// 0625); none for Dukovo's own.
    #[serde(default)]
    pub runtime: Option<String>,
    /// The major version line it is of, such as `26` (manifest.json(5)'s
    /// `line`): the bundle is `ID@line` beside the ID's other lines, and
    /// each line is a port of its own.
    #[serde(default)]
    pub line: Option<String>,
    /// The files a project chooses its line with, such as `.nvmrc`.
    #[serde(default)]
    pub line_files: Vec<String>,
    /// Where its package manager installs what each person adds
    /// (manifest.json(5)'s `add_ons`), written into its manifest as it is.
    #[serde(default)]
    pub add_ons: Option<toml::Table>,
}

impl BundleRecipe {
    /// What names this bundle among the ports' and the catalog's:
    /// `ID@line` for one of a line, its ID for another.
    pub fn key(&self) -> String {
        match &self.line {
            Some(line) => format!("{}@{line}", self.id),
            None => self.id.clone(),
        }
    }

    /// Whether its programs are Linux programs.
    pub fn is_linux(&self) -> bool {
        self.runtime.as_deref() == Some("linux")
    }
}

/// Where a source comes from: a git repository and a commit in it, a
/// release archive, the layers of an image in a registry, or rust-src;
/// none of them for a port of crates or of extra sources alone.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    /// The repository, for a source from git.
    #[serde(default)]
    pub git: String,
    /// The full commit ID, which pins the tree.
    #[serde(default)]
    pub commit: String,
    /// The release or tag the commit is, or for an image the tag it was
    /// pinned from, for people.
    #[serde(default)]
    pub tag: Option<String>,
    /// The directories of the repository the port needs, for a large one
    /// of which it needs only these: only their files are fetched (with a
    /// partial clone, checked out sparsely) and copied into its tree.
    #[serde(default)]
    pub paths: Vec<String>,
    /// A release archive (`.tar.gz`, `.tar.xz` or `.tar.bz2`), for a
    /// project whose releases hold more than its repository does, such as
    /// GNU's, whose archives hold their generated configure scripts.
    #[serde(default)]
    pub url: String,
    /// The archive's SHA-256, which pins it.
    #[serde(default)]
    pub sha256: String,
    /// Other places that serve the same archive, tried in order when
    /// `url` cannot be reached: Ubuntu's source pool keeps upstream's
    /// release archives, unchanged, as `<name>_<version>.orig.tar.*`.
    #[serde(default)]
    pub mirrors: Vec<String>,
    /// Whether the source is the `library` directory of the pinned Rust
    /// toolchain's rust-src component, with the crates it vendors: the
    /// toolchain pins it, and it needs no fetching.
    #[serde(default, rename = "rust-src")]
    pub rust_src: bool,
    /// The registry that serves an image, such as `registry-1.docker.io`,
    /// for a source that is the layers of an image.
    #[serde(default)]
    pub registry: String,
    /// The image's repository in it, such as `freedesktopsdk/platform`.
    #[serde(default)]
    pub repository: String,
    /// The image's layers, in the order the image stacks them, each pinned
    /// by the digest of its blob and its size. The manifest that lists them
    /// is not read: a manifest is a tag's to change, and a blob is its own
    /// digest's.
    #[serde(default)]
    pub layers: Vec<Layer>,
}

/// A layer of an image: a blob the registry serves by its digest.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Layer {
    /// `sha256:` and the blob's SHA-256, which pins it.
    pub digest: String,
    /// The blob's size in bytes, which a download is refused beyond.
    pub size: u64,
}

impl Layer {
    /// The blob's SHA-256 in lower-case hexadecimal, which is what
    /// `sha256sum` prints; none for a digest that is not one.
    pub fn sha256(&self) -> Option<&str> {
        let hex = self.digest.strip_prefix("sha256:")?;
        (is_hex(hex, 64) && !hex.bytes().any(|b| b.is_ascii_uppercase())).then_some(hex)
    }
}

impl Source {
    /// Whether the source is the layers of an image.
    pub fn is_image(&self) -> bool {
        !self.layers.is_empty()
    }

    /// Where the source can be downloaded, as an SPDX download location.
    pub fn download(&self) -> String {
        if !self.url.is_empty() {
            return self.url.clone();
        }
        if let Some(layer) = self.layers.first() {
            return format!(
                "https://{}/v2/{}/blobs/{}",
                self.registry, self.repository, layer.digest
            );
        }
        format!("git+{}@{}", self.git, self.commit)
    }

    /// Which release it is, for people: its tag, its commit or its
    /// archive's name.
    pub fn release(&self) -> &str {
        match &self.tag {
            Some(tag) => tag,
            None if !self.url.is_empty() => self.url.rsplit('/').next().unwrap_or(&self.url),
            None => &self.commit,
        }
    }
}

/// The project behind a release archive: its git repository and the tag of
/// the release the archive is, or the reason it has no repository. An
/// archive names no commit, so the daily scan resolves the tag to one to
/// ask OSV by (ADR 0534), and `ob ports outdated` reads the repository's
/// tags for a newer release (ADR 0370). The archive stays what is built.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Upstream {
    /// The project's own repository, over https. A mirror is named only
    /// where the project has no repository of its own to name, and the
    /// recipe says so.
    #[serde(default)]
    pub git: String,
    /// The tag of the release the archive is, which names the recipe's
    /// version.
    #[serde(default)]
    pub tag: String,
    /// Why the project has no public git repository, in one line.
    #[serde(default)]
    pub none: Option<String>,
}

impl Upstream {
    /// The repository and the tag, unless the project has no repository.
    pub fn tagged(&self) -> Option<(&str, &str)> {
        (self.none.is_none() && !self.git.is_empty() && !self.tag.is_empty())
            .then_some((self.git.as_str(), self.tag.as_str()))
    }
}

/// A source unpacked inside a port's tree, such as a Meson subproject.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Extra {
    /// Where it goes, relative to the port's source.
    pub path: String,
    pub git: String,
    pub commit: String,
    /// SPDX license expression.
    pub license: String,
}

/// A crate from crates.io unpacked inside a port's tree: the crates a port
/// of Rust's standard library or of crates patches for Radix.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Crate {
    pub name: String,
    pub version: String,
    /// The SHA-256 of its `.crate` file, which Cargo.lock files record too.
    pub sha256: String,
    /// SPDX license expression, as its Cargo.toml gives it.
    pub license: String,
    /// Where it goes, relative to the port's source: `<name>-<version>`
    /// unless given.
    #[serde(default)]
    pub path: Option<String>,
}

impl Crate {
    /// Where it goes in the port's source.
    pub fn path(&self) -> String {
        self.path.clone().unwrap_or_else(|| format!("{}-{}", self.name, self.version))
    }
}

/// A Go module from Go's module proxy unpacked inside a port's tree: the
/// modules the go-modules port patches for Radix (ADR 0600).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct GoModule {
    /// Its path, such as `golang.org/x/sys`.
    pub path: String,
    /// Its version, such as `v0.50.0`.
    pub version: String,
    /// The SHA-256 of the proxy's zip of it, which the proxy never
    /// changes once it has served it.
    pub sha256: String,
    /// SPDX license expression, as its license files say.
    pub license: String,
    /// Whether the patched module takes the place of every version of it
    /// a program selects, not only of this one: for a module that
    /// requires no other, whose code is newer than the versions it
    /// replaces.
    #[serde(default)]
    pub all_versions: bool,
}

impl GoModule {
    /// Where it goes in the port's source, a name of one component:
    /// `golang.org_x_sys@v0.50.0`.
    pub fn dir(&self) -> String {
        format!("{}@{}", self.path.replace('/', "_"), self.version)
    }

    /// Its zip's URL on Go's module proxy.
    pub fn url(&self) -> String {
        go_proxy_url(&self.path, &self.version)
    }
}

/// A module path or version as Go's module proxy and module cache spell
/// it: an upper-case letter is `!` and the letter in lower case, since
/// file systems that ignore case would make two modules one.
fn go_escaped(text: &str) -> String {
    text.chars().fold(String::new(), |mut out, c| {
        if c.is_ascii_uppercase() {
            out.push('!');
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
        out
    })
}

/// The URL of a module's zip on Go's module proxy.
pub fn go_proxy_url(path: &str, version: &str) -> String {
    format!("{GO_PROXY}/{}/@v/{}.zip", go_escaped(path), go_escaped(version))
}

/// Where Go's module cache, [`go_module_cache`], unpacks a module of the
/// proxy: a folder `<escaped path>@<escaped version>`, with the path's
/// parts as folders.
pub fn go_cached_module(ctx: &Ctx, path: &str, version: &str) -> PathBuf {
    go_module_cache(ctx).join(format!("{}@{}", go_escaped(path), go_escaped(version)))
}

/// The list of a port's Go modules that ob writes into its source for its
/// build.sh: a line `PATH VERSION FOLDER all|one` for each.
pub const GO_MODULE_LIST: &str = "modules.list";

/// Go's module proxy, the one place Go for Radix fetches modules from.
pub const GO_PROXY: &str = "https://proxy.golang.org";

/// The port of Go's own toolchain, whose source holds the standard library
/// and runtime every program in Go carries.
pub const GO_PORT: &str = "go";

/// The port of the modules patched for Radix, which programs in Go are
/// built with in place of the proxy's.
pub const GO_MODULES_PORT: &str = "go-modules";

/// A module that programs in Go link, as `go version -m` names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkedModule {
    /// A module of Go's module proxy, by the path and version of the code
    /// that is linked: the replacement's, where go.mod replaces the module
    /// with another.
    Proxy { path: String, version: String },
    /// A module the go-modules port patched, which takes the place of the
    /// versions of it that the programs name: `folder` is its
    /// [`GoModule::dir`].
    Patched { folder: String, named: BTreeSet<String> },
}

/// A `dep` line of `go version -m`, with the `=>` line after it, if any.
struct GoDep<'a> {
    path: &'a str,
    version: &'a str,
    /// The path and version that take its place; a folder's version is
    /// `(devel)`.
    replacement: Option<(&'a str, &'a str)>,
    line: usize,
}

/// The `dep` lines of what `go version -m` printed. The lines are tab
/// separated and start with a tab: `dep PATH VERSION SUM`, and after a
/// module that is replaced `=> PATH VERSION SUM`, where a replacement by a
/// folder has `(devel)` for its version and no sum. The line that names a
/// program, `path`, `mod` and `build` lines are not modules.
fn go_deps(text: &str) -> Result<Vec<GoDep<'_>>, String> {
    let mut deps: Vec<GoDep> = Vec::new();
    // Whether the line before was a `dep` line, which a `=>` line follows.
    let mut after_dep = false;
    for (n, line) in text.lines().enumerate() {
        let fields: Vec<&str> = line.split('\t').collect();
        let field = |i: usize| fields.get(i).copied().filter(|field| !field.is_empty());
        let kind = if fields[0].is_empty() { field(1) } else { None };
        let line_number = n + 1;
        match kind {
            Some("dep") => {
                let (Some(path), Some(version)) = (field(2), field(3)) else {
                    return Err(format!("line {line_number}: a dep line has no path and version"));
                };
                deps.push(GoDep { path, version, replacement: None, line: line_number });
                after_dep = true;
                continue;
            }
            Some("=>") if after_dep => {
                let (Some(path), Some(version)) = (field(2), field(3)) else {
                    return Err(format!("line {line_number}: a `=>` line has no path and version"));
                };
                deps.last_mut().expect("a dep line came before").replacement =
                    Some((path, version));
            }
            Some("=>") => return Err(format!("line {line_number}: a `=>` line replaces no dep")),
            _ => {}
        }
        after_dep = false;
    }
    Ok(deps)
}

/// The modules that the programs `go version -m` described in `text` link,
/// once each, in the order they are first named. `text` is what a port's
/// build.sh leaves in `$PORT_INFO/modules.txt`.
///
/// # Errors
///
/// A line that is not in the form `go version -m` prints, or a module
/// replaced by a folder that is not one of the go-modules port's: the
/// license of its code cannot be known.
pub fn linked_go_modules(text: &str) -> Result<Vec<LinkedModule>, String> {
    let mut modules: Vec<LinkedModule> = Vec::new();
    for dep in go_deps(text)? {
        match linked_module(&dep)? {
            LinkedModule::Patched { folder, named } => {
                let known = modules.iter_mut().find_map(|have| match have {
                    LinkedModule::Patched { folder: other, named } if *other == folder => {
                        Some(named)
                    }
                    _ => None,
                });
                match known {
                    Some(have) => have.extend(named),
                    None => modules.push(LinkedModule::Patched { folder, named }),
                }
            }
            proxy if !modules.contains(&proxy) => modules.push(proxy),
            _ => {}
        }
    }
    Ok(modules)
}

/// The module whose code a `dep` line links.
fn linked_module(dep: &GoDep) -> Result<LinkedModule, String> {
    let Some((target, version)) = dep.replacement else {
        return Ok(LinkedModule::Proxy { path: dep.path.into(), version: dep.version.into() });
    };
    if let Some(folder) = target.strip_prefix(&format!("./{}/", Toolchain::GO_LINK)) {
        let named = BTreeSet::from([dep.version.to_string()]);
        return Ok(LinkedModule::Patched { folder: folder.into(), named });
    }
    // Go reads a replacement that starts with `.` or `/` as a folder, which
    // no module's path does.
    if target.starts_with(['.', '/']) {
        return Err(format!(
            "line {}: {} {} is replaced by the folder {target}, which is not one of \
             {GO_MODULES_PORT}'s, so its code and license are not known",
            dep.line, dep.path, dep.version
        ));
    }
    Ok(LinkedModule::Proxy { path: target.into(), version: version.into() })
}

/// Code from another project that a port keeps in its own directory, as
/// ports/mlibc keeps musl's in `musl/`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bundled {
    pub name: String,
    pub version: String,
    /// SPDX license expression.
    pub license: String,
    pub homepage: String,
    /// The files that hold its copyright and license notice, relative to
    /// the port's directory.
    pub notices: Vec<String>,
}

/// A port that was built: its recipe, its directory, and where its
/// sources are.
pub struct Port {
    pub recipe: Recipe,
    /// `ports/<name>`.
    pub dir: PathBuf,
    /// Its source as fetched, if it has one.
    pub source: Option<PathBuf>,
    /// The extra sources, in the recipe's order.
    pub extras: Vec<PathBuf>,
    /// The crates, unpacked, in the recipe's order.
    pub crates: Vec<PathBuf>,
    /// The Go modules, unpacked, in the recipe's order.
    pub modules: Vec<PathBuf>,
}

/// Reads every recipe in `ports/`, in an order that builds each port
/// after those it needs.
pub fn recipes(ctx: &Ctx) -> Result<Vec<(Recipe, PathBuf)>> {
    let ports = ctx.root.join("ports");
    let mut found = BTreeMap::new();
    let entries = std::fs::read_dir(&ports).context(|| format!("reading {}", ports.display()))?;
    for entry in entries {
        let dir = entry.context(|| format!("reading {}", ports.display()))?.path();
        let file = dir.join("recipe.toml");
        if !file.is_file() {
            continue;
        }
        let text =
            std::fs::read_to_string(&file).context(|| format!("reading {}", file.display()))?;
        let recipe: Recipe = toml::from_str(&text)
            .map_err(|err| Error::new(format!("{}: {err}", ctx.display(&file))))?;
        check(ctx, &recipe, &dir)?;
        found.insert(recipe.name.clone(), (recipe, dir));
    }
    // A bundle, or a line of one, is one port's: the catalog and
    // `--next-build` find its port by it.
    let mut bundles: BTreeMap<String, &str> = BTreeMap::new();
    for (name, (recipe, _)) in &found {
        if let Some(bundle) = &recipe.bundle
            && let Some(other) = bundles.insert(bundle.key(), name)
        {
            return Err(Error::new(format!(
                "ports {other} and {name} both pack {}: give each line its own `line`",
                bundle.key()
            )));
        }
    }
    // Each port after those it needs: a depth-first walk.
    let mut order = Vec::new();
    let names: Vec<String> = found.keys().cloned().collect();
    let mut state: BTreeMap<String, bool> = BTreeMap::new();
    for name in &names {
        visit(name, &found, &mut state, &mut order)?;
    }
    Ok(order
        .into_iter()
        .map(|name| found.remove(&name).expect("every name visited was found"))
        .collect())
}

fn visit(
    name: &str,
    found: &BTreeMap<String, (Recipe, PathBuf)>,
    state: &mut BTreeMap<String, bool>,
    order: &mut Vec<String>,
) -> Result<()> {
    match state.get(name) {
        Some(true) => return Ok(()),
        Some(false) => return Err(Error::new(format!("ports: {name} needs itself"))),
        None => {}
    }
    let Some((recipe, _)) = found.get(name) else {
        return Err(Error::new(format!("ports: no port is named {name}")));
    };
    state.insert(name.to_string(), false);
    for dep in &recipe.depends {
        visit(dep, found, state, order)
            .map_err(|err| Error::new(format!("ports: {name} needs {dep}: {err}")))?;
    }
    state.insert(name.to_string(), true);
    order.push(name.to_string());
    Ok(())
}

/// Checks what a recipe says that serde does not.
fn check(ctx: &Ctx, recipe: &Recipe, dir: &Path) -> Result<()> {
    let what = ctx.display(&dir.join("recipe.toml"));
    if dir.file_name().and_then(|n| n.to_str()) != Some(recipe.name.as_str()) {
        return Err(Error::new(format!("{what}: the name must be the directory's")));
    }
    let source = &recipe.source;
    let (git, archive) = (!source.git.is_empty(), !source.url.is_empty());
    let image = source.is_image();
    let kinds =
        usize::from(git) + usize::from(archive) + usize::from(image) + usize::from(source.rust_src);
    if kinds > 1 {
        return Err(Error::new(format!(
            "{what}: a source is one of git, url, layers and rust-src, not more"
        )));
    }
    if let Some(problem) = image_problem(source) {
        return Err(Error::new(format!("{what}: {problem}")));
    }
    if source.rust_src && !recipe.toolchain {
        return Err(Error::new(format!("{what}: only a toolchain port builds rust-src")));
    }
    if kinds == 0
        && recipe.crates.is_empty()
        && recipe.modules.is_empty()
        && recipe.extra.is_empty()
    {
        return Err(Error::new(format!(
            "{what}: a port needs a source: git and commit, url and sha256, an image's layers, \
             rust-src, crates, Go modules or extra sources"
        )));
    }
    if !git && (!source.commit.is_empty() || (source.tag.is_some() && !image)) {
        return Err(Error::new(format!("{what}: commit and tag go with git")));
    }
    if !archive && (!source.sha256.is_empty() || !source.mirrors.is_empty()) {
        return Err(Error::new(format!("{what}: sha256 and mirrors go with url")));
    }
    if archive && !is_hex(&source.sha256, 64) {
        return Err(Error::new(format!("{what}: `{}` is not a SHA-256", source.sha256)));
    }
    for url in archive.then_some(&source.url).into_iter().chain(&source.mirrors) {
        if !url.starts_with("https://") || url.ends_with('/') {
            return Err(Error::new(format!("{what}: `{url}` is not an https:// URL of a file")));
        }
    }
    let main = git.then_some(&source.commit);
    for commit in main.into_iter().chain(recipe.extra.iter().map(|e| &e.commit)) {
        if !is_hex(commit, 40) {
            return Err(Error::new(format!("{what}: `{commit}` is not a full commit ID")));
        }
    }
    for krate in &recipe.crates {
        let path = krate.path();
        if !is_hex(&krate.sha256, 64) {
            return Err(Error::new(format!(
                "{what}: crate {} {}: `{}` is not a SHA-256",
                krate.name, krate.version, krate.sha256
            )));
        }
        let plain = |s: &str| {
            !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.+".contains(&b))
        };
        if !plain(&krate.name) || !plain(&krate.version) || !inside(&path) {
            return Err(Error::new(format!(
                "{what}: crate `{}` `{}` at `{path}` is not a crate's name, version and \
                 a path inside the source",
                krate.name, krate.version
            )));
        }
    }
    if let Some(problem) = modules_problem(recipe) {
        return Err(Error::new(format!("{what}: {problem}")));
    }
    if let Some(problem) = upstream_problem(recipe) {
        return Err(Error::new(format!("{what}: {problem}")));
    }
    if let Some(problem) = series_problem(recipe) {
        return Err(Error::new(format!("{what}: {problem}")));
    }
    if let Some(problem) = data_only_problem(recipe) {
        return Err(Error::new(format!("{what}: {problem}")));
    }
    if let Some(path) = outside_path(recipe) {
        return Err(Error::new(format!(
            "{what}: `{path}` is not a path inside the tree it is taken from"
        )));
    }
    if let Some(problem) = bootstrap_problem(recipe) {
        return Err(Error::new(format!("{what}: {problem}")));
    }
    if !dir.join("build.sh").is_file() {
        return Err(Error::new(format!("{what}: the port has no build.sh")));
    }
    if recipe.toolchain && !recipe.image.is_empty() {
        return Err(Error::new(format!("{what}: a toolchain port puts nothing in the image")));
    }
    if let Some(bundle) = &recipe.bundle {
        if recipe.toolchain || !recipe.image.is_empty() {
            return Err(Error::new(format!(
                "{what}: a bundle is the catalog's: it is no toolchain port and puts nothing in \
                 the image"
            )));
        }
        let plain = |s: &str| {
            !s.is_empty()
                && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
                && !s.starts_with(['.', '-'])
        };
        // A bundle of libraries alone gives no command, and its manifest
        // still names a program (manifest.json(5)).
        let mut named = bundle.commands.iter().chain(&bundle.programs);
        if (bundle.commands.is_empty() && bundle.programs.is_empty()) || !named.all(|c| plain(c)) {
            return Err(Error::new(format!(
                "{what}: a bundle gives commands or programs, each a name of letters, digits, \
                 `-`, `_` and `.`"
            )));
        }
        if bundle.runtime.as_deref().is_some_and(|runtime| runtime != "linux") {
            return Err(Error::new(format!(
                "{what}: a bundle's runtime is \"linux\", or none for Dukovo's programs"
            )));
        }
        // The manifest's reader checks the rest of each when the bundle
        // is packed.
        if bundle.line.is_none() && (!bundle.line_files.is_empty() || bundle.add_ons.is_some()) {
            return Err(Error::new(format!("{what}: line_files and add_ons need a line")));
        }
    }
    Ok(())
}

/// What is wrong with `source` as the layers of an image, if anything: the
/// registry is a host name, the repository a path of the names a registry
/// takes, each layer's digest a SHA-256 and its size is not 0, and the tag
/// it was pinned from, which says which image the layers are, is there.
/// Nothing but the layers goes with a registry and a repository.
fn image_problem(source: &Source) -> Option<String> {
    if !source.is_image() {
        return (!source.registry.is_empty() || !source.repository.is_empty())
            .then(|| "registry and repository go with layers".to_string());
    }
    let name = |part: &str| {
        !part.is_empty()
            && !part.starts_with(['.', '_', '-'])
            && part
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(&b))
    };
    let host = !source.registry.is_empty()
        && source.registry.split('.').all(|label| {
            !label.is_empty()
                && !label.starts_with('-')
                && label.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        });
    if !host {
        return Some(format!("registry `{}` is not a host name", source.registry));
    }
    if !source.repository.split('/').all(name) {
        return Some(format!(
            "repository `{}` is not a repository's name, such as library/alpine",
            source.repository
        ));
    }
    if source.tag.as_deref().is_none_or(str::is_empty) {
        return Some("an image needs its tag, which says which image the layers are".to_string());
    }
    for layer in &source.layers {
        if layer.sha256().is_none() {
            return Some(format!(
                "layer `{}` is not `sha256:` and 64 lower-case hexadecimal digits",
                layer.digest
            ));
        }
        if layer.size == 0 {
            return Some(format!("layer {} is 0 bytes: the size pins the blob too", layer.digest));
        }
    }
    None
}

/// What is wrong with `recipe`'s `series`, if anything: it is one or more
/// numbers set apart by dots, and the version the recipe pins is in it.
fn series_problem(recipe: &Recipe) -> Option<String> {
    let series = recipe.series.as_deref()?;
    let numbers: Option<Vec<&str>> = series
        .split('.')
        .map(|part| (!part.is_empty() && part.bytes().all(|b| b.is_ascii_digit())).then_some(part))
        .collect();
    let Some(numbers) = numbers else {
        return Some(format!("series {series} is not numbers set apart by dots, such as 3.5"));
    };
    let version: Vec<&str> = recipe.version.split(['.', '-', '+']).collect();
    if !version.starts_with(&numbers) {
        return Some(format!(
            "series {series} does not hold the version the recipe pins, {}",
            recipe.version
        ));
    }
    None
}

/// What is wrong with `recipe`'s `data_only`, if anything: its reason is
/// one line, and the port has a source to take the data from.
fn data_only_problem(recipe: &Recipe) -> Option<String> {
    let reason = recipe.data_only.as_deref()?;
    if reason.trim().is_empty() || reason.contains(['\n', '\r']) {
        return Some("data_only needs a reason of one line".to_string());
    }
    if recipe.source.git.is_empty() && recipe.source.url.is_empty() {
        return Some(
            "data_only goes with a source, git or url, that the data comes from".to_string(),
        );
    }
    if !recipe.crates.is_empty() || !recipe.modules.is_empty() {
        return Some("a port that takes only data builds no crates or Go modules".to_string());
    }
    None
}

/// What is wrong with the recipe's `[upstream]`, if anything. A port
/// pinned by a release archive has one: the project's repository and the
/// release's tag, or `none` with the reason there is no repository, so that
/// a missing repository is a decision in the recipe and not an oversight.
/// A port pinned by git has none, as its source is its upstream; a port
/// with no source of its own has none either.
fn upstream_problem(recipe: &Recipe) -> Option<String> {
    let archive = !recipe.source.url.is_empty();
    let Some(upstream) = &recipe.upstream else {
        return archive.then(|| {
            "a port pinned by a release archive needs [upstream]: git and tag, or `none` with \
             the reason the project has no git repository"
                .to_string()
        });
    };
    if !archive {
        return Some(
            "[upstream] goes with a release archive (url): a git source is its own upstream"
                .to_string(),
        );
    }
    let (git, tag) = (upstream.git.as_str(), upstream.tag.as_str());
    if let Some(reason) = &upstream.none {
        if !git.is_empty() || !tag.is_empty() {
            return Some("[upstream] is `none` or names git and tag, not both".to_string());
        }
        if reason.trim().is_empty() || reason.contains(['\n', '\r']) {
            return Some("[upstream] `none` needs a reason of one line".to_string());
        }
        return None;
    }
    if git.is_empty() || tag.is_empty() {
        return Some("[upstream] needs git and tag, or `none` with a reason".to_string());
    }
    let url = git.strip_prefix("https://").unwrap_or_default();
    if url.is_empty() || url.contains(|c: char| c.is_whitespace() || c.is_control()) {
        return Some(format!("[upstream] git `{git}` is not an https:// URL of a repository"));
    }
    // The scan passes the tag to `git ls-remote`, which reads it as a pattern.
    let plain = |c: char| c.is_ascii_alphanumeric() || "-_.+".contains(c);
    if !tag.chars().all(plain) || tag.starts_with(['-', '.']) {
        return Some(format!(
            "[upstream] tag `{tag}` is not a tag name of letters, digits, `-`, `_`, `.` and `+`"
        ));
    }
    if !tag_names_version(tag, &recipe.version) {
        return Some(format!(
            "[upstream] tag `{tag}` is not the release of version {}: move it with the archive",
            recipe.version
        ));
    }
    None
}

/// Whether `tag` names `version`: the runs of digits and of letters of the
/// version, as `3.7c` is 3, 7 and c, stand together in the tag's, as in
/// `curl-8_23_0` for 8.23.0. Another tag of the project, such as the next
/// release's, does not name it.
fn tag_names_version(tag: &str, version: &str) -> bool {
    let wanted = word_runs(version);
    !wanted.is_empty() && word_runs(tag).windows(wanted.len()).any(|run| run == wanted)
}

/// The runs of digits and the runs of letters in `text`, letters in lower
/// case; anything else ends a run.
fn word_runs(text: &str) -> Vec<String> {
    let mut runs: Vec<String> = Vec::new();
    let mut digits = None;
    for c in text.chars() {
        let kind = c.is_ascii_alphanumeric().then(|| c.is_ascii_digit());
        match kind {
            Some(_) if kind == digits => {
                if let Some(run) = runs.last_mut() {
                    run.push(c.to_ascii_lowercase());
                }
            }
            Some(_) => runs.push(c.to_ascii_lowercase().to_string()),
            None => {}
        }
        digits = kind;
    }
    runs
}

/// What is wrong with the recipe's Go modules, if anything: each is a
/// module path, a version and a SHA-256, and no two go to one place.
fn modules_problem(recipe: &Recipe) -> Option<String> {
    let mut dirs = std::collections::BTreeSet::new();
    for module in &recipe.modules {
        let what = format!("module `{}` `{}`", module.path, module.version);
        // Go's own rules are wider; these are what a path and a version
        // of the proxy's are made of, and keep the place a plain name.
        let path_ok = !module.path.is_empty()
            && module.path.split('/').all(|part| {
                !part.is_empty()
                    && !part.starts_with('.')
                    && part.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.~".contains(&b))
            });
        let version_ok = module.version.starts_with('v')
            && module.version.bytes().all(|b| b.is_ascii_alphanumeric() || b"-.+".contains(&b));
        if !path_ok || !version_ok {
            return Some(format!("{what} is not a Go module's path and version"));
        }
        if !is_hex(&module.sha256, 64) {
            return Some(format!("{what}: `{}` is not a SHA-256", module.sha256));
        }
        if !dirs.insert(module.dir()) {
            return Some(format!("{what} goes where another module does, {}", module.dir()));
        }
    }
    None
}

/// What is wrong with the recipe's `[bootstrap]`, if anything: the oldest
/// release is a version, the download is an https:// URL of a zip pinned by
/// its SHA-256 and size, and the tree in the zip is a path inside it.
fn bootstrap_problem(recipe: &Recipe) -> Option<String> {
    let bootstrap = recipe.bootstrap.as_ref()?;
    if dotted_numbers(&bootstrap.minimum).is_none() {
        return Some(format!(
            "[bootstrap] minimum `{}` is not numbers set apart by dots, such as 1.24.6",
            bootstrap.minimum
        ));
    }
    let zip =
        Path::new(&bootstrap.url).extension().is_some_and(|ext| ext.eq_ignore_ascii_case("zip"));
    if !bootstrap.url.starts_with("https://") || !zip {
        return Some(format!(
            "[bootstrap] url `{}` is not an https:// URL of a .zip file",
            bootstrap.url
        ));
    }
    if !is_hex(&bootstrap.sha256, 64) {
        return Some(format!("[bootstrap] sha256 `{}` is not a SHA-256", bootstrap.sha256));
    }
    if bootstrap.size == 0 {
        return Some("[bootstrap] size is the zip's size in bytes, not 0".to_string());
    }
    if !inside(&bootstrap.root) {
        return Some(format!("[bootstrap] root `{}` is not a path inside the zip", bootstrap.root));
    }
    None
}

/// The numbers of a version such as `1.24.6`, set apart by dots.
fn dotted_numbers(text: &str) -> Option<Vec<u64>> {
    text.split('.')
        .map(|part| if part.bytes().all(|b| b.is_ascii_digit()) { part.parse().ok() } else { None })
        .collect()
}

/// Whether `version`, the first line of a Go tree's VERSION file, such as
/// `go1.24.7`, is a release of Go that is `minimum`, such as `1.24.6`, or
/// later; `None` for a version that is no release's, as a tree built from
/// git has, `devel go1.27-...`. A release candidate or beta, such as
/// `go1.25rc1`, comes before the release it leads to.
fn go_release_at_least(version: &str, minimum: &str) -> Option<bool> {
    let rest = version.split_whitespace().next()?.strip_prefix("go")?;
    let end = rest.find(|c: char| !(c.is_ascii_digit() || c == '.')).unwrap_or(rest.len());
    let (numbers, pre_release) = rest.split_at(end);
    let mut found = dotted_numbers(numbers)?;
    let mut wanted = dotted_numbers(minimum)?;
    // go1.25 is go1.25.0.
    let len = found.len().max(wanted.len());
    found.resize(len, 0);
    wanted.resize(len, 0);
    Some(if pre_release.is_empty() { found >= wanted } else { found > wanted })
}

/// The first path `recipe` names that leaves the tree it is taken from:
/// an extra source's place in the source, the source's paths, the
/// sysroot's files for the image (a directory ends in `/`), and the
/// notices in the source or in the port's directory.
fn outside_path(recipe: &Recipe) -> Option<&str> {
    let image = recipe.image.iter().map(|p| p.strip_suffix('/').unwrap_or(p));
    let bundled = recipe.bundled.iter().flat_map(|b| b.notices.iter().map(String::as_str));
    recipe
        .extra
        .iter()
        .map(|e| e.path.as_str())
        .chain(recipe.source.paths.iter().map(String::as_str))
        .chain(image)
        .chain(recipe.notices.iter().map(String::as_str))
        .chain(bundled)
        .find(|path| !inside(path))
}

/// Whether `path` is a relative path that stays inside the directory it
/// is relative to.
fn inside(path: &str) -> bool {
    !path.starts_with('/') && path.split('/').all(|part| !part.is_empty() && part != "..")
}

/// Whether `text` is `len` hexadecimal digits.
fn is_hex(text: &str, len: usize) -> bool {
    text.len() == len && text.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The host programs the toolchain is made of.
pub struct Toolchain {
    /// `out/toolchain`.
    pub dir: PathBuf,
    /// The first line of `clang --version`.
    pub version: String,
    clang: PathBuf,
    resource: PathBuf,
    builtins: PathBuf,
    tools: BTreeMap<&'static str, PathBuf>,
}

/// The LLVM tools the toolchain wraps, by the name the wrapper gets.
pub const TOOLS: &[(&str, &str)] = &[
    ("ar", "llvm-ar"),
    ("ranlib", "llvm-ranlib"),
    ("nm", "llvm-nm"),
    ("strip", "llvm-strip"),
    ("objcopy", "llvm-objcopy"),
    ("readelf", "llvm-readelf"),
    ("ld", "ld.lld"),
];

/// The first of `name-N` for N from the newest down to [`CLANG_MIN`], or
/// `name`, that is on `PATH`.
pub fn find_llvm(name: &str) -> Option<PathBuf> {
    (CLANG_MIN..CLANG_MIN + 10)
        .rev()
        .find_map(|major| process::find_program(format!("{name}-{major}")))
        .or_else(|| process::find_program(name))
}

/// Clang's major version from the first line of `clang --version`.
fn clang_major(line: &str) -> Option<u32> {
    let after = line.split("clang version ").nth(1)?;
    after.split('.').next()?.parse().ok()
}

/// Clang, [`CLANG_MIN`] or later, and the first line of its `--version`.
pub fn find_clang() -> Option<(PathBuf, String)> {
    let clang = find_llvm("clang")?;
    let out = Command::new(&clang).arg("--version").output().ok()?;
    let version = String::from_utf8_lossy(&out.stdout).lines().next()?.to_string();
    (clang_major(&version)? >= CLANG_MIN).then_some((clang, version))
}

/// `clang`'s resource directory, and compiler-rt's x86-64 builtins in it.
pub fn find_builtins(clang: &Path) -> Option<(PathBuf, PathBuf)> {
    let out = Command::new(clang).arg("-print-resource-dir").output().ok()?;
    let resource = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
    // Distributions put them in one of these.
    let builtins = [
        "lib/linux/libclang_rt.builtins-x86_64.a",
        "lib/x86_64-pc-linux-gnu/libclang_rt.builtins.a",
        "lib/x86_64-unknown-linux-gnu/libclang_rt.builtins.a",
        "lib/x86_64-redhat-linux-gnu/libclang_rt.builtins.a",
    ]
    .iter()
    .map(|path| resource.join(path))
    .find(|path| path.is_file())?;
    Some((resource, builtins))
}

/// Finds Clang, LLD and the LLVM tools, and compiler-rt's builtins.
pub fn find_toolchain(ctx: &Ctx) -> Result<Toolchain> {
    let missing = || {
        Error::new(format!(
            "ports need Clang {CLANG_MIN} or later, LLD, compiler-rt and LLVM's tools"
        ))
        .hint("run `./ob doctor` to see what to install")
    };
    let (clang, version) = find_clang().ok_or_else(missing)?;
    let (resource, builtins) = find_builtins(&clang).ok_or_else(missing)?;
    let mut tools = BTreeMap::new();
    for (name, program) in TOOLS {
        let path = find_llvm(program).ok_or_else(missing)?;
        tools.insert(*name, path);
    }
    Ok(Toolchain { dir: ctx.out.join("toolchain"), version, clang, resource, builtins, tools })
}

impl Toolchain {
    /// A toolchain of nothing but its folder, for tests of what reads no
    /// program of it.
    #[cfg(test)]
    pub fn bare(dir: PathBuf) -> Self {
        Self {
            dir,
            version: String::new(),
            clang: PathBuf::new(),
            resource: PathBuf::new(),
            builtins: PathBuf::new(),
            tools: BTreeMap::new(),
        }
    }

    pub fn bin(&self) -> PathBuf {
        self.dir.join("bin")
    }

    /// The C++ headers the C library is built with: libstdc++'s
    /// freestanding ones, from the freestnd-cxx-hdrs port.
    fn cxx_headers(&self) -> PathBuf {
        self.dir.join("include/c++")
    }

    /// The Meson cross file for building the C library itself.
    pub fn libc_cross_file(&self) -> PathBuf {
        self.dir.join("meson/radix-libc.ini")
    }

    /// The Meson cross file for building programs and libraries.
    pub fn cross_file(&self) -> PathBuf {
        self.dir.join("meson/radix.ini")
    }

    /// LLVM's version, `20.1.2` in `Ubuntu clang version 20.1.2 (...)`.
    pub fn llvm_version(&self) -> String {
        self.version
            .split("clang version ")
            .nth(1)
            .and_then(|rest| rest.split_whitespace().next())
            .unwrap_or("NOASSERTION")
            .to_string()
    }

    /// The C compiler wrapper.
    pub fn cc(&self) -> PathBuf {
        self.bin().join(format!("{TRIPLE}-cc"))
    }

    /// The C++ compiler wrapper.
    pub fn cxx(&self) -> PathBuf {
        self.bin().join(format!("{TRIPLE}-c++"))
    }

    /// Clang's C++ driver, beside Clang: clang++-20 for clang-20.
    fn clangxx(&self) -> PathBuf {
        let name = self.clang.file_name().and_then(|n| n.to_str()).unwrap_or("clang");
        self.clang.with_file_name(name.replacen("clang", "clang++", 1))
    }

    /// CMake's files for Radix: the toolchain file, and the platform
    /// files it finds Radix's description in.
    pub fn cmake(&self) -> PathBuf {
        self.dir.join("cmake")
    }

    /// The CMake toolchain file, for `-DCMAKE_TOOLCHAIN_FILE=`.
    pub fn cmake_toolchain_file(&self) -> PathBuf {
        self.cmake().join("radix.cmake")
    }

    /// Cargo for Radix: the wrapper that builds programs on Rust's
    /// standard library.
    pub fn cargo(&self) -> PathBuf {
        self.bin().join(format!("{TRIPLE}-cargo"))
    }

    /// The sysroot of Rust's standard library for Radix, which the
    /// rust-std port builds, with the crates the ports patch for Radix.
    pub fn rust(&self) -> PathBuf {
        self.dir.join("rust")
    }

    /// The `[patch]` configurations of the crates the ports patch for
    /// Radix, one per crate, named `<name>-<version>.toml`.
    pub fn crate_patches(&self) -> PathBuf {
        self.rust().join("patches")
    }

    /// Go for Radix: the wrapper that builds programs in Go (ADR 0600).
    pub fn go(&self) -> PathBuf {
        self.bin().join(format!("{TRIPLE}-go"))
    }

    /// The Go modules the go-modules port patches for Radix, each in a
    /// folder named as [`GoModule::dir`] names it, and their `list`.
    pub fn go_modules(&self) -> PathBuf {
        self.dir.join("go-modules")
    }

    /// The autoconf site file for Radix, and what else configure scripts
    /// need beyond the wrappers.
    pub fn autotools(&self) -> PathBuf {
        self.dir.join("autotools")
    }

    /// Writes the wrapper scripts and cross files into `out/toolchain`.
    pub fn write(&self, ctx: &Ctx) -> Result<()> {
        process::create_dir(&self.bin())?;
        process::create_dir(&self.dir.join("meson"))?;
        process::create_dir(&self.autotools())?;
        process::create_dir(&self.cmake().join("Platform"))?;
        let sysroot = ctx.out.join("sysroot");
        let maps = crate::repro::cc_flags(ctx);
        write_script(&self.cc(), &self.compiler_script(&sysroot, &maps, false))?;
        write_script(&self.cxx(), &self.compiler_script(&sysroot, &maps, true))?;
        let script = |name: &str, path: &Path| {
            format!(
                "#!/bin/sh\n# Written by ob (ob(1), PORTS): the Radix toolchain's {name}.\n\
                 exec '{}' \"$@\"\n",
                path.display()
            )
        };
        for (name, path) in &self.tools {
            write_script(&self.bin().join(format!("{TRIPLE}-{name}")), &script(name, path))?;
        }
        // The C library is built for no OS, for which Clang hands linking
        // to the host's GCC, and GCC takes `-fuse-ld=lld` to mean the first
        // `ld.lld` on PATH: this one, the toolchain's own LLD, rather than
        // whichever the host has, if any (Ubuntu's lld-20 installs only
        // `ld.lld-20`).
        write_script(&self.bin().join("ld.lld"), &script("ld", &self.tools["ld"]))?;
        write_script(&self.cargo(), &self.cargo_script(ctx)?)?;
        write_script(&self.go(), &self.go_script(ctx))?;
        // Ports with GNU configure scripts (autoconf's) run them with this.
        write_script(
            &self.bin().join(format!("{TRIPLE}-configure")),
            include_str!("../autotools/configure.sh"),
        )?;
        write_file(
            &self.autotools().join("config.site"),
            include_str!("../autotools/config.site"),
        )?;
        write_file(&self.libc_cross_file(), &self.libc_cross(&sysroot, &maps))?;
        write_file(&self.cross_file(), &self.programs_cross())?;
        self.write_cmake(&sysroot)?;
        Ok(())
    }

    /// The C compiler: Clang for Linux's x86-64 ABI, which is Radix's
    /// (radix-posix(7)), with Linux's own macros taken away and Radix's
    /// given, the sysroot's headers only, and, when it links, the C
    /// library's start files, the C library and compiler-rt's builtins,
    /// without the functions and data it never uses (each is in a section
    /// of its own, as the C library's are, for the linker to drop). It
    /// links a static position-independent program, as system programs
    /// are; with `-dynamic`, which it takes away, a program that the
    /// dynamic linker starts (`/system/libraries/ld.so`), and with `-shared` a
    /// shared library, both on the shared C library, for bundles that ship
    /// libraries of their own (ADR 0071). Linux's target, rather than
    /// none, makes Clang link with LLD itself (for no OS it hands linking
    /// to the host's GCC). It names the checkout and the build directory by
    /// the names of [`crate::repro`] in `__FILE__`, assertions and debug
    /// information, so that a program does not depend on where it was
    /// built (ADR 0535).
    ///
    /// The C++ compiler is the same with Clang's C++ driver, libc++'s
    /// headers before the C library's (as libc++ needs: its own
    /// `<stdlib.h>` and the like wrap the C library's), and, when it
    /// links, libc++, libc++abi and libunwind before the C library (ADR
    /// 0090). `-nostdinc++` leaves out the headers and `-nostdlib++` or
    /// `-nodefaultlibs` the libraries, as libc++'s own build asks.
    fn compiler_script(&self, sysroot: &Path, maps: &[String], cxx: bool) -> String {
        let lib = sysroot.join("system/libraries");
        let (what, clang) =
            if cxx { ("C++ compiler", self.clangxx()) } else { ("C compiler", self.clang.clone()) };
        // What only C++ adds: the arguments that leave out libc++, its
        // headers, and its libraries.
        let (cases, headers, libraries) = if cxx {
            (
                "\t-nostdinc++)\n\t\tcxxinc= ;;\n\
                 \t-nostdlib++ | -nodefaultlibs)\n\t\tcxxlib= ;;\n",
                format!(
                    "[ -n \"$cxxinc\" ] && set -- -isystem '{}/system/include/c++/v1' \"$@\"\n\
                     set -- -nostdinc++ \"$@\"\n",
                    sysroot.display()
                ),
                "-lc++ -lc++abi -lunwind",
            )
        } else {
            ("", String::new(), "")
        };
        let mut s = String::new();
        let _ = write!(
            s,
            r#"#!/bin/sh
# Written by ob (ob(1), PORTS): the Radix toolchain's {what}.
# {version}
link=yes
mode=static
cxxinc=yes
cxxlib=yes
for arg do
	shift
	case $arg in
	-c | -S | -E | -M | -MM | -fsyntax-only | -print-* | -dump* | --version | -v | -'###')
		link= ;;
	-shared)
		mode=shared ;;
	-dynamic)
		mode=dynamic
		continue ;;
	# The target is this script's to choose: the cc crate names Rust's
	# (x86_64-unknown-radix-mlibc), which Clang does not know.
	--target=*)
		continue ;;
{cases}	esac
	set -- "$@" "$arg"
done
set -- -isystem '{sysroot}/system/include' -isystem '{resource}/include' \
	-fPIE -ffunction-sections -fdata-sections {maps}"$@"
{headers}set -- --target=x86_64-unknown-linux-musl \
	-U__linux__ -U__linux -Ulinux -U__gnu_linux__ \
	-D__radix__ -D__dukovo__ \
	--sysroot='{sysroot}' -nostdinc "$@"
if [ -n "$link" ]; then
	# The C library goes in as the program does, whichever way the
	# arguments left the linker (rustc's end with -Bdynamic).
	case $mode in
	static) start='{lib}/Scrt1.o' libc=-Bstatic; set -- -static-pie "$@" ;;
	dynamic) start='{lib}/Scrt1.o' libc=-Bdynamic
		set -- -pie -Wl,--dynamic-linker=/system/libraries/ld.so -Wl,--as-needed "$@" ;;
	shared) start= libc=-Bdynamic; set -- -Wl,--as-needed "$@" ;;
	esac
	[ -n "$cxxlib" ] && cxx='{libraries}' || cxx=
	set -- -fuse-ld=lld -nostdlib -Wl,-z,max-page-size=4096 -Wl,--gc-sections \
		${{start:+"$start"}} '{lib}/crti.o' "$@" \
		-L'{lib}' -Wl,$libc $cxx -lc '{builtins}' '{lib}/crtn.o'
fi
exec '{clang}' "$@"
"#,
            version = self.version,
            maps = maps.iter().fold(String::new(), |all, flag| format!("{all}'{flag}' ")),
            sysroot = sysroot.display(),
            resource = self.resource.display(),
            lib = lib.display(),
            builtins = self.builtins.display(),
            clang = clang.display(),
        );
        s
    }

    /// CMake's toolchain file for Radix, and the platform files that
    /// describe it: a Unix whose programs and libraries are built as
    /// Linux's are (Radix's ABI is Linux's, radix-posix(7)), with its
    /// files under `/system` in the sysroot, where nothing of the host's
    /// is looked for.
    fn write_cmake(&self, sysroot: &Path) -> Result<()> {
        let tool = |name: &str| self.bin().join(format!("{TRIPLE}-{name}"));
        let mut text = format!(
            "# Written by ob (ob(1), PORTS): CMake's toolchain file for Radix.\n\
             list(APPEND CMAKE_MODULE_PATH \"{cmake}\")\n\
             set(CMAKE_SYSTEM_NAME Radix)\n\
             set(CMAKE_SYSTEM_PROCESSOR x86_64)\n\
             set(CMAKE_C_COMPILER \"{cc}\")\n\
             set(CMAKE_CXX_COMPILER \"{cxx}\")\n",
            cmake = self.cmake().display(),
            cc = self.cc().display(),
            cxx = self.cxx().display(),
        );
        for name in ["ar", "ranlib", "nm", "strip", "objcopy", "readelf"] {
            let _ =
                writeln!(text, "set(CMAKE_{} \"{}\")", name.to_uppercase(), tool(name).display());
        }
        let _ = write!(
            text,
            "set(CMAKE_SYSROOT \"{sysroot}\")\n\
             set(CMAKE_FIND_ROOT_PATH \"{sysroot}\")\n\
             set(CMAKE_FIND_ROOT_PATH_MODE_PROGRAM NEVER)\n\
             set(CMAKE_FIND_ROOT_PATH_MODE_LIBRARY ONLY)\n\
             set(CMAKE_FIND_ROOT_PATH_MODE_INCLUDE ONLY)\n\
             set(CMAKE_FIND_ROOT_PATH_MODE_PACKAGE ONLY)\n",
            sysroot = sysroot.display(),
        );
        write_file(&self.cmake_toolchain_file(), &text)?;
        // Read before the compilers are checked: what Radix is, and whose
        // compiler and linker settings it takes (CMake 3.28 and later).
        write_file(
            &self.cmake().join("Platform/Radix-Initialize.cmake"),
            "# Written by ob (ob(1), PORTS): Radix, for CMake.\n\
             set(UNIX 1)\n\
             set(CMAKE_EFFECTIVE_SYSTEM_NAME Linux)\n",
        )?;
        write_file(
            &self.cmake().join("Platform/Radix.cmake"),
            "# Written by ob (ob(1), PORTS): Radix, for CMake: Linux's\n\
             # settings, with /system first among the places to look.\n\
             include(Platform/Linux)\n\
             list(PREPEND CMAKE_SYSTEM_PREFIX_PATH /system)\n",
        )
    }

    /// The link to the toolchain's Rust folder that Cargo for Radix makes
    /// in a workspace's folder, and gives the patched crates through.
    pub const RUST_LINK: &str = ".radix-rust";

    /// Cargo for programs on Rust's standard library (ADR 0070), as
    /// `cargo` is used: `x86_64-radix-cargo build --release`. It builds for
    /// the x86_64-unknown-radix target against the rust-std port's
    /// sysroot, with the crates the ports patch for Radix in place of
    /// crates.io's (those the workspace's Cargo.lock names, so Cargo has no
    /// unused patches to warn of; all of them before there is one), given
    /// through a link to the toolchain's Rust folder, `.radix-rust`, in the
    /// workspace's folder, and links with the toolchain's C compiler, which
    /// the cc crate builds C with too. The pinned Rust toolchain builds,
    /// whatever a port's tree pins.
    fn cargo_script(&self, ctx: &Ctx) -> Result<String> {
        let pin = ctx.root.join("rust-toolchain.toml");
        let text =
            std::fs::read_to_string(&pin).context(|| format!("reading {}", pin.display()))?;
        let channel = text
            .lines()
            .find_map(|line| line.strip_prefix("channel = "))
            .map(|value| value.trim().trim_matches('"').to_string())
            .ok_or_else(|| Error::new(format!("{} names no channel", ctx.display(&pin))))?;
        let tool = |name: &str| self.bin().join(format!("{TRIPLE}-{name}"));
        Ok(format!(
            r#"#!/bin/sh
# Written by ob (ob(1), PORTS): Cargo for Radix's programs on Rust's
# standard library.
if [ $# -eq 0 ]; then
	echo "usage: {TRIPLE}-cargo command [argument...]" >&2
	exit 2
fi
command=$1
shift
# The workspace's Cargo.lock: beside --manifest-path, or here or above.
dir=.
take=
for arg do
	[ -n "$take" ] && dir=$(dirname "$arg")
	take=
	case $arg in
	--manifest-path) take=yes ;;
	--manifest-path=*) dir=$(dirname "${{arg#*=}}") ;;
	esac
done
dir=$(cd "$dir" && pwd) || exit
lock=
while [ -z "$lock" ]; do
	if [ -f "$dir/Cargo.lock" ]; then
		lock=$dir/Cargo.lock
	elif [ "$dir" = / ]; then
		break
	fi
	dir=$(dirname "$dir")
done
# The patched crates, which are in the toolchain's Rust folder, are given
# through a link to it in the workspace's folder: Cargo names a crate
# inside the workspace by its path from there, and one outside by its
# whole path, which every name Cargo gives what it builds would then hold
# (ADR 0535).
link=
if [ -n "$lock" ]; then
	link=${{lock%/*}}/{link}
	if [ "$(readlink "$link" 2>/dev/null)" != '{rust}' ]; then
		if [ -e "$link" ] && [ ! -L "$link" ]; then
			link=
		else
			ln -sfn '{rust}' "$link" 2>/dev/null || link=
		fi
	fi
fi
for patch in '{patches}'/*.toml; do
	[ -f "$patch" ] || continue
	crate=${{patch##*/}}
	crate=${{crate%.toml}}
	name=${{crate%-*}}
	if [ -n "$lock" ] && ! grep -qx "name = \"$name\"" "$lock"; then
		continue
	fi
	path=$(sed -n 's/^path = "\(.*\)"$/\1/p' "$patch")
	rest=${{path#'{rust}'/}}
	if [ -n "$link" ] && [ "$rest" != "$path" ] && [ -d "$link/$rest" ]; then
		table=$name-$(printf '%s' "${{crate##*-}}" | tr . _)
		set -- --config "patch.crates-io.$table.package=\"$name\"" \
			--config "patch.crates-io.$table.path=\"$link/$rest\"" "$@"
	else
		set -- --config "$patch" "$@"
	fi
done
case $command in
build | b | check | c | clippy | doc | d | fetch | run | r | rustc | test | t | tree)
	set -- --target '{spec}' -Zjson-target-spec "$@" ;;
esac
unset RUSTFLAGS CARGO_BUILD_RUSTFLAGS CARGO_BUILD_TARGET
us=$(printf '\037')
export RUSTUP_TOOLCHAIN='{channel}'
export CARGO_ENCODED_RUSTFLAGS='--sysroot={rust}'{remaps}
export CARGO_TARGET_X86_64_UNKNOWN_RADIX_LINKER='{cc}'
export CC_x86_64_unknown_radix='{cc}'
export AR_x86_64_unknown_radix='{ar}'
export CFLAGS_x86_64_unknown_radix=-O2
export CARGO_FUTURE_INCOMPAT_REPORT_FREQUENCY=never
exec cargo "$command" "$@"
"#,
            patches = self.crate_patches().display(),
            link = Self::RUST_LINK,
            // The flags are separated by the unit separator, as Cargo reads
            // them from CARGO_ENCODED_RUSTFLAGS.
            remaps = crate::repro::rustc_flags(ctx)
                .iter()
                .fold(String::new(), |all, flag| format!("{all}\"$us\"'{flag}'")),
            spec = ctx.root.join(crate::userspace::STD_TARGET_SPEC).display(),
            rust = self.rust().display(),
            cc = self.cc().display(),
            ar = tool("ar").display(),
        ))
    }

    /// The link to the toolchain's patched Go modules that Go for Radix
    /// makes in a module's folder, and gives them through.
    pub const GO_LINK: &str = ".radix-go";

    /// Go for programs in Go (ADR 0600), as `go` is used in a module's
    /// folder: `x86_64-radix-go build -o prog ./cmd/prog`. It builds for
    /// GOOS=radix with the toolchain's Go, and gives the program the
    /// modules the go-modules port patches for Radix in place of the
    /// proxy's, those its go.sum names, through a module file of its own
    /// beside go.mod, `.radix-go.mod`: go.mod and its `replace` lines. A
    /// patched module whose code needs a newer Go than go.mod's `go` line
    /// raises it there, with `godebug default=` the old line, so the
    /// program keeps the run-time defaults of the Go it names. Every other
    /// module comes from Go's module proxy, checked against go.sum, which
    /// nothing changes: one go.sum does not name stops the build. With
    /// `--host` first, it builds for the build machine instead, with the
    /// same modules: a generator a program runs as it builds, say.
    fn go_script(&self, ctx: &Ctx) -> String {
        let go_root = self.dir.join("go");
        let caches = ctx.target.join("go");
        format!(
            r#"#!/bin/sh
# Written by ob (ob(1), PORTS): Go for Radix's programs.
set -eu
me={TRIPLE}-go
modules='{modules}'
host=
if [ "${{1:-}}" = --host ]; then
	host=yes
	shift
fi
# The module: go.mod here or above.
dir=$(pwd)
until [ -f "$dir/go.mod" ]; do
	if [ "$dir" = / ]; then
		echo "$me: no go.mod in $(pwd) or above" >&2
		exit 1
	fi
	dir=$(dirname "$dir")
done
case $dir in
*[[:space:]]*)
	echo "$me: $dir holds a space, which GOFLAGS cannot" >&2
	exit 1
	;;
esac
# The patched modules are given through a link to them in the module's
# folder, so that the program's build information names them by a path
# from there, whatever the folder is (ADR 0535).
link=$dir/{link}
if [ -e "$link" ] && [ ! -L "$link" ]; then
	echo "$me: $link is in the way" >&2
	exit 1
fi
ln -sfn "$modules" "$link"
sum=$dir/go.sum
[ -f "$sum" ] || sum=/dev/null
# go.sum names a module as `PATH VERSION h1:...` or `PATH VERSION/go.mod
# h1:...`; the list, as `PATH VERSION FOLDER all|one GO`, GO being the
# oldest Go its code builds with. Go compares versions such as 1.22 and
# 1.19.0 number by number.
awk -v list="$modules/list" -v me="$me" '
function older(a, b, x, y, n, m, i) {{
	n = split(a, x, ".")
	m = split(b, y, ".")
	for (i = 1; i <= (n > m ? n : m); i++) {{
		if (x[i] + 0 != y[i] + 0) return x[i] + 0 < y[i] + 0
	}}
	return 0
}}
BEGIN {{
	while ((getline line < list) > 0) {{
		split(line, f, " ")
		count++
		path[count] = f[1]; version[count] = f[2]; folder[count] = f[3]
		every[count] = f[4] == "all"; needs[count] = f[5]
	}}
}}
FILENAME == ARGV[1] {{
	v = $2
	sub(/\/go\.mod$/, "", v)
	named[$1] = 1
	named[$1 " " v] = 1
	next
}}
/^go[ \t]/ {{ go = $2 }}
/^replace[ \t]*\(/ {{ block = 1; lines[++total] = $0; next }}
block && /^[ \t]*\)/ {{ block = 0 }}
block && NF {{ replaced[$1] = 1 }}
$1 == "replace" && !block {{ replaced[$2] = 1 }}
{{ lines[++total] = $0 }}
END {{
	old = go == "" ? "1.16" : go
	top = old
	for (i = 1; i <= count; i++) {{
		if (!(every[i] ? named[path[i]] : named[path[i] " " version[i]])) continue
		if (path[i] in replaced) {{
			print me ": go.mod replaces " path[i] ", which Radix needs patched" > "/dev/stderr"
			exit 1
		}}
		use[i] = 1
		if (older(top, needs[i])) top = needs[i]
	}}
	for (j = 1; j <= total; j++) {{
		if (top != old && lines[j] ~ /^go[ \t]/) print "go " top
		else print lines[j]
	}}
	if (top != old) {{
		if (go == "") print "go " top
		print "godebug default=go" old
	}}
	for (i = 1; i <= count; i++) {{
		if (use[i]) print "replace " path[i] (every[i] ? "" : " " version[i]) " => ./{link}/" folder[i]
	}}
}}' "$sum" "$dir/go.mod" >"$dir/.radix-go.mod"
cp "$sum" "$dir/.radix-go.sum"
unset GOBIN GODEBUG GOEXPERIMENT GOFIPS140 GOINSECURE GONOPROXY GONOSUMDB GOPRIVATE GOVCS
unset GOOS GOARCH GOAMD64
[ -n "$host" ] || export GOOS=radix GOARCH=amd64 GOAMD64=v1
export CGO_ENABLED=0
export GOROOT='{root}' GOTOOLCHAIN=local GOTELEMETRY=off GOENV=off GOWORK=off
export GOPROXY={proxy} GOSUMDB=off
export GOFLAGS="-mod=readonly -modfile=$dir/.radix-go.mod -trimpath -buildvcs=false -modcacherw"
export GOCACHE='{cache}' GOMODCACHE='{modcache}' GOPATH='{path}'
exec '{root}/bin/go' "$@"
"#,
            modules = self.go_modules().display(),
            link = Self::GO_LINK,
            root = go_root.display(),
            proxy = GO_PROXY,
            cache = caches.join("cache").display(),
            modcache = go_module_cache(ctx).display(),
            path = caches.join("path").display(),
        )
    }

    /// The cross file the C library is built with: Clang for no OS, so
    /// that nothing of Linux's (ifuncs, say) leaks in; `unknown-attributes`
    /// as an error makes mlibc's ifunc check fail, as it must there. The
    /// sysroot's headers come after all others, for the kernel's
    /// (`radix/`) only: the C library has its own.
    fn libc_cross(&self, sysroot: &Path, maps: &[String]) -> String {
        let clangxx = self.clangxx();
        format!(
            "# Written by ob (ob(1), PORTS): Meson's cross file for the C library.\n\
             [binaries]\n\
             c = ['{clang}', '--target=x86_64-unknown-none-elf', '-nostdinc', '-isystem', '{res}/include']\n\
             cpp = ['{clangxx}', '--target=x86_64-unknown-none-elf', '-nostdinc', '-nostdinc++', \
             '-isystem', '{cxx}', '-isystem', '{res}/include', '-D_GNU_SOURCE']\n\
             ar = '{ar}'\n\
             strip = '{strip}'\n\
             c_ld = 'lld'\n\
             cpp_ld = 'lld'\n\
             \n\
             [built-in options]\n\
             c_args = ['-fPIC', '-ffunction-sections', '-fdata-sections', '-D__radix__', \
             '-D__dukovo__', '-D__unix__', '-idirafter', '{radix}'{maps}]\n\
             cpp_args = ['-fPIC', '-ffunction-sections', '-fdata-sections', \
             '-Werror=unknown-attributes', '-D__radix__', '-D__dukovo__', '-D__unix__', \
             '-idirafter', '{radix}'{maps}]\n\
             c_link_args = ['-nostdlib', '{builtins}']\n\
             cpp_link_args = ['-nostdlib', '{builtins}']\n\
             \n{host}",
            clang = self.clang.display(),
            clangxx = clangxx.display(),
            res = self.resource.display(),
            cxx = self.cxx_headers().display(),
            ar = self.tools["ar"].display(),
            strip = self.tools["strip"].display(),
            radix = sysroot.join("system/include").display(),
            maps = maps.iter().fold(String::new(), |all, flag| format!("{all}, '{flag}'")),
            builtins = self.builtins.display(),
            host = HOST_MACHINE,
        )
    }

    /// The cross file programs and libraries are built with.
    fn programs_cross(&self) -> String {
        let tool = |name: &str| self.bin().join(format!("{TRIPLE}-{name}"));
        format!(
            "# Written by ob (ob(1), PORTS): Meson's cross file for Radix.\n\
             [binaries]\n\
             c = '{cc}'\n\
             cpp = '{cxx}'\n\
             ar = '{ar}'\n\
             strip = '{strip}'\n\
             nm = '{nm}'\n\
             \n{host}",
            cc = self.cc().display(),
            cxx = self.cxx().display(),
            ar = tool("ar").display(),
            strip = tool("strip").display(),
            nm = tool("nm").display(),
            host = HOST_MACHINE,
        )
    }
}

/// Meson's description of the machine programs run on.
const HOST_MACHINE: &str = "[host_machine]\n\
system = 'radix'\n\
cpu_family = 'x86_64'\n\
cpu = 'x86_64'\n\
endian = 'little'\n";

pub fn write_file(path: &Path, text: &str) -> Result<()> {
    std::fs::write(path, text).context(|| format!("writing {}", path.display()))
}

fn write_script(path: &Path, text: &str) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    write_file(path, text)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
        .context(|| format!("making {} executable", path.display()))
}

/// The ports, built, and the toolchain they were built with.
pub struct Ports {
    pub toolchain: Toolchain,
    /// Every port, each after those it needs.
    pub ports: Vec<Port>,
}

/// Builds every port that changed, in order, and returns them all.
pub fn build_all(ctx: &Ctx, rebuild: &[String]) -> Result<Ports> {
    build_ports(ctx, rebuild, false)
}

/// Builds every port that changed, as [`build_all`] does, and packs the
/// bundles of the ports the catalog carries into out/bundles (ADR 0370).
pub fn build_and_pack(ctx: &Ctx, rebuild: &[String]) -> Result<Ports> {
    build_ports(ctx, rebuild, true)
}

fn build_ports(ctx: &Ctx, rebuild: &[String], pack: bool) -> Result<Ports> {
    let toolchain = find_toolchain(ctx)?;
    toolchain.write(ctx)?;
    let mut stamps: BTreeMap<String, u64> = BTreeMap::new();
    let mut ports = Vec::new();
    for (recipe, dir) in recipes(ctx)? {
        let source = &recipe.source;
        let source = if source.rust_src {
            Some(rust_src(ctx)?.0)
        } else if !source.url.is_empty() {
            Some(fetch_archive(ctx, &recipe.name, source)?)
        } else if source.is_image() {
            Some(fetch_image(ctx, &recipe.name, source)?)
        } else if !source.git.is_empty() {
            Some(fetch(ctx, &recipe.name, &source.git, &source.commit, &source.paths)?)
        } else {
            None
        };
        let mut extras = Vec::new();
        for extra in &recipe.extra {
            let name = extra.path.rsplit('/').next().unwrap_or(&extra.path);
            extras.push(fetch(ctx, name, &extra.git, &extra.commit, &[])?);
        }
        let mut crates = Vec::new();
        for krate in &recipe.crates {
            crates.push(fetch_crate(ctx, krate)?);
        }
        let mut modules = Vec::new();
        for module in &recipe.modules {
            modules.push(fetch_go_module(ctx, module)?);
        }
        let port = Port { recipe, dir, source, extras, crates, modules };
        let recipe = &port.recipe;
        let stamp = inputs_hash(ctx, &port.dir, &toolchain, recipe, &stamps)?;
        let stamp_file = ctx.out.join("ports").join(format!("{}.stamp", recipe.name));
        let built = std::fs::read_to_string(&stamp_file).ok() == Some(format!("{stamp:016x}\n"));
        // A bundle's port is packed from the folder it installed into, and
        // its licenses from the sources of the crates it linked, in
        // Cargo's registry, and of the Go modules, in Go's module cache. A
        // cache can bring the stamp back without either: CI's prunes the
        // registry to the workspace's crates.
        let staged = !pack
            || recipe.bundle.is_none()
            || (crate::bundles::staging(ctx, &recipe.name).is_dir()
                && crate::licenses::crates_fetched(ctx, &recipe.name)
                && crate::licenses::go_modules_fetched(ctx, &recipe.name));
        if built && staged && !rebuild.contains(&recipe.name) {
            step(&format!("Port {} {} is up to date", recipe.name, recipe.version));
        } else {
            build(ctx, &toolchain, &port)?;
            // A port of Rust's standard library must agree with the C
            // library it calls.
            if recipe.source.rust_src {
                crate::libc_check::check(ctx, &toolchain)?;
            }
            process::create_dir(&ctx.out.join("ports"))?;
            write_file(&stamp_file, &format!("{stamp:016x}\n"))?;
        }
        stamps.insert(recipe.name.clone(), stamp);
        ports.push(port);
    }
    for name in rebuild {
        if !stamps.contains_key(name) {
            return Err(Error::new(format!("no port is named {name}"))
                .hint("the ports are the directories of ports/"));
        }
    }
    let ports = Ports { toolchain, ports };
    if pack {
        crate::bundles::pack_all(ctx, &ports)?;
    }
    Ok(ports)
}

/// The environment variables that name the toolchain's programs for a
/// port's build.sh, and the wrapper each names. A wrapper is named as it
/// is on `PATH`, where the toolchain's folder comes first, and not by
/// where it is: configure records the compiler as it is given, and some
/// programs keep that record (`jq --build-configuration`, `curl-config`),
/// which would then hold where the checkout is (ADR 0535).
const BUILD_TOOLS: [(&str, &str); 7] = [
    ("CC", "cc"),
    ("CXX", "c++"),
    ("AR", "ar"),
    ("RANLIB", "ranlib"),
    ("NM", "nm"),
    ("STRIP", "strip"),
    ("OBJCOPY", "objcopy"),
];

/// The time every port is built at, as `SOURCE_DATE_EPOCH`: 2026-01-01
/// 00:00:00 UTC, whatever the commit. What a port records of its build,
/// a `__DATE__` or a manual page's date, is then the same at every
/// commit, so that a port cached by its stamp at one commit is the port
/// another builds from the same recipe (ADR 0535). It is part of every
/// port's stamp: changing it builds every port again. The programs on
/// Rust's standard library are built at it too
/// ([`crate::userspace::std_cargo`]).
pub const PORT_TIME: &str = "1767225600";

/// What went into a port's build: its directory's files, and those of a
/// folder in it that is a link to another's, the toolchain
/// (its version, and the wrappers, cross files and site file ob writes),
/// what the environment ob gives build.sh names the toolchain's programs
/// and how its Rust crates are named, the time it is built at
/// ([`PORT_TIME`]), the kernel's system call headers, and the builds of
/// the ports it needs.
fn inputs_hash(
    ctx: &Ctx,
    dir: &Path,
    toolchain: &Toolchain,
    recipe: &Recipe,
    stamps: &BTreeMap<String, u64>,
) -> Result<u64> {
    let mut hasher = DefaultHasher::new();
    let headers = ctx.out.join("sysroot/system/include/radix");
    let (wrappers, cross) = (toolchain.bin(), toolchain.dir.join("meson"));
    let (autotools, cmake) = (toolchain.autotools(), toolchain.cmake());
    for dir in [dir, &wrappers, &cross, &autotools, &cmake, &headers] {
        // The system call headers carry the interface's documentation,
        // which no port compiles: leaving it out means a change to a
        // comment in abi/ does not rebuild every port.
        hash_folder(dir, dir == headers, &mut hasher)?;
    }
    toolchain.version.hash(&mut hasher);
    toolchain.builtins.hash(&mut hasher);
    BUILD_TOOLS.hash(&mut hasher);
    crate::rustc_wrapper::FORMAT.hash(&mut hasher);
    PORT_TIME.hash(&mut hasher);
    if recipe.source.rust_src {
        rust_src(ctx)?.1.hash(&mut hasher);
    }
    for dep in &recipe.depends {
        stamps.get(dep).hash(&mut hasher);
    }
    Ok(hasher.finish())
}

/// Hashes the files of `dir`, each by its path in it and its bytes, or its
/// C source without comments when `code_only`. A folder in it that is a
/// link to another folder, as go-radix's `patches` is the go port's (ADR
/// 0356), counts by where it points and by the files there, so that a
/// change to them builds the port again.
fn hash_folder(dir: &Path, code_only: bool, hasher: &mut DefaultHasher) -> Result<()> {
    let mut files = Vec::new();
    list_files(dir, &mut files)?;
    files.sort();
    for file in &files {
        file.strip_prefix(dir).unwrap_or(file).hash(hasher);
        // list_files lists a link itself: one that leads to a folder is
        // the only kind that is a folder here.
        if file.is_dir() {
            let target =
                std::fs::read_link(file).context(|| format!("reading {}", file.display()))?;
            target.hash(hasher);
            hash_folder(file, code_only, hasher)?;
            continue;
        }
        let bytes = std::fs::read(file).context(|| format!("reading {}", file.display()))?;
        if code_only {
            without_c_comments(&bytes).hash(hasher);
        } else {
            bytes.hash(hasher);
        }
    }
    Ok(())
}

/// C source as the compiler sees it, for hashing: each comment and each run
/// of spaces between tokens becomes one space, or one newline if it spans
/// lines, so that adding or rewording a comment changes nothing. String
/// and character literals are kept whole, so a `/*` inside one is not
/// taken for a comment.
fn without_c_comments(source: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(source.len());
    let mut at = 0;
    while at < source.len() {
        let gap_start = at;
        let mut newline = false;
        // Skip a run of spaces and comments, noting whether it ends a line.
        loop {
            match (source.get(at), source.get(at + 1)) {
                (Some(b'/'), Some(b'*')) => {
                    let end = source[at + 2..].windows(2).position(|pair| pair == b"*/");
                    let next = end.map_or(source.len(), |end| at + 2 + end + 2);
                    newline |= source[at..next].contains(&b'\n');
                    at = next;
                }
                (Some(b'/'), Some(b'/')) => {
                    let end = source[at..].iter().position(|&byte| byte == b'\n');
                    at = end.map_or(source.len(), |end| at + end);
                }
                (Some(&byte), _) if byte.is_ascii_whitespace() => {
                    newline |= byte == b'\n';
                    at += 1;
                }
                _ => break,
            }
        }
        if at > gap_start {
            out.push(if newline { b'\n' } else { b' ' });
            continue;
        }
        let start = at;
        if matches!(source[at], b'"' | b'\'') {
            // The literal runs to the next quote of its kind that no
            // backslash escapes.
            let quote = source[at];
            at += 1;
            while at < source.len() && source[at] != quote {
                at += if source[at] == b'\\' { 2 } else { 1 };
            }
            at = (at + 1).min(source.len());
        } else {
            at += 1;
        }
        out.extend_from_slice(&source[start..at]);
    }
    out
}

pub fn list_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in std::fs::read_dir(dir).context(|| format!("reading {}", dir.display()))? {
        let entry = entry.context(|| format!("reading {}", dir.display()))?;
        let path = entry.path();
        // A symbolic link is listed itself, even one to a directory.
        let kind = entry.file_type().context(|| format!("reading {}", path.display()))?;
        if kind.is_dir() {
            list_files(&path, out)?;
        } else {
            out.push(path);
        }
    }
    Ok(())
}

/// The pinned Rust toolchain's rust-src library, and its `rustc -vV`,
/// which says which it is.
fn rust_src(ctx: &Ctx) -> Result<(PathBuf, String)> {
    let mut cmd = Command::new("rustc");
    cmd.current_dir(&ctx.root).arg("-vV");
    let version = String::from_utf8_lossy(&process::output(ctx, &mut cmd)?.stdout).into_owned();
    let mut cmd = Command::new("rustc");
    cmd.current_dir(&ctx.root).args(["--print", "sysroot"]);
    let sysroot = String::from_utf8_lossy(&process::output(ctx, &mut cmd)?.stdout).into_owned();
    let library = PathBuf::from(sysroot.trim()).join("lib/rustlib/src/rust/library");
    if !library.join("vendor").is_dir() {
        return Err(Error::new(format!("rust-src is not in {}", library.display()))
            .hint("run `./ob setup`, which installs the toolchain's rust-src component"));
    }
    Ok((library, version))
}

/// Fetches commit `commit` of `git` into `target/ports/sources`, once,
/// and checks that it is that commit; with `paths`, only the files of
/// those directories. Returns where it is.
pub fn fetch(ctx: &Ctx, name: &str, git: &str, commit: &str, paths: &[String]) -> Result<PathBuf> {
    let dir = ctx.target.join("ports/sources").join(format!("{name}-{}", &commit[..12]));
    // The commit object too, which a plain `rev-parse HEAD` does not
    // read: a clone that has lost its objects is fetched again.
    let head = |dir: &Path| -> Option<String> {
        let mut cmd = Command::new("git");
        cmd.arg("-C").arg(dir).args(["rev-parse", "--verify", "--quiet", "HEAD^{commit}"]);
        let out = cmd.output();
        out.ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
    };
    let git_in = |args: &[&str]| {
        let mut cmd = Command::new("git");
        cmd.arg("-C").arg(&dir).args(args);
        cmd
    };
    // In cone mode, which takes directories, each with what is beneath it.
    let sparse = || {
        let mut cmd = git_in(&["sparse-checkout", "set", "--cone", "--"]);
        cmd.args(paths);
        process::run(ctx, &mut cmd)
    };
    if dir.join(".git").is_dir() && head(&dir).as_deref() == Some(commit) {
        // The recipe may name other directories than it did.
        if !paths.is_empty() {
            sparse()?;
        }
        return Ok(dir);
    }
    step(&format!("Fetching {name} {} from {git}", &commit[..12]));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).context(|| format!("removing {}", dir.display()))?;
    }
    process::create_dir(&dir)?;
    process::run(ctx, &mut git_in(&["init", "--quiet"]))?;
    if paths.is_empty() {
        process::run(ctx, &mut git_in(&["fetch", "--quiet", "--depth", "1", git, commit]))?;
    } else {
        // The commit and its trees only; git fetches the files of `paths`
        // as it checks them out, from the remote it records for that.
        process::run(ctx, &mut git_in(&["remote", "add", "origin", git]))?;
        let fetch = ["fetch", "--quiet", "--depth", "1", "--filter=blob:none", "origin", commit];
        process::run(ctx, &mut git_in(&fetch))?;
        sparse()?;
    }
    process::run(ctx, &mut git_in(&["checkout", "--quiet", "--detach", "FETCH_HEAD"]))?;
    match head(&dir) {
        Some(found) if found == commit => Ok(dir),
        found => Err(Error::new(format!(
            "{git} gave {} for commit {commit}",
            found.unwrap_or_else(|| "nothing".into())
        ))),
    }
}

/// The source of the port `name`, from its release archive, fetched if
/// it is not yet: for builds of it outside Radix, such as `ob bench`'s
/// git for Linux.
pub fn archive_source(ctx: &Ctx, name: &str) -> Result<PathBuf> {
    let Some((recipe, _)) = recipes(ctx)?.into_iter().find(|(recipe, _)| recipe.name == name)
    else {
        return Err(Error::new(format!("there is no port {name}")));
    };
    fetch_archive(ctx, &recipe.name, &recipe.source)
}

/// Fetches a release archive into `target/ports/sources`, once, from its
/// URL or, failing that, from each mirror in turn, checks its SHA-256,
/// and unpacks it there. Returns where it is unpacked: the directory the
/// archive holds, as a release archive holds one, or else the whole.
fn fetch_archive(ctx: &Ctx, name: &str, source: &Source) -> Result<PathBuf> {
    let sources = ctx.target.join("ports/sources");
    let base = format!("{name}-{}", &source.sha256[..12]);
    // Unpacked beside its final place, then moved there, and the files
    // it holds noted beside it: a tree that has lost some since, as a
    // cache's cleanup cut short leaves one, is fetched again.
    let dir = sources.join(&base);
    let files = sources.join(format!("{base}.files"));
    if dir.is_dir() {
        if std::fs::read_to_string(&files).ok() == Some(listing(&dir)?) {
            return Ok(dir);
        }
        step(&format!("{} is not the tree ob unpacked; fetching it again", dir.display()));
        std::fs::remove_dir_all(&dir).context(|| format!("removing {}", dir.display()))?;
    }
    process::create_dir(&sources)?;
    let file = sources.join(format!("{base}.download"));
    let mut failed = Vec::new();
    for url in std::iter::once(&source.url).chain(&source.mirrors) {
        step(&format!("Fetching {name} from {url}"));
        let mut cmd = Command::new("curl");
        cmd.args(["--proto", "=https", "--tlsv1.2", "-fsSL", "--retry", "3", "-o"])
            .arg(&file)
            .arg(url);
        if let Err(err) = process::run(ctx, &mut cmd) {
            failed.push(format!("{url}: {err}"));
            continue;
        }
        let sum = process::sha256(ctx, &file)?;
        if sum == source.sha256 {
            failed.clear();
            break;
        }
        failed.push(format!("{url} has SHA-256 {sum}, not the {} the recipe pins", source.sha256));
    }
    if !failed.is_empty() {
        let _ = std::fs::remove_file(&file);
        return Err(Error::new(format!(
            "no copy of {name}'s source would do: {}",
            failed.join("; ")
        ))
        .hint("add a mirror that serves the same archive to its recipe.toml"));
    }
    let partial = sources.join(format!("{base}.partial"));
    if partial.exists() {
        std::fs::remove_dir_all(&partial).context(|| format!("removing {}", partial.display()))?;
    }
    process::create_dir(&partial)?;
    // tar finds out how the archive is compressed.
    let mut cmd = Command::new("tar");
    cmd.arg("-xf").arg(&file).arg("-C").arg(&partial).arg("--no-same-owner");
    process::run(ctx, &mut cmd)?;
    let entries: Vec<PathBuf> = std::fs::read_dir(&partial)
        .context(|| format!("reading {}", partial.display()))?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .collect();
    let top = match entries.as_slice() {
        [one] if one.is_dir() => one.clone(),
        _ => partial.clone(),
    };
    std::fs::rename(&top, &dir).context(|| format!("moving it to {}", dir.display()))?;
    if partial.exists() {
        std::fs::remove_dir_all(&partial).context(|| format!("removing {}", partial.display()))?;
    }
    let _ = std::fs::remove_file(&file);
    write_file(&files, &listing(&dir)?)?;
    Ok(dir)
}

/// Fetches the layers of the image `source` names into
/// `target/ports/sources`, once, each as the blob the registry serves by its
/// digest, `layer-1.blob`, `layer-2.blob` and so on in the order the image
/// stacks them, and checks each one's size and SHA-256 against the recipe.
/// Returns the folder that holds them. The blobs stay packed: build.sh
/// unpacks the files it keeps, in order, so that the rest of an image's
/// files are never written. A blob is renamed to its name only once it has
/// been checked, so a blob that is there with the pinned size is the one
/// that was checked.
fn fetch_image(ctx: &Ctx, name: &str, source: &Source) -> Result<PathBuf> {
    let first = source.layers.first().and_then(Layer::sha256).unwrap_or_default();
    let dir = ctx.target.join("ports/sources").join(format!("{name}-{}", &first[..12]));
    let blobs: Vec<(PathBuf, &Layer)> = source
        .layers
        .iter()
        .enumerate()
        .map(|(at, layer)| (dir.join(format!("layer-{}.blob", at + 1)), layer))
        .collect();
    let size = |file: &Path| std::fs::metadata(file).ok().map(|meta| meta.len());
    if blobs.iter().all(|(file, layer)| size(file) == Some(layer.size)) {
        return Ok(dir);
    }
    process::create_dir(&dir)?;
    let image = format!(
        "{}/{}:{}",
        source.registry,
        source.repository,
        source.tag.as_deref().unwrap_or_default()
    );
    step(&format!("Fetching the layers of {name}'s image, {image}"));
    let token = registry_token(ctx, source)?;
    // The token goes to curl in a file, so that no message of a failed
    // command shows it.
    let config = dir.join("curl.config");
    let _ = std::fs::remove_file(&config);
    crate::bundles::write_private(
        &config,
        &format!("header = \"Authorization: Bearer {token}\"\n"),
    )?;
    let fetched = blobs
        .iter()
        .filter(|(file, layer)| size(file) != Some(layer.size))
        .try_for_each(|(file, layer)| fetch_layer(ctx, source, &config, file, layer));
    let _ = std::fs::remove_file(&config);
    fetched?;
    Ok(dir)
}

/// Fetches the blob of `layer` from `source`'s registry into `file`, with
/// the request headers `config` holds, and checks its size and SHA-256.
fn fetch_layer(
    ctx: &Ctx,
    source: &Source,
    config: &Path,
    file: &Path,
    layer: &Layer,
) -> Result<()> {
    let url =
        format!("https://{}/v2/{}/blobs/{}", source.registry, source.repository, layer.digest);
    let partial = file.with_extension("partial");
    // The registry sends the blob from a storage service, to which curl
    // does not take the token.
    let mut cmd = Command::new("curl");
    cmd.args(["--proto", "=https", "--tlsv1.2", "-fsSL", "--retry", "3", "--max-filesize"])
        .arg(layer.size.to_string())
        .arg("--config")
        .arg(config)
        .arg("-o")
        .arg(&partial)
        .arg(&url);
    process::run(ctx, &mut cmd)?;
    let got = std::fs::metadata(&partial).ok().map(|meta| meta.len());
    let sum = process::sha256(ctx, &partial)?;
    if got != Some(layer.size) || Some(sum.as_str()) != layer.sha256() {
        let _ = std::fs::remove_file(&partial);
        return Err(Error::new(format!(
            "{url} has SHA-256 {sum} and {} bytes, not the {} and {} the recipe pins; not using it",
            got.unwrap_or(0),
            layer.digest,
            layer.size
        )));
    }
    std::fs::rename(&partial, file).context(|| format!("moving it to {}", file.display()))
}

/// A token that lets anyone pull `source`'s repository from its registry.
/// The registry's `/v2/` answers without one and names, in its challenge,
/// the service that gives tokens ([`bearer_challenge`]); that service gives
/// one for a pull of the repository with no account.
fn registry_token(ctx: &Ctx, source: &Source) -> Result<String> {
    let mut cmd = Command::new("curl");
    cmd.args(["--proto", "=https", "--tlsv1.2", "-sSI", "--retry", "3"])
        .arg(format!("https://{}/v2/", source.registry));
    let answer = process::output(ctx, &mut cmd)?;
    let headers = String::from_utf8_lossy(&answer.stdout);
    let Some((realm, service)) = bearer_challenge(&headers) else {
        return Err(Error::new(format!(
            "{} does not ask for a bearer token at /v2/: ob cannot pull from it",
            source.registry
        )));
    };
    let mut cmd = Command::new("curl");
    cmd.args(["--proto", "=https", "--tlsv1.2", "-fsSL", "--retry", "3", "--get"])
        .args(["--data-urlencode", &format!("service={service}")])
        .args(["--data-urlencode", &format!("scope=repository:{}:pull", source.repository)])
        .arg(&realm);
    let answer = process::output(ctx, &mut cmd)?;
    let body = String::from_utf8_lossy(&answer.stdout);
    serde_json::from_str::<serde_json::Value>(&body)
        .ok()
        .and_then(|reply| {
            ["token", "access_token"].iter().find_map(|key| {
                reply.get(key)?.as_str().filter(|t| !t.is_empty()).map(String::from)
            })
        })
        .ok_or_else(|| Error::new(format!("{realm} gave no token for {}", source.repository)))
}

/// The token service a registry's `WWW-Authenticate: Bearer` challenge
/// names, as (realm, service), from the headers of its answer. The realm
/// is an https:// address, which ob sends the request to.
fn bearer_challenge(headers: &str) -> Option<(String, String)> {
    let line = headers.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case("www-authenticate").then_some(value.trim())
    })?;
    let params = line.strip_prefix("Bearer ")?;
    let param = |key: &str| {
        let rest = &params[params.find(&format!("{key}=\""))? + key.len() + 2..];
        Some(rest[..rest.find('"')?].to_string())
    };
    let realm = param("realm").filter(|realm| realm.starts_with("https://"))?;
    Some((realm, param("service")?))
}

/// The files under `dir`, one line each with its size, in order: what
/// ob notes beside a source tree it unpacks, to know the tree whole
/// again before it builds from it.
fn listing(dir: &Path) -> Result<String> {
    let mut files = Vec::new();
    list_files(dir, &mut files)?;
    files.sort();
    let mut text = String::new();
    for file in &files {
        let size = std::fs::symlink_metadata(file)
            .context(|| format!("reading {}", file.display()))?
            .len();
        let _ = writeln!(text, "{size} {}", file.strip_prefix(dir).unwrap_or(file).display());
    }
    Ok(text)
}

/// The tree of the release of Go that builds the port: the host's, which
/// `$GOROOT_BOOTSTRAP` names, if it is set, and else the release the recipe
/// pins, downloaded once.
fn go_bootstrap(ctx: &Ctx, recipe: &Recipe, bootstrap: &Bootstrap) -> Result<PathBuf> {
    match std::env::var_os("GOROOT_BOOTSTRAP").filter(|dir| !dir.is_empty()) {
        Some(dir) => {
            let dir = PathBuf::from(dir);
            host_go(recipe, bootstrap, &dir)?;
            Ok(dir)
        }
        None => fetch_bootstrap(ctx, &recipe.name, bootstrap),
    }
}

/// Checks that `dir`, which `$GOROOT_BOOTSTRAP` names, is the tree of a
/// release of Go that is `bootstrap.minimum` or later.
fn host_go(recipe: &Recipe, bootstrap: &Bootstrap, dir: &Path) -> Result<()> {
    let hint = format!(
        "set GOROOT_BOOTSTRAP to the tree of Go {} or later (`go env GOROOT`), or unset it, and \
         ob downloads the release the recipe pins",
        bootstrap.minimum
    );
    let file = dir.join("VERSION");
    let text = std::fs::read_to_string(&file).map_err(|err| {
        Error::new(format!(
            "GOROOT_BOOTSTRAP is {}, whose VERSION file cannot be read ({err}): it is not the \
             tree of a Go release",
            dir.display()
        ))
        .hint(hint.clone())
    })?;
    let version = text.lines().next().unwrap_or_default();
    match go_release_at_least(version, &bootstrap.minimum) {
        Some(true) => {}
        Some(false) => {
            return Err(Error::new(format!(
                "GOROOT_BOOTSTRAP is {version}, and {} {} builds with Go {} or later",
                recipe.name, recipe.version, bootstrap.minimum
            ))
            .hint(hint));
        }
        None => {
            return Err(Error::new(format!(
                "{} says `{version}`, which is no release of Go",
                file.display()
            ))
            .hint(hint));
        }
    }
    if !dir.join("bin/go").is_file() {
        return Err(Error::new(format!(
            "GOROOT_BOOTSTRAP is {}, which has no bin/go",
            dir.display()
        ))
        .hint(hint));
    }
    Ok(())
}

/// Fetches the release of Go that `bootstrap` pins into
/// `target/ports/sources`, once, checks its size and SHA-256, and unpacks
/// it there. Returns where it is.
fn fetch_bootstrap(ctx: &Ctx, name: &str, bootstrap: &Bootstrap) -> Result<PathBuf> {
    let sources = ctx.target.join("ports/sources");
    let base = format!("{name}-bootstrap-{}", &bootstrap.sha256[..12]);
    // As a release archive is: the files unpacked are noted beside it.
    let dir = sources.join(&base);
    let files = sources.join(format!("{base}.files"));
    if dir.is_dir() {
        if std::fs::read_to_string(&files).ok() == Some(listing(&dir)?) {
            return Ok(dir);
        }
        step(&format!("{} is not the tree ob unpacked; fetching it again", dir.display()));
    }
    process::create_dir(&sources)?;
    let file = sources.join(format!("{base}.download"));
    step(&format!("Fetching the Go release {name} is built with, from {}", bootstrap.url));
    let mut cmd = Command::new("curl");
    cmd.args(["--proto", "=https", "--tlsv1.2", "-fsSL", "--retry", "3", "--max-filesize"])
        .arg(bootstrap.size.to_string())
        .arg("-o")
        .arg(&file)
        .arg(&bootstrap.url);
    process::run(ctx, &mut cmd).map_err(|err| {
        err.hint(format!(
            "set GOROOT_BOOTSTRAP to the tree of Go {} or later to build without the download",
            bootstrap.minimum
        ))
    })?;
    let unpacked = unpack_bootstrap(ctx, bootstrap, &file, &sources.join(&base));
    let _ = std::fs::remove_file(&file);
    let dir = unpacked?;
    write_file(&files, &listing(&dir)?)?;
    Ok(dir)
}

/// Unpacks `file`, the zip downloaded from `bootstrap.url`, to `dir`, if it
/// is the size and SHA-256 the recipe pins: the tree of the release,
/// which the zip holds in a folder of its own, with its programs
/// executable.
fn unpack_bootstrap(ctx: &Ctx, bootstrap: &Bootstrap, file: &Path, dir: &Path) -> Result<PathBuf> {
    let size = std::fs::metadata(file).context(|| format!("reading {}", file.display()))?.len();
    if size != bootstrap.size {
        return Err(Error::new(format!(
            "{} is {size} bytes, not the {} the recipe pins; not using it",
            bootstrap.url, bootstrap.size
        )));
    }
    let sum = process::sha256(ctx, file)?;
    if sum != bootstrap.sha256 {
        return Err(Error::new(format!(
            "{} has SHA-256 {sum}, not the {} the recipe pins; not using it",
            bootstrap.url, bootstrap.sha256
        )));
    }
    // Unpacked beside its final place, then moved there: a directory there
    // is a whole tree.
    let partial = dir.with_extension("partial");
    for old in [dir, &partial] {
        if old.exists() {
            std::fs::remove_dir_all(old).context(|| format!("removing {}", old.display()))?;
        }
    }
    process::create_dir(&partial)?;
    // Python's zipfile writes no path outside the folder it is given.
    let mut cmd = Command::new("python3");
    cmd.args(["-I", "-m", "zipfile", "-e"]).arg(file).arg(&partial);
    process::run(ctx, &mut cmd)?;
    let tree = partial.join(&bootstrap.root);
    if !tree.join("bin/go").is_file() || !tree.join("VERSION").is_file() {
        return Err(Error::new(format!(
            "{} holds no {}/bin/go and VERSION",
            bootstrap.url, bootstrap.root
        )));
    }
    make_programs_executable(&tree)?;
    std::fs::rename(&tree, dir).context(|| format!("moving it to {}", dir.display()))?;
    std::fs::remove_dir_all(&partial).context(|| format!("removing {}", partial.display()))?;
    Ok(dir.to_path_buf())
}

/// Makes the files of a Go tree's `bin` and `pkg/tool/*` executable, as the
/// go command does when it unpacks the zip of a toolchain: a zip keeps no
/// modes.
fn make_programs_executable(tree: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let tool = tree.join("pkg/tool");
    let mut dirs = vec![tree.join("bin")];
    for entry in std::fs::read_dir(&tool).context(|| format!("reading {}", tool.display()))? {
        let path = entry.context(|| format!("reading {}", tool.display()))?.path();
        if path.is_dir() {
            dirs.push(path);
        }
    }
    for dir in dirs {
        for entry in std::fs::read_dir(&dir).context(|| format!("reading {}", dir.display()))? {
            let path = entry.context(|| format!("reading {}", dir.display()))?.path();
            if path.is_file() {
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
                    .context(|| format!("making {} executable", path.display()))?;
            }
        }
    }
    Ok(())
}

/// Fetches a crate from crates.io into `target/ports/sources/crates`,
/// once, checks its SHA-256, and unpacks it there. Returns where it is
/// unpacked: with a `.cargo-checksum.json` that gives its checksum, as
/// Cargo wants of a vendored crate.
pub fn fetch_crate(ctx: &Ctx, krate: &Crate) -> Result<PathBuf> {
    let crates = ctx.target.join("ports/sources/crates");
    let name = format!("{}-{}", krate.name, krate.version);
    let dir = crates.join(&name);
    let checksum = format!("{{\"files\":{{}},\"package\":\"{}\"}}", krate.sha256);
    let marker = dir.join(".cargo-checksum.json");
    // As with a release archive, the files unpacked are noted beside it.
    let files = crates.join(format!("{name}.files"));
    if std::fs::read_to_string(&marker).ok().as_deref() == Some(checksum.as_str())
        && std::fs::read_to_string(&files).ok() == Some(listing(&dir)?)
    {
        return Ok(dir);
    }
    let url = format!("https://static.crates.io/crates/{}/{name}.crate", krate.name);
    step(&format!("Fetching crate {} {} from crates.io", krate.name, krate.version));
    process::create_dir(&crates)?;
    let file = crates.join(format!("{name}.crate"));
    let mut cmd = Command::new("curl");
    cmd.args(["--proto", "=https", "--tlsv1.2", "-fsSL", "--retry", "3", "-o"])
        .arg(&file)
        .arg(&url);
    process::run(ctx, &mut cmd)?;
    let sum = process::sha256(ctx, &file)?;
    if sum != krate.sha256 {
        let _ = std::fs::remove_file(&file);
        return Err(Error::new(format!(
            "{url} has SHA-256 {sum}, not the {} the recipe pins; not using it",
            krate.sha256
        )));
    }
    // Unpacked beside its final place, then moved there: a directory
    // there is a whole crate.
    let partial = crates.join(format!("{name}.partial"));
    for old in [&dir, &partial] {
        if old.exists() {
            std::fs::remove_dir_all(old).context(|| format!("removing {}", old.display()))?;
        }
    }
    process::create_dir(&partial)?;
    let mut cmd = Command::new("tar");
    cmd.arg("-xzf").arg(&file).arg("-C").arg(&partial).arg("--no-same-owner");
    process::run(ctx, &mut cmd)?;
    let unpacked = partial.join(&name);
    if !unpacked.join("Cargo.toml").is_file() {
        return Err(Error::new(format!("{url} holds no {name}/Cargo.toml")));
    }
    write_file(&unpacked.join(".cargo-checksum.json"), &checksum)?;
    std::fs::rename(&unpacked, &dir).context(|| format!("moving it to {}", dir.display()))?;
    std::fs::remove_dir_all(&partial).context(|| format!("removing {}", partial.display()))?;
    let _ = std::fs::remove_file(&file);
    write_file(&files, &listing(&dir)?)?;
    Ok(dir)
}

/// Go's module cache for the ports' programs in Go: the modules Go for
/// Radix fetches from Go's module proxy, each checked against the
/// program's go.sum.
pub fn go_module_cache(ctx: &Ctx) -> PathBuf {
    ctx.target.join("ports/sources/go-mod")
}

/// Fetches a Go module's zip from Go's module proxy into
/// `target/ports/sources/go-modules`, once, checks its SHA-256, and
/// unpacks the module there. Returns where it is unpacked. Go's own
/// module cache is not used: what is in it was checked against a go.sum,
/// and these are checked against the recipe.
pub fn fetch_go_module(ctx: &Ctx, module: &GoModule) -> Result<PathBuf> {
    let modules = ctx.target.join("ports/sources/go-modules");
    let name = module.dir();
    let dir = modules.join(&name);
    // The SHA-256 it was unpacked from is noted beside it, as a crate's
    // is in it, and so are the files unpacked.
    let marker = modules.join(format!("{name}.sha256"));
    let files = modules.join(format!("{name}.files"));
    if std::fs::read_to_string(&marker).ok().as_deref() == Some(module.sha256.as_str())
        && std::fs::read_to_string(&files).ok() == Some(listing(&dir)?)
    {
        return Ok(dir);
    }
    let url = module.url();
    step(&format!("Fetching Go module {} {} from Go's module proxy", module.path, module.version));
    process::create_dir(&modules)?;
    let file = modules.join(format!("{name}.zip"));
    let mut cmd = Command::new("curl");
    cmd.args(["--proto", "=https", "--tlsv1.2", "-fsSL", "--retry", "3", "-o"])
        .arg(&file)
        .arg(&url);
    process::run(ctx, &mut cmd)?;
    let sum = process::sha256(ctx, &file)?;
    if sum != module.sha256 {
        let _ = std::fs::remove_file(&file);
        return Err(Error::new(format!(
            "{url} has SHA-256 {sum}, not the {} the recipe pins; not using it",
            module.sha256
        )));
    }
    // Unpacked beside its final place, then moved there: a directory
    // there is a whole module. A module's zip holds it in a folder named
    // `<path>@<version>`, as the proxy spells them.
    let partial = modules.join(format!("{name}.partial"));
    for old in [&dir, &partial] {
        if old.exists() {
            std::fs::remove_dir_all(old).context(|| format!("removing {}", old.display()))?;
        }
    }
    let _ = std::fs::remove_file(&marker);
    process::create_dir(&partial)?;
    // Python's zipfile writes no path outside the folder it is given.
    let mut cmd = Command::new("python3");
    cmd.args(["-I", "-m", "zipfile", "-e"]).arg(&file).arg(&partial);
    process::run(ctx, &mut cmd)?;
    let unpacked = partial.join(format!("{}@{}", module.path, module.version));
    if !unpacked.is_dir() {
        return Err(Error::new(format!("{url} holds no {}@{}/", module.path, module.version)));
    }
    std::fs::rename(&unpacked, &dir).context(|| format!("moving it to {}", dir.display()))?;
    std::fs::remove_dir_all(&partial).context(|| format!("removing {}", partial.display()))?;
    let _ = std::fs::remove_file(&file);
    write_file(&files, &listing(&dir)?)?;
    write_file(&marker, &module.sha256)?;
    Ok(dir)
}

/// Copies the tree of `repo`'s checked-out commit, without its `.git`, to
/// `to`; with `paths`, only those directories of it.
pub fn export(ctx: &Ctx, repo: &Path, to: &Path, paths: &[String]) -> Result<()> {
    process::create_dir(to)?;
    let mut archive = Command::new("git");
    archive.arg("-C").arg(repo).args(["archive", "--format=tar", "HEAD", "--"]).args(paths);
    archive.stdout(Stdio::piped());
    if ctx.verbose {
        eprintln!("+ {} | tar -x -C {}", process::render(&archive), to.display());
    }
    let mut child = archive.spawn().context(|| "running git archive".into())?;
    let stdout = child.stdout.take().expect("stdout is piped");
    let status = Command::new("tar")
        .arg("-x")
        .arg("-C")
        .arg(to)
        .stdin(stdout)
        .status()
        .context(|| "running tar".into())?;
    let archived = child.wait().context(|| "running git archive".into())?;
    if !status.success() || !archived.success() {
        return Err(Error::new(format!("copying {} to {} failed", repo.display(), to.display())));
    }
    Ok(())
}

/// Copies the directory `from`, with everything in it, to `to`, keeping
/// each file's time: make would otherwise find a release's generated
/// files, such as its configure script, older than what they are made
/// from, and make them again with programs the host need not have.
pub fn copy_tree(ctx: &Ctx, from: &Path, to: &Path) -> Result<()> {
    process::create_dir(to)?;
    let mut cmd = Command::new("cp");
    cmd.arg("-R").arg("-p").arg(from.join(".")).arg(to);
    process::run(ctx, &mut cmd)
}

/// Builds one port: a fresh copy of its sources with its patches applied,
/// then its build.sh.
fn build(ctx: &Ctx, toolchain: &Toolchain, port: &Port) -> Result<()> {
    let (recipe, dir) = (&port.recipe, port.dir.as_path());
    step(&format!("Building port {} {}", recipe.name, recipe.version));
    let work = ctx.target.join("ports/work").join(&recipe.name);
    if work.exists() {
        std::fs::remove_dir_all(&work).context(|| format!("removing {}", work.display()))?;
    }
    let src = work.join("src");
    let build_dir = work.join("build");
    match &port.source {
        Some(source) if recipe.source.git.is_empty() => copy_tree(ctx, source, &src)?,
        Some(source) => export(ctx, source, &src, &recipe.source.paths)?,
        None => process::create_dir(&src)?,
    }
    for (extra, source) in recipe.extra.iter().zip(&port.extras) {
        export(ctx, source, &src.join(&extra.path), &[])?;
    }
    for (krate, source) in recipe.crates.iter().zip(&port.crates) {
        copy_tree(ctx, source, &src.join(krate.path()))?;
    }
    for (module, source) in recipe.modules.iter().zip(&port.modules) {
        copy_tree(ctx, source, &src.join(module.dir()))?;
    }
    apply_patches(ctx, dir, &src)?;
    if !recipe.modules.is_empty() {
        // Each module's path, version, folder, and whether it takes the
        // place of every version, for build.sh to install them by.
        let mut list = String::new();
        for m in &recipe.modules {
            let versions = if m.all_versions { "all" } else { "one" };
            let _ = writeln!(list, "{} {} {} {versions}", m.path, m.version, m.dir());
        }
        write_file(&src.join(GO_MODULE_LIST), &list)?;
    }
    process::create_dir(&build_dir)?;
    let info = info_dir(ctx, &recipe.name);
    if info.exists() {
        std::fs::remove_dir_all(&info).context(|| format!("removing {}", info.display()))?;
    }
    process::create_dir(&info)?;
    let bundle = crate::bundles::staging(ctx, &recipe.name);
    if bundle.exists() {
        std::fs::remove_dir_all(&bundle).context(|| format!("removing {}", bundle.display()))?;
    }
    if recipe.bundle.is_some() {
        process::create_dir(&bundle)?;
    }
    let sysroot = ctx.out.join("sysroot");
    process::create_dir(&sysroot)?;
    let jobs = std::thread::available_parallelism().map_or(1, std::num::NonZero::get);
    let path = std::env::var_os("PATH").unwrap_or_default();
    let mut paths = vec![toolchain.bin()];
    paths.extend(std::env::split_paths(&path));
    let mut cmd = Command::new("sh");
    if let Some(bootstrap) = &recipe.bootstrap {
        cmd.env("GOROOT_BOOTSTRAP", go_bootstrap(ctx, recipe, bootstrap)?);
    }
    cmd.arg(dir.join("build.sh"))
        .current_dir(&work)
        .env(
            "PATH",
            std::env::join_paths(paths)
                .map_err(|err| Error::new(format!("building PATH: {err}")))?,
        )
        .env("PORT_DIR", dir)
        .env("PORT_SRC", &src)
        .env("PORT_BUILD", &build_dir)
        .env("PORT_INFO", &info)
        .env("PORT_BUNDLE", &bundle)
        .env("SYSROOT", &sysroot)
        .env("TOOLCHAIN", &toolchain.dir)
        .env("CARGO_RADIX", toolchain.cargo())
        .env("GO_RADIX", toolchain.go())
        .env("RUST_TARGET", ctx.root.join(crate::userspace::STD_TARGET_SPEC))
        .env("PREFIX", "/system")
        .env("JOBS", jobs.to_string())
        .env("HOST", TRIPLE)
        .envs(BUILD_TOOLS.map(|(variable, name)| (variable, format!("{TRIPLE}-{name}"))))
        .env("CFLAGS", "-O2")
        .env("CXXFLAGS", "-O2")
        .env("CROSS_FILE", toolchain.cross_file())
        .env("CMAKE_TOOLCHAIN_FILE", toolchain.cmake_toolchain_file())
        .env("LIBC_CROSS_FILE", toolchain.libc_cross_file())
        // The host's settings must not reach a build for Radix.
        .env_remove("CPPFLAGS")
        .env_remove("LDFLAGS")
        .env_remove("PKG_CONFIG_PATH")
        .env("PKG_CONFIG_LIBDIR", sysroot.join("system/libraries/pkgconfig"))
        .env("PKG_CONFIG_SYSROOT_DIR", &sysroot)
        .env("SOURCE_DATE_EPOCH", PORT_TIME)
        // The host's language must not reach a build either: uutils'
        // uucore, for one, builds in the translations of LANG's.
        .env("LANG", "C.UTF-8")
        .env("LC_ALL", "C.UTF-8")
        // What a recipe that sets compiler flags itself, and does not use
        // the toolchain's wrappers, adds to them so that what it builds
        // does not record where it was built (ADR 0535): the flags for
        // Rust's compiler, as Cargo's CARGO_ENCODED_RUSTFLAGS has them,
        // separated by the unit separator.
        .env("REMAP_RUSTFLAGS", crate::repro::rustc_flags(ctx).join("\x1f"));
    // A port's crates and the crates ports patch for Radix are path
    // dependencies outside its workspace, whose identity Cargo would take
    // from where the checkout is (ADR 0535).
    crate::rustc_wrapper::configure(ctx, &mut cmd);
    process::run(ctx, &mut cmd)
        .map_err(|err| Error::new(format!("port {}: {err}", recipe.name)))?;
    if let Some(spec) = &recipe.bundle {
        for command in &spec.commands {
            if !bundle.join("bin").join(command).is_file() {
                return Err(Error::new(format!(
                    "port {}: build.sh installed no bin/{command} in $PORT_BUNDLE",
                    recipe.name
                )));
            }
        }
    }
    // Nothing a recipe or build.sh says goes in the image may be missing.
    for file in &image_list(ctx, recipe)? {
        let path = sysroot.join(file);
        let there = if file.ends_with('/') { path.is_dir() } else { path.is_file() };
        if !there {
            return Err(Error::new(format!(
                "port {}: build.sh installed no {file} in the sysroot",
                recipe.name
            )));
        }
    }
    Ok(())
}

/// Applies the patches of the port in `dir` to its source `src`, in name
/// order.
pub fn apply_patches(ctx: &Ctx, dir: &Path, src: &Path) -> Result<()> {
    let patches = dir.join("patches");
    if patches.is_dir() {
        let mut list: Vec<PathBuf> = std::fs::read_dir(&patches)
            .context(|| format!("reading {}", patches.display()))?
            .filter_map(|entry| entry.ok().map(|e| e.path()))
            .filter(|path| path.extension().is_some_and(|e| e == "patch"))
            .collect();
        list.sort();
        for patch in list {
            let mut cmd = Command::new("patch");
            cmd.args(["-p1", "--batch", "--forward", "--quiet", "-d"])
                .arg(src)
                .arg("-i")
                .arg(&patch);
            process::run(ctx, &mut cmd)?;
        }
    }
    Ok(())
}

/// Where a port's build.sh leaves what ob needs to know of the build
/// (`$PORT_INFO`): `image`, the files it installed for the image beyond
/// those the recipe names, one path beneath the sysroot a line, such as
/// manual pages; and for a port in Rust, the crates it linked
/// (`crates.txt`, as `cargo tree --format '{p}|{l}'` lists them) and its
/// `Cargo.lock`, for the license manifest.
pub fn info_dir(ctx: &Ctx, name: &str) -> PathBuf {
    ctx.out.join("ports").join(name)
}

/// The modules the programs of the port `name` link, from the
/// `modules.txt` its build.sh left in its info folder: what `go version
/// -m` printed for each program it installed. None for a port that left
/// none, as a port that is not in Go.
pub fn go_modules_linked(ctx: &Ctx, name: &str) -> Result<Option<Vec<LinkedModule>>> {
    let file = info_dir(ctx, name).join("modules.txt");
    if !file.is_file() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(&file).context(|| format!("reading {}", file.display()))?;
    linked_go_modules(&text)
        .map(Some)
        .map_err(|err| Error::new(format!("{}: {err}", file.display())))
}

/// What of the sysroot a port puts in the image: what its recipe names,
/// then what its build.sh listed in `$PORT_INFO/image`.
fn image_list(ctx: &Ctx, recipe: &Recipe) -> Result<Vec<String>> {
    let mut list = recipe.image.clone();
    let listed = info_dir(ctx, &recipe.name).join("image");
    if listed.is_file() {
        let text =
            std::fs::read_to_string(&listed).context(|| format!("reading {}", listed.display()))?;
        for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
            if line.starts_with('/') || line.split('/').any(|part| part == "..") {
                return Err(Error::new(format!(
                    "port {}: `{line}` in {} is not a path beneath the sysroot",
                    recipe.name,
                    listed.display()
                )));
            }
            list.push(line.to_string());
        }
    }
    Ok(list)
}

/// The files of `recipe`'s image in `sysroot`, as (path in the image,
/// path on the host) pairs, in name order.
pub fn image_files(ctx: &Ctx, recipe: &Recipe, sysroot: &Path) -> Result<Vec<(String, PathBuf)>> {
    let mut files = Vec::new();
    for file in &image_list(ctx, recipe)? {
        let path = sysroot.join(file);
        if !file.ends_with('/') {
            files.push((file.clone(), path));
            continue;
        }
        let mut found = Vec::new();
        list_files(&path, &mut found)?;
        found.sort();
        for host in found {
            let inside = host.strip_prefix(sysroot).unwrap_or(&host);
            files.push((inside.to_string_lossy().into_owned(), host));
        }
    }
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ports_are_built_at_the_start_of_2026() {
        // 56 years from 1970, 14 of them leap years, in seconds.
        let days = 56 * 365 + 14;
        assert_eq!(PORT_TIME.parse::<u64>().unwrap(), days * 86_400);
    }

    #[test]
    fn a_folder_linked_from_a_port_counts_by_its_files() {
        let root = std::env::temp_dir().join(format!("ob-linked-patches-{}", std::process::id()));
        let (go, radix) = (root.join("go"), root.join("go-radix"));
        std::fs::create_dir_all(go.join("patches")).unwrap();
        std::fs::create_dir_all(&radix).unwrap();
        std::fs::write(go.join("patches/0001-a.patch"), "one\n").unwrap();
        std::fs::write(radix.join("recipe.toml"), "name = \"go-radix\"\n").unwrap();
        std::os::unix::fs::symlink("../go/patches", radix.join("patches")).unwrap();
        let hash = |dir: &Path| {
            let mut hasher = DefaultHasher::new();
            hash_folder(dir, false, &mut hasher).unwrap();
            hasher.finish()
        };
        let before = hash(&radix);
        assert_eq!(hash(&radix), before);
        // A patch changed, or one added, in the linked folder is a change
        // to the port that links it.
        std::fs::write(go.join("patches/0001-a.patch"), "two\n").unwrap();
        let changed = hash(&radix);
        assert_ne!(changed, before);
        std::fs::write(go.join("patches/0002-b.patch"), "three\n").unwrap();
        assert_ne!(hash(&radix), changed);
        // A link to a file still counts by the link itself, as before.
        std::fs::remove_file(go.join("patches/0002-b.patch")).unwrap();
        std::fs::write(go.join("patches/0001-a.patch"), "one\n").unwrap();
        assert_eq!(hash(&radix), before);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn header_hash_ignores_comments_but_not_code() {
        let strip = |text: &str| without_c_comments(text.as_bytes());
        let header = "struct s {\n    /*\n     * Old words.\n     */\n    int a; // why\n};\n";
        let reworded = "struct s {\n    /* New words,\n       on two lines. */\n    int a;\n};\n";
        let uncommented = "struct s {\n    int a;\n};\n";
        assert_eq!(strip(header), strip(reworded));
        assert_eq!(strip(header), strip(uncommented));
        assert_ne!(strip(header), strip("struct s {\n    long a;\n};\n"));
        // A comment marker inside a literal is part of the literal.
        assert_ne!(strip("char *p = \"/* a */\";"), strip("char *p = \"/* b */\";"));
        assert_eq!(strip("#define A 1 /* one */\n#define B 2"), strip("#define A 1\n#define B 2"));
    }

    #[test]
    fn reads_clang_versions() {
        assert_eq!(clang_major("Ubuntu clang version 20.1.2 (0ubuntu1~24.04.3)"), Some(20));
        assert_eq!(clang_major("clang version 21.0.0git"), Some(21));
        assert_eq!(clang_major("gcc (GCC) 14.2.1"), None);
    }

    #[test]
    fn recipes_parse() {
        let text = r#"
            name = "lua"
            version = "5.4.7"
            summary = "the Lua interpreter"
            license = "MIT"
            homepage = "https://www.lua.org"
            depends = ["mlibc"]
            image = ["system/commands/lua"]
            [source]
            git = "https://github.com/lua/lua.git"
            commit = "0123456789012345678901234567890123456789"
            tag = "v5.4.7"
        "#;
        let recipe: Recipe = toml::from_str(text).unwrap();
        assert_eq!(recipe.depends, ["mlibc"]);
        assert!(!recipe.toolchain);
        assert!(toml::from_str::<Recipe>(&text.replace("summary", "sumary")).is_err());
        let bundled = format!(
            "{text}\n[[bundled]]\nname = \"musl\"\nversion = \"1.2.5\"\nlicense = \"MIT\"\n\
             homepage = \"https://musl.libc.org\"\nnotices = [\"musl/COPYRIGHT\"]\n"
        );
        let recipe: Recipe = toml::from_str(&bundled).unwrap();
        assert_eq!(recipe.bundled[0].notices, ["musl/COPYRIGHT"]);
        assert_eq!(outside_path(&recipe), None);
    }

    #[test]
    fn named_paths_stay_inside() {
        let text = r#"
            name = "x"
            version = "1"
            summary = "x"
            license = "MIT"
            homepage = "https://x"
            image = ["system/commands/x", "system/resources/x/"]
            [source]
            git = "https://x/x.git"
            commit = "0123456789012345678901234567890123456789"
        "#;
        let recipe: Recipe = toml::from_str(text).unwrap();
        assert_eq!(outside_path(&recipe), None);
        for (from, to) in [
            ("\"system/commands/x\"", "\"/etc/x\""),
            ("\"system/commands/x\"", "\"system/../../x\""),
            ("image = [", "notices = [\"../COPYING\"]\nimage = ["),
            (
                "[source]",
                "[[extra]]\npath = \"/tmp/x\"\ngit = \"https://y\"\ncommit = \"0123456789012345678901234567890123456789\"\nlicense = \"MIT\"\n[source]",
            ),
            ("commit = ", "paths = [\"a/../../b\"]\ncommit = "),
        ] {
            let recipe: Recipe = toml::from_str(&text.replacen(from, to, 1)).unwrap();
            assert!(outside_path(&recipe).is_some(), "{to}");
        }
    }

    /// A port that is only extra sources, as sqlite-cli is (linenoise and
    /// the shell the sqlite port generates), needs no source of its own;
    /// one with no source of any kind is refused.
    #[test]
    fn a_port_of_extra_sources_alone_needs_no_source() {
        let text = r#"
            name = "x"
            version = "1"
            summary = "x"
            license = "MIT"
            homepage = "https://x"
            [[extra]]
            path = "y"
            git = "https://y/y.git"
            commit = "0123456789012345678901234567890123456789"
            license = "MIT"
        "#;
        let root =
            std::env::temp_dir().join(format!("ob-extra-sources-port-{}", std::process::id()));
        let dir = root.join("ports/x");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("build.sh"), "#!/bin/sh\n").unwrap();
        let ctx = Ctx {
            out: root.join("out"),
            target: root.join("target"),
            root: root.clone(),
            config: crate::config::Config::default(),
            verbose: false,
        };
        let recipe: Recipe = toml::from_str(text).unwrap();
        let checked = check(&ctx, &recipe, &dir);
        let bare: Recipe = toml::from_str(&text[..text.find("[[extra]]").unwrap()]).unwrap();
        let refused = check(&ctx, &bare, &dir);
        std::fs::remove_dir_all(&root).unwrap();
        checked.unwrap();
        let err = refused.unwrap_err().to_string();
        assert!(err.contains("a port needs a source"), "{err}");
    }

    #[test]
    fn archive_sources() {
        let text = r#"
            name = "grep"
            version = "3.12"
            summary = "GNU grep"
            license = "GPL-3.0-or-later"
            homepage = "https://www.gnu.org/software/grep/"
            [source]
            url = "https://ftp.gnu.org/gnu/grep/grep-3.12.tar.xz"
            sha256 = "2649b27c0e90e632eadcd757be06c6e9a4f48d941de51e7c0f83ff76408a07b9"
            mirrors = ["https://archive.ubuntu.com/ubuntu/pool/main/g/grep/grep_3.12.orig.tar.xz"]
        "#;
        let recipe: Recipe = toml::from_str(text).unwrap();
        assert_eq!(recipe.source.download(), "https://ftp.gnu.org/gnu/grep/grep-3.12.tar.xz");
        assert_eq!(recipe.source.release(), "grep-3.12.tar.xz");
        assert_eq!(recipe.source.mirrors.len(), 1);
        let git =
            Source { git: "https://x/y.git".into(), commit: "ab".into(), ..Source::default() };
        assert_eq!(git.download(), "git+https://x/y.git@ab");
        assert_eq!(git.release(), "ab");
    }

    /// A recipe for curl 8.23.0, pinned by a release archive, with `rest`
    /// after its `[source]` table.
    fn curl_recipe(rest: &str) -> Recipe {
        let text = format!(
            "name = \"curl\"\nversion = \"8.23.0\"\nsummary = \"curl\"\nlicense = \"curl\"\n\
             homepage = \"https://curl.se\"\n[source]\nurl = \"https://example.org/curl.tar.xz\"\n\
             sha256 = \"{}\"\n{rest}\n",
            "ab".repeat(32)
        );
        toml::from_str(&text).unwrap()
    }

    const CURL_UPSTREAM: &str = "[upstream]\ngit = \"https://github.com/curl/curl.git\"\n\
                                 tag = \"curl-8_23_0\"";

    #[test]
    fn an_archive_port_names_its_upstream_by_git_and_tag_or_says_there_is_none() {
        let recipe = curl_recipe(CURL_UPSTREAM);
        assert_eq!(upstream_problem(&recipe), None);
        let upstream = recipe.upstream.unwrap();
        assert_eq!(upstream.tagged(), Some(("https://github.com/curl/curl.git", "curl-8_23_0")));
        assert_eq!(upstream.none, None);

        let none = curl_recipe("[upstream]\nnone = \"curl keeps no repository\"");
        assert_eq!(upstream_problem(&none), None);
        let upstream = none.upstream.unwrap();
        assert_eq!(upstream.tagged(), None);
        assert_eq!(upstream.none.as_deref(), Some("curl keeps no repository"));
        // A key the table does not have is a mistake, not ignored.
        let unknown = "name = \"x\"\nversion = \"1\"\nsummary = \"x\"\nlicense = \"MIT\"\n\
                       homepage = \"https://x\"\n[upstream]\nrepo = \"https://x\"\n";
        assert!(toml::from_str::<Recipe>(unknown).is_err());
    }

    #[test]
    fn the_check_refuses_each_wrong_shape_of_upstream() {
        let wrong = [
            // An archive with no [upstream] at all, and with an empty one.
            ("", "needs [upstream]"),
            ("[upstream]", "needs git and tag"),
            // Half of git and tag.
            ("[upstream]\ngit = \"https://github.com/curl/curl.git\"", "needs git and tag"),
            ("[upstream]\ntag = \"curl-8_23_0\"", "needs git and tag"),
            // A repository that is not an https:// URL.
            (
                "[upstream]\ngit = \"git://github.com/curl/curl.git\"\ntag = \"curl-8_23_0\"",
                "not an https:// URL",
            ),
            (
                "[upstream]\ngit = \"git@github.com:curl/curl.git\"\ntag = \"curl-8_23_0\"",
                "not an https:// URL",
            ),
            ("[upstream]\ngit = \"https://\"\ntag = \"curl-8_23_0\"", "not an https:// URL"),
            (
                "[upstream]\ngit = \"https://x.org/a b.git\"\ntag = \"curl-8_23_0\"",
                "not an https:// URL",
            ),
            // A tag that `git ls-remote` would read as a pattern or an option.
            ("[upstream]\ngit = \"https://x.org/c.git\"\ntag = \"curl-8_23_*\"", "not a tag name"),
            ("[upstream]\ngit = \"https://x.org/c.git\"\ntag = \"curl 8.23.0\"", "not a tag name"),
            ("[upstream]\ngit = \"https://x.org/c.git\"\ntag = \"-8.23.0\"", "not a tag name"),
            // The tag of another release than the archive's.
            (
                "[upstream]\ngit = \"https://x.org/c.git\"\ntag = \"curl-8_22_0\"",
                "not the release of version 8.23.0",
            ),
            // None with a repository, and none with no reason.
            ("[upstream]\nnone = \"no repository\"\ngit = \"https://x.org/c.git\"", "not both"),
            ("[upstream]\nnone = \"no repository\"\ntag = \"curl-8_23_0\"", "not both"),
            ("[upstream]\nnone = \"\"", "needs a reason of one line"),
            ("[upstream]\nnone = \"  \"", "needs a reason of one line"),
            ("[upstream]\nnone = \"one line\\nand another\"", "needs a reason of one line"),
        ];
        for (rest, expected) in wrong {
            let problem = upstream_problem(&curl_recipe(rest))
                .unwrap_or_else(|| panic!("`{rest}` was not refused"));
            assert!(problem.contains(expected), "`{rest}` gave `{problem}`, not `{expected}`");
        }
    }

    #[test]
    fn a_series_is_numbers_that_hold_the_version() {
        let with = |series: &str| {
            let mut recipe = curl_recipe(CURL_UPSTREAM);
            recipe.series = Some(series.to_string());
            recipe
        };
        assert_eq!(series_problem(&curl_recipe(CURL_UPSTREAM)), None);
        assert_eq!(series_problem(&with("8")), None);
        assert_eq!(series_problem(&with("8.23")), None);
        assert_eq!(series_problem(&with("8.23.0")), None);
        for (series, expected) in [
            ("8.22", "does not hold the version"),
            ("8.2", "does not hold the version"),
            ("9", "does not hold the version"),
            ("8.x", "not numbers"),
            ("8..23", "not numbers"),
            ("", "not numbers"),
            ("v8", "not numbers"),
        ] {
            let problem = series_problem(&with(series))
                .unwrap_or_else(|| panic!("`{series}` was not refused"));
            assert!(problem.contains(expected), "`{series}` gave `{problem}`, not `{expected}`");
        }
    }

    #[test]
    fn a_port_that_takes_only_data_says_why_and_has_a_source_and_no_crates() {
        let with = |data_only: &str, rest: &str| -> Recipe {
            toml::from_str(&format!(
                "name = \"locales\"\nversion = \"2.42\"\nsummary = \"s\"\nlicense = \"MIT\"\n\
                 homepage = \"https://example.org\"\ndata_only = \"{data_only}\"\n{rest}\n"
            ))
            .unwrap()
        };
        let git = "[source]\ngit = \"https://example.org/glibc.git\"\n\
                   commit = \"0123456789012345678901234567890123456789\"";
        assert_eq!(data_only_problem(&with("only the locale sources", git)), None);
        assert_eq!(data_only_problem(&curl_recipe(CURL_UPSTREAM)), None);
        for (data_only, rest, expected) in [
            ("", git, "a reason of one line"),
            (" ", git, "a reason of one line"),
            ("two\\nlines", git, "a reason of one line"),
            ("tables", "", "goes with a source"),
            (
                "tables",
                &format!(
                    "{git}\n[[crate]]\nname = \"x\"\nversion = \"1\"\nsha256 = \"ab\"\nlicense = \"MIT\""
                ),
                "builds no crates",
            ),
        ] {
            let problem = data_only_problem(&with(data_only, rest))
                .unwrap_or_else(|| panic!("`{data_only}` with `{rest}` was not refused"));
            assert!(problem.contains(expected), "`{problem}`, not `{expected}`");
        }
    }

    #[test]
    fn only_a_port_pinned_by_an_archive_has_an_upstream() {
        let git = r#"
            name = "lua"
            version = "5.4.7"
            summary = "Lua"
            license = "MIT"
            homepage = "https://www.lua.org"
            [source]
            git = "https://github.com/lua/lua.git"
            commit = "0123456789012345678901234567890123456789"
            tag = "v5.4.7"
        "#;
        assert_eq!(upstream_problem(&toml::from_str(git).unwrap()), None);
        for table in [CURL_UPSTREAM, "[upstream]\nnone = \"no repository\""] {
            let with = format!("{git}\n{table}\n");
            let problem = upstream_problem(&toml::from_str(&with).unwrap()).unwrap();
            assert!(problem.contains("goes with a release archive"), "{problem}");
        }
        // A port of rust-src, of crates or of extra sources has none either.
        let crates = "name = \"c\"\nversion = \"1\"\nsummary = \"c\"\nlicense = \"MIT\"\n\
                      homepage = \"https://c\"\n";
        assert_eq!(upstream_problem(&toml::from_str(crates).unwrap()), None);
        let with = format!("{crates}\n{CURL_UPSTREAM}\n");
        assert!(upstream_problem(&toml::from_str(&with).unwrap()).is_some());
    }

    #[test]
    fn a_tag_names_the_version_it_is_the_release_of() {
        for (tag, version) in [
            ("curl-8_23_0", "8.23.0"),
            ("FILE5_47", "5.47"),
            ("v2.55.0", "2.55.0"),
            ("26.02", "26.02"),
            ("3.7c", "3.7c"),
            ("release-2.1.13-stable", "2.1.13"),
            ("v668", "668"),
            ("openssl-3.5.4", "3.5.4"),
        ] {
            assert!(tag_names_version(tag, version), "{tag} is {version}");
        }
        for (tag, version) in [
            // The next release, a longer number and a release candidate.
            ("curl-8_22_0", "8.23.0"),
            ("v2.80", "2.8"),
            ("v2.55.01", "2.55.0"),
            ("3.7b", "3.7c"),
            ("3.7", "3.7c"),
            ("v5.8", "5.8.4"),
            // No digits or letters in the version gives nothing to name.
            ("v1", ""),
            ("v1", "..."),
        ] {
            assert!(!tag_names_version(tag, version), "{tag} is not {version}");
        }
    }

    /// The recipes of the repository itself pass the check, and every port
    /// pinned by an archive says where its project's repository is, or why
    /// it has none.
    #[test]
    fn every_port_of_the_repository_has_the_upstream_it_needs() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let ctx = Ctx {
            out: root.join("out"),
            target: root.join("target"),
            root,
            config: crate::config::Config::default(),
            verbose: false,
        };
        let recipes = recipes(&ctx).unwrap();
        let archives: Vec<&Recipe> =
            recipes.iter().map(|(recipe, _)| recipe).filter(|r| !r.source.url.is_empty()).collect();
        assert!(archives.len() >= 26, "{} ports are pinned by an archive", archives.len());
        for recipe in &archives {
            assert!(recipe.upstream.is_some(), "{} has no [upstream]", recipe.name);
        }
        let named = archives.iter().filter_map(|r| r.upstream.as_ref()?.tagged()).count();
        assert!(named > archives.len() / 2, "only {named} of {} name a repository", archives.len());
    }

    #[test]
    fn a_tree_that_lost_files_is_not_listed_as_whole() {
        let dir = std::env::temp_dir().join(format!("ob-listing-test-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("CPP/Windows")).unwrap();
        std::fs::write(dir.join("CPP/Windows/System.cpp"), "int x;\n").unwrap();
        std::fs::write(dir.join("README"), "").unwrap();
        std::os::unix::fs::symlink("README", dir.join("readme")).unwrap();
        let whole = listing(&dir).unwrap();
        assert_eq!(whole, "7 CPP/Windows/System.cpp\n0 README\n6 readme\n");
        std::fs::write(dir.join("CPP/Windows/System.cpp"), "int\n").unwrap();
        assert_ne!(listing(&dir).unwrap(), whole);
        std::fs::write(dir.join("CPP/Windows/System.cpp"), "int y;\n").unwrap();
        assert_eq!(listing(&dir).unwrap(), whole);
        std::fs::remove_dir_all(dir.join("CPP")).unwrap();
        let cut = listing(&dir);
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(cut.unwrap(), "0 README\n6 readme\n");
    }

    /// A recipe for Go 1.27.4 with `bootstrap` for its `[bootstrap]` table.
    fn go_recipe(bootstrap: &str) -> Recipe {
        let text = format!(
            "name = \"go\"\nversion = \"1.27.4\"\nsummary = \"Go\"\nlicense = \"BSD-3-Clause\"\n\
             homepage = \"https://go.dev\"\ntoolchain = true\n[source]\n\
             git = \"https://github.com/golang/go.git\"\n\
             commit = \"7b2881024c95a11ddb86663a616340aa3a88b67f\"\n[bootstrap]\n{bootstrap}\n"
        );
        toml::from_str(&text).unwrap()
    }

    const GO_BOOTSTRAP: &str = "minimum = \"1.24.6\"\n\
        url = \"https://proxy.golang.org/golang.org/toolchain/@v/v0.0.1-go1.27.4.linux-amd64.zip\"\n\
        sha256 = \"7ecd542f86ccbbb5017de33ac529872bc48c71f4edb658d63a18d2206f482b9b\"\n\
        size = 75846323\n\
        root = \"golang.org/toolchain@v0.0.1-go1.27.4.linux-amd64\"";

    #[test]
    fn a_bootstrap_is_a_pinned_download_and_the_oldest_release_that_will_do() {
        let recipe = go_recipe(GO_BOOTSTRAP);
        assert_eq!(bootstrap_problem(&recipe), None);
        let bootstrap = recipe.bootstrap.unwrap();
        assert_eq!(bootstrap.minimum, "1.24.6");
        assert_eq!(bootstrap.size, 75_846_323);
        // A recipe without one has none; a key the table does not have is
        // a mistake.
        assert!(bootstrap_problem(&curl_recipe(CURL_UPSTREAM)).is_none());
        assert!(
            toml::from_str::<Recipe>(&format!(
                "name = \"x\"\nversion = \"1\"\nsummary = \"x\"\nlicense = \"MIT\"\n\
             homepage = \"https://x\"\n[bootstrap]\n{GO_BOOTSTRAP}\nmirror = \"https://y\"\n"
            ))
            .is_err()
        );
    }

    #[test]
    fn the_check_refuses_each_wrong_shape_of_bootstrap() {
        let sha = "7ecd542f86ccbbb5017de33ac529872bc48c71f4edb658d63a18d2206f482b9b";
        for (from, to, expected) in [
            ("minimum = \"1.24.6\"", "minimum = \"1.24.x\"", "not numbers set apart by dots"),
            ("minimum = \"1.24.6\"", "minimum = \"go1.24.6\"", "not numbers set apart by dots"),
            ("minimum = \"1.24.6\"", "minimum = \"\"", "not numbers set apart by dots"),
            ("minimum = \"1.24.6\"", "minimum = \"1..6\"", "not numbers set apart by dots"),
            ("https://proxy", "http://proxy", "not an https:// URL of a .zip file"),
            (".zip\"", ".tar.gz\"", "not an https:// URL of a .zip file"),
            (sha, &sha[..63], "not a SHA-256"),
            (sha, &sha.replace('7', "g"), "not a SHA-256"),
            ("size = 75846323", "size = 0", "not 0"),
            ("root = \"golang.org", "root = \"/golang.org", "not a path inside the zip"),
            ("root = \"golang.org", "root = \"../golang.org", "not a path inside the zip"),
            ("amd64\"\n", "amd64/\"\n", "not a path inside the zip"),
        ] {
            let text = format!("{GO_BOOTSTRAP}\n");
            assert!(text.contains(from), "{from}");
            let problem = bootstrap_problem(&go_recipe(&text.replacen(from, to, 1)))
                .unwrap_or_else(|| panic!("`{to}` was not refused"));
            assert!(problem.contains(expected), "`{to}` gave `{problem}`, not `{expected}`");
        }
    }

    #[test]
    fn a_go_release_is_compared_with_the_oldest_that_will_do() {
        for (version, minimum, at_least) in [
            ("go1.24.6", "1.24.6", true),
            ("go1.24.7", "1.24.6", true),
            ("go1.27.4", "1.24.6", true),
            ("go1.25", "1.24.6", true),
            ("go1.30.0", "1.24.6", true),
            ("go2.0", "1.24.6", true),
            ("go1.24.5", "1.24.6", false),
            ("go1.24", "1.24.6", false),
            ("go1.22.12", "1.24.6", false),
            ("go1.9.9", "1.24.6", false),
            // go1.25 is go1.25.0.
            ("go1.25", "1.25.0", true),
            ("go1.25.0", "1.25", true),
            // A candidate comes before its release, and after the last
            // release of the series before it.
            ("go1.25rc1", "1.25", false),
            ("go1.25rc1", "1.24.6", true),
            ("go1.25beta2", "1.25.0", false),
            // What a VERSION file's first line may carry after the release.
            ("go1.24.7 X:nocoverageredesign", "1.24.6", true),
            ("go1.24.6 X:nocoverageredesign", "1.24.6", true),
        ] {
            assert_eq!(
                go_release_at_least(version, minimum),
                Some(at_least),
                "{version} against {minimum}"
            );
        }
        for version in ["", "devel go1.27-abc123", "1.24.7", "go", "gox", "go.1", "go1..2", "go-1"]
        {
            assert_eq!(go_release_at_least(version, "1.24.6"), None, "`{version}`");
        }
    }

    /// A folder under the temporary folder, empty.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ob-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_hosts_go_is_taken_when_it_is_a_release_that_will_do() {
        let recipe = go_recipe(GO_BOOTSTRAP);
        let bootstrap = recipe.bootstrap.as_ref().unwrap();
        let host = scratch("host-go");
        let refused = |dir: &Path| host_go(&recipe, bootstrap, dir).unwrap_err().to_string();
        // Nothing there; a tree with no VERSION.
        assert!(refused(&host).contains("whose VERSION file cannot be read"));
        std::fs::create_dir_all(host.join("bin")).unwrap();
        std::fs::write(host.join("bin/go"), "").unwrap();
        assert!(refused(&host).contains("whose VERSION file cannot be read"));
        // Too old, not a release, and a release that will do.
        std::fs::write(host.join("VERSION"), "go1.22.1\ntime 2024-03-05T00:00:00Z\n").unwrap();
        let old = refused(&host);
        assert!(old.contains("is go1.22.1, and go 1.27.4 builds with Go 1.24.6 or later"), "{old}");
        std::fs::write(host.join("VERSION"), "devel go1.27-abc\n").unwrap();
        assert!(refused(&host).contains("is no release of Go"));
        std::fs::write(host.join("VERSION"), "go1.24.7\ntime 2025-08-27T15:49:40Z\n").unwrap();
        host_go(&recipe, bootstrap, &host).unwrap();
        // A release with no go command in it.
        std::fs::remove_file(host.join("bin/go")).unwrap();
        assert!(refused(&host).contains("has no bin/go"));
        std::fs::remove_dir_all(&host).unwrap();
    }

    /// A zip as Go's toolchain module has, small: `root` holds a tree with
    /// a VERSION file, programs in `bin` and `pkg/tool/linux_amd64`, and a
    /// source file; its modes are those of a file made here, not
    /// executable. Returns its path, and a [`Bootstrap`] that pins it.
    fn fake_toolchain_zip(dir: &Path, root: &str) -> (PathBuf, Bootstrap) {
        let tree = dir.join("tree").join(root);
        for (file, text) in [
            ("VERSION", "go1.99.0\n"),
            ("bin/go", "#!/bin/sh\n"),
            ("bin/gofmt", "#!/bin/sh\n"),
            ("pkg/tool/linux_amd64/compile", "compile"),
            ("pkg/tool/linux_amd64/link", "link"),
            ("src/fmt/print.go", "package fmt\n"),
        ] {
            std::fs::create_dir_all(tree.join(file).parent().unwrap()).unwrap();
            std::fs::write(tree.join(file), text).unwrap();
        }
        let zip = dir.join("toolchain.zip");
        let top = root.split('/').next().unwrap();
        let made = Command::new("python3")
            .args(["-I", "-m", "zipfile", "-c"])
            .arg(&zip)
            .arg(top)
            .current_dir(dir.join("tree"))
            .status()
            .unwrap();
        assert!(made.success());
        let ctx = test_ctx(dir);
        let bootstrap = Bootstrap {
            minimum: "1.24.6".into(),
            url: "https://example.org/toolchain.zip".into(),
            sha256: process::sha256(&ctx, &zip).unwrap(),
            size: std::fs::metadata(&zip).unwrap().len(),
            root: root.into(),
        };
        (zip, bootstrap)
    }

    /// A toolchain of nothing but its folder, for the scripts it writes.
    fn bare_toolchain(root: &Path) -> Toolchain {
        Toolchain::bare(root.join("out/toolchain"))
    }

    /// Go for Radix in `root`, over a Go that prints how it was run, with
    /// three patched modules: x/sys for every version, two of go-isatty.
    fn go_for_radix(root: &Path) -> Toolchain {
        let toolchain = bare_toolchain(root);
        let bin = toolchain.dir.join("go/bin");
        std::fs::create_dir_all(&bin).unwrap();
        write_script(
            &bin.join("go"),
            "#!/bin/sh\necho \"GOFLAGS=$GOFLAGS\"\necho \"GOOS=$GOOS GOPROXY=$GOPROXY \
             GOSUMDB=$GOSUMDB GOTOOLCHAIN=$GOTOOLCHAIN\"\necho \"args=$*\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(toolchain.go_modules()).unwrap();
        std::fs::write(
            toolchain.go_modules().join("list"),
            "golang.org/x/sys v0.50.0 golang.org_x_sys@v0.50.0 all 1.22\n\
             github.com/mattn/go-isatty v0.0.20 github.com_mattn_go-isatty@v0.0.20 one 1.15\n\
             github.com/mattn/go-isatty v0.0.24 github.com_mattn_go-isatty@v0.0.24 one 1.15\n",
        )
        .unwrap();
        std::fs::create_dir_all(toolchain.bin()).unwrap();
        write_script(&toolchain.go(), &toolchain.go_script(&test_ctx(root))).unwrap();
        toolchain
    }

    /// A program's module in `root`/app with `go_mod` and `go_sum`, and a
    /// folder in it to run from.
    fn go_program(root: &Path, go_mod: &str, go_sum: &str) -> PathBuf {
        let app = root.join("app");
        std::fs::create_dir_all(app.join("cmd/app")).unwrap();
        std::fs::write(app.join("go.mod"), go_mod).unwrap();
        std::fs::write(app.join("go.sum"), go_sum).unwrap();
        app
    }

    const APP_GO_SUM: &str = "github.com/mattn/go-isatty v0.0.20 h1:one=\n\
        github.com/mattn/go-isatty v0.0.20/go.mod h1:two=\n\
        golang.org/x/sys v0.30.0/go.mod h1:three=\n";

    #[test]
    fn go_for_radix_gives_a_program_the_patched_modules_its_go_sum_names() {
        let root = scratch("radix-go");
        let toolchain = go_for_radix(&root);
        let go_mod = "module example.com/app\n\ngo 1.19\n\nrequire (\n\
                      \tgithub.com/mattn/go-isatty v0.0.20\n\tgolang.org/x/sys v0.30.0\n)\n";
        let app = go_program(&root, go_mod, APP_GO_SUM);
        let out = Command::new("sh")
            .arg(toolchain.go())
            .args(["build", "./cmd/app"])
            .current_dir(app.join("cmd/app"))
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        // x/sys at any version, go-isatty at the one go.sum names; the
        // patched modules need Go 1.22, so the line is raised and the
        // program keeps Go 1.19's run-time defaults.
        assert_eq!(
            std::fs::read_to_string(app.join(".radix-go.mod")).unwrap(),
            "module example.com/app\n\ngo 1.22\n\nrequire (\n\
             \tgithub.com/mattn/go-isatty v0.0.20\n\tgolang.org/x/sys v0.30.0\n)\n\
             godebug default=go1.19\n\
             replace golang.org/x/sys => ./.radix-go/golang.org_x_sys@v0.50.0\n\
             replace github.com/mattn/go-isatty v0.0.20 => \
             ./.radix-go/github.com_mattn_go-isatty@v0.0.20\n"
        );
        assert_eq!(std::fs::read_to_string(app.join(".radix-go.sum")).unwrap(), APP_GO_SUM);
        assert_eq!(std::fs::read_link(app.join(".radix-go")).unwrap(), toolchain.go_modules());
        let printed = String::from_utf8_lossy(&out.stdout);
        let modfile = app.join(".radix-go.mod");
        assert!(
            printed.contains(&format!(
                "GOFLAGS=-mod=readonly -modfile={} -trimpath -buildvcs=false -modcacherw\n",
                modfile.display()
            )),
            "{printed}"
        );
        assert!(
            printed.contains(
                "GOOS=radix GOPROXY=https://proxy.golang.org GOSUMDB=off GOTOOLCHAIN=local\n"
            ),
            "{printed}"
        );
        assert!(printed.contains("args=build ./cmd/app\n"), "{printed}");
        // For the build machine, with the same modules.
        let out = Command::new("sh")
            .arg(toolchain.go())
            .args(["--host", "run", "./gen"])
            .current_dir(&app)
            .env("GOOS", "plan9")
            .output()
            .unwrap();
        let printed = String::from_utf8_lossy(&out.stdout);
        assert!(printed.contains("GOOS= GOPROXY=https://proxy.golang.org"), "{printed}");
        assert!(printed.contains(&format!("-modfile={}", modfile.display())), "{printed}");
        assert!(printed.contains("args=run ./gen\n"), "{printed}");
        // go.mod itself is as it was.
        assert_eq!(std::fs::read_to_string(app.join("go.mod")).unwrap(), go_mod);
    }

    #[test]
    fn go_for_radix_keeps_a_go_line_newer_than_the_patched_modules_need() {
        let root = scratch("radix-go-newer");
        let toolchain = go_for_radix(&root);
        let go_mod = "module example.com/app\n\ngo 1.26.0\n";
        let app = go_program(&root, go_mod, "golang.org/x/sys v0.48.0 h1:four=\n");
        let out = Command::new("sh").arg(toolchain.go()).arg("version").current_dir(&app).output();
        assert!(out.unwrap().status.success());
        assert_eq!(
            std::fs::read_to_string(app.join(".radix-go.mod")).unwrap(),
            "module example.com/app\n\ngo 1.26.0\n\
             replace golang.org/x/sys => ./.radix-go/golang.org_x_sys@v0.50.0\n"
        );
    }

    #[test]
    fn go_for_radix_refuses_a_program_that_replaces_a_patched_module() {
        for replace in [
            "replace github.com/mattn/go-isatty => github.com/someone/isatty v1.0.0\n",
            "replace (\n\tgithub.com/mattn/go-isatty v0.0.20 => ../isatty\n)\n",
        ] {
            let root = scratch("radix-go-replaced");
            let toolchain = go_for_radix(&root);
            let go_mod = format!("module example.com/app\n\ngo 1.24\n\n{replace}");
            let app = go_program(&root, &go_mod, APP_GO_SUM);
            let out =
                Command::new("sh").arg(toolchain.go()).arg("build").current_dir(&app).output();
            let out = out.unwrap();
            assert!(!out.status.success(), "{replace}");
            let said = String::from_utf8_lossy(&out.stderr);
            assert!(
                said.contains(
                    "x86_64-radix-go: go.mod replaces github.com/mattn/go-isatty, which Radix \
                     needs patched"
                ),
                "{said}"
            );
            assert!(out.stdout.is_empty(), "Go ran: {}", String::from_utf8_lossy(&out.stdout));
        }
    }

    #[test]
    fn a_go_modules_proxy_url_spells_capitals_as_the_proxy_does() {
        let module = |path: &str, version: &str| GoModule {
            path: path.into(),
            version: version.into(),
            sha256: "0".repeat(64),
            license: "MIT".into(),
            all_versions: false,
        };
        let bloom = module("github.com/AndreasBriese/bbloom", "v0.0.0-20190825152654-46b345b51c96");
        assert_eq!(
            bloom.url(),
            "https://proxy.golang.org/github.com/!andreas!briese/bbloom/@v/\
             v0.0.0-20190825152654-46b345b51c96.zip"
        );
        assert_eq!(
            bloom.dir(),
            "github.com_AndreasBriese_bbloom@v0.0.0-20190825152654-46b345b51c96"
        );
        let recipe = |modules: Vec<GoModule>| Recipe { modules, ..go_recipe(GO_BOOTSTRAP) };
        assert_eq!(modules_problem(&recipe(vec![module("golang.org/x/sys", "v0.50.0")])), None);
        for (path, version) in [
            ("", "v1.0.0"),
            ("a//b", "v1.0.0"),
            ("a/../b", "v1.0.0"),
            ("a b", "v1"),
            ("a", "1.0.0"),
        ] {
            let problem = modules_problem(&recipe(vec![module(path, version)])).unwrap();
            assert!(problem.contains("is not a Go module's path and version"), "{problem}");
        }
        let twice = recipe(vec![module("a/b", "v1.0.0"), module("a_b", "v1.0.0")]);
        assert!(modules_problem(&twice).unwrap().contains("goes where another module does"));
        let mut unsure = module("a/b", "v1.0.0");
        unsure.sha256 = "abc".into();
        assert!(modules_problem(&recipe(vec![unsure])).unwrap().contains("is not a SHA-256"));
    }

    /// What `go version -m` printed for micro, trimmed: a module from the
    /// proxy, one the go-modules port patched, one replaced by another of
    /// the proxy's, and the lines that are no module.
    const MICRO_MODULES: &str = "/out/bundle/bin/micro: go1.27.4\n\
        \tpath\tgithub.com/zyedidia/micro/v2/cmd/micro\n\
        \tmod\tgithub.com/zyedidia/micro/v2\t(devel)\t\n\
        \tdep\tgithub.com/blang/semver\tv3.5.1+incompatible\th1:cQNTCjp1=\n\
        \tdep\tgithub.com/creack/pty\tv1.1.18\n\
        \t=>\t./.radix-go/github.com_creack_pty@v1.1.18\t(devel)\t\n\
        \t\n\
        \tdep\tgithub.com/kballard/go-shellquote\tv0.0.0-20180428030007-95032a82bc51\n\
        \t=>\tgithub.com/micro-editor/go-shellquote\tv0.0.0-20250101105543-feb6c39314f5\th1:D7BP=\n\
        \tdep\tgolang.org/x/sys\tv0.30.0\n\
        \t=>\t./.radix-go/golang.org_x_sys@v0.50.0\t(devel)\t\n\
        \t\n\
        \tbuild\t-buildmode=exe\n\
        \tbuild\tCGO_ENABLED=0\n";

    fn proxy(path: &str, version: &str) -> LinkedModule {
        LinkedModule::Proxy { path: path.into(), version: version.into() }
    }

    fn patched(folder: &str, named: &[&str]) -> LinkedModule {
        LinkedModule::Patched {
            folder: folder.into(),
            named: named.iter().map(ToString::to_string).collect(),
        }
    }

    #[test]
    fn a_module_a_go_program_links_is_the_proxys_or_a_patched_one_or_the_one_that_replaces_it() {
        assert_eq!(
            linked_go_modules(MICRO_MODULES).unwrap(),
            [
                proxy("github.com/blang/semver", "v3.5.1+incompatible"),
                patched("github.com_creack_pty@v1.1.18", &["v1.1.18"]),
                // The code is the replacing module's, not the one go.mod names.
                proxy(
                    "github.com/micro-editor/go-shellquote",
                    "v0.0.0-20250101105543-feb6c39314f5"
                ),
                patched("golang.org_x_sys@v0.50.0", &["v0.30.0"]),
            ]
        );
        // No program, no module.
        assert!(linked_go_modules("").unwrap().is_empty());
        assert!(linked_go_modules("/bin/x: go1.27.4\n\tpath\texample.com/x\n").unwrap().is_empty());
    }

    #[test]
    fn a_module_two_programs_link_is_listed_once_and_a_patched_one_names_each_version() {
        let text = format!(
            "{MICRO_MODULES}/out/bundle/bin/other: go1.27.4\n\
             \tpath\texample.com/other\n\
             \tdep\tgithub.com/blang/semver\tv3.5.1+incompatible\th1:cQNTCjp1=\n\
             \tdep\tgithub.com/blang/semver\tv4.0.0\th1:other=\n\
             \tdep\tgolang.org/x/sys\tv0.35.0\n\
             \t=>\t./.radix-go/golang.org_x_sys@v0.50.0\t(devel)\t\n\
             \tdep\tgolang.org/x/sys\tv0.30.0\n\
             \t=>\t./.radix-go/golang.org_x_sys@v0.50.0\t(devel)\t\n"
        );
        assert_eq!(
            linked_go_modules(&text).unwrap(),
            [
                proxy("github.com/blang/semver", "v3.5.1+incompatible"),
                patched("github.com_creack_pty@v1.1.18", &["v1.1.18"]),
                proxy(
                    "github.com/micro-editor/go-shellquote",
                    "v0.0.0-20250101105543-feb6c39314f5"
                ),
                patched("golang.org_x_sys@v0.50.0", &["v0.30.0", "v0.35.0"]),
                proxy("github.com/blang/semver", "v4.0.0"),
            ]
        );
    }

    #[test]
    fn a_modules_file_that_go_did_not_print_or_names_an_unknown_folder_is_refused() {
        for (text, expected) in [
            ("\tdep\tgithub.com/a/b\n", "line 1: a dep line has no path and version"),
            ("\tpath\tx\n\tdep\t\tv1.0.0\n", "line 2: a dep line has no path and version"),
            ("\tdep\ta/b\tv1.0.0\n\t=>\tc/d\n", "line 2: a `=>` line has no path and version"),
            ("\t=>\tc/d\tv1.0.0\n", "line 1: a `=>` line replaces no dep"),
            (
                "\tdep\ta/b\tv1.0.0\n\tbuild\tx=y\n\t=>\tc/d\tv1.0.0\n",
                "line 3: a `=>` line replaces no dep",
            ),
            (
                "\tdep\ta/b\tv1.0.0\n\t=>\t./vendor/b\t(devel)\t\n",
                "line 1: a/b v1.0.0 is replaced by the folder ./vendor/b, which is not one of go-modules's",
            ),
            ("\tdep\ta/b\tv1.0.0\n\t=>\t/src/b\t(devel)\t\n", "is replaced by the folder /src/b"),
        ] {
            let err = linked_go_modules(text).unwrap_err();
            assert!(err.contains(expected), "`{err}` does not say `{expected}`");
        }
    }

    #[test]
    fn a_port_lists_its_modules_in_its_info_folder_or_has_none() {
        let root = scratch("go-linked");
        let ctx = test_ctx(&root);
        assert_eq!(go_modules_linked(&ctx, "micro").unwrap(), None);
        let info = info_dir(&ctx, "micro");
        std::fs::create_dir_all(&info).unwrap();
        std::fs::write(info.join("modules.txt"), MICRO_MODULES).unwrap();
        assert_eq!(go_modules_linked(&ctx, "micro").unwrap().unwrap().len(), 4);
        std::fs::write(info.join("modules.txt"), "\tdep\ta/b\n").unwrap();
        let err = go_modules_linked(&ctx, "micro").unwrap_err().to_string();
        assert!(err.contains("modules.txt: line 1: a dep line has no path and version"), "{err}");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_module_is_in_the_cache_by_its_escaped_path_and_version() {
        let ctx = test_ctx(Path::new("/src"));
        assert_eq!(
            go_cached_module(
                &ctx,
                "github.com/AndreasBriese/bbloom",
                "v0.0.0-20190825152654-46b345b51c96"
            ),
            Path::new(
                "/src/target/ports/sources/go-mod/github.com/!andreas!briese/bbloom@\
                 v0.0.0-20190825152654-46b345b51c96"
            )
        );
        assert_eq!(
            go_cached_module(&ctx, "github.com/blang/semver", "v3.5.1+incompatible"),
            Path::new(
                "/src/target/ports/sources/go-mod/github.com/blang/semver@v3.5.1+incompatible"
            )
        );
        assert_eq!(
            go_proxy_url("github.com/Masterminds/semver/v3", "v3.2.0"),
            "https://proxy.golang.org/github.com/!masterminds/semver/v3/@v/v3.2.0.zip"
        );
    }

    fn test_ctx(root: &Path) -> Ctx {
        Ctx {
            out: root.join("out"),
            target: root.join("target"),
            root: root.to_path_buf(),
            config: crate::config::Config::default(),
            verbose: false,
        }
    }

    #[test]
    fn a_toolchain_zip_is_unpacked_with_its_programs_executable() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch("go-zip");
        let root = "golang.org/toolchain@v0.0.1-go1.99.0.linux-amd64";
        let (zip, bootstrap) = fake_toolchain_zip(&dir, root);
        let ctx = test_ctx(&dir);
        let into = dir.join("sources/go-bootstrap-abc");
        let tree = unpack_bootstrap(&ctx, &bootstrap, &zip, &into).unwrap();
        assert_eq!(tree, into);
        assert_eq!(std::fs::read_to_string(tree.join("VERSION")).unwrap(), "go1.99.0\n");
        let executable =
            |file: &str| tree.join(file).metadata().unwrap().permissions().mode() & 0o777;
        for program in
            ["bin/go", "bin/gofmt", "pkg/tool/linux_amd64/compile", "pkg/tool/linux_amd64/link"]
        {
            assert_eq!(executable(program), 0o755, "{program}");
        }
        // What is not a program is not made one, and nothing is left of the
        // unpacking beside the tree.
        assert_eq!(executable("src/fmt/print.go") & 0o111, 0);
        assert_eq!(executable("VERSION") & 0o111, 0);
        assert!(!dir.join("sources/go-bootstrap-abc.partial").exists());
        // Unpacked again over the old tree, it is the same.
        std::fs::write(tree.join("stray"), "").unwrap();
        let again = unpack_bootstrap(&ctx, &bootstrap, &zip, &into).unwrap();
        assert!(!again.join("stray").exists());
        assert_eq!(listing(&again).unwrap(), listing(&tree).unwrap());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_toolchain_zip_of_another_size_or_hash_or_layout_is_refused() {
        let dir = scratch("go-zip-refused");
        let root = "golang.org/toolchain@v0.0.1-go1.99.0.linux-amd64";
        let (zip, bootstrap) = fake_toolchain_zip(&dir, root);
        let ctx = test_ctx(&dir);
        let into = dir.join("sources/go-bootstrap-abc");
        let refused = |pinned: &Bootstrap| {
            let err = unpack_bootstrap(&ctx, pinned, &zip, &into).unwrap_err().to_string();
            assert!(!into.exists(), "{err}: a tree was left");
            err
        };
        let size = Bootstrap { size: bootstrap.size + 1, ..clone_of(&bootstrap) };
        let err = refused(&size);
        assert!(
            err.contains(&format!("is {} bytes, not the {}", bootstrap.size, size.size)),
            "{err}"
        );
        let hash = Bootstrap { sha256: "ab".repeat(32), ..clone_of(&bootstrap) };
        let err = refused(&hash);
        assert!(err.contains("has SHA-256 ") && err.contains(&"ab".repeat(32)), "{err}");
        assert!(err.contains("the recipe pins; not using it"), "{err}");
        // The zip is what the recipe pins, but holds no tree where it says.
        let elsewhere =
            Bootstrap { root: "golang.org/toolchain@other".into(), ..clone_of(&bootstrap) };
        let err = refused(&elsewhere);
        assert!(err.contains("holds no golang.org/toolchain@other/bin/go and VERSION"), "{err}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn clone_of(bootstrap: &Bootstrap) -> Bootstrap {
        Bootstrap {
            minimum: bootstrap.minimum.clone(),
            url: bootstrap.url.clone(),
            sha256: bootstrap.sha256.clone(),
            size: bootstrap.size,
            root: bootstrap.root.clone(),
        }
    }

    #[test]
    fn an_unpacked_toolchain_is_not_fetched_again() {
        let dir = scratch("go-zip-cached");
        let ctx = test_ctx(&dir);
        let recipe = go_recipe(GO_BOOTSTRAP);
        let bootstrap = recipe.bootstrap.unwrap();
        // As fetch_bootstrap leaves it: the tree, and the files it holds
        // noted beside it. No download is attempted for it: this test has
        // no network, and a download would fail it.
        let sources = ctx.target.join("ports/sources");
        let tree = sources.join(format!("go-bootstrap-{}", &bootstrap.sha256[..12]));
        std::fs::create_dir_all(tree.join("bin")).unwrap();
        std::fs::write(tree.join("bin/go"), "go").unwrap();
        write_file(
            &sources.join(format!("go-bootstrap-{}.files", &bootstrap.sha256[..12])),
            &listing(&tree).unwrap(),
        )
        .unwrap();
        assert_eq!(fetch_bootstrap(&ctx, "go", &bootstrap).unwrap(), tree);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The layers of an image, as a recipe writes them.
    const IMAGE: &str = r#"
        registry = "registry-1.docker.io"
        repository = "freedesktopsdk/platform"
        tag = "25.08-x86_64"
        [[layers]]
        digest = "sha256:c941ef6db317ca33564e5ab2bdb75667a55db876648fd65953e4614d7dd60868"
        size = 22603712
        [[layers]]
        digest = "sha256:4cce114f478cab3a53fe95cb9cb9ff945518127d954502cb69015670f428fc82"
        size = 220796320
    "#;

    #[test]
    fn an_images_layers_are_pinned_by_registry_repository_digest_and_size() {
        let image: Source = toml::from_str(IMAGE).unwrap();
        assert!(image.is_image());
        assert_eq!(image_problem(&image), None);
        assert_eq!(
            image.download(),
            "https://registry-1.docker.io/v2/freedesktopsdk/platform/blobs/\
             sha256:c941ef6db317ca33564e5ab2bdb75667a55db876648fd65953e4614d7dd60868"
        );
        assert_eq!(image.release(), "25.08-x86_64");
        for (from, to, wanted) in [
            ("registry-1.docker.io", "", "is not a host name"),
            ("registry-1.docker.io", "docker.io/../x", "is not a host name"),
            ("registry-1.docker.io", "-x", "is not a host name"),
            ("freedesktopsdk/platform", "", "is not a repository's name"),
            ("freedesktopsdk/platform", "a//b", "is not a repository's name"),
            ("freedesktopsdk/platform", "a/../b", "is not a repository's name"),
            ("freedesktopsdk/platform", "A/b", "is not a repository's name"),
            ("tag = \"25.08-x86_64\"\n", "", "needs its tag"),
            ("sha256:c941", "md5:c941", "is not `sha256:`"),
            (
                "sha256:4cce114f478cab3a53fe95cb9cb9ff945518127d954502cb69015670f428fc82",
                "sha256:4CCE114F478CAB3A53FE95CB9CB9FF945518127D954502CB69015670F428FC82",
                "lower-case",
            ),
            ("size = 22603712", "size = 0", "is 0 bytes"),
        ] {
            let text = IMAGE.replacen(from, to, 1);
            let broken: Source = toml::from_str(&text).unwrap();
            let problem = image_problem(&broken).unwrap_or_default();
            assert!(problem.contains(wanted), "{from} -> {to}: {problem}");
        }
        // A registry and a repository name layers to fetch from them.
        let bare: Source = toml::from_str("registry = \"registry-1.docker.io\"").unwrap();
        assert!(!bare.is_image());
        assert!(image_problem(&bare).unwrap().contains("go with layers"));
        assert_eq!(image_problem(&Source::default()), None);
    }

    #[test]
    fn a_source_is_one_kind_and_an_image_has_no_commit() {
        let recipe = |source: &str| {
            let text = format!(
                "name = \"base\"\nversion = \"1.0.0\"\nsummary = \"s\"\nlicense = \"MIT\"\n\
                 homepage = \"https://example.org\"\n[source]\n{source}"
            );
            toml::from_str::<Recipe>(&text).unwrap()
        };
        let dir = scratch("image-recipe").join("base");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("build.sh"), "").unwrap();
        let ctx = test_ctx(&dir);
        let checked = |source: &str| check(&ctx, &recipe(source), &dir).map_err(|e| e.to_string());
        let layers = IMAGE.replace("[[layers]]", "[[source.layers]]");
        // The layers are a source of their own, and no other goes with them.
        assert_eq!(checked(&layers).err(), None::<String>);
        let with_url = format!("url = \"https://example.org/a.tar.xz\"\n{layers}");
        assert!(checked(&with_url).unwrap_err().contains("one of git, url, layers and rust-src"));
        let with_commit = format!("commit = \"{}\"\n{layers}", "0".repeat(40));
        assert!(checked(&with_commit).unwrap_err().contains("commit and tag go with git"));
        std::fs::remove_dir_all(dir.parent().unwrap()).unwrap();
    }

    #[test]
    fn a_registrys_challenge_names_its_token_service() {
        let docker = "HTTP/2 401 \r\ncontent-type: application/json\r\n\
                      www-authenticate: Bearer realm=\"https://auth.docker.io/token\",\
                      service=\"registry.docker.io\"\r\n\r\n";
        assert_eq!(
            bearer_challenge(docker),
            Some(("https://auth.docker.io/token".into(), "registry.docker.io".into()))
        );
        // Header names are not case-sensitive, and the order of the fields
        // is the registry's.
        let other = "WWW-Authenticate: Bearer service=\"ghcr.io\",realm=\"https://ghcr.io/token\"";
        assert_eq!(
            bearer_challenge(other),
            Some(("https://ghcr.io/token".into(), "ghcr.io".into()))
        );
        for refused in [
            "HTTP/2 200 \r\n",
            "www-authenticate: Basic realm=\"registry\"",
            "www-authenticate: Bearer realm=\"http://auth.example/token\",service=\"s\"",
            "www-authenticate: Bearer realm=\"https://auth.example/token\"",
        ] {
            assert_eq!(bearer_challenge(refused), None, "{refused}");
        }
    }

    #[test]
    fn layers_that_are_there_whole_are_not_fetched_again() {
        let dir = scratch("image-cached");
        let ctx = test_ctx(&dir);
        let image: Source = toml::from_str(IMAGE).unwrap();
        // As fetch_image leaves them, but for their size: a layer is
        // taken for the blob that was checked when it has the pinned size.
        // No download is attempted for them: this test has no network, and
        // a download would fail it.
        let folder = ctx.target.join("ports/sources/base-c941ef6db317");
        std::fs::create_dir_all(&folder).unwrap();
        for (at, layer) in image.layers.iter().enumerate() {
            let file =
                std::fs::File::create(folder.join(format!("layer-{}.blob", at + 1))).unwrap();
            file.set_len(layer.size).unwrap();
        }
        assert_eq!(fetch_image(&ctx, "base", &image).unwrap(), folder);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

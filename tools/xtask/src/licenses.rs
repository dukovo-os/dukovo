// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The license manifest and notices that every image carries.
//!
//! `ob image` lists every component that ends up in the image, checks each
//! license against the allow-list of the component's area, and writes an
//! SPDX 2.3 document, the license texts and each component's own notice
//! files to `/system/resources/licenses`. See
//! docs/adr/0021-license-manifest-in-every-image.md.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::ports::{self, LinkedModule, Port, Ports, Recipe, Toolchain};
use crate::process::{self, step};
use crate::{Context, Ctx, Error, Result, VERSION, kernel, patched, userspace};

/// Where the manifest and notices go, relative to the image root.
pub const DIR: &str = "system/resources/licenses";

/// File name of the SPDX document, in [`DIR`] and next to the image.
pub const MANIFEST: &str = "dukovo.spdx";

/// The allow-list of the kernel area. It is also the cargo-deny policy of the
/// kernel workspace, so CI and `ob image` enforce the same list.
pub const KERNEL_POLICY: &str = "kernel/deny.toml";

/// Licenses a boot loader, and the memory test its menu starts in the
/// kernel's place, may use: each is a separate program that shares no
/// code with Dukovo's, so the GPL does not reach Dukovo code (ADRs 0021
/// and 0488). GRUB is under the GPL version 3, memtest86+ version 2.
const BOOTLOADER_ALLOW: &[&str] = &["GPL-2.0-only", "GPL-3.0-or-later"];

/// Licenses of closed firmware, which only a local build adds (ADR 0007).
const FIRMWARE_ALLOW: &[&str] = &["LicenseRef-Intel-Microcode", "LicenseRef-AMD-Microcode"];

/// Licenses a ported program, and what is built into it, may use beyond
/// the kernel area's: it runs as a process of its own, so its terms reach
/// no Dukovo code (ADR 0021), and the GPL, which GNU grep, sed, diffutils
/// and patch are under, is allowed for that reason (ADR 0087). CC0-1.0 is
/// a public domain dedication (uutils' `tail` follows files with the
/// notify crate, ADR 0080). ncurses' license is the X11 license that also
/// lets one distribute with modifications; the One True Awk's is Lucent's,
/// which SPDX has no identifier for. The rest are permissive licenses of
/// the compression and archive ports: bzip2's own, Info-ZIP's for zip,
/// and in jq, ICU's for decNumber, David M. Gay's for its number
/// formatting, and CC-BY-3.0 for its manual (ADR 0088); XZ Utils' 0BSD
/// is in the kernel area's list, for smoltcp (ADR 0180). The
/// 4.4BSD database library vi keeps its lines in is under the University
/// of California's license with the advertising clause, which the
/// University rescinded in 1999 (ADR 0090). 7-Zip is under the LGPL, as a
/// program of its own that nothing links, on the GPL's terms (ADR 0090).
/// tmux's allocation wrappers are under Tatu Ylonen's short notice from
/// ssh, and the base64 code it builds in, as the C library has none, also
/// under IBM's notice, which SPDX has no identifier for (ADR 0091).
/// The root certificates Mozilla trusts come as data under the Community
/// Data License Agreement - Permissive 2.0, which asks only that its text
/// go with them (ADR 0192). git is under version 2 of the GPL only, with
/// parts under the LGPL and the GPL's later versions, as a program of its
/// own (ADR 0287). The IANA time zone database is in the public domain,
/// which SPDX has no identifier for (ADR 0194). curl's license is its own,
/// the MIT license in other words (ADR 0195). OpenSSH is under Tatu
/// Ylonen's and OpenSSH's notices, David Mazieres' for ssh-keyscan and
/// Patrick Powell's for snprintf, and its cryptography from SUPERCOP and
/// Rijndael's is in the public domain (ADR 0289). iperf3 keeps the first
/// iperf's units.c under the University of Illinois/NCSA license, parts
/// of Lucent's libtask under the notice SPDX names dtoa, and Mathias
/// Panzenböck's portable_endian.h in the public domain (ADR 0342).
/// libpcap and tcpdump are under the Lawrence Berkeley Laboratory's and
/// the University of California's BSD licenses, some with the clause on
/// advertising (BSD-4-Clause-Shortened, BSD-4.3RENO, BSD-4.3TAHOE and the
/// 4-clause ones), and others' BSD-style notices, each of which their
/// LicenseRef-libpcap-notices and LicenseRef-tcpdump-notices keep as the
/// files have it (ADR 0346). eSpeak NG builds Unicode's character data
/// into itself, under Unicode's license for its data files (ADR 0324).
/// BRLTTY and its tables are under the LGPL, as a program of its own with
/// the project's screen driver built in (ADR 0326). CPython is under the
/// PSF's license and those before it, `Python-2.0`, which are permissive
/// (ADR 0495). SQLite is in the public domain: its files carry a blessing
/// in place of a license, which SPDX names `blessing` (ADR 0511). Tcl,
/// whose license is BSD-style, is a toolchain port that builds SQLite's
/// tests only and is in no manifest, so its license is not here.
/// libxkbcommon and the keyboard layouts of xkeyboard-config are under
/// the X Consortium's and The Open Group's forms of the MIT license
/// (`X11`, `MIT-open-group`), the historical permission notices of the
/// companies that wrote X's keyboard code (`HPND`, `HPND-sell-variant`
/// and Digital's, which SPDX has no identifier for), and Ivan Zinoviev's
/// for his layouts, all of them permissive (ADR 0712).
const PORT_EXTRA_ALLOW: &[&str] = &[
    "CC0-1.0",
    "GPL-2.0-only",
    "GPL-2.0-or-later",
    "GPL-3.0-or-later",
    "X11-distribute-modifications-variant",
    "LicenseRef-onetrueawk",
    "bzip2-1.0.6",
    "Info-ZIP",
    "ICU",
    "dtoa",
    "CC-BY-3.0",
    "BSD-4-Clause-UC",
    "LGPL-2.1-or-later",
    "SSH-short",
    "LicenseRef-IBM-base64",
    "CDLA-Permissive-2.0",
    "LicenseRef-tz-public-domain",
    "curl",
    "SSH-OpenSSH",
    "ssh-keyscan",
    "snprintf",
    "LicenseRef-OpenSSH-public-domain",
    "NCSA",
    "LicenseRef-iperf3-public-domain",
    "BSD-4-Clause",
    "BSD-4-Clause-Shortened",
    "BSD-4.3RENO",
    "BSD-4.3TAHOE",
    "LicenseRef-libpcap-notices",
    "LicenseRef-tcpdump-notices",
    "Unicode-DFS-2016",
    "Python-2.0",
    "blessing",
    "HPND",
    "HPND-sell-variant",
    "X11",
    "MIT-open-group",
    "xkeyboard-config-Zinoviev",
    "LicenseRef-digital-equipment-corporation",
];

/// Crates from crates.io that compile another project's C code into what
/// links them, under a license their Cargo.toml does not name: each
/// crate, the license that code adds, and the files in the crate that
/// hold its notice. onig_sys builds Oniguruma, by K. Kosako, which uutils'
/// `expr` matches regular expressions with.
const BUNDLED_C: &[(&str, &str, &[&str])] = &[("onig_sys", "BSD-2-Clause", &["oniguruma/COPYING"])];

/// Crates whose packages on crates.io carry no license file: the one their
/// repository has at that version, which Dukovo's keeps (REUSE.toml says
/// whose it is), as their licenses ask a binary's documentation to hold
/// their notices.
const KEPT_NOTICES: &[(&str, &str, &str)] =
    &[("boringtun", "0.7.1", "userspace/programs/systools/notices/LICENSE-boringtun.md")];

/// Names of the files in a source tree that hold license terms or notices.
/// PATENTS is the patent grant that Go's tree and the golang.org/x modules
/// keep beside their LICENSE, which is a grant to whoever gets the code.
const NOTICE_PREFIXES: &[&str] =
    &["LICENSE", "LICENCE", "COPYING", "COPYRIGHT", "NOTICE", "PATENTS"];

/// Where in rust-src each crate of `-Zbuild-std` lives.
const SYSROOT_CRATES: &[(&str, &str)] = &[
    ("core", "core"),
    ("alloc", "alloc"),
    ("compiler_builtins", "compiler-builtins/compiler-builtins"),
];

/// Which allow-list a component is checked against.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Area {
    /// Linked into the kernel or a /system program, in Rust or in C:
    /// permissive only.
    Kernel,
    /// A ported program, or built into one: the kernel area's licenses and
    /// [`PORT_EXTRA_ALLOW`].
    Port,
    /// A separate boot loader program.
    Bootloader,
    /// Closed firmware loaded by the kernel.
    Firmware,
}

impl Area {
    fn describe(self) -> &'static str {
        match self {
            Area::Kernel => "the kernel allow-list in kernel/deny.toml",
            Area::Port => {
                "the port allow-list (kernel/deny.toml and PORT_EXTRA_ALLOW in \
                 tools/xtask/src/licenses.rs)"
            }
            Area::Bootloader => "the boot loader allow-list in tools/xtask/src/licenses.rs",
            Area::Firmware => "the firmware allow-list in tools/xtask/src/licenses.rs",
        }
    }
}

/// One component of an image.
#[derive(Debug)]
pub struct Package {
    pub name: String,
    pub version: String,
    /// SPDX license expression.
    pub license: String,
    pub area: Area,
    /// SPDX download location, or `NOASSERTION`.
    pub download: String,
    /// SHA-256 of the source archive, when there is one (crates.io).
    pub sha256: Option<String>,
    /// License and notice files from the component's source.
    pub notices: Vec<PathBuf>,
    pub comment: String,
}

impl Package {
    fn spdx_id(&self) -> String {
        spdx_id(&format!("{}-{}", self.name, self.version))
    }

    /// Where its notices go among a bundle's or an image's: a Go module's
    /// path has slashes, which would make a folder inside a folder.
    fn notice_dir(&self) -> String {
        format!("notices/{}-{}", self.name.replace('/', "_"), self.version)
    }

    /// Its package URL, by which advisories name it (ADR 0389): a crate's
    /// from crates.io, and none for other components.
    fn purl(&self) -> Option<String> {
        self.download
            .starts_with(CRATES_DOWNLOAD)
            .then(|| format!("pkg:cargo/{}@{}", self.name, self.version.replace('+', "%2B")))
    }
}

/// Where a crate from crates.io downloads from, the start of its
/// [`Package::download`].
const CRATES_DOWNLOAD: &str = "https://crates.io/api/v1/crates/";

/// The processor makers whose microcode an image carries, each listed in
/// its manifest under its own license.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Microcode {
    pub intel: bool,
    pub amd: bool,
}

impl Microcode {
    /// The makers of the updates in `data`, the boot module of
    /// [`crate::image::read_microcode`]; none for no module.
    pub fn of(data: Option<&[u8]>) -> Self {
        let mut found = Self::default();
        for piece in data.map(radix_microcode::pieces).into_iter().flatten().flatten() {
            match piece {
                radix_microcode::Piece::Intel(_) => found.intel = true,
                radix_microcode::Piece::Amd(_) => found.amd = true,
            }
        }
        found
    }
}

/// What besides the kernel's crates goes into an image.
pub struct Extras<'a> {
    /// The GRUB the image boots with, built from its pinned source.
    pub grub: &'a crate::grub::Grub,
    /// The memory test on its start menu, built from its pinned source.
    pub memtest: &'a crate::memtest::Memtest,
    /// The processor makers whose microcode the image carries.
    pub microcode: Microcode,
    /// The ports, when the image carries the user programs: the test
    /// image and the boot archive, with the C programs and what the
    /// ports put there.
    pub programs: Option<&'a Ports>,
}

/// Lists the image's components, checks their licenses, and writes the
/// manifest and notices into `staging`. Returns the manifest's text.
pub fn install(ctx: &Ctx, staging: &Path, extras: &Extras<'_>) -> Result<String> {
    let mut packages = rust_crates(ctx, &KERNEL_PROGRAM)?;
    packages.extend(sysroot_crates(ctx)?);
    if let Some(ports) = extras.programs {
        for program in &USER_PROGRAMS {
            for package in rust_crates(ctx, program)? {
                // `core` and the like are listed once, from the kernel.
                if !packages.iter().any(|p| p.name == package.name && p.version == package.version)
                {
                    packages.push(package);
                }
            }
        }
        if !userspace::STD_ARCHIVE.is_empty() {
            for package in std_packages(ctx, ports)? {
                // `core` and the like are listed once, from the kernel.
                if !packages.iter().any(|p| p.name == package.name && p.version == package.version)
                {
                    packages.push(package);
                }
            }
        }
        for package in c_packages(ctx, ports)? {
            // A crate a Rust port links may be in the standard library too.
            if !packages.iter().any(|p| p.name == package.name && p.version == package.version) {
                packages.push(package);
            }
        }
    }
    packages.push(grub(extras.grub));
    packages.push(memtest(extras.memtest));
    if extras.microcode.intel {
        packages.push(intel_microcode());
    }
    if extras.microcode.amd {
        packages.push(amd_microcode());
    }
    let kernel_allow = kernel_allow_list(ctx)?;
    check(&packages, &kernel_allow)?;
    let ids = license_ids(&packages)?;

    let dir = staging.join(DIR);
    process::create_dir(&dir.join("texts"))?;
    for id in &ids {
        process::copy(&text_source(&ctx.root, id), &dir.join(format!("texts/{id}.txt")))?;
    }
    for package in &packages {
        for (notice, name) in package.notices.iter().zip(notice_names(&package.notices)) {
            process::copy(notice, &dir.join(package.notice_dir()).join(name))?;
        }
    }
    let created = crate::image::source_date_epoch(ctx).and_then(|e| e.parse().ok()).unwrap_or(0);
    let document = Document::image(&git_commit(ctx), created);
    let manifest = document.render(&packages, &ids, &ctx.root)?;
    write(&dir.join(MANIFEST), &manifest)?;
    write(&dir.join("README"), &readme(&packages))?;
    step(&format!(
        "Wrote the license manifest of {} components to /{DIR}/{MANIFEST}",
        packages.len()
    ));
    Ok(manifest)
}

/// Licenses a catalog bundle's port, and what is built into it, may use
/// beyond a port's (ADR 0370). The Mozilla Public License 2.0 is copyleft
/// file by file: each such file's source is the crate on crates.io that
/// the bundle's SBOM names, with its SHA-256, which is the "reasonable
/// means" of obtaining it that it asks for; for certifi, in pip in
/// Python's bundle, it is pip's wheel in CPython's repository at the
/// commit the SBOM names (ADR 0495). PCRE2's exception, which
/// nginx's bundle carries, lifts the BSD licence's condition on binaries
/// from the programs that only include a library that includes PCRE2.
/// The Boost Software License 1.0, fpconv's in Valkey's bundle, asks for
/// nothing in a binary. PHP's bundle (ADR 0499) is under the PHP License
/// 3.01 and the Zend Engine License 2.00, which are BSD-style licenses
/// that ask for their notices and for the names "PHP" and "Zend" not to be
/// used for products derived from it. It builds in Tcl's license for
/// scanf (`TCL`), the Apache Group's first, for its snprintf
/// (`Apache-1.0`), the OpenLDAP Public License 2.8 for tables derived from
/// ucgendat.c, and Solar Designer's crypt_blowfish, in the public domain
/// with a permissive fallback. re2c, which writes its scanners, is in the
/// public domain. PostgreSQL's bundle (ADR 0521) is under the PostgreSQL
/// License, a permissive license of the BSD kind, with Henry Spencer's
/// regular expression package under his own (`Spencer-99`), Philip
/// Warner's notice on the archiver of pg_dump and Darrell Whitley's on the
/// recombination operators of the genetic query optimizer, which SPDX has
/// no identifiers for, and the double metaphone code of fuzzystrmatch
/// under the terms of Perl itself: the Artistic License of Perl 5 or the
/// GPL, of which the allow-list has the first. lazygit's bundle (ADR 0600)
/// links github.com/integrii/flaggy, which is under the Unlicense alone: it
/// dedicates the code to the public domain, as CC0-1.0 does, which the
/// ports' list allows.
const BUNDLE_EXTRA_ALLOW: &[&str] = &[
    "MPL-2.0",
    "BSD-3-Clause WITH PCRE2-exception",
    "BSL-1.0",
    "PHP-3.01",
    "Zend-2.0",
    "TCL",
    "Apache-1.0",
    "OLDAP-2.8",
    "LicenseRef-crypt-blowfish-public-domain",
    "LicenseRef-re2c-public-domain",
    "PostgreSQL",
    "Spencer-99",
    "Artistic-1.0-Perl",
    "LicenseRef-pg-dump-Warner",
    "LicenseRef-geqo-Whitley",
    "Unlicense",
];

/// What a bundle says of its licenses (ADR 0370): every component of the
/// port the bundle is built from, checked against the port allow-list
/// and [`BUNDLE_EXTRA_ALLOW`], written into the bundle's `folder` as
/// `sbom.spdx`, which lists each, and `licenses/`, with the texts of
/// every license named and each component's notices. `created` is the
/// document's time, in seconds since 1970.
pub fn bundle(ctx: &Ctx, ports: &Ports, port: &Port, folder: &Path, created: i64) -> Result<()> {
    let recipe = &port.recipe;
    let spec = recipe.bundle.as_ref().expect("a bundle's port");
    let mut packages = Vec::new();
    port_packages(ctx, ports, port, &mut packages)?;
    // What its programs carry, linked in: a C program carries the C
    // library's code and the compiler-rt builtins', and those of the
    // libraries it links (ADR 0491).
    for name in &spec.links {
        let linked = ports.ports.iter().find(|p| p.recipe.name == *name).ok_or_else(|| {
            Error::new(format!("bundle {}: it links port {name}, which there is none of", spec.id))
        })?;
        port_packages(ctx, ports, linked, &mut packages)?;
    }
    if !spec.links.is_empty() {
        packages.push(compiler_rt(ports));
    }
    let mut problems = Vec::new();
    let allowed: Vec<String> = kernel_allow_list(ctx)?
        .into_iter()
        .chain(PORT_EXTRA_ALLOW.iter().chain(BUNDLE_EXTRA_ALLOW).map(ToString::to_string))
        .collect();
    for package in &packages {
        let what = format!("{} {}", package.name, package.version);
        match Expr::parse(&package.license) {
            _ if package.license.is_empty() => {
                problems.push(format!("{what} declares no SPDX license"));
            }
            Err(err) => problems.push(format!("{what}: license `{}`: {err}", package.license)),
            Ok(expr) if !expr.satisfied_by(&allowed) => problems.push(format!(
                "{what}: license `{}` is outside the bundle allow-list (the port's and \
                 BUNDLE_EXTRA_ALLOW in tools/xtask/src/licenses.rs)",
                package.license
            )),
            Ok(_) => {}
        }
    }
    if !problems.is_empty() {
        return Err(Error::new(format!(
            "bundle {}: license check failed:\n  {}",
            spec.id,
            problems.join("\n  ")
        ))
        .hint("see docs/adr/0370-ports-the-catalog-carries-are-packed-as-bundles.md"));
    }
    let ids = license_ids(&packages)?;
    let dir = folder.join("licenses");
    process::create_dir(&dir.join("texts"))?;
    for id in &ids {
        process::copy(&text_source(&ctx.root, id), &dir.join(format!("texts/{id}.txt")))?;
    }
    for package in &packages {
        for (notice, name) in package.notices.iter().zip(notice_names(&package.notices)) {
            process::copy(notice, &dir.join(package.notice_dir()).join(name))?;
        }
    }
    let commit = git_commit(ctx);
    let name = format!("{}-{}", spec.id, recipe.version);
    let document = Document {
        namespace: format!(
            "https://dukovo.org/spdx/bundles/{name}-{}",
            &commit[..commit.len().min(12)]
        ),
        name,
        created,
        top: Top {
            id: "bundle".into(),
            name: spec.id.clone(),
            version: recipe.version.clone(),
            download: format!(
                "git+https://github.com/dukovo-os/dukovo@{commit}#ports/{}",
                recipe.name
            ),
            license: recipe.license.clone(),
            comment: format!(
                "The Dukovo catalog's bundle of {} {}, built by ports/{} of Dukovo. Each \
                 component below keeps its own license; the texts are in licenses/texts and \
                 the notices in licenses/notices.",
                spec.name, recipe.version, recipe.name
            ),
        },
        notices: "licenses".into(),
    };
    write(&folder.join("sbom.spdx"), &document.render(&packages, &ids, &ctx.root)?)?;
    write(&dir.join("README"), &bundle_readme(&spec.name, &recipe.version, &packages))?;
    Ok(())
}

/// A one-line-per-component summary of a bundle's licenses.
fn bundle_readme(name: &str, version: &str, packages: &[Package]) -> String {
    let mut out = format!(
        "{name} {version}: its components and their licenses\n\n\
         ../sbom.spdx lists every component of this bundle in the SPDX 2.3\n\
         tag-value format. texts/ holds the full text of every license named\n\
         there, and notices/ the copyright and license files that came with\n\
         each component's source.\n\n"
    );
    let width = packages.iter().map(|p| p.name.len() + p.version.len() + 1).max().unwrap_or(0);
    for package in packages {
        let what = format!("{} {}", package.name, package.version);
        let _ = writeln!(out, "{what:width$}  {}", package.license);
    }
    out
}

fn write(path: &Path, text: &str) -> Result<()> {
    std::fs::write(path, text).context(|| format!("writing {}", path.display()))
}

/// A Rust program in the image: its workspace, package and target.
struct Program {
    what: &'static str,
    workspace: &'static str,
    package: &'static str,
    target: &'static str,
}

const KERNEL_PROGRAM: Program = Program {
    what: "the kernel",
    workspace: "kernel",
    package: "radix",
    target: kernel::TARGET_SPEC,
};

/// The user programs are /system code too: permissive licenses only.
const USER_PROGRAMS: [Program; 5] = [
    Program {
        what: "the user test programs",
        workspace: "userspace",
        package: "usertests",
        target: userspace::TARGET_SPEC,
    },
    Program {
        what: "Spark",
        workspace: "userspace",
        package: "spark",
        target: userspace::TARGET_SPEC,
    },
    Program {
        what: "the hello test program",
        workspace: "userspace",
        package: "hello",
        target: userspace::TARGET_SPEC,
    },
    Program {
        what: "the sockets test program",
        workspace: "userspace",
        package: "sockets",
        target: userspace::TARGET_SPEC,
    },
    Program {
        what: "the storage test program",
        workspace: "userspace",
        package: "storage",
        target: userspace::TARGET_SPEC,
    },
];

/// The crates linked into a program, from `cargo tree`. Build scripts and
/// dev-dependencies never reach the image, so only normal edges count.
fn rust_crates(ctx: &Ctx, program: &Program) -> Result<Vec<Package>> {
    if program.workspace == "kernel" {
        patched::ensure(ctx)?;
    }
    let mut cmd = process::cargo(ctx);
    let manifest = format!("{}/Cargo.toml", program.workspace);
    cmd.args(["tree", "--manifest-path", &manifest, "-p", program.package, "--target"])
        .arg(ctx.root.join(program.target))
        .args(["-Zjson-target-spec"]);
    let lock = read_lock(&ctx.root.join(program.workspace).join("Cargo.lock"))?;
    tree_packages(ctx, program.what, &mut cmd, &lock, None)
}

/// The crates `cmd`, a `cargo tree` of `what`, lists, with their
/// checksums from `lock`; those the ports patch for Radix in the
/// toolchain, which `ports` has, as crates.io's that the ports patch.
fn tree_packages(
    ctx: &Ctx,
    what: &str,
    cmd: &mut Command,
    lock: &Lock,
    ports: Option<&Ports>,
) -> Result<Vec<Package>> {
    cmd.args(["-e", "normal", "--prefix", "none", "--format", "{p}|{l}"]);
    let output = process::output(ctx, cmd)?;
    let text = String::from_utf8_lossy(&output.stdout);
    let packages = listed_packages(ctx, what, &text, lock, ports, None)?;
    if packages.is_empty() {
        crate::bail!("`cargo tree` listed no crates for {what}");
    }
    Ok(packages)
}

/// The crates of `text`, a `cargo tree --format '{p}|{l}'` listing of
/// `what`, with their checksums from `lock`. A crate in the toolchain is
/// one a port patches for Radix, which `ports` has; one in `own`, the
/// source of the port that listed them, is the port's own, and left out.
fn listed_packages(
    ctx: &Ctx,
    what: &str,
    text: &str,
    lock: &Lock,
    ports: Option<&Ports>,
    own: Option<&Path>,
) -> Result<Vec<Package>> {
    let commit = git_commit(ctx);
    let kernel_patched = ctx.root.join(patched::DIR);
    let mut seen = BTreeSet::new();
    let mut packages = Vec::new();
    for line in text.lines() {
        let Some(entry) = parse_tree_line(line) else { continue };
        if !seen.insert((entry.name.clone(), entry.version.clone())) {
            continue;
        }
        let mut package = Package {
            license: entry.license,
            area: Area::Kernel,
            download: "NOASSERTION".into(),
            sha256: None,
            notices: Vec::new(),
            comment: String::new(),
            name: entry.name,
            version: entry.version,
        };
        let source = match (entry.source, ports) {
            (Source::Path(path), Some(ports)) => {
                Source::Path(through_link(&path, &ports.toolchain.rust()).unwrap_or(path))
            }
            (source, _) => source,
        };
        match source {
            Source::Path(path) if own.is_some_and(|dir| path.starts_with(dir)) => continue,
            Source::Path(path)
                if ports.is_some_and(|ports| path.starts_with(&ports.toolchain.dir)) =>
            {
                let ports = ports.expect("checked above");
                let Some(patched) = patched_crate(ports, &package.name, &package.version) else {
                    crate::bail!(
                        "{what} uses {} {} from {}, which no port's recipe lists",
                        package.name,
                        package.version,
                        path.display()
                    );
                };
                packages.push(patched);
                continue;
            }
            Source::Path(path) if path.starts_with(&kernel_patched) => {
                let crates = patched::crates(ctx)?;
                let Some(krate) = crates
                    .iter()
                    .find(|krate| krate.name == package.name && krate.version == package.version)
                else {
                    crate::bail!(
                        "{what} uses {} {} from {}, which {} does not list",
                        package.name,
                        package.version,
                        path.display(),
                        patched::LIST
                    );
                };
                if krate.license != package.license {
                    crate::bail!(
                        "{} gives {} {} the license {}, but its Cargo.toml says {}",
                        patched::LIST,
                        package.name,
                        package.version,
                        krate.license,
                        package.license
                    );
                }
                package.sha256 = Some(krate.sha256.clone());
                package.download = crate_download(&package.name, &package.version);
                package.comment =
                    "A crate from crates.io, patched by kernel/patches of Dukovo.".into();
                package.notices = notice_files(&path);
            }
            Source::Path(path) => {
                package.download = format!("git+https://github.com/dukovo-os/dukovo@{commit}");
                package.comment = "Part of Dukovo.".into();
                package.notices = declared_notices(&path)?;
                package.notices.extend(notice_files(&path));
            }
            Source::Registry => {
                let key = (package.name.clone(), package.version.clone());
                package.sha256 = lock.get(&key).cloned();
                package.download = crate_download(&package.name, &package.version);
                package.comment = "A crate from crates.io.".into();
                let dir = registry_dir(&package.name, &package.version);
                if let Some(dir) = &dir {
                    package.notices = notice_files(dir);
                }
                if let Some((.., kept)) = KEPT_NOTICES
                    .iter()
                    .find(|(name, version, _)| *name == package.name && *version == package.version)
                {
                    package.notices.push(ctx.root.join(kept));
                }
                if let Some((_, license, files)) =
                    BUNDLED_C.iter().find(|(name, ..)| *name == package.name)
                {
                    package.license = format!("({}) AND {license}", package.license);
                    for file in *files {
                        let Some(notice) = dir.as_ref().map(|dir| dir.join(file)) else { break };
                        if !notice.is_file() {
                            crate::bail!(
                                "{} {} has no {file}, the notice of the C code it builds",
                                package.name,
                                package.version
                            );
                        }
                        package.notices.push(notice);
                    }
                }
            }
            Source::Other(source) => {
                crate::bail!(
                    "{what} uses {} {} from {source}; only crates.io and the \
                     repository are allowed (deny.toml [sources])",
                    package.name,
                    package.version
                );
            }
        }
        packages.push(package);
    }
    Ok(packages)
}

/// Where crates.io serves a crate.
fn crate_download(name: &str, version: &str) -> String {
    format!("{CRATES_DOWNLOAD}{name}/{version}/download")
}

/// A crate from crates.io that a port's recipe lists, and so patches.
fn patched_crate(ports: &Ports, name: &str, version: &str) -> Option<Package> {
    ports.ports.iter().find_map(|port| {
        let (krate, dir) = port
            .recipe
            .crates
            .iter()
            .zip(&port.crates)
            .find(|(krate, _)| krate.name == name && krate.version == version)?;
        Some(Package {
            name: krate.name.clone(),
            version: krate.version.clone(),
            license: krate.license.clone(),
            area: Area::Kernel,
            download: crate_download(name, version),
            sha256: Some(krate.sha256.clone()),
            notices: notice_files(dir),
            comment: format!(
                "A crate from crates.io, patched by ports/{} of Dukovo.",
                port.recipe.name
            ),
        })
    })
}

/// The crates of Rust's standard library that ports/rust-std patches for
/// Radix (std holds the backtrace crate's code too).
const STD_PATCHED: &[&str] = &["std", "unwind", "libc", "unwinding"];

/// What the programs on Rust's standard library are made of (ADR 0070):
/// their own crates, from `cargo tree` with the libc crate the rust-std
/// port patched, and the standard library's, which the port listed, with
/// their notices, when it built.
fn std_packages(ctx: &Ctx, ports: &Ports) -> Result<Vec<Package>> {
    let toolchain = &ports.toolchain;
    let lock = read_lock(&ctx.root.join("userspace/programs/Cargo.lock"))?;
    let mut packages: Vec<Package> = Vec::new();
    for (_, program) in userspace::STD_ARCHIVE {
        let mut cmd = userspace::std_cargo(ctx, "tree", toolchain);
        cmd.args(["-p", program]);
        let what = format!("the program {program}");
        for package in tree_packages(ctx, &what, &mut cmd, &lock, Some(ports))? {
            if !packages.iter().any(|p| p.name == package.name && p.version == package.version) {
                packages.push(package);
            }
        }
    }
    let Some(library) = ports
        .ports
        .iter()
        .find_map(|port| port.source.as_ref().filter(|_| port.recipe.source.rust_src))
    else {
        crate::bail!("no port builds Rust's standard library for the programs on it");
    };
    let rustc = rustc_info(ctx)?;
    let sysroot = toolchain.dir.join("rust");
    let list = sysroot.join("crates.txt");
    let text = std::fs::read_to_string(&list).context(|| format!("reading {}", list.display()))?;
    // rust-src's own lock file, which has the vendored crates' checksums.
    let library_lock = read_lock(&library.join("Cargo.lock"))?;
    for line in text.lines() {
        let Some(entry) = parse_tree_line(line) else { continue };
        let mut package = Package {
            license: entry.license,
            area: Area::Kernel,
            download: String::new(),
            sha256: None,
            notices: Vec::new(),
            comment: String::new(),
            name: entry.name,
            version: entry.version,
        };
        match entry.source {
            Source::Path(_) => {
                if package.version == "0.0.0" {
                    package.version.clone_from(&rustc.release);
                }
                package.download =
                    format!("git+https://github.com/rust-lang/rust@{}", rustc.commit);
                package.comment = format!(
                    "Rust standard library crate, from the rust-src of rustc {}",
                    rustc.release
                );
            }
            // A crate the port puts in place of the one rust-src vendors.
            Source::Registry
                if let Some(patched) = patched_crate(ports, &package.name, &package.version) =>
            {
                if !packages.iter().any(|p| p.name == patched.name && p.version == patched.version)
                {
                    packages.push(patched);
                }
                continue;
            }
            Source::Registry => {
                let key = (package.name.clone(), package.version.clone());
                package.sha256 = library_lock.get(&key).cloned();
                package.download = crate_download(&package.name, &package.version);
                package.comment = format!(
                    "A crate from crates.io that the rust-src of rustc {} vendors for the \
                     standard library",
                    rustc.release
                );
                let dir =
                    sysroot.join("notices").join(format!("{}-{}", package.name, package.version));
                package.notices = notice_files(&dir);
            }
            Source::Other(source) => {
                crate::bail!("{list:?} lists {} from {source}", package.name);
            }
        }
        if STD_PATCHED.contains(&package.name.as_str()) {
            package.comment.push_str(", patched for Radix by ports/rust-std of Dukovo");
        }
        package.comment.push('.');
        if !packages.iter().any(|p| p.name == package.name && p.version == package.version) {
            packages.push(package);
        }
    }
    Ok(packages)
}

/// Where `path`, a crate Cargo for Radix gave through the workspace's
/// link to the toolchain's Rust folder `rust` (`.radix-rust`, ob(1)), is
/// in that folder; `None` for a path not through the link.
fn through_link(path: &Path, rust: &Path) -> Option<PathBuf> {
    let mut parts = path.components();
    while parts.next()?.as_os_str() != Toolchain::RUST_LINK {}
    Some(rust.join(parts.as_path()))
}

/// Where a crate came from, as `cargo tree` prints it.
#[derive(Debug, PartialEq, Eq)]
enum Source {
    Path(PathBuf),
    Registry,
    Other(String),
}

#[derive(Debug, PartialEq, Eq)]
struct TreeEntry {
    name: String,
    version: String,
    source: Source,
    license: String,
}

/// Parses `name vX.Y.Z (source)|license`, with an optional ` (*)` that
/// marks a crate printed before.
fn parse_tree_line(line: &str) -> Option<TreeEntry> {
    let (package, license) = line.split_once('|')?;
    // Some crates still write `MIT/Apache-2.0`, Cargo's old form.
    let license = license.trim().trim_end_matches("(*)").trim().replace('/', " OR ");
    // A procedural macro runs only while building, but what it writes is
    // in the program: listed as any other crate.
    let package = package.replacen(" (proc-macro)", "", 1);
    let mut words = package.trim().splitn(3, ' ');
    let name = words.next()?.to_string();
    let version = words.next()?.strip_prefix('v')?.to_string();
    let source = match words.next().map(|s| s.trim().trim_start_matches('(').trim_end_matches(')'))
    {
        None | Some("") => Source::Registry,
        Some(s) if s.starts_with('/') => Source::Path(PathBuf::from(s)),
        Some(s) => Source::Other(s.to_string()),
    };
    Some(TreeEntry { name, version, source, license })
}

/// `[package.metadata.dukovo] notices`: notice files a crate carries
/// outside its top directory, such as the font license of radix-fbcon.
fn declared_notices(dir: &Path) -> Result<Vec<PathBuf>> {
    let path = dir.join("Cargo.toml");
    let text = std::fs::read_to_string(&path).context(|| format!("reading {}", path.display()))?;
    let manifest: toml::Value =
        toml::from_str(&text).map_err(|err| Error::new(format!("{}: {err}", path.display())))?;
    let Some(list) = manifest
        .get("package")
        .and_then(|p| p.get("metadata"))
        .and_then(|m| m.get("dukovo"))
        .and_then(|o| o.get("notices"))
    else {
        return Ok(Vec::new());
    };
    let mut notices = Vec::new();
    for item in list.as_array().into_iter().flatten() {
        let Some(name) = item.as_str() else {
            crate::bail!("{}: package.metadata.dukovo.notices must hold paths", path.display());
        };
        let notice = dir.join(name);
        if !notice.is_file() {
            crate::bail!("{} names notice {name}, which does not exist", path.display());
        }
        notices.push(notice);
    }
    Ok(notices)
}

/// The license and notice files at the top of a source tree, sorted.
fn notice_files(dir: &Path) -> Vec<PathBuf> {
    top_files(dir, NOTICE_PREFIXES)
}

/// The files at the top of a source tree whose names, in upper case, begin
/// with one of `prefixes`, sorted.
fn top_files(dir: &Path, prefixes: &[&str]) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_file() && begins_with_one_of(path, prefixes))
        .collect();
    files.sort();
    files
}

/// The name each of a component's notice files has among its notices: the
/// file's own, and for a file whose name an earlier one has (PHP keeps a
/// LICENSE in nearly every folder of a bundled library), the name of the
/// folder it is in before it, as `Zend-LICENSE`, so that none is lost.
fn notice_names(notices: &[PathBuf]) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for notice in notices {
        let base = notice.file_name().and_then(|n| n.to_str()).expect("a notice is a file");
        let folder = notice.parent().and_then(|p| p.file_name()).and_then(|n| n.to_str());
        let mut name = base.to_string();
        if names.contains(&name) {
            name = format!("{}-{base}", folder.unwrap_or("notice"));
        }
        // Two folders of one name: a number tells them apart.
        let mut n = 2;
        while names.contains(&name) {
            name = format!("{}-{base}-{n}", folder.unwrap_or("notice"));
            n += 1;
        }
        names.push(name);
    }
    names
}

fn begins_with_one_of(path: &Path, prefixes: &[&str]) -> bool {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default();
    let upper = name.to_ascii_uppercase();
    prefixes.iter().any(|prefix| upper.starts_with(prefix))
}

/// Whether every crates.io crate the port `name` linked, as its
/// `crates.txt` lists them, has its sources in Cargo's registry, where its
/// notices are read from; true for a port that lists none. Building the
/// port again fetches them.
pub fn crates_fetched(ctx: &Ctx, name: &str) -> bool {
    let list = crate::ports::info_dir(ctx, name).join("crates.txt");
    let Ok(text) = std::fs::read_to_string(&list) else { return true };
    text.lines().filter_map(parse_tree_line).all(|entry| {
        !matches!(entry.source, Source::Registry)
            || registry_dir(&entry.name, &entry.version).is_some()
    })
}

/// The unpacked source of a crates.io crate in Cargo's cache.
fn registry_dir(name: &str, version: &str) -> Option<PathBuf> {
    let home = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| Path::new(&home).join(".cargo")))?;
    let src = home.join("registry/src");
    std::fs::read_dir(src)
        .ok()?
        .flatten()
        .map(|index| index.path().join(format!("{name}-{version}")))
        .find(|dir| dir.is_dir())
}

/// Whether every module of Go's module proxy that the port `name` linked,
/// as its `modules.txt` lists them, is unpacked in Go's module cache, where
/// its license files are read from; true for a port that lists none.
/// Building the port again fetches them. A `modules.txt` that cannot be
/// read is reported where the licenses are read.
pub fn go_modules_fetched(ctx: &Ctx, name: &str) -> bool {
    let Ok(Some(linked)) = ports::go_modules_linked(ctx, name) else { return true };
    linked.iter().all(|module| match module {
        LinkedModule::Proxy { path, version } => {
            ports::go_cached_module(ctx, path, version).is_dir()
        }
        LinkedModule::Patched { .. } => true,
    })
}

/// The files of a Go module that state its license: those at the top of its
/// folder whose names begin with one of these. A NOTICE file lists what a
/// project keeps of others' work and grants nothing itself.
const GO_LICENSE_PREFIXES: &[&str] = &["LICENSE", "LICENCE", "COPYING", "UNLICENSE"];

/// The licenses [`go_license`] knows, in the order it lists those that a
/// module's files state together.
const GO_LICENSE_ORDER: &[&str] = &[
    "Apache-2.0",
    "MIT",
    "BSD-3-Clause",
    "BSD-2-Clause",
    "ISC",
    "0BSD",
    "MPL-2.0",
    "Unlicense",
    "CC0-1.0",
];

/// The words that make a text, with its whitespace collapsed, a license:
/// all of its phrases, in the case the license's text has them. 0BSD is
/// ISC's grant without ISC's condition, so ISC is told by that condition
/// and 0BSD by the sentence ending where ISC's goes on. The BSD licenses
/// are told apart by [`bsd_license`].
const GO_PHRASES: &[(&str, &[&str])] = &[
    ("Apache-2.0", &["Apache License", "Version 2.0"]),
    (
        "MIT",
        &[
            "Permission is hereby granted, free of charge",
            "The above copyright notice and this permission notice shall be included",
        ],
    ),
    (
        "ISC",
        &[
            "Permission to use, copy, modify, and/or distribute",
            "provided that the above copyright notice and this permission notice appear in all copies",
        ],
    ),
    (
        "0BSD",
        &[
            "Permission to use, copy, modify, and/or distribute",
            "with or without fee is hereby granted.",
        ],
    ),
    ("MPL-2.0", &["Mozilla Public License", "2.0"]),
    ("Unlicense", &["This is free and unencumbered software released into the public domain"]),
    ("CC0-1.0", &["CC0 1.0 Universal"]),
];

/// Licenses that the files of a module linked into a program in Go must not
/// name: the copyleft ones put terms on the whole program, which only a
/// person can weigh, and the source-available ones limit what it may be
/// used for. A module under one has its license given under `[go-licenses]`
/// by a person who read its terms. Each is as it is said, with the words
/// that name it, in lower case.
const GO_REFUSED: &[(&str, &[&str])] = &[
    (
        "the GNU GPL, LGPL or AGPL",
        &[
            "gnu general public license",
            "gnu lesser general public license",
            "gnu library general public license",
            "gnu affero general public license",
            "gpl",
        ],
    ),
    ("the Eclipse Public License", &["eclipse public license"]),
    (
        "the Common Development and Distribution License",
        &["common development and distribution license", "cddl"],
    ),
    ("the Server Side Public License", &["server side public license", "sspl"]),
    ("the Business Source License", &["business source license", "busl"]),
    ("the Commons Clause", &["commons clause"]),
    ("a Creative Commons ShareAlike license", &["attribution-sharealike", "cc by-sa", "cc-by-sa"]),
];

/// The sentence of the MPL-2.0 that names the GPL, LGPL and AGPL as the
/// "Secondary Licenses" its code may also be distributed under: the text
/// of the MPL-2.0 holds it, and the license is none of those.
const MPL_SECONDARY_LICENSES: &str = "means either the GNU General Public License, Version 2.0, \
    the GNU Lesser General Public License, Version 2.1, the GNU Affero General Public License, \
    Version 3.0, or any later versions of those licenses.";

/// The SPDX expression that the license files of the Go module unpacked in
/// `dir` state, read by their words (see [`GO_PHRASES`]): a license for
/// each that a file holds, joined with AND in the order of
/// [`GO_LICENSE_ORDER`], since a file may carry a second license for code
/// it copied.
///
/// # Errors
///
/// The reason, as a sentence about the module, why no license is read: it
/// has no license file, none of its files states a license that is known,
/// or one names a license a bundle does not take, or the advertising
/// clause of the original BSD license.
pub fn go_license(dir: &Path) -> Result<String, String> {
    let files = top_files(dir, GO_LICENSE_PREFIXES);
    if files.is_empty() {
        return Err("it has no LICENSE, LICENCE, COPYING or UNLICENSE file".into());
    }
    let mut found = BTreeSet::new();
    let mut names = Vec::new();
    for file in &files {
        let name = file.file_name().expect("a license file has a name").to_string_lossy();
        let bytes = std::fs::read(file).map_err(|err| format!("{name} cannot be read: {err}"))?;
        let text = String::from_utf8_lossy(&bytes).split_whitespace().collect::<Vec<_>>().join(" ");
        found.extend(licenses_stated(&name, &text)?);
        names.push(name.into_owned());
    }
    let ids: Vec<&str> = GO_LICENSE_ORDER.iter().copied().filter(|id| found.contains(id)).collect();
    if ids.is_empty() {
        return Err(format!(
            "its license files ({}) hold none of {} in words that are known",
            names.join(", "),
            GO_LICENSE_ORDER.join(", ")
        ));
    }
    Ok(ids.join(" AND "))
}

/// The licenses that the text of the license file `file` states, its
/// whitespace collapsed; or why it is refused.
fn licenses_stated(file: &str, text: &str) -> Result<Vec<&'static str>, String> {
    let lower = text.replace(MPL_SECONDARY_LICENSES, "").to_lowercase();
    for (refused, words) in GO_REFUSED {
        if words.iter().any(|word| lower.contains(word)) {
            return Err(format!("{file} names {refused}"));
        }
    }
    let mut found: Vec<&str> = GO_PHRASES
        .iter()
        .filter(|(_, phrases)| phrases.iter().all(|phrase| text.contains(phrase)))
        .map(|(id, _)| *id)
        .collect();
    if let Some(bsd) = bsd_license(file, text)? {
        found.push(bsd);
    }
    Ok(found)
}

/// The BSD license that `text` states: with the clause that bars using the
/// names of the authors to endorse what is made of the code, BSD-3-Clause,
/// and without it, BSD-2-Clause. The original license's fourth clause, on
/// advertising, is refused: it asks every advertisement of a product to
/// credit the authors.
fn bsd_license(file: &str, text: &str) -> Result<Option<&'static str>, String> {
    if !text.contains("Redistribution and use in source and binary forms") {
        return Ok(None);
    }
    if text.contains("advertising materials") {
        return Err(format!("{file} has the BSD license's clause on advertising"));
    }
    if text.contains("endorse or promote products derived from") {
        Ok(Some("BSD-3-Clause"))
    } else {
        Ok(Some("BSD-2-Clause"))
    }
}

/// A Go module's notices: its license files, an UNLICENSE among them, and
/// the other notice files at the top of its folder, sorted.
fn go_notices(dir: &Path) -> Vec<PathBuf> {
    let mut files = notice_files(dir);
    files.extend(top_files(dir, &["UNLICENSE"]));
    files.sort();
    files
}

/// (name, version) to checksum, from a Cargo.lock.
type Lock = std::collections::BTreeMap<(String, String), String>;

/// The checksums of a Cargo.lock.
fn read_lock(path: &Path) -> Result<Lock> {
    let text = std::fs::read_to_string(path).context(|| format!("reading {}", path.display()))?;
    let lock: toml::Value =
        toml::from_str(&text).map_err(|err| Error::new(format!("{}: {err}", path.display())))?;
    let mut sums = std::collections::BTreeMap::new();
    for package in lock.get("package").and_then(|p| p.as_array()).into_iter().flatten() {
        let field = |key: &str| package.get(key).and_then(|v| v.as_str()).map(str::to_string);
        if let (Some(name), Some(version), Some(sum)) =
            (field("name"), field("version"), field("checksum"))
        {
            sums.insert((name, version), sum);
        }
    }
    Ok(sums)
}

/// The standard library crates that `-Zbuild-std` compiles into the kernel.
fn sysroot_crates(ctx: &Ctx) -> Result<Vec<Package>> {
    let rustc = rustc_info(ctx)?;
    let library = rustc.sysroot.join("lib/rustlib/src/rust/library");
    let mut packages = Vec::new();
    for name in build_std_crates(&kernel::target_args(ctx)) {
        let Some((_, path)) = SYSROOT_CRATES.iter().find(|(crate_name, _)| *crate_name == name)
        else {
            crate::bail!(
                "the kernel builds `{name}` from rust-src, but tools/xtask/src/licenses.rs \
                 does not know where it lives (SYSROOT_CRATES)"
            );
        };
        let dir = library.join(path);
        let manifest_path = dir.join("Cargo.toml");
        let text = std::fs::read_to_string(&manifest_path)
            .context(|| format!("reading {}", manifest_path.display()))?;
        let manifest: toml::Value = toml::from_str(&text)
            .map_err(|err| Error::new(format!("{}: {err}", manifest_path.display())))?;
        let field =
            |key: &str| manifest.get("package").and_then(|p| p.get(key)).and_then(|v| v.as_str());
        let version = match field("version") {
            Some(v) if v != "0.0.0" => v.to_string(),
            _ => rustc.release.clone(),
        };
        // Notices sit in the crate or in its repository's top directory
        // (compiler-builtins/LICENSE.txt), never above `library`.
        let mut notices = Vec::new();
        let mut current = dir.as_path();
        while notices.is_empty() && current.starts_with(&library) && current != library {
            notices = notice_files(current);
            current = current.parent().expect("inside the library directory");
        }
        packages.push(Package {
            name: name.to_string(),
            version,
            license: field("license").unwrap_or_default().to_string(),
            area: Area::Kernel,
            download: format!("git+https://github.com/rust-lang/rust@{}", rustc.commit),
            sha256: None,
            notices,
            comment: format!(
                "Rust standard library crate compiled into the kernel with -Zbuild-std, \
                 from rustc {}.",
                rustc.release
            ),
        });
    }
    Ok(packages)
}

/// The crates named by `-Zbuild-std=...` in `args`.
fn build_std_crates(args: &[String]) -> Vec<&str> {
    args.iter()
        .filter_map(|arg| arg.strip_prefix("-Zbuild-std="))
        .flat_map(|list| list.split(','))
        .filter(|name| !name.is_empty())
        .collect()
}

struct Rustc {
    sysroot: PathBuf,
    release: String,
    commit: String,
}

fn rustc_info(ctx: &Ctx) -> Result<Rustc> {
    let run = |args: &[&str]| -> Result<String> {
        let mut cmd = Command::new("rustc");
        cmd.args(args).current_dir(&ctx.root);
        let output = process::output(ctx, &mut cmd)?;
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    };
    let sysroot = PathBuf::from(run(&["--print", "sysroot"])?.trim());
    let verbose = run(&["-vV"])?;
    let field = |key: &str| {
        verbose
            .lines()
            .find_map(|line| line.strip_prefix(key))
            .map_or_else(|| "unknown".into(), |v| v.trim().to_string())
    };
    Ok(Rustc { sysroot, release: field("release:"), commit: field("commit-hash:") })
}

/// What the C programs of the boot archive are made of: Dukovo's own
/// test programs, each port that is not part of the toolchain and the
/// sources unpacked in it, and the compiler-rt builtins every C program
/// links (ADR 0069).
fn c_packages(ctx: &Ctx, ports: &Ports) -> Result<Vec<Package>> {
    let commit = git_commit(ctx);
    let mut packages = vec![Package {
        name: "dukovo-posix-tests".into(),
        version: VERSION.into(),
        license: "Apache-2.0 OR MIT".into(),
        area: Area::Kernel,
        download: format!("git+https://github.com/dukovo-os/dukovo@{commit}"),
        sha256: None,
        notices: Vec::new(),
        comment: format!("Part of Dukovo: the C test programs of {}.", userspace::POSIX_SOURCES),
    }];
    // A bundle's port is the catalog's, and not in the image (ADR 0370).
    for port in ports.ports.iter().filter(|p| !p.recipe.toolchain && p.recipe.bundle.is_none()) {
        port_packages(ctx, ports, port, &mut packages)?;
    }
    packages.push(compiler_rt(ports));
    Ok(packages)
}

/// The compiler-rt builtins of the toolchain's LLVM, which every C
/// program links.
fn compiler_rt(ports: &Ports) -> Package {
    let llvm = ports.toolchain.llvm_version();
    Package {
        name: "compiler-rt-builtins".into(),
        version: llvm.clone(),
        license: "Apache-2.0 WITH LLVM-exception".into(),
        area: Area::Kernel,
        download: "https://github.com/llvm/llvm-project".into(),
        sha256: None,
        notices: Vec::new(),
        comment: format!(
            "The compiler-rt builtins of LLVM {llvm}, from the build host, which every C \
             program links (https://compiler-rt.llvm.org)."
        ),
    }
}

/// What `port` builds into what it installs: its source, extra sources,
/// crates and bundled code, and for a port in Rust the crates its build
/// linked; each added to `packages` unless it is there already.
fn port_packages(ctx: &Ctx, ports: &Ports, port: &Port, packages: &mut Vec<Package>) -> Result<()> {
    let recipe = &port.recipe;
    if let Some(source) = &port.source {
        packages.push(Package {
            name: recipe.name.clone(),
            version: recipe.version.clone(),
            license: recipe.license.clone(),
            area: Area::Port,
            download: recipe.source.download(),
            sha256: (!recipe.source.sha256.is_empty()).then(|| recipe.source.sha256.clone()),
            notices: if recipe.notices.is_empty() {
                notice_files(source)
            } else {
                recipe.notices.iter().map(|file| source.join(file)).collect()
            },
            comment: format!(
                "{}, from {} ({}); built by ports/{} of Dukovo.",
                recipe.summary,
                recipe.homepage,
                recipe.source.release(),
                recipe.name
            ),
        });
    }
    for (extra, source) in recipe.extra.iter().zip(&port.extras) {
        let name = extra.path.rsplit('/').next().unwrap_or(&extra.path);
        packages.push(Package {
            name: name.to_string(),
            version: extra.commit[..12].to_string(),
            license: extra.license.clone(),
            area: Area::Port,
            download: format!("git+{}@{}", extra.git, extra.commit),
            sha256: None,
            notices: notice_files(source),
            comment: format!(
                "Built into {} {} from its {}.",
                recipe.name, recipe.version, extra.path
            ),
        });
    }
    for (krate, source) in recipe.crates.iter().zip(&port.crates) {
        packages.push(Package {
            name: krate.name.clone(),
            version: krate.version.clone(),
            license: krate.license.clone(),
            area: Area::Port,
            download: crate_download(&krate.name, &krate.version),
            sha256: Some(krate.sha256.clone()),
            notices: notice_files(source),
            comment: format!(
                "A crate from crates.io, built into {} {} by ports/{} of Dukovo.",
                recipe.name, recipe.version, recipe.name
            ),
        });
    }
    for bundled in &recipe.bundled {
        packages.push(Package {
            name: bundled.name.clone(),
            version: bundled.version.clone(),
            license: bundled.license.clone(),
            area: Area::Port,
            download: bundled.homepage.clone(),
            sha256: None,
            notices: bundled.notices.iter().map(|file| port.dir.join(file)).collect(),
            comment: format!(
                "Built into {} {} from ports/{} of Dukovo, which keeps its code.",
                recipe.name, recipe.version, recipe.name
            ),
        });
    }
    // A port in Rust: the crates its build linked.
    let info = crate::ports::info_dir(ctx, &recipe.name);
    let list = info.join("crates.txt");
    if list.is_file() {
        let text =
            std::fs::read_to_string(&list).context(|| format!("reading {}", list.display()))?;
        let lock = read_lock(&info.join("Cargo.lock"))?;
        let what = format!("port {}", recipe.name);
        let own = ctx.target.join("ports/work").join(&recipe.name);
        for mut package in listed_packages(ctx, &what, &text, &lock, Some(ports), Some(&own))? {
            if packages.iter().any(|p| p.name == package.name && p.version == package.version) {
                continue;
            }
            package.area = Area::Port;
            package.comment = format!(
                "{} Built into {} {} by ports/{} of Dukovo.",
                package.comment, recipe.name, recipe.version, recipe.name
            );
            packages.push(package);
        }
    }
    go_packages(ctx, ports, port, packages)
}

/// A port in Go, which lists the modules its programs link in
/// `modules.txt`: each of them, and Go's standard library and runtime,
/// that every program carries; each added to `packages` unless it is there
/// already.
fn go_packages(ctx: &Ctx, ports: &Ports, port: &Port, packages: &mut Vec<Package>) -> Result<()> {
    let linked = ports::go_modules_linked(ctx, &port.recipe.name)?;
    go_module_packages(ctx, ports, port, linked.as_deref().unwrap_or_default(), packages)?;
    if linked.is_none() {
        return Ok(());
    }
    let name = ports::GO_PORT;
    let Some(go) = ports.ports.iter().find(|p| p.recipe.name == name) else {
        crate::bail!(
            "port {}: its programs are in Go, and there is no port {name} to take the standard \
             library from",
            port.recipe.name
        );
    };
    let mut standard = Vec::new();
    port_packages(ctx, ports, go, &mut standard)?;
    for package in standard {
        if !packages.iter().any(|p| p.name == package.name && p.version == package.version) {
            packages.push(package);
        }
    }
    Ok(())
}

/// The packages of the Go modules `linked`, which the programs of `port`
/// link. The licenses that cannot be known are reported together.
fn go_module_packages(
    ctx: &Ctx,
    ports: &Ports,
    port: &Port,
    linked: &[LinkedModule],
    packages: &mut Vec<Package>,
) -> Result<()> {
    let recipe = &port.recipe;
    let mut unread = Vec::new();
    let mut proxied = Vec::new();
    for module in linked {
        let package = match module {
            LinkedModule::Patched { folder, named } => {
                patched_go_module(ports, port, folder, named)?
            }
            LinkedModule::Proxy { path, version } => {
                proxied.push(path.as_str());
                let dir = ports::go_cached_module(ctx, path, version);
                if !dir.is_dir() {
                    return Err(Error::new(format!(
                        "port {}: Go module {path} {version}, which its programs link, is not in \
                         {}",
                        recipe.name,
                        ports::go_module_cache(ctx).display()
                    ))
                    .hint(format!("build the port again: ./ob ports --rebuild {}", recipe.name)));
                }
                let license = match recipe.go_licenses.get(path) {
                    Some(license) => Ok(license.clone()),
                    None => go_license(&dir),
                };
                match license {
                    Ok(license) => proxy_go_module(recipe, path, version, license, &dir),
                    Err(why) => {
                        unread.push(format!("{path} {version}: {why}"));
                        continue;
                    }
                }
            }
        };
        if !packages.iter().any(|p| p.name == package.name && p.version == package.version) {
            packages.push(package);
        }
    }
    if !unread.is_empty() {
        return Err(Error::new(format!(
            "port {}: the license of a Go module its programs link is not known:\n  {}",
            recipe.name,
            unread.join("\n  ")
        ))
        .hint(
            "a recipe gives the license of a module whose files do not state it in known words \
             under [go-licenses], with a comment saying where it is stated",
        ));
    }
    if let Some(path) = recipe.go_licenses.keys().find(|path| !proxied.contains(&path.as_str())) {
        return Err(Error::new(format!(
            "port {}: [go-licenses] gives a license for {path}, which none of its programs links \
             from Go's module proxy",
            recipe.name
        ))
        .hint(format!(
            "remove the line; a module patched by ports/{} has the license its recipe gives",
            ports::GO_MODULES_PORT
        )));
    }
    Ok(())
}

/// The package of a module the go-modules port patched for Radix, in the
/// folder `folder`, which takes the place of the versions `named` of it
/// that the programs of `port` name.
fn patched_go_module(
    ports: &Ports,
    port: &Port,
    folder: &str,
    named: &BTreeSet<String>,
) -> Result<Package> {
    let recipe = &port.recipe;
    let patching = ports::GO_MODULES_PORT;
    let Some(patched) = ports.ports.iter().find(|p| p.recipe.name == patching) else {
        crate::bail!(
            "port {}: its programs link {folder}, and there is no port {patching}",
            recipe.name
        );
    };
    let modules = patched.recipe.modules.iter().zip(&patched.modules);
    let Some((module, tree)) = modules.into_iter().find(|(module, _)| module.dir() == folder)
    else {
        return Err(Error::new(format!(
            "port {}: its programs link {folder}, which ports/{patching} does not patch",
            recipe.name
        ))
        .hint(format!("build the port again: ./ob ports --rebuild {}", recipe.name)));
    };
    let named: Vec<&str> = named.iter().map(String::as_str).collect();
    Ok(Package {
        name: module.path.clone(),
        version: module.version.clone(),
        license: module.license.clone(),
        area: Area::Port,
        download: module.url(),
        sha256: Some(module.sha256.clone()),
        notices: go_notices(tree),
        comment: format!(
            "A Go module from Go's module proxy, patched for Radix by ports/{patching}. It \
             stands in for the version of it that {name} {version} names ({}), and is built into \
             {name} {version} by ports/{name} of Dukovo.",
            named.join(", "),
            name = recipe.name,
            version = recipe.version,
        ),
    })
}

/// The package of a module of Go's module proxy that the programs of the
/// port `recipe` link, unpacked in `dir`.
fn proxy_go_module(
    recipe: &Recipe,
    path: &str,
    version: &str,
    license: String,
    dir: &Path,
) -> Package {
    Package {
        name: path.to_string(),
        version: version.to_string(),
        license,
        area: Area::Port,
        download: ports::go_proxy_url(path, version),
        sha256: None,
        notices: go_notices(dir),
        comment: format!(
            "A Go module from Go's module proxy, built into {} {} by ports/{} of Dukovo.",
            recipe.name, recipe.version, recipe.name
        ),
    }
}

/// GRUB, which `./ob` builds from the GNU project's pinned tarball for
/// every image (ADR 0306).
fn grub(grub: &crate::grub::Grub) -> Package {
    let version = crate::grub::VERSION;
    Package {
        name: "grub".into(),
        version: version.into(),
        license: "GPL-3.0-or-later".into(),
        area: Area::Bootloader,
        download: crate::grub::URL.into(),
        sha256: Some(crate::grub::SHA256.into()),
        notices: vec![grub.copying.clone()],
        comment: format!(
            "The GRUB {version} boot loader and the modules of it the image's GRUB \
             holds, built by Dukovo's `./ob` from the GNU project's tarball of the \
             download location and SHA-256 given, unchanged but for the file \
             grub-core/extra_deps.lst the release leaves out, as tools/xtask/src/grub.rs \
             in Dukovo's source says (its corresponding source). GRUB is a separate \
             program that loads the kernel; see https://www.gnu.org/software/grub/."
        ),
    }
}

/// memtest86+, which `./ob` builds from its pinned tarball for every
/// image's start menu (ADR 0488).
fn memtest(memtest: &crate::memtest::Memtest) -> Package {
    let version = crate::memtest::VERSION;
    Package {
        name: "memtest86+".into(),
        version: version.into(),
        license: "GPL-2.0-only".into(),
        area: Area::Bootloader,
        download: crate::memtest::URL.into(),
        sha256: Some(crate::memtest::SHA256.into()),
        notices: vec![memtest.license.clone()],
        comment: format!(
            "The memtest86+ {version} memory test, which the start menu's Troubleshooting \
             starts, built unchanged by Dukovo's `./ob` from the tarball of the download \
             location and SHA-256 given, as tools/xtask/src/memtest.rs in Dukovo's source says \
             (its corresponding source). memtest86+ is a separate program that runs instead of \
             the system; see https://www.memtest.org/."
        ),
    }
}

fn intel_microcode() -> Package {
    Package {
        name: "intel-microcode".into(),
        version: "NOASSERTION".into(),
        license: "LicenseRef-Intel-Microcode".into(),
        area: Area::Firmware,
        download: "https://github.com/intel/Intel-Linux-Processor-Microcode-Data-Files".into(),
        sha256: None,
        notices: Vec::new(),
        comment: "Closed Intel CPU microcode, added to this local build with \
                  image.microcode or --microcode. It may be redistributed only unmodified; \
                  release images never carry it (ADR 0007)."
            .into(),
    }
}

fn amd_microcode() -> Package {
    Package {
        name: "amd-microcode".into(),
        version: "NOASSERTION".into(),
        license: "LicenseRef-AMD-Microcode".into(),
        area: Area::Firmware,
        download: "https://git.kernel.org/pub/scm/linux/kernel/git/firmware/linux-firmware.git/tree/amd-ucode"
            .into(),
        sha256: None,
        notices: Vec::new(),
        comment: "Closed AMD CPU microcode, added to this local build with \
                  image.microcode or --microcode. It may be redistributed only in binary \
                  form, with its copyright notice and license; release images never carry \
                  it (ADR 0007)."
            .into(),
    }
}

/// `[licenses] allow` of kernel/deny.toml.
pub fn kernel_allow_list(ctx: &Ctx) -> Result<Vec<String>> {
    let path = ctx.root.join(KERNEL_POLICY);
    let text = std::fs::read_to_string(&path).context(|| format!("reading {}", path.display()))?;
    parse_allow_list(&text).map_err(|err| Error::new(format!("{}: {err}", path.display())))
}

fn parse_allow_list(text: &str) -> Result<Vec<String>, String> {
    let policy: toml::Value = toml::from_str(text).map_err(|err| err.to_string())?;
    let allow = policy
        .get("licenses")
        .and_then(|l| l.get("allow"))
        .and_then(|a| a.as_array())
        .ok_or("no [licenses] allow list")?;
    allow
        .iter()
        .map(|item| item.as_str().map(str::to_string).ok_or("allow holds a non-string".into()))
        .collect()
}

/// Fails when a component has no license or one outside its area.
fn check(packages: &[Package], kernel_allow: &[String]) -> Result<()> {
    let mut problems = Vec::new();
    for package in packages {
        let allowed: Vec<String> = match package.area {
            Area::Kernel => kernel_allow.to_vec(),
            Area::Port => kernel_allow
                .iter()
                .cloned()
                .chain(PORT_EXTRA_ALLOW.iter().map(ToString::to_string))
                .collect(),
            Area::Bootloader => BOOTLOADER_ALLOW.iter().map(ToString::to_string).collect(),
            Area::Firmware => FIRMWARE_ALLOW.iter().map(ToString::to_string).collect(),
        };
        let what = format!("{} {}", package.name, package.version);
        if package.license.is_empty() {
            problems.push(format!("{what} declares no SPDX license"));
            continue;
        }
        match Expr::parse(&package.license) {
            Err(err) => problems.push(format!("{what}: license `{}`: {err}", package.license)),
            Ok(expr) if !expr.satisfied_by(&allowed) => problems.push(format!(
                "{what}: license `{}` is outside {}",
                package.license,
                package.area.describe()
            )),
            Ok(_) => {}
        }
    }
    if problems.is_empty() {
        return Ok(());
    }
    Err(Error::new(format!("license check failed:\n  {}", problems.join("\n  ")))
        .hint("see docs/adr/0021-license-manifest-in-every-image.md for the policy"))
}

/// Every license and exception named in the manifest, each of which needs
/// its text in the image.
fn license_ids(packages: &[Package]) -> Result<BTreeSet<String>> {
    let mut ids = BTreeSet::new();
    for package in packages {
        Expr::parse(&package.license)
            .map_err(|err| Error::new(format!("{}: {err}", package.name)))?
            .collect_ids(&mut ids);
    }
    Ok(ids)
}

/// Where the text of a license or exception is kept in the repository.
/// Licenses that files in the repository use are in LICENSES/, as the REUSE
/// specification asks; every other text is in image/licenses as
/// `LICENSE-<id>.txt`. Each text is kept once, verbatim from the SPDX
/// license list (ADR 0026).
pub fn text_source(root: &Path, id: &str) -> PathBuf {
    let reuse = root.join("LICENSES").join(format!("{id}.txt"));
    if reuse.is_file() {
        reuse
    } else {
        root.join("image/licenses").join(format!("LICENSE-{id}.txt"))
    }
}

/// A parsed SPDX license expression.
#[derive(Debug, PartialEq, Eq)]
enum Expr {
    /// A license, or `<license> WITH <exception>`.
    License {
        id: String,
        exception: Option<String>,
    },
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
}

impl Expr {
    /// Parses an expression. The `/` that old crates use for OR is accepted.
    fn parse(text: &str) -> Result<Expr, String> {
        let spaced = text.replace('(', " ( ").replace(')', " ) ").replace('/', " OR ");
        let tokens: Vec<&str> = spaced.split_whitespace().collect();
        let mut pos = 0;
        let expr = Self::parse_or(&tokens, &mut pos)?;
        if pos != tokens.len() {
            return Err(format!("unexpected `{}`", tokens[pos]));
        }
        Ok(expr)
    }

    fn parse_or(tokens: &[&str], pos: &mut usize) -> Result<Expr, String> {
        let mut left = Self::parse_and(tokens, pos)?;
        while tokens.get(*pos) == Some(&"OR") {
            *pos += 1;
            left = Expr::Or(Box::new(left), Box::new(Self::parse_and(tokens, pos)?));
        }
        Ok(left)
    }

    fn parse_and(tokens: &[&str], pos: &mut usize) -> Result<Expr, String> {
        let mut left = Self::parse_atom(tokens, pos)?;
        while tokens.get(*pos) == Some(&"AND") {
            *pos += 1;
            left = Expr::And(Box::new(left), Box::new(Self::parse_atom(tokens, pos)?));
        }
        Ok(left)
    }

    fn parse_atom(tokens: &[&str], pos: &mut usize) -> Result<Expr, String> {
        let token = *tokens.get(*pos).ok_or("the expression ends early")?;
        *pos += 1;
        if token == "(" {
            let inner = Self::parse_or(tokens, pos)?;
            if tokens.get(*pos) != Some(&")") {
                return Err("a `(` is not closed".into());
            }
            *pos += 1;
            return Ok(inner);
        }
        if !is_id(token) {
            return Err(format!("`{token}` is not a license identifier"));
        }
        let mut exception = None;
        if tokens.get(*pos) == Some(&"WITH") {
            let name = *tokens.get(*pos + 1).ok_or("WITH needs an exception")?;
            if !is_id(name) {
                return Err(format!("`{name}` is not an exception identifier"));
            }
            exception = Some(name.to_string());
            *pos += 2;
        }
        Ok(Expr::License { id: token.to_string(), exception })
    }

    /// Whether some choice among the ORs uses allowed licenses only. An
    /// exception must be allowed as a whole, as in `Apache-2.0 WITH
    /// LLVM-exception`.
    fn satisfied_by(&self, allowed: &[String]) -> bool {
        match self {
            Expr::License { id, exception: None } => allowed.iter().any(|a| a == id),
            Expr::License { id, exception: Some(exception) } => {
                allowed.iter().any(|a| *a == format!("{id} WITH {exception}"))
            }
            Expr::And(a, b) => a.satisfied_by(allowed) && b.satisfied_by(allowed),
            Expr::Or(a, b) => a.satisfied_by(allowed) || b.satisfied_by(allowed),
        }
    }

    fn collect_ids(&self, ids: &mut BTreeSet<String>) {
        match self {
            Expr::License { id, exception } => {
                ids.insert(id.clone());
                ids.extend(exception.clone());
            }
            Expr::And(a, b) | Expr::Or(a, b) => {
                a.collect_ids(ids);
                b.collect_ids(ids);
            }
        }
    }
}

fn is_id(token: &str) -> bool {
    !matches!(token, "AND" | "OR" | "WITH" | "(" | ")")
        && token.chars().all(|c| c.is_ascii_alphanumeric() || ".-+:".contains(c))
}

/// SPDX identifiers allow letters, digits, `.` and `-` only.
fn spdx_id(name: &str) -> String {
    let clean: String =
        name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '.' { c } else { '-' }).collect();
    format!("SPDXRef-Package-{clean}")
}

fn git_commit(ctx: &Ctx) -> String {
    Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(&ctx.root)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|commit| !commit.is_empty())
        .unwrap_or_else(|| "unknown".into())
}

/// The SPDX document's own fields.
struct Document {
    name: String,
    namespace: String,
    /// Seconds since 1970, from `SOURCE_DATE_EPOCH` or the commit time.
    created: i64,
    /// What it describes, which contains every package.
    top: Top,
    /// Where the notices are, for the packages' comments: `/` and the
    /// image's [`DIR`], or a bundle's `licenses`.
    notices: String,
}

/// What an SPDX document describes: the image, or a bundle.
struct Top {
    /// Its SPDX ID's last part: `SPDXRef-Package-<id>`.
    id: String,
    name: String,
    version: String,
    download: String,
    license: String,
    comment: String,
}

impl Document {
    /// The document of the image of Dukovo at `commit`.
    fn image(commit: &str, created: i64) -> Document {
        let name = format!("dukovo-{VERSION}-x86_64");
        Document {
            namespace: format!("https://github.com/dukovo-os/dukovo/spdx/{name}-{commit}"),
            name,
            created,
            top: Top {
                id: "dukovo".into(),
                name: "dukovo".into(),
                version: VERSION.into(),
                download: format!("git+https://github.com/dukovo-os/dukovo@{commit}"),
                license: "NOASSERTION".into(),
                comment: format!(
                    "The Dukovo {VERSION} x86-64 boot image. Each component below keeps its \
                     own license; the texts are in /{DIR}/texts and the notices in \
                     /{DIR}/notices."
                ),
            },
            notices: format!("/{DIR}"),
        }
    }

    /// Renders the document in the SPDX 2.3 tag-value format.
    fn render(&self, packages: &[Package], ids: &BTreeSet<String>, root: &Path) -> Result<String> {
        let top = &self.top;
        let top_id = format!("SPDXRef-Package-{}", top.id);
        let mut out = String::new();
        let o = &mut out;
        let _ = writeln!(o, "SPDXVersion: SPDX-2.3");
        let _ = writeln!(o, "DataLicense: CC0-1.0");
        let _ = writeln!(o, "SPDXID: SPDXRef-DOCUMENT");
        let _ = writeln!(o, "DocumentName: {}", self.name);
        let _ = writeln!(o, "DocumentNamespace: {}", self.namespace);
        let _ = writeln!(o, "Creator: Tool: ob-{VERSION}");
        let _ = writeln!(o, "Created: {}", rfc3339(self.created));
        let _ = writeln!(o);
        let _ = writeln!(o, "PackageName: {}", top.name);
        let _ = writeln!(o, "SPDXID: {top_id}");
        let _ = writeln!(o, "PackageVersion: {}", top.version);
        let _ = writeln!(o, "PackageDownloadLocation: {}", top.download);
        let _ = writeln!(o, "FilesAnalyzed: false");
        let _ = writeln!(o, "PackageLicenseConcluded: NOASSERTION");
        let _ = writeln!(o, "PackageLicenseDeclared: {}", top.license);
        let _ = writeln!(o, "PackageCopyrightText: NOASSERTION");
        let _ = writeln!(o, "PackageComment: <text>{}</text>", top.comment);
        let _ = writeln!(o, "Relationship: SPDXRef-DOCUMENT DESCRIBES {top_id}");
        for package in packages {
            let _ = writeln!(o);
            let _ = writeln!(o, "PackageName: {}", package.name);
            let _ = writeln!(o, "SPDXID: {}", package.spdx_id());
            // SPDX has no NOASSERTION for versions; an unknown one is left out.
            if package.version != "NOASSERTION" {
                let _ = writeln!(o, "PackageVersion: {}", package.version);
            }
            let _ = writeln!(o, "PackageDownloadLocation: {}", package.download);
            let _ = writeln!(o, "FilesAnalyzed: false");
            if let Some(sum) = &package.sha256 {
                let _ = writeln!(o, "PackageChecksum: SHA256: {sum}");
            }
            let _ = writeln!(o, "PackageLicenseConcluded: {}", package.license);
            let _ = writeln!(o, "PackageLicenseDeclared: {}", package.license);
            let _ = writeln!(o, "PackageCopyrightText: NOASSERTION");
            if let Some(purl) = package.purl() {
                let _ = writeln!(o, "ExternalRef: PACKAGE-MANAGER purl {purl}");
            }
            let mut comment = package.comment.clone();
            if !package.notices.is_empty() {
                let _ = write!(comment, " Notices: {}/{}.", self.notices, package.notice_dir());
            }
            let _ = writeln!(o, "PackageComment: <text>{comment}</text>");
            let _ = writeln!(o, "Relationship: {top_id} CONTAINS {}", package.spdx_id());
        }
        for id in ids.iter().filter(|id| id.starts_with("LicenseRef-")) {
            let path = text_source(root, id);
            let text =
                std::fs::read_to_string(&path).context(|| format!("reading {}", path.display()))?;
            let _ = writeln!(o);
            let _ = writeln!(o, "LicenseID: {id}");
            let _ = writeln!(o, "ExtractedText: <text>{}</text>", text.trim_end());
            let _ = writeln!(o, "LicenseName: {}", id.trim_start_matches("LicenseRef-"));
        }
        Ok(out)
    }
}

/// A one-line-per-component summary for people reading the image.
fn readme(packages: &[Package]) -> String {
    let mut out = format!(
        "Dukovo {VERSION} third-party and component licenses\n\n\
         dukovo.spdx lists every component of this image in the SPDX 2.3\n\
         tag-value format. texts/ holds the full text of every license named\n\
         there, and notices/ the copyright and license files that came with\n\
         each component's source.\n\n"
    );
    let width = packages.iter().map(|p| p.name.len() + p.version.len() + 1).max().unwrap_or(0);
    for package in packages {
        let what = format!("{} {}", package.name, package.version);
        let _ = writeln!(out, "{what:width$}  {}", package.license);
    }
    out
}

/// Formats seconds since 1970 as `YYYY-MM-DDThh:mm:ssZ`.
pub fn rfc3339(seconds: i64) -> String {
    let days = seconds.div_euclid(86_400);
    let secs = seconds.rem_euclid(86_400);
    // Howard Hinnant's days-to-civil algorithm.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        secs / 3600,
        secs % 3600 / 60,
        secs % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    fn allow(list: &[&str]) -> Vec<String> {
        list.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn each_kept_notice_is_there() {
        for (name, version, path) in KEPT_NOTICES {
            let text = std::fs::read_to_string(root().join(path))
                .unwrap_or_else(|err| panic!("{name} {version}'s notice {path}: {err}"));
            assert!(text.contains("Copyright"), "{path} holds no copyright notice");
        }
    }

    #[test]
    fn parses_expressions() {
        let expr =
            Expr::parse("MIT AND Apache-2.0 WITH LLVM-exception AND (MIT OR Apache-2.0)").unwrap();
        let mut ids = BTreeSet::new();
        expr.collect_ids(&mut ids);
        assert_eq!(ids, BTreeSet::from(["Apache-2.0", "LLVM-exception", "MIT"].map(String::from)));
        assert!(Expr::parse("MIT/Apache-2.0").unwrap().satisfied_by(&allow(&["MIT"])));
        assert!(Expr::parse("MIT AND").is_err());
        assert!(Expr::parse("(MIT").is_err());
        assert!(Expr::parse("MIT Apache-2.0").is_err());
    }

    #[test]
    fn checks_allow_lists() {
        let ok = |text: &str, list: &[&str]| Expr::parse(text).unwrap().satisfied_by(&allow(list));
        assert!(ok("Apache-2.0 OR MIT", &["MIT"]));
        assert!(!ok("GPL-2.0-only", &["MIT", "Apache-2.0"]));
        assert!(ok("MIT OR GPL-2.0-only", &["MIT"]));
        assert!(!ok("MIT AND GPL-2.0-only", &["MIT"]));
        assert!(!ok("Apache-2.0 WITH LLVM-exception", &["Apache-2.0"]));
        assert!(ok("Apache-2.0 WITH LLVM-exception", &["Apache-2.0 WITH LLVM-exception"]));
    }

    #[test]
    fn parses_cargo_tree_lines() {
        assert_eq!(
            parse_tree_line("radix-klog v0.0.1 (/src/kernel/lib/klog)|Apache-2.0 OR MIT (*)"),
            Some(TreeEntry {
                name: "radix-klog".into(),
                version: "0.0.1".into(),
                source: Source::Path("/src/kernel/lib/klog".into()),
                license: "Apache-2.0 OR MIT".into(),
            })
        );
        let entry = parse_tree_line("bitflags v2.13.2|MIT OR Apache-2.0").unwrap();
        assert_eq!(entry.source, Source::Registry);
        let entry = parse_tree_line("x v1.0.0 (https://example.com/x#abc)|MIT").unwrap();
        assert!(matches!(entry.source, Source::Other(_)));
        let entry = parse_tree_line("memchr v2.7.4|Unlicense/MIT").unwrap();
        assert_eq!(entry.license, "Unlicense OR MIT");
        let entry =
            parse_tree_line("serde_derive v1.0.229 (proc-macro)|MIT OR Apache-2.0").unwrap();
        assert_eq!((entry.name.as_str(), entry.source), ("serde_derive", Source::Registry));
        let entry = parse_tree_line(
            "i18n-embed-fl v0.9.4 (proc-macro) (/src/w/.radix-rust/crates/i18n-embed-fl-0.9.4)|MIT",
        )
        .unwrap();
        assert_eq!(
            (entry.version.as_str(), entry.source),
            ("0.9.4", Source::Path("/src/w/.radix-rust/crates/i18n-embed-fl-0.9.4".into()))
        );
        assert_eq!(parse_tree_line("").map(|e| e.name), None);
    }

    /// A crate Cargo for Radix gave through the workspace's link is listed
    /// from the toolchain, where the ports that patch it put it.
    #[test]
    fn maps_crates_through_the_link() {
        let rust = Path::new("/src/out/toolchain/rust");
        assert_eq!(
            through_link(Path::new("/src/userspace/programs/.radix-rust/crates/nix-0.30.1"), rust),
            Some(PathBuf::from("/src/out/toolchain/rust/crates/nix-0.30.1"))
        );
        assert_eq!(
            through_link(Path::new("/src/userspace/programs/.radix-rust/vendor/libc"), rust),
            Some(PathBuf::from("/src/out/toolchain/rust/vendor/libc"))
        );
        assert_eq!(through_link(Path::new("/src/kernel/lib/klog"), rust), None);
    }

    /// The kernel area is permissive only (ADR 0021): copyleft licenses
    /// belong to separate programs and bundles, never to kernel code.
    #[test]
    fn kernel_policy_is_permissive() {
        let text = std::fs::read_to_string(root().join(KERNEL_POLICY)).unwrap();
        for id in parse_allow_list(&text).unwrap() {
            for copyleft in ["GPL", "LGPL", "AGPL", "MPL", "EPL", "CDDL", "EUPL", "OSL", "SSPL"] {
                assert!(!id.contains(copyleft), "kernel/deny.toml allows {id}");
            }
        }
    }

    /// Every license the kernel area allows, and every license a fixed
    /// area or a bundle allows, has its text ready to copy into the image
    /// or the bundle.
    #[test]
    fn allowed_licenses_have_texts() {
        let text = std::fs::read_to_string(root().join(KERNEL_POLICY)).unwrap();
        let mut ids = BTreeSet::new();
        for id in parse_allow_list(&text).unwrap() {
            Expr::parse(&id).unwrap().collect_ids(&mut ids);
        }
        let fixed = BOOTLOADER_ALLOW.iter().chain(FIRMWARE_ALLOW).chain(PORT_EXTRA_ALLOW);
        for id in fixed.chain(BUNDLE_EXTRA_ALLOW) {
            Expr::parse(id).unwrap().collect_ids(&mut ids);
        }
        for id in ids {
            let path = text_source(&root(), &id);
            assert!(path.is_file(), "{} is missing", path.display());
        }
    }

    /// No license text is kept twice, and the copies at the top of the
    /// repository, which GitHub and crate tools look for, match LICENSES/.
    #[test]
    fn license_texts_are_kept_once() {
        let root = root();
        for entry in std::fs::read_dir(root.join("LICENSES")).unwrap() {
            let name = entry.unwrap().file_name().into_string().unwrap();
            let id = name.strip_suffix(".txt").unwrap();
            let other = root.join("image/licenses").join(format!("LICENSE-{id}.txt"));
            assert!(!other.exists(), "{} duplicates LICENSES/{name}", other.display());
        }
        for (top, id) in [("LICENSE-APACHE", "Apache-2.0"), ("LICENSE-MIT", "MIT")] {
            let top = std::fs::read(root.join(top)).unwrap();
            assert!(top == std::fs::read(text_source(&root, id)).unwrap(), "{id} differs");
        }
    }

    /// A component with a LICENSE in several folders keeps all of them.
    #[test]
    fn names_notices_apart() {
        let paths: Vec<PathBuf> =
            ["LICENSE", "Zend/LICENSE", "Zend/asm/LICENSE", "README", "a/Zend/LICENSE"]
                .iter()
                .map(|p| Path::new("/src").join(p))
                .collect();
        assert_eq!(
            notice_names(&paths),
            ["LICENSE", "Zend-LICENSE", "asm-LICENSE", "README", "Zend-LICENSE-2"]
        );
        assert!(notice_names(&[]).is_empty());
    }

    #[test]
    fn finds_build_std_crates() {
        let args = ["--target".into(), "x.json".into(), "-Zbuild-std=core,alloc".into()];
        assert_eq!(build_std_crates(&args), ["core", "alloc"]);
        let names: Vec<&str> = SYSROOT_CRATES.iter().map(|(name, _)| *name).collect();
        for name in build_std_crates(&["-Zbuild-std=core,compiler_builtins,alloc".into()]) {
            assert!(names.contains(&name), "{name}");
        }
    }

    #[test]
    fn formats_times() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339(1_792_537_558), "2026-10-20T23:05:58Z");
        assert_eq!(rfc3339(951_782_400), "2000-02-29T00:00:00Z");
    }

    /// A folder under the temporary folder, empty.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ob-licenses-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// The folder `name` in `root`, with `files` in it, each a name and a text.
    fn folder_with(root: &Path, name: &str, files: &[(&str, &str)]) -> PathBuf {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        for (file, text) in files {
            std::fs::write(dir.join(file), text).unwrap();
        }
        dir
    }

    /// The text of the license `id`, as the repository keeps it.
    fn text_of(id: &str) -> String {
        std::fs::read_to_string(text_source(&root(), id)).unwrap()
    }

    #[test]
    fn go_license_reads_each_license_by_its_words() {
        let scratch = scratch("go-each");
        // The texts are the licenses' own, so the words are the ones the
        // licenses use: 0BSD's text must not read as ISC, nor the
        // MPL-2.0's, which names the GPL, as refused.
        for id in [
            "Apache-2.0",
            "MIT",
            "BSD-3-Clause",
            "BSD-2-Clause",
            "ISC",
            "0BSD",
            "MPL-2.0",
            "Unlicense",
            "CC0-1.0",
        ] {
            let dir = folder_with(&scratch, id, &[("LICENSE", &text_of(id))]);
            assert_eq!(go_license(&dir).as_deref(), Ok(id), "{id}");
        }
        // The notice alone, in the words of the Apache license's appendix.
        let short = "Licensed under the Apache License, Version 2.0 (the \"License\");";
        let dir = folder_with(&scratch, "short", &[("LICENSE.md", short)]);
        assert_eq!(go_license(&dir).as_deref(), Ok("Apache-2.0"));
        std::fs::remove_dir_all(&scratch).unwrap();
    }

    #[test]
    fn the_bsd_license_is_three_clause_for_the_names_it_bars_and_two_clause_without() {
        let scratch = scratch("go-bsd");
        let three = text_of("BSD-3-Clause");
        let wrapped = three.replace("may be used to endorse", "may be\n\tused to\n  endorse");
        let dir = folder_with(&scratch, "wrapped", &[("LICENSE", &wrapped)]);
        assert_eq!(go_license(&dir).as_deref(), Ok("BSD-3-Clause"));
        // One of the variants: the author's name, in the singular.
        let single = three.replace(
            "Neither the name of the copyright holder nor the names of its",
            "The name of the author may not be used to endorse or promote products derived\n\
             from this software, nor may the names of its",
        );
        assert_ne!(single, three, "the variant is not in the text");
        let dir = folder_with(&scratch, "single", &[("LICENSE", &single)]);
        assert_eq!(go_license(&dir).as_deref(), Ok("BSD-3-Clause"));
        std::fs::remove_dir_all(&scratch).unwrap();
    }

    #[test]
    fn go_license_joins_the_licenses_a_module_states_in_a_fixed_order() {
        let scratch = scratch("go-joined");
        let (mit, bsd, apache) = (text_of("MIT"), text_of("BSD-3-Clause"), text_of("Apache-2.0"));
        // A file that carries a second license for code it copied, in either
        // order of the two texts.
        for both in [format!("{mit}\n\n{bsd}"), format!("{bsd}\n\n{mit}")] {
            let dir = folder_with(&scratch, "one-file", &[("LICENSE", &both)]);
            assert_eq!(go_license(&dir).as_deref(), Ok("MIT AND BSD-3-Clause"));
        }
        // Two files, and a NOTICE that names nothing the module is under.
        let notice = "This product includes software developed at Canonical Ltd.";
        let files =
            [("LICENSE", apache.as_str()), ("LICENSE.libyaml", mit.as_str()), ("NOTICE", notice)];
        let dir = folder_with(&scratch, "two-files", &files);
        assert_eq!(go_license(&dir).as_deref(), Ok("Apache-2.0 AND MIT"));
        // A license two files state is named once.
        let dir = folder_with(&scratch, "twice", &[("LICENSE", &mit), ("COPYING", &mit)]);
        assert_eq!(go_license(&dir).as_deref(), Ok("MIT"));
        let (cc0, unlicense) = (text_of("CC0-1.0"), text_of("Unlicense"));
        let files = [
            ("LICENSE", cc0.as_str()),
            ("COPYING", unlicense.as_str()),
            ("LICENCE", apache.as_str()),
        ];
        let dir = folder_with(&scratch, "three", &files);
        assert_eq!(go_license(&dir).as_deref(), Ok("Apache-2.0 AND Unlicense AND CC0-1.0"));
        std::fs::remove_dir_all(&scratch).unwrap();
    }

    #[test]
    fn go_license_reads_the_files_at_the_top_of_the_module_by_their_names_in_upper_case() {
        let scratch = scratch("go-names");
        let (mit, gpl) = (text_of("MIT"), text_of("GPL-2.0-only"));
        let files = [
            ("license.md", mit.as_str()),
            ("LICENCE", &text_of("Apache-2.0")),
            ("COPYING.txt", &text_of("BSD-2-Clause")),
            ("UNLICENSE", &text_of("Unlicense")),
            // Not license files: what a project keeps of others' work, its
            // documentation, and files that are not at the top.
            ("NOTICE", &gpl),
            ("COPYRIGHT", &gpl),
            ("README", &gpl),
            ("PATENTS", &gpl),
        ];
        let dir = folder_with(&scratch, "module", &files);
        folder_with(&dir, "sub", &[("LICENSE", &gpl)]);
        folder_with(&dir, "LICENSES", &[("GPL-2.0-only.txt", &gpl)]);
        assert_eq!(
            go_license(&dir).as_deref(),
            Ok("Apache-2.0 AND MIT AND BSD-2-Clause AND Unlicense")
        );
        std::fs::remove_dir_all(&scratch).unwrap();
    }

    #[test]
    fn go_license_refuses_the_licenses_a_bundle_does_not_take_and_names_the_file() {
        let scratch = scratch("go-refused");
        let copyleft = "the GNU GPL, LGPL or AGPL";
        let mit_or_gpl =
            format!("{}\n\nThis code may also be used under the GPL, version 2.", text_of("MIT"));
        let texts: [(String, &str); 12] = [
            (text_of("GPL-2.0-only"), copyleft),
            (text_of("LGPL-2.1-or-later"), copyleft),
            (text_of("GPL-3.0-or-later"), copyleft),
            // A module that offers the code under the GPL too names it,
            // whatever else it states.
            (mit_or_gpl, copyleft),
            ("GNU AFFERO GENERAL PUBLIC LICENSE Version 3".into(), copyleft),
            ("Eclipse Public License - v 2.0".into(), "the Eclipse Public License"),
            (
                "COMMON DEVELOPMENT AND DISTRIBUTION LICENSE (CDDL) Version 1.0".into(),
                "the Common Development and Distribution License",
            ),
            (
                "Server Side Public License VERSION 1, OCTOBER 16, 2018".into(),
                "the Server Side Public License",
            ),
            ("Business Source License 1.1".into(), "the Business Source License"),
            ("\"Commons Clause\" License Condition v1.0".into(), "the Commons Clause"),
            (
                "Creative Commons Attribution-ShareAlike 4.0 International".into(),
                "a Creative Commons ShareAlike license",
            ),
            ("Licensed under CC BY-SA 3.0".into(), "a Creative Commons ShareAlike license"),
        ];
        for (n, (text, named)) in texts.iter().enumerate() {
            let dir = folder_with(&scratch, &format!("module{n}"), &[("COPYING", text)]);
            assert_eq!(go_license(&dir), Err(format!("COPYING names {named}")), "{text}");
        }
        std::fs::remove_dir_all(&scratch).unwrap();
    }

    #[test]
    fn a_bsd_license_with_the_clause_on_advertising_is_refused() {
        let scratch = scratch("go-bsd4");
        for id in ["BSD-4-Clause", "BSD-4-Clause-UC", "BSD-4-Clause-Shortened"] {
            let dir = folder_with(&scratch, id, &[("LICENSE", &text_of(id))]);
            assert_eq!(
                go_license(&dir),
                Err("LICENSE has the BSD license's clause on advertising".into()),
                "{id}"
            );
        }
        std::fs::remove_dir_all(&scratch).unwrap();
    }

    #[test]
    fn the_mpl_2_0_is_not_refused_for_naming_the_licenses_its_code_may_also_be_distributed_under() {
        let scratch = scratch("go-mpl");
        let mpl = text_of("MPL-2.0");
        // The reason for the rule: the license's own text names them.
        assert!(mpl.contains("GNU General Public License"));
        let dir = folder_with(&scratch, "plain", &[("LICENSE", &mpl)]);
        assert_eq!(go_license(&dir).as_deref(), Ok("MPL-2.0"));
        // A module that offers its code under the GPL as well says so
        // beside the text, and is refused as any that names it.
        let dual =
            format!("{mpl}\n\nThis code is also available under the GNU General Public License.");
        let dir = folder_with(&scratch, "dual", &[("LICENSE", &dual)]);
        assert_eq!(go_license(&dir), Err("LICENSE names the GNU GPL, LGPL or AGPL".into()));
        std::fs::remove_dir_all(&scratch).unwrap();
    }

    #[test]
    fn go_license_refuses_a_module_with_no_license_file_or_none_in_words_it_knows() {
        let scratch = scratch("go-none");
        let none = "it has no LICENSE, LICENCE, COPYING or UNLICENSE file";
        let dir = folder_with(&scratch, "empty", &[]);
        assert_eq!(go_license(&dir), Err(none.into()));
        // Another file's text is no license file, nor is a NOTICE: it lists
        // what the project keeps of others' work.
        let mit = text_of("MIT");
        let dir = folder_with(&scratch, "readme", &[("README.md", &mit), ("NOTICE", &mit)]);
        assert_eq!(go_license(&dir), Err(none.into()));
        // A license file of words that are not known: MIT-0 asks for no
        // notice, and a copyright line is no license.
        let files = [
            ("LICENSE", "Copyright 2020 Some One. All rights reserved."),
            ("COPYING", &text_of("MIT-0")),
        ];
        let dir = folder_with(&scratch, "unknown", &files);
        let err = go_license(&dir).unwrap_err();
        assert_eq!(
            err,
            "its license files (COPYING, LICENSE) hold none of Apache-2.0, MIT, BSD-3-Clause, \
             BSD-2-Clause, ISC, 0BSD, MPL-2.0, Unlicense, CC0-1.0 in words that are known"
        );
        // A typo in a phrase is a phrase not found: micro-editor/terminal's
        // MIT license says "thismssion notice".
        let typo = mit.replace("this permission", "thismssion");
        let dir = folder_with(&scratch, "typo", &[("LICENSE", &typo)]);
        assert!(
            go_license(&dir).unwrap_err().starts_with("its license files (LICENSE) hold none of")
        );
        // What a NOTICE names does not count against a module.
        let files = [("LICENSE", mit.as_str()), ("NOTICE", &text_of("GPL-3.0-or-later"))];
        let dir = folder_with(&scratch, "notice", &files);
        assert_eq!(go_license(&dir).as_deref(), Ok("MIT"));
        std::fs::remove_dir_all(&scratch).unwrap();
    }

    #[test]
    fn a_go_modules_notices_are_its_license_and_notice_files_with_its_patent_grant() {
        let scratch = scratch("go-notices");
        let files = [
            ("LICENSE", "a"),
            ("PATENTS", "b"),
            ("NOTICE", "c"),
            ("UNLICENSE", "d"),
            ("COPYRIGHT", "e"),
            ("README.md", "f"),
        ];
        let dir = folder_with(&scratch, "module", &files);
        folder_with(&dir, "sub", &[("LICENSE", "g")]);
        let names = |files: Vec<PathBuf>| -> Vec<String> {
            files.iter().map(|f| f.file_name().unwrap().to_string_lossy().into_owned()).collect()
        };
        assert_eq!(
            names(go_notices(&dir)),
            ["COPYRIGHT", "LICENSE", "NOTICE", "PATENTS", "UNLICENSE"]
        );
        // Go's tree keeps the grant beside its LICENSE as well, and the notices
        // of a port in C or Rust are those of its names.
        assert_eq!(names(notice_files(&dir)), ["COPYRIGHT", "LICENSE", "NOTICE", "PATENTS"]);
        std::fs::remove_dir_all(&scratch).unwrap();
    }

    /// A recipe of the port `name`, `rest` being what follows its fixed keys.
    fn recipe(name: &str, version: &str, license: &str, rest: &str) -> Recipe {
        toml::from_str(&format!(
            "name = \"{name}\"\nversion = \"{version}\"\nsummary = \"s\"\nlicense = \"{license}\"\n\
             homepage = \"https://example.org\"\n{rest}"
        ))
        .unwrap()
    }

    fn port(recipe: Recipe, root: &Path, source: Option<PathBuf>, modules: Vec<PathBuf>) -> Port {
        let dir = root.join("ports").join(&recipe.name);
        Port { recipe, dir, source, extras: Vec::new(), crates: Vec::new(), modules }
    }

    fn context(root: &Path) -> Ctx {
        Ctx {
            out: root.join("out"),
            target: root.join("target"),
            root: root.to_path_buf(),
            config: crate::config::Config::default(),
            verbose: false,
        }
    }

    /// What `go version -m` printed for two programs of one port: modules
    /// from the proxy, one of them named in capitals, a patched one under
    /// two versions, one patched alone, and one replaced by another's.
    const MODULES: &str = "/out/bundle/bin/tool: go1.27.4\n\
        \tpath\texample.com/tool/cmd/tool\n\
        \tmod\texample.com/tool\t(devel)\t\n\
        \tdep\tgithub.com/BurntSushi/toml\tv1.3.2\th1:x=\n\
        \tdep\tgithub.com/creack/pty\tv1.1.18\n\
        \t=>\t./.radix-go/github.com_creack_pty@v1.1.18\t(devel)\t\n\
        \t\n\
        \tdep\tgithub.com/kballard/go-shellquote\tv0.0.0-20180428030007-95032a82bc51\n\
        \t=>\tgithub.com/micro-editor/go-shellquote\tv0.0.0-20250101105543-feb6c39314f5\th1:y=\n\
        \tdep\tgithub.com/kr/logfmt\tv0.0.0-20140226030751-b84e30acd515\th1:z=\n\
        \tdep\tgolang.org/x/sys\tv0.30.0\n\
        \t=>\t./.radix-go/golang.org_x_sys@v0.50.0\t(devel)\t\n\
        \t\n\
        /out/bundle/bin/other: go1.27.4\n\
        \tpath\texample.com/tool/cmd/other\n\
        \tdep\tgithub.com/BurntSushi/toml\tv1.3.2\th1:x=\n\
        \tdep\tgolang.org/x/sys\tv0.35.0\n\
        \t=>\t./.radix-go/golang.org_x_sys@v0.50.0\t(devel)\t\n\
        \t\n";

    /// A port `tool` in Go that links [`MODULES`], with the go and go-modules
    /// ports it is built with, and Go's module cache holding what it links
    /// from the proxy. Returns the context and the ports; the tool is last.
    fn go_bundle(root: &Path) -> (Ctx, Ports) {
        let ctx = context(root);
        let cached = |path: &str, version: &str, files: &[(&str, &str)]| {
            let dir = ports::go_cached_module(&ctx, path, version);
            std::fs::create_dir_all(&dir).unwrap();
            for (file, text) in files {
                std::fs::write(dir.join(file), text).unwrap();
            }
        };
        let (mit, bsd) = (text_of("MIT"), text_of("BSD-3-Clause"));
        cached("github.com/BurntSushi/toml", "v1.3.2", &[("COPYING", &mit)]);
        cached(
            "github.com/micro-editor/go-shellquote",
            "v0.0.0-20250101105543-feb6c39314f5",
            &[("LICENSE.txt", &mit)],
        );
        // The words of an MIT license are in its Readme, not in a license file.
        cached("github.com/kr/logfmt", "v0.0.0-20140226030751-b84e30acd515", &[("Readme", &mit)]);
        let tree = |name: &str, license: &str| {
            folder_with(
                root,
                name,
                &[("LICENSE", license), ("PATENTS", "grant"), ("main.go", "package x")],
            )
        };
        let go_source = tree("go-source", &bsd);
        let sys = tree("sys", &bsd);
        let pty = tree("pty", &mit);
        let sha = "ab".repeat(32);
        let go = recipe(
            "go",
            "1.27.4",
            "BSD-3-Clause",
            "toolchain = true\n[source]\ngit = \"https://github.com/golang/go.git\"\n\
             commit = \"7b2881024c95a11ddb86663a616340aa3a88b67f\"\ntag = \"go1.27.4\"\n",
        );
        let patching = recipe(
            "go-modules",
            "1",
            "BSD-3-Clause AND MIT",
            &format!(
                "toolchain = true\n\
                 [[module]]\npath = \"golang.org/x/sys\"\nversion = \"v0.50.0\"\nsha256 = \"{sha}\"\n\
                 license = \"BSD-3-Clause\"\nall-versions = true\n\
                 [[module]]\npath = \"github.com/creack/pty\"\nversion = \"v1.1.18\"\n\
                 sha256 = \"{sha}\"\nlicense = \"MIT\"\n"
            ),
        );
        let tool = recipe(
            "tool",
            "2.0",
            "MIT",
            "depends = [\"go\", \"go-modules\"]\n\
             [go-licenses]\n\"github.com/kr/logfmt\" = \"MIT\"\n",
        );
        let info = ports::info_dir(&ctx, "tool");
        std::fs::create_dir_all(&info).unwrap();
        std::fs::write(info.join("modules.txt"), MODULES).unwrap();
        let ports = Ports {
            toolchain: Toolchain::bare(root.join("out/toolchain")),
            ports: vec![
                port(go, root, Some(go_source), vec![]),
                port(patching, root, None, vec![sys, pty]),
                port(tool, root, None, vec![]),
            ],
        };
        (ctx, ports)
    }

    /// The licenses the recipe of the port `go_bundle` lists last gives its modules.
    fn go_licenses(ports: &mut Ports) -> &mut std::collections::BTreeMap<String, String> {
        &mut ports.ports.last_mut().unwrap().recipe.go_licenses
    }

    /// The packages `port_packages` gives a port, by name, version and license.
    fn listed(ctx: &Ctx, ports: &Ports, name: &str) -> Result<Vec<Package>> {
        let port = ports.ports.iter().find(|p| p.recipe.name == name).unwrap();
        let mut packages = Vec::new();
        port_packages(ctx, ports, port, &mut packages)?;
        Ok(packages)
    }

    fn named(packages: &[Package]) -> Vec<(&str, &str, &str)> {
        packages.iter().map(|p| (&*p.name, &*p.version, &*p.license)).collect()
    }

    fn notice_names_of(package: &Package) -> Vec<String> {
        package
            .notices
            .iter()
            .map(|n| n.file_name().unwrap().to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn a_port_in_go_has_a_package_for_each_module_its_programs_link_and_one_for_go() {
        let root = scratch("go-packages");
        let (ctx, ports) = go_bundle(&root);
        let packages = listed(&ctx, &ports, "tool").unwrap();
        // In the order the modules are named, each once, and Go's standard
        // library and runtime last.
        assert_eq!(
            named(&packages),
            [
                ("github.com/BurntSushi/toml", "v1.3.2", "MIT"),
                ("github.com/creack/pty", "v1.1.18", "MIT"),
                (
                    "github.com/micro-editor/go-shellquote",
                    "v0.0.0-20250101105543-feb6c39314f5",
                    "MIT"
                ),
                ("github.com/kr/logfmt", "v0.0.0-20140226030751-b84e30acd515", "MIT"),
                ("golang.org/x/sys", "v0.50.0", "BSD-3-Clause"),
                ("go", "1.27.4", "BSD-3-Clause"),
            ]
        );
        assert!(packages.iter().all(|p| p.area == Area::Port));
        // A module of the proxy: its zip's URL, with the capitals spelled as
        // the proxy does, no checksum, and the notices of its folder.
        let toml = &packages[0];
        assert_eq!(
            toml.download,
            "https://proxy.golang.org/github.com/!burnt!sushi/toml/@v/v1.3.2.zip"
        );
        assert_eq!(toml.sha256, None);
        assert_eq!(notice_names_of(toml), ["COPYING"]);
        assert_eq!(
            toml.comment,
            "A Go module from Go's module proxy, built into tool 2.0 by ports/tool of Dukovo."
        );
        // The replacing module's code is what is linked.
        assert!(packages[2].download.contains("/github.com/micro-editor/go-shellquote/@v/"));
        // A patched module: the go-modules recipe's license, SHA-256 and
        // notices, the proxy's URL, and the versions it stands in for.
        let sys = &packages[4];
        assert_eq!(sys.sha256.as_deref(), Some("ab".repeat(32).as_str()));
        assert_eq!(sys.download, "https://proxy.golang.org/golang.org/x/sys/@v/v0.50.0.zip");
        assert_eq!(notice_names_of(sys), ["LICENSE", "PATENTS"]);
        assert_eq!(
            sys.comment,
            "A Go module from Go's module proxy, patched for Radix by ports/go-modules. It stands \
             in for the version of it that tool 2.0 names (v0.30.0, v0.35.0), and is built into \
             tool 2.0 by ports/tool of Dukovo."
        );
        // The standard library comes with its patent grant.
        let go = &packages[5];
        assert_eq!(
            go.download,
            "git+https://github.com/golang/go.git@7b2881024c95a11ddb86663a616340aa3a88b67f"
        );
        assert_eq!(notice_names_of(go), ["LICENSE", "PATENTS"]);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_package_is_listed_once_though_a_bundle_names_its_port_and_a_port_it_links() {
        let root = scratch("go-once");
        let (ctx, ports) = go_bundle(&root);
        let tool = ports.ports.iter().find(|p| p.recipe.name == "tool").unwrap();
        let mut packages = Vec::new();
        port_packages(&ctx, &ports, tool, &mut packages).unwrap();
        let count = packages.len();
        port_packages(&ctx, &ports, tool, &mut packages).unwrap();
        assert_eq!(packages.len(), count);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn the_license_a_recipe_gives_a_go_module_is_used_and_the_files_are_not_read() {
        let root = scratch("go-override");
        let (ctx, mut ports) = go_bundle(&root);
        // logfmt's files state nothing, and the recipe says why it is MIT.
        let without = go_licenses(&mut ports).remove("github.com/kr/logfmt");
        assert_eq!(without.as_deref(), Some("MIT"));
        let err = listed(&ctx, &ports, "tool").unwrap_err().to_string();
        assert!(
            err.contains("port tool: the license of a Go module its programs link is not known:")
                && err.contains(
                    "github.com/kr/logfmt v0.0.0-20140226030751-b84e30acd515: it has no LICENSE"
                ),
            "{err}"
        );
        // A module whose files are readable gets the recipe's license when it
        // gives one.
        go_licenses(&mut ports).insert("github.com/kr/logfmt".into(), "MIT".into());
        go_licenses(&mut ports).insert("github.com/BurntSushi/toml".into(), "ISC".into());
        let packages = listed(&ctx, &ports, "tool").unwrap();
        assert_eq!(named(&packages)[0], ("github.com/BurntSushi/toml", "v1.3.2", "ISC"));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_license_in_go_licenses_that_no_program_links_is_an_error() {
        let root = scratch("go-unused");
        let (ctx, mut ports) = go_bundle(&root);
        // A module no program links, one the go-modules port patches, whose
        // license is its recipe's, and one go.mod replaces, whose path is
        // not the code's.
        for path in
            ["github.com/gone/module", "golang.org/x/sys", "github.com/kballard/go-shellquote"]
        {
            go_licenses(&mut ports).insert(path.into(), "MIT".into());
            let err = format!("{:?}", listed(&ctx, &ports, "tool").unwrap_err());
            assert!(
                err.contains(&format!(
                    "port tool: [go-licenses] gives a license for {path}, which none of its \
                     programs links from Go's module proxy"
                )),
                "{err}"
            );
            go_licenses(&mut ports).remove(path);
        }
        // A port that lists no modules has none to give a license to.
        std::fs::remove_file(ports::info_dir(&ctx, "tool").join("modules.txt")).unwrap();
        go_licenses(&mut ports).insert("github.com/gone/module".into(), "MIT".into());
        assert!(listed(&ctx, &ports, "tool").is_err());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_module_missing_from_the_cache_or_the_patched_ones_is_an_error_that_says_to_build_again() {
        let root = scratch("go-missing");
        let (ctx, ports) = go_bundle(&root);
        let cached = ports::go_cached_module(&ctx, "github.com/BurntSushi/toml", "v1.3.2");
        std::fs::remove_dir_all(&cached).unwrap();
        let err = format!("{:?}", listed(&ctx, &ports, "tool").unwrap_err());
        assert!(
            err.contains(
                "Go module github.com/BurntSushi/toml v1.3.2, which its programs link, is not in"
            ),
            "{err}"
        );
        assert!(err.contains("./ob ports --rebuild tool"), "{err}");
        // A patched module that go-modules does not have any more.
        let info = ports::info_dir(&ctx, "tool");
        std::fs::write(
            info.join("modules.txt"),
            "\tdep\ta/b\tv1.0.0\n\t=>\t./.radix-go/a_b@v1.0.0\t(devel)\t\n",
        )
        .unwrap();
        let err = format!("{:?}", listed(&ctx, &ports, "tool").unwrap_err());
        assert!(
            err.contains("its programs link a_b@v1.0.0, which ports/go-modules does not patch"),
            "{err}"
        );
        assert!(err.contains("./ob ports --rebuild tool"), "{err}");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn every_module_whose_license_is_not_known_is_named_at_once() {
        let root = scratch("go-unread");
        let (ctx, mut ports) = go_bundle(&root);
        go_licenses(&mut ports).clear();
        let toml = ports::go_cached_module(&ctx, "github.com/BurntSushi/toml", "v1.3.2");
        std::fs::write(toml.join("COPYING"), text_of("GPL-2.0-only")).unwrap();
        let err = listed(&ctx, &ports, "tool").unwrap_err().to_string();
        assert!(
            err.contains(
                "github.com/BurntSushi/toml v1.3.2: COPYING names the GNU GPL, LGPL or AGPL"
            ),
            "{err}"
        );
        assert!(
            err.contains(
                "github.com/kr/logfmt v0.0.0-20140226030751-b84e30acd515: it has no LICENSE"
            ),
            "{err}"
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_port_not_in_go_has_no_go_packages() {
        let root = scratch("go-not");
        let (ctx, mut ports) = go_bundle(&root);
        std::fs::remove_file(ports::info_dir(&ctx, "tool").join("modules.txt")).unwrap();
        go_licenses(&mut ports).clear();
        assert!(listed(&ctx, &ports, "tool").unwrap().is_empty());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn go_modules_are_fetched_when_the_proxys_modules_a_port_linked_are_in_the_cache() {
        let root = scratch("go-fetched");
        let ctx = context(&root);
        // No list: nothing to be missing.
        assert!(go_modules_fetched(&ctx, "tool"));
        let info = ports::info_dir(&ctx, "tool");
        std::fs::create_dir_all(&info).unwrap();
        std::fs::write(info.join("modules.txt"), MODULES).unwrap();
        assert!(!go_modules_fetched(&ctx, "tool"));
        // The three of the proxy's, and the patched ones are not in the cache.
        let cached = [
            ("github.com/BurntSushi/toml", "v1.3.2"),
            ("github.com/micro-editor/go-shellquote", "v0.0.0-20250101105543-feb6c39314f5"),
            ("github.com/kr/logfmt", "v0.0.0-20140226030751-b84e30acd515"),
        ];
        for (n, (path, version)) in cached.iter().enumerate() {
            assert!(!go_modules_fetched(&ctx, "tool"), "{n}");
            std::fs::create_dir_all(ports::go_cached_module(&ctx, path, version)).unwrap();
        }
        assert!(go_modules_fetched(&ctx, "tool"));
        // A list that cannot be read is reported with the licenses.
        std::fs::write(info.join("modules.txt"), "\tdep\ta\n").unwrap();
        assert!(go_modules_fetched(&ctx, "tool"));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn renders_spdx() {
        let crate_named = |name: &str, download: String| Package {
            name: name.into(),
            version: "0.1.160".into(),
            license: "MIT".into(),
            area: Area::Kernel,
            download,
            sha256: Some("ab".into()),
            notices: vec!["LICENSE.txt".into()],
            comment: "C.".into(),
        };
        let packages = vec![
            crate_named("compiler_builtins", "NOASSERTION".into()),
            crate_named("regex", format!("{CRATES_DOWNLOAD}regex/0.1.160/download")),
        ];
        let document = Document::image("c0ffee", 0);
        let ids = BTreeSet::from(["MIT".to_string()]);
        let text = document.render(&packages, &ids, &root()).unwrap();
        assert!(text.starts_with("SPDXVersion: SPDX-2.3\n"));
        assert!(text.contains("SPDXID: SPDXRef-Package-compiler-builtins-0.1.160\n"));
        assert!(text.contains("PackageChecksum: SHA256: ab\n"));
        assert!(text.contains(
            "Relationship: SPDXRef-Package-dukovo CONTAINS \
             SPDXRef-Package-compiler-builtins-0.1.160\n"
        ));
        assert!(
            text.contains("Notices: /system/resources/licenses/notices/compiler_builtins-0.1.160.")
        );
        assert!(text.contains("Created: 1970-01-01T00:00:00Z\n"));
        // A crate from crates.io, by its package URL, for advisories.
        assert!(text.contains("ExternalRef: PACKAGE-MANAGER purl pkg:cargo/regex@0.1.160\n"));
        assert_eq!(text.matches("ExternalRef:").count(), 1);
    }
}

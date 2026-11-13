// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The Dukovo Lab's side of `ob` (ADR 0401): `ob image --lab`, the image
//! the lab tests, and `ob lab`, which runs the lab's harness on this
//! tree's tests in `lab/tests`.
//!
//! A lab image is an installed system's disk with three things added,
//! all outside the system's slot, so that no release (which is made from
//! the slot alone) can ever carry them:
//!
//! - the `lab` account, an administrator's that signs in over SSH with
//!   the lab key, whose password is the lab password (ADR 0408), for tests
//!   that run as root, or without one a password no one knows;
//! - its home, the account's, holding the lab's public key in
//!   `~/.ssh/authorized_keys`;
//! - sshd turned on, through an override file, and a sign-in banner that
//!   says the image is a test image.
//!
//! The harness itself, dukovo-lab, lives in dukovo-os/lab; `ob lab` finds
//! it on PATH (or at `OB_LAB`), builds the lab image and passes it on.
//! When `ob lab run` fails before a test starts, run `dukovo-lab` with
//! the same arguments to see its own message.
//!
//! Plain `ob lab`, with no dukovo-lab command, is the one command a person
//! needs: it sets up what is missing (dukovo-lab, the lab key, the
//! lab-results clone that Send uses), checks QEMU, puts in place the
//! timer that starts schedules, then builds the image and opens the
//! runner page. Each setup step is skipped once it is done; the timer is
//! made again each time, quietly, so it names the dukovo-lab of now.
//!
//! The commands that build the image also build the Linux kit that
//! performance tests hold Dukovo to ([`linux_kit`]), and pass it on with
//! `--linux`. A kit that will not build (offline the first time, say) only
//! makes those tests skip. They also make the browser kit that the web
//! console's tests drive pages with ([`browser_kit`], ADR 0510) and pass
//! it on with `--browsers`, with the same effect when it will not build.
//!
//! `ob lab coverage` is ob's own, not dukovo-lab's: it holds Phase 16's
//! commands, services, pages and routes to the tests that cover them
//! ([`coverage`]).

mod coverage;

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::account::{self, Account};
use crate::image::{self, Overrides};
use crate::process::{self, step};
use crate::rootfs::Owned;
use crate::{Args, Context as _, Ctx, Error, Result, VERSION, bail, baseline, bench, doctor, qemu};

/// The lab's account.
pub const ACCOUNT: &str = "lab";

/// Its full name, as `user list` shows it.
const FULL_NAME: &str = "Dukovo Lab";

/// dukovo-lab's configuration folder, under the home folder, when its
/// `--config` does not name another.
const CONFIG: &str = ".config/dukovo-lab";

/// The lab key's public half in that folder (`dukovo-lab key` makes it).
const PUBLIC_KEY: &str = "lab_ed25519.pub";

/// The lab password in that folder (`dukovo-lab key` makes it too):
/// the lab account's, which tests that run as root give elevate (ADR 0408).
const PASSWORD: &str = "lab_password";

/// What a lab image's console says before signing in, after the default
/// banner.
pub const BANNER: &str = "Test image of the Dukovo Lab: the lab account signs in over SSH \
                          with the lab's key. Never use it as a release.";

/// Where dukovo-lab comes from.
const LAB_REPO: &str = "https://github.com/dukovo-os/lab";

/// How dukovo-lab is installed when `ob lab` cannot find it.
const INSTALL: &str = "cargo install --locked --git https://github.com/dukovo-os/lab";

/// dukovo-lab's store, under the home folder, when its `--store` does not
/// name another.
const STORE: &str = ".local/share/dukovo-lab";

/// The private repository Send pushes findings to, over SSH and then
/// HTTPS: whichever way this PC's GitHub sign-in works.
const RESULTS_REPO: [&str; 2] =
    ["git@github.com:dukovo-os/lab-results.git", "https://github.com/dukovo-os/lab-results.git"];

/// What the lab runs on this PC, each with the Debian or Ubuntu package
/// that brings it.
const NEEDS: &[(&str, &str)] = &[
    ("qemu-system-x86_64", "qemu-system-x86"),
    ("qemu-img", "qemu-utils"),
    ("ssh", "openssh-client"),
    ("ssh-keygen", "openssh-client"),
    ("git", "git"),
    ("gzip", "gzip"),
];

/// What some tests run on this PC as the far end of what they check (their
/// `host_needs`: a file server, a client), each with the package that
/// brings it. A test is skipped where one is missing, so the lab only
/// notes what is.
const PEER_WANTS: &[(&str, &str)] = &[
    ("smbd", "samba"),
    ("pdbedit", "samba"),
    ("smbclient", "smbclient"),
    ("ganesha.nfsd", "nfs-ganesha nfs-ganesha-vfs"),
    ("mount.nfs4", "nfs-common"),
    ("nc", "netcat-openbsd"),
    ("wg", "wireguard-tools"),
    ("ip", "iproute2"),
    ("krb5kdc", "krb5-kdc"),
    ("kdb5_util", "krb5-kdc"),
    ("kadmin.local", "krb5-admin-server"),
    ("arecord", "alsa-utils"),
    ("python3", "python3"),
    ("age", "age"),
    ("restic", "restic"),
    ("rsyslogd", "rsyslog rsyslog-gnutls"),
    ("openssl", "openssl"),
    ("dhcpcd", "dhcpcd-base"),
    ("chronyd", "chrony"),
    ("chronyc", "chrony"),
    ("promtool", "prometheus"),
];

/// How a lab image lets the lab in: the lab's public key, for SSH, and
/// the hash of the lab's password, for elevate (ADR 0408). Without a
/// password the account's is one no one knows, and only tests that run
/// nothing as root pass.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Access {
    pub key: Key,
    password_hash: Option<String>,
}

impl Access {
    /// Reads the key from `key_file` (`--lab-key`) and the password from
    /// `password_file` (`--lab-password`), each else from dukovo-lab's
    /// configuration folder `config` (its `--config`), or from the folder
    /// it uses when given none. A password not given and not there is
    /// none; one given and not there is an error.
    pub fn read(
        key_file: Option<&str>,
        password_file: Option<&str>,
        config: Option<&str>,
    ) -> Result<Access> {
        let key = Key::read(key_file, config)?;
        let path = match password_file {
            Some(path) => PathBuf::from(path),
            None => config_dir(config)?.join(PASSWORD),
        };
        if password_file.is_none() && !path.exists() {
            eprintln!(
                "    note: no lab password at {}, so tests that run as root will refuse: \
                 `dukovo-lab key` makes it",
                path.display()
            );
            return Ok(Access { key, password_hash: None });
        }
        // The hash is all the image holds; the password stays on this PC.
        let mut text = std::fs::read(&path)
            .context(|| format!("reading the lab password {}", path.display()))?;
        let end = text.iter().position(|&b| b == b'\n').unwrap_or(text.len());
        let line = text[..end].strip_suffix(b"\r").unwrap_or(&text[..end]);
        let hashed = if line.is_empty() {
            Err(Error::new(format!(
                "{} is empty: remove it, and `dukovo-lab key` makes a new one",
                path.display()
            )))
        } else {
            account::hash_password(line)
        };
        accounts::wipe(&mut text);
        Ok(Access { key, password_hash: Some(hashed?) })
    }
}

/// dukovo-lab's configuration folder: `config` (its `--config`), or the
/// one it uses when given none.
fn config_dir(config: Option<&str>) -> Result<PathBuf> {
    match config {
        Some(config) => Ok(PathBuf::from(config)),
        None => {
            let home = std::env::var_os("HOME")
                .ok_or_else(|| Error::new("HOME is not set: give the key with --lab-key"))?;
            Ok(Path::new(&home).join(CONFIG))
        }
    }
}

/// The lab's public key, which a lab image trusts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Key {
    /// One `authorized_keys` line: `ssh-ed25519 <base64> <comment>`.
    line: String,
}

impl Key {
    /// Reads the key from `file` (`--lab-key`), or from dukovo-lab's
    /// configuration folder `config` (its `--config`), or from the folder
    /// it uses when given none.
    pub fn read(file: Option<&str>, config: Option<&str>) -> Result<Key> {
        let path = match file {
            Some(path) => PathBuf::from(path),
            None => config_dir(config)?.join(PUBLIC_KEY),
        };
        let text = std::fs::read_to_string(&path).context(|| {
            format!(
                "reading the lab's public key {}: run `dukovo-lab key` once to make it, \
                 or give one with --lab-key",
                path.display()
            )
        })?;
        Key::parse(&text).map_err(|why| Error::new(format!("{}: {why}", path.display())))
    }

    /// Checks that `text` is one Ed25519 public key, as `ssh-keygen` writes
    /// it. sshd passes over lines with options, so none is allowed here.
    fn parse(text: &str) -> std::result::Result<Key, String> {
        // A copy made on Windows ends its line with \r\n.
        let line = text.trim();
        if line.contains(['\n', '\r']) {
            return Err("holds more than one line; the lab key is one".into());
        }
        let mut words = line.split(' ');
        if words.next() != Some("ssh-ed25519") {
            return Err("is not an Ed25519 public key (ssh-ed25519 ...)".into());
        }
        let key = words.next().unwrap_or("");
        let base64 = |c: char| c.is_ascii_alphanumeric() || "+/=".contains(c);
        if key.len() < 60 || !key.chars().all(base64) {
            return Err("does not hold a public key after ssh-ed25519".into());
        }
        Ok(Key { line: line.into() })
    }
}

/// Adds the lab's account, home, key, sshd and banner to a root being
/// staged in `staging`, whose subvolumes are named as `ob image` names
/// them; `system` is its staged system slot, which is not changed. Gives
/// the home, which must be the account's.
pub fn stage(staging: &Path, system: &Path, access: &Access) -> Result<Owned> {
    let settings = staging.join("@settings");
    let key = &access.key;
    let hash = match &access.password_hash {
        Some(hash) => hash.clone(),
        None => account::unknown_password_hash()?,
    };
    let account = Account { name: ACCOUNT.into(), full_name: FULL_NAME.into(), hash };
    let (owner, group) = account::add(system, &settings, &staging.join("@state"), &account)?;

    // Its home, made as authd makes one (the folders a home starts with)
    // and with the key; given to the account as the root is written.
    let home = staging.join("@users").join(ACCOUNT);
    make_dir(&home, 0o700)?;
    for folder in accounts::HOME_FOLDERS {
        make_dir(&home.join(folder), if folder == "app-data" { 0o700 } else { 0o755 })?;
    }
    make_dir(&home.join(".ssh"), 0o700)?;
    write(&home.join(".ssh/authorized_keys"), &format!("{}\n", key.line), 0o600)?;

    // sshd is off by default (ADR 0347): an override turns it on, as
    // `service sshd enable` does, without changing the system's file.
    let overrides = settings.join("spark/services/settings.d");
    std::fs::create_dir_all(&overrides).context(|| format!("making {}", overrides.display()))?;
    write(
        &overrides.join("sshd.toml"),
        "# The lab image: the lab signs in over SSH (ADR 0401).\nenabled = true\n",
        0o644,
    )?;

    // The sign-in banner: the default's, then what this image is.
    let defaults = crate::userspace::DEFAULTS_DIR.strip_prefix("system/").expect("in the system");
    let issue = std::fs::read_to_string(system.join(defaults).join("issue")).unwrap_or_default();
    write(&settings.join("issue"), &format!("{issue}{BANNER}\n"), 0o644)?;

    Ok(Owned { path: Path::new("@users").join(ACCOUNT), owner, group })
}

fn make_dir(path: &Path, mode: u32) -> Result<()> {
    std::fs::create_dir_all(path).context(|| format!("making {}", path.display()))?;
    set_mode(path, mode)
}

fn write(path: &Path, text: &str, mode: u32) -> Result<()> {
    std::fs::write(path, text).context(|| format!("writing {}", path.display()))?;
    set_mode(path, mode)
}

fn set_mode(path: &Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
        .context(|| format!("setting the mode of {}", path.display()))
}

/// The dukovo-lab commands that test an image, and so need one built:
/// `run`, and `ui`, the runner page, which starts runs.
const NEEDS_IMAGE: &[&str] = &["run", "ui"];

/// dukovo-lab's options that take a value, which is not its command.
const VALUED: &[&str] = &[
    "--tests",
    "--config",
    "--store",
    "--image",
    "--machine",
    "--build",
    "--preset",
    "--port",
    "--limit",
    "--run",
    "--results",
    "--note",
    "--at",
    "--days",
    "--title",
    "--schedule",
    "--run-id",
    "--ob",
    "--browsers",
];

/// `ob lab`: runs dukovo-lab on `lab/tests`, building the lab image first
/// for the commands that need one.
pub fn command(ctx: &Ctx, args: &mut Args) -> Result<()> {
    let no_build = args.flag("--no-build");
    let given_image = args.value("--image")?;
    let key_file = args.value("--lab-key")?;
    let password_file = args.value("--lab-password")?;
    let profile = args.profile(&ctx.config)?;
    let mut rest = args.rest();
    rest.extend(args.take_passthrough());
    if wants_setup(&rest) {
        set_up(ctx, &rest)?;
        rest.push("ui".into());
    }
    // `ob lab attack` is a name for running the security range: it becomes
    // a dukovo-lab `run` of the `security` category (or the named tests in
    // it) on a freshly built image.
    if lab_command(&rest) == Some("attack") {
        rest = attack_to_run(&rest);
    }
    if lab_command(&rest) == Some("coverage") {
        let check = match rest.iter().filter(|word| *word != "coverage").collect::<Vec<_>>()[..] {
            [] => false,
            [word] if word == "--check" => true,
            ref other => bail!(
                "`ob lab coverage` takes only --check, not {}",
                other.iter().map(|word| format!("`{word}`")).collect::<Vec<_>>().join(" ")
            ),
        };
        return coverage::command(ctx, check);
    }
    let tool = find_tool()?;

    let mut lab = Command::new(&tool);
    lab.arg("--tests").arg(ctx.root.join("lab/tests"));
    if lab_command(&rest).is_some_and(|command| NEEDS_IMAGE.contains(&command)) {
        // An image given by hand has no stick or DVD of its own: one comes
        // only with --stick or --dvd.
        let built_here = given_image.is_none();
        let image = match given_image {
            Some(image) => PathBuf::from(image),
            None if no_build => ctx.out.join(image::file_name(image::Kind::Lab)),
            None => {
                // The key dukovo-lab will sign in with is the one in the
                // folder it is given, so the image trusts that one.
                let access = Access::read(
                    key_file.as_deref(),
                    password_file.as_deref(),
                    option_value(&rest, "--config"),
                )?;
                let overrides = Overrides { lab: Some(access), ..Overrides::default() };
                image::build_release(ctx, profile, image::Kind::Lab, &overrides)?
            }
        };
        if !image.is_file() {
            bail!("no lab image at {}: leave out --no-build to build one", ctx.display(&image));
        }
        lab.arg("--image").arg(image);
        lab.args(["--build", &format!("{VERSION} {}", bench::commit(ctx))]);
        // Without the kit, performance tests skip and say why; a kit that
        // will not build is no reason to run nothing.
        if !rest.iter().any(|arg| arg == "--linux") {
            let kit = if no_build {
                existing_kit(ctx)
            } else {
                linux_kit(ctx).map_err(|e| e.to_string())
            };
            match kit {
                Ok(kit) => {
                    lab.arg("--linux").arg(kit);
                }
                Err(why) => eprintln!("    note: performance tests will skip: {why}"),
            }
        }
        // The console's fonts, the kernel's and consoled's own, for the
        // steps that read the screen.
        if !rest.iter().any(|arg| arg == "--fonts") {
            lab.arg("--fonts").arg(ctx.root.join(CONSOLE_FONTS));
        }
        // Playwright's browsers and axe-core, which the web console's tests
        // drive pages with (ADR 0510).
        if !rest.iter().any(|arg| arg == "--browsers") {
            match browser_kit(ctx, no_build) {
                Ok(kit) => {
                    lab.arg("--browsers").arg(kit);
                }
                Err(why) => eprintln!("    note: the web console's tests will skip: {why}"),
            }
        }
        // ob itself, whose disc-drive holds a test's music CD in a QEMU
        // machine, as QEMU's own drives hold none (ADR 0447).
        if !rest.iter().any(|arg| arg == "--ob") {
            lab.arg("--ob").arg(ctx.root.join("ob"));
        }
        // The install stick of the same tree, for the installer's tests.
        if built_here && !rest.iter().any(|arg| arg == "--stick") {
            let stick = if no_build {
                let stick = ctx.out.join(image::file_name(image::Kind::Stick));
                if stick.is_file() {
                    Ok(stick)
                } else {
                    Err("no install stick was built yet, and --no-build builds none".to_string())
                }
            } else {
                step("Building the install stick, for the installer's tests");
                image::build_release(ctx, profile, image::Kind::Stick, &Overrides::default())
                    .map_err(|e| e.to_string())
            };
            match stick {
                Ok(stick) => {
                    lab.arg("--stick").arg(stick);
                }
                Err(why) => eprintln!("    note: the installer's tests will skip: {why}"),
            }
        }
        // The install DVD of the same tree, for the installer's tests that
        // start from a disc (ADR 0433).
        if built_here && !rest.iter().any(|arg| arg == "--dvd") {
            let dvd = if no_build {
                let dvd = ctx.out.join(image::file_name(image::Kind::Iso));
                if dvd.is_file() {
                    Ok(dvd)
                } else {
                    Err("no install DVD was built yet, and --no-build builds none".to_string())
                }
            } else {
                step("Building the install DVD, for the installer's tests");
                image::build_release(ctx, profile, image::Kind::Iso, &Overrides::default())
                    .map_err(|e| e.to_string())
            };
            match dvd {
                Ok(dvd) => {
                    lab.arg("--dvd").arg(dvd);
                }
                Err(why) => eprintln!("    note: the installer's DVD tests will skip: {why}"),
            }
        }
        // The test updates of the same tree, which the update tests copy
        // onto the machine from the lab PC (ADR 0451).
        if built_here {
            let updates = if no_build {
                let updates = ctx.out.join("lab/updates");
                if updates.is_dir() {
                    Ok(updates)
                } else {
                    Err("no test updates were built yet, and --no-build builds none".to_string())
                }
            } else {
                crate::test::lab_updates(ctx, profile).map_err(|e| e.to_string())
            };
            match updates {
                Ok(updates) => {
                    lab.env(UPDATES_ENV, updates);
                }
                Err(why) => eprintln!("    note: the update tests will skip: {why}"),
            }
        }
        // The catalog's bundles of the same tree, which the service
        // bundles' tests copy onto the machine from the lab PC (ADR 0491).
        // With an image given by hand, the bundles this tree packed
        // already are used, if there are any: packing takes long, and the
        // image is most often this tree's too.
        if built_here || crate::bundles::dir(ctx).is_dir() {
            let bundles = if no_build || !built_here {
                let bundles = crate::bundles::dir(ctx);
                if bundles.is_dir() {
                    Ok(bundles)
                } else {
                    Err("no bundles were packed yet, and --no-build packs none".to_string())
                }
            } else {
                step("Packing the catalog's bundles, for the lab's service bundle tests");
                crate::ports::build_and_pack(ctx, &[])
                    .map(|_| crate::bundles::dir(ctx))
                    .map_err(|e| e.to_string())
            };
            match bundles {
                Ok(bundles) => {
                    lab.env(BUNDLES_ENV, bundles);
                }
                Err(why) => eprintln!("    note: the service bundle tests will skip: {why}"),
            }
        }
    }
    lab.args(&rest);
    // dukovo-lab's own exit status says how the run went (0 passed, 1 a
    // test failed, 2 a bad request, 3 the lab broke): it is ob's too.
    let status = lab.status().context(|| format!("starting {}", tool.display()))?;
    match status.code() {
        Some(0) => Ok(()),
        Some(code) => std::process::exit(code),
        None => bail!("{} was stopped by a signal", tool.display()),
    }
}

/// The variable that names the folder of the test updates for the update
/// tests' host steps (`lab/tests/updates/copy-update.sh`): one repository
/// a test update, as `sysupdate stage` reads it.
const UPDATES_ENV: &str = "DUKOVO_LAB_UPDATES";

/// The variable that names the folder of the catalog's bundles, packed
/// from the same tree, for the service bundles' tests' host steps
/// (`lab/tests/services/copy-bundle.sh`).
const BUNDLES_ENV: &str = "DUKOVO_LAB_BUNDLES";

/// The console's fonts, which dukovo-lab reads the screen's text with:
/// the PSF files the kernel and consoled draw in.
const CONSOLE_FONTS: &str = "kernel/lib/fbcon/fonts";

/// The folder whose `package.json` pins the packages of the browser kit to
/// exact versions, and whose `package-lock.json` pins what they need.
const BROWSERS: &str = "lab/browsers";

/// The file in a Linux kit's folder that describes it, as dukovo-lab reads
/// it.
const KIT_FILE: &str = "kit.toml";

/// What Linux's init, linux-bench, prints once every benchmark has run.
const KIT_DONE: &str = "linux-bench: done";

/// Builds the Linux kit that performance tests hold Dukovo to, and gives
/// its folder, `out/lab-linux`, for dukovo-lab's `--linux`: the kernel
/// `ob bench` holds Radix to (`--linux-kernel`'s, else the pinned
/// baseline of ADR 0290), and an initramfs whose init is linux-bench,
/// which runs the POSIX benchmarks built for Linux, prints their lines on
/// the serial console and `linux-bench: done`, and powers off. So the lab
/// measures against the same Linux, with the same benchmark program, as
/// CI's `ob bench --gate`.
pub fn linux_kit(ctx: &Ctx) -> Result<PathBuf> {
    step("Building the Linux kit that performance tests hold Dukovo to");
    let dir = ctx.out.join("lab-linux");
    process::create_dir(&dir)?;
    let posix = bench::posix_for_linux(ctx, &dir)?;
    let (kernel, modules) = match bench::linux_kernel(None)? {
        Some(kernel) => (kernel, None),
        None => {
            let pinned = baseline::fetch(ctx)?;
            (pinned.kernel, Some(pinned.modules))
        }
    };
    let initrd = bench::linux_init(ctx, &dir, Some(&posix), None, &kernel, modules.as_deref())?;
    // dukovo-lab takes the kit's files from beside its kit.toml.
    let copy = dir.join("vmlinuz");
    std::fs::copy(&kernel, &copy).context(|| {
        format!("copying the Linux kernel {} to {}", kernel.display(), copy.display())
    })?;
    let initrd_name = initrd.file_name().map_or_default(|name| name.to_string_lossy());
    std::fs::write(dir.join(KIT_FILE), kit_file("vmlinuz", &initrd_name))
        .context(|| format!("writing {}", dir.join(KIT_FILE).display()))?;
    Ok(dir)
}

/// The kit's `kit.toml`, naming its kernel and initramfs, the files beside
/// it, in the form dukovo-lab's `--linux` reads.
fn kit_file(kernel: &str, initrd: &str) -> String {
    format!(
        "# The Linux that the lab's performance tests hold Dukovo to: `ob lab` makes\n\
         # this folder, and dukovo-lab boots it with --linux.\n\
         kernel = \"{kernel}\"\n\
         initrd = \"{initrd}\"\n\
         cmdline = \"{}\"\n\
         done = \"{KIT_DONE}\"\n",
        bench::LINUX_CMDLINE
    )
}

/// Makes the browser kit that the web console's tests drive pages with, and
/// gives its folder, `out/lab-browsers`, for dukovo-lab's `--browsers`:
/// Playwright and axe-core as `lab/browsers/package-lock.json` pins them,
/// installed with `npm ci`, and Playwright's three engines in the kit's
/// `browsers` folder. The environment may provide engines of its own with
/// `PLAYWRIGHT_BROWSERS_PATH`, as the cloud build machines do, and then none
/// are installed. A kit that holds the lock's packages is not made again,
/// and `--no-build` makes none.
fn browser_kit(ctx: &Ctx, no_build: bool) -> std::result::Result<PathBuf, String> {
    let source = ctx.root.join(BROWSERS);
    let kit = ctx.out.join("lab-browsers");
    let lock = std::fs::read(source.join("package-lock.json"))
        .map_err(|err| format!("reading {BROWSERS}/package-lock.json: {err}"))?;
    let made = kit.join("node_modules").is_dir()
        && std::fs::read(kit.join("package-lock.json")).is_ok_and(|made| made == lock);
    if no_build {
        return if made {
            Ok(kit)
        } else {
            Err("no browser kit was made yet, and --no-build makes none".into())
        };
    }
    // npx comes with npm, so one message serves both.
    let node_tool = |name: &str| {
        process::find_program(name)
            .ok_or_else(|| "npm is not installed: install Node.js 22 or later".to_string())
    };
    if !made {
        let npm = node_tool("npm")?;
        step("Making the browser kit that the web console's tests drive pages with");
        for file in ["package.json", "package-lock.json"] {
            process::copy(&source.join(file), &kit.join(file)).map_err(|err| err.to_string())?;
        }
        let installed = kit_command(ctx, Command::new(npm).arg("ci").current_dir(&kit));
        if installed.is_err() {
            // The copy of the lock says the packages are in; a failed
            // install left some of them out.
            let _ = std::fs::remove_file(kit.join("package-lock.json"));
        }
        installed?;
    }
    if std::env::var_os("PLAYWRIGHT_BROWSERS_PATH").is_none() {
        let npx = node_tool("npx")?;
        step("Installing the browsers that the web console's tests drive");
        let engines = ["playwright", "install", "chromium", "firefox", "webkit"];
        let browsers = kit.join("browsers");
        kit_command(
            ctx,
            Command::new(npx)
                .args(engines)
                .env("PLAYWRIGHT_BROWSERS_PATH", browsers)
                .current_dir(&kit),
        )?;
    }
    Ok(kit)
}

/// Runs `command` in the browser kit, quietly; when it fails, the last
/// lines of what it said.
fn kit_command(ctx: &Ctx, command: &mut Command) -> std::result::Result<(), String> {
    process::output(ctx, command).map(|_| ()).map_err(|err| {
        let said = err.to_string();
        let lines: Vec<&str> = said.lines().collect();
        lines[lines.len().saturating_sub(8)..].join("\n")
    })
}

/// The kit an earlier `ob lab` built, for `--no-build`.
fn existing_kit(ctx: &Ctx) -> std::result::Result<PathBuf, String> {
    let dir = ctx.out.join("lab-linux");
    if dir.join(KIT_FILE).is_file() {
        Ok(dir)
    } else {
        Err("no Linux kit was built yet, and --no-build builds none".into())
    }
}

/// Whether `args` hold no dukovo-lab command, nor ask for its help: plain
/// `ob lab`, which sets the lab up and opens the runner page.
fn wants_setup(args: &[String]) -> bool {
    let asks = |word: &String| matches!(word.as_str(), "--help" | "-h" | "--version" | "-V");
    lab_command(args).is_none() && !args.iter().any(asks)
}

/// Sets up what the lab needs and is not there yet, for plain `ob lab`:
/// the programs it runs, dukovo-lab itself (installed, or updated to the
/// newest), the lab key, and the lab-results clone that Send uses. `args`
/// are the options for dukovo-lab, whose `--config`, `--store` and
/// `--results` it follows.
fn set_up(ctx: &Ctx, args: &[String]) -> Result<()> {
    step("Checking what the lab needs on this PC");
    let missing = missing_needs();
    if !missing.is_empty() {
        let packages = packages(&missing);
        let programs: Vec<&str> = missing.iter().map(|(program, _)| *program).collect();
        return Err(Error::new(format!("the lab needs {} on this PC", programs.join(", "))).hint(
            format!(
                "on Linux Mint, Debian or Ubuntu: `sudo apt install {}`, then ./ob lab again",
                packages.join(" ")
            ),
        ));
    }
    if !qemu::kvm_available() {
        eprintln!(
            "    note: /dev/kvm is not usable, so tests run under QEMU's emulator, some four times slower"
        );
    }
    let wanted = missing_peer_wants();
    if !wanted.is_empty() {
        eprintln!(
            "    note: tests that use this PC as the machine's peer are skipped without {}: \
             `sudo apt install {}`",
            wanted.iter().map(|(program, _)| *program).collect::<Vec<_>>().join(", "),
            packages(&wanted).join(" ")
        );
    }

    // dukovo-lab: the newest from its repository. cargo builds it only
    // when there is a newer commit, and an update that fails (offline,
    // say) leaves the one installed.
    if std::env::var_os("OB_LAB").is_none() {
        let installed = find_tool().is_ok();
        step(if installed {
            "Updating dukovo-lab, if there is a newer one"
        } else {
            "Installing dukovo-lab, the lab's harness"
        });
        // With this tree's pinned toolchain, which ob setup installed.
        let mut cargo = Command::new("cargo");
        cargo
            .args(["install", "--locked", "--git", LAB_REPO])
            .env("RUSTUP_TOOLCHAIN", doctor::toolchain_channel(ctx)?);
        if installed {
            // Up to date is the usual answer, and cargo's words for it
            // ("Ignored package ... use --force") read like a problem.
            cargo.arg("--quiet");
        }
        match process::run(ctx, &mut cargo) {
            Ok(()) => {}
            Err(err) if installed => {
                eprintln!("    note: keeping the dukovo-lab already installed: {err}");
            }
            Err(err) => return Err(err),
        }
    }
    let tool = find_tool()?;

    // The lab key, which lab images trust.
    let config = match option_value(args, "--config") {
        Some(config) => PathBuf::from(config),
        None => home()?.join(CONFIG),
    };
    if !config.join("lab_ed25519").is_file() || !config.join(PASSWORD).is_file() {
        step("Making the lab key and password");
        let mut key = Command::new(&tool);
        key.arg("--config").arg(&config).arg("key");
        process::run(ctx, &mut key)?;
    }

    // The lab-results clone, without which Send is off. A clone that
    // fails is not the end: the page opens, and says Send needs it.
    let clone = match (option_value(args, "--results"), option_value(args, "--store")) {
        (Some(results), _) => PathBuf::from(results),
        (None, Some(store)) => Path::new(store).join("lab-results"),
        (None, None) => home()?.join(STORE).join("lab-results"),
    };
    if !clone.join(".git").exists() {
        step("Cloning lab-results, where Send puts findings");
        if let Some(parent) = clone.parent() {
            std::fs::create_dir_all(parent).context(|| format!("making {}", parent.display()))?;
        }
        // Over SSH first, quietly: no key for GitHub is no error, and
        // github.com's host key is taken on first use, as a person
        // answering yes would. Then over HTTPS, where git may ask for a
        // sign-in in the terminal.
        let mut ssh = Command::new("git");
        ssh.args(["clone", "-q", RESULTS_REPO[0]])
            .arg(&clone)
            .env("GIT_SSH_COMMAND", "ssh -o BatchMode=yes -o StrictHostKeyChecking=accept-new");
        let cloned = process::output(ctx, &mut ssh).is_ok() || {
            let mut https = Command::new("git");
            https.args(["clone", "-q", RESULTS_REPO[1]]).arg(&clone);
            process::run(ctx, &mut https).is_ok()
        };
        if !cloned {
            eprintln!(
                "    note: could not clone lab-results, so Send stays off. This PC's GitHub account \
                 needs push access to dukovo-os/lab-results, through an SSH key on GitHub or a \
                 sign-in git keeps (such as `gh auth login`). ./ob lab tries again next time."
            );
        }
    }
    // Send commits as the person on this PC, so git needs their name.
    let mut email = Command::new("git");
    email.args(["config", "user.email"]);
    if clone.join(".git").exists() && process::output(ctx, email.current_dir(&clone)).is_err() {
        eprintln!(
            "    note: git has no name and email to commit findings with, so Send will fail: \
             `git config --global user.name \"Your Name\"` and \
             `git config --global user.email you@example.org`"
        );
    }
    turn_on_schedules(ctx, &tool, args);
    // The lab network, for the real machines in the machine files: none
    // there, nothing done. One that cannot be set up stops only the real
    // machines' tests, which `dukovo-lab machines check` explains, not
    // QEMU's.
    if let Err(err) = crate::lab_net::set_up(ctx, &tool, &config) {
        eprintln!("    note: the lab network for real machines is not set up: {err}");
        if let Some(hint) = &err.hint {
            eprintln!("    {hint}");
        }
    }
    step("Building the lab image and opening the runner page");
    Ok(())
}

/// Puts in place the systemd timer that starts the lab's schedules
/// (`dukovo-lab schedule install`). It runs each time, so the timer
/// always names the dukovo-lab installed now; a PC where it cannot (no
/// systemd) still opens the page, and only its schedules wait.
fn turn_on_schedules(ctx: &Ctx, tool: &Path, args: &[String]) {
    let timer = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .or_else(|| home().ok().map(|home| home.join(".config")))
        .map(|config| config.join("systemd/user/dukovo-lab-schedules.timer"));
    if !timer.is_some_and(|timer| timer.is_file()) {
        step("Turning on the timer that starts schedules");
    }
    let mut install = Command::new(tool);
    for option in ["--config", "--store", "--results"] {
        if let Some(value) = option_value(args, option) {
            install.arg(option).arg(value);
        }
    }
    install.args(["schedule", "install"]);
    match process::output(ctx, &mut install) {
        // It says so when schedules can run only while someone is signed
        // in: that is worth passing on.
        Ok(output) => {
            for line in String::from_utf8_lossy(&output.stdout).lines().skip(1) {
                eprintln!("    note: {line}");
            }
        }
        Err(err) => eprintln!("    note: schedules cannot start by themselves on this PC: {err}"),
    }
}

/// The packages that bring `programs`, each once, in order.
fn packages<'a>(programs: &[(&str, &'a str)]) -> Vec<&'a str> {
    let mut packages: Vec<&str> = Vec::new();
    for package in programs.iter().flat_map(|(_, package)| package.split(' ')) {
        if !packages.contains(&package) {
            packages.push(package);
        }
    }
    packages
}

/// The programs in [`PEER_WANTS`] not on PATH, nor in the sbin folders an
/// ordinary account's PATH may leave out, as dukovo-lab looks for them.
fn missing_peer_wants() -> Vec<(&'static str, &'static str)> {
    let in_sbin = |program: &str| {
        ["/usr/local/sbin", "/usr/sbin", "/sbin"].iter().any(|dir| {
            let path = Path::new(dir).join(program);
            path.is_file()
        })
    };
    PEER_WANTS
        .iter()
        .copied()
        .filter(|(program, _)| process::find_program(program).is_none() && !in_sbin(program))
        .collect()
}

/// The programs in [`NEEDS`] not on PATH, and OVMF if it is not there.
fn missing_needs() -> Vec<(&'static str, &'static str)> {
    let mut missing: Vec<(&str, &str)> = NEEDS
        .iter()
        .copied()
        .filter(|(program, _)| process::find_program(program).is_none())
        .collect();
    if qemu::find_ovmf().is_none() {
        missing.push(("OVMF (UEFI firmware for QEMU)", "ovmf"));
    }
    missing
}

/// The home folder.
fn home() -> Result<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from).ok_or_else(|| {
        Error::new("HOME is not set: give dukovo-lab's folders with --config and --store")
    })
}

/// The value `args` give the dukovo-lab option `name`, if any; the last
/// one, as dukovo-lab takes the last.
fn option_value<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.windows(2).rev().find(|pair| pair[0] == name).map(|pair| pair[1].as_str())
}

/// Turns `ob lab attack [tests...]` into the dukovo-lab command it means:
/// `run` of the `security` category, or of the named tests in it. Options
/// are left as they are; a bare test name becomes `security/<name>`, and
/// one already under `security` is left alone. With no test named, the
/// whole `security` category runs.
fn attack_to_run(args: &[String]) -> Vec<String> {
    let mut out = Vec::with_capacity(args.len() + 1);
    let mut named = 0;
    let mut seen_command = false;
    let mut skip_value = false;
    for word in args {
        if skip_value {
            skip_value = false;
            out.push(word.clone());
            continue;
        }
        if VALUED.contains(&word.as_str()) {
            skip_value = true;
            out.push(word.clone());
        } else if word.starts_with('-') {
            out.push(word.clone());
        } else if !seen_command {
            // The `attack` word itself.
            seen_command = true;
            out.push("run".into());
        } else {
            // A test to run, named under the security category.
            named += 1;
            out.push(if word == "security" || word.starts_with("security/") {
                word.clone()
            } else {
                format!("security/{word}")
            });
        }
    }
    if named == 0 {
        out.push("security".into());
    }
    out
}

/// The dukovo-lab command among `args`: the first word that is neither
/// an option nor an option's value.
fn lab_command(args: &[String]) -> Option<&str> {
    let mut words = args.iter();
    while let Some(word) = words.next() {
        if VALUED.contains(&word.as_str()) {
            words.next();
        } else if !word.starts_with('-') {
            return Some(word);
        }
    }
    None
}

/// dukovo-lab: `OB_LAB`, or the first on PATH, or where `cargo install`
/// puts it (which is not on every PC's PATH).
fn find_tool() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("OB_LAB") {
        return Ok(PathBuf::from(path));
    }
    let path = std::env::var_os("PATH").unwrap_or_default();
    let cargo_bin = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| Path::new(&home).join(".cargo")))
        .map(|cargo| cargo.join("bin"));
    std::env::split_paths(&path)
        .chain(cargo_bin)
        .map(|dir| dir.join("dukovo-lab"))
        .find(|candidate| candidate.is_file())
        .ok_or_else(|| {
            Error::new(format!(
                "dukovo-lab, the lab's harness, is not installed: run ./ob lab, which installs it, or `{INSTALL}`"
            ))
        })
}

/// Whether dukovo-lab is installed, for `ob doctor`.
pub fn tool_found() -> Option<PathBuf> {
    find_tool().ok()
}

/// What `ob doctor` says when dukovo-lab is missing.
pub const DOCTOR_NOTE: &str = INSTALL;

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIGv8yKkzUYtD3Q5jzN7mKpWv0bL3Vx2m5C9E4fQwZr8T dukovo-lab\n";

    #[test]
    fn names_each_package_once() {
        let missing = [
            ("smbd", "samba"),
            ("pdbedit", "samba"),
            ("ganesha.nfsd", "nfs-ganesha nfs-ganesha-vfs"),
            ("ssh", "openssh-client"),
        ];
        assert_eq!(
            packages(&missing),
            ["samba", "nfs-ganesha", "nfs-ganesha-vfs", "openssh-client"]
        );
    }

    #[test]
    fn takes_one_ed25519_key() {
        assert_eq!(Key::parse(KEY).unwrap().line, KEY.trim_end());
        let windows = KEY.replace('\n', "\r\n");
        assert_eq!(Key::parse(&windows).unwrap().line, KEY.trim_end());
        for bad in [
            "",
            "ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABAQ lab",
            "ssh-ed25519",
            "command=\"x\" ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIGv8yKkzUYtD3Q5jzN7mKpWv0bL3Vx2m5C9E4fQwZr8T",
            &format!("{KEY}{KEY}"),
        ] {
            assert!(Key::parse(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn the_linux_kit_names_its_files_and_sends_linux_to_the_serial_port() {
        let kit = kit_file("vmlinuz", "linux-bench.cpio");
        assert!(kit.contains("kernel = \"vmlinuz\"\n"), "{kit}");
        assert!(kit.contains("initrd = \"linux-bench.cpio\"\n"), "{kit}");
        // dukovo-lab reads Linux's numbers from the first serial port, and
        // stops waiting at the line linux-bench prints last.
        assert!(bench::LINUX_CMDLINE.contains("console=ttyS0"));
        assert!(kit.contains(&format!("cmdline = \"{}\"\n", bench::LINUX_CMDLINE)), "{kit}");
        assert!(kit.contains("done = \"linux-bench: done\"\n"), "{kit}");
    }

    #[test]
    fn the_console_fonts_are_where_the_lab_reads_them() {
        // dukovo-lab reads the screen with every PSF font in the folder;
        // one moved elsewhere would leave the screen steps with none.
        let folder =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").join(CONSOLE_FONTS);
        let fonts = std::fs::read_dir(&folder)
            .unwrap()
            .filter(|entry| entry.as_ref().unwrap().path().extension().is_some_and(|e| e == "psfu"))
            .count();
        assert!(fonts > 0, "no .psfu fonts in {}", folder.display());
    }

    #[test]
    fn the_browser_kit_names_each_package_at_one_exact_version() {
        // A kit made a month later is the same kit only when no package
        // floats: no `^`, `~`, `*`, `x`, range or tag.
        let path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").join(BROWSERS).join("package.json");
        let text = std::fs::read_to_string(&path).unwrap();
        let package: serde_json::Value = serde_json::from_str(&text).unwrap();
        let packages = package["dependencies"].as_object().unwrap();
        assert!(packages.contains_key("playwright") && packages.contains_key("axe-core"));
        for (name, version) in packages {
            let version = version.as_str().unwrap();
            let numbers = version.split('.').collect::<Vec<_>>();
            let exact = numbers.len() == 3
                && numbers
                    .iter()
                    .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()));
            assert!(exact, "{name} is at \"{version}\", which is not one exact version");
        }
    }

    #[test]
    fn plain_ob_lab_sets_up_and_opens_the_page() {
        let words = |line: &str| line.split_whitespace().map(String::from).collect::<Vec<_>>();
        assert!(wants_setup(&words("")));
        assert!(wants_setup(&words("--store /tmp/lab --results /tmp/clone")));
        assert!(!wants_setup(&words("list")));
        assert!(!wants_setup(&words("--store /tmp/lab ui")));
        assert!(!wants_setup(&words("--help")));
        assert!(!wants_setup(&words("findings --run 20261107-153012-4f2a")));
    }

    #[test]
    fn attack_runs_the_security_category_or_its_named_tests() {
        let words = |line: &str| line.split(' ').map(String::from).collect::<Vec<_>>();
        // Plain `attack` runs the whole security category.
        assert_eq!(attack_to_run(&words("attack")), words("run security"));
        // A bare name is a test in that category; one already under it is left.
        assert_eq!(
            attack_to_run(&words("attack ssh-keys-only security/open-ports")),
            words("run security/ssh-keys-only security/open-ports")
        );
        // Options and their values are kept, and still no name means the
        // whole category.
        assert_eq!(
            attack_to_run(&words("--machine qemu attack --json")),
            words("--machine qemu run --json security")
        );
        // `security` itself names the category, not a test in it.
        assert_eq!(attack_to_run(&words("attack security")), words("run security"));
    }

    #[test]
    fn finds_the_command_past_options_and_their_values() {
        let words = |line: &str| line.split(' ').map(String::from).collect::<Vec<_>>();
        assert_eq!(lab_command(&words("run --all")), Some("run"));
        assert_eq!(lab_command(&words("--machine run --json run boot/")), Some("run"));
        assert_eq!(lab_command(&words("--json list")), Some("list"));
        assert_eq!(lab_command(&words("--machine qemu")), None);
        assert_eq!(lab_command(&words("--port 7421 ui")), Some("ui"));
        assert_eq!(lab_command(&words("--preset quick run")), Some("run"));
        assert_eq!(
            option_value(&words("run --config /a --all --config /b"), "--config"),
            Some("/b")
        );
        assert_eq!(option_value(&words("run --all"), "--config"), None);
    }

    #[test]
    fn reads_the_lab_password_as_its_hash_alone() {
        let config = std::env::temp_dir().join(format!("ob-lab-access-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&config);
        std::fs::create_dir_all(&config).unwrap();
        std::fs::write(config.join(PUBLIC_KEY), KEY).unwrap();
        let dir = config.to_str().unwrap();

        // No password file: the account keeps one no one knows.
        let access = Access::read(None, None, Some(dir)).unwrap();
        assert_eq!(access.password_hash, None);

        // A password file: its first line, hashed as authd hashes, and
        // nothing of the password itself.
        std::fs::write(config.join(PASSWORD), "Sesame0pen\n").unwrap();
        let hash = Access::read(None, None, Some(dir)).unwrap().password_hash.unwrap();
        assert!(hash.starts_with("$argon2id$"), "{hash}");
        assert!(!hash.contains("Sesame0pen"));
        assert_eq!(accounts::hash::verify(b"Sesame0pen", &hash), Ok(true));
        assert_eq!(accounts::hash::verify(b"Sesame0pen\n", &hash), Ok(false));

        // An empty file is an error, as is a --lab-password that is not there.
        std::fs::write(config.join(PASSWORD), "\n").unwrap();
        assert!(Access::read(None, None, Some(dir)).is_err());
        let missing = config.join("missing").to_str().unwrap().to_string();
        assert!(Access::read(None, Some(&missing), Some(dir)).is_err());
        std::fs::remove_dir_all(&config).unwrap();
    }

    #[test]
    fn stages_the_account_home_key_sshd_and_banner_beside_the_system() {
        let staging = std::env::temp_dir().join(format!("ob-lab-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&staging);
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let system = staging.join("@system.a");
        let defaults = system.join("resources/default-settings");
        for dir in [
            &defaults,
            &staging.join("@settings"),
            &staging.join("@state"),
            &staging.join("@users"),
        ] {
            std::fs::create_dir_all(dir).unwrap();
        }
        for name in ["passwd", "group", "issue"] {
            std::fs::copy(root.join("userspace/defaults").join(name), defaults.join(name)).unwrap();
        }
        let access = Access { key: Key::parse(KEY).unwrap(), password_hash: None };
        let owned = stage(&staging, &system, &access).unwrap();
        assert_eq!(owned, Owned { path: "@users/lab".into(), owner: 1000, group: 1000 });

        let read = |path: &str| std::fs::read_to_string(staging.join(path)).unwrap();
        assert!(read("@settings/passwd").contains("lab:x:1000:1000:Dukovo Lab:/users/lab:"));
        assert!(read("@settings/group").contains("admins:x:10:lab"));
        let hash = read("@state/authd/passwords");
        assert!(hash.starts_with("lab:$argon2id$"), "a password no one knows, not an empty one");
        assert_eq!(read("@users/lab/.ssh/authorized_keys"), KEY);
        assert!(
            read("@settings/spark/services/settings.d/sshd.toml").contains("\nenabled = true\n")
        );
        assert!(read("@settings/issue").ends_with(&format!("{BANNER}\n")));
        assert!(std::fs::metadata(staging.join("@users/lab/documents")).unwrap().is_dir());
        let mode = |path: &str| {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::metadata(staging.join(path)).unwrap().permissions().mode() & 0o7777
        };
        assert_eq!(
            [mode("@users/lab"), mode("@users/lab/.ssh"), mode("@users/lab/.ssh/authorized_keys")],
            [0o700, 0o700, 0o600]
        );
        // The system slot, which releases are made from, is as it was.
        let mut staged: Vec<_> =
            std::fs::read_dir(&defaults).unwrap().map(|e| e.unwrap().file_name()).collect();
        staged.sort();
        assert_eq!(staged, ["group", "issue", "passwd"]);
        assert!(!read("@system.a/resources/default-settings/issue").contains(BANNER));
        std::fs::remove_dir_all(&staging).unwrap();
    }
}

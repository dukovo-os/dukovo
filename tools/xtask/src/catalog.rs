// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `ob catalog`: makes and signs the catalog's TUF repository (ADR 0381)
//! with catalog-repo, which ob builds for this machine and runs with the
//! source tree's paths: the repository in `catalog/`, the YubiKeys through
//! `tools/yubikey-piv.py`, the bundles `ob ports` packs in `out/bundles/`,
//! their bytes waiting to be published in `out/catalog/store/`, and what
//! a server serves laid out in `out/catalog/site/`.
//!
//! It also sets up the update server and deploys to it (ADR 0382), over
//! the signer's own SSH login: the signed bundles exist only on the
//! machine that signed them, so they go from there.

use std::path::PathBuf;
use std::process::Command;

use crate::process::{self, step};
use crate::{Context, Ctx, Result, bail};

/// The repository's place in the source tree.
pub const REPO: &str = "catalog";

/// The YubiKey helper.
pub const YUBIKEY_HELPER: &str = "tools/yubikey-piv.py";

/// The update server's name, which its certificate is for and clients
/// fetch from.
pub const HOST: &str = "updates.dukovo.org";

/// Where the server serves the catalog.
pub const URL: &str = "https://updates.dukovo.org/catalog/";

/// The server's folder a deploy copies into (tools/update-server/).
const INCOMING: &str = "/var/lib/dukovo-catalog-incoming";

/// The online key's public half, as the server's setup leaves it.
const SERVER_ONLINE_KEY: &str = "/usr/local/share/dukovo-catalog/online.json";

/// How long a deploy waits for the server to publish.
const PUBLISH_WAIT_SECONDS: u32 = 120;

/// Runs catalog-repo with `args`, after the source tree's paths, or sets
/// up the server or deploys to it.
pub fn command(ctx: &Ctx, args: Vec<String>) -> Result<()> {
    match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["server", "setup"] => return server_setup(ctx),
        ["deploy"] => return deploy(ctx),
        ["check"] => return check(ctx, URL),
        ["check", url] => return check(ctx, url),
        ["server" | "deploy" | "check", ..] => {
            bail!("usage: ./ob catalog server setup | deploy | check [<url>]")
        }
        _ => {}
    }
    // `sign` signs the bundles `ob ports` packs unless it is named others,
    // so they are packed first, from this tree.
    if args.first().is_some_and(|word| word == "sign")
        && !args.iter().any(|arg| arg.ends_with(&format!(".{}", dkv::EXTENSION)))
    {
        crate::userspace::install_headers(ctx)?;
        crate::ports::build_and_pack(ctx, &[])?;
    }
    let mut cmd = catalog_repo(ctx)?;
    cmd.args(args);
    process::run(ctx, &mut cmd)
}

/// Builds catalog-repo for this machine and gives the command that runs
/// it with the source tree's paths.
fn catalog_repo(ctx: &Ctx) -> Result<Command> {
    let program = build(ctx, false)?;
    let mut cmd = Command::new(program);
    cmd.arg("--repo")
        .arg(ctx.root.join(REPO))
        .arg("--yubikey-helper")
        .arg(ctx.root.join(YUBIKEY_HELPER))
        .arg("--bundles")
        .arg(crate::bundles::dir(ctx))
        .arg("--store")
        .arg(ctx.out.join("catalog/store"))
        .arg("--out")
        .arg(ctx.out.join("catalog/site"));
    Ok(cmd)
}

/// Builds catalog-repo for this machine, optimised for the server's when
/// `release`, and gives its path.
fn build(ctx: &Ctx, release: bool) -> Result<PathBuf> {
    step("Building catalog-repo for this machine");
    let mut cmd = crate::userspace::host_cargo_of(ctx, "build", &["catalog-repo"]);
    cmd.args(["--bin", "catalog-repo"]);
    if release {
        cmd.arg("--release");
    }
    process::run(ctx, &mut cmd)?;
    let profile = if release { "release" } else { "debug" };
    Ok(ctx.target.join("programs-host").join(profile).join("catalog-repo"))
}

/// The server as `ssh` names it: `$OB_UPDATE_SSH`, such as
/// `user@updates.dukovo.org` or a name in ~/.ssh/config, or [`HOST`].
pub fn ssh_destination() -> String {
    std::env::var("OB_UPDATE_SSH")
        .ok()
        .filter(|dest| !dest.is_empty())
        .unwrap_or_else(|| HOST.into())
}

/// `ssh` to the server, running `script` with sh.
pub fn ssh(dest: &str, script: &str) -> Command {
    let mut cmd = Command::new("ssh");
    cmd.args(["--", dest, script]);
    cmd
}

/// Quotes `word` for the server's shell.
pub fn quote(word: &str) -> String {
    format!("'{}'", word.replace('\'', r"'\''"))
}

/// `ob catalog server setup` and `ob release server setup`: copies
/// tools/update-server/, catalog-repo and system-release to the server and
/// runs its setup.sh there through sudo, which asks for the password;
/// then keeps the public halves of the online keys it made, the
/// catalog's in catalog/keys/online.json and the system releases' in
/// releases/keys/online.json, for the next roots.
pub fn server_setup(ctx: &Ctx) -> Result<()> {
    let dest = ssh_destination();
    let program = build(ctx, true)?;
    let system_release = crate::release::tool_for(ctx, true)?;
    let staging = ctx.out.join("update-server");
    if staging.exists() {
        std::fs::remove_dir_all(&staging).context(|| format!("removing {}", staging.display()))?;
    }
    std::fs::create_dir_all(&staging).context(|| format!("making {}", staging.display()))?;
    let source = ctx.root.join("tools/update-server");
    for entry in std::fs::read_dir(&source).context(|| format!("reading {}", source.display()))? {
        let entry = entry.context(|| format!("reading {}", source.display()))?;
        std::fs::copy(entry.path(), staging.join(entry.file_name()))
            .context(|| format!("copying {}", entry.path().display()))?;
    }
    std::fs::copy(&program, staging.join("catalog-repo"))
        .context(|| format!("copying {}", program.display()))?;
    std::fs::copy(&system_release, staging.join("system-release"))
        .context(|| format!("copying {}", system_release.display()))?;

    // With scp, which every server with SSH has: setup.sh installs the
    // rsync that deploys use.
    step(&format!("Copying the server's files to {dest}"));
    process::run(ctx, &mut ssh(&dest, "rm -rf dukovo-update-server"))?;
    let mut cmd = Command::new("scp");
    cmd.args(["-q", "-r", "-p", "--"]).arg(&staging).arg(format!("{dest}:dukovo-update-server"));
    process::run(ctx, &mut cmd)?;

    step(&format!("Setting up {HOST} on {dest}: sudo asks for your password there"));
    let mut cmd = Command::new("ssh");
    cmd.args([
        "-t",
        "--",
        &dest,
        &format!("sudo sh dukovo-update-server/setup.sh {}", quote(HOST)),
    ]);
    process::run(ctx, &mut cmd)?;

    keep_online_key(
        ctx,
        &dest,
        SERVER_ONLINE_KEY,
        REPO,
        "sign a root that lists it with `./ob catalog root`",
    )?;
    keep_online_key(
        ctx,
        &dest,
        crate::release::SERVER_ONLINE_KEY,
        crate::release::REPO,
        "the release keys' next root names it, as releases/README.md says",
    )
}

/// Keeps the public half of an online key the server made, `server_path`
/// there, in `repo`/keys/online.json, or fails if another is there;
/// `then` says what the next root does with it.
fn keep_online_key(ctx: &Ctx, dest: &str, server_path: &str, repo: &str, then: &str) -> Result<()> {
    let output = process::output(ctx, &mut ssh(dest, &format!("cat {server_path}")))?;
    let path = ctx.root.join(repo).join("keys/online.json");
    match std::fs::read(&path) {
        Ok(kept) if kept == output.stdout => {
            println!("{repo}/keys/online.json is the server's online key.");
        }
        Ok(_) => bail!(
            "the server's online key is not {repo}/keys/online.json: the server was set up anew, \
             or the file was changed. A new root must list the server's key ({repo}/README.md \
             says how); it is {server_path} on the server"
        ),
        Err(_) => {
            let keys = ctx.root.join(repo).join("keys");
            std::fs::create_dir_all(&keys).context(|| format!("making {}", keys.display()))?;
            std::fs::write(&path, &output.stdout)
                .context(|| format!("writing {}", path.display()))?;
            println!(
                "Wrote the server's online key to {repo}/keys/online.json: commit it; {then}."
            );
        }
    }
    Ok(())
}

/// `ob catalog deploy`: copies the catalog's keys and metadata and the
/// signed bundles to the server, which publishes them with its online key
/// and says whether that worked; then reads them back over https.
fn deploy(ctx: &Ctx) -> Result<()> {
    let dest = ssh_destination();
    let repo = ctx.root.join(REPO);
    if !repo.join("metadata/root.json").is_file() {
        bail!("catalog/ has no root yet: sign one with `./ob catalog root` first");
    }
    let store = ctx.out.join("catalog/store");
    step(&format!("Copying the catalog to {dest}"));
    // Made first, so that the server's user may read them, whatever the
    // login's umask.
    let folders = format!("mkdir -p -m 2770 {INCOMING}/catalog {INCOMING}/store");
    process::run(ctx, &mut ssh(&dest, &folders))?;
    let mut cmd = Command::new("rsync");
    cmd.args(["-rtp", "--delete", "--chmod=D2770,F0660"])
        .arg(repo.join("keys"))
        .arg(repo.join("metadata"))
        .arg(format!("{dest}:{INCOMING}/catalog/"));
    process::run(ctx, &mut cmd)?;
    if store.join("bundles").is_dir() {
        // Bundles are never taken away from the server's store: a target
        // a client was told of stays there to be fetched.
        let mut cmd = Command::new("rsync");
        cmd.args(["-rtp", "--chmod=D2770,F0660"])
            .arg(store.join("bundles"))
            .arg(format!("{dest}:{INCOMING}/store/"));
        process::run(ctx, &mut cmd)?;
    }

    step("Publishing it on the server");
    let script = format!(
        "rm -f {INCOMING}/result && touch {INCOMING}/ready && \
         i=0; while [ ! -e {INCOMING}/result ] && [ $i -lt {PUBLISH_WAIT_SECONDS} ]; do \
         sleep 1; i=$((i + 1)); done; \
         if [ -e {INCOMING}/result ]; then cat {INCOMING}/result; else echo waiting; fi"
    );
    let output = process::output(ctx, &mut ssh(&dest, &script))?;
    let text = String::from_utf8_lossy(&output.stdout);
    let (status, rest) = text.split_once('\n').unwrap_or((text.trim_end(), ""));
    if !rest.trim().is_empty() {
        println!("{}", rest.trim_end());
    }
    match status {
        "published" => {}
        "waiting" => bail!(
            "the server did not publish in {PUBLISH_WAIT_SECONDS} seconds: \
             `systemctl status dukovo-catalog-publish` on the server says why"
        ),
        _ => bail!(
            "the server could not publish the catalog, as the output above says; it serves what it did before"
        ),
    }
    check(ctx, URL)
}

/// Makes a catalog signed with throwaway keys, for a boot test, in
/// `out/test/catalog`: `site/`, laid out as the update server serves it,
/// listing `bundles`, and `root.json`, its first root (ADR 0383). With
/// `advisories`, OSV advisories as JSON, it also lists the bundles' known
/// flaws, as the update server does each day (ADR 0389).
pub fn test_site(ctx: &Ctx, bundles: &[PathBuf], advisories: Option<&str>) -> Result<PathBuf> {
    let dir = ctx.out.join("test/catalog");
    if dir.exists() {
        std::fs::remove_dir_all(&dir).context(|| format!("removing {}", dir.display()))?;
    }
    std::fs::create_dir_all(&dir).context(|| format!("making {}", dir.display()))?;
    let program = build(ctx, false)?;
    step("Signing a test catalog with throwaway keys");
    let (repo, store, site) = (dir.join("repo"), dir.join("store"), dir.join("site"));
    let (test_key, online_key) = (dir.join("test.key"), dir.join("online.key"));
    let run = |args: &[&std::ffi::OsStr]| {
        let mut cmd = Command::new(&program);
        cmd.arg("--repo").arg(&repo).args(args);
        process::output(ctx, &mut cmd).map(drop)
    };
    let os = |text: &'static str| std::ffi::OsStr::new(text);
    run(&[os("key"), os("new"), test_key.as_os_str(), os("test"), os("root"), os("targets")])?;
    run(&[os("key"), os("new"), online_key.as_os_str(), os("online")])?;
    run(&[os("root"), os("--key"), test_key.as_os_str()])?;
    let mut sign = vec![os("sign"), os("--yes"), os("--key"), test_key.as_os_str()];
    sign.extend([os("--store"), store.as_os_str()]);
    sign.extend(bundles.iter().map(|b| b.as_os_str()));
    run(&sign)?;
    run(&[
        os("publish"),
        os("--key"),
        online_key.as_os_str(),
        os("--store"),
        store.as_os_str(),
        os("--out"),
        site.as_os_str(),
    ])?;
    if let Some(advisories) = advisories {
        let file = dir.join("advisories.json");
        std::fs::write(&file, advisories).context(|| format!("writing {}", file.display()))?;
        run(&[
            os("flaws"),
            os("--key"),
            online_key.as_os_str(),
            os("--store"),
            store.as_os_str(),
            os("--out"),
            site.as_os_str(),
            file.as_os_str(),
        ])?;
    }
    let root = dir.join("root.json");
    std::fs::copy(repo.join("metadata/1.root.json"), &root)
        .context(|| format!("writing {}", root.display()))?;
    Ok(dir)
}

/// `ob catalog check`: reads the catalog `url` serves as a client would,
/// from catalog/'s first root, and fetches every bundle.
fn check(ctx: &Ctx, url: &str) -> Result<()> {
    let mut cmd = catalog_repo(ctx)?;
    cmd.args(["check", url]);
    process::run(ctx, &mut cmd)
}

// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `ob catalog`: makes and signs the catalog's TUF repository (ADR 0381)
//! with catalog-repo, which ob builds for this machine and runs with the
//! source tree's paths: the repository in `catalog/`, the YubiKeys through
//! `tools/yubikey-piv.py`, the bundles `ob ports` packs in `out/bundles/`,
//! their bytes waiting to be published in `out/catalog/store/`, and what
//! a server serves laid out in `out/catalog/site/`.
//!
//! It also sets up the update server, an Ubuntu one or a Dukovo one, and
//! deploys to it (ADR 0382), over the signer's own SSH login: the signed
//! bundles exist only on the machine that signed them, so they go from
//! there, with the copying `deploy.rs` does. `rollout` pauses or frees
//! the waves a bundle's new version goes out in (ADR 0544), on the server
//! itself.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::deploy::{self, Server};
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

/// The server's folder a deploy copies into (tools/update-server/): the
/// same on an Ubuntu server and a Dukovo one, where `/var` is the
/// system's state.
const INCOMING: &str = "/var/lib/dukovo-catalog-incoming";

/// The folder of the bundles the server's store holds, whose names a
/// deploy skips; only an Ubuntu server's login may not read it, and then
/// the incoming folder's own copies are what is skipped.
const SERVED_BUNDLES: &str = "/var/lib/dukovo-catalog/store/bundles";

/// The online key's public half, as the server's setup leaves it: an
/// Ubuntu server's and a Dukovo server's.
const SERVER_ONLINE_KEYS: [&str; 2] =
    ["/usr/local/share/dukovo-catalog/online.json", "/var/lib/dukovo-catalog/online.json"];

/// How long a deploy waits for the server to publish: a Dukovo server
/// starts its publish job once a minute, and the job reads every bundle
/// back.
const PUBLISH_WAIT_SECONDS: u32 = 300;

/// Runs catalog-repo with `args`, after the source tree's paths, or sets
/// up the server or deploys to it.
pub fn command(ctx: &Ctx, args: Vec<String>) -> Result<()> {
    match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["server", "setup"] => return server_setup(ctx, false),
        ["server", "setup", "--dukovo"] => return server_setup(ctx, true),
        ["deploy"] => return deploy(ctx),
        ["check"] => return check(ctx, URL),
        ["check", url] => return check(ctx, url),
        ["withdraw", rest @ ..] => return withdraw(ctx, rest),
        ["rollout", "canary", ..] => bail!("{}", crate::release::CANARY_MOVED),
        ["rollout", rest @ ..] if !rest.is_empty() => return rollout(ctx, rest),
        ["server" | "deploy" | "check" | "rollout", ..] => bail!(
            "usage: ./ob catalog server setup [--dukovo] | deploy | check [<url>] | rollout \
             (pause <id> <version> <reason> | resume <id> <version>) | withdraw <id> <version> \
             --kind broken|malicious --reason <text>"
        ),
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
    let signing = args.first().is_some_and(|word| word == "sign");
    let mut cmd = catalog_repo(ctx)?;
    cmd.args(args);
    if signing {
        for id in verified_ids(ctx)? {
            cmd.args(["--verified", &id]);
        }
    }
    process::run(ctx, &mut cmd)
}

/// The IDs of the bundles the ports build. The catalog vouches for their
/// publisher itself, so a bundle of one of these IDs is signed as
/// verified (ADR 0592) and catalog-repo lists it as such; a bundle of any
/// other ID is not. Each bundle signed as verified is named in the
/// changes catalog-repo prints before it asks to sign.
fn verified_ids(ctx: &Ctx) -> Result<Vec<String>> {
    let recipes = crate::ports::recipes(ctx)?;
    Ok(recipes.into_iter().filter_map(|(recipe, _)| recipe.bundle.map(|b| b.id)).collect())
}

/// `ob catalog withdraw ID VERSION --kind K --reason TEXT`: signs the
/// next targets with that version, as the catalog lists it, withdrawn
/// (ADR 0593). Like `sign`, it needs the targets key and asks first; the
/// operator then deploys as after `sign`. The other words are
/// catalog-repo's: `--yes`, `--key`, `--days`.
fn withdraw(ctx: &Ctx, args: &[&str]) -> Result<()> {
    let [id, version, flags @ ..] = args else {
        bail!("{WITHDRAW_USAGE}");
    };
    if id.starts_with('-') || version.starts_with('-') {
        bail!("{WITHDRAW_USAGE}");
    }
    let mut cmd = catalog_repo(ctx)?;
    cmd.args(["withdraw", "--id", id, "--version", version]).args(flags);
    process::run(ctx, &mut cmd)
}

const WITHDRAW_USAGE: &str =
    "usage: ./ob catalog withdraw <id> <version> --kind broken|malicious --reason <text> [--yes]";

/// `ob catalog rollout ...`: on the update server, through sudo as its
/// catalog user, pauses or resumes the waves of a bundle's newest
/// version and signs the rollouts again (ADR 0544); needs no key of the
/// signer's. The canaries are changed with `ob release canary`.
fn rollout(ctx: &Ctx, args: &[&str]) -> Result<()> {
    let words: Vec<String> = args.iter().map(|word| deploy::quote(word)).collect();
    let [ubuntu, dukovo] = ROLLOUT_SCRIPTS;
    let words = words.join(" ");
    let script = format!(
        "if [ -x {ubuntu} ]; then sudo -u dukovo-catalog {ubuntu} {words}; \
         else elevate sh {dukovo} {words}; fi"
    );
    // A terminal, for sudo or elevate to ask for the password.
    process::run(ctx, &mut Server::from_environment().ssh_terminal(&script))
}

/// Where `rollout` finds the catalog's rollout script: on an Ubuntu
/// server, and on a Dukovo one, where a wrapper gives it the jobs' places.
const ROLLOUT_SCRIPTS: [&str; 2] = [
    "/usr/local/lib/dukovo-catalog/catalog-rollout.sh",
    "/var/lib/dukovo-update-server/catalog-rollout",
];

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
pub(crate) fn build(ctx: &Ctx, release: bool) -> Result<PathBuf> {
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

/// `ob catalog server setup` and `ob release server setup`: copies
/// tools/update-server/ and the programs it runs to the server and runs
/// its setup there as root, which asks for the password: setup.sh through
/// sudo on an Ubuntu server, and with `dukovo` the files of
/// tools/update-server/dukovo/ through elevate on a Dukovo one. Then it
/// keeps the public halves of the online keys it made, the catalog's in
/// catalog/keys/online.json and the system releases' in
/// releases/keys/online.json, for the next roots.
pub fn server_setup(ctx: &Ctx, dukovo: bool) -> Result<()> {
    let server = Server::from_environment();
    let staging = if dukovo { stage_dukovo(ctx)? } else { stage_ubuntu(ctx)? };

    copy_staging(ctx, &server, &staging)?;

    let setup = if dukovo {
        // A full path, as elevate runs the file from a folder of its own.
        format!("elevate sh \"$PWD/dukovo-update-server/dukovo/setup\" {}", deploy::quote(HOST))
    } else {
        format!("sudo sh dukovo-update-server/setup.sh {}", deploy::quote(HOST))
    };
    let asker = if dukovo { "elevate" } else { "sudo" };
    step(&format!("Setting up {HOST} on {}: {asker} asks for your password there", server.dest()));
    process::run(ctx, &mut server.ssh_terminal(&setup))?;

    keep_online_key(
        ctx,
        &server,
        &SERVER_ONLINE_KEYS,
        REPO,
        "sign a root that lists it with `./ob catalog root`",
    )?;
    keep_online_key(
        ctx,
        &server,
        &crate::release::SERVER_ONLINE_KEYS,
        crate::release::REPO,
        "the release keys' next root names it, as releases/README.md says",
    )
}

/// Copies the folder `staging` to `dukovo-update-server` in the login's
/// home on the server, with scp, which every server with ssh has: no
/// rsync is needed.
///
/// # Errors
///
/// If the server cannot be reached or does not take the files.
pub fn copy_staging(ctx: &Ctx, server: &Server, staging: &Path) -> Result<()> {
    step(&format!("Copying the server's files to {}", server.dest()));
    process::run(ctx, &mut server.ssh("rm -rf dukovo-update-server"))?;
    process::run(ctx, &mut server.scp(staging, "dukovo-update-server"))
}

/// Makes the folder an Ubuntu server's setup runs from, `out/update-server`:
/// the files of tools/update-server/, and catalog-repo and system-release
/// built for this machine, as the server is a Linux of its own.
fn stage_ubuntu(ctx: &Ctx) -> Result<PathBuf> {
    let program = build(ctx, true)?;
    let system_release = crate::release::tool_for(ctx, true)?;
    let staging = fresh_staging(ctx)?;
    copy_files(&ctx.root.join("tools/update-server"), &staging)?;
    let copied = staging.join("catalog-repo");
    std::fs::copy(&program, &copied).context(|| format!("copying {}", program.display()))?;
    let copied = staging.join("system-release");
    std::fs::copy(&system_release, &copied)
        .context(|| format!("copying {}", system_release.display()))?;
    Ok(staging)
}

/// Makes the folder a Dukovo server's setup runs from, `out/update-server`:
/// the files of tools/update-server/ and of its `dukovo/`, and catalog-repo
/// built for Radix. Dukovo's image has system-release already, as one of
/// systools' commands, but not catalog-repo, which is a program of the
/// build machine's: building it here keeps the server from needing a
/// compiler.
///
/// # Errors
///
/// If catalog-repo cannot be built for Radix or the files cannot be copied.
pub fn stage_dukovo(ctx: &Ctx) -> Result<PathBuf> {
    let program = crate::userspace::std_program(ctx, "catalog-repo")?;
    let staging = fresh_staging(ctx)?;
    let source = ctx.root.join("tools/update-server");
    copy_files(&source, &staging)?;
    let own = staging.join("dukovo");
    std::fs::create_dir_all(&own).context(|| format!("making {}", own.display()))?;
    copy_files(&source.join("dukovo"), &own)?;
    let copied = staging.join("catalog-repo");
    std::fs::copy(&program, &copied).context(|| format!("copying {}", program.display()))?;
    Ok(staging)
}

/// An empty `out/update-server`.
fn fresh_staging(ctx: &Ctx) -> Result<PathBuf> {
    let staging = ctx.out.join("update-server");
    if staging.exists() {
        std::fs::remove_dir_all(&staging).context(|| format!("removing {}", staging.display()))?;
    }
    std::fs::create_dir_all(&staging).context(|| format!("making {}", staging.display()))?;
    Ok(staging)
}

/// Copies the files of `from`, not its folders, into `to`, with their
/// modes.
fn copy_files(from: &Path, to: &Path) -> Result<()> {
    for entry in std::fs::read_dir(from).context(|| format!("reading {}", from.display()))? {
        let entry = entry.context(|| format!("reading {}", from.display()))?;
        if entry.file_type().context(|| format!("reading {}", entry.path().display()))?.is_file() {
            std::fs::copy(entry.path(), to.join(entry.file_name()))
                .context(|| format!("copying {}", entry.path().display()))?;
        }
    }
    Ok(())
}

/// Keeps the public half of an online key the server made, at the first
/// of `server_paths` that is there, in `repo`/keys/online.json, or fails
/// if another is there; `then` says what the next root does with it.
fn keep_online_key(
    ctx: &Ctx,
    server: &Server,
    server_paths: &[&str; 2],
    repo: &str,
    then: &str,
) -> Result<()> {
    let [ubuntu, dukovo] = server_paths;
    let output = process::output(
        ctx,
        &mut server.ssh(&format!("cat {ubuntu} 2>/dev/null || cat {dukovo}")),
    )?;
    let path = ctx.root.join(repo).join("keys/online.json");
    match std::fs::read(&path) {
        Ok(kept) if kept == output.stdout => {
            println!("{repo}/keys/online.json is the server's online key.");
        }
        Ok(_) => bail!(
            "the server's online key is not {repo}/keys/online.json: the server was set up anew, \
             or the file was changed. A new root must list the server's key ({repo}/README.md \
             says how); it is {ubuntu} on an Ubuntu server and {dukovo} on a Dukovo one"
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
    let repo = ctx.root.join(REPO);
    if !repo.join("metadata/root.json").is_file() {
        bail!("catalog/ has no root yet: sign one with `./ob catalog root` first");
    }
    publish_to(ctx, &Server::from_environment(), &repo, &ctx.out.join("catalog/store"))?;
    check(ctx, URL)
}

/// Copies the catalog repository `repo` and the bundles of `store` to
/// `server` and has it publish them: [`deploy`] copies this tree's, and
/// the boot tests those of a catalog of their own.
///
/// # Errors
///
/// If the server does not take them or could not publish them.
pub fn publish_to(ctx: &Ctx, server: &Server, repo: &Path, store: &Path) -> Result<()> {
    step(&format!("Copying the catalog to {}", server.dest()));
    deploy::replace(
        ctx,
        server,
        repo,
        &["keys", "metadata"],
        INCOMING,
        &format!("{INCOMING}/catalog"),
    )?;
    let bundles = store.join("bundles");
    if bundles.is_dir() {
        // Bundles are never taken away from the server's store: a target
        // a client was told of stays there to be fetched.
        deploy::add_missing(
            ctx,
            server,
            &bundles,
            deploy::is_bundle_name,
            INCOMING,
            &format!("{INCOMING}/store/bundles"),
            &[SERVED_BUNDLES],
        )?;
    }
    deploy::publish(
        ctx,
        server,
        INCOMING,
        PUBLISH_WAIT_SECONDS,
        "catalog",
        "dukovo-catalog-publish",
    )
}

/// Makes a catalog signed with throwaway keys, for a boot test, in
/// `out/test/catalog`: `site/`, laid out as the update server serves it,
/// listing `bundles`, and `root.json`, its first root (ADR 0383). With
/// `advisories`, OSV advisories as JSON, it also lists the bundles' known
/// flaws, as the update server does each day (ADR 0389). With `paused`,
/// a bundle's ID, a version of it and a reason, it also signs the
/// rollouts of the releases `begun`, each `ID/VERSION`, with the waves
/// just begun, and the waves of that one held, as the server's plan would
/// be after `ob catalog rollout pause` (ADR 0544). A release the first
/// rollouts list without a plan has every wave open, so those are planned
/// before the first are signed. A bundle whose ID is one of `verified` is
/// listed as one whose publisher the catalog vouches for (ADR 0592).
pub fn test_site(
    ctx: &Ctx,
    bundles: &[PathBuf],
    advisories: Option<&str>,
    paused: Option<(&str, &str, &str)>,
    begun: &[String],
    verified: &[&str],
) -> Result<PathBuf> {
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
    for id in verified {
        sign.extend([os("--verified"), std::ffi::OsStr::new(id)]);
    }
    sign.extend(bundles.iter().map(|b| b.as_os_str()));
    run(&sign)?;
    let plan = dir.join("rollout.json");
    if paused.is_some() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_secs());
        let releases: serde_json::Map<String, serde_json::Value> = begun
            .iter()
            .map(|release| (release.clone(), serde_json::json!({ "released": now, "open": 0 })))
            .collect();
        let text = serde_json::json!({ "releases": releases }).to_string();
        std::fs::write(&plan, text).context(|| format!("writing {}", plan.display()))?;
    }
    let mut publish = vec![
        os("publish"),
        os("--key"),
        online_key.as_os_str(),
        os("--store"),
        store.as_os_str(),
        os("--out"),
        site.as_os_str(),
    ];
    if paused.is_some() {
        publish.extend([os("--plan"), plan.as_os_str()]);
    }
    run(&publish)?;
    if let Some((id, version, reason)) = paused {
        let word = std::ffi::OsStr::new;
        run(&[
            os("rollout"),
            os("--key"),
            online_key.as_os_str(),
            os("--store"),
            store.as_os_str(),
            os("--out"),
            site.as_os_str(),
            os("--plan"),
            plan.as_os_str(),
            os("pause"),
            word(id),
            word(version),
            word(reason),
        ])?;
    }
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

/// Withdraws a version in the test catalog `test_site` made, as
/// `ob catalog withdraw` does for the real one (ADR 0593): the targets
/// are signed again with the test key, with `version` of `id`, the
/// target's full ID, withdrawn as `kind`, `broken` or `malicious`, for
/// `reason`; then the site is published again, so that a machine that
/// reads it sees the new targets. The rollout plan `test_site` kept, if
/// it made one, is signed again as it was.
pub fn test_withdraw(ctx: &Ctx, id: &str, version: &str, kind: &str, reason: &str) -> Result<()> {
    let dir = ctx.out.join("test/catalog");
    let program = build(ctx, false)?;
    step(&format!("Withdrawing {id} {version} in the test catalog"));
    let (repo, store, site) = (dir.join("repo"), dir.join("store"), dir.join("site"));
    let (test_key, online_key, plan) =
        (dir.join("test.key"), dir.join("online.key"), dir.join("rollout.json"));
    let run = |args: &[&std::ffi::OsStr]| {
        let mut cmd = Command::new(&program);
        cmd.arg("--repo").arg(&repo).args(args);
        process::output(ctx, &mut cmd).map(drop)
    };
    let word = std::ffi::OsStr::new;
    run(&[
        word("withdraw"),
        word("--yes"),
        word("--key"),
        test_key.as_os_str(),
        word("--store"),
        store.as_os_str(),
        word("--id"),
        word(id),
        word("--version"),
        word(version),
        word("--kind"),
        word(kind),
        word("--reason"),
        word(reason),
    ])?;
    let mut publish = vec![
        word("publish"),
        word("--key"),
        online_key.as_os_str(),
        word("--store"),
        store.as_os_str(),
        word("--out"),
        site.as_os_str(),
    ];
    if plan.exists() {
        publish.extend([word("--plan"), plan.as_os_str()]);
    }
    run(&publish)
}

/// Makes an online key as the update server's setup does, for a boot
/// test that brings keys to a server as a move does: `catalog.key`, the
/// secret half, and `catalog.json`, the public half, in `dir`, which is
/// made new.
///
/// # Errors
///
/// If catalog-repo cannot be built or run, or the files cannot be written.
pub fn test_online_key(ctx: &Ctx, dir: &Path) -> Result<(PathBuf, PathBuf)> {
    if dir.exists() {
        std::fs::remove_dir_all(dir).context(|| format!("removing {}", dir.display()))?;
    }
    std::fs::create_dir_all(dir).context(|| format!("making {}", dir.display()))?;
    let program = build(ctx, false)?;
    let (secret, public) = (dir.join("catalog.key"), dir.join("catalog.json"));
    let mut cmd = Command::new(program);
    cmd.arg("--repo").arg(dir.join("new")).args(["key", "new"]).arg(&secret);
    cmd.args(["online", "snapshot", "timestamp"]);
    process::output(ctx, &mut cmd)?;
    std::fs::copy(dir.join("new/keys/online.json"), &public)
        .context(|| format!("writing {}", public.display()))?;
    Ok((secret, public))
}

/// Makes a catalog repository for a boot test that deploys it to an
/// update server of its own, in `dir`: `repo/`, whose keys and metadata
/// throwaway keys signed and whose root names the server's online key,
/// the public half of which is the file `online`, and `store/`, which
/// holds `bundles` as signed. Nothing is published: the server does that
/// with its own key, as it does for [`deploy`]. The repository and the
/// store.
///
/// # Errors
///
/// If catalog-repo cannot be built or run, or the files cannot be written.
pub fn test_repo(
    ctx: &Ctx,
    dir: &Path,
    bundles: &[PathBuf],
    online: &Path,
) -> Result<(PathBuf, PathBuf)> {
    if dir.exists() {
        std::fs::remove_dir_all(dir).context(|| format!("removing {}", dir.display()))?;
    }
    std::fs::create_dir_all(dir).context(|| format!("making {}", dir.display()))?;
    let program = build(ctx, false)?;
    step("Signing a test catalog for the update server");
    let (repo, store, test_key) = (dir.join("repo"), dir.join("store"), dir.join("test.key"));
    let run = |args: &[&std::ffi::OsStr]| {
        let mut cmd = Command::new(&program);
        cmd.arg("--repo").arg(&repo).args(args);
        process::output(ctx, &mut cmd).map(drop)
    };
    let os = |text: &'static str| std::ffi::OsStr::new(text);
    run(&[os("key"), os("new"), test_key.as_os_str(), os("test"), os("root"), os("targets")])?;
    // The root lists the server's key where `key new online` would have
    // put one.
    let keys = repo.join("keys/online.json");
    std::fs::copy(online, &keys).context(|| format!("copying {}", online.display()))?;
    run(&[os("root"), os("--key"), test_key.as_os_str()])?;
    let mut sign = vec![os("sign"), os("--yes"), os("--key"), test_key.as_os_str()];
    sign.extend([os("--store"), store.as_os_str()]);
    sign.extend(bundles.iter().map(|b| b.as_os_str()));
    run(&sign)?;
    Ok((repo, store))
}

/// Runs `catalog-repo check` for the catalog in `repo`, built for this
/// machine, against `url`, as a client reads it from the repository's first
/// root, and gives what it said.
///
/// # Errors
///
/// If catalog-repo cannot be built, or the client refuses what `url`
/// serves.
pub fn check_test_repo(ctx: &Ctx, repo: &Path, url: &str) -> Result<String> {
    let program = build(ctx, false)?;
    let mut cmd = Command::new(program);
    cmd.arg("--repo").arg(repo).args(["check", url]);
    let output = process::output(ctx, &mut cmd)?;
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// `ob catalog check`: reads the catalog `url` serves as a client would,
/// from catalog/'s first root, and fetches every bundle.
fn check(ctx: &Ctx, url: &str) -> Result<()> {
    let mut cmd = catalog_repo(ctx)?;
    cmd.args(["check", url]);
    process::run(ctx, &mut cmd)
}

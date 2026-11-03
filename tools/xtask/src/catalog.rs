// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `ob catalog`: makes and signs the catalog's TUF repository (ADR 0381)
//! with catalog-repo, which ob builds for this machine and runs with the
//! source tree's paths: the repository in `catalog/`, the YubiKeys through
//! `tools/yubikey-piv.py`, the bundles `ob ports` packs in `out/bundles/`,
//! their bytes waiting to be published in `out/catalog/store/`, and what
//! a server serves laid out in `out/catalog/site/`.

use std::process::Command;

use crate::process::{self, step};
use crate::{Ctx, Result};

/// The repository's place in the source tree.
pub const REPO: &str = "catalog";

/// The YubiKey helper.
pub const YUBIKEY_HELPER: &str = "tools/yubikey-piv.py";

/// Runs catalog-repo with `args`, after the source tree's paths.
pub fn command(ctx: &Ctx, args: Vec<String>) -> Result<()> {
    // `sign` signs the bundles `ob ports` packs unless it is named others,
    // so they are packed first, from this tree.
    if args.first().is_some_and(|word| word == "sign")
        && !args.iter().any(|arg| arg.ends_with(&format!(".{}", dkv::EXTENSION)))
    {
        crate::userspace::install_headers(ctx)?;
        crate::ports::build_and_pack(ctx, &[])?;
    }
    step("Building catalog-repo for this machine");
    let mut cmd = crate::userspace::host_cargo_of(ctx, "build", &["catalog-repo"]);
    cmd.args(["--bin", "catalog-repo"]);
    process::run(ctx, &mut cmd)?;
    let program = ctx.target.join("programs-host/debug/catalog-repo");
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
        .arg(ctx.out.join("catalog/site"))
        .args(args);
    process::run(ctx, &mut cmd)
}

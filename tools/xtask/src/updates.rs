// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `ob updates`: what the update server serves, read as a new computer
//! reads it: the catalog with `catalog-repo served`, and the system's
//! releases with `system-release served`, both built for this machine and
//! run with `--json`. `ob updates status` puts the two together, with what
//! needs the owner: signatures that run out, which only the YubiKeys can
//! sign again, a server whose own signing has stopped, and rollouts that
//! are held. Dukovo Ops reads its `--json`.

use std::process::Command;

use serde_json::{Value, json};

use crate::process;
use crate::{Ctx, Result, bail, catalog, release};

/// How many days before a root or targets role expires it needs signing
/// again: `catalog_repo::WARN_DAYS`, which ob does not depend on.
const WARN_DAYS: i64 = 30;

/// Seconds in a day.
const DAY: i64 = 86_400;

/// Why the system's releases have nothing to read yet.
const NOT_SET_UP: &str = "the first root of the system's releases is not signed yet (ADR 0457)";

/// What reading one side of the update server gave.
#[derive(Debug, Clone, PartialEq)]
pub enum Side {
    /// What the program printed with `served --json`.
    Read(Value),
    /// The program failed, with the last line it said.
    Failed(String),
    /// There is nothing to read from yet.
    NotSetUp,
}

/// Something the owner has to look at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Need {
    /// `catalog` or `system`.
    pub area: &'static str,
    /// `red` for what is broken or has run out, `amber` for what is soon
    /// or held.
    pub level: &'static str,
    /// One plain sentence.
    pub text: String,
    /// What Dukovo Ops offers to do about it.
    pub action: &'static str,
}

impl Need {
    fn json(&self) -> Value {
        json!({ "area": self.area, "level": self.level, "text": self.text, "action": self.action })
    }
}

/// `./ob updates status [--json]`.
pub fn command(ctx: &Ctx, args: &[String]) -> Result<()> {
    let json = match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["status"] => false,
        ["status", "--json"] => true,
        _ => bail!("usage: ./ob updates status [--json]"),
    };
    let catalog = read_catalog(ctx)?;
    let system = read_system(ctx)?;
    let now = now();
    let needs = needs_you(&catalog, &system, now);
    if json {
        println!("{}", status_json(now, &catalog, &system, &needs));
    } else {
        for line in summary(&catalog, &system, &needs) {
            println!("{line}");
        }
    }
    Ok(())
}

/// Seconds since 1970.
fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| i64::try_from(since.as_secs()).unwrap_or(i64::MAX))
}

/// Reads the catalog the server serves.
fn read_catalog(ctx: &Ctx) -> Result<Side> {
    let program = catalog::build(ctx, false)?;
    let mut cmd = Command::new(program);
    cmd.arg("--repo").arg(ctx.root.join(catalog::REPO)).args(["served", "--json", catalog::URL]);
    Ok(run_json(ctx, &mut cmd))
}

/// Reads the system's releases the server serves, if their first root is
/// signed.
fn read_system(ctx: &Ctx) -> Result<Side> {
    let root = ctx.root.join(release::REPO).join("metadata/1.root.json");
    if !root.is_file() {
        return Ok(Side::NotSetUp);
    }
    let program = release::tool(ctx)?;
    let mut cmd = Command::new(program);
    cmd.args(["served", "--json", "--root"]).arg(root).arg(release::URL);
    Ok(run_json(ctx, &mut cmd))
}

/// Runs `cmd`, which prints one JSON value, and gives it, or the last
/// line the program said when it failed.
fn run_json(ctx: &Ctx, cmd: &mut Command) -> Side {
    if ctx.verbose {
        eprintln!("+ {}", process::render(cmd));
    }
    let output = match cmd.output() {
        Ok(output) => output,
        Err(err) => return Side::Failed(format!("cannot run it: {err}")),
    };
    if !output.status.success() {
        return Side::Failed(last_line(&String::from_utf8_lossy(&output.stderr)));
    }
    match serde_json::from_slice(&output.stdout) {
        Ok(value) => Side::Read(value),
        Err(err) => Side::Failed(format!("it printed no JSON: {err}")),
    }
}

/// The last line of `text` that says something, without the program's
/// name before it.
fn last_line(text: &str) -> String {
    let line = text.lines().rev().find(|line| !line.trim().is_empty()).unwrap_or("it failed");
    let line = line.trim();
    match line.split_once(": ") {
        Some((program, rest)) if !program.contains(' ') && program.contains('-') => rest.into(),
        _ => line.into(),
    }
}

/// One side as `ob updates status --json` gives it.
fn side_json(side: &Side) -> Value {
    match side {
        Side::Read(Value::Object(served)) => {
            let mut object = serde_json::Map::new();
            object.insert("state".into(), "ok".into());
            object.extend(served.clone());
            Value::Object(object)
        }
        Side::Read(_) => json!({ "state": "error", "error": "it printed no JSON object" }),
        Side::Failed(error) => json!({ "state": "error", "error": error }),
        Side::NotSetUp => json!({ "state": "not_set_up", "why": NOT_SET_UP }),
    }
}

/// What `ob updates status --json` prints.
pub fn status_json(checked: i64, catalog: &Side, system: &Side, needs: &[Need]) -> Value {
    json!({
        "checked": checked,
        "catalog": side_json(catalog),
        "system": side_json(system),
        "needs_you": needs.iter().map(Need::json).collect::<Vec<_>>(),
    })
}

/// The array at `key` of `value`, empty if there is none.
fn array<'a>(value: &'a Value, key: &str) -> &'a [Value] {
    value[key].as_array().map_or(&[], Vec::as_slice)
}

/// What needs the owner, as of `now`: red before amber, each side in turn.
pub fn needs_you(catalog: &Side, system: &Side, now: i64) -> Vec<Need> {
    let mut needs = Vec::new();
    for (area, what, side) in
        [("catalog", "catalog", catalog), ("system", "system releases", system)]
    {
        match side {
            Side::NotSetUp => {}
            Side::Failed(error) => needs.push(Need {
                area,
                level: "red",
                text: format!("The {what} on the update server cannot be read: {error}"),
                action: "check-server",
            }),
            Side::Read(served) => needs.extend(side_needs(area, what, served, now)),
        }
    }
    // Stable: red first, each side's order kept.
    needs.sort_by_key(|need| need.level != "red");
    needs
}

/// What needs the owner in one side that was read.
fn side_needs(area: &'static str, what: &str, served: &Value, now: i64) -> Vec<Need> {
    let mut needs = Vec::new();
    for role in array(served, "roles") {
        let (Some(name), Some(expires)) = (role["role"].as_str(), role["expires"].as_i64()) else {
            continue;
        };
        let days = (expires - now).div_euclid(DAY);
        if matches!(name, "root" | "targets") {
            let (level, text) = if expires <= now {
                (
                    "red",
                    format!(
                        "The {name} signature of the {what} has run out. It needs the YubiKeys to \
                         sign it again."
                    ),
                )
            } else if days < WARN_DAYS {
                (
                    "amber",
                    format!(
                        "The {name} signature of the {what} runs out in {days} days. It needs \
                         the YubiKeys to sign it again."
                    ),
                )
            } else {
                continue;
            };
            needs.push(Need { area, level, text, action: "resign" });
        } else if expires <= now {
            needs.push(Need {
                area,
                level: "red",
                text: format!(
                    "The {name} signature of the {what} has run out: the server signs it again by \
                     itself, so its refresh job has stopped working."
                ),
                action: "check-server",
            });
        }
    }
    let mut held = |name: String, why: &Value| {
        if let Some(why) = why.as_str() {
            needs.push(Need {
                area,
                level: "amber",
                text: format!("Rollout of {name} is held: {why}"),
                action: "rollout",
            });
        }
    };
    for rollout in array(served, "rollouts") {
        let bundle = rollout["bundle"].as_str().unwrap_or("a bundle");
        let version = rollout["version"].as_str().unwrap_or_default();
        held(format!("{bundle} {version}").trim_end().into(), &rollout["held"]);
    }
    for release in array(served, "releases") {
        let version = release["version"].as_str().unwrap_or("a release");
        held(format!("Dukovo {version}"), &release["rollout"]["held"]);
    }
    for set in array(served, "firmware") {
        let version = set["version"].as_str().unwrap_or("a set");
        held(format!("firmware {version}"), &set["rollout"]["held"]);
    }
    // Bundles that computers can install with a serious known flaw: a new
    // version of each is the fix.
    let serious: Vec<String> = array(served, "bundles")
        .iter()
        .filter(|bundle| {
            array(bundle, "flaws")
                .iter()
                .any(|flaw| matches!(flaw["severity"].as_str(), Some("high" | "critical")))
        })
        .map(|bundle| {
            let id = bundle["id"].as_str().unwrap_or("a bundle");
            let version = bundle["version"].as_str().unwrap_or_default();
            format!("{id} {version}").trim_end().to_string()
        })
        .collect();
    if !serious.is_empty() {
        let shown = serious.iter().take(SHOWN_BUNDLES).cloned().collect::<Vec<_>>().join(", ");
        let more = serious.len().saturating_sub(SHOWN_BUNDLES);
        let more = if more > 0 { format!(" and {more} more") } else { String::new() };
        needs.push(Need {
            area,
            level: "amber",
            text: format!(
                "{} {} a known high or critical flaw: {shown}{more}",
                count(serious.len(), "bundle"),
                if serious.len() == 1 { "has" } else { "have" }
            ),
            action: "flaws",
        });
    }
    needs
}

/// How many bundles a need about flaws names before it counts the rest.
const SHOWN_BUNDLES: usize = 5;

/// `n` of `thing`, as `1 bundle` or `2 bundles`.
fn count(n: usize, thing: &str) -> String {
    if n == 1 { format!("1 {thing}") } else { format!("{n} {thing}s") }
}

/// The lines `ob updates status` prints without `--json`.
pub fn summary(catalog: &Side, system: &Side, needs: &[Need]) -> Vec<String> {
    let mut lines = Vec::new();
    lines.push(match catalog {
        Side::Read(served) => {
            let bundles = array(served, "bundles");
            let flawed = bundles.iter().filter(|b| !array(b, "flaws").is_empty()).count();
            format!("Catalog: {}, {flawed} with known flaws", count(bundles.len(), "bundle"))
        }
        Side::Failed(error) => format!("Catalog: cannot be read ({error})"),
        Side::NotSetUp => "Catalog: not set up".into(),
    });
    lines.push(match system {
        Side::Read(served) => {
            let releases = array(served, "releases");
            let newest = releases.last().and_then(|r| r["version"].as_str()).unwrap_or("none");
            format!("System: {}, newest {newest}", count(releases.len(), "release"))
        }
        Side::Failed(error) => format!("System: cannot be read ({error})"),
        Side::NotSetUp => format!("System: not set up yet: {NOT_SET_UP}"),
    });
    for need in needs {
        lines.push(format!("  {}: {}", need.level, need.text));
    }
    lines.push(match needs.len() {
        0 => "Updates: all well".into(),
        n => {
            format!("Updates: {} {}", n, if n == 1 { "thing needs you" } else { "things need you" })
        }
    });
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_800_000_000;

    fn role(name: &str, days: i64) -> Value {
        json!({ "role": name, "version": 1, "expires": NOW + days * DAY })
    }

    fn roles(days: [i64; 4]) -> Vec<Value> {
        ["root", "timestamp", "snapshot", "targets"]
            .iter()
            .zip(days)
            .map(|(name, days)| role(name, days))
            .collect()
    }

    fn well() -> Side {
        Side::Read(json!({ "roles": roles([300, 1, 5, 100]), "bundles": [], "rollouts": [] }))
    }

    #[test]
    fn nothing_needs_the_owner_when_all_is_signed_and_nothing_is_held() {
        assert_eq!(needs_you(&well(), &Side::NotSetUp, NOW), []);
        assert_eq!(needs_you(&well(), &well(), NOW), []);
    }

    #[test]
    fn a_side_that_cannot_be_read_is_red() {
        let needs = needs_you(&Side::Failed("timed out".into()), &Side::NotSetUp, NOW);
        assert_eq!(needs.len(), 1);
        assert_eq!(
            (needs[0].area, needs[0].level, needs[0].action),
            ("catalog", "red", "check-server")
        );
        assert!(needs[0].text.ends_with("timed out"));
    }

    #[test]
    fn root_and_targets_need_the_yubikeys_before_they_run_out() {
        let soon = Side::Read(json!({ "roles": roles([29, 1, 5, 200]) }));
        let needs = needs_you(&soon, &Side::NotSetUp, NOW);
        assert_eq!(needs.len(), 1);
        assert_eq!((needs[0].level, needs[0].action), ("amber", "resign"));
        assert!(needs[0].text.contains("root") && needs[0].text.contains("29 days"));
        let far = Side::Read(json!({ "roles": roles([30, 1, 5, 30]) }));
        assert_eq!(needs_you(&far, &Side::NotSetUp, NOW), []);
        let out = Side::Read(json!({ "roles": roles([300, 1, 5, 0]) }));
        let needs = needs_you(&Side::NotSetUp, &out, NOW);
        assert_eq!((needs[0].area, needs[0].level, needs[0].action), ("system", "red", "resign"));
        assert!(needs[0].text.contains("targets"));
    }

    #[test]
    fn the_roles_the_server_signs_itself_are_red_only_once_expired() {
        let stale = Side::Read(json!({
            "roles": [role("timestamp", -1), role("snapshot", 0), role("flaws", -3),
                      role("rollout", -1), role("root", 300), role("targets", 100)],
        }));
        let needs = needs_you(&stale, &Side::NotSetUp, NOW);
        assert_eq!(needs.len(), 4);
        assert!(needs.iter().all(|n| (n.level, n.action) == ("red", "check-server")));
        let fresh = Side::Read(json!({ "roles": [role("timestamp", 0)] }));
        assert_eq!(needs_you(&fresh, &Side::NotSetUp, NOW).len(), 1);
        let soon = Side::Read(json!({ "roles": [role("timestamp", 1), role("snapshot", 2)] }));
        assert_eq!(needs_you(&soon, &Side::NotSetUp, NOW), []);
    }

    #[test]
    fn held_rollouts_are_amber_and_name_what_is_held() {
        let catalog = Side::Read(json!({
            "roles": roles([300, 1, 5, 100]),
            "rollouts": [
                { "bundle": "org.dukovo.ripgrep", "version": "14.1.1", "wave": 1, "held": "paused: disk errors" },
                { "bundle": "org.dukovo.bat", "version": "0.25.0", "wave": 2, "held": null },
            ],
        }));
        let system = Side::Read(json!({
            "roles": roles([300, 1, 5, 100]),
            "releases": [
                { "version": "0.14.0", "withdrawn": null, "rollout": { "wave": 1, "held": "canary red" } },
                { "version": "0.13.0", "withdrawn": null, "rollout": null },
            ],
            "firmware": [{ "version": "2026.11.12", "rollout": { "wave": 0, "held": "paused: bad" } }],
        }));
        let needs = needs_you(&catalog, &system, NOW);
        let texts: Vec<&str> = needs.iter().map(|n| n.text.as_str()).collect();
        assert_eq!(
            texts,
            [
                "Rollout of org.dukovo.ripgrep 14.1.1 is held: paused: disk errors",
                "Rollout of Dukovo 0.14.0 is held: canary red",
                "Rollout of firmware 2026.11.12 is held: paused: bad",
            ]
        );
        assert!(needs.iter().all(|n| (n.level, n.action) == ("amber", "rollout")));
        assert_eq!(needs[0].area, "catalog");
        assert_eq!(needs[1].area, "system");
    }

    #[test]
    fn serious_known_flaws_are_amber_and_name_the_bundles() {
        let flaw = |severity: &str| json!({ "id": "X-1", "severity": severity });
        let catalog = Side::Read(json!({
            "roles": roles([300, 1, 5, 100]),
            "bundles": [
                { "id": "org.dukovo.a", "version": "1", "flaws": [flaw("low"), flaw("critical")] },
                { "id": "org.dukovo.b", "version": "2", "flaws": [flaw("medium")] },
                { "id": "org.dukovo.c", "version": "3", "flaws": [flaw("high")] },
                { "id": "org.dukovo.d", "version": "4", "flaws": [] },
            ],
        }));
        let needs = needs_you(&catalog, &Side::NotSetUp, NOW);
        assert_eq!(needs.len(), 1);
        assert_eq!((needs[0].level, needs[0].action), ("amber", "flaws"));
        assert_eq!(
            needs[0].text,
            "2 bundles have a known high or critical flaw: org.dukovo.a 1, org.dukovo.c 3"
        );
        let many: Vec<Value> = (0..7)
            .map(|n| json!({ "id": format!("b{n}"), "version": "1", "flaws": [flaw("high")] }))
            .collect();
        let catalog = Side::Read(json!({ "roles": roles([300, 1, 5, 100]), "bundles": many }));
        let needs = needs_you(&catalog, &Side::NotSetUp, NOW);
        assert!(needs[0].text.ends_with("b4 1 and 2 more"), "{}", needs[0].text);
    }

    #[test]
    fn red_comes_before_amber() {
        let held = Side::Read(json!({
            "roles": roles([300, 1, 5, 100]),
            "rollouts": [{ "bundle": "a", "version": "1", "wave": 0, "held": "x" }],
        }));
        let needs = needs_you(&held, &Side::Failed("down".into()), NOW);
        assert_eq!((needs[0].level, needs[1].level), ("red", "amber"));
    }

    #[test]
    fn the_sides_are_merged_with_their_state() {
        let catalog = Side::Read(json!({ "url": "u", "roles": [], "bundles": [] }));
        let needs = needs_you(&catalog, &Side::NotSetUp, NOW);
        let merged = status_json(NOW, &catalog, &Side::NotSetUp, &needs);
        assert_eq!(merged["checked"], NOW);
        assert_eq!(merged["catalog"]["state"], "ok");
        assert_eq!(merged["catalog"]["url"], "u");
        assert_eq!(merged["system"]["state"], "not_set_up");
        assert!(merged["system"]["why"].as_str().unwrap().contains("ADR 0457"));
        assert_eq!(merged["needs_you"], json!([]));

        let failed = Side::Failed("no route".into());
        let needs = needs_you(&failed, &catalog, NOW);
        let merged = status_json(NOW, &failed, &catalog, &needs);
        assert_eq!(merged["catalog"], json!({ "state": "error", "error": "no route" }));
        assert_eq!(merged["system"]["state"], "ok");
        assert_eq!(merged["needs_you"][0]["area"], "catalog");
        assert_eq!(merged["needs_you"][0]["level"], "red");
        assert_eq!(merged["needs_you"][0]["action"], "check-server");
    }

    #[test]
    fn the_summary_ends_with_one_result_line() {
        let lines = summary(&well(), &Side::NotSetUp, &[]);
        assert_eq!(lines.last().unwrap(), "Updates: all well");
        let needs = needs_you(&Side::Failed("down".into()), &Side::NotSetUp, NOW);
        let lines = summary(&Side::Failed("down".into()), &Side::NotSetUp, &needs);
        assert_eq!(lines.last().unwrap(), "Updates: 1 thing needs you");
        assert_eq!(lines.iter().filter(|l| l.starts_with("Updates:")).count(), 1);
    }

    #[test]
    fn the_last_line_of_stderr_loses_the_programs_name() {
        assert_eq!(
            last_line("x\ncatalog-repo: the client refuses u: expired\n\n"),
            "the client refuses u: expired"
        );
        assert_eq!(last_line("Error: no thing"), "Error: no thing");
        assert_eq!(last_line(""), "it failed");
    }
}

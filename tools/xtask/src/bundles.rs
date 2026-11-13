// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The catalog's bundles of ports (ADR 0370): a port whose recipe has a
//! `[bundle]` table installs into `$PORT_BUNDLE` instead of the sysroot,
//! and nothing of it reaches the image. ob gives that folder its
//! manifest, its SBOM and its licenses, signs it and packs it into
//! `out/bundles/<id>-<version>.dkv`, with the same code bundle(1) packs
//! with on Dukovo (the dkv crate, ADR 0222).
//!
//! The key that signs them is the build's: `out/bundles/build.key`, made
//! once, or the minisign secret key the file in `OB_BUNDLE_KEY` holds.
//! Dukovo trusts neither until someone says so; the catalog signs what it
//! publishes with its own keys (13e).

use std::fmt::Write as _;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use dkv::minisign::SecretKey;

use crate::ports::{Port, Ports};
use crate::process::{self, step};
use crate::{Context, Ctx, Error, Result};

/// Where ob packs bundles, beneath `out/`.
const DIR: &str = "bundles";

/// The environment variable that names the file of the key to sign with.
pub const KEY_VARIABLE: &str = "OB_BUNDLE_KEY";

/// The lowest Dukovo release the bundles run on: the one they were built
/// for.
const REQUIRES_OS: &str = "0.12";

/// The lowest release that reads what a service needs and when it is
/// ready (ADR 0470): a bundle whose services say so requires it.
const REQUIRES_OS_GRAPH: &str = "1.0";

/// The keys of a bundle's service that only [`REQUIRES_OS_GRAPH`] reads.
const GRAPH_KEYS: [&str; 5] = ["requires", "wants", "after", "give_up_after", "ready"];

/// The folder a bundle's port installs into, `$PORT_BUNDLE`: beside the
/// port's stamp, in `out/ports/<name>/bundle`, so that a cache keeps or
/// loses both together. Under target/, CI's Cargo cache brought back
/// part of it, and a bundle was packed without its program.
pub fn staging(ctx: &Ctx, name: &str) -> PathBuf {
    crate::ports::info_dir(ctx, name).join("bundle")
}

/// Where the `.dkv` files are.
pub fn dir(ctx: &Ctx) -> PathBuf {
    ctx.out.join(DIR)
}

/// The `.dkv` file of `port`'s bundle.
pub fn file(ctx: &Ctx, port: &Port) -> Option<PathBuf> {
    let spec = port.recipe.bundle.as_ref()?;
    Some(dir(ctx).join(format!("{}-{}.{}", spec.id, port.recipe.version, dkv::EXTENSION)))
}

/// Packs the bundle of every port that is one, and returns their files.
pub fn pack_all(ctx: &Ctx, ports: &Ports) -> Result<Vec<PathBuf>> {
    let bundles: Vec<&Port> = ports.ports.iter().filter(|p| p.recipe.bundle.is_some()).collect();
    if bundles.is_empty() {
        return Ok(Vec::new());
    }
    let key = signing_key(ctx)?;
    let mut files = Vec::new();
    for port in bundles {
        files.push(pack(ctx, ports, port, &key)?);
    }
    Ok(files)
}

/// Packs `port`'s bundle: a copy of what its build.sh installed, with
/// its manifest, SBOM and licenses, signed by `key`.
fn pack(ctx: &Ctx, ports: &Ports, port: &Port, key: &SecretKey) -> Result<PathBuf> {
    let recipe = &port.recipe;
    let spec = recipe.bundle.as_ref().expect("a bundle's port");
    let built = staging(ctx, &recipe.name);
    for command in spec.commands.iter().chain(&spec.programs) {
        if !built.join("bin").join(command).is_file() {
            return Err(Error::new(format!(
                "port {}: {} has no bin/{command} to pack",
                recipe.name,
                ctx.display(&built)
            ))
            .hint(format!("build it again with `./ob ports --rebuild {}`", recipe.name)));
        }
    }
    for (alias, program) in &spec.aliases {
        if !spec.commands.iter().chain(&spec.programs).any(|p| p == program) {
            return Err(Error::new(format!(
                "port {}: its command {alias} runs {program:?}, which is none of its commands \
                 or programs",
                recipe.name
            )));
        }
    }
    let folder = ctx.target.join("ports/packing").join(&recipe.name);
    if folder.exists() {
        std::fs::remove_dir_all(&folder).context(|| format!("removing {}", folder.display()))?;
    }
    process::create_dir(folder.parent().expect("the folder has a parent"))?;
    crate::ports::copy_tree(ctx, &built, &folder)?;
    let manifest = manifest(port);
    // The reader bundle(1) installs with must take it.
    dkv::signed::parse_manifest(manifest.as_bytes())
        .map_err(|why| Error::new(format!("port {}: its bundle's {why}", recipe.name)))?;
    std::fs::write(folder.join(dkv::MANIFEST), &manifest)
        .context(|| format!("writing {}", folder.join(dkv::MANIFEST).display()))?;
    let time = crate::image::source_date_epoch(ctx).and_then(|t| t.parse().ok()).unwrap_or(0);
    crate::licenses::bundle(ctx, ports, port, &folder, time)?;
    let created = crate::licenses::rfc3339(time);
    dkv::signed::sign_folder(&folder, key, time, &created)
        .map_err(|why| Error::new(format!("signing the bundle of port {}: {why}", recipe.name)))?;
    let out = file(ctx, port).expect("a bundle's port");
    process::create_dir(&dir(ctx))?;
    let partial = out.with_extension("dkv.partial");
    let mut writer = std::io::BufWriter::new(
        std::fs::File::create(&partial).context(|| format!("writing {}", partial.display()))?,
    );
    let checked = dkv::signed::pack(&folder, &mut writer)
        .map_err(|why| Error::new(format!("packing the bundle of port {}: {why}", recipe.name)))?;
    std::io::Write::flush(&mut writer).context(|| format!("writing {}", partial.display()))?;
    drop(writer);
    std::fs::rename(&partial, &out).context(|| format!("writing {}", out.display()))?;
    step(&format!(
        "Packed {} {} into {}: {} files, signed by {}",
        spec.id,
        recipe.version,
        ctx.display(&out),
        checked.files,
        checked.key.encoded()
    ));
    Ok(out)
}

/// A small bundle the boot tests make (ADR 0384): one program, a shell
/// script, and the permissions its manifest asks for, as JSON.
pub struct TestBundle {
    pub id: &'static str,
    pub version: &'static str,
    pub script: &'static str,
    pub permissions: &'static str,
    /// Its manifest's `services`, a JSON array's insides; empty for none.
    pub services: &'static str,
    /// Its manifest's `provides`, a JSON array's insides; empty for none.
    pub provides: &'static str,
    /// Its manifest's `line`, with `.NAME-version` as the file a project
    /// chooses it with (ADR 0387); empty for none.
    pub line: &'static str,
    /// The script of `NAMEpm`, a second command, which runs with your
    /// access and adds add-ons to the folder `NAME_HOME` names (ADR
    /// 0388); empty for none.
    pub add_ons: &'static str,
    /// Its manifest's `update.feed`, the address of its own feed, which
    /// the build key signs, `update.key` (ADR 0390); empty for none.
    pub feed: &'static str,
}

/// Packs `bundle` into out/test/bundles, signed by the build key, and
/// gives its file.
pub fn pack_test(ctx: &Ctx, bundle: &TestBundle) -> Result<PathBuf> {
    let name = bundle.id.rsplit('.').next().unwrap_or(bundle.id);
    let folder = ctx.target.join("test-bundles").join(format!("{}-{}", bundle.id, bundle.version));
    if folder.exists() {
        std::fs::remove_dir_all(&folder).context(|| format!("removing {}", folder.display()))?;
    }
    process::create_dir(&folder.join("bin"))?;
    let manifest = manifest::bundle::template(bundle.id, name, "Dukovo tests", "MIT")
        .replace("\"version\": \"0.1.0\"", &format!("\"version\": \"{}\"", bundle.version))
        .replace("\"permissions\": {}", &format!("\"permissions\": {}", bundle.permissions));
    let manifest = if bundle.line.is_empty() {
        manifest
    } else {
        manifest.replace(
            "  \"permissions\":",
            &format!(
                "  \"line\": \"{}\",\n  \"line_files\": [\".{name}-version\"],\n  \"permissions\":",
                bundle.line
            ),
        )
    };
    let manifest = if bundle.add_ons.is_empty() {
        manifest
    } else {
        let quote = |text: &str| format!("\"{text}\"");
        let path = |program: &str| format!("{{ \"path\": {{ \"x86_64\": \"bin/{program}\" }} }}");
        let pm = format!("{name}pm");
        manifest
            .replace(
                &format!("    {}: {}\n  }},", quote(name), path(name)),
                &format!(
                    "    {}: {},\n    {}: {}\n  }},",
                    quote(name),
                    path(name),
                    quote(&pm),
                    path(&pm)
                ),
            )
            .replace(
                "\"runs_with\": \"sandbox\" }\n  ],",
                &format!(
                    "\"runs_with\": \"sandbox\" }},\n    {{ \"name\": {pm}, \"program\": {pm}, \
                     \"runs_with\": \"your-access\" }}\n  ],",
                    pm = quote(&pm)
                ),
            )
            .replace(
                "  \"permissions\":",
                &format!(
                    "  \"add_ons\": {{ \"variables\": [\"{}_HOME\"], \"commands\": \"bin\" }},\n  \
                     \"permissions\":",
                    name.to_uppercase()
                ),
            )
    };
    let manifest = if bundle.services.is_empty() {
        manifest
    } else {
        manifest.replace(
            "  \"permissions\":",
            &format!("  \"services\": [{}],\n  \"permissions\":", bundle.services),
        )
    };
    let manifest = if bundle.provides.is_empty() {
        manifest
    } else {
        manifest.replace(
            "  \"permissions\":",
            &format!("  \"provides\": [{}],\n  \"permissions\":", bundle.provides),
        )
    };
    let key = signing_key(ctx)?;
    let manifest = if bundle.feed.is_empty() {
        manifest
    } else {
        manifest.replace(
            "  \"permissions\":",
            &format!(
                "  \"update\": {{ \"feed\": \"{}\", \"key\": \"{}\" }},\n  \"permissions\":",
                bundle.feed,
                key.public().encoded()
            ),
        )
    };
    dkv::signed::parse_manifest(manifest.as_bytes())
        .map_err(|why| Error::new(format!("test bundle {}: {why}", bundle.id)))?;
    std::fs::write(folder.join(dkv::MANIFEST), &manifest)
        .context(|| format!("writing {}", folder.join(dkv::MANIFEST).display()))?;
    let pm = format!("{name}pm");
    let programs = [(name, bundle.script), (pm.as_str(), bundle.add_ons)];
    for (program, script) in programs.into_iter().filter(|(_, script)| !script.is_empty()) {
        let program = folder.join("bin").join(program);
        std::fs::write(&program, script).context(|| format!("writing {}", program.display()))?;
        std::fs::set_permissions(&program, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .context(|| format!("writing {}", program.display()))?;
    }
    dkv::signed::sign_folder(&folder, &key, 0, &crate::licenses::rfc3339(0))
        .map_err(|why| Error::new(format!("signing test bundle {}: {why}", bundle.id)))?;
    let dir = ctx.out.join("test/bundles");
    process::create_dir(&dir)?;
    let out = dir.join(format!("{}-{}.{}", bundle.id, bundle.version, dkv::EXTENSION));
    let mut writer = std::io::BufWriter::new(
        std::fs::File::create(&out).context(|| format!("writing {}", out.display()))?,
    );
    dkv::signed::pack(&folder, &mut writer)
        .map_err(|why| Error::new(format!("packing test bundle {}: {why}", bundle.id)))?;
    std::io::Write::flush(&mut writer).context(|| format!("writing {}", out.display()))?;
    Ok(out)
}

/// Signs the update feed in `folder` with the build key, adding `files`,
/// with `bundle feed` of this machine's build of systools (ADR 0390).
pub fn sign_test_feed(ctx: &Ctx, folder: &Path, files: &[PathBuf]) -> Result<()> {
    signing_key(ctx)?;
    let key =
        std::env::var_os(KEY_VARIABLE).map_or_else(|| dir(ctx).join("build.key"), PathBuf::from);
    let mut build = crate::userspace::host_cargo_of(ctx, "build", &["systools"]);
    build.args(["--bin", "systools"]);
    process::run(ctx, &mut build)?;
    let program = ctx.target.join("programs-host/debug/systools");
    let mut cmd = std::process::Command::new(program);
    cmd.args(["bundle", "feed"]).arg(folder).args(files).arg("--key").arg(key);
    process::output(ctx, &mut cmd).map(drop)
}

/// The manifest of `port`'s bundle (manifest.json(5)): its identity from
/// the recipe, a program for each command, run with the access of
/// whoever runs it, as command-line tools are, and each alias a command
/// that runs another program by its own name.
pub fn manifest(port: &Port) -> String {
    let recipe = &port.recipe;
    let spec = recipe.bundle.as_ref().expect("a bundle's port");
    let mut o = String::new();
    let _ = writeln!(o, "{{");
    let _ = writeln!(o, "  \"manifest_version\": 1,");
    let _ = writeln!(o, "  \"id\": {},", json(&spec.id));
    let _ = writeln!(o, "  \"name\": {},", json(&spec.name));
    let _ = writeln!(o, "  \"version\": {},", json(&recipe.version));
    let _ = writeln!(o, "  \"summary\": {},", json(&sentence(&recipe.summary)));
    if let Some(description) = &spec.description {
        let _ = writeln!(o, "  \"description\": {},", json(description));
    }
    let _ = writeln!(o, "  \"publisher\": {{ \"name\": \"Dukovo catalog\" }},");
    let _ = writeln!(o, "  \"license\": {},", json(&recipe.license));
    let _ = writeln!(o, "  \"homepage\": {},", json(&recipe.homepage));
    let _ = writeln!(o, "  \"source\": {},", json(&source_url(port)));
    let graph = spec.services.iter().any(|service| {
        GRAPH_KEYS.iter().any(|key| service.contains_key(*key))
            || service.get("health").and_then(|health| health.get("command")).is_some()
    });
    let requires_os = if graph { REQUIRES_OS_GRAPH } else { REQUIRES_OS };
    let _ = writeln!(o, "  \"requires_os\": {},", json(requires_os));
    let _ = writeln!(o, "  \"architectures\": [\"x86_64\"],");
    let programs: Vec<String> = spec
        .commands
        .iter()
        .chain(&spec.programs)
        .map(|c| {
            format!(
                "    {}: {{ \"path\": {{ \"x86_64\": {} }} }}",
                json(c),
                json(&format!("bin/{c}"))
            )
        })
        .collect();
    let _ = writeln!(o, "  \"programs\": {{\n{}\n  }},", programs.join(",\n"));
    let commands: Vec<String> = spec
        .commands
        .iter()
        .map(|c| (c, c))
        .chain(&spec.aliases)
        .map(|(name, program)| {
            format!(
                "    {{ \"name\": {}, \"program\": {}, \"runs_with\": \"your-access\" }}",
                json(name),
                json(program)
            )
        })
        .collect();
    let _ = write!(o, "  \"commands\": [\n{}\n  ]", commands.join(",\n"));
    if !spec.services.is_empty() {
        let services: Vec<String> =
            spec.services.iter().map(|service| format!("    {}", table_json(service))).collect();
        let _ = write!(o, ",\n  \"services\": [\n{}\n  ]", services.join(",\n"));
    }
    if !spec.permissions.is_empty() {
        let permissions: Vec<String> = spec
            .permissions
            .iter()
            .map(|(name, request)| format!("    {}: {}", json(name), toml_json(request)))
            .collect();
        let _ = write!(o, ",\n  \"permissions\": {{\n{}\n  }}", permissions.join(",\n"));
    }
    if let Some(data) = &spec.data {
        let _ = write!(o, ",\n  \"data\": {}", table_json(data));
    }
    let _ = writeln!(o, "\n}}");
    o
}

/// A TOML value of a recipe as JSON, on one line: a recipe gives a
/// bundle's services and permissions in TOML, as manifest.json(5) has
/// them in JSON.
fn toml_json(value: &toml::Value) -> String {
    match value {
        toml::Value::String(text) => json(text),
        toml::Value::Integer(number) => number.to_string(),
        toml::Value::Float(number) => number.to_string(),
        toml::Value::Boolean(yes) => yes.to_string(),
        toml::Value::Datetime(time) => json(&time.to_string()),
        toml::Value::Array(items) => {
            let items: Vec<String> = items.iter().map(toml_json).collect();
            format!("[{}]", items.join(", "))
        }
        toml::Value::Table(table) => table_json(table),
    }
}

/// A TOML table as a JSON object, on one line.
fn table_json(table: &toml::Table) -> String {
    let fields: Vec<String> =
        table.iter().map(|(key, value)| format!("{}: {}", json(key), toml_json(value))).collect();
    format!("{{ {} }}", fields.join(", "))
}

/// Where the bundle's source is: its upstream repository, pinned in the
/// port, whose `https://` address a manifest takes.
fn source_url(port: &Port) -> String {
    let git = &port.recipe.source.git;
    if git.starts_with("https://") {
        git.strip_suffix(".git").unwrap_or(git).to_string()
    } else {
        port.recipe.homepage.clone()
    }
}

/// A recipe's summary, which starts in lowercase, as a manifest's, which
/// starts with a capital.
fn sentence(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |first| first.to_uppercase().chain(chars).collect())
}

/// `text` as a JSON string.
fn json(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if u32::from(c) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", u32::from(c));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The key that signs the bundles: `OB_BUNDLE_KEY`'s, or the build's,
/// made the first time.
fn signing_key(ctx: &Ctx) -> Result<SecretKey> {
    if let Some(path) = std::env::var_os(KEY_VARIABLE) {
        let path = PathBuf::from(path);
        let text =
            std::fs::read_to_string(&path).context(|| format!("reading {}", path.display()))?;
        return SecretKey::read(&text)
            .map_err(|why| Error::new(format!("{}: {why}", path.display())));
    }
    let path = dir(ctx).join("build.key");
    if let Ok(text) = std::fs::read_to_string(&path) {
        return SecretKey::read(&text)
            .map_err(|why| Error::new(format!("{}: {why}", path.display())));
    }
    let key = SecretKey::generate(&random::<40>()?).map_err(Error::new)?;
    process::create_dir(&dir(ctx))?;
    let text = key.file(
        "Dukovo build key: it signs the bundles of ports of this build; Dukovo trusts it for \
         nothing",
        &random::<32>()?,
    );
    write_private(&path, &text)?;
    std::fs::write(dir(ctx).join("build.pub"), key.public().file())
        .context(|| format!("writing {}", dir(ctx).join("build.pub").display()))?;
    step(&format!("Made the build's bundle key {}", ctx.display(&path)));
    Ok(key)
}

/// Writes a file only its owner may read.
fn write_private(path: &Path, text: &str) -> Result<()> {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt as _;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .context(|| format!("writing {}", path.display()))?;
    file.write_all(text.as_bytes()).context(|| format!("writing {}", path.display()))
}

/// `N` bytes from the system's random numbers.
fn random<const N: usize>() -> Result<[u8; N]> {
    let mut bytes = [0; N];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut bytes))
        .context(|| "reading /dev/urandom".to_string())?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::{BundleRecipe, Recipe};
    use manifest::bundle::ReadyWord;

    fn port() -> Port {
        let text = r#"
            name = "ripgrep"
            version = "15.2.0"
            summary = "searches files for lines that match a pattern"
            license = "Unlicense OR MIT"
            homepage = "https://github.com/BurntSushi/ripgrep"
            [source]
            git = "https://github.com/BurntSushi/ripgrep.git"
            commit = "0000000000000000000000000000000000000000"
            [bundle]
            id = "io.github.burntsushi.ripgrep"
            name = "ripgrep"
            commands = ["rg"]
        "#;
        let recipe: Recipe = toml::from_str(text).unwrap();
        Port { recipe, dir: PathBuf::new(), source: None, extras: Vec::new(), crates: Vec::new() }
    }

    #[test]
    fn the_manifest_reads() {
        let port = port();
        let text = manifest(&port);
        let manifest = dkv::signed::parse_manifest(text.as_bytes()).unwrap();
        assert_eq!(manifest.id.0, "io.github.burntsushi.ripgrep");
        assert!(text.contains("\"summary\": \"Searches files for lines that match a pattern\""));
        assert!(text.contains("\"source\": \"https://github.com/BurntSushi/ripgrep\""));
        assert!(text.contains("\"runs_with\": \"your-access\""));
        let _: Option<&BundleRecipe> = port.recipe.bundle.as_ref();
    }

    /// A bundle's own paragraph reaches the manifest as its description,
    /// and a bundle without one has none.
    #[test]
    fn the_description_reaches_the_manifest() {
        let mut port = port();
        assert!(
            dkv::signed::parse_manifest(manifest(&port).as_bytes()).unwrap().description.is_none()
        );
        port.recipe.bundle.as_mut().unwrap().description = Some("It says \"hello\".".into());
        let text = manifest(&port);
        assert!(text.contains("\"description\": \"It says \\\"hello\\\".\""));
        let manifest = dkv::signed::parse_manifest(text.as_bytes()).unwrap();
        assert_eq!(manifest.description.unwrap().0, "It says \"hello\".");
    }

    /// nginx's recipe, a service bundle's: its services, permissions and
    /// data folder reach the manifest, which reads.
    #[test]
    fn a_service_bundles_manifest_reads() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ports/nginx");
        let text = std::fs::read_to_string(dir.join("recipe.toml")).unwrap();
        let recipe: Recipe = toml::from_str(&text).unwrap();
        let port = Port { recipe, dir, source: None, extras: Vec::new(), crates: Vec::new() };
        let text = manifest(&port);
        let manifest = dkv::signed::parse_manifest(text.as_bytes()).unwrap();
        assert_eq!(manifest.id.0, "org.nginx.nginx");
        assert_eq!(manifest.programs.len(), 2, "nginx and nginx-service");
        let service = &manifest.services[0];
        assert_eq!(service.name.0, "nginx");
        assert_eq!(service.program.0, "nginx-service");
        assert_eq!(service.limits.memory, Some(512 << 20));
        assert!(manifest.permissions.get("network.listen").is_some());
        assert!(manifest.permissions.get("files.shared").is_some());
        assert!(manifest.data.settings && manifest.data.data && !manifest.data.cache);
        // It starts after the network and the resolver, and is ready once
        // it listens, which only 1.0 and later read.
        let after: Vec<&str> = service.after.iter().map(|name| name.0.as_str()).collect();
        assert_eq!(after, ["netd", "resolved"]);
        let ready = service.ready.as_ref().unwrap();
        assert_eq!((ready.when, ready.timeout), (ReadyWord::Listening, Some(30)));
        assert_eq!(manifest.requires_os.to_string(), "1.0");
    }

    /// Valkey's recipe: its aliases are commands that run valkey-server
    /// by their own names, and only its programs are files.
    #[test]
    fn aliases_run_another_program() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ports/valkey");
        let text = std::fs::read_to_string(dir.join("recipe.toml")).unwrap();
        let recipe: Recipe = toml::from_str(&text).unwrap();
        let port = Port { recipe, dir, source: None, extras: Vec::new(), crates: Vec::new() };
        let text = manifest(&port);
        let manifest = dkv::signed::parse_manifest(text.as_bytes()).unwrap();
        let program = |name: &str| {
            let command = manifest.commands.iter().find(|c| c.name.0 == name).unwrap();
            command.program.0.clone()
        };
        assert_eq!(program("valkey-cli"), "valkey-cli");
        assert_eq!(program("valkey-check-aof"), "valkey-server");
        assert_eq!(program("valkey-check-rdb"), "valkey-server");
        assert_eq!(manifest.commands.len(), 4);
        assert_eq!(manifest.programs.len(), 4, "the cli, benchmark, server and service");
        assert!(manifest.programs.get("valkey-check-aof").is_none());
    }

    #[test]
    fn json_strings_are_escaped() {
        assert_eq!(json("a\"b\\c\n"), "\"a\\\"b\\\\c\\u000a\"");
    }
}

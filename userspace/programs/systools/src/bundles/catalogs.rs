// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Catalogs (ADR 0383): TUF repositories of bundles that bundled(8)
//! reads, with the client of ADR 0380, and installs from.
//!
//! - Each catalog is a folder of `/system/settings/catalogs`: `url`, where
//!   its repository is, and `root.json`, the root that came with the
//!   system or that an administrator added it with. A file `off` there
//!   turns it off. Dukovo's own comes among the default settings.
//! - bundled keeps each one's client in `/system/state/bundled/catalogs`:
//!   its trusted metadata, and an index of the bundles its targets list,
//!   written each time they are read anew, which `bundle search` and the
//!   check of a file installed from a disk read.
//! - A catalog vouches for a bundle by the SHA-256 and length of its
//!   `.dkv` file, which the client checks when it fetches it, and which
//!   bundled checks a file from anywhere else against.
//! - Its list of known flaws (ADR 0389), which its update server signs
//!   each day, is fetched each time it is read, and kept beside the index
//!   while it is good: of a bundle it does not list, a catalog tells
//!   nothing.
//! - Its update server also signs, in the role `rollout`, which wave of
//!   machines may take the newest version of each bundle (ADR 0544). An
//!   update waits for this machine's wave, unless it ends a known flaw of
//!   the version that runs ([`waiting`]); a catalog that signs no
//!   rollouts has every update taken at once.

use std::cmp::Ordering;
use std::path::{Path, PathBuf};

use dkv::flaws::{List, Severity};
use system_release::rollout::{self, Rollout};
use tuf::metadata::Targets;
use tuf::{Updater, Url};

use super::protocol::Flags;

/// Where the catalogs are set.
pub const SETTINGS: &str = "/system/settings/catalogs";
/// The catalogs the system comes with, which Spark copies into
/// [`SETTINGS`] (spark(8)).
pub const DEFAULTS: &str = "/system/resources/default-settings/catalogs";
/// Where bundled keeps each one's client.
pub const STATE: &str = "/system/state/bundled/catalogs";

/// A catalog's address, its root and whether it is off, in its folder.
pub const URL_FILE: &str = "url";
pub const ROOT_FILE: &str = "root.json";
pub const OFF_FILE: &str = "off";

/// In a catalog's state: the client's metadata, the root it started from,
/// the index, the list of flaws, the rollouts fetched, and files on their
/// way in.
const TUF_DIR: &str = "tuf";
const FIRST_ROOT: &str = "first-root.json";
const INDEX: &str = "index";
const FLAWS: &str = "flaws.json";
const ROLLOUTS: &str = "rollouts";
const DOWNLOADS: &str = "downloads";

/// The targets a catalog's bundles are, by path.
pub const BUNDLES: &str = "bundles/";

/// The most a root may take.
const ROOT_MAX: u64 = 512 * 1024;

/// A catalog, as its settings say.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Catalog {
    pub name: String,
    /// Its repository: `metadata/` and `targets/` are under it.
    pub url: String,
    /// The root it was set with.
    pub root: Vec<u8>,
    /// Whether an administrator turned it off.
    pub off: bool,
    /// Whether the system comes with it.
    pub default: bool,
}

/// Checks a catalog's name: a lowercase letter, then at most 31
/// lowercase letters, digits and `-`.
pub fn check_name(name: &str) -> Result<(), String> {
    let mut bytes = name.bytes();
    let first = bytes.next().is_some_and(|b| b.is_ascii_lowercase());
    let rest = bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    if !first || !rest || name.len() > 32 {
        return Err(format!(
            "{name:?} is no catalog's name: a lowercase letter, then at most 31 lowercase \
             letters, digits and -"
        ));
    }
    Ok(())
}

/// Checks a catalog's address: https, http or file, ending in `/` once
/// made a folder's.
pub fn check_url(url: &str) -> Result<String, String> {
    let url = if url.ends_with('/') { url.to_string() } else { format!("{url}/") };
    let parsed = Url::parse(&url).map_err(|e| format!("{url}: {e}"))?;
    if !matches!(parsed.scheme(), "https" | "http" | "file") {
        return Err(format!("{url}: a catalog is at an https, http or file address"));
    }
    if url.chars().any(char::is_control) {
        return Err(format!("{url:?} holds a control character"));
    }
    Ok(url)
}

/// Checks that `root` is a root's metadata, signed by its own keys.
pub fn check_root(root: &[u8]) -> Result<(), String> {
    tuf::trusted::Trusted::new(root, tuf::time::now()).map(drop).map_err(|e| e.to_string())
}

/// Reads one catalog's folder, `dir`.
fn read_one(name: &str, dir: &Path, default: bool) -> Result<Catalog, String> {
    let read = |file: &str| {
        std::fs::read(dir.join(file)).map_err(|e| format!("{}: {e}", dir.join(file).display()))
    };
    let url = String::from_utf8(read(URL_FILE)?)
        .map_err(|_| format!("{}: not text", dir.join(URL_FILE).display()))?;
    let url =
        check_url(url.trim()).map_err(|e| format!("{}: {e}", dir.join(URL_FILE).display()))?;
    let root = read(ROOT_FILE)?;
    if root.len() as u64 > ROOT_MAX {
        return Err(format!("{}: larger than a root may be", dir.join(ROOT_FILE).display()));
    }
    Ok(Catalog { name: name.into(), url, root, off: dir.join(OFF_FILE).exists(), default })
}

/// The catalogs set in `settings`, those the system comes with in
/// `defaults` first, then the others, each by name; and why each folder
/// that is not one could not be read.
#[must_use]
pub fn read_all(settings: &Path, defaults: &Path) -> (Vec<Catalog>, Vec<String>) {
    let mut names: Vec<String> = std::fs::read_dir(settings)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|t| t.is_dir()))
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    let mut catalogs = Vec::new();
    let mut errors = Vec::new();
    for name in names {
        if let Err(why) = check_name(&name) {
            errors.push(format!("{}: {why}", settings.join(&name).display()));
            continue;
        }
        let default = defaults.join(&name).is_dir();
        match read_one(&name, &settings.join(&name), default) {
            Ok(catalog) => catalogs.push(catalog),
            Err(why) => errors.push(why),
        }
    }
    catalogs.sort_by_key(|c| !c.default);
    (catalogs, errors)
}

/// A bundle a catalog lists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// Its target's path, `bundles/ID-VERSION.dkv`.
    pub path: String,
    /// Its `.dkv` file's SHA-256, in hex, and length.
    pub sha256: String,
    pub length: u64,
    pub id: String,
    pub version: String,
    /// Its name, as the catalog gives it.
    pub name: String,
}

/// What bundled keeps of a catalog's targets, as it last read them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Index {
    /// When it was read, and when what it was read from expires, in
    /// seconds since 1970: after that, it vouches for nothing.
    pub read: i64,
    pub expires: i64,
    /// Targets' version.
    pub version: u64,
    pub entries: Vec<Entry>,
}

/// A field of the index: printable, without tabs.
fn field(text: &str) -> String {
    text.chars().map(|c| if c.is_control() { ' ' } else { c }).collect()
}

impl Index {
    /// The index of `targets`, version `version`, read at `read`, good
    /// until `expires`. A target that is not a bundle's, or whose
    /// `custom` does not say its ID and version, is left out.
    #[must_use]
    pub fn of(targets: &Targets, version: u64, read: i64, expires: i64) -> Index {
        let mut entries: Vec<Entry> = targets
            .targets
            .iter()
            .filter(|(path, _)| path.starts_with(BUNDLES))
            .filter_map(|(path, target)| {
                let custom = target.custom.as_ref()?;
                let text = |key: &str| custom.get(key)?.as_str().map(String::from);
                let id = text("id")?;
                manifest::bundle::check_full_id(&id).ok()?;
                let version = text("version")?;
                let sha256 = target.hashes.0.iter().find(|(alg, _)| alg == "sha256")?.1.clone();
                let hex = sha256.len() == 64 && sha256.bytes().all(|b| b.is_ascii_hexdigit());
                hex.then(|| Entry {
                    path: path.clone(),
                    sha256: sha256.to_ascii_lowercase(),
                    length: target.length,
                    name: field(&text("name").unwrap_or_else(|| id.clone())),
                    version: field(&version),
                    id,
                })
            })
            .collect();
        entries
            .sort_by(|a, b| a.id.cmp(&b.id).then_with(|| compare_versions(&a.version, &b.version)));
        Index { read, expires, version, entries }
    }

    /// The index's file.
    #[must_use]
    pub fn text(&self) -> String {
        let mut out =
            format!("read\t{}\texpires\t{}\tversion\t{}\n", self.read, self.expires, self.version);
        for e in &self.entries {
            out += &format!(
                "{}\t{}\t{}\t{}\t{}\t{}\n",
                e.sha256, e.length, e.id, e.version, e.path, e.name
            );
        }
        out
    }

    /// Reads an index's file.
    pub fn read(text: &str) -> Result<Index, String> {
        let mut lines = text.lines();
        let head: Vec<&str> = lines.next().unwrap_or_default().split('\t').collect();
        let number = |text: &str| text.parse::<i64>().map_err(|_| format!("{text:?} is no number"));
        let (read, expires, version) = match head.as_slice() {
            ["read", read, "expires", expires, "version", version] => (
                number(read)?,
                number(expires)?,
                version.parse().map_err(|_| format!("{version:?} is no version"))?,
            ),
            _ => return Err("its first line is not the index's".into()),
        };
        let entries = lines
            .map(|line| match line.split('\t').collect::<Vec<_>>().as_slice() {
                [sha256, length, id, version, path, name] => Ok(Entry {
                    sha256: (*sha256).into(),
                    length: length.parse().map_err(|_| format!("{length:?} is no length"))?,
                    id: (*id).into(),
                    version: (*version).into(),
                    path: (*path).into(),
                    name: (*name).into(),
                }),
                _ => Err(format!("{line:?} is no line of an index")),
            })
            .collect::<Result<_, _>>()?;
        Ok(Index { read, expires, version, entries })
    }

    /// Whether it still vouches for what it lists at `now`.
    #[must_use]
    pub fn current(&self, now: i64) -> bool {
        now < self.expires
    }

    /// The newest version it lists of `id`; of an ID it lists only with
    /// lines, the newest of its newest line (ADR 0387).
    #[must_use]
    pub fn newest(&self, id: &str) -> Option<&Entry> {
        let newest = |of: &dyn Fn(&Entry) -> bool| {
            self.entries
                .iter()
                .filter(|e| of(e))
                .max_by(|a, b| compare_versions(&a.version, &b.version))
        };
        newest(&|e| e.id == id)
            .or_else(|| newest(&|e| e.id.split_once('@').is_some_and(|(base, _)| base == id)))
    }

    /// Whether it lists a file of `id` with this SHA-256 and length.
    #[must_use]
    pub fn lists(&self, id: &str, sha256: &str, length: u64) -> bool {
        self.entries.iter().any(|e| e.id == id && e.sha256 == sha256 && e.length == length)
    }

    /// The newest version of each bundle whose ID or name holds every one
    /// of `words`, in any case.
    #[must_use]
    pub fn search(&self, words: &[String]) -> Vec<&Entry> {
        let words: Vec<String> = words.iter().map(|w| w.to_lowercase()).collect();
        let mut hits: Vec<&Entry> = Vec::new();
        for entry in &self.entries {
            let text = format!("{} {}", entry.id, entry.name).to_lowercase();
            if !words.iter().all(|w| text.contains(w.as_str())) {
                continue;
            }
            match hits.iter_mut().find(|h| h.id == entry.id) {
                Some(hit) if compare_versions(&hit.version, &entry.version).is_lt() => *hit = entry,
                Some(_) => {}
                None => hits.push(entry),
            }
        }
        hits
    }
}

/// Orders two versions as semantic versioning does: by their numbers,
/// a missing one 0, then a pre-release before the release, pre-releases
/// by their dot-separated parts, numbers before words; a build is left
/// out.
#[must_use]
pub fn compare_versions(a: &str, b: &str) -> Ordering {
    fn split(version: &str) -> (Vec<u64>, Option<&str>) {
        let version = version.split('+').next().unwrap_or_default();
        let (numbers, pre) = match version.split_once('-') {
            Some((numbers, pre)) => (numbers, Some(pre)),
            None => (version, None),
        };
        (numbers.split('.').map(|n| n.parse().unwrap_or(0)).collect(), pre)
    }
    let (an, ap) = split(a);
    let (bn, bp) = split(b);
    for i in 0..an.len().max(bn.len()) {
        let order = an.get(i).copied().unwrap_or(0).cmp(&bn.get(i).copied().unwrap_or(0));
        if order.is_ne() {
            return order;
        }
    }
    match (ap, bp) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) => Ordering::Less,
        (Some(ap), Some(bp)) => {
            let mut ai = ap.split('.');
            let mut bi = bp.split('.');
            loop {
                match (ai.next(), bi.next()) {
                    (None, None) => return Ordering::Equal,
                    (None, Some(_)) => return Ordering::Less,
                    (Some(_), None) => return Ordering::Greater,
                    (Some(x), Some(y)) => {
                        let order = match (x.parse::<u64>(), y.parse::<u64>()) {
                            (Ok(x), Ok(y)) => x.cmp(&y),
                            (Ok(_), Err(_)) => Ordering::Less,
                            (Err(_), Ok(_)) => Ordering::Greater,
                            (Err(_), Err(_)) => x.cmp(y),
                        };
                        if order.is_ne() {
                            return order;
                        }
                    }
                }
            }
        }
    }
}

/// Writes `data` to `path` whole: to a file beside it, then renamed.
fn write_whole(path: &Path, data: &[u8]) -> Result<(), String> {
    let partial = path.with_extension("partial");
    std::fs::write(&partial, data).map_err(|e| format!("{}: {e}", partial.display()))?;
    std::fs::rename(&partial, path).map_err(|e| format!("{}: {e}", path.display()))
}

/// A catalog's state folder, under `state`.
#[must_use]
pub fn state_dir(state: &Path, name: &str) -> PathBuf {
    state.join(name)
}

/// The index bundled last wrote for catalog `name`, if it can be read.
#[must_use]
pub fn read_index(state: &Path, name: &str) -> Option<Index> {
    let text = std::fs::read_to_string(state_dir(state, name).join(INDEX)).ok()?;
    Index::read(&text).ok()
}

/// The list of flaws bundled last fetched from catalog `name`, if it
/// was good then, can be read, and was made less than
/// [`FLAWS_GOOD_DAYS`] ago (ADR 0389).
#[must_use]
pub fn read_flaws(state: &Path, name: &str) -> Option<List> {
    let data = std::fs::read(state_dir(state, name).join(FLAWS)).ok()?;
    let list = List::read(&data).ok()?;
    let made = tuf::time::parse(&list.made)?;
    (tuf::time::now() - made < FLAWS_GOOD_DAYS * 86_400).then_some(list)
}

/// How long a catalog's list of flaws tells of them, in days: the week
/// its update server signs it for, and a day more for clocks.
const FLAWS_GOOD_DAYS: i64 = 8;

/// What `list` says of version `version` of bundle `id`, in short.
#[must_use]
pub fn flags(list: Option<&List>, id: &str, version: &str) -> Flags {
    let Some(list) = list.filter(|l| l.checked(id, version)) else {
        return Flags::Unknown;
    };
    let mut count = 0;
    let mut worst = Severity::Unknown;
    for flaw in list.of(id, version) {
        count += 1;
        worst = worst.max(flaw.severity);
    }
    if count == 0 { Flags::None } else { Flags::Known { count, worst } }
}

/// Whether taking `to`, an ID and version, in place of `from` ends a high
/// or critical flaw that `list` knows in `from` and names no more in `to`
/// (ADR 0544). A version the list did not look at is not known to be
/// without the flaw, so it is no fix.
#[must_use]
pub fn fixes_a_flaw(list: Option<&List>, from: (&str, &str), to: (&str, &str)) -> bool {
    let Some(list) = list.filter(|l| l.checked(to.0, to.1)) else { return false };
    let names = |flaw: &dkv::flaws::Flaw| -> Vec<String> {
        std::iter::once(flaw.id.clone()).chain(flaw.aliases.iter().cloned()).collect()
    };
    list.of(from.0, from.1).filter(|flaw| flaw.severity >= Severity::High).any(|flaw| {
        let known = names(flaw);
        !list.of(to.0, to.1).any(|left| names(left).iter().any(|n| known.contains(n)))
    })
}

/// Why the update of bundle `id` to `version` waits for this machine's
/// wave, in the lines to show, or none when it is taken now (ADR 0544).
/// It is taken when the catalog signs no `rollout` of that version, when
/// the wave open in it reaches the machine numbered `number` or the
/// machine is a `canary`, and when it `fixes` a known flaw of the
/// version that runs, which a wave never holds back.
#[must_use]
pub fn waiting(
    id: &str,
    version: &str,
    rollout: Option<&Rollout>,
    number: u16,
    canary: bool,
    fixes: bool,
) -> Option<Vec<String>> {
    if fixes {
        return None;
    }
    let rollout = rollout?;
    if rollout::reaches(rollout, number, canary) {
        return None;
    }
    let day = rollout::day_for(number);
    let mut lines = vec![format!(
        "{id} {version} reaches machines in waves; this machine's opens on day {day}"
    )];
    lines.extend(rollout.held.as_ref().map(|why| format!("the waves are held: {why}")));
    lines.push(format!("`bundle update {id}` takes it now"));
    Some(lines)
}

/// A catalog's client, brought up to date.
pub struct Open {
    updater: Updater,
    dir: PathBuf,
    pub index: Index,
}

/// Reads `catalog` anew with the client kept in `state`: from the root
/// it was set with the first time, or again if that root changed, then
/// from what it trusts. Writes its index.
///
/// # Errors
///
/// If its repository cannot be read, or what it says fails a check; the
/// index stays as it was.
pub fn open(catalog: &Catalog, state: &Path) -> Result<Open, String> {
    let dir = state_dir(state, &catalog.name);
    let first = dir.join(FIRST_ROOT);
    let same = std::fs::read(&first).ok().as_deref() == Some(catalog.root.as_slice());
    let root = (!same || !started(&dir)).then_some(catalog.root.as_slice());
    let open = open_dir(&dir, &catalog.url, root)?;
    if root.is_some() {
        write_whole(&first, &catalog.root)?;
    }
    Ok(open)
}

/// Whether the client in `dir` has started: it trusts a root.
#[must_use]
pub fn started(dir: &Path) -> bool {
    dir.join(TUF_DIR).join("root.json").exists()
}

/// The root the client in `dir` trusts now, if it has started.
#[must_use]
pub fn trusted_root(dir: &Path) -> Option<tuf::metadata::Root> {
    let bytes = std::fs::read(dir.join(TUF_DIR).join("root.json")).ok()?;
    tuf::metadata::Signed::<tuf::metadata::Root>::parse(&bytes).ok().map(|r| r.signed)
}

/// Reads the TUF repository at `url` anew with the client kept in `dir`:
/// from `root` when it is given, which starts the client over, else from
/// what it trusts. Writes its index (ADR 0383); a bundle's own feed is
/// read so too (ADR 0390).
///
/// # Errors
///
/// As [`open`].
pub fn open_dir(dir: &Path, url: &str, root: Option<&[u8]>) -> Result<Open, String> {
    let tuf_dir = dir.join(TUF_DIR);
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    if let Some(root) = root {
        let _ = std::fs::remove_dir_all(&tuf_dir);
        let _ = std::fs::remove_file(dir.join(INDEX));
        tuf::init(&tuf_dir, root).map_err(|e| e.to_string())?;
    }
    let base = Url::parse(url).map_err(|e| format!("{url}: {e}"))?;
    let metadata = base.resolve("metadata/").map_err(|e| e.to_string())?;
    let targets = base.resolve("targets/").map_err(|e| e.to_string())?;
    let mut updater =
        Updater::new(&tuf_dir, &metadata, Some(&targets)).map_err(|e| e.to_string())?;
    updater.refresh().map_err(|e| e.to_string())?;
    let trusted = updater.trusted();
    let top = trusted.targets("targets").ok_or("its targets were not read")?;
    let expires = [
        Some(trusted.root().expires),
        trusted.timestamp().map(|t| t.expires),
        trusted.snapshot().map(|s| s.expires),
        Some(top.expires),
    ]
    .into_iter()
    .flatten()
    .min()
    .unwrap_or_default();
    let index = Index::of(&top.signed, top.version, tuf::time::now(), expires);
    write_whole(&dir.join(INDEX), index.text().as_bytes())?;
    // The list of flaws, while it is good; one that cannot be had is not
    // kept, so that nothing tells of flaws from an old one.
    let flaws = dir.join(FLAWS);
    if fetch_flaws(&mut updater, &flaws).is_err() {
        let _ = std::fs::remove_file(&flaws);
    }
    Ok(Open { updater, dir: dir.to_path_buf(), index })
}

/// Fetches a catalog's list of flaws into `file`, if its snapshot lists
/// the role that signs it.
fn fetch_flaws(updater: &mut Updater, file: &Path) -> Result<(), String> {
    let listed = updater
        .trusted()
        .snapshot()
        .is_some_and(|s| s.signed.meta.contains_key(&format!("{}.json", dkv::flaws::ROLE)));
    if !listed {
        return Err("the catalog signs no list of flaws".into());
    }
    let target = updater
        .target(dkv::flaws::TARGET)
        .map_err(|e| e.to_string())?
        .ok_or("the catalog's flaws role lists no list")?;
    if !Updater::cached(file, &target) {
        updater.download_target(dkv::flaws::TARGET, &target, file).map_err(|e| e.to_string())?;
    }
    List::read(&std::fs::read(file).map_err(|e| e.to_string())?).map(drop)
}

impl Open {
    /// The rollout the catalog signs of `entry`, the newest version of a
    /// bundle: which waves of machines may take it (ADR 0544). None when
    /// the catalog signs none of it, as when its targets delegate none.
    ///
    /// # Errors
    ///
    /// If the rollout cannot be fetched, does not check, or is another
    /// version's.
    pub fn rollout(&mut self, entry: &Entry) -> Result<Option<Rollout>, String> {
        let listed = self
            .updater
            .trusted()
            .snapshot()
            .is_some_and(|s| s.signed.meta.contains_key(&format!("{}.json", rollout::ROLE)));
        if !listed {
            return Ok(None);
        }
        let release = format!("{}/{}", entry.id, entry.version);
        let path = rollout::target_path(&release);
        let Some(target) = self.updater.target(&path).map_err(|e| e.to_string())? else {
            return Ok(None);
        };
        let file = Updater::target_path(&self.dir.join(ROLLOUTS), &path);
        if !Updater::cached(&file, &target) {
            self.updater.download_target(&path, &target, &file).map_err(|e| e.to_string())?;
        }
        let bytes = std::fs::read(&file).map_err(|e| format!("{}: {e}", file.display()))?;
        let rollout: Rollout =
            serde_json::from_slice(&bytes).map_err(|e| format!("{path}: {e}"))?;
        if rollout.release != release || rollout.wave > rollout::LAST {
            return Err(format!("{path} is not a rollout of {} {}", entry.id, entry.version));
        }
        Ok(Some(rollout))
    }

    /// Fetches `entry` into this catalog's downloads, checked against its
    /// length and SHA-256 as its targets give them, and gives its path.
    ///
    /// # Errors
    ///
    /// If it is not among the targets, or cannot be fetched whole.
    pub fn fetch(&mut self, entry: &Entry) -> Result<PathBuf, String> {
        let target = self
            .updater
            .target(&entry.path)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("{} is not among its targets", entry.path))?;
        let file = self.dir.join(DOWNLOADS).join(format!("{}.{}", entry.sha256, dkv::EXTENSION));
        if !Updater::cached(&file, &target) {
            self.updater.download_target(&entry.path, &target, &file).map_err(|e| e.to_string())?;
        }
        Ok(file)
    }
}

/// Forgets what bundled kept of catalog `name`.
pub fn forget(state: &Path, name: &str) {
    let _ = std::fs::remove_dir_all(state_dir(state, name));
}

/// Removes what is on its way in for every catalog: a download is kept
/// only while its install runs, or, for the one sheet bundled keeps, until
/// the next sheet is read; none outlasts bundled's start.
pub fn tidy(state: &Path) {
    for entry in std::fs::read_dir(state).into_iter().flatten().flatten() {
        let _ = std::fs::remove_dir_all(entry.path().join(DOWNLOADS));
    }
}

#[cfg(test)]
mod tests {
    use system_release::rollout::Plans;

    use super::*;

    #[test]
    fn versions_order_as_semantic_versioning_orders_them() {
        let ordered = [
            "0.9",
            "1.0.0-alpha",
            "1.0.0-alpha.1",
            "1.0.0-alpha.beta",
            "1.0.0-beta",
            "1.0.0-beta.2",
            "1.0.0-beta.11",
            "1.0.0-rc.1",
            "1.0.0",
            "1.0.1",
            "1.2",
            "1.10",
            "2",
        ];
        for pair in ordered.windows(2) {
            assert_eq!(compare_versions(pair[0], pair[1]), Ordering::Less, "{pair:?}");
            assert_eq!(compare_versions(pair[1], pair[0]), Ordering::Greater, "{pair:?}");
        }
        assert_eq!(compare_versions("1.0", "1.0.0"), Ordering::Equal);
        assert_eq!(compare_versions("1.0.0+build.5", "1.0.0"), Ordering::Equal);
    }

    #[test]
    fn names_and_addresses_are_checked() {
        assert!(check_name("dukovo").is_ok());
        assert!(check_name("my-feed2").is_ok());
        for bad in ["", "Dukovo", "2feed", "a b", "a/b", "..", &"a".repeat(33)] {
            assert!(check_name(bad).is_err(), "{bad:?}");
        }
        assert_eq!(
            check_url("https://updates.dukovo.org/catalog").unwrap(),
            "https://updates.dukovo.org/catalog/"
        );
        assert!(check_url("file:///volumes/vdb/catalog/").is_ok());
        assert!(check_url("ftp://example.org/").is_err());
    }

    fn entry(id: &str, version: &str, sha: char) -> Entry {
        Entry {
            path: format!("bundles/{id}-{version}.dkv"),
            sha256: sha.to_string().repeat(64),
            length: 10,
            id: id.into(),
            version: version.into(),
            name: id.rsplit('.').next().unwrap().into(),
        }
    }

    #[test]
    fn an_index_reads_back_and_finds_bundles() {
        let index = Index {
            read: 1_793_000_000,
            expires: 1_793_086_400,
            version: 3,
            entries: vec![
                entry("io.github.burntsushi.ripgrep", "14.1.0", 'a'),
                entry("io.github.burntsushi.ripgrep", "15.2.0", 'b'),
                entry("io.github.sharkdp.bat", "0.26.1", 'c'),
            ],
        };
        let read = Index::read(&index.text()).unwrap();
        assert_eq!(read, index);
        assert!(read.current(1_793_000_001));
        assert!(!read.current(1_793_086_400));
        assert_eq!(read.newest("io.github.burntsushi.ripgrep").unwrap().version, "15.2.0");
        assert!(read.lists("io.github.sharkdp.bat", &"c".repeat(64), 10));
        assert!(!read.lists("io.github.sharkdp.bat", &"c".repeat(64), 11));
        assert!(!read.lists("io.github.burntsushi.ripgrep", &"c".repeat(64), 10));
        let hits = read.search(&["RIP".into()]);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].version, "15.2.0");
        assert_eq!(read.search(&["github".into()]).len(), 2);
        assert!(read.search(&["github".into(), "zzz".into()]).is_empty());
        assert!(Index::read("read\t1\n").is_err());
    }

    #[test]
    fn an_ids_lines_are_bundles_of_their_own() {
        let index = Index {
            read: 1,
            expires: 2,
            version: 1,
            entries: vec![
                entry("org.nodejs.node@20", "20.18.0", 'a'),
                entry("org.nodejs.node@22", "22.11.0", 'b'),
                entry("org.nodejs.node@22", "22.12.0", 'c'),
            ],
        };
        // An update stays in its line; an install by the ID takes the
        // newest line.
        assert_eq!(index.newest("org.nodejs.node@20").unwrap().version, "20.18.0");
        assert_eq!(index.newest("org.nodejs.node@22").unwrap().version, "22.12.0");
        assert_eq!(index.newest("org.nodejs.node").unwrap().id, "org.nodejs.node@22");
        assert!(index.newest("org.nodejs.node@24").is_none());
        assert!(index.newest("org.nodejs").is_none());
        assert_eq!(index.search(&["node".into()]).len(), 2);
        assert_eq!(Index::read(&index.text()).unwrap(), index);
    }

    #[test]
    fn settings_are_read_defaults_first() {
        let dir = std::env::temp_dir().join(format!("catalogs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let settings = dir.join("settings");
        let defaults = dir.join("defaults");
        for name in ["alpha", "dukovo", "Bad"] {
            std::fs::create_dir_all(settings.join(name)).unwrap();
            std::fs::write(settings.join(name).join(URL_FILE), "https://example.org/c\n").unwrap();
            std::fs::write(settings.join(name).join(ROOT_FILE), "{}").unwrap();
        }
        std::fs::create_dir_all(defaults.join("dukovo")).unwrap();
        std::fs::write(settings.join("alpha").join(OFF_FILE), "").unwrap();
        let (catalogs, errors) = read_all(&settings, &defaults);
        let names: Vec<&str> = catalogs.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["dukovo", "alpha"]);
        assert!(catalogs[0].default && !catalogs[0].off);
        assert!(catalogs[1].off && !catalogs[1].default);
        assert_eq!(catalogs[1].url, "https://example.org/c/");
        assert_eq!(errors.len(), 1, "{errors:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A bundle file of `org.example.app` at `version`, in `dir`.
    fn bundle_file(dir: &Path, version: &str) -> PathBuf {
        let manifest =
            format!(r#"{{"id": "org.example.app", "version": "{version}", "name": "App"}}"#);
        let sbom = "PackageName: org.example.app\n\
                    ExternalRef: PACKAGE-MANAGER purl pkg:cargo/regex@1.10.2\n";
        let path = dir.join(format!("app-{version}.dkv"));
        let mut zip = dkv::zip::Writer::new(std::fs::File::create(&path).unwrap());
        let files = [
            (dkv::MANIFEST, manifest.as_bytes()),
            ("bin/app", version.as_bytes()),
            (dkv::sbom::SBOM, sbom.as_bytes()),
        ];
        for (name, data) in files {
            zip.add(&dkv::zip::Input { path: name, executable: false, data }).unwrap();
        }
        zip.finish().unwrap();
        path
    }

    /// A catalog of two versions of a bundle, signed and published into
    /// `dir/site`, with the rollouts of `plans` signed too when `rolled`;
    /// its address and the root it starts from.
    fn catalog_with(dir: &Path, plans: Option<&mut Plans>) -> (String, Vec<u8>) {
        use catalog_repo::keys::{KeyFile, Signers, from_minisign, minisign_key};
        let secret = dkv::minisign::SecretKey::generate(&[11u8; 40]).unwrap();
        let repo = catalog_repo::Repo::new(&dir.join("repository"));
        let roles = catalog_repo::ROLES.iter().map(|r| (*r).to_string()).collect();
        let key = KeyFile {
            name: "publisher".into(),
            roles,
            yubikey: None,
            key: minisign_key(&secret.public()),
        };
        key.write(&repo.keys_dir()).unwrap();
        let mut signers = Signers::default();
        signers.add(from_minisign(&secret).unwrap());
        repo.sign_root(&signers, 365).unwrap();
        let files = [bundle_file(dir, "1.0.0"), bundle_file(dir, "1.1.0")];
        let changes = repo.changes(&files, &[]).unwrap();
        let store = dir.join("store");
        repo.sign_targets(&changes, &files, &store, &signers, 30).unwrap();
        if let Some(plans) = plans {
            let signed =
                repo.sign_rollouts(&store, &signers, Some(plans), tuf::time::now()).unwrap();
            assert!(signed.is_some());
        }
        repo.publish(&store, &dir.join("site"), &signers, (30, 30)).unwrap();
        let first = std::fs::read(dir.join("site/metadata/1.root.json")).unwrap();
        (format!("file://{}/site/", dir.display()), first)
    }

    #[test]
    fn a_rollout_is_read_through_the_catalogs_client() {
        let dir = std::env::temp_dir().join(format!("catalogs-rollout-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut plans = Plans::default();
        let release = "org.example.app/1.1.0";
        plans.releases.insert(
            release.into(),
            rollout::Plan { released: tuf::time::now(), open: 0, paused: Some("bad logs".into()) },
        );
        let (address, first) = catalog_with(&dir, Some(&mut plans));
        let mut read = open_dir(&dir.join("state"), &address, Some(&first)).unwrap();
        let newest = read.index.newest("org.example.app").cloned().unwrap();
        assert_eq!(newest.version, "1.1.0");
        let rolled = read.rollout(&newest).unwrap().unwrap();
        assert_eq!(rolled.release, release);
        assert_eq!(rolled.wave, 0);
        assert_eq!(rolled.held.as_deref(), Some("paused: bad logs"));
        // A machine that is not in wave 0 waits, and is told why; read
        // again from what is kept, it says the same.
        let lines = waiting(&newest.id, &newest.version, Some(&rolled), 700, false, false).unwrap();
        assert!(lines.iter().any(|l| l.contains("opens on day 7")), "{lines:?}");
        assert!(lines.iter().any(|l| l.contains("paused: bad logs")), "{lines:?}");
        assert!(read.rollout(&newest).unwrap().is_some());
        // The version before it has no rollout: it is not the newest.
        let older = Entry { version: "1.0.0".into(), ..newest.clone() };
        assert_eq!(read.rollout(&older).unwrap(), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_catalog_that_signs_no_rollouts_has_every_update_taken() {
        let dir = std::env::temp_dir().join(format!("catalogs-no-rollout-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (address, first) = catalog_with(&dir, None);
        let mut read = open_dir(&dir.join("state"), &address, Some(&first)).unwrap();
        let newest = read.index.newest("org.example.app").cloned().unwrap();
        assert_eq!(read.rollout(&newest).unwrap(), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn flaw(version: &str, id: &str, aliases: &[&str], severity: Severity) -> dkv::flaws::Flaw {
        dkv::flaws::Flaw {
            bundle: "org.example.app".into(),
            version: version.into(),
            id: id.into(),
            aliases: aliases.iter().map(|a| (*a).to_string()).collect(),
            component: "zlib 1.2.11".into(),
            severity,
            summary: "a flaw".into(),
            fixed: None,
        }
    }

    fn list(checked: &[&str], flaws: Vec<dkv::flaws::Flaw>) -> List {
        List {
            made: "2026-11-14T00:00:00Z".into(),
            checked: checked.iter().map(|v| ("org.example.app".to_string(), (*v).into())).collect(),
            flaws,
        }
    }

    #[test]
    fn a_version_that_ends_a_high_flaw_is_a_fix() {
        let app = "org.example.app";
        let high = flaw("1.0.0", "RUSTSEC-2026-0001", &["CVE-2026-1"], Severity::High);
        let low = flaw("1.0.0", "RUSTSEC-2026-0002", &[], Severity::Low);
        let same = flaw("1.1.0", "RUSTSEC-2026-0001", &[], Severity::High);
        let renamed = flaw("1.1.0", "GHSA-xxxx", &["CVE-2026-1"], Severity::High);
        let from = (app, "1.0.0");
        let to = (app, "1.1.0");
        // The newer version is looked at and has none of it.
        let fixed = list(&["1.0.0", "1.1.0"], vec![high.clone(), low.clone()]);
        assert!(fixes_a_flaw(Some(&fixed), from, to));
        // It still has it, by its ID or by another name of it.
        let kept = list(&["1.0.0", "1.1.0"], vec![high.clone(), same]);
        assert!(!fixes_a_flaw(Some(&kept), from, to));
        let kept = list(&["1.0.0", "1.1.0"], vec![high.clone(), renamed]);
        assert!(!fixes_a_flaw(Some(&kept), from, to));
        // Ending a low flaw alone is no fix.
        let minor = list(&["1.0.0", "1.1.0"], vec![low]);
        assert!(!fixes_a_flaw(Some(&minor), from, to));
        // A newer version the list did not look at is not known to be
        // without the flaw; a catalog with no list tells nothing.
        let unseen = list(&["1.0.0"], vec![high]);
        assert!(!fixes_a_flaw(Some(&unseen), from, to));
        assert!(!fixes_a_flaw(None, from, to));
    }

    fn at_wave(wave: u8, held: Option<&str>) -> Rollout {
        Rollout { release: "org.example.app/1.1.0".into(), wave, held: held.map(str::to_string) }
    }

    #[test]
    fn an_update_waits_for_the_wave_that_reaches_the_machine() {
        let app = "org.example.app";
        let wait = |r: Option<&Rollout>, number, canary, fixes| {
            waiting(app, "1.1.0", r, number, canary, fixes)
        };
        // A catalog that signs no rollout has every update taken at once.
        assert_eq!(wait(None, 999, false, false), None);
        // A wave that is open for the machine.
        assert_eq!(wait(Some(&at_wave(1, None)), 5, false, false), None);
        assert!(wait(Some(&at_wave(0, None)), 5, false, false).is_some(), "wave 0 is canaries");
        assert_eq!(wait(Some(&at_wave(2, None)), 99, false, false), None);
        assert_eq!(wait(Some(&at_wave(4, None)), 999, false, false), None);
        // One not open yet says on which day it opens.
        let lines = wait(Some(&at_wave(0, None)), 700, false, false).unwrap();
        assert_eq!(
            lines,
            [
                "org.example.app 1.1.0 reaches machines in waves; this machine's opens on day 7",
                "`bundle update org.example.app` takes it now",
            ]
        );
        let lines = wait(Some(&at_wave(1, None)), 50, false, false).unwrap();
        assert!(lines[0].ends_with("opens on day 3"), "{lines:?}");
        // A hold says why.
        let lines = wait(Some(&at_wave(0, Some("paused: bad logs"))), 700, false, false).unwrap();
        assert_eq!(lines.len(), 3, "{lines:?}");
        assert_eq!(lines[1], "the waves are held: paused: bad logs");
        // A canary is in every wave, and a fix is held by none.
        assert_eq!(wait(Some(&at_wave(0, Some("paused: x"))), 900, true, false), None);
        assert_eq!(wait(Some(&at_wave(0, Some("paused: x"))), 900, false, true), None);
    }
}

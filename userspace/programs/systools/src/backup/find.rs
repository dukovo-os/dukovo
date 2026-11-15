// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `backup find`: the backups of machines and the recovery keys that a
//! person has plugged in, for a move to new hardware to start from. It
//! looks in each mounted volume under `/volumes`, to a depth of three
//! folders, for a folder that holds a machine's backups (backup's own
//! mark and the folder of the machine's records), and for the files that
//! are a destination's key file. It passes over this machine's own root,
//! which Spark mounts there too: its key files are this machine's own
//! destinations', not an old machine's.
//!
//! It reads only a key file's two comment lines, which say what the key
//! is for and its public half, and never the key: it stops reading at the
//! end of the second line.

use std::fs::{self, File};
use std::io::Read;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use serde_json::{Value, json};

use super::json::time_ms;
use super::layout::{self, ENDING, MARK, RECORDS};
use super::may_name;

/// How deep it looks: folders under a volume's top.
const DEPTH: usize = 3;

/// The most entries it looks at in all, so that a volume of many files is
/// not read through.
const ENTRIES_MOST: usize = 20_000;

/// The most bytes a key file has.
const KEY_FILE_MOST: u64 = 4096;

/// How a key file's first line begins: `add` writes it.
const KEY_LINE: &str = "# The key of backup(8)'s destination ";

/// How its second line begins.
const PUBLIC_LINE: &str = "# public key: ";

/// A machine's backups found.
#[derive(Debug, PartialEq, Eq)]
pub struct Source {
    /// Its folder, which `backup restore` takes as its source.
    pub path: PathBuf,
    /// The machine's name: the folder's.
    pub machine: String,
    /// The time of its newest record, if it holds one.
    pub newest: Option<String>,
    /// The volume it is on, by its name in `/volumes`.
    pub volume: String,
    /// Whether the volume is a drive that is plugged in and out.
    pub removable: bool,
}

/// A key file found.
#[derive(Debug, PartialEq, Eq)]
pub struct KeyFile {
    pub path: PathBuf,
    /// The destination it is the key of, by its name where it was made.
    pub name: String,
    /// The age public key that goes with it.
    pub public: String,
}

/// What `find` found.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Found {
    /// By the time of the newest record, the newest first.
    pub sources: Vec<Source>,
    /// By path.
    pub keys: Vec<KeyFile>,
}

/// The time of the newest record in `folder`, a machine's records.
fn newest_record(folder: &Path) -> Option<String> {
    fs::read_dir(folder)
        .ok()?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            name.strip_suffix(ENDING).filter(|time| layout::is_time(time)).map(String::from)
        })
        .max()
}

/// Whether `path` is a plain file, not a link to one.
fn is_file(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|meta| meta.is_file())
}

/// What the two comment lines of the key file `path` say, if it is one:
/// the destination it is for, and its public key. Nothing after the second
/// line's end is read.
pub fn read_key_file(path: &Path) -> Option<(String, String)> {
    // A link is not followed, and a pipe is not waited for.
    let mut file = File::options()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .ok()?;
    let meta = file.metadata().ok()?;
    // A key file is a small plain file.
    if !meta.is_file() || meta.len() > KEY_FILE_MOST {
        return None;
    }
    let mut start = [0u8; KEY_LINE.len()];
    file.read_exact(&mut start).ok()?;
    if start != KEY_LINE.as_bytes() {
        return None;
    }
    // The rest of the line, and the next, a byte at a time, so that the
    // read ends at the second line's end and goes no further.
    let mut lines: [Vec<u8>; 2] = [Vec::new(), Vec::new()];
    let mut byte = [0u8; 1];
    for line in &mut lines {
        loop {
            if file.read(&mut byte).ok()? == 0 {
                return None;
            }
            if byte[0] == b'\n' {
                break;
            }
            line.push(byte[0]);
            if line.len() > 1024 {
                return None;
            }
        }
    }
    let first = String::from_utf8(std::mem::take(&mut lines[0])).ok()?;
    let second = String::from_utf8(std::mem::take(&mut lines[1])).ok()?;
    let name = first.split_once(", at ")?.0;
    let public = second.strip_prefix(PUBLIC_LINE)?.trim();
    age::x25519::Recipient::from_str(public).ok()?;
    may_name(name).then(|| (name.to_string(), public.to_string()))
}

/// Looks through one volume.
struct Search<'a> {
    volume: &'a str,
    removable: bool,
    /// Entries left to look at.
    left: &'a mut usize,
    found: &'a mut Found,
}

impl Search<'_> {
    /// Looks in `dir`, `depth` folders below the volume's top.
    fn look(&mut self, dir: &Path, depth: usize) {
        let machine = dir.file_name().and_then(|name| name.to_str());
        if let Some(machine) = machine.filter(|_| depth > 0)
            && is_file(&dir.join(MARK))
            && dir.join(RECORDS).is_dir()
        {
            self.found.sources.push(Source {
                path: dir.to_path_buf(),
                machine: machine.to_string(),
                newest: newest_record(&dir.join(RECORDS)),
                volume: self.volume.to_string(),
                removable: self.removable,
            });
            return;
        }
        let Ok(entries) = fs::read_dir(dir) else { return };
        let mut entries: Vec<_> = entries.filter_map(Result::ok).collect();
        entries.sort_by_key(fs::DirEntry::file_name);
        for entry in entries {
            if *self.left == 0 {
                return;
            }
            *self.left -= 1;
            if entry.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            let Ok(kind) = entry.file_type() else { continue };
            let path = entry.path();
            if kind.is_dir() && depth < DEPTH {
                self.look(&path, depth + 1);
            } else if kind.is_file()
                && let Some((name, public)) = read_key_file(&path)
            {
                self.found.keys.push(KeyFile { path, name, public });
            }
        }
    }
}

/// What the volumes under `volumes` hold: each one's top is looked
/// through but `own`'s, this machine's root, and `removable` says of a
/// volume whether it is a drive that is plugged in and out.
pub fn find(volumes: &Path, own: Option<&str>, removable: &dyn Fn(&Path) -> bool) -> Found {
    let mut found = Found::default();
    let mut left = ENTRIES_MOST;
    let Ok(entries) = fs::read_dir(volumes) else { return found };
    let mut names: Vec<_> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect();
    names.retain(|name| Some(name.as_str()) != own);
    names.sort();
    for name in names {
        let top = volumes.join(&name);
        let mut search = Search {
            volume: &name,
            removable: removable(&top),
            left: &mut left,
            found: &mut found,
        };
        search.look(&top, 0);
    }
    found.sources.sort_by(|a, b| (&b.newest, &a.path).cmp(&(&a.newest, &b.path)));
    found.keys.sort_by(|a, b| a.path.cmp(&b.path));
    found
}

/// `backup find --json`.
pub fn json(found: &Found) -> Value {
    let sources: Vec<Value> = found
        .sources
        .iter()
        .map(|source| {
            json!({
                "source": source.path,
                "machine": source.machine,
                "newest": source.newest,
                "newest_ms": source.newest.as_deref().and_then(time_ms),
                "volume": source.volume,
                "removable": source.removable,
            })
        })
        .collect();
    let keys: Vec<Value> = found
        .keys
        .iter()
        .map(|key| json!({ "path": key.path, "for": key.name, "public": key.public }))
        .collect();
    json!({ "sources": sources, "keys": keys })
}

/// Adds `rows` to `out`, their cells in columns.
fn table(out: &mut String, rows: Vec<Vec<String>>) {
    let columns = rows.first().map_or(0, Vec::len);
    let widths: Vec<usize> = (0..columns)
        .map(|i| rows.iter().map(|row| crate::util::width(&row[i])).max().unwrap_or(0))
        .collect();
    for row in rows {
        let line: Vec<String> =
            row.iter().zip(&widths).map(|(cell, w)| format!("{cell:<w$}", w = *w)).collect();
        out.push_str(line.join("  ").trim_end());
        out.push('\n');
    }
}

/// `backup find` for a person: what was found, as `backup list` is
/// shown.
pub fn text(found: &Found, volumes: &Path) -> String {
    if found.sources.is_empty() && found.keys.is_empty() {
        return format!("nothing found in the volumes under {}\n", volumes.display());
    }
    let mut out = String::new();
    if !found.sources.is_empty() {
        let mut rows = vec![["BACKUPS", "MACHINE", "NEWEST", "VOLUME"].map(String::from).to_vec()];
        for source in &found.sources {
            let newest = source.newest.as_deref().map_or("-".into(), super::shown_time);
            let volume = if source.removable {
                format!("{} (removable)", source.volume)
            } else {
                source.volume.clone()
            };
            rows.push(vec![
                source.path.display().to_string(),
                source.machine.clone(),
                newest,
                volume,
            ]);
        }
        table(&mut out, rows);
    }
    if !found.keys.is_empty() {
        if !found.sources.is_empty() {
            out.push('\n');
        }
        let mut rows = vec![["KEY FILE", "FOR", "PUBLIC KEY"].map(String::from).to_vec()];
        for key in &found.keys {
            rows.push(vec![key.path.display().to_string(), key.name.clone(), key.public.clone()]);
        }
        table(&mut out, rows);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A volumes folder of its own.
    struct Tree(PathBuf);

    impl Tree {
        fn new(name: &str) -> Tree {
            let root =
                std::env::temp_dir().join(format!("backup-find-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(&root).unwrap();
            Tree(root)
        }

        fn put(&self, path: &str, text: &str) {
            let path = self.0.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        }

        /// A machine's backups: the mark, and a record a time.
        fn machine(&self, folder: &str, times: &[&str]) {
            self.put(&format!("{folder}/{MARK}"), "format = \"btrfs\"\n");
            for time in times {
                self.put(&format!("{folder}/{RECORDS}/{time}{ENDING}"), "x");
            }
            fs::create_dir_all(self.0.join(folder).join(RECORDS)).unwrap();
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// The text of a key file as `backup add` writes it, with a key made
    /// for the test.
    fn key_text(name: &str) -> (String, String) {
        let identity = age::x25519::Identity::generate();
        let public = identity.to_public().to_string();
        let text = format!(
            "# The key of backup(8)'s destination {name}, at /volumes/BACKUP/dukovo-backup.\n\
             # public key: {public}\n{}\n",
            age::secrecy::ExposeSecret::expose_secret(&identity.to_string())
        );
        (text, public)
    }

    #[test]
    fn a_machines_backups_are_found_by_their_mark_and_records_to_three_folders_deep() {
        let tree = Tree::new("sources");
        tree.machine("BACKUP/dukovo-backup/duke-nas", &["2026-11-13-013000", "2026-11-14-013000"]);
        tree.machine("OLD/dukovo-backup/duke-old", &["2026-01-02-030000"]);
        tree.machine("OLD/a/b/c/too-deep", &["2026-11-14-013000"]);
        // A folder with the mark and no records is not a machine's.
        tree.put("HALF/dukovo-backup/half/backup.toml", "x");
        // Nor is one with records and no mark.
        tree.put("HALF/dukovo-backup/other/machine/2026-11-14-013000.age", "x");
        // A folder of a machine with no records yet is one with an empty folder.
        tree.machine("NEW/dukovo-backup/duke-new", &[]);
        // Hidden folders are passed over.
        tree.machine("BACKUP/.hidden/duke-hidden", &["2026-11-14-013000"]);
        let found = find(&tree.0, None, &|volume| volume.ends_with("BACKUP"));
        let listed: Vec<(&str, Option<&str>, &str, bool)> = found
            .sources
            .iter()
            .map(|s| (s.machine.as_str(), s.newest.as_deref(), s.volume.as_str(), s.removable))
            .collect();
        assert_eq!(
            listed,
            [
                ("duke-nas", Some("2026-11-14-013000"), "BACKUP", true),
                ("duke-old", Some("2026-01-02-030000"), "OLD", false),
                ("duke-new", None, "NEW", false),
            ]
        );
        assert_eq!(found.sources[0].path, tree.0.join("BACKUP/dukovo-backup/duke-nas"));
        assert!(found.keys.is_empty());
    }

    #[test]
    fn a_key_file_is_known_by_its_two_comment_lines_and_a_decoy_is_not() {
        let tree = Tree::new("keys");
        let (text, public) = key_text("nas");
        tree.put("KEYS/duke-nas-recovery.key", &text);
        let (other, _) = key_text("lab_2");
        tree.put("KEYS/in/a/folder/lab.txt", &other);
        // What is like a key file and is not one.
        tree.put("KEYS/notes.txt", "# The key of my house\nit is under the mat\n");
        tree.put("KEYS/readme", "hello\n");
        tree.put("KEYS/wrong-name.key", &text.replace("destination nas,", "destination -x,"));
        tree.put("KEYS/wrong-public.key", &text.replace("age1", "age2"));
        tree.put("KEYS/no-second-line.key", "# The key of backup(8)'s destination nas, at x.\n");
        tree.put("KEYS/large.key", &format!("{text}{}", "#".repeat(5000)));
        tree.put("KEYS/.hidden.key", &text);
        let found = find(&tree.0, None, &|_| true);
        let keys: Vec<(String, &str, &str)> = found
            .keys
            .iter()
            .map(|k| {
                (
                    k.path.strip_prefix(&tree.0).unwrap().display().to_string(),
                    k.name.as_str(),
                    k.public.as_str(),
                )
            })
            .collect();
        assert_eq!(keys.len(), 2, "{keys:?}");
        assert_eq!(keys[0], ("KEYS/duke-nas-recovery.key".into(), "nas", public.as_str()));
        assert_eq!((keys[1].0.as_str(), keys[1].1), ("KEYS/in/a/folder/lab.txt", "lab_2"));
        assert!(found.sources.is_empty());
    }

    #[test]
    fn this_machines_own_root_is_passed_over() {
        let tree = Tree::new("own");
        let (text, _) = key_text("nas");
        // Spark mounts the root's Btrfs at /volumes/<disk>, where its
        // destinations' key files are this machine's own.
        tree.put("sda3/@settings/backup/nas.key", &text);
        tree.machine("sda3/@users/dukovo-backup/duke-nas", &["2026-11-14-013000"]);
        tree.put("STICK/duke-nas-recovery.key", &text);
        let found = find(&tree.0, Some("sda3"), &|_| false);
        let keys: Vec<&Path> =
            found.keys.iter().map(|k| k.path.strip_prefix(&tree.0).unwrap()).collect();
        assert_eq!(keys, [Path::new("STICK/duke-nas-recovery.key")]);
        assert!(found.sources.is_empty());
        // Without the root's record, it is a volume as any other.
        assert_eq!(find(&tree.0, None, &|_| false).keys.len(), 2);
    }

    #[test]
    fn the_key_itself_is_never_read() {
        let tree = Tree::new("secret");
        let (text, _) = key_text("nas");
        let secret = text.lines().nth(2).unwrap().to_string();
        tree.put("KEYS/nas.key", &text);
        let path = tree.0.join("KEYS/nas.key");
        // The file ends after its second line, and the third is never
        // needed: a key file whose key line is missing is read all the same.
        let two = format!("{}\n", text.lines().take(2).collect::<Vec<_>>().join("\n"));
        tree.put("KEYS/two.key", &two);
        assert!(read_key_file(&tree.0.join("KEYS/two.key")).is_some());
        let (name, public) = read_key_file(&path).unwrap();
        assert_eq!(name, "nas");
        // Nothing the function gives has the key in it.
        assert!(!public.contains(&secret) && !name.contains(&secret));
        let shown = json(&find(&tree.0, None, &|_| false));
        assert!(!shown.to_string().contains(&secret));
        assert!(!text_of(&tree).contains(&secret));
    }

    fn text_of(tree: &Tree) -> String {
        text(&find(&tree.0, None, &|_| false), &tree.0)
    }

    #[test]
    fn a_link_is_not_followed() {
        let tree = Tree::new("links");
        let (text, _) = key_text("nas");
        tree.put("OUT/real.key", &text);
        tree.put("VOL/placeholder", "x");
        std::os::unix::fs::symlink(tree.0.join("OUT/real.key"), tree.0.join("VOL/link.key"))
            .unwrap();
        std::os::unix::fs::symlink(tree.0.join("OUT"), tree.0.join("VOL/linked-folder")).unwrap();
        let found = find(&tree.0, None, &|_| false);
        let keys: Vec<String> = found
            .keys
            .iter()
            .map(|k| k.path.strip_prefix(&tree.0).unwrap().display().to_string())
            .collect();
        assert_eq!(keys, ["OUT/real.key"]);
        assert_eq!(read_key_file(&tree.0.join("VOL/placeholder")), None);
        // Asked of a link directly, it does not follow it either.
        assert_eq!(read_key_file(&tree.0.join("VOL/link.key")), None);
        assert_eq!(read_key_file(&tree.0.join("no-such")), None);
    }

    #[test]
    fn what_is_found_is_said_as_json_and_for_a_person() {
        let tree = Tree::new("said");
        tree.machine("BACKUP/dukovo-backup/duke-nas", &["2026-11-14-013000"]);
        let (key, public) = key_text("nas");
        tree.put("KEYS/duke-nas-recovery.key", &key);
        let found = find(&tree.0, None, &|volume| volume.ends_with("BACKUP"));
        let shown = json(&found);
        let source = tree.0.join("BACKUP/dukovo-backup/duke-nas");
        assert_eq!(
            shown["sources"][0],
            json!({
                "source": source,
                "machine": "duke-nas",
                "newest": "2026-11-14-013000",
                "newest_ms": time_ms("2026-11-14-013000"),
                "volume": "BACKUP",
                "removable": true,
            })
        );
        assert_eq!(
            shown["keys"][0],
            json!({"path": tree.0.join("KEYS/duke-nas-recovery.key"), "for": "nas", "public": public})
        );
        let said = text(&found, &tree.0);
        assert!(said.contains("duke-nas") && said.contains("2026-11-14 01:30:00"), "{said}");
        assert!(said.contains("BACKUP (removable)") && said.contains(&public), "{said}");
        assert_eq!(
            text(&Found::default(), Path::new("/volumes")),
            "nothing found in the volumes under /volumes\n"
        );
    }

    #[test]
    fn a_volumes_folder_that_is_not_there_has_nothing() {
        let found = find(Path::new("/no/such/volumes"), None, &|_| false);
        assert_eq!(found, Found::default());
        assert_eq!(json(&found), json!({"sources": [], "keys": []}));
    }

    #[test]
    fn it_stops_looking_after_its_share_of_entries() {
        let tree = Tree::new("most");
        for i in 0..50 {
            tree.put(&format!("BIG/f{i:03}"), "x");
        }
        tree.machine("ZED/dukovo-backup/z", &["2026-11-14-013000"]);
        let mut left = 10;
        let mut found = Found::default();
        let mut search =
            Search { volume: "BIG", removable: false, left: &mut left, found: &mut found };
        search.look(&tree.0.join("BIG"), 0);
        assert_eq!(left, 0);
        assert_eq!(found, Found::default());
    }
}

// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The folders of the websites (ADR 0569): made, looked at, given a
//! starter page and a place for uploads, held a check file, and removed.
//!
//! A site's folder is the owner's, who may put anything in it, a link
//! among it. So every name below `/shared/www` is looked up through a
//! folder held open, one part at a time and never through a link
//! (`bundles::folder::Dir`), and a file is made new, never over one that
//! is there: a link a person plants cannot make a write of root's land
//! elsewhere. The folders above the site's own are root's, and a folder
//! that is somebody else's or that others may write is refused.

use std::fs::File;
use std::io::{self, Write};
use std::os::fd::AsFd;

use super::FOLDER_MAX;
use super::conf::Kind;
use super::host::Paths;
use super::plan::{self, SITES};
use crate::bundles::folder::Dir;
use crate::util::strip_os_error;

/// The folder the sites' folders are in.
const WWW: &str = "/shared/www";

/// An account's numbers, for the owner of what is made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Owner {
    /// Its user number.
    pub uid: u32,
    /// Its group's number.
    pub gid: u32,
}

/// The parts of `folder` below the sites' folder.
///
/// # Errors
///
/// What is wrong with it: it is not under `/shared/www/sites/`, or a part
/// of it is not one `website` writes into a file as it is.
pub fn parts(folder: &str) -> Result<Vec<&str>, String> {
    let rest = folder
        .strip_prefix(SITES)
        .and_then(|rest| rest.strip_prefix('/'))
        .filter(|rest| !rest.is_empty())
        .ok_or_else(|| format!("{folder} is not a folder under {SITES}/"))?;
    let fits = |part: &&str| {
        let mut bytes = part.bytes();
        let first = bytes.next().is_some_and(|byte| byte.is_ascii_alphanumeric() || byte == b'_');
        first
            && part.len() <= 255
            && bytes.all(|byte| byte.is_ascii_alphanumeric() || b"._@+-".contains(&byte))
    };
    let parts: Vec<&str> = rest.split('/').collect();
    if folder.len() > FOLDER_MAX || !parts.iter().all(fits) {
        return Err(format!(
            "{folder} is not a folder name website writes into a file: each part of it has \
             letters, digits and . _ @ + - and starts with a letter, a digit or _"
        ));
    }
    Ok(parts)
}

/// The site, of `others` (name and folder), whose folder is `folder`, is
/// in or has in it.
pub fn overlap<'a>(folder: &str, others: &'a [(String, String)]) -> Option<&'a str> {
    let inside = |inner: &str, outer: &str| {
        inner == outer || inner.strip_prefix(outer).is_some_and(|rest| rest.starts_with('/'))
    };
    others
        .iter()
        .find(|(_, other)| inside(folder, other) || inside(other, folder))
        .map(|(name, _)| name.as_str())
}

/// What is there of a site's folder.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Seen {
    /// Whether the folder is there.
    pub exists: bool,
    /// How many entries it has itself, not counting what is in them.
    pub entries: usize,
    /// Whose it is.
    pub owner: Option<u32>,
    /// Whether it has an `index.html` or an `index.php`.
    pub indexed: bool,
}

/// The user who runs this, whose folders `website` trusts above a site's.
fn own() -> u32 {
    sysapi::process::euid()
}

/// Who the folders above a site's and the check files belong to: root, as
/// `website` runs as root.
fn root_owner() -> Owner {
    Owner { uid: own(), gid: sysapi::process::egid() }
}

/// Whether `st` is a folder.
fn is_folder(st: &libc::stat) -> bool {
    st.st_mode & libc::S_IFMT == libc::S_IFDIR
}

/// Why the folder at `view`, which `st` describes, is not one `website`
/// makes folders in: it is not its own, or others may write in it.
fn trusted(view: &str, st: &libc::stat) -> Result<(), String> {
    if st.st_uid != own() || st.st_mode & 0o022 != 0 {
        return Err(format!("{view} is not root's alone, so website does not make folders in it"));
    }
    Ok(())
}

/// Opens `view`, a folder below `/shared/www`, one part at a time and
/// never through a link: the folder, or none when a part is not there.
///
/// # Errors
///
/// When a part is a link or is not a folder, or the folders above the
/// site's own are not root's alone.
fn open(paths: &Paths, view: &str, parts: &[&str]) -> Result<Option<Dir>, String> {
    let words = |error: io::Error| format!("{view}: {}", strip_os_error(&error));
    let mut at = match Dir::open(&paths.text(WWW)) {
        Ok(dir) => dir,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("{WWW}: {}", strip_os_error(&error))),
    };
    let mut path = String::from(WWW);
    let names = std::iter::once("sites").chain(parts.iter().copied());
    let last = parts.len();
    for (number, name) in names.enumerate() {
        path.push('/');
        path.push_str(name);
        let Some(st) = at.stat_at(name).map_err(words)? else { return Ok(None) };
        if !is_folder(&st) {
            let what =
                if st.st_mode & libc::S_IFMT == libc::S_IFLNK { "a link" } else { "not a folder" };
            return Err(format!("{path} is {what}"));
        }
        if number < last {
            trusted(&path, &st)?;
        }
        at = at.dir_at(name).map_err(words)?;
    }
    Ok(Some(at))
}

/// What is there of the site's folder `folder`.
///
/// # Errors
///
/// As [`open`].
pub fn inspect(paths: &Paths, folder: &str) -> Result<Seen, String> {
    let parts = parts(folder)?;
    let Some(dir) = open(paths, folder, &parts)? else { return Ok(Seen::default()) };
    let words = |error: io::Error| format!("{folder}: {}", strip_os_error(&error));
    let st = dir.stat().map_err(words)?;
    let names = dir.names().map_err(words)?;
    let indexed = names.iter().any(|name| name == "index.html" || name == "index.php");
    Ok(Seen { exists: true, entries: names.len(), owner: Some(st.st_uid), indexed })
}

/// Whether `/shared/www/sites` is there and root's alone, for the check
/// that comes before it is made.
///
/// # Errors
///
/// When it is there and is not.
pub fn check_sites(paths: &Paths) -> Result<(), String> {
    open(paths, SITES, &[]).map(|_| ())?;
    let Ok(sites) = Dir::open(&paths.text(WWW)) else { return Ok(()) };
    match sites.stat_at("sites") {
        Ok(Some(st)) if is_folder(&st) => trusted(SITES, &st),
        _ => Ok(()),
    }
}

/// What was made, to be taken back: folders and files, in the order made.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Made {
    /// Each entry: a folder or a file, by its path in the machine's view.
    pub entries: Vec<Entry>,
}

/// A folder or a file `website` made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Entry {
    /// A folder.
    Folder(String),
    /// A file.
    File(String),
}

/// Changes the owner of what `fd` holds.
fn chown(fd: impl AsFd, owner: Owner) -> io::Result<()> {
    std::os::unix::fs::fchown(fd, Some(owner.uid), Some(owner.gid))
}

/// Changes the mode of what `fd` holds, whatever the mask says.
fn chmod(fd: impl AsFd, mode: libc::mode_t) -> io::Result<()> {
    sysapi::fs::set_mode(fd, mode)
}

/// Opens the folder `name` of `parent`, making it first, `owner`'s with
/// mode 0755, when it is not there. A folder that is there is not
/// changed.
fn open_or_make(
    parent: &Dir,
    name: &str,
    view: &str,
    owner: Owner,
    made: &mut Made,
) -> Result<Dir, String> {
    let words = |error: io::Error| format!("{view}: {}", strip_os_error(&error));
    match parent.stat_at(name).map_err(words)? {
        Some(st) if is_folder(&st) => parent.dir_at(name).map_err(words),
        Some(st) if st.st_mode & libc::S_IFMT == libc::S_IFLNK => Err(format!("{view} is a link")),
        Some(_) => Err(format!("{view} is not a folder")),
        None => {
            parent.make(name, 0o755).map_err(words)?;
            made.entries.push(Entry::Folder(view.to_string()));
            let dir = parent.dir_at(name).map_err(words)?;
            chown(&dir, owner).map_err(words)?;
            chmod(&dir, 0o755).map_err(words)?;
            Ok(dir)
        }
    }
}

/// Writes the new file `name` in `dir`, which must not be there, with
/// `bytes`, mode 0644, `owner`'s.
fn write_new(dir: &Dir, name: &str, owner: Owner, bytes: &[u8]) -> io::Result<()> {
    let fd = dir.create_new(name, 0o644)?;
    chown(&fd, owner)?;
    chmod(&fd, 0o644)?;
    File::from(fd).write_all(bytes)
}

/// What to make of the site's folder.
pub struct Make<'a> {
    /// The folder, below `/shared/www/sites`.
    pub folder: &'a str,
    /// The site's name, for the starter page.
    pub name: &'a str,
    /// What the site serves.
    pub kind: Kind,
    /// Who the folder and the starter page are the property of.
    pub owner: Owner,
    /// Who the uploads folder of a PHP site is the property of.
    pub php: Owner,
}

/// Makes the site's folder as `what` says: `/shared/www/sites` and the
/// folders above the site's own, root's; the site's folder, the owner's,
/// with a starter page when it has no `index.html` or `index.php`; and a
/// PHP site's `uploads`, PHP's. Whatever is there is kept as it is. What
/// is made is put in `made` as it is made, so that a failure part way
/// can be taken back.
///
/// # Errors
///
/// What stopped it, with the folder.
pub fn make(paths: &Paths, what: &Make<'_>, made: &mut Made) -> Result<(), String> {
    let parts = parts(what.folder)?;
    let root = root_owner();
    let words = |error: io::Error| format!("{WWW}: {}", strip_os_error(&error));
    let mut at = Dir::open(&paths.text(WWW)).map_err(words)?;
    let mut path = String::from(WWW);
    let names = std::iter::once("sites").chain(parts.iter().copied());
    let last = parts.len();
    for (number, name) in names.enumerate() {
        path.push('/');
        path.push_str(name);
        let owner = if number == last { what.owner } else { root };
        if number < last
            && let Some(st) =
                at.stat_at(name).map_err(|e| format!("{path}: {}", strip_os_error(&e)))?
            && is_folder(&st)
        {
            trusted(&path, &st)?;
        }
        at = open_or_make(&at, name, &path, owner, made)?;
    }
    let folder = what.folder;
    let words = |error: io::Error| format!("{folder}: {}", strip_os_error(&error));
    let exists = |name: &str| at.stat_at(name).map(|found| found.is_some());
    if !exists("index.html").map_err(words)? && !exists("index.php").map_err(words)? {
        let (file, text) = plan::starter(what.kind, what.name, folder);
        write_new(&at, file, what.owner, text.as_bytes()).map_err(words)?;
        made.entries.push(Entry::File(format!("{folder}/{file}")));
    }
    if what.kind == Kind::Php {
        open_or_make(&at, "uploads", &format!("{folder}/uploads"), what.php, made)?;
    }
    Ok(())
}

/// Takes back what `made` holds, the last made first: a file is removed,
/// a folder when it is empty. What could not be is told.
pub fn undo(paths: &Paths, made: &Made) -> Vec<String> {
    let mut left = Vec::new();
    for entry in made.entries.iter().rev() {
        let (view, folder) = match entry {
            Entry::File(view) => (view, false),
            Entry::Folder(view) => (view, true),
        };
        let Some((above, name)) = view.rsplit_once('/') else { continue };
        let parts: Vec<&str> = above
            .strip_prefix(SITES)
            .unwrap_or_default()
            .split('/')
            .filter(|p| !p.is_empty())
            .collect();
        let opened = if above == WWW {
            Dir::open(&paths.text(WWW)).map(Some).map_err(|e| strip_os_error(&e))
        } else {
            open(paths, above, &parts)
        };
        let result = match opened {
            Ok(Some(parent)) if folder => rmdir(&parent, name).map_err(|e| strip_os_error(&e)),
            Ok(Some(parent)) => parent.unlink(name).map_err(|e| strip_os_error(&e)),
            Ok(None) => Ok(()),
            Err(why) => Err(why),
        };
        if let Err(why) = result {
            left.push(format!("{view} was not taken away: {why}"));
        }
    }
    left
}

/// Removes the empty folder `name` of `parent`.
fn rmdir(parent: &Dir, name: &str) -> io::Result<()> {
    parent.remove_dir(name)
}

/// Removes the site's folder `folder`, and all that is in it.
///
/// # Errors
///
/// What stopped it.
pub fn remove(paths: &Paths, folder: &str) -> Result<(), String> {
    let parts = parts(folder)?;
    let Some((last, above)) = parts.split_last() else { return Ok(()) };
    let words = |error: io::Error| format!("{folder}: {}", strip_os_error(&error));
    let Some(parent) = open(paths, folder, above)? else { return Ok(()) };
    parent.remove_tree(last, 0).map_err(words)
}

/// The files `website` puts in a site's folder to ask for them, which it
/// takes away again when this is dropped.
pub struct CheckFiles {
    dir: Dir,
    names: Vec<String>,
}

impl CheckFiles {
    /// Makes the check files in `folder`: a text file that holds
    /// `content`, and for PHP a file that prints it. `hex` makes their
    /// names; the files are root's, mode 0644.
    ///
    /// # Errors
    ///
    /// When the folder cannot be opened, or a file made.
    pub fn make(
        paths: &Paths,
        folder: &str,
        hex: &str,
        content: &str,
        php: bool,
    ) -> Result<CheckFiles, String> {
        let parts = parts(folder)?;
        let words = |error: io::Error| format!("{folder}: {}", strip_os_error(&error));
        let dir = open(paths, folder, &parts)?.ok_or_else(|| format!("{folder} is not there"))?;
        let mut files = CheckFiles { dir, names: Vec::new() };
        let root = root_owner();
        let mut put = |name: String, bytes: String| -> Result<(), String> {
            write_new(&files.dir, &name, root, bytes.as_bytes()).map_err(words)?;
            files.names.push(name);
            Ok(())
        };
        put(format!("website-check-{hex}.txt"), content.to_string())?;
        if php {
            put(format!("website-check-{hex}.php"), format!("<?php\necho \"{content}\";\n"))?;
        }
        Ok(files)
    }
}

impl Drop for CheckFiles {
    fn drop(&mut self) {
        for name in &self.names {
            let _ = self.dir.unlink(name);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::path::Path;

    use super::*;

    /// A tree with `/shared/www` in it, removed when dropped.
    struct Scratch(std::path::PathBuf);

    impl Scratch {
        fn new(tag: &str) -> Scratch {
            let path =
                std::env::temp_dir().join(format!("website-folder-{tag}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(path.join("shared/www")).unwrap();
            Scratch(path)
        }

        fn paths(&self) -> Paths {
            Paths::under(&self.0)
        }

        fn at(&self, view: &str) -> std::path::PathBuf {
            self.0.join(view.trim_start_matches('/'))
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn me() -> Owner {
        Owner { uid: sysapi::process::euid(), gid: sysapi::process::egid() }
    }

    fn make_of<'a>(folder: &'a str, kind: Kind) -> Make<'a> {
        Make { folder, name: "shop.example.org", kind, owner: me(), php: me() }
    }

    const FOLDER: &str = "/shared/www/sites/shop.example.org";

    #[test]
    fn a_folder_is_under_the_sites_folder_and_each_part_is_one_website_writes_as_it_is() {
        assert_eq!(parts(FOLDER).unwrap(), ["shop.example.org"]);
        assert_eq!(
            parts("/shared/www/sites/clients/shop_1@2+x-y.z").unwrap(),
            ["clients", "shop_1@2+x-y.z"]
        );
        for bad in [
            "/shared/www/sites",
            "/shared/www/sites/",
            "/shared/www/sitesx/a",
            "/etc",
            "/shared/www/sites/..",
            "/shared/www/sites/a/../b",
            "/shared/www/sites/a//b",
            "/shared/www/sites/a/",
            "/shared/www/sites/.hidden",
            "/shared/www/sites/a b",
            "/shared/www/sites/a;b",
            "/shared/www/sites/a$b",
            "/shared/www/sites/a\"b",
            "/shared/www/sites/a{b",
            "/shared/www/sites/a#b",
            "/shared/www/sites/-a",
            "/shared/www/sites/a\nb",
            "/shared/www/sites/é",
            "shared/www/sites/a",
        ] {
            assert!(parts(bad).is_err(), "{bad:?}");
        }
        let long = format!("{SITES}/{}", "a".repeat(FOLDER_MAX));
        assert!(parts(&long).is_err());
    }

    #[test]
    fn a_folder_in_another_site_s_folder_or_holding_it_overlaps() {
        let others = vec![("a".to_string(), "/shared/www/sites/a".to_string())];
        assert_eq!(overlap("/shared/www/sites/a", &others), Some("a"));
        assert_eq!(overlap("/shared/www/sites/a/b", &others), Some("a"));
        assert_eq!(overlap("/shared/www/sites", &others), Some("a"));
        assert_eq!(overlap("/shared/www/sites/ab", &others), None);
        assert_eq!(overlap("/shared/www/sites/b", &others), None);
        assert_eq!(overlap("/shared/www/sites/b", &[]), None);
    }

    #[test]
    fn the_sites_folder_and_the_site_s_are_made_with_a_starter_page() {
        let tree = Scratch::new("make");
        let mut made = Made::default();
        make(&tree.paths(), &make_of(FOLDER, Kind::Static), &mut made).unwrap();
        let site = tree.at(FOLDER);
        assert_eq!(fs::metadata(tree.at(SITES)).unwrap().permissions().mode() & 0o777, 0o755);
        assert_eq!(fs::metadata(&site).unwrap().permissions().mode() & 0o777, 0o755);
        let page = fs::read_to_string(site.join("index.html")).unwrap();
        assert!(page.contains(
            "shop.example.org works. Put your site's files in /shared/www/sites/shop.example.org."
        ));
        assert_eq!(
            fs::metadata(site.join("index.html")).unwrap().permissions().mode() & 0o777,
            0o644
        );
        assert!(!site.join("uploads").exists());
        assert_eq!(
            made.entries,
            [
                Entry::Folder(SITES.into()),
                Entry::Folder(FOLDER.into()),
                Entry::File(format!("{FOLDER}/index.html")),
            ]
        );
        let seen = inspect(&tree.paths(), FOLDER).unwrap();
        assert_eq!((seen.exists, seen.entries, seen.indexed), (true, 1, true));
        assert_eq!(seen.owner, Some(me().uid));
    }

    #[test]
    fn a_php_site_gets_an_index_php_and_an_uploads_folder() {
        let tree = Scratch::new("php");
        make(&tree.paths(), &make_of(FOLDER, Kind::Php), &mut Made::default()).unwrap();
        let site = tree.at(FOLDER);
        assert!(
            fs::read_to_string(site.join("index.php"))
                .unwrap()
                .starts_with("<?php\necho \"shop.example.org works.")
        );
        assert!(!site.join("index.html").exists());
        assert!(site.join("uploads").is_dir());
    }

    #[test]
    fn a_folder_that_is_there_is_kept_as_it_is_and_its_index_is_not_written_over() {
        let tree = Scratch::new("kept");
        let site = tree.at(FOLDER);
        fs::create_dir_all(&site).unwrap();
        fs::write(site.join("index.html"), "mine").unwrap();
        fs::set_permissions(&site, fs::Permissions::from_mode(0o700)).unwrap();
        fs::set_permissions(tree.at(SITES), fs::Permissions::from_mode(0o755)).unwrap();
        let mut made = Made::default();
        make(&tree.paths(), &make_of(FOLDER, Kind::Php), &mut made).unwrap();
        assert_eq!(fs::read_to_string(site.join("index.html")).unwrap(), "mine");
        assert!(!site.join("index.php").exists(), "an index file is there");
        assert_eq!(fs::metadata(&site).unwrap().permissions().mode() & 0o777, 0o700);
        // Only the uploads folder was made.
        assert_eq!(made.entries, [Entry::Folder(format!("{FOLDER}/uploads"))]);
        // A second run makes nothing.
        let mut again = Made::default();
        make(&tree.paths(), &make_of(FOLDER, Kind::Php), &mut again).unwrap();
        assert!(again.entries.is_empty());
    }

    #[test]
    fn a_folder_with_other_folders_above_it_makes_them_root_s() {
        let tree = Scratch::new("nested");
        let folder = "/shared/www/sites/clients/shop";
        let mut made = Made::default();
        make(&tree.paths(), &make_of(folder, Kind::Static), &mut made).unwrap();
        assert!(tree.at(folder).join("index.html").is_file());
        assert_eq!(made.entries.len(), 4);
        assert_eq!(made.entries[1], Entry::Folder("/shared/www/sites/clients".into()));
    }

    #[test]
    fn a_link_in_the_way_is_refused_at_every_part_and_nothing_is_written_through_it() {
        let tree = Scratch::new("links");
        let elsewhere = tree.at("/elsewhere");
        fs::create_dir_all(&elsewhere).unwrap();
        // The folder itself is a link.
        fs::create_dir_all(tree.at(SITES)).unwrap();
        fs::set_permissions(tree.at(SITES), fs::Permissions::from_mode(0o755)).unwrap();
        symlink(&elsewhere, tree.at(FOLDER)).unwrap();
        let why =
            make(&tree.paths(), &make_of(FOLDER, Kind::Static), &mut Made::default()).unwrap_err();
        assert!(why.contains("is a link"), "{why}");
        assert!(inspect(&tree.paths(), FOLDER).unwrap_err().contains("is a link"));
        assert_eq!(fs::read_dir(&elsewhere).unwrap().count(), 0, "nothing was written through it");
        // A link above it.
        fs::remove_file(tree.at(FOLDER)).unwrap();
        symlink(&elsewhere, tree.at("/shared/www/sites/clients")).unwrap();
        let below = "/shared/www/sites/clients/shop";
        let why =
            make(&tree.paths(), &make_of(below, Kind::Static), &mut Made::default()).unwrap_err();
        assert!(why.contains("clients is a link"), "{why}");
        assert_eq!(fs::read_dir(&elsewhere).unwrap().count(), 0);
        // The sites folder is a link.
        let tree = Scratch::new("links2");
        symlink(tree.at("/"), tree.at(SITES)).unwrap();
        assert!(
            make(&tree.paths(), &make_of(FOLDER, Kind::Static), &mut Made::default())
                .unwrap_err()
                .contains("is a link")
        );
        // A file where a folder should be.
        let tree = Scratch::new("links3");
        fs::create_dir_all(tree.at(SITES)).unwrap();
        fs::set_permissions(tree.at(SITES), fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(tree.at(FOLDER), "x").unwrap();
        assert!(
            make(&tree.paths(), &make_of(FOLDER, Kind::Static), &mut Made::default())
                .unwrap_err()
                .contains("is not a folder")
        );
    }

    #[test]
    fn a_folder_above_the_site_s_that_others_may_write_in_is_refused() {
        let tree = Scratch::new("open");
        fs::create_dir_all(tree.at("/shared/www/sites/clients")).unwrap();
        fs::set_permissions(
            tree.at("/shared/www/sites/clients"),
            fs::Permissions::from_mode(0o777),
        )
        .unwrap();
        fs::set_permissions(tree.at(SITES), fs::Permissions::from_mode(0o755)).unwrap();
        let why = make(
            &tree.paths(),
            &make_of("/shared/www/sites/clients/shop", Kind::Static),
            &mut Made::default(),
        )
        .unwrap_err();
        assert!(why.contains("is not root's alone"), "{why}");
        assert!(!tree.at("/shared/www/sites/clients/shop").exists());
        fs::set_permissions(
            tree.at("/shared/www/sites/clients"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        fs::set_permissions(tree.at(SITES), fs::Permissions::from_mode(0o775)).unwrap();
        assert!(check_sites(&tree.paths()).unwrap_err().contains("not root's alone"));
        fs::set_permissions(tree.at(SITES), fs::Permissions::from_mode(0o755)).unwrap();
        assert!(check_sites(&tree.paths()).is_ok());
    }

    #[test]
    fn a_folder_that_is_not_there_is_nothing_and_the_sites_folder_may_be_missing() {
        let tree = Scratch::new("none");
        assert_eq!(inspect(&tree.paths(), FOLDER).unwrap(), Seen::default());
        assert!(check_sites(&tree.paths()).is_ok());
        fs::remove_dir(tree.at("/shared/www")).unwrap();
        assert_eq!(inspect(&tree.paths(), FOLDER).unwrap(), Seen::default());
        let why =
            make(&tree.paths(), &make_of(FOLDER, Kind::Static), &mut Made::default()).unwrap_err();
        assert!(why.starts_with("/shared/www: "), "{why}");
    }

    #[test]
    fn what_was_made_is_taken_back_last_first_and_a_folder_with_files_is_left() {
        let tree = Scratch::new("undo");
        let mut made = Made::default();
        make(&tree.paths(), &make_of(FOLDER, Kind::Php), &mut made).unwrap();
        assert!(undo(&tree.paths(), &made).is_empty());
        assert!(!tree.at(SITES).exists(), "the sites folder was made, so it goes too");
        // Something the owner put in the meantime is not removed.
        let mut made = Made::default();
        make(&tree.paths(), &make_of(FOLDER, Kind::Static), &mut made).unwrap();
        fs::write(tree.at(FOLDER).join("theirs.txt"), "x").unwrap();
        let left = undo(&tree.paths(), &made);
        assert_eq!(left.len(), 2, "{left:?}");
        assert!(left[0].starts_with(FOLDER), "{left:?}");
        assert!(tree.at(FOLDER).join("theirs.txt").exists());
        assert!(!tree.at(FOLDER).join("index.html").exists());
    }

    #[test]
    fn a_site_s_folder_is_removed_with_what_is_in_it_and_a_link_in_it_is_not_followed() {
        let tree = Scratch::new("remove");
        make(&tree.paths(), &make_of(FOLDER, Kind::Static), &mut Made::default()).unwrap();
        let outside = tree.at("/outside");
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("keep.txt"), "x").unwrap();
        let site = tree.at(FOLDER);
        fs::create_dir_all(site.join("a/b")).unwrap();
        fs::write(site.join("a/b/c.txt"), "x").unwrap();
        symlink(&outside, site.join("link")).unwrap();
        remove(&tree.paths(), FOLDER).unwrap();
        assert!(!site.exists());
        assert!(outside.join("keep.txt").exists(), "what the link led to is not touched");
        assert!(tree.at(SITES).is_dir(), "the sites folder stays");
        // A folder that is not there is not a failure.
        remove(&tree.paths(), FOLDER).unwrap();
        assert!(remove(&tree.paths(), "/shared/www/sites").is_err());
        assert!(remove(&tree.paths(), "/shared/www").is_err());
    }

    #[test]
    fn the_check_files_are_new_root_s_and_gone_when_dropped_and_never_over_a_link() {
        let tree = Scratch::new("check");
        make(&tree.paths(), &make_of(FOLDER, Kind::Php), &mut Made::default()).unwrap();
        let site = tree.at(FOLDER);
        {
            let files = CheckFiles::make(&tree.paths(), FOLDER, "abc", "content", true).unwrap();
            assert_eq!(files.names, ["website-check-abc.txt", "website-check-abc.php"]);
            assert_eq!(fs::read_to_string(site.join("website-check-abc.txt")).unwrap(), "content");
            assert_eq!(
                fs::read_to_string(site.join("website-check-abc.php")).unwrap(),
                "<?php\necho \"content\";\n"
            );
            assert_eq!(
                fs::metadata(site.join("website-check-abc.txt")).unwrap().permissions().mode()
                    & 0o777,
                0o644
            );
        }
        assert!(
            !site.join("website-check-abc.txt").exists()
                && !site.join("website-check-abc.php").exists()
        );
        // A link at the name is not followed: the file is not made, and
        // what the link leads to is not changed.
        let target = tree.at("/target.txt");
        fs::write(&target, "mine").unwrap();
        symlink(&target, site.join("website-check-def.txt")).unwrap();
        assert!(CheckFiles::make(&tree.paths(), FOLDER, "def", "content", false).is_err());
        assert_eq!(fs::read_to_string(&target).unwrap(), "mine");
        assert!(
            Path::new(&site.join("website-check-def.txt")).is_symlink(),
            "a link that was there stays"
        );
        // A folder that is not there.
        assert!(
            CheckFiles::make(&tree.paths(), "/shared/www/sites/none", "x", "c", false).is_err()
        );
    }
}

// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Where the windows were, for those that show again to show there
//! (`dukovo_session_v1`, ADR 0729).
//!
//! A program gives a window a name that lasts longer than the window
//! does: its main window, the terminal of one profile. With the name
//! of the window's application it is the window's [`Key`]. windowd
//! keeps a table of where each window with a key last was (a
//! [`Place`]): the screen, the corner and the size of its frame there,
//! what of the screen it filled, its desktop, and whether it was
//! minimized. A window that shows with a key the table has shows on
//! that screen, at that corner and size, filling what it filled
//! ([`crate::windows`]).
//!
//! The table is a file among the person's own, written a second after
//! the windows last changed, whole and beside the old one, which it
//! then takes the place of. It outlives windowd, and so do the places.
//!
//! The protocol is spoken from the table of `window-protocols`, with
//! no code written by a tool.

use std::cell::{Cell, RefCell};
use std::fmt::Display;
use std::fs;
use std::io;
use std::os::fd::OwnedFd;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{Map, Value, json};
use smithay::desktop::Window;
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::XdgToplevel;
use smithay::reexports::wayland_server::backend::protocol::{Argument, Message};
use smithay::reexports::wayland_server::backend::{
    ClientId, GlobalHandler, GlobalId, Handle, ObjectData, ObjectId,
};
use smithay::reexports::wayland_server::{DisplayHandle, Resource};
use window_protocols::session::{MANAGER, NAME_MOST, manager};

use crate::shell::{Shell, app_of};
use crate::windows::zones::{Fills, Level, Side, Zone};

/// The file the table is kept in, from the person's home folder.
pub const FILE: &str = ".config/windowd-session.json";

/// How many windows' places are kept at the most. When there are more,
/// those that changed longest ago are let go of.
const KEPT_MOST: usize = 256;

/// How long the windows have to have stayed as they are before the
/// table is written, in nanoseconds: a window that is dragged changes
/// its place with every motion.
const QUIET_NS: u64 = 1_000_000_000;

/// What a window is known again by.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Key {
    /// The name of its application (`xdg_toplevel.set_app_id`).
    pub app: String,
    /// The name its program gave it.
    pub name: String,
}

/// Where a window was.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Place {
    /// The name of the screen it was on, and how wide and high that
    /// screen's area of the desktop was.
    pub screen: String,
    pub screen_size: (i32, i32),
    /// Which of the screen's desktops it was on.
    pub desk: usize,
    /// The corner of its frame's box, from the corner of the screen's
    /// area, while it filled nothing of the screen; `None` for a window
    /// that never did.
    pub at: Option<(i32, i32)>,
    /// How wide and high its frame's box was then.
    pub frame: Option<(i32, i32)>,
    /// What of the screen it filled: nothing, all of it, or a zone.
    pub fills: Fills,
    /// Whether it was minimized.
    pub minimized: bool,
}

/// The name a program gave a window, kept with the window.
#[derive(Debug, Default)]
struct Named(RefCell<Option<String>>);

/// The key of `window`: none for a window whose program gave it no
/// name, or whose application has none.
pub fn key_of(window: &Window) -> Option<Key> {
    let name = window.user_data().get::<Named>()?.0.borrow().clone()?;
    let app = app_of(window.toplevel()?);
    (!app.is_empty()).then_some(Key { app, name })
}

/// The table of the places, and the file it is kept in.
#[derive(Debug, Default)]
pub struct Places {
    /// The one that changed last is last.
    kept: Vec<(Key, Place)>,
    file: Option<PathBuf>,
    /// Whether the table has changes the file has not, and the time it
    /// was first asked when it is due since the last of them.
    changed: bool,
    since: Cell<Option<u64>>,
}

impl Places {
    /// The table that is kept in `file`, and what is wrong with the
    /// file if it cannot be read: the table is then empty, and the file
    /// is written anew with the next change.
    pub fn read(file: PathBuf) -> (Places, Option<String>) {
        let (kept, wrong) = match fs::read_to_string(&file) {
            Ok(text) => match table_of(&text) {
                Ok(kept) => (kept, None),
                Err(why) => (Vec::new(), Some(format!("{}: {why}", file.display()))),
            },
            Err(error) if error.kind() == io::ErrorKind::NotFound => (Vec::new(), None),
            Err(error) => (Vec::new(), Some(format!("{}: {error}", file.display()))),
        };
        (Places { kept, file: Some(file), changed: false, since: Cell::new(None) }, wrong)
    }

    /// Where the window of `key` last was.
    pub fn of(&self, key: &Key) -> Option<&Place> {
        self.kept.iter().find(|(kept, _)| kept == key).map(|(_, place)| place)
    }

    /// The windows with keys are at `placed` now.
    pub fn note(&mut self, placed: Vec<(Key, Place)>) {
        for (key, place) in placed {
            let known = self.kept.iter().position(|(kept, _)| *kept == key);
            if known.is_some_and(|known| self.kept[known].1 == place) {
                continue;
            }
            if let Some(known) = known {
                self.kept.remove(known);
            }
            self.kept.push((key, place));
            (self.changed, self.since) = (true, Cell::new(None));
        }
        let over = self.kept.len().saturating_sub(KEPT_MOST);
        self.kept.drain(..over);
    }

    /// When the table is to be written, on the monotonic clock, if it
    /// has changes the file has not: [`QUIET_NS`] after the first time
    /// this is asked since the last of them, which is `now_ns` if this
    /// is that time.
    pub fn due(&self, now_ns: u64) -> Option<u64> {
        if !self.changed || self.file.is_none() {
            return None;
        }
        let since = self.since.get().unwrap_or(now_ns);
        self.since.set(Some(since));
        Some(since + QUIET_NS)
    }

    /// Writes the table to its file, if it has changes the file has
    /// not. The file is written whole beside the old one and then takes
    /// its place, so that no reader finds half of it.
    ///
    /// # Errors
    ///
    /// Why the file was not written, for the person to read. The
    /// changes are written with the next ones.
    pub fn write(&mut self) -> Result<(), String> {
        let Some(file) = self.file.as_deref().filter(|_| self.changed) else { return Ok(()) };
        (self.changed, self.since) = (false, Cell::new(None));
        let said = |why: &dyn Display| format!("{}: {why}", file.display());
        if let Some(folder) = file.parent() {
            fs::create_dir_all(folder).map_err(|err| said(&err))?;
        }
        let mut beside = file.as_os_str().to_owned();
        beside.push(".new");
        let beside = PathBuf::from(beside);
        fs::write(&beside, text_of(&self.kept)).map_err(|err| said(&err))?;
        fs::rename(&beside, file).map_err(|err| said(&err))
    }
}

/// The names of the zones in the file.
const ZONES: [(&str, Zone); 6] = [
    ("left", Zone::Half(Side::Left)),
    ("right", Zone::Half(Side::Right)),
    ("left-upper", Zone::Quarter(Side::Left, Level::Upper)),
    ("left-lower", Zone::Quarter(Side::Left, Level::Lower)),
    ("right-upper", Zone::Quarter(Side::Right, Level::Upper)),
    ("right-lower", Zone::Quarter(Side::Right, Level::Lower)),
];

/// What a window fills as the file names it. A window that is full
/// screen is kept as what it filled before.
fn fills_name(fills: Fills) -> &'static str {
    match fills {
        Fills::Nothing | Fills::Screen => "nothing",
        Fills::All => "all",
        Fills::Zone(zone) => {
            ZONES.iter().find(|(_, named)| *named == zone).map_or("nothing", |z| z.0)
        }
    }
}

/// What the file names so.
fn fills_named(name: &str) -> Option<Fills> {
    match name {
        "nothing" => Some(Fills::Nothing),
        "all" => Some(Fills::All),
        zone => ZONES.iter().find(|(named, _)| *named == zone).map(|(_, zone)| Fills::Zone(*zone)),
    }
}

/// The file's text for the table `kept`.
fn text_of(kept: &[(Key, Place)]) -> String {
    let pair =
        |pair: Option<(i32, i32)>| pair.map_or(Value::Null, |(one, other)| json!([one, other]));
    let window = |(key, place): &(Key, Place)| {
        json!({
            "app": key.app,
            "name": key.name,
            "screen": place.screen,
            "screen_size": pair(Some(place.screen_size)),
            "desktop": place.desk,
            "at": pair(place.at),
            "frame": pair(place.frame),
            "fills": fills_name(place.fills),
            "minimized": place.minimized,
        })
    };
    let root = json!({ "windows": kept.iter().map(window).collect::<Vec<Value>>() });
    let mut text = serde_json::to_string_pretty(&root).unwrap_or_default();
    text.push('\n');
    text
}

/// The table the file's `text` holds. A window's entry that is not
/// what windowd writes is left out.
///
/// # Errors
///
/// What is wrong with a text that holds no table.
fn table_of(text: &str) -> Result<Vec<(Key, Place)>, String> {
    let root = serde_json::from_str::<Value>(text).map_err(|err| format!("not JSON: {err}"))?;
    let windows = root.get("windows").and_then(Value::as_array).ok_or("no list of `windows`")?;
    let mut kept: Vec<(Key, Place)> = windows.iter().filter_map(entry_of).collect();
    let over = kept.len().saturating_sub(KEPT_MOST);
    kept.drain(..over);
    Ok(kept)
}

/// One window's entry of the file.
fn entry_of(entry: &Value) -> Option<(Key, Place)> {
    let entry: &Map<String, Value> = entry.as_object()?;
    let text = |name: &str| entry.get(name)?.as_str().map(str::to_owned);
    let pair = |name: &str| -> Option<Option<(i32, i32)>> {
        match entry.get(name)? {
            Value::Null => Some(None),
            Value::Array(pair) => {
                let [one, other] = &pair[..] else { return None };
                let side = |side: &Value| i32::try_from(side.as_i64()?).ok();
                Some(Some((side(one)?, side(other)?)))
            }
            _ => None,
        }
    };
    let key = Key { app: text("app")?, name: text("name")? };
    let named = !key.app.is_empty() && (1..=NAME_MOST).contains(&key.name.len());
    let place = Place {
        screen: text("screen")?,
        screen_size: pair("screen_size")??,
        desk: usize::try_from(entry.get("desktop")?.as_u64()?).ok()?,
        at: pair("at")?,
        frame: pair("frame")?,
        fills: fills_named(entry.get("fills")?.as_str()?)?,
        minimized: entry.get("minimized")?.as_bool()?,
    };
    let sized = |(width, height): (i32, i32)| width > 0 && height > 0;
    (named && sized(place.screen_size) && place.frame.is_none_or(sized)).then_some((key, place))
}

/// The global, and each program's bound manager: a manager keeps
/// nothing.
#[derive(Debug)]
struct Manager;

/// Offers the session manager on `display`. The global stays for as
/// long as the display does.
pub fn offer(display: &DisplayHandle) -> GlobalId {
    display.backend_handle().create_global::<Shell>(&MANAGER, MANAGER.version, Arc::new(Manager))
}

impl GlobalHandler<Shell> for Manager {
    fn bind(
        self: Arc<Self>,
        _handle: &Handle,
        _shell: &mut Shell,
        _client: ClientId,
        _global: GlobalId,
        _manager: ObjectId,
    ) -> Arc<dyn ObjectData<Shell>> {
        self
    }
}

impl ObjectData<Shell> for Manager {
    fn request(
        self: Arc<Self>,
        handle: &Handle,
        shell: &mut Shell,
        _client: ClientId,
        request: Message<ObjectId, OwnedFd>,
    ) -> Option<Arc<dyn ObjectData<Shell>>> {
        // The backend has held the arguments to the table: the other
        // request, `destroy`, has none.
        let [Argument::Object(toplevel), Argument::Str(Some(name))] = &request.args[..] else {
            return None;
        };
        debug_assert_eq!(request.opcode, manager::NAME_WINDOW);
        let name = name.to_str().ok().filter(|name| (1..=NAME_MOST).contains(&name.len()));
        let Some(name) = name else {
            let said = c"a window's name is 1 to 255 bytes of UTF-8 text".to_owned();
            handle.post_error(request.sender_id, manager::BAD_NAME, said);
            return None;
        };
        let display = DisplayHandle::from(handle.clone());
        let toplevel = XdgToplevel::from_id(&display, toplevel.clone()).ok()?;
        // A window that is gone has no name to give.
        let window = shell.window_of_toplevel(&toplevel)?;
        let named = window.user_data().get_or_insert(Named::default);
        // The name is one window's at a time: while another of the
        // application's has it, this one has none.
        *named.0.borrow_mut() = None;
        let key = Key { app: app_of(window.toplevel()?), name: name.to_owned() };
        if !shell.has_key(&key) {
            *named.0.borrow_mut() = Some(key.name);
        }
        None
    }

    fn destroyed(
        self: Arc<Self>,
        _handle: &Handle,
        _shell: &mut Shell,
        _client: ClientId,
        _manager: ObjectId,
    ) {
    }
}

/// The file the person's windows' places are kept in, if the
/// environment says where their home folder is.
pub fn personal() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").filter(|home| !home.is_empty())?;
    Some(Path::new(&home).join(FILE))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(name: &str) -> Key {
        Key { app: "org.example.editor".to_owned(), name: name.to_owned() }
    }

    fn place(at: (i32, i32), fills: Fills) -> Place {
        Place {
            screen: "Virtual-1".to_owned(),
            screen_size: (1024, 768),
            desk: 1,
            at: Some(at),
            frame: Some((322, 232)),
            fills,
            minimized: false,
        }
    }

    #[test]
    fn the_table_reads_back_from_the_text_it_is_written_as() {
        let upper = Fills::Zone(Zone::Quarter(Side::Right, Level::Upper));
        let never = Place { at: None, frame: None, minimized: true, ..place((0, 0), Fills::All) };
        let kept = vec![
            (key("main"), place((40, -3), Fills::Nothing)),
            (key("side"), place((7, 9), upper)),
            (key("log"), never),
        ];
        let text = text_of(&kept);
        assert!(text.ends_with("}\n") && text.contains("\"fills\": \"right-upper\""), "{text}");
        assert_eq!(table_of(&text), Ok(kept));
        for (name, zone) in ZONES {
            assert_eq!(
                fills_named(fills_name(Fills::Zone(zone))),
                Some(Fills::Zone(zone)),
                "{name}"
            );
        }
        // A window that is full screen is kept as one that fills
        // nothing: where it was before is what is kept of it.
        assert_eq!(fills_name(Fills::Screen), "nothing");
    }

    #[test]
    fn an_entry_that_is_not_what_windowd_writes_is_left_out() {
        let good = text_of(&[(key("main"), place((1, 2), Fills::Nothing))]);
        let mut root: Value = serde_json::from_str(&good).unwrap();
        let entry = root["windows"][0].clone();
        let spoiled = |name: &str, value: Value| {
            let mut entry = entry.clone();
            entry[name] = value;
            entry
        };
        root["windows"] = json!([
            spoiled("name", json!("")),
            spoiled("app", json!("")),
            spoiled("fills", json!("middle")),
            spoiled("frame", json!([0, 10])),
            spoiled("at", json!([1])),
            spoiled("desktop", json!(-1)),
            spoiled("screen_size", Value::Null),
            json!("a window"),
            entry,
        ]);
        let kept = table_of(&root.to_string()).unwrap();
        assert_eq!(kept, [(key("main"), place((1, 2), Fills::Nothing))]);
        assert!(table_of("[]").unwrap_err().contains("no list"));
        assert!(table_of("{").unwrap_err().starts_with("not JSON"));
    }

    #[test]
    fn a_place_that_changed_is_due_a_second_after_it_is_first_asked_about() {
        let mut places =
            Places { file: Some(PathBuf::from("/nowhere/kept.json")), ..Places::default() };
        assert_eq!(places.due(5), None);
        places.note(vec![(key("main"), place((1, 2), Fills::Nothing))]);
        assert_eq!(places.due(100), Some(100 + QUIET_NS));
        assert_eq!(places.due(900), Some(100 + QUIET_NS));
        // The same place again is no change; another is, and the wait
        // starts anew.
        places.note(vec![(key("main"), place((1, 2), Fills::Nothing))]);
        assert_eq!(places.due(950), Some(100 + QUIET_NS));
        places.note(vec![(key("main"), place((3, 2), Fills::Nothing))]);
        assert_eq!(places.due(2000), Some(2000 + QUIET_NS));
        assert_eq!(places.of(&key("main")), Some(&place((3, 2), Fills::Nothing)));
        // A table with no file is never due.
        let mut nowhere = Places::default();
        nowhere.note(vec![(key("main"), place((1, 2), Fills::Nothing))]);
        assert_eq!(nowhere.due(5), None);
    }

    #[test]
    fn the_places_that_changed_longest_ago_are_let_go_of_first() {
        let mut places = Places::default();
        for number in 0..KEPT_MOST + 2 {
            places.note(vec![(key(&format!("w{number}")), place((1, 2), Fills::Nothing))]);
        }
        // The first is noted again before the table is full.
        assert_eq!(places.kept.len(), KEPT_MOST);
        assert_eq!(places.of(&key("w0")), None);
        assert_eq!(places.of(&key("w1")), None);
        assert!(places.of(&key("w2")).is_some());
        places.note(vec![(key("w2"), place((9, 9), Fills::Nothing))]);
        places.note(vec![(key("new"), place((1, 2), Fills::Nothing))]);
        assert!(places.of(&key("w2")).is_some());
        assert_eq!(places.of(&key("w3")), None);
    }

    #[test]
    fn the_file_is_written_whole_and_read_again() {
        let folder = std::env::temp_dir().join(format!("windowd-places-{}", std::process::id()));
        let _ = fs::remove_dir_all(&folder);
        let file = folder.join("home/.config/kept.json");
        let (mut places, wrong) = Places::read(file.clone());
        assert_eq!((places.kept.len(), wrong), (0, None));
        places.note(vec![(key("main"), place((40, 60), Fills::All))]);
        places.write().unwrap();
        assert_eq!(places.due(7), None);
        let (again, wrong) = Places::read(file.clone());
        assert_eq!(wrong, None);
        assert_eq!(again.of(&key("main")), Some(&place((40, 60), Fills::All)));
        let beside: Vec<_> = fs::read_dir(file.parent().unwrap()).unwrap().collect();
        assert_eq!(beside.len(), 1, "{beside:?}");
        // A file that is no table is said to be, and written over.
        fs::write(&file, "windows").unwrap();
        let (mut spoiled, wrong) = Places::read(file.clone());
        assert!(wrong.unwrap().contains("kept.json: not JSON"));
        spoiled.note(vec![(key("side"), place((1, 1), Fills::Nothing))]);
        spoiled.write().unwrap();
        assert!(Places::read(file).0.of(&key("side")).is_some());
        fs::remove_dir_all(&folder).unwrap();
    }
}

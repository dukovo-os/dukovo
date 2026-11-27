// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What `hardware screens` says: every graphics card the display layer
//! draws through and every screen of it, with the monitor plugged into
//! each, the mode it shows, the modes it offers, and which program has
//! the screens.
//!
//! All of it is the kernel's own account of the layer,
//! _/kernel/class/display/state_ (radix-sysfs(5)): a line for who is in
//! front, then a line for each card, screen, mode and pipe, each of
//! `key=value` words after the thing's number. The program in front is
//! named from its process's status. Nothing here asks a driver or a
//! service anything, so the command works with no console and no
//! desktop running, and changes nothing.
//!
//! No serial number of a monitor is read, and the kernel's account has
//! none, so that the answer can be shown and put in a report as it is.

use std::fmt::Write as _;
use std::fs;
use std::path::PathBuf;

use serde::Serialize;

use super::words::{Words, line_of};
use crate::proc;

/// Where the kernel says what the display layer has.
pub(crate) const STATE: &str = "/kernel/class/display/state";

/// Where each source is, so that tests can read their own.
pub(crate) struct Sources {
    state: PathBuf,
    processes: PathBuf,
}

impl Sources {
    /// Where the system keeps them.
    pub(crate) fn system() -> Sources {
        Sources { state: PathBuf::from(STATE), processes: PathBuf::from(proc::PROC) }
    }
}

/// What `hardware screens --json` says.
#[derive(Debug, PartialEq, Eq, Serialize)]
pub(crate) struct Screens {
    /// The version of this document: 1.
    format: u32,
    front: Front,
    cards: Vec<Card>,
}

/// Who has the screens.
#[derive(Debug, PartialEq, Eq, Serialize)]
struct Front {
    /// `log` for the kernel's log, `program` for a program that draws,
    /// and `last-picture` while the last picture of a program that ended
    /// stays.
    holder: &'static str,
    /// The program's process, once it has drawn.
    process: Option<u64>,
    /// That process's name.
    name: Option<String>,
}

/// A graphics card.
#[derive(Debug, Default, PartialEq, Eq, Serialize)]
struct Card {
    /// The layer's name for it: `firmware0`, `virtio0`.
    name: String,
    /// What drives it: `firmware` or `virtio`.
    kind: String,
    /// What it is, in words.
    model: String,
    /// Whether its screens come and go while the system runs.
    hot_plug: bool,
    /// Whether it sets modes: without, each screen has the mode it was
    /// found in.
    sets_modes: bool,
    /// Whether the host shows a program's own pages, so that nothing is
    /// copied.
    shared_pages: bool,
    /// How many times the thread that keeps its beat has woken.
    wakes: u64,
    screens: Vec<Screen>,
    /// The layer's number for it.
    #[serde(skip)]
    id: u32,
}

/// A screen: a socket a monitor plugs into.
#[derive(Debug, Default, PartialEq, Eq, Serialize)]
struct Screen {
    /// Its place among its card's screens, from 1.
    number: u32,
    /// Whether a monitor is plugged in.
    plugged: bool,
    /// Whether the card stopped showing on it, having not confirmed a
    /// change in time.
    failed: bool,
    /// The monitor's name, when it gives one.
    monitor: Option<String>,
    /// Its maker's three letters.
    maker: Option<String>,
    /// Its model number, when it gives one.
    product: Option<u32>,
    /// Its size in millimetres.
    width_mm: Option<u32>,
    height_mm: Option<u32>,
    /// The bits a colour it takes at most.
    depth: Option<u32>,
    /// Whether it takes high dynamic range, and a variable refresh rate.
    hdr: bool,
    variable_refresh: bool,
    /// The mode it shows, if it shows.
    mode: Option<Mode>,
    /// The modes it offers, the one it prefers first.
    modes: Vec<Mode>,
    /// The layer's numbers for it and for the pipe that feeds it.
    #[serde(skip)]
    id: u32,
    #[serde(skip)]
    pipe: u32,
    #[serde(skip)]
    card: u32,
}

/// One way a screen is driven.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
struct Mode {
    width: u32,
    height: u32,
    /// Frames in 1000 seconds.
    refresh_mhz: u32,
    /// Whether the monitor, or the host's window, asks for it.
    preferred: bool,
}

fn mode_of(words: &Words<'_>) -> Mode {
    Mode {
        width: words.number("width"),
        height: words.number("height"),
        refresh_mhz: words.number("refresh_mhz"),
        preferred: words.flag("preferred"),
    }
}

fn card_of(id: u32, words: &Words<'_>) -> Card {
    Card {
        id,
        name: words.named("name").unwrap_or_default(),
        kind: words.named("kind").unwrap_or_default(),
        model: words.named("model").unwrap_or_default(),
        hot_plug: words.flag("hot_plug"),
        sets_modes: words.flag("sets_modes"),
        shared_pages: words.flag("shared_pages"),
        wakes: words.text("wakes").and_then(|value| value.parse().ok()).unwrap_or_default(),
        screens: Vec::new(),
    }
}

fn screen_of(id: u32, words: &Words<'_>) -> Screen {
    Screen {
        id,
        card: words.number("card"),
        pipe: words.number("pipe"),
        number: words.number("index") + 1,
        plugged: words.flag("plugged"),
        failed: words.flag("failed"),
        monitor: words.named("name"),
        maker: words.named("maker"),
        product: words.said("product"),
        width_mm: words.said("width_mm"),
        height_mm: words.said("height_mm"),
        depth: words.said("depth"),
        hdr: words.flag("hdr"),
        variable_refresh: words.flag("variable_refresh"),
        mode: None,
        modes: Vec::new(),
    }
}

/// Who is in front, from the first line's words after `front`.
fn front_of(rest: &str, sources: &Sources) -> Front {
    let none = |holder| Front { holder, process: None, name: None };
    match rest.split_once(' ') {
        Some(("process", number)) => {
            let process = number.trim().parse().ok();
            let status = process
                .and_then(|n: u64| fs::read(sources.processes.join(format!("{n}/status"))).ok());
            let name = status.map(|text| proc::parse_status(&text).name).filter(|n| !n.is_empty());
            Front { holder: "program", process, name }
        }
        _ if rest == "holder" => none("program"),
        _ if rest == "last" => none("last-picture"),
        _ => none("log"),
    }
}

/// Reads what the kernel says of the display layer. A machine with no
/// such file, or with no card, has no cards here.
pub(crate) fn gather(sources: &Sources) -> Screens {
    let text = fs::read_to_string(&sources.state).unwrap_or_default();
    parse(&text, sources)
}

fn parse(text: &str, sources: &Sources) -> Screens {
    let mut front = Front { holder: "log", process: None, name: None };
    let mut cards: Vec<Card> = Vec::new();
    let mut screens: Vec<Screen> = Vec::new();
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("front ") {
            front = front_of(rest, sources);
        } else if let Some((id, words)) = line_of(line, "card", "model") {
            cards.push(card_of(id, &words));
        } else if let Some((id, words)) = line_of(line, "screen", "name") {
            screens.push(screen_of(id, &words));
        } else if let Some((id, words)) = line_of(line, "mode", "") {
            if let Some(screen) = screens.iter_mut().find(|screen| screen.id == id) {
                screen.modes.push(mode_of(&words));
            }
        } else if let Some((id, words)) = line_of(line, "pipe", "") {
            // A pipe that makes frames gives the screens it feeds their mode.
            let shown = words.flag("active").then(|| mode_of(&words));
            for screen in screens.iter_mut().filter(|screen| screen.pipe == id) {
                let preferred = |mode: &Mode| {
                    screen.modes.first().is_some_and(|first| {
                        first.preferred
                            && (first.width, first.height, first.refresh_mhz)
                                == (mode.width, mode.height, mode.refresh_mhz)
                    })
                };
                screen.mode = shown.map(|mode| Mode { preferred: preferred(&mode), ..mode });
            }
        }
    }
    for screen in screens {
        if let Some(card) = cards.iter_mut().find(|card| card.id == screen.card) {
            card.screens.push(screen);
        }
    }
    Screens { format: 1, front, cards }
}

/// How many screens of the kernel's account `state` have a monitor
/// plugged in, or `None` if it has no card: for `hardware show`.
pub(crate) fn plugged_in(state: &str) -> Option<u32> {
    let nowhere = Sources { state: PathBuf::new(), processes: PathBuf::new() };
    let found = parse(state, &nowhere);
    found.any().then(|| found.plugged())
}

/// A mode in words: `1920x1080 at 60 Hz`, with the rate's decimals
/// only where it has them.
fn mode_text(mode: &Mode) -> String {
    let (hz, rest) = (mode.refresh_mhz / 1000, mode.refresh_mhz % 1000);
    let rate = if rest == 0 { format!("{hz}") } else { format!("{hz}.{:02}", rest / 10) };
    format!("{}x{} at {rate} Hz", mode.width, mode.height)
}

impl Screens {
    /// One line of JSON.
    pub(crate) fn json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }

    /// How many screens have a monitor plugged in.
    fn plugged(&self) -> u32 {
        self.cards.iter().flat_map(|card| &card.screens).filter(|screen| screen.plugged).count()
            as u32
    }

    /// Whether the system draws through a graphics card at all.
    fn any(&self) -> bool {
        !self.cards.is_empty()
    }

    /// The text a terminal shows: who has the screens, then each card
    /// and under it each of its screens.
    pub(crate) fn text(&self) -> String {
        if self.cards.is_empty() {
            return "No graphics card: the system draws on no screen.\n".into();
        }
        let mut text = String::new();
        let _ = writeln!(text, "In front: {}", self.front.text());
        for card in &self.cards {
            let _ = writeln!(text, "\n{}: {}", card.name, card.text());
            for screen in &card.screens {
                screen.write(&mut text);
            }
        }
        text
    }
}

impl Front {
    fn text(&self) -> String {
        match (self.holder, self.process, &self.name) {
            ("program", Some(process), Some(name)) => format!("{name} (process {process})"),
            ("program", Some(process), None) => format!("process {process}"),
            ("program", ..) => "a program that has not drawn yet".into(),
            ("last-picture", ..) => "nothing: the last picture of a program that ended".into(),
            _ => "the kernel's log".into(),
        }
    }
}

impl Card {
    fn text(&self) -> String {
        let mut text = self.model.clone();
        if !self.sets_modes {
            text.push_str(", in the mode the firmware set");
        }
        if self.shared_pages {
            text.push_str(", shows programs' own pages");
        }
        text
    }
}

impl Screen {
    fn write(&self, text: &mut String) {
        if !self.plugged {
            let _ = writeln!(text, "  Screen {}: nothing plugged in", self.number);
            return;
        }
        let monitor = match (&self.monitor, &self.maker) {
            (Some(name), Some(maker)) => format!("{name} ({maker})"),
            (Some(name), None) => name.clone(),
            (None, Some(maker)) => format!("a monitor of {maker}"),
            (None, None) => "a monitor that does not say what it is".into(),
        };
        let size = match (self.width_mm, self.height_mm) {
            (Some(width), Some(height)) => format!(", {width} by {height} mm"),
            _ => String::new(),
        };
        let _ = writeln!(text, "  Screen {}: {monitor}{size}", self.number);
        let shows = match (&self.mode, self.failed) {
            (_, true) => "nothing: the card did not confirm a change, and stopped".into(),
            (Some(mode), false) => mode_text(mode),
            (None, false) => "nothing".into(),
        };
        let _ = writeln!(text, "    Shows:  {shows}");
        let modes: Vec<String> = self
            .modes
            .iter()
            .map(|mode| {
                let preferred = if mode.preferred { " (preferred)" } else { "" };
                format!("{}{preferred}", mode_text(mode))
            })
            .collect();
        let _ = writeln!(text, "    Modes:  {}", modes.join(", "));
        let depth = match self.depth {
            Some(bits) => format!("{bits} bits a colour"),
            None => "depth not said".into(),
        };
        let yes_no = |yes: bool| if yes { "yes" } else { "no" };
        let _ = writeln!(
            text,
            "    Colour: {depth}; HDR: {}; variable refresh: {}",
            yes_no(self.hdr),
            yes_no(self.variable_refresh)
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The kernel's account of a virtual machine with the firmware's
    /// card and a virtio card of two screens, one with a monitor.
    const STATE: &str = "\
front process 41
card 1 name=firmware0 kind=firmware hot_plug=0 mirrors=0 sets_modes=0 shared_pages=0 wakes=3 model=Firmware framebuffer
card 5 name=virtio0 kind=virtio hot_plug=1 mirrors=0 sets_modes=1 shared_pages=0 wakes=977 model=Virtio graphics card
screen 3 card=1 index=0 plugged=1 failed=0 hdr=0 variable_refresh=0 pipe=2 depth=0 width_mm=0 height_mm=0 refresh_min_mhz=0 refresh_max_mhz=0 luminance_max=0 product=0 maker= name=
mode 3 width=1280 height=800 refresh_mhz=60000 preferred=1
screen 8 card=5 index=0 plugged=1 failed=0 hdr=0 variable_refresh=0 pipe=6 depth=8 width_mm=260 height_mm=160 refresh_min_mhz=0 refresh_max_mhz=0 luminance_max=0 product=4660 maker=RHT name=QEMU Monitor 2
mode 8 width=1024 height=768 refresh_mhz=74990 preferred=1
mode 8 width=800 height=600 refresh_mhz=60000 preferred=0
screen 9 card=5 index=1 plugged=0 failed=0 hdr=0 variable_refresh=0 pipe=0 depth=0 width_mm=0 height_mm=0 refresh_min_mhz=0 refresh_max_mhz=0 luminance_max=0 product=0 maker= name=
pipe 2 card=1 index=0 active=1 width=1280 height=800 refresh_mhz=60000 beat_ns=16666666 beat_count=40 mode_sets=1
pipe 6 card=5 index=0 active=1 width=800 height=600 refresh_mhz=60000 beat_ns=16666666 beat_count=12 mode_sets=2
pipe 7 card=5 index=1 active=0 width=0 height=0 refresh_mhz=0 beat_ns=0 beat_count=0 mode_sets=0
";

    /// Sources in a directory of the test's own, with `state` as the
    /// kernel's account and process 41 named consoled.
    fn sources(name: &str, state: &str) -> Sources {
        let dir = std::env::temp_dir().join(format!("screens-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("processes/41")).unwrap();
        fs::write(dir.join("processes/41/status"), "number: 41\nname: consoled\n").unwrap();
        fs::write(dir.join("state"), state).unwrap();
        Sources { state: dir.join("state"), processes: dir.join("processes") }
    }

    #[test]
    fn each_card_and_screen_is_read_from_the_kernels_account() {
        let found = gather(&sources("read", STATE));
        assert_eq!(found.front.text(), "consoled (process 41)");
        assert_eq!((found.cards.len(), found.plugged(), found.any()), (2, 2, true));
        let virtio = &found.cards[1];
        assert_eq!((virtio.name.as_str(), virtio.kind.as_str()), ("virtio0", "virtio"));
        assert_eq!(virtio.model, "Virtio graphics card");
        assert!(virtio.hot_plug && virtio.sets_modes && !virtio.shared_pages);
        assert_eq!((virtio.wakes, virtio.screens.len()), (977, 2));
        let screen = &virtio.screens[0];
        // A name with spaces is whole; what the monitor does not say is
        // not a 0.
        assert_eq!(screen.monitor.as_deref(), Some("QEMU Monitor 2"));
        assert_eq!((screen.maker.as_deref(), screen.product), (Some("RHT"), Some(4660)));
        assert_eq!(
            (screen.width_mm, screen.height_mm, screen.depth),
            (Some(260), Some(160), Some(8))
        );
        assert_eq!(screen.modes.len(), 2);
        // It shows a mode that is not the one it prefers.
        let shown = screen.mode.unwrap();
        assert_eq!((shown.width, shown.height, shown.preferred), (800, 600, false));
        let firmware = &found.cards[0].screens[0];
        assert_eq!((firmware.monitor.as_deref(), firmware.depth), (None, None));
        assert!(firmware.mode.unwrap().preferred);
        assert!(!virtio.screens[1].plugged && virtio.screens[1].mode.is_none());
    }

    #[test]
    fn the_text_names_who_is_in_front_and_each_screen() {
        let text = gather(&sources("text", STATE)).text();
        let expected = "\
In front: consoled (process 41)

firmware0: Firmware framebuffer, in the mode the firmware set
  Screen 1: a monitor that does not say what it is
    Shows:  1280x800 at 60 Hz
    Modes:  1280x800 at 60 Hz (preferred)
    Colour: depth not said; HDR: no; variable refresh: no

virtio0: Virtio graphics card
  Screen 1: QEMU Monitor 2 (RHT), 260 by 160 mm
    Shows:  800x600 at 60 Hz
    Modes:  1024x768 at 74.99 Hz (preferred), 800x600 at 60 Hz
    Colour: 8 bits a colour; HDR: no; variable refresh: no
  Screen 2: nothing plugged in
";
        assert_eq!(text, expected);
    }

    #[test]
    fn the_json_is_one_document_with_what_the_text_says() {
        let json = gather(&sources("json", STATE)).json().unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["format"], 1);
        assert_eq!(value["front"]["holder"], "program");
        assert_eq!(value["front"]["name"], "consoled");
        let screen = &value["cards"][1]["screens"][0];
        assert_eq!(screen["monitor"], "QEMU Monitor 2");
        assert_eq!(screen["mode"]["width"], 800);
        assert_eq!(screen["modes"][0]["preferred"], true);
        assert_eq!(value["cards"][1]["screens"][1]["mode"], serde_json::Value::Null);
        // The layer's own numbers are not part of the document, and no
        // serial number is.
        assert!(screen.get("id").is_none() && screen.get("pipe").is_none());
        assert!(value["cards"][0].get("id").is_none());
        assert!(!json.contains("serial"), "{json}");
    }

    #[test]
    fn who_is_in_front_is_said_in_each_case() {
        let front = |line: &str| gather(&sources("front", &format!("{line}\n{STATE}"))).front;
        // The last `front` line read is the one that counts: STATE's.
        assert_eq!(front("front log").holder, "program");
        let alone = |line: &str| gather(&sources("alone", &format!("{line}\n"))).front.text();
        assert_eq!(alone("front log"), "the kernel's log");
        assert_eq!(alone("front holder"), "a program that has not drawn yet");
        assert_eq!(alone("front last"), "nothing: the last picture of a program that ended");
        // A process that ended since has its number only.
        assert_eq!(alone("front process 99"), "process 99");
    }

    #[test]
    fn a_machine_with_no_card_says_so() {
        let none =
            Sources { state: PathBuf::from("/nonexistent/state"), processes: PathBuf::new() };
        let found = gather(&none);
        assert!(!found.any());
        assert_eq!(found.text(), "No graphics card: the system draws on no screen.\n");
        assert_eq!(
            found.json().unwrap(),
            r#"{"format":1,"front":{"holder":"log","process":null,"name":null},"cards":[]}"#
        );
    }
}

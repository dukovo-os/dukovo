// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Telling a program when a frame of its showed (presentation-time,
//! ADR 0728).
//!
//! A program may ask, with a commit, to be told when what it committed
//! first shows on a screen. windowd takes what was asked of every
//! surface in a frame when it hands the frame to the layer, and answers
//! when the layer says the frame shows: with the time of the beat it
//! showed at, how long the screen's beats are apart, and the beat's
//! number. A frame that never showed is answered too: discarded.

use std::time::Duration;

use display::abi::Mode;
use smithay::output::Output;
use smithay::reexports::wayland_protocols::wp::presentation_time::server::wp_presentation_feedback::Kind;
use smithay::wayland::presentation::{PresentationFeedbackCallback, Refresh};

/// What the programs asked to be told of one frame.
#[derive(Debug)]
struct Asked {
    /// The cookie of the change that shows the frame.
    cookie: u64,
    callbacks: Vec<PresentationFeedbackCallback>,
    /// How the frame shows.
    kind: Kind,
    /// When the frame was handed over, if it was to show at once and
    /// not at a beat.
    at_once_ns: Option<u64>,
}

/// What the programs asked to be told of the frames of one screen that
/// have not shown yet.
#[derive(Debug, Default)]
pub struct Waiting {
    asked: Vec<Asked>,
}

impl Waiting {
    /// `callbacks` are answered when the change with `cookie` shows, or
    /// at the screen's next beat if that comes first. `kind` is how the
    /// frame shows, and `at_once_ns` when it was handed over if it does
    /// not wait for a beat.
    pub fn handed(
        &mut self,
        cookie: u64,
        callbacks: Vec<PresentationFeedbackCallback>,
        kind: Kind,
        at_once_ns: Option<u64>,
    ) {
        if !callbacks.is_empty() {
            self.asked.push(Asked { cookie, callbacks, kind, at_once_ns });
        }
    }

    /// A frame shows on `screen`, whose mode is `mode`: the change with
    /// cookie `upto` and those before it, or for `None` every frame
    /// that was handed over. `time_ns` is the time of the beat it shows
    /// at, and `beat` the beat's number.
    pub fn shown(
        &mut self,
        screen: &Output,
        mode: &Mode,
        upto: Option<u64>,
        time_ns: u64,
        beat: u64,
    ) {
        let refresh = match mode.refresh_mhz {
            0 => Refresh::Unknown,
            mhz => Refresh::fixed(Duration::from_nanos(1_000_000_000_000 / u64::from(mhz))),
        };
        let shows = |asked: &Asked| upto.is_none_or(|upto| asked.cookie <= upto);
        for asked in self.asked.extract_if(.., |asked| shows(asked)) {
            // A frame that did not wait showed when it was handed over,
            // after the beat the layer names.
            let time = asked.at_once_ns.map_or(time_ns, |at_once| at_once.max(time_ns));
            for callback in asked.callbacks {
                callback.presented(screen, Duration::from_nanos(time), refresh, beat, asked.kind);
            }
        }
    }

    /// No frame that was handed over shows: the screen went, or the
    /// desktop is not in front.
    pub fn discard(&mut self) {
        for asked in self.asked.drain(..) {
            asked.callbacks.into_iter().for_each(PresentationFeedbackCallback::discarded);
        }
    }
}

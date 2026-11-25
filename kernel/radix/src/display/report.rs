// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `/kernel/class/display/state`: the layer in words, for
//! `hardware screens` and whatever else an administrator reads the
//! screens with. The text is `radix_display`'s
//! ([`radix_display::layer::Layer::describe`]), where its lines are
//! described and tested; here it is written with the layer's lock held,
//! so that it is of one moment.

use core::fmt;
use core::sync::atomic::Ordering;

use radix_vfs::Text;

use super::{CardRef, LAYER};
use crate::sched;

/// Writes the layer in words to `text`.
///
/// # Errors
///
/// `text`'s, when it is full.
pub fn report(text: &mut Text) -> fmt::Result {
    let mut layer = LAYER.lock();
    layer.set_time(sched::now_ns());
    layer.describe(&|card: &CardRef| card.wakes.load(Ordering::Relaxed), text)
}

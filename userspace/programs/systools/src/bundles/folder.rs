// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Folders open as descriptors, whose names are looked up one part at a
//! time and never through a link: bundled(8) works in folders of the
//! people it installs for, and the runner checks a sealed bundle's folder
//! (ADRs 0223 and 0225), so nothing there may lead elsewhere. They are
//! sysapi's.

pub use sysapi::dir::{DEPTH_MAX, Dir, c_name, same, stat as fd_stat};

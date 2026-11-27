// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Where programs find the desktop: its socket.
//!
//! A program connects to the socket WAYLAND_DISPLAY names in the folder
//! XDG_RUNTIME_DIR names. windowd listens at `wayland-0` there, or at
//! the first of `wayland-1` and so on that no other desktop of the
//! person listens at. The folder is the person's alone: the one
//! XDG_RUNTIME_DIR names if the environment has one, and otherwise
//! `/tmp/runtime-N` for the person of number N, which windowd makes.
//! Whoever can open the folder can draw windows and read the keys they
//! get, so a folder that others can open is not used.

use std::fs::{self, DirBuilder};
use std::io;
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};

/// How many names are tried.
const NAMES: u32 = 32;

/// The socket, and what the programs' environment says to find it.
#[derive(Debug)]
pub struct Listening {
    /// The socket.
    pub listener: UnixListener,
    /// XDG_RUNTIME_DIR.
    pub folder: PathBuf,
    /// WAYLAND_DISPLAY.
    pub name: String,
}

/// The person's folder for the sockets of what they run: `named`, which
/// is what the environment's XDG_RUNTIME_DIR says, or one made for the
/// person of number `uid`.
fn folder(named: Option<PathBuf>, uid: u32) -> io::Result<PathBuf> {
    let folder = match named.filter(|named| named.is_absolute()) {
        Some(named) => named,
        None => {
            let own = PathBuf::from(format!("/tmp/runtime-{uid}"));
            match DirBuilder::new().mode(0o700).create(&own) {
                Err(error) if error.kind() != io::ErrorKind::AlreadyExists => return Err(error),
                _ => own,
            }
        }
    };
    // Not through a link another person may have put in its place.
    let found = fs::symlink_metadata(&folder)?;
    if !found.is_dir() || found.uid() != uid || found.mode() & 0o077 != 0 {
        let why = format!("{} is not a folder of the person's alone", folder.display());
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, why));
    }
    Ok(folder)
}

/// Listens at the first free name in `folder`.
fn listen_in(folder: &Path) -> io::Result<(UnixListener, String)> {
    for number in 0..NAMES {
        let name = format!("wayland-{number}");
        let path = folder.join(&name);
        match UnixListener::bind(&path) {
            Ok(listener) => return Ok((listener, name)),
            Err(error) if error.kind() == io::ErrorKind::AddrInUse => {}
            Err(error) => return Err(error),
        }
        if UnixStream::connect(&path).is_ok() {
            continue;
        }
        // Nothing listens there: a desktop that ended without tidying
        // up left it.
        fs::remove_file(&path)?;
        return Ok((UnixListener::bind(&path)?, name));
    }
    Err(io::Error::new(io::ErrorKind::AddrInUse, "every name for a desktop's socket is taken"))
}

impl Listening {
    /// Listens for the programs of the person of number `uid`, whose
    /// environment names the folder `named`, if any.
    ///
    /// # Errors
    ///
    /// When the folder is not the person's alone or cannot be made, and
    /// what bind(2) fails with.
    pub fn start(named: Option<PathBuf>, uid: u32) -> io::Result<Listening> {
        let folder = folder(named, uid)?;
        let (listener, name) = listen_in(&folder)?;
        listener.set_nonblocking(true)?;
        Ok(Listening { listener, folder, name })
    }
}

impl Drop for Listening {
    fn drop(&mut self) {
        // A socket that stays is taken away by the next desktop.
        let _ = fs::remove_file(self.folder.join(&self.name));
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    /// A folder of the test's own, which is gone with it.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Scratch {
            let path = std::env::temp_dir().join(format!("windowd-{name}-{}", std::process::id()));
            DirBuilder::new().mode(0o700).create(&path).unwrap();
            Scratch(path)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn uid() -> u32 {
        sysapi::process::euid()
    }

    #[test]
    fn a_second_desktop_listens_at_the_next_name_and_a_stale_socket_is_taken_over() {
        let scratch = Scratch::new("names");
        let first = Listening::start(Some(scratch.0.clone()), uid()).unwrap();
        assert_eq!((first.name.as_str(), &first.folder), ("wayland-0", &scratch.0));
        let second = Listening::start(Some(scratch.0.clone()), uid()).unwrap();
        assert_eq!(second.name, "wayland-1");
        // A desktop that ended tidies up, and one that was killed left
        // a socket nothing listens at.
        drop(first);
        assert!(!scratch.0.join("wayland-0").exists());
        drop(UnixListener::bind(scratch.0.join("wayland-0")).unwrap());
        assert!(scratch.0.join("wayland-0").exists());
        let third = Listening::start(Some(scratch.0.clone()), uid()).unwrap();
        assert_eq!(third.name, "wayland-0");
        assert!(UnixStream::connect(scratch.0.join("wayland-0")).is_ok());
    }

    #[test]
    fn a_folder_others_can_open_or_that_is_another_s_is_not_used() {
        let scratch = Scratch::new("open");
        fs::set_permissions(&scratch.0, fs::Permissions::from_mode(0o750)).unwrap();
        let refused = Listening::start(Some(scratch.0.clone()), uid()).unwrap_err();
        assert_eq!(refused.kind(), io::ErrorKind::PermissionDenied);
        fs::set_permissions(&scratch.0, fs::Permissions::from_mode(0o700)).unwrap();
        let refused = Listening::start(Some(scratch.0.clone()), uid() + 1).unwrap_err();
        assert_eq!(refused.kind(), io::ErrorKind::PermissionDenied);
        // A link to a folder is not the folder.
        let link = scratch.0.join("link");
        std::os::unix::fs::symlink(&scratch.0, &link).unwrap();
        let refused = Listening::start(Some(link), uid()).unwrap_err();
        assert_eq!(refused.kind(), io::ErrorKind::PermissionDenied);
    }
}

// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What the test does as root, in one command each, so that it types
//! `elevate monitoring-test setup` and not a script: it makes the
//! receivers a service and starts it, makes the system trust the test
//! authority, and writes alerts.toml.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use crate::config::{self, CA_FILE, FOLDER, HTTP_PORT, HTTPS_PORT, SMTP_PORT};
use crate::records::READY;
use crate::tls;

/// The program, as the service runs it.
const PROGRAM: &str = "/system/tests/posix/monitoring-test";

/// The service's file, which Spark reads from the settings.
const SERVICE_FILE: &str = "/system/settings/spark/services/monitoring-test.toml";

/// alerts.toml, which alertd reads.
const SETTINGS_FILE: &str = "/system/settings/alerts.toml";

/// The root certificates the system comes with, and the file of them that
/// programs read, which is a link to them until an administrator makes a
/// file of their own (dukovo-certificates(7)).
const SYSTEM_ROOTS: &str = "/system/resources/certificates/ca-certificates.crt";
const ROOTS: &str = "/system/settings/ssl/certs/ca-certificates.crt";

/// The `service` command.
const SERVICE: &str = "/system/commands/service";

/// How long the receivers have to listen once started.
const LISTEN_SECONDS: u64 = 20;

/// How long alertd has to read alerts.toml after it is told to: it
/// looks for the signal twice a second.
const RELOAD_PAUSE: Duration = Duration::from_secs(2);

/// Which alerts.toml to write.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Settings {
    /// The receivers, root's alone to read.
    Good,
    /// With a wrong password and an address the receiver refuses.
    Broken,
    /// As `Good`, but anyone may read it, which alertd refuses.
    Loose,
}

impl Settings {
    /// The variant `name` stands for.
    pub fn parse(name: &str) -> Option<Settings> {
        match name {
            "good" => Some(Settings::Good),
            "broken" => Some(Settings::Broken),
            "loose" => Some(Settings::Loose),
            _ => None,
        }
    }

    fn text(self) -> String {
        config::alerts_toml(self == Settings::Broken)
    }

    fn mode(self) -> u32 {
        if self == Settings::Loose { 0o644 } else { 0o600 }
    }
}

/// The receivers' service file: what Spark needs to start them, with only
/// the ports they listen on and the folder they write in.
pub fn service_toml() -> String {
    format!(
        "service_version = 1\n\
         description = \"Mail and webhook receivers for the monitoring test\"\n\
         program = \"{PROGRAM}\"\n\
         \n\
         [permissions]\n\
         network.listen = [{SMTP_PORT}, {HTTPS_PORT}, {HTTP_PORT}]\n\
         files.read-write = [\"{FOLDER}\"]\n"
    )
}

/// Starts the receivers as a service, makes the system trust their
/// certificate's authority, and has alertd send to them.
///
/// # Errors
///
/// What failed, in words for the test's log.
pub fn setup() -> Result<(), String> {
    // Readable whatever umask the command runs with: the test reads what
    // the receivers heard as a user.
    let folder = Path::new(FOLDER);
    fs::create_dir_all(folder)
        .and_then(|()| fs::set_permissions(folder, fs::Permissions::from_mode(0o755)))
        .map_err(|err| format!("{FOLDER}: {err}"))?;
    write(Path::new(SERVICE_FILE), service_toml().as_bytes(), 0o644)?;
    service(&["monitoring-test", "start"])?;
    listening(Path::new(FOLDER), LISTEN_SECONDS)?;
    write(Path::new(CA_FILE), tls::authority(), 0o644)?;
    trust(Path::new(SYSTEM_ROOTS), tls::authority(), Path::new(ROOTS))?;
    settings(Settings::Good)?;
    println!("monitoring-test: the receivers listen, and alerts.toml names them");
    Ok(())
}

/// Writes alerts.toml as `which` says, and has alertd read it.
///
/// # Errors
///
/// Why the file could not be written, or alertd not told.
pub fn settings(which: Settings) -> Result<(), String> {
    write(Path::new(SETTINGS_FILE), which.text().as_bytes(), which.mode())?;
    service(&["alertd", "reload"])?;
    thread::sleep(RELOAD_PAUSE);
    Ok(())
}

/// Runs the `service` command with `args`.
fn service(args: &[&str]) -> Result<(), String> {
    let status = Command::new(SERVICE)
        .args(args)
        .status()
        .map_err(|err| format!("cannot run {SERVICE}: {err}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("service {} failed: {status}", args.join(" ")))
    }
}

/// Waits until the receivers say, in `folder`, that they listen.
fn listening(folder: &Path, seconds: u64) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(seconds);
    while !folder.join(READY).exists() {
        if Instant::now() >= deadline {
            return Err(format!("the receivers did not listen within {seconds} seconds"));
        }
        thread::sleep(Duration::from_millis(200));
    }
    Ok(())
}

/// Makes `target` a copy of `system` with the certificate `extra` added,
/// in place of what it was, as dukovo-certificates(7) says to trust an
/// authority of one's own.
fn trust(system: &Path, extra: &[u8], target: &Path) -> Result<(), String> {
    let mut roots = fs::read(system).map_err(|err| format!("{}: {err}", system.display()))?;
    if !roots.ends_with(b"\n") {
        roots.push(b'\n');
    }
    roots.extend_from_slice(extra);
    let new = target.with_extension("crt.new");
    write(&new, &roots, 0o644)?;
    fs::rename(&new, target).map_err(|err| format!("{}: {err}", target.display()))
}

/// Writes `bytes` to a new file `path` with `mode`, in place of the file
/// or link that was there: the mode is given as the file is made, so that
/// a file meant for root alone is never readable, and set again after, so
/// that the umask the command runs with does not change it.
fn write(path: &Path, bytes: &[u8], mode: u32) -> Result<(), String> {
    let wrong = |err: std::io::Error| format!("{}: {err}", path.display());
    match fs::remove_file(path) {
        Err(err) if err.kind() != std::io::ErrorKind::NotFound => return Err(wrong(err)),
        _ => {}
    }
    let mut file =
        OpenOptions::new().write(true).create_new(true).mode(mode).open(path).map_err(wrong)?;
    file.set_permissions(fs::Permissions::from_mode(mode)).map_err(wrong)?;
    file.write_all(bytes).map_err(wrong)
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::os::unix::fs::{MetadataExt, symlink};
    use std::path::PathBuf;

    fn folder(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("monitoring-setup-{name}-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_variants_have_their_names_texts_and_modes() {
        assert_eq!(Settings::parse("good"), Some(Settings::Good));
        assert_eq!(Settings::parse("broken"), Some(Settings::Broken));
        assert_eq!(Settings::parse("loose"), Some(Settings::Loose));
        assert_eq!(Settings::parse("fine"), None);
        assert!(Settings::Broken.text().contains("coffee"));
        assert!(!Settings::Good.text().contains("coffee"));
        assert_eq!(Settings::Loose.text(), Settings::Good.text());
        assert_eq!((Settings::Good.mode(), Settings::Loose.mode()), (0o600, 0o644));
    }

    #[test]
    fn the_service_has_its_program_its_ports_and_its_folder() {
        let text = service_toml();
        assert!(text.contains("program = \"/system/tests/posix/monitoring-test\""));
        assert!(text.contains("network.listen = [587, 8443, 8080]"));
        assert!(text.contains("files.read-write = [\"/shared/monitoring\"]"));
        assert!(text.starts_with("service_version = 1\n"));
    }

    #[test]
    fn a_file_is_made_with_its_mode_in_place_of_the_one_before() {
        let dir = folder("write");
        let path = dir.join("alerts.toml");
        write(&path, b"first", 0o644).unwrap();
        write(&path, b"second", 0o600).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"second");
        assert_eq!(fs::metadata(&path).unwrap().mode() & 0o777, 0o600);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn trusting_an_authority_replaces_the_link_with_a_copy_that_has_it_added() {
        let dir = folder("trust");
        let system = dir.join("system.crt");
        fs::write(&system, "# root one\nBEGIN ONE").unwrap();
        let target = dir.join("ca-certificates.crt");
        symlink(&system, &target).unwrap();
        trust(&system, b"BEGIN TEST\n", &target).unwrap();
        assert!(!fs::symlink_metadata(&target).unwrap().file_type().is_symlink());
        assert_eq!(fs::read_to_string(&target).unwrap(), "# root one\nBEGIN ONE\nBEGIN TEST\n");
        // The system's own file is as it was.
        assert_eq!(fs::read_to_string(&system).unwrap(), "# root one\nBEGIN ONE");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn listening_waits_for_the_ready_file_and_gives_up() {
        let dir = folder("ready");
        assert!(listening(&dir, 0).unwrap_err().contains("did not listen within 0 seconds"));
        fs::write(dir.join(READY), "ready\n").unwrap();
        assert!(listening(&dir, 0).is_ok());
        fs::remove_dir_all(dir).unwrap();
    }
}

// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What the checks of dukovo-check(8) read: the system's files, found
//! under a root so that the unit tests can plant them in a folder of their
//! own, and the few things no file holds, which Spark, the firewall and
//! alertd answer. They are asked once, before the first check, and each
//! answer that failed is kept as the reason its checks are skipped.

use std::fs;
use std::io;
use std::path::PathBuf;

use spark_api::show::Service;

use crate::alerts::command;
use crate::alerts::protocol::Active;
use crate::firewall::{self, Reached};
use crate::proc;
use crate::service;
use crate::util;

/// How long a computer must have been up before a check that asks whether
/// something happened lately is made of it. Services start themselves:
/// timed asks for the time a minute after the start, and updated looks
/// for updates ten minutes after, so a computer that started a short while
/// ago has had no time to, and a fresh install in a machine with no
/// network would otherwise warn at once.
pub(crate) const SETTLING: u64 = 3600;

/// The system as the checks see it.
pub(crate) struct Context {
    /// The folder the system's files are found under: `/`.
    pub(crate) root: PathBuf,
    /// The time now, in seconds since 1970.
    pub(crate) now: u64,
    /// How long the computer has been up, in seconds.
    pub(crate) uptime: u64,
    /// The release this system runs.
    pub(crate) release: String,
    /// The services Spark knows, or why it could not say.
    pub(crate) services: Result<Vec<Service>, String>,
    /// The ports other machines reach, or why the firewall could not say.
    pub(crate) reached: Result<Vec<Reached>, String>,
    /// The alerts that are active, or why alertd could not say.
    pub(crate) alerts: Result<Vec<Active>, String>,
}

impl Context {
    /// The running system, asked now.
    pub(crate) fn gather() -> Context {
        Context {
            root: PathBuf::from("/"),
            now: util::now_ns() / 1_000_000_000,
            uptime: proc::uptime_ns() / 1_000_000_000,
            release: env!("CARGO_PKG_VERSION").to_string(),
            services: service::shown(None).map(|show| show.services).map_err(|fail| fail.message),
            reached: firewall::reached().map_err(|fail| fail.message),
            alerts: command::active().map_err(|fail| fail.message),
        }
    }

    /// The text of the file at the absolute path `path` of the system, or
    /// `None` when there is no such file.
    ///
    /// # Errors
    ///
    /// Why it cannot be read, as a person says it.
    pub(crate) fn read(&self, path: &str) -> Result<Option<String>, String> {
        let file = self.root.join(path.trim_start_matches('/'));
        match fs::read_to_string(&file) {
            Ok(text) => Ok(Some(text)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(format!("cannot read {path}: {}", util::strip_os_error(&error))),
        }
    }

    /// `when`, in seconds since 1970, as the report says it: the time of
    /// day for a time today, else the date and the time.
    pub(crate) fn said(&self, when: u64) -> String {
        let local = |seconds: u64, format: &str| {
            util::local_time(i64::try_from(seconds).unwrap_or(0), format)
        };
        if local(when, "%F") == local(self.now, "%F") {
            format!("today at {}", local(when, "%R"))
        } else {
            format!("{} at {}", local(when, "%F"), local(when, "%R"))
        }
    }

    /// The services Spark knows.
    ///
    /// # Errors
    ///
    /// Why Spark could not say.
    pub(crate) fn services(&self) -> Result<&[Service], String> {
        self.services.as_deref().map_err(|why| format!("cannot ask Spark: {why}"))
    }

    /// The service called `name`: `None` when Spark has no such service.
    ///
    /// # Errors
    ///
    /// Why Spark could not say.
    pub(crate) fn service(&self, name: &str) -> Result<Option<&Service>, String> {
        Ok(self.services()?.iter().find(|service| service.name == name))
    }
}

#[cfg(test)]
pub(crate) mod planted {
    use std::path::Path;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    /// A system of a test's own: a folder, removed when the test is done,
    /// that a [`Context`] reads its files from.
    pub(crate) struct System {
        pub(crate) context: Context,
    }

    /// The time the planted systems say it is: 2026-11-14 12:00:30 UTC, so
    /// that a minute either side is the same day in every time zone.
    pub(crate) const NOW: u64 = 1_794_657_630;

    impl System {
        /// A system that has been up for two days, whose services are
        /// `services`, with no file, nothing reached and no alert.
        pub(crate) fn new(services: Vec<Service>) -> System {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let root = std::env::temp_dir().join(format!(
                "dukovo-check-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&root).unwrap();
            System {
                context: Context {
                    root,
                    now: NOW,
                    uptime: 2 * 86_400,
                    release: "0.16.0".into(),
                    services: Ok(services),
                    reached: Ok(Vec::new()),
                    alerts: Ok(Vec::new()),
                },
            }
        }

        /// Writes `text` as the file at the absolute path `path`.
        pub(crate) fn plant(&self, path: &str, text: &str) {
            let file = self.context.root.join(path.trim_start_matches('/'));
            fs::create_dir_all(file.parent().unwrap()).unwrap();
            fs::write(file, text).unwrap();
        }

        pub(crate) fn root(&self) -> &Path {
            &self.context.root
        }
    }

    impl Drop for System {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.context.root); // a test's leftovers
        }
    }

    /// A service in `state`.
    pub(crate) fn service(name: &str, state: spark_api::show::State) -> Service {
        Service { name: name.into(), state, ..Service::default() }
    }
}

#[cfg(test)]
mod tests {
    use super::planted::{System, service};
    use super::*;
    use spark_api::show::State;

    #[test]
    fn a_file_is_read_under_the_root_and_a_missing_one_is_none() {
        let system = System::new(Vec::new());
        system.plant("/system/settings/firewall", "allow tcp 22\n");
        let context = &system.context;
        assert_eq!(context.read("/system/settings/firewall"), Ok(Some("allow tcp 22\n".into())));
        assert_eq!(context.read("system/settings/firewall"), Ok(Some("allow tcp 22\n".into())));
        assert_eq!(context.read("/system/settings/blocking.toml"), Ok(None));
    }

    #[test]
    fn a_file_that_cannot_be_read_says_which_and_why() {
        let system = System::new(Vec::new());
        // A folder where a file is expected cannot be read as one.
        std::fs::create_dir_all(system.root().join("system/settings/passwd")).unwrap();
        let why = system.context.read("/system/settings/passwd").unwrap_err();
        assert!(why.starts_with("cannot read /system/settings/passwd: "), "{why}");
    }

    #[test]
    fn a_time_is_said_by_its_hour_today_and_by_its_date_before() {
        let system = System::new(Vec::new());
        let local = |seconds: u64, format: &str| util::local_time(seconds as i64, format);
        let hour = local(super::planted::NOW - 60, "%R");
        assert_eq!(system.context.said(super::planted::NOW - 60), format!("today at {hour}"));
        let before = super::planted::NOW - 3 * 86_400;
        let said = format!("{} at {}", local(before, "%F"), local(before, "%R"));
        assert_eq!(system.context.said(before), said);
    }

    #[test]
    fn a_service_is_found_by_name_and_spark_that_cannot_say_is_the_reason() {
        let mut system = System::new(vec![service("sshd", State::Running)]);
        let found = system.context.service("sshd").unwrap().unwrap();
        assert_eq!(found.state, State::Running);
        assert!(system.context.service("web").unwrap().is_none());
        system.context.services = Err("spark: not running".into());
        let why = system.context.service("sshd").unwrap_err();
        assert_eq!(why, "cannot ask Spark: spark: not running");
    }
}

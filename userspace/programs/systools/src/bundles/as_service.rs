// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A program of a bundle installed for everyone, run once as one of its
//! services runs (ADR 0569): website(8) has nginx check the settings it
//! is about to use, `nginx -t`, as the account nginx and in nginx's own
//! view, so that the check reads what nginx will read and nothing more.
//!
//! A process here cannot take another account's name: only Spark, which
//! holds the names capability, starts one as an account. So the program
//! is checked here as `bundle run` checks it, and Spark runs it, through
//! its control socket's `run` request (`spark_api::run`), as it runs the
//! service's health checks: in the service's domain and view, as its
//! account, with its environment, `/devices/null` as its input. Spark
//! kills it after 30 seconds and answers with its code and the first 16
//! KiB of what it wrote.
//!
//! What is checked first, as `bundle run` checks a bundle (`run.rs`):
//! the caller is root; bundled says the bundle is installed for everyone;
//! its folder is still sealed, its list the one recorded and signed by the
//! key recorded, its manifest the one the list names; the program is one
//! of the manifest's, a sealed file the list names that may run; the
//! service is one of the manifest's, and the service file of that name is
//! the one bundled wrote for this bundle, with the account the manifest
//! gives it. Spark refuses a service with no account of its own: a
//! bundle's program never runs as root here. The trusted prompt is not
//! asked: the program runs as the account Spark runs the bundle's service
//! as, with what the service holds, not with the caller's access, which is
//! what the prompt guards (ADR 0224).

use std::io::{Read, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::time::Duration;

use manifest::bundle::Manifest;
use spark_api::run::{self as spark_run, Run};

pub use spark_api::run::Ran;

use super::protocol::{Answer, Find, Found, Request};
use super::registry::{self, Place};
use super::run::{self, ARCH};
use super::services::{self, Record};
use super::{answer, ask};
use crate::deadline::Deadline;
use crate::util;

/// How long to wait for Spark's answer: the program's time, and room for
/// Spark to start it.
const ANSWER_WAIT: Duration = Duration::from_secs(spark_run::SECONDS + 15);

/// How many times a run is asked for while another runs, a second apart.
const TRIES: u32 = 30;

/// Runs program `program` of bundle `id`, installed for everyone, as its
/// service `service` runs, with `args`: how it ended, and what it wrote.
///
/// # Errors
///
/// When the caller is not root, the bundle, its program or its service is
/// not as it was installed, or Spark refused or could not be reached.
pub fn run(id: &str, program: &str, service: &str, args: &[&str]) -> Result<Ran, String> {
    root_only(sysapi::process::euid())?;
    let found = installed_for_everyone(id)?;
    let sealed = run::open_sealed(&found)?;
    let record = crate::bundled::read_record(service);
    let path = program_of(&sealed.manifest, &found.id, program, service, record.as_ref())?;
    // Sealed, named in the list, and one that may run; opened as `bundle
    // run` opens it, never through a link.
    drop(run::open_program(&sealed, &path)?);
    let asked = Run {
        service: service.into(),
        program: format!("{}/{path}", sealed.path),
        arguments: args.iter().map(ToString::to_string).collect(),
    };
    let line = asked.line()?;
    for _ in 1..TRIES {
        match spark_run::read(&ask_spark(&line)?) {
            Err(why) if why == spark_run::BUSY => std::thread::sleep(Duration::from_secs(1)),
            done => return done,
        }
    }
    spark_run::read(&ask_spark(&line)?)
}

/// Refuses every caller but root, `euid`.
fn root_only(euid: u32) -> Result<(), String> {
    if euid == 0 {
        Ok(())
    } else {
        Err("only root runs a bundle's program as its service's account".into())
    }
}

/// What bundled says of bundle `id`: it, installed for everyone.
fn installed_for_everyone(id: &str) -> Result<Found, String> {
    let (_stream, mut input) = ask(&Request::Run { find: Find::Id(id.into()) }, None)?;
    let mut found = None;
    loop {
        match answer(&mut input).map_err(|f| f.message)? {
            Answer::Found(f) if found.is_none() => found = Some(f),
            // What a person said of its permissions: a service holds none
            // of them.
            Answer::Granted(_) if found.is_some() => {}
            Answer::Ok(_) => break,
            Answer::Error(why) => return Err(why),
            Answer::Lines(_) => {
                return Err(format!("several lines of {id} are installed: name one, {id}@LINE"));
            }
            _ => return Err("bundled answered out of turn".into()),
        }
    }
    let found = found.ok_or("bundled found nothing")?;
    if found.id != id && registry::base_id(&found.id) != id {
        return Err("bundled found another bundle than the one asked for".into());
    }
    if found.place != Place::Everyone {
        return Err(format!("{id} is not installed for everyone"));
    }
    Ok(found)
}

/// The path in the bundle `id`, whose manifest is `manifest`, of its
/// program `program`, once `service` is one of its services and the
/// service file of that name is the one bundled wrote for it, as
/// `record` says, with the account the manifest gives the service.
fn program_of(
    manifest: &Manifest,
    id: &str,
    program: &str,
    service: &str,
    record: Option<&Record>,
) -> Result<String, String> {
    let named = manifest
        .services
        .iter()
        .find(|s| s.name.0 == service)
        .ok_or_else(|| format!("{id} has no service {service}"))?;
    let account = services::account(named);
    match record {
        Some(record) if record.id == id && record.account == account => {}
        _ => {
            return Err(format!(
                "the service {service} is not the one bundled set up for {id}: its file is \
                 another's, or was never written"
            ));
        }
    }
    manifest
        .programs
        .get(program)
        .and_then(|found| found.path.get(&ARCH))
        .map(|path| path.0.clone())
        .ok_or_else(|| format!("{id} has no program {program} for this computer"))
}

/// Sends `line` to Spark's control socket, and gives its whole answer.
fn ask_spark(line: &str) -> Result<Vec<u8>, String> {
    let socket = crate::power::CONTROL;
    let failed = |e: std::io::Error| format!("{socket}: {}", util::strip_os_error(&e));
    let stream = UnixStream::connect(socket).map_err(failed)?;
    // Radix's Unix sockets take no SO_RCVTIMEO: the wait is by poll.
    let mut timed = Deadline::new(&stream, ANSWER_WAIT).map_err(failed)?;
    timed.write_all(format!("{line}\n").as_bytes()).map_err(failed)?;
    let _ = stream.shutdown(Shutdown::Write);
    let mut answer = Vec::new();
    // The first line, and the output Spark keeps: no more is read.
    let most = (spark_run::OUTPUT_MAX + 64) as u64;
    timed.take(most).read_to_end(&mut answer).map_err(failed)?;
    Ok(answer)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nginx() -> Manifest {
        let text = manifest::bundle::template("org.nginx.nginx", "nginx", "Example, Inc.", "MIT")
            .replace(
                "\"permissions\": {}",
                "\"services\": [{\"name\": \"nginx\", \"program\": \"nginx\", \"account\": \
                 \"web\"}],\n  \"permissions\": {}",
            );
        manifest::bundle::from_json(&text).unwrap()
    }

    fn record(id: &str, account: &str) -> Record {
        Record { id: id.into(), account: account.into(), written: "0".repeat(64) }
    }

    #[test]
    fn only_root_runs_a_program_as_a_service() {
        assert!(root_only(0).is_ok());
        assert!(root_only(1000).unwrap_err().contains("only root"));
    }

    #[test]
    fn the_program_is_the_bundle_s_and_the_service_file_the_one_bundled_wrote() {
        let m = nginx();
        let program = "nginx";
        let ours = record("org.nginx.nginx", "web");
        let path = program_of(&m, "org.nginx.nginx", program, "nginx", Some(&ours));
        assert_eq!(path.as_deref(), Ok("bin/nginx"));
        // A program the manifest does not have.
        let none = program_of(&m, "org.nginx.nginx", "other", "nginx", Some(&ours));
        assert!(none.unwrap_err().contains("no program other"));
        // A service it does not have.
        let none = program_of(&m, "org.nginx.nginx", program, "php", Some(&ours));
        assert!(none.unwrap_err().contains("no service php"));
        // A service file of that name that bundled did not write for it,
        // or with another account, or none at all.
        let others = [record("org.example.other", "web"), record("org.nginx.nginx", "root")];
        for record in others.iter().map(Some).chain([None]) {
            let refused = program_of(&m, "org.nginx.nginx", program, "nginx", record);
            assert!(refused.unwrap_err().contains("not the one bundled set up"));
        }
    }
}

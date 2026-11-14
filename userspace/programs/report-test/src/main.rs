// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The program the `problem-report` boot test and the lab's `reports` tests
//! crash on purpose (ADR 0531), installed as
//! `/system/tests/report/crash-test`. Each way it ends is one
//! that reportd(8) keeps a record of:
//!
//! - `crash-test segv` writes through a null pointer in
//!   `report_test::fault_here`, which `report_test::middle` calls and
//!   `report_test::outer` calls in turn, so that the frames reportd unwinds,
//!   and the signature it makes of them, have names the test looks for;
//! - `crash-test abort` ends with SIGABRT, which reportd leaves out of the
//!   signature with the rest of the abort machinery;
//! - `crash-test panic` panics with [`PANIC_MESSAGE`]: a program on Rust's
//!   standard library prints where and why to its standard error and ends
//!   with no signal, so the kernel records nothing, and reportd learns of a
//!   service's panic from the line in the service's log;
//! - `crash-test say TEXT` prints TEXT and waits until it is killed, to put a
//!   line in a service's log;
//! - `crash-test ask REQUEST` sends the line REQUEST to reportd's socket and
//!   prints each line it answers, as whoever runs it: what reportd answers
//!   depends on who asks, which the test checks as an administrator and as a
//!   user who is not one;
//! - `crash-test send HAPPENED` reports the newest crash of `crash-test` that
//!   is not reported yet, with HAPPENED as the answer to the first question,
//!   by asking reportd for the draft and sending the text it drafts, as
//!   report(1) does after its preview. A lab test runs it over SSH, where
//!   there is no terminal to press the preview's keys on.
//!
//! The test's own service files run it, so a crash is the end of a service,
//! with a line in its log that the test reads back through `report`.

use std::hint::black_box;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::process::ExitCode;
use std::time::Duration;

use serde_json::{Value, json};

/// Reportd's socket (reportd(8)).
const SOCKET: &str = "/system/live/reports/socket";

/// The longest title a report may have, in characters (reportd's protocol).
const TITLE_MAX: usize = 80;

/// What `panic` panics with. It holds no number or quotation mark, which
/// reportd leaves out of a panic's signature.
const PANIC_MESSAGE: &str = "crash-test was asked to panic";

/// The deepest function of the crash: it writes to address 0, which nothing
/// maps, so the kernel ends the process with SIGSEGV here.
#[inline(never)]
pub fn fault_here() -> u32 {
    // SAFETY: nothing is mapped at address 0, so the store faults and the
    // kernel ends the process with SIGSEGV before any memory changes; that
    // fault is what the test is for. The store is volatile so that the
    // compiler cannot take a write to a null pointer for one that never
    // happens and drop it.
    unsafe { std::ptr::null_mut::<u32>().write_volatile(42) };
    1
}

/// The caller of [`fault_here`]. It uses what that returns, so that the call
/// is not the last thing it does, which would make it a jump with no frame
/// of its own.
#[inline(never)]
pub fn middle() -> u32 {
    black_box(fault_here()) + 1
}

/// The caller of [`middle`], for the same reason.
#[inline(never)]
pub fn outer() -> u32 {
    black_box(middle()) + 1
}

/// Prints `text` and sleeps until the process is killed.
fn say(text: &str) -> ! {
    println!("{text}");
    loop {
        std::thread::sleep(Duration::from_secs(3600));
    }
}

/// Sends `request` to reportd and returns the lines it answers, up to the
/// end of the connection, which reportd closes after the last one.
fn lines(request: &str) -> Result<Vec<String>, String> {
    let mut stream =
        UnixStream::connect(SOCKET).map_err(|err| format!("cannot connect to {SOCKET}: {err}"))?;
    writeln!(stream, "{request}").map_err(|err| format!("cannot send to {SOCKET}: {err}"))?;
    BufReader::new(stream)
        .lines()
        .collect::<Result<_, _>>()
        .map_err(|err| format!("cannot read from {SOCKET}: {err}"))
}

/// Sends `request` to reportd and prints what it answers.
fn ask(request: &str) -> Result<(), String> {
    for line in lines(request)? {
        println!("{line}");
    }
    Ok(())
}

/// Asks reportd `request` and returns each answer as JSON, or the words of
/// its refusal.
fn exchange(request: &Value) -> Result<Vec<Value>, String> {
    let answers: Vec<Value> = lines(&request.to_string())?
        .iter()
        .map(|line| serde_json::from_str(line).map_err(|err| format!("{line}: {err}")))
        .collect::<Result<_, _>>()?;
    match answers.first().and_then(|answer| answer["error"].as_str()) {
        Some(error) => Err(format!("reportd refuses: {error}")),
        None => Ok(answers),
    }
}

/// Reports the newest crash of this program that is not reported yet, as
/// the person at the preview would: reportd drafts the text, and the same
/// text is sent unchanged.
fn send(happened: &str) -> Result<(), String> {
    let crashes = exchange(&json!({"ask": "crashes"}))?;
    let id = crashes
        .iter()
        .find(|crash| crash["program"] == "crash-test" && crash["reported"].is_null())
        .and_then(|crash| crash["id"].as_str())
        .ok_or("reportd keeps no crash of crash-test that is not reported yet")?;
    let drafted = exchange(&json!({
        "ask": "draft",
        "about": {"kind": "crash", "id": id},
        "answers": {"happened": happened},
    }))?;
    let text = drafted
        .first()
        .and_then(|draft| draft["text"].as_str())
        .ok_or("reportd drafted no text")?;
    let title: String = happened.chars().take(TITLE_MAX).collect();
    let sent = exchange(&json!({"ask": "send", "text": text, "title": title, "crash": id}))?;
    let sent = sent.first().ok_or("reportd answered nothing to the report")?;
    match (sent["number"].as_u64(), sent["waiting"].as_str()) {
        (Some(number), _) => println!("sent {number}"),
        (None, Some(why)) => println!("waiting: {why}"),
        (None, None) => return Err(format!("reportd answered {sent}")),
    }
    Ok(())
}

/// Ends the way the command did: success, or a line that says why not.
fn finish(result: Result<(), String>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("crash-test: {err}");
            ExitCode::FAILURE
        }
    }
}

fn main() -> ExitCode {
    let mut arguments = std::env::args().skip(1);
    match (arguments.next().as_deref(), arguments.next(), arguments.next()) {
        (Some("segv"), None, None) => {
            // A machine that maps address 0 does not fault there.
            eprintln!("crash-test: the write to address 0 did not fault ({})", outer());
            ExitCode::from(3)
        }
        (Some("abort"), None, None) => std::process::abort(),
        (Some("panic"), None, None) => panic!("{PANIC_MESSAGE}"),
        (Some("say"), Some(text), None) => say(&text),
        (Some("ask"), Some(request), None) => finish(ask(&request)),
        (Some("send"), Some(happened), None) => finish(send(&happened)),
        _ => {
            eprintln!(
                "usage: crash-test segv | abort | panic | say TEXT | ask REQUEST | send HAPPENED"
            );
            ExitCode::from(2)
        }
    }
}

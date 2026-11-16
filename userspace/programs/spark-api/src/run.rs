// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A program run once as a process of a service's own (ADR 0569): root
//! asks Spark, on its control socket, `run NAME PROGRAM [ARGUMENT...]`,
//! and Spark starts PROGRAM as it starts the service's health checks, in
//! the service's resource domain and view, as its account, with its
//! environment, `/devices/null` as its input and a pipe as its output and
//! errors. It answers once the program ends: `ran CODE`, the code as a
//! shell gives it (128 and the signal's number for one a signal ended),
//! or `stopped` when it ran past [`SECONDS`] and was killed, then what the
//! program wrote, the first [`OUTPUT_MAX`] bytes of it. A refusal is
//! `error: WHY`, as Spark's other answers are.
//!
//! Only a service with an account of its own is run so, and not one that
//! holds the names capability or runs with the whole tree: what runs
//! holds no more than the service does, and never acts for root. One run
//! is under way at a time.
//!
//! Its one caller is website(8), which has nginx check its settings as
//! the account nginx, in nginx's view, before they are used.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// The request's first word.
pub const VERB: &str = "run";

/// How long a program may run before Spark kills it.
pub const SECONDS: u64 = 30;

/// How much of what the program writes is kept for the answer; the rest
/// is read and dropped.
pub const OUTPUT_MAX: usize = 16 * 1024;

/// The longest request Spark reads, its line ended.
pub const LINE_MAX: usize = 1024;

/// Spark's words, after `error: `, while another program runs: the
/// caller asks again a moment later.
pub const BUSY: &str = "another program runs as a service's now; ask again";

/// What to run, and as which service.
#[derive(Debug, PartialEq, Eq)]
pub struct Run {
    /// The service's name, its file's without `.toml`.
    pub service: String,
    /// The program's absolute path.
    pub program: String,
    /// What it is run with.
    pub arguments: Vec<String>,
}

/// Whether `word` can be a word of the request: not empty, and with no
/// white space or control character, as the words are split on spaces.
fn is_word(word: &str) -> bool {
    !word.is_empty() && !word.chars().any(|c| c.is_whitespace() || c.is_control())
}

impl Run {
    /// The request's line, without its newline.
    ///
    /// # Errors
    ///
    /// When a word is empty or holds white space or a control
    /// character, the program's path is not absolute, or the line is
    /// longer than Spark reads.
    pub fn line(&self) -> Result<String, String> {
        let words = [&self.service, &self.program].into_iter().chain(&self.arguments);
        let mut line = String::from(VERB);
        for word in words {
            if !is_word(word) {
                return Err(format!(
                    "{word:?} cannot be sent to Spark: a word may not be empty or hold a space"
                ));
            }
            line.push(' ');
            line.push_str(word);
        }
        if !self.program.starts_with('/') {
            return Err(format!("{} is not an absolute path", self.program));
        }
        if line.len() + 1 > LINE_MAX {
            return Err(format!("the request is longer than Spark reads ({LINE_MAX} bytes)"));
        }
        Ok(line)
    }

    /// The request `words`, the line's words after [`VERB`].
    ///
    /// # Errors
    ///
    /// When it names no service or no program, or the program's path is
    /// not absolute.
    pub fn parse(words: &str) -> Result<Run, String> {
        let mut words = words.split(' ').filter(|w| !w.is_empty());
        let service = words.next().ok_or("run names a service and a program")?;
        let program = words.next().ok_or("run names a service and a program")?;
        if !program.starts_with('/') {
            return Err(format!("{program} is not an absolute path"));
        }
        Ok(Run {
            service: service.to_string(),
            program: program.to_string(),
            arguments: words.map(ToString::to_string).collect(),
        })
    }
}

/// The code a shell gives for the wait status `status`: the exit code, or
/// 128 and the number of the signal that ended it.
#[must_use]
pub fn code(status: u32) -> i32 {
    if status & 0x7f == 0 { ((status >> 8) & 0xff) as i32 } else { 128 + (status & 0x7f) as i32 }
}

/// Keeps what of `bytes` fits in `kept`'s [`OUTPUT_MAX`].
pub fn keep(kept: &mut Vec<u8>, bytes: &[u8]) {
    let room = OUTPUT_MAX.saturating_sub(kept.len());
    kept.extend_from_slice(&bytes[..bytes.len().min(room)]);
}

/// The answer's first line, with its newline: the code, or `stopped`.
#[must_use]
pub fn head(code: i32, stopped: bool) -> String {
    if stopped { "stopped\n".to_string() } else { format!("ran {code}\n") }
}

/// How the program ended, and what it wrote.
#[derive(Debug, PartialEq, Eq)]
pub struct Ran {
    /// Its code, as a shell gives it; none when it was stopped.
    pub code: Option<i32>,
    /// What it wrote, at most [`OUTPUT_MAX`] bytes.
    pub output: Vec<u8>,
}

/// Reads Spark's `answer`.
///
/// # Errors
///
/// Spark's words when it refused, and when the answer is not one.
pub fn read(answer: &[u8]) -> Result<Ran, String> {
    let end = answer.iter().position(|&b| b == b'\n').ok_or("Spark's answer had no line")?;
    let first = core::str::from_utf8(&answer[..end]).map_err(|_| "Spark's answer was not text")?;
    let output = answer[end + 1..].to_vec();
    if let Some(why) = first.strip_prefix("error: ") {
        return Err(why.to_string());
    }
    if output.len() > OUTPUT_MAX {
        return Err("Spark's answer held more than a run keeps".into());
    }
    if first == "stopped" {
        return Ok(Ran { code: None, output });
    }
    let code = first
        .strip_prefix("ran ")
        .and_then(|code| code.parse::<i32>().ok())
        .ok_or_else(|| format!("Spark answered {first:?}"))?;
    Ok(Ran { code: Some(code), output })
}

#[cfg(test)]
mod tests {
    use alloc::vec;

    use super::*;

    fn nginx_check() -> Run {
        Run {
            service: "nginx".into(),
            program: "/applications/org.nginx.nginx/bin/nginx".into(),
            arguments: vec!["-t".into(), "-q".into(), "-c".into(), "/x/.website-check.conf".into()],
        }
    }

    #[test]
    fn a_request_is_a_line_of_words_that_reads_back_as_it_was() {
        let run = nginx_check();
        let line = run.line().unwrap();
        assert_eq!(
            line,
            "run nginx /applications/org.nginx.nginx/bin/nginx -t -q -c /x/.website-check.conf"
        );
        assert_eq!(Run::parse(line.strip_prefix("run ").unwrap()).unwrap(), run);
        // Without arguments.
        let bare = Run { arguments: Vec::new(), ..nginx_check() };
        assert_eq!(Run::parse(&bare.line().unwrap()[4..]).unwrap(), bare);
    }

    #[test]
    fn a_word_that_would_not_read_back_is_refused() {
        for word in ["", "two words", "tab\there", "line\nend", "nul\0"] {
            let run = Run { arguments: vec![word.into()], ..nginx_check() };
            assert!(run.line().is_err(), "{word:?}");
        }
        let relative = Run { program: "bin/nginx".into(), ..nginx_check() };
        assert!(relative.line().is_err());
        let long = Run { arguments: vec!["x".repeat(LINE_MAX)], ..nginx_check() };
        assert!(long.line().unwrap_err().contains("longer"));
    }

    #[test]
    fn a_request_names_a_service_and_an_absolute_program() {
        assert!(Run::parse("").is_err());
        assert!(Run::parse("nginx").is_err());
        assert!(Run::parse("nginx bin/nginx -t").unwrap_err().contains("absolute"));
        let run = Run::parse("nginx  /bin/nginx  -t").unwrap();
        assert_eq!(run.arguments, ["-t"]);
    }

    #[test]
    fn a_status_is_a_shell_s_code() {
        assert_eq!(code(0), 0);
        assert_eq!(code(1 << 8), 1);
        assert_eq!(code(9), 137);
        assert_eq!(code(15), 143);
    }

    #[test]
    fn output_is_kept_up_to_its_room() {
        let mut kept = Vec::new();
        keep(&mut kept, &[b'a'; 10]);
        keep(&mut kept, &vec![b'b'; OUTPUT_MAX]);
        keep(&mut kept, b"c");
        assert_eq!(kept.len(), OUTPUT_MAX);
        assert_eq!(&kept[..11], b"aaaaaaaaaab");
    }

    #[test]
    fn an_answer_reads_as_its_code_and_its_output() {
        let answer = [head(1, false).as_bytes(), b"nginx: [emerg] unknown directive\n"].concat();
        assert_eq!(
            read(&answer).unwrap(),
            Ran { code: Some(1), output: b"nginx: [emerg] unknown directive\n".to_vec() }
        );
        assert_eq!(read(b"ran 0\n").unwrap(), Ran { code: Some(0), output: Vec::new() });
        assert_eq!(read(head(0, true).as_bytes()).unwrap().code, None);
        assert_eq!(read(b"error: nginx runs as root\n").unwrap_err(), "nginx runs as root");
        for wrong in [&b""[..], b"ran", b"ran x\n", b"done\n", b"\xff\n"] {
            assert!(read(wrong).is_err(), "{wrong:?}");
        }
        let too_much = [&b"ran 0\n"[..], &vec![b'x'; OUTPUT_MAX + 1]].concat();
        assert!(read(&too_much).is_err());
    }
}

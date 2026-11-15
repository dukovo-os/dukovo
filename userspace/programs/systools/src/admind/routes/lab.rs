// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The route that only the lab's build has (ADR 0514): `POST
//! /api/1/test/elevate`, which runs a command as an administrator for an
//! unlocked session, the way an action of the console's does and through
//! the same code, so that the lab can test the unlock, `console-elevate`'s
//! four descriptors and the hang-up of the command when its relay closes,
//! with any command. The shipped admind is built without the `lab`
//! feature and has no such route.
//!
//! The body is `{"command": ["/system/commands/echo", "hello"]}`, or the
//! command as one line of words apart by spaces; the answer is
//! `{"output", "errors", "status"}`: the command's output and its error
//! output, kept as an action's are, and the exit code a shell would see.
//! It shares the limit of 32 elevated commands at once with the actions.

use accounts::console::Elevator;
use serde::Deserialize;
use serde_json::json;

use super::{Context, Live, Outcome, Reply, Text, refused, shown};
use crate::admind::errors::{Failure, Kind};
use crate::admind::run;

/// A command, as the body gives it.
#[derive(Deserialize)]
#[serde(untagged)]
enum Command {
    /// Each word.
    Words(Vec<Text>),
    /// One line, whose words are apart by spaces.
    Line(Text),
}

/// The words of a command.
fn words(command: &Command) -> Result<Vec<String>, Failure> {
    let words: Vec<String> = match command {
        Command::Words(words) => words.iter().map(|word| word.to_string()).collect(),
        Command::Line(line) => line.split_whitespace().map(str::to_string).collect(),
    };
    if words.is_empty() {
        return Err(Failure::new(Kind::BadRequest, "give a command to run"));
    }
    Ok(words)
}

/// `POST /api/1/test/elevate`.
pub async fn elevate(cx: &Context<'_>, live: &Live) -> Outcome {
    #[derive(Deserialize)]
    struct Body {
        command: Command,
    }
    let body: Body = cx.json()?;
    let argv = words(&body.command)?;
    let by = Elevator::Session(live.authd.clone());
    let ran =
        run::elevated(cx.shared, by, &argv, None, run::RUN_TIME).await.map_err(|failure| {
            let what = format!("to run a command for {}", shown(&live.name));
            if failure.kind.status() == 403 { refused(cx, &what, failure) } else { failure }
        })?;
    Ok(Reply::json(json!({ "output": ran.output, "errors": ran.errors, "status": ran.status })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_command_is_its_words_or_a_line_of_them_and_there_must_be_one() {
        let echo = ["/system/commands/echo".to_string(), "hi".to_string()];
        let given = |json: &str| words(&serde_json::from_str::<Command>(json).unwrap());
        assert_eq!(given(r#"["/system/commands/echo", "hi"]"#).unwrap(), echo);
        assert_eq!(given(r#""/system/commands/echo  hi ""#).unwrap(), echo);
        assert_eq!(given("[]").unwrap_err().kind, Kind::BadRequest);
        assert_eq!(given(r#""  ""#).unwrap_err().kind, Kind::BadRequest);
        // A word with a NUL would split a field of the request.
        assert!(serde_json::from_str::<Command>(r#"["a\u0000b"]"#).is_err());
    }
}

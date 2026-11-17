// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

use serde_json::json;

use super::*;

/// A task that waits, numbered `id`.
fn task(id: i64) -> Task {
    Task {
        id,
        action: "lab.long".into(),
        args: json!({}),
        argv: vec!["admin".into(), "lab-steps".into()],
        who: "chase".into(),
        how: "console".into(),
        started: 1_000,
        state: State::Waiting,
        step: 0,
        steps: 0,
        words: String::new(),
        percent: None,
        ended: None,
        status: None,
        note: None,
        log: Vec::new(),
        changed: None,
    }
}

#[test]
fn a_step_and_a_progress_line_are_read_and_nothing_else_is() {
    assert_eq!(
        progress("step 2 of 5: Copying the files"),
        Some(Progress::Step { step: 2, steps: 5, words: "Copying the files".into() })
    );
    assert_eq!(
        progress("step 1 of 1: x  "),
        Some(Progress::Step { step: 1, steps: 1, words: "x".into() })
    );
    assert_eq!(progress("progress 0%"), Some(Progress::Percent(0)));
    assert_eq!(progress("progress 100%"), Some(Progress::Percent(100)));
    assert_eq!(progress("progress 42%  "), Some(Progress::Percent(42)));
    for line in [
        "",
        "step 0 of 5: Nothing",
        "step 6 of 5: Too far",
        "step 2 of 0: None",
        "step 2 of 5:",
        "step 2 of 5: ",
        "step 2 of 5 Copying",
        "step two of 5: Copying",
        "step -1 of 5: Copying",
        "step 2 of x: Copying",
        "Step 2 of 5: Copying",
        " step 2 of 5: Copying",
        "step  2 of 5: Copying",
        "progress 101%",
        "progress -1%",
        "progress 5",
        "progress %",
        "progress 5 %",
        "progress 50.5%",
        "progress 9999999999%",
        "downloading: progress 50%",
    ] {
        assert_eq!(progress(line), None, "{line:?}");
    }
}

#[test]
fn the_words_of_a_step_are_cut_and_have_no_control_characters() {
    let Some(Progress::Step { words, .. }) = progress(&format!("step 1 of 2: {}", "é".repeat(500)))
    else {
        panic!("a step");
    };
    assert_eq!(words.chars().count(), WORDS_MAX);
    let Some(Progress::Step { words, .. }) = progress("step 1 of 2: a\u{1b}[31mred") else {
        panic!("a step");
    };
    assert_eq!(words, "a\u{fffd}[31mred");
}

#[test]
fn bytes_are_cut_into_lines_wherever_they_split() {
    let mut splitter = LineSplitter::default();
    assert_eq!(splitter.push(b"one\ntw"), ["one"]);
    assert_eq!(splitter.push(b"o\n\nthree"), ["two", ""]);
    assert!(splitter.push(b"").is_empty());
    assert_eq!(splitter.push(b"\nfour"), ["three"]);
    assert_eq!(splitter.finish().as_deref(), Some("four"));
    assert_eq!(splitter.finish(), None);
    // Bytes that are not UTF-8 are shown as the replacement character.
    assert_eq!(splitter.push(b"a\xffb\n"), ["a\u{fffd}b"]);
}

#[test]
fn a_line_longer_than_the_limit_keeps_its_start_and_the_next_line_is_whole() {
    let mut splitter = LineSplitter::default();
    let long = vec![b'x'; LINE_MAX * 3];
    assert!(splitter.push(&long).is_empty());
    let lines = splitter.push(b"\nnext\n");
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0].len(), LINE_MAX);
    assert_eq!(lines[1], "next");
    // A line of exactly the limit is kept whole.
    let exact = vec![b'y'; LINE_MAX];
    let mut again = LineSplitter::default();
    assert_eq!(again.push(&[exact.as_slice(), b"\n"].concat())[0].len(), LINE_MAX);
}

#[test]
fn the_log_keeps_the_last_2000_lines_and_at_most_256_kib() {
    let tasks = Tasks::new();
    tasks.add(task(1));
    for number in 0..2500 {
        tasks.line(1, Stream::Output, &format!("line {number}"));
    }
    let log = tasks.live.borrow()[&1].full().log;
    assert_eq!(log.len(), 2000);
    assert_eq!((log[0].as_str(), log[1999].as_str()), ("line 500", "line 2499"));
    // Long lines fill the bytes before the lines.
    tasks.add(task(2));
    let long = "z".repeat(LINE_MAX);
    for _ in 0..1000 {
        tasks.line(2, Stream::Errors, &long);
    }
    let entry = tasks.live.borrow();
    assert_eq!(entry[&2].log.len(), LOG_BYTES_MAX / LINE_MAX);
    assert!(entry[&2].log_bytes <= LOG_BYTES_MAX);
}

#[test]
fn a_progress_is_said_on_the_output_a_step_on_either_stream_and_neither_is_the_log() {
    let tasks = Tasks::new();
    tasks.add(task(1));
    tasks.line(1, Stream::Output, "step 1 of 3: Starting");
    tasks.line(1, Stream::Output, "progress 40%");
    tasks.line(1, Stream::Errors, "progress 90%");
    tasks.line(1, Stream::Output, "   ");
    tasks.line(1, Stream::Output, "plain line");
    let now = tasks.live.borrow()[&1].full();
    assert_eq!(
        (now.step, now.steps, now.words.as_str(), now.percent),
        (1, 3, "Starting", Some(40))
    );
    assert_eq!(now.log, ["progress 90%", "plain line"]);
    // A new step starts its progress again.
    tasks.line(1, Stream::Output, "step 2 of 3: Next");
    assert_eq!(tasks.live.borrow()[&1].task.percent, None);
    // A command that prints JSON writes its steps on its errors.
    tasks.line(1, Stream::Errors, "step 3 of 3: memory pattern 2 passed");
    let now = tasks.live.borrow()[&1].full();
    assert_eq!((now.step, now.words.as_str()), (3, "memory pattern 2 passed"));
    assert_eq!(now.log, ["progress 90%", "plain line"]);
}

#[test]
fn what_a_stream_is_told_of_is_each_change_once_and_each_task_four_times_a_second_at_most() {
    let tasks = Tasks::new();
    tasks.add(task(1));
    tasks.add(task(2));
    let mut told = Told::default();
    let start = Instant::now();
    // A tab that opens is told of each task.
    let (due, wait) = tasks.events(&mut told, start);
    assert_eq!(due.len(), 2);
    assert_eq!(wait, None);
    let first = String::from_utf8(due[0].to_vec()).unwrap();
    assert!(first.starts_with("event: task\ndata: {"), "{first}");
    assert!(first.contains("\"id\":1") && first.contains("\"state\":\"waiting\""), "{first}");
    assert!(!first.contains("argv") && !first.contains("log"), "{first}");
    assert!(first.ends_with("\n\n"));
    // Nothing changed, nothing is told.
    assert!(tasks.events(&mut told, start).0.is_empty());
    // A change soon after waits for the time to be up, and the latest is told.
    tasks.line(1, Stream::Output, "progress 10%");
    let soon = start + Duration::from_millis(100);
    let (due, wait) = tasks.events(&mut told, soon);
    assert!(due.is_empty());
    assert_eq!(wait, Some(EVENT_SPACE - Duration::from_millis(100)));
    tasks.line(1, Stream::Output, "progress 20%");
    tasks.line(1, Stream::Output, "progress 30%");
    let (due, wait) = tasks.events(&mut told, start + EVENT_SPACE);
    assert_eq!((due.len(), wait), (1, None));
    assert!(String::from_utf8(due[0].to_vec()).unwrap().contains("\"percent\":30"));
    // A task whose change comes at a different time has its own clock.
    tasks.line(2, Stream::Output, "progress 1%");
    assert_eq!(tasks.events(&mut told, start + EVENT_SPACE).0.len(), 1);
    // A log line is not told: the log is read from the task.
    tasks.line(1, Stream::Output, "only the log");
    assert!(tasks.events(&mut told, start + EVENT_SPACE * 4).0.is_empty());
}

#[test]
fn a_task_is_never_told_more_than_four_times_in_a_second() {
    let tasks = Tasks::new();
    tasks.add(task(1));
    let mut told = Told::default();
    let start = Instant::now();
    let mut times = Vec::new();
    // A task that changes every 10 milliseconds for 3 seconds.
    for step in 0..300u32 {
        tasks.line(1, Stream::Output, &format!("progress {}%", step % 100));
        let now = start + Duration::from_millis(u64::from(step) * 10);
        let (due, _) = tasks.events(&mut told, now);
        times.extend(due.iter().map(|_| now));
    }
    assert!(times.len() >= 10, "{}", times.len());
    for time in &times {
        let in_a_second =
            times.iter().filter(|other| *other >= time && **other < *time + Duration::from_secs(1));
        assert!(in_a_second.count() <= 4);
    }
}

#[test]
fn a_task_that_ended_is_told_once_and_forgotten_after_a_minute() {
    let tasks = Tasks::new();
    let store = Store::in_memory();
    let id = store.add_task("lab.long", &json!({}), &[], "chase", "console", 1_000).unwrap();
    tasks.add(task(id));
    let mut told = Told::default();
    let now = Instant::now();
    tasks.events(&mut told, now);
    tasks.end(
        &store,
        id,
        &End { state: State::Done, status: Some(0), note: None, changed: Some(true) },
    );
    let (due, _) = tasks.events(&mut told, now + EVENT_SPACE);
    assert_eq!(due.len(), 1);
    assert!(String::from_utf8(due[0].to_vec()).unwrap().contains("\"state\":\"done\""));
    assert!(tasks.get(&store, id).unwrap().is_some());
    tasks.prune(now + Duration::from_secs(30));
    assert_eq!(tasks.live.borrow().len(), 1);
    tasks.prune(Instant::now() + Duration::from_secs(61));
    assert!(tasks.live.borrow().is_empty());
    // It is the store's now, and a stream no longer has it.
    assert_eq!(tasks.get(&store, id).unwrap().unwrap().state, State::Done);
    assert!(tasks.events(&mut told, now + Duration::from_secs(90)).0.is_empty());
    assert!(told.sent.is_empty());
}

#[test]
fn the_live_task_is_read_before_the_store_s() {
    let tasks = Tasks::new();
    let store = Store::in_memory();
    let id =
        store.add_task("lab.long", &json!({}), &["x".to_string()], "chase", "console", 1).unwrap();
    tasks.add(Task { id, ..task(id) });
    tasks.line(id, Stream::Output, "step 1 of 2: Live");
    tasks.line(id, Stream::Output, "a log line");
    assert_eq!(store.task(id).unwrap().unwrap().steps, 0);
    let live = tasks.get(&store, id).unwrap().unwrap();
    assert_eq!((live.step, live.log.as_slice()), (1, &["a log line".to_string()][..]));
    let listed = tasks.list(&store, 0, 10).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].words, "Live");
    assert_eq!(tasks.get(&store, id + 1).unwrap(), None);
}

#[test]
fn a_task_is_saved_when_it_changes_and_five_seconds_have_passed() {
    let tasks = Tasks::new();
    let store = Store::in_memory();
    let id = store.add_task("lab.long", &json!({}), &[], "chase", "console", 1).unwrap();
    tasks.add(task(id));
    tasks.running(&store, id);
    assert_eq!(store.task(id).unwrap().unwrap().state, State::Running);
    tasks.line(id, Stream::Output, "progress 10%");
    tasks.line(id, Stream::Output, "log");
    let later = Instant::now();
    tasks.save_if_due(&store, id, later);
    assert_eq!(store.task(id).unwrap().unwrap().percent, None);
    assert!(tasks.save_due(id).is_some());
    tasks.save_if_due(&store, id, later + SAVE_EVERY);
    let saved = store.task(id).unwrap().unwrap();
    assert_eq!((saved.percent, saved.log.as_slice()), (Some(10), &["log".to_string()][..]));
    assert_eq!(tasks.save_due(id), None);
}

#[test]
fn a_command_ends_a_task_by_how_it_ended() {
    let limits = Limits { time: Duration::from_hours(12), grace: STOP_GRACE };
    let done = |status, stopped_by: Option<&str>, out_of_time, last_error: Option<&str>| Done {
        status,
        stopped_by: stopped_by.map(String::from),
        out_of_time,
        last_error: last_error.map(String::from),
    };
    let concluded = |done: &Done| {
        let end = conclude(done, "admin", limits, &[0, 2]);
        (end.state, end.status, end.note)
    };
    assert_eq!(concluded(&done(Some(0), None, false, None)), (State::Done, Some(0), None));
    // An error line on a command that worked is no failure.
    assert_eq!(concluded(&done(Some(0), None, false, Some("warning"))).0, State::Done);
    assert_eq!(
        concluded(&done(Some(1), None, false, Some("it broke"))),
        (State::Failed, Some(1), Some("it broke".into()))
    );
    assert_eq!(
        concluded(&done(Some(3), None, false, None)),
        (State::Failed, Some(3), Some("admin ended with status 3".into()))
    );
    // A status the action takes as an answer, as a check's warning, is
    // done, whatever its last error line said.
    assert_eq!(
        concluded(&done(Some(2), None, false, Some("one warning"))),
        (State::Done, Some(2), None)
    );
    // A stop is a stop whether the command ends well, as one that puts back
    // what it did does, or not.
    for status in [0, 143] {
        assert_eq!(
            concluded(&done(Some(status), Some("alice"), false, None)),
            (State::Stopped, Some(status), Some("stopped by alice".into()))
        );
    }
    assert_eq!(
        concluded(&done(None, Some("alice"), false, None)).2.as_deref(),
        Some("stopped by alice, and the command did not end after it was hung up")
    );
    // Time ran out: not done, even if the command ended well after SIGTERM.
    let out = concluded(&done(Some(0), None, true, None));
    assert_eq!(
        out,
        (State::Failed, Some(0), Some("it did not end in 12 hours, and was stopped".into()))
    );
    assert_eq!(
        concluded(&done(None, None, false, None)),
        (State::Failed, None, Some("the command did not end after it was hung up".into()))
    );
}

#[test]
fn a_task_that_is_done_changed_nothing_only_when_its_files_read_the_same() {
    let file = ["/system/settings/a.toml".to_string()];
    assert!(!task_changed(&file, true));
    assert!(task_changed(&file, false));
    // With no files there is nothing to compare: it did something.
    assert!(task_changed(&[], true));
}

#[test]
fn a_time_is_said_in_the_unit_that_is_whole() {
    assert_eq!(lasting(Duration::from_hours(12)), "12 hours");
    assert_eq!(lasting(Duration::from_hours(1)), "1 hour");
    assert_eq!(lasting(Duration::from_mins(90)), "90 minutes");
    assert_eq!(lasting(Duration::from_mins(1)), "1 minute");
    assert_eq!(lasting(Duration::from_secs(30)), "30 seconds");
    assert_eq!(lasting(Duration::from_secs(1)), "1 second");
}

#[test]
fn only_a_task_that_waits_or_runs_is_stopped_and_once_is_enough() {
    let tasks = Tasks::new();
    tasks.add(task(1));
    assert!(tasks.stop(1, "alice").is_ok());
    assert_eq!(tasks.stop_asked(1).as_deref(), Some("alice"));
    // Asked again, it is still alice who stopped it.
    assert!(tasks.stop(1, "chase").is_ok());
    assert_eq!(tasks.stop_asked(1).as_deref(), Some("alice"));
    let store = Store::in_memory();
    let id = store.add_task("lab.long", &json!({}), &[], "chase", "console", 1).unwrap();
    tasks.add(task(id));
    tasks.end(
        &store,
        id,
        &End { state: State::Done, status: Some(0), note: None, changed: Some(true) },
    );
    assert_eq!(tasks.stop(id, "alice").unwrap_err().kind, Kind::Cannot);
    assert_eq!(tasks.stop(99, "alice").unwrap_err().kind, Kind::Cannot);
}

#[test]
fn the_tasks_an_admind_left_are_ended_as_stopped_and_the_old_are_forgotten() {
    let store = Store::in_memory();
    let waiting = store.add_task("a", &json!({}), &[], "chase", "console", 1).unwrap();
    let running = store.add_task("a", &json!({}), &[], "chase", "console", 2).unwrap();
    let done = store.add_task("a", &json!({}), &[], "chase", "console", 3).unwrap();
    let old = store.add_task("a", &json!({}), &[], "chase", "console", 4).unwrap();
    let (recent, ancient) = (unix_now() - 100, unix_now() - 31 * 86_400);
    for (id, state, ended) in [
        (running, State::Running, None),
        (done, State::Done, Some(recent)),
        (old, State::Done, Some(ancient)),
    ] {
        let mut row = store.task(id).unwrap().unwrap();
        (row.state, row.ended) = (state, ended);
        store.save_task(&row).unwrap();
    }
    restarted(&store);
    for id in [waiting, running] {
        let row = store.task(id).unwrap().unwrap();
        assert_eq!(row.state, State::Stopped, "{id}");
        assert_eq!(row.note.as_deref(), Some("the console restarted"));
        assert!(row.ended.is_some_and(|at| at > 1_700_000_000));
    }
    assert_eq!(store.task(done).unwrap().unwrap().state, State::Done);
    assert_eq!(store.task(done).unwrap().unwrap().ended, Some(recent));
    // Ended more than 30 days ago, it is forgotten; the journal keeps the change.
    assert!(store.task(old).unwrap().is_none());
}

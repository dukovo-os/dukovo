// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Host tests: hand-written cases for each module, and random operations
//! on the process tree checked against its invariants.

use std::alloc::Global;

use proptest::prelude::*;
use radix_abi::errors::Kernel;
use radix_abi::event_queue::{
    QueueEvent, QueueFilter, QueueFlags, USER_AND, USER_CONTROL, USER_COPY, USER_FLAGS, USER_OR,
    USER_TRIGGER,
};
use radix_abi::process::{EXEC_VERSION, ExecHeader, PollEvents};
use radix_abi::signal::{
    ActionFlags, IGNORE, SIGCHLD, SIGCONT, SIGINT, SIGKILL, SIGNALS, SIGRTMIN, SIGSEGV, SIGSTOP,
    SIGTERM, SIGTSTP, SIGUSR1,
};

use crate::exec::{Block, BlockError, Shebang, Stack, aux, shebang};
use crate::ipc::{
    self, Caller, GetError, NOWAIT, OpError, Operation, Outcome, Perm, SEMVMX, Table, TableError,
    UNDO,
};
use crate::signal::{self, Action, ActionError, Actions, Disposition, Info, Pending};
use crate::status::{self, Ended};
use crate::tree::{Exit, Found, Life, Reaps, Tree, TreeError, WaitFor, Which};
use crate::watch::{self, Asks, Invalid, State, Then};

type T = Tree<u64, Global>;

#[test]
fn statuses_read_as_linux_reads_them() {
    // WIFEXITED: (status & 0x7f) == 0; WEXITSTATUS: (status >> 8) & 0xff.
    let exited = status::exited(0x1_2a);
    assert_eq!((exited & 0x7f, (exited >> 8) & 0xff), (0, 0x2a));
    // WIFSIGNALED: ((status & 0x7f) + 1) as i8 >= 2; WTERMSIG: status & 0x7f.
    let killed = status::killed(SIGSEGV);
    assert!((((killed & 0x7f) + 1) as i8) >= 2);
    assert_eq!(killed & 0x7f, SIGSEGV);
    // WIFSTOPPED: (status & 0xff) == 0x7f; WSTOPSIG: (status >> 8) & 0xff.
    let stopped = status::stopped(SIGTSTP);
    assert_eq!((stopped & 0xff, (stopped >> 8) & 0xff), (0x7f, SIGTSTP));
    assert_eq!(status::CONTINUED, 0xffff);
    assert_eq!(Ended::Killed(SIGINT).code(), 2);
    assert_eq!(Ended::Exited(300).value(), 300 % 256);
}

#[test]
fn kill_and_stop_keep_their_default() {
    let mut actions = Actions::new();
    let handler = Action { handler: 0x1000, flags: 0, mask: u64::MAX, restorer: 0x2000 };
    assert_eq!(actions.set(SIGKILL, handler), Err(ActionError::Unchangeable));
    assert_eq!(
        actions.set(SIGSTOP, Action { handler: IGNORE, ..handler }),
        Err(ActionError::Unchangeable)
    );
    assert_eq!(actions.set(0, handler), Err(ActionError::NoSuchSignal));
    assert_eq!(actions.set(65, handler), Err(ActionError::NoSuchSignal));
    assert_eq!(actions.set(SIGUSR1, handler), Ok(Action::default()));
    // The mask a handler runs with never blocks SIGKILL or SIGSTOP.
    assert_eq!(actions.get(SIGUSR1).mask & signal::UNBLOCKABLE, 0);
    assert_eq!(actions.disposition(SIGUSR1), Disposition::Handle(actions.get(SIGUSR1)));
    assert_eq!(actions.disposition(SIGTERM), Disposition::Terminate);
    assert_eq!(actions.disposition(SIGTSTP), Disposition::Stop);
    assert_eq!(actions.disposition(SIGCHLD), Disposition::Ignore);
    assert!(actions.ignores(SIGCHLD) && !actions.ignores(SIGTERM));
    // Ignored by default, but its children wait for `wait`.
    assert!(!actions.reaps_children());
    let ignore = Action { handler: IGNORE, ..Action::default() };
    assert_eq!(actions.set(SIGCHLD, ignore), Ok(Action::default()));
    assert!(actions.reaps_children());
    let no_wait = Action { flags: ActionFlags::NO_CHILD_WAIT.0, ..Action::default() };
    assert_eq!(actions.set(SIGCHLD, no_wait), Ok(ignore));
    assert!(actions.reaps_children());
}

#[test]
fn exec_keeps_only_ignored_signals() {
    let mut actions = Actions::new();
    let handler = Action { handler: 0x1000, flags: ActionFlags::RESTART.0, mask: 0, restorer: 1 };
    actions.set(SIGUSR1, handler).unwrap();
    actions.set(SIGINT, Action { handler: IGNORE, ..Action::default() }).unwrap();
    actions.reset_for_exec();
    assert_eq!(actions.get(SIGUSR1), Action::default());
    assert_eq!(actions.get(SIGINT).handler, IGNORE);
}

#[test]
fn reset_hand_runs_a_handler_once() {
    let mut actions = Actions::new();
    let once = Action { handler: 0x1000, flags: ActionFlags::RESET_HAND.0, mask: 0, restorer: 1 };
    actions.set(SIGUSR1, once).unwrap();
    actions.handled(SIGUSR1);
    assert_eq!(actions.get(SIGUSR1), Action::default());
}

/// Nothing pending, kept on the host's heap.
fn pending() -> Pending<[Info; SIGNALS as usize], Global> {
    Pending::in_storage([Info::default(); SIGNALS as usize], Global).unwrap()
}

#[test]
fn faults_come_first_and_real_time_signals_queue() {
    let mut pending = pending();
    let info = |pid| Info { code: signal::SI_USER, errno: 0, pid, status: 0 };
    assert!(pending.add(SIGTERM, info(1)));
    assert!(!pending.add(SIGTERM, info(2)), "a standard signal pending twice is one");
    assert!(pending.add(SIGSEGV, info(3)));
    assert!(pending.add(SIGRTMIN, info(4)));
    assert!(pending.add(SIGRTMIN, info(5)));
    assert!(pending.add(SIGINT, info(6)));
    // SIGSEGV is a fault's: first, although SIGINT has a lower number.
    assert_eq!(pending.take(u64::MAX), Some((SIGSEGV, info(3))));
    assert_eq!(pending.take(u64::MAX), Some((SIGINT, info(6))));
    // A blocked one stays; each real-time signal keeps what it carries.
    assert_eq!(pending.take(!signal::bit(SIGTERM)), Some((SIGRTMIN, info(4))));
    assert_eq!(pending.take(!signal::bit(SIGTERM)), Some((SIGRTMIN, info(5))));
    assert_eq!(pending.take(!signal::bit(SIGTERM)), None);
    assert_eq!(pending.take(u64::MAX), Some((SIGTERM, info(1))));
    assert_eq!(pending.set(), 0);
    // SIGCONT drops pending stops; a stop drops a pending SIGCONT.
    assert_eq!(signal::on_send(SIGCONT).discard, signal::STOPPING);
    assert!(signal::on_send(SIGKILL).resume);
    assert_eq!(signal::on_send(SIGTSTP).discard, signal::bit(SIGCONT));
    pending.add(SIGRTMIN + 1, info(7));
    pending.discard(signal::REAL_TIME);
    assert_eq!(pending.take(u64::MAX), None);
}

#[test]
fn real_time_signals_go_lowest_first_and_queue_up_to_the_limit() {
    let mut pending = pending();
    let info = |status| Info { code: signal::SI_QUEUE, errno: 0, pid: 1, status };
    assert!(pending.add(SIGRTMIN + 2, info(1)));
    assert!(pending.add(SIGRTMIN, info(2)));
    assert!(pending.add(SIGRTMIN + 2, info(3)));
    assert_eq!(pending.take(u64::MAX), Some((SIGRTMIN, info(2))));
    assert_eq!(pending.take(u64::MAX), Some((SIGRTMIN + 2, info(1))));
    assert_eq!(pending.set(), signal::bit(SIGRTMIN + 2));
    // Discarding one real-time signal leaves the others queued.
    assert!(pending.add(SIGRTMIN + 1, info(4)));
    pending.discard(signal::bit(SIGRTMIN + 2));
    assert_eq!(pending.take(u64::MAX), Some((SIGRTMIN + 1, info(4))));
    assert_eq!(pending.set(), 0);
    for n in 0..signal::QUEUE_MAX as u64 {
        assert!(pending.add(SIGRTMIN + (n % 3) as u32, info(n)));
    }
    assert!(!pending.add(SIGRTMIN, info(0)), "the queue is full");
    assert!(pending.holds(SIGRTMIN + 1, &info(4)));
    assert!(!pending.holds(SIGRTMIN + 1, &info(5)), "that one is SIGRTMIN + 2's");
    assert!(pending.remove(SIGRTMIN + 1, &info(4)), "a timer's signal goes with the timer");
    assert!(!pending.holds(SIGRTMIN + 1, &info(4)));
    assert!(!pending.remove(SIGRTMIN + 1, &info(4)), "it was queued once");
    assert!(pending.add(SIGRTMIN, info(0)), "there is room for one again");
    assert!(pending.add(SIGUSR1, info(0)), "standard signals do not queue");
    assert!(!pending.remove(SIGUSR1, &info(1)), "another sender's SIGUSR1 stays");
    assert_eq!(pending.set() & signal::bit(SIGUSR1), signal::bit(SIGUSR1));
    let mut taken = 0;
    while pending.take(u64::MAX).is_some() {
        taken += 1;
    }
    assert_eq!(taken, signal::QUEUE_MAX + 1);
}

/// A block as a C library writes it.
fn block(args: &[&str], env: &[&str], extra: &[&str]) -> Vec<u8> {
    let mut bytes = vec![0u8; ExecHeader::SIZE];
    let mut put = |at: usize, value: u32| bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
    put(ExecHeader::OFFSET_VERSION, EXEC_VERSION);
    put(ExecHeader::OFFSET_ARGS, args.len() as u32);
    put(ExecHeader::OFFSET_ENV, env.len() as u32);
    put(ExecHeader::OFFSET_EXTRA, extra.len() as u32);
    for s in args.iter().chain(env).chain(extra) {
        bytes.extend_from_slice(s.as_bytes());
        bytes.push(0);
    }
    bytes
}

#[test]
fn exec_blocks_are_checked() {
    let bytes = block(&["/bin/echo", "hi", ""], &["PATH=/system/bin"], &["/home"]);
    let parsed = Block::read(&bytes).unwrap();
    assert_eq!(parsed.args().collect::<Vec<_>>(), [&b"/bin/echo"[..], b"hi", b""]);
    assert_eq!(parsed.env().collect::<Vec<_>>(), [&b"PATH=/system/bin"[..]]);
    assert_eq!(parsed.extra().collect::<Vec<_>>(), [&b"/home"[..]]);
    assert_eq!((parsed.arg_count(), parsed.env_count()), (3, 1));
    // Too few strings, junk after the last NUL, a wrong version.
    let mut short = bytes.clone();
    short[ExecHeader::OFFSET_ARGS] = 4;
    assert_eq!(Block::read(&short).err(), Some(BlockError::Malformed));
    let mut junk = bytes.clone();
    junk.push(b'x');
    assert_eq!(Block::read(&junk).err(), Some(BlockError::Malformed));
    let mut old = bytes.clone();
    old[ExecHeader::OFFSET_VERSION] = 9;
    assert_eq!(Block::read(&old).err(), Some(BlockError::Malformed));
    assert_eq!(Block::read(&bytes[..4]).err(), Some(BlockError::Malformed));
    let empty = block(&[], &[], &[]);
    assert_eq!(Block::read(&empty).unwrap().args().count(), 0);
}

#[test]
fn shebang_lines_are_read_as_linux_reads_them() {
    assert_eq!(
        shebang(b"#!/system/bin/nukesh\necho hi\n"),
        Some(Shebang { interpreter: b"/system/bin/nukesh", argument: None })
    );
    assert_eq!(
        shebang(b"#! /usr/bin/env  lua  -i \n"),
        Some(Shebang { interpreter: b"/usr/bin/env", argument: Some(b"lua  -i") })
    );
    assert_eq!(shebang(b"#!\n"), None);
    assert_eq!(shebang(b"\x7fELF"), None);
    assert_eq!(shebang(b"#!/bin/sh"), Some(Shebang { interpreter: b"/bin/sh", argument: None }));
}

/// The words of a stack from its stack pointer.
fn word(out: &[u8], base: u64, address: u64) -> u64 {
    let i = (address - base) as usize;
    u64::from_le_bytes(out[i..i + 8].try_into().unwrap())
}

fn string(out: &[u8], base: u64, address: u64) -> Vec<u8> {
    let i = (address - base) as usize;
    out[i..].iter().take_while(|&&b| b != 0).copied().collect()
}

#[test]
fn the_initial_stack_is_the_psabi_one() {
    let args: [&[u8]; 2] = [b"/system/bin/lua", b"-v"];
    let env: [&[u8]; 1] = [b"HOME=/home/duke"];
    let stack = Stack {
        args: &args,
        env: &env,
        aux: &[(aux::AT_PAGESZ, 4096), (aux::AT_ENTRY, 0x401000)],
        execfn: b"/system/bin/lua",
        cwd: Some(b"/home/duke"),
        platform: b"x86_64",
        random: [7; 16],
    };
    let top = 0x7fff_0000u64;
    let mut out = vec![0xaa; stack.size() as usize];
    let base = top - out.len() as u64;
    let sp = stack.write(top, &mut out).unwrap();
    assert_eq!(sp % 16, 0);
    assert!(sp >= base);
    assert_eq!(word(&out, base, sp), 2);
    assert_eq!(string(&out, base, word(&out, base, sp + 8)), b"/system/bin/lua");
    assert_eq!(string(&out, base, word(&out, base, sp + 16)), b"-v");
    assert_eq!(word(&out, base, sp + 24), 0);
    assert_eq!(string(&out, base, word(&out, base, sp + 32)), b"HOME=/home/duke");
    assert_eq!(word(&out, base, sp + 40), 0);
    // The auxiliary vector, in pairs, to AT_NULL.
    let mut at = sp + 48;
    let mut seen = Vec::new();
    loop {
        let (key, value) = (word(&out, base, at), word(&out, base, at + 8));
        at += 16;
        if key == aux::AT_NULL {
            break;
        }
        seen.push((key, value));
    }
    let get = |key| seen.iter().find(|(k, _)| *k == key).map(|(_, v)| *v).unwrap();
    assert_eq!(get(aux::AT_PAGESZ), 4096);
    assert_eq!(string(&out, base, get(aux::AT_PLATFORM)), b"x86_64");
    assert_eq!(string(&out, base, get(aux::AT_RADIX_CWD)), b"/home/duke");
    assert_eq!(string(&out, base, get(aux::AT_EXECFN)), b"/system/bin/lua");
    let random = (get(aux::AT_RANDOM) - base) as usize;
    assert_eq!(out[random..random + 16], [7; 16]);
    // The arguments' strings lie below the environment's, in order.
    assert!(word(&out, base, sp + 8) < word(&out, base, sp + 16));
    assert!(word(&out, base, sp + 16) < word(&out, base, sp + 32));
    // The top 8 bytes are zero.
    assert_eq!(out[out.len() - 8..], [0; 8]);
    // Too little room is refused.
    let mut small = vec![0; 64];
    assert!(stack.write(top, &mut small).is_err());
}

#[test]
fn wait_reports_ends_stops_and_continues() {
    let mut tree = T::new(Global);
    tree.insert_root(1, 1).unwrap();
    tree.insert_child(2, 1, 2).unwrap();
    tree.insert_child(3, 1, 3).unwrap();
    let all = WaitFor { stopped: true, continued: true, ..WaitFor::default() };
    assert_eq!(tree.wait(1, Which::Any, all), Found::NotYet);
    assert_eq!(tree.wait(2, Which::Any, all), Found::NoChild);
    assert_eq!(tree.wait(1, Which::Pid(9), all), Found::NoChild);
    // A stop, reported once.
    assert_eq!(tree.stopped(3, SIGTSTP), Some(1));
    assert_eq!(tree.wait(1, Which::Any, WaitFor::default()), Found::NotYet);
    let stopped =
        Found::Child { pid: 3, status: status::stopped(SIGTSTP), times: (0, 0), reaped: None };
    assert_eq!(tree.wait(1, Which::Any, all), stopped);
    assert_eq!(tree.wait(1, Which::Any, all), Found::NotYet);
    assert_eq!(tree.continued(3), Some(1));
    assert_eq!(tree.continued(3), None);
    let continued = Found::Child { pid: 3, status: status::CONTINUED, times: (0, 0), reaped: None };
    assert_eq!(tree.wait(1, Which::Pid(3), all), continued);
    // An end, seen with no_reap, then reaped.
    let mut gone = Vec::new();
    let exit = tree.exit(2, Ended::Exited(7), (5, 6), Reaps::default(), &mut |h| gone.push(h));
    assert_eq!(exit.parent, Some(1));
    let peek = WaitFor { no_reap: true, ..WaitFor::default() };
    let ended = |reaped| Found::Child { pid: 2, status: status::exited(7), times: (5, 6), reaped };
    assert_eq!(tree.wait(1, Which::Any, peek), ended(None));
    assert_eq!(tree.wait(1, Which::Group(1), WaitFor::default()), ended(Some(2)));
    assert!(tree.get(2).is_none());
    tree.check().unwrap();
}

#[test]
fn wait_can_leave_out_ends() {
    // waitid(WSTOPPED) without WEXITED: an ended child is passed over,
    // left to be reaped later, and a stop of another is reported.
    let mut tree = T::new(Global);
    tree.insert_root(1, 1).unwrap();
    tree.insert_child(2, 1, 2).unwrap();
    tree.insert_child(3, 1, 3).unwrap();
    let mut gone = Vec::new();
    tree.exit(2, Ended::Exited(7), (5, 6), Reaps::default(), &mut |h| gone.push(h));
    let stops = WaitFor { stopped: true, skip_ended: true, ..WaitFor::default() };
    assert_eq!(tree.wait(1, Which::Any, stops), Found::NotYet);
    assert_eq!(tree.wait(1, Which::Pid(2), stops), Found::NotYet);
    tree.stopped(3, SIGTSTP);
    let stopped =
        Found::Child { pid: 3, status: status::stopped(SIGTSTP), times: (0, 0), reaped: None };
    assert_eq!(tree.wait(1, Which::Any, stops), stopped);
    let ended = Found::Child { pid: 2, status: status::exited(7), times: (5, 6), reaped: Some(2) };
    assert_eq!(tree.wait(1, Which::Any, WaitFor::default()), ended);
    tree.check().unwrap();
}

#[test]
fn orphans_lose_their_parent_and_ended_ones_go() {
    let mut tree = T::new(Global);
    tree.insert_root(1, 1).unwrap();
    tree.insert_child(2, 1, 2).unwrap();
    tree.insert_child(3, 2, 3).unwrap();
    tree.insert_child(4, 2, 4).unwrap();
    let mut gone = Vec::new();
    tree.exit(4, Ended::Killed(SIGTERM), (0, 0), Reaps::default(), &mut |h| gone.push(h));
    assert!(gone.is_empty(), "4 waits for 2 to reap it");
    tree.exit(2, Ended::Exited(0), (0, 0), Reaps::default(), &mut |h| gone.push(h));
    assert_eq!(gone, [4]);
    assert_eq!(tree.get(3).unwrap().parent, 0);
    // 3 has no parent now, so it goes as soon as it ends.
    tree.exit(3, Ended::Exited(1), (0, 0), Reaps::default(), &mut |h| gone.push(h));
    assert_eq!(gone, [4, 3]);
    // A parent that ignores SIGCHLD has its children reaped at once.
    tree.exit(2, Ended::Exited(0), (0, 0), Reaps::default(), &mut |h| gone.push(h));
    tree.insert_child(5, 1, 5).unwrap();
    tree.exit(5, Ended::Exited(0), (0, 0), REAPS, &mut |h| gone.push(h));
    assert!(tree.get(5).is_none());
    tree.check().unwrap();
}

/// A parent that does not wait for its children.
const REAPS: Reaps = Reaps { parent: true, reaper: false };

#[test]
fn a_subreaper_adopts_its_descendants_orphans() {
    // 1 is a subreaper; 2 is its child, 3 and 4 are 2's. When 2 ends, 3
    // and 4 go to 1, and 1 is told of 4, which had ended.
    let mut tree = T::new(Global);
    tree.insert_root(1, 1).unwrap();
    tree.insert_child(2, 1, 2).unwrap();
    tree.insert_child(3, 2, 3).unwrap();
    tree.insert_child(4, 2, 4).unwrap();
    tree.insert_child(5, 3, 5).unwrap();
    tree.get_mut(1).unwrap().subreaper = true;
    assert_eq!(tree.reaper(3), 1, "the nearest subreaper above");
    assert_eq!(tree.reaper(1), 1, "itself");
    let mut gone = Vec::new();
    tree.exit(4, Ended::Exited(3), (0, 0), Reaps::default(), &mut |h| gone.push(h));
    let exit = tree.exit(2, Ended::Exited(0), (0, 0), Reaps::default(), &mut |h| gone.push(h));
    assert_eq!(exit, Exit { parent: Some(1), adopted: Some((1, 4, Ended::Exited(3))) });
    assert!(gone.is_empty());
    assert_eq!(tree.get(3).unwrap().parent, 1);
    let ended = Found::Child { pid: 4, status: status::exited(3), times: (0, 0), reaped: Some(4) };
    assert_eq!(tree.wait(1, Which::Pid(4), WaitFor::default()), ended);
    // A grandchild goes to the subreaper too, past its ended parent.
    tree.exit(3, Ended::Exited(0), (0, 0), Reaps::default(), &mut |h| gone.push(h));
    assert_eq!(tree.get(5).unwrap().parent, 1);
    tree.check().unwrap();
    // A subreaper that ignores SIGCHLD reaps the ended ones it adopts.
    tree.insert_child(6, 5, 6).unwrap();
    tree.insert_child(7, 6, 7).unwrap();
    tree.exit(7, Ended::Exited(0), (0, 0), Reaps::default(), &mut |h| gone.push(h));
    let reaps = Reaps { parent: false, reaper: true };
    let exit = tree.exit(6, Ended::Exited(0), (0, 0), reaps, &mut |h| gone.push(h));
    assert_eq!(exit, Exit { parent: Some(5), adopted: None });
    assert_eq!(gone, [7]);
    tree.check().unwrap();
    // With the subreaper gone too, orphans lose their parent.
    tree.get_mut(1).unwrap().subreaper = false;
    tree.exit(5, Ended::Exited(0), (0, 0), Reaps::default(), &mut |h| gone.push(h));
    assert_eq!(tree.reaper(1), 0);
    tree.check().unwrap();
}

#[test]
fn groups_and_sessions_follow_posix() {
    let mut tree = T::new(Global);
    tree.insert_root(10, 10).unwrap();
    tree.insert_child(11, 10, 11).unwrap();
    tree.insert_child(12, 10, 12).unwrap();
    // A shell puts a job in a group of its own, from either side.
    tree.set_group(10, 11, 11).unwrap();
    tree.set_group(12, 0, 11).unwrap();
    assert_eq!(tree.get(12).unwrap().group, 11);
    // Not a group of another session, not a session leader, not a child
    // that has run exec, not someone else's child.
    assert_eq!(tree.set_group(10, 10, 11), Err(TreeError::NotPermitted));
    assert_eq!(tree.set_group(10, 10, 0), Err(TreeError::NotPermitted));
    tree.get_mut(12).unwrap().execed = true;
    assert_eq!(tree.set_group(10, 12, 12), Err(TreeError::Execed));
    assert_eq!(tree.set_group(11, 12, 11), Err(TreeError::NoSuchProcess));
    // A group leader cannot start a session; a member can.
    assert_eq!(tree.new_session(11), Err(TreeError::NotPermitted));
    assert_eq!(tree.new_session(12), Ok(12));
    assert_eq!(tree.get(12).unwrap().group, 12);
    assert_eq!(tree.set_group(10, 12, 11), Err(TreeError::NotPermitted));
    // Signals: to itself, its descendants, and its user's processes in its
    // session or its resource domain; tracing: only its descendants.
    let same = |_: &u64, _: &u64| true;
    let apart = |_: &u64, _: &u64| false;
    assert!(tree.may_signal(10, 12, same, apart) && tree.may_signal(11, 10, same, apart));
    // 12 has a session of its own now: its sibling reaches it only when
    // they share a domain, as a service's processes do.
    assert!(!tree.may_signal(12, 11, same, apart) && !tree.may_signal(11, 12, same, apart));
    assert!(tree.may_signal(12, 11, same, same) && tree.may_signal(11, 12, same, same));
    let user = |pid: &u64| if *pid == 11 { 1000 } else { 0 };
    let users = |a: &u64, b: &u64| user(a) == user(b);
    assert!(!tree.may_signal(11, 10, users, apart) && tree.may_signal(10, 11, users, apart));
    // A shared domain does not reach another user's processes.
    assert!(!tree.may_signal(11, 10, users, same) && !tree.may_signal(12, 11, users, same));
    assert!(tree.may_trace(10, 12) && !tree.may_trace(11, 10) && !tree.may_trace(11, 12));
    let mut named = Vec::new();
    assert_eq!(tree.targets(10, 10, -11, &mut |m| named.push(m.pid)), 1);
    assert_eq!(named, [11]);
    named.clear();
    tree.targets(10, 10, -1, &mut |m| named.push(m.pid));
    named.sort_unstable();
    assert_eq!(named, [11, 12]);
    // Group 11's only member's parent is in another group of its session:
    // not orphaned until that parent goes.
    assert!(!tree.orphaned(11));
    tree.stopped(11, SIGSTOP);
    assert!(tree.has_stopped(11));
    tree.exit(10, Ended::Exited(0), (0, 0), Reaps::default(), &mut |_| {});
    assert!(tree.orphaned(11));
    tree.check().unwrap();
}

#[test]
fn kill_of_every_process_leaves_init_out() {
    let mut tree = T::new(Global);
    tree.insert_root(1, 1).unwrap();
    tree.insert_child(2, 1, 2).unwrap();
    tree.insert_child(3, 1, 3).unwrap();
    tree.new_session(3).unwrap();
    let mut named = Vec::new();
    tree.targets(2, 1, -1, &mut |m| named.push(m.pid));
    assert_eq!(named, [3]);
    // Process group 1 by its number: its members alone.
    named.clear();
    tree.targets(0, 1, 0, &mut |m| named.push(m.pid));
    named.sort_unstable();
    assert_eq!(named, [1, 2]);
}

/// An operation on the tree, with members picked by position.
#[derive(Clone, Debug)]
enum Op {
    Fork(usize),
    Exit(usize, bool, bool),
    Wait(usize),
    Subreaper(usize),
    SetGroup(usize, usize, usize),
    Session(usize),
    Stop(usize),
    Continue(usize),
}

fn op() -> impl Strategy<Value = Op> {
    let n = 0usize..16;
    prop_oneof![
        3 => n.clone().prop_map(Op::Fork),
        2 => (n.clone(), any::<bool>(), any::<bool>()).prop_map(|(a, b, c)| Op::Exit(a, b, c)),
        1 => n.clone().prop_map(Op::Subreaper),
        2 => n.clone().prop_map(Op::Wait),
        2 => (n.clone(), n.clone(), n.clone()).prop_map(|(a, b, c)| Op::SetGroup(a, b, c)),
        1 => n.clone().prop_map(Op::Session),
        1 => n.clone().prop_map(Op::Stop),
        1 => n.prop_map(Op::Continue),
    ]
}

proptest! {
    #[test]
    fn the_tree_keeps_its_invariants(ops in proptest::collection::vec(op(), 1..80)) {
        let mut tree = T::new(Global);
        tree.insert_root(1, 1).unwrap();
        let mut next = 2;
        let pick = |tree: &T, i: usize| {
            let pids: Vec<u64> = tree.members().map(|m| m.pid).collect();
            pids[i % pids.len()]
        };
        for op in ops {
            if tree.is_empty() {
                break;
            }
            match op {
                Op::Fork(i) => {
                    let parent = pick(&tree, i);
                    let _ = tree.insert_child(next, parent, next);
                    next += 1;
                }
                Op::Exit(i, parent, reaper) => {
                    let pid = pick(&tree, i);
                    if !matches!(tree.get(pid).unwrap().life, Life::Zombie { .. }) {
                        let reaps = Reaps { parent, reaper };
                        tree.exit(pid, Ended::Exited(0), (0, 0), reaps, &mut |_| {});
                    }
                }
                Op::Subreaper(i) => {
                    let pid = pick(&tree, i);
                    let member = tree.get_mut(pid).unwrap();
                    member.subreaper = !member.subreaper;
                }
                Op::Wait(i) => {
                    let pid = pick(&tree, i);
                    let _ = tree.wait(pid, Which::Any, WaitFor { stopped: true, continued: true, ..WaitFor::default() });
                }
                Op::SetGroup(a, b, c) => {
                    let (caller, pid, group) = (pick(&tree, a), pick(&tree, b), pick(&tree, c));
                    let _ = tree.set_group(caller, pid, group);
                }
                Op::Session(i) => {
                    let pid = pick(&tree, i);
                    if !matches!(tree.get(pid).unwrap().life, Life::Zombie { .. }) {
                        let _ = tree.new_session(pid);
                    }
                }
                Op::Stop(i) => {
                    let pid = pick(&tree, i);
                    tree.stopped(pid, SIGSTOP);
                }
                Op::Continue(i) => {
                    let pid = pick(&tree, i);
                    tree.continued(pid);
                }
            }
            prop_assert_eq!(tree.check(), Ok(()));
        }
    }
}

// System V IPC (ADR 0490).

fn caller(user: u32, group: u32, groups: &[u32]) -> Caller<'_> {
    Caller { user, group, groups, bypass: false }
}

#[test]
fn ipc_permissions_are_linux_ones() {
    let maker = caller(1000, 100, &[]);
    let mut perm = Perm::new(42, &maker, 0o640 | 0o1000);
    assert_eq!(perm.mode, 0o640);
    assert!(perm.permits(&maker, ipc::want::READ | ipc::want::WRITE));
    // The group reads, the others nothing.
    assert!(perm.permits(&caller(1001, 100, &[]), ipc::want::READ));
    assert!(!perm.permits(&caller(1001, 100, &[]), ipc::want::WRITE));
    assert!(perm.permits(&caller(1001, 7, &[100]), ipc::want::READ));
    assert!(!perm.permits(&caller(1001, 7, &[]), ipc::want::READ));
    assert!(perm.permits(&Caller { bypass: true, ..caller(1001, 7, &[]) }, ipc::want::WRITE));
    // A new owner keeps the maker's rights, as Linux's cuid does.
    perm.set(2000, 200, 0o600);
    assert!(perm.permits(&maker, ipc::want::WRITE));
    assert!(perm.controls(&maker));
    assert!(perm.controls(&caller(2000, 1, &[])));
    assert!(!perm.controls(&caller(1001, 100, &[])));
    // A get asking nothing is never refused; 0666 asks to read and write.
    assert_eq!(ipc::asked(0), 0);
    assert_eq!(ipc::asked(0o666), 0o6);
    assert_eq!(ipc::asked(0o400), 0o4);
}

#[test]
fn ipc_gets_make_or_find_as_linux() {
    assert_eq!(ipc::decide(Some(5), ipc::PRIVATE, false, false), Ok(None));
    assert_eq!(ipc::decide(None, 9, false, false), Err(GetError::NotFound));
    assert_eq!(ipc::decide(None, 9, true, true), Ok(None));
    assert_eq!(ipc::decide(Some(5), 9, true, true), Err(GetError::Exists));
    assert_eq!(ipc::decide(Some(5), 9, true, false), Ok(Some(5)));
    assert_eq!(ipc::decide(Some(5), 9, false, false), Ok(Some(5)));
}

#[test]
fn ipc_numbers_name_one_object_until_they_come_round() {
    let mut table: Table<&str, Global> = Table::new(2, Global);
    let a = table.insert(7, "a").unwrap();
    let b = table.insert(ipc::PRIVATE, "b").unwrap();
    assert_eq!(table.insert(8, "c"), Err((TableError::Full, "c")));
    assert_eq!(table.find(7), Some(a));
    // A private key finds nothing, even an object made with it.
    assert_eq!(table.find(ipc::PRIVATE), None);
    assert_eq!(table.get(b), Some(&"b"));
    assert_eq!(table.remove(a), Some("a"));
    assert_eq!(table.get(a), None);
    // The place is used again, under another number.
    let c = table.insert(7, "c").unwrap();
    assert_ne!(c, a);
    assert_eq!(c % ipc::IPCMNI, a % ipc::IPCMNI);
    assert_eq!(table.get(a), None);
    table.forget_key(c);
    assert_eq!(table.find(7), None);
    assert_eq!(table.key(c), Some(ipc::PRIVATE));
    assert_eq!(table.iter().map(|(id, _, v)| (id, *v)).collect::<Vec<_>>(), [(c, "c"), (b, "b")]);
    assert_eq!(table.len(), 2);
}

fn sem(number: u16, op: i16, flags: u16) -> Operation {
    Operation { number, op, flags }
}

#[test]
fn semaphore_operations_are_all_or_none() {
    let mut values = [1u16, 0];
    let mut adjust = [0i32; 2];
    // Take 1 from the first and wait for the second to grow: neither.
    let ops = [sem(0, -1, 0), sem(1, -1, 0)];
    assert_eq!(
        ipc::operate(&mut values, &mut adjust, &ops),
        Outcome::Wait { number: 1, zero: false }
    );
    assert_eq!(values, [1, 0]);
    let ops = [sem(0, -1, UNDO), sem(1, -1, NOWAIT)];
    assert_eq!(ipc::operate(&mut values, &mut adjust, &ops), Outcome::WouldBlock);
    assert_eq!((values, adjust), ([1, 0], [0, 0]));
    // Waiting for zero.
    assert_eq!(
        ipc::operate(&mut values, &mut adjust, &[sem(0, 0, 0)]),
        Outcome::Wait { number: 0, zero: true }
    );
    assert_eq!(ipc::operate(&mut values, &mut adjust, &[sem(1, 0, 0)]), Outcome::Done);
    // In order: one operation can let the next go on.
    let ops = [sem(1, 2, UNDO), sem(1, -1, UNDO), sem(0, -1, 0)];
    assert_eq!(ipc::operate(&mut values, &mut adjust, &ops), Outcome::Done);
    assert_eq!((values, adjust), ([0, 1], [0, -1]));
    // Past the highest value.
    values[1] = SEMVMX as u16;
    assert_eq!(ipc::operate(&mut values, &mut adjust, &[sem(1, 1, 0)]), Outcome::OutOfRange);
    assert_eq!(values[1], SEMVMX as u16);
    // The undo of an ending process, kept in range.
    let mut values = [3u16, 1];
    ipc::undo(&mut values, &[-5, 2]);
    assert_eq!(values, [0, 3]);
}

#[test]
fn semaphore_operation_lists_are_checked_first() {
    assert_eq!(ipc::check(&[], 1), Err(OpError::Empty));
    assert_eq!(ipc::check(&[sem(1, 1, 0)], 1), Err(OpError::BadNumber));
    let many = vec![sem(0, 1, 0); ipc::SEMOPM + 1];
    assert_eq!(ipc::check(&many, 1), Err(OpError::TooMany));
    let checked = ipc::check(&[sem(0, 0, 0)], 1).unwrap();
    assert!(!checked.alters && !checked.undoes);
    let checked = ipc::check(&[sem(0, 0, 0), sem(0, -1, UNDO)], 1).unwrap();
    assert!(checked.alters && checked.undoes);
    // struct sembuf { unsigned short sem_num; short sem_op; short sem_flg; }
    assert_eq!(Operation::from_bytes([1, 0, 0xff, 0xff, 0, 0x10]), sem(1, -1, UNDO));
    assert!(ipc::valid_value(0) && ipc::valid_value(32767));
    assert!(!ipc::valid_value(-1) && !ipc::valid_value(32768));
}

fn semaphore_op() -> impl Strategy<Value = Operation> {
    (0u16..3, -3i16..4, prop_oneof![Just(0u16), Just(UNDO), Just(NOWAIT), Just(UNDO | NOWAIT)])
        .prop_map(|(number, op, flags)| Operation { number, op, flags })
}

proptest! {
    #[test]
    fn semaphores_stay_in_range_and_undo_what_they_did(
        calls in proptest::collection::vec(proptest::collection::vec(semaphore_op(), 1..5), 1..40),
    ) {
        let mut values = [0u16; 3];
        let mut adjust = [0i32; 3];
        for ops in &calls {
            let (before, had) = (values, adjust);
            match ipc::operate(&mut values, &mut adjust, ops) {
                Outcome::Done => {
                    // Each value moved by what its operations added, and
                    // each adjustment the other way for those undone.
                    for n in 0..3u16 {
                        let added: i32 = ops.iter().filter(|o| o.number == n).map(|o| i32::from(o.op)).sum();
                        let undone: i32 = ops.iter().filter(|o| o.number == n && o.flags & UNDO != 0).map(|o| i32::from(o.op)).sum();
                        prop_assert_eq!(i32::from(values[n as usize]), i32::from(before[n as usize]) + added);
                        prop_assert_eq!(adjust[n as usize], had[n as usize] - undone);
                    }
                }
                _ => prop_assert_eq!((values, adjust), (before, had)),
            }
            prop_assert!(values.iter().all(|&v| i32::from(v) <= SEMVMX));
        }
        // Undoing every adjustment leaves what operations without UNDO
        // did, which stayed in range.
        let mut undone = values;
        ipc::undo(&mut undone, &adjust);
        for n in 0..3 {
            prop_assert_eq!(i32::from(undone[n]), (i32::from(values[n]) + adjust[n]).clamp(0, SEMVMX));
        }
    }
}

// Event queues' watches (ADR 0353).

/// The descriptor of the read and write watches these tests make.
const DESCRIPTOR: u64 = 7;

/// The number of the user event these tests make, as a program picks one.
const EVENT: u64 = 0xee1e_b9f4;

fn all_flags(each: &[QueueFlags]) -> QueueFlags {
    each.iter().fold(QueueFlags::NONE, |all, flag| all.union(*flag))
}

fn all_events(each: &[PollEvents]) -> PollEvents {
    each.iter().fold(PollEvents::NONE, |all, event| all.union(*event))
}

/// An event's 16 bits of flags.
fn bits(of: QueueFlags) -> u16 {
    u16::try_from(of.bits()).unwrap()
}

/// A change as a C program's `EV_SET` makes it, with the flags `with`.
fn change(ident: u64, filter: QueueFilter, with: QueueFlags) -> QueueEvent {
    QueueEvent { ident, filter: filter.raw(), flags: bits(with), ..QueueEvent::default() }
}

/// A change of the user event, with its `fflags` and `data`.
fn user_change(with: QueueFlags, fflags: u32, data: i64) -> QueueEvent {
    QueueEvent { fflags, data, ..change(EVENT, QueueFilter::User, with) }
}

/// The watch that adding the descriptor with `with` makes.
fn watch_of(filter: QueueFilter, with: QueueFlags) -> State {
    State::added(&change(DESCRIPTOR, filter, with), filter)
}

/// The watch that adding the user event with `with`, `fflags` and `data` makes.
fn user_watch(with: QueueFlags, fflags: u32, data: i64) -> State {
    State::added(&user_change(with, fflags, data), QueueFilter::User)
}

/// What a look at the descriptor's watch reports when the descriptor is ready as `ready` says.
fn look_at(
    state: &mut State,
    filter: QueueFilter,
    ready: PollEvents,
) -> Option<(QueueEvent, Then)> {
    state.report(DESCRIPTOR, filter, ready)
}

/// What a look at the user event reports, which no descriptor's readiness changes.
fn look_at_event(state: &mut State) -> Option<(QueueEvent, Then)> {
    state.report(EVENT, QueueFilter::User, PollEvents::NONE)
}

/// The flags a triggered user event reports after a change of `fflags`.
fn flags_after_change(state: &mut State, fflags: u32) -> u32 {
    state.change(&user_change(QueueFlags::NONE, fflags, 0), QueueFilter::User, false);
    look_at_event(state).unwrap().0.fflags
}

#[test]
fn the_flags_filters_and_notes_have_bsds_numbers() {
    let flags = [
        (QueueFlags::ADD, 0x1),
        (QueueFlags::DELETE, 0x2),
        (QueueFlags::ENABLE, 0x4),
        (QueueFlags::DISABLE, 0x8),
        (QueueFlags::ONESHOT, 0x10),
        (QueueFlags::CLEAR, 0x20),
        (QueueFlags::RECEIPT, 0x40),
        (QueueFlags::DISPATCH, 0x80),
        (QueueFlags::ERROR, 0x4000),
        (QueueFlags::EOF, 0x8000),
    ];
    for (flag, number) in flags {
        assert_eq!(flag.bits(), number);
    }
    // EVFILT_READ, EVFILT_WRITE and EVFILT_USER, as the C library's 16-bit `short`.
    assert_eq!(QueueFilter::Read.raw(), (-1i16) as u16);
    assert_eq!(QueueFilter::Write.raw(), (-2i16) as u16);
    assert_eq!(QueueFilter::User.raw(), (-11i16) as u16);
    // NOTE_TRIGGER, NOTE_FFLAGSMASK, NOTE_FFAND, NOTE_FFOR, NOTE_FFCOPY and NOTE_FFCTRLMASK.
    assert_eq!(USER_TRIGGER, 0x0100_0000);
    assert_eq!(USER_FLAGS, 0x00ff_ffff);
    assert_eq!(USER_AND, 0x4000_0000);
    assert_eq!(USER_OR, 0x8000_0000);
    assert_eq!(USER_COPY, 0xc000_0000);
    assert_eq!(USER_CONTROL, USER_AND | USER_OR);
}

#[test]
fn a_kevent_is_32_bytes_with_each_field_at_its_offset() {
    // struct kevent { uintptr_t ident; short filter; unsigned short flags;
    //                 unsigned int fflags; int64_t data; void *udata; }
    assert_eq!(QueueEvent::SIZE, 32);
    let offsets = [
        QueueEvent::OFFSET_IDENT,
        QueueEvent::OFFSET_FILTER,
        QueueEvent::OFFSET_FLAGS,
        QueueEvent::OFFSET_FFLAGS,
        QueueEvent::OFFSET_DATA,
        QueueEvent::OFFSET_UDATA,
    ];
    assert_eq!(offsets, [0, 8, 10, 12, 16, 24]);
    let event = QueueEvent {
        ident: 0x0807_0605_0403_0201,
        filter: 0xfff5,
        flags: 0x4021,
        fflags: 0x0b0a_0908,
        data: -2,
        udata: 0x1f1e_1d1c_1b1a_1918,
    };
    let bytes = watch::write(&event);
    assert_eq!(bytes[0..8], [1, 2, 3, 4, 5, 6, 7, 8]);
    assert_eq!(bytes[8..10], [0xf5, 0xff]);
    assert_eq!(bytes[10..12], [0x21, 0x40]);
    assert_eq!(bytes[12..16], [8, 9, 10, 11]);
    assert_eq!(bytes[16..24], [0xfe, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff]);
    assert_eq!(bytes[24..32], [0x18, 0x19, 0x1a, 0x1b, 0x1c, 0x1d, 0x1e, 0x1f]);
}

#[test]
fn a_change_is_read_from_the_bytes_a_c_program_leaves() {
    let mut bytes = [0u8; QueueEvent::SIZE];
    bytes[0..8].copy_from_slice(&9u64.to_le_bytes());
    bytes[8..10].copy_from_slice(&(-1i16).to_le_bytes());
    bytes[10..12].copy_from_slice(&0x21u16.to_le_bytes());
    bytes[12..16].copy_from_slice(&USER_TRIGGER.to_le_bytes());
    bytes[16..24].copy_from_slice(&(-5i64).to_le_bytes());
    bytes[24..32].copy_from_slice(&0xdead_beef_u64.to_le_bytes());
    let expected = QueueEvent {
        ident: 9,
        filter: QueueFilter::Read.raw(),
        flags: 0x21,
        fflags: USER_TRIGGER,
        data: -5,
        udata: 0xdead_beef,
    };
    assert_eq!(watch::read(&bytes), expected);
    assert_eq!(watch::write(&expected), bytes);
}

#[test]
fn an_event_is_read_back_as_it_was_written_whatever_its_values() {
    let largest = QueueEvent {
        ident: u64::MAX,
        filter: u16::MAX,
        flags: u16::MAX,
        fflags: u32::MAX,
        data: i64::MAX,
        udata: u64::MAX,
    };
    for event in [QueueEvent::default(), largest, QueueEvent { data: i64::MIN, ..largest }] {
        assert_eq!(watch::read(&watch::write(&event)), event);
    }
}

#[test]
fn a_change_asks_to_add_delete_or_change_a_watch() {
    let read = QueueFilter::Read;
    let asks = |with| watch::asks(&change(DESCRIPTOR, read, with));
    assert_eq!(asks(QueueFlags::ADD), Ok(Some((read, Asks::Add))));
    assert_eq!(asks(QueueFlags::DELETE), Ok(Some((read, Asks::Delete))));
    assert_eq!(asks(QueueFlags::NONE), Ok(Some((read, Asks::Change))));
    // The other flags say how, not what.
    let others = [
        QueueFlags::ENABLE,
        QueueFlags::DISABLE,
        QueueFlags::ONESHOT,
        QueueFlags::CLEAR,
        QueueFlags::RECEIPT,
        QueueFlags::DISPATCH,
    ];
    for other in others {
        assert_eq!(asks(other), Ok(Some((read, Asks::Change))));
        assert_eq!(asks(other.union(QueueFlags::ADD)), Ok(Some((read, Asks::Add))));
        assert_eq!(asks(other.union(QueueFlags::DELETE)), Ok(Some((read, Asks::Delete))));
    }
}

#[test]
fn a_change_asks_of_the_watch_of_its_filter() {
    for filter in QueueFilter::ALL {
        let asked = watch::asks(&change(DESCRIPTOR, filter, QueueFlags::ADD));
        assert_eq!(asked, Ok(Some((filter, Asks::Add))));
    }
}

#[test]
fn a_change_that_adds_and_deletes_is_invalid() {
    let both = all_flags(&[QueueFlags::ADD, QueueFlags::DELETE]);
    for filter in QueueFilter::ALL {
        assert_eq!(watch::asks(&change(DESCRIPTOR, filter, both)), Err(Invalid));
    }
}

#[test]
fn a_change_of_filter_zero_is_passed_over() {
    let passed = QueueEvent { filter: 0, ..QueueEvent::default() };
    assert_eq!(watch::asks(&passed), Ok(None));
    // Whatever else it says, even what would be invalid.
    let both = all_flags(&[QueueFlags::ADD, QueueFlags::DELETE]);
    for with in [QueueFlags::ADD, QueueFlags::DELETE, both] {
        let passed = QueueEvent { flags: bits(with), ..passed };
        assert_eq!(watch::asks(&passed), Ok(None));
    }
}

#[test]
fn a_change_of_an_unknown_filter_is_invalid() {
    // 0xfff9 is -7 as a C program's `short`; the others lie next to the known filters.
    let add = change(DESCRIPTOR, QueueFilter::Read, QueueFlags::ADD);
    for filter in [1, 3, 0x7fff, 0xfff4, 0xfff6, 0xfff9, 0xfffd] {
        let unknown = QueueEvent { filter, ..add };
        assert_eq!(watch::asks(&unknown), Err(Invalid), "filter {filter:#x}");
    }
}

#[test]
fn flags_outside_the_low_byte_make_no_difference() {
    // A program may leave `error`, `eof` or EV_FLAG1 in a change's flags. FreeBSD clears them,
    // and they change nothing here. `exclusive` and `create` (0x0c00) are the queue's own.
    for low in 0..=0xffu16 {
        for high in [0x0100, 0x2000, 0x4000, 0x8000, 0xf300] {
            for filter in QueueFilter::ALL {
                let none = change(DESCRIPTOR, filter, QueueFlags::NONE);
                let plain = QueueEvent { flags: low, ..none };
                let noisy = QueueEvent { flags: low | high, ..plain };
                assert_eq!(watch::asks(&noisy), watch::asks(&plain));
                assert_eq!(State::added(&noisy, filter), State::added(&plain, filter));
                for adding in [false, true] {
                    for first in [QueueFlags::NONE, QueueFlags::DISABLE] {
                        let mut with_noise = watch_of(filter, first);
                        let mut without = with_noise;
                        with_noise.change(&noisy, filter, adding);
                        without.change(&plain, filter, adding);
                        assert_eq!(with_noise, without);
                    }
                }
            }
        }
    }
}

#[test]
fn an_added_watch_is_enabled_unless_its_change_disables_it() {
    for filter in QueueFilter::ALL {
        assert!(watch_of(filter, QueueFlags::ADD).enabled());
        assert!(watch_of(filter, all_flags(&[QueueFlags::ADD, QueueFlags::ENABLE])).enabled());
        assert!(!watch_of(filter, all_flags(&[QueueFlags::ADD, QueueFlags::DISABLE])).enabled());
    }
}

#[test]
fn a_change_sets_the_udata_the_events_carry() {
    let read = QueueFilter::Read;
    let ready = PollEvents::READABLE;
    let added = QueueEvent { udata: 0x11, ..change(DESCRIPTOR, read, QueueFlags::ADD) };
    let mut state = State::added(&added, read);
    assert_eq!(look_at(&mut state, read, ready).unwrap().0.udata, 0x11);
    let plain = QueueEvent { udata: 0x22, ..change(DESCRIPTOR, read, QueueFlags::NONE) };
    state.change(&plain, read, false);
    assert_eq!(look_at(&mut state, read, ready).unwrap().0.udata, 0x22);
    let again = QueueEvent { udata: 0x33, ..added };
    state.change(&again, read, true);
    assert_eq!(look_at(&mut state, read, ready).unwrap().0.udata, 0x33);
}

#[test]
fn enable_wins_over_disable_in_one_change() {
    let read = QueueFilter::Read;
    let both = change(DESCRIPTOR, read, all_flags(&[QueueFlags::ENABLE, QueueFlags::DISABLE]));
    let adding = all_flags(&[QueueFlags::ADD, QueueFlags::ENABLE, QueueFlags::DISABLE]);
    assert!(watch_of(read, adding).enabled());
    for added in [QueueFlags::ADD, all_flags(&[QueueFlags::ADD, QueueFlags::DISABLE])] {
        let mut state = watch_of(read, added);
        state.change(&both, read, false);
        assert!(state.enabled());
        let mut state = watch_of(read, added);
        state.change(&change(DESCRIPTOR, read, adding), read, true);
        assert!(state.enabled());
    }
}

#[test]
fn a_change_with_neither_enable_nor_disable_leaves_the_watch_as_it_is() {
    let read = QueueFilter::Read;
    for added in [QueueFlags::ADD, all_flags(&[QueueFlags::ADD, QueueFlags::DISABLE])] {
        let mut state = watch_of(read, added);
        let before = state.enabled();
        state.change(&change(DESCRIPTOR, read, QueueFlags::NONE), read, false);
        assert_eq!(state.enabled(), before);
    }
}

#[test]
fn an_add_of_a_held_watch_enables_it_unless_it_disables_it() {
    let read = QueueFilter::Read;
    let mut state = watch_of(read, all_flags(&[QueueFlags::ADD, QueueFlags::DISABLE]));
    assert!(!state.enabled());
    state.change(&change(DESCRIPTOR, read, QueueFlags::ADD), read, true);
    assert!(state.enabled());
    let disabling = change(DESCRIPTOR, read, all_flags(&[QueueFlags::ADD, QueueFlags::DISABLE]));
    state.change(&disabling, read, true);
    assert!(!state.enabled());
}

#[test]
fn a_held_watch_keeps_the_options_it_was_first_added_with() {
    let read = QueueFilter::Read;
    let ready = PollEvents::READABLE;
    let options = all_flags(&[QueueFlags::ONESHOT, QueueFlags::CLEAR, QueueFlags::DISPATCH]);
    let mut clear = watch_of(read, all_flags(&[QueueFlags::ADD, QueueFlags::CLEAR]));
    let others = all_flags(&[QueueFlags::ADD, QueueFlags::ONESHOT, QueueFlags::DISPATCH]);
    clear.change(&change(DESCRIPTOR, read, others), read, true);
    let (event, then) = look_at(&mut clear, read, ready).unwrap();
    assert_eq!((event.flags, then), (bits(QueueFlags::CLEAR), Then::AwaitChange));
    // One added with none gains none, from a change that adds or one that only changes.
    let mut plain = watch_of(read, QueueFlags::ADD);
    plain.change(&change(DESCRIPTOR, read, options.union(QueueFlags::ADD)), read, true);
    plain.change(&change(DESCRIPTOR, read, options), read, false);
    let (event, then) = look_at(&mut plain, read, ready).unwrap();
    assert_eq!((event.flags, then), (0, Then::LookAgain));
}

#[test]
fn a_user_events_flags_change_as_the_control_bits_say() {
    let mut state = user_watch(QueueFlags::ADD, USER_TRIGGER | USER_COPY | 5, 0);
    assert_eq!(look_at_event(&mut state).unwrap().0.fflags, 5);
    // OR adds, AND keeps what both have, no control bit leaves them, COPY replaces them.
    assert_eq!(flags_after_change(&mut state, USER_OR | 2), 7);
    assert_eq!(flags_after_change(&mut state, USER_AND | 6), 6);
    assert_eq!(flags_after_change(&mut state, 0xff), 6);
    assert_eq!(flags_after_change(&mut state, USER_COPY | 0x30), 0x30);
    assert_eq!(flags_after_change(&mut state, USER_COPY), 0);
}

#[test]
fn a_user_event_starts_with_no_flags_of_its_own() {
    // The change that adds the event applies its flags to none: NOP and AND leave none, OR and
    // COPY give the change's.
    for (control, expected) in [(0, 0), (USER_AND, 0), (USER_OR, 0b110), (USER_COPY, 0b110)] {
        let mut state = user_watch(QueueFlags::ADD, USER_TRIGGER | control | 0b110, 0);
        assert_eq!(look_at_event(&mut state).unwrap().0.fflags, expected, "control {control:#x}");
    }
}

#[test]
fn a_user_events_own_flags_are_the_low_24_bits_of_fflags() {
    let mut state = user_watch(QueueFlags::ADD, USER_TRIGGER | USER_COPY | USER_FLAGS, 0);
    assert_eq!(look_at_event(&mut state).unwrap().0.fflags, 0x00ff_ffff);
}

#[test]
fn a_user_event_is_ready_once_a_change_triggers_it() {
    let mut state = user_watch(QueueFlags::ADD, 0, 0);
    // Its trigger alone makes it ready, whatever the descriptors are ready for.
    let everything = all_events(&[PollEvents::READABLE, PollEvents::WRITABLE, PollEvents::HANG_UP]);
    assert_eq!(state.report(EVENT, QueueFilter::User, everything), None);
    // Setting its flags is not triggering it.
    state.change(&user_change(QueueFlags::NONE, USER_OR | 1, 0), QueueFilter::User, false);
    assert_eq!(look_at_event(&mut state), None);
    state.change(&user_change(QueueFlags::NONE, USER_TRIGGER, 0), QueueFilter::User, false);
    assert!(look_at_event(&mut state).is_some());
    // A later change that does not trigger leaves it triggered.
    state.change(&user_change(QueueFlags::NONE, 0, 0), QueueFilter::User, false);
    assert!(look_at_event(&mut state).is_some());
}

#[test]
fn an_add_of_a_held_user_event_can_trigger_it() {
    let mut state = user_watch(QueueFlags::ADD, 0, 0);
    state.change(&user_change(QueueFlags::ADD, USER_TRIGGER, 0), QueueFilter::User, true);
    assert!(look_at_event(&mut state).is_some());
}

#[test]
fn a_user_event_can_be_triggered_as_it_is_added() {
    let mut state = user_watch(QueueFlags::ADD, USER_TRIGGER, 0);
    assert!(look_at_event(&mut state).is_some());
}

proptest! {
    /// `ready`, the look an exclusive watch is given as it is added, says
    /// what a report would, for any filter, options, events asked for and
    /// readiness.
    #[test]
    fn ready_says_what_a_report_would(
        filter in prop::sample::select(vec![
            QueueFilter::Read,
            QueueFilter::Write,
            QueueFilter::Poll,
            QueueFilter::User,
        ]),
        options in prop::sample::select(vec![
            QueueFlags::NONE,
            QueueFlags::CLEAR,
            QueueFlags::ONESHOT,
            QueueFlags::DISPATCH,
            QueueFlags::DISABLE,
        ]),
        asked in any::<u32>(),
        ready in any::<u32>(),
        triggered in any::<bool>(),
    ) {
        let with = QueueFlags::ADD.union(options);
        let added = if filter == QueueFilter::User {
            user_change(with, if triggered { USER_TRIGGER } else { 0 }, 0)
        } else {
            QueueEvent { fflags: asked, ..change(DESCRIPTOR, filter, with) }
        };
        let mut state = State::added(&added, filter);
        let events = PollEvents(ready);
        let said = state.ready(filter, events);
        prop_assert_eq!(said, state.report(added.ident, filter, events).is_some());
    }
}

#[test]
fn every_change_of_a_user_event_sets_its_data() {
    let user = QueueFilter::User;
    let mut state = user_watch(QueueFlags::ADD, USER_TRIGGER, 11);
    assert_eq!(look_at_event(&mut state).unwrap().0.data, 11);
    // Even one that triggers nothing and touches no flags, and an add of the held event.
    state.change(&user_change(QueueFlags::NONE, 0, -3), user, false);
    assert_eq!(look_at_event(&mut state).unwrap().0.data, -3);
    state.change(&user_change(QueueFlags::NONE, USER_TRIGGER, i64::MAX), user, false);
    assert_eq!(look_at_event(&mut state).unwrap().0.data, i64::MAX);
    state.change(&user_change(QueueFlags::ADD, 0, 0), user, true);
    assert_eq!(look_at_event(&mut state).unwrap().0.data, 0);
}

#[test]
fn a_disabled_watch_reports_nothing_though_it_is_ready() {
    let ready = all_events(&[
        PollEvents::READABLE,
        PollEvents::WRITABLE,
        PollEvents::HANG_UP,
        PollEvents::ERROR,
    ]);
    for filter in [QueueFilter::Read, QueueFilter::Write] {
        let mut state = watch_of(filter, all_flags(&[QueueFlags::ADD, QueueFlags::DISABLE]));
        assert_eq!(look_at(&mut state, filter, ready), None);
        state.change(&change(DESCRIPTOR, filter, QueueFlags::ENABLE), filter, false);
        assert!(look_at(&mut state, filter, ready).is_some());
        state.change(&change(DESCRIPTOR, filter, QueueFlags::DISABLE), filter, false);
        assert!(!state.enabled());
        assert_eq!(look_at(&mut state, filter, ready), None);
    }
    // A triggered user event is disabled the same way.
    let mut event = user_watch(all_flags(&[QueueFlags::ADD, QueueFlags::DISABLE]), USER_TRIGGER, 0);
    assert_eq!(look_at_event(&mut event), None);
    event.change(&user_change(QueueFlags::ENABLE, 0, 0), QueueFilter::User, false);
    assert!(look_at_event(&mut event).is_some());
}

#[test]
fn a_watch_reports_nothing_while_its_descriptor_is_not_ready() {
    let mut read = watch_of(QueueFilter::Read, QueueFlags::ADD);
    let mut write = watch_of(QueueFilter::Write, QueueFlags::ADD);
    for quiet in [PollEvents::NONE, PollEvents::PRIORITY, PollEvents::INVALID] {
        assert_eq!(look_at(&mut read, QueueFilter::Read, quiet), None);
        assert_eq!(look_at(&mut write, QueueFilter::Write, quiet), None);
    }
    // A read watch is not ready for writing, nor a write watch for reading.
    assert_eq!(look_at(&mut read, QueueFilter::Read, PollEvents::WRITABLE), None);
    assert_eq!(look_at(&mut write, QueueFilter::Write, PollEvents::READABLE), None);
}

#[test]
fn a_read_watch_is_ready_when_readable_hung_up_or_failed() {
    let readable = all_events(&[PollEvents::READABLE, PollEvents::WRITABLE]);
    for ready in [PollEvents::READABLE, PollEvents::HANG_UP, PollEvents::ERROR, readable] {
        let mut state = watch_of(QueueFilter::Read, QueueFlags::ADD);
        assert!(look_at(&mut state, QueueFilter::Read, ready).is_some(), "{ready:?}");
    }
}

#[test]
fn a_write_watch_is_ready_when_writable_hung_up_or_failed() {
    let writable = all_events(&[PollEvents::READABLE, PollEvents::WRITABLE]);
    for ready in [PollEvents::WRITABLE, PollEvents::HANG_UP, PollEvents::ERROR, writable] {
        let mut state = watch_of(QueueFilter::Write, QueueFlags::ADD);
        assert!(look_at(&mut state, QueueFilter::Write, ready).is_some(), "{ready:?}");
    }
}

#[test]
fn a_hung_up_or_failed_descriptor_reports_eof_and_a_merely_ready_one_does_not() {
    let (read, write) = (QueueFilter::Read, QueueFilter::Write);
    let kinds = [(read, PollEvents::READABLE), (write, PollEvents::WRITABLE)];
    for (filter, ready) in kinds {
        let mut state = watch_of(filter, QueueFlags::ADD);
        assert_eq!(look_at(&mut state, filter, ready).unwrap().0.flags, 0);
        let ended = [
            PollEvents::HANG_UP,
            PollEvents::ERROR,
            all_events(&[ready, PollEvents::HANG_UP]),
            all_events(&[ready, PollEvents::ERROR]),
        ];
        for events in ended {
            let flags = look_at(&mut state, filter, events).unwrap().0.flags;
            assert_eq!(flags, bits(QueueFlags::EOF), "{filter:?} {events:?}");
        }
    }
}

#[test]
fn a_user_event_never_reports_eof() {
    let mut state = user_watch(QueueFlags::ADD, USER_TRIGGER, 0);
    let ended = all_events(&[PollEvents::HANG_UP, PollEvents::ERROR]);
    let (event, _) = state.report(EVENT, QueueFilter::User, ended).unwrap();
    assert_eq!(event.flags, 0);
}

#[test]
fn an_event_carries_the_watchs_key_and_udata() {
    // The key is the one the queue holds the watch under, not the added change's.
    let add = change(DESCRIPTOR, QueueFilter::Write, QueueFlags::ADD);
    let mut state = State::added(&QueueEvent { udata: 0xfeed, ..add }, QueueFilter::Write);
    let (event, _) = state.report(42, QueueFilter::Write, PollEvents::WRITABLE).unwrap();
    let expected =
        QueueEvent { ident: 42, filter: 0xfffe, flags: 0, fflags: 0, data: 0, udata: 0xfeed };
    assert_eq!(event, expected);
    let user = QueueEvent { udata: 0xbeef, ..user_change(QueueFlags::ADD, USER_TRIGGER, 0) };
    let mut state = State::added(&user, QueueFilter::User);
    let (event, _) = look_at_event(&mut state).unwrap();
    assert_eq!((event.ident, event.filter, event.udata), (EVENT, 0xfff5, 0xbeef));
}

#[test]
fn an_event_carries_the_oneshot_clear_and_dispatch_it_was_added_with() {
    let (oneshot, clear, dispatch) = (QueueFlags::ONESHOT, QueueFlags::CLEAR, QueueFlags::DISPATCH);
    let combinations = [
        (oneshot, 0x10),
        (clear, 0x20),
        (dispatch, 0x80),
        (all_flags(&[oneshot, clear]), 0x30),
        (all_flags(&[clear, dispatch]), 0xa0),
        (all_flags(&[oneshot, clear, dispatch]), 0xb0),
    ];
    let read = QueueFilter::Read;
    for (options, expected) in combinations {
        let added = options.union(QueueFlags::ADD);
        let (event, _) = look_at(&mut watch_of(read, added), read, PollEvents::READABLE).unwrap();
        assert_eq!(event.flags, expected);
        // Beside `eof`, when the descriptor has hung up.
        let (event, _) = look_at(&mut watch_of(read, added), read, PollEvents::HANG_UP).unwrap();
        assert_eq!(event.flags, expected | 0x8000);
    }
}

#[test]
fn a_read_or_write_event_has_no_fflags_and_no_data() {
    for filter in [QueueFilter::Read, QueueFilter::Write] {
        // A program may leave anything in them, and a trigger means nothing here.
        let noisy = QueueEvent {
            fflags: USER_TRIGGER | USER_COPY | 5,
            data: 9,
            ..change(DESCRIPTOR, filter, QueueFlags::ADD)
        };
        let mut state = State::added(&noisy, filter);
        state.change(&QueueEvent { flags: 0, ..noisy }, filter, false);
        assert_eq!(look_at(&mut state, filter, PollEvents::NONE), None);
        let ready = all_events(&[PollEvents::READABLE, PollEvents::WRITABLE]);
        let (event, _) = look_at(&mut state, filter, ready).unwrap();
        assert_eq!((event.fflags, event.data), (0, 0));
    }
}

#[test]
fn a_level_triggered_watch_is_looked_at_again_for_as_long_as_it_is_ready() {
    let ready = all_events(&[PollEvents::READABLE, PollEvents::WRITABLE]);
    for filter in [QueueFilter::Read, QueueFilter::Write] {
        let mut state = watch_of(filter, QueueFlags::ADD);
        for _ in 0..3 {
            let (_, then) = look_at(&mut state, filter, ready).unwrap();
            assert_eq!(then, Then::LookAgain);
        }
        assert!(state.enabled());
    }
}

#[test]
fn a_clear_watch_is_looked_at_again_only_after_a_change() {
    let ready = PollEvents::READABLE;
    let mut state = watch_of(QueueFilter::Read, all_flags(&[QueueFlags::ADD, QueueFlags::CLEAR]));
    let (_, then) = look_at(&mut state, QueueFilter::Read, ready).unwrap();
    assert_eq!(then, Then::AwaitChange);
    // The watch stays enabled: the queue looks again after a change, and reports what it finds.
    assert!(state.enabled());
    assert!(look_at(&mut state, QueueFilter::Read, ready).is_some());
}

#[test]
fn a_clear_user_event_forgets_its_trigger_flags_and_value_when_reported() {
    let user = QueueFilter::User;
    let clear = all_flags(&[QueueFlags::ADD, QueueFlags::CLEAR]);
    let mut state = user_watch(clear, USER_TRIGGER | USER_COPY | 5, 42);
    let (event, then) = look_at_event(&mut state).unwrap();
    assert_eq!((event.fflags, event.data, then), (5, 42, Then::AwaitChange));
    assert_eq!(look_at_event(&mut state), None);
    // The next trigger reports only what comes with it: its flags OR into none, not into 5.
    state.change(&user_change(QueueFlags::NONE, USER_TRIGGER | USER_OR | 2, 7), user, false);
    let (event, _) = look_at_event(&mut state).unwrap();
    assert_eq!((event.fflags, event.data), (2, 7));
    // A trigger with no flags and no value reports none, not the last ones.
    assert_eq!(look_at_event(&mut state), None);
    state.change(&user_change(QueueFlags::NONE, USER_TRIGGER, 0), user, false);
    let (event, _) = look_at_event(&mut state).unwrap();
    assert_eq!((event.fflags, event.data), (0, 0));
}

#[test]
fn a_user_event_without_clear_keeps_its_trigger_flags_and_value() {
    let mut state = user_watch(QueueFlags::ADD, USER_TRIGGER | USER_COPY | 5, 42);
    for _ in 0..3 {
        let (event, then) = look_at_event(&mut state).unwrap();
        assert_eq!((event.fflags, event.data, then), (5, 42, Then::LookAgain));
    }
}

#[test]
fn a_dispatch_watch_is_disabled_when_reported_and_looked_at_again_when_enabled() {
    let read = QueueFilter::Read;
    let ready = PollEvents::READABLE;
    let mut state = watch_of(read, all_flags(&[QueueFlags::ADD, QueueFlags::DISPATCH]));
    let (_, then) = look_at(&mut state, read, ready).unwrap();
    assert_eq!(then, Then::AwaitChange);
    assert!(!state.enabled());
    assert_eq!(look_at(&mut state, read, ready), None);
    state.change(&change(DESCRIPTOR, read, QueueFlags::ENABLE), read, false);
    assert!(state.enabled());
    assert!(look_at(&mut state, read, ready).is_some());
}

#[test]
fn a_dispatch_user_event_with_clear_is_disabled_and_forgets_its_trigger() {
    let flags = all_flags(&[QueueFlags::ADD, QueueFlags::DISPATCH, QueueFlags::CLEAR]);
    let mut state = user_watch(flags, USER_TRIGGER | USER_COPY | 3, 8);
    let (_, then) = look_at_event(&mut state).unwrap();
    assert_eq!(then, Then::AwaitChange);
    assert!(!state.enabled());
    // Enabling it does not make it ready: nothing has triggered it since.
    state.change(&user_change(QueueFlags::ENABLE, 0, 0), QueueFilter::User, false);
    assert!(state.enabled());
    assert_eq!(look_at_event(&mut state), None);
}

#[test]
fn a_oneshot_watch_is_deleted_when_reported() {
    let ready = all_events(&[PollEvents::READABLE, PollEvents::WRITABLE]);
    for filter in [QueueFilter::Read, QueueFilter::Write] {
        let mut state = watch_of(filter, all_flags(&[QueueFlags::ADD, QueueFlags::ONESHOT]));
        let (event, then) = look_at(&mut state, filter, ready).unwrap();
        assert_eq!((event.flags, then), (0x10, Then::Delete));
    }
    let mut event = user_watch(all_flags(&[QueueFlags::ADD, QueueFlags::ONESHOT]), USER_TRIGGER, 0);
    assert_eq!(look_at_event(&mut event).unwrap().1, Then::Delete);
}

#[test]
fn oneshot_wins_over_dispatch_and_clear() {
    let oneshot = all_flags(&[QueueFlags::ADD, QueueFlags::ONESHOT]);
    let (clear, dispatch) = (QueueFlags::CLEAR, QueueFlags::DISPATCH);
    for others in [dispatch, clear, all_flags(&[dispatch, clear])] {
        let mut state = watch_of(QueueFilter::Read, oneshot.union(others));
        let (event, then) = look_at(&mut state, QueueFilter::Read, PollEvents::READABLE).unwrap();
        assert_eq!(then, Then::Delete);
        // It is still reported with every option it was added with.
        assert_eq!(event.flags, bits(QueueFlags::ONESHOT.union(others)));
    }
}

#[test]
fn an_error_event_is_the_change_with_the_error_flag_and_the_status_in_data() {
    let failed = QueueEvent {
        ident: 12,
        filter: QueueFilter::Write.raw(),
        flags: bits(all_flags(&[QueueFlags::ADD, QueueFlags::ONESHOT, QueueFlags::RECEIPT])),
        fflags: 0x1234,
        data: 99,
        udata: 0xcafe,
    };
    let status = Kernel::InvalidArgument.status(1).0;
    let reported = watch::error_event(&failed, status);
    assert_eq!(reported, QueueEvent { flags: 0x4000, data: status as i64, ..failed });
    // A receipt for a change that worked says 0, and the status is a bit pattern.
    assert_eq!(watch::error_event(&failed, 0).data, 0);
    assert_eq!(watch::error_event(&failed, u64::MAX).data, -1);
}

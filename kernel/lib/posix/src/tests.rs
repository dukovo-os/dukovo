// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Host tests: hand-written cases for each module, and random operations
//! on the process tree checked against its invariants.

use std::alloc::Global;

use proptest::prelude::*;
use radix_abi::process::{EXEC_VERSION, ExecHeader};
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
use crate::tree::{Found, Life, Tree, TreeError, WaitFor, Which};

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
    let info = |pid| Info { code: signal::SI_USER, pid, status: 0 };
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
    let info = |status| Info { code: signal::SI_QUEUE, pid: 1, status };
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
    assert_eq!(tree.exit(2, Ended::Exited(7), (5, 6), false, &mut |h| gone.push(h)), Some(1));
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
    tree.exit(2, Ended::Exited(7), (5, 6), false, &mut |h| gone.push(h));
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
    tree.exit(4, Ended::Killed(SIGTERM), (0, 0), false, &mut |h| gone.push(h));
    assert!(gone.is_empty(), "4 waits for 2 to reap it");
    tree.exit(2, Ended::Exited(0), (0, 0), false, &mut |h| gone.push(h));
    assert_eq!(gone, [4]);
    assert_eq!(tree.get(3).unwrap().parent, 0);
    // 3 has no parent now, so it goes as soon as it ends.
    tree.exit(3, Ended::Exited(1), (0, 0), false, &mut |h| gone.push(h));
    assert_eq!(gone, [4, 3]);
    // A parent that ignores SIGCHLD has its children reaped at once.
    tree.exit(2, Ended::Exited(0), (0, 0), false, &mut |h| gone.push(h));
    tree.insert_child(5, 1, 5).unwrap();
    tree.exit(5, Ended::Exited(0), (0, 0), true, &mut |h| gone.push(h));
    assert!(tree.get(5).is_none());
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
    // Signals: to itself, its descendants and its session's processes of
    // its user; tracing: only its descendants.
    let same = |_: &u64, _: &u64| true;
    assert!(tree.may_signal(10, 12, same) && tree.may_signal(11, 10, same));
    assert!(!tree.may_signal(12, 11, same));
    let user = |pid: &u64| if *pid == 11 { 1000 } else { 0 };
    let users = |a: &u64, b: &u64| user(a) == user(b);
    assert!(!tree.may_signal(11, 10, users) && tree.may_signal(10, 11, users));
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
    tree.exit(10, Ended::Exited(0), (0, 0), false, &mut |_| {});
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
    Exit(usize, bool),
    Wait(usize),
    SetGroup(usize, usize, usize),
    Session(usize),
    Stop(usize),
    Continue(usize),
}

fn op() -> impl Strategy<Value = Op> {
    let n = 0usize..16;
    prop_oneof![
        3 => n.clone().prop_map(Op::Fork),
        2 => (n.clone(), any::<bool>()).prop_map(|(a, b)| Op::Exit(a, b)),
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
                Op::Exit(i, auto) => {
                    let pid = pick(&tree, i);
                    if !matches!(tree.get(pid).unwrap().life, Life::Zombie { .. }) {
                        tree.exit(pid, Ended::Exited(0), (0, 0), auto, &mut |_| {});
                    }
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

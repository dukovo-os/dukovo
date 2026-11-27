// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The datagrams a Linux program sends on a netlink route socket, which
//! linuxd takes apart: the framing of the messages in one datagram, what
//! rtnetlink does with each, and the NLMSG_ERROR that answers a request
//! that fails. A datagram is any bytes, and none of it stops the parser
//! or makes an answer that does not read back.

#![forbid(unsafe_code)]
#![no_main]

use libfuzzer_sys::fuzz_target;
use linuxd::netlink::wire::{self, Action, HEADER, Lookup};

/// NLMSG_ERROR, the type of the answer to a request that fails.
const NLMSG_ERROR: u16 = 2;

/// The errnos of Linux's x86-64 that a request can fail with.
const EPERM: i32 = 1;
const ENODEV: i32 = 19;
const EINVAL: i32 = 22;
const EOPNOTSUPP: i32 = 95;

fuzz_target!(|data: &[u8]| {
    // The messages lie inside the datagram, one after the other, and a
    // program cannot make more of them than there are headers.
    let mut taken = 0;
    for message in wire::messages(data) {
        taken += 1;
        assert!(HEADER + message.payload.len() <= data.len());
    }
    assert!(taken <= data.len() / HEADER);

    // The two kinds of sender are served alike, except for the errno of a
    // request that would change the network.
    let plain: Vec<_> = wire::requests(data, false).collect();
    let root: Vec<_> = wire::requests(data, true).collect();
    assert_eq!(plain.len(), taken);
    assert_eq!(root.len(), taken);

    for ((message, action), (same, with_right)) in plain.into_iter().zip(root) {
        assert_eq!(message, same);
        match (action, with_right) {
            (Action::Refuse(without), Action::Refuse(with)) => {
                assert!(matches!(without, EPERM | EOPNOTSUPP | EINVAL));
                assert!(matches!(with, EOPNOTSUPP | EINVAL));
            }
            (action, with_right) => assert_eq!(action, with_right),
        }
        match action {
            Action::Dump { family, .. } => assert_eq!(Some(&family), message.payload.first()),
            Action::Link(Lookup::Index(index)) => assert!(index > 0),
            Action::Link(Lookup::Name(name)) => assert!(name.len() <= 15 && !name.contains(&0)),
            Action::Ignore | Action::Refuse(_) => {}
        }

        // An error for the request reads back as one message that echoes
        // the request's numbers, for a socket that is bound or not and
        // one that asked for short errors or not.
        for (errno, cap_ack, pid) in [(0, false, 0), (EPERM, false, 7), (ENODEV, true, u32::MAX)] {
            let bytes = wire::error(errno, &message, pid, cap_ack);
            assert_eq!(bytes.len() % 4, 0);
            let mut read = wire::messages(&bytes);
            let answer = read.next().expect("the error is a message");
            assert!(read.next().is_none());
            assert_eq!((answer.kind, answer.seq, answer.pid), (NLMSG_ERROR, message.seq, pid));
            assert_eq!(answer.payload[..4], (-errno).to_ne_bytes());
            // The request's header follows, and its body when the error
            // is whole.
            assert_eq!(
                answer.payload[4..8],
                u32::try_from(HEADER + message.payload.len()).unwrap_or(u32::MAX).to_ne_bytes()
            );
            let whole = errno != 0 && !cap_ack;
            assert_eq!(
                answer.payload.len(),
                4 + HEADER + if whole { message.payload.len() } else { 0 }
            );
        }
    }
});

// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A client's COMPOUNDs against the server, with a worker in the same
//! process: sessions, opens, reads and writes, folders, locks, and who
//! may reach what.

use std::fs;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::fh::Fh;
use crate::ipc::{Answer, Request};
use crate::nfs::{Attrs, attr, err, kind, op};
use crate::rpc;
use crate::server::{Backend, Context, Server};
use crate::shares;
use crate::worker::Worker;
use crate::worker::tests::tempdir;
use crate::xdr::{Reader, Writer};

/// The worker, in the process, and the people it was asked to serve as.
struct Local {
    worker: Worker,
    people: Vec<u32>,
}

impl Backend for Local {
    fn call(&mut self, uid: u32, request: Request) -> Answer {
        self.people.push(uid);
        self.worker.serve(request)
    }

    fn restart(&mut self) {}
}

/// A server sharing `music` as Music to 10.0.2.0/24, `docs` as Docs to
/// the same, read-only, and `music` as Private to none.
fn server(music: &Path, docs: &Path) -> Server<Local> {
    let text = format!(
        "[[share]]\nname = \"Music\"\nfolder = \"{}\"\nnfs = [\"10.0.2.0/24\"]\n\
         [[share]]\nname = \"Docs\"\nfolder = \"{}\"\nread-only = true\nnfs = [\"10.0.2.0/24\"]\n\
         [[share]]\nname = \"Private\"\nfolder = \"{}\"\n",
        music.display(),
        docs.display(),
        music.display()
    );
    let shares = shares::of_toml(&text).unwrap();
    let worker = Worker::new(&[music.to_path_buf(), docs.to_path_buf()]);
    Server::new(Local { worker, people: Vec::new() }, 7, [3; 8], "test").with_shares(shares)
}

const CLIENT: &str = "10.0.2.15";

fn cx(uid: u32) -> Context {
    Context { peer: CLIENT.parse().unwrap(), uid }
}

/// An operation: its number, then what `args` writes.
fn op(code: u32, args: impl FnOnce(&mut Writer)) -> Vec<u8> {
    let mut w = Writer::new();
    w.u32(code);
    args(&mut w);
    w.bytes
}

/// A COMPOUND of `ops`, minor version 2: its answer.
fn compound(server: &mut Server<Local>, cx: &Context, ops: &[Vec<u8>]) -> Vec<u8> {
    let mut w = Writer::new();
    w.opaque(b"test").u32(2).u32(ops.len() as u32);
    for o in ops {
        w.bytes.extend_from_slice(o);
    }
    server.compound(cx, &w.bytes).unwrap()
}

/// An answer's status, and a reader at its first result.
fn results(answer: &[u8]) -> (u32, Reader<'_>) {
    let mut r = Reader::new(answer);
    let status = r.u32().unwrap();
    r.opaque(1024).unwrap();
    r.u32().unwrap();
    (status, r)
}

/// The next result's operation and status.
fn next(r: &mut Reader) -> (u32, u32) {
    (r.u32().unwrap(), r.u32().unwrap())
}

/// Reads past a result's fattr4.
fn skip_attrs(r: &mut Reader) {
    r.bitmap().unwrap();
    r.opaque(1 << 16).unwrap();
}

/// A client with a session.
struct Session {
    id: [u8; 16],
    sequence: u32,
    client: u64,
}

impl Session {
    /// EXCHANGE_ID and CREATE_SESSION, as `owner`.
    fn new(server: &mut Server<Local>, owner: &[u8]) -> Session {
        Session::try_new(server, owner).unwrap()
    }

    /// EXCHANGE_ID and CREATE_SESSION, as `owner`: the session, or
    /// CREATE_SESSION's status.
    fn try_new(server: &mut Server<Local>, owner: &[u8]) -> Result<Session, u32> {
        let exchange = op(op::EXCHANGE_ID, |w| {
            w.fixed(&[9; 8]).opaque(owner).u32(0).u32(0).u32(0);
        });
        let answer = compound(server, &cx(0), &[exchange]);
        let (status, mut r) = results(&answer);
        assert_eq!(status, err::OK);
        assert_eq!(next(&mut r), (op::EXCHANGE_ID, err::OK));
        let client = r.u64().unwrap();
        let sequence = r.u32().unwrap();
        let create = op(op::CREATE_SESSION, |w| {
            w.u64(client).u32(sequence).u32(0);
            w.u32(0).u32(1 << 20).u32(1 << 20).u32(65536).u32(8).u32(16).u32(0);
            w.u32(0).u32(4096).u32(4096).u32(0).u32(2).u32(1).u32(0);
            w.u32(0x4000_0000).u32(1).u32(0);
        });
        let answer = compound(server, &cx(0), &[create]);
        let (status, mut r) = results(&answer);
        if status != err::OK {
            return Err(status);
        }
        next(&mut r);
        let id = r.array().unwrap();
        Ok(Session { id, sequence: 0, client })
    }

    /// SEQUENCE, the next request in slot 0.
    fn sequence(&mut self, keep: bool) -> Vec<u8> {
        self.sequence += 1;
        let (id, sequence) = (self.id, self.sequence);
        op(op::SEQUENCE, |w| {
            w.fixed(&id).u32(sequence).u32(0).u32(0).bool(keep);
        })
    }

    /// A COMPOUND of `ops` after SEQUENCE, as `uid`: its answer.
    fn call(&mut self, server: &mut Server<Local>, uid: u32, ops: &[Vec<u8>]) -> Vec<u8> {
        let mut all = vec![self.sequence(false)];
        all.extend_from_slice(ops);
        compound(server, &cx(uid), &all)
    }
}

/// The results after SEQUENCE's.
fn after_sequence(answer: &[u8]) -> (u32, Reader<'_>) {
    let (status, mut r) = results(answer);
    assert_eq!(next(&mut r), (op::SEQUENCE, err::OK));
    r.fixed(36).unwrap();
    (status, r)
}

fn putfh(fh: &[u8]) -> Vec<u8> {
    op(op::PUTFH, |w| {
        w.opaque(fh);
    })
}

fn lookup(name: &str) -> Vec<u8> {
    op(op::LOOKUP, |w| {
        w.opaque(name.as_bytes());
    })
}

fn getattr(attrs: &[u32]) -> Vec<u8> {
    let attrs = Attrs::of(attrs);
    op(op::GETATTR, |w| {
        w.bitmap(&attrs.0);
    })
}

/// OPEN of `name` in the current folder, by `owner`, made if `create`,
/// to read and write.
fn open(owner: &[u8], name: &str, create: bool) -> Vec<u8> {
    op(op::OPEN, |w| {
        w.u32(0).u32(3).u32(0).u64(0).opaque(owner);
        if create {
            // UNCHECKED4, mode 0644.
            let mut values = Writer::new();
            values.u32(0o644);
            w.u32(1).u32(0).bitmap(&Attrs::of(&[attr::MODE]).0).opaque(&values.bytes);
        } else {
            w.u32(0);
        }
        w.u32(0).opaque(name.as_bytes());
    })
}

/// Reads OPEN's result: its stateid.
fn opened(r: &mut Reader) -> (u32, [u8; 12]) {
    let seqid = r.u32().unwrap();
    let other = r.array().unwrap();
    r.fixed(4 + 16 + 4).unwrap();
    r.bitmap().unwrap();
    assert_eq!(r.u32(), Ok(0));
    (seqid, other)
}

fn stateid(w: &mut Writer, stateid: (u32, [u8; 12])) {
    w.u32(stateid.0).fixed(&stateid.1);
}

/// The share's handle, found from the root.
fn share_fh(server: &mut Server<Local>, session: &mut Session, name: &str) -> Vec<u8> {
    let get = op(op::GETFH, |_| {});
    let answer = session.call(server, 1000, &[op(op::PUTROOTFH, |_| {}), lookup(name), get]);
    let (status, mut r) = after_sequence(&answer);
    assert_eq!(status, err::OK);
    next(&mut r);
    next(&mut r);
    assert_eq!(next(&mut r), (op::GETFH, err::OK));
    r.opaque(128).unwrap().to_vec()
}

fn dirs(name: &str) -> (PathBuf, PathBuf) {
    // Apart from the worker's own tests' folders.
    let base = tempdir(&format!("compound-{name}"));
    let (music, docs) = (base.join("music"), base.join("docs"));
    fs::create_dir_all(&music).unwrap();
    fs::create_dir_all(&docs).unwrap();
    (music, docs)
}

#[test]
fn sessions_order_their_requests() {
    let (music, docs) = dirs("sessions");
    let mut server = server(&music, &docs);
    // No session: only the operations that make one.
    let answer = compound(&mut server, &cx(0), &[op(op::PUTROOTFH, |_| {})]);
    assert_eq!(results(&answer).0, err::OP_NOT_IN_SESSION);
    let mut w = Writer::new();
    w.opaque(b"").u32(0).u32(0);
    assert_eq!(results(&server.compound(&cx(0), &w.bytes).unwrap()).0, err::MINOR_VERS_MISMATCH);
    let mut s = Session::new(&mut server, b"linux client");
    let root =
        s.call(&mut server, 0, &[op(op::PUTROOTFH, |_| {}), getattr(&[attr::TYPE, attr::FILEID])]);
    let (status, mut r) = after_sequence(&root);
    assert_eq!(status, err::OK);
    next(&mut r);
    assert_eq!(next(&mut r), (op::GETATTR, err::OK));
    r.bitmap().unwrap();
    let mut v = Reader::new(r.opaque(100).unwrap());
    assert_eq!((v.u32(), v.u64()), (Ok(kind::DIR), Ok(1)));
    // The same request again: its answer, if the client asked it kept.
    let kept = s.sequence(true);
    let first = compound(&mut server, &cx(0), &[kept.clone(), op(op::PUTROOTFH, |_| {})]);
    let again = compound(&mut server, &cx(0), &[kept, op(op::PUTROOTFH, |_| {})]);
    assert_eq!(first, again);
    let unkept = s.sequence(false);
    compound(&mut server, &cx(0), std::slice::from_ref(&unkept));
    let answer = compound(&mut server, &cx(0), &[unkept]);
    assert_eq!(results(&answer).0, err::RETRY_UNCACHED_REP);
    // One too far ahead, and SEQUENCE anywhere but first.
    s.sequence += 1;
    assert_eq!(results(&s.call(&mut server, 0, &[])).0, err::SEQ_MISORDERED);
    s.sequence -= 2;
    let id = s.id;
    let late = op(op::SEQUENCE, |w| {
        w.fixed(&id).u32(1).u32(0).u32(0).bool(false);
    });
    let answer = s.call(&mut server, 0, &[late]);
    assert_eq!(results(&answer).0, err::SEQUENCE_POS);
    // The same client again, unchanged: the same record.
    let again = Session::new(&mut server, b"linux client");
    assert_eq!(again.client, s.client);
}

#[test]
fn kept_answers_and_sessions_are_bounded() {
    let (music, docs) = dirs("bounded");
    fs::write(music.join("big.bin"), vec![7; 32 * 1024]).unwrap();
    let mut server = server(&music, &docs);
    let mut s = Session::new(&mut server, b"bounded");
    let share = share_fh(&mut server, &mut s, "Music");
    // A READ too large to keep: refused, as asked to be kept, and read as
    // not.
    let read = op(op::READ, |w| {
        stateid(w, (0, [0; 12]));
        w.u64(0).u32(16 * 1024);
    });
    let ops = [putfh(&share), lookup("big.bin"), read];
    let kept = s.sequence(true);
    let mut all = vec![kept.clone()];
    all.extend_from_slice(&ops);
    let answer = compound(&mut server, &cx(1000), &all);
    let (status, mut r) = after_sequence(&answer);
    assert_eq!(status, err::REP_TOO_BIG_TO_CACHE);
    next(&mut r);
    next(&mut r);
    assert_eq!(next(&mut r), (op::READ, err::REP_TOO_BIG_TO_CACHE));
    assert_eq!(compound(&mut server, &cx(1000), &all), answer);
    let answer = s.call(&mut server, 1000, &ops);
    let (status, mut r) = after_sequence(&answer);
    assert_eq!(status, err::OK);
    next(&mut r);
    next(&mut r);
    assert_eq!(next(&mut r), (op::READ, err::OK));
    assert_eq!((r.bool(), r.opaque(1 << 20).map(<[u8]>::len)), (Ok(false), Ok(16 * 1024)));
    // A client has eight sessions at most.
    for _ in 1..8 {
        Session::new(&mut server, b"bounded");
    }
    assert_eq!(Session::try_new(&mut server, b"bounded").err(), Some(err::NOSPC));
    assert!(Session::try_new(&mut server, b"another").is_ok());
}

#[test]
fn files_are_made_written_read_and_closed() {
    let (music, docs) = dirs("files");
    let mut server = server(&music, &docs);
    let mut s = Session::new(&mut server, b"client");
    let share = share_fh(&mut server, &mut s, "music");
    let answer = s.call(
        &mut server,
        1000,
        &[putfh(&share), open(b"me", "a.txt", true), op(op::GETFH, |_| {})],
    );
    let (status, mut r) = after_sequence(&answer);
    assert_eq!(status, err::OK);
    next(&mut r);
    assert_eq!(next(&mut r), (op::OPEN, err::OK));
    let id = opened(&mut r);
    next(&mut r);
    let fh = r.opaque(128).unwrap().to_vec();
    assert!(matches!(Fh::decode(&fh), Some(Fh::File(..))));
    let write = op(op::WRITE, |w| {
        stateid(w, id);
        w.u64(0).u32(0).opaque(b"hello, world");
    });
    let commit = op(op::COMMIT, |w| {
        w.u64(0).u32(0);
    });
    let read = op(op::READ, |w| {
        stateid(w, id);
        w.u64(7).u32(100);
    });
    let answer = s.call(&mut server, 1000, &[putfh(&fh), write, commit, read]);
    let (status, mut r) = after_sequence(&answer);
    assert_eq!(status, err::OK);
    next(&mut r);
    assert_eq!(next(&mut r), (op::WRITE, err::OK));
    assert_eq!((r.u32(), r.u32(), r.array::<8>()), (Ok(12), Ok(0), Ok([3; 8])));
    assert_eq!(next(&mut r), (op::COMMIT, err::OK));
    r.array::<8>().unwrap();
    assert_eq!(next(&mut r), (op::READ, err::OK));
    assert_eq!((r.bool(), r.opaque(100)), (Ok(true), Ok(&b"world"[..])));
    assert_eq!(fs::read(music.join("a.txt")).unwrap(), b"hello, world");
    // The current stateid closes it; a closed one reads no more.
    let close = op(op::CLOSE, |w| {
        w.u32(0);
        stateid(w, (1, [0; 12]));
    });
    let answer = s.call(&mut server, 1000, &[putfh(&share), open(b"me", "a.txt", false), close]);
    assert_eq!(after_sequence(&answer).0, err::OK);
    let read = op(op::READ, |w| {
        stateid(w, id);
        w.u64(0).u32(1);
    });
    let answer = s.call(&mut server, 1000, &[putfh(&fh), read]);
    assert_eq!(after_sequence(&answer).0, err::BAD_STATEID);
    // LOOKUPP goes up from folders alone.
    let answer = s.call(
        &mut server,
        1000,
        &[
            op(op::PUTROOTFH, |_| {}),
            lookup("Music"),
            open(b"me", "a.txt", false),
            op(op::LOOKUPP, |_| {}),
        ],
    );
    assert_eq!(after_sequence(&answer).0, err::NOTDIR);
}

#[test]
fn folders_are_made_listed_renamed_and_removed() {
    let (music, docs) = dirs("folders");
    let mut server = server(&music, &docs);
    let mut s = Session::new(&mut server, b"client");
    let share = share_fh(&mut server, &mut s, "Music");
    fs::write(music.join("song.flac"), b"la").unwrap();
    let mkdir = op(op::CREATE, |w| {
        let mut values = Writer::new();
        values.u32(0o750);
        w.u32(kind::DIR)
            .opaque(b"albums")
            .bitmap(&Attrs::of(&[attr::MODE]).0)
            .opaque(&values.bytes);
    });
    let answer = s.call(&mut server, 1000, &[putfh(&share), mkdir, getattr(&[attr::MODE])]);
    let (status, mut r) = after_sequence(&answer);
    assert_eq!(status, err::OK);
    next(&mut r);
    assert_eq!(next(&mut r), (op::CREATE, err::OK));
    r.fixed(4 + 16).unwrap();
    assert_eq!(r.bitmap(), Ok(vec![0, 1 << (attr::MODE - 32)]));
    next(&mut r);
    r.bitmap().unwrap();
    assert_eq!(Reader::new(r.opaque(8).unwrap()).u32(), Ok(0o750));
    assert!(music.join("albums").is_dir());
    // The listing, with each entry's handle.
    let readdir = op(op::READDIR, |w| {
        w.u64(0).fixed(&[0; 8]).u32(4096).u32(32768);
        w.bitmap(&Attrs::of(&[attr::TYPE, attr::FILEHANDLE]).0);
    });
    let answer = s.call(&mut server, 1000, &[putfh(&share), readdir]);
    let (status, mut r) = after_sequence(&answer);
    assert_eq!(status, err::OK);
    next(&mut r);
    next(&mut r);
    r.array::<8>().unwrap();
    let mut names = Vec::new();
    while r.bool().unwrap() {
        let cookie = r.u64().unwrap();
        names.push((cookie, String::from_utf8(r.opaque(255).unwrap().to_vec()).unwrap()));
        skip_attrs(&mut r);
    }
    assert_eq!(r.bool(), Ok(true));
    assert_eq!(names, [(3, "albums".to_string()), (4, "song.flac".to_string())]);
    // From cookie 3 on, the rest; too small a reply for one, TOOSMALL.
    let more = op(op::READDIR, |w| {
        w.u64(3).fixed(&[0; 8]).u32(4096).u32(32768).u32(0);
    });
    let answer = s.call(&mut server, 1000, &[putfh(&share), more]);
    let (_, mut r) = after_sequence(&answer);
    next(&mut r);
    next(&mut r);
    r.array::<8>().unwrap();
    assert_eq!((r.bool(), r.u64(), r.opaque(255)), (Ok(true), Ok(4), Ok(&b"song.flac"[..])));
    let small = op(op::READDIR, |w| {
        w.u64(0).fixed(&[0; 8]).u32(10).u32(30).u32(0);
    });
    let answer = s.call(&mut server, 1000, &[putfh(&share), small]);
    assert_eq!(after_sequence(&answer).0, err::TOOSMALL);
    // Into the folder, and the song moved there by RENAME.
    let rename = op(op::RENAME, |w| {
        w.opaque(b"song.flac").opaque(b"first.flac");
    });
    let ops = [
        putfh(&share),
        op(op::SAVEFH, |_| {}),
        lookup("albums"),
        rename,
        op(op::LOOKUPP, |_| {}),
        op(op::GETFH, |_| {}),
    ];
    let answer = s.call(&mut server, 1000, &ops);
    let (status, mut r) = after_sequence(&answer);
    assert_eq!(status, err::OK);
    for _ in 0..3 {
        next(&mut r);
    }
    assert_eq!(next(&mut r), (op::RENAME, err::OK));
    r.fixed(2 * 20).unwrap();
    next(&mut r);
    next(&mut r);
    assert_eq!(r.opaque(128).unwrap(), share.as_slice());
    assert_eq!(fs::read(music.join("albums/first.flac")).unwrap(), b"la");
    // A folder that is not empty stays.
    let remove = |name: &str| {
        op(op::REMOVE, |w| {
            w.opaque(name.as_bytes());
        })
    };
    let answer = s.call(&mut server, 1000, &[putfh(&share), remove("albums")]);
    assert_eq!(after_sequence(&answer).0, err::NOTEMPTY);
    let ops = [
        putfh(&share),
        lookup("albums"),
        remove("first.flac"),
        op(op::LOOKUPP, |_| {}),
        remove("albums"),
    ];
    let answer = s.call(&mut server, 1000, &ops);
    assert_eq!(after_sequence(&answer).0, err::OK);
    assert!(!music.join("albums").exists());
    // Names that are no names.
    for bad in ["..", "a/b"] {
        let answer = s.call(&mut server, 1000, &[putfh(&share), lookup(bad)]);
        assert_eq!(after_sequence(&answer).0, err::BADNAME);
    }
}

#[test]
fn locks_keep_other_owners_out() {
    let (music, docs) = dirs("locks");
    let mut server = server(&music, &docs);
    let mut s = Session::new(&mut server, b"client");
    let share = share_fh(&mut server, &mut s, "Music");
    let mut ids = Vec::new();
    for owner in [&b"one"[..], b"two"] {
        let answer = s.call(
            &mut server,
            1000,
            &[putfh(&share), open(owner, "db", true), op(op::GETFH, |_| {})],
        );
        let (status, mut r) = after_sequence(&answer);
        assert_eq!(status, err::OK);
        next(&mut r);
        next(&mut r);
        let id = opened(&mut r);
        next(&mut r);
        ids.push((id, r.opaque(128).unwrap().to_vec()));
    }
    let lock = |open: (u32, [u8; 12]), owner: &[u8], offset: u64| {
        op(op::LOCK, |w| {
            w.u32(2).bool(false).u64(offset).u64(10).bool(true).u32(0);
            stateid(w, open);
            w.u32(0).u64(0).opaque(owner);
        })
    };
    let fh = ids[0].1.clone();
    let answer = s.call(&mut server, 1000, &[putfh(&fh), lock(ids[0].0, b"lock one", 0)]);
    let (status, mut r) = after_sequence(&answer);
    assert_eq!(status, err::OK);
    next(&mut r);
    next(&mut r);
    let held = (r.u32().unwrap(), r.array::<12>().unwrap());
    assert_eq!(held.0, 1);
    // The other owner is denied, and told whose it is.
    let answer = s.call(&mut server, 1000, &[putfh(&fh), lock(ids[1].0, b"lock two", 5)]);
    let (status, mut r) = after_sequence(&answer);
    assert_eq!(status, err::DENIED);
    next(&mut r);
    next(&mut r);
    assert_eq!((r.u64(), r.u64(), r.u32(), r.u64()), (Ok(0), Ok(10), Ok(2), Ok(s.client)));
    assert_eq!(r.opaque(100), Ok(&b"lock one"[..]));
    // Past it, it is granted; after LOCKU, where it was too.
    let answer = s.call(&mut server, 1000, &[putfh(&fh), lock(ids[1].0, b"lock two", 10)]);
    assert_eq!(after_sequence(&answer).0, err::OK);
    let unlock = op(op::LOCKU, |w| {
        w.u32(2).u32(0);
        stateid(w, held);
        w.u64(0).u64(u64::MAX);
    });
    let test = op(op::LOCKT, |w| {
        w.u32(2).u64(0).u64(5).u64(0).opaque(b"lock three");
    });
    let answer = s.call(&mut server, 1000, &[putfh(&fh), unlock, test]);
    assert_eq!(after_sequence(&answer).0, err::OK);
    // A locker with no locks is freed; an open is not.
    let free = |id: (u32, [u8; 12])| {
        op(op::FREE_STATEID, |w| {
            stateid(w, id);
        })
    };
    let answer = s.call(&mut server, 1000, &[free((0, held.1))]);
    assert_eq!(after_sequence(&answer).0, err::OK);
    let answer = s.call(&mut server, 1000, &[free(ids[0].0)]);
    assert_eq!(after_sequence(&answer).0, err::LOCKS_HELD);
}

#[test]
fn read_only_shares_and_other_networks_are_kept_out() {
    let (music, docs) = dirs("kept-out");
    let mut server = server(&music, &docs);
    let mut s = Session::new(&mut server, b"client");
    let docs_fh = share_fh(&mut server, &mut s, "Docs");
    let answer = s.call(&mut server, 1000, &[putfh(&docs_fh), open(b"me", "new", true)]);
    assert_eq!(after_sequence(&answer).0, err::ROFS);
    let access = op(op::ACCESS, |w| {
        w.u32(0x3f);
    });
    let answer = s.call(&mut server, 1000, &[putfh(&docs_fh), access]);
    let (status, mut r) = after_sequence(&answer);
    assert_eq!(status, err::OK);
    next(&mut r);
    next(&mut r);
    assert_eq!(r.u32(), Ok(0x3f));
    assert_eq!(r.u32().unwrap() & 0x1c, 0);
    // A share without networks is not in the root, and another network
    // reaches nothing.
    let answer = s.call(&mut server, 1000, &[op(op::PUTROOTFH, |_| {}), lookup("Private")]);
    assert_eq!(after_sequence(&answer).0, err::NOENT);
    let away: IpAddr = "192.0.2.9".parse().unwrap();
    assert!(server.reaches(CLIENT.parse().unwrap()));
    assert!(!server.reaches(away));
    let sequenced = s.sequence(false);
    let answer =
        compound(&mut server, &Context { peer: away, uid: 1000 }, &[sequenced, putfh(&docs_fh)]);
    assert_eq!(results(&answer).0, err::ACCESS);
}

#[test]
fn handles_last_through_a_restart() {
    let (music, docs) = dirs("restart");
    fs::create_dir_all(music.join("a/b")).unwrap();
    fs::write(music.join("a/b/c.txt"), b"deep").unwrap();
    let mut first = server(&music, &docs);
    let mut s = Session::new(&mut first, b"client");
    let share = share_fh(&mut first, &mut s, "Music");
    let ops = [putfh(&share), lookup("a"), lookup("b"), lookup("c.txt"), op(op::GETFH, |_| {})];
    let answer = s.call(&mut first, 1000, &ops);
    let (status, mut r) = after_sequence(&answer);
    assert_eq!(status, err::OK);
    for _ in 0..5 {
        next(&mut r);
    }
    let fh = r.opaque(128).unwrap().to_vec();
    // A new server has seen nothing: it looks for the file.
    let mut second = server(&music, &docs);
    let mut s = Session::new(&mut second, b"client");
    let read = op(op::READ, |w| {
        w.u32(0).fixed(&[0; 12]).u64(0).u32(10);
    });
    let answer = s.call(&mut second, 1000, &[putfh(&fh), read]);
    let (status, mut r) = after_sequence(&answer);
    assert_eq!(status, err::OK);
    next(&mut r);
    next(&mut r);
    assert_eq!((r.bool(), r.opaque(10)), (Ok(true), Ok(&b"deep"[..])));
    // Moved behind its back, it is found again; gone, it is stale.
    fs::rename(music.join("a"), music.join("z")).unwrap();
    let answer = s.call(&mut second, 1000, &[putfh(&fh)]);
    assert_eq!(after_sequence(&answer).0, err::OK);
    fs::remove_file(music.join("z/b/c.txt")).unwrap();
    let answer = s.call(&mut second, 1000, &[putfh(&fh)]);
    assert_eq!(after_sequence(&answer).0, err::STALE);
}

#[test]
fn root_and_calls_without_credentials_are_served_as_no_one() {
    let (music, docs) = dirs("nobody");
    let mut plain = server(&music, &docs);
    let mut s = Session::new(&mut plain, b"client");
    let server = Mutex::new(plain);
    let peer: IpAddr = CLIENT.parse().unwrap();
    let cases = [
        (rpc::AUTH_SYS, 0, rpc::NOBODY),
        (rpc::AUTH_NONE, 0, rpc::NOBODY),
        (rpc::AUTH_SYS, 1000, 1000),
    ];
    for (flavor, uid, served) in cases {
        let mut call = Writer::new();
        call.u32(5).u32(0).u32(2).u32(rpc::NFS_PROGRAM).u32(4).u32(1);
        if flavor == rpc::AUTH_SYS {
            let mut cred = Writer::new();
            cred.u32(0).opaque(b"client").u32(uid).u32(uid).u32(0);
            call.u32(rpc::AUTH_SYS).opaque(&cred.bytes);
        } else {
            call.u32(rpc::AUTH_NONE).u32(0);
        }
        call.u32(rpc::AUTH_NONE).u32(0);
        call.opaque(b"").u32(2).u32(3);
        for o in [s.sequence(false), op(op::PUTROOTFH, |_| {}), lookup("Music")] {
            call.bytes.extend_from_slice(&o);
        }
        server.lock().unwrap().backend.people.clear();
        let reply = crate::listener::answer(&server, peer, &call.bytes).unwrap();
        let mut r = Reader::new(&reply);
        // The xid, a reply, accepted, AUTH_NONE's verifier, SUCCESS, OK.
        let header: Vec<u32> = (0..7).map(|_| r.u32().unwrap()).collect();
        assert_eq!(header, [5, 1, 0, 0, 0, rpc::SUCCESS, err::OK]);
        let people = server.lock().unwrap().backend.people.clone();
        assert!(!people.is_empty() && people.iter().all(|&p| p == served), "{people:?}");
    }
    // Another program, and another version of NFS's.
    let mut call = Writer::new();
    call.u32(6).u32(0).u32(2).u32(100_005).u32(3).u32(0).u32(0).u32(0).u32(0).u32(0);
    let reply = crate::listener::answer(&server, peer, &call.bytes).unwrap();
    assert_eq!(&reply[20..24], &rpc::PROG_UNAVAIL.to_be_bytes());
    let mut call = Writer::new();
    call.u32(7).u32(0).u32(2).u32(rpc::NFS_PROGRAM).u32(3).u32(0).u32(0).u32(0).u32(0).u32(0);
    let reply = crate::listener::answer(&server, peer, &call.bytes).unwrap();
    assert_eq!(&reply[20..], [0, 0, 0, 2, 0, 0, 0, 4, 0, 0, 0, 4]);
}

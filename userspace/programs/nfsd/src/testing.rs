// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! nfsd for the tests of nfs(1), the client (feature `testing`): the
//! server, with a worker in the same process, as the user the tests run
//! as, on a port of the loopback; with Kerberos, under a keytab the
//! tests give.

use std::io;
use std::net::{SocketAddr, TcpListener};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;

use kerberos::keytab;

use crate::contexts::{Accounts, Contexts, Keys};
use crate::ipc::{Answer, Request};
use crate::listener;
use crate::server::{Backend, Server};
use crate::shares;
use crate::worker::Worker;

/// The worker, in the process.
struct Local(Worker);

impl Backend for Local {
    fn call(&mut self, _uid: u32, request: Request) -> Answer {
        self.0.serve(request)
    }

    fn restart(&mut self) {}
}

/// Serves `shares`, each a name and a folder, to the loopback, on a port
/// of its own: its address. Each connection is served by a thread, for
/// as long as the tests run.
///
/// # Errors
///
/// What binding the port, or reading the shares, failed with.
pub fn serve(shares: &[(&str, PathBuf)]) -> io::Result<SocketAddr> {
    let shares: Vec<_> =
        shares.iter().map(|(name, folder)| (*name, folder.clone(), "sys")).collect();
    serve_kerberos(&shares, Vec::new())
}

/// [`serve`], with each share taking the flavors its third part lists,
/// as `nfs-sec` would (`"krb5p"`, `"sys:krb5"`), and Kerberos's
/// contexts set up with `keytab`, alice being number 1000.
///
/// # Errors
///
/// What binding the port, or reading the shares, failed with.
pub fn serve_kerberos(
    shares: &[(&str, PathBuf, &str)],
    keytab: Vec<keytab::Entry>,
) -> io::Result<SocketAddr> {
    let mut text = String::new();
    for (name, folder, sec) in shares {
        let sec: Vec<String> = sec.split(':').map(|flavor| format!("\"{flavor}\"")).collect();
        text.push_str(&format!(
            "[[share]]\nname = \"{name}\"\nfolder = \"{}\"\nnfs = [\"127.0.0.0/8\"]\n\
             nfs-sec = [{}]\n",
            folder.display(),
            sec.join(", ")
        ));
    }
    let shares = shares::of_toml(&text).map_err(io::Error::other)?;
    let folders: Vec<PathBuf> = shares.iter().map(|share| share.folder.clone()).collect();
    let mut server = Server::new(Local(Worker::new(&folders)), 1, [7; 8], "testing");
    let accounts = Accounts::Given(vec![("alice".into(), 1000)]);
    server.contexts = Contexts::new([7; 4], Keys::Given(keytab), accounts);
    let server = Arc::new(Mutex::new(server.with_shares(shares)));
    let socket = TcpListener::bind("127.0.0.1:0")?;
    let addr = socket.local_addr()?;
    thread::spawn(move || {
        for stream in socket.incoming().flatten() {
            let server = Arc::clone(&server);
            thread::spawn(move || {
                if let Ok(from) = stream.peer_addr() {
                    let _ = listener::serve(&stream, from, &server);
                }
            });
        }
    });
    Ok(addr)
}

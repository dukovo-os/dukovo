// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! smb2's client, the one smb(1) is built on, against smbd over the
//! loopback: the listener's sign-in, with authd's NTLMv2 check done here,
//! then the worker serving a share in a folder of its own.

use std::fs;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use manifest::shares::{Share, Shares};
use smb2::{ClientConfig, SmbClient};

use crate::listener::{Finish, SignIn, Step};
use crate::wire;
use crate::worker::Worker;

/// The sharing password the tests sign in with.
const PASSWORD: &str = "correct horse battery staple";

/// Serves one connection as smbd would, with the share `Docs` at `root`
/// and authd's check of [`PASSWORD`] for alice made here.
fn serve_one(listener: &TcpListener, root: &Path) {
    let (stream, _) = listener.accept().unwrap();
    let hash = accounts::ntlm::nt_hash(PASSWORD.as_bytes()).unwrap();
    let mut sign_in = SignIn::new(
        "127.0.0.1".into(),
        [7; 16],
        "testbox",
        [1; 32],
        42,
        vec![[2; 8], [3; 8], [4; 8]],
    );
    loop {
        let Some(frame) = wire::read_frame_max(&mut &stream, 64 * 1024).unwrap() else { return };
        let pending = match sign_in.frame(&frame) {
            Step::Answer(response) if response.is_empty() => continue,
            Step::Answer(response) => {
                wire::write_frame(&mut &stream, &response).unwrap();
                continue;
            }
            Step::End(response, _) => {
                if let Some(response) = response {
                    let _ = wire::write_frame(&mut &stream, &response);
                }
                return;
            }
            Step::Check(pending) => pending,
        };
        let [negotiate, challenge, authenticate] = &pending.messages;
        let answer = accounts::ntlm::check(&hash, negotiate, challenge, authenticate)
            .map(|key| ("alice".to_string(), key))
            .map_err(|why| why.to_string());
        match sign_in.finish(&pending, answer) {
            Finish::Again(response) => wire::write_frame(&mut &stream, &response).unwrap(),
            Finish::End(response, _) => {
                let _ = wire::write_frame(&mut &stream, &response);
                return;
            }
            Finish::Accepted(handoff, response) => {
                let mut worker = Worker::new(*handoff);
                worker.catalog.shares = Shares {
                    shares: vec![Share {
                        name: "Docs".into(),
                        folder: root.to_string_lossy().into_owned(),
                        comment: "documents".into(),
                        read_only: false,
                        nfs: Vec::new(),
                    }],
                };
                wire::write_frame(&mut &stream, &response).unwrap();
                let _ = worker.serve(&stream);
                return;
            }
        }
    }
}

/// A folder of its own for a test.
fn folder(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("smbd-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::canonicalize(&dir).unwrap()
}

/// smbd on a port of the loopback, for `connections` connections.
fn start(root: &Path, connections: usize) -> String {
    serve_on(TcpListener::bind("127.0.0.1:0").unwrap(), root, connections)
}

/// smbd on `listener`, for `connections` connections.
fn serve_on(listener: TcpListener, root: &Path, connections: usize) -> String {
    let addr = listener.local_addr().unwrap().to_string();
    let root = root.to_path_buf();
    thread::spawn(move || {
        for _ in 0..connections {
            serve_one(&listener, &root);
        }
    });
    addr
}

fn config(addr: &str, password: &str) -> ClientConfig {
    ClientConfig {
        addr: addr.into(),
        timeout: Duration::from_secs(10),
        username: "alice".into(),
        password: password.into(),
        domain: String::new(),
        auto_reconnect: false,
        dfs_enabled: false,
        ..ClientConfig::default()
    }
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap()
}

#[test]
fn a_client_signs_in_and_uses_a_share() {
    let root = folder("share");
    fs::write(root.join("Notes.TXT"), b"kept on the server").unwrap();
    let addr = start(&root, 1);
    let rt = runtime();
    let mut client = rt.block_on(SmbClient::connect(config(&addr, PASSWORD))).unwrap();
    let shares = rt.block_on(client.list_shares()).unwrap();
    let names: Vec<&str> = shares.iter().map(|share| share.name.as_str()).collect();
    assert!(names.contains(&"Docs"), "{names:?}");
    let mut tree = rt.block_on(client.connect_share("docs")).unwrap();

    // Read in any case, written, listed.
    assert_eq!(
        rt.block_on(client.read_file(&mut tree, "notes.txt")).unwrap(),
        b"kept on the server"
    );
    rt.block_on(client.write_file(&tree, "hello.txt", b"hello from a PC")).unwrap();
    assert_eq!(fs::read(root.join("hello.txt")).unwrap(), b"hello from a PC");
    let mut listed: Vec<String> = rt
        .block_on(client.list_directory(&mut tree, ""))
        .unwrap()
        .into_iter()
        .map(|entry| entry.name)
        .collect();
    listed.sort();
    // `.` and `..` first, as Windows lists them too.
    assert_eq!(listed, [".", "..", "Notes.TXT", "hello.txt"]);

    // Folders, renames, sizes and deletes.
    rt.block_on(client.create_directory(&tree, "Folder")).unwrap();
    assert!(root.join("Folder").is_dir());
    rt.block_on(client.rename(&tree, "hello.txt", "Folder/hi.txt")).unwrap();
    assert!(root.join("Folder/hi.txt").is_file());
    let info = rt.block_on(client.stat(&mut tree, "Folder/hi.txt")).unwrap();
    assert_eq!(info.size, 15);
    rt.block_on(client.delete_file(&tree, "Folder/hi.txt")).unwrap();
    rt.block_on(client.delete_directory(&tree, "Folder")).unwrap();
    assert!(!root.join("Folder").exists());

    // Many megabytes, in pieces of the largest size, both ways.
    let big: Vec<u8> = (0..9 * 1024 * 1024 + 123).map(|i: u32| (i % 251) as u8).collect();
    rt.block_on(client.write_file_pipelined(&tree, "big.bin", &big)).unwrap();
    assert_eq!(fs::read(root.join("big.bin")).unwrap(), big);
    assert_eq!(rt.block_on(client.read_file_pipelined(&mut tree, "big.bin")).unwrap(), big);

    // The share stops at its folder.
    assert!(rt.block_on(client.read_file(&mut tree, "../outside")).is_err());
    let space = rt.block_on(client.fs_info(&mut tree)).unwrap();
    assert!(space.total_bytes > 0);
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn a_wrong_password_is_refused() {
    let root = folder("refused");
    let addr = start(&root, 1);
    let rt = runtime();
    let refused = rt.block_on(SmbClient::connect(config(&addr, "not the password")));
    assert!(refused.is_err());
    fs::remove_dir_all(&root).unwrap();
}

/// Runs Samba's smbclient, if the machine has it, with `args`: its
/// output, or `None` without it.
fn smbclient(args: &[&str]) -> Option<std::process::Output> {
    std::process::Command::new("smbclient").args(args).output().ok()
}

#[test]
fn sambas_smbclient_lists_and_copies() {
    if smbclient(&["--version"]).is_none() {
        eprintln!("no smbclient here: the boot test runs it");
        return;
    }
    let root = folder("smbclient");
    fs::write(root.join("from-server.txt"), b"served\n").unwrap();
    let local = folder("smbclient-local");
    fs::write(local.join("from-pc.txt"), b"uploaded\n").unwrap();
    let addr = start(&root, 2);
    let (host, port) = addr.split_once(':').unwrap();
    let user = format!("alice%{PASSWORD}");
    let common = ["-p", port, "-U", &user, "-m", "SMB3", "--client-protection=encrypt"];
    let listed = smbclient(&[&["-L", host], &common[..]].concat()).unwrap();
    let text = String::from_utf8_lossy(&listed.stdout);
    assert!(
        listed.status.success() && text.contains("Docs"),
        "{text}{}",
        String::from_utf8_lossy(&listed.stderr)
    );
    let share = format!("//{host}/Docs");
    let commands =
        format!("lcd {}; put from-pc.txt; get from-server.txt; mkdir sub; ls", local.display());
    let copied = smbclient(&[&[share.as_str()], &common[..], &["-c", &commands]].concat()).unwrap();
    let text = String::from_utf8_lossy(&copied.stdout);
    assert!(copied.status.success(), "{text}{}", String::from_utf8_lossy(&copied.stderr));
    assert_eq!(fs::read(root.join("from-pc.txt")).unwrap(), b"uploaded\n");
    assert_eq!(fs::read(local.join("from-server.txt")).unwrap(), b"served\n");
    assert!(root.join("sub").is_dir());
    fs::remove_dir_all(&root).unwrap();
    fs::remove_dir_all(&local).unwrap();
}

/// Runs Samba's smbcacls with `args` on `path` in the share at `host`:
/// its output and whether it succeeded.
fn smbcacls(host: &str, path: &str, args: &[&str]) -> (String, bool) {
    let share = format!("//{host}/Docs");
    let user = format!("alice%{PASSWORD}");
    let common = ["-U", &user, "-m", "SMB3", "--client-protection=encrypt"];
    let ran = std::process::Command::new("smbcacls")
        .args([share.as_str(), path])
        .args(common)
        .args(args)
        .output()
        .unwrap();
    let text =
        format!("{}{}", String::from_utf8_lossy(&ran.stdout), String::from_utf8_lossy(&ran.stderr));
    (text, ran.status.success())
}

#[test]
fn sambas_smbcacls_reads_and_sets_access_lists() {
    use acl::{NO_ID, Tag};
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    if std::process::Command::new("smbcacls").arg("--version").output().is_err() {
        eprintln!("no smbcacls here: the boot test runs it");
        return;
    }
    let Some(nobody) = crate::sys::user_id("nobody") else {
        eprintln!("no user nobody here to name in a list");
        return;
    };
    let root = folder("smbcacls");
    let file = root.join("file.txt");
    fs::write(&file, b"listed\n").unwrap();
    fs::set_permissions(&file, fs::Permissions::from_mode(0o640)).unwrap();
    fs::create_dir(root.join("sub")).unwrap();
    let meta = fs::metadata(&file).unwrap();
    let me = crate::sys::user_name(meta.uid()).unwrap();
    let my_group = crate::sys::group_name(meta.gid()).unwrap();
    // smbcacls has no -p: it reaches port 445 alone, which CI lets the
    // tests listen on (ci.yml).
    let listener = match TcpListener::bind("127.0.0.1:445") {
        Ok(listener) => listener,
        Err(err) if std::env::var_os("GITHUB_ACTIONS").is_none() => {
            eprintln!("smbcacls needs port 445, which this test may not have: {err}");
            return;
        }
        Err(err) => panic!("port 445: {err}"),
    };
    let addr = serve_on(listener, &root, 6);
    let (host, _) = addr.split_once(':').unwrap();

    // The owner, the group and the mode's three ACEs, named over lsarpc.
    let (text, ok) = smbcacls(host, "file.txt", &[]);
    assert!(ok, "{text}");
    assert!(text.contains(&format!("OWNER:Unix User\\{me}")), "{text}");
    assert!(text.contains(&format!("GROUP:Unix Group\\{my_group}")), "{text}");
    assert!(text.contains(&format!("ACL:Unix Group\\{my_group}:ALLOWED/0x0/R\n")), "{text}");
    assert!(text.contains("ACL:Everyone:ALLOWED/0x0/"), "{text}");

    // An ACE for nobody, by name: a named entry and a mask.
    let (text, ok) = smbcacls(host, "file.txt", &["-a", "ACL:Unix User\\nobody:ALLOWED/0/R"]);
    assert!(ok, "{text}");
    let list = acl::access_list(&file, fs::metadata(&file).unwrap().mode(), false).unwrap();
    assert_eq!(list.get(Tag::User, nobody).map(|e| e.perm), Some(4), "{list:?}");
    assert_eq!(list.mask(), Some(4));
    assert_eq!(list.get(Tag::UserObj, NO_ID).map(|e| e.perm), Some(6));
    let (text, ok) = smbcacls(host, "file.txt", &[]);
    assert!(ok && text.contains("ACL:Unix User\\nobody:ALLOWED/0x0/R\n"), "{text}");

    // What a list cannot keep is refused, and nothing changes.
    let (text, ok) = smbcacls(host, "file.txt", &["-a", "ACL:Everyone:DENIED/0/R"]);
    assert!(!ok && text.contains("NT_STATUS_NOT_SUPPORTED"), "{text}");
    let (text, ok) = smbcacls(host, "file.txt", &["-a", "ACL:S-1-5-32-544:ALLOWED/0/FULL"]);
    assert!(!ok && text.contains("NT_STATUS_NONE_MAPPED"), "{text}");
    assert_eq!(acl::access_list(&file, 0, false).unwrap(), list);

    // An ACE that inherits gives a folder a default list.
    let (text, ok) = smbcacls(host, "sub", &["-a", "ACL:Unix User\\nobody:ALLOWED/3/READ"]);
    assert!(ok, "{text}");
    let default = acl::default_list(&root.join("sub"), false).unwrap().unwrap();
    assert_eq!(default.get(Tag::User, nobody).map(|e| e.perm), Some(5), "{default:?}");
    assert_eq!(default.get(Tag::Other, NO_ID).map(|e| e.perm), Some(0));
    fs::remove_dir_all(&root).unwrap();
}

// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Problem reports (ADR 0531): reportd(8) keeps a record of each crash, and
//! report(1) sends the person's report of it, once they have seen the whole
//! text, to a reports server. An installed system's disk boots under UEFI
//! and alice, the owner, signs in. The server is the test's own staging
//! server (lab/tests/reports/report-server.py), run on the host in HTTPS
//! with a certificate from an authority made here with openssl, which the
//! machine reaches at 10.0.2.2; report.toml(5) names the server, the
//! authority that signed its certificate and the security box's recipient.
//! *crash-test*, the test's program (userspace/programs/report-test), is run
//! by two services of the test's: one logs a line that names alice and an
//! address, then ends with SIGSEGV in three functions of its own, and the
//! other panics.
//!
//! The crash is kept with its functions' names: *report list* has them, the
//! log has a warning, and the next sign-in's notice names the crash once
//! though it happens twice. *report crash* asks its three questions and
//! shows the text, which has marks for the user name and the address and
//! the three functions in order. What the server keeps is byte for byte what
//! *--save* writes, and *report status* says what the server says: received,
//! then fixed in 0.16.1. *report delete* takes the report off the server.
//! With the server stopped the report is queued and listed as waiting, and
//! *report retry* sends it once the server is back. *--security* sends only
//! age's ciphertext, which holds none of the words. *--save* writes the text
//! at mode 600, and without a file it prints an address with a QR code,
//! which the host fetches once through a port QEMU forwards; the second
//! fetch is refused. The service that panics is kept with its panic's
//! location.
//!
//! The power button ends it. problem-report-access, on the same set-up,
//! tests the failure notice's keys, what a person who is no administrator
//! sees, and the kernel's records with reportd stopped.

use std::fs;
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use sha2::{Digest, Sha256};

use super::super::*;

pub const CASE: Case = Case {
    name: "problem-report",
    about: "problem reports (ADR 0531): a service of the test's crashes in three named functions and a second one panics, and reportd keeps each, lists the crash, warns in the log and offers it once at the next sign-in; report crash shows the text with the user name and address as marks, sends exactly what --save writes to the host's staging server over TLS, and report status reads the server's changes; report delete removes it; with the server away the report waits and report retry sends it; --security sends age's ciphertext alone; --save writes the text at mode 600 and prints an address and QR code that the host fetches once",
    firmware: Firmware::Uefi,
    media: Media::Disk,
    cmdline: "",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Installed,
    cpu_model: None,
    qemu: &["-netdev", "user,id=n0", "-device", "virtio-net-pci,netdev=n0"],
    script: problem_report,
    disks: &[],
    second: None,
};

/// alice's password, which elevate asks for once at each sign-in.
pub(super) const PASSWORD: &str = "queen of hearts";

/// Where the test keeps its files on the host, from the top of the tree.
const DIR: &str = "out/test/problem-report";

/// The staging server, from the top of the tree.
const SERVER: &str = "lab/tests/reports/report-server.py";

/// The security box's recipient: the age project's example key, whose
/// secret half nobody needs, as the test reads only the ciphertext, and the
/// file on the machine that report.toml(5) names it by.
const RECIPIENT: &str = "age1ql3z7hjy54pw3hyww5ayyfg7zqgvc7w3j2elw8zmrj2kg5sfn9aqmcac8p";
const RECIPIENTS: &str = "/system/settings/report-security.txt";

/// The authority's certificate on the machine, which report.toml(5) names
/// as the one that signed the server's.
const CA: &str = "/system/settings/ssl/report-test-ca.pem";

/// The port the one-time download listens on, which the firewall is told
/// to allow first, as a program a person runs listens for the machine
/// alone, and which QEMU forwards a port of the host's loopback to.
const DOWNLOAD_PORT: u16 = 9443;

/// The line the crashing service logs before it crashes: a user name and
/// an address, which the text of the report must show as marks.
const LOGGED: &str = "alice signed in from 192.168.7.8";

/// What report asks, and what it offers once it has shown the text.
pub(super) const QUESTIONS: [&str; 3] =
    ["What happened?", "What did you expect?", "How can it be made to happen again?"];
pub(super) const CHOICES: &str = "C to cancel";

/// How long a report may take to reach the server or to leave it, and a
/// crash to be kept, with the time to read the screen, in seconds.
pub(super) const SEND_SECONDS: u64 = 60;
pub(super) const CRASH_SECONDS: u64 = 150;

/// Each report's three answers; the last two may be empty.
const SENT: [&str; 3] = ["alice's crash-test stopped", "It should keep running.", ""];
const WAITING: [&str; 3] = ["A report sent while the server is away", "", ""];
const BOXED: [&str; 3] = ["BOXED-WORDS stay in the box", "", ""];
const PANICKED: [&str; 3] = ["The service panicked", "", ""];

/// What the shell is told first, as functions for alice's and bob's shells,
/// written by the test so that it types little: what reportd answers a
/// person who asks over its socket, and a crash made by a service and
/// waited for until reportd has kept it, which its log's warning says. A
/// crash like one kept and not yet reported is counted on that one, so the
/// list of crashes need not grow.
const SHELL_FUNCTIONS: &str = r#"c=/system/tests/report/crash-test
export PAGER=cat
crashes() { $c ask '{"ask":"crashes"}'; }
queue() { $c ask '{"ask":"queue"}'; }
service_draft() { $c ask '{"ask":"draft","about":{"kind":"service","name":"crash-test"}}'; }
kept() { elevate log -s reportd -p warning | grep -c 'reports i[t]'; }
crash() {
	n=$(kept)
	elevate service $1 restart --no-wait -q
	i=0
	while [ $i -lt 90 ] && [ "$(kept)" -le "$n" ]; do
		sleep 1
		i=$((i + 1))
	done
	[ "$(kept)" -gt "$n" ] && echo "kept $2 $3"
}
"#;

/// The test's two services, as Spark reads them. The first logs a line and
/// then becomes the crashing program, so that the process that crashes is
/// the service's own; the shell runs it, so the service reads its folder.
fn service_files() -> [(&'static str, String); 2] {
    let crashing = format!(
        "service_version = 1\ndescription = \"Crashes on purpose\"\nprogram = \
         \"/system/commands/sh\"\narguments = [\"-c\", \"echo {LOGGED}; exec \
         /system/tests/report/crash-test segv\"]\nrestart = \"never\"\n[permissions]\n\
         files.read = [\"/system/tests/report\"]\n"
    );
    let panicking = "service_version = 1\ndescription = \"Panics on purpose\"\nprogram = \
                     \"/system/tests/report/crash-test\"\narguments = [\"panic\"]\nrestart = \
                     \"never\"\n"
        .to_string();
    [("crash-test", crashing), ("crash-panic", panicking)]
}

/// What the machine fetches and runs as root to set itself up: it names in
/// report.toml(5) the server, the authority that signed its certificate and
/// the file of the security box's recipient, and adds the two services.
/// reportd reads its settings again once it is restarted.
fn setup_script(port: u16, files_port: u16) -> String {
    let mut lines = vec![
        "set -e".to_string(),
        format!("curl -sf http://10.0.2.2:{files_port}/ca.pem -o {CA}"),
        format!("echo {RECIPIENT} > {RECIPIENTS}"),
        format!(
            "printf '%s\\n' 'server = \"https://10.0.2.2:{port}\"' 'ca = [\"{CA}\"]' \
             'security-recipients = \"{RECIPIENTS}\"' > /system/settings/report.toml"
        ),
    ];
    for (name, text) in service_files() {
        lines.push(format!("printf '%s' '{text}' > /system/settings/spark/services/{name}.toml"));
    }
    lines.push("service reportd restart".to_string());
    lines.join("\n") + "\n"
}

/// The staging server and a file server beside it, on the host's loopback,
/// which the machine reaches as 10.0.2.2. Both end with it.
pub(super) struct Host {
    dir: PathBuf,
    port: u16,
    pub(super) files_port: u16,
    server: Option<Child>,
    files: Child,
}

impl Drop for Host {
    fn drop(&mut self) {
        self.stop_server();
        let _ = self.files.kill();
        let _ = self.files.wait();
    }
}

/// A port of the host's loopback that nothing listens on.
fn free_port() -> Result<u16, String> {
    std::net::TcpListener::bind("127.0.0.1:0")
        .and_then(|listener| listener.local_addr())
        .map(|address| address.port())
        .map_err(|err| format!("no free port: {err}"))
}

/// Waits until `ready` holds, for up to `seconds`.
fn within(seconds: u64, what: &str, ready: impl Fn() -> bool) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(seconds);
    while !ready() {
        if Instant::now() >= deadline {
            return Err(format!("{what} did not happen within {seconds} s"));
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    Ok(())
}

/// Runs openssl in `dir`.
fn openssl(dir: &Path, args: &[&str]) -> Result<(), String> {
    host_tool(Command::new("openssl").current_dir(dir).args(args)).map(|_| ())
}

/// Makes, in `dir`, an authority of the test's own and a certificate it
/// signs for 10.0.2.2, which is how the machine names the host. The keys
/// are P-256, as quick to make as any, good for two days, and protect
/// nothing.
fn authority(dir: &Path) -> Result<(), String> {
    for key in ["ca.key", "server.key"] {
        openssl(
            dir,
            &["genpkey", "-algorithm", "EC", "-pkeyopt", "ec_paramgen_curve:P-256", "-out", key],
        )?;
    }
    openssl(
        dir,
        &[
            "req",
            "-x509",
            "-new",
            "-key",
            "ca.key",
            "-out",
            "ca.pem",
            "-days",
            "2",
            "-subj",
            "/CN=Report test authority",
            "-addext",
            "basicConstraints=critical,CA:TRUE",
            "-addext",
            "keyUsage=critical,keyCertSign",
        ],
    )?;
    let extensions = "basicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature\n\
                      extendedKeyUsage=serverAuth\nsubjectAltName=IP:10.0.2.2\n";
    fs::write(dir.join("server.ext"), extensions).map_err(|err| format!("server.ext: {err}"))?;
    let request = ["req", "-new", "-key", "server.key", "-subj", "/CN=10.0.2.2"];
    openssl(dir, &[&request[..], &["-out", "server.csr"]].concat())?;
    let signing = ["x509", "-req", "-in", "server.csr", "-CA", "ca.pem", "-CAkey", "ca.key"];
    openssl(
        dir,
        &[
            &signing[..],
            &["-CAcreateserial", "-days", "2", "-extfile", "server.ext", "-out", "server.pem"],
        ]
        .concat(),
    )?;
    Ok(())
}

impl Host {
    /// Makes the authority, the files the machine fetches to set itself up,
    /// and starts both servers, keeping their files in `dir`, from the top
    /// of the tree.
    pub(super) fn start(dir: &str) -> Result<Host, String> {
        let root = std::env::current_dir().map_err(|err| err.to_string())?;
        let dir = root.join(dir);
        if dir.exists() {
            fs::remove_dir_all(&dir).map_err(|err| format!("removing {}: {err}", dir.display()))?;
        }
        let web = dir.join("web");
        fs::create_dir_all(&web).map_err(|err| format!("making {}: {err}", web.display()))?;
        authority(&dir)?;
        let (port, files_port) = (free_port()?, free_port()?);
        fs::copy(dir.join("ca.pem"), web.join("ca.pem")).map_err(|err| format!("ca.pem: {err}"))?;
        for (name, text) in
            [("setup.sh", setup_script(port, files_port)), ("functions.sh", SHELL_FUNCTIONS.into())]
        {
            fs::write(web.join(name), text).map_err(|err| format!("{name}: {err}"))?;
        }
        let log = fs::File::create(dir.join("files.log")).map_err(|err| err.to_string())?;
        let files = Command::new("python3")
            .args(["-I", "-m", "http.server", &files_port.to_string(), "--bind", "127.0.0.1"])
            .arg("--directory")
            .arg(&web)
            .stdin(Stdio::null())
            .stdout(log.try_clone().map_err(|err| err.to_string())?)
            .stderr(log)
            .spawn()
            .map_err(|err| format!("starting the file server: {err}"))?;
        let mut host = Host { dir, port, files_port, server: None, files };
        within(10, "the file server's listening", || {
            TcpStream::connect(("127.0.0.1", files_port)).is_ok()
        })?;
        host.start_server()?;
        Ok(host)
    }

    fn reports(&self) -> PathBuf {
        self.dir.join("reports")
    }

    /// Runs the staging server, which keeps its reports and their numbers
    /// from one run to the next, and waits until it says it listens.
    fn start_server(&mut self) -> Result<(), String> {
        let log = self.dir.join("server.log");
        let before = ready_lines(&log);
        let output = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log)
            .map_err(|err| format!("opening {}: {err}", log.display()))?;
        let child = Command::new("python3")
            .args(["-I", SERVER, "serve", "--bind", "127.0.0.1", "--port", &self.port.to_string()])
            .arg("--cert")
            .arg(self.dir.join("server.pem"))
            .arg("--key")
            .arg(self.dir.join("server.key"))
            .arg("--dir")
            .arg(self.reports())
            .stdin(Stdio::null())
            .stdout(output.try_clone().map_err(|err| err.to_string())?)
            .stderr(output)
            .spawn()
            .map_err(|err| format!("starting the staging server: {err}"))?;
        self.server = Some(child);
        within(10, "the staging server's readiness", || ready_lines(&log) > before)
    }

    fn stop_server(&mut self) {
        if let Some(mut server) = self.server.take() {
            let _ = server.kill();
            let _ = server.wait();
        }
    }

    /// The text the server kept of report `number`, once it has come.
    fn report(&self, number: u32) -> Result<Vec<u8>, String> {
        let record = self.reports().join(format!("{number}.json"));
        within(SEND_SECONDS, &format!("the server's keeping report {number}"), || record.exists())?;
        let kept = ["txt", "age"]
            .map(|kind| self.reports().join(format!("{number}.{kind}")))
            .into_iter()
            .find(|path| path.exists())
            .ok_or_else(|| format!("the server has no file of report {number}"))?;
        fs::read(&kept).map_err(|err| format!("{}: {err}", kept.display()))
    }

    /// Waits until the server has no report `number`.
    fn forgets(&self, number: u32) -> Result<(), String> {
        let record = self.reports().join(format!("{number}.json"));
        within(SEND_SECONDS, &format!("the server's deleting report {number}"), || !record.exists())
    }

    /// Has the staging server's `set-status` change report `number`.
    fn fixed(&self, number: u32, version: &str) -> Result<(), String> {
        let mut set = Command::new("python3");
        set.args(["-I", SERVER, "set-status"]).arg(self.reports());
        host_tool(set.args([&number.to_string(), "fixed", "--fixed-in", version])).map(|_| ())
    }
}

/// How many times the staging server has said it listens, in its log.
fn ready_lines(log: &Path) -> usize {
    fs::read_to_string(log).unwrap_or_default().matches("report server ready").count()
}

/// Whether `bytes` hold `word`.
fn holds(bytes: &[u8], word: &str) -> bool {
    bytes.windows(word.len()).any(|window| window == word.as_bytes())
}

/// Waits until the screen shows each of `needles`, for up to `seconds`.
pub(super) fn shown_within(vm: &mut Vm, needles: &[&str], seconds: u64) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(seconds);
    loop {
        match vm.expect_screen(needles) {
            Err(_) if Instant::now() < deadline => {}
            done => return done,
        }
    }
}

/// Types `command` at the prompt, on a cleared screen, and checks that the
/// screen shows each of `shown` once it has run: up to `seconds`. Each of
/// `shown` is output, never in the command itself, so that the command's
/// own echo is not taken for its answer.
pub(super) fn run(vm: &mut Vm, command: &str, seconds: u64, shown: &[&str]) -> Result<(), String> {
    answer_prompt(vm, "$", &format!("clear; {command}"))?;
    shown_within(vm, shown, seconds).map_err(|err| format!("`{command}`: {err}"))
}

/// Waits until the last line with anything on it ends with `end`, as
/// [`answer_prompt`] does before it types, but types nothing: for a key
/// that is one press, not a line.
pub(super) fn wait_for_end(vm: &mut Vm, end: &str, seconds: u64) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(seconds);
    loop {
        let screen = vm.screen()?;
        // The cursor's block, after the text, reads as `?`.
        let last = screen
            .iter()
            .rev()
            .map(|line| line.trim_end().trim_end_matches('?').trim_end())
            .find(|line| !line.is_empty());
        if last.is_some_and(|line| line.ends_with(end)) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(format!("the screen does not end with \"{end}\"; its last line: {last:?}"));
        }
        std::thread::sleep(Duration::from_millis(300));
    }
}

/// Runs `report args`, answers its three questions (an empty answer is
/// skipped with Enter, any other ends with the empty line report asks for),
/// and, if there is a `key`, presses it once report has shown the text and
/// what to press.
pub(super) fn report(vm: &mut Vm, args: &str, answers: [&str; 3], key: Option<&str>) -> Result<(), String> {
    answer_prompt(vm, "$", &format!("clear; report {args}"))?;
    for (question, answer) in QUESTIONS.into_iter().zip(answers) {
        shown_within(vm, &[question], SEND_SECONDS)
            .map_err(|err| format!("report {args}: {err}"))?;
        let typed = if answer.is_empty() { "\n".to_string() } else { format!("{answer}\n\n") };
        vm.type_text(&typed)?;
    }
    let Some(key) = key else { return Ok(()) };
    shown_within(vm, &[CHOICES], SEND_SECONDS).map_err(|err| format!("report {args}: {err}"))?;
    vm.type_text(key)
}

/// The 64 hexadecimal digits that follow `label ` on the screen, as `echo
/// "label $(sha256sum FILE | cut -c1-64)"` shows them: the command line the
/// shell echoes holds no such digits.
fn hash_after(vm: &mut Vm, label: &str) -> Result<String, String> {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let screen = vm.screen()?;
        let found = screen.iter().find_map(|line| {
            let digits = line.split_once(&format!("{label} "))?.1.get(..64)?;
            digits.bytes().all(|b| b.is_ascii_hexdigit()).then(|| digits.to_string())
        });
        match found {
            Some(hash) => return Ok(hash),
            None if Instant::now() >= deadline => {
                return Err(format!("the screen shows no hash after \"{label}\""));
            }
            None => std::thread::sleep(Duration::from_millis(500)),
        }
    }
}

/// The rows of the screen that a QR code of half blocks fills: each block
/// reads as `?`, as no glyph of the console's font matches it.
fn qr_rows(screen: &[String]) -> usize {
    screen.iter().filter(|line| line.matches('?').count() >= 10).count()
}

/// Checks that the screen shows a QR code, which takes at least 10 rows
/// of blocks even for the shortest address.
pub(super) fn expect_qr_code(vm: &mut Vm, what: &str) -> Result<(), String> {
    let rows = qr_rows(&vm.screen()?);
    if rows < 10 {
        return Err(format!("{what}: the screen shows {rows} rows of blocks, not a QR code"));
    }
    Ok(())
}

fn problem_report(vm: &mut Vm, _case: &Case) -> Result<(), String> {
    let mut host = Host::start(DIR)?;
    owner_signs_in(vm)?;
    set_up(vm, &host)?;
    a_crash_is_kept_and_offered_once(vm)?;
    a_report_is_sent_and_its_status_read(vm, &host)?;
    a_report_waits_for_the_server(vm, &mut host)?;
    the_security_box_holds_ciphertext(vm, &host)?;
    a_report_is_saved_and_downloaded_once(vm)?;
    a_panic_is_kept(vm)?;

    vm.press_power_button()?;
    vm.expect("spark: every service has stopped")?;
    vm.expect("Power: `spark` asked to power off the machine; syncing the filesystems")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}

/// The machine has no account yet: the owner's comes first, as in the
/// monitoring test. reportd is among the services Spark starts.
pub(super) fn owner_signs_in(vm: &mut Vm) -> Result<(), String> {
    vm.expect("Boot complete.")?;
    vm.expect_anywhere("spark: started reportd (")?;
    vm.expect_anywhere("authd: 0 accounts; answering at /system/live/authd/socket")?;
    vm.expect_anywhere("consoled: started /system/commands/login on tty1 as process ")?;
    vm.expect_screen(&[
        &format!("{RELEASE} {VERSION}"),
        "This computer has no account yet: make yours, the owner's,",
    ])?;
    answer_prompt(vm, "Name:", "alice")?;
    answer_prompt(vm, "Full name (may be empty):", "Alice Liddell")?;
    answer_prompt(vm, "New password:", PASSWORD)?;
    answer_prompt(vm, "The same again:", PASSWORD)?;
    vm.expect_anywhere("authd: alice signed in on pts/")?;
    vm.expect_screen(&["Made alice, number 1000, home /users/alice"])
}

/// The machine fetches the host's two scripts over plain HTTP, once its
/// network is up. As an administrator, through elevate, which asks for her
/// password once, it names the staging server and its authority in
/// report.toml and adds the two services; reportd is restarted to read it.
pub(super) fn set_up(vm: &mut Vm, host: &Host) -> Result<(), String> {
    vm.expect_anywhere("netd: net0 has 10.0.2.15/24")?;
    let fetch = |script: &str| format!("curl -sf http://10.0.2.2:{}/{script}", host.files_port);
    answer_prompt(
        vm,
        "$",
        &format!(
            "clear; {} -o setup.sh; elevate sh setup.sh; echo \"setup $?\"",
            fetch("setup.sh")
        ),
    )?;
    answer_prompt(vm, "setup.sh:", PASSWORD)?;
    shown_within(vm, &["setup 0"], 120).map_err(|err| format!("the setup script: {err}"))?;
    let functions = format!(
        "{} -o functions.sh; . ./functions.sh; echo \"functions $((6 * 7))\"",
        fetch("functions.sh")
    );
    run(vm, &functions, 30, &["functions 42"])
}

/// The service crashes in `fault_here`, which `middle` calls and `outer`
/// calls. reportd keeps the crash, which `report list` has by the function's
/// name, warns in the log, and offers it at the next sign-in. The service
/// crashes a second time the same way, which is warned of but not offered
/// again.
fn a_crash_is_kept_and_offered_once(vm: &mut Vm) -> Result<(), String> {
    run(
        vm,
        "crash crash-test signal 1; report list | grep -q 'fault_[h]ere' && echo \"listed \
         $((6 * 7))\"",
        CRASH_SECONDS,
        &["kept signal 1", "listed 42"],
    )?;
    run(
        vm,
        "elevate cat /system/live/sign-in/reportd; echo \"warned $(elevate log -s reportd -p \
         warning | grep -c 'SIGSEG[V]')\"",
        30,
        &["the service crash-test stopped at", "fault_here", "warned 1"],
    )?;
    run(
        vm,
        "crash crash-test signal 2; echo \"offers $(elevate cat /system/live/sign-in/reportd | \
         grep -c 'stopped a[t]') warned $(elevate log -s reportd -p warning | \
         grep -c 'SIGSEG[V]')\"",
        CRASH_SECONDS,
        &["kept signal 2", "offers 1 warned 2"],
    )
}

/// `report crash --save` writes the text first, and `report crash` sends
/// it: the server's file holds the same bytes, which show marks where the
/// user name and the address were and the three functions in order, and
/// none of what they hid. `report status` says what the server does, as it
/// changes, and `report delete` takes the report off the server.
fn a_report_is_sent_and_its_status_read(vm: &mut Vm, host: &Host) -> Result<(), String> {
    report(vm, "crash --save /tmp/saved.txt", SENT, None)?;
    answer_prompt(vm, "$", "clear; echo \"saved $(sha256sum /tmp/saved.txt | cut -c1-64)\"")?;
    let saved = hash_after(vm, "saved")?;
    report(vm, "crash", SENT, Some("S"))?;
    let sent = host.report(1)?;
    if format!("{:x}", Sha256::digest(&sent)) != saved {
        return Err("the server's report is not the bytes report --save wrote".into());
    }
    let text = String::from_utf8_lossy(&sent);
    if !text.starts_with("Dukovo problem report\n") {
        return Err(format!("the server's report does not start as a report does: {text}"));
    }
    for marked in ["[user] signed in from [IPv4]", "[user]'s crash-test stopped"] {
        if !text.contains(marked) {
            return Err(format!("the server's report lacks \"{marked}\": {text}"));
        }
    }
    for hidden in ["alice", "192.168.7.8"] {
        if text.contains(hidden) {
            return Err(format!("the server's report holds \"{hidden}\": {text}"));
        }
    }
    let crash = text.split("== Crash").nth(1).unwrap_or_default();
    let frames: Vec<Option<usize>> =
        ["fault_here", "middle", "outer"].map(|function| crash.find(function)).to_vec();
    if frames.iter().any(Option::is_none) || !frames.is_sorted() {
        return Err(format!(
            "the report's crash has not fault_here, middle and outer in order: {text}"
        ));
    }

    run(vm, "report status", SEND_SECONDS, &["Received"])?;
    host.fixed(1, "0.16.1")?;
    run(vm, "report status", SEND_SECONDS, &["Fixed in 0.16.1"])?;
    answer_prompt(vm, "$", "clear; report delete 1")?;
    host.forgets(1)
}

/// The service crashes again, a crash of its own now that the first is
/// reported. With the server stopped its report is queued, not lost: it is
/// listed as waiting, and `report retry` sends it once the server is back.
/// The server numbers it 2, as it does not use a number again.
fn a_report_waits_for_the_server(vm: &mut Vm, host: &mut Host) -> Result<(), String> {
    run(vm, "crash crash-test signal 3", CRASH_SECONDS, &["kept signal 3"])?;
    host.stop_server();
    report(vm, "crash", WAITING, Some("S"))?;
    run(
        vm,
        "echo \"waiting $(queue | grep -c tries) listed $(report list | grep -c 'while the server \
         is awa[y]')\"",
        SEND_SECONDS,
        &["waiting 1 listed 1"],
    )?;
    host.start_server()?;
    answer_prompt(vm, "$", "clear; report retry")?;
    let sent = host.report(2)?;
    if !holds(&sent, "A report sent while the server is away") {
        return Err("the retried report is not the one that waited".into());
    }
    run(vm, "echo \"waiting $(queue | grep -c tries)\"", SEND_SECONDS, &["waiting 0"])
}

/// A fourth crash, reported with `--security`: the server's file is age's
/// ciphertext, which holds none of the words the report had.
fn the_security_box_holds_ciphertext(vm: &mut Vm, host: &Host) -> Result<(), String> {
    run(vm, "crash crash-test signal 4", CRASH_SECONDS, &["kept signal 4"])?;
    report(vm, "crash --security", BOXED, Some("S"))?;
    let boxed = host.report(3)?;
    if !boxed.starts_with(b"age-encryption.org/v1") {
        return Err("the security box's report is not age's format".into());
    }
    for word in ["BOXED-WORDS", "fault_here", "crash-test", "Dukovo problem report"] {
        if holds(&boxed, word) {
            return Err(format!("the security box's report holds \"{word}\" in the clear"));
        }
    }
    Ok(())
}

/// A fifth crash, saved to a file, which only its owner reads, and then
/// for a phone: report prints an address and a QR code and waits for one
/// download, on a port the firewall allows. The host fetches it through a
/// port QEMU forwards to the one in the address, and gets the text that
/// was saved; the second fetch is refused.
fn a_report_is_saved_and_downloaded_once(vm: &mut Vm) -> Result<(), String> {
    run(vm, "crash crash-test signal 5", CRASH_SECONDS, &["kept signal 5"])?;
    report(vm, "crash --save /tmp/r.txt", SENT, None)?;
    answer_prompt(
        vm,
        "$",
        "clear; echo \"mode $(stat -c %a /tmp/r.txt) first $(head -n 1 /tmp/r.txt)\"; echo \"r.txt \
         $(sha256sum /tmp/r.txt | cut -c1-64)\"",
    )?;
    shown_within(vm, &["mode 600 first Dukovo problem report"], 30)?;
    let saved = hash_after(vm, "r.txt")?;

    run(
        vm,
        &format!("elevate firewall allow tcp {DOWNLOAD_PORT} && echo \"allowed $((6 * 7))\""),
        30,
        &["allowed 42"],
    )?;
    report(vm, &format!("crash --save --port {DOWNLOAD_PORT}"), SENT, None)?;
    shown_within(vm, &["Waiting for one download"], SEND_SECONDS)?;
    let screen = vm.screen()?;
    expect_qr_code(vm, "the download's address")?;
    let (port, path) = download_address(&screen)?;
    if port != DOWNLOAD_PORT {
        return Err(format!("the download listens on port {port}, not {DOWNLOAD_PORT}"));
    }
    let forwarded = free_port()?;
    vm.monitor(&format!("hostfwd_add n0 tcp:127.0.0.1:{forwarded}-:{port}"))?;
    let (code, body) = fetch(forwarded, &path)?;
    if code != "200" || format!("{:x}", Sha256::digest(&body)) != saved {
        return Err(format!("the first fetch of {path} gave {code} and not the saved text"));
    }
    let (code, _) = fetch(forwarded, &path)?;
    if code == "200" {
        return Err(format!("the second fetch of {path} was answered 200"));
    }
    Ok(())
}

/// The port and the path, `/<token>/report.txt`, of the one-time download,
/// from the address report printed: `https://10.0.2.15:<port>/<token>/report.txt`.
fn download_address(screen: &[String]) -> Result<(u16, String), String> {
    let address = screen
        .iter()
        .find_map(|line| {
            let rest = line.split_once("https://")?.1.trim_end();
            rest.ends_with("/report.txt").then_some(rest)
        })
        .ok_or_else(|| format!("no download address on the screen: {}", screen.join(" | ")))?;
    let (host, path) = address
        .split_once('/')
        .ok_or_else(|| format!("the download address has no path: {address}"))?;
    let port = host
        .rsplit_once(':')
        .and_then(|(_, port)| port.parse().ok())
        .ok_or_else(|| format!("the download address has no port: {address}"))?;
    Ok((port, format!("/{path}")))
}

/// Fetches `path` from the machine's download through the host's `port`,
/// without checking the certificate, which the fingerprint printed with the
/// address stands for: the HTTP status, or 000 where there was none, and
/// the body.
fn fetch(port: u16, path: &str) -> Result<(String, Vec<u8>), String> {
    let saved = Path::new(DIR).join("download");
    // A fetch that got nothing leaves no file, and must not read the last one's.
    let _ = fs::remove_file(&saved);
    let output = Command::new("curl")
        .args(["-k", "-sS", "--max-time", "30", "-o"])
        .arg(&saved)
        .args(["-w", "%{http_code}"])
        .arg(format!("https://127.0.0.1:{port}{path}"))
        .output()
        .map_err(|err| format!("`curl` is not installed: {err}"))?;
    let code = String::from_utf8_lossy(&output.stdout).into_owned();
    Ok((code, fs::read(&saved).unwrap_or_default()))
}

/// A service that panics ends with no signal, so the kernel records
/// nothing: reportd follows the log for the panic's line, and keeps it with
/// its location, and the text of its report has the message.
fn a_panic_is_kept(vm: &mut Vm) -> Result<(), String> {
    run(
        vm,
        "crash crash-panic panic 1; echo \"located $(crashes | \
         grep -c 'panic at .*main[.]rs:[0-9]*:[0-9]*')\"",
        CRASH_SECONDS,
        &["kept panic 1", "located 1"],
    )?;
    report(vm, "crash --save /tmp/p.txt", PANICKED, None)?;
    run(
        vm,
        "grep -q 'was asked to pani[c]' /tmp/p.txt && echo \"message $((6 * 7))\"",
        30,
        &["message 42"],
    )
}


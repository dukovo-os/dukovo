// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `ob test lab-writer` (ADR 0429): the lab's net boot, end to end, in
//! QEMU. OVMF boots the network GRUB from QEMU's own DHCP and TFTP on a
//! network laid out as the lab's (`10.77.0.0/24`, the lab PC at
//! `10.77.0.1`), as dnsmasq serves it there; GRUB reads the machine's
//! boot script, as dukovo-lab writes it, and loads the writer over HTTP
//! from a pretend lab; the writer finds the disk by its serial number
//! among two, writes a test image to it from the pretend lab's plan and
//! data, and restarts once the lab takes its hash.
//!
//! Then the same with a lab that refuses the hash, after which the
//! writer must power the machine off. Each run checks the disks: the
//! image on the named one, its last mebibyte zeroed, what lies between
//! left as it was, and the other disk untouched.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use super::{GRUB_FILE, WRITER_CMDLINE, net_grub, writer_kit};
use crate::process::{self, step};
use crate::{Context as _, Ctx, Error, Result, bail, qemu};

/// The machine's card, and the boot script's name in the TFTP folder.
const MAC: &str = "52:54:00:4c:41:42";

/// The serial numbers of the disk to write and of the one to leave.
const DISK_SERIAL: &str = "LAB-WRITER-DISK";
const OTHER_SERIAL: &str = "SOME-OTHER-DISK";

/// The disks' size, and what each holds before the write.
const DISK_BYTES: usize = 24 << 20;
const DISK_FILL: u8 = 0xaa;
const OTHER_FILL: u8 = 0x55;

/// The test image's size: not a whole number of the lab's 64 KiB grains.
const IMAGE_BYTES: usize = (9 << 20) + 4096;

/// The boot's token, as dukovo-lab makes one new for each boot.
const TOKEN: &str = "0123456789abcdef0123456789abcdef";

/// How long one boot may take under KVM; without it, four times as long.
const BOOT_TIME: Duration = Duration::from_secs(240);

/// How the pretend lab answers the writer's hash.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Lab {
    /// As the real one does: `restart` when it is the image's.
    Takes,
    /// 409, as when the disk did not hold the image.
    Refuses,
}

pub fn check(ctx: &Ctx) -> Result<()> {
    qemu::require_qemu()?;
    let (code, vars) = qemu::find_ovmf().ok_or_else(|| {
        Error::new("OVMF (UEFI firmware for QEMU) is not installed").hint("run `./ob doctor`")
    })?;
    let kit = writer_kit(ctx)?;
    let grub = net_grub(ctx)?;
    let dir = ctx.out.join("lab-writer-check");
    if dir.exists() {
        std::fs::remove_dir_all(&dir).context(|| format!("removing {}", dir.display()))?;
    }
    let tftp = dir.join("tftp");
    process::copy(&grub, &tftp.join(GRUB_FILE))?;
    let image = test_image();
    for lab in [Lab::Takes, Lab::Refuses] {
        step(&format!(
            "Net booting the writer in QEMU, from a pretend lab that {} its hash",
            if lab == Lab::Takes { "takes" } else { "refuses" }
        ));
        boot(ctx, &dir, &tftp, &kit, (&code, &vars), &image, lab)?;
    }
    println!(
        "The writer net booted, wrote the disk with its serial number alone, and ended as told."
    );
    Ok(())
}

/// An image with data at its start, in its middle and at its end, and
/// zeros between, as a disk image has.
fn test_image() -> Vec<u8> {
    let mut image = vec![0u8; IMAGE_BYTES];
    for (at, length, seed) in
        [(0, 200_000, 1u32), (3 << 20, 70_000, 2), (IMAGE_BYTES - 5000, 5000, 3)]
    {
        let mut state = seed;
        for byte in &mut image[at..at + length] {
            state = state.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            *byte = (state >> 16) as u8 | 1;
        }
    }
    image
}

/// The image's runs of data, as the lab's plan gives them: 64 KiB grains
/// that hold anything but zeros, joined where they touch.
fn extents(image: &[u8]) -> Vec<(usize, usize)> {
    const GRAIN: usize = 64 * 1024;
    let mut extents: Vec<(usize, usize)> = Vec::new();
    for (index, grain) in image.chunks(GRAIN).enumerate() {
        if grain.iter().all(|&b| b == 0) {
            continue;
        }
        let at = index * GRAIN;
        match extents.last_mut() {
            Some((start, length)) if *start + *length == at => *length += grain.len(),
            _ => extents.push((at, grain.len())),
        }
    }
    extents
}

/// One net boot of the writer, and the checks after it.
fn boot(
    ctx: &Ctx,
    dir: &Path,
    tftp: &Path,
    kit: &Path,
    ovmf: (&Path, &Path),
    image: &[u8],
    lab: Lab,
) -> Result<()> {
    let disk = dir.join("disk.raw");
    let other = dir.join("other.raw");
    std::fs::write(&disk, vec![DISK_FILL; DISK_BYTES]).context(|| "writing the disk".into())?;
    std::fs::write(&other, vec![OTHER_FILL; DISK_BYTES]).context(|| "writing the disk".into())?;
    let vars = dir.join("vars.fd");
    process::copy(ovmf.1, &vars)?;
    let serial = dir.join(format!("serial-{lab:?}.log").to_lowercase());

    let server = PretendLab::start(kit, image, lab)?;
    let base = format!("http://10.77.0.1:{}/{TOKEN}/", server.port);
    let cmdline = format!(
        "{WRITER_CMDLINE} console=ttyS0,115200 dukovo.lab={base} dukovo.lab.ip=10.77.0.2/24 \
         dukovo.lab.mac={MAC}"
    );
    // The boot script, as dukovo-lab writes it (its docs/net-boot.md).
    let script = format!(
        "set timeout=0\ninsmod http\necho 'dukovo-lab: loading the writer'\n\
         linux (http,10.77.0.1:{port})/{TOKEN}/kernel {cmdline}\n\
         initrd (http,10.77.0.1:{port})/{TOKEN}/initrd\nboot\n",
        port = server.port
    );
    let script_file = tftp.join(format!("dukovo-lab/boot/{MAC}.cfg"));
    process::create_dir(script_file.parent().expect("the script has a folder"))?;
    std::fs::write(&script_file, script)
        .context(|| format!("writing {}", script_file.display()))?;

    let kvm = qemu::kvm_available();
    let (accel, cpu) = if kvm { ("kvm", "host") } else { ("tcg", "max") };
    let option = qemu::option_value;
    let mut cmd = Command::new("qemu-system-x86_64");
    cmd.args(["-M", "q35", "-m", "1G", "-smp", "2", "-accel", accel, "-cpu", cpu])
        .args(["-drive", &format!("if=pflash,format=raw,readonly=on,file={}", option(ovmf.0))])
        .args(["-drive", &format!("if=pflash,format=raw,file={}", option(&vars))])
        // QEMU's network, laid out as the lab's: QEMU answers DHCP and
        // TFTP as dnsmasq does there, and the lab PC's address reaches
        // this PC's, where the pretend lab listens.
        .args([
            "-netdev",
            &format!(
                "user,id=lab,net=10.77.0.0/24,host=10.77.0.1,dhcpstart=10.77.0.2,tftp={},\
                 bootfile={GRUB_FILE}",
                option(tftp)
            ),
        ])
        .args(["-device", &format!("e1000e,netdev=lab,mac={MAC},bootindex=0")])
        .args(["-drive", &format!("if=none,id=disk,format=raw,file={}", option(&disk))])
        .args(["-device", &format!("ide-hd,drive=disk,bus=ide.0,serial={DISK_SERIAL}")])
        .args(["-drive", &format!("if=none,id=other,format=raw,file={}", option(&other))])
        .args(["-device", &format!("ide-hd,drive=other,bus=ide.1,serial={OTHER_SERIAL}")])
        .args(["-serial", &format!("file:{}", serial.display())])
        .args(["-display", "none", "-monitor", "none", "-no-reboot"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    if ctx.verbose {
        eprintln!("+ {cmd:?}");
    }
    let mut child = cmd.spawn().context(|| "starting qemu-system-x86_64".into())?;
    let limit = if kvm { BOOT_TIME } else { BOOT_TIME * 4 };
    let deadline = Instant::now() + limit;
    let exited = loop {
        if let Some(status) = child.try_wait().context(|| "waiting for QEMU".into())? {
            break Some(status);
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(250));
    };
    let mut stderr = String::new();
    if let Some(mut pipe) = child.stderr.take() {
        let _ = pipe.read_to_string(&mut stderr);
    }
    let events = server.stop();
    let log = std::fs::read_to_string(&serial).unwrap_or_default();
    let fail = |why: String| -> Result<()> {
        crate::bench::show_trouble(&log);
        eprintln!("    the pretend lab saw: {}", events.join("; "));
        if !stderr.trim().is_empty() {
            eprintln!("    QEMU said: {}", stderr.trim());
        }
        Err(Error::new(format!("{why}; the serial log is {}", serial.display())))
    };
    match exited {
        None => return fail(format!("the writer did not end within {} s", limit.as_secs())),
        Some(status) if !status.success() => return fail(format!("QEMU failed ({status})")),
        Some(_) => {}
    }

    let ending = match lab {
        Lab::Takes => "lab-writer: done; restarting into the disk",
        Lab::Refuses => "the disk does not hold the image; powering off",
    };
    if !log.contains(ending) {
        return fail(format!("the writer did not say \"{ending}\""));
    }
    for asked in ["GET kernel", "GET initrd", "GET plan", "GET data", "POST done"] {
        if !events.iter().any(|event| event == asked) {
            return fail(format!("the writer never asked for {asked}"));
        }
    }
    let written = std::fs::read(&disk).context(|| "reading the disk".into())?;
    if written[..image.len()] != *image {
        return fail("the disk does not hold the image".into());
    }
    if written[image.len()..DISK_BYTES - (1 << 20)].iter().any(|&b| b != DISK_FILL) {
        return fail("the writer changed the disk past the image".into());
    }
    if written[DISK_BYTES - (1 << 20)..].iter().any(|&b| b != 0) {
        return fail("the writer did not zero the disk's last mebibyte".into());
    }
    let left = std::fs::read(&other).context(|| "reading the other disk".into())?;
    if left.iter().any(|&b| b != OTHER_FILL) {
        return fail("the writer wrote a disk with another serial number".into());
    }
    Ok(())
}

/// The lab's side of a boot, as dukovo-lab's net-boot server answers it
/// (its docs/net-boot.md), on this PC's loopback.
struct PretendLab {
    port: u16,
    done: Arc<AtomicBool>,
    events: Arc<Mutex<Vec<String>>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

/// What the pretend lab serves.
struct Served {
    kernel: Vec<u8>,
    initrd: Vec<u8>,
    plan: String,
    data: Vec<u8>,
    sha256: String,
    lab: Lab,
}

impl PretendLab {
    fn start(kit: &Path, image: &[u8], lab: Lab) -> Result<PretendLab> {
        let read = |name: &str| {
            let path = kit.join(name);
            std::fs::read(&path).context(|| format!("reading {}", path.display()))
        };
        let extents = extents(image);
        let data: Vec<u8> = extents
            .iter()
            .flat_map(|&(at, length)| image[at..at + length].iter().copied())
            .collect();
        let sha256 = sha256(image)?;
        let plan = serde_json::json!({
            "version": 1,
            "disk": DISK_SERIAL,
            "size": image.len(),
            "sha256": sha256,
            "extents": extents,
        })
        .to_string();
        let served = Arc::new(Served {
            kernel: read("vmlinuz")?,
            initrd: read(super::WRITER_INITRD)?,
            plan,
            data,
            sha256,
            lab,
        });
        let listener =
            TcpListener::bind("127.0.0.1:0").context(|| "listening on loopback".into())?;
        let port = listener.local_addr().context(|| "listening on loopback".into())?.port();
        listener.set_nonblocking(true).context(|| "listening on loopback".into())?;
        let done = Arc::new(AtomicBool::new(false));
        let events = Arc::new(Mutex::new(Vec::new()));
        let thread = {
            let (done, events) = (Arc::clone(&done), Arc::clone(&events));
            std::thread::spawn(move || {
                while !done.load(Ordering::Relaxed) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            let said = answer(&stream, &served);
                            events.lock().unwrap_or_else(PoisonError::into_inner).push(said);
                        }
                        Err(_) => std::thread::sleep(Duration::from_millis(20)),
                    }
                }
            })
        };
        Ok(PretendLab { port, done, events, thread: Some(thread) })
    }

    /// Stops it, and gives what it was asked, in order.
    fn stop(mut self) -> Vec<String> {
        self.done.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        std::mem::take(&mut *self.events.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

/// The SHA-256 of `bytes`, by `sha256sum`.
fn sha256(bytes: &[u8]) -> Result<String> {
    let mut child = Command::new("sha256sum")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .context(|| "running sha256sum".into())?;
    child.stdin.take().expect("piped").write_all(bytes).context(|| "running sha256sum".into())?;
    let output = child.wait_with_output().context(|| "running sha256sum".into())?;
    let text = String::from_utf8_lossy(&output.stdout);
    match text.split_whitespace().next() {
        Some(sum) if sum.len() == 64 => Ok(sum.to_string()),
        _ => bail!("sha256sum gave no hash"),
    }
}

/// Answers one request, and says what it was: its method and what it
/// asked for, and for a log line, the line.
fn answer(stream: &TcpStream, served: &Served) -> String {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    if reader.read_line(&mut line).is_err() {
        return "a request that broke off".into();
    }
    let mut words = line.split_whitespace();
    let (method, path) =
        (words.next().unwrap_or_default().to_string(), words.next().unwrap_or_default());
    let what = path.strip_prefix(&format!("/{TOKEN}/")).unwrap_or("?").to_string();
    let mut length = 0;
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header).unwrap_or(0) == 0 || header.trim().is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':')
            && name.trim().eq_ignore_ascii_case("content-length")
        {
            length = value.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0u8; length];
    let _ = reader.read_exact(&mut body);
    let body = String::from_utf8_lossy(&body).trim().to_string();
    let (status, content): (u16, &[u8]) = match (method.as_str(), what.as_str()) {
        ("GET", "kernel") => (200, &served.kernel),
        ("GET", "initrd") => (200, &served.initrd),
        ("GET", "plan") => (200, served.plan.as_bytes()),
        ("GET", "data") => (200, &served.data),
        ("POST", "log" | "failed") => (200, b"ok\n"),
        ("POST", "done") if served.lab == Lab::Takes && body.contains(&served.sha256) => {
            (200, b"restart\n")
        }
        ("POST", "done") => (409, b"wrong hash\n"),
        _ => (404, b"not here\n"),
    };
    let head = format!(
        "HTTP/1.1 {status} {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        if status == 200 { "OK" } else { "Error" },
        content.len()
    );
    let mut stream = stream;
    let _ = stream.write_all(head.as_bytes()).and_then(|()| stream.write_all(content));
    match (method.as_str(), what.as_str()) {
        ("POST", "log" | "failed") => format!("{method} {what}: {body}"),
        _ => format!("{method} {what}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_test_image_has_runs_of_data_and_zeros_as_the_lab_plans_them() {
        let image = test_image();
        let extents = extents(&image);
        assert_eq!(extents.len(), 3);
        assert_eq!(extents[0], (0, 4 * 64 * 1024));
        // The last grain is short: the image is not a whole number of them.
        let (at, length) = extents[2];
        assert_eq!(at + length, IMAGE_BYTES);
    }
}

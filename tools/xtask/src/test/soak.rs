// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `ob soak` (ADR 0427): an installed system under a mixed load for
//! hours or days, in QEMU, while what it uses is watched for creep.
//!
//! The machine boots an installed system's disk, as the `installed-uefi`
//! boot test does, with QEMU's user network, whose 10.0.2.100 serves two
//! things through programs of the host's: on port 80, 32 MiB of random
//! bytes; on port 81, the round's script ([`ROUND`]). The shell on tty1
//! fetches the script, then runs it round after round until the time is
//! up. A round writes, copies, compares and removes 128 MiB of files on
//! the root's `/shared`, runs 300 programs, takes the 32 MiB over the
//! network, and then says the memory in use (neither free nor the page
//! cache) and the disk space `/shared`'s filesystem uses.
//!
//! A round whose files differ, whose programs stop, or whose bytes do
//! not all come fails the soak at once, as does memory past twice the
//! first round's. At the end, the mean of the last quarter of the rounds
//! is held to that of the second quarter, after the caches have warmed:
//! memory may grow by a twentieth (8 MiB at least) and the disk by
//! 64 MiB, as Btrfs keeps some of its metadata. Each round is a line of
//! `out/test/soak/rounds.csv`.
//!
//! Releases soak for seven days on the lab's PC (`--hours 168`), and CI
//! for five hours each week, as GitHub stops a job at six.

use std::fmt::Write as _;
use std::sync::OnceLock;

use super::*;

/// The soak's machine: an installed system on a SATA disk under UEFI,
/// with a virtio network card.
const SOAK: Case = Case {
    name: "soak",
    about: "an installed system under files, programs and network traffic for hours, its memory and disk use held flat (ADR 0427)",
    firmware: Firmware::Uefi,
    media: Media::Disk,
    cmdline: "console-sign-in=off",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Installed,
    cpu_model: None,
    qemu: &["-netdev", NETDEV, "-device", "virtio-net-pci,netdev=n0"],
    script: soak,
    disks: &[],
    second: None,
};

/// Where the host's two programs are, from the top of the tree, which
/// QEMU runs in.
const DIR: &str = "out/test/soak";

/// QEMU's user network, with the host's bytes at 10.0.2.100 port 80 and
/// the round's script at port 81.
const NETDEV: &str = "user,id=n0,guestfwd=tcp:10.0.2.100:80-cmd:out/test/soak/bytes,\
                      guestfwd=tcp:10.0.2.100:81-cmd:out/test/soak/script";

/// The bytes a round takes over the network.
const NETWORK_BYTES: u64 = 32 << 20;

/// The programs a round runs.
const PROGRAMS: u64 = 300;

/// One round of the load, which the machine runs as `sh round.sh N`.
const ROUND: &str = r#"#!/bin/sh
# One round of ob soak's load (ADR 0427): files, programs and the
# network, then what the machine uses.
round=$1
dir=/shared/soak
files=ok
mkdir -p $dir || files=bad
dd if=/dev/urandom of=$dir/a bs=1M count=128 2> /dev/null || files=bad
cp $dir/a $dir/b || files=bad
cmp -s $dir/a $dir/b || files=bad
rm -rf $dir || files=bad
programs=0
while [ $programs -lt 300 ]; do
    /system/commands/true || break
    programs=$((programs + 1))
done
network=$(nc -d 10.0.2.100 80 | wc -c)
sync
sleep 5
memory=$(awk '/^total:/ { t = $2 } /^free:/ { f = $2 } /^page_cache:/ { c = $2 } END { print t - f - c }' /processes/meminfo)
disk=$(df -kP /shared | awk 'NR == 2 { print $3 }')
echo "soak-round $round: files $files, programs $programs, network $network, memory $memory, disk $disk"
"#;

/// How long a round may take before the soak gives up on it.
const ROUND_LIMIT: Duration = Duration::from_mins(15);

/// How much the memory in use may grow from the second quarter of the
/// rounds to the last, in KiB: a twentieth, and at least this.
const MEMORY_SLACK_KIB: u64 = 8 * 1024;

/// How much the disk space used may grow, in KiB.
const DISK_SLACK_KIB: u64 = 64 * 1024;

/// The fewest rounds whose quarters say anything.
const FEWEST_ROUNDS: usize = 8;

/// How long the load runs, set by [`command`] for [`soak`].
static LENGTH: OnceLock<Duration> = OnceLock::new();

/// Where `--summary` appends the report, if anywhere.
static SUMMARY: OnceLock<PathBuf> = OnceLock::new();

/// `ob soak`.
pub fn command(ctx: &Ctx, args: &mut Args) -> Result<()> {
    let hours = match args.value("--hours")? {
        Some(value) => value
            .parse::<f64>()
            .ok()
            .filter(|hours| hours.is_finite() && *hours > 0.0 && *hours <= 24.0 * 30.0)
            .ok_or_else(|| {
                Error::new(format!("--hours takes a number of hours up to 720, not `{value}`"))
            })?,
        None => 168.0,
    };
    let summary = args.value("--summary")?;
    let profile = args.profile(&ctx.config)?;
    args.finish()?;

    let length = Duration::from_secs_f64(hours * 3600.0);
    let _ = LENGTH.set(length);
    if let Some(summary) = summary {
        let _ = SUMMARY.set(PathBuf::from(summary));
    }
    let dir = ctx.root.join(DIR);
    process::create_dir(&dir)?;
    write_program(&dir.join("bytes"), &format!("exec head -c {NETWORK_BYTES} /dev/urandom"))?;
    write_program(&dir.join("script"), &format!("cat <<'ROUND'\n{ROUND}ROUND"))?;
    step(&format!("Soaking an installed system for {hours} hours (ADR 0427)"));
    // The boot and the end besides the load's own time.
    boot_tests_of(ctx, profile, &[&SOAK], length + Duration::from_hours(1))
}

/// Writes a shell script of `body` at `path` that QEMU can run.
fn write_program(path: &Path, body: &str) -> Result<()> {
    std::fs::write(path, format!("#!/bin/sh\n{body}\n"))
        .context(|| format!("writing {}", path.display()))?;
    set_executable(path)
}

#[cfg(unix)]
fn set_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
        .context(|| format!("making {} executable", path.display()))
}

#[cfg(not(unix))]
fn set_executable(_: &Path) -> Result<()> {
    Ok(())
}

/// What a round says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Round {
    /// Its number, from 1.
    number: usize,
    /// Seconds since the load began, when it ended.
    seconds: u64,
    /// Whether its files read back as written.
    files_ok: bool,
    /// The programs it ran.
    programs: u64,
    /// The bytes it took over the network.
    network: u64,
    /// The memory in use, in KiB: neither free nor the page cache.
    memory_kib: u64,
    /// The space `/shared`'s filesystem uses, in KiB.
    disk_kib: u64,
}

impl Round {
    /// Reads the line `soak-round N: files ok, programs P, network B,
    /// memory M, disk D`.
    fn parse(line: &str, seconds: u64) -> Option<Round> {
        let (head, rest) = line.trim().strip_prefix("soak-round ")?.split_once(": ")?;
        let mut fields = rest.split(", ").map(|field| field.split_once(' '));
        let mut next = |name: &str| match fields.next() {
            Some(Some((key, value))) if key == name => Some(value.trim()),
            _ => None,
        };
        let files = next("files")?;
        let mut number = |name: &str| next(name)?.parse::<u64>().ok();
        Some(Round {
            number: head.parse().ok()?,
            seconds,
            files_ok: files == "ok",
            programs: number("programs")?,
            network: number("network")?,
            memory_kib: number("memory")?,
            disk_kib: number("disk")?,
        })
    }

    /// What is wrong with the round's load, if anything.
    fn fault(&self) -> Option<String> {
        if !self.files_ok {
            Some("its files did not read back as written".into())
        } else if self.programs != PROGRAMS {
            Some(format!("it ran {} of its {PROGRAMS} programs", self.programs))
        } else if self.network != NETWORK_BYTES {
            Some(format!("it took {} of the network's {NETWORK_BYTES} bytes", self.network))
        } else {
            None
        }
    }
}

/// The load, round after round, then the verdict.
fn soak(vm: &mut Vm, _case: &Case) -> Result<(), String> {
    let length = *LENGTH.get().ok_or("ob soak sets the soak's length")?;
    vm.expect("spark: started consoled (/system/commands/consoled) as process ")?;
    vm.expect_screen(&[&format!("{RELEASE} {VERSION} (tty1)"), "# "])?;
    vm.expect_anywhere("netd: net0 has 10.0.2.15/24")?;
    // The word is split in the command so that only the answer shows it
    // whole.
    vm.type_text("clear; nc -d 10.0.2.100 81 > /shared/round.sh; echo \"script\"\"-fetched\"\n")?;
    vm.expect_screen(&["script-fetched"])?;

    let csv = vm.dir().join("soak/rounds.csv");
    let mut table =
        String::from("round,seconds,files,programs,network_bytes,memory_kib,disk_kib\n");
    let mut rounds: Vec<Round> = Vec::new();
    let started = Instant::now();
    while started.elapsed() < length {
        let number = rounds.len() + 1;
        let round = run_round(vm, number, started)?;
        let _ = writeln!(
            table,
            "{},{},{},{},{},{},{}",
            round.number,
            round.seconds,
            if round.files_ok { "ok" } else { "bad" },
            round.programs,
            round.network,
            round.memory_kib,
            round.disk_kib
        );
        std::fs::write(&csv, &table).map_err(|err| format!("writing {}: {err}", csv.display()))?;
        if let Some(fault) = round.fault() {
            return Err(format!("soak round {number} failed: {fault}"));
        }
        let first = rounds.first().unwrap_or(&round).memory_kib;
        if round.memory_kib > first * 2 {
            return Err(format!(
                "soak round {number}: {} KiB of memory in use, over twice the first round's {first}",
                round.memory_kib
            ));
        }
        if number == 1 || number.is_power_of_two() || number.is_multiple_of(100) {
            eprintln!(
                "          round {number} at {} min: memory {} KiB, disk {} KiB",
                round.seconds / 60,
                round.memory_kib,
                round.disk_kib
            );
        }
        rounds.push(round);
    }

    let verdict = judge(&rounds);
    let report = report(&rounds, verdict.as_ref().err());
    eprint!("{report}");
    if let Some(path) = SUMMARY.get() {
        use std::io::Write as _;
        let written = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .and_then(|mut file| file.write_all(report.as_bytes()));
        written.map_err(|err| format!("writing {}: {err}", path.display()))?;
    }
    verdict?;
    vm.type_text("poweroff\n")?;
    vm.expect("spark: asked to power off the machine")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}

/// Runs round `number` and reads what it says from the screen.
fn run_round(vm: &mut Vm, number: usize, started: Instant) -> Result<Round, String> {
    vm.type_text(&format!("clear; sh /shared/round.sh {number}\n"))?;
    let marker = format!("soak-round {number}: ");
    let deadline = Instant::now() + ROUND_LIMIT;
    loop {
        let screen = vm.screen()?;
        if let Some(line) = screen.iter().find(|line| line.trim_start().starts_with(&marker)) {
            let seconds = started.elapsed().as_secs();
            return Round::parse(line, seconds)
                .ok_or_else(|| format!("soak round {number} said `{}`", line.trim()));
        }
        if Instant::now() >= deadline || vm.exited().is_some() {
            return Err(format!(
                "soak round {number} did not end within {} minutes: {}",
                ROUND_LIMIT.as_secs() / 60,
                screen.join(" | ")
            ));
        }
        std::thread::sleep(Duration::from_secs(2));
    }
}

/// The mean of `values`, or 0 for none.
fn mean(values: impl Iterator<Item = u64>) -> u64 {
    let (sum, count) = values.fold((0, 0), |(sum, count), value| (sum + value, count + 1));
    sum.checked_div(count).unwrap_or(0)
}

/// What the second quarter of `rounds` used, and the last: memory and
/// disk, in KiB.
fn quarters(rounds: &[Round]) -> ((u64, u64), (u64, u64)) {
    let n = rounds.len();
    let warm = &rounds[n / 4..n / 2];
    let late = &rounds[n - n / 4..];
    let of = |part: &[Round]| {
        (mean(part.iter().map(|r| r.memory_kib)), mean(part.iter().map(|r| r.disk_kib)))
    };
    (of(warm), of(late))
}

/// Whether the memory and disk use stayed flat: the last quarter of the
/// rounds against the second.
fn judge(rounds: &[Round]) -> Result<(), String> {
    if rounds.len() < FEWEST_ROUNDS {
        return Err(format!(
            "only {} rounds ran, fewer than the {FEWEST_ROUNDS} a verdict needs: give more --hours",
            rounds.len()
        ));
    }
    let ((warm_memory, warm_disk), (late_memory, late_disk)) = quarters(rounds);
    let memory_slack = (warm_memory / 20).max(MEMORY_SLACK_KIB);
    if late_memory > warm_memory + memory_slack {
        return Err(format!(
            "the memory in use crept from {warm_memory} KiB to {late_memory} KiB, more than \
             {memory_slack} KiB"
        ));
    }
    if late_disk > warm_disk + DISK_SLACK_KIB {
        return Err(format!(
            "the disk space used crept from {warm_disk} KiB to {late_disk} KiB, more than \
             {DISK_SLACK_KIB} KiB"
        ));
    }
    Ok(())
}

/// The soak's report, in Markdown.
fn report(rounds: &[Round], fault: Option<&String>) -> String {
    let mut text = String::from("## Soak (ADR 0427)\n\n");
    let hours = rounds.last().map_or(0.0, |round| round.seconds as f64 / 3600.0);
    let _ = writeln!(text, "{} rounds in {hours:.1} hours.\n", rounds.len());
    if rounds.len() >= FEWEST_ROUNDS {
        let ((warm_memory, warm_disk), (late_memory, late_disk)) = quarters(rounds);
        text.push_str("| | Second quarter | Last quarter |\n|---|---:|---:|\n");
        let _ = writeln!(text, "| Memory in use (KiB) | {warm_memory} | {late_memory} |");
        let _ = writeln!(text, "| Disk used (KiB) | {warm_disk} | {late_disk} |");
        text.push('\n');
    }
    match fault {
        Some(fault) => {
            let _ = writeln!(text, "Failed: {fault}.\n");
        }
        None => text.push_str("Memory and disk use stayed flat.\n\n"),
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round(number: usize, memory_kib: u64, disk_kib: u64) -> Round {
        Round {
            number,
            seconds: number as u64 * 60,
            files_ok: true,
            programs: PROGRAMS,
            network: NETWORK_BYTES,
            memory_kib,
            disk_kib,
        }
    }

    #[test]
    fn reads_a_rounds_line() {
        let line =
            "soak-round 12: files ok, programs 300, network 33554432, memory 81234, disk 5678";
        assert_eq!(Round::parse(line, 720), Some(round(12, 81234, 5678)));
        assert_eq!(Round::parse(line, 720).unwrap().fault(), None);
        let bad = "soak-round 3: files bad, programs 120, network 100, memory 1, disk 2";
        let bad = Round::parse(bad, 0).unwrap();
        assert_eq!(bad.fault().unwrap(), "its files did not read back as written");
        assert!(Round { files_ok: true, ..bad }.fault().unwrap().contains("120 of its 300"));
        assert_eq!(Round::parse("soak-round 3: files ok, programs 300", 0), None);
        assert_eq!(Round::parse("# sh /shared/round.sh 3", 0), None);
    }

    #[test]
    fn flat_use_passes_and_creep_fails() {
        // Caches warm in the first quarter, which is not held to.
        let flat: Vec<Round> = (1..=40)
            .map(|n| round(n, if n < 10 { 60_000 + n as u64 * 1000 } else { 80_000 }, 5000))
            .collect();
        assert_eq!(judge(&flat), Ok(()));
        let leaking: Vec<Round> =
            (1..=40).map(|n| round(n, 80_000 + n as u64 * 500, 5000)).collect();
        assert!(judge(&leaking).unwrap_err().contains("memory in use crept"));
        let filling: Vec<Round> =
            (1..=40).map(|n| round(n, 80_000, 5000 + n as u64 * 4096)).collect();
        assert!(judge(&filling).unwrap_err().contains("disk space used crept"));
        assert!(judge(&flat[..5]).unwrap_err().contains("only 5 rounds"));
    }

    #[test]
    fn the_report_gives_the_quarters() {
        let rounds: Vec<Round> = (1..=8).map(|n| round(n, 80_000, 5000)).collect();
        let text = report(&rounds, None);
        assert!(text.contains("8 rounds in 0.1 hours."), "{text}");
        assert!(text.contains("| Memory in use (KiB) | 80000 | 80000 |"), "{text}");
        assert!(text.contains("stayed flat"), "{text}");
    }

    #[test]
    fn the_round_script_says_what_the_harness_reads() {
        assert!(ROUND.contains("files $files, programs $programs, network $network"));
        assert!(ROUND.contains(&format!("-lt {PROGRAMS}")));
        assert!(ROUND.contains("memory $memory, disk $disk"));
        assert!(!ROUND.contains("\nROUND\n"), "the script's own end marker");
    }
}

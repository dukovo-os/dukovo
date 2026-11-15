// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `ob test replay`: power cuts of an update, replayed from QEMU's write
//! log (ADR 0536).
//!
//! The recording runs a case (*update-power-replay*, or its BIOS twin)
//! with its boot disk behind QEMU's *blklogwrites* driver, and notes in
//! `out/test/replay/<case>.marks` which entries of the log the update
//! made. The replay then rebuilds the disk as a power cut at each flush of
//! those entries, or between two flushes, would have left it
//! ([`crate::writelog::cuts`]), boots each rebuilt disk, and checks that
//! it starts slot A or slot B whole ([`cases::update_power_replay`]).
//!
//! A cut that fails keeps its disk, the log, the marks and the serial log
//! in `out/test/replay`, and the command that boots that cut again from
//! the log, without recording, is printed. The recording's disk before
//! the run, `<case>.base.img`, is what the log applies to.

use std::path::{Path, PathBuf};
use std::time::Duration;

use super::cases::update_power_replay::check_cut;
use super::{
    Case, Ctx, Script, all_cases, append_own, build_for, case_budget, clean_log, run_case,
    test_images, test_live, test_roots, test_update_server, test_updates, with_cmdline,
    write_with_holes,
};
use crate::config::Profile;
use crate::process::{self, step};
use crate::writelog::{Cut, Disk, Log, Points, cuts, part_of};
use crate::{Args, Context as _, Error, Result, qemu};

/// The case `ob test replay` records without `--case`: under UEFI.
const DEFAULT_CASE: &str = "update-power-replay";

/// The cases it can record.
const CASES: [&str; 2] = [DEFAULT_CASE, "update-power-replay-bios"];

/// How many cuts a run takes without `--points`.
const DEFAULT_POINTS: usize = 16;

/// How `ob test replay` changes a run of a case.
pub(super) struct Replaying {
    /// The disk to boot, which the run uses as it is: a copy made for it.
    pub(super) image: PathBuf,
    /// Where to log the writes to that disk, to record them.
    pub(super) write_log: Option<PathBuf>,
    /// What to do with the machine instead of the case's own script.
    pub(super) script: Option<Script>,
}

/// The options of `ob test replay`, apart from `--case`.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct Options {
    points: Option<Points>,
    part: Option<(usize, usize)>,
    seed: Option<u64>,
    log: Option<PathBuf>,
    point: Option<usize>,
}

impl Options {
    /// Takes the options of `ob test replay` out of `args`.
    pub(super) fn take(args: &mut Args) -> Result<Options> {
        let points = match args.value("--points")? {
            Some(value) => Some(parse_points(&value)?),
            None => None,
        };
        let part = match args.value("--part")? {
            Some(value) => Some(parse_part(&value)?),
            None => None,
        };
        let seed = match args.value("--seed")? {
            Some(value) => Some(
                value
                    .parse::<u64>()
                    .map_err(|_| Error::new(format!("--seed takes a number, not `{value}`")))?,
            ),
            None => None,
        };
        let point = match args.value("--point")? {
            Some(value) => Some(value.parse::<usize>().map_err(|_| {
                Error::new(format!("--point takes the number of a cut, not `{value}`"))
            })?),
            None => None,
        };
        let log = args.value("--log")?.map(PathBuf::from);
        Ok(Options { points, part, seed, log, point })
    }

    /// Fails if an option was given, for a command that is not `replay`.
    pub(super) fn none_given(&self) -> Result<()> {
        if *self != Options::default() {
            crate::bail!(
                "--points, --part, --seed, --log and --point belong to `ob test replay`; give \
                 `replay` with them"
            );
        }
        Ok(())
    }

    /// Fails if the options contradict one another.
    fn check(&self) -> Result<()> {
        if self.point.is_some() && (self.points.is_some() || self.part.is_some()) {
            crate::bail!("--point boots one cut; leave out --points and --part");
        }
        if self.point.is_some() && self.log.is_none() {
            crate::bail!("--point names a cut of a recording; give the recording's log with --log");
        }
        Ok(())
    }
}

/// `--points all` or `--points N`.
fn parse_points(value: &str) -> Result<Points> {
    match value {
        "all" => Ok(Points::All),
        _ => match value.parse::<usize>() {
            Ok(count) if count > 0 => Ok(Points::Sample(count)),
            _ => crate::bail!("--points takes `all` or a number of cuts, not `{value}`"),
        },
    }
}

/// `--part K/M`: the `K`th of `M` parts, counted from 1.
fn parse_part(value: &str) -> Result<(usize, usize)> {
    let parsed = value.split_once('/').and_then(|(part, parts)| {
        Some((part.parse::<usize>().ok()?, parts.parse::<usize>().ok()?))
    });
    match parsed {
        Some((part, parts)) if (1..=parts).contains(&part) => Ok((part, parts)),
        _ => crate::bail!("--part takes <part>/<parts> such as 1/8, not `{value}`"),
    }
}

/// The file beside the write log `log` where the marks of the run are kept.
pub(super) fn marks_file(log: &Path) -> PathBuf {
    log.with_extension("marks")
}

/// The copy of the disk as the recording started with it, which `log`'s
/// writes apply to.
fn base_file(log: &Path) -> PathBuf {
    log.with_extension("base.img")
}

/// The entries between which the update ran, from the marks `stage` and
/// `end` that `text` holds.
fn parse_marks(text: &str) -> Result<std::ops::Range<usize>, String> {
    let mark = |name: &str| {
        let value = text
            .lines()
            .filter_map(|line| line.strip_prefix(name)?.strip_prefix(' '))
            .next_back()
            .ok_or_else(|| format!("the marks have no `{name}` line"))?;
        value
            .trim()
            .parse::<usize>()
            .map_err(|_| format!("the `{name}` mark `{value}` is not a number"))
    };
    let (stage, end) = (mark("stage")?, mark("end")?);
    if stage >= end {
        return Err(format!(
            "the marks put the stage at entry {stage}, not before the end at {end}"
        ));
    }
    Ok(stage..end)
}

/// The slot, `A` or `B`, that the serial log `log` says started.
fn slot_started(log: &str) -> Option<char> {
    let (_, rest) = log.split_once("spark: started slot ")?;
    rest.chars().next().map(|slot| slot.to_ascii_uppercase())
}

/// The seed of a run that was not given one: the clock's nanoseconds.
fn clock_seed() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| u64::try_from(since.as_nanos() & u128::from(u64::MAX)).unwrap_or(0))
}

/// The command that boots cut `point` of the recording `log` again.
fn rerun_command(case: &str, log: &str, point: usize, seed: u64) -> String {
    format!("./ob test replay --case {case} --log {log} --point {point} --seed {seed}")
}

/// What a run of `ob test replay` has to work with.
struct Replay<'a> {
    ctx: &'a Ctx,
    case: &'a Case,
    /// The test image the case boots, without its command line.
    base: &'a [u8],
    root: Option<&'a Path>,
    live: Option<&'a Path>,
    /// `out/test`.
    dir: PathBuf,
    /// `out/test/replay`.
    replay_dir: PathBuf,
    kvm: bool,
    budget: Duration,
}

/// The files of a recording.
struct Recording {
    log: PathBuf,
    base: PathBuf,
    marks: PathBuf,
}

impl Replay<'_> {
    /// Boots `image`, a disk made for the run, with the case's machine and
    /// `script`, and gives why it failed.
    fn boot(
        &self,
        image: PathBuf,
        write_log: Option<PathBuf>,
        script: Option<Script>,
    ) -> Result<(), String> {
        let replaying = Replaying { image, write_log, script };
        let roots = (self.root, self.live);
        let run = (self.kvm, self.budget);
        run_case(self.ctx, self.case, self.base, roots, &self.dir, run, Some(&replaying))
    }

    /// Runs the case with its disk behind a write log, and notes the
    /// marks its script makes.
    fn record(&self) -> Result<Recording, String> {
        let name = self.case.name;
        let log = self.replay_dir.join(format!("{name}.log"));
        let recording = Recording { base: base_file(&log), marks: marks_file(&log), log };
        let copy = self.replay_dir.join(format!("{name}.img"));
        let image = with_cmdline(self.base, self.case.cmdline)?;
        write_with_holes(&recording.base, &image)
            .map_err(|err| format!("writing {}: {err}", recording.base.display()))?;
        append_own(self.case, &recording.base, self.root, self.live)?;
        let _ = std::fs::remove_file(&recording.marks);
        std::fs::write(&recording.log, b"")
            .map_err(|err| format!("creating {}: {err}", recording.log.display()))?;
        Disk::copy_of(&recording.base, &copy).map_err(|err| err.to_string())?;
        if let Err(reason) = self.boot(copy.clone(), Some(recording.log.clone()), None) {
            self.keep_serial_log(&format!("{name}-recording.log"));
            return Err(reason);
        }
        let _ = std::fs::remove_file(&copy);
        Ok(recording)
    }

    /// Keeps the serial log of the last boot as `name` in `out/test/replay`.
    fn keep_serial_log(&self, name: &str) -> PathBuf {
        let kept = self.replay_dir.join(name);
        let _ = std::fs::copy(self.dir.join(format!("boot-{}.log", self.case.name)), &kept);
        kept
    }

    /// Boots each of `chosen`, a cut after another, and prints how each
    /// went. Gives the number that failed.
    fn replay(&self, recording: &Recording, chosen: &[(usize, Cut)], seed: u64) -> Result<usize> {
        let name = self.case.name;
        let log = Log::open(&recording.log)?;
        let prefix_path = self.replay_dir.join(format!("{name}-prefix.img"));
        let cut_path = self.replay_dir.join(format!("{name}-cut.img"));
        let prefix = Disk::copy_of(&recording.base, &prefix_path)?;
        let mut applied = 0;
        let mut failed = 0;
        for (place, (number, cut)) in chosen.iter().enumerate() {
            let flush = cut.flush();
            prefix.apply(&log, applied..flush)?;
            applied = flush;
            let image = match cut {
                Cut::AtFlush(_) => prefix_path.clone(),
                Cut::Within { keep, .. } => {
                    Disk::copy_of(&prefix_path, &cut_path)?.apply_each(&log, keep)?;
                    cut_path.clone()
                }
            };
            let heading = format!(
                "replay {name} cut {}/{} ({} at entry {flush})",
                place + 1,
                chosen.len(),
                cut.kind()
            );
            let outcome = self.boot(image.clone(), None, Some(check_cut));
            let serial =
                std::fs::read(self.dir.join(format!("boot-{name}.log"))).unwrap_or_default();
            let slot = slot_started(&clean_log(&String::from_utf8_lossy(&serial)));
            match outcome {
                Ok(()) => {
                    eprintln!("{heading}: slot {}, ok", slot.unwrap_or('?'));
                    let _ = std::fs::remove_file(&cut_path);
                }
                Err(reason) => {
                    failed += 1;
                    eprintln!("{heading}: FAILED: {reason}");
                    self.keep_failed(&image, &prefix, *number, &recording.log, seed)?;
                }
            }
        }
        let _ = std::fs::remove_file(&prefix_path);
        Ok(failed)
    }

    /// Keeps the disk of cut `number`, which `image` is, and its serial log,
    /// and says how to boot it again.
    fn keep_failed(
        &self,
        image: &Path,
        prefix: &Disk,
        number: usize,
        log: &Path,
        seed: u64,
    ) -> Result<()> {
        let name = self.case.name;
        let kept = self.replay_dir.join(format!("{name}-cut-{number}.img"));
        if image == prefix.path() {
            Disk::copy_of(image, &kept)?;
        } else {
            std::fs::rename(image, &kept).context(|| format!("keeping {}", image.display()))?;
        }
        let serial = self.keep_serial_log(&format!("{name}-cut-{number}.log"));
        eprintln!("    disk of the cut: {}", self.ctx.display(&kept));
        eprintln!("    serial log: {}", self.ctx.display(&serial));
        eprintln!(
            "    log and marks: {}, {}",
            self.ctx.display(log),
            self.ctx.display(&marks_file(log))
        );
        eprintln!(
            "    to boot this cut again without recording: {}",
            rerun_command(name, &self.ctx.display(log), number, seed)
        );
        Ok(())
    }
}

/// The cuts to boot: the one `--point` names, or those `--points` and
/// `--part` choose, among those of the entries in `range`.
fn select(
    log: &Log,
    range: std::ops::Range<usize>,
    options: &Options,
    seed: u64,
) -> Result<Vec<(usize, Cut)>> {
    if let Some(point) = options.point {
        let all = cuts(log, range, Points::All, seed);
        let count = all.len();
        return all.into_iter().find(|(number, _)| *number == point).map(|cut| vec![cut]).ok_or_else(|| {
            Error::new(format!("--point {point}: the recording holds {count} cuts, numbered from 0, for seed {seed}"))
        });
    }
    let how = options.points.unwrap_or(Points::Sample(DEFAULT_POINTS));
    let mut chosen = cuts(log, range, how, seed);
    if let Some((part, parts)) = options.part {
        chosen = part_of(chosen, part, parts);
    }
    Ok(chosen)
}

/// `ob test replay`.
pub(super) fn command(
    ctx: &Ctx,
    profile: Profile,
    selected: &[String],
    options: &Options,
    timeout: Duration,
) -> Result<()> {
    options.check()?;
    let name = match selected {
        [] => DEFAULT_CASE,
        [one] if CASES.contains(&one.as_str()) => one.as_str(),
        _ => crate::bail!("replay takes one --case, one of {}", CASES.join(" and ")),
    };
    let case = all_cases().find(|case| case.name == name).expect("the replay cases exist");
    let seed = options.seed.unwrap_or_else(clock_seed);
    qemu::require_qemu()?;

    let dir = ctx.out.join("test");
    let replay_dir = dir.join("replay");
    process::create_dir(&replay_dir)?;
    let built = build_for(ctx, profile, &[case])?;
    let roots = test_roots(ctx, &built, &dir, &[case])?;
    let live = test_live(ctx, &built, &dir, &[case])?;
    test_updates(ctx, &built, &dir, &[case])?;
    test_update_server(ctx, &built, &dir, &[case])?;
    let images = test_images(ctx, &built, &dir, &[case], &roots)?;
    let base = &images
        .iter()
        .find(|(made, _)| *made == (case.image, case.media.is_cd()))
        .expect("built above")
        .1;
    let replay = Replay {
        ctx,
        case,
        base,
        root: roots.of(case).map(|root| root.path.as_path()),
        live: live.as_deref(),
        dir,
        replay_dir,
        kvm: qemu::kvm_available(),
        budget: case_budget(case, timeout),
    };

    let recorded = options.log.is_none();
    let recording = match &options.log {
        Some(log) => {
            let recording =
                Recording { base: base_file(log), marks: marks_file(log), log: log.clone() };
            for file in [&recording.log, &recording.base, &recording.marks] {
                if !file.is_file() {
                    return Err(Error::new(format!("--log needs {}, which is not there", file.display()))
                        .hint("a recording leaves its log, marks and base disk together; a run that passes removes the log and base disk"));
                }
            }
            recording
        }
        None => {
            step(&format!("Recording {name}, its boot disk behind a write log"));
            replay.record().map_err(|reason| {
                Error::new(format!("{name}: the recording failed: {reason}")).hint(format!(
                    "the serial log is {}",
                    ctx.display(&replay.replay_dir.join(format!("{name}-recording.log")))
                ))
            })?
        }
    };
    let result = replay_recording(&replay, &recording, options, seed);
    let passed = matches!(result, Ok(0));
    if recorded && passed {
        for file in [&recording.log, &recording.base] {
            let _ = std::fs::remove_file(file);
        }
    }
    roots.remove();
    if let Some(live) = &live {
        let _ = std::fs::remove_file(live);
    }
    match result? {
        0 => Ok(()),
        failed => crate::bail!("{name}: {failed} cuts failed; each says how to boot it again"),
    }
}

/// Replays the cuts of `recording` that `options` choose, and gives how
/// many failed.
fn replay_recording(
    replay: &Replay,
    recording: &Recording,
    options: &Options,
    seed: u64,
) -> Result<usize> {
    let marks = std::fs::read_to_string(&recording.marks)
        .context(|| format!("reading {}", recording.marks.display()))?;
    let range = parse_marks(&marks)
        .map_err(|why| Error::new(format!("{}: {why}", recording.marks.display())))?;
    let log = Log::open(&recording.log)?;
    let chosen = select(&log, range.clone(), options, seed)?;
    if chosen.is_empty() {
        crate::bail!(
            "the recording holds no cut between its marks, or --part leaves none to this part"
        );
    }
    step(&format!(
        "Replaying {} cuts of {} (entries {} to {}), seed {seed}",
        chosen.len(),
        replay.case.name,
        range.start,
        range.end
    ));
    replay.replay(recording, &chosen, seed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(list: &[&str]) -> Result<Options> {
        let mut args = Args::new("test", list.iter().map(ToString::to_string).collect());
        let options = Options::take(&mut args)?;
        args.finish()?;
        Ok(options)
    }

    #[test]
    fn the_options_are_taken_out_of_the_arguments() {
        let taken = options(&[
            "--points",
            "all",
            "--part=3/8",
            "--seed",
            "42",
            "--log",
            "a.log",
            "--point",
            "7",
        ])
        .unwrap();
        assert_eq!(
            taken,
            Options {
                points: Some(Points::All),
                part: Some((3, 8)),
                seed: Some(42),
                log: Some("a.log".into()),
                point: Some(7),
            }
        );
        assert_eq!(options(&["--points", "16"]).unwrap().points, Some(Points::Sample(16)));
        assert_eq!(options(&[]).unwrap(), Options::default());
    }

    #[test]
    fn options_that_make_no_sense_are_refused() {
        for bad in [
            ["--points", "0"],
            ["--points", "many"],
            ["--part", "0/8"],
            ["--part", "9/8"],
            ["--part", "3"],
            ["--seed", "-1"],
            ["--point", "x"],
        ] {
            assert!(options(&bad).is_err(), "{bad:?} was taken");
        }
    }

    #[test]
    fn options_given_to_another_suite_are_refused() {
        assert!(options(&["--seed", "1"]).unwrap().none_given().is_err());
        assert!(options(&[]).unwrap().none_given().is_ok());
    }

    #[test]
    fn one_cut_is_named_by_its_log_and_alone() {
        assert!(options(&["--point", "3", "--log", "a.log"]).unwrap().check().is_ok());
        assert!(options(&["--point", "3"]).unwrap().check().is_err());
        assert!(
            options(&["--point", "3", "--log", "a.log", "--points", "4"]).unwrap().check().is_err()
        );
        assert!(
            options(&["--point", "3", "--log", "a.log", "--part", "1/2"]).unwrap().check().is_err()
        );
    }

    #[test]
    fn the_marks_give_the_entries_of_the_update() {
        assert_eq!(parse_marks("stage 120\nend 4500\n"), Ok(120..4500));
        // A mark made twice is the later one.
        assert_eq!(parse_marks("stage 1\nstage 120\nend 4500\n"), Ok(120..4500));
        assert!(parse_marks("stage 120\n").is_err());
        assert!(parse_marks("stage 120\nend x\n").is_err());
        assert!(parse_marks("stage 90\nend 90\n").is_err());
    }

    #[test]
    fn the_slot_that_started_is_read_from_the_serial_log() {
        assert_eq!(slot_started("x\nspark: started slot b, @system.b\n"), Some('B'));
        assert_eq!(slot_started("spark: started slot a, @system.a"), Some('A'));
        assert_eq!(slot_started("no start"), None);
    }

    #[test]
    fn the_rerun_command_names_the_log_the_cut_and_the_seed() {
        assert_eq!(
            rerun_command("update-power-replay", "out/test/replay/update-power-replay.log", 12, 99),
            "./ob test replay --case update-power-replay --log \
             out/test/replay/update-power-replay.log --point 12 --seed 99"
        );
    }

    #[test]
    fn the_replay_cases_exist_and_record_through_the_write_log() {
        for name in CASES {
            let case = all_cases().find(|case| case.name == name);
            assert!(case.is_some(), "no boot test {name}");
        }
    }

    #[test]
    fn the_marks_file_and_the_base_disk_lie_beside_the_log() {
        let log = Path::new("out/test/replay/update-power-replay.log");
        assert_eq!(marks_file(log), Path::new("out/test/replay/update-power-replay.marks"));
        assert_eq!(base_file(log), Path::new("out/test/replay/update-power-replay.base.img"));
    }
}

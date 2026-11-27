<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# Deferred work

Every piece of work that was left for later, in one place, so that none of
it is lost. The [roadmap](roadmap.md) plans whole phases and releases; this
list holds the smaller things those phases left behind: limits a design
note or decision record accepted for now, checks that need real hardware,
features that wait for a port, and questions that wait for a decision.

## How to use this list

- **Adding.** A pull request that leaves something for later adds a row
  here in the same pull request: what is left, why it waits, and what it
  waits on. A row names its source (a decision record, a design note or a
  lab test) so that the reasoning stays with it.
- **Waits on** is one of: a phase or release of the roadmap; real hardware
  (and which); a port or an upstream change; a need (a benchmark or a
  program that asks for it, as the decision records put it); or a
  decision by the maintainers.
- **Closing.** The pull request that does the work removes its row and says
  so in its description. A row that turns out to be wrong is removed the
  same way, with the reason.
- **Reviewing.** Before each phase starts, its planner reads the rows that
  wait on that phase and either takes them into the plan or moves them on
  with a reason. Before each release, the hardware rows are read against
  what the lab can now run.

ADR numbers link to [`docs/adr/`](adr/README.md). A decision record is a
record of its day, so a "not yet" in it may since have been done; this
list is kept current instead.

## Maintainer tasks

| Item | Why it waits | Waits on |
| --- | --- | --- |
| Post-Phase-13 code audit (audit 10) | Planned and scoped; starts only on approval | Decision |
| `ob keys ceremony`: the yearly root signed from a Dukovo stick with networking off, both YubiKeys checked, and one replaced every other year (ADRs 0457, 0780) | `./ob release root` signs the next root on the maintainer's computer, from the keys in `releases/keys/` (ADR 0780) | Before 1.0.0, as the first root expires on 2027-11-17 |
| The handbook's key and incident runbooks followed once, end to end, against a test server and test YubiKeys | The `ob`, system-release and catalog-repo commands they give are tested with stand-in YubiKeys and servers; no one has yet followed a whole page | Before 1.0.0 |
| The install images of a release published for download, beside its notes: the stick, the ISO, `SHA256SUMS` and its signature (ADR 0779) | `./ob release images` leaves them in `out/release-images/VERSION`, and nothing uploads them yet | Before 1.0.0-alpha.1 |
| `ob` commands for a channel's move, a withdrawal and the advisories alone on a YubiKey (ADR 0778) | `system-release fetch`, then `channel`, `pull` or `advisories` with `--yubikey-helper`, then `./ob release deploy` do each by hand | Before 1.0.0 |
| The release train is off: `.github/release-train.json` says `"enabled": false`, so the release-train bot writes what it would do into its job summary and cuts no branch and opens no issue (ADRs 0458, 0534) | The first release branch is cut at 1.0.0, and the backport bot has no release branch to copy to until then | 1.0.0, when a maintainer sets `"enabled": true` |
| The daily scan cannot ask OSV about db1, mandoc, netcat and zip, whose recipes say `[upstream] none`: db1 is Berkeley DB 1.85 as glibc 2.1.3 carried it and only Debian keeps it, mandoc keeps its source in CVS, OpenBSD does not release nc by itself, and Info-ZIP publishes archives alone; `./ob security scan` lists them as not asked about, with their reasons (ADR 0534) | OSV answers for a git commit or a package of an ecosystem, and these projects have no public git repository to take the commit of a release from | A project that publishes git, or a decision by the maintainers to ask by a distribution's source package |
| Send the 27 mlibc and frigg patches upstream | Prepared; planned after the audit's last part | Audit 10 |
| smoltcp's own tests in CI, with its three UDP broadcast tests mended or changed for the kernel's patch (ADR 0182) | CI runs radix-net's tests, which drive the patched stack, but not the crate's own; those three already fail with the patch | Before the smoltcp patch goes upstream |
| Trademark clearance of the project's name | The application for the name is filed; the clearance waits on its examination and a legal review, and the pre-releases do not wait for it | Before 1.0.0 |

## Waiting on real hardware

QEMU cannot show these. Each runs in the lab when it has the machine or
the device, or by hand on a reference machine. The lab has an Intel
machine with AMT, an AMD machine and a laptop; see
[the lab chapter](handbook/src/lab.md).

### Lab machines and harness

| Item | Why it waits | Waits on |
| --- | --- | --- |
| A restart of the lab PC during `stress/server-week` ends the run: the lab does not pick a run up again, and the log it wrote as it went keeps what was done | A run that survives a restart of the lab PC | Lab harness |
| The installer's tests on a real lab machine: booting it from the install stick or DVD | The `amt` driver boots only the lab image, written to the disk by the net-booted writer (ADR 0429); a stick a person plugs in still needs the machine told to boot from it | Lab harness |
| The security range's attacker on a real lab machine | The attacker runs on a private QEMU network; a real machine needs a second network of its own for it | Lab harness |
| The power button and a graceful off on a real lab machine (`power = "button"` and `"off"`) | AMT asks the system to shut down only when an agent in the system answers AMT, which Dukovo has none of | A decision by the maintainers |
| Lab machines with Secure Boot on, for Secure Boot's tests on real hardware | The network GRUB and the pinned Linux kernel the writer runs on are not signed (ADR 0429) | A need: Secure Boot's lab tests |
| AMD lab machine set up as a lab target (no AMT: a smart plug, network boot and network logs) | Need the machine | AMD lab machine |
| The display layer on real machines and real hosts (ADRs 0704, 0705): the firmware card's bars on the OptiPlex (a 1920 by 1080 frame in under 3 ms, 64 by 64 pixels in under 10 µs, each no slower than Linux there), sixteen screens on one virtio card, and the text band on hosts other than QEMU | CI's virtual machines have four screens and no real framebuffer; the bars are of real memory | The lab's machines and its virtual machine host |
| virtio-gpu's shared pages are off unless `virtio-gpu=shared` asks for them (ADR 0704) | No host that offers them has run the path: QEMU on CI's runners does not | The lab's virtual machine host, if its kernel offers what QEMU needs |
| The lab PC's half of `sharing/nfs-server` | Needs a kernel NFS client, which the build container lacks | A run on the lab PC |
| The lab test of Repair startup (`boot/repair-startup`, ADR 0503): the installed system's menu taken off its EFI system partition, then the install stick's Repair startup puts it back | A test plugs in a stick only as a file beside the test or a blank one; the install stick is started only by `boot = "stick"`, which comes with a blank disk | Lab harness: a step that plugs in the install stick mid-test |
| The lab test of troubleshoot for a start that failed on bad firmware (`troubleshoot/bad-firmware`, ADR 0474): a firmware set with damaged microcode staged and tried, and troubleshoot naming FW-101 | `boot/troubleshoot` matches the shipped signatures against facts a file holds, and `boot/startup` runs startup; a real failed start needs a firmware set made damaged for the test and a restart that the health check fails | Lab harness: a damaged firmware set for a test (lab PR 10) |
| Tests that need two disks besides the stick, such as installing the mirror layout (`install/mirror-needs-two`, ADR 0315) | The lab gives a test one disk | Lab harness |
| A lab test of a PostgreSQL standby: streaming replication between two machines (`libpqwalreceiver` is built and `pg_basebackup` is in the bundle, ADRs 0521, 0522) | The lab gives a test one machine, and a standby needs a second server | Lab harness |
| A lab image without the lab account, to test the first boot's "no account yet" page (ADR 0401) | The lab image always has its account | Lab harness |
| A Stop for one test, which skips the test that is running and goes on with the run (`dukovo-lab stop` ends a whole run, and a run refuses a machine another run holds) | Left out of the harness's first parts | Lab harness |
| A lab test of a keyring entry set and read back | Needs a console sign-in with a password, which tests cannot type yet | Lab harness |
| A real lab machine's console over Intel AMT's Serial-over-LAN for a whole run, with no serial cable (lab `amt` driver, dukovo-os/lab's docs/real-machines.md) | On the lab's OptiPlex 7040 (AMT 11.8, I219-LM), the net-booted writer's download stalled at 40 to 50% of the image in all five boots of the first run with a session open, where the cable takes 14 seconds; its Linux says `Reset blocked by ME` as e1000e starts. The lab's one AMT machine keeps its serial cable | Real hardware: an AMT machine without a serial port |
| A lab test of `whois` | Needs a reachable whois server | Lab harness |
| Receive-side scaling, checksum and segmentation offloads for the server cards and virtio-net; GSO and multi-queue tunnels (ADR 0339) | Measured against Linux on 10 GbE, and the lab's network is 1 GbE; moved out of Phase 16 so it does not hold 1.0.0-alpha.1 | Phase 19 step 12 (a queue per CPU, RSS and offloads), measured on the reference server's 10 GbE ports |
| Intel's I217, I218 and I219s of Cannon Lake and later, and Realtek's RTL8127, on real chips (ADRs 0580, 0581) | No lab machine has one and QEMU emulates none; the unit tests check FreeBSD's and OpenBSD's sequences | A machine or card with one |
| `services/log-forward` on the Mint lab PC, where it skips: AppArmor's profile for rsyslogd lets it read no configuration outside /etc; a local rule in `/etc/apparmor.d/local/usr.sbin.rsyslogd` allowing `/run/user/*/dukovo-lab/** rw,` lets it run | AppArmor's rules are the lab PC owner's to change, never a test's | A local AppArmor rule on the lab PC |

### Installer, boot and cloud (Phase 14)

| Item | Why it waits | Waits on |
| --- | --- | --- |
| Quiet boot on real machines: the look, and Shift, Esc and F4 at GRUB (`boot/quiet-boot`, ADR 0426) | Firmware and screens differ | Lab, Dell, Framework |
| Boot to sign-in within 10 s and idle memory within 256 MiB on real machines (`performance/boot-time`, `performance/idle-memory`, ADR 0427) | CI checks these under KVM only | Lab, Dell, Framework |
| Seven-day soak with `ob soak` before each release (ADR 0427) | Runs on the lab PC | Lab PC, before each release |
| Hyper-V and Azure, Azure accelerated networking, and VMware guests (`hardware/hyperv`, `hardware/azure-accelerated`, `hardware/vmware`, `install/azure`, ADRs 0423 to 0425) | QEMU has no synthetic devices, MANA or VMware devices | Accounts on those platforms |
| Cloud images on AWS, DigitalOcean and Proxmox (ADR 0328) | Need real clouds | Cloud accounts |
| Installing next to a real Windows (`install/next-to-windows`, ADR 0314) | The lab has no Windows disk image, and Dukovo makes no NTFS from nothing | A Windows disk in the lab |
| A burned install DVD starting a real PC (`install/from-dvd`, ADR 0433) | Needs a real drive and a burned disc | Lab drive |
| The Dell's exit from an SSH session and a mount of a Windows share, checked by hand | Left from Phase 13's wrap-up | Dell |
| DHCP on the Dell's built-in Ethernet, and iperf3 against Linux on real hardware | Left from Phase 12's wrap-up | Dell, lab |

### Removable media (roadmap 11h)

| Item | Why it waits | Waits on |
| --- | --- | --- |
| Real SATA, USB and IDE DVD and Blu-ray drives; multisession discs; the tray lock and the eject button (ADRs 0431, 0432) | QEMU's drives are simulated | Real drives in the lab |
| A real music CD: ripping, playing and its CD-Text (ADR 0447) | The tests use `ob disc-drive` | A real drive and disc |
| Finalizing DVD+R and BD-R discs (close function 6) on real drives (ADR 0449) | Checked only against the emulated drive | Real burners |
| A real floppy drive (ADRs 0435, 0436) | QEMU's floppy controller is simulated | A floppy drive |
| SD readers: UHS-I speeds, eMMC, the write-protect switch, MSI on SD host controllers (ADRs 0437 to 0439) | QEMU 8.2 cannot show these; unit tests cover them | Real readers |
| Ricoh and JMicron readers' quirks (ADR 0445) | Written from other systems' descriptions | Real readers |
| O2 Micro readers are driven as standard SD host controllers | No BSD-licensed description of their quirks exists | A real reader; a description Dukovo can use |
| Realtek PCI readers on real chips (ADR 0448) | QEMU has no rtsx | Real readers |
| CompactFlash cards that run well by DMA, to be listed (ADR 0446) | Every card runs by PIO until one is shown to work | Real cards |

### Reference machines' firmware and devices

| Item | Why it waits | Waits on |
| --- | --- | --- |
| The fixed ACPI power button on both reference machines (ADR 0142) | Firmware differs | Dell, Framework |
| MWAIT idle on real CPUs (scheduler design note) | QEMU does not offer MWAIT, so boot tests exercise HLT | Dell, Framework |
| HWP and CPPC frequency control, thermal zones, embedded controller events, batteries and lids (ADRs 0201 to 0203) | QEMU emulates none of them | Dell, Framework |
| The Framework Desktop's embedded controller, fans and ARGB header (ADRs 0207, 0208) | Tested against a simulated EC | Framework |
| Corrected-error interrupts (CMCI on the Dell, Zen 5's counters) and Zen's scalable machine-check banks (ADRs 0169, 0204) | QEMU's CPUs have neither | Dell, Framework |
| The chipset watchdogs: the Dell's TCO timer behind its SMBus controller, and the AMD FCH's (ADR 0167), and an update that hangs falling back through them (ADR 0453; the boot test `update-hang` proves it on QEMU's i6300esb, and the lab's QEMU machines have no watchdog) | Phase 15's rollback relies on them; not proven on hardware | Dell, Framework, AMD lab machine |
| The Dell's touchpad over Intel GPIO, I2C and HID over I2C (ADRs 0233 to 0235) | Tested against simulated controllers | Dell |
| The Dell's DDR4 modules' SPD data (ADR 0232) | QEMU has no SPD EEPROMs | Dell |
| The console over the xHCI Debug Capability (ADR 0160) | QEMU has no Debug Capability | A debug cable |
| VT-d's blocking of compatibility-format interrupts (ADR 0166) | QEMU passes them through | Dell |
| Machine checks that end only the program that read bad memory (ADR 0168) | Not checked on a machine yet | A machine with error injection |
| The jitter entropy test's strict limit (random design note) | If real machines fail it, adopt jitterentropy's rules | Real machines |
| HD Audio and USB audio on real codecs and devices (`sound/hda`, `sound/usb`, ADRs 0322, 0323) | QEMU's codecs are simulated | Lab |

## Waiting on a phase

### Phase 15: atomic updates and snapshots

| Item | Why it waits | Waits on |
| --- | --- | --- |
| USB repair, which also rewrites GRUB and the EFI system partition, and the installer's reinstall that keeps data (ADRs 0459, 0460) | Rescue covers a machine whose EFI system partition still starts | The installer's next changes |
| Rescue resetting the settings to their defaults (ADR 0460) | Putting back the settings from before the last update covers a bad update | A need |
| Each slot's release list kept beside it, so that `sysupdate check` reads none of the slot's files (ADR 0466) | A check reads the running slot's files whose size matches a wanted one, as a stage does | A need |
| The three cloud canaries (ADRs 0456, 0533) | The reference machine and the lab machines are the canaries: each reports every hour, and one that fails or goes quiet holds the waves | Buying the cloud machines, before 1.0.0-alpha.1 |
| Firmware that drivers load for their cards, in firmware sets, with a per-device check that the driver loaded it (ADR 0468) | Sets hold CPU microcode only; no driver Dukovo has loads firmware yet | A port: the first driver that needs firmware (Wi-Fi, release 1.11) |
| AMD CPU microcode in firmware sets (ADRs 0014, 0468) | The kernel's loader is Intel's; AMD's patches have their own format and loader | A need |
| Disks other than the root's in the health check's device record (ADR 0468) | Data disks, USB disks and discs come and go with their owner, so a missing one says nothing of the update | A need |
| A summary of the last starts on the EFI system partition, for a root Rescue cannot mount (ADRs 0500, 0501) | Rescue reads the root's records, and says how many starts the boot menu counted as not seen through | A need |
| A boot test that starts Repair startup from the install stick's menu entry, on the hotkey R, and one of a repair that finds the partition readable, with only GRUB or its menu gone (ADR 0503) | The `repair-startup` boot test runs the repair from the live system's shell, on a partition whose FAT is overwritten with zeros, and restarts from the stick in place; the entry and the readable case have unit tests, and the lab test waits too (Lab machines and harness) | When the boot tests gain a third boot |
| How far a start got before Spark, in the kernel or mounting the root, written as it is reached (ADR 0503) | Spark marks the starts that reached their services; Rescue tells the rest from the boot menu's count | A need, with the starts' summary on the EFI system partition |
| Rescue, Repair startup and the memory test unlocking an encrypted root with its recovery key or passphrase (ADR 0501) | No root is encrypted yet | Disk encryption, release 1.2 |
| Unattended updates limited to security fixes, and the security advisories each update fixes, shown by `update` (ADR 0502) | Unattended updates take every release, through the waves; the release notes name fixes | The channel listing advisories per release, 16b |
| Pinning the system so that no newer version is taken (ADR 0502) | `update hold` skips one version and `update later` puts a reminder off | A need |
| SFTP in Rescue, to copy files off over the network (ADR 0540) | Rescue's menu copies the users' and shared files to another disk, and its shell is there over SSH; the only session over SSH is the menu, so that no program that faces the network reads the root's files, and sshd refuses SFTP, commands and forwarding there | A need, after the outside security review of SSH into Rescue (16e) |
| A help session across a home router, through a relay (ADR 0540) | `help-session` needs SSH to reach the computer: the same network, a VPN, or a port the router forwards to it; nothing leaves the network or passes through a server that is not the owner's | A decision by the maintainers, on who runs a relay and what it may see |
| A start without graphics on the Troubleshooting menu, and the window manager's screen check (ADR 0501) | No graphical start yet: every start is text | 1.1, with the window manager |
| `changes --since` a date (ADR 0500) | `--between` and `--list` compare any two starts kept | A need |
| Settings migrations, run before the services start, each with tests in both directions (ADR 0454) | No release has changed a settings format yet | The first release that does |
| On a mirror, a try GRUB counts on one disk's EFI system partition is not counted on the other's until the system next writes the state (ADR 0452) | A machine must start from each disk in turn to try an update more than three times | A need |
| When GRUB cannot write the slot state, the menu still titles the try that it then does not start (ADR 0452) | The default starts, as it should; only the title is wrong | A need |
| The failed attempt's crash log kept beside its settings (ADR 0453) | The log of the fallback says what failed | A need |
| Installs made before 0.15.0 keep a menu that starts `@system.a` directly, and are reinstalled to take updates (ADR 0451) | No public install exists before 1.0.0-alpha.1 | A need |
| Rolling a whole place back to a snapshot, such as all of `/users` (ADR 0461) | Restoring by file or folder covers mistakes; a whole place goes back only with every program that writes there stopped | Rescue's next changes |
| Apps restoring their own data from a snapshot (ADR 0461) | An app's view has no snapshot; a person restores its folder with `snap restore` | A portal for apps |
| Retention set place by place in `snap.toml` (ADR 0461) | One retention serves the default places | A need |
| `BTRFS_IOC_GET_SUBVOL_INFO` on a descriptor of a file that is not a folder (ADR 0461) | The C library asks a folder's `subvolume` method; `ls` and `snap` ask of folders | A need |
| Backups encrypted to post-quantum recipients as well as X25519 (ADR 0463) | The age crate's release with them needs another version of `ml-kem` than the one the build already locks | A port: an age release on the locked `ml-kem` |
| Backups to servers that offer SFTP alone, without a shell (ADRs 0463 and 0465) | The server runs `find`, `cat`, `dd`, `mv` and `rm` over SSH, as any POSIX server can, for streams and restic repositories alike | A need |
| Backup keys in the system key store in place of files readable by root alone (ADR 0463) | The key store comes later | Release 1.2 |
| Restoring a whole machine from the installer, for a computer with no Dukovo on it yet (ADR 0464) | `backup restore --machine` runs on an installed system, so a fresh install comes first | A decision: the installer's restore screen |
| Tags checked when Linux's `age` and `btrfs receive` restore a backup (ADR 0464) | The tag is an HMAC-SHA256 beside each file; Linux's tools read the backup without it | A need |
| Checking who wrote a backup with another of its keys, not the recovery key (ADR 0464) | The tag key comes from the destination's own key; other recipients restore with `--unchecked` | A need |
| A local cache of a restic repository's indexes and snapshots, so that a round to remote storage reads less (ADR 0465) | Each round reads the repository's indexes from the destination | A need: a benchmark of rounds to S3 storage |
| Restic's lock files, so that Linux's `restic prune` never runs on a repository during a round (ADR 0465) | rustic's library writes no locks; backup(8) says not to prune from elsewhere while the machine backs up | A port: a rustic_core release that keeps restic's locks |
| S3 addresses in the virtual-hosted style, and temporary credentials with a session token (ADR 0465) | Path-style addresses and an access key's ID and secret are what every S3-compatible store takes | A need |
| Restic destinations on restic's REST server (`rest:` addresses) (ADR 0465) | A share, SSH and S3 cover the servers people have | A need |
| Owner, mode and times of a place's top folder kept when Linux's `restic` restores it (ADR 0465) | restic sets the top folder of a restore itself; Dukovo's own restore keeps them | An upstream change in restic |
| Which advisories a bundle's update fixes, shown before it is taken (ADR 0389) | The sign-in screens list installed versions with known flaws, and the sheet names the version that fixes each (ADR 0504) | The catalog listing fixes per version, 16b |
| The legal checks of the web console (ADR 0514): the terms of use of CISA's Known Exploited Vulnerabilities catalog and of FIRST's EPSS scores, read before the console shows data from them; an accessibility conformance report that states only what testing shows; no comparison of the console with another product that is not proved; and a patent check of its features | Each needs a person's reading or judgement, and none is code; the console ships nothing that one of them has not cleared | A maintainer, before the console ships |

### Phase 16: server platform and web console

| Item | Why it waits | Waits on |
| --- | --- | --- |
| A bundle that offers a socket its program makes itself, as PostgreSQL makes `.s.PGSQL.5432`: ADR 0477 offers only a socket Spark holds, and a program that takes no inherited socket cannot serve on one | Spark would set the group and mode of the socket once the service has made it; the catalog's PostgreSQL is reached over TCP meanwhile | The first bundle that offers such a socket, 16a |
| `batch`: jobs that wait for a low load (ADR 0478) | `at` and `crontab` run jobs at a time; a job that waits for the load to fall needs the load average the scheduler can read, and no job asks for it yet | 16a |
| updated's and backup's own schedules moved to timers (ADR 0478) | They keep their own clocks, which work; a timer in each service's file is the same schedule in one place, and moves them when their settings do | 16a |
| The system's own service files using service_version 2's keys: `requires`, `wants`, `after`, `[ready]` and `[health]` for the system's services, and `service new`'s template writing version 2 (ADR 0470) | A release writes only what the one before it reads (ADR 0454), and 0.15 reads service_version 1 alone; the system's services start by `order` meanwhile, as before | The release after 1.0.0-alpha.1 |
| bundled refusing a bundle whose `requires_os` is newer than the system, with a message to update (ADR 0470) | `requires_os` is informational; a bundle whose service uses version 2's keys says 1.0, and alpha.1 is the first release any machine runs | Before the first release after 1.0.0-alpha.1 |
| A second process started before the first ends (zero-downtime restarts for programs that cannot take an inherited socket) (ADR 0477) | Spark empties a service's domain before it starts the next process, and Spark's held sockets keep the connections waiting meanwhile | 16a |
| A person's own programs reaching a socket a bundle offers to approved programs (`bundle allow`), and approving a bundle's use of one after it was installed (ADR 0477) | No catalog bundle offers a socket to people's programs yet; a bundle installed without the approval is installed again to ask | 16a |
| sshd taking Spark's socket (ADR 0477) | sshd listens for itself and refuses connections while it restarts | 16a |
| The services that still hold the whole tree (`runs-with-your-access`): authd, backup, bundled, cloud-setup, consoled, firewall, guest-agent, serial-console, and updated for the apps it updates (ADR 0473) | Each needs a view that holds what it reaches and no more, which the finer views now make possible; narrowed one at a time where a view suffices | 16a, in A5 to A8 |
| The update server publishing the troubleshoot signatures as the target `troubleshoot/signatures.toml`, an update file carrying them, and the `update-online` boot test checking that a served set is kept (ADR 0474) | The machine's side is done: the daily check and a stage from an update file or a repository read the target, keep it when its version is newer and refuse a file that does not parse; the repository's `publish_with` writes one, but `updates::publish` and the update server's scripts do not yet, so no server has one to offer | Phase 16, with the update server on Dukovo (16e) |
| The Ubuntu update server's files (`tools/update-server/setup.sh`, its systemd units and Caddy's settings) removed (ADR 0542) | They stay beside `tools/update-server/dukovo/` until the move to Dukovo is done, so that going back is the DNS record alone | After the move, `docs/runbooks/update-server-move.md` |
| Accounts for services that no bundle installs, such as the update server's jobs (ADR 0542) | The jobs run as root without its right to pass owners and modes, each in a view of the folders its file names; the incoming folders are the deploying login's | Phase 17 |
| The `server` preset's web console reachable from private networks, and SSH with keys only once a key is added | The preset sets authd's jobs, `tune`, unattended updates at 03:30 and `dukovo-check`; the console's reach comes with its pages (C3) | Ready to do: `console.reach` arrived with the Security page (ADR 0562) |
| The facts of troubleshoot's planted faults saved from machines that really had each fault (ADR 0474) | Each shipped signature is proved by facts written by hand to match what the kernel, Spark and the health check say; a fault made on a machine and saved with `troubleshoot --save-facts` would also prove that what they say is what the signature reads | Real hardware, for the firmware and disk faults; a boot test that makes the others |
| Rescue naming the cause of a root that does not mount from more than its own line (BOOT-105, ADR 0474) | The starts' records and logs are on the root, which Rescue cannot read then; a short summary of the last starts kept on the EFI system partition would give it more to match | A decision by the maintainers |
| troubleshoot of an older start matching on which services had failed and how full the disks were (ADR 0474) | Spark's states and the filesystems' fullness are read as they are now, so only the newest start has them; an older start matches by its record, what changed and its log | A need: a record of each start's failed services and fullness |
| The `catalog-waves` boot test planting a high flaw whose fix goes ahead of the waves (ADR 0544) | The test bundles carry no SBOM, so the test catalog can list no flaw for them; the rule is proved by `fixes_a_flaw`'s unit tests alone | A test bundle with an SBOM, and advisories for its parts |
| A bundle's own feed going out in waves (ADR 0544) | A feed is signed by its publisher's key and served from the publisher's own host, where we run no signer; its updates reach every machine at once | A need |
| The installer installing the install stick's firmware set, and update files carrying one (ADR 0468) | A machine takes the firmware package by staging a set from the update server or a repository folder after it is installed | Phase 16, with the installer's question (16c) |
| A change of the serial console's settings acting at once (ADR 0481) | `serial` writes the settings for the next start: the kernel and serial-console read the command line, which a start makes | A way to change the kernel's serial settings in a running system, which no plan item asks for yet |
| GRUB's menu on the second port of a PCI function, on a port at a parity or 7 data bits, on a fifth port, and on a PCI domain other than 0000 (ADR 0481) | GRUB's `pciserial` runs BAR 0's first port only, GRUB's `serial` 8 data bits and no parity, and the menu has four slots; `serial` says which console is left off the menu and why | A GRUB that runs them, or a boot loader of Dukovo's own |
| The speed of the SPCR's "as the firmware left it" read from the UART's divisor (ADR 0481) | Taken as 115200 baud, as GRUB takes it | A machine in the lab whose firmware leaves another speed and gives none |
| elevate in the web console, with comments kept by its settings editor (ADRs 0216, 0219) | Dukovo Admin | 16c |
| A page for logforward in the web console: its destinations, their status, their TLS and a test record (ADR 0482) | logforward(8) is set up at a terminal; no board of the console shows it yet | A board for it, then 16c's console |
| TLS and a password for the Prometheus endpoint (ADR 0484) | The endpoint is plain HTTP, answering only the networks `allow` names, as most node_exporter setups run it | A need |
| A bundle's service raising alerts (ADR 0485) | alertd serves root's processes and the system's services alone, which covers every shipped service; a bundle's service runs as an account of its own | A need |
| A certificate authority of its own for a webhook in alerts.toml(5) (ADR 0485) | Webhooks trust the system's roots; a private authority is added to them as dukovo-certificates(7) says | A need |
| A mail queue for sendmail(8) (ADR 0478) | sendmail sends a message at once through the submission server alerts.toml(5) names, and a message that cannot go is the command's failure, with sysexits(3)'s status and a line in the log; cron's mail and a script's are lost unless the caller tries again | A need |
| Silencing an alert for a while (ADR 0485) | An alert is told only when it changes, so one that stays does not repeat; `severity` and `cleared` in alerts.toml(5) quiet a channel | A need |
| A check-in from a second Dukovo server, which alerts when this one stops checking in (ADR 0485) | A machine that stops sends nothing: alertd runs on the machine it watches, and nothing outside it notices | The web console managing more than one server; a lab test with two machines |
| DNS providers' APIs, so that a certificate proved by DNS-01 (a wildcard, or a name on a machine no router forwards port 80 to) renews by itself (ADR 0513) | `cert get --dns` has a person add the TXT records, and certd does not renew a certificate proved that way: each provider has its own API and credentials | A later phase, which picks the providers most people use |
| External Account Binding, which ZeroSSL and Google Trust Services ask for before they make an account (ADR 0513) | `cert get --directory` reaches any other ACME authority that makes an account without it | A later phase |
| TLS-ALPN-01, the check of a name made over port 443 (ADR 0513) | It needs port 443 free or a TLS hook in nginx; HTTP-01 and DNS-01 cover the cases | A later phase, if an authority comes to ask for it |
| Certificates for IP addresses (ADR 0513) | Let's Encrypt gives them only on its short-lived profile; `cert get` asks for DNS names | A later phase, with `--profile` |
| A service's choice of being restarted instead of sent SIGHUP when its certificates change (ADR 0512) | Every server Dukovo ships reads its certificates again on SIGHUP: nginx, PostgreSQL and admind | Zero-downtime restarts (A4) |
| A second step at SSH and at the consoles' text sign-in (ADR 0514) | The console's sign-in rules and each account's second steps are authd's, where the consoles' and SSH's sign-ins already are, so a later change adds the step to `login` and to sshd's keyboard-interactive sign-in without moving anything; `second-step` in admin.toml(5) rules the web console alone meanwhile | A need, before 1.0.0-beta.1 |
| The keyring opened by a sign-in to the web console (ADR 0514) | authd opens a keyring when a person signs in with their password at a console or over SSH; the console's sign-in checks the password and opens none, and the console's pages use no keyring entry; `keyring unlock` opens it at a terminal | A page that needs an entry of the keyring |
| Web fonts for the console (ADR 0514) | None ships: the console's style falls back to the system's fonts, and its security policy allows fonts from its own folder alone | A decision by the maintainers |
| Terminals that outlive admind, and `term list` and `term attach` from SSH (ADR 0517) | A terminal's panes are admind's, and end when the console restarts or stops; the routes and the socket's messages are shaped so that a keeper process can take the terminals over without the page changing | A keeper process, to be made after the console's pages and before 1.0.0-beta.1 |
| A pane's exit status on the Terminal page (ADR 0517) | authd reaps the shell and admind holds only its terminal, so a pane that ends as `exited` has no `status`; authd's audit log has the status | A need |
| The browser's language for a pane's shell (ADR 0517) | admind asks for each shell with no language, so it starts in the machine's; authd takes language settings when they come | The console in languages other than English |
| A look at keys typed while nukesh waits for a cursor answer after a resize (the boot test admin-terminal, the lab test web-console/terminal) | After a resize, nukesh's line editor draws its prompt again, asks where the cursor is and waits up to 2 s for the answer; a line typed in that wait did not reach the shell, from a test client that had not answered and, once, from the Terminal page in Chromium, which answers at once. The boot test reads for 5 s before it types, and the lab test waits until the screen has not changed for 2 s. A pane resized again within that wait may also have its prompt drawn over the lines above it, its last output among them: the lab test lost a line so when it chose a layout less than a second after the one before, and now waits until no pane's screen has changed for 2 s after each | A need: a person who loses keys or output after splitting a pane, changing the layout or resizing the window |
| WebAuthn attestation checking (ADR 0514) | authd takes any passkey of a kind it checks, ES256, EdDSA or RS256, and does not check who made the key, so it cannot limit passkeys to certain makes | A need, for an organisation that allows only certain keys |
| Modules of the `dukovo.system` Ansible collection for the actions that have none: `certificate.renew`, `console.reach` and `console.certificate` (ADRs 0515, 0545, 0567) | Show's task for the services' changes is the collection's `dukovo.system.service`, which reports "changed" only when it changed something; the task for these is `ansible.builtin.command`, which cannot tell a change from no change, so it reports "changed" on its second run too, and the lab test web-console/ansible says so | A need: a playbook that renews a certificate, sets the console's reach or sets the certificate the console serves and must report no change when there is none |
| The web console's lab tests run on QEMU in CI for every PR (ADR 0510) | CI runs no lab tests yet; they run on QEMU before each PR that changes the console | A CI job that runs lab tests on QEMU |
| Screenshots of the real web console for dukovo.org, saved by the lab's web-console category at each release: the duke-nas demo data, dark and light, of the boards the site shows (Overview, Services with Show commands, Reliability, Firewall, the phone views and Move to new hardware); then a clickable demo mode of the console (ADR 0510) | The site shows drawn pictures of the console's boards | The console's pages (C3 to C6) for the screenshots; 1.0.0-alpha.1 for the demo mode |
| Changing a service's file, limits or timer from the Services page (ADR 0518) | No command sets one key of a service file; `service try NAME` opens an editor, so the page shows the limits, the file's path and the command to change them | A `service NAME set KEY VALUE` command, after the console's last page PR |
| Keeping or undoing a `service try` trial from the Services page (ADR 0518) | The page shows the trial and the commands that keep or undo it, since it cannot begin one (the row above) | The row above |
| A per-person view of the Services page | Spark runs no services for a person, only the system's | A need |
| The rest of network(5) from the Networking page: IPv6 choices, temporary addresses, the DHCP server and its reservations | `network set` leaves those keys as they are | A need |
| The rest of a kernel crash dump sent with a problem report, with the person's consent (ADRs 0483, 0531) | A report of a kernel panic holds the dump's `== Panic` section, the report and backtrace the consoles showed; the sections with the kernel's stacks, threads and locks hold what the kernel held, secrets too, and stay on the machine | A need |
| Tests of displays in `hardware check`: colours and patterns that show dead or stuck pixels, and the backlight (ADR 0488) | A test draws on the screen and needs the person to look at it, which takes the window manager (windowd) | Release 1.1, with the window manager |
| `hardware check` saying that a network link is slower than the card and its partner both offer, HW-151 (ADR 0488) | The kernel shows a card's current speed and duplex alone, not the speeds the card offers or its partner does, so a cable that makes a link negotiate down looks like a slow port | A driver that reports what the card and its partner offer |
| `hardware check` counting a card's receive errors, HW-153, on a driver that does (ADR 0488) | The check reads `rx_errors` of each card, which the kernel gives as 0 for every card, since its drivers count only the frames they drop | Drivers that count frames with a bad checksum or length |
| A panic of a program that no service runs, in a problem report (ADR 0531) | reportd finds a panic in the log lines that services write; a command or an app that panics prints to its terminal and leaves no record | A need |
| Attachments to a problem report, such as a screenshot or a file (ADR 0531) | A report is text, and the server takes text alone | A need, and for screenshots the window manager (release 1.1) |
| The status of a sent report checked with the update check, and the update notes saying that an update "fixes a problem you reported" (ADR 0531) | `report status` asks the server when it is run; updated does not ask it | 16b, with the reports server |
| The Report a problem page saying that the reports server keeps no IP address and deletes reports after a year (ADR 0531) | The page says what is sent and where, and the privacy chapter states the server's rules; no report desk exists that the page could state them for, and reportd only sends | 16b, with the reports server |
| Outbound rules that limit reportd to its server (ADR 0531) | reportd may reach any address, as its service file gives the network whole; the firewall's rules do not name a service | A need, with the firewall's rules per service |
| The phone download's port opened in the firewall for the download alone, and closed after it (ADR 0532) | The console's Report a problem page hands the text to a phone by a one-time link on the console's own port, which needs no rule. `report`'s own download at a terminal (`P`, or `--save` with no file) listens on a port of its own, which is for the machine alone: report prints the command that allows the port, and `--port` lets an administrator allow it first | A need, with rules that last as long as a socket |
| Taking back a report that waits in the queue (ADR 0531) | A report waits until it is sent or 30 days pass; `report list` shows it | A need |
| `changes bisect` undoing a change to the settings, a service file or a setting edited by hand, from the snapshots (ADR 0475) | A try leaves out the slot, the boot options and an app's services; the start record holds no settings, which `service check` and `try` cover meanwhile | A need: a failed start that no change in the record explains |
| `changes bisect` finding two changes that break a start only together (ADR 0475) | The search names one change; when the start still fails with that one left out, it says so and `changes --failed` lists every change, and trying pairs takes more restarts | A need: a search that ends so |
| `changes bisect` after an update fell back (ADR 0475) | The update's slot is then neither the default nor the previous version, which the boot menu starts, so a try cannot start it; the failure notice offers no search, and the log says why the update failed | A need |
| A setting for the 60 seconds between a try's verdict and the next restart of `changes bisect`, for a machine nobody sits at (ADR 0475) | The wait lets whoever is at the machine stop the search, and no machine has asked for another | A need |
| A website that passes requests to a program, or sends visitors to another address, added from the web console | nginx's `network.outbound` is optional and is given only with `bundle grant` at a terminal, so the Add a website page cannot make a site that reaches a program or another address | A way for an administrator to give a service bundle's permission from the web console |
| The catalog's summaries and categories in the search of Add an app | The catalog's index holds an app's ID, version and name only, so the search matches those and the titles of the setups | A new index format of the catalog (ADR 0383) |
| The check of a backup opening a database's copy, to see that `pg_restore` reads it (ADR 0570) | `backup verify --restore` restores the newest backup of `/shared` and compares each file with the snapshot it was made from, a database's copy among them, and nothing opens a copy; a copy is tested by restoring it as a new database | A need |
| A copy of a database on a schedule of its own, apart from backups (ADR 0570) | A database is copied before each backup and never on its own, so a machine with no backup place has no copies, and what a database changes between two backups is in no copy. A timer that runs `postgresql-backup` would make them, with a decision where they are kept and for how long | A need |
| A restore that brings back the subscriptions of the copy (ADR 0570) | `database restore` passes `--no-subscriptions` to `pg_restore`, since a subscription connects to another server as soon as it is made, and a restore into a new database beside the first would connect it as well. It would take a choice for each subscription, asked of the person | A need |
| The retry of a certificate that could not be got, tried against an authority that grants it (ADR 0570) | The boot test `admin-certificates` makes a record by hand and has certd try it, which fails because the name leads nowhere, and so tests the schedule, the history and the alerts; no test has a try get a certificate from an authority, the lab's or a public one, whose limits and `Retry-After` the retry has to follow | A need, with a lab machine that the internet reaches on port 80 by a name that leads to it |
| The counts of the days a machine was off, for a website (ADR 0570) | `website count` counts one day a run, yesterday's, and logturn keeps two turned copies of a site's log, so a machine that was off for more than a day counts only the last day, and the older turned logs are cut to two. A count of each day the turned files still hold, with a record of the days counted, would fill the days between | A need |
| A service's `logs` taking more than one entry, each with its own number of turned files to keep (ADR 0570) | A service has one `logs` entry and one `keep`, so the `keep = 2` that nginx's service file sets for the websites' request logs, which are kept for about two days, also cuts any log file of nginx's own that an administrator names in `logs/*.log` from the usual 14 turned copies to 2. With an entry for each pattern, nginx's own files keep 14 while the sites' keep 2: the manifest's service schema, Spark and logturn change | A need: the next change to logturn or to the service schema |
| The Updates page saying that Rescue is being written, and Go back waiting for it (ADR 0454) | After an update proves itself, Rescue is written again from it, which holds the slot state for a few minutes, and `update undo` waits for it; the console runs the command for at most 30 seconds, so Go back pressed meanwhile says that the command did not end in time, and works when pressed again once Rescue is written. The lab test `web-console/updates` waits for the write before it goes back | `sysupdate status --json` telling a write of Rescue under way, so that the page can say so and the action can wait for it as a task |
| Audit lines kept whole: the kernel's log keeps the first 200 bytes of a line, so authd's line for a long elevated command loses its end, and the web console's journal cannot name who changed a file that such a command names after them (ADRs 0210, 0516) | Found by the lab tests `web-console/journal` and `web-console/overview`, whose hand edits are now each a short command of its own; a person who edits a file names it early in a short command, as `elevate vi PATH` does | A decision: a longer record in the kernel's log, or a way for authd's audit lines to reach logd whole |

### Phase 17: Go

| Item | Why it waits | Waits on |
| --- | --- | --- |
| An interface's multicast groups: radix-sysfs does not show the groups an interface has joined, so Go's `net.Interface.MulticastAddrs` lists none, and Go's interface tests name Radix among the systems that list none (ADR 0355) | No program found that reads them | Need |
| cgo on Dukovo itself: the bundle's `go` builds with `CGO_ENABLED=0`, so a package with C code builds only with `ob` on the build machine (ADRs 0356, 0357) | Dukovo has no C compiler for `go` to call | Release 1.17 of the roadmap (Clang) |
| Go's race detector, `go build -race` and `go test -race` (ADR 0357) | No race runtime for Radix | An upstream race runtime for Radix, or a port of it |
| `go build` as fast as on Linux, held in `ob bench` | The bench's Linux machine has no Go toolchain to compare with | A Go toolchain in the bench's Linux machine |
| Go's build modes `c-archive`, `c-shared`, `shared` and `plugin` (ADR 0357) | Go's table of platforms does not enable them for Radix, and Dukovo loads no library at run time for Go code | Need |
| File watching in Go programs: fsnotify has no backend for Radix, so micro and lazygit do not see a file another program changes until they are asked to look again (ADR 0600) | Radix's kqueue has no `EVFILT_VNODE`, and Radix has no inotify (ADR 0353) | Need |
| A clipboard for micro and lazygit: their clipboard modules take their "unsupported" paths (ADR 0600) | Dukovo has no display server | Release 1.1 of the roadmap (windowd) |
| Caddy as a choice of the admin app's Websites and of `website(8)`, beside nginx (ADR 0601) | nginx serves them (ADR 0491) | Need |
| HTTP/3 from Caddy's service, over a UDP socket on port 443 that Spark holds and Caddy binds as `fdgram/N` (ADR 0601) | Spark's UDP sockets are not yet tried with Caddy | Need |
| Zero-copy file transfers in Go programs (`sendfile`, `splice`, `copy_file_range`): `io.Copy` between files and sockets reads and writes through a buffer (ADR 0350) | The C library has none of them | Need |
| `go test -fuzz` for Radix: Go's fuzzing shares memory with a mapped file between processes (ADR 0350) | Shared writable mappings of files (see Files and storage) | Need |

### Phase 18: Linux compatibility and containers

| Item | Why it waits | Waits on |
| --- | --- | --- |
| `epoll`, `eventfd`, `signalfd`, `timerfd`, `memfd` and `pidfd` (posix design note) | Built as native objects with the Linux layer | 18a |
| Neovim and Midnight Commander, as their official Linux builds | Moved off the native port list | 18a |
| Linux's ALSA programs playing sound (ADR 0320) | Need the compatibility layer | 18a |
| The Linux layout (`/dev`, `/proc`, `/sys` as directories) inside Linux programs' view (ADR 0113) | Served by `linuxd` | 18a |
| nginx on the kernel's event queue, with sendfile and thread pools: it waits with `poll` and copies files through its buffers (ADR 0491) | Built as for a POSIX system until Radix has them | 18a |
| Valkey on the kernel's event queue: it waits with `poll`, on an event loop of the port's own (ADR 0492) | Built as for a POSIX system until Radix has one | 18a |
| The `data` of the event queue's read and write events, which `kevent` reports as 0: the bytes to read, the room to write, the connections waiting (ADR 0353) | nginx's kqueue module reads them, and nginx moves to the event queue with the nginx row above | 18a |
| The event queue's timer, signal, process and file filters (`EVFILT_TIMER`, `EVFILT_SIGNAL`, `EVFILT_PROC`, `EVFILT_VNODE`): a change that names one fails with EINVAL, so PostgreSQL's wait on events, libevent and Python's `select.kqueue`, which name them, are built without kqueue and wait with `poll` (ADR 0353) | Built with `timerfd`, `signalfd` and `pidfd` on the queue | 18a |

### Phase 19: server hardware

| Item | Why it waits | Waits on |
| --- | --- | --- |
| Native PCIe control through `_OSC` (ADR 0660) | Taking control of PCIe errors and hot-plug before anything handles them would hide errors the firmware reports today; the firmware keeps control until then | Phase 19 steps 4 (hardware error reporting) and 14 (disk hot-swap), each asking for the control it handles |
| More than 256 CPU threads and more than 8 NUMA nodes (ADR 0660) | Each address space's 4 KiB frame holds one 64-bit word per CPU to track which CPUs loaded its TLB generation (`Space::seen`), and that frame is full at 256 CPUs; the array needs a new design before the limit can rise | Phase 19 step 1b |
| Correcting a TSC warp between packages (ADR 0660) | The kernel measures the first CPU of each other package against the boot CPU and logs the result, but does not correct it, as Linux's `tsc_sync` does; no machine seen so far shows a warp to test a correction on | A machine that shows a warp between packages |
| NUMA tested beyond QEMU's emulated nodes: every device on its node, and memory placement, on two sockets | QEMU's nodes are emulated, and the lab has no two-socket machine yet | Phase 19 step 15, on the reference server |
| Machine-check counts by DIMM slot | The kernel counts errors by bank, and names no memory module | Phase 19 step 4 |
| SFP+ modules: their information, and fiber ports | No driver reads a module's EEPROM, and the cards with cages are the server cards | Phase 19 step 12 |

### Releases after 1.0

These come from finished work; the releases themselves are in the
[roadmap](roadmap.md#post-10-roadmap).

| Item | Why it waits | Waits on |
| --- | --- | --- |
| The graphical sign-in, its startup-trouble menu and its elevate prompt; the version on the graphical sign-in (ADRs 0251, 0253, 0254, 0261) | Need the window manager, and come with its lock screen | Release 1.3 |
| The Precision Touchpad's reports (fingers, scrolling, gestures), touchpad sleep and wake (ADR 0235) | The desktop reads them | Release 1.3 |
| The lid lock, brightness keys and sleep; the sleep button; wake events (ADRs 0142, 0201) | Power management with the desktop | Release 1.3 |
| Mouse selection in the consoles, and fonts beyond Spleen's (ADR 0100) | Console polish | Release 1.1 or a need |
| A monitor's name, size and modes on the firmware's framebuffer, so its size in millimetres is unknown there (ADR 0705) | The boot loader passes none of the monitor's data | Release 1.10 |
| Drivers for the other virtual video cards: a VGA card whose mode can be changed, VMware's, VirtualBox's and Hyper-V's (ADR 0705) | Each keeps the firmware's picture in 1.1, and is a small driver on the display layer when its turn comes | A need |
| A graphics card with no device behind it, for a desktop on a machine with no screen (ADR 0700) | Belongs with remote desktop | Remote desktop |
| Turning a screen off after a time (ADR 0700) | Power management with the desktop | Release 1.3 |
| A desktop that stands in a console again after consoled was started again under it: the desktop goes on with the screens and the keys, and Ctrl+Alt+F1 to F12 show no console until it ends (ADRs 0713, 0716) | The desktop has to hand the new consoled its holder and its console, which is the session's work | Release 1.1 step 5, with the desktop as a session |
| Keyboards and pointers plugged in after inputd was started again under a desktop: those it had handed over are heard on, and new ones are not (ADR 0715) | The desktop has to ask the new inputd, which answers with every device again | Release 1.1 step 5, with the desktop as a session |
| Who a program in the desktop is: windowd is not told the account or the process of a connection (ADR 0712) | wayland-backend reads them with a Linux socket option; nothing asks until a window may be given more than another | Release 1.1 step 5, with the grants |
| Popups that take a grab, so that a click outside a menu closes it (ADR 0712) | A menu shows and works without one; the grab comes with the windows' own frames and their focus rules | Release 1.1 step 4 |
| presentation-time and viewporter for the desktop's programs (ADR 0714) | A program is told that a frame showed, by the frame callback, and not when; the first programs that ask for the time are step 4's | Release 1.1 step 4 |
| Windows left where no screen shows once a monitor is pulled out, and a choice of how the screens lie (ADR 0714) | The screens lie left to right in the display layer's order; moving windows comes with the window manager | Release 1.1 step 4 |
| A setting for the mouse's speed and for the keys' repeat in the desktop (ADR 0715) | One curve and the consoles' repeat serve until windowd has its settings file | Release 1.1 step 4, with windowd.json |
| The desktop's rows beside a Linux compositor: a window's frame and a key to its program (ADR 0714) | The frame's way is the display layer's, whose rows are measured (ADR 0709); the compositor's own rows need a second compositor in the Linux image | Release 1.1 step 4 |
| A pointer picture larger than 64 by 64 pixels, and pointers a program asks for by name (ADR 0715) | The pointer planes of release 1.1 take one size, and windowd has one arrow | Release 1.1 step 7, with the looks |
| The XKB compose tables, and the layouts' names in other languages, in the desktop (ADR 0712) | xkeyboard-config's layouts are installed alone; dead keys of a layout work without the tables | Release 1.31 (languages) |
| A console layout of a person's own in the desktop's programs, which get English (US) for it (ADR 0712) | The consoles' layout files and XKB's describe a keyboard differently, and nothing translates one to the other | A need |
| Ctrl, Alt and Delete pressed one after another with sticky keys as the secure key while a program has the screens (ADR 0708) | Sticky keys are consoled's, which reads no key while a program has the screens, and the three keys are never down together for the kernel to see | Release 1.3, with the desktop's sticky keys |
| A reader of a USB or PS/2 input device running after its driver's thread, on that thread's CPU, as a virtio device's does (ADR 0710) | Their threads serve other devices, or go on to other work after a report, so a reader queued behind one could wait; what a sleeping CPU costs a motion on a machine of its own is measured first | Release 1.1, with the window server's input rows on the lab's machines |
| A wheel's fine steps on a mouse or touchpad that speaks HID over I2C: it is counted in notches (ADR 0706) | Both USB host drivers ask a wheel to count finer than a notch; the I2C driver comes with the touchpads | Release 1.3 |
| Touch screens, touchpads' fingers, pens' pressure and tilt, and game controllers as kinds of input device with their own codes (ADR 0706) | The files and the hub take them as they are; each needs its driver's reports decoded and a reader | Releases 1.3 and 1.26 |
| Console layouts whose letters are not Latin, with a second group (ADR 0149) | Needs group switching in the consoles | Release 1.31 (languages) or a need |
| A sound server that mixes, as the one player (ADR 0320) | Full audio | Release 1.13 |
| Version 2 of the bundle manifest: `entries`, `actions` and `display` (ADR 0221) | Desktop integration | Releases 1.3 and 1.4 |
| The Trash for deleted bundle data; installing by dropping a file into `~/applications` (ADR 0223) | The file manager | Release 1.7 |
| Optional permissions asked through the desktop's portals (ADR 0224) | Portals | Release 1.7 |
| Secrets sealed by the TPM (ADR 0217) | TPM support | Release 1.2 |
| Password reset with the recovery key once disks are encrypted (ADR 0253) | Disk encryption | Release 1.2 |
| Bundle signing keys with passwords (ADR 0222) | Key store | Release 1.2 |
| zellij | Needs Wasmtime | Release 1.19 |
| Ogg Opus and Vorbis for music CDs (ADR 0447) | Needs the media service's libopus port | The media service |
| Crash dumps of all the kernel's memory, which a post-mortem tool reads with the kernel's symbols (ADR 0483) | The text dump holds the state a person reads without a tool; a dump of memory is as large as the RAM and needs somewhere to be written from the panic path | Release 1.27 |

## Waiting on a port, an upstream or a description

| Item | Why it waits | Waits on |
| --- | --- | --- |
| libxkbcommon 1.13.2 and later: the port is 1.13.1 (ADR 0712) | Their build asks for Meson 1.4, and the build machines and CI have Ubuntu 24.04's 1.3 | The build machines' next Ubuntu |
| The rest of the Rust batch: fd, hyperfine (Linux perf events gated off), dust, delta, eza, zoxide, tealdeer, starship (Radix sources for `sysinfo` and the battery crate) | Paused after ripgrep and bat | Approval to continue |
| The C and C++ batches: make, bash, zsh, fish, Vim, Perl, GnuPG, htop, btop with a Radix collector, ncdu 1.x, mosh, an IRC or Matrix client | Follow the Rust batch | Approval to continue |
| delta in the catalog | Needs libgit2; its xdiff is LGPL, to be checked | License review |
| yazi in the catalog | Needs tokio support | Port work |
| socket2's Radix patch to offer TCP keep-alive's interval and count, then admind's own setsockopt for TCP_KEEPINTVL (`probe_every` in admind/serve.rs) removed | The patch predates the kernel's TCP_KEEPINTVL and TCP_KEEPCNT (#360) and leaves them out for Radix; changing it rebuilds the toolchain's crates | The next change to ports/rust-crates |
| nmap in the catalog | NPSL license | Legal review |
| Advisories for ecosystems other than crates.io (ADR 0389) | Each ecosystem's advisories need a source of their own; npm's export no longer waits on the `.dkv` format (ADR 0590) | A need |
| mlibc functions with no code, which end the program (posix design note) | Each is written when a port needs it, upstream where general | A port |
| `crypt(3)`, for nginx's `auth_basic` with `$5$` and `$6$` hashes (ADR 0491) | The C library has none; nginx's own schemes, `$apr1$` among them, work | A port that needs it, or upstream |
| jemalloc for Valkey, with its active defragmentation: Valkey uses the C library's allocator (ADR 0492) | jemalloc has no port for Radix | A jemalloc port |
| Valkey's own test suite (`runtest --host`) run against the machine from the lab PC; the lab test `services/valkey` stores and reads back keys with a client of its own meanwhile (ADR 0492) | Needs Tcl and Valkey's tests at the bundle's version on the lab PC; many of them need the `DEBUG` command, which Valkey keeps off unless its settings turn it on | A lab step that fetches them |
| Python's modules that need a library Dukovo has no port of: `ctypes` (libffi), `dbm.gnu` and `dbm.ndbm` (GDBM, Berkeley DB), `readline` (GNU Readline or libedit; libedit is now ported, so this one waits only on Python's build), `uuid`'s C part (libuuid), `tkinter` and IDLE (Tk) (ADR 0495) | Each module is built into `python3` when its library is | A port of each library |
| Python packages with code in C, from PyPI (ADR 0495) | Radix loads no library at run time; Python's own modules in C are built in | Loading libraries, or a bundle that builds a package in |
| The kernel's gaps CPython's suite finds (ADR 0495, `lab/tests/services/python-skips.txt`): `mmap(2)` of tmpfs files; multiprocessing's three suites, which needed Unix-domain sockets passed with `SCM_RIGHTS` (ADR 0525 gives them) and wait for a run of them that passes; RFC 3542 IPv6 ancillary data, and urgent data sent with `MSG_OOB` (ftplib's `abort()`), which smoltcp does not carry (ADR 0496); `scandir` results | Found by the suite, not by a program that needs them yet | Kernel work, each removing its lines from the skip list |
| PHP's extensions that need a library Dukovo has no port of: `gd` (libgd, with libpng, libjpeg-turbo, libwebp and FreeType), `intl` (ICU, which PostgreSQL's ICU collations need too, ADR 0521), `zip` (libzip), `sodium` (libsodium), `gmp` (GMP), `iconv` (the C library has no `iconv(3)`) and mbstring's regular expressions, `mb_ereg` (Oniguruma) (ADR 0499) | Each is built into `php` when its library is | A port of each library, and `iconv(3)` in the C library |
| PHP's `sqlite3` and `pdo_sqlite` (ADR 0499) | They needed SQLite, which is now ported (ADR 0511); PHP's build does not turn them on yet | Ready to do: PHP's build |
| PHP's JIT, for PCRE and for opcache (ADR 0499) | Both want memory that is writable and executable at once, which a program's pages never are on Radix; nginx's PCRE2 is built without JIT as well (ADR 0491) | A decision to allow such mappings, or a JIT that does without |
| PHP's `dl()`, and extensions from PECL (ADR 0499) | `php` is a static program that loads no library at run time | Loading libraries, or building PHP with `-dynamic` (ADR 0071) for a catalog that needs extensions of its own |
| PHP's `dns_get_record`, `dns_check_record` (`checkdnsrr`) and `dns_get_mx` (`getmxrr`) (ADR 0499) | They need `res_search(3)`, which the C library lacks | `res_search(3)` in the C library, or a resolver of PHP's own |
| The system's gaps PHP's suite finds (ADR 0499, `lab/tests/services/php-skips.txt`): the C library's `gethostbyname_r(3)` finds nothing (PHP is built to use `gethostbyname(3)`, which does), nor do its `getprotobyname(3)` and `getprotobynumber(3)`; its `getservbyport(3)` ignores the protocol, `wait3(2)` is not implemented and `sysconf(3)` lacks some names; the socket options `TCP_CONGESTION`, `SO_ZEROCOPY`, `IPV6_PKTINFO` and `IPV6_RECVPKTINFO`; `getaddrinfo(3)` of an empty name, which glibc refuses; the machine's own datagrams to an interface-local IPv6 multicast group one of its sockets joined; the ioctl that gives an interface's address by its number; a SIGCHLD's sender's user number; `fsync(2)` of a descriptor that is not a file; `utimes(2)` of a time before 1970; `getcwd(3)` in a removed folder; `iconv(3)`, without which libxml2 reads and writes only the encodings it knows itself, not Windows-1251 or SHIFT_JIS; a `locale(1)` command | Found by the suite, not by a program that needs them yet | Kernel and C library work, each removing its lines from the skip list |
| PHP-FPM's slow log, `request_slowlog_timeout` (ADR 0520) | It reads a worker's stack with `ptrace` or `/proc/PID/mem`, which Radix does not offer | `ptrace` or `/proc/PID/mem` |
| Installing a service bundle's commands for one person, without its services: PHP's `php` and PostgreSQL's `psql` (ADR 0385, ADR 0520, ADR 0522) | A bundle with services installs only for everyone, so a person who is not an administrator cannot have PHP's command line or `psql` for themselves | A decision by the maintainers on how a bundle offers its commands alone |
| PHP-FPM pools that run as different accounts (ADR 0520) | PHP-FPM must start as root to give a pool another account, and a bundle's service never runs as root (ADR 0385); every pool runs as `php` | A decision to let a service start as root and drop to accounts of its own |
| PostgreSQL's PL/Perl, PL/Python and PL/Tcl (ADR 0521) | There is no Perl port, Python's is static and has no `libpython` to embed, and Tcl is ported but PostgreSQL is built `--without-tcl` | A Perl port, a Python that can be embedded, and PostgreSQL's build for PL/Tcl |
| PostgreSQL's JIT compilation of queries, which uses LLVM (ADR 0521) | There is no port of LLVM's libraries | An LLVM port |
| PostgreSQL's contrib modules `uuid-ossp` (libuuid), `xml2` (libxslt) and `sepgsql` (SELinux) (ADR 0521) | Each is built into the bundle when its library is; Dukovo has no SELinux | Ports of libuuid and libxslt |
| PostgreSQL's `io_method = io_uring` (ADR 0521) | Radix has rings of its own, not Linux's `io_uring`; PostgreSQL reads asynchronously through its worker processes meanwhile | A decision to offer Linux's `io_uring`, or a PostgreSQL that uses Radix's rings |
| PostgreSQL's TAP tests, `src/test/*/t` and the contrib modules' (ADR 0521): recovery, replication, authentication, `pg_upgrade`, `pg_basebackup` and the programs' own | They are Perl scripts, and there is no Perl port | A Perl port |
| PostgreSQL's messages in languages other than English, `--enable-nls` (ADR 0521) | The ports are built without native language support | A need |
| What PostgreSQL's build works around (ADR 0521, `ports/postgresql/build.sh`): the processes' titles in `ps`, such as `postgres: checkpointer`, which Radix has no way to set; `sync_file_range(2)`, without which the `*_flush_after` settings are 0; and `backtrace(3)`, which looks for a `libgcc_s` that a bundle does not have, so backtraces are off | Found by the build, not by a program that needs them yet | Kernel and C library work, each removing its workaround |
| A cap on the memory a user's pipes hold (Linux's `pipe-user-pages-soft`) (ADR 0497) | Pipe buffers are kernel memory, not yet charged to a resource domain | Charging kernel memory to resource domains |
| The RTS5260 card reader; UHS-I, MultiMediaCards and Memory Sticks on Realtek readers (ADR 0448) | Only GPL drivers describe them | Realtek's documentation or a BSD driver |
| The RTL8127ATF, the RTL8127 with an SFP+ cage (ADR 0581) | rge(4) refuses it: OpenBSD's driver sets up no SerDes for the cage, and Linux's setup is under the GPL | A BSD-licensed description of its SerDes setup |
| AML fixes reach Dukovo when uACPI is vendored again (ADR 0200) | `aml=off` works around a firmware table uACPI cannot run until then | Upstream |
| eSpeak NG is pinned to a commit of master after 1.52.0 (`ports/espeak-ng`, version `1.53.0~git20261108`); the pin moves to the 1.53.0 release | No release holds the fixes of nine memory-safety flaws that OSS-Fuzz's records name, and `ob ports outdated` does not watch a port pinned to a commit alone, so the move is done when the release is tagged | The 1.53.0 release |

## Waiting on a need

The decision records leave these until a benchmark, a program or a
machine asks for them.

### Kernel and memory

| Item | Source |
| --- | --- |
| Spark's pet of the hardware watchdog came more than 10 s late while nginx served the lab PC's load on the OptiPlex 7040: what holds Spark's loop up (nginx's access log copied into the system's log is the first suspect) | Lab, services/nginx on the OptiPlex, 2026-11-13 |
| Transparent huge pages for anonymous memory | ADR 0121 |
| Batched and lazy TLB shootdowns | ADR 0020 |
| `getrusage`'s `ru_maxrss`, a process's peak resident memory in KiB, as on Linux and the BSDs: the C library leaves it 0, as the kernel keeps no peak, so Go's `runtime/pprof` heap profiles say `MaxRSS = 0` | Go's test suite, ADR 0358 |
| `/devices/shm`'s directory keeps its names in one array in the order they were made, with an index sorted by name, as tmpfs did before ADR 0367 kept its entries in pieces: it holds at most 32,768 shared memory objects, and removing one moves every one after it | ADR 0367 |
| Frames grouped by how long they live, and compaction that moves pages to join free blocks, so that large blocks stay likely as memory breaks up: for the heap's slabs, blocks of up to 32 KiB, and for devices' memory; the heap's large requests already fall back to areas | ADR 0524 |
| A lazy IOMMU mode | ADR 0164 |
| Memory hot-plug | memory design note |
| Swap files; zram or zswap; read-ahead on swap-in; reading the dirty bit so clean pages are not written | ADR 0152, memory design note |
| Lower page tables charged to the process's resource domain | processes design note |
| Kernel objects (threads, page tables) retyped from a process's own memory | capabilities design note |
| radix-rt's heap giving memory back | processes design note |
| The boot archive mapped from its module's pages rather than copied | ADR 0049 |
| Cold pages of programs' files that every process shares (kept pages) dropped under memory pressure and read again from the file when touched; they are shared, so never swapped out, and stay while a process maps them | ADR 0563 |
| A kept page of a program's file whose memory went bad taken from every space that maps it, to be read again from the file at the next fault, rather than each process that reads it getting SIGBUS | ADR 0563, ADR 0168 |
| Reseeding the random number generator from fresh entropy | random design note |
| A tree in place of scanned run-queue vectors | scheduler design note |
| The tree lock under heavy capability passing on many CPUs | ipc design note |
| Queued messages charged to a resource domain | ipc design note |
| A process's handles freed before its parent is told it ended | ADR 0309 |
| System V message queues (`msgget`, `msgsnd`, `msgrcv`), which PHP's `sysvmsg` extension needs; libc-test's `ipc_msg` | ADR 0490, ADR 0499 |
| Folders and symbolic links in `/dev/shm` | ADR 0490 |
| A memory filesystem, and so System V keys, for each account or view, as Linux's IPC namespaces: every service shares `/devices/shm`, as Linux's services share one namespace unless told otherwise | ADRs 0490, 0522 |
| `ipcs` and `ipcrm`; `IPC_INFO`, `SHM_INFO`, `SEM_INFO` and the `*_STAT` commands that take an index | ADR 0490 |
| `shmat`'s `SHM_EXEC` with write access, which Radix never maps together | ADR 0490 |
| An event queue's watches ending when one descriptor of their description is closed, as on the BSDs: Radix ends a watch with the description, as Linux's `epoll` does | ADR 0353 |
| An event queue watching another event queue: adding one to a queue fails with EINVAL | ADR 0353 |
| The crash dump written to a disk from the panic path through polled drivers, so that a power cycle or a firmware that clears the RAM keeps it | ADR 0483 |
| A CPU quota for a resource domain (`cpu_max`, as cgroups' `cpu.max`), beside its weight, which only shares the CPUs while they are busy | ADR 0476 |
| More than 4,032 descriptors in one process, whatever `open_files` says: the capability space's room for them | ADR 0476 |
| A `cpu.idle` kernel setting: how deep the CPUs' idle states may go | ADR 0476 |
| A `driver` file for each PCI function in `/kernel/bus/pci/devices`, so that `hardware devices` and the web console's Devices card name the driver of every device: the kernel shows the driver of a network card (`/kernel/class/net/*/card`) and of a USB interface, and for the rest the console says "Not told"; it is a kernel change of its own, since a change under `kernel/` runs the boot tests' broad sample, which a console change should not carry | hardware(8) |

### Files and storage

| Item | Source |
| --- | --- |
| Names relative to a working folder whose path is longer than `PATH_MAX` (4,096 bytes): the C library joins a relative path to the working folder it keeps, as the kernel keeps none, so `mkdir` or `open` of a short name in such a folder fails with `ENAMETOOLONG`, where Linux resolves it from the folder (Go's `TestGetwdDeep`, `userspace/go/skips.txt`) | ADR 0350 |
| A name cache for path lookups | storage design note |
| Shared writable mappings of files other than `/dev/shm`'s, and `msync`, which SQLite's WAL between connections and its memory-mapped I/O need (ADR 0511) | posix design note, ADRs 0110, 0490, 0511, `lab/tests/services/sqlite-skips.txt` |
| Read-ahead in the background; write clustering; `O_DIRECT` for files | storage design note, ADR 0133 |
| FAT read-ahead | storage design note |
| Disk I/O rates for each disk: a domain's rates count across all disks together | ADR 0476 |
| A disk I/O weight for the page cache's writeback: its requests are the kernel's, so no domain's weight decides their share of a disk, and no rate holds them | ADR 0476 |
| A disk queue depth for weights that follows the disk: while domains have weights, every disk lets 8 requests go at once, which costs an NVMe disk's small random reads some speed | ADR 0476 |
| Btrfs: trim as blocks are freed (`discard`), small files inline, compression of small files, a per-CPU compression workspace, removing empty block groups, deleting a subvolume in the background, scrubbing in a thread of its own, a log of fsyncs a path at a time, tree blocks read whole | storage design note, ADRs 0130, 0158 |
| Btrfs RAID5 and RAID6 | Never planned (storage design note) |
| Removing a subvolume with a file of it open, or the default subvolume | ADR 0138 |
| Btrfs send streams of version 2 (compressed data sent as it is, `fallocate`, file flags), sent and received | ADR 0462 |
| `subvolume send -c` (clone sources), and the sender finding data a file shares with another to send as a clone | ADR 0462 |
| `subvolume receive` finding a stream's parent anywhere on the filesystem, through its UUID tree, rather than in the folder received into | ADR 0462 |
| `subvolume receive` refusing a stream's path that passes through a symbolic link, as Linux's `RESOLVE_NO_SYMLINKS` would; the subvolume's capability keeps such a path inside it meanwhile | ADR 0462 |
| `user.` extended attributes on device nodes, pipes and sockets left out of a send, or refused as Linux refuses them, so that Linux receives every stream Dukovo sends | ADR 0462 |
| `subvolume send --no-data` and `subvolume receive --max-errors` | ADR 0462 |
| ext4: several syncs in one transaction, and bench rows | ADR 0141 |
| `posix_fallocate(3)` only sets a file's length and takes no room, so a later write into the range can still fail with ENOSPC, on a disk or a tmpfs, where Linux's `fallocate(2)` takes the blocks or pages first | C library (`Sysdeps<Fallocate>`), storage design note |
| FUSE and share mounts: a page cache with writeback | ADR 0226 |
| A page or partition written raw through two different nodes at once | storage design note |
| A claim on a block device that keeps others from mounting it, as Linux's `O_EXCL` open of one does: `mkfs` and `floppy format` refuse a device that is mounted, but nothing keeps volumed(8) from mounting one while they write it, as it may a floppy disk in the moment between the kernel's look at it and the last write of its new filesystem | floppy(1), volumed(8), ADR 0435 |

### Networking and sharing

| Item | Source |
| --- | --- |
| A `net.tcp_buffer_kib` kernel setting: the size of TCP sockets' buffers | ADR 0476 |
| A TCP connection that outlives a moment in which its interface lacks its address, as on Linux: smoltcp ends it at the stack's next poll, without a reset, though the doc of the kernel's `Stack::configure` says it stays. netd keeps the addresses that stay through a reload, and those a card set to DHCP had until its lease comes, so a change of the settings no longer does this to them | Lab test `web-console/networking` (Phase 16) |
| An interface MTU below 1500 from DHCP's option 26 | ADR 0197 |
| A ping socket bound to an address its echo requests do not go from: Radix sends them from the outgoing interface's own address, where Linux sends from the bound one or refuses with EINVAL when no route goes from it, so the routers' errors about them do not reach the socket (mtr's was, bound to 127.0.0.1, until its patch 0002) | ADR 0345 |
| RSA keys made by certd: `this-machine` has an ECDSA P-256 key, since ring makes no RSA keys and every browser takes ECDSA; a certificate with an RSA key added by `cert add` works | ADR 0512 |
| ENA's low-latency queues; gVNIC's DQO queue format; fiber and SerDes gigabit cards | ADR 0197 |
| Legacy INTx interrupts through `_PRT`, for cards and SD controllers without MSI | ADRs 0197, 0198, 0437 |
| MSI-X with several queues in driver programs | ADR 0198 |
| Intel PCH Ethernet: Energy Efficient Ethernet, wake on LAN, jumbo frames, 82577 and 82578 | ADRs 0361, 0580 |
| K1 kept off on the two Meteor Lake laptops Linux lists for it (Dell Pro 16 Plus PB16250, ThinkPad P14s Gen 5): the list goes by the machine's DMI names, which the driver does not see | ADR 0580 |
| Promiscuous mode for packet capture | ADR 0346 |
| Firewall rules for outbound traffic and rate limits | ADR 0334 |
| `netcheck` saying that a firewall rule stops this machine's own DNS, HTTPS or time (NET-151): the firewall filters only what other machines start, so no rule can until it has outbound rules | ADR 0334, ADR 0489 |
| Routing beyond a tunnel's subnet before the routing table | ADR 0339 |
| Private access to the networks the machine is on: a device added with `wireguard add` reaches the server's address on the tunnel alone, as the kernel does not forward between network cards; forwarding, with firewall rules for it, would let a device reach the other machines there | ADR 0562 |
| timed: choosing among servers, training the clock's frequency, `CLOCK_REALTIME` timers after a step, authenticated fallback, leap seconds | ADRs 0194, 0487 |
| `AF_UNIX` sequenced-packet sockets (`SOCK_SEQPACKET`), which need record boundaries in the pipe and which no port asks for | ADRs 0091, 0525 |
| `AF_UNIX` abstract names (a leading NUL byte) and autobind (`bind` with only the family), which Linux has and Radix refuses | ADRs 0091, 0525 |
| `SCM_CREDENTIALS` naming another process, which Linux lets a process with `CAP_SYS_ADMIN` or `CAP_SETUID` send: Radix gives `EPERM` | ADR 0525 |
| A stream socket's `SCM_CREDENTIALS` are those of the connection's peer, as `SO_PEERCRED` gives them, where Linux gives those of the process that wrote each piece; only a stream passed to another process between writes tells | ADR 0525 |
| A Unix socket sent with `SCM_RIGHTS` while a queue it reads holds a socket not yet received, or into a queue it reads (a connection's other end sent over that connection): `ETOOMANYREFS`, where Linux allows both and collects the sockets that end up holding each other open | ADR 0525 |
| `ECONNRESET` for the datagram socket that is connected to another when that one reconnects or disconnects and throws its queue away (Linux's `unix_dgram_disconnected`) | ADR 0525 |
| A per-person setting for low ports | ADR 0257 |
| SMB: pipelined reads and writes, a page cache, leases and oplocks, change notices, locks other sessions see, `LsarQueryInformationPolicy` | ADRs 0227 to 0229 |
| NFS: locks the server sees, delegations, a page cache, several calls at once, an account-mapping table, `SP4_MACH_CRED`, contexts kept across a restart | ADRs 0344, 0399 |
| Kerberos: FAST, PKINIT, cross-realm tickets, renewing in place, finding KDCs by DNS; GSS-API for SSH and SMB | ADRs 0397, 0398 |
| SSH: `PermitOpen`, `PermitListen`, X11 and agent forwarding, Unix sockets, listen addresses by name, keyboard-interactive | ADRs 0348, 0349 |
| nginx's HTTP/3 (QUIC) | ADR 0491 |
| SYN cookies, and connections queued for `accept` without their 128 KiB of buffers, as Linux's request sockets are, for backlogs past 128 on an interface | ADR 0494 |
| logforward: RFC 3164 (BSD) messages, for receivers that do not read RFC 5424 | ADR 0482 |
| logforward: RELP, which has the receiver acknowledge each message, in place of the 10-second wait before a position counts as delivered | ADR 0482 |
| logforward: structured data beyond MSGID, such as the boot and the user a record has | ADR 0482 |
| Receiving logs from other machines: a syslog server on Dukovo | ADR 0482 |
| A DHCPv6 server and router advertisements: IPv6 computers keep the addresses their network's router has them make | ADR 0486 |
| DHCP relay agents: a request with `giaddr` set is ignored, and netd's log says so once | ADR 0486 |
| An ICMP probe of an address before netd's DHCP server offers it: a client's DECLINE covers a conflict (RFC 2131, section 3.1.5) | ADR 0486 |
| `netcheck` saying that another machine on the network has a card's address (NET-112): netd neither probes an address before it uses it nor reports a conflict (RFC 5227), so the address layer has nothing to read | ADR 0489 |
| Detecting another DHCP server on a card's network | ADR 0486 |
| NTS keys shared between several time servers (RFC 8915, section 6): the cookies of timed's server work at that server alone | ADR 0487 |

### Devices and firmware

| Item | Source |
| --- | --- |
| IOMMU: devices behind a PCIe-to-PCI bridge, the display controller's DMA, 5- and 6-level AMD tables, HPET MSIs, posted interrupts, 128-bit AMD-Vi entries | ADRs 0164 to 0166 |
| Watchdogs of Intel Atom SoCs and AMD SB700 and SB800 | ADR 0167 |
| Machine checks: bad pages dropped from the file cache, a kernel copy from user memory ending its call, Zen's data-fabric address translation and deferred-error interrupt, retiring a page after many corrected errors, resetting the counts | ADRs 0168, 0169, 0204 |
| ACPI: the ECDT, controllers in GPE block devices, burst mode, battery alarms and charge limits | ADR 0201 |
| CPU frequency: writable preference and range, HWP interrupts, energy-performance bias, preferred cores in the scheduler, CPPC through shared memory | ADR 0202 |
| Thermal: active trip points and fans, `_PSL`, throttling states, hibernating at the hot trip point, writable modes | ADR 0203 |
| Tdie on early Ryzen and Threadripper models | ADR 0205 |
| SMBus: block transfers, host notify, alerts, the FCH's other ports, older AMD chipsets and PIIX4, `i2c-dev` | ADR 0206 |
| Framework's Intel laptops' MEC embedded controllers; EC events, charge limits, keyboard backlight, EC firmware updates; a settings page for fans and the ARGB header; finer rights | ADRs 0207, 0208 |
| SPD: SPD5118 behind a write-disable bit, I2C block reads, module temperature sensors, LPDDR layouts | ADR 0232 |
| GPIO: other Intel PCHs, debounce and pulls, outputs, a GPIO device for programs | ADR 0233 |
| I2C: other Intel PCHs, fast mode plus and high speed, the controller's interrupt, the AMD PSP's semaphore, I2C as SMBus, `/dev/i2c-N` | ADR 0234 |
| HID over I2C devices whose interrupt is not a GPIO pin's, and output reports | ADR 0235 |
| DMI: `modalias` and `/sys/firmware/dmi/entries` | ADR 0231 |
| USB-C: swapping roles, alternate modes, UCSI over I2C | ADR 0248 |
| ASIX adapters' interrupt endpoint | ADR 0362 |
| HD Audio: microphones, line in, HDMI and DisplayPort audio, more than two channels, S/PDIF, interrupts | ADR 0322 |
| USB audio: microphones, implicit feedback, more than two channels, UAC3, vendor requests | ADR 0323 |
| The virtio sound card telling its driver when its status changes, as a virtio input device does: the card's worker reads its status at each look, 50 a second while it plays and one a second otherwise, and each read is an exit to the host | ADR 0711 |
| Braille displays that are HID or USB-bulk devices | ADR 0326 |
| Hyper-V: disks added while running, the mouse, the video device, dynamic memory, key-value exchange | ADR 0423 |
| virtio-scsi: the event queue for hot-plugged disks, and task management | ADR 0327 |
| Floppy drives 2 and 3, 5.25-inch drives, and 2.88 MB disks | ADRs 0435, 0436 |
| CompactFlash in PCMCIA and CardBus adapters | ADR 0446 |
| MS-JIS CD-Text | ADR 0447 |
| Writing CD-Text when burning a CD | ADR 0449 |
| Serial ports: typing by INTx on PCI ports without MSI, and by MSI-X on functions that have only that (they are polled); COM3's interrupt, which is COM1's (it is polled); memory-mapped 16550s, such as AMD's `AMDI0020` and Intel's LPSS UARTs (not used); the AX99100's 256-byte FIFO and rates above 115200 | ADRs 0360, 0480 |
| FLAC with linear prediction, a few per cent smaller | ADR 0447 |
| A pointer that windowd draws into its frames, for a graphics card with no pointer plane: every card of release 1.1 has one, or a driver that draws the pointer | ADR 0715 |
| windowd waiting with a kernel queue in place of poll(2): a wait names a few dozen descriptors | ADR 0717 |

### Accounts, bundles and the console

| Item | Source |
| --- | --- |
| An elevated command in the person's view; clearing a person's `/tmp` after their last session | ADR 0255 |
| Sign-in records kept for a time; notices of a failed sign-in streak | ADRs 0253, 0254 |
| The administrator set split into named areas for delegation | ADR 0219 |
| A user's own services and their lines in their log; a group that reads the whole log | ADR 0252 |
| `sh -c 'program'` runs the program in a child and waits, where bash and dash run it in the shell's place: a caller that ends the program by the number it was given (PHP's `proc_terminate`, which run-tests uses for a test that takes too long) ends the shell and the program goes on, and a program that a signal ends gets "Terminated" printed on the stderr it shares with the shell | Lab, `services/php`, 2026-11-21 |
| Valkey's, PHP-FPM's and PostgreSQL's Unix sockets reached by other bundles' services, as they are on Linux: a bundle offers only a socket that Spark makes and holds for its service (ADR 0477), and these servers make their own sockets in their data or live folders, which are their accounts' alone, so they talk over TCP on the machine itself: nginx and PHP-FPM on 127.0.0.1:9000, where any program on the machine may connect, and PHP's `pgsql` reaches PostgreSQL over 127.0.0.1 with a role's password | ADRs 0477, 0492, 0520, 0522 |
| Rich and NFSv4 access lists; Btrfs properties | ADR 0256 |
| Bundles: grants before install (`--grant`), resource limits for the sandbox, programs for other architectures, read-only subvolumes and reflink sharing for sealed bundles, asking people about their data when a bundle is removed for everyone | ADRs 0223 to 0225 |
| The copy of a service's data folder that an update of its bundle makes while the service is stopped is a whole copy, where a reflink would share the data, so an update of a bundle with a large database, PostgreSQL's, keeps the service down for as long as the copy takes | ADRs 0386, 0522 |
| Moving a PostgreSQL cluster to the next major version with `pg_upgrade` when the bundle has two lines (ADR 0387): the catalog has one major version | ADR 0387, ADR 0522 |
| Views: detaching an entry; devices attached one by one | ADR 0215 |
| Manual pages inside bundles on `MANPATH` | ADR 0370 |
| Removing an everyone's line also removes each person's add-ons | ADR 0388 |
| The proof of a domain for a third-party app's publisher: a file at `https://DOMAIN/.well-known/dukovo-app-ids` that lists the IDs the domain's owner publishes, which a catalog checks before it vouches for the IDs (ADR 0592). Every bundle Dukovo's catalog lists is built from Dukovo's own ports, so the catalog vouches for those IDs itself | ADR 0592; comes with the first third-party app the catalog takes |
| A wavy underline in the consoles | ADR 0258 |
| Pairing unpaired diff runs in git's word marks | ADR 0293 |
| Pseudo-terminal packet mode, `TIOCSTI` and BSD line disciplines | ADR 0066 |
| Cloud first boot: Google Cloud and OpenStack metadata, NoCloud from the command line or SMBIOS, `bootcmd`, `timezone`, `ssh_pwauth`, `chpasswd` lists, bonds, bridges and VLANs | ADR 0328 |
| The guest agent's other commands | ADR 0329 |
| Installer translations | ADR 0310 |
| GRUB's menu refreshed when Windows is added or moved later | ADR 0314 |
| Rebuilding a mirror onto a new disk without manual steps | ADR 0315 |
| The filesystem, label and ID of a partition that is not mounted, in `storage show` as a person: blkid must open the device, which only an administrator may, so the console's Storage page, which reads as the person, shows such a partition with no filesystem; an unlocked administrator's page could read them through `elevate` | storage(8) |
| `ls --app` naming the app that downloaded a file (needs provenance tags) | ADR 0442 |
| Disk and ISO images compared byte for byte between machines (the tree, its release list and each image's identifiers are, ADR 0535): the images `ob image` makes hold a release signed with the machine's own keys; those `./ob release images` makes hold nothing of a machine's own (ADR 0779), and no second machine makes them yet | ADR 0010, ADR 0382, ADR 0535, ADR 0779 |
| A git settings page in Settings (on hold) | Git work, ADR 0287 |
| An app's own settings file and its own update rule: a bundle's manifest names neither, so an app's page shows its data folder, the page of its service and the one rule that every app installed for everyone follows | bundle(1) |
| A version of an app older than the one kept: bundled keeps one version back, so an app's page lists the version it runs, the one on offer and the one kept | bundled(8) |
| A new alert rule tried on last week's history before it is on: no alert rule is the owner's to write, and a script raises an alert with `alerts raise`, so this waits until rules exist | ADR 0485 |
| An alert when an administrator signs in from a new place or device, and Sign-ins as a kind of alert: no program raises an alert for a sign-in, since authd blocks an address that fails too often but keeps no record of where each administrator signs in from; it waits on a record in authd of the places and devices each administrator signs in from, with its own privacy note | ADR 0485, ADR 0566 |
| Ops' catalog prepare does not see a lab round reading out/: the lab tool keeps its run lock in its own store, which Ops does not read, so the prepare window warns instead | ADR 0594 |
| AI clients of people who are not administrators: authd lets only an administrator add one, since a client's changes are approved by administrators and its reads as an administrator need an owner who is one | ADR 0690 |
| Changing an AI client's areas or its end without removing it: a client is removed and added again, so that what it reaches is what its owner saw when adding it | ADR 0690 |
| One switch that turns every AI client off at once: `admin mcp list --all` lists them, and each is removed by itself | ADR 0690 |
| A yes to an AI client's request at a terminal: the requests are in admind's memory, and are answered from a signed-in console session, unlocked for a yes | ADR 0690 |
| An AI client's requests kept across a restart of admind: those that wait are lost, the history shows them as lapsed, and the client asks again | ADR 0690 |
| An AI client asking for a change that runs as the person: the changes to a person's own account, `snapshots.restore` and `console.export` are offered to no client, since an approved change runs as the administrator who approved it | ADR 0690 |
| Signing an AI client in to `/mcp` by OAuth, as the protocol describes for HTTP: a client over HTTPS has a token of its own kind, and one over SSH needs none | ADR 0690 |
| The protocol's tasks, resources and prompts, and a stream that tells an AI client its request was answered: a client asks `request` again | ADR 0692 |
| `*:read` as an API token's scope, as an AI client has it: a token that reads every area names `*`, which also changes them | admin(8) |
| The banner of a change that is being tried naming the AI client that asked for it: the journal's row and the task do, and the Security page's script is at its size budget | ADR 0690 |
| What only a screen reader is told (`.sr-only`) held inside the scrolling box of every page, as the AI clients page holds its own: on a page whose table has such a caption below the window's end, the Overview at 1280 by 800 for one, the document is taller than its window and scrolls into nothing. One rule on the shell's page box does it for all pages, and moves what each page's menus and popovers are placed against, so each page is looked at first | Lab test `web-console/clients` |

### Language runtimes

The roadmap lists these as [not scheduled](roadmap.md#not-scheduled).
Each waits on programs or users asking for more than the Linux builds
give.

| Item | Source |
| --- | --- |
| A native OpenJDK port (Java and the JVM languages), with HotSpot's own operating-system layer for Dukovo and its JIT within W^X, for speed and DukUI integration. Until then the official Linux builds run through the Linux compatibility layer, once Phase 18 lets a program ask for JIT memory | Roadmap, not scheduled |
| A native .NET port (C# and F#), with the runtime's platform layer and native shims for Dukovo and its JIT within W^X, for speed and DukUI integration. Until then the official Linux builds run through the Linux compatibility layer, once Phase 18 lets a program ask for JIT memory | Roadmap, not scheduled |

### Benchmarks

| Item | Source |
| --- | --- |
| `posix_git_status` reported, not gated, until parity | ADR 0294 |
| The display layer's rows and the input rows beside Linux (`display_check` to `input_to_reader_p99`) reported, not gated, until parity on CI's runners. Their first run there had three above Linux's: a mouse's motion at 3.71 times (38.8 µs against 10.4 µs), then at 2.00 times with ADR 0710 (23.7 µs against 11.8 µs), the rest of whose cause ADR 0711 takes away; the pointer moved on a virtio card, last at 1.24 times there (32.4 µs against 26.1 µs); and 64 by 64 pixels on the firmware's card, last at 1.08 times (6,464 ns against 5,998 ns). Counted, the last two do nothing in the kernel beyond their own work. Under KVM on the lab's PC (an AMD Zen 5 host) all three are below Linux's: a motion at 0.50 times (5.3 µs against 10.7 µs), the pointer at 0.70 and the 64 by 64 pixels at 0.37. One row is above it there that is not on CI's runners: a whole 1280 by 800 picture on the firmware's card takes 2.76 times Linux's time (516.9 µs against 187.1 µs, 0.81 times on CI's runners), whose cause is not known and is looked at next on that machine. Linux's two rows that `virtio_gpu` holds for a frame are processor time, which leaves out what it does outside the calling thread; a whole frame's time, from a client's picture to the screen, comes with the window server's rows | ADRs 0709, 0710, 0711 |
| `posix_poll_idle` reported, not gated, until parity: `poll` on 1,001 pipes takes 1.52 times Linux's time on CI's runners (9.6 µs against 6.3 µs), from 12.7 before ADR 0354, about 130 instructions a descriptor. Each descriptor reads five cache lines, its slot, its record, the file, the pipe's end and the pipe, where Linux reads two or three; next, the address kept in the slot as well as the record, and a pipe's end named by the address itself | ADR 0354 |
| On an AMD EPYC 9V45 runner, an event queue's wait for one ready pipe among 1,000 idle ones (`posix_events_idle`) takes 1.10 times Linux's time (326 ns against epoll's 297 ns), where Intel 6975P-C runners give 0.90 to 0.92: the gate on the second runner fails when it lands on that machine. Its first AMD run came with ADR 0354, which does not touch the wait; next, a count of its instructions and cache misses there against epoll's | ADR 0353 |
| Optical reads reported, not gated, until parity; USB storage rows | ADR 0434 |
| `git status` takes about 2.5 times Linux's time (5.0 ms against 1.98 ms on 1,000 files, with 15 context switches); no work is planned to close it yet | ADR 0294 |
| Rows still above the 0.90 margin target, though under Linux: Btrfs zstd write, unmapping a page on another CPU, NVMe 1 MiB read (0.92 to 0.94) | Benchmark margin work |
| smoltcp goes through every socket of an interface's stack for each packet and each poll: an idle socket costs about 110 instructions a round trip on the host, having been passed over in the send loop since it has nothing to send (the network benchmarks run beside 11, ADR 0550), where Linux looks a socket up by its address | ADR 0550 |
| On some Intel Xeon 6975P-C runner hosts, with the vCPUs pinned, a forked process's open and close takes about 70 ns more than a thread's: one process 585 against 512 ns, 1.10 times Linux's 530, which fails the scaling gate's one-process row there. The kernel before ADR 0555 does the same in one job, and other hosts of the same CPU give 511 to 515 ns. In the child a chain of SSE adds takes 3 cycles rather than 2 while integer multiplies are unchanged, as when the vector registers' upper halves are held as in use. A 1 ms sleep or a second thread before the child pins itself made it fast; VZEROUPPER at the child's start, reading XINUSE before the fork or before the loop, and a kernel that drops all-zero upper halves from each save did not | ADR 0554 |
| On real hardware (OptiPlex 7040, i5-6500), on 2026-11-20: fork, exec and wait 1.12 times Linux's time, and the UDP round trip on one CPU 1.16 before ADR 0553; the lab's next run there confirms both | Lab, performance/loopback and processes on the OptiPlex; mlibc patches 0092 and 0093 |
| TLS rows of the server benchmarks (nginx over HTTPS, PostgreSQL over TLS): the handshake and record costs are OpenSSL's arithmetic, the same code on both systems, and the Linux kit's servers are built without OpenSSL | ADR 0546 |
| The server benchmarks over the network card: they run client and server on the machine, over the loopback or a Unix socket, so that they need no peer; the card's path is measured by its own rows | ADR 0546 |
| In QEMU on the lab PC (an AMD Zen 5 host, which gives the virtual machine no PCIDs), some server rows take longer than Linux's in the same virtual machine: PostgreSQL's select 2.0 to 2.6 times Linux's time, TPC-B 2.0 to 2.1, and serverbench's wait for one ready pipe among idle ones 1.9, where PHP-FPM is level with Linux and on the OptiPlex 7040 every server row is at or under Linux's time. The profiles put the extra time in switching between programs and in the return from interrupts, each of which costs more in the virtual machine. 1.0.0-alpha.1 holds the server rows to Linux's time on the OptiPlex; 1.0.0-beta.1 holds them in QEMU too | Lab, performance/server-profile, php and postgresql on QEMU; ADR 0770 |
| At the CPUs' default clocks, Dukovo's and Linux's differ on real hardware: Dukovo idles in C1 with the firmware's C1E promotion left on and no deeper state, so idle cores count as active and the busy core reaches the four-core turbo level (3.3 GHz on the OptiPlex 7040) where Linux, whose intel_idle turns the promotion off and uses deeper states, can reach 3.6 GHz. The lab's performance tests hold both systems at the base frequency instead; what the default clocks cost is measured with `cpu_cycle_cold`, `cpu_cycle` and `cpu_cycle_end` once that run is in, and deep idle states come with laptop power | ADR 0557; the first OptiPlex run with the clocks held |

## Lab findings to confirm

Seen while the lab harness was built, before findings were recorded in the
lab's results. Each is checked on the current system and either filed as a
finding or removed.

| Item | Source |
| --- | --- |
| virtio-net's link going up or down is not logged on the serial console | Lab harness, part 4 |
| PHP's file tests skipped for a slash after a file's name (`lab/tests/services/php-skips.txt`, the Files group) should pass since ADR 0352: drop each that passes on the next run of PHP's suite | ADR 0352 |
| A redirected subshell sent to the background in Nukesh keeps an SSH session open | Lab harness, part 7 |
| No AMT OS agent: a graceful power-off asked through AMT is not seen, so `boot/power-button` fails on the Intel lab machine | Lab, first real-machine runs |
| authd's errors (stderr) reach the console but not log(1); log lines longer than one write are split and the pieces lose their priority and tag | Lab, first real-machine runs |
| `lsof -i` lists no sockets and `ss -p` is not supported | Lab, first real-machine runs |
| timedatectl prints its name twice in errors; sudo's refusal names "Phase 13d"; timed says "Unknown error (6)" for a failed lookup and "()" when no NTS server answered | Lab, first real-machine runs |
| A lab test for COM1 as the serial console (console=ttyS0): serial-console running on it and a sign-in over it; hardware/serial-port covers COM1 only when it is not the console | Lab, first real-machine runs |
| SSH: RSA and P-521 keys and authorized_keys options are not taken; sshd_config takes eight keywords (no `Port`, `ListenAddress`, `MaxAuthTries`); ssh is built without OpenSSL, so ssh-keygen makes ed25519 keys alone | Lab, first real-machine runs |
| "[elevate] password for lab:" is printed into the output of commands run with sudo -S | Lab, first real-machine runs |

## Waiting on a decision

| Item | Why it waits |
| --- | --- |
| `radix_abi`'s generated wrappers that pass the kernel a buffer as an address and a length made `unsafe`, or made to take a slice, so that a crate that forbids `unsafe` cannot give the kernel an address of its own making (ADR 0730) | The kernel touches only the caller's memory through such an address, so a wrong one spoils only that program; sysapi's `kernel::control` is the safe way to pass a buffer meanwhile. Which of the two suits the generator, ridlc, is open |
| The SD 64 KiB and 1 MiB benchmark rows: printed but not gated (ADR 0552) | QEMU's SD emulation takes nearly all of both systems' time, and its speed moves with the runner; a real reader in the lab would show each system's own work |
| Printing: the design's open questions | Drafted for a release after 1.0 |
| Known weak spots to plan for: Wi-Fi, Bluetooth and webcams; large desktop apps; Btrfs maturity | Raised, not yet discussed |
| Nightly system releases after 1.0.0, and the key that signs them (ADR 0781) | There are none before 1.0.0: a nightly release would be signed at every merge with nobody there, and ADR 0457 keeps every key that signs a release off servers and out of CI |
| A build taken out of the targets with `--remove` could be given its number again (ADR 0595) | Removing a signed target is rare, and a number given again is still ordered right: it is above every build the targets list |

## Documentation to bring up to date

| Item | Why |
| --- | --- |
| Support center pages built from the settings schemas (each field with the release it arrived in and its stability) and stability labels on the pages ridlc writes from `abi/*.ridl` (ADR 0419) | `ob docs site` renders the docs in the tree first; these generators follow it |
| A scheduled check of the support center's links to other sites (ADR 0420) | Pull requests check only the links between the site's own pages; outside links need a job of their own |
| The *Known limits* sections of `docs/design/posix.md` and `docs/design/storage.md`, and radix-posix(7) | They still describe Phases 7 to 9: no network sockets, no users, no hot plug, no exFAT, no Btrfs quotas, locales "until Phase 12" |
| The roadmap's Phase 16 status paragraph | It lists the work up to the web console's sign-in (ADR 0514); the console's pages, PostgreSQL, PHP, nginx, Valkey, certificates, the update server on Dukovo, Ansible and problem reports have merged since |

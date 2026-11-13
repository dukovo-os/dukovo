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
| The catalog's first signed batch and deploy (`./ob catalog sign`, then `./ob catalog deploy`; ADR 0381) | The first root is signed and every image trusts it, but the server serves no bundles until a batch is signed; signing packs every port, a long build | A maintainer, before the first public release |
| Canary machines for updates | Phase 15 uses QEMU canaries in CI; real canaries are wanted before the first public release | Before 1.0.0-alpha.1 |
| Post-Phase-13 code audit (audit 10) | Planned and scoped; starts only on approval | Decision |
| System releases signed on the two hardware keys: the system's TUF root (`releases/metadata/1.root.json`, naming the update server's online key that `./ob release server setup` writes to `releases/keys/online.json`), `ob release sign` with its gates and a third build that must match CI's two, the Rekor record, `ob keys ceremony` and the key runbooks (ADRs 0450, 0457, 0467) | Phase 15 signs with each build machine's own keys and tests against a local update server; `ob release deploy` deploys nothing until that root exists | Before 1.0.0-alpha.1 |
| The security-fix process: the release branch and its bots, the advisories feed, and the daily SBOM scan against OSV and NVD (ADR 0458) | Needed once there are releases to fix | Before 1.0.0-alpha.1 |
| The full update test matrix on a self-hosted runner: power-cut replay with QEMU's blklogwrites, upgrade paths from 12 months back, Kani on the slot-state reader, fuzzing of release lists and TUF metadata (ADR 0450) | Each needs many VM starts or tooling of its own; `update-power-cut` cuts one stage at one point meanwhile | Before 1.0.0-alpha.1 |
| Bit-for-bit reproducible images: grub-mkrescue still writes random identifiers (ADRs 0010, 0457) | The three-build check before signing depends on it | Before 1.0.0-alpha.1 |
| Send the 27 mlibc and frigg patches upstream | Prepared; planned after the audit's last part | Audit 10 |
| smoltcp's own tests in CI, with its three UDP broadcast tests mended or changed for the kernel's patch (ADR 0182) | CI runs radix-net's tests, which drive the patched stack, but not the crate's own; those three already fail with the patch | Before the smoltcp patch goes upstream |
| Trademark clearance of the project's name | Legal review | Before 1.0.0-alpha.1 |
| SECURITY.md says plainly that the alpha is unaudited; the update server is live for testers | Part of the checklist before the first public release | Before 1.0.0-alpha.1 |

## Waiting on real hardware

QEMU cannot show these. Each runs in the lab when it has the machine or
the device, or by hand on a reference machine. The lab has an Intel
machine with AMT, an AMD machine and a laptop; see
[the lab chapter](handbook/src/lab.md).

### Lab machines and harness

| Item | Why it waits | Waits on |
| --- | --- | --- |
| Lab harness parts 9c and 10: the screen, keyboard and mouse through AMT's remote screen, and the suite's first run on the Intel lab machine | Parts 9a and 9b brought power, the serial console and a fresh disk for each test (ADR 0429); these come next | Intel lab machine |
| The installer's tests on a real lab machine: booting it from the install stick or DVD | The `amt` driver boots only the lab image, written to the disk by the net-booted writer (ADR 0429); a stick a person plugs in still needs the machine told to boot from it | Lab harness |
| The security range's attacker on a real lab machine | The attacker runs on a private QEMU network; a real machine needs a second network of its own for it | Lab harness |
| The power button and a graceful off on a real lab machine (`power = "button"` and `"off"`) | AMT asks the system to shut down only when an agent in the system answers AMT, which Dukovo has none of | A decision by the maintainers |
| Lab machines with Secure Boot on, for Secure Boot's tests on real hardware | The network GRUB and the pinned Linux kernel the writer runs on are not signed (ADR 0429) | A need: Secure Boot's lab tests |
| AMD lab machine set up as a lab target (no AMT: a smart plug, network boot and network logs) | Need the machine | AMD lab machine |
| The lab PC's half of `sharing/nfs-server` | Needs a kernel NFS client, which the build container lacks | A run on the lab PC |
| The lab test of Repair startup (`boot/repair-startup`, ADR 0503): the installed system's menu taken off its EFI system partition, then the install stick's Repair startup puts it back | A test plugs in a stick only as a file beside the test or a blank one; the install stick is started only by `boot = "stick"`, which comes with a blank disk | Lab harness: a step that plugs in the install stick mid-test |
| Tests that need two disks besides the stick, such as installing the mirror layout (`install/mirror-needs-two`, ADR 0315) | The lab gives a test one disk | Lab harness |
| A lab image without the lab account, to test the first boot's "no account yet" page (ADR 0401) | The lab image always has its account | Lab harness |
| `ob lab` checks that a machine is free before a run; a Stop for one test | Left out of the harness's first parts | Lab harness |
| A lab test of a keyring entry set and read back | Needs a console sign-in with a password, which tests cannot type yet | Lab harness |
| A lab test of `whois` | Needs a reachable whois server | Lab harness |
| Receive-side scaling, checksum and segmentation offloads for the server cards and virtio-net; GSO and multi-queue tunnels (ADR 0339) | Measured against Linux on 10 GbE, and the lab's network is 1 GbE; moved out of Phase 16 so it does not hold 1.0.0-alpha.1 | 10 GbE in the lab: two Intel X520, X540 or X550 cards and a cable |
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
| Canaries that report to the update server by themselves (ADR 0456) | The maintainer marks a canary red or green with `ob release rollout canary` | The canary machines, before 1.0.0-alpha.1 |
| `system-release pull` signing with the hardware keys (ADRs 0457, 0469) | It signs with the release keys' files, as `add` and `channel` do | The key ceremony |
| Firmware that drivers load for their cards, in firmware sets, with a per-device check that the driver loaded it (ADR 0468) | Sets hold CPU microcode only; no driver Dukovo has loads firmware yet | A port: the first driver that needs firmware (Wi-Fi, Phase 17) |
| AMD CPU microcode in firmware sets (ADRs 0014, 0468) | The kernel's loader is Intel's; AMD's patches have their own format and loader | A need |
| Disks other than the root's in the health check's device record (ADR 0468) | Data disks, USB disks and discs come and go with their owner, so a missing one says nothing of the update | A need |
| A summary of the last starts on the EFI system partition, for a root Rescue cannot mount (ADRs 0500, 0501) | Rescue reads the root's records, and says how many starts the boot menu counted as not seen through | A need |
| Repair startup re-making an EFI system partition whose FAT cannot be read, with Rescue's files from the stick, and reading the firmware's Secure Boot and SATA mode settings (ADR 0503) | Repair startup rewrites GRUB and the menu on a partition that reads, and names AHCI when it finds no Dukovo | 16b, with `hardware check` |
| A boot test of Repair startup in QEMU, which needs a third boot from the stick after the install (ADR 0503) | The unit tests of what it finds and keeps; the lab test waits too (Lab machines and harness) | When the boot tests gain a third boot |
| How far a start got before Spark, in the kernel or mounting the root, written as it is reached (ADR 0503) | Spark marks the starts that reached their services; Rescue tells the rest from the boot menu's count | A need, with the starts' summary on the EFI system partition |
| Rescue, Repair startup and the memory test unlocking an encrypted root with its recovery key or passphrase (ADR 0501) | No root is encrypted yet | Disk encryption, after 1.0 |
| Unattended updates limited to security fixes, and the security advisories each update fixes, shown by `update` (ADR 0502) | Unattended updates take every release, through the waves; the release notes name fixes | The channel listing advisories per release, 16b |
| Snoozing an update reminder, and pinning the system so that no newer version is taken (ADR 0502) | The reminder comes once a day; `check = "off"` stops it; `update hold` skips one version | 16c, with the web console's updater page |
| The updater's page in the web console (ADR 0502) | `update` and `updated` on the command line | 16c, Dukovo Admin |
| The memory test on the start menu, memtest86+ built from source onto the EFI system partition (ADR 0501) | The firmware's own memory test, where the machine has one | 16b, with `hardware check` |
| A QR code to the error's support page, and single-key choices, on the failure notice (ADR 0501) | The notice gives the support page's address and the commands to run | 16f, with `report --save` |
| SSH into Rescue when the system had SSH on (ADR 0501) | Rescue on the screen and the serial port; after a failed start that falls back, the system runs, SSH and all | 16e, in the outside security review |
| A start without graphics on the Troubleshooting menu, and the window manager's screen check (ADR 0501) | No graphical start yet: every start is text | 1.1, with the window manager |
| `changes --since` a date (ADR 0500) | `--between` and `--list` compare any two starts kept | A need |
| The owner told of a fallback on the web console, with the offer to send a report (ADRs 0453, 0456, 0501) | The sign-in screens and the serial port show the failure notice until the next start, and the consoles are told at once | 16c and 16f, with the web console and `report --save` |
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
| The owner told of a backup that is overdue or failed on the web console (ADR 0463) | The sign-in screens keep a notice of it, the consoles are told at once, and `backup list` shows it (ADR 0504) | 16c, Dukovo Admin |
| Restoring a whole machine from the installer, for a computer with no Dukovo on it yet (ADR 0464) | `backup restore --machine` runs on an installed system, so a fresh install comes first | A decision: the installer's restore screen |
| Tags checked when Linux's `age` and `btrfs receive` restore a backup (ADR 0464) | The tag is an HMAC-SHA256 beside each file; Linux's tools read the backup without it | A need |
| Checking who wrote a backup with another of its keys, not the recovery key (ADR 0464) | The tag key comes from the destination's own key; other recipients restore with `--unchecked` | A need |
| A local cache of a restic repository's indexes and snapshots, so that a round to remote storage reads less (ADR 0465) | Each round reads the repository's indexes from the destination | A need: a benchmark of rounds to S3 storage |
| Restic's lock files, so that Linux's `restic prune` never runs on a repository during a round (ADR 0465) | rustic's library writes no locks; backup(8) says not to prune from elsewhere while the machine backs up | A port: a rustic_core release that keeps restic's locks |
| S3 addresses in the virtual-hosted style, and temporary credentials with a session token (ADR 0465) | Path-style addresses and an access key's ID and secret are what every S3-compatible store takes | A need |
| Restic destinations on restic's REST server (`rest:` addresses) (ADR 0465) | A share, SSH and S3 cover the servers people have | A need |
| Owner, mode and times of a place's top folder kept when Linux's `restic` restores it (ADR 0465) | restic sets the top folder of a restore itself; Dukovo's own restore keeps them | An upstream change in restic |
| Which advisories a bundle's update fixes, shown before it is taken (ADR 0389) | The sign-in screens list installed versions with known flaws, and the sheet names the version that fixes each (ADR 0504) | The catalog listing fixes per version, 16b |

### Phase 16: server platform and web console

| Item | Why it waits | Waits on |
| --- | --- | --- |
| `service status` for an ordinary user: Spark's control socket is root's, so status and `--status-all` need root, where Linux's `systemctl status` needs none; a read-only way to ask what Spark knows | The lab test install/cloud-setup asks as root meanwhile | 16a |
| `update` (and `update check`, `update history`) for an ordinary user, which update(8) says anyone may run: it finds the system's root by reading the disk's Btrfs UUID from `/devices`, which only root may read, so it says "this system did not start from a Dukovo root on a disk"; it could ask updated(8) instead, or take the UUID from the kernel's command line | Found on the OptiPlex 7040 by `updates/updater`, which fails at step 3 until then | 16a |
| Timers: a service started at set times or intervals (ADR 0216) | Production service management; dependencies, readiness and health checks came first (ADR 0470) | 16a |
| The system's own service files using service_version 2's keys: `requires`, `wants`, `after`, `[ready]` and `[health]` for the system's services, and `service new`'s template writing version 2 (ADR 0470) | A release writes only what the one before it reads (ADR 0454), and 0.15 reads service_version 1 alone; the system's services start by `order` meanwhile, as before | The release after 1.0.0-alpha.1 |
| bundled refusing a bundle whose `requires_os` is newer than the system, with a message to update (ADR 0470) | `requires_os` is informational; a bundle whose service uses version 2's keys says 1.0, and alpha.1 is the first release any machine runs | Before the first release after 1.0.0-alpha.1 |
| A user's jobs that run while they are signed out (ADR 0220) | Needs jobs | 16a |
| Sockets a bundle offers to others, and starting a service at its first connection (ADRs 0385, 0386) | Socket activation | 16a |
| `service status` listing what a service holds; volumed and diskhealth given disks one by one; `/processes` showing a service or session only its own processes (ADRs 0215, 0217) | Finer views for services | 16a |
| sysupdate's own narrower capabilities in place of the administrator's access (ADR 0455) | Needs a view of the Btrfs root that leaves out `@users` | 16a, with finer views for services |
| The `snap` service's own view in place of the whole tree (ADR 0461) | `snap.toml` may name any subvolume, reached through the top of each filesystem | 16a, with finer views for services and sysupdate's (ADR 0455) |
| Rescue kept up to date: a newer copy written to the EFI system partition when an update proves itself (ADR 0460) | Rescue is the release the installer wrote; its reinstall puts that version back, and the system then updates as usual | Phase 16, with the update server on Dukovo (16e) |
| Rescue's reinstall from the update server (ADR 0460) | Rescue reinstalls from its own copy, an update folder or an update file, with networking off | Phase 16, with the update server on Dukovo (16e) |
| The firmware sets and the catalog's bundles going through the waves (ADRs 0456, 0469) | Only system releases have rollouts; a firmware set is staged only when its owner asks | Phase 16, with the update server on Dukovo (16e) |
| The installer installing the install stick's firmware set, and update files carrying one (ADR 0468) | A machine takes the firmware package by staging a set from the update server or a repository folder after it is installed | Phase 16, with the installer's question (16c) |
| A change of the serial console's settings acting at once (ADR 0481) | `serial` writes the settings for the next start: the kernel and serial-console read the command line, which a start makes | A way to change the kernel's serial settings in a running system, which no plan item asks for yet |
| GRUB's menu on the second port of a PCI function, on a port at a parity or 7 data bits, on a fifth port, and on a PCI domain other than 0000 (ADR 0481) | GRUB's `pciserial` runs BAR 0's first port only, GRUB's `serial` 8 data bits and no parity, and the menu has four slots; `serial` says which console is left off the menu and why | A GRUB that runs them, or a boot loader of Dukovo's own |
| The speed of the SPCR's "as the firmware left it" read from the UART's divisor (ADR 0481) | Taken as 115200 baud, as GRUB takes it | A machine in the lab whose firmware leaves another speed and gives none |
| elevate in the web console, with comments kept by its settings editor (ADRs 0216, 0219) | Dukovo Admin | 16c |
| A page for logforward in the web console: its destinations, their status and a test record (ADR 0482) | Dukovo Admin | 16c |
| TLS and a password for the Prometheus endpoint (ADR 0484) | The endpoint is plain HTTP, answering only the networks `allow` names, as most node_exporter setups run it | A need |
| The alerts page and push notices in the web console (ADR 0485) | Alerts reach the screen, the next sign-in, email and webhooks; `alerts` lists them | 16c, Dukovo Admin |
| A bundle's service raising alerts (ADR 0485) | alertd serves root's processes and the system's services alone, which covers every shipped service; a bundle's service runs as an account of its own | A need |
| A certificate authority of its own for a webhook in alerts.toml(5) (ADR 0485) | Webhooks trust the system's roots; a private authority is added to them as dukovo-certificates(7) says | A need |
| Silencing an alert for a while (ADR 0485) | An alert is told only when it changes, so one that stays does not repeat; `severity` and `cleared` in alerts.toml(5) quiet a channel | A need |
| A certificate from certd for nginx's sites and for the lab test `services/nginx`, which makes one with openssl meanwhile (ADR 0491) | certd comes with Phase 16 | Phase 16's certd |
| Spark setting a service's `open_files` (RLIMIT_NOFILE); nginx raises its own to the hard limit, 4096, meanwhile (ADRs 0385, 0491) | Service limits | 16a |
| A service's own log files, turned over: nginx logs to the system's log, and an `access_log` to a file of its own is never turned over (ADR 0491) | Needs timers | 16a |
| The web console's lab tests run on QEMU in CI for every PR (ADR 0510) | CI runs no lab tests yet; they run on QEMU before each PR, and the console's pages arrive from C3 | admind (Phase 16, PR C2a) |
| A crash dump sent with a problem report, with the person's consent (ADR 0483) | A dump is kept on the machine and sent nowhere; the reports come with the reporting service | 16b, with the problem reports |

### Phase 17: Go

| Item | Why it waits | Waits on |
| --- | --- | --- |
| The six-part plan for the Go port, and its netpoller: `poll()` now and the kernel event queue with Phase 18, or a kqueue-style queue in Phase 17 | Drafted; waiting for approval | Decision |
| fzf, lazygit and duf in the catalog | Written in Go | Phase 17 |

### Phase 18: Linux compatibility and containers

| Item | Why it waits | Waits on |
| --- | --- | --- |
| `epoll`, `eventfd`, `signalfd`, `timerfd`, `memfd`, `pidfd` and a vDSO clock page (posix design note) | Built as native objects with the Linux layer | 18a |
| Neovim and Midnight Commander, as their official Linux builds | Moved off the native port list | 18a |
| Linux's ALSA programs playing sound (ADR 0320) | Need the compatibility layer | 18a |
| The Linux layout (`/dev`, `/proc`, `/sys` as directories) inside Linux programs' view (ADR 0113) | Served by `linuxd` | 18a |
| nginx on the kernel's event queue, with sendfile and thread pools: it waits with `poll` and copies files through its buffers (ADR 0491) | Built as for a POSIX system until Radix has them | 18a |
| Valkey on the kernel's event queue: it waits with `poll`, on an event loop of the port's own (ADR 0492) | Built as for a POSIX system until Radix has one | 18a |

### Releases after 1.0

These come from finished work; the releases themselves are in the
[roadmap](roadmap.md#post-10-roadmap).

| Item | Why it waits | Waits on |
| --- | --- | --- |
| The graphical sign-in, its startup-trouble menu and its elevate prompt; the version on the graphical sign-in (ADRs 0251, 0253, 0254, 0261) | Need the window manager | Release 1.1 |
| The Precision Touchpad's reports (fingers, scrolling, gestures), touchpad sleep and wake (ADR 0235) | The desktop reads them | Release 1.1 |
| The lid lock, brightness keys and sleep; the sleep button; wake events (ADRs 0142, 0201) | Power management with the desktop | Release 1.1 |
| Mouse selection in the consoles, and fonts beyond Spleen's (ADR 0100) | Console polish | Release 1.1 or a need |
| Console layouts whose letters are not Latin, with a second group (ADR 0149) | Needs group switching in the consoles | Release 1.1 or a need |
| A sound server that mixes, as the one player (ADR 0320) | The desktop's sound | Release 1.1 |
| Version 2 of the bundle manifest: `entries`, `actions` and `display` (ADR 0221) | Desktop integration | Releases 1.1 and 1.5 |
| The Trash for deleted bundle data; installing by dropping a file into `~/applications` (ADR 0223) | Desktop features | Release 1.1 |
| Optional permissions asked through the desktop's portals (ADR 0224) | Portals | Release 1.5 |
| Secrets sealed by the TPM (ADR 0217) | TPM support | Release 1.2 |
| Password reset with the recovery key once disks are encrypted (ADR 0253) | Disk encryption | Release 1.4 |
| Bundle signing keys with passwords (ADR 0222) | Key store | Release 1.6 |
| zellij | Needs Wasmtime | Release 1.5 |
| Ogg Opus and Vorbis for music CDs (ADR 0447) | Needs the media service's libopus port | The media service |
| Crash dumps of all the kernel's memory, which a post-mortem tool reads with the kernel's symbols (ADR 0483) | The text dump holds the state a person reads without a tool; a dump of memory is as large as the RAM and needs somewhere to be written from the panic path | Release 1.13 |

## Waiting on a port, an upstream or a description

| Item | Why it waits | Waits on |
| --- | --- | --- |
| The rest of the Rust batch: fd, hyperfine (Linux perf events gated off), dust, delta, eza, zoxide, tealdeer, starship (Radix sources for `sysinfo` and the battery crate) | Paused after ripgrep and bat | Approval to continue |
| The C and C++ batches: make, bash, zsh, fish, Vim, Perl, GnuPG, htop, btop with a Radix collector, ncdu 1.x, mosh, an IRC or Matrix client | Follow the Rust batch | Approval to continue |
| delta in the catalog | Needs libgit2; its xdiff is LGPL, to be checked | License review |
| yazi in the catalog | Needs tokio support | Port work |
| nmap in the catalog | NPSL license | Legal review |
| Advisories for ecosystems other than crates.io; bundles over 4 GiB (ADRs 0222, 0389) | npm's export and large bundles need ZIP64 in `dkv` | A need |
| mlibc functions with no code, which end the program (posix design note) | Each is written when a port needs it, upstream where general | A port |
| `crypt(3)`, for nginx's `auth_basic` with `$5$` and `$6$` hashes (ADR 0491) | The C library has none; nginx's own schemes, `$apr1$` among them, work | A port that needs it, or upstream |
| jemalloc for Valkey, with its active defragmentation: Valkey uses the C library's allocator (ADR 0492) | jemalloc has no port for Radix | A jemalloc port |
| Valkey's own test suite (`runtest --host`) run against the machine from the lab PC; the lab test `services/valkey` stores and reads back keys with a client of its own meanwhile (ADR 0492) | Needs Tcl and Valkey's tests at the bundle's version on the lab PC; many of them need the `DEBUG` command, which Valkey keeps off unless its settings turn it on | A lab step that fetches them |
| Python's modules that need a library Dukovo has no port of: `ctypes` (libffi), `dbm.gnu` and `dbm.ndbm` (GDBM, Berkeley DB), `readline` (GNU Readline or libedit), `uuid`'s C part (libuuid), `tkinter` and IDLE (Tk) (ADR 0495) | Each module is built into `python3` when its library is | A port of each library |
| Python packages with code in C, from PyPI (ADR 0495) | Radix loads no library at run time; Python's own modules in C are built in | Loading libraries, or a bundle that builds a package in |
| The kernel's gaps CPython's suite finds (ADR 0495, `lab/tests/services/python-skips.txt`): `mmap(2)` of tmpfs files; Unix-domain datagram sockets, and Unix-domain sockets passed with `SCM_RIGHTS`, which multiprocessing's suites need; RFC 3542 IPv6 ancillary data, and urgent data sent with `MSG_OOB` (ftplib's `abort()`), which smoltcp does not carry (ADR 0496); `scandir` results | Found by the suite, not by a program that needs them yet | Kernel work, each removing its lines from the skip list |
| PHP's extensions that need a library Dukovo has no port of: `gd` (libgd, with libpng, libjpeg-turbo, libwebp and FreeType), `intl` (ICU), `zip` (libzip), `sodium` (libsodium), `gmp` (GMP), `iconv` (the C library has no `iconv(3)`) and mbstring's regular expressions, `mb_ereg` (Oniguruma) (ADR 0499) | Each is built into `php` when its library is | A port of each library, and `iconv(3)` in the C library |
| PHP's `pgsql` and `pdo_pgsql`, and `sqlite3` and `pdo_sqlite` (ADR 0499) | They need libpq and SQLite | The PostgreSQL port, and a port of SQLite |
| PHP's JIT, for PCRE and for opcache (ADR 0499) | Both want memory that is writable and executable at once, which a program's pages never are on Radix; nginx's PCRE2 is built without JIT as well (ADR 0491) | A decision to allow such mappings, or a JIT that does without |
| PHP's `dl()`, and extensions from PECL (ADR 0499) | `php` is a static program that loads no library at run time | Loading libraries, or building PHP with `-dynamic` (ADR 0071) for a catalog that needs extensions of its own |
| PHP's `dns_get_record`, `dns_check_record` (`checkdnsrr`) and `dns_get_mx` (`getmxrr`) (ADR 0499) | They need `res_search(3)`, which the C library lacks | `res_search(3)` in the C library, or a resolver of PHP's own |
| The system's gaps PHP's suite finds (ADR 0499, `lab/tests/services/php-skips.txt`): the C library's `gethostbyname_r(3)` finds nothing (PHP is built to use `gethostbyname(3)`, which does), nor do its `getprotobyname(3)` and `getprotobynumber(3)`; its `getservbyport(3)` ignores the protocol, `wait3(2)` is not implemented and `sysconf(3)` lacks some names; Unix-domain datagram sockets; the socket options `TCP_CONGESTION`, `SO_ZEROCOPY`, `IPV6_PKTINFO` and `IPV6_RECVPKTINFO`; the ioctl that gives an interface's address by its number; a SIGCHLD's sender's user number; a tmpfs folder of more than 32,768 entries; `fsync(2)` of a descriptor that is not a file; a slash after a file's name, which Linux refuses; `utimes(2)` of a time before 1970; `getcwd(3)` in a removed folder | Found by the suite, not by a program that needs them yet | Kernel and C library work, each removing its lines from the skip list |
| Installing a bundle of 20,000 files, PHP's, for everyone on a machine of 2 GiB (ADR 0499, ADR 0520) | `bundle install` stops with `no_memory` on one of the files, and Files says its write-back to the disk ran out of memory; the boot tests `php` and `php-fpm` give their machine 4 GiB | Kernel work: finding what a machine of 2 GiB runs out of while the root's Btrfs takes the files |
| PHP-FPM's slow log, `request_slowlog_timeout` (ADR 0520) | It reads a worker's stack with `ptrace` or `/proc/PID/mem`, which Radix does not offer | `ptrace` or `/proc/PID/mem` |
| Installing a service bundle's commands for one person, without its services: PHP's `php` now, PostgreSQL's `psql` next (ADR 0385, ADR 0520) | A bundle with services installs only for everyone, so a person who is not an administrator cannot have PHP's command line for themselves | A decision by the maintainers on how a bundle offers its commands alone |
| PHP-FPM pools that run as different accounts (ADR 0520) | PHP-FPM must start as root to give a pool another account, and a bundle's service never runs as root (ADR 0385); every pool runs as `php` | A decision to let a service start as root and drop to accounts of its own |
| A cap on the memory a user's pipes hold (Linux's `pipe-user-pages-soft`) (ADR 0497) | Pipe buffers are kernel memory, not yet charged to a resource domain | Charging kernel memory to resource domains |
| The RTS5260 card reader; UHS-I, MultiMediaCards and Memory Sticks on Realtek readers (ADR 0448) | Only GPL drivers describe them | Realtek's documentation or a BSD driver |
| AML fixes reach Dukovo when uACPI is vendored again (ADR 0200) | `aml=off` works around a firmware table uACPI cannot run until then | Upstream |

## Waiting on a need

The decision records leave these until a benchmark, a program or a
machine asks for them.

### Kernel and memory

| Item | Source |
| --- | --- |
| Spark's pet of the hardware watchdog came more than 10 s late while nginx served the lab PC's load on the OptiPlex 7040: what holds Spark's loop up (nginx's access log copied into the system's log is the first suspect) | Lab, services/nginx on the OptiPlex, 2026-11-13 |
| Transparent huge pages for anonymous memory | ADR 0121 |
| Batched and lazy TLB shootdowns | ADR 0020 |
| A lazy IOMMU mode | ADR 0164 |
| Memory hot-plug; NUMA tested beyond QEMU's emulated nodes | memory design note |
| Swap files; zram or zswap; read-ahead on swap-in; reading the dirty bit so clean pages are not written | ADR 0152, memory design note |
| Lower page tables charged to the process's resource domain | processes design note |
| Kernel objects (threads, page tables) retyped from a process's own memory | capabilities design note |
| radix-rt's heap giving memory back | processes design note |
| The boot archive mapped from its module's pages rather than copied | ADR 0049 |
| Reseeding the random number generator from fresh entropy | random design note |
| A tree in place of scanned run-queue vectors | scheduler design note |
| The tree lock under heavy capability passing on many CPUs | ipc design note |
| Queued messages charged to a resource domain | ipc design note |
| A process's handles freed before its parent is told it ended | ADR 0309 |
| System V message queues (`msgget`, `msgsnd`, `msgrcv`), which PHP's `sysvmsg` extension needs; libc-test's `ipc_msg` | ADR 0490, ADR 0499 |
| Folders and symbolic links in `/dev/shm` | ADR 0490 |
| A memory filesystem, and so System V keys, for each account or view, as Linux's IPC namespaces | ADR 0490 |
| `ipcs` and `ipcrm`; `IPC_INFO`, `SHM_INFO`, `SEM_INFO` and the `*_STAT` commands that take an index | ADR 0490 |
| `shmat`'s `SHM_EXEC` with write access, which Radix never maps together | ADR 0490 |
| The crash dump written to a disk from the panic path through polled drivers, so that a power cycle or a firmware that clears the RAM keeps it | ADR 0483 |

### Files and storage

| Item | Source |
| --- | --- |
| A name cache for path lookups | storage design note |
| Shared writable mappings of files other than `/dev/shm`'s, and `msync`, which SQLite's WAL between connections and its memory-mapped I/O need (ADR 0511) | posix design note, ADRs 0110, 0490, 0511, `lab/tests/services/sqlite-skips.txt` |
| Read-ahead in the background; write clustering; `O_DIRECT` for files | storage design note, ADR 0133 |
| FAT read-ahead | storage design note |
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
| FUSE and share mounts: a page cache with writeback | ADR 0226 |
| A page or partition written raw through two different nodes at once | storage design note |

### Networking and sharing

| Item | Source |
| --- | --- |
| An interface MTU below 1500 from DHCP's option 26 | ADR 0197 |
| ENA's low-latency queues; gVNIC's DQO queue format; SFP+ modules; fiber and SerDes gigabit cards | ADR 0197 |
| Legacy INTx interrupts through `_PRT`, for cards and SD controllers without MSI | ADRs 0197, 0198, 0437 |
| MSI-X with several queues in driver programs | ADR 0198 |
| Intel PCH Ethernet: Energy Efficient Ethernet, wake on LAN, jumbo frames, I217, I218, newer I219s, 82577 and 82578 | ADR 0361 |
| Promiscuous mode for packet capture | ADR 0346 |
| Firewall rules for outbound traffic and rate limits | ADR 0334 |
| Routing beyond a tunnel's subnet before the routing table | ADR 0339 |
| timed: choosing among servers, training the clock's frequency, `CLOCK_REALTIME` timers after a step, authenticated fallback, leap seconds | ADRs 0194, 0487 |
| `AF_UNIX` datagram and sequenced-packet sockets, `MSG_PEEK`, credentials as control messages | ADR 0091 |
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
| Detecting another DHCP server on a card's network | ADR 0486 |
| NTS keys shared between several time servers (RFC 8915, section 6): the cookies of timed's server work at that server alone | ADR 0487 |

### Devices and firmware

| Item | Source |
| --- | --- |
| IOMMU: devices behind a PCIe-to-PCI bridge, the display controller's DMA, 5- and 6-level AMD tables, HPET MSIs, posted interrupts, 128-bit AMD-Vi entries | ADRs 0164 to 0166 |
| Watchdogs of Intel Atom SoCs and AMD SB700 and SB800 | ADR 0167 |
| Machine checks: bad pages dropped from the file cache, a kernel copy from user memory ending its call, counts by DIMM, Zen's data-fabric address translation and deferred-error interrupt, retiring a page after many corrected errors, resetting the counts | ADRs 0168, 0169, 0204 |
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
| Braille displays that are HID or USB-bulk devices | ADR 0326 |
| Hyper-V: disks added while running, the mouse, the video device, dynamic memory, key-value exchange | ADR 0423 |
| virtio-scsi: the event queue for hot-plugged disks, and task management | ADR 0327 |
| Floppy drives 2 and 3, 5.25-inch drives, and 2.88 MB disks | ADRs 0435, 0436 |
| CompactFlash in PCMCIA and CardBus adapters | ADR 0446 |
| MS-JIS CD-Text | ADR 0447 |
| Writing CD-Text when burning a CD | ADR 0449 |
| Serial ports: typing by INTx on PCI ports without MSI, and by MSI-X on functions that have only that (they are polled); COM3's interrupt, which is COM1's (it is polled); memory-mapped 16550s, such as AMD's `AMDI0020` and Intel's LPSS UARTs (not used); the AX99100's 256-byte FIFO and rates above 115200 | ADRs 0360, 0480 |
| FLAC with linear prediction, a few per cent smaller | ADR 0447 |

### Accounts, bundles and the console

| Item | Source |
| --- | --- |
| An elevated command in the person's view; clearing a person's `/tmp` after their last session | ADR 0255 |
| Sign-in records kept for a time; notices of a failed sign-in streak | ADRs 0253, 0254 |
| The administrator set split into named areas for delegation | ADR 0219 |
| A user's own services and their lines in their log; a group that reads the whole log | ADR 0252 |
| A Unix socket of one bundle's service that another bundle's services reach, as Valkey's and PHP-FPM's are on Linux: a service's data folder is its account's alone, so they talk over TCP on the machine itself, nginx and PHP-FPM on 127.0.0.1:9000, where any program on the machine may connect | ADR 0492, ADR 0520 |
| Rich and NFSv4 access lists; Btrfs properties | ADR 0256 |
| Bundles: grants before install (`--grant`), resource limits for the sandbox, programs for other architectures, read-only subvolumes and reflink sharing for sealed bundles, asking people about their data when a bundle is removed for everyone | ADRs 0223 to 0225 |
| Views: detaching an entry; devices attached one by one | ADR 0215 |
| Manual pages inside bundles on `MANPATH` | ADR 0370 |
| Removing an everyone's line also removes each person's add-ons | ADR 0388 |
| A wavy underline in the consoles | ADR 0258 |
| Pairing unpaired diff runs in git's word marks | ADR 0293 |
| Pseudo-terminal packet mode, `TIOCSTI` and BSD line disciplines | ADR 0066 |
| Cloud first boot: Google Cloud and OpenStack metadata, NoCloud from the command line or SMBIOS, `bootcmd`, `timezone`, `ssh_pwauth`, `chpasswd` lists, bonds, bridges and VLANs | ADR 0328 |
| The guest agent's other commands | ADR 0329 |
| Installer translations | ADR 0310 |
| GRUB's menu refreshed when Windows is added or moved later | ADR 0314 |
| Rebuilding a mirror onto a new disk without manual steps | ADR 0315 |
| `ls --app` naming the app that downloaded a file (needs provenance tags) | ADR 0442 |
| Bit-for-bit reproducible images | ADR 0010, ADR 0382 |
| A git settings page in Settings (on hold) | Git work, ADR 0287 |

### Benchmarks

| Item | Source |
| --- | --- |
| `posix_git_status` reported, not gated, until parity | ADR 0294 |
| Optical reads reported, not gated, until parity; USB storage rows | ADR 0434 |
| `git status` takes about 2.5 times Linux's time (5.0 ms against 1.98 ms on 1,000 files, with 15 context switches); no work is planned to close it yet | ADR 0294 |
| Rows still above the 0.90 margin target, though under Linux: Btrfs zstd write, unmapping a page on another CPU, NVMe 1 MiB read (0.92 to 0.94) | Benchmark margin work |
| On real hardware (OptiPlex 7040, i5-6500) rows over Linux that pass under QEMU: loopback UDP round trips 3.7 to 3.9 times, TCP round trips 2.2 to 2.6, TCP 1 MiB 2.65, TCP connect 1.23; `poll` 2.23; shared futex across CPUs 1.38; fork, exec and wait 1.38; pipe 1 MiB 1.12 | Lab, performance/ipc, loopback and processes on the OptiPlex, 2026-11-13 |

## Lab findings to confirm

Seen while the lab harness was built, before findings were recorded in the
lab's results. Each is checked on the current system and either filed as a
finding or removed.

| Item | Source |
| --- | --- |
| virtio-net's link going up or down is not logged on the serial console | Lab harness, part 4 |
| A redirected subshell sent to the background in Nukesh keeps an SSH session open | Lab harness, part 7 |
| No AMT OS agent: a graceful power-off asked through AMT is not seen, so `boot/power-button` fails on the Intel lab machine | Lab, first real-machine runs |
| authd's errors (stderr) reach the console but not log(1); log lines longer than one write are split and the pieces lose their priority and tag | Lab, first real-machine runs |
| `lsof -i` lists no sockets and `ss -p` is not supported | Lab, first real-machine runs |
| timedatectl prints its name twice in errors; sudo's refusal names "Phase 13d"; timed says "Unknown error (6)" for a failed lookup and "()" when no NTS server answered | Lab, first real-machine runs |
| No /etc/os-release for scripts that read it | Lab, first real-machine runs |
| A lab test for COM1 as the serial console (console=ttyS0): serial-console running on it and a sign-in over it; hardware/serial-port covers COM1 only when it is not the console | Lab, first real-machine runs |
| SSH: RSA and P-521 keys and authorized_keys options are not taken; sshd_config takes eight keywords (no `Port`, `ListenAddress`, `MaxAuthTries`); ssh is built without OpenSSL, so ssh-keygen makes ed25519 keys alone | Lab, first real-machine runs |
| "[elevate] password for lab:" is printed into the output of commands run with sudo -S | Lab, first real-machine runs |

## Waiting on a decision

| Item | Why it waits |
| --- | --- |
| The SD 1 MiB benchmark rows: printed but not gated | They sit at 1.00 because QEMU's SD emulation takes nearly all of both systems' time |
| Printing: the design's open questions | Drafted for a release after 1.0 |
| Known weak spots to plan for: Wi-Fi, Bluetooth and webcams; large desktop apps; Btrfs maturity | Raised, not yet discussed |
| Nightly system releases on the update server, and the key that signs them (ADRs 0456, 0467); to decide before 1.0.0-alpha.1 | The server offers stable and beta; a nightly release would be signed at every merge with no person there, and ADR 0457 keeps release keys off servers and out of CI |
| The update server's online keys kept encrypted at rest, as ADR 0457's table has them (ADRs 0382, 0467); to decide before 1.0.0-alpha.1 | They are files only their system users read; a timer signs with them with nobody there, so a key the server decrypts itself guards only a copy of its disk |

## Documentation to bring up to date

| Item | Why |
| --- | --- |
| Support center pages built from the settings schemas (each field with the release it arrived in and its stability) and stability labels on the pages ridlc writes from `abi/*.ridl` (ADR 0419) | `ob docs site` renders the docs in the tree first; these generators follow it |
| Error pages in `docs/errors/`, one per error ID such as `UPD-103` (ADR 0420) | No program gives its errors IDs yet; the first area to do so adds its row to `docs/errors/README.md` and a page for each ID |
| A scheduled check of the support center's links to other sites (ADR 0420) | Pull requests check only the links between the site's own pages; outside links need a job of their own |
| The *Known limits* sections of `docs/design/posix.md` and `docs/design/storage.md`, and radix-posix(7) | They still describe Phases 7 to 9: no network sockets, no users, no hot plug, no exFAT, no Btrfs quotas, locales "until Phase 12" |
| The roadmap's catalog ports paragraph says Go comes with release 1.1 | Go is Phase 17 |

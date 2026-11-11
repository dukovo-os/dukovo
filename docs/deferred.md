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
| Phase 16 (server platform and web console) | Plan approved; starts once Phase 15 is complete, on approval | Phase 15, then a decision |
| The build container's own package step falling back to kernel.org's Ubuntu mirror, as the CI jobs' install step does: its `apt-get` stops when Ubuntu's mirrors do not answer | The Containerfile keys the CI cache of built ports, so changing it rebuilds every port from source | The Containerfile's next change |
| System releases signed on the two hardware keys: the system's TUF root, `ob release sign` with its gates and a third build that must match CI's two, the Rekor record, `ob keys ceremony` and the key runbooks (ADRs 0450, 0457) | Phase 15 signs with each build machine's own keys and tests against a local update server | Before 1.0.0-alpha.1 |
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
| Lab harness parts 9 and 10: tests on real machines, power and screen through AMT, network boot from the lab PC | Need the machine | Intel lab machine |
| AMD lab machine set up as a lab target (no AMT: a smart plug, network boot and network logs) | Need the machine | AMD lab machine |
| The lab PC's half of `sharing/nfs-server` | Needs a kernel NFS client, which the build container lacks | A run on the lab PC |
| Tests that need two disks besides the stick, such as installing the mirror layout (`install/mirror-needs-two`, ADR 0315) | The lab gives a test one disk | Lab harness |
| A lab image without the lab account, to test the first boot's "no account yet" page (ADR 0401) | The lab image always has its account | Lab harness |
| `ob lab` checks that a machine is free before a run; a Stop for one test | Left out of the harness's first parts | Lab harness |
| A lab test of a keyring entry set and read back | Needs a console sign-in with a password, which tests cannot type yet | Lab harness |
| A lab test of `whois` | Needs a reachable whois server | Lab harness |
| Receive-side scaling, checksum and segmentation offloads for the server cards and virtio-net; GSO and multi-queue tunnels (ADR 0339) | Measured against Linux on 10 GbE, and the lab's network is 1 GbE; moved out of Phase 16 so it does not hold 1.0.0-alpha.1 | 10 GbE in the lab: two Intel X520, X540 or X550 cards and a cable |

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
| Rescue kept up to date: a newer copy written to the EFI system partition once per stable release, when an update is proved (ADR 0460) | Rescue is written by the installer and images until the unified updater | 15d |
| Rescue's reinstall from the update server (ADR 0460) | Rescue reinstalls from its own copy or an update folder until delivery comes | 15c |
| USB repair, which also rewrites GRUB and the EFI system partition, and the installer's reinstall that keeps data (ADRs 0459, 0460) | Rescue covers a machine whose EFI system partition still starts | The installer's next changes |
| Rescue resetting the settings to their defaults (ADR 0460) | Putting back the settings from before the last update covers a bad update | A need |
| Fetching a release's changed files from the update server, the offline update file, `sysupdate verify` and a weekly check with the scrub (ADR 0451) | `sysupdate stage` reads a repository folder until delivery comes | 15c |
| Waves, the rollout role, canaries, pause and pull (ADR 0456) | Update delivery | 15c |
| The owner told of a fallback at sign-in and on the web console, with the offer to send a report (ADRs 0453, 0456) | The unified updater's notifications | 15d |
| `rollback --settings-from-before`, shown before a rollback past the release before (ADR 0454) | The two-version rule covers one step | 15d |
| sysupdate's own narrower capabilities in place of the administrator's access (ADR 0455) | Needs a view of the Btrfs root that leaves out `@users` | 15d |
| Settings migrations, run before the services start, each with tests in both directions (ADR 0454) | No release has changed a settings format yet | The first release that does |
| On a mirror, a try GRUB counts on one disk's EFI system partition is not counted on the other's until the system next writes the state (ADR 0452) | A machine must start from each disk in turn to try an update more than three times | A need |
| When GRUB cannot write the slot state, the menu still titles the try that it then does not start (ADR 0452) | The default starts, as it should; only the title is wrong | A need |
| The failed attempt's crash log kept beside its settings (ADR 0453) | The log of the fallback says what failed | A need |
| Installs made before 0.15.0 keep a menu that starts `@system.a` directly, and are reinstalled to take updates (ADR 0451) | No public install exists before 1.0.0-alpha.1 | A need |
| Btrfs `send` and `receive` (ADR 0138) | Incremental backups need them | 15b |
| Rolling a whole place back to a snapshot, such as all of `/users` (ADR 0461) | Restoring by file or folder covers mistakes; a whole place goes back only with every program that writes there stopped | Rescue's next changes |
| Apps restoring their own data from a snapshot (ADR 0461) | An app's view has no snapshot; a person restores its folder with `snap restore` | A portal for apps |
| Retention set place by place in `snap.toml` (ADR 0461) | One retention serves the default places | A need |
| The `snap` service's own view in place of the whole tree (ADR 0461) | `snap.toml` may name any subvolume, reached through the top of each filesystem | 15d, with the updater's (ADR 0455) |
| `BTRFS_IOC_GET_SUBVOL_INFO` on a descriptor of a file that is not a folder (ADR 0461) | The C library asks a folder's `subvolume` method; `ls` and `snap` ask of folders | A need |
| System updates served by the update server (ADR 0382) | Catalogs are served; system images come with update delivery | 15c |
| Telling a person of a new flaw, and which advisories an update fixes (ADR 0389) | Needs the unified updater's notifications | 15d |

### Phase 16: server platform and web console

| Item | Why it waits | Waits on |
| --- | --- | --- |
| Service dependencies, readiness, health checks and timers; Spark orders services by `order` alone today (ADR 0101, ADR 0216) | Production service management | 16a |
| A user's jobs that run while they are signed out (ADR 0220) | Needs jobs | 16a |
| Sockets a bundle offers to others, and starting a service at its first connection (ADRs 0385, 0386) | Socket activation | 16a |
| `service status` listing what a service holds; volumed and diskhealth given disks one by one; `/processes` showing a service or session only its own processes (ADRs 0215, 0217) | Finer views for services | 16a |
| Serial ports as terminals programs open, and a choice of which ports carry the log (ADR 0360) | Full serial console | 16b |
| A local time server, and netd's DHCP server (ADR 0194) | Server roles | 16b |
| elevate in the web console, with comments kept by its settings editor (ADRs 0216, 0219) | Dukovo Admin | 16c |
| A certificate from certd for nginx's sites and for the lab test `services/nginx`, which makes one with openssl meanwhile (ADR 0491) | certd comes with Phase 16 | Phase 16's certd |
| The services a bundle's service needs started first, and its readiness: nginx's manifest names none (ADR 0491) | Manifests get dependency lines with the service graph | 16a |
| Spark setting a service's `open_files` (RLIMIT_NOFILE); nginx raises its own to the hard limit, 4096, meanwhile (ADRs 0385, 0491) | Service limits | 16a |
| A service's own log files, turned over: nginx logs to the system's log, and an `access_log` to a file of its own is never turned over (ADR 0491) | Needs timers | 16a |

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

## Waiting on a port, an upstream or a description

| Item | Why it waits | Waits on |
| --- | --- | --- |
| Python (CPython) in the catalog | Its own item on the catalog ports track | Approval to start |
| The rest of the Rust batch: fd, hyperfine (Linux perf events gated off), dust, delta, eza, zoxide, tealdeer, starship (Radix sources for `sysinfo` and the battery crate) | Paused after ripgrep and bat | Approval to continue |
| The C and C++ batches: make, bash, zsh, fish, Vim, Perl, GnuPG, htop, btop with a Radix collector, ncdu 1.x, mosh, an IRC or Matrix client | Follow the Rust batch | Approval to continue |
| delta in the catalog | Needs libgit2; its xdiff is LGPL, to be checked | License review |
| yazi in the catalog | Needs tokio support | Port work |
| nmap in the catalog | NPSL license | Legal review |
| Advisories for ecosystems other than crates.io; bundles over 4 GiB (ADRs 0222, 0389) | npm's export and large bundles need ZIP64 in `dkv` | A need |
| mlibc functions with no code, which end the program (posix design note) | Each is written when a port needs it, upstream where general | A port |
| `crypt(3)`, for nginx's `auth_basic` with `$5$` and `$6$` hashes (ADR 0491) | The C library has none; nginx's own schemes, `$apr1$` among them, work | A port that needs it, or upstream |
| The RTS5260 card reader; UHS-I, MultiMediaCards and Memory Sticks on Realtek readers (ADR 0448) | Only GPL drivers describe them | Realtek's documentation or a BSD driver |
| AML fixes reach Dukovo when uACPI is vendored again (ADR 0200) | `aml=off` works around a firmware table uACPI cannot run until then | Upstream |

## Waiting on a need

The decision records leave these until a benchmark, a program or a
machine asks for them.

### Kernel and memory

| Item | Source |
| --- | --- |
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
| System V message queues (`msgget`, `msgsnd`, `msgrcv`); libc-test's `ipc_msg` | ADR 0490 |
| Folders and symbolic links in `/dev/shm` | ADR 0490 |
| A memory filesystem, and so System V keys, for each account or view, as Linux's IPC namespaces | ADR 0490 |
| `ipcs` and `ipcrm`; `IPC_INFO`, `SHM_INFO`, `SEM_INFO` and the `*_STAT` commands that take an index | ADR 0490 |
| `shmat`'s `SHM_EXEC` with write access, which Radix never maps together | ADR 0490 |

### Files and storage

| Item | Source |
| --- | --- |
| A name cache for path lookups | storage design note |
| Shared writable mappings of files other than `/dev/shm`'s, and `msync` | posix design note, ADRs 0110, 0490 |
| Read-ahead in the background; write clustering; `O_DIRECT` for files | storage design note, ADR 0133 |
| FAT read-ahead | storage design note |
| Btrfs: trim as blocks are freed (`discard`), small files inline, compression of small files, a per-CPU compression workspace, removing empty block groups, deleting a subvolume in the background, scrubbing in a thread of its own, a log of fsyncs a path at a time, tree blocks read whole | storage design note, ADRs 0130, 0158 |
| Btrfs RAID5 and RAID6 | Never planned (storage design note) |
| Removing a subvolume with a file of it open, or the default subvolume | ADR 0138 |
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
| timed: choosing among servers, training the clock's frequency, `CLOCK_REALTIME` timers after a step, authenticated fallback, leap seconds | ADR 0194 |
| `AF_UNIX` datagram and sequenced-packet sockets, `MSG_PEEK`, credentials as control messages | ADR 0091 |
| A per-person setting for low ports | ADR 0257 |
| SMB: pipelined reads and writes, a page cache, leases and oplocks, change notices, locks other sessions see, `LsarQueryInformationPolicy` | ADRs 0227 to 0229 |
| NFS: locks the server sees, delegations, a page cache, several calls at once, an account-mapping table, `SP4_MACH_CRED`, contexts kept across a restart | ADRs 0344, 0399 |
| Kerberos: FAST, PKINIT, cross-realm tickets, renewing in place, finding KDCs by DNS; GSS-API for SSH and SMB | ADRs 0397, 0398 |
| SSH: `PermitOpen`, `PermitListen`, X11 and agent forwarding, Unix sockets, listen addresses by name, keyboard-interactive | ADRs 0348, 0349 |
| nginx's HTTP/3 (QUIC) | ADR 0491 |
| SYN cookies, and connections queued for `accept` without their 128 KiB of buffers, as Linux's request sockets are, for backlogs past 128 on an interface | ADR 0494 |

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
| FLAC with linear prediction, a few per cent smaller | ADR 0447 |

### Accounts, bundles and the console

| Item | Source |
| --- | --- |
| An elevated command in the person's view; clearing a person's `/tmp` after their last session | ADR 0255 |
| Sign-in records kept for a time; notices of a failed sign-in streak | ADRs 0253, 0254 |
| The administrator set split into named areas for delegation | ADR 0219 |
| A user's own services and their lines in their log; a group that reads the whole log | ADR 0252 |
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

## Lab findings to confirm

Seen while the lab harness was built, before findings were recorded in the
lab's results. Each is checked on the current system and either filed as a
finding or removed.

| Item | Source |
| --- | --- |
| virtio-net's link going up or down is not logged on the serial console | Lab harness, part 4 |
| A redirected subshell sent to the background in Nukesh keeps an SSH session open | Lab harness, part 7 |

## Waiting on a decision

| Item | Why it waits |
| --- | --- |
| The SD 1 MiB benchmark rows: printed but not gated | They sit at 1.00 because QEMU's SD emulation takes nearly all of both systems' time |
| Printing: the design's open questions | Drafted for a release after 1.0 |
| Known weak spots to plan for: Wi-Fi, Bluetooth and webcams; large desktop apps; Btrfs maturity | Raised, not yet discussed |

## Documentation to bring up to date

| Item | Why |
| --- | --- |
| Support center pages built from the settings schemas (each field with the release it arrived in and its stability) and stability labels on the pages ridlc writes from `abi/*.ridl` (ADR 0419) | `ob docs site` renders the docs in the tree first; these generators follow it |
| Error pages in `docs/errors/`, one per error ID such as `UPD-103` (ADR 0420) | No program gives its errors IDs yet; the first area to do so adds its row to `docs/errors/README.md` and a page for each ID |
| A scheduled check of the support center's links to other sites (ADR 0420) | Pull requests check only the links between the site's own pages; outside links need a job of their own |
| The *Known limits* sections of `docs/design/posix.md` and `docs/design/storage.md`, and radix-posix(7) | They still describe Phases 7 to 9: no network sockets, no users, no hot plug, no exFAT, no Btrfs quotas, locales "until Phase 12" |
| The roadmap's catalog ports paragraph says Go comes with release 1.1 | Go is Phase 17 |

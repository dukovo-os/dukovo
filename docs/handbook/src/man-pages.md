<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# Manual pages

Every Dukovo command, file format and interface has a manual page.
`./ob docs` compiles them into `out/docs/man/`; read one with
`man -l out/docs/man/man1/ob.1`. The sources are in `docs/man/`, except
for section 2, which `ridlc` generates from the interface definitions in
`abi/*.ridl` so that the pages always match the code.

| Page | Describes |
| --- | --- |
| `ob(1)` | The build tool: every command and option |
| `dukovo.toml(5)` | The build configuration file |
| `radix-proc(5)` | `/processes`: the processes, the mounts, memory and uptime as text |
| `radix-sysfs(5)` | `/kernel`: the kernel's view of the hardware and its settings, one value per file, in Linux's layout |
| `dukovo-boot(7)` | How Dukovo boots, its consoles, the kernel command line and microcode |
| `radix-debug(7)` | Panic reports, the crash log and crash dump, the kernel debugger, the lock checker, GDB and trace points |
| `radix-capabilities(7)` | Capabilities: object kinds, rights, badges, sealing, delegation and revocation |
| `radix-scheduling(7)` | Threads, scheduling classes, priority inheritance, RCU, resource domains and the out-of-memory policy |
| `radix-ipc(7)` | User mode, the system calls, endpoints, notifications, channels, rings and the random capability |
| `radix-processes(7)` | Processes, program loading, the address space layout and the boot archive |
| `radix-files(7)` | Files, directory capabilities, paths, filesystems, the page cache, the directory hierarchy and `/volumes` |
| `radix-posix(7)` | POSIX programs: the C library and toolchain, fork and exec, signals, descriptors and paths, pipes, terminals, timers, threads, limits, file locks, dynamic linking and Rust's standard library |
| `spark(8)` | Spark, the first program, which starts and supervises every other |
| `sysupdate(8)`, `updates.toml(5)`, `health.toml(5)` | Updates of the system: channels, checking and downloading from the update server, update files, the two slots, staging an update, trying and proving it, the health check and what it asks, falling back, rolling back, and the boot menu |
| `snap(1)`, `snap.toml(5)` | Snapshots: the daily ones of `/users`, `/shared` and `/system/settings` in `/snapshots`, how many are kept, and restoring a file from one |
| `service(8)`, `shutdown(8)`, `reboot(8)` | Starting, stopping and looking at services; stopping, restarting or powering off the machine |
| `consoled(8)` | The virtual consoles on the screen and keyboard, and their keys |
| `authd(8)`, `user(8)`, `passwd(1)`, `login(1)` | Accounts: the service that keeps them, checks passwords and starts sessions, making and listing accounts, changing a password, and signing in at a console |
| `elevate(8)`, `sudo(8)` | Running a command as an administrator, after the password once in each terminal pane every 15 minutes |
| `bundle(1)`, `bundled(8)`, `manifest.json(5)` | Bundles: making and checking a bundle's manifest, each field and permission it may hold, signing, packing and unpacking bundles, the service that installs, seals and removes them, and running them in their sandbox |
| `nukesh(1)` | Nukesh, the shell: options, start-up files, scripts, the line editor, history, completion, job control, POSIX mode and its limits |
| `coreutils(1)`, `7zz(1)` | What the ported core utilities and 7-Zip do on Dukovo |
| `systools(1)`, `ps(1)`, `top(1)`, `free(1)`, `df(1)`, `uptime(1)`, `w(1)`, `who(1)`, `last(1)`, `lastlog(8)`, `pidstat(1)`, `vmstat(8)`, `lsof(8)`, `scrub(8)`, `subvolume(8)`, `mkswap(8)`, `swapon(8)`, `swapoff(8)`, `dmesg(1)`, `kill(1)`, `pgrep(1)`, `renice(1)`, `ionice(1)`, `taskset(1)`, `watch(1)`, `tree(1)`, `hexdump(1)`, `xxd(1)`, `column(1)`, `which(1)`, `script(1)`, `chmod(1)`, `chown(1)`, `getfacl(1)`, `setfacl(1)`, `reach(1)` | Dukovo's own administration tools, with the Linux tools' names, options and output |
| `rdisk(8)`, `rdisk-layout(5)`, `mkfs(8)`, `mkfs-fat(8)`, `mkfs-exfat(8)`, `fsck-fat(8)`, `mkfs-btrfs(8)`, `fsck-btrfs(8)`, `resize-ntfs(8)`, `rgrub-setup(8)`, `efiboot(8)` | Partitioning disks at a prompt or from a layout file, making and checking FAT, exFAT and Btrfs filesystems, checking and shrinking NTFS volumes, putting GRUB on disks, and the firmware's boot entries |
| `cloud-setup(8)`, `cloud-config(5)` | Setting a cloud machine up at its first boot from its config disk or its cloud's metadata service, and the user data it reads |
| `guest-agent(8)` | Answering a virtual machine's host in QEMU's guest agent protocol: its addresses and system, and a clean shutdown |
| `logd(8)`, `log(1)`, `journalctl(1)`, `logger(1)`, `logtrim(8)`, `log(5)` | The system's log and its audit log: what logd keeps, who reads what, how to read it by service, level, time and boot, how a script writes to it, and how much it keeps |
| `dumps(8)` | The kernel's crash dumps: what a panic leaves, saving, listing, showing and deleting the dumps, and testing that a panic is saved |
| `logforward(8)`, `logforward.toml(5)` | Sending the log to syslog servers over TLS, TCP or UDP: the destinations and what each is sent, the certificate check, how a server that was away is sent what it missed, and the commands that show where each stands and test it |
| `metricsd(8)`, `metrics(1)`, `metrics.toml(5)` | The machine's history and its early warnings: what is kept and for how long, showing it now and over any time range, the warnings and when each is raised and cleared, and the Prometheus endpoint |
| `notifyd(8)`, `notify-send(1)`, `notifications(1)` | Notifications: sending them, how the console shows them, and listing them |
| `alertd(8)`, `alerts(8)`, `alerts.toml(5)` | Alerts: what raises one and what is told, the notice at sign-in, email and webhooks with their retries, the command that lists, tests, raises and clears them, and the file that names where alerts go |
| `dukovo-privacy(7)` | The privacy promise: what Dukovo keeps on the machine and what it sends on its own |
| `rtrace(1)` | Tracing a process's system calls and capability messages |
| `radix_cap_call(2)`, `radix_send(2)`, `radix_recv(2)`, `radix_reply_recv(2)`, `radix_notify(2)`, `radix_wait(2)`, `radix_yield(2)`, `radix_map(2)`, `radix_unmap(2)`, `radix_ring_setup(2)`, `radix_ring_enter(2)`, `radix_debug_log(2)` | The twelve system calls, one page each, with their registers and errors |
| `radix-capability(2)`, `radix-memory(2)`, `radix-address-space(2)`, `radix-thread(2)`, `radix-endpoint(2)`, `radix-notification(2)`, `radix-random(2)`, `radix-ring(2)`, `radix-kernel-log(2)`, `radix-process(2)`, `radix-signal(2)`, `radix-terminal(2)`, `radix-directory(2)`, `radix-file(2)`, `radix-resource-domain(2)`, `radix-framebuffer(2)`, `radix-input(2)`, `radix-power(2)`, `radix-network(2)`, `radix-names(2)`, `radix-trace(2)`, `radix-startup(2)`, `radix-errors(2)` | The interfaces of each kind of object, the start-up page and the kernel's errors |

The software Dukovo ports from elsewhere brings its own pages, such as
`grep(1)`, `sed(1)`, `tmux(1)`, `nano(1)` and `bsdtar(1)`, and `man -k`
searches them all with Dukovo's own.

Sections follow the usual convention: 1 user commands, 2 system calls,
3 library interfaces, 4 devices and drivers, 5 file formats, 7 overviews
and concepts, 8 administration.

## On the web

Every page listed here, with the rest of this handbook, the design notes
and the decision records, is also in the support center at
[dukovo.org/support](https://dukovo.org/support), with search. `./ob docs
site` builds it into `out/docs/site/` from the same files, so the pages on
the web and the pages on your machine never differ.

Examples marked as tests in the handbook are typed at a test system's
console by the `docs-examples` boot test, so the output they show is the
output you get.

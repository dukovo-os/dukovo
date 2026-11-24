<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# Lab tests

The boot tests run in QEMU on every change. The Dukovo Lab's tests go
further: they pull network cables, cut power, plug in USB sticks and run
on real machines as well as in QEMU. Each test is a small TOML file in
`lab/tests`, and `dukovo-lab`, the lab's harness, runs them
([ADR 0401](https://github.com/dukovo-os/dukovo/blob/main/docs/adr/0401-the-lab-tests-a-lab-image-over-ssh-with-dukovo-lab.md)).

## Running them

One command sets the lab up and opens it:

```sh
./ob lab
```

It checks that QEMU, OVMF, OpenSSH, git and gzip are installed (and
prints the `apt install` line if not), installs `dukovo-lab` or updates
it to the newest, makes the lab's key and password, clones the private
dukovo-os/lab-results repository that Send uses, turns on the timer that
starts schedules, builds the lab image and opens the runner page. Each
setup step is skipped once it is done, so after the first time it only
builds and opens. The clone needs this PC's GitHub account to have push
access to lab-results, through an SSH key on GitHub or a sign-in git
keeps (`gh auth login`); without it the page still opens, with Send off.

The same from a terminal, a command at a time:

```sh
./ob lab list          # every test
./ob lab run --all     # build the lab image and run every test on QEMU
./ob lab run boot/     # one category
./ob lab run network/link-pull --machine qemu,qemu-bios
./ob lab run --preset quick         # the presets are in lab/tests/presets.toml
./ob lab attack                     # the security range: run the security category
./ob lab ui                         # the runner page, in a browser on this PC
./ob lab findings                   # what failed, each failure with the runs it came back in
./ob lab send --all                 # new findings and their runs, to lab-results
./ob lab schedule                   # the schedules: when each runs next, how it went last
./ob lab schedule save nightly --preset quick --at 01:00   # every night at 1:00 AM, Mountain Time
```

`./ob lab run` and `./ob lab ui` build the lab image,
`out/dukovo-0.17.0-x86_64-lab.img`, first; `--no-build` tests the one
already there. The runner page picks machines and tests, runs them,
shows each run as it goes and what failed, and lists past runs; each of
its buttons is a `dukovo-lab` command, which it shows. After a rebuild,
open the page again so its runs carry the new build's name. A long run
can also go in the background: `./ob lab run --all --detach`, then
`./ob lab status --wait`, `./ob lab results --failed` or
`./ob lab stop`. A run keeps testing the image it started with even if
the image is rebuilt meanwhile. Each run leaves a bundle with the
results, every machine's serial log and the screenshots of failures
under `~/.local/share/dukovo-lab/runs`. Each failure belongs to a
*finding*: the same test failing the same way, so one that comes back
joins it rather than making a new one. On the page, a finding can be
sent to lab-results, where a thread proposes a fix that waits for the
maintainer's approval, or marked not a bug. Without KVM, QEMU is
some four times slower, and the time limits stretch to match.

Schedules run tests by themselves, with the page closed: at a time of
day in Mountain Time, or on each new build on main. A systemd timer
starts them. Each scheduled run brings the lab's own Dukovo checkout
(`~/.local/share/dukovo-lab/dukovo`, never one to work in) up to main,
builds the lab image there and runs that commit's tests, waiting first
if a run holds its machines; then it sends its findings to lab-results,
or holds them for the page, as the schedule says. They are made and
changed on the page (Schedules) or with `./ob lab schedule save`; the
lab's [Schedules](https://github.com/dukovo-os/lab/blob/main/docs/schedules.md)
page has the rest.

## The lab image

`./ob image --lab` is an installed system's disk with four things more,
all outside the system's slot that a release is made from:

- an administrator's account, `lab`, that signs in over SSH with the
  lab's key, and whose password, which elevate asks for, is the lab
  password (ADR 0408);
- sshd turned on;
- a sign-in banner that says it is a test image;
- netlog's settings naming the lab computer, `10.77.0.1`, as its target
  (netlog(5)), so that a real machine with no serial port sends its log
  to the lab, which reads it as the machine's console. The other lab
  machines send it too, which costs nothing.

The lab's private key and password stay on the lab computer, in
`~/.config/dukovo-lab`: the image holds only the public half of the key
and the password's hash. Without a password file the account's password
is one no one knows, and tests that run as root refuse to start. Never
give a lab image to anyone as a release.

## Real machines

Tests run on real machines through the harness's `amt` driver: a PC
with Intel AMT on a lab network of its own, a second network card in
the lab PC and a switch with no route anywhere else. AMT turns the
machine on and off, a serial cable brings its console to the lab PC,
and before each fresh boot the machine boots from the lab network into
a small writer that writes the image under test to its disk
([ADR 0429](https://github.com/dukovo-os/dukovo/blob/main/docs/adr/0429-real-lab-machines-are-written-fresh-by-a-writer-booted-from-the-lab-network.md)).
The writer finds the disk by its serial number, writes it, reads it
back and restarts into it only when the lab finds the image's hash.

The lab's [Real machines](https://github.com/dukovo-os/lab/blob/main/docs/real-machines.md)
page sets a machine up: its firmware, AMT in MEBx, and its machine
file in `~/.config/dukovo-lab/machines`, with the MAC address of its
network card and the serial number of the disk the lab may overwrite.
On the lab PC, give the lab card the lab address with no gateway, and
make sure dnsmasq is there (`dnsmasq-base`, which NetworkManager brings
on Mint; the `dnsmasq` package's own service is not needed):

```sh
nmcli connection add type ethernet ifname <card> con-name dukovo-lab \
    ipv4.method manual ipv4.addresses 10.77.0.1/24 ipv4.never-default yes \
    ipv6.method disabled
sudo apt install dnsmasq-base
```

Then plain `./ob lab` sets the rest up once a machine file names a real
machine. It builds the writer and the network GRUB the machine boots,
installs them in `/srv/dukovo-lab`, and runs dnsmasq for the lab
network as the service `dukovo-lab-dnsmasq`. dnsmasq answers on the lab
card alone, and only for the machines in the machine files; `./ob lab`
refuses a card that carries the PC's default route. Installing asks for
sudo, and only when something changed; when the lab network cannot be
set up, `./ob lab` says why and goes on, for QEMU's tests.
`./ob lab machines check` then checks AMT, the serial port, the lab
network and the rest without changing anything.

`./ob test lab-writer` checks the net boot without a machine: QEMU boots
the network GRUB from its own DHCP and TFTP, on a network laid out as
the lab's, and the writer writes a test image from a pretend lab to the
one of two disks with the right serial number.

## Writing a test

A test is a file at `lab/tests/<category>/<name>.toml`. The categories,
their order and their titles on the runner page are in
`lab/tests/categories.toml`: boot and power, hardware and drivers, sound,
processes and monitors, disks and filesystems, network, SSH, services and
logs, bundles, system tuning, time and date, monitoring, problem reports, accounts
and elevate, commands, the console, the web console, installing, updates,
startup trouble, stress and soak, security, performance against Linux,
file sharing, and user journeys. Add one there when a new kind of test
needs it. This test
pulls the network cable:

```toml
about = "The network cable comes out and goes back in, and the network comes back"
time = "3m"

[[step]]
run = "ip link"
expect = "LOWER_UP"
[[step]]
link = "down"
[[step]]
wait = "10s"
[[step]]
link = "up"
[[step]]
run = "ip link"
expect = "LOWER_UP"
```

`run` steps are commands over SSH as the `lab` account, or as root with
`root = true`, which runs the command through `elevate -S` with the lab
password; `serial` waits for text on the serial console; `power`,
`link`, `usb` (a stick holding an image, or a blank one of a size),
`keys`, `type`, `point`, `click` and `screenshot` work the machine,
`screen` waits for text on its screen (below), `wait` waits for it to be
ready, to turn itself off, or for a time, and `bench` measures it
against Linux (below). A new feature is complete only
with its lab tests ([Contributing](https://github.com/dukovo-os/dukovo/blob/main/CONTRIBUTING.md)). The harness's
[test file reference](https://github.com/dukovo-os/lab/blob/main/docs/test-files.md)
lists every key, and `./ob lab show <test>` shows which machines can run
a test. A new file needs no other change.

A user journey, in `lab/tests/journeys`, is one long test that follows
what an owner does with a new server, on one install: the first sign-in,
accounts and SSH keys, firewall rules, a website with a certificate,
alerts, a restart, an update, backups and a power cut, and a day of two
people and an SSH session at once. It chains what the single-feature
tests check apart, so that a bug in the join shows: state one step leaves
for the next, a restart in the middle, the console and SSH side by side.
Each step checks what the person sees, in the web console's page, against
the machine's own answer, and a journey reuses the web console tests'
helpers (`lab/tests/web-console/console.mjs`) and the tests' scripts
rather than copying them. The `journeys` preset runs all four.

A test of what only some machines have, such as Azure's, Hyper-V's or
VMware's devices, starts with a `run` step that checks the machine is of
that kind, and `or_skip` gives the reason the test is skipped on any
other machine rather than failed. The firmware's names for the machine
are in `/kernel/class/dmi/id`, and `lspci -n -d <vendor>:<device>` finds
a device:

```toml
[[step]]
run = "cat /kernel/class/dmi/id/sys_vendor"
expect = "VMware"
or_skip = "needs a VMware machine"
```

Each account has its own `/tmp` (ADR 0255), so a file a `run` step
writes there as `lab` is not in the `/tmp` of a later step with
`root = true`: a test that hands a file from one to the other writes it
as the account that reads it, or under the lab account's home.

## The screen and the installer

`screen = "text"` waits until the screen shows the text, and
`screen_gone` until it no longer does. The harness reads the console's
text from screenshots with no OCR: consoled says on the serial console
where its cells are, and each cell is looked up in the console's own
fonts in `kernel/lib/fbcon/fonts`, which `./ob lab` passes. `type`
types on a US keyboard, with `\n` for Enter and `{password}` for the
lab password, which is never shown or kept; `point = [x, y]` and
`click = "left"` work a USB tablet, which `/devices/mouse` reads. The
tests in `lab/tests/console` sign in at the console this way:

```toml
[[step]]
type = "lab\n"
[[step]]
screen = "Password:"
[[step]]
type = "{password}\n"
[[step]]
serial = "authd: lab signed in on pts/"
[[step]]
type = "echo \"in\"\": $(whoami)\"\n"
[[step]]
screen = "in: lab"
```

Splitting the label in two means only the command's output, not the
command line that typed it, holds it whole. A test that signs in signs
out at its end, so the next test finds the sign-in a fresh boot leaves.

A test with `boot = "stick"` boots the install stick, which `./ob lab`
builds beside the lab image, with a blank disk (`disk = "8G"` by
default). `install/erase-disk` answers every installer screen by keys,
pulls the stick, restarts from the Done page and signs in on the
installed system with the account the screens made. A test with `boot =
"dvd"` boots the install DVD, which `./ob lab` builds too, from the
machine's optical drive in the same way; `install/from-dvd` installs
from it, takes the disc out with `disc = "eject"` and restarts from the
disk.

The update tests stage signed test updates of the same tree, which `./ob
lab` builds in `out/lab/updates` and names to the lab in
`DUKOVO_LAB_UPDATES`: `good`, a newer version of the lab image's system,
and `broken`, whose log service cannot start. A host step copies one
onto the machine over SSH (`lab/tests/updates/copy-update.sh`), and the
test stages it with `sysupdate stage`. Without them, as with an image
given by `--image`, those tests are skipped and say why.

`updates/server-on-dukovo` moves an update server to the machine, as the
update server move runbook does, over the lab network and HTTP: the lab
PC serves the test update server of `out/lab/updates` first, and the
machine reads it once; the machine is set up as an update server with the
files `./ob lab` stages in `DUKOVO_LAB_SERVER_FILES`; and the lab PC
deploys the test update server's repository to it with `./ob release
send`, whose `$OB_UPDATE_SSH_OPTIONS` reach the machine at its port with
the lab's key, and reads it back with `./ob release check --root`. The
machine then takes the new server's metadata with no refusal for a
version lower than the old server's. No ACME authority is in the lab, so
the certificate steps of the runbook are not tried.

The service bundles' tests install bundles of the catalog that `./ob
lab` packs from the same tree, and names to the lab in
`DUKOVO_LAB_BUNDLES`. A host step copies one onto the machine over SSH
(`lab/tests/services/copy-bundle.sh`), and the test installs it for
everyone: `services/nginx` has nginx serve the lab PC over HTTP and
HTTPS, and reload its settings while the lab PC keeps requests coming,
`services/valkey` has the lab PC store 40,000 keys in Valkey, with
a client of its own in Python, and find them all after a restart, and
`services/python` runs CPython's whole test suite, leaving out only the
tests `lab/tests/services/python-skips.txt` lists, each with the reason
it fails on Dukovo, which `lab/tests/services/copy-file.sh` copies over
too, and `services/php` does the same with PHP's, and its
`php-skips.txt`. `services/php-fpm` has nginx pass the pages of a PHP site
to PHP-FPM, and reload PHP-FPM's settings while the lab PC keeps requests
coming. `services/postgresql` runs PostgreSQL's own suites on a server the
lab account starts by hand, leaving out the tests `postgresql-skips.txt`
lists, then has its service stop with a session open, runs PHP's tests of
`pgsql` and `pdo_pgsql` against it, and loads it with `pgbench`.
`services/sqlite` runs the SQLite shell and the Rust program that links
the library, and `services/sqlite-suite` runs SQLite's own test suite on
the machine, leaving out the test files
`lab/tests/services/sqlite-skips.txt` lists; the suite's program and
scripts are not a bundle but a file the sqlite port builds for the lab,
which `./ob lab` names, with the other ports' such files, in
`DUKOVO_LAB_PORTS` (the folder of the ports' builds, `out/ports`) and
`lab/tests/services/copy-port-file.sh` copies over. From the nginx and
Python bundles `./ob lab` also signs a catalog with throwaway keys, in
`out/lab-catalog`, which it names in `DUKOVO_LAB_CATALOG` for the web
console's test of adding an app (`lab/tests/web-console/add-app-catalog.sh`
copies it over). With an image given by `--image`, `./ob lab` packs no
bundles but names the ones this tree packed already, if any. Without the
bundles or the ports' builds those tests are skipped too.

`processes/linux-view` is of the same kind: it installs the Linux base
bundle, which `./ob lab` packs from `ports/linux-base` with the others,
builds `userspace/linux/view.c` on the lab PC with its own `cc` and glibc,
as a program on a Linux machine is built (`lab/tests/processes/linux-view.sh`
copies it over), and runs it from a folder that is not its own, then again
with `--restart`, which ends `linuxd` in the middle of the run.

Removable media go in and out with steps of their own: `usb` plugs and
pulls a USB stick, `card` puts a memory card in a card reader, `disc` a
disc in an optical drive, and `floppy` a floppy disk in a floppy drive,
write-protected with `protected = true`. A test with `card`, `disc` or
`floppy` steps boots a machine of its own with the drive, empty, so that
no other test finds it. The tests in `lab/tests/storage` use them. A
`disc` step whose image is a cue sheet puts a music CD in: QEMU's own
drives hold none, so under QEMU it goes in an emulated USB CD drive that
`./ob disc-drive` serves, and on a PC the person running the test burns
the cue sheet to a CD-R with its CD-Text.

## Performance tests

The tests in `lab/tests/performance` hold Dukovo to Linux on the same
machine. A `bench` step runs one of the test image's benchmark programs
over SSH and names the rows it holds to Linux's:

```toml
about = "fork and exec cost no more than on Linux on the same machine"
time = "10m"

[[step]]
bench = "/system/tests/posix/bench"
rows = ["posix_fork_wait", "posix_fork_exec_wait"]
```

Linux's numbers come from the machine's Linux twin, booted once a run
(on a real machine, before the boot of the first test that measures)
from a kit that `./ob lab` builds in `out/lab-linux`: the same Linux
kernel `./ob bench` holds Radix to (ADR 0290) and an initramfs whose
first program is `tools/linux-bench`, running the same POSIX benchmarks
built from the same file. So the lab and CI measure against the same
Linux, and the lab adds the machines CI cannot: each real machine in
the lab, under its own firmware and devices. A row passes at 1.00,
parity, unless the step gives a `limit`. On QEMU without KVM the numbers
are kept but the steps skip, as an emulator's numbers say nothing about
either system. When the kit cannot be built (no network for the kernel,
say), `./ob lab` says so and the performance tests skip.

Both systems run the rows at the machine's base frequency, because a
row measures the clock it ran at as much as the code (ADR 0557). A
performance test chooses Dukovo's `benchmark` profile before its `bench`
step and `balanced` after it (`tune set benchmark`, see
[Performance profiles](performance.md)), which turns the turbo levels
off and holds each CPU at its highest level. The Linux kit's command
line ends with `linux-bench.clock=base`, which has `linux-bench` do the
same as the twin's process 1, before any row, and print what it found
and did:

```
linux-bench: CPU frequency: intel_pstate (active), powersave, energy preference balance_performance, 800 to 3600 MHz of 800 to 3600, base 3200 MHz, turbo on
linux-bench: idle: intel_idle, menu: POLL C1 C1E C3 C6 C7s C8 C9 C10
linux-bench: CPU frequency held at 3200 MHz on 4 CPUs (turbo off, performance)
```

The first line is the twin's clock policy as Linux started it, the
second its idle states, and the third what it held the CPUs at, or
`linux-bench: CPU frequency not held:` and the reason. The POSIX
benchmarks time a chain of additions three times, which gives the clock:
`cpu_cycle_cold` right after a pause, `cpu_cycle` after the CPU has run
for 250 ms, and `cpu_cycle_end` as the last row of the run. The lab
puts both systems' `cpu_cycle` clocks above each table, and when they
differ by more than 3% it reports an error and not a comparison of the
rows. A step's note ends with every `BENCH` line Dukovo printed, so the
run's bundle keeps the three clock rows.

### Server rows

`performance/nginx`, `php`, `valkey` and `postgresql` hold the servers of
the catalog to the same servers on Linux (ADR 0546). Each installs its
bundles, puts the machine in the benchmark profile, and runs
`/system/tests/posix/serverbench` with the server's name. The program starts
one worker of the server on the machine, drives it itself over the loopback
or a Unix socket with its data in `/tmp`, and prints each row after a
warm-up pass and three measured passes, of which the lab keeps the fastest.
The rows are one operation each:

| Row | One operation |
| --- | --- |
| `server_nginx_small` | a GET of a 1 KiB file, 8 keep-alive connections |
| `server_nginx_connect` | the same GET on a new connection that nginx closes, 8 at a time |
| `server_nginx_large_mib` | one MiB of a 1 MiB file, 8 keep-alive connections |
| `server_php_fpm` | a GET of a PHP page through nginx and PHP-FPM on a Unix socket |
| `server_php_compute` | one pass of a loop of sorting, strings, regular expressions, hashes and JSON, timed inside PHP |
| `server_valkey_set`, `_get`, `_lpush` | a command of `valkey-benchmark`, 8 connections, 64-byte values, 100000 keys |
| `server_valkey_get_pipelined` | a GET with 16 in a pipeline |
| `server_postgresql_select` | a select-only pgbench transaction, 8 clients on 2 threads |
| `server_postgresql_tpcb` | a TPC-B-like pgbench transaction, 8 clients on 2 threads |

The Linux kit builds the same release and commit of each server that the
port does, with the host's compiler at `-O2`, without the port's patches,
which adapt a program to Dukovo, and with only what the benchmarks use. Each
is built with the prefix of its bundle, so that the paths are the same on
both systems, and is linked to the host's C library, which the kit carries
with it. nginx waits with `poll` on Dukovo and `epoll` on Linux, as each
does on its own system. A server that cannot be built for Linux leaves its
rows out, `./ob lab` says why when it builds the kit, and the test fails and
names the missing row. `./ob lab run --preset server` runs these with the
rest of what a server release is held to.

## The week-long soak

`stress/server-week` keeps a small web shop up for a week on one machine
(ADR 0546): nginx, PHP-FPM, PostgreSQL and Valkey from the catalog, set up
with `website add --kind php`, `database add --site`, `firewall` and
`backup`, and a PHP page that reads items through Valkey from PostgreSQL,
lists the newest writes and writes. The lab PC sends it 20 requests a
second, and signs in over SSH and to the web console's API every five
minutes. The machine checks itself 24 times a day, and between the days it
has an update and its rollback, two power cuts and its cable out for two
minutes, after which every write the page had acknowledged must be in
PostgreSQL. The week is held when no service went down, the memory in use
and the processes stayed flat day by day (the rule of `ob soak`), and day
7's 99th percentile of the page's own time is within 20% of day 1's.

The test has `only_by_name = true`, so `all`, a category or the `server`
preset never starts it. Name it:

```
./ob lab run stress/server-week
```

It holds the one machine it runs on for about seven and a half days, and no
other machine: the lab PC stays free for other runs on other machines. The
page times its own work and the verdict reads only that, so a busy lab PC
does not move it; the round trip the lab PC measures is shown beside it and
never judged. A restart of the lab PC ends the run.

To follow it, read `/tmp/lab-server-week/minutes.csv` on the lab PC, which
gets a line a minute (the minute, the requests, the errors, the writes
acknowledged, the sign-ins tried and failed, and two histograms of the
latencies) and stays there after the run until the next one starts, and
the test's log in the run's logs folder, which is written
after each step with a last line that says `Still running` and when. A day
prints its 24 checks, and the log keeps them. On a real machine, run with
`--with-person` for a person to pull the cable between days 4 and 5; without
one the step is left out and the log says so. A QEMU run of the test itself
uses a copy whose days are shorter: every script takes its day's length in
minutes.

## Security tests

The tests in `lab/tests/security` probe the machine the way someone on
the same network would. `./ob lab attack` runs them (it is a name for
`./ob lab run security`). A test with an `attack` step gets a security
range: the machine boots with a second network port on a private network
only it and an *attacker* share, reachable from nowhere else, and the
runner stands up the attacker on it — under QEMU, a second Dukovo guest
started for the test. The step runs its command on the attacker, and
`{target}` becomes the machine's address on that network:

```toml
about = "To the private network, the machine answers on SSH and nothing else probed"
time = "6m"
fresh = true

[[step]]
attack = "nc -z -w3 {target} 22"
[[step]]
attack = "nc -z -w3 {target} 23"
exit = 1
```

An attack test always boots afresh and its boot is not shared, so the
private port never shows up in another test. The first `attack` step
gives each side its address, as root, so the lab needs its password (as
a root step does). The first scans check that only SSH answers, of the
ports services usually offer, and that nothing but sshd listens for
other machines; that SSH turns away a password sign-in; that its key
exchange, host key and cipher are modern, and the server offers nothing
with SHA-1, CBC or MD5; and that five wrong passwords from the attacker
block its address from SSH, which `firewall blocked` shows, while
`firewall unblock` lifts the block and `firewall never-block` keeps it
from being made. The lab's `docs/test-files.md` describes the `attack`
step in full.

## Tests with the lab PC as a peer

File sharing, WireGuard, Kerberos, syslog and Prometheus need a computer
at the other end. A `host` step runs its command on the lab PC itself, in a
folder of the test's own, so a test stands that end up with the lab
PC's own programs and checks Dukovo against them both ways: Dukovo's
`smb` and `nfs` against the lab PC's Samba and NFS-Ganesha, the lab
PC's `smbclient` and kernel NFS client against Dukovo's `smbd` and
`nfsd`, `wgd` against the lab PC's WireGuard, `logforward` against the lab
PC's rsyslog over TLS, `metricsd`'s Prometheus endpoint against the lab
PC's `promtool`, and `kinit` against the lab PC's MIT KDC. A server started with `background = true` runs until the test
ends, which stops it. `{peer}` is the lab PC's address as the machine
sees it, `{port:NAME}` a free port of the lab PC's for one of the test's
servers, and `{machine}` with `{machine-port:N}` where the lab PC
reaches the machine's port N (under QEMU, a port it forwards):

```toml
host_needs = ["smbd", "pdbedit", "nc"]

[[step]]
host = "sh {here}/samba.sh {port:smb}"
background = true
ready = "samba ready"
[[step]]
run = "smb ls -o user=alice,password=alice-share-pass //{peer}:{port:smb}/share"
expect = "from-lab-pc.txt"
```

The ACME test (`services/acme`) has the lab PC run Pebble, Let's Encrypt's
own test authority, with its DNS server. `lab/pebble/kit.sh` builds the
two programs from one release of Pebble, named in the script with the
SHA-256 of the source it takes from the Go module proxy, with the lab PC's
Go; it keeps them under `~/.cache/dukovo-lab` (or `DUKOVO_LAB_PEBBLE`) and
skips the test where it cannot fetch or build them. The test points the
machine's resolver at that DNS server, so the made-up names it asks
certificates for resolve, and has the machine trust an authority of the
test's own for Pebble's address.

The scripts that set the lab PC's side up sit beside the tests that use
them, as `lab/tests/sharing/samba.sh`, and keep everything in the
test's folder: they never change the lab PC's own settings or
services. A lab PC without a program a test names in `host_needs` skips
the test, and a host command that exits with 77 skips it for a lab PC
that cannot do its part, such as one whose kernel has no NFS client.
The NFS and WireGuard tests run their servers, mounts and tunnels as
root on the lab PC: as the lab is, or through `sudo` with no password,
else they are skipped. Plain `./ob lab` names the packages these tests
want and the lab PC lacks. The lab's `docs/test-files.md` describes
`host` steps in full.

The lab PC also listens. `sound/tone` plays `audio test` on a machine
whose headphone or line out jack is cabled to the lab PC's line in, and
the lab PC records it with `arecord` and checks that 440 Hz came out on
the left alone and 660 Hz on the right alone. Which machines have the
cable is the lab's to say, one machine a line in
`~/.config/dukovo-lab/line-in`: its address and the ALSA device that
records it.

```
192.168.50.21 hw:1,0
```

A machine with no line there, and every QEMU guest, skips the test.

## Web console tests

The web console is used in browsers, so its tests are too. A `browser`
step starts a script of the test's own, a `.mjs` file beside the test,
on the lab PC in Chromium, Firefox and WebKit in turn, and the script
opens the machine's page over HTTPS and uses it as a person does: it
finds a heading or a button by its role and its text, clicks, waits for
what the page should then show, and runs axe-core, which fails the test
on anything that breaks WCAG 2.2 levels A and AA. The tests are in
`lab/tests/web-console`
([ADR 0510](https://github.com/dukovo-os/dukovo/blob/main/docs/adr/0510-the-web-console-s-lab-tests-drive-real-browsers.md)):

```toml
[[step]]
browser = "overview.mjs"
engines = ["chromium", "firefox"]
```

The engines and the checker are a kit that `./ob lab` makes in
`out/lab-browsers` from `lab/browsers`, which pins Playwright and
axe-core, and passes to `dukovo-lab` as `--browsers`. The first time it
needs the network, for the packages and the three engines, and Node.js
22 or later on the lab PC. On Linux Mint the engines also need system
libraries, which one command installs, once:

```sh
sudo npx --prefix out/lab-browsers playwright install-deps
```

A test that cannot run in an engine does not fail: the engine is
skipped, and the step's note and the test's log say which and why, as
`skipped firefox (firefox is not in the browser kit)`. A PC with no
Node.js, and a run without a kit, skip the whole step. The cloud build
machines carry Chromium alone, so their runs skip the other two.

A page that fails keeps what shows it in the run's bundle: a screenshot
of each page the test had open, `<category>-<test>-<engine>-failed-1.png`,
among the screenshots the runner page shows, and a Playwright trace for
each, `<category>-<test>-<engine>-trace-1.zip`, in the logs. Open one
with `npx --prefix out/lab-browsers playwright show-trace`. The lab's
[test file reference](https://github.com/dukovo-os/lab/blob/main/docs/test-files.md)
has a chapter, "Browser tests", on how to write a script.

## Coverage

Each command, service, page of the web console and route of its API
that is new since Dukovo 0.15 has lab tests that cover it before it
counts as done. `lab/coverage.toml` lists them, each with the tests that
cover it, by category and name:

```toml
[commands]
valkey-cli = ["services/valkey"]

[services]
valkey = ["services/valkey"]

[pages]
"/services" = ["console/services"]

[routes]
"GET /services" = ["admind/services"]
```

`./ob lab coverage` prints the table and how much of it has tests, and
`./ob lab coverage --check`, which CI runs on every change, fails on
anything that falls short:

- a command or service the tree has, that `lab/before-phase-16.txt`
  does not list, with no entry. The tree's commands are the manual pages
  of sections 1 and 8 and the catalog bundles' commands, and its services
  the system's service files and the bundles' services, so a new one
  shows up by itself;
- an entry with no tests, or naming a test that is not in `lab/tests`;
- a test that never names what it covers: each word of the name, as a
  whole word, in the test's file, in a script beside it that the test
  runs as `{here}/NAME`, or in the script of a `browser` step;
- an entry of a command or service the tree no longer has;
- an action of the console's API with a test missing, which the next
  section says.

The console's pages and the API's routes have no list in the tree to
compare with, so their entries are held to their tests alone, and each
change that adds one adds its entry. A subcommand that does something of
its own, as `service explain` does, gets an entry of its own as well.
`lab/before-phase-16.txt` holds what there was before; nothing is added
to it.

### The console's actions

Each action of the console's API (ADR 0515) has five tests, which
`lab/actions.toml` names. The registry in `systools` writes the file, so
an action is in it as soon as it is registered: its host test
`actions_toml` fails when the file is stale, and `UPDATE_ACTIONS=1 ./ob
test unit` rewrites it. An action's tests are:

- `checks`, the Rust module of host tests that holds its examples, as
  `admind::actions::service::tests`: a module path under
  `userspace/programs/systools/src` that declares `mod tests` with a
  `#[test]` in it;
- `route`, a boot test case, which calls the action through its route;
- `same_as_command`, a boot test case, which shows that the action
  does what its command does at a terminal;
- `browser`, a lab test, as `web-console/actions`, which runs the action
  from a button in the console;
- `ansible`, a lab test, which runs the Ansible task that the console
  shows for the action.

`./ob lab coverage --check` reads the file and fails for each test that is
not in the tree, once, naming the first three actions that name it and
how many more do. Boot test cases are looked for by name, and lab tests
by category and name in `lab/tests`.

### The console's scans

Three lab tests in `lab/tests/web-console` scan the console's HTTPS port
instead of using it as a person does. `zap` and `testssl` fail on a finding
that their accepted list does not name, and each name in the list has its
reason on the same line; `fuzz` fails on any answer that the console's own
description of its API does not give.

- `zap` runs ZAP's baseline scan, which only watches, against the console
  signed out and against its API with an API token for every scope.
  `zap-rules.tsv` lists the rules that are no fault, as ZAP's own
  configuration file does. ZAP is pinned to a release by the digest of its
  image, which the lab PC pulls through Docker the first time.
- `testssl` runs testssl.sh, pinned to a release tag, and checks that the
  console offers TLS 1.2 and 1.3 only, forward secrecy only and no weak
  cipher, and that its certificate names `localhost` and is valid now. It
  also fails on any finding of severity MEDIUM or worse that
  `testssl-accepted.txt` does not list; the certificate being self-signed
  is listed.
- `fuzz` sends the requests that Schemathesis makes from the console's
  OpenAPI document, `GET /api/1/openapi.json`, and from nothing else,
  signed out, with a token that only reads, and with a token of every scope
  on the examples of the changes alone, each followed by its undo. No
  answer may be a 5xx, one the document does not list, or slower than 2
  seconds (10 for a change), and afterwards admind must answer and not have
  started again, the journal must hold the test's own changes and nothing
  else, and the service must be as it was found. Schemathesis is pinned by
  its release and the hash of every package it needs, in
  `fuzz-requirements.txt`, and installed with `pip install
  --require-hashes` into a virtual environment of the lab PC's cache; the
  test is skipped on a lab PC that has no Python 3.11 or later or cannot
  reach the package index. A failure names a seed, which `LAB_FUZZ_SEED=N`
  sends again.

The console's actions have a third kind of test beside these: `ansible`
runs the Ansible task that the console shows for each action, against the
machine over SSH, twice in a row, with a token the test makes. It uses the
ansible-core and the collection `dukovo.system` that `system/ansible`
pins, which the lab sets up in its cache. The services' changes are tasks
of the collection's module, so a second run reports no change (a restart
always does something); a change that is a command, as the console's
reach is, reports "changed" on every run, because `ansible.builtin.command`
cannot tell a change from none. Before each change and after it, the test
asks the console's check mode, `POST /api/1/check`, whether the change
would do anything, and it must agree with what the module reported.

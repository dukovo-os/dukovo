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
it to the newest, makes the lab's key, clones the private
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
./ob lab run drivers/link-pull --machine qemu,qemu-bios
./ob lab run --preset quick         # the presets are in lab/tests/presets.toml
./ob lab ui                         # the runner page, in a browser on this PC
./ob lab findings                   # what failed, each failure with the runs it came back in
./ob lab send --all                 # new findings and their runs, to lab-results
./ob lab schedule                   # the schedules: when each runs next, how it went last
./ob lab schedule save nightly --preset quick --at 01:00   # every night at 1:00 AM, Mountain Time
```

`./ob lab run` and `./ob lab ui` build the lab image,
`out/dukovo-0.13.0-x86_64-lab.img`, first; `--no-build` tests the one
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

`./ob image --lab` is an installed system's disk with three things more,
all outside the system's slot that a release is made from:

- an administrator's account, `lab`, that signs in only over SSH with
  the lab's key, since no one knows its password;
- sshd turned on;
- a sign-in banner that says it is a test image.

The lab's private key stays on the lab computer: the image holds only
the public half. Never give a lab image to anyone as a release.

## Writing a test

A test is a file at `lab/tests/<category>/<name>.toml`. The categories
are `boot`, `drivers`, `features`, `performance`, `stress`, `security`,
`console`, `web` and `desktop`. This one pulls the network cable:

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

`run` steps are commands over SSH as the `lab` account; `serial` waits
for text on the serial console; `power`, `link`, `usb`, `keys` and
`screenshot` work the machine. The harness's
[test file reference](https://github.com/dukovo-os/lab/blob/main/docs/test-files.md)
lists every key, and `./ob lab show <test>` shows which machines can run
a test. A new file needs no other change.

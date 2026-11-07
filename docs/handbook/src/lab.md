<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# Lab tests

The boot tests run in QEMU on every change. The Dukovo Lab's tests go
further: they pull network cables, cut power, plug in USB sticks and run
on real machines as well as in QEMU. Each test is a small TOML file in
`lab/tests`, and `dukovo-lab`, the lab's harness, runs them
([ADR 0400](https://github.com/dukovo-os/dukovo/blob/main/docs/adr/0400-the-lab-tests-a-lab-image-over-ssh-with-dukovo-lab.md)).

## Running them

Install the harness and make the lab's key once:

```sh
cargo install --locked --git https://github.com/dukovo-os/lab
dukovo-lab key
```

Then, from this repository:

```sh
./ob lab list          # every test
./ob lab run --all     # build the lab image and run every test on QEMU
./ob lab run boot/     # one category
./ob lab run drivers/link-pull --machine qemu,qemu-bios
```

`./ob lab run` builds the lab image, `out/dukovo-0.13.0-x86_64-lab.img`,
first; `--no-build` tests the one already there. Each run leaves a
bundle with the results, every machine's serial log and the screenshots
of failures under `~/.local/share/dukovo-lab/runs`. Without KVM, QEMU is
some four times slower, and the time limits stretch to match.

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
serial = "link is up again"
```

`run` steps are commands over SSH as the `lab` account; `serial` waits
for text on the serial console; `power`, `link`, `usb`, `keys` and
`screenshot` work the machine. The harness's
[test file reference](https://github.com/dukovo-os/lab/blob/main/docs/test-files.md)
lists every key, and `./ob lab show <test>` shows which machines can run
a test. A new file needs no other change.

<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0724. The desktop's rows are measured beside labwc, from the host

- Status: Accepted
- Date: 2026-11-28

## Context

Everything the system does on a short path has a row beside Linux at
the same work (ADR 0039), the display's and the input devices' calls
among them (ADR 0709). The desktop had none. With window management
(ADR 0721) it does the work a person feels all day: a frame of a
window drawn, a key handed to a program, a window dragged, a window
resized.

Two things make these rows unlike the others.

**There is no one call to time.** A frame is the program's drawing,
the message to windowd, windowd's composing of the screen, the kernel's
display path and the card's driver, in three processes and the kernel.
A count kept in any one of them leaves out the rest, and Linux divides
the same work at other places, so two counts kept inside would not be
of the same thing. Time on the wall says nothing either: a desktop
draws a frame when the screen takes one, so a frame takes a sixtieth
of a second however little work it was.

**Linux at the same work is a whole desktop.** It has to be one that
does what windowd does: stacks windows, draws their title bars itself,
and draws on the processor, as windowd does until the graphics cards'
drivers render (pixman, ADR 0712). And it has to run in the benchmark's
virtual machine from an initramfs, with no distribution around it.

## Decision

`ob bench --desktop` measures six rows of Dukovo's desktop and of a
Linux desktop, each in a boot of its own on one machine: the
benchmark's, with a virtio graphics card of 1024 by 768 as its one
screen and a virtio keyboard and mouse.

- **The Linux desktop is labwc**, 0.7.1 on wlroots 0.17.1, with
  wlroots' pixman renderer (`WLR_RENDERER=pixman`). labwc is a stacking
  compositor that draws its windows' title bars and borders itself, and
  is small enough to run alone.
- **It is Ubuntu 24.04's build, pinned.** The 83 packages it needs
  are named in `tools/xtask/src/linux_desktop.pins`, each by the
  SHA-256 the archive's index gives it and by the files taken from it.
  They are from the release pocket, whose packages never change.
  `ob bench` fetches them once into `.cache/linux-desktop`, checks
  them, unpacks them without installing them, and puts the files in the
  initramfs beside `linux-bench`. Nothing of them is in the source
  tree or in an image of Dukovo. The kernel is the pinned one of the
  other rows (ADR 0290).
- **`linux-bench --only desktop` is what a distribution would have
  been.** As process 1 it mounts the tmpfs a desktop expects, writes
  udev's database for the keyboard, the mouse and the card, because
  libinput takes no device that udev has not described and no udev
  runs, and starts labwc as root on the console, where libseat opens
  the devices itself.
- **Both run one program**, `window-test cost`, built for each system
  from one source. It shows a window of 640 by 480, says in the log
  which step it is at, and does the step's work; `ob bench` sends the
  keys and the mouse's motions through QEMU.

  | Row | What | Measured by |
  | --- | --- | --- |
  | `desktop_idle` | a second of the desktop with the window showing and nothing moving | the host |
  | `desktop_frame` | one frame of the window drawn anew, of 600 | the host |
  | `desktop_key`, `desktop_key_p99` | a key, from the kernel's driver to the program: the middle one of 400 and the one 99 in 100 beat | the program |
  | `desktop_move` | one motion of the mouse that drags the window, of 200 | the host |
  | `desktop_resize` | one size of the window while the mouse resizes it, the program drawing it anew at each | the host |

- **A row measured by the host is processor time.** It is how long the
  host's processors ran the machine's CPUs during the step (the
  `schedstat` of QEMU's thread for each CPU), less what they run in as
  long with nothing moving, divided by the step's count. That is all
  the work of a frame wherever it is done, the idle loop left out, by
  one clock for both systems. QEMU's other threads are not counted:
  they serve the commands `ob bench` itself sends.
- **A key is timed from the driver's stamp.** windowd tells the program
  the stamp (ADR 0723). labwc does not, so there the program reads the
  same event from the keyboard's event device, which carries the stamp
  Linux's driver gave it, and takes the time from that.
- **The mouse moves the same for both, and slowly.** A motion comes
  every 20 ms, slower than either desktop draws, so that each is drawn
  by itself, in strokes of ten one way and ten back, so that the window
  stays where it was and near its size whatever each desktop's pointer
  curve makes of a motion. The resize row is divided by the sizes the
  program was asked to draw, which each desktop decides.
- **The rows are a look.** They are measured when asked for and in
  CI's benchmark job, once, and reported there; nothing is gated on
  them, and `ob bench` without `--desktop` does not measure them.

## Alternatives

- **Weston**, the reference compositor, also draws with pixman. Its
  windows draw their own title bars, so its move and resize rows would
  time a program's toolkit, not a window manager.
- **sway** is the wlroots compositor most used. It tiles, and a window
  dragged by its title bar is not what it is for.
- **KWin or Mutter**, what most people run. Each needs OpenGL, which
  in this machine is Mesa drawing on the processor, and a session of
  services around it: the rows would measure Mesa and D-Bus.
- **Building labwc as a port.** It would bring wlroots, libinput,
  Pango and Cairo for Linux into the tree to build a measuring stick,
  and would not be the build people run.
- **Counts kept inside each desktop.** windowd could time its own
  composing. labwc would need patching to do the same, and neither
  count has the program's drawing or the kernel's part.
- **All of QEMU's processor time.** It would add QEMU's work for each
  command `ob bench` sends, the same on both sides, to rows where a
  motion is all the work there is.
- **A gate from the first day.** A gate has to allow for how far a
  runner moves a row from one run to the next, and these rows have no
  runs behind them yet.

## Consequences

- The desktop's cost shows beside a Linux desktop's on every pull
  request, in the benchmark job's summary, and on any machine with
  `./ob bench --desktop`.
- The first run on a machine fetches 28 MB from Ubuntu's archive and
  needs `dpkg-deb`. A run with `--no-linux` needs neither.
- Without KVM the rows measure QEMU's emulation of each system; the
  report says so.
- The rows are by the frame and by the motion, not by the second, so
  the pace at which either desktop draws does not enter them, and the
  idle row is taken while neither draws.
- A gate on these rows, once their run-to-run spread is known, is in
  docs/deferred.md.

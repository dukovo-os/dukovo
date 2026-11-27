<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# Keyboards and mice

Every keyboard, mouse, tablet and other thing you press or move is an
input device. Dukovo reads a computer's built-in PS/2 keyboard, USB
keyboards and mice, and in a virtual machine the virtio keyboard,
mouse and tablet. Touchpads and touch screens come in release 1.3.
The full references are hardware(8), inputd(8), consoled(8),
radix-input(2) and ADRs 0706 to 0708.

## Listing the input devices

```
# hardware inputs
PS/2 keyboard
  Is:        keyboard
  Connected: PS/2
  Has:       104 keys; lights for num lock, caps lock, scroll lock
  File:      /devices/input/0, which no program has open

USB Optical Mouse
  Is:        pointer
  Connected: USB, 046d:c077, at usb-1-2/0, read 125 times a second
  Has:       3 buttons; a wheel
  File:      /devices/input/2, which no program has open
```

Each device is listed under the name it gives itself. **Is** says
what it is, worked out from the keys and axes it has: a `keyboard`
has the keys of the 26 letters; `keys` are keys without them, as a
power button or a row of media keys; a `pointer` moves both ways and
has a left button; a `tablet` says where it points; a `touch` device
says where a finger is; and a `switch` is a laptop's lid. A device
can be several of them, and one that is none is `other`.
**Connected** says how, with the maker's number and the device's own,
where it is plugged in, and how many times a second it asks to be
read. **Has** counts its keys and buttons and names its wheels and
lights. **File** is the device's own file.

The command opens no device and reads no key. It asks no service, so
it works with no console running, and it needs no rights. No device's
serial number is read. `hardware inputs --json` prints the same as
one line of JSON, and the web console's Hardware page shows it under
[Input devices](web-console.md#input-devices).

A device that is plugged in and is not in the list has no driver
yet. `hardware devices` shows it with **No driver**, and the web
console's **Report it** sends its vendor and device numbers.

## Who reads what you type

What you type goes to one place: the program that has the screens.
On the consoles that is the console you are on. A program that draws
the screens itself, as a desktop does, is handed the input devices
when it takes the screens, and reads them only while its picture is
the one in front. When the screens pass to another program, the keys
pass with them at the same moment:

- A program that is not in front reads no key, no motion and no
  click, whatever it has open.
- A key that is held down while the screens pass is let go for the
  program that saw it go down, and the next program sees nothing of
  it, not even its release. Nothing repeats into the wrong window.
- A program that ended reads nothing more.

The devices are handed out by the `inputd` service, which gives them
only to a program that holds the screens. It reads no key itself: it
runs when a device is plugged in or pulled out and when a program
asks for the devices, and sleeps between (inputd(8)). The consoles do
not need it: they read the keyboard themselves, whether inputd runs
or not.

## Ctrl+Alt+Delete

Ctrl+Alt+Delete is the secure key. The kernel itself sees the three
keys, on any keyboard, and no program is ever told of them. Whatever
has the screens, the consoles come to the front with their menu on
the bottom line:

```
Ctrl+Alt+Delete    Enter: sign in    R: restart    P: power off    Esc: back
```

From the moment the third key goes down until you answer, nothing but
the consoles reads the keyboard: a program cannot draw a menu of its
own and collect what you type into it. **Esc** gives the screens back
to the program that had them. **Enter** keeps the consoles in front
and shows a real sign-in; press Ctrl+Alt+Delete and Esc to go back to
the program. **R** restarts the machine and **P** powers it off.

With no program in front of the consoles the key shows their menu
as before, where Enter takes you to a real sign-in (see [Users and
administration](users.md)).

## A wheel in fine steps

A USB mouse whose wheel turns smoothly, or in finer notches than
usual, says so, and Dukovo asks it for its fine steps. Every wheel is
counted in 120ths of a notch: a plain wheel moves 120 at a time, and
a smooth one by as little as it turned, so a program that scrolls by
the pixel follows the wheel exactly.

## Input devices on a virtual machine

A virtual machine's PS/2 keyboard and USB keyboard, mouse and tablet
work as a real computer's do. For the virtio devices, with QEMU:

```
qemu-system-x86_64 ... -device virtio-keyboard-pci -device virtio-tablet-pci
```

A tablet says where the host's pointer is, so the pointer in the
virtual machine's window follows the host's and does not have to be
caught; `virtio-mouse-pci` moves by steps, as a real mouse does.
`hardware inputs` lists them as `QEMU Virtio Keyboard`, `QEMU Virtio
Tablet` and `QEMU Virtio Mouse`, connected as `virtual`.

## When a key or the mouse does nothing

- **A keyboard that types nothing on the consoles.** `hardware
  inputs` from another keyboard, or over SSH, says whether the system
  reads it. If it is listed, `dmesg | grep -i keyboard` as root says
  what the kernel made of it.
- **Nothing reaches a program that draws the screens.** `service
  inputd status` says whether inputd runs, which hands such a program
  its devices.
- **The screen shows a program that takes no key, or one you do not
  trust.** Press Ctrl+Alt+Delete: the consoles come to the front
  whatever the program does.

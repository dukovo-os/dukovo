inputd(8) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

inputd - hand the input devices to the program that has the screens

# SYNOPSIS

*inputd*

# DESCRIPTION

inputd hands every keyboard, mouse and tablet to a program that draws
the screens itself, as a desktop does, and tells it of each device that
is plugged in or pulled out (ADR 0707). Spark runs it as the *inputd*
service. The consoles do not use it: *consoled*(8) reads the keyboard
itself, and shows whether inputd runs or not.

Each input device is a file of its own, _/devices/input/N_, which only
root opens (*radix-input*(2)). inputd opens it for the program, ties
that description to the program's holder of the screens
(*radix-display*(2)), and passes it over a socket. A description that
is tied reads the device only while its holder is in front: the keys
typed into one program's screen never reach another's. For as long as
the secure key, Ctrl+Alt+Delete, has the consoles in front, it reads
nothing.

inputd reads no key and no motion itself. An event goes from the kernel
to the program in front, and wakes nobody else. inputd waits for a
device to come or go and for a program to connect, and does not run
between.

inputd keeps no list of programs it trusts. What a program is handed is
decided by what it holds: it must send a holder of the screens, which
only root, or a program root passed one to, has; and what it is handed
is silent whenever that holder is not in front.

# THE EXCHANGE

A program connects to inputd's socket, _/system/live/inputd/devices_, a
stream socket. Both ends write records of 16 bytes: three numbers of 32
bits each, the least significant byte first, and four bytes that are 0.
The numbers are what the record says, a number that belongs to it, and
with a device what the device is.

The program writes one record, *5*, a holder, whose number is the
version of the exchange, 1. With it (SCM_RIGHTS, *unix*(7)) it sends a
copy of its description of the display layer's file. The copy names the
holder and needs no right but the one to be passed on; inputd asks
nothing of the display layer through it.

inputd answers with:

[[ *Record*
:[ *Number*
:[ *What it says*
|  *1*, a device
:  the N of _/devices/input/N_
:  A device, and with the record a description of its file, tied to
   the program's holder. The third number says what the device is.
|  *2*, gone
:  the device's
:  The device went. Its description reads nothing more.
|  *3*, ready
:  how many devices were sent
:  Every device there was has been sent.
|  *4*, refused
:  why
:  No device is handed out, and inputd closes the connection.

It sends a device for each one there is, then ready; and from then on a
device when one comes and gone when one goes, for as long as the
program keeps the connection. The program writes nothing more. When it
closes the connection, inputd closes its copy of the holder.

inputd refuses with *1* if nothing came with the first record, or what
came is not a holder of the display layer; with *2* if the first record
is not a holder of a version it knows; and with *3* if 16 programs are
connected already. A program has 2 seconds to send its first record,
and one that does not read what it is sent is disconnected.

What a device is, is a sum of:

[[ *Bit*
:[ *The device*
|  *1*
:  a keyboard: it has the keys of the 26 letters
|  *2*
:  keys, without the letters: a power button, media keys, a remote
|  *4*
:  a pointer: it moves both ways and has a left button
|  *8*
:  a tablet: it says where it points, with a pen or a left button
|  *16*
:  a touch device: it says where a finger is
|  *32*
:  a switch: a laptop's lid

and 0 for a device that is none of them. *hardware inputs* shows the
same of every device (*hardware*(8)). The program asks the description
itself for the rest: the device's name, where it is plugged in, its
keys and axes (*radix-input*(2)).

# FILES

_/system/live/inputd/devices_
	The socket inputd answers on, which root connects to.

_/devices/input/control_
	Where inputd learns which devices there are, and of each that
	comes or goes.

_/system/settings/spark/services/inputd.toml_
	The service.

# EXIT STATUS

As a service, inputd does not exit but when it is stopped, with 0; it
exits with 1 if it cannot start, and with 2 if it is given an argument
it does not know.

# SEE ALSO

*radix-input*(2), *radix-display*(2), *consoled*(8), *hardware*(8),
*spark*(8)

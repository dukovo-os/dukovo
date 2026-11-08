uart(4) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

uart - serial ports on PCI cards and Intel AMT's Serial-over-LAN port

# DESCRIPTION

Serial ports that show themselves on the PCI bus as 16550-compatible
serial controllers: the ports of PCI Express serial cards built on the
ASIX AX99100, one PCI function each; the Serial-over-LAN port of Intel's
Active Management Technology (AMT) on vPro machines, which the
management engine carries over the network to a management console;
QEMU's *pci-serial*, *pci-serial-2x* and *pci-serial-4x*; and the first
port of any other card whose PCI class says it is a 16550, 16650,
16750, 16850 or 16950. Only ports in I/O space are used. COM1, the
serial port of a machine's board, is the kernel's own and not these:
programs open it as _/devices/ttyS0_ (*serial-console*(8)).

The kernel finds the ports on the PCI bus as it starts, checks that each
answers, and sets it to 115200 baud, 8 data bits, no parity and one
stop bit, with no flow control. The kernel log names them:

```
Serial: ASIX AX99100 serial port at 0000:03:00.0, I/O port 0xe010: 115200 baud, 8N1; the kernel log and kdb are on it
```

The ports carry what COM1 carries: the kernel log from the start of
boot, a panic report, and the kernel debugger, *kdb*, which reads what
is typed on any of them (*radix-debug*(7)). A kernel thread, *serial*,
sends the log to every port as the ports take it, from a ring of the
last 64 KiB. A port that takes nothing for 100 ms, such as AMT's while
no management console has a session open, is skipped until it does.
Programs cannot open these ports yet.

To read a port from another computer, connect a null-modem cable, or a
USB serial adapter, to it and open it at 115200 baud, for example with
*screen /dev/ttyUSB0 115200* on Linux. AMT's port is read with a
Serial-over-LAN client, such as MeshCommander or *amtterm*, once AMT is
set up in the machine's management engine menu.

# SEE ALSO

*radix-debug*(7), *lspci*(8), *serial-console*(8)

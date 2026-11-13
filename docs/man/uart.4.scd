uart(4) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

uart - serial ports: COM1 to COM4, PCI serial cards and Intel AMT's
Serial-over-LAN port

# DESCRIPTION

The 16550-compatible serial ports Radix drives, each a terminal named
as Linux names it (ADRs 0318, 0360 and 0480):

- COM1 to COM4, the board's ports, at I/O ports 0x3F8, 0x2F8, 0x3E8
  and 0x2E8, are _/devices/ttyS0_ to _ttyS3_, each if it answers. A
  server's BMC, such as a Dell iDRAC, an HPE iLO or a Supermicro IPMI
  controller, most often puts its Serial-over-LAN on COM2, _ttyS1_.
- Ports that show themselves on the PCI bus as 16550-compatible serial
  controllers follow from _/devices/ttyS4_, in the order the kernel
  finds them: the ports of PCI Express serial cards built on the ASIX
  AX99100, one PCI function each; the Serial-over-LAN port of Intel's
  Active Management Technology (AMT) on vPro machines, which the
  management engine carries over the network to a management console;
  QEMU's *pci-serial*, *pci-serial-2x* and *pci-serial-4x*; and the
  first port of any other card whose PCI class says it is a 16550,
  16650, 16750, 16850 or 16950. Only ports in I/O space are used.

A missing COM port's name is not given to another, so a port has the
same name on Dukovo and on Linux, and a *console=* flag from a server's
handbook names the same port. Up to eleven ports besides COM1 are used.

The kernel checks that each port answers and runs it at 115200 baud, 8
data bits, no parity and one stop bit, with no flow control, or at the
line its *console=ttyS*_n_,_options_ flag gives, from its first use
(*dukovo-boot*(7)). The kernel log names each port as it is found:

```
Serial: COM2 at I/O port 0x2f8 is the terminal /devices/ttyS1, at 115200 baud, 8N1; the kernel log and kdb are on it; typing by ISA interrupt 3
Serial: the ASIX AX99100 serial port at 0000:03:00.0, I/O port 0xe010, is the terminal /devices/ttyS4, at 115200 baud, 8N1; the kernel log and kdb are on it; typing by MSI
```

_/kernel/class/tty_ shows them too (*radix-sysfs*(5)).

# THE KERNEL LOG

The ports carry the kernel log from the start of boot, a panic report,
and the kernel debugger, *kdb*, which reads what is typed on any of
them (*radix-debug*(7)), unless the kernel flag *serial-log=* leaves
them out: *serial-log=ttyS0,ttyS4* keeps the log on COM1 and the first
PCI port alone, and *serial-log=none* off every port. A port with a UPS
or a modem on it is best left out, so that the kernel never sends it
text or reads it as commands.

A kernel thread, *serial*, sends the log to each port at the pace that
port takes it, from a ring of the last 64 KiB, so a slow port never
holds up a fast one, and a port that falls behind skips what the ring
no longer holds. A port that takes nothing for 100 ms, such as AMT's
while no management console has a session open, is skipped until it
takes bytes again, when it picks up at the last screenful, about 2 KiB,
so that a session opened long after boot shows what is happening now.
While a program has a port open, the log stays off it; a panic report
and *kdb* still go out on it.

# TYPING

What is typed on a port reaches its terminal by the port's interrupt
where the kernel has it: ISA interrupt 4 for COM1, ISA interrupt 3,
which COM2 and COM4 share, and MSI for a PCI port whose function has
it. COM3, whose interrupt 4 is COM1's, and PCI ports without MSI are
read every 10 ms while their terminal is open. A port's interrupt is
on only while its terminal is open, and every open port is read every
100 ms besides, should an interrupt be lost.

# CONNECTING

To use a port from another computer, connect a null-modem cable, or a
USB serial adapter, to it and open it at its speed, for example with
*screen /dev/ttyUSB0 115200* on Linux. A BMC's Serial-over-LAN is
opened from its web page or with *ipmitool sol activate*; AMT's with a
Serial-over-LAN client, such as MeshCommander or *amtterm*, once AMT is
set up in the machine's management engine menu. *serial-console*(8)
gives a port a sign-in.

# SEE ALSO

*dukovo-boot*(7), *radix-debug*(7), *radix-sysfs*(5), *lspci*(8),
*serial-console*(8), *stty*(1)

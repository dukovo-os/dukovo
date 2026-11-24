linuxd(8) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

linuxd - answer the calls and files a Linux program asks of what the kernel does not have

# SYNOPSIS

*linuxd serve* [*--again*]

# DESCRIPTION

linuxd is the server beside a Linux program. *linux*(1) starts one for
each program it runs, as the person who ran it, in their own root and with
no view of its own, and ends it when the program ends. Nobody runs it by
hand: it takes what it serves from the descriptors and the slot *linux*
gives it, and fails without them.

The Linux table the kernel serves a Linux program with (*radix-linux*(7))
ends each call in a method of the program's own capabilities. Some of
what Linux programs ask has no such method: the files of _/proc_ and
_/sys_ that describe the program and the machine in Linux's formats, the
netlink sockets that list the network interfaces, and the few calls that
tell a program what system it runs on. linuxd answers those. It holds no
authority the person does not: it reads _/processes_ and _/kernel_ as
them, and a program sees of another process only what the person's own
access shows.

linuxd serves two things.

*The forwarded calls.* The program's slot 57 (*SLOT_LINUX*) holds the
send end of an endpoint, and linuxd's holds the receive end. A call the
kernel's table does not know, a *socket* of a family the kernel does not
have, and a socket call on a descriptor that is no socket of the
kernel's, go there as a message with the call's number and six arguments
(*radix-linux-call*(2)). A few threads of linuxd receive them. Each takes
a capability to the call, which reaches the program's memory and
descriptors for as long as the call lasts, and replies with Linux's
result. A call that would wait is answered as a non-blocking one is, so
that no program holds a thread of linuxd. The calls linuxd answers are:

- *socket* of *AF_NETLINK*, and the socket calls on the sockets it makes
  (*connect*, *bind*, *getsockname*, *getpeername*, *sendto*, *sendmsg*,
  *recvfrom*, *recvmsg*, *shutdown*, *setsockopt* and *getsockopt*);
- *personality*, which answers 0 to a question about the persona and to
  *PER_LINUX*, and EINVAL to any other;
- *socket* of any other family the kernel has not: EAFNOSUPPORT.

Any other number is ENOSYS, as it is for a program with no linuxd.

*The filesystem.* The connection of _/devices/fuse_ that *linux* gave it
is the kernel's link to a filesystem linuxd serves (*radix-directory*(2),
*serve_fuse*). The program's view holds its _proc_ folder at _/proc_ and
its _sys_ folder at _/sys_.

# /proc

_/proc/self_ and _/proc/thread-self_ lead to the asking process, and
each process of yours has a folder with Linux's *cmdline*, *comm*,
*environ* (of the program's own processes), *status*, *stat*, *statm*,
*maps*, *mounts*, *mountinfo*, *cgroup*, *limits*, *oom_score_adj*,
*exe*, *cwd*, *root*, *fd* and *task*. The kernel answers the program's
own links in _/proc_ itself (*radix-linux*(7)), so linuxd's are for other
processes and for what a path met part way through leads to. The top
level has *cpuinfo*, *meminfo*, *stat*, *loadavg*, *uptime*, *version*,
*filesystems*, *mounts*, *swaps*, *net* and the files of *sys*:
_kernel/_ (*ostype*, *osrelease*, *version*, *hostname*, *domainname*,
*pid_max*, *threads-max*, *ngroups_max* and _random/_), _vm/_, _fs/_ and
_net/_. The text is from _/processes_, _/kernel_ and the call's own
process (*radix-proc*(5), *radix-processes*(7)), in Linux's formats, so
that *ps*, *top*, *free* and the language runtimes that read them find
what they look for.

The mounts a program sees in _/proc/mounts_ and _/proc/self/mountinfo_
are the ones *linux* put in its view: _/usr_ as *linux-base*, _/proc_ as
*proc*, _/sys_ as *sysfs*, _/tmp_ as *tmpfs*, and each folder of yours as
*dukovo*. *linux* writes them, in _/proc/mounts_'s format, to a file in
its private folder and gives linuxd its path in *LINUXD_MOUNTS*. Without
the variable there is one line, for _/_.

# /sys

_devices/system/cpu_ (*online*, *possible*, *present*, *kernel_max*, and
each processor's folder with its topology and, where known, its frequency),
_class/net_ (each interface's *address*, *mtu*, *operstate*, *ifindex*,
*flags*, *type* and *carrier*), _fs/cgroup_, a read-only version 2
hierarchy of one group whose *cgroup.controllers* name *cpu memory
pids* and whose limits say *max*, and _kernel/mm/transparent_hugepage/enabled_.

# NETLINK

A netlink socket of the *NETLINK_ROUTE* protocol is a file of linuxd's
filesystem, in a folder no one lists, which linuxd opens and gives to the
program as the descriptor *socket* returns. It answers the dump requests
*RTM_GETLINK*, *RTM_GETADDR* and *RTM_GETROUTE*, from _/kernel/class/net_
and _/processes/net_, which is what *getifaddrs*(3), Go's
*net.Interfaces* and Node's *os.networkInterfaces* read; any other
request gets an *NLMSG_ERROR* of EOPNOTSUPP. *bind* gives the socket the
asking process's number as its port, *recvmsg* takes the queued answer
(*MSG_PEEK*, *MSG_TRUNC* and *MSG_DONTWAIT* work), nothing queued is
EAGAIN, and the socket options *SO_RCVBUF*, *SO_SNDBUF* and the
*NETLINK_\** ones are accepted and remembered. There are no multicast
groups.

# STARTING AND STARTING AGAIN

*linuxd serve* finds the connection on descriptor 3, the filesystem's root
on descriptor 4, and the endpoint in slot 57, and writes its messages to
the standard error, which is the terminal's. It serves until the
filesystem ends, which is when the program's last process has gone and the
kernel lets go of the mount, and exits 0.

*linuxd serve --again* is how *linux* starts it when the one before ended
while the program runs: the connection is the same and its filesystem is
the same, so the session takes it up where it was and does not wait for
the kernel's first message, and the numbers that name each file in it are
the ones the kernel already holds, derived from the file's path and the
process's number and not counted in the order files are met. *linux*
starts it again at most 5 times in a minute (*linux*(1), LINUXD); a call
made while it is not there waits in the endpoint for a thread to receive
it.

# EXIT STATUS

0 when the filesystem ended, 1 when it stopped on an error, which it
writes first, and 2 for a command line that is neither *serve* nor
*serve --again*.

# FILES

_/system/commands/linuxd_
	The program.

_/tmp/.linux-PID/mounts_
	The mounts _/proc/mounts_ shows, while the program runs.

# SEE ALSO

*linux*(1), *radix-linux*(7), *radix-linux-call*(2), *radix-directory*(2),
*radix-proc*(5), *radix-processes*(7)

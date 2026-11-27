#!/usr/bin/env python3
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

"""Writes the filters radix-bpf's tests read, as libseccomp compiles them.

    make.py PODMAN_JSON OUT_DIR

PODMAN_JSON is the default profile of Podman and Buildah, containers/common's
pkg/seccomp/seccomp.json at v0.64.2. The script writes:

- podman-default.bpf: that profile as Podman installs it for a container on
  x86-64 with Podman's default capabilities (CHOWN, DAC_OVERRIDE, FOWNER,
  FSETID, KILL, NET_BIND_SERVICE, SETFCAP, SETGID, SETPCAP, SETUID,
  SYS_CHROOT), with the x86 and x32 sub-architectures it adds;
- allow-list.bpf: a small filter that allows read, write, exit_group,
  getppid and rt_sigreturn, allows ioctl with TCGETS only, kills the
  process on socket, and fails every other call with EPERM.

Each file is the program's instructions, 8 bytes each, as
seccomp_export_bpf(3) writes them. It needs libseccomp 2.5 (Ubuntu's
libseccomp2) and runs on any Linux; the tests pin the files, not the
library.
"""

import ctypes
import errno
import json
import os
import sys

ACT_ALLOW = 0x7FFF0000
ACT_KILL_PROCESS = 0x80000000


def act_errno(number):
    return 0x00050000 | (number & 0xFFFF)


ARCH_X86 = 0x40000003
ARCH_X32 = 0x4000003E
COMPARE = {"SCMP_CMP_NE": 1, "SCMP_CMP_LT": 2, "SCMP_CMP_LE": 3, "SCMP_CMP_EQ": 4,
           "SCMP_CMP_GE": 5, "SCMP_CMP_GT": 6, "SCMP_CMP_MASKED_EQ": 7}
CAPABILITIES = {"CAP_CHOWN", "CAP_DAC_OVERRIDE", "CAP_FOWNER", "CAP_FSETID", "CAP_KILL",
                "CAP_NET_BIND_SERVICE", "CAP_SETFCAP", "CAP_SETGID", "CAP_SETPCAP",
                "CAP_SETUID", "CAP_SYS_CHROOT"}
TCGETS = 0x5401


class Compare(ctypes.Structure):
    _fields_ = [("arg", ctypes.c_uint), ("op", ctypes.c_int),
                ("datum_a", ctypes.c_uint64), ("datum_b", ctypes.c_uint64)]


lib = ctypes.CDLL("libseccomp.so.2")
lib.seccomp_init.restype = ctypes.c_void_p
lib.seccomp_init.argtypes = [ctypes.c_uint32]
lib.seccomp_arch_add.argtypes = [ctypes.c_void_p, ctypes.c_uint32]
lib.seccomp_syscall_resolve_name.argtypes = [ctypes.c_char_p]
lib.seccomp_rule_add_array.argtypes = [ctypes.c_void_p, ctypes.c_uint32, ctypes.c_int,
                                       ctypes.c_uint, ctypes.POINTER(Compare)]
lib.seccomp_export_bpf.argtypes = [ctypes.c_void_p, ctypes.c_int]
lib.seccomp_release.argtypes = [ctypes.c_void_p]


def add(ctx, action, name, compares=()):
    number = lib.seccomp_syscall_resolve_name(name.encode())
    if number < 0:
        return
    array = (Compare * max(len(compares), 1))(*compares)
    status = lib.seccomp_rule_add_array(ctx, action, number, len(compares), array)
    # The profile names one socket rule twice, which libseccomp refuses
    # with EEXIST and Podman takes as added.
    if status not in (0, -errno.EEXIST):
        sys.exit(f"make.py: libseccomp refused {name}: {status}")


def applies(rule):
    includes, excludes = rule.get("includes") or {}, rule.get("excludes") or {}
    if includes.get("arches") and "amd64" not in includes["arches"]:
        return False
    if includes.get("caps") and not set(includes["caps"]) <= CAPABILITIES:
        return False
    if excludes.get("caps") and set(excludes["caps"]) & CAPABILITIES:
        return False
    return True


def export(ctx, path):
    with open(path, "wb") as out:
        if lib.seccomp_export_bpf(ctx, out.fileno()) != 0:
            sys.exit(f"make.py: cannot export {path}")
    lib.seccomp_release(ctx)


def podman(profile, path):
    ctx = lib.seccomp_init(act_errno(profile["defaultErrnoRet"]))
    for arch in (ARCH_X86, ARCH_X32):
        lib.seccomp_arch_add(ctx, arch)
    for rule in profile["syscalls"]:
        if not applies(rule):
            continue
        if rule["action"] == "SCMP_ACT_ALLOW":
            action = ACT_ALLOW
        else:
            action = act_errno(rule.get("errnoRet", 1))
        compares = [Compare(a["index"], COMPARE[a["op"]], a["value"], a.get("valueTwo", 0))
                    for a in rule.get("args") or []]
        for name in rule["names"]:
            add(ctx, action, name, compares)
    export(ctx, path)


def allow_list(path):
    ctx = lib.seccomp_init(act_errno(1))
    for name in ("read", "write", "exit_group", "getppid", "rt_sigreturn"):
        add(ctx, ACT_ALLOW, name)
    add(ctx, ACT_ALLOW, "ioctl", [Compare(1, COMPARE["SCMP_CMP_EQ"], TCGETS, 0)])
    add(ctx, ACT_KILL_PROCESS, "socket")
    export(ctx, path)


def main():
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    with open(sys.argv[1]) as file:
        profile = json.load(file)
    podman(profile, os.path.join(sys.argv[2], "podman-default.bpf"))
    allow_list(os.path.join(sys.argv[2], "allow-list.bpf"))


if __name__ == "__main__":
    main()

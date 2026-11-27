#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# The lab PC's side of the linux-seccomp and linux-jit tests: it builds one
# of the test programs of userspace/posix/linux, which the linux-programs
# boot test runs too, as a static Linux program with the lab PC's own C
# compiler and glibc, as the boot test builds it, and copies it onto the
# machine into the folder /users/lab/linux-static.
#
#   linux-static.sh NAME MACHINE PORT
#
# NAME is the program, `seccomp` or `jit`; MACHINE is the machine's address
# and PORT the lab PC's port that reaches its SSH, as {machine} and
# {machine-port:22} give them. A lab PC without a C compiler, or without
# glibc's static library, skips the test, with 77 (host_needs names `cc`,
# so that the lab says so first).
set -eu

name="$1"
machine="$2"
port="$3"
root=$(cd "$(dirname "$0")/../../.." && pwd)
key="${XDG_CONFIG_HOME:-$HOME/.config}/dukovo-lab/lab_ed25519"

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
if ! command -v cc >/dev/null 2>&1; then
	echo "no cc: the lab PC has no C compiler to build $name with"
	exit 77
fi
if ! cc -std=c11 -D_GNU_SOURCE -O2 -Wall -Wextra -static -I "$root/userspace/posix" \
	-o "$work/$name" "$root/userspace/posix/linux/$name.c" -pthread; then
	echo "no static glibc: the lab PC cannot build $name as a static program (install libc6-dev)"
	exit 77
fi

scp -q -P "$port" -i "$key" \
	-o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o BatchMode=yes \
	"$work/$name" "lab@$machine:linux-static/$name"
echo "copied: $name"

#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# The lab PC's side of the linux-view test: it builds the test's program,
# userspace/linux/view.c, with the lab PC's own C compiler and glibc, as a
# program on a Linux machine is built, and copies it onto the machine into
# the folder /users/lab/linux-view, as `view`.
#
#   linux-view.sh MACHINE PORT
#
# MACHINE is the machine's address and PORT the lab PC's port that reaches
# its SSH, as {machine} and {machine-port:22} give them. A lab PC without a
# C compiler skips the test, with 77 (host_needs names `cc`, so that the
# lab says so first).
set -eu

machine="$1"
port="$2"
root=$(cd "$(dirname "$0")/../../.." && pwd)
key="${XDG_CONFIG_HOME:-$HOME/.config}/dukovo-lab/lab_ed25519"

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
if ! command -v cc >/dev/null 2>&1; then
	echo "no cc: the lab PC has no C compiler to build view with"
	exit 77
fi
cc -std=c11 -D_GNU_SOURCE -O2 -Wall -Wextra -o "$work/view" "$root/userspace/linux/view.c"

scp -q -P "$port" -i "$key" \
	-o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o BatchMode=yes \
	"$work/view" "lab@$machine:linux-view/view"
echo "copied: view"

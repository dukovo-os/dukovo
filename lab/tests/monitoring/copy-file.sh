#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# The lab PC's side of a test that needs a file of the repository on the
# machine: it copies it into the lab account's home, under its own name.
#
#   copy-file.sh FILE MACHINE PORT
#
# MACHINE is the machine's address and PORT the lab PC's port that
# reaches its SSH, as {machine} and {machine-port:22} give them.
set -eu

file="$1"
machine="$2"
port="$3"
key="${XDG_CONFIG_HOME:-$HOME/.config}/dukovo-lab/lab_ed25519"

scp -q -P "$port" -i "$key" \
	-o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o BatchMode=yes \
	"$file" "lab@$machine:"
echo "copied: ${file##*/}"

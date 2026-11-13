#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# The lab PC's side of a test that needs a file one of the ports built for
# the lab: it copies the file onto the machine, into the lab account's home.
#
#   copy-port-file.sh PORT FILE MACHINE SSHPORT
#
# PORT is the port's name, such as sqlite, and FILE a file of its for-lab
# folder, which `ob ports` leaves in out/ports/PORT/for-lab and `ob lab`
# names to the lab in DUKOVO_LAB_PORTS. MACHINE is the machine's address
# and SSHPORT the lab PC's port that reaches its SSH, as {machine} and
# {machine-port:22} give them. The file goes to /users/lab/FILE. Without
# the ports' builds, as when the lab was started with an image of its own,
# the script exits with 77, which skips the test.
set -eu

port="$1"
file="$2"
machine="$3"
ssh_port="$4"

ports="${DUKOVO_LAB_PORTS:-}"
if [ -z "$ports" ] || [ ! -f "$ports/$port/for-lab/$file" ]; then
	echo "no $file from port $port: run the test through ./ob lab, which builds the ports"
	exit 77
fi
key="${XDG_CONFIG_HOME:-$HOME/.config}/dukovo-lab/lab_ed25519"

scp -q -P "$ssh_port" -i "$key" \
	-o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o BatchMode=yes \
	"$ports/$port/for-lab/$file" "lab@$machine:$file"
echo "copied: $file"

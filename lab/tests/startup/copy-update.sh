#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# The lab PC's side of the update tests: it copies a test update onto the
# machine, into the lab account's home, for `sysupdate stage` to read.
#
#   copy-update.sh NAME MACHINE PORT
#
# NAME is one of the test updates `ob lab` builds beside the lab image
# (good or broken: test.rs's TEST_UPDATES), MACHINE the machine's address
# and PORT the lab PC's port that reaches its SSH, as {machine} and
# {machine-port:22} give them. The update goes to /users/lab/update-NAME.
# Without the test updates, as when the lab was started with an image of
# its own, the script exits with 77, which skips the test.
set -eu

name="$1"
machine="$2"
port="$3"

updates="${DUKOVO_LAB_UPDATES:-}"
if [ -z "$updates" ] || [ ! -d "$updates/$name" ]; then
	echo "no test update called $name: run the update tests through ./ob lab, which builds them"
	exit 77
fi
key="${XDG_CONFIG_HOME:-$HOME/.config}/dukovo-lab/lab_ed25519"

scp -r -q -P "$port" -i "$key" \
	-o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o BatchMode=yes \
	"$updates/$name" "lab@$machine:update-$name"
echo "copied: $name"

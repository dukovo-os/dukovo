#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# The lab PC's side of the time tests: it reads the machine's clock over
# SSH as the lab account and says whether it is within two seconds of
# the lab PC's own.
#
#   clock-near.sh MACHINE PORT
#
# MACHINE is the machine's address and PORT the lab PC's port that
# reaches its SSH, as {machine} and {machine-port:22} give them.
set -eu

machine="$1"
port="$2"
key="${XDG_CONFIG_HOME:-$HOME/.config}/dukovo-lab/lab_ed25519"

there=$(ssh -p "$port" -i "$key" -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null \
	-o BatchMode=yes -o LogLevel=ERROR "lab@$machine" "date -u +%s")
here=$(date -u +%s)
apart=$((here - there))
[ "$apart" -lt 0 ] && apart=$((-apart))
if [ "$apart" -gt 2 ]; then
	echo "the machine's clock is $apart seconds from the lab PC's"
	exit 1
fi
echo "the machine's clock is within 2 seconds of the lab PC's"

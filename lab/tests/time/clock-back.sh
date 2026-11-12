#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# The lab PC's side of the time tests: it sets the machine's clock to
# the lab PC's own, in UTC, over SSH as the lab account with the lab
# password on elevate's standard input, and checks the two are within
# two seconds. Elevating takes seconds, so the time sent is late by
# then: a second pass sends it ahead by as much as the first was late.
#
#   clock-back.sh MACHINE PORT
#
# MACHINE is the machine's address and PORT the lab PC's port that
# reaches its SSH, as {machine} and {machine-port:22} give them. The
# lab password comes on standard input, as the step's `input` gives it.
set -eu

read -r password
machine="$1"
port="$2"
key="${XDG_CONFIG_HOME:-$HOME/.config}/dukovo-lab/lab_ed25519"
ssh_to() {
	ssh -p "$port" -i "$key" -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null \
		-o BatchMode=yes -o LogLevel=ERROR "lab@$machine" "$@"
}
# How far the machine's clock is behind the lab PC's, in seconds.
behind() {
	there=$(ssh_to "date -u +%s")
	echo $(($(date -u +%s) - there))
}
set_ahead() {
	when=$(date -u -d "@$(($(date -u +%s) + $1))" '+%Y-%m-%d %H:%M:%S')
	printf '%s\n' "$password" |
		ssh_to "sudo -S -p '' timedatectl set-time '$when'" >/dev/null
}

set_ahead 0
late=$(behind)
set_ahead "$late"
apart=$(behind)
[ "$apart" -lt 0 ] && apart=$((-apart))
if [ "$apart" -gt 2 ]; then
	echo "the machine's clock is $apart seconds from the lab PC's"
	exit 1
fi
echo "set the machine's clock to the lab PC's"

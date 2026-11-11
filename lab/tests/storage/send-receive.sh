#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# The lab PC's side of the send and receive test (ADR 0462): Linux's own
# btrfs reads what the machine's `subvolume send` wrote, and sends a
# stream the other way.
#
#   send-receive.sh fetch MACHINE PORT
#   send-receive.sh receive
#   send-receive.sh send MACHINE PORT
#
# MACHINE is the machine's address and PORT the lab PC's port that
# reaches its SSH, as {machine} and {machine-port:22} give them.
#
# fetch copies lab-1.stream, lab-2.stream and lab-send.sums from the lab
# account's home into streams/ in the test's folder (the working
# directory), with the lab's key.
#
# receive, as root, shows the streams' first commands with `btrfs receive
# --dump`, receives both into a Btrfs the lab PC makes in a file in the
# test's folder and mounts, and checks the received copy of ~/lab-send
# against the machine's own sums. It says "sums match" when every file is
# the same, and unmounts and removes the Btrfs whatever happens.
#
# send copies Linux's own stream of the Btrfs driver's tests (made by
# `ob fixture btrfs-send` with Linux's btrfs send) to the lab account's
# home as linux.stream, for the machine to receive.
set -eu

action=$1
machine=${2:-}
port=${3:-}
here=$(cd "$(dirname "$0")" && pwd)
key="${XDG_CONFIG_HOME:-$HOME/.config}/dukovo-lab/lab_ed25519"
ssh_options="-o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o BatchMode=yes"

case "$action" in
fetch)
	rm -rf streams && mkdir streams
	# shellcheck disable=SC2086
	scp -q -P "$port" -i "$key" $ssh_options \
		"lab@$machine:lab-1.stream" "lab@$machine:lab-2.stream" "lab@$machine:lab-send.sums" streams/
	echo "fetched: $(ls streams | wc -l) files"
	;;
receive)
	if ! grep -qw btrfs /proc/filesystems && ! modprobe -q btrfs 2> /dev/null; then
		echo "the lab PC's kernel has no Btrfs"
		exit 77
	fi
	for stream in streams/lab-1.stream streams/lab-2.stream; do
		btrfs receive --dump -f "$stream" | head -1 | tr -s ' ' | cut -d ' ' -f 1-2
	done
	rm -f received.btrfs && truncate -s 512M received.btrfs
	mkfs.btrfs -q received.btrfs
	mkdir -p received
	mount -o loop received.btrfs received
	trap 'umount received; rm -f received.btrfs' EXIT
	btrfs receive -f streams/lab-1.stream received
	btrfs receive -f streams/lab-2.stream received
	(cd received/lab-send-2/lab/lab-send && sha256sum --quiet -c "$OLDPWD/streams/lab-send.sums")
	btrfs subvolume show received/lab-send-2 | grep -e 'Received UUID' -e Flags
	echo "sums match"
	;;
send)
	stream="$here/../../../kernel/fs/btrfs/src/tests/fixtures/btrfs-send-full.stream"
	# shellcheck disable=SC2086
	scp -q -P "$port" -i "$key" $ssh_options "$stream" "lab@$machine:linux.stream"
	echo "copied: linux.stream"
	;;
*)
	echo "usage: send-receive.sh fetch|send MACHINE PORT, or send-receive.sh receive" >&2
	exit 2
	;;
esac

#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# The lab PC's side of the backup test (ADR 0463): Linux's own age and
# btrfs read what the machine's backup(8) wrote, with the recovery key
# alone, as backup(8)'s RESTORING WITH LINUX says.
#
#   backup.sh fetch MACHINE PORT
#   backup.sh restore
#
# MACHINE is the machine's address and PORT the lab PC's port that
# reaches its SSH, as {machine} and {machine-port:22} give them.
#
# fetch copies the machine's folder of backups (lab-backup), its recovery
# key and lab-backup.sums from the lab account's home into backups/ in
# the test's folder (the working directory), with the lab's key.
#
# restore, as root, decrypts each backup of /users' chain with age, shows
# its first command with `btrfs receive --dump`, receives it into a Btrfs
# the lab PC makes in a file in the test's folder and mounts, and checks
# the newest copy of ~/lab-backup-files against the machine's own sums.
# It says "sums match" when every file is the same, and unmounts and
# removes the Btrfs whatever happens.
set -eu

action=$1
machine=${2:-}
port=${3:-}
key="${XDG_CONFIG_HOME:-$HOME/.config}/dukovo-lab/lab_ed25519"
ssh_options="-o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o BatchMode=yes"

case "$action" in
fetch)
	rm -rf backups && mkdir backups
	# shellcheck disable=SC2086
	scp -q -r -P "$port" -i "$key" $ssh_options \
		"lab@$machine:lab-backup" "lab@$machine:lab-backup.key" "lab@$machine:lab-backup.sums" backups/
	echo "fetched: $(ls backups/lab-backup | wc -l) machine"
	;;
restore)
	if ! grep -qw btrfs /proc/filesystems && ! modprobe -q btrfs 2> /dev/null; then
		echo "the lab PC's kernel has no Btrfs"
		exit 77
	fi
	rm -f received.btrfs && truncate -s 1G received.btrfs
	mkfs.btrfs -q received.btrfs
	mkdir -p received
	mount -o loop received.btrfs received
	trap 'umount received; rm -f received.btrfs' EXIT
	# The newest chain of /users, its backups in the order of their names,
	# each the parts of one file in order.
	chain=$(ls -d backups/lab-backup/*/users/*/ | tail -1)
	for backup in $(ls "$chain" | sed 's/\.age.*//' | sort -u); do
		cat "$chain$backup".age* | age -d -i backups/lab-backup.key > backup.stream
		btrfs receive --dump -f backup.stream | head -1 | tr -s ' ' | cut -d ' ' -f 1-2
		btrfs receive -f backup.stream received
	done
	rm -f backup.stream
	newest=$(ls -d received/backup-* | tail -1)
	(cd "$newest/lab/lab-backup-files" && sha256sum --quiet -c "$OLDPWD/backups/lab-backup.sums")
	btrfs subvolume show "$newest" | grep -e 'Received UUID' -e Flags
	echo "sums match"
	;;
*)
	echo "usage: backup.sh fetch MACHINE PORT, or backup.sh restore" >&2
	exit 2
	;;
esac

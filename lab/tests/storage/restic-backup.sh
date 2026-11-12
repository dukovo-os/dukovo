#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# The lab PC's side of the restic-backup test (ADR 0465): Linux's own
# restic reads the repository the machine's backup(8) wrote, with the
# recovery key as its password, as backup(8)'s RESTORING WITH LINUX says.
#
#   restic-backup.sh fetch MACHINE PORT
#   restic-backup.sh restore
#
# MACHINE is the machine's address and PORT the lab PC's port that
# reaches its SSH, as {machine} and {machine-port:22} give them.
#
# fetch copies the machine's folder (lab-restic), its recovery key and
# lab-restic.sums from the lab account's home into repository/ in the
# test's folder (the working directory), with the lab's key.
#
# restore checks the repository with every pack read through, counts its
# snapshots (one of /users and one of the machine's record), restores the
# newest backup of /users into restored/ and checks ~/lab-restic-files
# there against the machine's own sums. It says "sums match" when every
# file is the same.
set -eu

action=$1
machine=${2:-}
port=${3:-}
key="${XDG_CONFIG_HOME:-$HOME/.config}/dukovo-lab/lab_ed25519"
ssh_options="-o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o BatchMode=yes"

case "$action" in
fetch)
	rm -rf repository && mkdir repository
	# shellcheck disable=SC2086
	scp -q -r -P "$port" -i "$key" $ssh_options \
		"lab@$machine:lab-restic" "lab@$machine:lab-restic.key" "lab@$machine:lab-restic.sums" repository/
	echo "fetched: $(ls repository/lab-restic | wc -l) machine"
	;;
restore)
	RESTIC_PASSWORD=$(cat repository/lab-restic.key)
	RESTIC_REPOSITORY=$(ls -d repository/lab-restic/*/)
	export RESTIC_PASSWORD RESTIC_REPOSITORY
	restic --no-cache check --read-data
	echo "snapshots: $(restic --no-cache snapshots --json | grep -o '"short_id"' | wc -l)"
	rm -rf restored
	restic --no-cache restore latest --path /users --target restored > /dev/null
	(cd restored/users/lab/lab-restic-files && sha256sum --quiet -c "$OLDPWD/repository/lab-restic.sums")
	rm -rf restored
	echo "sums match"
	;;
*)
	echo "usage: restic-backup.sh fetch MACHINE PORT, or restic-backup.sh restore" >&2
	exit 2
	;;
esac

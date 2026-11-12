#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# The lab PC's side of ssh/key-types: it makes a key of each kind with
# the lab PC's ssh-keygen, adds them to the lab account's
# authorized_keys over SSH with the lab key, signs in with each and says
# whether it was let in, and takes them back out.
#
#   key-types.sh MACHINE PORT
#
# MACHINE is the machine's address and PORT the lab PC's port that
# reaches its SSH, as {machine} and {machine-port:22} give them. It runs
# in the test's own folder.
set -eu

machine="$1"
port="$2"
lab_key="${XDG_CONFIG_HOME:-$HOME/.config}/dukovo-lab/lab_ed25519"
options="-p $port -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o BatchMode=yes -o LogLevel=ERROR -o IdentitiesOnly=yes"

kinds="ed25519 ecdsa-256 ecdsa-384 ecdsa-521 rsa-3072"
for kind in $kinds; do
	case "$kind" in
	ed25519) ssh-keygen -q -t ed25519 -N "" -C "lab-key-types-$kind" -f "$kind" ;;
	*) ssh-keygen -q -t "${kind%-*}" -b "${kind#*-}" -N "" -C "lab-key-types-$kind" -f "$kind" ;;
	esac
done
# shellcheck disable=SC2086 # the options are words
cat ./*.pub | ssh $options -i "$lab_key" "lab@$machine" "cat >> .ssh/authorized_keys"
for kind in $kinds; do
	# shellcheck disable=SC2086
	if ssh $options -i "./$kind" "lab@$machine" echo in >/dev/null 2>&1; then
		echo "$kind: signed in"
	else
		echo "$kind: refused"
	fi
done
# shellcheck disable=SC2086
ssh $options -i "$lab_key" "lab@$machine" "sed -i /lab-key-types-/d .ssh/authorized_keys"
echo "keys taken back"

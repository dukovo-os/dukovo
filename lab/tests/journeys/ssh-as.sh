#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# The lab PC's side of a journey's steps as one of the machine's people: it
# signs in over SSH as the account, with the key a journey's first step made
# for it, and runs a command, whose output and exit status are its own.
#
#   ssh-as.sh KEYFOLDER ACCOUNT MACHINE PORT COMMAND
#
# The key is the file ACCOUNT in KEYFOLDER. MACHINE is the machine's address
# and PORT the lab PC's port that reaches its SSH, as {machine} and
# {machine-port:22} give them. COMMAND is one argument, which the account's
# shell runs; a script on the standard input is read by `sh -s`.
set -eu

folder=$1
account=$2
machine=$3
port=$4
command=$5

exec ssh -p "$port" -i "$folder/$account" -o IdentitiesOnly=yes -o BatchMode=yes \
	-o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR \
	"$account@$machine" "$command"

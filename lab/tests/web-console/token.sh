#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# The lab PC's way to an API token of the web console (ADR 0515): the
# tests that ask the console's API from the lab PC (ansible, fuzz and zap)
# make one with `admin token add`, as the lab account, over SSH, and take
# it away again with `admin token remove`.
#
#   token.sh make NAME MACHINE SSHPORT [SCOPE]   prints the token, and nothing else
#   token.sh remove NAME MACHINE SSHPORT
#
# NAME is the token's name, letters, digits and hyphens; MACHINE the
# machine's address and SSHPORT the lab PC's port that reaches its SSH, as
# {machine} and {machine-port:22} give them. SCOPE is the token's scope, `*`
# (all that the lab account may do) when it is not given, or scopes joined
# by commas, as admin(8) says: `services:read,certificates:read`. `admin
# token add` asks for the account's password, which it takes from its
# standard input when that is no terminal, so the lab account's own, which
# the lab PC keeps in ${XDG_CONFIG_HOME:-$HOME/.config}/dukovo-lab/lab_password,
# goes in over the same SSH. The password is not shown or kept. A token left
# from an earlier run, which a test that failed has not taken away, is
# replaced.
#
# Exits with 77 where the lab PC cannot do its part: it has no lab
# password, as when the lab was started with an image of its own. That
# skips the test.
set -eu

what="$1"
name="$2"
machine="$3"
port="$4"
scope="${5:-*}"

config="${XDG_CONFIG_HOME:-$HOME/.config}/dukovo-lab"
key="$config/lab_ed25519"
password="$config/lab_password"

on_machine() {
	ssh -p "$port" -i "$key" \
		-o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o BatchMode=yes \
		-o IdentitiesOnly=yes -o LogLevel=ERROR "lab@$machine" "$@"
}

case "$scope" in
*[!A-Za-z0-9:,*-]*)
	echo "token.sh: the scope $scope has more than letters, digits, : , * and -" >&2
	exit 2
	;;
esac

case "$what" in
make)
	if [ ! -f "$password" ]; then
		echo "no lab password at $password: an API token is made with the lab account's password"
		exit 77
	fi
	on_machine "admin token remove $name" > /dev/null 2>&1 < /dev/null || true
	# `admin token add` prints the token on its standard output and says
	# on its standard error that it is not shown again.
	head -n 1 "$password" | on_machine "admin token add $name --scope '$scope'" 2> /dev/null
	;;
remove)
	on_machine "admin token remove $name" < /dev/null
	;;
*)
	echo "usage: token.sh make NAME MACHINE SSHPORT [SCOPE] | token.sh remove NAME MACHINE SSHPORT" >&2
	exit 2
	;;
esac

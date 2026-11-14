#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# The lab PC's side of services/acme's DNS-01 certificate: what a person
# does at their DNS provider. `cert get --dns` shows the TXT records to
# add; this reads them from the file the machine's command wrote its words
# to, and adds each to the test's DNS server (pebble-challtestsrv's
# management interface), where the machine's resolver and Pebble ask.
#
#   acme-dns.sh MACHINE SSH-PORT CHALLENGE-PORT FILE
#
# FILE is the machine's file, in the lab account's home. MACHINE is the
# machine's address and SSH-PORT the lab PC's port that reaches its SSH, as
# {machine} and {machine-port:22} give them; CHALLENGE-PORT is the DNS
# server's management port on the lab PC. It waits, 60 seconds at most, for
# the file to show the records, and prints each as it adds it.
set -eu

machine=$1
ssh_port=$2
challenge_port=$3
file=$4
key="${XDG_CONFIG_HOME:-$HOME/.config}/dukovo-lab/lab_ed25519"

on_machine() {
	ssh -q -p "$ssh_port" -i "$key" -o StrictHostKeyChecking=no \
		-o UserKnownHostsFile=/dev/null -o BatchMode=yes "lab@$machine" "$@"
}

i=0
text=""
while [ "$i" -lt 60 ]; do
	text=$(on_machine cat "$file" 2> /dev/null || true)
	case "$text" in
	*'TXT  "'*) break ;;
	esac
	i=$((i + 1))
	sleep 1
done
case "$text" in
*'TXT  "'*) ;;
*)
	echo "the machine's command showed no TXT record in 60 seconds; its words:"
	echo "$text"
	exit 1
	;;
esac

added=0
# A line is  _acme-challenge.NAME.  TXT  "VALUE"
echo "$text" | sed -n 's/^ *\(_acme-challenge\.[^ ]*\)  TXT  "\([^"]*\)".*$/\1 \2/p' |
	while read -r host value; do
		curl -sS --noproxy '*' -X POST -d "{\"host\":\"$host\",\"value\":\"$value\"}" \
			"http://127.0.0.1:$challenge_port/set-txt" > /dev/null
		echo "TXT added: $host"
	done
added=$(echo "$text" | grep -c 'TXT  "' || true)
echo "records shown: $added"

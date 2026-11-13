#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# The lab PC's end of the alerts-webhook test: the receiver that stands
# in for ntfy, Slack, Discord, Matrix and a program of the owner's
# (alerts-receiver.py beside this file), taking HTTPS with a certificate
# that an authority of the test's own signed (ADR 0485).
#
#   alerts-hooks.sh serve PORT PEER
#   alerts-hooks.sh wait PHRASE [SECONDS]
#
# serve makes the certificates (alerts-ca.sh) and starts the receiver on
# the lab PC's port PORT, as {port:NAME} gives it, and says "receiver
# ready" once it listens. PEER is the lab PC's address as the machine sees
# it, as {peer} gives it, which the certificate is for. ca.pem is the
# authority's certificate, for the test to copy to the machine, and
# received.log has a line for each request. It runs until the lab ends it.
#
# wait waits up to SECONDS (60 without) for a request whose line has
# PHRASE, shows the lines that have it, and says how many:
#
#   hooks with PHRASE: 5
#
# serve exits with 77 where the lab PC has no python3 or openssl.
set -eu

action=$1

case "$action" in
serve)
	port=$2
	peer=$3
	for program in python3 openssl; do
		if ! command -v "$program" > /dev/null 2>&1; then
			echo "the lab PC has no $program"
			exit 77
		fi
	done
	rm -f received.log
	sh "$(dirname "$0")/alerts-ca.sh" "$peer"
	exec python3 "$(dirname "$0")/alerts-receiver.py" "$port"
	;;
wait)
	phrase=$2
	seconds=${3:-60}
	i=0
	until grep -q -F -- "$phrase" received.log 2> /dev/null; do
		i=$((i + 1))
		[ "$i" -gt "$seconds" ] && break
		sleep 1
	done
	# Others of the same moment arrive a little later.
	sleep 2
	grep -F -- "$phrase" received.log || true
	echo "hooks with $phrase: $(grep -c -F -- "$phrase" received.log || true)"
	;;
*)
	echo "usage: alerts-hooks.sh serve PORT PEER | wait PHRASE [SECONDS]"
	exit 2
	;;
esac

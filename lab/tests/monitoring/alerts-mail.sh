#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# The lab PC's end of the alerts-email and early-warning-disk tests: a
# mail catcher, Mailpit, as an administrator runs one to see what a
# program sends. It takes mail with STARTTLS on port 587, the port alertd
# submits to, with a certificate that an authority of the test's own
# signed (ADR 0485), and keeps what it takes in memory, for the test to ask
# its API about. Everything is in the test's folder, and the lab PC's own
# mail is left alone.
#
#   alerts-mail.sh serve API PEER
#   alerts-mail.sh wait API PHRASE [SECONDS]
#   alerts-mail.sh count API PHRASE
#
# serve starts Mailpit, whose API listens on the lab PC's port API (as
# {port:NAME} gives it) and which signs in nobody and takes every account,
# and says "mailpit ready" once it takes mail. It runs until the lab ends
# it. PEER is the lab PC's address as the machine sees it, as {peer} gives
# it, which the certificate is for; ca.pem is the authority's certificate,
# for the test to copy to the machine.
#
# wait waits up to SECONDS (60 without) for a mail whose subject has
# PHRASE, and count does not wait; both say how many there are:
#
#   mails with PHRASE: 1
#
# serve exits with 77 where the lab PC cannot do its part: no Mailpit,
# openssl or curl, or port 587, which is a privileged one, that it cannot
# listen on as a user that is not root and has no sudo without a password.
set -eu

action=$1

case "$action" in
serve)
	api=$2
	peer=$3
	for program in mailpit openssl curl nc; do
		if ! command -v "$program" > /dev/null 2>&1; then
			echo "the lab PC has no $program"
			exit 77
		fi
	done
	as_root=
	if [ "$(id -u)" != 0 ]; then
		if sudo -n true > /dev/null 2>&1; then
			as_root="sudo -n"
		else
			first=$(cat /proc/sys/net/ipv4/ip_unprivileged_port_start 2> /dev/null || echo 1024)
			if [ "$first" -gt 587 ]; then
				echo "the lab PC cannot listen on port 587: not root, no sudo, and ports from $first up only"
				exit 77
			fi
		fi
	fi
	if nc -z 127.0.0.1 587 2> /dev/null; then
		echo "something on the lab PC already listens on port 587"
		exit 77
	fi

	sh "$(dirname "$0")/alerts-ca.sh" "$peer"
	work=$(pwd)
	# shellcheck disable=SC2086
	$as_root mailpit --smtp 0.0.0.0:587 --listen "127.0.0.1:$api" \
		--smtp-tls-cert "$work/server.pem" --smtp-tls-key "$work/server.key" \
		--smtp-auth-accept-any > mailpit.out 2>&1 &
	pid=$!
	# shellcheck disable=SC2086
	trap 'kill "$pid" 2> /dev/null || $as_root kill "$pid" 2> /dev/null; exit 0' TERM INT

	i=0
	until nc -z 127.0.0.1 587 2> /dev/null && curl -fs "http://127.0.0.1:$api/api/v1/info" > /dev/null 2>&1; do
		i=$((i + 1))
		if [ "$i" -gt 40 ] || ! kill -0 "$pid" 2> /dev/null; then
			cat mailpit.out
			echo "Mailpit does not take mail on port 587"
			exit 1
		fi
		sleep 0.5
	done
	echo "mailpit ready"
	wait "$pid"
	;;
wait | count)
	api=$2
	phrase=$3
	seconds=0
	[ "$action" = wait ] && seconds=${4:-60}
	heard() {
		curl -fs "http://127.0.0.1:$api/api/v1/messages?limit=500" \
			| grep -o '"Subject":"[^"]*"' | grep -c -F -- "$phrase" || true
	}
	i=0
	while [ "$(heard)" -lt 1 ] && [ "$i" -lt "$seconds" ]; do
		i=$((i + 1))
		sleep 1
	done
	echo "mails with $phrase: $(heard)"
	;;
*)
	echo "usage: alerts-mail.sh serve API PEER | wait API PHRASE [SECONDS] | count API PHRASE"
	exit 2
	;;
esac

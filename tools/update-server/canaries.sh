#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# The canaries of the update server (ADR 0533), for `ob release canary`
# and `ob release canaries`. The list is root's, and the receiver and the
# jobs that hold the waves only read it:
#
#   canaries.sh add NAME KEY           a canary, by the key `sysupdate canary
#                                      enrol` prints
#   canaries.sh remove NAME            takes it off the list
#   canaries.sh NAME red|green REASON  a colour by hand, which wins over its
#                                      reports
#   canaries.sh NAME auto              back to its reports
#   canaries.sh show [--json]          each canary: colour, why, last report,
#                                      release; read only
#
# A change is made as root, through sudo, and is seen at once by the jobs
# that watch the list (dukovo-system-canaries.path and
# dukovo-catalog-canaries.path), which sign the waves again if it matters.
# `show` runs as the user dukovo-system, which keeps the plan: the person
# who deploys may run it through sudo with no password (setup.sh), and on
# a Dukovo server, where the plan is open to read, with no sudo at all.
#
# On the server DUKOVO_SERVER_ROOT is empty. The boot tests set it to a
# folder that stands in for the server's /. The other places are an Ubuntu
# server's unless the environment names others, as the service files of a
# Dukovo server do (dukovo/README.md).
set -eu

server_root=${DUKOVO_SERVER_ROOT:-}
canaries=$server_root${DUKOVO_CANARIES:-/etc/dukovo-update-server/canaries.json}
reports=$server_root${DUKOVO_CANARY_REPORTS:-/var/lib/dukovo-canaries/reports}
home=$server_root${DUKOVO_SYSTEM_HOME:-/var/lib/dukovo-system}
program=$server_root${DUKOVO_SYSTEM_PROGRAM:-/usr/local/lib/dukovo-system/system-release}
site=$server_root${DUKOVO_SYSTEM_SITE:-/srv/dukovo-system}

if [ "$#" -eq 0 ]; then
	echo "usage: canaries.sh add NAME KEY | remove NAME | NAME red|green REASON | NAME auto" >&2
	echo "       canaries.sh show [--json]" >&2
	exit 2
fi

if [ "$1" = show ]; then
	shift
	# The plan is the user dukovo-system's: root, which runs this through
	# sudo, takes its place where there is such a user.
	if [ "$(id -u)" = 0 ] && id dukovo-system >/dev/null 2>&1; then
		set -- runuser -u dukovo-system -- "$program" canaries show \
			--canaries "$canaries" --reports "$reports" \
			--repo "$site/system" --plan "$home/rollout.json" "$@"
	else
		set -- "$program" canaries show --canaries "$canaries" --reports "$reports" \
			--repo "$site/system" --plan "$home/rollout.json" "$@"
	fi
	exec "$@"
fi

exec "$program" canary --canaries "$canaries" "$@"

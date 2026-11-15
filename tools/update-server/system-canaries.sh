#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# Holds or frees the waves of the system releases by what the canaries
# report (ADR 0533), as the user dukovo-system, when
# dukovo-system-canaries.path sees a report or a change of the list of
# canaries, and every 15 minutes (on a Dukovo server,
# dukovo/dukovo-system-canaries.toml starts it every 5 minutes, as the
# service dukovo-system-canaries). It works out which canaries are not
# green: one the list names that has no report, whose newest report is
# more than two hours old, that failed a check, fell back, found
# failures, has a failed service or did not take a release its channel
# has offered for six hours, or that a person marked red. When they are
# not those the plan in rollout.json holds the waves for, it signs the
# rollouts again, so that each machine's next check sees it; when they
# are, it signs nothing. With no canary on the list, nothing is held.
#
# The list is /etc/dukovo-update-server/canaries.json and the reports
# are in /var/lib/dukovo-canaries/reports. The reports are checked again
# here against the list, so a receiver that someone took over can hold
# the waves back but never free them. The extra words of the command line
# go to system-release: the boot tests give `--now`, the time to judge
# at. Nothing until the first deploy brings a root.
set -eu

# shellcheck source=tools/update-server/system-site.sh
. "$(dirname "$0")/system-site.sh"

canaries=$server_root${DUKOVO_CANARIES:-/etc/dukovo-update-server/canaries.json}
reports=$server_root${DUKOVO_CANARY_REPORTS:-/var/lib/dukovo-canaries/reports}

if [ ! -e "$site/system/metadata/root.json" ]; then
	echo "No system release has been deployed yet."
	exit 0
fi

# 0 when the rollouts are to be signed again, 3 when they are not.
status=0
"$program" canaries changed --canaries "$canaries" --reports "$reports" \
	--repo "$site/system" --plan "$home/rollout.json" "$@" || status=$?
case $status in
0) ;;
3) exit 0 ;;
*) exit "$status" ;;
esac
sign_site rollout --plan "$home/rollout.json" canaries \
	--canaries "$canaries" --reports "$reports" "$@"

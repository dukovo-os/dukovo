#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# Holds or frees the waves of the catalog's bundles by what the canaries
# report (ADR 0533), as the user dukovo-catalog, when
# dukovo-catalog-canaries.path sees a report or a change of the list of
# canaries, and every 15 minutes (on a Dukovo server,
# dukovo/dukovo-catalog-canaries.toml starts it every 5 minutes, as the
# service dukovo-catalog-canaries). The canaries are those of
# system-canaries.sh, from the same list and reports, judged by the same
# rules but one: a canary that did not take a newer system release is the
# system's concern alone. When the canaries not green are not those the
# plan in rollout.json holds the waves for, the rollouts are signed again,
# so that each machine's next check sees it; when they are, nothing is
# signed. The extra words of the command line go to catalog-repo: the boot
# tests give `--now`, the time to judge at. Nothing until the first deploy
# brings a root.
set -eu

# shellcheck source=tools/update-server/site.sh
. "$(dirname "$0")/site.sh"

canaries=${DUKOVO_CANARIES:-/etc/dukovo-update-server/canaries.json}
reports=${DUKOVO_CANARY_REPORTS:-/var/lib/dukovo-canaries/reports}

if [ ! -e "$home/repo/metadata/root.json" ]; then
	echo "No catalog has been deployed yet."
	exit 0
fi

# 0 when the rollouts are to be signed again, 3 when they are not.
status=0
"$program" --repo "$home/repo" canaries changed --canaries "$canaries" \
	--reports "$reports" --plan "$home/rollout.json" "$@" || status=$?
case $status in
0) ;;
3) exit 0 ;;
*) exit "$status" ;;
esac
sign_site rollout --store "$home/store" --plan "$home/rollout.json" canaries \
	--canaries "$canaries" --reports "$reports" "$@"

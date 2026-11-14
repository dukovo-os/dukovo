#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# Holds or frees the waves of the catalog's bundles (ADR 0544), as the
# user dukovo-catalog, which `ob catalog rollout` runs over ssh through
# sudo, and signs the rollouts again at once, so that each machine's next
# check sees it:
#
#   catalog-rollout.sh pause ID VERSION REASON   no further wave opens
#   catalog-rollout.sh resume ID VERSION         the waves open on schedule
#   catalog-rollout.sh canary NAME red|green     a red canary holds them all
#
# Only the newest version of a bundle has a rollout. The plan of every
# one, kept in rollout.json beside the online key, is never served.
set -eu

# shellcheck source=tools/update-server/site.sh
. "$(dirname "$0")/site.sh"

if [ ! -e "$home/repo/metadata/root.json" ]; then
	echo "No catalog has been deployed yet." >&2
	exit 1
fi
sign_site rollout --store "$home/store" --plan "$home/rollout.json" "$@"

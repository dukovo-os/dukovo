#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# Holds or frees the waves of the system releases (ADR 0469), as the user
# dukovo-system, which `ob release rollout` runs over ssh through sudo,
# and signs the rollouts again at once, so that each machine's next check
# sees it:
#
#   system-rollout.sh pause VERSION REASON   no further wave opens
#   system-rollout.sh resume VERSION         the waves open on schedule
#   system-rollout.sh canary NAME red|green  a red canary holds them all
#
# The plan of every release, kept in rollout.json beside the online key,
# is never served.
set -eu

# shellcheck source=tools/update-server/system-site.sh
. "$(dirname "$0")/system-site.sh"

if [ ! -e "$site/system/metadata/root.json" ]; then
	echo "No system release has been deployed yet." >&2
	exit 1
fi
sign_site rollout --plan "$home/rollout.json" "$@"

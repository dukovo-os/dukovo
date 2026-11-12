#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# Signs the system releases' timestamp again (ADR 0467), as the user
# dukovo-system, every six hours: a timestamp alone while the snapshot
# lists the newest targets and is good for two more days, a snapshot and
# a timestamp when it is not. Each release a channel offers has its
# rollout signed too, opening its waves on schedule (ADR 0469), from the
# plan in rollout.json. Nothing until the first deploy brings a root.
set -eu

# shellcheck source=tools/update-server/system-site.sh
. "$(dirname "$0")/system-site.sh"

if [ ! -e "$site/system/metadata/root.json" ]; then
	echo "No system release has been deployed yet."
	exit 0
fi
sign_site refresh --plan "$home/rollout.json"

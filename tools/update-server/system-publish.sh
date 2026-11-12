#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# Publishes the system releases a deploy copied into the incoming folder
# (ADR 0467), as the user dukovo-system, when dukovo-system-publish.path
# sees `ready`. system-release takes from the deploy only the roots and
# targets the release keys signed, the release lists, the channels and
# the files, each checked against its SHA-256 and none ever replaced;
# signs a snapshot and timestamp with the online key; and reads every
# release and channel back from the first root before the repository is
# served (system-site.sh). What happened is written to `result`, for the
# deploy to show; the deploy's files, now in the store, are removed.
set -eu

# shellcheck source=tools/update-server/system-site.sh
. "$(dirname "$0")/system-site.sh"

rm -f "$incoming/ready" "$incoming/result"

if output=$(sign_site publish --plan "$home/rollout.json" "$incoming" 2>&1); then
	status=published
	# The store holds them now, and the next deploy skips what it holds.
	rm -rf "${incoming:?}/files"
else
	status=failed
fi
printf '%s\n%s\n' "$status" "$output" >"$incoming/result.partial"
mv "$incoming/result.partial" "$incoming/result"
echo "$output"
[ "$status" = published ]

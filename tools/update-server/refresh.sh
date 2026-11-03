#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# Signs the catalog's timestamp again (ADR 0382), as the user
# dukovo-catalog, every six hours: a timestamp alone while the snapshot is
# good for two more days, everything again when it is not. Nothing until
# the first deploy brings a root.
set -eu

# shellcheck source=tools/update-server/site.sh
. /usr/local/lib/dukovo-catalog/site.sh

if [ ! -e "$home/repo/metadata/root.json" ]; then
	echo "No catalog has been deployed yet."
	exit 0
fi
sign_site refresh --store "$home/store"

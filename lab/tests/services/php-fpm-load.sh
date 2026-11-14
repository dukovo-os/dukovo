#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# The lab PC's side of services/php-fpm: it asks the machine's nginx for
# the site's front page, which PHP-FPM runs, in batches of 1,000 requests,
# 50 at a time, for 20 seconds, while the machine has PHP-FPM reload its
# settings, and counts the requests that got no 200: a reload must lose
# none. (The page that checks things starts a session for each request,
# so a load of it would fill the machine's folder of sessions.)
#
#   php-fpm-load.sh MACHINE PORT
#
# MACHINE is the machine's address and PORT the lab PC's that reaches its
# port 80, as {machine} and {machine-port:80} give them.
set -eu

machine="$1"
port="$2"

codes=$(mktemp)
trap 'rm -f "$codes"' EXIT
end=$(($(date +%s) + 20))
while [ "$(date +%s)" -lt "$end" ]; do
	# Each code goes to the standard error output (curl's %{stderr}),
	# the pages to nowhere; one that never came back is 000.
	curl -s --parallel --parallel-max 50 -w '%{stderr}%{http_code}\n' \
		"http://$machine:$port/?n=[1-1000]" \
		>/dev/null 2>>"$codes" || true
done
total=$(grep -c . "$codes" || true)
lost=$(grep -vc '^200$' "$codes" || true)
# No requests at all is a failure of the load, not a load that lost none.
if [ "$total" -eq 0 ]; then
	echo "requests 0: curl sent none"
	exit 1
fi
echo "requests $total, lost $lost"

#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# The lab PC's side of services/nginx: it asks the machine's nginx for its
# page over HTTP and HTTPS, 16 requests at a time, for 20 seconds, while
# the machine has it reload its settings, and counts the requests that got
# no 200: a reload must lose none.
#
#   nginx-load.sh MACHINE HTTP-PORT HTTPS-PORT
#
# MACHINE is the machine's address and the ports the lab PC's that reach
# its ports 80 and 443, as {machine} and {machine-port:N} give them.
set -eu

machine="$1"
http="$2"
https="$3"

: > codes
end=$(($(date +%s) + 20))
while [ "$(date +%s)" -lt "$end" ]; do
	# Each code goes to the standard error output (curl's %{stderr}),
	# the pages to nowhere; one that never came back is 000.
	curl -sk --parallel --parallel-max 16 -w '%{stderr}%{http_code}\n' \
		"http://$machine:$http/?n=[1-200]" "https://$machine:$https/?n=[1-200]" \
		>/dev/null 2>>codes || true
done
total=$(grep -c . codes || true)
lost=$(grep -vc '^200$' codes || true)
echo "requests $total, lost $lost"

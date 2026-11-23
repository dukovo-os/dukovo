#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# The machine's side of the lab test send-with-cable-out: it measures the
# CPUs' time busy, user and kernel, over five seconds that begin $1 seconds
# from now, and writes the milliseconds to /tmp/send-with-cable-out.busy.
# The test starts it before the cable comes out and reads the file after the
# cable is back, so no SSH link is needed, and none is used, while the
# measurement runs.
#
#   send-with-cable-out-busy.sh DELAY
set -eu

delay=$1
out=/tmp/send-with-cable-out.busy

busy() {
	awk '/^(user|system)_ns:/ { s += $2 } END { printf "%.0f\n", s }' /processes/stat
}

rm -f "$out"
sleep "$delay"
first=$(busy)
sleep 5
echo $((($(busy) - first) / 1000000)) >"$out.new"
mv "$out.new" "$out"

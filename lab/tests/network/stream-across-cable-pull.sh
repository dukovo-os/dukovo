#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# The lab PC's end of the stream-across-cable-pull test: 20 MiB of zeros
# to the first connection on TCP port $1, 64 KiB every 50 ms, so that the
# stream lasts about 20 seconds and the cable can be pulled in the middle
# of it; then nc closes its side, and the machine's nc ends. It says
# "stream ready" as nc starts to listen, and runs until the stream is
# sent or the lab ends it.
set -eu

port=$1
echo "stream ready"
i=0
while [ "$i" -lt 320 ]; do
	head -c 65536 /dev/zero
	sleep 0.05
	i=$((i + 1))
done | nc -N -l "$port"

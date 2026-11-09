#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# The lab PC's end of the send-with-cable-out test: takes what the
# machine sends to TCP port $1 and throws it away. It says "sink ready"
# as nc starts to listen, and runs until the machine's nc ends the
# connection or the lab ends it.
set -eu

port=$1
echo "sink ready"
nc -l "$port" > /dev/null

#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# Runs on the machine, as root: hands the mail that alertd sends to the
# machine's own port 587 on to the lab PC's mail catcher. alertd takes only
# ports 587 and 465, and the lab PC listens on neither unless it runs as
# root, so the catcher listens on a port of its own and this relays to it,
# a connection at a time, in both directions.
#
#   mail-relay.sh HOST PORT
#
# HOST and PORT are where the catcher listens, as {peer} and {port:NAME}
# give them. It runs until the machine restarts.
set -eu

host=$1
port=$2
fifo=/tmp/mail-relay.fifo

rm -f "$fifo"
mkfifo "$fifo"
while true; do
	nc -l 127.0.0.1 587 < "$fifo" | nc "$host" "$port" > "$fifo"
done

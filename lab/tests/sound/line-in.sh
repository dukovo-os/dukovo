#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# The lab PC's side of sound/tone.toml: it records the machine's jack
# through its own line in, in the test's folder (the working directory).
#
#   line-in.sh check MACHINE   exits with 77, which skips the test, if
#                              the lab has no cable from MACHINE (its
#                              address, as {machine} gives it)
#   line-in.sh record MACHINE  says "recording" and records 12 seconds
#                              to line-in.wav
#   line-in.sh hear            waits for the recording to end and says
#                              what it holds (line-in.py)
#
# ~/.config/dukovo-lab/line-in names the cables: a machine's address and
# the ALSA capture device its jack is plugged into, one machine a line.
# A QEMU guest has no jack. Nothing here changes the lab PC's own
# settings: the recording's level is the machine's to set.
set -eu

config="${XDG_CONFIG_HOME:-$HOME/.config}/dukovo-lab/line-in"

# The capture device for machine $1, or nothing.
device() {
	[ -r "$config" ] || return 0
	awk -v machine="$1" '$1 == machine { print $2; exit }' "$config"
}

case "$1" in
check)
	case "$2" in
	127.* | localhost | ::1)
		echo "the machine is a QEMU guest, with no jack to record"
		exit 77
		;;
	esac
	if [ -z "$(device "$2")" ]; then
		echo "no cable from $2 to the lab PC's line in: $config has no line for it"
		exit 77
	fi
	echo "recording $2 through $(device "$2")"
	;;
record)
	rm -f line-in.wav line-in.done
	echo "recording"
	arecord -q -D "$(device "$2")" -f S16_LE -r 48000 -c 2 -d 12 line-in.wav
	touch line-in.done
	;;
hear)
	i=0
	while [ ! -e line-in.done ]; do
		i=$((i + 1))
		if [ "$i" -gt 30 ]; then
			echo "the recording did not end"
			exit 1
		fi
		sleep 1
	done
	exec python3 -I "$(dirname "$0")/line-in.py" line-in.wav
	;;
*)
	echo "usage: line-in.sh check MACHINE | record MACHINE | hear" >&2
	exit 2
	;;
esac

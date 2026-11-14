#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# The machine's side of storage/io-weight (ADR 0476): it adds the three
# services of the io-weight boot test, with I/O weights 100 and 400 and
# with io_read = "10M", each reading the disk that holds Dukovo's root
# raw in place of the boot test's /devices/vda, starts them, and reads
# the counters of their domains twice, 10 seconds apart, each reading
# stamped twice by the kernel's clock. It says what each read a second,
# and whether the heavier read 3 to 5 times the bytes of the lighter and
# the rated one 9 to 11 MiB a second. Run as root.
#
#   io-weight.sh
set -eu

readings=/tmp/io-weight-readings
services="iow-light iow-heavy iow-rated"

# The whole disk of the partition that holds Dukovo's root, as Spark
# records it (spark(8)): sda for sda3, nvme0n1 for nvme0n1p2, and the
# disk itself when the root fills it.
root_disk() {
	part=$(sed -n 's/^disk //p' /system/live/spark/root)
	case "$part" in
	*[0-9]p[0-9]*) echo "${part%p*}" ;;
	nvme* | mmcblk*) echo "$part" ;;
	*) echo "$part" | sed 's/[0-9]*$//' ;;
	esac
}

disk=$(root_disk)
[ -e "/devices/$disk" ] || { echo "no disk /devices/$disk holds the root"; exit 1; }
echo "disk: $disk"
for s in $services; do
	sed "s|/devices/vda|/devices/$disk|g" "/system/tests/services/io-weight/$s.toml" \
		> "/system/settings/spark/services/$s.toml"
done

# A service's reading: its name, when the kernel's clock said it asked and
# when it had the answer, in nanoseconds, and the bytes its domain read.
reading() {
	before=$(cut -d ' ' -f 2 /processes/uptime)
	bytes=$(service "$1" status -n 0 | grep -o 'disk read [0-9.]*[KMGT]*' | cut -d ' ' -f 3)
	after=$(cut -d ' ' -f 2 /processes/uptime)
	echo "$1 $before $after $bytes"
}

: > "$readings"
for s in $services; do
	service "$s" start -q
done
sleep 5
for s in $services; do
	reading "$s" >> "$readings"
done
sleep 10
for s in $services; do
	reading "$s" >> "$readings"
done
for s in $services; do
	service "$s" stop -q
done

awk '
function size(text,   unit, number) {
	unit = substr(text, length(text), 1)
	number = text + 0
	if (unit == "K") number *= 1024
	else if (unit == "M") number *= 1048576
	else if (unit == "G") number *= 1073741824
	return number
}
{
	n = ++seen[$1]
	at[$1, n] = ($2 + $3) / 2e9
	bytes[$1, n] = size($4)
}
function rate(name) {
	return (bytes[name, 2] - bytes[name, 1]) / (at[name, 2] - at[name, 1])
}
END {
	light = rate("iow-light"); heavy = rate("iow-heavy"); rated = rate("iow-rated")
	printf "light %.1f MiB/s, heavy %.1f MiB/s, rated %.1f MiB/s\n", light / 1048576, heavy / 1048576, rated / 1048576
	if (light > 0 && heavy / light >= 3 && heavy / light <= 5) print "weights" "-ok"
	if (rated / 1048576 >= 9 && rated / 1048576 <= 11) print "rate" "-ok"
}' "$readings"
rm -f "$readings"

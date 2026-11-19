#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# The machine's side of journeys/day-in-the-life, run as root on the machine:
# what it holds before the day, and what it holds after, held to the first.
#
#   sh day-machine.sh before
#   sh day-machine.sh after
#   sh day-machine.sh memory
#
# before writes the memory in use, the files open, the disk used, the
# services that are up and the time, to journey-* files in the lab account's
# home. after reads them and says, a line each:
#
#   memory in use: 41000 KiB before, 52000 KiB after, grew 11000 KiB
#   open files: 400 before, 410 after, grew 10 (at most 100): kept
#   disk used: 8000 KiB more (at most 262144): kept
#   services down since the start: 0
#   errors in the log since the start: 0
#   panics in the log since the start: 0
#
# memory then holds the growth to its limit, for the step that comes last, so
# that a day that leaks has every other check made first. A line that ends in
# "kept" is within its limit. The memory is the kernel's
# own account of what is in use apart from the page cache, which a day of
# reading files fills and which is given back when it is wanted. When it grew,
# after says where: each kind of memory that changed by more than 1000 KiB, and
# the five programs whose resident memory grew most.
set -u

home=/users/lab
memory_limit=65536
files_limit=100
disk_limit=262144

in_use() {
	awk '/^total:/ { t = $2 } /^free:/ { f = $2 } /^page_cache:/ { c = $2 } END { print t - f - c }' /processes/meminfo
}

open_files() {
	lsof | wc -l
}

disk_used() {
	df -Pk /users | tail -n 1 | tr -s ' ' | cut -d ' ' -f 3
}

# The health check gives its result and ends, so it is up at the start and done later.
# The resident memory of each program, its processes added together, one
# `name KiB` line each.
resident_by_program() {
	ps -eo rss,comm | awk 'NR > 1 { kib[$2] += $1 } END { for (name in kib) print name, kib[name] }' | sort
}

services_up() {
	service --status-all | grep '\[ + \]' | sed 's/.*\]  //' | grep -v '^sysupdate$' | sort
}

within() {
	if [ "$1" -le "$2" ]; then echo kept; else echo "too much"; fi
}

case "${1:-}" in
before)
	in_use > "$home/journey-memory-before"
	open_files > "$home/journey-files-before"
	disk_used > "$home/journey-disk-before"
	services_up > "$home/journey-services-before"
	grep -v '^total:' /processes/meminfo | tr -d ':' > "$home/journey-meminfo-before"
	resident_by_program > "$home/journey-resident-before"
	date '+%Y-%m-%d %H:%M:%S' > "$home/journey-started"
	echo "before: $(cat "$home/journey-memory-before") KiB in use, $(cat "$home/journey-files-before") files open, $(cat "$home/journey-disk-before") KiB used, $(wc -l < "$home/journey-services-before") services up"
	;;
after)
	memory_before=$(cat "$home/journey-memory-before")
	memory_after=$(in_use)
	growth=$((memory_after - memory_before))
	echo "memory in use: $memory_before KiB before, $memory_after KiB after, grew $growth KiB"
	echo "$growth" > "$home/journey-memory-growth"

	files_before=$(cat "$home/journey-files-before")
	files_after=$(open_files)
	growth=$((files_after - files_before))
	echo "open files: $files_before before, $files_after after, grew $growth (at most $files_limit): $(within "$growth" "$files_limit")"

	grep -v '^total:' /processes/meminfo | tr -d ':' > "$home/journey-meminfo-after"
	resident_by_program > "$home/journey-resident-after"
	echo "kinds of memory that changed by more than 1000 KiB:"
	awk 'NR == FNR { before[$1] = $2; next } { change = $2 - before[$1]; if (change > 1000 || change < -1000) printf "  %s %+d KiB\n", $1, change }' "$home/journey-meminfo-before" "$home/journey-meminfo-after"
	echo "programs whose resident memory grew most:"
	awk 'NR == FNR { before[$1] = $2; next } { printf "%d %s\n", $2 - before[$1], $1 }' "$home/journey-resident-before" "$home/journey-resident-after" | sort -rn | head -n 5 | sed 's/^/  /'

	disk_before=$(cat "$home/journey-disk-before")
	disk_after=$(disk_used)
	growth=$((disk_after - disk_before))
	echo "disk used: $growth KiB more (at most $disk_limit): $(within "$growth" "$disk_limit")"

	services_up > "$home/journey-services-after"
	echo "services down since the start: $(comm -23 "$home/journey-services-before" "$home/journey-services-after" | wc -l)"
	comm -23 "$home/journey-services-before" "$home/journey-services-after"

	started=$(cat "$home/journey-started")
	echo "errors in the log since the start: $(log -p err -S "$started" | grep -v "^-- No entries --$" | grep -c .)"
	log -p err -S "$started" | grep -v "^-- No entries --$" | tail -n 10
	echo "panics in the log since the start: $(log -S "$started" -g 'Radix [p]anic|[p]anicked at' | grep -v "^-- No entries --$" | grep -c .)"
	;;
memory)
	growth=$(cat "$home/journey-memory-growth")
	echo "memory in use grew $growth KiB over the day (at most $memory_limit): $(within "$growth" "$memory_limit")"
	;;
*)
	echo "usage: sh day-machine.sh before | after | memory" >&2
	exit 2
	;;
esac

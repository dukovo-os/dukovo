#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# The machine's side of metrics-scrape: it reads the Prometheus scrape at
# the loopback and compares what it says with what free, df, ps and date
# say, which read the same kernel files another way (ADR 0484). Each line
# it prints says what matched; the first thing that does not stops it.
set -eu

scrape="/tmp/metrics-compare.$$"
trap 'rm -f "$scrape"' EXIT
curl -sf http://127.0.0.1:9100/metrics > "$scrape"

# The value of the sample NAME with no labels.
plain() {
	awk -v name="$1 " 'index($0, name) == 1 { print $2; exit }' "$scrape"
}

# The value of the sample NAME whose labels end with LABEL.
labelled() {
	awk -v name="$1{" -v label="$2" \
		'index($0, name) == 1 && index($0, label) { print $NF; exit }' "$scrape"
}

# Whether A is within SLACK of B.
near() {
	awk -v a="$1" -v b="$2" -v slack="$3" \
		'BEGIN { d = a - b; if (d < 0) d = -d; exit !(d <= slack) }'
}

# Says `ok: WHAT` when A is within SLACK of B, and stops when it is not.
agree() {
	what="$1"
	scraped="$2"
	other="$3"
	slack="$4"
	if [ -z "$scraped" ] || [ -z "$other" ]; then
		echo "missing: $what (scrape '$scraped', other '$other')"
		exit 1
	fi
	if near "$scraped" "$other" "$slack"; then
		echo "ok: $what"
	else
		echo "differs: $what (scrape $scraped, other $other)"
		exit 1
	fi
}

mib=1048576

# Memory: the total is fixed, so it is the same to the byte; what is
# available changes as the programs run, so it is the same to 64 MiB.
free_total="$(free -b | awk '/^Mem:/ { print $2 }')"
free_available="$(free -b | awk '/^Mem:/ { print $7 }')"
agree "memory total is free's" "$(plain node_memory_MemTotal_bytes)" "$free_total" 0
agree "memory available is free's" "$(plain node_memory_MemAvailable_bytes)" \
	"$free_available" "$((64 * mib))"

# The root filesystem: df counts in KiB, rounded up, so its size is the
# same to a KiB, and what is used and what is free to 64 MiB.
root_size="$(labelled node_filesystem_size_bytes 'mountpoint="/"}')"
root_free="$(labelled node_filesystem_avail_bytes 'mountpoint="/"}')"
df_size="$(df -kP / | awk 'NR == 2 { print $2 * 1024 }')"
df_available="$(df -kP / | awk 'NR == 2 { print $4 * 1024 }')"
agree "the root's size is df's" "$root_size" "$df_size" 1024
agree "the root's free space is df's" "$root_free" "$df_available" "$((64 * mib))"

# The service's memory: the resident memory of the process ps shows, to a
# quarter and 2 MiB, since the scrape is taken while metricsd answers it.
ps_kib="$(ps -C metricsd -o rss= | awk '{ total += $1 } END { print total * 1024 }')"
agree "metricsd's memory is ps's" "$(labelled dukovo_service_memory_bytes 'service="metricsd"}')" \
	"$ps_kib" "$((ps_kib / 4 + 2 * mib))"

# The clock the scrape reports is the machine's, and it booted before now.
agree "the time is date's" "$(plain node_time_seconds)" "$(date +%s)" 120
boot="$(plain node_boot_time_seconds)"
now="$(plain node_time_seconds)"
awk -v boot="$boot" -v now="$now" 'BEGIN { exit !(boot < now) }' ||
	{ echo "the machine booted after now: $boot, $now"; exit 1; }
echo "ok: it booted before now"

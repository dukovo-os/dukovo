#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# The lab PC's side of metrics-scrape: it asks the machine's Prometheus
# endpoint (ADR 0484) as a Prometheus server does, and has promtool read
# what comes back.
#
#   metrics-scrape.sh MACHINE PORT check | refused
#
# MACHINE is the machine's address and PORT the lab PC's port that reaches
# the endpoint, as {machine} and {machine-port:9100} give them. `check`
# waits for the endpoint, and says whether the scrape reads as Prometheus's
# text format, whether it has node_exporter's names, and how the endpoint
# answers a path and a method it does not serve. `refused` says whether
# the endpoint closes the connection without an answer, as it does for an
# address that its allow list does not name.
set -eu

machine="$1"
port="$2"
mode="$3"
url="http://$machine:$port"
scrape="$(mktemp)"
trap 'rm -f "$scrape"' EXIT

case "$mode" in
check)
	# The service binds the address once it has read its settings again.
	i=0
	while [ "$i" -lt 60 ]; do
		curl -sf --max-time 5 "$url/metrics" > "$scrape" && break
		i=$((i + 1))
		sleep 1
	done
	[ -s "$scrape" ] || { echo "no scrape from $url/metrics"; exit 1; }
	echo "scrape: $(wc -l < "$scrape") lines"

	# promtool exits with 3 for style findings. node_exporter's own
	# names, node_memory_MemTotal_bytes, are camel case, which its lint
	# does not like, and which dashboards need, so those findings are
	# no failure; any other is, and so is a scrape it cannot read.
	status=0
	found="$(promtool check metrics < "$scrape" 2>&1)" || status=$?
	case "$status" in
	0 | 3) ;;
	*) echo "promtool could not read the scrape ($status): $found"; exit 1 ;;
	esac
	other="$(printf '%s\n' "$found" | grep -v 'camelCase' | grep -v '^$' || true)"
	if [ -n "$other" ]; then
		echo "promtool found:"
		printf '%s\n' "$other"
		exit 1
	fi
	echo "promtool: the scrape is well formed"

	for name in node_cpu_seconds_total node_load1 node_memory_MemTotal_bytes \
		node_filesystem_size_bytes node_boot_time_seconds dukovo_service_memory_bytes; do
		grep -q "^$name[ {]" "$scrape" || { echo "the scrape has no $name"; exit 1; }
	done
	echo "names: node_exporter's and Dukovo's are there"

	echo "path: $(curl -s -o /dev/null -w '%{http_code}' "$url/other")"
	echo "method: $(curl -s -o /dev/null -w '%{http_code}' -X POST "$url/metrics")"
	;;
refused)
	status=0
	curl -s --max-time 10 "$url/metrics" > "$scrape" || status=$?
	if [ "$status" -ne 0 ] && [ ! -s "$scrape" ]; then
		echo "refused: the connection was closed without an answer"
	else
		echo "answered: $(head -1 "$scrape")"
		exit 1
	fi
	;;
*)
	echo "usage: metrics-scrape.sh MACHINE PORT check | refused"
	exit 2
	;;
esac

#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# The lab PC's KDC, the far end of the Kerberos tests: MIT's krb5kdc run
# as the lab's own account, in the test's folder (the working directory),
# on port $1 (TCP and UDP), for the realm DUKOVO.TEST, with nothing of
# the lab PC's own Kerberos settings. Its principals are alice (password
# drink-me), who must prove herself first (pre-authentication); bob
# (eat-me), who has only an aes128-cts key; and the service
# nfs/server.dukovo.test, whose keytab it serves on TCP port $2 to each
# connection. The KDC logs to kdc.log here. It says "kdc ready" once both
# listen, and runs until the lab ends it.
set -eu

port=$1
keytab_port=$2
work=$(pwd)
export KRB5_CONFIG="$work/krb5.conf" KRB5_KDC_PROFILE="$work/kdc.conf"

cat > kdc.conf <<CONF
[kdcdefaults]
  kdc_ports = $port
  kdc_tcp_ports = $port
[realms]
  DUKOVO.TEST = {
    database_name = $work/principal
    key_stash_file = $work/stash
    acl_file = $work/kadm5.acl
    max_life = 10h
    max_renewable_life = 7d
  }
[logging]
  kdc = FILE:$work/kdc.log
CONF
cat > krb5.conf <<CONF
[libdefaults]
  default_realm = DUKOVO.TEST
  dns_canonicalize_hostname = false
  rdns = false
[realms]
  DUKOVO.TEST = {
    kdc = 127.0.0.1:$port
  }
CONF
: > kadm5.acl

kdb5_util create -s -r DUKOVO.TEST -P "the master key of a test realm" > /dev/null
for query in \
	"addprinc -pw drink-me +requires_preauth alice" \
	"addprinc -pw eat-me -e aes128-cts:normal bob" \
	"addprinc -randkey nfs/server.dukovo.test" \
	"ktadd -k $work/nfs.keytab -e aes256-cts:normal nfs/server.dukovo.test"; do
	kadmin.local -r DUKOVO.TEST -q "$query" > /dev/null
done

# In this script's process group, which the lab ends at the test's end.
krb5kdc -n -r DUKOVO.TEST &
# The service's keytab, to whoever connects, a connection at a time.
while :; do nc -N -l "$keytab_port" < nfs.keytab > /dev/null 2>&1 || sleep 0.2; done &
i=0
until nc -z 127.0.0.1 "$port" 2> /dev/null && nc -z 127.0.0.1 "$keytab_port" 2> /dev/null; do
	i=$((i + 1))
	if [ "$i" -gt 200 ]; then
		echo "the KDC or the keytab did not listen; the KDC's log:"
		cat kdc.log
		exit 1
	fi
	sleep 0.1
done
echo "kdc ready"
wait

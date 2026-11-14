#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# The lab PC's end of the log-forward test: a syslog server of the lab
# PC's own rsyslog, as an administrator runs one, taking TLS on port $1
# with a certificate for lab-pc.test that a certificate authority of the
# test's own signed. The authority's certificate is ca.pem in the test's
# folder, for the test to copy to the machine, and what the server
# receives goes to received.log there, a line each: the application name
# and the message. It says "rsyslog ready" once it takes connections,
# and runs until the lab ends it. Everything is in the test's folder, and
# the lab PC's own rsyslog is left alone.
#
# Exits with 77 where the lab PC cannot do its part: no rsyslogd, no
# openssl, or an rsyslogd without the gtls driver (Debian's
# rsyslog-gnutls).
set -eu

port=$1
work=$(pwd)

for program in rsyslogd openssl nc; do
	if ! command -v "$program" > /dev/null 2>&1 && ! [ -x "/usr/sbin/$program" ]; then
		echo "the lab PC has no $program"
		exit 77
	fi
done
rsyslogd=$(command -v rsyslogd 2> /dev/null || echo /usr/sbin/rsyslogd)

cat > server.ext << 'END'
subjectAltName=DNS:lab-pc.test
basicConstraints=critical,CA:FALSE
keyUsage=critical,digitalSignature
extendedKeyUsage=serverAuth
END
key="openssl genpkey -algorithm EC -pkeyopt ec_paramgen_curve:prime256v1"
{
	$key -out ca.key
	openssl req -new -x509 -key ca.key -subj /CN=lab-forward-ca -days 30 -out ca.pem
	$key -out server.key
	openssl req -new -key server.key -subj /CN=lab-pc.test -out server.csr
	openssl x509 -req -in server.csr -CA ca.pem -CAkey ca.key -set_serial 2 -days 30 \
		-extfile server.ext -out server.pem
} > openssl.out 2>&1 || {
	cat openssl.out
	exit 1
}

cat > rsyslog.conf << END
global(
  workDirectory="$work"
  DefaultNetstreamDriver="gtls"
  DefaultNetstreamDriverCAFile="$work/ca.pem"
  DefaultNetstreamDriverCertFile="$work/server.pem"
  DefaultNetstreamDriverKeyFile="$work/server.key"
)
module(load="imtcp" StreamDriver.Name="gtls" StreamDriver.Mode="1" StreamDriver.AuthMode="anon")
input(type="imtcp" port="$port")
template(name="plain" type="string" string="%app-name% %msg%\n")
*.* action(type="omfile" file="$work/received.log" template="plain")
END

"$rsyslogd" -n -f "$work/rsyslog.conf" -i "$work/rsyslog.pid" > rsyslog.out 2>&1 &
pid=$!
trap 'kill "$pid" 2> /dev/null; exit 0' TERM INT

i=0
until nc -z 127.0.0.1 "$port" 2> /dev/null; do
	i=$((i + 1))
	if [ "$i" -gt 40 ] || ! kill -0 "$pid" 2> /dev/null; then
		cat rsyslog.out
		if grep -q "could not open config file .*Permission denied" rsyslog.out; then
			# A distribution's AppArmor profile for rsyslogd (Ubuntu's,
			# Mint's) lets it read only its own configuration.
			echo "the lab PC's rsyslogd may not read a configuration outside /etc (AppArmor's profile for it)"
			exit 77
		fi
		if grep -qi "gtls" rsyslog.out; then
			echo "the lab PC's rsyslogd has no gtls driver"
			exit 77
		fi
		echo "rsyslogd does not take connections on port $port"
		exit 1
	fi
	sleep 0.5
done
echo "rsyslog ready"
wait "$pid"

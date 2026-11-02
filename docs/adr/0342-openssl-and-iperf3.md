<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0342. openssl and iperf3

- Status: Accepted
- Date: 2026-11-02

## Context

The plan's 12e also lists openssl, which administrators use to make keys
and certificate requests, read certificates and test TLS servers with
s_client, and iperf3, which measures the network between two machines.
Dukovo's TLS is rustls's (ADR 0195), and no program of the base image
links OpenSSL's libraries; the openssl command is its own program. The
OpenSSL 3.5 series is OpenSSL's long-term support release, under
Apache-2.0, supported until April 2030. iperf3 is ESnet's, BSD-3-Clause
with parts under MIT, NCSA, Lucent's libtask notice and the public
domain; its sign-in with a password is the only part that needs
OpenSSL.

## Decision

- **openssl(1) is OpenSSL 3.5.4's command, as a port**, built as
  Linux's x86-64 build with its assembly, static, without shared
  libraries, modules, engines, asynchronous jobs, kernel TLS or the
  kernel's crypto interface. Only the command, its section 1 pages and
  openssl.cnf go into the image; OpenSSL's libraries stay in the port's
  folder, so that no other port builds on them and rustls stays the
  system's TLS. Its folder is /etc/ssl's, /system/settings/ssl, where
  the default settings already put the root certificates as cert.pem
  and certs/, and now its openssl.cnf. A patch gives OpenSSL's seeding
  getentropy(3) on Radix, which its Linux build reaches through
  `__linux`; mlibc's getentropy is getrandom's. Its pages keep their
  names, openssl-x509.1, without the suffix OpenSSL gives them, and its
  pages for its commands' old names (passwd.1, version.1...), which
  would take other commands' places, are left out.
- **iperf3(1) is ESnet's iperf3 3.19.1, as a port**, static, without
  OpenSSL, SCTP or profiling. Its LICENSE, which holds every notice of
  the parts it keeps, goes with the image's notices; the licenses it
  adds to the ports' list are NCSA (units.c from the first iperf) and
  the public domain of portable_endian.h, and Lucent's libtask notice is
  the one SPDX names dtoa.

## Consequences

- An administrator makes keys, requests and certificates, checks a
  server's certificate and tests TLS with s_client and s_server as on
  Linux; the network-tools boot test makes an Ed25519 key and a
  certificate, checks digests and ciphers, and has one machine's
  s_client reach the other's s_server over TLS 1.3.
- The image grows by the openssl command, about 7 MB, and iperf3,
  about 600 KB.
- OpenSSL's security releases in the 3.5 series are taken as they come,
  as every port's are.
- iperf3 measures TCP and UDP between machines, by IPv4 and IPv6 and
  either way; the network-tools boot test runs it against the other
  machine's server.

# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

"""A plain NTP (SNTP, RFC 4330) server for the lab's time tests.

It answers each request on the port given with the lab PC's own clock,
as a stratum 2 server whose reference clock is close, and prints how
many it answered. It runs in the test's folder until the test ends.
"""

import socket
import struct
import sys
import time

# Seconds from NTP's era (1900) to the Unix epoch (1970).
NTP_DELTA = 2208988800


def stamp(seconds):
    """An NTP timestamp: whole seconds and a 32-bit fraction."""
    whole = int(seconds)
    return struct.pack("!II", whole + NTP_DELTA, int((seconds - whole) * 2**32))


def main():
    port = int(sys.argv[1])
    server = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    server.bind(("0.0.0.0", port))
    print(f"sntp ready on {port}", flush=True)
    answered = 0
    while True:
        request, client = server.recvfrom(512)
        received = time.time()
        if len(request) < 48:
            continue
        version = (request[0] >> 3) & 7
        # No leap second warning, the client's version, mode 4 (server);
        # stratum 2, the client's poll, a precision of about a
        # microsecond; no root delay or dispersion; a reference ID; the
        # reference time; the client's transmit time as the origin; and
        # the receive and transmit times.
        reply = (
            struct.pack("!BBbb", version << 3 | 4, 2, request[2], -20)
            + struct.pack("!II", 0, 0)
            + b"LAB\0"
            + stamp(received)
            + request[40:48]
            + stamp(received)
            + stamp(time.time())
        )
        server.sendto(reply, client)
        answered += 1
        print(f"answered {client[0]}: {answered}", flush=True)


if __name__ == "__main__":
    main()

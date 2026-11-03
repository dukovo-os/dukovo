/* SPDX-FileCopyrightText: 2026 The Dukovo contributors */
/* SPDX-License-Identifier: Apache-2.0 OR MIT */

/* The ICMP errors a datagram or ping socket that set IP_RECVERR or
 * IPV6_RECVERR takes from its error queue with recvmsg(2)'s
 * MSG_ERRQUEUE, as Linux's structure and numbers give them (ADR 0345,
 * radix-posix(7)). The control message, of level SOL_IP and type
 * IP_RECVERR (SOL_IPV6 and IPV6_RECVERR for an IPv6 socket), holds a
 * struct sock_extended_err, then the address of the machine that sent
 * the error, which SO_EE_OFFENDER finds. */

#ifndef _LINUX_ERRQUEUE_H
#define _LINUX_ERRQUEUE_H

#include <stdint.h>

struct sock_extended_err {
	/* The error, an errno: the one the socket's next call fails with. */
	uint32_t ee_errno;
	/* What it came as: SO_EE_ORIGIN_ICMP or SO_EE_ORIGIN_ICMP6. */
	uint8_t ee_origin;
	/* The ICMP message's type and code. */
	uint8_t ee_type;
	uint8_t ee_code;
	uint8_t ee_pad;
	/* The MTU of a "fragmentation needed" or "packet too big", the
	 * pointer of a "parameter problem". */
	uint32_t ee_info;
	union {
		uint32_t ee_data;
		struct {
			uint16_t len;
			uint8_t flags;
			uint8_t reserved;
		} ee_rfc4884;
	};
};

#define SO_EE_ORIGIN_NONE 0
#define SO_EE_ORIGIN_LOCAL 1
#define SO_EE_ORIGIN_ICMP 2
#define SO_EE_ORIGIN_ICMP6 3
#define SO_EE_ORIGIN_TXSTATUS 4
#define SO_EE_ORIGIN_ZEROCOPY 5
#define SO_EE_ORIGIN_TXTIME 6
#define SO_EE_ORIGIN_TIMESTAMPING SO_EE_ORIGIN_TXSTATUS

/* The address of the machine that sent the error, which follows `ee`:
 * a struct sockaddr_in, or a struct sockaddr_in6 for an IPv6 socket. */
#define SO_EE_OFFENDER(ee) ((struct sockaddr *)((ee) + 1))

#define SO_EE_RFC4884_FLAG_INVALID 1

#endif

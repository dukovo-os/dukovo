// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// libpcap's capture module for Radix (ADR 0346), in place of pcap-null.c:
// a capture opened with radix-network(2)'s open_capture, through the
// first network capability whose control right allows it, as Linux's
// packet sockets and the BSDs' BPF devices are opened. Its reads give
// whole frames, each after a radix_capture_header and padded to 8 bytes,
// as a BPF device's do, and its control requests set the filter and give
// the counts. An interface's frames are Ethernet's (DLT_EN10MB), `lo`'s
// with zero addresses as on Linux, or a tunnel's IP packets (DLT_RAW).
// "any" captures every interface's, which come with Linux's cooked
// headers (DLT_LINUX_SLL or DLT_LINUX_SLL2) as they do on Linux, built
// here from the interface's number in each header. The kernel runs the
// filter of one interface's capture; "any"'s, which looks at the cooked
// headers, runs here. A read waits in poll, with a pipe that
// pcap_breakloop writes to, so that it ends at once. pcap_lookupnet is
// here too (patch 0001), from getifaddrs.

#include <config.h>

#include <arpa/inet.h>
#include <errno.h>
#include <fcntl.h>
#include <ifaddrs.h>
#include <net/if.h>
#include <netinet/in.h>
#include <poll.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

#include <radix/radix.h>

#include "pcap-int.h"
#include "pcap/sll.h"

// Linux's ARPHRD_ETHER, ARPHRD_LOOPBACK and ARPHRD_NONE, as an
// interface's `type` file gives them (radix-sysfs(5)).
#define TYPE_ETHER 1
#define TYPE_LOOPBACK 772
#define TYPE_NONE 65534

// The bytes of a record's header, and a record's alignment.
#define HEADER ((u_int)sizeof(struct radix_capture_header))
#define ALIGN8(n) (((n) + 7u) & ~7u)

// A status of the kernel's domain, with `code`.
#define KERNEL_STATUS(code) ((uint64_t)RADIX_KERNEL_ERROR_DOMAIN << 16 | (uint64_t)(code))

// What "any" knows of an interface, for its frames' cooked headers.
struct known {
	uint32_t index;
	uint16_t type;
	uint8_t mac[6];
};

#define KNOWN 32

struct pcap_radix {
	// Frames the filter here turned away since it was set, which the
	// kernel counted as received.
	uint64_t rejected;
	// Whether the kernel runs the filter.
	int kernel_filter;
	int nonblock;
	// The pipe pcap_breakloop writes to, and a read polls.
	int wake[2];
	int any;
	// "any"'s frame with its cooked header.
	u_char *cooked;
	struct known known[KNOWN];
	int known_count;
};

// The errno of a Radix status, of whichever domain it is.
static int status_errno(uint64_t status)
{
	switch (RADIX_STATUS_DOMAIN(status)) {
	case RADIX_KERNEL_ERROR_DOMAIN:
		return radix_kernel_errno(status);
	case RADIX_FILE_ERROR_DOMAIN:
		return radix_file_errno(status);
	default:
		return EIO;
	}
}

// Reads `field` of interface `name` in /kernel/class/net into `text`,
// without its newline: 0, or -1 if it is not there.
static int read_field(const char *name, const char *field, char *text, size_t size)
{
	char path[64 + IF_NAMESIZE];
	snprintf(path, sizeof(path), "/kernel/class/net/%s/%s", name, field);
	int fd = open(path, O_RDONLY | O_CLOEXEC);
	if (fd < 0)
		return -1;
	ssize_t got = read(fd, text, size - 1);
	close(fd);
	if (got < 0)
		return -1;
	text[got] = '\0';
	text[strcspn(text, "\n")] = '\0';
	return 0;
}

// Interface `name`'s ARPHRD type: TYPE_ETHER if its file cannot be read.
static uint16_t type_of(const char *name)
{
	char text[16];
	if (read_field(name, "type", text, sizeof(text)) < 0)
		return TYPE_ETHER;
	return (uint16_t)strtoul(text, NULL, 10);
}

// What "any" knows of interface `index`, learned the first time; one of
// `link`'s kind with no address if it is gone.
static const struct known *known_of(struct pcap_radix *pr, uint32_t index, uint16_t link)
{
	for (int i = 0; i < pr->known_count; i++) {
		if (pr->known[i].index == index)
			return &pr->known[i];
	}
	static struct known gone;
	struct known *known = pr->known_count < KNOWN ? &pr->known[pr->known_count] : &gone;
	memset(known, 0, sizeof(*known));
	known->index = index;
	known->type = link == RADIX_NETWORK_CAPTURE_LINK_RAW ? TYPE_NONE : TYPE_ETHER;
	char name[IF_NAMESIZE];
	if (if_indextoname(index, name) == NULL)
		return known;
	known->type = type_of(name);
	char text[32];
	unsigned int mac[6];
	if (read_field(name, "address", text, sizeof(text)) == 0 &&
	    sscanf(text, "%x:%x:%x:%x:%x:%x", &mac[0], &mac[1], &mac[2], &mac[3], &mac[4],
	           &mac[5]) == 6) {
		for (int i = 0; i < 6; i++)
			known->mac[i] = (uint8_t)mac[i];
	}
	if (known != &gone)
		pr->known_count++;
	return known;
}

// Builds in pr->cooked the frame "any" gives for `frame`, as Linux's
// packet sockets give it: the link header taken off, and in its place a
// cooked header of the interface, the packet's type and protocol, and
// the link address it came from. Returns the frame's length, and its
// captured bytes in `*captured`.
static u_int cook(pcap_t *p, const struct radix_capture_header *h, const u_char *frame,
                  u_int *captured)
{
	struct pcap_radix *pr = p->priv;
	const struct known *known = known_of(pr, h->interface, h->link);
	u_int length = h->length;
	u_int kept = h->captured;
	uint16_t protocol = 0;
	uint8_t pkttype = h->outgoing ? LINUX_SLL_OUTGOING : LINUX_SLL_HOST;
	uint8_t halen = 0;
	uint8_t addr[SLL_ADDRLEN] = {0};
	if (h->link == RADIX_NETWORK_CAPTURE_LINK_ETHERNET && kept >= 14) {
		const u_char *to = frame;
		const u_char *from = frame + 6;
		protocol = (uint16_t)(frame[12] << 8 | frame[13]);
		if (protocol < 0x600) {
			// An 802.3 length: Novell's raw 802.3 if 0xffff follows,
			// otherwise 802.2, as Linux's eth_type_trans says.
			protocol = kept >= 16 && frame[14] == 0xff && frame[15] == 0xff ? LINUX_SLL_P_802_3
			                                                                 : LINUX_SLL_P_802_2;
		}
		if (!h->outgoing && known->type != TYPE_LOOPBACK) {
			if (memcmp(to, "\xff\xff\xff\xff\xff\xff", 6) == 0)
				pkttype = LINUX_SLL_BROADCAST;
			else if (to[0] & 1)
				pkttype = LINUX_SLL_MULTICAST;
			else if (memcmp(to, known->mac, 6) != 0)
				pkttype = LINUX_SLL_OTHERHOST;
		}
		halen = 6;
		memcpy(addr, from, 6);
		frame += 14;
		kept -= 14;
		length -= 14;
	} else if (kept >= 1) {
		// An IP packet of a tunnel: its version says which.
		protocol = (frame[0] >> 4) == 6 ? 0x86dd : 0x0800;
	}
	u_int head;
	u_char *out = pr->cooked;
	if (p->linktype == DLT_LINUX_SLL2) {
		struct sll2_header sll;
		memset(&sll, 0, sizeof(sll));
		sll.sll2_protocol = htons(protocol);
		sll.sll2_if_index = htonl(h->interface);
		sll.sll2_hatype = htons(known->type);
		sll.sll2_pkttype = pkttype;
		sll.sll2_halen = halen;
		memcpy(sll.sll2_addr, addr, SLL_ADDRLEN);
		memcpy(out, &sll, SLL2_HDR_LEN);
		head = SLL2_HDR_LEN;
	} else {
		struct sll_header sll;
		memset(&sll, 0, sizeof(sll));
		sll.sll_pkttype = htons(pkttype);
		sll.sll_hatype = htons(known->type);
		sll.sll_halen = htons(halen);
		memcpy(sll.sll_addr, addr, SLL_ADDRLEN);
		sll.sll_protocol = htons(protocol);
		memcpy(out, &sll, SLL_HDR_LEN);
		head = SLL_HDR_LEN;
	}
	memcpy(out + head, frame, kept);
	*captured = head + kept;
	return head + length;
}

// Waits for a frame, for the timeout, or for pcap_breakloop: 1 to read
// again, 0 at the timeout, PCAP_ERROR on an error.
static int wait_for(pcap_t *p)
{
	struct pcap_radix *pr = p->priv;
	struct pollfd fds[2] = {
		{.fd = p->fd, .events = POLLIN},
		{.fd = pr->wake[0], .events = POLLIN},
	};
	int timeout = p->opt.timeout > 0 && !p->opt.immediate ? p->opt.timeout : -1;
	int ready = poll(fds, 2, timeout);
	if (ready < 0) {
		if (errno == EINTR)
			return 1;
		pcapint_fmt_errmsg_for_errno(p->errbuf, PCAP_ERRBUF_SIZE, errno, "poll");
		return PCAP_ERROR;
	}
	if (ready == 0)
		return 0;
	if (fds[1].revents & POLLIN) {
		char drained[16];
		while (read(pr->wake[0], drained, sizeof(drained)) > 0) {
		}
	}
	return 1;
}

static int pcap_read_radix(pcap_t *p, int max, pcap_handler callback, u_char *user)
{
	struct pcap_radix *pr = p->priv;
	int count = 0;
	while (p->cc == 0) {
		if (p->break_loop) {
			p->break_loop = 0;
			return PCAP_ERROR_BREAK;
		}
		ssize_t got = read(p->fd, p->buffer, p->bufsize);
		if (got > 0) {
			p->bp = p->buffer;
			p->cc = (int)got;
			break;
		}
		if (got == 0 || errno == EINTR)
			continue;
		if (errno == ENXIO) {
			snprintf(p->errbuf, PCAP_ERRBUF_SIZE,
			         "The interface disappeared");
			return PCAP_ERROR;
		}
		if (errno != EAGAIN) {
			pcapint_fmt_errmsg_for_errno(p->errbuf, PCAP_ERRBUF_SIZE, errno, "read");
			return PCAP_ERROR;
		}
		if (pr->nonblock)
			return 0;
		int waited = wait_for(p);
		if (waited <= 0)
			return waited;
	}
	while (p->cc > 0) {
		if (p->break_loop) {
			if (count > 0)
				return count;
			p->break_loop = 0;
			return PCAP_ERROR_BREAK;
		}
		if ((u_int)p->cc < HEADER) {
			p->cc = 0;
			break;
		}
		struct radix_capture_header h;
		memcpy(&h, p->bp, sizeof(h));
		const u_char *frame = p->bp + HEADER;
		u_int record = ALIGN8(HEADER + h.captured);
		if (record > (u_int)p->cc) {
			record = (u_int)p->cc;
			h.captured = record - HEADER;
		}
		p->bp += record;
		p->cc -= (int)record;
		if ((p->direction == PCAP_D_IN && h.outgoing) ||
		    (p->direction == PCAP_D_OUT && !h.outgoing))
			continue;
		u_int length = h.length;
		u_int captured = h.captured;
		if (pr->any) {
			length = cook(p, &h, frame, &captured);
			frame = pr->cooked;
		}
		if (!pr->kernel_filter && p->fcode.bf_insns != NULL &&
		    pcapint_filter(p->fcode.bf_insns, p->fcode.bf_len, frame, length, captured) == 0) {
			pr->rejected++;
			continue;
		}
		struct pcap_pkthdr header;
		header.ts.tv_sec = (time_t)(h.time / 1000000000u);
		header.ts.tv_usec = (suseconds_t)(h.time % 1000000000u);
		if (p->opt.tstamp_precision != PCAP_TSTAMP_PRECISION_NANO)
			header.ts.tv_usec /= 1000;
		header.len = length;
		header.caplen = captured < (u_int)p->snapshot ? captured : (u_int)p->snapshot;
		callback(user, &header, frame);
		count++;
		if (!PACKET_COUNT_IS_UNLIMITED(max) && count >= max)
			return count;
	}
	return count;
}

// Sets the kernel's filter to `program`, or takes it away for NULL,
// which also lets go of the frames kept and starts the counts again:
// 0, or the errno.
static int kernel_filter(pcap_t *p, const struct bpf_program *program)
{
	struct radix_file_control_results results;
	uint64_t size = program ? (uint64_t)program->bf_len * sizeof(struct bpf_insn) : 0;
	uint64_t buffer = program ? (uint64_t)(uintptr_t)program->bf_insns : 0;
	uint64_t status = radix_file_control(RADIX_STARTUP_SLOT_FD_BASE + (uint32_t)p->fd,
	                                     RADIX_NETWORK_CAPTURE_FILTER, 0, buffer, size, &results);
	return status ? status_errno(status) : 0;
}

static int pcap_setfilter_radix(pcap_t *p, struct bpf_program *program)
{
	struct pcap_radix *pr = p->priv;
	// The frames read before it might not pass it, as with BIOCSETF.
	p->cc = 0;
	pr->rejected = 0;
	if (!pr->any) {
		int err = kernel_filter(p, program);
		if (err == 0) {
			pr->kernel_filter = 1;
			pcap_freecode(&p->fcode);
			return 0;
		}
		if (err != EINVAL) {
			pcapint_fmt_errmsg_for_errno(p->errbuf, PCAP_ERRBUF_SIZE, err,
			                             "setting the capture's filter");
			return PCAP_ERROR;
		}
	}
	// One the kernel does not run, or "any"'s, runs here, on everything
	// the kernel keeps.
	int err = kernel_filter(p, NULL);
	if (err != 0) {
		pcapint_fmt_errmsg_for_errno(p->errbuf, PCAP_ERRBUF_SIZE, err,
		                             "taking the capture's filter away");
		return PCAP_ERROR;
	}
	pr->kernel_filter = 0;
	return pcapint_install_bpf_program(p, program);
}

static int pcap_stats_radix(pcap_t *p, struct pcap_stat *stats)
{
	struct pcap_radix *pr = p->priv;
	struct radix_capture_counts counts;
	struct radix_file_control_results results;
	uint64_t status = radix_file_control(RADIX_STARTUP_SLOT_FD_BASE + (uint32_t)p->fd,
	                                     RADIX_NETWORK_CAPTURE_COUNTS, 0,
	                                     (uint64_t)(uintptr_t)&counts, sizeof(counts), &results);
	if (status) {
		pcapint_fmt_errmsg_for_errno(p->errbuf, PCAP_ERRBUF_SIZE, status_errno(status),
		                             "reading the capture's counts");
		return PCAP_ERROR;
	}
	// Received counts what the filter took, as Linux's and the BSDs' do,
	// dropped what found no room.
	uint64_t received = counts.received;
	if (!pr->kernel_filter)
		received -= pr->rejected < received ? pr->rejected : received;
	stats->ps_recv = (u_int)received;
	stats->ps_drop = (u_int)counts.dropped;
	stats->ps_ifdrop = 0;
	return 0;
}

static int pcap_inject_radix(pcap_t *p, const void *buffer _U_, int size _U_)
{
	pcapint_strlcpy(p->errbuf, "Sending packets isn't supported on Dukovo", PCAP_ERRBUF_SIZE);
	return PCAP_ERROR;
}

static int pcap_setdirection_radix(pcap_t *p, pcap_direction_t direction)
{
	p->direction = direction;
	return 0;
}

static int pcap_set_datalink_radix(pcap_t *p, int dlt)
{
	p->linktype = dlt;
	return 0;
}

static int pcap_getnonblock_radix(pcap_t *p)
{
	return ((struct pcap_radix *)p->priv)->nonblock;
}

static int pcap_setnonblock_radix(pcap_t *p, int nonblock)
{
	((struct pcap_radix *)p->priv)->nonblock = nonblock;
	return 0;
}

static void pcap_breakloop_radix(pcap_t *p)
{
	struct pcap_radix *pr = p->priv;
	pcapint_breakloop_common(p);
	if (pr->wake[1] >= 0) {
		char wake = 1;
		// Safe in a signal handler, and never waits: the pipe holds a
		// wake already if it is full.
		ssize_t wrote = write(pr->wake[1], &wake, 1);
		(void)wrote;
	}
}

static void pcap_cleanup_radix(pcap_t *p)
{
	struct pcap_radix *pr = p->priv;
	for (int i = 0; i < 2; i++) {
		if (pr->wake[i] >= 0) {
			close(pr->wake[i]);
			pr->wake[i] = -1;
		}
	}
	free(pr->cooked);
	pr->cooked = NULL;
	pcapint_cleanup_live_common(p);
}

// Opens the capture of interface `index` (0 for every one) into the
// slot of a descriptor, through the first network capability that
// allows it: the descriptor, or -1 with the status in `*status`.
static int open_capture(uint32_t index, uint32_t snap, uint32_t buffer, uint64_t *status)
{
	*status = KERNEL_STATUS(RADIX_KERNEL_EMPTY_SLOT);
	for (int attempt = 0; attempt < 8; attempt++) {
		// The slot of the lowest free descriptor, unless another thread
		// takes it first.
		int fd = open("/devices/null", O_RDONLY | O_CLOEXEC);
		if (fd < 0) {
			*status = KERNEL_STATUS(RADIX_KERNEL_NO_MEMORY);
			return -1;
		}
		close(fd);
		uint32_t to = RADIX_STARTUP_SLOT_FD_BASE + (uint32_t)fd;
		uint64_t refusal = KERNEL_STATUS(RADIX_KERNEL_EMPTY_SLOT);
		int occupied = 0;
		for (uint32_t slot = RADIX_STARTUP_SLOT_NETWORK;
		     slot < RADIX_STARTUP_SLOT_NETWORK + RADIX_STARTUP_NETWORK_SLOTS; slot++) {
			uint64_t s = radix_network_open_capture(slot, to, index, snap, buffer,
			                                        RADIX_PIPE_FLAGS_NON_BLOCKING |
			                                            RADIX_PIPE_FLAGS_CLOSE_ON_EXEC);
			if (s == 0)
				return fd;
			if (RADIX_STATUS_DOMAIN(s) == RADIX_KERNEL_ERROR_DOMAIN) {
				switch (RADIX_STATUS_CODE(s)) {
				case RADIX_KERNEL_MISSING_RIGHTS:
					refusal = s;
					continue;
				case RADIX_KERNEL_NO_SUCH_SLOT:
				case RADIX_KERNEL_EMPTY_SLOT:
				case RADIX_KERNEL_WRONG_KIND:
					continue;
				case RADIX_KERNEL_OCCUPIED:
					occupied = 1;
					break;
				default:
					break;
				}
			}
			if (occupied)
				break;
			*status = s;
			return -1;
		}
		if (!occupied) {
			*status = refusal;
			return -1;
		}
	}
	*status = KERNEL_STATUS(RADIX_KERNEL_OCCUPIED);
	return -1;
}

static int pcap_activate_radix(pcap_t *p)
{
	struct pcap_radix *pr = p->priv;
	const char *device = p->opt.device;
	uint32_t index = 0;
	uint16_t type = TYPE_ETHER;
	pr->any = strcmp(device, "any") == 0;
	if (!pr->any) {
		index = if_nametoindex(device);
		if (index == 0) {
			pcapint_fmt_errmsg_for_errno(p->errbuf, PCAP_ERRBUF_SIZE, ENODEV,
			                             "if_nametoindex");
			return PCAP_ERROR_NO_SUCH_DEVICE;
		}
		type = type_of(device);
	}
	if (p->opt.rfmon)
		return PCAP_ERROR_RFMON_NOTSUP;
	if (p->snapshot <= 0 || p->snapshot > MAXIMUM_SNAPLEN)
		p->snapshot = MAXIMUM_SNAPLEN;
	u_int buffer = p->opt.buffer_size ? p->opt.buffer_size
	                                  : (u_int)RADIX_NETWORK_CAPTURE_BUFFER_DEFAULT;
	if (buffer < RADIX_NETWORK_CAPTURE_BUFFER_MIN)
		buffer = (u_int)RADIX_NETWORK_CAPTURE_BUFFER_MIN;
	if (buffer > RADIX_NETWORK_CAPTURE_BUFFER_MAX)
		buffer = (u_int)RADIX_NETWORK_CAPTURE_BUFFER_MAX;

	uint64_t status;
	p->fd = open_capture(index, (uint32_t)p->snapshot, buffer, &status);
	if (p->fd < 0) {
		// The cause, which tcpdump says after the device and what
		// went wrong, as Linux's libpcap gives it.
		int err = status_errno(status);
		if (RADIX_STATUS_DOMAIN(status) == RADIX_KERNEL_ERROR_DOMAIN &&
		    (RADIX_STATUS_CODE(status) == RADIX_KERNEL_MISSING_RIGHTS ||
		     RADIX_STATUS_CODE(status) == RADIX_KERNEL_EMPTY_SLOT)) {
			pcapint_strlcpy(p->errbuf,
			                "capturing needs admin.network: run it with elevate(8)",
			                PCAP_ERRBUF_SIZE);
			return PCAP_ERROR_PERM_DENIED;
		}
		if (err == ENXIO) {
			pcapint_fmt_errmsg_for_errno(p->errbuf, PCAP_ERRBUF_SIZE, err,
			                             "open_capture");
			return PCAP_ERROR_NO_SUCH_DEVICE;
		}
		if (err == EBUSY) {
			snprintf(p->errbuf, PCAP_ERRBUF_SIZE,
			         "%u packet captures are open, the most there may be",
			         (unsigned)RADIX_NETWORK_CAPTURES_MAX);
			return PCAP_ERROR;
		}
		pcapint_fmt_errmsg_for_errno(p->errbuf, PCAP_ERRBUF_SIZE, err, "open_capture");
		return PCAP_ERROR;
	}
	p->selectable_fd = p->fd;

	if (pipe2(pr->wake, O_NONBLOCK | O_CLOEXEC) < 0) {
		pcapint_fmt_errmsg_for_errno(p->errbuf, PCAP_ERRBUF_SIZE, errno, "pipe");
		goto fail;
	}

	if (pr->any) {
		// Linux's DLT_LINUX_SLL, and DLT_LINUX_SLL2 for those that ask.
		p->linktype = DLT_LINUX_SLL;
		p->dlt_list = malloc(2 * sizeof(u_int));
		if (p->dlt_list == NULL) {
			pcapint_fmt_errmsg_for_errno(p->errbuf, PCAP_ERRBUF_SIZE, errno, "malloc");
			goto fail;
		}
		p->dlt_list[0] = DLT_LINUX_SLL;
		p->dlt_list[1] = DLT_LINUX_SLL2;
		p->dlt_count = 2;
		pr->cooked = malloc(SLL2_HDR_LEN + (size_t)p->snapshot);
		if (pr->cooked == NULL) {
			pcapint_fmt_errmsg_for_errno(p->errbuf, PCAP_ERRBUF_SIZE, errno, "malloc");
			goto fail;
		}
	} else {
		p->linktype = type == TYPE_NONE ? DLT_RAW : DLT_EN10MB;
	}

	// Room for a frame as long as the snapshot, and for many shorter:
	// as much as the kernel keeps, up to 256 KiB.
	u_int longest = ALIGN8(HEADER + (u_int)p->snapshot);
	p->bufsize = buffer < 256 * 1024 ? buffer : 256 * 1024;
	if (p->bufsize < longest)
		p->bufsize = longest;
	p->buffer = malloc(p->bufsize);
	if (p->buffer == NULL) {
		pcapint_fmt_errmsg_for_errno(p->errbuf, PCAP_ERRBUF_SIZE, errno, "malloc");
		goto fail;
	}

	p->read_op = pcap_read_radix;
	p->inject_op = pcap_inject_radix;
	p->setfilter_op = pcap_setfilter_radix;
	p->setdirection_op = pcap_setdirection_radix;
	p->set_datalink_op = pcap_set_datalink_radix;
	p->getnonblock_op = pcap_getnonblock_radix;
	p->setnonblock_op = pcap_setnonblock_radix;
	p->stats_op = pcap_stats_radix;
	p->breakloop_op = pcap_breakloop_radix;
	p->cleanup_op = pcap_cleanup_radix;

	// A card takes the frames its driver lets in, which Dukovo cannot
	// yet ask for all of; promiscuity means nothing for "any", `lo` or a
	// tunnel, as on Linux.
	if (p->opt.promisc && !pr->any && type == TYPE_ETHER) {
		pcapint_strlcpy(p->errbuf,
		                "Dukovo cannot set a network card to promiscuous mode yet",
		                PCAP_ERRBUF_SIZE);
		return PCAP_WARNING_PROMISC_NOTSUP;
	}
	return 0;
fail:
	pcap_cleanup_radix(p);
	return PCAP_ERROR;
}

pcap_t *pcapint_create_interface(const char *device _U_, char *ebuf)
{
	pcap_t *p = PCAP_CREATE_COMMON(ebuf, struct pcap_radix);
	if (p == NULL)
		return NULL;
	struct pcap_radix *pr = p->priv;
	pr->wake[0] = pr->wake[1] = -1;
	p->activate_op = pcap_activate_radix;
	// Each frame's time is in nanoseconds.
	p->tstamp_precision_list = malloc(2 * sizeof(u_int));
	if (p->tstamp_precision_list == NULL) {
		pcapint_fmt_errmsg_for_errno(ebuf, PCAP_ERRBUF_SIZE, errno, "malloc");
		pcap_close(p);
		return NULL;
	}
	p->tstamp_precision_list[0] = PCAP_TSTAMP_PRECISION_MICRO;
	p->tstamp_precision_list[1] = PCAP_TSTAMP_PRECISION_NANO;
	p->tstamp_precision_count = 2;
	return p;
}

static int can_be_bound(const char *name _U_)
{
	return 1;
}

// Whether interface `name` is connected: by its `operstate`, except for
// `lo` and a tunnel, for which it means nothing.
static int get_if_flags(const char *name, bpf_u_int32 *flags, char *errbuf _U_)
{
	if ((*flags & PCAP_IF_LOOPBACK) || type_of(name) == TYPE_NONE) {
		*flags |= PCAP_IF_CONNECTION_STATUS_NOT_APPLICABLE;
		return 0;
	}
	char state[16];
	if (read_field(name, "operstate", state, sizeof(state)) == 0) {
		if (strcmp(state, "up") == 0)
			*flags |= PCAP_IF_CONNECTION_STATUS_CONNECTED;
		else if (strcmp(state, "down") == 0)
			*flags |= PCAP_IF_CONNECTION_STATUS_DISCONNECTED;
	}
	return 0;
}

int pcapint_platform_finddevs(pcap_if_list_t *devices, char *errbuf)
{
	// Those with addresses, with their addresses.
	if (pcapint_findalldevs_interfaces(devices, errbuf, can_be_bound, get_if_flags) == -1)
		return -1;
	// Then those without, which getifaddrs does not list.
	struct if_nameindex *names = if_nameindex();
	if (names != NULL) {
		for (struct if_nameindex *n = names; n->if_index != 0; n++) {
			char text[32];
			uint64_t flags = 0;
			if (read_field(n->if_name, "flags", text, sizeof(text)) == 0)
				flags = strtoull(text, NULL, 16);
			if (read_field(n->if_name, "carrier", text, sizeof(text)) == 0 &&
			    text[0] == '1')
				flags |= IFF_RUNNING;
			if (pcapint_find_or_add_if(devices, n->if_name, flags, get_if_flags, errbuf) ==
			    NULL) {
				if_freenameindex(names);
				return -1;
			}
		}
		if_freenameindex(names);
	}
	if (pcap_add_any_dev(devices, errbuf) == NULL)
		return -1;
	return 0;
}

// The network and netmask of `device`'s first IPv4 address, as pcap.c's
// gives them through SIOCGIFADDR and SIOCGIFNETMASK elsewhere: 0 for
// both for "any".
int pcap_lookupnet(const char *device, bpf_u_int32 *netp, bpf_u_int32 *maskp, char *errbuf)
{
	if (device == NULL || strcmp(device, "any") == 0) {
		*netp = *maskp = 0;
		return 0;
	}
	if (if_nametoindex(device) == 0) {
		snprintf(errbuf, PCAP_ERRBUF_SIZE, "%s: No such device exists", device);
		return -1;
	}
	struct ifaddrs *list;
	if (getifaddrs(&list) < 0) {
		pcapint_fmt_errmsg_for_errno(errbuf, PCAP_ERRBUF_SIZE, errno, "getifaddrs");
		return -1;
	}
	int found = 0;
	for (struct ifaddrs *a = list; a != NULL; a = a->ifa_next) {
		if (a->ifa_addr == NULL || a->ifa_addr->sa_family != AF_INET ||
		    strcmp(a->ifa_name, device) != 0)
			continue;
		*netp = ((struct sockaddr_in *)a->ifa_addr)->sin_addr.s_addr;
		*maskp = a->ifa_netmask ? ((struct sockaddr_in *)a->ifa_netmask)->sin_addr.s_addr : 0;
		found = 1;
		break;
	}
	freeifaddrs(list);
	if (!found) {
		snprintf(errbuf, PCAP_ERRBUF_SIZE, "%s: no IPv4 address assigned", device);
		return -1;
	}
	if (*maskp == 0) {
		// The address's class's, as pcap.c guesses.
		in_addr_t host = ntohl(*netp);
		if (IN_CLASSA(host))
			*maskp = htonl(IN_CLASSA_NET);
		else if (IN_CLASSB(host))
			*maskp = htonl(IN_CLASSB_NET);
		else if (IN_CLASSC(host))
			*maskp = htonl(IN_CLASSC_NET);
		else {
			snprintf(errbuf, PCAP_ERRBUF_SIZE, "inet class for 0x%x unknown", *netp);
			return -1;
		}
	}
	*netp &= *maskp;
	return 0;
}

const char *pcap_lib_version(void)
{
	return PCAP_VERSION_STRING;
}

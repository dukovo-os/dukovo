// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Network interfaces' names and numbers on Radix (ADR 0190):
// if_nametoindex, if_indextoname and if_nameindex read /kernel/class/net,
// where each interface is a directory of its name with its number in
// `ifindex` (radix-sysfs(5)), as Linux's C libraries ask the kernel
// through ioctl and netlink. getaddrinfo's AI_ADDRCONFIG reads their
// `ipv4` and `ipv6` files, for whether the machine has addresses of each
// family (ADR 0332), and getifaddrs lists those addresses with the
// interfaces' `flags` (ADR 0336).

#include <arpa/inet.h>
#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <ifaddrs.h>
#include <mlibc/all-sysdeps.hpp>
#include <mlibc/allocator.hpp>
#include <net/if.h>
#include <netinet/in.h>
#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

#include "radix.hpp"

namespace {

constexpr char NET[] = "/kernel/class/net/";

// Whether `name` could be an interface's: a file name shorter than
// IFNAMSIZ, so that a path made of it stays in the interface's directory.
bool plausible(const char *name, size_t len) {
	return len > 0 && len < IFNAMSIZ && !memchr(name, '/', len)
			&& strcmp(name, ".") && strcmp(name, "..");
}

// Reads `file` of the directory of the interface `len` bytes of `name`
// name into `text`, at most `size` bytes with a NUL after them: false if
// it cannot be read, or is empty.
bool read_field(const char *name, size_t len, const char *file, char *text, size_t size) {
	char path[sizeof(NET) + IFNAMSIZ + sizeof("/operstate")];
	size_t file_len = strlen(file);
	if(file_len >= sizeof("/operstate") - 1)
		return false;
	memcpy(path, NET, sizeof(NET) - 1);
	memcpy(path + sizeof(NET) - 1, name, len);
	path[sizeof(NET) - 1 + len] = '/';
	memcpy(path + sizeof(NET) + len, file, file_len + 1);
	int fd;
	if(mlibc::sysdep<Open>(path, O_RDONLY | O_CLOEXEC, 0, &fd))
		return false;
	size_t at = 0;
	for(;;) {
		ssize_t got = 0;
		if(at + 1 >= size || mlibc::sysdep<Read>(fd, text + at, size - 1 - at, &got) || got <= 0)
			break;
		at += static_cast<size_t>(got);
	}
	mlibc::sysdep<Close>(fd);
	text[at] = 0;
	return at > 0;
}

// The number of the interface `len` bytes of `name` names, read from its
// `ifindex`, or 0 if there is none.
unsigned int index_of(const char *name, size_t len) {
	char text[16];
	if(!read_field(name, len, "ifindex", text, sizeof(text)))
		return 0;
	return static_cast<unsigned int>(strtoul(text, nullptr, 10));
}

// Calls `f` with each line of `text`, with its NUL in place of its
// newline, until it returns false.
template<typename F>
void each_line(char *text, F f) {
	for(char *line = text; *line;) {
		char *end = strchr(line, '\n');
		if(end)
			*end = 0;
		if(*line && !f(line))
			return;
		if(!end)
			return;
		line = end + 1;
	}
}

// The address and prefix of `text`, an `ipv4` or `ipv6` file's
// 10.0.2.15/24 or 2001:db8::5/64, into `address` (an in_addr or an
// in6_addr, as `family`) and `prefix`: false if it is not one.
bool parse_cidr(int family, const char *text, void *address, unsigned int *prefix) {
	const char *slash = strchr(text, '/');
	char ip[INET6_ADDRSTRLEN];
	if(!slash || static_cast<size_t>(slash - text) >= sizeof(ip))
		return false;
	memcpy(ip, text, slash - text);
	ip[slash - text] = 0;
	char *end;
	unsigned long bits = strtoul(slash + 1, &end, 10);
	if(end == slash + 1 || bits > (family == AF_INET ? 32u : 128u))
		return false;
	*prefix = static_cast<unsigned int>(bits);
	return inet_pton(family, ip, address) == 1;
}

// Calls `f` with each interface's name and its length, in the listing's
// order, until it returns false; an errno if the interfaces cannot be
// listed.
template<typename F>
int each_interface(F f) {
	int dir;
	if(int e = mlibc::sysdep<OpenDir>(NET, &dir); e)
		return e;
	alignas(struct dirent) char entries[1024];
	for(bool more = true; more;) {
		size_t got = 0;
		if(int e = mlibc::sysdep<ReadEntries>(dir, entries, sizeof(entries), &got); e) {
			mlibc::sysdep<Close>(dir);
			return e;
		}
		more = got > 0;
		for(size_t at = 0; at < got && more;) {
			auto entry = reinterpret_cast<struct dirent *>(entries + at);
			at += entry->d_reclen;
			size_t len = strlen(entry->d_name);
			if(plausible(entry->d_name, len))
				more = f(entry->d_name, len);
		}
	}
	mlibc::sysdep<Close>(dir);
	return 0;
}

// Whether `file` of the interface `len` bytes of `name` name holds a
// line that starts with `start` (`start_len` bytes, none for any line).
bool has_line(const char *name, size_t len, const char *file, const char *start,
		size_t start_len) {
	char text[1024];
	if(!read_field(name, len, file, text, sizeof(text)))
		return false;
	bool found = false;
	each_line(text, [&](const char *line) {
		found = !strncmp(line, start, start_len);
		return !found;
	});
	return found;
}

// An entry of getifaddrs' list and what it points to, in one allocation,
// which freeifaddrs frees.
struct Entry {
	struct ifaddrs ifa;
	char name[IFNAMSIZ];
	union Address {
		struct sockaddr_in in;
		struct sockaddr_in6 in6;
	} addr, netmask, broadcast;
};

// The netmask of `prefix` bits, of an IPv6 address, into `mask`.
void ipv6_mask(unsigned int prefix, struct in6_addr *mask) {
	memset(mask, 0, sizeof(*mask));
	for(unsigned int i = 0; i < 16 && prefix; i++) {
		unsigned int bits = prefix < 8 ? prefix : 8;
		mask->s6_addr[i] = static_cast<uint8_t>(0xff00 >> bits);
		prefix -= bits;
	}
}

} // namespace

namespace radix {

unsigned int interface_with(uint32_t address) {
	unsigned int found = 0;
	each_interface([&](const char *name, size_t len) {
		char text[1024];
		if(!read_field(name, len, "ipv4", text, sizeof(text)))
			return true;
		each_line(text, [&](const char *line) {
			struct in_addr ip;
			unsigned int prefix;
			if(parse_cidr(AF_INET, line, &ip, &prefix) && ip.s_addr == address)
				found = index_of(name, len);
			return !found;
		});
		return !found;
	});
	return found;
}

uint32_t interface_address(unsigned int index) {
	char name[IF_NAMESIZE];
	if(!index || mlibc::sysdep<IfIndextoname>(index, name))
		return 0;
	char text[1024];
	if(!read_field(name, strlen(name), "ipv4", text, sizeof(text)))
		return 0;
	uint32_t address = 0;
	each_line(text, [&](const char *line) {
		struct in_addr ip;
		unsigned int prefix;
		if(parse_cidr(AF_INET, line, &ip, &prefix))
			address = ip.s_addr;
		return false;
	});
	return address;
}

} // namespace radix

namespace mlibc {

// As glibc counts them: any address but a loopback one, link-local IPv6
// ones too. `lo` has the cards' IPv4 addresses after its own, and their
// IPv6 ones, so only the cards are asked.
int Sysdeps<InetConfigured>::operator()(bool *ipv4, bool *ipv6) {
	*ipv4 = *ipv6 = false;
	return each_interface([&](const char *name, size_t len) {
		if(len == 2 && !memcmp(name, "lo", 2))
			return true;
		*ipv4 = *ipv4 || has_line(name, len, "ipv4", "", 0);
		*ipv6 = *ipv6 || has_line(name, len, "ipv6", "address ", 8);
		return !(*ipv4 && *ipv6);
	});
}

int Sysdeps<IfNametoindex>::operator()(const char *name, unsigned int *ret) {
	size_t len = strlen(name);
	*ret = plausible(name, len) ? index_of(name, len) : 0;
	return *ret ? 0 : ENODEV;
}

int Sysdeps<IfIndextoname>::operator()(unsigned int index, char *name) {
	bool found = false;
	if(int e = each_interface([&](const char *each, size_t len) {
			found = index && index_of(each, len) == index;
			if(found)
				memcpy(name, each, len + 1);
			return !found;
		}); e)
		return e;
	return found ? 0 : ENXIO;
}

// The list and its names are one allocation, which IfFreeNameindex frees:
// the entries, the last all zeros, then a name of IFNAMSIZ bytes for each.
int Sysdeps<IfNameindex>::operator()(struct if_nameindex **out) {
	size_t count = 0;
	if(int e = each_interface([&](const char *, size_t) {
			count++;
			return true;
		}); e)
		return e;
	size_t names = (count + 1) * sizeof(struct if_nameindex);
	auto list = static_cast<struct if_nameindex *>(
			getAllocator().allocate(names + count * IFNAMSIZ));
	if(!list)
		return ENOBUFS;
	// An interface that comes between the two listings is left out.
	size_t filled = 0;
	if(int e = each_interface([&](const char *name, size_t len) {
			if(filled == count)
				return false;
			unsigned int index = index_of(name, len);
			if(!index)
				return true;
			char *place = reinterpret_cast<char *>(list) + names + filled * IFNAMSIZ;
			memcpy(place, name, len + 1);
			list[filled++] = {index, place};
			return true;
		}); e) {
		getAllocator().free(list);
		return e;
	}
	list[filled] = {0, nullptr};
	*out = list;
	return 0;
}

void Sysdeps<IfFreeNameindex>::operator()(struct if_nameindex *list) {
	if(list)
		getAllocator().free(list);
}

// Each interface's IPv4 and IPv6 addresses, an entry each, in the
// order of their `ipv4` and `ipv6` files, the one each sends from first:
// with its netmask, an IPv4 address of a card its broadcast address, and
// a link-local IPv6 one its interface's number as sin6_scope_id, as
// glibc gives them. Linux's list has an AF_PACKET entry for each
// interface too, of its MAC address and counters: Radix has no packet
// sockets, so it has none, as getifaddrs gives on the BSDs no AF_LINK
// entry where there are no such addresses. The flags are the interface's
// `flags`, with IFF_RUNNING and IFF_LOWER_UP while its `carrier` is 1.
int Sysdeps<Getifaddrs>::operator()(struct ifaddrs **out) {
	struct ifaddrs *first = nullptr;
	struct ifaddrs **last = &first;
	bool full = false;
	auto add = [&](const char *name, size_t len, unsigned int flags) -> Entry * {
		auto entry = static_cast<Entry *>(getAllocator().allocate(sizeof(Entry)));
		if(!entry) {
			full = true;
			return nullptr;
		}
		memset(entry, 0, sizeof(*entry));
		memcpy(entry->name, name, len);
		entry->ifa.ifa_name = entry->name;
		entry->ifa.ifa_flags = flags;
		entry->ifa.ifa_addr = reinterpret_cast<struct sockaddr *>(&entry->addr);
		entry->ifa.ifa_netmask = reinterpret_cast<struct sockaddr *>(&entry->netmask);
		*last = &entry->ifa;
		last = &entry->ifa.ifa_next;
		return entry;
	};
	int e = each_interface([&](const char *name, size_t len) {
		char text[4096];
		unsigned int flags = 0;
		if(read_field(name, len, "flags", text, sizeof(text)))
			flags = static_cast<unsigned int>(strtoul(text, nullptr, 16));
		if(read_field(name, len, "carrier", text, sizeof(text)) && text[0] == '1')
			flags |= IFF_RUNNING | IFF_LOWER_UP;
		if(read_field(name, len, "ipv4", text, sizeof(text))) {
			each_line(text, [&](const char *line) {
				struct in_addr ip;
				unsigned int prefix;
				if(!parse_cidr(AF_INET, line, &ip, &prefix))
					return true;
				Entry *entry = add(name, len, flags);
				if(!entry)
					return false;
				uint32_t mask = prefix ? htonl(~UINT32_C(0) << (32 - prefix)) : 0;
				entry->addr.in.sin_family = AF_INET;
				entry->addr.in.sin_addr = ip;
				entry->netmask.in.sin_family = AF_INET;
				entry->netmask.in.sin_addr.s_addr = mask;
				if(flags & IFF_BROADCAST) {
					entry->broadcast.in.sin_family = AF_INET;
					entry->broadcast.in.sin_addr.s_addr = ip.s_addr | ~mask;
					entry->ifa.ifa_broadaddr =
							reinterpret_cast<struct sockaddr *>(&entry->broadcast);
				}
				return true;
			});
		}
		if(!full && read_field(name, len, "ipv6", text, sizeof(text))) {
			unsigned int index = index_of(name, len);
			each_line(text, [&](char *line) {
				if(strncmp(line, "address ", 8))
					return true;
				char *cidr = line + 8;
				if(char *space = strchr(cidr, ' '); space)
					*space = 0;
				struct in6_addr ip;
				unsigned int prefix;
				if(!parse_cidr(AF_INET6, cidr, &ip, &prefix))
					return true;
				Entry *entry = add(name, len, flags);
				if(!entry)
					return false;
				entry->addr.in6.sin6_family = AF_INET6;
				entry->addr.in6.sin6_addr = ip;
				if(IN6_IS_ADDR_LINKLOCAL(&ip))
					entry->addr.in6.sin6_scope_id = index;
				entry->netmask.in6.sin6_family = AF_INET6;
				ipv6_mask(prefix, &entry->netmask.in6.sin6_addr);
				return true;
			});
		}
		return !full;
	});
	if(e || full) {
		mlibc::sysdep<Freeifaddrs>(first);
		return e ? e : ENOMEM;
	}
	*out = first;
	return 0;
}

void Sysdeps<Freeifaddrs>::operator()(struct ifaddrs *list) {
	while(list) {
		struct ifaddrs *next = list->ifa_next;
		// Each entry's ifaddrs is the first member of its Entry.
		getAllocator().free(list);
		list = next;
	}
}

} // namespace mlibc

// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Sockets on Radix (ADRs 0085 and 0091): AF_UNIX stream sockets. The
// pairs radix-process(2)'s `socket_pair` makes, connected to each other,
// and those its `socket` makes, which bind names through
// radix-directory(2)'s `bind_socket`, connect through `connect_socket`,
// and listen and accept with radix-file(2)'s `listen` and `accept`. Each
// is a file description, so read, write, poll, dup, fcntl and close treat
// it as they treat a pipe; send and receive are radix-file(2)'s `send`
// and `receive`, which take MSG_DONTWAIT and MSG_NOSIGNAL, or with
// SCM_RIGHTS descriptors, `send_files` and `receive_files`; shutdown is
// its `shutdown`.
//
// AF_INET stream and datagram sockets are the kernel's TCP and UDP
// (ADR 0183), made, bound, connected and sent from through the network
// capabilities in the NETWORK_SLOTS slots from SLOT_NETWORK
// (radix-network(2)). Each such call tries them in turn until one allows
// it; when none does, it fails with EACCES, as Linux's calls do when a
// policy forbids them. A program given none cannot use the network.
// AF_INET6 sockets (ADR 0332) are the same, with IPv6 addresses, and
// IPv4 ones too, mapped (::ffff:10.0.2.2), unless IPV6_V6ONLY is set.

#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <mlibc/all-sysdeps.hpp>
#include <net/if.h>
#include <mlibc/threads.hpp>
#include <stddef.h>
#include <stdint.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/uio.h>
#include <sys/un.h>
#include <netinet/in.h>
#include <netinet/tcp.h>
#include <linux/errqueue.h>
#include <sys/time.h>
#include <stdlib.h>

#include "radix.hpp"

namespace {

// Whether `status` is the kernel error `code`.
bool is(uint64_t status, uint16_t code) {
	return RADIX_STATUS_DOMAIN(status) == RADIX_KERNEL_ERROR_DOMAIN
			&& RADIX_STATUS_CODE(status) == code;
}

// The errno of a socket call that failed with `status`: a descriptor that
// is not a file (a directory) is not a socket either, and one not open
// for the call's direction is a bad one for it, as for read and write.
int socket_errno(uint64_t status) {
	if(is(status, RADIX_KERNEL_NO_SUCH_METHOD) || is(status, RADIX_KERNEL_WRONG_KIND))
		return ENOTSOCK;
	if(is(status, RADIX_KERNEL_MISSING_RIGHTS))
		return EBADF;
	return radix::to_errno(status);
}

// 0 if `fd` is a socket; else ENOTSOCK, or EBADF if it is no descriptor.
int check_socket(int fd) {
	struct stat st;
	if(int e = mlibc::sysdep<Stat>(mlibc::fsfd_target::fd, fd, "", 0, &st); e)
		return e;
	return S_ISSOCK(st.st_mode) ? 0 : ENOTSOCK;
}

// The slot of socket descriptor `fd` in `slot`, or EBADF.
int slot_of(int fd, uint32_t *slot) {
	*slot = radix::slot_of(fd);
	return *slot == radix::NO_SLOT ? EBADF : 0;
}

// The flags send takes: the others that Linux takes for a stream socket
// (MSG_OOB, MSG_EOR) are for what these sockets do not have.
constexpr int SEND_FLAGS = MSG_DONTWAIT | MSG_NOSIGNAL | MSG_MORE;

// The flags receive takes.
constexpr int RECEIVE_FLAGS = MSG_DONTWAIT | MSG_WAITALL | MSG_CMSG_CLOEXEC | MSG_PEEK;

// The kernel's SocketFlags for send's `flags`.
uint32_t send_flags(int flags) {
	uint32_t kernel = 0;
	if(flags & MSG_DONTWAIT)
		kernel |= RADIX_SOCKET_FLAGS_DONT_WAIT;
	if(flags & MSG_NOSIGNAL)
		kernel |= RADIX_SOCKET_FLAGS_NO_SIGNAL;
	return kernel;
}

// Sends `size` bytes from `data` on `fd`, as send(2) with `flags`, with
// the `count` files whose slots `slots` holds riding on the first byte,
// made with `syscall`.
int send_bytes(radix_syscall_fn *syscall, int fd, const void *data, size_t size, int flags,
		const uint32_t *slots, size_t count, size_t *sent) {
	uint32_t slot;
	if(int e = slot_of(fd, &slot); e)
		return e;
	if(flags & ~SEND_FLAGS)
		return EOPNOTSUPP;
	uint64_t status;
	if(count) {
		struct radix_file_send_files_results r;
		status = radix_file_send_files_with(syscall, slot, data, size, slots,
				count * sizeof(uint32_t), send_flags(flags), &r);
		*sent = r.count;
	} else {
		struct radix_file_send_results r;
		status = radix_file_send_with(syscall, slot, data, size, send_flags(flags), &r);
		*sent = r.count;
	}
	if(status)
		return socket_errno(status);
	return 0;
}

// What a receive with room for descriptors got: where they go, how many
// fit, how many came, and whether some were lost for want of room.
struct Received {
	uint32_t *slots;
	size_t room;
	size_t count;
	bool lost;
};

// Receives up to `size` bytes on `fd` into `buffer`, waiting for some
// unless `dont_wait`: 0 bytes once the other end has stopped sending.
// With `peek`, they stay for the next receive (MSG_PEEK). With `files`,
// the descriptors that came with the bytes go there, closed by exec if
// `close_on_exec`. Made with `syscall`.
int receive_bytes(radix_syscall_fn *syscall, int fd, void *buffer, size_t size, bool dont_wait,
		bool peek, Received *files, bool close_on_exec, size_t *got) {
	uint32_t slot;
	if(int e = slot_of(fd, &slot); e)
		return e;
	uint32_t flags = dont_wait ? RADIX_SOCKET_FLAGS_DONT_WAIT : 0;
	if(peek)
		flags |= RADIX_SOCKET_FLAGS_PEEK;
	uint64_t status;
	if(files) {
		if(close_on_exec)
			flags |= RADIX_SOCKET_FLAGS_CLOSE_ON_EXEC;
		struct radix_file_receive_files_results r;
		status = radix_file_receive_files_with(syscall, slot, buffer, size,
				files->slots + files->count, (files->room - files->count) * sizeof(uint32_t),
				flags, &r);
		if(!status) {
			*got = r.count;
			// The kernel put them in the lowest free slots it found.
			for(size_t i = files->count; i < files->count + r.files; i++)
				radix::fd_taken(static_cast<int>(files->slots[i] - radix::FD_BASE));
			files->count += r.files;
			files->lost = files->lost || r.lost;
		}
	} else {
		struct radix_file_receive_results r;
		status = radix_file_receive_with(syscall, slot, buffer, size, flags, &r);
		*got = r.count;
	}
	if(status)
		return socket_errno(status);
	return 0;
}

// Peeks into the buffers of `iov`, as recvmsg(2) with MSG_PEEK and
// `flags`: one receive, through a buffer of their sum's size if there are
// more than one, as a second would see the same bytes again. MSG_WAITALL
// waits for no more than some bytes. As receive_vector otherwise.
int peek_vector(int fd, const struct iovec *iov, size_t count, int flags, Received *files,
		ssize_t *length) {
	size_t size;
	if(int e = radix::iov_total(iov, count, &size); e)
		return e;
	char *buffer = count == 1 ? static_cast<char *>(iov[0].iov_base) : nullptr;
	if(count != 1) {
		buffer = static_cast<char *>(malloc(size ? size : 1));
		if(!buffer)
			return ENOBUFS;
	}
	size_t got = 0;
	bool dont_wait = flags & MSG_DONTWAIT;
	bool close_on_exec = flags & MSG_CMSG_CLOEXEC;
	int e = receive_bytes(radix::cancellable, fd, buffer, size, dont_wait, true, files,
			close_on_exec, &got);
	if(!e && count != 1) {
		size_t at = 0;
		for(size_t i = 0; i < count && at < got; i++) {
			size_t n = got - at < iov[i].iov_len ? got - at : iov[i].iov_len;
			memcpy(iov[i].iov_base, buffer + at, n);
			at += n;
		}
	}
	if(count != 1)
		free(buffer);
	if(e)
		return e;
	*length = static_cast<ssize_t>(got);
	return 0;
}

// Receives into the buffers of `iov`, as recvmsg(2) with `flags`: what
// waits now, after waiting for some unless MSG_DONTWAIT; with MSG_WAITALL,
// until the buffers are full, the other end stops sending, or an error
// comes after some bytes (which are then what it returns). With `files`,
// the descriptors that come with the bytes go there, and the receive ends
// with the bytes they came with, as on Linux. With MSG_PEEK, what comes
// stays (peek_vector). A cancellation point until bytes come.
int receive_vector(int fd, const struct iovec *iov, size_t count, int flags, Received *files,
		ssize_t *length) {
	if(flags & ~RECEIVE_FLAGS)
		return EOPNOTSUPP;
	if(flags & MSG_PEEK)
		return peek_vector(fd, iov, count, flags, files, length);
	bool dont_wait = flags & MSG_DONTWAIT;
	bool wait_all = (flags & MSG_WAITALL) && !dont_wait;
	bool close_on_exec = flags & MSG_CMSG_CLOEXEC;
	size_t total = 0;
	for(size_t i = 0; i < count; i++) {
		char *base = static_cast<char *>(iov[i].iov_base);
		size_t at = 0;
		while(at < iov[i].iov_len) {
			// Once some bytes are in, only MSG_WAITALL waits for more.
			size_t got = 0;
			bool now = dont_wait || (total && !wait_all);
			int e = receive_bytes(total ? radix_syscall : radix::cancellable, fd, base + at,
					iov[i].iov_len - at, now, false, files, close_on_exec, &got);
			if(e && !total)
				return e;
			at += got;
			total += got;
			// An error after some bytes, the end, descriptors, or a short
			// receive without MSG_WAITALL: what came is the answer.
			bool came = files && (files->count || files->lost);
			if(e || !got || came || (!wait_all && at < iov[i].iov_len)) {
				*length = static_cast<ssize_t>(total);
				return 0;
			}
		}
	}
	// With no room for a byte it is a cancellation point all the same.
	if(!total)
		mlibc::thread_testcancel();
	*length = static_cast<ssize_t>(total);
	return 0;
}

// The two ends of a pair in the lowest free descriptors.
int make_pair(uint32_t flags, int *fds) {
	for(;;) {
		radix::SlotClaim first_end;
		radix::SlotClaim second_end;
		if(int e = radix::free_slot(0, &first_end); e)
			return e;
		uint32_t first = first_end.slot();
		int next = static_cast<int>(first - radix::FD_BASE) + 1;
		if(next >= radix::MAX_FDS)
			return EMFILE;
		if(int e = radix::free_slot(next, &second_end); e)
			return e;
		uint32_t second = second_end.slot();
		uint64_t status = radix_process_socket_pair(radix::SLOT_PROCESS, first, second, flags);
		// Something else filled one of them first: that one stays taken.
		if(is(status, RADIX_KERNEL_OCCUPIED)) {
			first_end.keep_if_full();
			second_end.keep_if_full();
			continue;
		}
		if(status)
			return radix::to_errno(status);
		first_end.keep();
		second_end.keep();
		fds[0] = static_cast<int>(first - radix::FD_BASE);
		fds[1] = static_cast<int>(second - radix::FD_BASE);
		return 0;
	}
}

// The kernel's PipeFlags for SOCK_NONBLOCK and SOCK_CLOEXEC in `flags`.
uint32_t description_flags(int flags) {
	uint32_t kernel = 0;
	if(flags & SOCK_NONBLOCK)
		kernel |= RADIX_PIPE_FLAGS_NON_BLOCKING;
	if(flags & SOCK_CLOEXEC)
		kernel |= RADIX_PIPE_FLAGS_CLOSE_ON_EXEC;
	return kernel;
}

// The path of `addr`, an AF_UNIX address of `length` bytes, in `out`
// (with room for sun_path and a NUL), and its length in `len`. Linux's
// abstract names, which start with a NUL, are not taken.
int unix_path(const struct sockaddr *addr, socklen_t length, char *out, size_t *len) {
	constexpr size_t start = offsetof(struct sockaddr_un, sun_path);
	if(!addr || length < start || addr->sa_family != AF_UNIX)
		return EINVAL;
	auto un = reinterpret_cast<const struct sockaddr_un *>(addr);
	size_t most = length - start;
	if(most > sizeof(un->sun_path))
		most = sizeof(un->sun_path);
	size_t n = strnlen(un->sun_path, most);
	if(!n)
		return most ? EOPNOTSUPP : EINVAL;
	if(n > RADIX_FILE_SOCKET_NAME_MAX)
		return ENAMETOOLONG;
	memcpy(out, un->sun_path, n);
	out[n] = 0;
	*len = n;
	return 0;
}

bool is_inet(uint32_t slot, int *type = nullptr);
int inet_name(uint32_t slot, bool peer, struct sockaddr *addr, socklen_t max, socklen_t *actual);

// The address of socket `fd`, or with `peer` of the socket at the other
// end of its connection, as getsockname and getpeername give it: the
// family, and the name the socket was bound to, if it has one, with its
// NUL; or a network socket's address and port.
int address(int fd, bool peer, struct sockaddr *addr, socklen_t max, socklen_t *actual) {
	uint32_t slot;
	if(int e = slot_of(fd, &slot); e)
		return e;
	if(is_inet(slot))
		return inet_name(slot, peer, addr, max, actual);
	struct sockaddr_un un;
	memset(&un, 0, sizeof(un));
	un.sun_family = AF_UNIX;
	struct radix_file_socket_name_results r;
	if(uint64_t status = radix_file_socket_name(slot, peer, un.sun_path,
			sizeof(un.sun_path) - 1, &r); status)
		return socket_errno(status);
	size_t len = offsetof(struct sockaddr_un, sun_path);
	if(r.len)
		len += (r.len < sizeof(un.sun_path) ? r.len : sizeof(un.sun_path) - 1) + 1;
	if(addr)
		memcpy(addr, &un, max < len ? max : len);
	*actual = static_cast<socklen_t>(len);
	return 0;
}

// The address recvfrom and recvmsg give: the name of the socket at the
// other end, or nothing (a length of 0) if it has none, as on Linux.
int sender(int fd, struct sockaddr *addr, socklen_t max, socklen_t *actual) {
	if(int e = address(fd, true, addr, max, actual); e)
		return e;
	if(*actual <= offsetof(struct sockaddr_un, sun_path))
		*actual = 0;
	return 0;
}

// The slots of the descriptors in `hdr`'s SCM_RIGHTS messages, in `slots`
// (room for RADIX_FILE_SEND_FILES_MAX), and how many in `count`.
int rights_of(const struct msghdr *hdr, uint32_t *slots, size_t *count) {
	*count = 0;
	if(!hdr->msg_control || !hdr->msg_controllen)
		return 0;
	for(auto cmsg = CMSG_FIRSTHDR(hdr); cmsg; cmsg = CMSG_NXTHDR(hdr, cmsg)) {
		// Credentials and the like are not sent.
		if(cmsg->cmsg_level != SOL_SOCKET || cmsg->cmsg_type != SCM_RIGHTS
				|| cmsg->cmsg_len < CMSG_LEN(0))
			return EINVAL;
		size_t n = (cmsg->cmsg_len - CMSG_LEN(0)) / sizeof(int);
		if(*count + n > RADIX_FILE_SEND_FILES_MAX)
			return EINVAL;
		const unsigned char *data = CMSG_DATA(cmsg);
		for(size_t i = 0; i < n; i++) {
			int fd;
			memcpy(&fd, data + i * sizeof(int), sizeof(int));
			uint32_t slot = radix::slot_of(fd);
			if(slot == radix::NO_SLOT)
				return EBADF;
			slots[(*count)++] = slot;
		}
	}
	return 0;
}

// Fills `hdr`'s control buffer with one SCM_RIGHTS message holding the
// descriptors `files` got, if any came; sets MSG_CTRUNC if some were lost.
// The length it gives is at most the buffer's, as Linux's is when the
// buffer has no room for the last message's padding.
void give_rights(struct msghdr *hdr, const Received &files) {
	size_t room = hdr->msg_controllen;
	hdr->msg_controllen = 0;
	if(files.lost)
		hdr->msg_flags |= MSG_CTRUNC;
	if(!files.count)
		return;
	auto cmsg = reinterpret_cast<struct cmsghdr *>(hdr->msg_control);
	cmsg->cmsg_level = SOL_SOCKET;
	cmsg->cmsg_type = SCM_RIGHTS;
	cmsg->cmsg_len = CMSG_LEN(files.count * sizeof(int));
	unsigned char *data = CMSG_DATA(cmsg);
	for(size_t i = 0; i < files.count; i++) {
		int fd = static_cast<int>(files.slots[i] - radix::FD_BASE);
		// A new descriptor has no directory path until it is given one.
		radix::forget_dir_path(fd);
		memcpy(data + i * sizeof(int), &fd, sizeof(int));
	}
	size_t used = CMSG_SPACE(files.count * sizeof(int));
	hdr->msg_controllen = used < room ? used : room;
}

// Whether socket `slot` is a network socket, and its type in `type`
// (SOCK_STREAM, or SOCK_DGRAM for a datagram or a ping socket) if it is:
// a Unix socket has no options.
bool is_inet(uint32_t slot, int *type) {
	struct radix_file_socket_option_results r;
	if(radix_file_socket_option(slot, RADIX_SOCKET_OPTION_KIND, false, 0, &r))
		return false;
	if(type)
		*type = r.value == RADIX_SOCKET_TYPE_STREAM ? SOCK_STREAM : SOCK_DGRAM;
	return true;
}

// Whether a call failed with `status` because the network capability in
// the slot it was given does not allow it, or there is none there: the
// next one may.
bool not_allowed(uint64_t status) {
	if(RADIX_STATUS_DOMAIN(status) != RADIX_KERNEL_ERROR_DOMAIN)
		return false;
	switch(RADIX_STATUS_CODE(status)) {
	case RADIX_KERNEL_EMPTY_SLOT:
	case RADIX_KERNEL_NO_SUCH_SLOT:
	case RADIX_KERNEL_WRONG_KIND:
	case RADIX_KERNEL_OUT_OF_RANGE: return true;
	// Without the send, local or receive right (ADR 0337); a socket's own
	// descriptor without the right a call needs (read or write) is EBADF,
	// as for the others.
	case RADIX_KERNEL_MISSING_RIGHTS:
		return RADIX_STATUS_DETAIL(status)
				& (RADIX_RIGHTS_SEND | RADIX_RIGHTS_LOCAL | RADIX_RIGHTS_RECEIVE);
	default: return false;
	}
}

// The errno of a network socket call that failed with `status`.
int inet_errno(uint64_t status) {
	if(not_allowed(status))
		return EACCES;
	return socket_errno(status);
}

// Makes `call(network)` with each network capability's slot in turn,
// until one allows it: 0, or the errno of the first failure that is not
// a refusal, or EACCES if every one refused. The calls take an address,
// and fail with not_supported for one of the other family, which is
// EAFNOSUPPORT, as on Linux.
template<typename F>
int through_network(F call) {
	for(uint32_t i = 0; i < RADIX_STARTUP_NETWORK_SLOTS; i++) {
		uint64_t status = call(RADIX_STARTUP_SLOT_NETWORK + i);
		if(!status)
			return 0;
		if(is(status, RADIX_KERNEL_NOT_SUPPORTED))
			return EAFNOSUPPORT;
		if(!not_allowed(status))
			return inet_errno(status);
	}
	return EACCES;
}

// The kernel's address for `addr`, an AF_INET or AF_INET6 address of
// `length` bytes. An AF_INET6 one may stop short of sin6_scope_id, as
// RFC 2133's did, and as Linux takes it.
int inet_address(const struct sockaddr *addr, socklen_t length, struct radix_address *out) {
	if(!addr || length < sizeof(sa_family_t))
		return EINVAL;
	memset(out, 0, sizeof(*out));
	if(addr->sa_family == AF_INET) {
		if(length < sizeof(struct sockaddr_in))
			return EINVAL;
		auto in = reinterpret_cast<const struct sockaddr_in *>(addr);
		out->family = RADIX_ADDRESS_FAMILY_IPV4;
		out->port = ntohs(in->sin_port);
		memcpy(out->ip, &in->sin_addr, sizeof(in->sin_addr));
		return 0;
	}
	if(addr->sa_family == AF_INET6) {
		if(length < offsetof(struct sockaddr_in6, sin6_scope_id))
			return EINVAL;
		struct sockaddr_in6 in6;
		memset(&in6, 0, sizeof(in6));
		memcpy(&in6, addr, length < sizeof(in6) ? length : sizeof(in6));
		out->family = RADIX_ADDRESS_FAMILY_IPV6;
		out->port = ntohs(in6.sin6_port);
		memcpy(out->ip, &in6.sin6_addr, sizeof(in6.sin6_addr));
		out->scope = in6.sin6_scope_id;
		return 0;
	}
	return EAFNOSUPPORT;
}

// Copies the kernel's `address` to `addr` as an AF_INET or AF_INET6
// address, its family's, at most `max` bytes of it, and its length to
// `actual`.
void give_inet(const struct radix_address &address, struct sockaddr *addr, socklen_t max,
		socklen_t *actual) {
	union {
		struct sockaddr_in in;
		struct sockaddr_in6 in6;
	} name;
	memset(&name, 0, sizeof(name));
	socklen_t size;
	if(address.family == RADIX_ADDRESS_FAMILY_IPV6) {
		name.in6.sin6_family = AF_INET6;
		name.in6.sin6_port = htons(address.port);
		memcpy(&name.in6.sin6_addr, address.ip, sizeof(name.in6.sin6_addr));
		name.in6.sin6_scope_id = address.scope;
		size = sizeof(name.in6);
	} else {
		name.in.sin_family = AF_INET;
		name.in.sin_port = htons(address.port);
		memcpy(&name.in.sin_addr, address.ip, sizeof(name.in.sin_addr));
		size = sizeof(name.in);
	}
	if(addr)
		memcpy(addr, &name, max < size ? max : size);
	*actual = size;
}

// The address of network socket `slot`, or with `peer` where it is
// connected, as getsockname and getpeername give it.
int inet_name(uint32_t slot, bool peer, struct sockaddr *addr, socklen_t max, socklen_t *actual) {
	struct radix_address address;
	struct radix_file_socket_name_results r;
	if(uint64_t status = radix_file_socket_name(slot, peer, &address, sizeof(address), &r);
			status)
		return inet_errno(status);
	give_inet(address, addr, max, actual);
	return 0;
}

// Whether network capability `network` has the receive right on every
// port the kernel may pick, so a socket bound through it to port 0 may
// listen on whichever port it gets.
bool receives_on_picked_ports(uint32_t network) {
	struct radix_network_ports_results ports;
	if(radix_network_ports(network, &ports))
		return false;
	uint64_t past = static_cast<uint64_t>(ports.first) + ports.count;
	return ports.first <= RADIX_NETWORK_EPHEMERAL_FIRST && past >= 65536;
}

// Binds network socket `slot` to `address`, whose port is 0 for one the
// kernel picks, through a network capability with the receive right, as
// a socket that is to listen must be: the kernel lets it listen only if
// the capability it was bound through may receive on the port it got
// (ADR 0183, ADR 0415). One that receives on every port the kernel may
// pick comes first, then any other with the receive right. EACCES if
// there is none.
int bind_receiving(uint32_t slot, const struct radix_address *address) {
	uint32_t chosen = 0;
	for(uint32_t i = 0; i < RADIX_STARTUP_NETWORK_SLOTS; i++) {
		uint32_t network = RADIX_STARTUP_SLOT_NETWORK + i;
		struct radix_capability_info_results info;
		if(radix_capability_info(network, &info) || info.kind != RADIX_OBJECT_KIND_NETWORK
				|| !(info.rights & RADIX_RIGHTS_RECEIVE))
			continue;
		if(receives_on_picked_ports(network)) {
			chosen = network;
			break;
		}
		if(!chosen)
			chosen = network;
	}
	if(!chosen)
		return EACCES;
	uint64_t status = radix_file_bind_address(slot, chosen, address, sizeof(*address));
	return status ? inet_errno(status) : 0;
}

// Binds network socket `slot`, of `family` (an AddressFamily), to any
// address and a port the kernel picks, through a network capability with
// the receive right, as Linux binds a socket that listens unbound. EACCES
// if there is none.
int bind_to_listen(uint32_t slot, uint32_t family) {
	struct radix_address any;
	memset(&any, 0, sizeof(any));
	any.family = family;
	return bind_receiving(slot, &any);
}

// The bytes of the `count` buffers at `iov` gathered into one buffer of
// `size` bytes from malloc, in `out`: a datagram is sent in one call.
int gather(const struct iovec *iov, int count, size_t size, char **out) {
	*out = static_cast<char *>(malloc(size ? size : 1));
	if(!*out)
		return ENOBUFS;
	size_t at = 0;
	for(int i = 0; i < count; i++) {
		memcpy(*out + at, iov[i].iov_base, iov[i].iov_len);
		at += iov[i].iov_len;
	}
	return 0;
}

// The kernel's SocketFlags for a network socket's receive `flags`: only
// MSG_DONTWAIT and MSG_PEEK mean anything to a datagram's receive.
uint32_t receive_flags(int flags) {
	uint32_t kernel = flags & MSG_DONTWAIT ? RADIX_SOCKET_FLAGS_DONT_WAIT : 0;
	if(flags & MSG_PEEK)
		kernel |= RADIX_SOCKET_FLAGS_PEEK;
	return kernel;
}

// Gives `hdr`, in its control buffer, where a datagram came to: the
// address `to` (the kernel's) on the interface numbered `index`, as
// IP_PKTINFO's in_pktinfo for an IPv4 socket's or IPV6_PKTINFO's
// in6_pktinfo for an IPv6 socket's. An in_pktinfo's ipi_spec_dst, the
// address a reply goes from, is `to`, or for a datagram to a group or to
// 255.255.255.255 the interface's own address, as on Linux (which gives
// that for a network's broadcast address too). MSG_CTRUNC if there is no
// room.
void give_packet_info(struct msghdr *hdr, const struct radix_address &to, uint32_t index) {
	union {
		struct in_pktinfo in;
		struct in6_pktinfo in6;
	} info;
	memset(&info, 0, sizeof(info));
	int level, type;
	size_t size;
	if(to.family == RADIX_ADDRESS_FAMILY_IPV6) {
		level = IPPROTO_IPV6;
		type = IPV6_PKTINFO;
		size = sizeof(info.in6);
		memcpy(&info.in6.ipi6_addr, to.ip, sizeof(info.in6.ipi6_addr));
		info.in6.ipi6_ifindex = index;
	} else {
		level = IPPROTO_IP;
		type = IP_PKTINFO;
		size = sizeof(info.in);
		info.in.ipi_ifindex = static_cast<int>(index);
		memcpy(&info.in.ipi_addr, to.ip, sizeof(info.in.ipi_addr));
		// A group's address (224/4) or every machine's (255.255.255.255).
		bool every = to.ip[0] >= 224;
		info.in.ipi_spec_dst.s_addr = every ? radix::interface_address(index)
				: info.in.ipi_addr.s_addr;
	}
	if(!hdr->msg_control || hdr->msg_controllen < CMSG_LEN(size)) {
		hdr->msg_flags |= MSG_CTRUNC;
		hdr->msg_controllen = 0;
		return;
	}
	auto cmsg = static_cast<struct cmsghdr *>(hdr->msg_control);
	cmsg->cmsg_level = level;
	cmsg->cmsg_type = type;
	cmsg->cmsg_len = CMSG_LEN(size);
	memcpy(CMSG_DATA(cmsg), &info, size);
	size_t used = CMSG_SPACE(size);
	hdr->msg_controllen = used < hdr->msg_controllen ? used : hdr->msg_controllen;
}

// Receives one datagram on network socket `slot` into the buffers of
// `iov`, as recvmsg(2) with `flags`, and who sent it into `from` (up to
// `from_max` bytes, its length in `from_len`; nothing if `from` is null).
// With `hdr`, whose control buffer it fills, where it came to too, if
// the socket asked for that (IP_PKTINFO, IPV6_RECVPKTINFO); nothing else
// comes there.
int receive_datagram(uint32_t slot, const struct iovec *iov, int count, int flags,
		struct sockaddr *from, socklen_t from_max, socklen_t *from_len, ssize_t *length,
		struct msghdr *hdr = nullptr) {
	if(flags & ~(MSG_DONTWAIT | MSG_CMSG_CLOEXEC | MSG_PEEK | MSG_TRUNC))
		return EOPNOTSUPP;
	size_t size;
	if(int e = radix::iov_total(iov, count, &size); e)
		return e;
	// The kernel gives the datagram's whole size, which says whether it
	// was cut short (MSG_TRUNC, ADR 0496).
	uint32_t kernel_flags = receive_flags(flags) | RADIX_SOCKET_FLAGS_WHOLE_SIZE;
	// One buffer is received into in place, more through one of the sum's
	// size, as the datagram comes whole.
	char *buffer = count == 1 ? static_cast<char *>(iov[0].iov_base) : nullptr;
	if(count != 1) {
		buffer = static_cast<char *>(malloc(size ? size : 1));
		if(!buffer)
			return ENOBUFS;
	}
	struct radix_address address, to;
	memset(&address, 0, sizeof(address));
	memset(&to, 0, sizeof(to));
	struct radix_file_receive_message_results r = {};
	uint64_t status;
	// Where it came to is asked for only with room for it, as it costs a
	// call of its own.
	if(hdr && hdr->msg_control && hdr->msg_controllen) {
		status = radix_file_receive_message_with(radix::cancellable, slot, buffer, size,
				&address, sizeof(address), &to, sizeof(to), kernel_flags, &r);
	} else {
		struct radix_file_receive_from_results from_r;
		status = radix_file_receive_from_with(radix::cancellable, slot, buffer, size,
				&address, sizeof(address), kernel_flags, &from_r);
		r.count = from_r.count;
	}
	size_t copied = r.count < size ? r.count : size;
	if(!status && count != 1) {
		size_t at = 0;
		for(int i = 0; i < count && at < copied; i++) {
			size_t n = copied - at < iov[i].iov_len ? copied - at : iov[i].iov_len;
			memcpy(iov[i].iov_base, buffer + at, n);
			at += n;
		}
	}
	if(count != 1)
		free(buffer);
	if(status)
		return inet_errno(status);
	if(from_len) {
		if(from)
			give_inet(address, from, from_max, from_len);
		else
			*from_len = 0;
	}
	if(hdr) {
		hdr->msg_flags = r.count > size ? MSG_TRUNC : 0;
		if(r.info)
			give_packet_info(hdr, to, r.interface);
		else
			hdr->msg_controllen = 0;
	}
	// MSG_TRUNC asks for the whole size, as on Linux.
	*length = static_cast<ssize_t>(flags & MSG_TRUNC ? r.count : copied);
	return 0;
}

// Takes the oldest ICMP error that waits for datagram or ping socket
// `slot`, as recvmsg(2) with MSG_ERRQUEUE does on Linux (ADR 0345): what
// it quotes of the packet into the buffers of `iov` (past a datagram's
// UDP header, or from an echo request's ICMP header on), where the
// packet went into `from` (up to `from_max` bytes, its length in
// `from_len`; nothing if `from` is null), and with `hdr`, in its control
// buffer, IP_RECVERR's struct sock_extended_err and the address of the
// machine that sent the error; IPV6_RECVERR's of level SOL_IPV6 for an
// IPv6 socket, as Linux gives an IPv4 error there too. The other flags
// change nothing, as on Linux. EAGAIN when none waits, and for a stream
// socket, which hears none.
int receive_error(uint32_t slot, const struct iovec *iov, int count, struct sockaddr *from,
		socklen_t from_max, socklen_t *from_len, ssize_t *length, struct msghdr *hdr) {
	size_t size;
	if(int e = radix::iov_total(iov, count, &size); e)
		return e;
	// The kernel quotes this much at most.
	char quoted[128];
	struct radix_socket_error report;
	memset(&report, 0, sizeof(report));
	struct radix_file_receive_error_results r = {};
	uint64_t status = radix_file_receive_error(slot, quoted, sizeof(quoted), &report,
			sizeof(report), &r);
	if(status) {
		int e = inet_errno(status);
		return e == EOPNOTSUPP ? EAGAIN : e;
	}
	size_t copied = r.count < size ? r.count : size;
	size_t at = 0;
	for(int i = 0; i < count && at < copied; i++) {
		size_t n = copied - at < iov[i].iov_len ? copied - at : iov[i].iov_len;
		memcpy(iov[i].iov_base, quoted + at, n);
		at += n;
	}
	if(from_len) {
		if(from)
			give_inet(report.destination, from, from_max, from_len);
		else
			*from_len = 0;
	}
	if(hdr) {
		hdr->msg_flags = MSG_ERRQUEUE;
		if(r.kept > size)
			hdr->msg_flags |= MSG_TRUNC;
		struct {
			struct sock_extended_err ee;
			union {
				struct sockaddr_in in;
				struct sockaddr_in6 in6;
			} offender;
		} errhdr;
		memset(&errhdr, 0, sizeof(errhdr));
		errhdr.ee.ee_errno = static_cast<uint32_t>(radix::to_errno(report.error));
		errhdr.ee.ee_origin = report.origin;
		errhdr.ee.ee_type = report.kind;
		errhdr.ee.ee_code = report.code;
		errhdr.ee.ee_info = report.info;
		socklen_t offender_size;
		give_inet(report.offender, reinterpret_cast<struct sockaddr *>(&errhdr.offender),
				sizeof(errhdr.offender), &offender_size);
		bool v6 = report.destination.family == RADIX_ADDRESS_FAMILY_IPV6;
		size_t data = sizeof(errhdr.ee) + offender_size;
		if(!hdr->msg_control || hdr->msg_controllen < CMSG_LEN(data)) {
			hdr->msg_flags |= MSG_CTRUNC;
			hdr->msg_controllen = 0;
		} else {
			auto cmsg = static_cast<struct cmsghdr *>(hdr->msg_control);
			cmsg->cmsg_level = v6 ? SOL_IPV6 : SOL_IP;
			cmsg->cmsg_type = v6 ? IPV6_RECVERR : IP_RECVERR;
			cmsg->cmsg_len = CMSG_LEN(data);
			memcpy(CMSG_DATA(cmsg), &errhdr, data);
			size_t used = CMSG_SPACE(data);
			hdr->msg_controllen = used < hdr->msg_controllen ? used : hdr->msg_controllen;
		}
	}
	*length = static_cast<ssize_t>(copied);
	return 0;
}

// Sends `size` bytes from `data` on network socket `slot` as send(2) with
// `flags`, to `to` (an AF_INET address of `length` bytes) if it is not
// null, through a network capability that allows it.
int send_inet(uint32_t slot, const void *data, size_t size, int flags,
		const struct sockaddr *to, socklen_t length, ssize_t *sent) {
	if(flags & ~SEND_FLAGS)
		return EOPNOTSUPP;
	if(!to) {
		struct radix_file_send_results r;
		uint64_t status = radix_file_send_with(radix::cancellable, slot, data, size,
				send_flags(flags), &r);
		if(status)
			return inet_errno(status);
		*sent = static_cast<ssize_t>(r.count);
		return 0;
	}
	struct radix_address address;
	if(int e = inet_address(to, length, &address); e)
		return e;
	struct radix_file_send_to_results r;
	if(int e = through_network([&](uint32_t network) {
			return radix_file_send_to_with(radix::cancellable, slot, network, &address,
					sizeof(address), data, size, send_flags(flags), &r);
		}); e)
		return e;
	*sent = static_cast<ssize_t>(r.count);
	return 0;
}

// Reads network socket option `option` of `slot` into `value`.
int get_option(uint32_t slot, enum radix_socket_option option, uint64_t *value) {
	struct radix_file_socket_option_results r;
	if(uint64_t status = radix_file_socket_option(slot, option, false, 0, &r); status)
		return inet_errno(status);
	*value = r.value;
	return 0;
}

// Sets network socket option `option` of `slot` to `value`.
int set_option(uint32_t slot, enum radix_socket_option option, uint64_t value) {
	struct radix_file_socket_option_results r;
	uint64_t status = radix_file_socket_option(slot, option, true, value, &r);
	return status ? inet_errno(status) : 0;
}

// SO_BINDTODEVICE of `slot`, as Linux gives it: the interface's name and
// its NUL, or nothing for a socket bound to none.
int get_device(uint32_t slot, void *buffer, socklen_t *size) {
	if(*size < IFNAMSIZ)
		return EINVAL;
	uint64_t index;
	if(int e = get_option(slot, RADIX_SOCKET_OPTION_BIND_TO_DEVICE, &index); e)
		return e;
	if(!index) {
		*size = 0;
		return 0;
	}
	char name[IFNAMSIZ];
	if(int e = mlibc::sysdep<IfIndextoname>(static_cast<unsigned int>(index), name); e)
		return e;
	size_t len = strlen(name) + 1;
	memcpy(buffer, name, len);
	*size = static_cast<socklen_t>(len);
	return 0;
}

// Binds `slot` with SO_BINDTODEVICE to the interface the `size` bytes of
// `buffer` name, or to none for an empty name, as Linux takes it: a name
// without its NUL, cut at IFNAMSIZ - 1 bytes.
int set_device(uint32_t slot, const void *buffer, socklen_t size) {
	char name[IFNAMSIZ] = {};
	memcpy(name, buffer, size < IFNAMSIZ - 1 ? size : IFNAMSIZ - 1);
	unsigned int index = 0;
	if(name[0])
		if(int e = mlibc::sysdep<IfNametoindex>(name, &index); e)
			return e;
	return set_option(slot, RADIX_SOCKET_OPTION_BIND_TO_DEVICE, index);
}

// The kernel's option for `layer` and `number`, and whether it is a time
// (a struct timeval, in nanoseconds to the kernel): false for one it does
// not have.
bool inet_option(int layer, int number, enum radix_socket_option *option, bool *time) {
	*time = false;
	if(layer == SOL_SOCKET) {
		switch(number) {
		case SO_REUSEADDR: *option = RADIX_SOCKET_OPTION_REUSE_ADDRESS; return true;
		case SO_REUSEPORT: *option = RADIX_SOCKET_OPTION_REUSE_PORT; return true;
		case SO_KEEPALIVE: *option = RADIX_SOCKET_OPTION_KEEP_ALIVE; return true;
		case SO_BROADCAST: *option = RADIX_SOCKET_OPTION_BROADCAST; return true;
		case SO_RCVBUF: *option = RADIX_SOCKET_OPTION_RECEIVE_BUFFER; return true;
		case SO_SNDBUF: *option = RADIX_SOCKET_OPTION_SEND_BUFFER; return true;
		case SO_ERROR: *option = RADIX_SOCKET_OPTION_ERROR; return true;
		// Urgent data always comes inline: the option is kept, and read
		// back (ADR 0496).
		case SO_OOBINLINE: *option = RADIX_SOCKET_OPTION_OUT_OF_BAND_INLINE; return true;
		case SO_RCVTIMEO:
			*option = RADIX_SOCKET_OPTION_RECEIVE_TIMEOUT;
			*time = true;
			return true;
		case SO_SNDTIMEO:
			*option = RADIX_SOCKET_OPTION_SEND_TIMEOUT;
			*time = true;
			return true;
		default: return false;
		}
	}
	if(layer == IPPROTO_TCP) {
		switch(number) {
		case TCP_NODELAY: *option = RADIX_SOCKET_OPTION_NO_DELAY; return true;
		case TCP_KEEPIDLE: *option = RADIX_SOCKET_OPTION_KEEP_IDLE; return true;
		case TCP_KEEPINTVL: *option = RADIX_SOCKET_OPTION_KEEP_INTERVAL; return true;
		case TCP_KEEPCNT: *option = RADIX_SOCKET_OPTION_KEEP_COUNT; return true;
		case TCP_QUICKACK: *option = RADIX_SOCKET_OPTION_QUICK_ACK; return true;
		default: return false;
		}
	}
	// A group's datagrams' hop limit, and whether they would loop back,
	// are the socket's for both versions of IP (ADR 0336); IP_PKTINFO and
	// IPV6_RECVPKTINFO ask where datagrams came to, as the socket's
	// version's; IP_RECVERR and IPV6_RECVERR ask for the ICMP errors
	// about what it sends, both versions' (ADR 0345).
	if(layer == IPPROTO_IP) {
		switch(number) {
		case IP_TTL: *option = RADIX_SOCKET_OPTION_TIME_TO_LIVE; return true;
		case IP_MULTICAST_TTL: *option = RADIX_SOCKET_OPTION_MULTICAST_HOPS; return true;
		case IP_MULTICAST_LOOP: *option = RADIX_SOCKET_OPTION_MULTICAST_LOOP; return true;
		case IP_PKTINFO: *option = RADIX_SOCKET_OPTION_PACKET_INFO; return true;
		case IP_RECVERR: *option = RADIX_SOCKET_OPTION_RECEIVE_ERRORS; return true;
		default: return false;
		}
	}
	if(layer == IPPROTO_IPV6) {
		switch(number) {
		case IPV6_V6ONLY: *option = RADIX_SOCKET_OPTION_V6_ONLY; return true;
		// The hop limit is IPv6's time to live, the socket's one for both.
		case IPV6_UNICAST_HOPS: *option = RADIX_SOCKET_OPTION_TIME_TO_LIVE; return true;
		case IPV6_MULTICAST_IF: *option = RADIX_SOCKET_OPTION_MULTICAST_INTERFACE; return true;
		case IPV6_MULTICAST_HOPS: *option = RADIX_SOCKET_OPTION_MULTICAST_HOPS; return true;
		case IPV6_MULTICAST_LOOP: *option = RADIX_SOCKET_OPTION_MULTICAST_LOOP; return true;
		case IPV6_RECVPKTINFO: *option = RADIX_SOCKET_OPTION_PACKET_INFO; return true;
		case IPV6_RECVERR: *option = RADIX_SOCKET_OPTION_RECEIVE_ERRORS; return true;
		default: return false;
		}
	}
	return false;
}

// Whether `layer` and `number` are IP_TOS or IPV6_TCLASS.
bool type_of_service(int layer, int number) {
	return (layer == IPPROTO_IP && number == IP_TOS)
			|| (layer == IPPROTO_IPV6 && number == IPV6_TCLASS);
}

// Whether `layer` and `number` are IP_MULTICAST_TTL or IP_MULTICAST_LOOP,
// which Linux takes and gives as a byte as well as an int.
bool byte_option(int layer, int number) {
	return layer == IPPROTO_IP && (number == IP_MULTICAST_TTL || number == IP_MULTICAST_LOOP);
}

// The interface IP_MULTICAST_IF, IP_ADD_MEMBERSHIP and IP_DROP_MEMBERSHIP
// name in `size` bytes of `buffer`, as Linux reads them: an ip_mreqn's
// imr_ifindex, else its imr_address, an ip_mreq's imr_interface, or an
// in_addr (for IP_MULTICAST_IF, `bare`), an address of the interface;
// with the group, the ip_mreqn's or ip_mreq's first address, in `group`
// if it is not null. 0 for INADDR_ANY, the interface the route to the
// group goes through. EINVAL if it is too short, and EADDRNOTAVAIL (as
// ENODEV for a membership) if no interface has the address.
int interface_of(const void *buffer, socklen_t size, bool bare, struct in_addr *group,
		unsigned int *index) {
	struct ip_mreqn mreqn;
	memset(&mreqn, 0, sizeof(mreqn));
	if(size >= sizeof(struct ip_mreqn))
		memcpy(&mreqn, buffer, sizeof(mreqn));
	else if(size >= sizeof(struct ip_mreq))
		memcpy(&mreqn, buffer, sizeof(struct ip_mreq));
	else if(bare && size >= sizeof(struct in_addr))
		memcpy(&mreqn.imr_address, buffer, sizeof(struct in_addr));
	else
		return EINVAL;
	if(group)
		*group = mreqn.imr_multiaddr;
	if(mreqn.imr_ifindex) {
		*index = static_cast<unsigned int>(mreqn.imr_ifindex);
		return 0;
	}
	*index = 0;
	if(mreqn.imr_address.s_addr == htonl(INADDR_ANY))
		return 0;
	*index = radix::interface_with(mreqn.imr_address.s_addr);
	return *index ? 0 : (bare ? EADDRNOTAVAIL : ENODEV);
}

// Joins or leaves, as `join`, the group of `size` bytes of `buffer` on
// network socket `slot`: an ip_mreqn or ip_mreq for IP_ADD_MEMBERSHIP and
// IP_DROP_MEMBERSHIP (`v6` false), an ipv6_mreq for IPV6_ADD_MEMBERSHIP
// and IPV6_DROP_MEMBERSHIP. Joining hears the machines on the networks
// this one is on, which a network capability with the local right allows
// (ADR 0337). An IPv6 socket takes an IPv4 group as an IPv4-mapped one.
int socket_group(uint32_t slot, bool join, bool v6, const void *buffer, socklen_t size) {
	struct radix_address group;
	memset(&group, 0, sizeof(group));
	unsigned int index;
	if(v6) {
		if(size < sizeof(struct ipv6_mreq))
			return EINVAL;
		struct ipv6_mreq mreq;
		memcpy(&mreq, buffer, sizeof(mreq));
		group.family = RADIX_ADDRESS_FAMILY_IPV6;
		memcpy(group.ip, &mreq.ipv6mr_multiaddr, sizeof(mreq.ipv6mr_multiaddr));
		index = mreq.ipv6mr_interface;
	} else {
		struct in_addr address;
		if(int e = interface_of(buffer, size, false, &address, &index); e)
			return e;
		uint64_t family;
		if(int e = get_option(slot, RADIX_SOCKET_OPTION_FAMILY, &family); e)
			return e;
		group.family = static_cast<uint32_t>(family);
		if(family == RADIX_ADDRESS_FAMILY_IPV6) {
			group.ip[10] = group.ip[11] = 0xff;
			memcpy(group.ip + 12, &address, sizeof(address));
		} else {
			memcpy(group.ip, &address, sizeof(address));
		}
	}
	for(uint32_t i = 0; i < RADIX_STARTUP_NETWORK_SLOTS; i++) {
		uint64_t status = radix_file_socket_group(slot, RADIX_STARTUP_SLOT_NETWORK + i, join,
				&group, sizeof(group), index);
		if(!status)
			return 0;
		// A stream or ping socket has no groups: EPROTO, as Linux's
		// stream sockets say.
		if(is(status, RADIX_KERNEL_NOT_SUPPORTED))
			return EPROTO;
		// No such interface, or none the group's routes go through:
		// ENODEV, as on Linux, where no_device is ENXIO elsewhere.
		if(RADIX_STATUS_DOMAIN(status) == RADIX_FILE_ERROR_DOMAIN
				&& RADIX_STATUS_CODE(status) == RADIX_FILE_NO_DEVICE)
			return ENODEV;
		if(!not_allowed(status))
			return inet_errno(status);
	}
	return EACCES;
}

// getsockopt(2) on network socket `slot`, of type `type`.
int inet_getsockopt(uint32_t slot, int type, int layer, int number, void *buffer,
		socklen_t *size) {
	uint64_t value;
	enum radix_socket_option option;
	bool time = false;
	if(layer == SOL_SOCKET && number == SO_BINDTODEVICE)
		return get_device(slot, buffer, size);
	// SO_LINGER's struct linger, which the kernel keeps as 0 for off and
	// 1 plus the seconds for on (ADR 0496).
	if(layer == SOL_SOCKET && number == SO_LINGER) {
		if(int e = get_option(slot, RADIX_SOCKET_OPTION_LINGER, &value); e)
			return e;
		struct linger linger;
		linger.l_onoff = value != 0;
		linger.l_linger = value ? static_cast<int>(value - 1) : 0;
		socklen_t len = *size < sizeof(linger) ? *size : sizeof(linger);
		memcpy(buffer, &linger, len);
		*size = len;
		return 0;
	}
	// The interface's address, as Linux gives the one it was set to.
	if(layer == IPPROTO_IP && number == IP_MULTICAST_IF) {
		if(int e = get_option(slot, RADIX_SOCKET_OPTION_MULTICAST_INTERFACE, &value); e)
			return e;
		struct in_addr address;
		address.s_addr = radix::interface_address(static_cast<unsigned int>(value));
		socklen_t len = *size < sizeof(address) ? *size : sizeof(address);
		memcpy(buffer, &address, len);
		*size = len;
		return 0;
	}
	if(layer == SOL_SOCKET && (number == SO_TYPE || number == SO_DOMAIN
			|| number == SO_PROTOCOL || number == SO_ACCEPTCONN)) {
		struct radix_file_socket_state_results state;
		if(uint64_t status = radix_file_socket_state(slot, &state); status)
			return inet_errno(status);
		switch(number) {
		case SO_TYPE: value = type; break;
		case SO_DOMAIN:
			if(int e = get_option(slot, RADIX_SOCKET_OPTION_FAMILY, &value); e)
				return e;
			value = value == RADIX_ADDRESS_FAMILY_IPV6 ? AF_INET6 : AF_INET;
			break;
		case SO_PROTOCOL: {
			uint64_t kind, family;
			if(int e = get_option(slot, RADIX_SOCKET_OPTION_KIND, &kind); e)
				return e;
			if(int e = get_option(slot, RADIX_SOCKET_OPTION_FAMILY, &family); e)
				return e;
			if(kind == RADIX_SOCKET_TYPE_PING)
				value = family == RADIX_ADDRESS_FAMILY_IPV6 ? IPPROTO_ICMPV6 : IPPROTO_ICMP;
			else
				value = type == SOCK_DGRAM ? IPPROTO_UDP : IPPROTO_TCP;
			break;
		}
		default: value = state.listening; break;
		}
	} else if(type_of_service(layer, number)) {
		if(int e = get_option(slot, RADIX_SOCKET_OPTION_TYPE_OF_SERVICE, &value); e)
			return e;
	} else if(inet_option(layer, number, &option, &time)) {
		if(int e = get_option(slot, option, &value); e)
			return e;
		// Errors come back from the calls, and SO_ERROR as an errno.
		if(option == RADIX_SOCKET_OPTION_ERROR && value)
			value = static_cast<uint64_t>(radix::to_errno(value));
	} else {
		return ENOPROTOOPT;
	}
	if(time) {
		struct timeval tv;
		tv.tv_sec = static_cast<time_t>(value / 1000000000);
		tv.tv_usec = static_cast<suseconds_t>(value % 1000000000 / 1000);
		socklen_t len = *size < sizeof(tv) ? *size : sizeof(tv);
		memcpy(buffer, &tv, len);
		*size = len;
		return 0;
	}
	// A byte for less room, as Linux gives it.
	if(byte_option(layer, number) && *size > 0 && *size < sizeof(int)) {
		unsigned char byte = static_cast<unsigned char>(value);
		memcpy(buffer, &byte, 1);
		*size = 1;
		return 0;
	}
	if(*size < sizeof(int))
		return EINVAL;
	int as_int = static_cast<int>(value);
	memcpy(buffer, &as_int, sizeof(int));
	*size = sizeof(int);
	return 0;
}

// setsockopt(2) on network socket `slot`.
int inet_setsockopt(uint32_t slot, int layer, int number, const void *buffer, socklen_t size) {
	if(layer == SOL_SOCKET && number == SO_BINDTODEVICE)
		return set_device(slot, buffer, size);
	// A linger of 0 seconds resets the connection on close; a longer one
	// is kept, and the close still returns at once (ADR 0496). A negative
	// one is as long as can be, as Linux takes it.
	if(layer == SOL_SOCKET && number == SO_LINGER) {
		struct linger linger;
		if(size < sizeof(linger))
			return EINVAL;
		memcpy(&linger, buffer, sizeof(linger));
		uint64_t seconds = linger.l_linger < 0 ? INT_MAX : static_cast<uint64_t>(linger.l_linger);
		return set_option(slot, RADIX_SOCKET_OPTION_LINGER, linger.l_onoff ? 1 + seconds : 0);
	}
	if(layer == IPPROTO_IP && number == IP_MULTICAST_IF) {
		unsigned int index;
		if(int e = interface_of(buffer, size, true, nullptr, &index); e)
			return e;
		return set_option(slot, RADIX_SOCKET_OPTION_MULTICAST_INTERFACE, index);
	}
	if(layer == IPPROTO_IP && (number == IP_ADD_MEMBERSHIP || number == IP_DROP_MEMBERSHIP))
		return socket_group(slot, number == IP_ADD_MEMBERSHIP, false, buffer, size);
	if(layer == IPPROTO_IPV6
			&& (number == IPV6_ADD_MEMBERSHIP || number == IPV6_DROP_MEMBERSHIP))
		return socket_group(slot, number == IPV6_ADD_MEMBERSHIP, true, buffer, size);
	// The type of service and traffic class are taken and read back, as
	// on Linux, though every packet still goes with 0 (ADR 0345, ADR 0410):
	// IPV6_TCLASS takes -1 for the default and 0 to 255, IP_TOS any int,
	// keeping its low byte.
	if(type_of_service(layer, number)) {
		int as_int;
		if(size < sizeof(int))
			return EINVAL;
		memcpy(&as_int, buffer, sizeof(int));
		if(layer == IPPROTO_IPV6) {
			if(as_int == -1)
				as_int = 0;
			else if(as_int < 0 || as_int > 255)
				return EINVAL;
		} else {
			as_int &= 0xff;
		}
		return set_option(slot, RADIX_SOCKET_OPTION_TYPE_OF_SERVICE, static_cast<uint64_t>(as_int));
	}
	enum radix_socket_option option;
	bool time;
	if(!inet_option(layer, number, &option, &time))
		return ENOPROTOOPT;
	uint64_t value;
	if(time) {
		struct timeval tv;
		if(size < sizeof(tv))
			return EINVAL;
		memcpy(&tv, buffer, sizeof(tv));
		if(tv.tv_sec < 0 || tv.tv_usec < 0 || tv.tv_usec >= 1000000)
			return EDOM;
		value = static_cast<uint64_t>(tv.tv_sec) * 1000000000
				+ static_cast<uint64_t>(tv.tv_usec) * 1000;
	} else {
		int as_int;
		if(byte_option(layer, number) && size > 0 && size < sizeof(int)) {
			unsigned char byte;
			memcpy(&byte, buffer, 1);
			as_int = byte;
		} else {
			if(size < sizeof(int))
				return EINVAL;
			memcpy(&as_int, buffer, sizeof(int));
		}
		switch(option) {
		// A time to live or hop limit of -1 is the default, as on Linux.
		case RADIX_SOCKET_OPTION_TIME_TO_LIVE:
			if(as_int == -1)
				as_int = 64;
			if(as_int < 1 || as_int > 255)
				return EINVAL;
			break;
		// A group's datagrams go no further than the network by default,
		// and 0 keeps them on this machine (ADR 0336).
		case RADIX_SOCKET_OPTION_MULTICAST_HOPS:
			if(as_int == -1)
				as_int = 1;
			if(as_int < 0 || as_int > 255)
				return EINVAL;
			break;
		// IPv6's takes 0 and 1 alone; IPv4's any number, as on Linux.
		case RADIX_SOCKET_OPTION_MULTICAST_LOOP:
			if(layer == IPPROTO_IPV6 && as_int != 0 && as_int != 1)
				return EINVAL;
			as_int = as_int != 0;
			break;
		case RADIX_SOCKET_OPTION_REUSE_PORT:
		case RADIX_SOCKET_OPTION_PACKET_INFO:
		case RADIX_SOCKET_OPTION_RECEIVE_ERRORS: as_int = as_int != 0; break;
		case RADIX_SOCKET_OPTION_ERROR: return ENOPROTOOPT;
		default: break;
		}
		value = static_cast<uint64_t>(as_int < 0 ? 0 : as_int);
	}
	return set_option(slot, option, value);
}

} // namespace

namespace mlibc {

int Sysdeps<Socket>::operator()(int family, int type_and_flags, int protocol, int *fd) {
	int type = type_and_flags & ~(SOCK_NONBLOCK | SOCK_CLOEXEC);
	if(family == AF_INET || family == AF_INET6) {
		enum radix_socket_type kind;
		switch(type) {
		case SOCK_STREAM:
			kind = RADIX_SOCKET_TYPE_STREAM;
			if(protocol != 0 && protocol != IPPROTO_TCP)
				return EPROTONOSUPPORT;
			break;
		case SOCK_DGRAM:
			kind = RADIX_SOCKET_TYPE_DATAGRAM;
			// Linux's ping sockets, which send ICMP echo requests and take
			// their replies, of their version of IP (ADR 0336).
			if((family == AF_INET && protocol == IPPROTO_ICMP)
					|| (family == AF_INET6 && protocol == IPPROTO_ICMPV6))
				kind = RADIX_SOCKET_TYPE_PING;
			else if(protocol != 0 && protocol != IPPROTO_UDP)
				return EPROTONOSUPPORT;
			break;
		// Raw sockets need a right of their own; ping(8) needs none, as it
		// uses a ping socket.
		case SOCK_RAW: return EPERM;
		case SOCK_SEQPACKET: return ESOCKTNOSUPPORT;
		default: return EINVAL;
		}
		uint32_t flags = description_flags(type_and_flags);
		for(uint32_t i = 0; i < RADIX_STARTUP_NETWORK_SLOTS; i++) {
			bool refused = false;
			int e = radix::place(0, fd, [&](uint32_t slot) {
				uint64_t status = radix_network_socket(RADIX_STARTUP_SLOT_NETWORK + i, slot,
						family == AF_INET6 ? RADIX_ADDRESS_FAMILY_IPV6
								: RADIX_ADDRESS_FAMILY_IPV4,
						kind, flags);
				refused = not_allowed(status);
				return status;
			});
			// Refused by this capability: the next one may allow it.
			if(!refused)
				return e;
		}
		return EACCES;
	}
	if(family != AF_UNIX)
		return EAFNOSUPPORT;
	switch(type) {
	case SOCK_STREAM: break;
	// Linux's AF_UNIX has these, which Radix does not yet.
	case SOCK_DGRAM:
	case SOCK_SEQPACKET: return ESOCKTNOSUPPORT;
	default: return EINVAL;
	}
	if(protocol != 0 && protocol != PF_UNIX)
		return EPROTONOSUPPORT;
	uint32_t flags = description_flags(type_and_flags);
	return radix::place(0, fd, [&](uint32_t slot) {
		return radix_process_socket(radix::SLOT_PROCESS, slot, flags);
	});
}

int Sysdeps<Socketpair>::operator()(int domain, int type_and_flags, int proto, int *fds) {
	if(domain != AF_UNIX)
		return EAFNOSUPPORT;
	int type = type_and_flags & ~(SOCK_NONBLOCK | SOCK_CLOEXEC);
	switch(type) {
	case SOCK_STREAM: break;
	case SOCK_DGRAM:
	case SOCK_SEQPACKET: return ESOCKTNOSUPPORT;
	default: return EINVAL;
	}
	if(proto != 0 && proto != PF_UNIX)
		return EPROTONOSUPPORT;
	return make_pair(description_flags(type_and_flags), fds);
}

int Sysdeps<Bind>::operator()(int fd, const struct sockaddr *addr, socklen_t length) {
	uint32_t slot;
	if(int e = slot_of(fd, &slot); e)
		return e;
	int type;
	if(is_inet(slot, &type)) {
		struct radix_address address;
		if(int e = inet_address(addr, length, &address); e)
			return e;
		// A stream socket bound to a port the kernel picks is likely to
		// listen next (a server that prints its own address, `nc -l 0`),
		// so it is bound through a capability that may receive, if there
		// is one. Any capability allows a bind to port 0, and the first
		// one, often a send-only one, would leave it unable to listen. A
		// signed-in session holds the send right first and the receive
		// right from port 1024 up after it.
		if(type == SOCK_STREAM && address.port == 0) {
			if(int e = bind_receiving(slot, &address); e != EACCES)
				return e;
		}
		return through_network([&](uint32_t network) {
			return radix_file_bind_address(slot, network, &address, sizeof(address));
		});
	}
	char name[sizeof(sockaddr_un::sun_path) + 1];
	size_t len;
	if(int e = unix_path(addr, length, name, &len); e)
		return e;
	radix::Path path;
	if(int e = radix::resolve(AT_FDCWD, name, &path); e)
		return e;
	uint64_t status = radix_directory_bind_socket(path.dir, path.text, path.len, slot, name, len);
	// A file of that name is there already.
	if(RADIX_STATUS_DOMAIN(status) == RADIX_FILE_ERROR_DOMAIN
			&& RADIX_STATUS_CODE(status) == RADIX_FILE_EXISTS)
		return EADDRINUSE;
	if(is(status, RADIX_KERNEL_INVALID_ARGUMENT))
		return EINVAL;
	return status ? socket_errno(status) : 0;
}

int Sysdeps<Listen>::operator()(int fd, int backlog) {
	uint32_t slot;
	if(int e = slot_of(fd, &slot); e)
		return e;
	// As on Linux, a backlog out of range is the most there may be.
	uint32_t most = backlog < 0 || static_cast<uint64_t>(backlog) > RADIX_FILE_BACKLOG_MAX
			? static_cast<uint32_t>(RADIX_FILE_BACKLOG_MAX) : static_cast<uint32_t>(backlog);
	if(is_inet(slot)) {
		uint64_t status = radix_file_listen(slot, most);
		// Unbound, it is bound to a port the kernel picks first, as on
		// Linux.
		if(not_allowed(status)) {
			struct radix_address bound;
			struct radix_file_socket_name_results r;
			if(!radix_file_socket_name(slot, false, &bound, sizeof(bound), &r) && !bound.port) {
				if(int e = bind_to_listen(slot, bound.family); e)
					return e;
				status = radix_file_listen(slot, most);
			}
		}
		return status ? inet_errno(status) : 0;
	}
	uint64_t status = radix_file_listen(slot, most);
	return status ? socket_errno(status) : 0;
}

int Sysdeps<Connect>::operator()(int fd, const struct sockaddr *addr, socklen_t length) {
	uint32_t slot;
	if(int e = slot_of(fd, &slot); e)
		return e;
	if(is_inet(slot)) {
		struct radix_address address;
		if(int e = inet_address(addr, length, &address); e)
			return e;
		// A cancellation point, as accept is.
		return through_network([&](uint32_t network) {
			return radix_file_connect_address_with(radix::cancellable, slot, network, &address,
					sizeof(address));
		});
	}
	char name[sizeof(sockaddr_un::sun_path) + 1];
	size_t len;
	if(int e = unix_path(addr, length, name, &len); e)
		return e;
	radix::Path path;
	if(int e = radix::resolve(AT_FDCWD, name, &path); e)
		return e;
	// A cancellation point, as accept is.
	uint64_t status = radix_directory_connect_socket_with(radix::cancellable, path.dir, path.text,
			path.len, slot);
	// Another thread's connect is under way.
	if(is(status, RADIX_KERNEL_BUSY))
		return EALREADY;
	return status ? socket_errno(status) : 0;
}

int Sysdeps<Accept>::operator()(int fd, int *newfd, struct sockaddr *addr, socklen_t *length,
		int flags) {
	uint32_t slot;
	if(int e = slot_of(fd, &slot); e)
		return e;
	if(flags & ~(SOCK_NONBLOCK | SOCK_CLOEXEC))
		return EINVAL;
	uint32_t kernel = description_flags(flags);
	for(;;) {
		radix::SlotClaim claim;
		if(int e = radix::free_slot(0, &claim); e)
			return e;
		uint32_t to = claim.slot();
		uint64_t status = radix_file_accept_with(radix::cancellable, slot, to, kernel);
		// Something else filled the slot first: it stays taken.
		if(is(status, RADIX_KERNEL_OCCUPIED)) {
			claim.keep();
			continue;
		}
		if(status)
			return socket_errno(status);
		claim.keep();
		*newfd = static_cast<int>(to - radix::FD_BASE);
		break;
	}
	if(addr && length) {
		socklen_t actual;
		if(address(*newfd, true, addr, *length, &actual))
			actual = 0;
		*length = actual;
	}
	return 0;
}

int Sysdeps<Sendto>::operator()(int fd, const void *buffer, size_t size, int flags,
		const struct sockaddr *sock_addr, socklen_t addr_length, ssize_t *length) {
	// send(2) is sendto with no address: the kernel's send is the same
	// call for both families, so the family is asked for only with an
	// address, as these calls are made often.
	if(sock_addr && addr_length) {
		uint32_t slot;
		if(int e = slot_of(fd, &slot); e)
			return e;
		if(is_inet(slot))
			return send_inet(slot, buffer, size, flags, sock_addr, addr_length, length);
		// A stream socket sends to the socket it is connected to: an
		// address is one too many, as on Linux.
		if(int e = check_socket(fd); e)
			return e;
		return EISCONN;
	}
	size_t sent;
	if(int e = send_bytes(radix::cancellable, fd, buffer, size, flags, nullptr, 0, &sent); e)
		return e;
	*length = static_cast<ssize_t>(sent);
	return 0;
}

int Sysdeps<MsgSend>::operator()(int fd, const struct msghdr *hdr, int flags, ssize_t *length) {
	// The family is asked for only when it changes what is sent, as in
	// sendto.
	bool control = hdr->msg_control && hdr->msg_controllen >= sizeof(struct cmsghdr);
	uint32_t slot;
	if(int e = slot_of(fd, &slot); e)
		return e;
	int type;
	if(((hdr->msg_name && hdr->msg_namelen) || control || hdr->msg_iovlen > 1)
			&& is_inet(slot, &type)) {
		// Descriptors go only over Unix sockets.
		if(control)
			return EOPNOTSUPP;
		if(hdr->msg_iovlen < 0 || hdr->msg_iovlen > IOV_MAX)
			return EMSGSIZE;
		size_t sum;
		if(int e = radix::iov_total(hdr->msg_iov, hdr->msg_iovlen, &sum); e)
			return e;
		auto to = hdr->msg_namelen ? static_cast<const struct sockaddr *>(hdr->msg_name)
				: nullptr;
		// A datagram goes in one send, and so do a stream's bytes of one
		// buffer.
		if(hdr->msg_iovlen == 1 || (type == SOCK_STREAM && hdr->msg_iovlen == 0))
			return send_inet(slot, hdr->msg_iovlen ? hdr->msg_iov[0].iov_base : nullptr,
					sum, flags, to, hdr->msg_namelen, length);
		char *gathered;
		if(int e = gather(hdr->msg_iov, hdr->msg_iovlen, sum, &gathered); e)
			return e;
		int e = send_inet(slot, gathered, sum, flags, to, hdr->msg_namelen, length);
		free(gathered);
		return e;
	}
	if(hdr->msg_name && hdr->msg_namelen) {
		if(int e = check_socket(fd); e)
			return e;
		return EISCONN;
	}
	uint32_t slots[RADIX_FILE_SEND_FILES_MAX];
	size_t files;
	if(int e = rights_of(hdr, slots, &files); e)
		return e;
	size_t sum;
	// POSIX's int, which Linux reads as an unsigned count: a negative
	// one is too many.
	if(hdr->msg_iovlen < 0 || hdr->msg_iovlen > IOV_MAX)
		return EMSGSIZE;
	if(int e = radix::iov_total(hdr->msg_iov, hdr->msg_iovlen, &sum); e)
		return e;
	// Descriptors go with a byte, and a stream with no bytes sends none,
	// as on Linux.
	if(!sum) {
		if(int e = check_socket(fd); e)
			return e;
		thread_testcancel();
		*length = 0;
		return 0;
	}
	size_t sent;
	// Up to PIPE_BUF bytes go in one send, which the other end gets whole,
	// never mixed with another sender's bytes, as writev does on a pipe.
	if(hdr->msg_iovlen > 1 && sum <= PIPE_BUF) {
		char gathered[PIPE_BUF];
		size_t at = 0;
		for(int i = 0; i < hdr->msg_iovlen; i++) {
			memcpy(gathered + at, hdr->msg_iov[i].iov_base, hdr->msg_iov[i].iov_len);
			at += hdr->msg_iov[i].iov_len;
		}
		if(int e = send_bytes(radix::cancellable, fd, gathered, sum, flags, slots, files, &sent);
				e)
			return e;
		*length = static_cast<ssize_t>(sent);
		return 0;
	}
	size_t total = 0;
	for(int i = 0; i < hdr->msg_iovlen; i++) {
		if(!hdr->msg_iov[i].iov_len)
			continue;
		// The descriptors go with the first bytes, and only the first send
		// can act on a cancel, as writev's first write.
		if(int e = send_bytes(total ? radix_syscall : radix::cancellable, fd,
				hdr->msg_iov[i].iov_base, hdr->msg_iov[i].iov_len, flags,
				total ? nullptr : slots, total ? 0 : files, &sent); e) {
			if(total)
				break;
			return e;
		}
		total += sent;
		if(sent < hdr->msg_iov[i].iov_len)
			break;
	}
	*length = static_cast<ssize_t>(total);
	return 0;
}

int Sysdeps<Recvfrom>::operator()(int fd, void *buffer, size_t size, int flags,
		struct sockaddr *sock_addr, socklen_t *addr_length, ssize_t *length) {
	struct iovec iov = {buffer, size};
	uint32_t slot;
	if(int e = slot_of(fd, &slot); e)
		return e;
	// recv(2) is recvfrom with no address; a network datagram socket's
	// receive takes one datagram, so the family is asked for only when the
	// sender is, as in sendto.
	int type;
	// The ICMP errors about what a network socket sent (ADR 0345).
	if((flags & MSG_ERRQUEUE) && is_inet(slot, &type))
		return receive_error(slot, &iov, 1, sock_addr, addr_length ? *addr_length : 0,
				addr_length, length, nullptr);
	// MSG_TRUNC, which only a datagram's receive takes, asks for the
	// family too.
	if((addr_length || (flags & MSG_TRUNC)) && is_inet(slot, &type)) {
		if(type == SOCK_DGRAM)
			return receive_datagram(slot, &iov, 1, flags, sock_addr,
					addr_length ? *addr_length : 0, addr_length, length);
		// A stream's bytes say nothing of where they came from, as on
		// Linux.
		if(int e = receive_vector(fd, &iov, 1, flags, nullptr, length); e)
			return e;
		if(addr_length)
			*addr_length = 0;
		return 0;
	}
	if(int e = receive_vector(fd, &iov, 1, flags, nullptr, length); e)
		return e;
	// The sender is the socket at the other end.
	if(addr_length) {
		socklen_t actual;
		if(sender(fd, sock_addr, *addr_length, &actual))
			actual = 0;
		*addr_length = actual;
	}
	return 0;
}

int Sysdeps<MsgRecv>::operator()(int fd, struct msghdr *hdr, int flags, ssize_t *length) {
	size_t sum;
	// POSIX's int, which Linux reads as an unsigned count: a negative
	// one is too many.
	if(hdr->msg_iovlen < 0 || hdr->msg_iovlen > IOV_MAX)
		return EMSGSIZE;
	uint32_t slot;
	if(int e = slot_of(fd, &slot); e)
		return e;
	// The family is asked for only when it changes what is received, as
	// in recvfrom.
	bool asks = (hdr->msg_name && hdr->msg_namelen) || hdr->msg_iovlen > 1
			|| (hdr->msg_control && hdr->msg_controllen);
	int type;
	// The ICMP errors about what a network socket sent (ADR 0345).
	if((flags & MSG_ERRQUEUE) && is_inet(slot, &type)) {
		auto from = static_cast<struct sockaddr *>(hdr->msg_name);
		socklen_t from_len = hdr->msg_namelen;
		if(int e = receive_error(slot, hdr->msg_iov, hdr->msg_iovlen, from, from_len,
				&from_len, length, hdr); e)
			return e;
		hdr->msg_namelen = from ? from_len : 0;
		return 0;
	}
	// A network socket's receive: one datagram, or a stream's bytes.
	auto inet = [&]() -> int {
		auto from = static_cast<struct sockaddr *>(hdr->msg_name);
		socklen_t from_len = hdr->msg_namelen;
		int e;
		if(type == SOCK_DGRAM) {
			e = receive_datagram(slot, hdr->msg_iov, hdr->msg_iovlen, flags, from, from_len,
					&from_len, length, hdr);
			if(e)
				return e;
		} else {
			e = receive_vector(fd, hdr->msg_iov, hdr->msg_iovlen,
					flags & ~MSG_CMSG_CLOEXEC, nullptr, length);
			if(e)
				return e;
			from_len = 0;
			hdr->msg_flags = 0;
			// Nothing comes with a stream's bytes.
			hdr->msg_controllen = 0;
		}
		hdr->msg_namelen = from ? from_len : 0;
		return 0;
	};
	// MSG_TRUNC, which only a datagram's receive takes, asks too.
	if((asks || (flags & MSG_TRUNC)) && is_inet(slot, &type))
		return inet();
	if(int e = radix::iov_total(hdr->msg_iov, hdr->msg_iovlen, &sum); e)
		return e;
	// Room for as many descriptors as the control buffer holds: those
	// with nowhere to go are closed, and MSG_CTRUNC says so.
	uint32_t slots[RADIX_FILE_SEND_FILES_MAX];
	Received files = {slots, 0, 0, false};
	if(hdr->msg_control && hdr->msg_controllen >= CMSG_LEN(sizeof(int))) {
		files.room = (hdr->msg_controllen - CMSG_LEN(0)) / sizeof(int);
		if(files.room > RADIX_FILE_SEND_FILES_MAX)
			files.room = RADIX_FILE_SEND_FILES_MAX;
	}
	if(int e = receive_vector(fd, hdr->msg_iov, hdr->msg_iovlen, flags, &files, length); e) {
		// No files ride on a network socket's bytes: one that was not
		// asked about, as nothing in `hdr` asked, is received as one.
		if(e == EOPNOTSUPP && is_inet(slot, &type))
			return inet();
		return e;
	}
	hdr->msg_flags = 0;
	if(hdr->msg_name && hdr->msg_namelen) {
		socklen_t actual;
		if(sender(fd, static_cast<struct sockaddr *>(hdr->msg_name), hdr->msg_namelen,
				&actual))
			actual = 0;
		hdr->msg_namelen = actual;
	} else {
		hdr->msg_namelen = 0;
	}
	give_rights(hdr, files);
	return 0;
}

int Sysdeps<Shutdown>::operator()(int sockfd, int how) {
	uint32_t slot;
	if(int e = slot_of(sockfd, &slot); e)
		return e;
	if(how != SHUT_RD && how != SHUT_WR && how != SHUT_RDWR)
		return EINVAL;
	uint64_t status = radix_file_shutdown(slot, how != SHUT_WR, how != SHUT_RD);
	return status ? socket_errno(status) : 0;
}

int Sysdeps<GetSockopt>::operator()(int fd, int layer, int number, void *__restrict buffer,
		socklen_t *__restrict size) {
	uint32_t slot;
	if(int e = slot_of(fd, &slot); e)
		return e;
	if(int type; is_inet(slot, &type))
		return inet_getsockopt(slot, type, layer, number, buffer, size);
	struct radix_file_socket_state_results state;
	if(uint64_t status = radix_file_socket_state(slot, &state); status)
		return socket_errno(status);
	if(layer != SOL_SOCKET)
		return ENOPROTOOPT;
	if(number == SO_PEERCRED) {
		// The process at the other end, and the user and group it acted
		// for, as they were when the connection was made (ADR 0218).
		struct ucred cred;
		cred.pid = state.connected ? static_cast<pid_t>(state.peer) : 0;
		cred.uid = state.connected ? static_cast<uid_t>(state.user) : static_cast<uid_t>(-1);
		cred.gid = state.connected ? static_cast<gid_t>(state.group) : static_cast<gid_t>(-1);
		socklen_t len = *size < sizeof(cred) ? *size : sizeof(cred);
		memcpy(buffer, &cred, len);
		*size = len;
		return 0;
	}
	int value;
	switch(number) {
	case SO_TYPE: value = SOCK_STREAM; break;
	case SO_DOMAIN: value = AF_UNIX; break;
	case SO_PROTOCOL: value = 0; break;
	// Errors come back from each call, so none waits here.
	case SO_ERROR: value = 0; break;
	case SO_ACCEPTCONN: value = state.listening; break;
	// Each way has a pipe's buffer (radix-posix(7)).
	case SO_RCVBUF:
	case SO_SNDBUF: value = 65536; break;
	default: return ENOPROTOOPT;
	}
	if(*size < sizeof(int))
		return EINVAL;
	memcpy(buffer, &value, sizeof(int));
	*size = sizeof(int);
	return 0;
}

int Sysdeps<SetSockopt>::operator()(int fd, int layer, int number, const void *buffer,
		socklen_t size) {
	uint32_t slot;
	if(int e = slot_of(fd, &slot); e)
		return e;
	if(is_inet(slot))
		return inet_setsockopt(slot, layer, number, buffer, size);
	if(int e = check_socket(fd); e)
		return e;
	if(layer != SOL_SOCKET)
		return ENOPROTOOPT;
	switch(number) {
	// Taken, and nothing to do: the buffers have one size, as Linux takes
	// a size as a hint, and a local socket has no address to reuse and no
	// connection to keep alive.
	case SO_RCVBUF:
	case SO_SNDBUF:
	case SO_REUSEADDR:
	case SO_KEEPALIVE:
		if(size < sizeof(int))
			return EINVAL;
		return 0;
	default: return ENOPROTOOPT;
	}
}

int Sysdeps<Sockname>::operator()(int fd, struct sockaddr *addr_ptr, socklen_t max_addr_length,
		socklen_t *actual_length) {
	return address(fd, false, addr_ptr, max_addr_length, actual_length);
}

int Sysdeps<Peername>::operator()(int fd, struct sockaddr *addr_ptr, socklen_t max_addr_length,
		socklen_t *actual_length) {
	return address(fd, true, addr_ptr, max_addr_length, actual_length);
}

} // namespace mlibc

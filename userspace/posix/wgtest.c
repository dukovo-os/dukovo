// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The network-wireguard boot test's program (ADRs 0339 and 0340): two
// machines on a cable, 192.168.50.1 and .2 and fd50::1 and ::2, each
// with wgd(8) and a WireGuard interface wg0, 10.9.0.1 and .2 and fd09::1
// and ::2, whose files this program writes before it has wgd read them
// again. Machine a, `wgtest a`, listens on port 51820 and knows no
// endpoint for b, which it learns from b's first packet; machine b,
// `wgtest b`, knows a's, with a keepalive. b connects to a's control
// port through the tunnel, by IPv4, and does what a asks, a line each:
//
// - `probe ADDRESS PORT`: connects there, and says `open`, `refused`,
//   or `blocked` when nothing answers within PROBE_MS;
// - `fetch ADDRESS PORT`: says `ok` and the first line it reads there;
// - `ping ADDRESS`: pings it once with ping(8), and says `ok` or
//   `failed`;
// - `done`: says `bye` and ends.
//
// a checks the tunnel's link as ip(8) and /kernel/class/net show it,
// wg(8)'s keys and its show, dump, showconf and set; that b reaches a's
// page through the tunnel by IPv4 and IPv6 and pings it; that a reload
// of wgd keeps the session, its handshake the same; and that a refuses
// b's packets from an address b's AllowedIPs leave out, until they hold
// it again. The cable's dump must hold WireGuard's datagrams and never
// the page's words, which went inside them.

#include <arpa/inet.h>
#include <fcntl.h>
#include <netinet/in.h>
#include <poll.h>
#include <sys/socket.h>

#include "check.h"

// a's page, a line, and its control port.
#define PAGE_PORT 8080
#define CONTROL_PORT 7000
#define PROBE_MS 5000
#define PAGE_LINE "wgtest: a page that went through the tunnel"
#define PAGE PAGE_LINE "\n"

// The test's keys, made for it alone: a's, b's, and their preshared key.
#define A_PRIVATE "sAPO99YlIn6rFw7EJ1jEK36VpObw0F9wNNr07mAcPFc="
#define A_PUBLIC "9wjCu3Ap0zMQ2K+E+hjKuatuKFDsu29PFtTDfAyPzmQ="
#define B_PRIVATE "kH6cWsCWgR8luNZBb6lX5PA87i9q3efT2wqZ6rJP9lw="
#define B_PUBLIC "Yk5tuO5DdugKFEOy7wALrLWQep2CP3Oer3BZwoRgeWE="
#define PRESHARED "Ic0f0sbv72X5qelW36tWvlS4sCb9n7ul9DVw+W+bL2w="

#define CONF "/system/state/wireguard/wg0.conf"

// Runs `command` with sh(1), saying so: its exit code, or -1.
static int run(const char *command) {
	printf("wgtest: $ %s\n", command);
	fflush(stdout);
	int status = system(command);
	return WIFEXITED(status) ? WEXITSTATUS(status) : -1;
}

// Runs `command` with sh(1) and keeps what it writes, its standard error
// too, in `buffer`: its exit code, or -1.
static int output(const char *command, char *buffer, size_t size) {
	char line[512];
	snprintf(line, sizeof(line), "%s 2>&1", command);
	FILE *pipe = popen(line, "r");
	if(!pipe) {
		buffer[0] = 0;
		return -1;
	}
	size_t length = fread(buffer, 1, size - 1, pipe);
	buffer[length] = 0;
	int status = pclose(pipe);
	return WIFEXITED(status) ? WEXITSTATUS(status) : -1;
}

// Runs `command` until what it writes holds `needle`, for at most
// `seconds`: whether it did.
static int wait_for(const char *command, const char *needle, int seconds) {
	static char buffer[8192];
	long long deadline = check_now_ns() + seconds * 1000000000LL;
	while(check_now_ns() < deadline) {
		if(output(command, buffer, sizeof(buffer)) == 0 && strstr(buffer, needle))
			return 1;
		check_sleep_ms(500);
	}
	printf("wgtest: `%s` never said \"%s\"; it said: %s\n", command, needle, buffer);
	return 0;
}

// Whether what `command` writes holds `needle`, saying what it wrote.
static int says(const char *command, const char *needle) {
	static char buffer[8192];
	int code = output(command, buffer, sizeof(buffer));
	printf("wgtest: $ %s\n%s", command, buffer);
	if(code != 0 || !strstr(buffer, needle)) {
		printf("wgtest: `%s` (exit %d) did not say \"%s\"\n", command, code, needle);
		return 0;
	}
	return 1;
}

// Writes `text` into `path`, for the system alone.
static void write_file(const char *path, const char *text) {
	int fd = open(path, O_WRONLY | O_CREAT | O_TRUNC, 0600);
	CHECK(fd >= 0);
	if(fd < 0)
		return;
	CHECK(write(fd, text, strlen(text)) == (ssize_t)strlen(text));
	CHECK(close(fd) == 0);
}

// A TCP socket listening on every address, IPv4 and IPv6, at `port`.
static int listening(unsigned short port) {
	int fd = socket(AF_INET6, SOCK_STREAM, 0);
	CHECK(fd >= 0);
	int yes = 1;
	setsockopt(fd, SOL_SOCKET, SO_REUSEADDR, &yes, sizeof(yes));
	struct sockaddr_in6 at;
	memset(&at, 0, sizeof(at));
	at.sin6_family = AF_INET6;
	at.sin6_port = htons(port);
	at.sin6_addr = in6addr_any;
	CHECK(bind(fd, (struct sockaddr *)&at, sizeof(at)) == 0);
	CHECK(listen(fd, 8) == 0);
	return fd;
}

// Connects to `text` at `port`, waiting at most `ms`: the socket, or -1
// with `what` saying why: "refused", "blocked" or "error".
static int connect_to(const char *text, unsigned short port, int ms, const char **what) {
	struct sockaddr_storage to;
	memset(&to, 0, sizeof(to));
	struct sockaddr_in6 *six = (struct sockaddr_in6 *)&to;
	struct sockaddr_in *four = (struct sockaddr_in *)&to;
	socklen_t length = 0;
	if(inet_pton(AF_INET6, text, &six->sin6_addr) == 1) {
		six->sin6_family = AF_INET6;
		six->sin6_port = htons(port);
		length = sizeof(*six);
	} else if(inet_pton(AF_INET, text, &four->sin_addr) == 1) {
		four->sin_family = AF_INET;
		four->sin_port = htons(port);
		length = sizeof(*four);
	}
	*what = "error";
	if(!length)
		return -1;
	int fd = socket(to.ss_family, SOCK_STREAM | SOCK_NONBLOCK, 0);
	if(fd < 0)
		return -1;
	if(connect(fd, (struct sockaddr *)&to, length) < 0 && errno != EINPROGRESS) {
		*what = errno == ECONNREFUSED ? "refused" : "error";
		close(fd);
		return -1;
	}
	struct pollfd p = {fd, POLLOUT, 0};
	if(poll(&p, 1, ms) == 0) {
		*what = "blocked";
		close(fd);
		return -1;
	}
	int error = 0;
	socklen_t size = sizeof(error);
	getsockopt(fd, SOL_SOCKET, SO_ERROR, &error, &size);
	if(error) {
		*what = error == ECONNREFUSED ? "refused" : "error";
		close(fd);
		return -1;
	}
	fcntl(fd, F_SETFL, fcntl(fd, F_GETFL) & ~O_NONBLOCK);
	*what = "open";
	return fd;
}

// Answers each connection to `server` with PAGE, until killed.
static void answer(int server) {
	for(;;) {
		int fd = accept(server, NULL, NULL);
		if(fd < 0) {
			if(errno == EINTR)
				continue;
			_exit(1);
		}
		(void)!write(fd, PAGE, strlen(PAGE));
		shutdown(fd, SHUT_WR);
		char rest[256];
		struct pollfd p = {fd, POLLIN, 0};
		while(poll(&p, 1, 2000) == 1 && read(fd, rest, sizeof(rest)) > 0) {
		}
		close(fd);
	}
}

// The machine's card on the cable, at 192.168.50.N and fd50::N, and its
// WireGuard file, `conf`, which wgd reads again: wg0 is up once it has
// its IPv6 address.
static void set_up(int n, const char *conf) {
	char text[256];
	snprintf(text, sizeof(text), "[net0]\ndhcp = no\naddress = 192.168.50.%d/24\naddress = fd50::%d/64\n",
			n, n);
	FILE *file = fopen("/system/settings/network", "w");
	CHECK(file && fputs(text, file) >= 0 && fclose(file) == 0);
	CHECK(run("service netd reload") == 0);
	snprintf(text, sizeof(text), "inet6 fd50::%d/64 scope global", n);
	CHECK(wait_for("ip -6 address show dev net0", text, 60));
	write_file(CONF, conf);
	CHECK(run("service wgd reload") == 0);
	snprintf(text, sizeof(text), "inet6 fd09::%d/64 scope global", n);
	CHECK(wait_for("ip -6 address show dev wg0", text, 60));
	CHECK(run("ip address show") == 0);
}

// Machine a's connection to machine b, a stream each way.
static FILE *control_in, *control_out;

// Asks b to do `command`: whether it answered `wanted` (a prefix).
static int ask(const char *command, const char *wanted) {
	printf("wgtest: asking b: %s\n", command);
	fflush(stdout);
	fprintf(control_out, "%s\n", command);
	fflush(control_out);
	char line[512];
	if(!fgets(line, sizeof(line), control_in)) {
		printf("wgtest: FAILED: b did not answer `%s`\n", command);
		check_failures++;
		return 0;
	}
	line[strcspn(line, "\n")] = 0;
	printf("wgtest: b: %s\n", line);
	if(strncmp(line, wanted, strlen(wanted)) != 0) {
		printf("wgtest: FAILED: b answered `%s` with `%s`, not `%s`\n", command, line, wanted);
		check_failures++;
		return 0;
	}
	return 1;
}

// The link wgd made, as the kernel, ip(8) and wg(8) show it, and wg's
// keys.
static void tunnel_link(void) {
	CHECK(says("cat /kernel/class/net/wg0/type", "65534"));
	CHECK(says("cat /kernel/class/net/wg0/link", "tunnel"));
	CHECK(says("cat /kernel/class/net/wg0/operstate", "unknown"));
	CHECK(says("cat /kernel/class/net/wg0/mtu", "1420"));
	CHECK(says("ip link show wg0", "<POINTOPOINT,NOARP,UP,LOWER_UP> mtu 1420"));
	CHECK(says("ip link show wg0", "link/none"));
	CHECK(says("wg show interfaces", "wg0"));
	CHECK(says("wg show wg0", "public key: " A_PUBLIC));
	CHECK(says("wg show wg0", "listening port: 51820"));
	CHECK(says("wg show wg0", "peer: " B_PUBLIC));
	CHECK(says("wg show wg0", "allowed ips: 10.9.0.2/32, fd09::2/128"));
	CHECK(says("wg show wg0 public-key", A_PUBLIC));
	CHECK(says("echo " B_PRIVATE " | wg pubkey", B_PUBLIC));
	CHECK(says("wg genkey | wg pubkey | wc -c", "45"));
	CHECK(says("wg genpsk | wc -c", "45"));
	printf("wgtest: wg0 is a tunnel, as ip, wg and the kernel say\n");
}

// The latest handshake with b, as `wg show wg0 latest-handshakes` says
// it: seconds since 1970, or 0.
static long long handshake(void) {
	char buffer[512];
	output("wg show wg0 latest-handshakes", buffer, sizeof(buffer));
	const char *tab = strchr(buffer, '\t');
	return tab ? atoll(tab + 1) : 0;
}

// b reaches a's page through the tunnel by IPv4 and IPv6, and pings a;
// a learned b's endpoint from b's packets.
static void through(void) {
	CHECK(ask("fetch 10.9.0.1 8080", "ok " PAGE_LINE));
	CHECK(ask("fetch fd09::1 8080", "ok " PAGE_LINE));
	CHECK(ask("ping 10.9.0.1", "ok"));
	CHECK(ask("ping fd09::1", "ok"));
	CHECK(run("ping -c 2 -W 5 10.9.0.2") == 0);
	CHECK(run("ping -c 2 -W 5 fd09::2") == 0);
	CHECK(says("wg show wg0", "endpoint: 192.168.50.2:"));
	CHECK(says("wg show wg0", "latest handshake: "));
	CHECK(says("wg show wg0", "transfer: "));
	CHECK(says("wg show wg0", "preshared key: (hidden)"));
	CHECK(says("wg show wg0 dump", B_PUBLIC "\t" PRESHARED "\t192.168.50.2:"));
	CHECK(says("wg showconf wg0", "PublicKey = " B_PUBLIC));
	CHECK(handshake() > 0);
	printf("wgtest: b reached a through the tunnel by IPv4 and IPv6\n");
}

// A reload of wgd keeps b's session: no new handshake, and b still
// reaches a.
static void reload(void) {
	long long before = handshake();
	CHECK(before > 0);
	CHECK(run("service wgd reload") == 0);
	check_sleep_ms(2000);
	CHECK(ask("fetch 10.9.0.1 8080", "ok " PAGE_LINE));
	long long after = handshake();
	printf("wgtest: the handshake was at %lld, and is at %lld after the reload\n", before, after);
	CHECK(after == before);
	printf("wgtest: a reload of wgd kept the session\n");
}

// Without fd09::2 in b's AllowedIPs, what b sends from it is refused;
// with it again, it is let in.
static void refused(void) {
	CHECK(run("wg set wg0 peer " B_PUBLIC " allowed-ips 10.9.0.2/32") == 0);
	CHECK(says("wg show wg0 allowed-ips", B_PUBLIC "\t10.9.0.2/32\n"));
	CHECK(ask("probe fd09::1 8080", "blocked"));
	CHECK(ask("fetch 10.9.0.1 8080", "ok " PAGE_LINE));
	CHECK(run("wg set wg0 peer " B_PUBLIC " allowed-ips 10.9.0.2/32,fd09::2/128") == 0);
	CHECK(ask("probe fd09::1 8080", "open"));
	printf("wgtest: a refused b's packets from an address its AllowedIPs left out\n");
}

static int a(void) {
	set_up(1, "[Interface]\n"
			  "PrivateKey = " A_PRIVATE "\n"
			  "ListenPort = 51820\n"
			  "Address = 10.9.0.1/24, fd09::1/64\n"
			  "\n"
			  "[Peer]\n"
			  "PublicKey = " B_PUBLIC "\n"
			  "PresharedKey = " PRESHARED "\n"
			  "AllowedIPs = 10.9.0.2/32, fd09::2/128\n");
	RUN(tunnel_link);
	pid_t page = check_fork();
	if(page == 0)
		answer(listening(PAGE_PORT));
	int server = listening(CONTROL_PORT);
	printf("wgtest: waiting for b on port %d\n", CONTROL_PORT);
	fflush(stdout);
	struct pollfd p = {server, POLLIN, 0};
	CHECK(poll(&p, 1, 300000) == 1);
	int fd = accept(server, NULL, NULL);
	CHECK(fd >= 0);
	close(server);
	if(fd >= 0) {
		control_in = fdopen(fd, "r");
		control_out = fdopen(dup(fd), "w");
		// The reload first, while b's handshake is seconds old, far
		// from the two minutes after which b would make another.
		RUN(reload);
		RUN(through);
		RUN(refused);
		CHECK(ask("done", "bye"));
		fclose(control_out);
		fclose(control_in);
	}
	kill(page, SIGKILL);
	check_wait(page);
	return check_end();
}

// Does what a asks, from `from`, and answers it on `to`.
static void follow(FILE *from, FILE *to) {
	char line[512];
	while(fgets(line, sizeof(line), from)) {
		line[strcspn(line, "\n")] = 0;
		printf("wgtest: a asks: %s\n", line);
		char reply[600];
		char host[128];
		unsigned port;
		if(sscanf(line, "probe %127s %u", host, &port) == 2) {
			const char *what;
			int fd = connect_to(host, (unsigned short)port, PROBE_MS, &what);
			if(fd >= 0)
				close(fd);
			snprintf(reply, sizeof(reply), "%s", what);
		} else if(sscanf(line, "fetch %127s %u", host, &port) == 2) {
			const char *what;
			int fd = connect_to(host, (unsigned short)port, PROBE_MS, &what);
			char got[256] = "";
			if(fd >= 0) {
				FILE *stream = fdopen(fd, "r");
				if(!fgets(got, sizeof(got), stream))
					got[0] = 0;
				fclose(stream);
				got[strcspn(got, "\n")] = 0;
				snprintf(reply, sizeof(reply), "ok %s", got);
			} else {
				snprintf(reply, sizeof(reply), "failed %s", what);
			}
		} else if(sscanf(line, "ping %127s", host) == 1) {
			char command[200];
			snprintf(command, sizeof(command), "ping -c 1 -W 5 %s", host);
			snprintf(reply, sizeof(reply), "%s", run(command) == 0 ? "ok" : "failed");
		} else if(!strcmp(line, "done")) {
			fprintf(to, "bye\n");
			fflush(to);
			return;
		} else {
			snprintf(reply, sizeof(reply), "what is `%s`?", line);
		}
		printf("wgtest: answering: %s\n", reply);
		fprintf(to, "%s\n", reply);
		fflush(to);
	}
	printf("wgtest: FAILED: a went before it was done\n");
	check_failures++;
}

static int b(void) {
	set_up(2, "[Interface]\n"
			  "PrivateKey = " B_PRIVATE "\n"
			  "Address = 10.9.0.2/24, fd09::2/64\n"
			  "\n"
			  "[Peer]\n"
			  "PublicKey = " A_PUBLIC "\n"
			  "PresharedKey = " PRESHARED "\n"
			  "AllowedIPs = 10.9.0.0/24, fd09::/64\n"
			  "Endpoint = 192.168.50.1:51820\n"
			  "PersistentKeepalive = 25\n");
	// a may take longer to boot and set up.
	int fd = -1;
	const char *what = "error";
	for(int i = 0; i < 300 && fd < 0; i++) {
		fd = connect_to("10.9.0.1", CONTROL_PORT, 1000, &what);
		if(fd < 0)
			check_sleep_ms(500);
	}
	if(fd < 0)
		printf("wgtest: FAILED: cannot reach a on port %d: %s\n", CONTROL_PORT, what);
	CHECK(fd >= 0);
	if(fd >= 0) {
		printf("wgtest: connected to a through the tunnel\n");
		FILE *from = fdopen(fd, "r");
		FILE *to = fdopen(dup(fd), "w");
		follow(from, to);
		fclose(to);
		fclose(from);
	}
	CHECK(says("wg show wg0", "endpoint: 192.168.50.1:51820"));
	CHECK(says("wg show wg0", "persistent keepalive: every 25 seconds"));
	CHECK(says("wg show all dump", "wg0\t" A_PUBLIC "\t"));
	return check_end();
}

int main(int argc, char **argv) {
	check_start("wgtest");
	if(argc == 2 && !strcmp(argv[1], "a"))
		return a();
	if(argc == 2 && !strcmp(argv[1], "b"))
		return b();
	printf("usage: wgtest a | b\n");
	return 2;
}

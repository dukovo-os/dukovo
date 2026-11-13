// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The log-forward boot test's program (ADR 0482): two machines on a
// cable, 192.168.50.1 and .2, a's card an e1000e and b's a virtio card.
// b runs logforward(8), which sends its log to the syslog servers on a;
// a runs the servers and tells b what to do. b connects to a's control
// port and does what a asks, a line each:
//
// - `run COMMAND`: runs it with sh(1), and says `ok CODE FIRST-LINE`;
// - `put PATH LENGTH`: writes the LENGTH bytes that follow to PATH, and
//   says `ok`;
// - `wait SECONDS NEEDLE|COMMAND`: runs the command until what it writes
//   holds the needle, and says `ok`, or `no` after that many seconds;
// - `settled SECONDS NAME TEXT`: waits until the position that
//   destination NAME saved is past the log's line that holds TEXT, which
//   is when logforward counts what it sent of the line as delivered, and
//   says `ok`, or `no`;
// - `done`: says `bye` and ends.
//
// a makes a certificate authority, a certificate for a.test that it
// signs, and one for a.test that nothing vouches for, and serves
//
// - TLS on port 6514 with the first, as `openssl s_server`, which writes
//   what it receives to a file;
// - TLS on port 6515 with the second, the same;
// - TCP and UDP on port 514, which this program serves itself, writing
//   the stream and each datagram to a file.
//
// b gets the authority's certificate and the settings of four
// destinations, one for each server. a checks that each server gets the
// record `logger` writes on b, as RFC 5424 says, with its length in front
// over TLS and TCP; that the server of the second certificate is refused
// and gets nothing, with the reason in `logforward status` and `logforward
// test`; that records logged while the first server is away arrive in
// order when it is back; that a restart of logforward sends nothing
// again that was delivered, and goes on from there; and that `logforward
// test` sends a test record over TLS.

#include <arpa/inet.h>
#include <fcntl.h>
#include <netinet/in.h>
#include <poll.h>
#include <sys/socket.h>
#include <sys/stat.h>

#include "check.h"

#define CONTROL_PORT 7000
#define TLS_PORT 6514
#define ROGUE_PORT 6515
#define SYSLOG_PORT 514
// The files the servers on a write what they receive to.
#define TLS_OUT "/tmp/tls.out"
#define ROGUE_OUT "/tmp/rogue.out"
#define TCP_OUT "/tmp/tcp.out"
#define UDP_OUT "/tmp/udp.out"
// Where logforward keeps the status of each destination.
#define LIVE "/system/live/logforward"
// What the log's lines of a `logger -t forwardtest TEXT` hold.
#define LOGGED(text) "\"text\":\"" text "\""

// Runs `command` with sh(1), saying so: its exit code, or -1.
static int run(const char *command) {
	printf("logforwardtest: $ %s\n", command);
	fflush(stdout);
	int status = system(command);
	return WIFEXITED(status) ? WEXITSTATUS(status) : -1;
}

// Runs `command` with sh(1) and keeps what it writes, its standard error
// too, in `buffer`: its exit code, or -1.
static int output(const char *command, char *buffer, size_t size) {
	char line[1536];
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
	printf("logforwardtest: `%s` never said \"%s\"; it said: %s\n", command, needle, buffer);
	return 0;
}

// Writes `text` into `path`.
static void write_file(const char *path, const char *text) {
	FILE *file = fopen(path, "w");
	CHECK(file && fputs(text, file) >= 0 && fclose(file) == 0);
}

// What the file at `path` holds, in memory that is the caller's, or NULL;
// `length` is how many bytes.
static char *read_file(const char *path, size_t *length) {
	FILE *file = fopen(path, "r");
	if(!file)
		return NULL;
	size_t capacity = 65536, size = 0;
	char *buffer = malloc(capacity + 1);
	while(buffer) {
		size_t got = fread(buffer + size, 1, capacity - size, file);
		size += got;
		if(size < capacity)
			break;
		char *bigger = realloc(buffer, capacity * 2 + 1);
		if(!bigger) {
			free(buffer);
			buffer = NULL;
			break;
		}
		buffer = bigger;
		capacity *= 2;
	}
	fclose(file);
	if(buffer) {
		buffer[size] = 0;
		if(length)
			*length = size;
	}
	return buffer;
}

// A socket of `type` bound to every IPv4 address at `port`; a TCP one
// listening.
static int bound(int type, unsigned short port) {
	int fd = socket(AF_INET, type, 0);
	CHECK(fd >= 0);
	int yes = 1;
	setsockopt(fd, SOL_SOCKET, SO_REUSEADDR, &yes, sizeof(yes));
	struct sockaddr_in at;
	memset(&at, 0, sizeof(at));
	at.sin_family = AF_INET;
	at.sin_port = htons(port);
	at.sin_addr.s_addr = htonl(INADDR_ANY);
	CHECK(bind(fd, (struct sockaddr *)&at, sizeof(at)) == 0);
	if(type == SOCK_STREAM)
		CHECK(listen(fd, 8) == 0);
	return fd;
}

// The machine's card on the cable, at 192.168.50.N.
static void set_up(int n) {
	char text[256], wanted[64];
	snprintf(text, sizeof(text), "[net0]\ndhcp = no\naddress = 192.168.50.%d/24\n", n);
	write_file("/system/settings/network", text);
	CHECK(run("service netd reload") == 0);
	snprintf(wanted, sizeof(wanted), "192.168.50.%d/24", n);
	CHECK(wait_for("ip -4 address show dev net0", wanted, 60));
}

// --- the servers on a that are this program ----------------------------

// Writes each datagram that arrives at `fd`, a line each, to `path`.
static void udp_server(int fd, const char *path) {
	char datagram[4096];
	for(;;) {
		ssize_t got = recv(fd, datagram, sizeof(datagram) - 1, 0);
		if(got < 0)
			continue;
		FILE *file = fopen(path, "a");
		if(file) {
			datagram[got] = '\n';
			fwrite(datagram, 1, (size_t)got + 1, file);
			fclose(file);
		}
	}
}

// Writes what arrives on each connection to `fd`, one at a time, to
// `path`, as it comes.
static void tcp_server(int fd, const char *path) {
	for(;;) {
		int connection = accept(fd, NULL, NULL);
		if(connection < 0)
			continue;
		char chunk[4096];
		ssize_t got;
		while((got = read(connection, chunk, sizeof(chunk))) > 0) {
			FILE *file = fopen(path, "a");
			if(file) {
				fwrite(chunk, 1, (size_t)got, file);
				fclose(file);
			}
		}
		close(connection);
	}
}

// --- the messages the servers got ---------------------------------------

// A message as a server got it.
struct frame {
	char *text;
	size_t length;
};

#define MAX_FRAMES 2048
static struct frame frames[MAX_FRAMES];
static int frame_count;

static void forget_frames(void) {
	for(int i = 0; i < frame_count; i++)
		free(frames[i].text);
	frame_count = 0;
}

static void keep_frame(const char *text, size_t length) {
	char *copy = malloc(length + 1);
	if(!copy || frame_count == MAX_FRAMES)
		return;
	memcpy(copy, text, length);
	copy[length] = 0;
	frames[frame_count].text = copy;
	frames[frame_count++].length = length;
}

// Reads the messages in the file at `path` into `frames`: with their
// length and a space in front of each when `framed`, which is RFC 6587's
// octet counting, else a line each. Whole ones only, as the server may be
// in the middle of a write. 1 if the file is as it should be, or is not
// there yet, and 0, saying so, if a length is wrong.
static int load_frames(const char *path, int framed) {
	forget_frames();
	size_t size = 0;
	char *data = read_file(path, &size);
	if(!data)
		return 1;
	size_t at = 0;
	int good = 1;
	while(at < size) {
		if(framed) {
			size_t length = 0, digits = 0;
			while(at + digits < size && data[at + digits] >= '0' && data[at + digits] <= '9' && digits < 6) {
				length = length * 10 + (size_t)(data[at + digits] - '0');
				digits++;
			}
			if(at + digits >= size)
				break;
			if(digits == 0 || digits == 6 || data[at + digits] != ' ') {
				printf("logforwardtest: FAILED: %s is not octet-counted at byte %zu: %.40s\n", path,
						at, data + at);
				check_failures++;
				good = 0;
				break;
			}
			at += digits + 1;
			if(at + length > size)
				break;
			keep_frame(data + at, length);
			at += length;
		} else {
			char *end = memchr(data + at, '\n', size - at);
			if(!end)
				break;
			keep_frame(data + at, (size_t)(end - (data + at)));
			at = (size_t)(end - data) + 1;
		}
	}
	free(data);
	return good;
}

// Whether `frame` starts with `prefix` (the priority and the version),
// holds `infix` and ends with `suffix`.
static int frame_matches(const struct frame *frame, const char *prefix, const char *infix,
		const char *suffix) {
	size_t head = strlen(prefix), tail = strlen(suffix);
	return frame->length >= head && !memcmp(frame->text, prefix, head) && frame->length >= tail
			&& !strcmp(frame->text + frame->length - tail, suffix) && strstr(frame->text, infix);
}

// How many messages in the file at `path` match `prefix`, `infix` and
// `suffix` (frame_matches); -1 if the file is not framed as it should be.
// `first` is where the first is among the messages.
static int count_messages(const char *path, int framed, const char *prefix, const char *infix,
		const char *suffix, int *first) {
	if(!load_frames(path, framed))
		return -1;
	int count = 0;
	*first = -1;
	for(int i = 0; i < frame_count; i++) {
		if(frame_matches(&frames[i], prefix, infix, suffix)) {
			if(!count)
				*first = i;
			count++;
		}
	}
	return count;
}

// What `logger -t forwardtest TEXT` makes of TEXT, as RFC 5424 says: a
// program's facility, user (1), and logd's info level, which logger's
// notice becomes (6), make priority 14; the tag is the application name,
// a process id follows, and the message has no id and no structured data.
#define LOGGER_PREFIX "<14>1 "
#define FROM_LOGGER " forwardtest "
#define LOGGER_TEXT(text) " - - " text

// Prints what the file at `path` holds, for a check that failed.
static void show_file(const char *path) {
	char *text = read_file(path, NULL);
	printf("logforwardtest: %s holds: %s\n", path, text ? text : "(nothing, no file)");
	free(text);
}

// Waits for the file at `path` to hold a message of `text` from `logger`,
// for `seconds`: how many it holds then, which is 0 if it never did.
static int wait_for_message(const char *path, int framed, const char *prefix, const char *infix,
		const char *suffix, int seconds) {
	long long deadline = check_now_ns() + seconds * 1000000000LL;
	int first, count;
	do {
		count = count_messages(path, framed, prefix, infix, suffix, &first);
		if(count != 0)
			return count < 0 ? 0 : count;
		check_sleep_ms(500);
	} while(check_now_ns() < deadline);
	printf("logforwardtest: FAILED: no message with \"%s\" ... \"%s\" in %s within %d s\n", infix,
			suffix, path, seconds);
	check_failures++;
	show_file(path);
	return 0;
}

// How many messages from `logger` that say `text` the file at `path` holds.
static int logged_count(const char *path, int framed, const char *suffix) {
	int first;
	return count_messages(path, framed, LOGGER_PREFIX, FROM_LOGGER, suffix, &first);
}

// Where in the file at `path` the message of `suffix` from `logger` is.
static int logged_at(const char *path, int framed, const char *suffix) {
	int first;
	count_messages(path, framed, LOGGER_PREFIX, FROM_LOGGER, suffix, &first);
	return first;
}

// --- machine a's servers -------------------------------------------------

// Starts `openssl s_server` on `port` with `certificate` and its `key`,
// which appends what it receives to `out`, and its process id is in
// `pid_file`. Its standard input is a pipe that stays open, as it ends the
// connection at the end of its input. Whether it listens.
static int start_tls(int port, const char *certificate, const char *key, const char *out,
		const char *pid_file) {
	char command[1024];
	snprintf(command, sizeof(command),
			"sleep 3000 | sh -c 'echo $$ > %s; exec openssl s_server -accept %d -cert %s -key %s "
			"-brief -no_ign_eof' >> %s 2>> %s.err &",
			pid_file, port, certificate, key, out, out);
	CHECK(run(command) == 0);
	snprintf(command, sizeof(command), "nc -z -w 1 127.0.0.1 %d; echo listening=$?", port);
	return wait_for(command, "listening=0", 15);
}

// Starts the server again, as often as it takes: the port may not be free
// the moment the last one on it is gone.
static void restart_tls(int port, const char *certificate, const char *key, const char *out,
		const char *pid_file) {
	for(int tries = 0; tries < 5; tries++) {
		if(start_tls(port, certificate, key, out, pid_file))
			return;
		show_file(out);
		check_sleep_ms(2000);
	}
	printf("logforwardtest: FAILED: openssl s_server never listened on port %d\n", port);
	check_failures++;
}

// Ends the `openssl s_server` whose process id is in `pid_file`.
static void stop_tls(const char *pid_file) {
	char *text = read_file(pid_file, NULL);
	CHECK(text != NULL);
	if(text) {
		pid_t pid = (pid_t)atoi(text);
		free(text);
		CHECK(pid > 1 && kill(pid, SIGKILL) == 0);
	}
	check_sleep_ms(1000);
}

// A certificate authority, a certificate for a.test that it signs and one
// for a.test that nothing vouches for.
static void make_certificates(void) {
	const char *key = "openssl genpkey -algorithm EC -pkeyopt ec_paramgen_curve:prime256v1 -out ";
	char command[512];
	write_file("/tmp/a.ext",
			"subjectAltName=DNS:a.test\nbasicConstraints=critical,CA:FALSE\n"
			"keyUsage=critical,digitalSignature\nextendedKeyUsage=serverAuth\n");
	snprintf(command, sizeof(command), "%s/tmp/ca.key", key);
	CHECK(run(command) == 0);
	CHECK(run("openssl req -new -x509 -key /tmp/ca.key -subj /CN=forward-test-ca -days 3650 "
			  "-out /tmp/ca.pem") == 0);
	snprintf(command, sizeof(command), "%s/tmp/a.key", key);
	CHECK(run(command) == 0);
	CHECK(run("openssl req -new -key /tmp/a.key -subj /CN=a.test -out /tmp/a.csr") == 0);
	CHECK(run("openssl x509 -req -in /tmp/a.csr -CA /tmp/ca.pem -CAkey /tmp/ca.key -set_serial 2 "
			  "-days 3650 -extfile /tmp/a.ext -out /tmp/a.pem") == 0);
	snprintf(command, sizeof(command), "%s/tmp/rogue.key", key);
	CHECK(run(command) == 0);
	CHECK(run("openssl req -new -x509 -key /tmp/rogue.key -subj /CN=a.test "
			  "-addext subjectAltName=DNS:a.test -days 3650 -out /tmp/rogue.pem") == 0);
	CHECK(run("openssl verify -CAfile /tmp/ca.pem /tmp/a.pem") == 0);
	CHECK(run("openssl verify -CAfile /tmp/ca.pem /tmp/rogue.pem") != 0);
}

// Machine a's connection to machine b, a stream each way.
static FILE *control_in, *control_out;
// What b answered last.
static char answer[2048];

// Reads b's answer to `command`: whether it starts with `wanted`.
static int answered(const char *command, const char *wanted) {
	if(!fgets(answer, sizeof(answer), control_in)) {
		printf("logforwardtest: FAILED: b did not answer `%s`\n", command);
		check_failures++;
		answer[0] = 0;
		return 0;
	}
	answer[strcspn(answer, "\n")] = 0;
	printf("logforwardtest: b: %s\n", answer);
	if(strncmp(answer, wanted, strlen(wanted)) != 0) {
		printf("logforwardtest: FAILED: b answered `%s` with `%s`, not `%s`\n", command, answer,
				wanted);
		check_failures++;
		return 0;
	}
	return 1;
}

// Asks b to do `command`: whether it answered `wanted` (a prefix).
static int ask(const char *command, const char *wanted) {
	printf("logforwardtest: asking b: %s\n", command);
	fflush(stdout);
	fprintf(control_out, "%s\n", command);
	fflush(control_out);
	return answered(command, wanted);
}

// Asks b to do `command` and checks that its answer holds `needle`.
static int ask_says(const char *command, const char *wanted, const char *needle) {
	if(!ask(command, wanted))
		return 0;
	if(!strstr(answer, needle)) {
		printf("logforwardtest: FAILED: b answered `%s` with `%s`, which has no \"%s\"\n", command,
				answer, needle);
		check_failures++;
		return 0;
	}
	return 1;
}

// Has b write `text` to `path`.
static int put(const char *path, const char *text) {
	char command[320];
	snprintf(command, sizeof(command), "put %s %zu", path, strlen(text));
	printf("logforwardtest: asking b: %s\n", command);
	fflush(stdout);
	fprintf(control_out, "%s\n", command);
	fwrite(text, 1, strlen(text), control_out);
	fflush(control_out);
	return answered(command, "ok");
}

// Asks b to wait until `command` says `needle`, for `seconds`.
static int ask_wait(int seconds, const char *needle, const char *command) {
	char line[1024];
	snprintf(line, sizeof(line), "wait %d %s|%s", seconds, needle, command);
	return ask(line, "ok");
}

// Asks b to wait until the position destination `name` saved is past the
// line of the log that holds `text`.
static int ask_settled(const char *name, const char *text) {
	char line[512];
	snprintf(line, sizeof(line), "settled 40 %s %s", name, text);
	return ask(line, "ok");
}

// Asks b to wait until destination `name` is in `state`.
static int ask_state(const char *name, const char *state) {
	char needle[128], command[256];
	snprintf(needle, sizeof(needle), "\"state\":\"%s\"", state);
	snprintf(command, sizeof(command), "cat " LIVE "/%s.json", name);
	return ask_wait(90, needle, command);
}

// --- a's parts ----------------------------------------------------------

// b gets the authority's certificate and the destinations, and logforward
// connects to each.
static void configure(void) {
	char *authority = read_file("/tmp/ca.pem", NULL);
	CHECK(authority != NULL);
	if(authority)
		CHECK(put("/system/settings/ssl/test-ca.pem", authority));
	free(authority);
	CHECK(put("/system/settings/logforward.toml",
			"[[destination]]\n"
			"name = \"good\"\n"
			"address = \"192.168.50.1:6514\"\n"
			"ca = \"/system/settings/ssl/test-ca.pem\"\n"
			"server_name = \"a.test\"\n"
			"\n"
			"[[destination]]\n"
			"name = \"rogue\"\n"
			"address = \"192.168.50.1:6515\"\n"
			"ca = \"/system/settings/ssl/test-ca.pem\"\n"
			"server_name = \"a.test\"\n"
			"\n"
			"[[destination]]\n"
			"name = \"plain\"\n"
			"address = \"192.168.50.1:514\"\n"
			"transport = \"tcp\"\n"
			"\n"
			"[[destination]]\n"
			"name = \"datagrams\"\n"
			"address = \"192.168.50.1:514\"\n"
			"transport = \"udp\"\n"));
	CHECK(ask("run service logforward reload", "ok 0"));
	// A destination sends the log from when it starts, so what b logs
	// next is sent only once each has.
	CHECK(ask_state("good", "connected"));
	CHECK(ask_state("plain", "connected"));
	CHECK(ask_state("datagrams", "connected"));
	CHECK(ask_state("rogue", "waiting"));
}

// The record `logger` writes on b reaches the TLS, TCP and UDP servers.
static void first_record(void) {
	CHECK(ask("run logger -t forwardtest first", "ok 0"));
	CHECK(wait_for_message(TLS_OUT, 1, LOGGER_PREFIX, FROM_LOGGER, LOGGER_TEXT("first"), 30) == 1);
	CHECK(wait_for_message(TCP_OUT, 1, LOGGER_PREFIX, FROM_LOGGER, LOGGER_TEXT("first"), 30) == 1);
	CHECK(wait_for_message(UDP_OUT, 0, LOGGER_PREFIX, FROM_LOGGER, LOGGER_TEXT("first"), 30) == 1);
	printf("logforwardtest: the TLS, TCP and UDP servers got the record, as RFC 5424 has it, "
			"with its length in front over TLS and TCP\n");
}

// b does not trust the second TLS server's certificate, and says why.
static void untrusted(void) {
	CHECK(ask_says("run logforward status rogue | tail -n 1", "ok 0 rogue: TLS with 192.168.50.1:6515: ",
			"its certificate is not from an authority in /system/settings/ssl/test-ca.pem"));
	CHECK(ask_says("run logforward status good | tail -n 1", "ok 0 good: TLS 1.3, ",
			"certificate for a.test, issued by forward-test-ca"));
	struct stat st;
	CHECK(stat(ROGUE_OUT, &st) != 0 || st.st_size == 0);
	printf("logforwardtest: the server whose certificate b does not trust was refused and got "
			"nothing\n");
}

// A record logged while the server is away is sent when it is back, in
// order.
static void queue(void) {
	// What the connection carried counts as delivered once it has stayed
	// open ten seconds, so that nothing sent so far is sent again.
	CHECK(ask_settled("good", LOGGED("first")));
	stop_tls("/tmp/tls.pid");
	CHECK(ask_state("good", "waiting"));
	CHECK(ask("run logger -t forwardtest second", "ok 0"));
	CHECK(ask("run logger -t forwardtest third", "ok 0"));
	restart_tls(TLS_PORT, "/tmp/a.pem", "/tmp/a.key", TLS_OUT, "/tmp/tls.pid");
	CHECK(wait_for_message(TLS_OUT, 1, LOGGER_PREFIX, FROM_LOGGER, LOGGER_TEXT("second"), 60) == 1);
	CHECK(wait_for_message(TLS_OUT, 1, LOGGER_PREFIX, FROM_LOGGER, LOGGER_TEXT("third"), 60) == 1);
	int second = logged_at(TLS_OUT, 1, LOGGER_TEXT("second"));
	int third = logged_at(TLS_OUT, 1, LOGGER_TEXT("third"));
	CHECK(second >= 0 && second < third);
	CHECK(logged_count(TLS_OUT, 1, LOGGER_TEXT("first")) == 1);
	// The other servers had them as they were logged.
	CHECK(wait_for_message(TCP_OUT, 1, LOGGER_PREFIX, FROM_LOGGER, LOGGER_TEXT("third"), 30) == 1);
	CHECK(wait_for_message(UDP_OUT, 0, LOGGER_PREFIX, FROM_LOGGER, LOGGER_TEXT("third"), 30) == 1);
	printf("logforwardtest: records logged while the TLS server was away arrived in order when it "
			"was back\n");
}

// logforward starts again, and sends nothing again that was delivered.
static void restart(void) {
	CHECK(ask_settled("good", LOGGED("third")));
	CHECK(ask_settled("plain", LOGGED("third")));
	CHECK(ask("run service logforward restart", "ok 0"));
	CHECK(ask_state("good", "connected"));
	CHECK(ask_state("plain", "connected"));
	CHECK(ask_state("datagrams", "connected"));
	CHECK(ask("run logger -t forwardtest fourth", "ok 0"));
	CHECK(wait_for_message(TLS_OUT, 1, LOGGER_PREFIX, FROM_LOGGER, LOGGER_TEXT("fourth"), 30) == 1);
	CHECK(wait_for_message(TCP_OUT, 1, LOGGER_PREFIX, FROM_LOGGER, LOGGER_TEXT("fourth"), 30) == 1);
	CHECK(wait_for_message(UDP_OUT, 0, LOGGER_PREFIX, FROM_LOGGER, LOGGER_TEXT("fourth"), 30) == 1);
	const char *texts[] = {LOGGER_TEXT("first"), LOGGER_TEXT("second"), LOGGER_TEXT("third")};
	for(int i = 0; i < 3; i++) {
		int tls = logged_count(TLS_OUT, 1, texts[i]);
		int tcp = logged_count(TCP_OUT, 1, texts[i]);
		int udp = logged_count(UDP_OUT, 0, texts[i]);
		if(tls != 1 || tcp != 1 || udp != 1) {
			printf("logforwardtest: FAILED: `%s` reached the TLS, TCP and UDP servers %d, %d and %d "
					"times, not once each\n",
					texts[i], tls, tcp, udp);
			check_failures++;
			show_file(TLS_OUT);
		}
	}
	printf("logforwardtest: after logforward restarted, nothing delivered was sent again, and "
			"what came after was\n");
}

// `logforward test` sends a test record over TLS, and says why it cannot
// to the server b does not trust.
static void test_command(void) {
	// An `openssl s_server` serves one connection at a time, and the
	// service holds the first's; the command makes one of its own.
	CHECK(ask("run service logforward stop", "ok 0"));
	CHECK(ask("run logforward test good",
			"ok 0 good: sent a test record to 192.168.50.1:6514 over TLS 1.3 (certificate for a.test, "
			"issued by forward-test-ca)"));
	CHECK(wait_for_message(TLS_OUT, 1, "<13>1 ", " logforward - test - logforward: a test record from ", "", 30)
			== 1);
	CHECK(ask_says("run logforward test rogue", "ok 1 rogue: TLS with 192.168.50.1:6515: ",
			"its certificate is not from an authority"));
	struct stat st;
	CHECK(stat(ROGUE_OUT, &st) != 0 || st.st_size == 0);
	printf("logforwardtest: logforward test sent a test record over TLS and said why it cannot reach "
			"the server b does not trust\n");
}

static int a(void) {
	set_up(1);
	RUN(make_certificates);
	pid_t udp = check_fork();
	if(udp == 0)
		udp_server(bound(SOCK_DGRAM, SYSLOG_PORT), UDP_OUT);
	pid_t tcp = check_fork();
	if(tcp == 0)
		tcp_server(bound(SOCK_STREAM, SYSLOG_PORT), TCP_OUT);
	CHECK(start_tls(TLS_PORT, "/tmp/a.pem", "/tmp/a.key", TLS_OUT, "/tmp/tls.pid"));
	CHECK(start_tls(ROGUE_PORT, "/tmp/rogue.pem", "/tmp/rogue.key", ROGUE_OUT, "/tmp/rogue.pid"));
	int server = bound(SOCK_STREAM, CONTROL_PORT);
	printf("logforwardtest: waiting for b on port %d\n", CONTROL_PORT);
	fflush(stdout);
	struct pollfd p = {server, POLLIN, 0};
	CHECK(poll(&p, 1, 300000) == 1);
	int fd = accept(server, NULL, NULL);
	CHECK(fd >= 0);
	close(server);
	if(fd >= 0) {
		control_in = fdopen(fd, "r");
		control_out = fdopen(dup(fd), "w");
		RUN(configure);
		RUN(first_record);
		RUN(untrusted);
		RUN(queue);
		RUN(restart);
		RUN(test_command);
		CHECK(ask("done", "bye"));
		fclose(control_out);
		fclose(control_in);
	}
	kill(udp, SIGKILL);
	check_wait(udp);
	kill(tcp, SIGKILL);
	check_wait(tcp);
	forget_frames();
	return check_end();
}

// --- machine b ----------------------------------------------------------

// Whether `offset` in the log's day file `day` is past the end of the
// file's first line that holds `text`.
static int past_line(const char *day, unsigned long long offset, const char *text) {
	char log[320];
	snprintf(log, sizeof(log), "/system/state/log/%s", day);
	char *lines = read_file(log, NULL);
	const char *at = lines ? strstr(lines, text) : NULL;
	const char *end = at ? strchr(at, '\n') : NULL;
	int past = end && offset >= (unsigned long long)(end + 1 - lines);
	free(lines);
	return past;
}

// The first line of a position file; a day file of the log and the byte
// in it that was delivered up to follow, a line each.
#define POSITION_HEADER "logforward position 1\n"

// Waits for the position destination `name` saved to be past the end of
// the log's first line that holds `text`, for `seconds`.
static int settled(const char *name, const char *text, int seconds) {
	long long deadline = check_now_ns() + seconds * 1000000000LL;
	char path[256];
	snprintf(path, sizeof(path), "/system/state/logforward/%s.position", name);
	do {
		char *saved = read_file(path, NULL);
		int past = 0;
		const char *line = NULL;
		if(saved && !strncmp(saved, POSITION_HEADER, strlen(POSITION_HEADER)))
			line = saved + strlen(POSITION_HEADER);
		while(line && *line && !past) {
			char day[64];
			unsigned long long offset;
			if(sscanf(line, "%63s %llu", day, &offset) == 2)
				past = past_line(day, offset, text);
			line = strchr(line, '\n');
			if(line)
				line++;
		}
		free(saved);
		if(past)
			return 1;
		check_sleep_ms(500);
	} while(check_now_ns() < deadline);
	printf("logforwardtest: `%s` never moved its position past %s\n", name, text);
	show_file(path);
	return 0;
}

// Does what a asks, from `from`, and answers it on `to`.
static void follow(FILE *from, FILE *to) {
	char line[1024];
	while(fgets(line, sizeof(line), from)) {
		line[strcspn(line, "\n")] = 0;
		printf("logforwardtest: a asks: %s\n", line);
		char reply[1200];
		if(!strncmp(line, "run ", 4)) {
			static char said[16384];
			int code = output(line + 4, said, sizeof(said));
			printf("%s", said);
			said[strcspn(said, "\n")] = 0;
			snprintf(reply, sizeof(reply), "ok %d %.1000s", code, said);
		} else if(!strncmp(line, "put ", 4)) {
			char path[256];
			size_t length;
			if(sscanf(line + 4, "%255s %zu", path, &length) != 2 || length > 65536) {
				snprintf(reply, sizeof(reply), "what is `%.200s`?", line);
			} else {
				char *content = malloc(length + 1);
				size_t got = content ? fread(content, 1, length, from) : 0;
				FILE *file = got == length ? fopen(path, "w") : NULL;
				int wrote = file && fwrite(content, 1, length, file) == length;
				if(file && fclose(file) != 0)
					wrote = 0;
				free(content);
				snprintf(reply, sizeof(reply), "%s", wrote ? "ok" : "cannot write the file");
			}
		} else if(!strncmp(line, "wait ", 5)) {
			int seconds = atoi(line + 5);
			char *rest = strchr(line + 5, ' ');
			char *bar = rest ? strchr(rest + 1, '|') : NULL;
			if(!bar) {
				snprintf(reply, sizeof(reply), "what is `%.200s`?", line);
			} else {
				*bar = 0;
				int found = wait_for(bar + 1, rest + 1, seconds);
				snprintf(reply, sizeof(reply), "%s", found ? "ok" : "no");
			}
		} else if(!strncmp(line, "settled ", 8)) {
			int seconds = atoi(line + 8);
			char name[64];
			char *text = strchr(line + 8, ' ');
			char *after = text ? strchr(text + 1, ' ') : NULL;
			if(!after || (size_t)(after - text - 1) >= sizeof(name)) {
				snprintf(reply, sizeof(reply), "what is `%.200s`?", line);
			} else {
				snprintf(name, sizeof(name), "%.*s", (int)(after - text - 1), text + 1);
				int found = settled(name, after + 1, seconds);
				snprintf(reply, sizeof(reply), "%s", found ? "ok" : "no");
			}
		} else if(!strcmp(line, "done")) {
			fprintf(to, "bye\n");
			fflush(to);
			return;
		} else {
			snprintf(reply, sizeof(reply), "what is `%.200s`?", line);
		}
		printf("logforwardtest: answering: %s\n", reply);
		fprintf(to, "%s\n", reply);
		fflush(to);
	}
	printf("logforwardtest: FAILED: a went before it was done\n");
	check_failures++;
}

// Connects to a's control port, waiting for a to listen: the socket, or
// -1.
static int reach_a(void) {
	for(int i = 0; i < 300; i++) {
		int fd = socket(AF_INET, SOCK_STREAM, 0);
		struct sockaddr_in to;
		memset(&to, 0, sizeof(to));
		to.sin_family = AF_INET;
		to.sin_port = htons(CONTROL_PORT);
		inet_pton(AF_INET, "192.168.50.1", &to.sin_addr);
		if(fd >= 0 && connect(fd, (struct sockaddr *)&to, sizeof(to)) == 0)
			return fd;
		if(fd >= 0)
			close(fd);
		check_sleep_ms(1000);
	}
	return -1;
}

static int b(void) {
	set_up(2);
	int fd = reach_a();
	if(fd < 0)
		printf("logforwardtest: FAILED: cannot reach a on port %d\n", CONTROL_PORT);
	CHECK(fd >= 0);
	if(fd >= 0) {
		printf("logforwardtest: connected to a\n");
		FILE *from = fdopen(fd, "r");
		FILE *to = fdopen(dup(fd), "w");
		follow(from, to);
		fclose(to);
		fclose(from);
	}
	return check_end();
}

int main(int argc, char **argv) {
	check_start("logforwardtest");
	if(argc == 2 && !strcmp(argv[1], "a"))
		return a();
	if(argc == 2 && !strcmp(argv[1], "b"))
		return b();
	printf("usage: logforwardtest a | b\n");
	return 2;
}

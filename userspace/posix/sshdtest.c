// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the sshd boot test runs (ADR 0347): Spark starts it as root with
// the services, authd(8) among them, the names capability and the
// network. It makes alice's and bob's accounts with user(8), fetches the
// host's public keys from the host's port 80, and writes alice's
// ~/.ssh/authorized_keys: one of the host's keys as it is, the other
// behind an option, which is passed over, and a key of the guest's own;
// bob's file holds the first key too, but others may write it, so it
// signs no one in. As alice, she cannot turn sshd on, nor ask authd about
// keys. `service sshd enable` starts sshd(8), which makes its host key;
// the guest's own OpenSSH client signs in as alice, and root is refused.
// Then the host's client signs in through a port QEMU forwards, and
// copies files with sftp and scp; when the host's last file is in
// alice's home, the test finds her session in last(1) and the files the
// host copied, hers. The host forwards port 7000 here to a server of its
// own with ssh -R: it listens on the loopback alone, a MiB goes each way
// through it, and it closes with the host's client (ADR 0349). A
// sshd_config with a mistake is refused by sshd -t and a reload keeps
// what sshd had; then one that turns passwords on but keeps bob out,
// and forwarding to the host alone, and the host signs in with alice's
// password; lastb shows the wrong one it tried (ADR 0348). `service sshd
// disable` stops sshd and closes the port.

#include <arpa/inet.h>
#include <fcntl.h>
#include <netinet/in.h>
#include <sys/socket.h>
#include <sys/stat.h>

#include "authd.h"
#include "check.h"
#include "names.h"
#include "programs.h"

// This program, beneath the root.
#define PROGRAM "system/tests/posix/sshdtest"
#define USER "/system/commands/user"
#define SERVICE "/system/commands/service"
#define SERVICE_FILE "/system/settings/spark/services/sshd.toml"
#define SSHD "/system/commands/sshd"
#define SSHD_CONFIG "/system/settings/ssh/sshd_config"
#define LOG "/system/commands/log"
#define HOST_KEY "/system/state/sshd/ssh_host_ed25519_key"
#define ALICE 1000
#define BOB 1001
// The host's keys, at its port 80.
#define HOST "10.0.2.100"
// The guest's own client's files.
#define DIR "/tmp/sshdtest"
#define GUEST_KEY DIR "/guest"
#define KNOWN_HOSTS DIR "/known_hosts"
// The guest's client, as alice, to this computer.
#define SSH \
	"ssh -i " GUEST_KEY " -o IdentitiesOnly=yes -o BatchMode=yes -o StrictHostKeyChecking=yes " \
	"-o UserKnownHostsFile=" KNOWN_HOSTS " -o LogLevel=ERROR "
// The host's last files, written as alice when it has checked the rest,
// with keys, then with passwords.
#define DONE "/users/alice/host-done"
#define PASSWORDS_DONE "/users/alice/passwords-done"
// The size of each file the host copies.
#define COPIED_SIZE 262144
// How long the host may take, in seconds.
#define HOST_SECONDS 300
// The port the host forwards here to its server, and how much goes each
// way through it.
#define FORWARDED_PORT 7000
#define FORWARDED_SIZE (1024 * 1024)
// This computer's address on QEMU's network, from its DHCP server.
#define GUEST "10.0.2.15"

// What a program wrote to its standard error.
static char errors[4096];

// Runs `argv` given `input`, as run_program does, with what it writes to
// its standard error in `errors`.
static int run_both(const char *const argv[], const char *input) {
	char path[64];
	snprintf(path, sizeof(path), "/tmp/sshdtest-errors-%d", (int)getpid());
	int fd = open(path, O_RDWR | O_CREAT | O_TRUNC | O_CLOEXEC, 0600);
	int saved = dup(STDERR_FILENO);
	CHECK(fd >= 0 && saved >= 0);
	dup2(fd, STDERR_FILENO);
	int status = run_program(argv, input, strlen(input));
	dup2(saved, STDERR_FILENO);
	close(saved);
	ssize_t got = pread(fd, errors, sizeof(errors) - 1, 0);
	errors[got > 0 ? got : 0] = 0;
	close(fd);
	unlink(path);
	return status;
}

// Checks that `argv` given `input` exits with `status`, writes `out` and
// `err` among what it writes to its standard output and error.
static void says(const char *file, int line, int status, const char *input, const char *out,
		const char *err, const char *const argv[]) {
	int got = run_both(argv, input);
	if(got != status || !strstr(output, out) || !strstr(errors, err)) {
		char what[1024];
		snprintf(what, sizeof(what), "%s %s exited with %d, wrote \"%.300s\" and \"%.300s\"",
				argv[0], argv[1] ? argv[1] : "", got, output, errors);
		check_failed(file, line, what, 0);
	}
}

#define SAYS(status, input, out, err, ...) \
	says(__FILE__, __LINE__, status, input, out, err, (const char *const[]){__VA_ARGS__, NULL})

// The start of the file at `path`, or "" without it.
static const char *contents(const char *path) {
	static char data[4096];
	data[0] = 0;
	int fd = open(path, O_RDONLY | O_CLOEXEC);
	if(fd < 0)
		return data;
	ssize_t len = read(fd, data, sizeof(data) - 1);
	close(fd);
	data[len > 0 ? len : 0] = 0;
	return data;
}

// The file at `path`, with `text`, owned by `user` and of mode `mode`.
static void write_text(const char *path, const char *text, uint32_t user, mode_t mode) {
	int fd = open(path, O_WRONLY | O_CREAT | O_TRUNC | O_CLOEXEC, 0600);
	CHECK(fd >= 0);
	if(fd < 0)
		return;
	CHECK(write(fd, text, strlen(text)) == (ssize_t)strlen(text));
	CHECK(fchown(fd, user, user) == 0);
	CHECK(fchmod(fd, mode) == 0);
	CHECK(close(fd) == 0);
}

// A connection to `address` port `port`, or -1.
static int connect_to(const char *address, int port) {
	int fd = socket(AF_INET, SOCK_STREAM | SOCK_CLOEXEC, 0);
	CHECK(fd >= 0);
	struct sockaddr_in at = {.sin_family = AF_INET, .sin_port = htons(port)};
	CHECK(inet_pton(AF_INET, address, &at.sin_addr) == 1);
	if(connect(fd, (struct sockaddr *)&at, sizeof(at)) != 0) {
		close(fd);
		return -1;
	}
	return fd;
}

// Whether something listens on port 22 of the loopback.
static int listening(void) {
	int fd = connect_to("127.0.0.1", 22);
	if(fd >= 0)
		close(fd);
	return fd >= 0;
}

// Waits up to `seconds` for port 22 to be `open` or not; whether it is.
static int port_becomes(int open, int seconds) {
	long long deadline = check_now_ns() + seconds * 1000000000LL;
	while(listening() != open) {
		if(check_now_ns() > deadline)
			return 0;
		check_sleep_ms(200);
	}
	return 1;
}

// What the host's port `port` sends, up to `size` bytes, once netd has
// set the card up, within a minute.
static void fetch(int port, char *text, size_t size) {
	text[0] = 0;
	struct sockaddr_in at = {.sin_family = AF_INET, .sin_port = htons(port)};
	CHECK(inet_pton(AF_INET, HOST, &at.sin_addr) == 1);
	long long deadline = check_now_ns() + 60 * 1000000000LL;
	int fd;
	for(;;) {
		fd = socket(AF_INET, SOCK_STREAM | SOCK_CLOEXEC, 0);
		CHECK(fd >= 0);
		if(connect(fd, (struct sockaddr *)&at, sizeof(at)) == 0)
			break;
		close(fd);
		CHECK(check_now_ns() < deadline);
		if(check_now_ns() >= deadline)
			return;
		check_sleep_ms(500);
	}
	size_t len = 0;
	ssize_t got;
	while(len < size - 1 && (got = read(fd, text + len, size - 1 - len)) > 0)
		len += got;
	close(fd);
	text[len] = 0;
}

// The first line of `text` that starts with `start`, without its
// newline, in `line`.
static void line_of(const char *text, const char *start, char *line, size_t size) {
	line[0] = 0;
	const char *at = strstr(text, start);
	if(!at)
		return;
	size_t len = strcspn(at, "\n");
	if(len >= size)
		len = size - 1;
	memcpy(line, at, len);
	line[len] = 0;
}

// alice's and bob's accounts.
static void accounts(void) {
	SAYS(0, "wonderland\n", "Made alice, number 1000, home /users/alice\n", "", USER, "add",
			"alice");
	SAYS(0, "looking-glass\n", "Made bob, number 1001, home /users/bob\n", "", USER, "add", "bob");
}

// As alice: she cannot turn sshd on, and authd tells her nothing of
// keys.
static int alice(void) {
	check_start("sshdtest (alice)");
	SAYS(1, "", "", "only root may enable a service", SERVICE, "sshd", "enable");
	char answer[256];
	read_answer(send_request(REQUEST("ssh-key\0" "alice\0" "00\0")), answer, sizeof(answer));
	CHECK(strstr(answer, "no only sshd") == answer);
	return check_end();
}

// The keys: the host's two, from its port 80, and the guest's own, in
// alice's authorized_keys; the first in bob's too, which others may
// write.
static void keys(void) {
	CHECK(mkdir(DIR, 0700) == 0);
	// The client's own files are there too.
	setenv("HOME", DIR, 1);
	STATUS(0, "/bin/sh", "-c",
			"ssh-keygen -q -t ed25519 -N '' -C guest@dukovo -f " GUEST_KEY);
	char keys[2048], alice_key[512], optioned_key[512], guest_key[512];
	fetch(80, keys, sizeof(keys));
	line_of(keys, "ssh-ed25519 ", alice_key, sizeof(alice_key));
	CHECK(strstr(alice_key, " host-alice") != NULL);
	const char *second = strstr(keys, " host-alice\n");
	line_of(second ? second : "", "ssh-ed25519 ", optioned_key, sizeof(optioned_key));
	CHECK(strstr(optioned_key, " host-optioned") != NULL);
	line_of(contents(GUEST_KEY ".pub"), "ssh-ed25519 ", guest_key, sizeof(guest_key));
	CHECK(guest_key[0] != 0);
	char text[2048];
	snprintf(text, sizeof(text),
			"# alice's keys\n\n%s\ncommand=\"/bin/false\" %s\n%s\n", alice_key, optioned_key,
			guest_key);
	CHECK(mkdir("/users/alice/.ssh", 0700) == 0 && chown("/users/alice/.ssh", ALICE, ALICE) == 0);
	write_text("/users/alice/.ssh/authorized_keys", text, ALICE, 0600);
	CHECK(mkdir("/users/bob/.ssh", 0700) == 0 && chown("/users/bob/.ssh", BOB, BOB) == 0);
	snprintf(text, sizeof(text), "%s\n", alice_key);
	write_text("/users/bob/.ssh/authorized_keys", text, BOB, 0666);
}

// Waits up to `seconds` for sshd's log to say `text`; whether it did.
static int log_says(const char *text, int seconds) {
	long long deadline = check_now_ns() + seconds * 1000000000LL;
	for(;;) {
		run_both((const char *const[]){LOG, "-s", "sshd", NULL}, "");
		if(strstr(output, text))
			return 1;
		if(check_now_ns() > deadline)
			return 0;
		check_sleep_ms(500);
	}
}

// Waits up to `seconds` for alice's file `path`; whether it came, hers.
static int comes(const char *path, int seconds) {
	struct stat st;
	long long deadline = check_now_ns() + seconds * 1000000000LL;
	while(stat(path, &st) != 0 && check_now_ns() < deadline)
		check_sleep_ms(500);
	return stat(path, &st) == 0 && st.st_uid == ALICE && st.st_gid == ALICE;
}

// `service sshd enable` starts sshd, which makes its key and listens on
// port 22; the test shows the key, which the host's client checks. The
// default sshd_config says nothing: keys sign in, passwords do not.
static void on(void) {
	CHECK(!listening());
	SAYS(0, "", "", "", SSHD, "-t");
	SAYS(0, "",
			"PasswordAuthentication no\nPubkeyAuthentication yes\nAllowTcpForwarding yes\n"
			"GatewayPorts no\n",
			"", SSHD, "-T");
	SAYS(0, "", "sshd is running, and starts at each boot.", "", SERVICE, "sshd", "enable");
	CHECK(port_becomes(1, 60));
	CHECK(strstr(contents(SERVICE_FILE), "\nenabled = true\n") != NULL);
	struct stat st;
	CHECK(stat(HOST_KEY, &st) == 0 && st.st_uid == 0 && (st.st_mode & 07777) == 0600);
	CHECK(stat(HOST_KEY ".pub", &st) == 0 && (st.st_mode & 07777) == 0644);
	char key[512];
	line_of(contents(HOST_KEY ".pub"), "ssh-ed25519 ", key, sizeof(key));
	CHECK(strstr(key, " root@") != NULL);
	// The type and the key, without the comment.
	char *comment = strstr(key, " root@");
	if(comment)
		*comment = 0;
	printf("sshdtest: host key %s\n", key);
	fflush(stdout);
	char known[600];
	snprintf(known, sizeof(known), "127.0.0.1 %s\n", key);
	write_text(KNOWN_HOSTS, known, 0, 0644);
}

// The guest's own client signs in as alice, and root is refused.
static void guest(void) {
	OUTPUT("", "alice\n", "/bin/sh", "-c", SSH "alice@127.0.0.1 id -un");
	OUTPUT("", "127.0.0.1 127.0.0.1 22\n", "/bin/sh", "-c",
			SSH "alice@127.0.0.1 'echo $SSH_CONNECTION' | cut -d' ' -f1,3,4");
	OUTPUT("", "/users/alice\n", "/bin/sh", "-c", SSH "alice@127.0.0.1 pwd");
	STATUS(255, "/bin/sh", "-c", SSH "root@127.0.0.1 true 2>/dev/null");
}

// What the host's client did, which it ends by writing host-done as
// alice; last(1) shows her sessions on terminals, from the host, and the
// files it copied with sftp, scp and scp -O are hers.
static void host(void) {
	printf("sshdtest: ready for the host\n");
	fflush(stdout);
	CHECK(comes(DONE, HOST_SECONDS));
	CHECK(!strcmp(contents(DONE), "done\n"));
	CONTAINS("10.0.2.2", "/system/commands/last", "alice");
	CONTAINS("pts/", "/system/commands/last", "alice");
	const char *copied[] = {"/users/alice/sftp.bin", "/users/alice/scp.bin",
			"/users/alice/scp-o.bin"};
	for(size_t i = 0; i < sizeof(copied) / sizeof(copied[0]); i++) {
		struct stat st;
		CHECK(stat(copied[i], &st) == 0 && st.st_uid == ALICE && st.st_size == COPIED_SIZE);
	}
	// The folder sftp made, and removed.
	struct stat st;
	CHECK(stat("/users/alice/folder", &st) != 0);
}

// A byte of what goes through the forwarded port: to the host, or `back`.
static unsigned char pattern(size_t i, int back) {
	return back ? (unsigned char)((i * 13 + 5) % 251) : (unsigned char)(i * 7 % 253);
}

// The host forwards port 7000 here to a server of its own, with ssh -R
// (ADR 0349): alice's forwarder listens on the loopback alone, as
// GatewayPorts says by default, and a MiB goes each way, the server
// sending its own once all of this one came. The port closes as the
// host's client goes.
static void forwarding(void) {
	static unsigned char data[FORWARDED_SIZE];
	long long deadline = check_now_ns() + HOST_SECONDS * 1000000000LL;
	int fd;
	while((fd = connect_to("127.0.0.1", FORWARDED_PORT)) < 0) {
		CHECK(check_now_ns() < deadline);
		if(check_now_ns() >= deadline)
			return;
		check_sleep_ms(500);
	}
	CHECK(log_says("forwarding port 7000 at localhost for alice from 10.0.2.2", 10));
	CHECK(log_says("started forwarding for alice from 10.0.2.2", 0));
	int outside = connect_to(GUEST, FORWARDED_PORT);
	CHECK(outside < 0);
	if(outside >= 0)
		close(outside);
	for(size_t i = 0; i < FORWARDED_SIZE; i++)
		data[i] = pattern(i, 0);
	size_t done = 0;
	ssize_t n;
	while(done < FORWARDED_SIZE && (n = write(fd, data + done, FORWARDED_SIZE - done)) > 0)
		done += n;
	CHECK(done == FORWARDED_SIZE);
	CHECK(shutdown(fd, SHUT_WR) == 0);
	size_t same = 0;
	done = 0;
	while((n = read(fd, data, sizeof(data))) > 0) {
		for(ssize_t i = 0; i < n; i++)
			same += data[i] == pattern(done + i, 1);
		done += n;
	}
	close(fd);
	CHECK(done == FORWARDED_SIZE && same == FORWARDED_SIZE);
	printf("sshdtest: forwarded\n");
	fflush(stdout);
	deadline = check_now_ns() + 30 * 1000000000LL;
	while((fd = connect_to("127.0.0.1", FORWARDED_PORT)) >= 0) {
		close(fd);
		if(check_now_ns() > deadline)
			break;
		check_sleep_ms(500);
	}
	CHECK(fd < 0);
	// Nothing the forwarder sent broke its frames, and it said nothing.
	CHECK(!log_says("sshd: forwarding for ", 0));
}

// sshd_config: a mistake keeps sshd as it was; then passwords sign in,
// but not bob's, and the host signs in with alice's; forwarding goes to
// the host alone.
static void passwords(void) {
	write_text(SSHD_CONFIG, "PasswordAuthentication yes\nPort 2222\n", 0, 0644);
	SAYS(1, "", "", "sshd_config line 2: sshd does not take Port", SSHD, "-t");
	SAYS(0, "", "", "", SERVICE, "sshd", "reload");
	CHECK(log_says("kept the settings it had", 30));
	CHECK(listening());
	write_text(SSHD_CONFIG,
			"# The sshd test's.\nPasswordAuthentication yes\nDenyUsers bob\n"
			"AllowTcpForwarding remote\n",
			0, 0644);
	SAYS(0, "", "", "", SSHD, "-t");
	SAYS(0, "", "PasswordAuthentication yes\n", "", SSHD, "-T");
	SAYS(0, "", "AllowTcpForwarding remote\n", "", SSHD, "-T");
	SAYS(0, "", "", "", SERVICE, "sshd", "reload");
	CHECK(log_says("keys and passwords sign in", 30));
	printf("sshdtest: passwords on\n");
	fflush(stdout);
	CHECK(comes(PASSWORDS_DONE, HOST_SECONDS));
	CHECK(!strcmp(contents(PASSWORDS_DONE), "done\n"));
	// The host's ssh -W before it, refused by AllowTcpForwarding remote.
	CHECK(log_says("refused to forward a connection for alice from 10.0.2.2", 0));
	// The wrong password the host tried, from its address.
	CONTAINS("10.0.2.2", "/system/commands/lastb", "alice");
	CONTAINS("ssh", "/system/commands/lastb", "alice");
	CHECK(unlink(SSHD_CONFIG) == 0);
	// sshd told authd each password's connection ended.
	CHECK(!log_says("cannot tell authd", 0));
}

// `service sshd disable` stops sshd, which closes the port.
static void off(void) {
	SAYS(0, "", "sshd is stopped, and no longer starts at boot.", "", SERVICE, "sshd",
			"disable");
	CHECK(port_becomes(0, 30));
	CHECK(strstr(contents(SERVICE_FILE), "\nenabled = false\n") != NULL);
}

int main(int argc, char **argv) {
	if(argc == 2 && !strcmp(argv[1], "alice"))
		return alice();
	check_start("sshdtest");
	authd_answers();
	RUN(accounts);
	// She notes the commands she runs, for the `commands` test, too.
	int notes = open(COMMANDS_RUN, O_WRONLY | O_CREAT | O_APPEND | O_CLOEXEC, 0644);
	CHECK(notes >= 0 && fchmod(notes, 0666) == 0);
	close(notes);
	char *const alice_args[] = {"sshdtest", "alice", NULL};
	pid_t pid = start_as(PROGRAM, ALICE, ALICE, NULL, 0, alice_args);
	CHECK_CHILD(pid);
	RUN(keys);
	RUN(on);
	RUN(guest);
	RUN(host);
	RUN(forwarding);
	RUN(passwords);
	RUN(off);
	return check_end();
}

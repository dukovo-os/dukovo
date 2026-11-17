// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The blocking boot test's program (ADR 0479): two machines on a cable,
// a's card an e1000e at 10.9.0.1 and b's a virtio card at 10.9.0.20, which
// a's netd gives by DHCP. Machine a runs the system's own services, authd,
// the firewall and sshd among them, with the names and firewall
// capabilities; machine b guesses passwords at a's sshd, as an attacker
// would, and signs in once with a right one. The structure, a control port
// that a drives b through, is that of networkservetest.c.
//
// a makes alice's account, whom nobody signs in as, and carol's, an
// administrator's, turns passwords on in sshd_config and sshd on, and lets
// its control port, 7000, in through the firewall as a rule: a program a
// person runs is for the machine alone until one does, and 7000 is no
// sign-in port, so a block of b must leave it open. b connects to it and
// does what a asks, a line each:
//
// - `run COMMAND`: runs it with sh(1), and says `ok CODE FIRST-LINE`;
// - `put PATH LENGTH`: writes the LENGTH bytes that follow to PATH, and
//   says `ok`;
// - `wait SECONDS NEEDLE|COMMAND`: runs the command until what it writes
//   holds the needle, and says `ok`, or `no` after that many seconds;
// - `guess NAME...`: signs in over SSH to a as each name, with a wrong
//   password, at most three at once, and says `ok N` for the N the server
//   refused with ssh's status 255;
// - `session NAME`: signs in as NAME with the right password and keeps the
//   session for a minute and a half, running `sleep`, and says `ok`;
// - `done`: says `bye` and ends.
//
// a checks, in this order, that:
//
// - five wrong passwords from b, for five names, block b's address, that
//   `firewall blocked` says so with the reason, and that the kernel logs
//   what it dropped;
// - b's connections to port 22 then time out, while those to port 7000, no
//   sign-in port, go through;
// - `firewall unblock` lets b in at once, and a second one says it is not
//   blocked;
// - `firewall never-block` puts b's address in blocking.toml, six more
//   wrong passwords then block nothing, and `never-block remove` takes it
//   out again;
// - a block outlasts a restart of authd, in its list, the state file and
//   the kernel;
// - while an administrator is signed in from b, five more wrong passwords
//   from b block nothing;
// - the administrators were told of the blocks at their next sign-in.

#include <arpa/inet.h>
#include <fcntl.h>
#include <netinet/in.h>
#include <poll.h>
#include <sys/socket.h>
#include <sys/stat.h>

#include "check.h"

#define CONTROL_PORT 7000
#define A_ADDRESS "10.9.0.1"
#define B_ADDRESS "10.9.0.20"
#define B_MAC "52:54:00:12:34:a1"
#define SERVICE "/system/commands/service"
#define USER "/system/commands/user"
#define FIREWALL "/system/commands/firewall"
#define SSHD "/system/commands/sshd"
#define SSHD_CONFIG "/system/settings/ssh/sshd_config"
#define BLOCKING "/system/settings/blocking.toml"
#define BLOCKED_STATE "/system/state/authd/blocked"
#define NOTICES "/system/state/authd/notices"
// alice's password and carol's, as the test makes them. alice's is never
// typed; b gives carol's in the one session it signs in to.
#define ALICE_PASSWORD "wonderland"
#define CAROL_PASSWORD "looking-glass"
// The askpass programs of b's ssh: one answers each prompt with a wrong
// password, the other with carol's.
#define ASKPASS_WRONG "/tmp/askpass-wrong"
#define ASKPASS_CAROL "/tmp/askpass-carol"
// What a person's block says when `firewall blocked` lists it, as the
// reason of five wrong passwords.
#define FIVE_WRONG "5 wrong passwords over SSH in "
// The names b guesses, five or six a time, each used once, so that the wait
// before a wrong password is answered, which authd makes longer for each
// wrong password of the same name from the same address, is always the
// first's: two seconds. alice is the one account among them, so that a wrong
// password for an account that exists counts as one for a name that has
// none; carol's name is kept for her session.
#define NAMES_FIRST "alice admin test guest oracle"
#define NAMES_NEVER "dave user ftp mysql postgres nagios"
#define NAMES_RESTART "ubuntu pi git deploy www"
#define NAMES_ADMIN "jenkins hadoop tomcat vagrant svn"

// Runs `command` with sh(1), saying so: its exit code, or -1.
static int run(const char *command) {
	printf("blockingtest: $ %s\n", command);
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
	do {
		if(output(command, buffer, sizeof(buffer)) == 0 && strstr(buffer, needle))
			return 1;
		check_sleep_ms(500);
	} while(check_now_ns() < deadline);
	printf("blockingtest: FAILED: `%s` never said \"%s\"; it said: %s\n", command, needle, buffer);
	check_failures++;
	return 0;
}

// Runs `command` once and checks that its exit code is `code` and that
// what it writes holds `needle`: what it wrote, in a buffer that stays
// until the next call.
static const char *says(const char *command, int code, const char *needle) {
	static char buffer[8192];
	printf("blockingtest: $ %s\n", command);
	fflush(stdout);
	int got = output(command, buffer, sizeof(buffer));
	if(got != code || !strstr(buffer, needle)) {
		printf("blockingtest: FAILED: `%s` exited with %d, not %d, and said: %s\n", command, got,
				code, buffer);
		printf("blockingtest: it should have said \"%s\"\n", needle);
		check_failures++;
	}
	return buffer;
}

// What the file at `path` holds, in a buffer that stays until the next
// call; empty without the file.
static const char *contents(const char *path) {
	static char text[4096];
	text[0] = 0;
	FILE *file = fopen(path, "r");
	if(!file)
		return text;
	size_t length = fread(text, 1, sizeof(text) - 1, file);
	fclose(file);
	text[length] = 0;
	return text;
}

// Writes `text` to `path`.
static void write_file(const char *path, const char *text) {
	FILE *file = fopen(path, "w");
	CHECK(file && fputs(text, file) >= 0 && fclose(file) == 0);
}

// --- machine a: asking b ------------------------------------------------

// Machine a's connection to machine b, a stream each way.
static FILE *control_in, *control_out;
// What b answered last.
static char answer[2048];

// Reads b's answer to `command`: whether it starts with `wanted`.
static int answered(const char *command, const char *wanted) {
	if(!fgets(answer, sizeof(answer), control_in)) {
		printf("blockingtest: FAILED: b did not answer `%s`\n", command);
		check_failures++;
		answer[0] = 0;
		return 0;
	}
	answer[strcspn(answer, "\n")] = 0;
	printf("blockingtest: b: %s\n", answer);
	if(strncmp(answer, wanted, strlen(wanted)) != 0) {
		printf("blockingtest: FAILED: b answered `%s` with `%s`, not `%s`\n", command, answer,
				wanted);
		check_failures++;
		return 0;
	}
	return 1;
}

// Asks b to do `command`: whether it answered `wanted` (a prefix).
static int ask(const char *command, const char *wanted) {
	printf("blockingtest: asking b: %s\n", command);
	fflush(stdout);
	fprintf(control_out, "%s\n", command);
	fflush(control_out);
	return answered(command, wanted);
}

// Has b write `text` to `path`.
static int put(const char *path, const char *text) {
	char command[320];
	snprintf(command, sizeof(command), "put %s %zu", path, strlen(text));
	printf("blockingtest: asking b: %s\n", command);
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

// Waits up to `seconds` for b's connection to port `port` of a to be
// `reached` or not, as `nc -z`'s status says, and the command says it as
// `reached=0` or `reached=1` so that the wait finds it among what nc
// itself writes: whether it came to be so.
static int b_reaches(const char *port, int reached, int seconds) {
	char command[256];
	snprintf(command, sizeof(command), "nc -z -w 3 " A_ADDRESS " %s; echo reached=$?", port);
	return ask_wait(seconds, reached ? "reached=0" : "reached=1", command);
}

// Has b sign in to a as each of `names`, with a wrong password: how many
// of them a refused.
static int b_guesses(const char *names) {
	char command[256];
	snprintf(command, sizeof(command), "guess %s", names);
	if(!ask(command, "ok "))
		return -1;
	return atoi(answer + 3);
}

// How many names `names` holds.
static int count_names(const char *names) {
	int count = 0;
	for(const char *at = names; *at; at++)
		if(*at != ' ' && (at == names || at[-1] == ' '))
			count++;
	return count;
}

// Has b give `names`, each a wrong password, and checks that a refused
// every one.
static void guess_all(const char *names) {
	int refused = b_guesses(names);
	printf("blockingtest: a refused %d of the %d wrong passwords\n", refused, count_names(names));
	CHECK(refused == count_names(names));
}

// What `firewall blocked` says: its text, in a buffer that stays until the
// next call.
static const char *blocked(void) {
	static char text[4096];
	output(FIREWALL " blocked", text, sizeof(text));
	return text;
}

// --- machine a's parts --------------------------------------------------

// The card has its address, and gives b the one its MAC address reserves:
// network(5).
static void serve_addresses(void) {
	write_file("/system/settings/network",
			"[net0]\n"
			"dhcp = no\n"
			"address = " A_ADDRESS "/24\n"
			"dhcp-server = 10.9.0.100-10.9.0.150\n"
			"dhcp-server-lease = 1m\n"
			"reserve = " B_MAC " " B_ADDRESS " b\n");
	CHECK(run("service netd reload") == 0);
	CHECK(wait_for("ip -4 address show dev net0", A_ADDRESS "/24", 60));
}

// Waits up to `seconds` for something to listen on port 22 of a's
// loopback, which is where nothing a block does reaches.
static int sshd_listens(int seconds) {
	return wait_for("nc -z -w 1 127.0.0.1 22; echo listening=$?", "listening=0", seconds);
}

// Makes the two accounts, passwords on and sshd on, and a rule that lets
// b reach the control port.
static void set_up(void) {
	says("printf '" ALICE_PASSWORD "\\n' | " USER " add alice", 0,
			"Made alice, number 1000, home /users/alice");
	says("printf '" CAROL_PASSWORD "\\n' | " USER " add --admin carol", 0,
			"Made carol, number 1001, home /users/carol");
	write_file(SSHD_CONFIG, "PasswordAuthentication yes\n");
	says(SSHD " -t", 0, "");
	says(SERVICE " sshd enable", 0, "sshd is running, and starts at each boot.");
	CHECK(sshd_listens(60));
	// A program a person runs is for this machine alone until a rule lets
	// it in; the block of b must not reach what this rule lets in.
	says(FIREWALL " allow tcp 7000", 0, "is `allow tcp 7000`");
	says(FIREWALL " blocked", 0, "No address is blocked.");
}

// b's five wrong passwords, for five names, block its address: the list
// says so with the reason, the audit log's line is the one the log shows,
// and the administrator is to be told.
static void five_wrong_passwords(void) {
	guess_all(NAMES_FIRST);
	CHECK(wait_for(FIREWALL " blocked", B_ADDRESS, 30));
	const char *list = blocked();
	printf("blockingtest: firewall blocked:\n%s", list);
	CHECK(strstr(list, "Blocked") == list);
	CHECK(strstr(list, "Until") && strstr(list, "In a row") && strstr(list, "Why"));
	const char *row = strstr(list, B_ADDRESS " ");
	CHECK(row != NULL);
	if(row) {
		char line[512];
		snprintf(line, sizeof(line), "%.*s", (int)strcspn(row, "\n"), row);
		CHECK(strstr(line, FIVE_WRONG) != NULL);
		CHECK(strstr(line, " and others") != NULL);
	}
	// The block is kept at once, for a restart of authd.
	CHECK(strstr(contents(BLOCKED_STATE), B_ADDRESS " ") == contents(BLOCKED_STATE));
	struct stat st;
	CHECK(stat(BLOCKED_STATE, &st) == 0 && (st.st_mode & 07777) == 0600);
	printf("blockingtest: five wrong passwords from b, for five names, blocked its address, and "
		   "firewall blocked says why\n");
}

// What a block stops, and what it leaves: b's connections to port 22 get
// no answer, and its connections to port 7000 and its control
// connection, already open, go on.
static void what_a_block_stops(void) {
	int before = check_failures;
	CHECK(b_reaches("22", 0, 20));
	CHECK(b_reaches("7000", 1, 20));
	CHECK(ask("run echo still-open", "ok 0 still-open"));
	// The client's own message: the connection times out, not refused.
	CHECK(ask("run ssh -n -F /dev/null -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null "
			  "-o ConnectTimeout=5 alice@" A_ADDRESS " true",
			"ok 255 "));
	CHECK(strstr(answer, "timed out") != NULL);
	if(check_failures == before)
		printf("blockingtest: b's connections to port 22 timed out while those to port 7000 went "
			   "through\n");
}

// `firewall unblock` lets b in at once, and says so; a second says that
// it was not blocked.
static void unblock(void) {
	int before = check_failures;
	says(FIREWALL " unblock " B_ADDRESS, 0, "firewall: " B_ADDRESS " is no longer blocked");
	CHECK(b_reaches("22", 1, 20));
	says(FIREWALL " blocked", 0, "No address is blocked.");
	says(FIREWALL " unblock " B_ADDRESS, 1, B_ADDRESS " is not blocked");
	says(FIREWALL " unblock not-an-address", 1, "is not an address that is blocked");
	if(check_failures == before)
		printf("blockingtest: firewall unblock let b in at once, and a second one said it was not "
			   "blocked\n");
}

// b's address in `never` is never blocked: six more wrong passwords block
// nothing. `never-block remove` takes it out again.
static void never_blocked(void) {
	int before = check_failures;
	says(FIREWALL " never-block " B_ADDRESS, 0, "firewall: " B_ADDRESS " is never blocked");
	CHECK(strstr(contents(BLOCKING), "never = [\"" B_ADDRESS "\"]") != NULL);
	// As asked already: said so, and no failure, so a playbook can run it again.
	says(FIREWALL " never-block " B_ADDRESS, 0, B_ADDRESS " is never blocked already");
	says(FIREWALL " never-block 192.168.1.5/24", 1, "its network is 192.168.1.0/24");
	guess_all(NAMES_NEVER);
	says(FIREWALL " blocked", 0, "No address is blocked.");
	CHECK(b_reaches("22", 1, 20));
	says(FIREWALL " never-block remove " B_ADDRESS, 0, "firewall: " B_ADDRESS " may be blocked again");
	CHECK(strstr(contents(BLOCKING), B_ADDRESS) == NULL);
	says(FIREWALL " never-block remove " B_ADDRESS, 0, B_ADDRESS " is not in never, and may be blocked");
	if(check_failures == before)
		printf("blockingtest: with b's address in never, six more wrong passwords blocked "
			   "nothing\n");
}

// A block outlasts a restart of authd: it is in its list again, in the
// file it keeps, and b's port 22 is still shut.
static void restart_keeps_a_block(void) {
	int before = check_failures;
	guess_all(NAMES_RESTART);
	CHECK(wait_for(FIREWALL " blocked", B_ADDRESS, 30));
	CHECK(b_reaches("22", 0, 20));
	says(SERVICE " authd restart", 0, "");
	CHECK(wait_for(FIREWALL " blocked", B_ADDRESS, 60));
	CHECK(strstr(blocked(), FIVE_WRONG) != NULL);
	CHECK(strstr(contents(BLOCKED_STATE), B_ADDRESS " ") == contents(BLOCKED_STATE));
	CHECK(b_reaches("22", 0, 20));
	CHECK(b_reaches("7000", 1, 20));
	if(check_failures == before)
		printf("blockingtest: a block outlasted a restart of authd, in its list, its state file and "
			   "the kernel\n");
}

// While an administrator is signed in from b, b's address is spared: five
// more wrong passwords block nothing.
static void an_administrator_is_spared(void) {
	int before = check_failures;
	says(FIREWALL " unblock " B_ADDRESS, 0, "firewall: " B_ADDRESS " is no longer blocked");
	CHECK(b_reaches("22", 1, 20));
	CHECK(ask("session carol", "ok"));
	// Her session has started once the command it runs does.
	CHECK(wait_for("ps -ef", "sleep 90", 60));
	guess_all(NAMES_ADMIN);
	says(FIREWALL " blocked", 0, "No address is blocked.");
	CHECK(b_reaches("22", 1, 20));
	if(check_failures == before)
		printf("blockingtest: five wrong passwords from b blocked nothing while an administrator "
			   "was signed in from it\n");
}

// What each administrator is told at their next sign-in: carol, the one
// administrator, has not signed in at the console, so the notice of the
// blocks is still kept for her, and counts the two addresses the blocks
// of b were (it was blocked, unblocked and blocked again). The session
// over SSH, with no terminal, shows none.
static void the_administrator_is_told(void) {
	int before = check_failures;
	says("grep '^carol' " NOTICES, 0,
			"Since you last signed in, 2 addresses were blocked for guessing passwords: "
			"`elevate firewall blocked` lists those still blocked.");
	if(check_failures == before)
		printf("blockingtest: the administrator's notice told of the blocks\n");
}

static int a(void) {
	// authd answers once its socket does: `firewall blocked` asks it.
	CHECK(wait_for(FIREWALL " blocked", "No address is blocked.", 60));
	serve_addresses();
	set_up();
	int server = socket(AF_INET, SOCK_STREAM, 0);
	CHECK(server >= 0);
	int yes = 1;
	setsockopt(server, SOL_SOCKET, SO_REUSEADDR, &yes, sizeof(yes));
	struct sockaddr_in at;
	memset(&at, 0, sizeof(at));
	at.sin_family = AF_INET;
	at.sin_port = htons(CONTROL_PORT);
	at.sin_addr.s_addr = htonl(INADDR_ANY);
	CHECK(bind(server, (struct sockaddr *)&at, sizeof(at)) == 0);
	CHECK(listen(server, 8) == 0);
	printf("blockingtest: waiting for b on port %d\n", CONTROL_PORT);
	fflush(stdout);
	struct pollfd p = {server, POLLIN, 0};
	CHECK(poll(&p, 1, 300000) == 1);
	int fd = accept(server, NULL, NULL);
	CHECK(fd >= 0);
	// The listening socket stays open, so that a new connection to port
	// 7000 succeeds while port 22 is shut.
	if(fd >= 0) {
		control_in = fdopen(fd, "r");
		control_out = fdopen(dup(fd), "w");
		CHECK(put(ASKPASS_WRONG, "#!/bin/sh\necho not-the-password\n"));
		CHECK(put(ASKPASS_CAROL, "#!/bin/sh\necho " CAROL_PASSWORD "\n"));
		CHECK(ask("run chmod 755 " ASKPASS_WRONG " " ASKPASS_CAROL, "ok 0"));
		RUN(five_wrong_passwords);
		RUN(what_a_block_stops);
		RUN(unblock);
		RUN(never_blocked);
		RUN(restart_keeps_a_block);
		RUN(an_administrator_is_spared);
		RUN(the_administrator_is_told);
		CHECK(ask("done", "bye"));
		fclose(control_out);
		fclose(control_in);
	}
	close(server);
	return check_end();
}

// --- machine b ----------------------------------------------------------

// The command that has ssh sign in to a as `name`, running `remote`: with
// no terminal, so that ssh asks the askpass program `askpass`, which
// answers each prompt; and with no memory of a's host key, which is no
// one's to check here. One try, so that each connection is one password.
static void ssh_command(char *command, size_t size, const char *name, const char *askpass,
		const char *remote) {
	snprintf(command, size,
			"HOME=/tmp SSH_ASKPASS=%s SSH_ASKPASS_REQUIRE=force exec ssh -n -F /dev/null "
			"-o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR "
			"-o PubkeyAuthentication=no -o PreferredAuthentications=password "
			"-o NumberOfPasswordPrompts=1 -o ConnectTimeout=10 %s@" A_ADDRESS " %s",
			askpass, name, remote);
}

// Whether `name` is one a command may hold unquoted: an account's name.
static int plain_name(const char *name) {
	if(!*name)
		return 0;
	for(const char *at = name; *at; at++)
		if(!((*at >= 'a' && *at <= 'z') || (*at >= '0' && *at <= '9') || *at == '_' || *at == '-'))
			return 0;
	return 1;
}

// Starts ssh as `name`, as a child, running `remote`: its pid, or -1.
static pid_t start_ssh(const char *name, const char *askpass, const char *remote) {
	char command[768];
	ssh_command(command, sizeof(command), name, askpass, remote);
	pid_t pid = check_fork();
	if(pid == 0) {
		execl("/bin/sh", "sh", "-c", command, (char *)NULL);
		_exit(127);
	}
	return pid;
}

// `guess NAMES`: a wrong password for each name, three connections at a
// time, so that authd, which answers four of one process at once, is not
// asked for more: how many ssh ended with 255, which is what ssh says
// when the server refuses what it tried.
static int guess(char *names) {
	char *list[16];
	int count = 0;
	for(char *name = strtok(names, " "); name && count < 16; name = strtok(NULL, " "))
		list[count++] = name;
	int refused = 0;
	for(int first = 0; first < count; first += 3) {
		pid_t pids[3];
		int started = 0;
		for(int i = first; i < count && i < first + 3; i++) {
			pids[started] = plain_name(list[i]) ? start_ssh(list[i], ASKPASS_WRONG, "true") : -1;
			started++;
		}
		for(int i = 0; i < started; i++) {
			int status = pids[i] > 0 ? check_wait(pids[i]) : -1;
			if(WIFEXITED(status) && WEXITSTATUS(status) == 255)
				refused++;
			else
				printf("blockingtest: ssh as %s ended with status %#x, not 255\n", list[first + i],
						status);
		}
	}
	return refused;
}

// Does what a asks, from `from`, and answers it on `to`.
static void follow(FILE *from, FILE *to) {
	char line[1024];
	while(fgets(line, sizeof(line), from)) {
		line[strcspn(line, "\n")] = 0;
		printf("blockingtest: a asks: %s\n", line);
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
		} else if(!strncmp(line, "guess ", 6)) {
			snprintf(reply, sizeof(reply), "ok %d", guess(line + 6));
		} else if(!strncmp(line, "session ", 8)) {
			if(plain_name(line + 8) && start_ssh(line + 8, ASKPASS_CAROL, "sleep 90") > 0)
				snprintf(reply, sizeof(reply), "ok");
			else
				snprintf(reply, sizeof(reply), "no");
		} else if(!strcmp(line, "done")) {
			fprintf(to, "bye\n");
			fflush(to);
			return;
		} else {
			snprintf(reply, sizeof(reply), "what is `%.200s`?", line);
		}
		printf("blockingtest: answering: %s\n", reply);
		fprintf(to, "%s\n", reply);
		fflush(to);
	}
	printf("blockingtest: FAILED: a went before it was done\n");
	check_failures++;
}

// Connects to a's control port once b has its address, waiting for a to
// listen: the socket, or -1. netd on b asks a for the address at boot.
static int reach_a(void) {
	for(int i = 0; i < 300; i++) {
		int fd = socket(AF_INET, SOCK_STREAM, 0);
		struct sockaddr_in to;
		memset(&to, 0, sizeof(to));
		to.sin_family = AF_INET;
		to.sin_port = htons(CONTROL_PORT);
		inet_pton(AF_INET, A_ADDRESS, &to.sin_addr);
		if(fd >= 0 && connect(fd, (struct sockaddr *)&to, sizeof(to)) == 0)
			return fd;
		if(fd >= 0)
			close(fd);
		check_sleep_ms(1000);
	}
	return -1;
}

static int b(void) {
	int fd = reach_a();
	if(fd < 0)
		printf("blockingtest: FAILED: cannot reach a on port %d\n", CONTROL_PORT);
	CHECK(fd >= 0);
	if(fd >= 0) {
		printf("blockingtest: connected to a\n");
		FILE *from = fdopen(fd, "r");
		FILE *to = fdopen(dup(fd), "w");
		follow(from, to);
		fclose(to);
		fclose(from);
	}
	return check_end();
}

int main(int argc, char **argv) {
	check_start("blockingtest");
	if(argc == 2 && !strcmp(argv[1], "a"))
		return a();
	if(argc == 2 && !strcmp(argv[1], "b"))
		return b();
	printf("usage: blockingtest a | b\n");
	return 2;
}

// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The seccomp differential of the linux-suite run (ADR 0628): a seeded
// generator makes 2,000 filters, each of which a child process installs
// before it makes four calls, and the program says, a line for each filter,
// whether the filter was taken, what each call returned and how the child
// ended. It runs on Linux and on Dukovo, and the lines must be the same:
// every action, its data, the order in which a chain of checks refuses a
// program, and the end of a process a filter kills.
//
//   seccomp-diff [SEED [COUNT]]      the lines, then
//                                    `seccomp-diff: COUNT filters, digest D`
//   seccomp-diff --show N [SEED]     the instructions of filter N
//
// The filters are those Linux takes, of every instruction seccomp allows,
// with jumps, tests against the calls' numbers and arguments, and memory
// words stored before they are read; one in ten is changed so that Linux
// refuses it (an opcode seccomp does not allow, a jump past the end, a load
// outside the call's data or not on a word, a division by a constant 0, a
// shift by 32, a memory word read before it is stored, no return at the
// end, too many instructions or none). The value a filter returns is an
// action of each kind, with data, or one Linux does not know, or A. No
// filter reads the call's instruction pointer, which differs between the
// two systems. The calls ignore their arguments (getppid, getuid and
// getgid) or do not exist (999), so that only the filter decides what
// they return; their results are kept in shared memory, which takes no
// call to write. The child then calls exit_group, which the filter judges
// as well.

#ifndef _GNU_SOURCE
#define _GNU_SOURCE
#endif

#include <errno.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/prctl.h>
#include <sys/resource.h>
#include <sys/syscall.h>
#include <sys/wait.h>
#include <unistd.h>

// struct sock_filter and struct sock_fprog (linux/filter.h), here so that
// the program builds with musl, whose headers do not carry Linux's.
struct sock_filter {
	uint16_t code;
	uint8_t jt;
	uint8_t jf;
	uint32_t k;
};
struct sock_fprog {
	unsigned short len;
	const struct sock_filter *filter;
};

// The instructions' parts (linux/bpf_common.h).
#define BPF_LD 0x00
#define BPF_LDX 0x01
#define BPF_ST 0x02
#define BPF_STX 0x03
#define BPF_ALU 0x04
#define BPF_JMP 0x05
#define BPF_RET 0x06
#define BPF_MISC 0x07
#define BPF_W 0x00
#define BPF_H 0x08
#define BPF_B 0x10
#define BPF_IMM 0x00
#define BPF_ABS 0x20
#define BPF_IND 0x40
#define BPF_MEM 0x60
#define BPF_LEN 0x80
#define BPF_MSH 0xa0
#define BPF_ADD 0x00
#define BPF_SUB 0x10
#define BPF_MUL 0x20
#define BPF_DIV 0x30
#define BPF_OR 0x40
#define BPF_AND 0x50
#define BPF_LSH 0x60
#define BPF_RSH 0x70
#define BPF_NEG 0x80
#define BPF_MOD 0x90
#define BPF_XOR 0xa0
#define BPF_JA 0x00
#define BPF_JEQ 0x10
#define BPF_JGT 0x20
#define BPF_JGE 0x30
#define BPF_JSET 0x40
#define BPF_K 0x00
#define BPF_X 0x08
#define BPF_A 0x10
#define BPF_TAX 0x00
#define BPF_TXA 0x80

// linux/seccomp.h and linux/audit.h.
#define AUDIT_ARCH_X86_64 0xc000003eU
#define SECCOMP_SET_MODE_FILTER 1
#define SECCOMP_RET_KILL_PROCESS 0x80000000U
#define SECCOMP_RET_KILL_THREAD 0x00000000U
#define SECCOMP_RET_TRAP 0x00030000U
#define SECCOMP_RET_ERRNO 0x00050000U
#define SECCOMP_RET_USER_NOTIF 0x7fc00000U
#define SECCOMP_RET_TRACE 0x7ff00000U
#define SECCOMP_RET_LOG 0x7ffc0000U
#define SECCOMP_RET_ALLOW 0x7fff0000U

#ifndef SYS_seccomp
#define SYS_seccomp 317
#endif

enum {
	FILTERS = 2000,
	// The instructions of a filter, at most, before a change that makes
	// it longer than Linux takes.
	LONGEST = 48,
	// Linux's BPF_MAXINSNS.
	MAX_INSNS = 4096,
	CALLS = 4,
	// The memory words.
	WORDS = 16,
	// A call that does not exist on either system.
	NO_SUCH_CALL = 999,
};

static const long CALL_NUMBERS[CALLS] = {SYS_getppid, SYS_getuid, SYS_getgid, NO_SUCH_CALL};

// What the child did, which it writes where the parent reads it.
struct results {
	// 0 once the filter is installed, or the errno that refused it; -1
	// before the call to install it returned.
	int installed;
	// Whether each call was made, and what it returned: 0, or minus its
	// errno.
	int reached[CALLS];
	long returned[CALLS];
};

static uint64_t state;

// xorshift64*: the same numbers from the same seed on every system.
static uint64_t next(void) {
	state ^= state >> 12;
	state ^= state << 25;
	state ^= state >> 27;
	return state * 0x2545f4914f6cdd1dULL;
}

static uint32_t below(uint32_t n) {
	return (uint32_t)(next() % n);
}

// One of the actions, with data, or a value Linux does not know.
static uint32_t action(void) {
	uint32_t data = below(4) == 0 ? (uint32_t)next() & 0xffff : below(64);
	switch(below(16)) {
	case 0:
	case 1:
	case 2:
	case 3:
		return SECCOMP_RET_ALLOW | data;
	case 4:
		return SECCOMP_RET_LOG | data;
	case 5:
	case 6:
	case 7:
	case 8:
		return SECCOMP_RET_ERRNO | data;
	case 9:
		return SECCOMP_RET_TRAP | data;
	case 10:
		return SECCOMP_RET_KILL_THREAD | data;
	case 11:
		return SECCOMP_RET_KILL_PROCESS | data;
	case 12:
		return SECCOMP_RET_TRACE | data;
	case 13:
		return SECCOMP_RET_USER_NOTIF | data;
	case 14:
		return SECCOMP_RET_ALLOW;
	default: {
		// An action no version of Linux has.
		static const uint32_t unknown[] = {0x00010000, 0x00020000, 0x00040000, 0x7fe00000,
				0x7ffe0000, 0x40000000, 0xfff00000};
		return unknown[below(sizeof(unknown) / sizeof(unknown[0]))] | data;
	}
	}
}

// A constant worth comparing with: a call's number, the arch, a word of
// an argument the calls are made with, a small number, or any.
static uint32_t constant(const uint64_t args[CALLS][6]) {
	switch(below(8)) {
	case 0:
		return (uint32_t)CALL_NUMBERS[below(CALLS)];
	case 1:
		return AUDIT_ARCH_X86_64;
	case 2:
	case 3: {
		uint64_t arg = args[below(CALLS)][below(6)];
		return below(2) ? (uint32_t)arg : (uint32_t)(arg >> 32);
	}
	case 4:
		return below(17);
	case 5:
		return below(2) ? 0xffffffffU : 0x80000000U;
	default:
		return (uint32_t)next();
	}
}

// An argument: one of a few values, so that filters can match them, or
// any.
static uint64_t argument(void) {
	static const uint64_t usual[] = {0, 1, 2, 0xffffffffULL, 0x100000000ULL, 0x8000000000000000ULL,
			0xdeadbeefcafef00dULL};
	if(below(2))
		return usual[below(sizeof(usual) / sizeof(usual[0]))];
	return next();
}

static struct sock_filter insn(uint16_t code, uint8_t jt, uint8_t jf, uint32_t k) {
	struct sock_filter made = {code, jt, jf, k};
	return made;
}

// A word of `struct seccomp_data` to load, at a multiple of 4: the call's
// number, the arch, or a half of an argument, never the instruction
// pointer (words 2 and 3).
static uint32_t data_word(void) {
	static const uint32_t words[] = {0, 1, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15};
	return 4 * words[below(sizeof(words) / sizeof(words[0]))];
}

// Makes a filter Linux takes into `out`, of `count` instructions, the last
// a return; returns `count`.
static int valid_filter(struct sock_filter *out, const uint64_t args[CALLS][6]) {
	int count = 2 + (int)below(LONGEST - 1);
	// The memory words stored before the first jump, which every path
	// passes through: the only ones a load may read.
	uint16_t stored = 0;
	int jumped = 0;
	static const uint16_t arithmetic[] = {BPF_ADD, BPF_SUB, BPF_MUL, BPF_DIV, BPF_AND, BPF_OR,
			BPF_XOR, BPF_LSH, BPF_RSH};
	static const uint16_t tests[] = {BPF_JEQ, BPF_JGT, BPF_JGE, BPF_JSET};
	for(int at = 0; at < count - 1; at++) {
		// How far a jump from here may go: to the last instruction at
		// most.
		uint32_t room = (uint32_t)(count - 1 - at);
		switch(below(14)) {
		case 0:
		case 1:
		case 2:
			out[at] = insn(BPF_LD | BPF_W | BPF_ABS, 0, 0, data_word());
			break;
		case 3:
			out[at] = insn(below(2) ? BPF_LD | BPF_W | BPF_LEN : BPF_LDX | BPF_W | BPF_LEN, 0, 0, 0);
			break;
		case 4:
			out[at] = insn(below(2) ? BPF_LD | BPF_IMM : BPF_LDX | BPF_IMM, 0, 0, constant(args));
			break;
		case 5: {
			uint32_t word = below(WORDS);
			out[at] = insn(below(2) ? BPF_ST : BPF_STX, 0, 0, word);
			if(!jumped)
				stored |= (uint16_t)(1U << word);
			break;
		}
		case 6: {
			if(stored == 0) {
				out[at] = insn(BPF_MISC | BPF_TAX, 0, 0, 0);
				break;
			}
			uint32_t word;
			do
				word = below(WORDS);
			while(!(stored & (1U << word)));
			out[at] = insn(below(2) ? BPF_LD | BPF_MEM : BPF_LDX | BPF_MEM, 0, 0, word);
			break;
		}
		case 7:
		case 8: {
			uint16_t op = arithmetic[below(sizeof(arithmetic) / sizeof(arithmetic[0]))];
			if(below(3) == 0) {
				out[at] = insn(BPF_ALU | op | BPF_X, 0, 0, 0);
				break;
			}
			uint32_t k = constant(args);
			if(op == BPF_DIV && k == 0)
				k = 1 + below(7);
			if((op == BPF_LSH || op == BPF_RSH) && k >= 32)
				k %= 32;
			out[at] = insn(BPF_ALU | op | BPF_K, 0, 0, k);
			break;
		}
		case 9: {
			static const uint16_t others[] = {BPF_ALU | BPF_NEG, BPF_MISC | BPF_TAX,
					BPF_MISC | BPF_TXA};
			out[at] = insn(others[below(3)], 0, 0, 0);
			break;
		}
		case 10:
		case 11:
		case 12: {
			uint16_t op = tests[below(sizeof(tests) / sizeof(tests[0]))];
			uint16_t source = below(4) == 0 ? BPF_X : BPF_K;
			uint32_t limit = room < 256 ? room : 256;
			out[at] = insn(BPF_JMP | op | source, (uint8_t)below(limit), (uint8_t)below(limit),
					source == BPF_K ? constant(args) : 0);
			jumped = 1;
			break;
		}
		default:
			if(below(4) == 0) {
				out[at] = insn(BPF_RET | BPF_K, 0, 0, action());
			} else {
				out[at] = insn(BPF_JMP | BPF_JA, 0, 0, below(room));
				jumped = 1;
			}
			break;
		}
	}
	out[count - 1] = below(5) == 0 ? insn(BPF_RET | BPF_A, 0, 0, 0)
				       : insn(BPF_RET | BPF_K, 0, 0, action());
	return count;
}

// Changes the filter of `count` instructions in `out`, which has room for
// MAX_INSNS + 1, into one that Linux refuses; returns its new count.
static int spoil(struct sock_filter *out, int count) {
	int at = (int)below((uint32_t)count);
	switch(below(10)) {
	case 0: {
		// Opcodes the BPF machine has and seccomp does not allow, or
		// none at all.
		static const uint16_t refused[] = {BPF_LD | BPF_B | BPF_ABS, BPF_LD | BPF_H | BPF_ABS,
				BPF_LD | BPF_W | BPF_IND, BPF_LDX | BPF_B | BPF_MSH, BPF_ALU | BPF_MOD | BPF_K,
				BPF_ALU | BPF_MOD | BPF_X, 0x07 | 0xf0, BPF_JMP | 0x60};
		out[at] = insn(refused[below(sizeof(refused) / sizeof(refused[0]))], 0, 0, 1);
		return count;
	}
	case 1:
		out[at] = insn(BPF_JMP | BPF_JA, 0, 0, (uint32_t)(count - at - 1 + below(3)));
		return count;
	case 2: {
		uint32_t past = (uint32_t)(count - at - 1);
		uint8_t far = (uint8_t)(past + below(3) < 256 ? past + below(3) : 255);
		out[at] = below(2) ? insn(BPF_JMP | BPF_JEQ | BPF_K, far, 0, 1)
				   : insn(BPF_JMP | BPF_JGT | BPF_K, 0, far, 1);
		if(past > 255)
			out[at] = insn(BPF_JMP | BPF_JA, 0, 0, past);
		return count;
	}
	case 3: {
		static const uint32_t outside[] = {2 * 4, 3 * 4, 64, 68, 1, 2, 6, 0xfffffffcU};
		uint32_t k = outside[below(sizeof(outside) / sizeof(outside[0]))];
		// The instruction pointer's words are inside the data: Linux
		// takes their loads. The others are refused.
		if(k == 8 || k == 12)
			k += 54;
		out[at] = insn(BPF_LD | BPF_W | BPF_ABS, 0, 0, k);
		return count;
	}
	case 4:
		out[at] = insn(BPF_ALU | BPF_DIV | BPF_K, 0, 0, 0);
		return count;
	case 5:
		out[at] = insn(BPF_ALU | (below(2) ? BPF_LSH : BPF_RSH) | BPF_K, 0, 0, 32 + below(64));
		return count;
	case 6:
		// A word read on every path before any store of it: at the
		// start.
		memmove(out + 1, out, sizeof(*out) * (size_t)count);
		out[0] = insn(below(2) ? BPF_LD | BPF_MEM : BPF_LDX | BPF_MEM, 0, 0, below(WORDS));
		// A jump that skipped to the end still lands on it.
		return count + 1;
	case 7:
		out[count - 1] = insn(BPF_LD | BPF_IMM, 0, 0, 7);
		return count;
	case 8:
		for(int i = count - 1; i < MAX_INSNS; i++)
			out[i] = insn(BPF_LD | BPF_IMM, 0, 0, (uint32_t)i);
		out[MAX_INSNS] = insn(BPF_RET | BPF_K, 0, 0, SECCOMP_RET_ALLOW);
		return MAX_INSNS + 1;
	default:
		return 0;
	}
}

// Makes filter `index` of the run of `seed` and its calls' arguments.
static int make(uint64_t seed, int index, struct sock_filter *out, uint64_t args[CALLS][6]) {
	state = seed * 0x9e3779b97f4a7c15ULL + (uint64_t)index * 0xbf58476d1ce4e5b9ULL + 1;
	for(int i = 0; i < 8; i++)
		next();
	for(int call = 0; call < CALLS; call++)
		for(int i = 0; i < 6; i++)
			args[call][i] = argument();
	int count = valid_filter(out, args);
	if(below(10) == 0)
		count = spoil(out, count);
	return count;
}

// Runs filter `index` in a child and says its line; returns the line's
// hash, folded into `digest`.
static uint64_t run(uint64_t seed, int index, struct results *results, uint64_t digest) {
	static struct sock_filter code[MAX_INSNS + 2];
	uint64_t args[CALLS][6];
	int count = make(seed, index, code, args);
	memset(results, 0, sizeof(*results));
	results->installed = -1;
	fflush(stdout);
	pid_t child = fork();
	if(child < 0) {
		perror("seccomp-diff: fork");
		exit(2);
	}
	if(child == 0) {
		struct sock_fprog program = {(unsigned short)count, code};
		if(count > 0xffff)
			program.len = 0xffff;
		long done = syscall(SYS_seccomp, SECCOMP_SET_MODE_FILTER, 0, &program);
		if(done != 0) {
			results->installed = errno;
			_exit(0);
		}
		results->installed = 0;
		for(int call = 0; call < CALLS; call++) {
			results->reached[call] = 1;
			long got = syscall(CALL_NUMBERS[call], args[call][0], args[call][1], args[call][2],
					args[call][3], args[call][4], args[call][5]);
			results->returned[call] = got < 0 ? -errno : 0;
		}
		// exit_group is filtered too. When the filter has it fail, the
		// child ends with SIGILL rather than calling it again, as a C
		// library's _exit does, forever with musl's.
		syscall(SYS_exit_group, 0, 0, 0, 0, 0, 0);
		__builtin_trap();
	}
	int status;
	while(waitpid(child, &status, 0) < 0) {
		if(errno != EINTR) {
			perror("seccomp-diff: waitpid");
			exit(2);
		}
	}
	char line[256];
	int at = snprintf(line, sizeof(line), "%d: %d insns, installed %d, calls", index, count,
			results->installed);
	for(int call = 0; call < CALLS; call++) {
		if(results->reached[call])
			at += snprintf(line + at, sizeof(line) - (size_t)at, " %ld", results->returned[call]);
		else
			at += snprintf(line + at, sizeof(line) - (size_t)at, " -");
	}
	if(WIFSIGNALED(status))
		snprintf(line + at, sizeof(line) - (size_t)at, ", killed by signal %d", WTERMSIG(status));
	else
		snprintf(line + at, sizeof(line) - (size_t)at, ", exit %d", WEXITSTATUS(status));
	printf("%s\n", line);
	// FNV-1a.
	for(const char *c = line; *c; c++)
		digest = (digest ^ (uint8_t)*c) * 0x100000001b3ULL;
	return digest;
}

static void show(uint64_t seed, int index) {
	static struct sock_filter code[MAX_INSNS + 2];
	uint64_t args[CALLS][6];
	int count = make(seed, index, code, args);
	for(int call = 0; call < CALLS; call++) {
		printf("call %ld:", CALL_NUMBERS[call]);
		for(int i = 0; i < 6; i++)
			printf(" %#llx", (unsigned long long)args[call][i]);
		printf("\n");
	}
	for(int i = 0; i < count; i++)
		printf("%4d: code %#06x jt %3u jf %3u k %#010x\n", i, code[i].code, code[i].jt, code[i].jf,
				code[i].k);
}

int main(int argc, char **argv) {
	uint64_t seed = 1;
	int count = FILTERS;
	if(argc > 1 && strcmp(argv[1], "--show") == 0) {
		if(argc < 3) {
			fprintf(stderr, "usage: seccomp-diff --show N [SEED]\n");
			return 2;
		}
		if(argc > 3)
			seed = strtoull(argv[3], NULL, 0);
		show(seed, atoi(argv[2]));
		return 0;
	}
	if(argc > 1)
		seed = strtoull(argv[1], NULL, 0);
	if(argc > 2)
		count = atoi(argv[2]);
	// A filter needs no privilege once the process has none to gain, and
	// the children that SIGSYS ends leave no core behind.
	if(prctl(PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0) {
		perror("seccomp-diff: PR_SET_NO_NEW_PRIVS");
		return 2;
	}
	struct rlimit no_core = {0, 0};
	setrlimit(RLIMIT_CORE, &no_core);
	struct results *results = mmap(NULL, sizeof(struct results), PROT_READ | PROT_WRITE,
			MAP_SHARED | MAP_ANONYMOUS, -1, 0);
	if(results == MAP_FAILED) {
		perror("seccomp-diff: mmap");
		return 2;
	}
	uint64_t digest = 0xcbf29ce484222325ULL;
	for(int index = 0; index < count; index++)
		digest = run(seed, index, results, digest);
	printf("seccomp-diff: %d filters, digest %016llx\n", count, (unsigned long long)digest);
	return 0;
}

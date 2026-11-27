// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The C library's string functions, which work 16 bytes at a time: memcpy,
// memmove, memset, strlen, memcmp, memchr, strchr, strchrnul, strcmp,
// strncmp and strcpy at every alignment and length, against plain
// byte-at-a-time versions here. Each buffer sits in two pages with a page
// nothing may touch on both sides, at every offset from the start, ending
// at or just before the end and across the middle, so that a read or write
// past a buffer's end faults instead of going unseen. Then the case
// functions, tolower, toupper, strcasecmp, strncasecmp and their _l forms,
// in the C locale and in UTF-8 ones, and strstr and memmem, with needles
// that repeat themselves, against the same kind of plain versions.

#include <ctype.h>
#include <locale.h>
#include <stdint.h>
#include <strings.h>
#include <sys/mman.h>

#include "check.h"

// The functions are called through these so that the compiler neither
// replaces a call with code of its own nor folds it.
static void *(*volatile memcpy_fn)(void *, const void *, size_t) = memcpy;
static void *(*volatile memmove_fn)(void *, const void *, size_t) = memmove;
static void *(*volatile memset_fn)(void *, int, size_t) = memset;
static size_t (*volatile strlen_fn)(const char *) = strlen;
static int (*volatile memcmp_fn)(const void *, const void *, size_t) = memcmp;
static void *(*volatile memchr_fn)(const void *, int, size_t) = memchr;
static char *(*volatile strchr_fn)(const char *, int) = strchr;
static char *(*volatile strchrnul_fn)(const char *, int) = strchrnul;
static int (*volatile strcmp_fn)(const char *, const char *) = strcmp;
static int (*volatile strncmp_fn)(const char *, const char *, size_t) = strncmp;
static char *(*volatile strcpy_fn)(char *, const char *) = strcpy;
static int (*volatile strcasecmp_fn)(const char *, const char *) = strcasecmp;
static int (*volatile strncasecmp_fn)(const char *, const char *, size_t) = strncasecmp;
static char *(*volatile strstr_fn)(const char *, const char *) = strstr;
static void *(*volatile memmem_fn)(const void *, size_t, const void *, size_t) = memmem;

// The references stay as loops: the compiler must not turn them into the
// calls they check.
#pragma clang optimize off

static size_t ref_strlen(const char *s) {
	size_t n = 0;
	while(s[n])
		n++;
	return n;
}

static void *ref_memchr(const void *s, int c, size_t n) {
	const unsigned char *p = s;
	for(size_t i = 0; i < n; i++)
		if(p[i] == (unsigned char)c)
			return (void *)(p + i);
	return NULL;
}

static char *ref_strchrnul(const char *s, int c) {
	while(*s && *s != (char)c)
		s++;
	return (char *)s;
}

static char *ref_strchr(const char *s, int c) {
	s = ref_strchrnul(s, c);
	return *s == (char)c ? (char *)s : NULL;
}

static int ref_memcmp(const void *a, const void *b, size_t n) {
	const unsigned char *x = a, *y = b;
	for(size_t i = 0; i < n; i++)
		if(x[i] != y[i])
			return x[i] - y[i];
	return 0;
}

static int ref_strncmp(const char *a, const char *b, size_t n) {
	const unsigned char *x = (const unsigned char *)a, *y = (const unsigned char *)b;
	for(size_t i = 0; i < n; i++) {
		if(x[i] != y[i])
			return x[i] - y[i];
		if(!x[i])
			return 0;
	}
	return 0;
}

// Moves like memmove, byte by byte in the order that keeps the source.
static void ref_memmove(unsigned char *d, const unsigned char *s, size_t n) {
	if(d < s)
		for(size_t i = 0; i < n; i++)
			d[i] = s[i];
	else
		for(size_t i = n; i > 0; i--)
			d[i - 1] = s[i - 1];
}

// A byte in lowercase as the C locale has it: only ASCII letters change.
static int ref_lower(int c) {
	return c >= 'A' && c <= 'Z' ? c - 'A' + 'a' : c;
}

static int ref_upper(int c) {
	return c >= 'a' && c <= 'z' ? c - 'a' + 'A' : c;
}

static int ref_strncasecmp(const char *a, const char *b, size_t n) {
	const unsigned char *x = (const unsigned char *)a, *y = (const unsigned char *)b;
	for(size_t i = 0; i < n; i++) {
		int cx = ref_lower(x[i]), cy = ref_lower(y[i]);
		if(cx != cy)
			return cx - cy;
		if(!cx)
			return 0;
	}
	return 0;
}

static char *ref_strstr(const char *h, const char *n) {
	size_t l = ref_strlen(n);
	for(;; h++) {
		if(ref_strncmp(h, n, l) == 0)
			return (char *)h;
		if(!*h)
			return NULL;
	}
}

static void *ref_memmem(const void *h, size_t k, const void *n, size_t l) {
	const unsigned char *x = h;
	if(l == 0)
		return (void *)h;
	for(size_t i = 0; i + l <= k; i++)
		if(ref_memcmp(x + i, n, l) == 0)
			return (void *)(x + i);
	return NULL;
}

#pragma clang optimize on

// A pattern with no runs, to see a byte that moves to the wrong place.
static void fill_pattern(unsigned char *a, size_t n) {
	for(size_t i = 0; i < n; i++)
		a[i] = (unsigned char)(i * 7 + 3);
}

// A part stops once this many checks have failed, so that a function
// that is badly wrong fills a screen, not the log.
#define GIVE_UP() \
	do { \
		if(check_failures >= 20) \
			return; \
	} while(0)

#define PAGE 4096
#define AREA (2 * PAGE)

static int sign(int x) {
	return (x > 0) - (x < 0);
}

// The two areas the buffers go in.
static unsigned char *area_a, *area_b;

// Two pages with a page no access is allowed to on each side.
static unsigned char *make_area(void) {
	unsigned char *p = mmap(NULL, 4 * PAGE, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
	if(p == MAP_FAILED) {
		check_failed(__FILE__, __LINE__, "mmap of the test area", errno);
		exit(125);
	}
	CHECK(mprotect(p, PAGE, PROT_NONE) == 0);
	CHECK(mprotect(p + 3 * PAGE, PAGE, PROT_NONE) == 0);
	return p + PAGE;
}

enum { PLACES = 35 };

// Where in `base` the `which`th place of an `n` byte buffer is: 0..15 at
// that offset from the start; 16..31 ending that many bytes before the
// end; then across the middle page boundary, just after it, and at the
// very end.
static unsigned char *place(unsigned char *base, size_t n, size_t which) {
	if(which < 16)
		return base + which;
	if(which < 32)
		return base + AREA - n - (which - 16);
	if(which == 32)
		return base + PAGE - n / 2;
	if(which == 33)
		return base + PAGE - 1 > base + AREA - n ? base : base + PAGE - 1;
	return base + AREA - n;
}

// How the lengths from 0 on are stepped: each up to `exact`, then by `step`.
static size_t next(size_t n, size_t exact, size_t step) {
	return n < exact ? n + 1 : n + step;
}

static void strlens(void) {
	for(size_t len = 0; len < 200; len = next(len, 70, 9))
		for(size_t w = 0; w < PLACES; w++) {
			GIVE_UP();
			memset(area_a, 'x', AREA);
			char *s = (char *)place(area_a, len + 1, w);
			memset(s, 'a' + len % 20, len);
			s[len] = 0;
			CHECK(strlen_fn(s) == len);
		}
}

static void memchrs(void) {
	for(size_t n = 0; n < 130; n = next(n, 66, 5))
		for(size_t w = 0; w < PLACES; w++)
			for(long k = -1; k < (long)n; k += n > 40 ? 11 : 3) {
				GIVE_UP();
				memset(area_a, 'q', AREA);
				unsigned char *s = place(area_a, n, w);
				for(size_t i = 0; i < n; i++)
					s[i] = 'b' + i % 13;
				// The wanted byte just outside the buffer too.
				if(s > area_a)
					s[-1] = 0xAA;
				if(s + n < area_a + AREA)
					s[n] = 0xAA;
				if(k >= 0)
					s[k] = 0xAA;
				if(k >= 0 && k + 3 < (long)n)
					s[k + 3] = 0xAA;
				void *want = ref_memchr(s, 0xAA, n);
				// The byte as a negative int, which memchr takes as unsigned char.
				void *got = memchr_fn(s, 0xAA, n);
				void *negative = memchr_fn(s, 0xAA - 256, n);
				CHECK(got == want);
				CHECK(negative == want);
			}
	// A length that runs to the end of memory, with the byte there, as
	// programs use memchr like rawmemchr.
	memset(area_a, 1, AREA);
	area_a[AREA - 1] = 7;
	for(size_t off = 0; off < 32; off++)
		CHECK(memchr_fn(area_a + off, 7, SIZE_MAX) == area_a + AREA - 1);
	CHECK(memchr_fn(area_a + AREA - 1, 7, SIZE_MAX - 5) == area_a + AREA - 1);
}

static void strchrs(void) {
	for(size_t len = 0; len < 130; len = next(len, 66, 7))
		for(size_t w = 0; w < PLACES; w++)
			for(long k = -1; k < (long)len; k += len > 40 ? 11 : 3)
				for(int c = 0; c < 3; c++) {
					GIVE_UP();
					// A byte, the end, and a byte with the high bit as a negative int.
					int wanted = c == 0 ? 0xAA : c == 1 ? 0 : (int)(signed char)0xAA;
					memset(area_a, 0xAA, AREA);
					char *s = (char *)place(area_a, len + 1, w);
					for(size_t i = 0; i < len; i++)
						s[i] = 'b' + i % 13;
					s[len] = 0;
					if(k >= 0)
						s[k] = (char)0xAA;
					CHECK(strchr_fn(s, wanted) == ref_strchr(s, wanted));
					CHECK(strchrnul_fn(s, wanted) == ref_strchrnul(s, wanted));
				}
}

static void memcmps(void) {
	for(size_t n = 0; n < 130; n = next(n, 66, 5))
		for(size_t wa = 0; wa < PLACES; wa++)
			for(size_t wb = 0; wb < PLACES; wb += 4)
				for(long k = -1; k < (long)n; k += n > 40 ? 17 : 5) {
					GIVE_UP();
					unsigned char *a = place(area_a, n, wa), *b = place(area_b, n, wb);
					for(size_t i = 0; i < n; i++)
						a[i] = b[i] = 'c' + i % 11;
					if(k >= 0) {
						b[k] = (k & 1) ? 0x80 : 0x01;
						if(k + 1 < (long)n)
							b[k + 1] = 0xFF;
					}
					int want = ref_memcmp(a, b, n);
					CHECK(sign(memcmp_fn(a, b, n)) == sign(want));
					CHECK(sign(memcmp_fn(b, a, n)) == -sign(want));
				}
}

static void strcmps(void) {
	for(size_t la = 0; la < 70; la++)
		for(size_t lb = la; lb < la + 3 && lb < 70; lb++)
			for(size_t wa = 0; wa < PLACES; wa++)
				for(size_t wb = 0; wb < PLACES; wb += 5)
					for(long k = -1; k < (long)la; k += la > 20 ? 9 : 4) {
						GIVE_UP();
						char *a = (char *)place(area_a, la + 1, wa);
						char *b = (char *)place(area_b, lb + 1, wb);
						for(size_t i = 0; i < la; i++)
							a[i] = 'a' + i % 7;
						for(size_t i = 0; i < lb; i++)
							b[i] = 'a' + i % 7;
						a[la] = 0;
						b[lb] = 0;
						if(k >= 0)
							b[k] = (k & 1) ? (char)0xC3 : 'Z';
						int want = ref_strncmp(a, b, SIZE_MAX);
						CHECK(sign(strcmp_fn(a, b)) == sign(want));
						CHECK(sign(strcmp_fn(b, a)) == -sign(want));
						// n from 0 to past both lengths.
						for(size_t n = 0; n < lb + 20; n += n < 17 ? 1 : n < 40 ? 3 : 11) {
							int limited = ref_strncmp(a, b, n);
							CHECK(sign(strncmp_fn(a, b, n)) == sign(limited));
							CHECK(sign(strncmp_fn(b, a, n)) == -sign(limited));
						}
						CHECK(sign(strncmp_fn(a, b, SIZE_MAX)) == sign(want));
						CHECK(sign(strncmp_fn(b, a, SIZE_MAX)) == -sign(want));
					}
}

static unsigned char reference[AREA];

static void moves(void) {
	// An overlap in each direction and none, as `gap` bytes from the source.
	for(size_t n = 0; n < 400; n = next(n, 100, 31))
		for(long gap = -150; gap <= 150; gap += gap > -20 && gap < 20 ? 1 : 13)
			for(size_t align = 0; align < 16; align += n < 70 ? 1 : 5)
				for(int end = 0; end < 2; end++) {
					GIVE_UP();
					long low = gap < 0 ? -gap : 0;
					size_t span = n + (size_t)(gap < 0 ? -gap : gap);
					unsigned char *base = end ? area_a + AREA - span - align : area_a + align;
					unsigned char *s = base + low, *d = s + gap;
					fill_pattern(area_a, AREA);
					memcpy(reference, area_a, AREA);
					ref_memmove(reference + (d - area_a), reference + (s - area_a), n);
					memmove_fn(d, s, n);
					CHECK(memcmp(reference, area_a, AREA) == 0);
					// memcpy only where the two do not overlap.
					if(gap >= (long)n || -gap >= (long)n) {
						fill_pattern(area_a, AREA);
						memcpy_fn(d, s, n);
						CHECK(memcmp(reference, area_a, AREA) == 0);
					}
				}
	// Between the two areas, each against its guard pages.
	for(size_t n = 0; n < AREA; n += n < 300 ? 3 : 211)
		for(size_t w = 0; w < 4; w++) {
			unsigned char *s = w & 1 ? area_a + AREA - n : area_a;
			unsigned char *d = w & 2 ? area_b + AREA - n : area_b;
			for(size_t i = 0; i < n; i++)
				s[i] = (unsigned char)(i * 13 + w);
			memset(area_b, 0, AREA);
			memcpy_fn(d, s, n);
			CHECK(memcmp(d, s, n) == 0);
		}
}

static void memsets(void) {
	for(size_t n = 0; n < AREA; n += n < 300 ? 1 : 199)
		for(size_t w = 0; w < PLACES; w += n < 70 ? 1 : 3) {
			GIVE_UP();
			memset(area_a, 0x11, AREA);
			unsigned char *d = place(area_a, n, w);
			// The byte is the low 8 bits of the int: 0xF2.
			memset_fn(d, 0x1F2, n);
			size_t bad = 0;
			for(size_t i = 0; i < AREA; i++) {
				int inside = area_a + i >= d && area_a + i < d + n;
				if(area_a[i] != (inside ? 0xF2 : 0x11))
					bad++;
			}
			CHECK(bad == 0);
		}
}

static void strcpys(void) {
	for(size_t len = 0; len < 130; len = next(len, 66, 7))
		for(size_t w = 0; w < PLACES; w++) {
			GIVE_UP();
			memset(area_a, 'x', AREA);
			memset(area_b, 0x55, AREA);
			char *s = (char *)place(area_a, len + 1, w);
			memset(s, 'k', len);
			s[len] = 0;
			char *d = (char *)place(area_b, len + 1, (w + 5) % PLACES);
			CHECK(strcpy_fn(d, s) == d);
			CHECK(ref_strlen(d) == len && ref_memcmp(d, s, len + 1) == 0);
		}
}

// The same numbers on every run: a linear congruential generator.
static uint32_t seed = 1;

static uint32_t pick(uint32_t n) {
	seed = seed * 1103515245 + 12345;
	return (seed >> 16) % n;
}

// Bytes the case functions are tried on: letters of both cases, the bytes
// just outside the letters, and bytes above 0x7F, which only ASCII
// letters' cases leave as they are, in the C locale and in UTF-8.
static const unsigned char case_bytes[] = {
	'a', 'A', 'm', 'M', 'z', 'Z', '@', '[', '`', '{', '1', 0xC3, 0xA9, 0xC9, 0xE9, 0xFF,
};

static void case_strings(const char *name) {
	CHECK(setlocale(LC_ALL, name) != NULL);
	locale_t l = newlocale(LC_ALL_MASK, name, (locale_t)0);
	CHECK(l != (locale_t)0);
	if(!l)
		return;
	for(int c = -1; c < 256; c++) {
		GIVE_UP();
		CHECK(tolower(c) == ref_lower(c));
		CHECK(toupper(c) == ref_upper(c));
		CHECK(tolower_l(c, l) == ref_lower(c));
		CHECK(toupper_l(c, l) == ref_upper(c));
	}
	for(int round = 0; round < 3000; round++) {
		GIVE_UP();
		size_t la = pick(40), lb = la + pick(3);
		char *a = (char *)place(area_a, la + 1, pick(PLACES));
		char *b = (char *)place(area_b, lb + 1, pick(PLACES));
		for(size_t i = 0; i < la; i++) {
			unsigned char c = case_bytes[pick(sizeof(case_bytes))];
			a[i] = (char)c;
			// The same letter, often in the other case.
			b[i] = (char)(pick(2) ? ref_upper(c) : ref_lower(c));
		}
		for(size_t i = la; i < lb; i++)
			b[i] = (char)case_bytes[pick(sizeof(case_bytes))];
		// Now and then a byte that differs.
		if(la && pick(3) == 0)
			b[pick((uint32_t)la)] = (char)case_bytes[pick(sizeof(case_bytes))];
		a[la] = 0;
		b[lb] = 0;
		int want = ref_strncasecmp(a, b, SIZE_MAX);
		CHECK(sign(strcasecmp_fn(a, b)) == sign(want));
		CHECK(sign(strcasecmp_fn(b, a)) == -sign(want));
		CHECK(sign(strcasecmp_l(a, b, l)) == sign(want));
		for(size_t n = 0; n < lb + 3; n++) {
			int limited = ref_strncasecmp(a, b, n);
			CHECK(sign(strncasecmp_fn(a, b, n)) == sign(limited));
			CHECK(sign(strncasecmp_l(b, a, n, l)) == -sign(limited));
		}
	}
	freelocale(l);
	setlocale(LC_ALL, "C");
}

static void cases(void) {
	case_strings("C");
	case_strings("C.utf8");
	case_strings("en_US.UTF-8");
	// A byte that is not a whole UTF-8 character compares as itself, not
	// as the end of the string.
	CHECK(strncasecmp_fn("\xc3\xa9" "a", "\xc3\xa9" "b", 3) < 0);
	CHECK(strcasecmp_fn("\xc3\xa9" "a", "\xc3\xa9" "A") == 0);
}

// A haystack and a needle of letters from a small alphabet, so that the
// needle repeats itself and is found often, and often almost.
static void fill_letters(char *s, size_t n, uint32_t letters) {
	for(size_t i = 0; i < n; i++)
		s[i] = (char)('a' + pick(letters));
}

static void searches(void) {
	for(int round = 0; round < 20000; round++) {
		GIVE_UP();
		uint32_t letters = 1 + pick(3);
		size_t hl = round % 8 ? pick(48) : pick(400);
		size_t nl = round % 5 ? pick(9) : pick(70);
		// The haystack at the end of its area, and the needle at the end
		// of the other, so that reading past either faults.
		char *h = (char *)area_a + AREA - hl - 1;
		char *n = (char *)area_b + AREA - nl - 1;
		fill_letters(h, hl, letters);
		fill_letters(n, nl, letters);
		if(nl <= hl && pick(2))
			memcpy(h + pick((uint32_t)(hl - nl + 1)), n, nl);
		h[hl] = 0;
		n[nl] = 0;
		CHECK(strstr_fn(h, n) == ref_strstr(h, n));
		// memmem with no terminating byte, at the very end of the areas.
		CHECK(memmem_fn(h + 1, hl, n + 1, nl) == ref_memmem(h + 1, hl, n + 1, nl));
		CHECK(memmem_fn(h, hl, n, nl) == ref_memmem(h, hl, n, nl));
	}
	// A needle longer than the haystack, and an empty one.
	CHECK(strstr_fn("abc", "abcd") == NULL);
	CHECK(strstr_fn("", "") != NULL);
	CHECK(memmem_fn("abc", 3, "abcd", 4) == NULL);
	CHECK(memmem_fn("abc", 0, "", 0) != NULL);
}

int main(void) {
	check_start("strings");
	area_a = make_area();
	area_b = make_area();
	RUN(memsets);
	RUN(strlens);
	RUN(memchrs);
	RUN(strchrs);
	RUN(memcmps);
	RUN(strcmps);
	RUN(moves);
	RUN(strcpys);
	RUN(cases);
	RUN(searches);
	return check_end();
}

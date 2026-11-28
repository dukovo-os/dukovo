<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0800. chdir is one call to the kernel

- Status: Accepted
- Date: 2026-11-28

## Context

On optiplex-1, a Dell OptiPlex 7040 with a Core i5-6500, the lab's
`server_php_fpm` row took 42.2 µs a request against Linux's 39.7 µs.
With the protections both systems run on that processor by default
(page-table isolation, IBRS on entry and a buffer clear on exit,
ADR 0790), a call to the kernel costs about a microsecond on either, so
the count of calls a request makes matters as much as what each does.
A traced pass of 5,000 requests counted about 40 calls a request in
PHP-FPM's worker against Linux's 35, and 14.7 in nginx against 13.

PHP-FPM changes its working directory twice a request, to the script's
directory and back. The C library keeps the working directory as a
string (ADR 0089, radix-posix(7)), and its `chdir` made four calls:
radix-directory(2)'s `stat`, to find a directory; `check_access` with
EXECUTE_OK, since going in needs the search bit (ADR 0214); `real_path`,
for where it really is, links followed; and radix-process(2)'s
`set_directory`, which notes it for /processes (radix-proc(5)). Linux's
`chdir` is one call, and PHP's `getcwd` one more, which the C library
answers from its string. That was eight calls a request against three.

## Decision

- **radix-directory(2) has `enter`**, method 56, and the interface is
  at version 13: it walks the path once, as `real_path` does, and fails
  with not_directory if it leads to no directory and with denied if the
  name the capability acts for may not search it, in the order stat(2)
  and then access(2) with X_OK would give them. It writes where the
  directory is into the caller's buffer and returns its length; a path
  the buffer cannot hold, or longer than PATH_MAX, fails with
  name_too_long. It needs the read right, as the three calls it
  replaces did.
- **`enter` notes the directory as the caller's working directory**, as
  `set_directory` does: `/` and the path it wrote, or `/` alone for the
  capability's own directory. The path is made in the kernel's memory
  and copied to the caller's buffer from there, so that what is noted
  is what the walk found. The note is only what /processes shows: the
  C library still keeps the working directory and resolves paths from
  it, and a program could already note any absolute path with
  `set_directory`, so the method gives no program anything it could
  not do.
- **The C library's `chdir` and `fchdir` call `enter` on the root
  capability** with the path joined to the working directory, as every
  other call by path does, and keep the path it returns. They make one
  call where they made four. `set_directory` stays, for a program that
  keeps its working directory some other way.

## Alternatives

- **`enter` without the note, and `set_directory` after it.** Two calls
  where Linux makes one; the note is what the C library makes the
  second call for.
- **Read the working directory from the process's memory when
  /processes is read**, as Linux reads a process's arguments for
  `cmdline`. The C library would make no call at all, but the kernel
  would read a string another thread may be writing, and a process
  that keeps its working directory some other way would show a stale
  one.
- **Keep the working directory in the kernel**, as Linux does, and
  resolve relative paths from it there. That is a second root for every
  call by path, beside the capability the call names, which the design
  of capabilities that name what they reach avoids (ADR 0089).

## Consequences

- `chdir` and `fchdir` are one call to the kernel, and a PHP-FPM request
  makes six fewer.
- The VFS's test checks that `enter` follows links and works out `.`
  and `..` as `real_path` does, that a missing path, a file and a link
  to a file fail before the search bit is looked at, that a directory
  without it, or one walked through without it, fails with denied but
  for the kernel's own calls, and that a short buffer gets the start of
  the path and its whole length. `posix/files` checks that `chdir`
  into a directory, through a link and with `..`, makes `getcwd` and
  /processes say where it is, and that a file, a missing path and a
  directory without the search bit fail as on Linux.

<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

Section: Specifications and papers
- Linux's seccomp filters, as seccomp(2), prctl(2) and Linux's
  `Documentation/userspace-api/seccomp_filter.rst` describe them: the
  programs a Linux program installs, the checks they pass, the actions
  they return and which wins along a chain, and the calls a filter
  allows whatever their arguments, which the kernel need not run it for:
  radix-bpf and the Linux table's seccomp
  ([ADR 0623](docs/adr/0623-linux-programs-filter-their-own-calls-with-seccomp.md)).
  We use no code of Linux's.

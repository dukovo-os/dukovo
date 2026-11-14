// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the program of the `dynamic` test gives the plugin it opens: a
// function and a variable of its own, which the plugin calls and changes
// through the program's dynamic symbols (the program is linked with
// --export-dynamic), as PostgreSQL's modules call and change the server's.

#ifndef POSIX_DYNAMIC_PROGRAM_H
#define POSIX_DYNAMIC_PROGRAM_H

// Data of the program's that the plugin reads and changes.
extern int program_shared;

// Three times `number`, plus program_shared.
int program_answer(int number);

#endif

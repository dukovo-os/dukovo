gofmt(1) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

gofmt - set out the source of programs in Go

# SYNOPSIS

*gofmt* [*-l*] [*-w*] [*-d*] [*-s*] [_path_...]

# DESCRIPTION

*gofmt* reads Go source and writes it again in the one layout Go's
programs share: tabs for indentation, spaces for alignment, one way to
break a line. With no _path_ it reads standard input and writes standard
output. A folder stands for the _.go_ files in it and beneath it.

A file with a syntax error is reported, with its line, and left as it
is.

# OPTIONS

*-l*
	List the files whose layout differs, and write nothing else.

*-w*
	Write the result into each file, in place of printing it.

*-d*
	Print each difference as a diff.

*-s*
	Also simplify the code where a shorter form means the same, such as
	a slice expression's upper bound that is the length.

*-e*
	Report every syntax error, not the first ten.

*-r* _rule_
	Rewrite the code by a rule of the form _pattern_ -> _replacement_.

# EXIT STATUS

0 on success; 2 when a file could not be read or parsed.

# SEE ALSO

*go*(1)

https://go.dev/doc

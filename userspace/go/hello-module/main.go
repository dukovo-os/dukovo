// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Command hello greets the names it is given, or Alice and Bob, a line
// each, and then prints the greetings as JSON. The go-toolchain boot test
// builds, vets, tests and runs it with the go and gofmt of the Go bundle
// (tools/xtask/src/test/cases/go-toolchain.rs).
package main

import (
	"fmt"
	"os"

	"example.com/hello/greet"
)

func main() {
	names := os.Args[1:]
	if len(names) == 0 {
		names = []string{"alice", "bob"}
	}
	if err := greet.Write(names); err != nil {
		fmt.Fprintln(os.Stderr, "hello:", err)
		os.Exit(1)
	}
}

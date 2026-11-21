// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Package greet writes greetings for the go-toolchain boot test's program
// (tools/xtask/src/test/cases/go-toolchain.rs).
package greet

import (
	"encoding/json"
	"fmt"
	"os"
	"sort"
	"strings"
	"sync"
)

// Greeting is one line of the program's output, and a record in its JSON.
type Greeting struct {
	Name string `json:"name"`
	Line string `json:"line"`
}

// Line is the greeting for name, with its first letter in capitals.
func Line(name string) string {
	name = strings.TrimSpace(name)
	if name == "" {
		return "Hello, nobody!"
	}
	return fmt.Sprintf("Hello, %s!", strings.ToUpper(name[:1])+name[1:])
}

// All greets each of names in a goroutine of its own and returns the
// greetings sorted by name, whatever order the goroutines finished in.
func All(names []string) []Greeting {
	var (
		wait  sync.WaitGroup
		mutex sync.Mutex
		found []Greeting
	)
	for _, name := range names {
		wait.Add(1)
		go func() {
			defer wait.Done()
			greeting := Greeting{Name: name, Line: Line(name)}
			mutex.Lock()
			found = append(found, greeting)
			mutex.Unlock()
		}()
	}
	wait.Wait()
	sort.Slice(found, func(i, j int) bool { return found[i].Name < found[j].Name })
	return found
}

// Write prints the greetings of names to standard output, a line each, and
// then the same as JSON.
func Write(names []string) error {
	greetings := All(names)
	for _, greeting := range greetings {
		fmt.Fprintln(os.Stdout, greeting.Line)
	}
	text, err := json.Marshal(greetings)
	if err != nil {
		return err
	}
	_, err = fmt.Fprintf(os.Stdout, "%s\n", text)
	return err
}

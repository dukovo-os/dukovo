// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Command go-runtime-test checks that Go's runtime and the packages that
// reach the system work on Radix: threads, channels, timers, panics,
// signals, memory, files, processes, time zones and random numbers. The
// go-runtime boot test runs it from the test disk
// (tools/xtask/src/test/cases/go-runtime.rs); its lines are the first place
// to look when a Go program misbehaves on Radix.
//
// Usage:
//
//	go-runtime-test CPUS
//
// CPUS is the number of CPUs the machine has, which GOMAXPROCS must equal.
// Each check prints "go-runtime: NAME ok" or "go-runtime: NAME FAILED: WHY",
// and the last line is "go-runtime: N checks, M failed". The exit status is 1
// if a check failed. A few checks run the program again as a child, with
// "child KIND" as its arguments, to see how a process ends or what it
// inherits.
package main

import (
	"fmt"
	"os"
	"strconv"
	"time"
)

// A check is one thing the test proves, under a time limit of its own.
type check struct {
	name  string
	run   func() error
	limit time.Duration
}

// watchdogGrace is how long past its limit a check may keep the program
// before the watchdog ends it.
const watchdogGrace = 10 * time.Second

func main() {
	if len(os.Args) == 3 && os.Args[1] == "child" {
		runChild(os.Args[2])
		return
	}
	if len(os.Args) != 2 {
		fmt.Fprintln(os.Stderr, "usage: go-runtime-test CPUS")
		os.Exit(2)
	}
	cpus, err := strconv.Atoi(os.Args[1])
	if err != nil || cpus < 1 {
		fmt.Fprintf(os.Stderr, "go-runtime-test: %q is not a number of CPUs\n", os.Args[1])
		os.Exit(2)
	}

	checks := []check{
		{"threads", func() error { return threads(cpus) }, time.Minute},
		{"channels-and-mutexes", channelsAndMutexes, time.Minute},
		{"timers", timers, time.Minute},
		{"clocks", clocks, time.Minute},
		{"preemption", preemption, time.Minute},
		{"panic-trace", panicTrace, time.Minute},
		{"nil-dereference", nilDereference, time.Minute},
		{"signals", signals, time.Minute},
		{"memory", memory, 5 * time.Minute},
		{"files", files, time.Minute},
		{"pipes-and-exec", pipesAndExec, time.Minute},
		{"environment", environment, time.Minute},
		{"time-zones", timeZones, time.Minute},
		{"random", random, time.Minute},
	}
	failed := 0
	for _, c := range checks {
		if err := runWithin(c); err != nil {
			fmt.Printf("go-runtime: %s FAILED: %v\n", c.name, err)
			failed++
			continue
		}
		fmt.Printf("go-runtime: %s ok\n", c.name)
	}
	fmt.Printf("go-runtime: %d checks, %d failed\n", len(checks), failed)
	if failed > 0 {
		os.Exit(1)
	}
}

// runWithin runs a check and gives up on it at its limit, so that a check
// that never ends fails instead of holding the boot test until its own
// timeout. Two things watch it: the select below, and a timer function that
// ends the program from a goroutine of its own if the goroutine waiting
// here is never woken.
func runWithin(c check) error {
	watchdog := time.AfterFunc(c.limit+watchdogGrace, func() {
		fmt.Printf("go-runtime: %s FAILED: still running %v after its limit; the program stops\n",
			c.name, watchdogGrace)
		os.Exit(1)
	})
	defer watchdog.Stop()

	finished := make(chan error, 1)
	go func() { finished <- c.run() }()
	select {
	case err := <-finished:
		return err
	case <-time.After(c.limit):
		return fmt.Errorf("no answer within %v", c.limit)
	}
}

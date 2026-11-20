// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

package main

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"os/signal"
	"runtime"
	"strings"
	"sync/atomic"
	"syscall"
	"time"
)

// childLimit is how long a child that should end at once may take.
const childLimit = 30 * time.Second

// runChild is the program run as a child, "child KIND": each kind ends the
// process in the way a check wants to see.
func runChild(kind string) {
	switch kind {
	case "panic":
		panicChild()
	case "nil":
		nilChild()
	case "sigint":
		sigintChild()
	case "preempt":
		preemptChild()
	case "pid":
		fmt.Println(os.Getpid())
	case "zone":
		zoneChild()
	default:
		fmt.Fprintf(os.Stderr, "go-runtime-test: no child of kind %q\n", kind)
		os.Exit(2)
	}
}

// runSelf runs this program as a child of kind `kind`, with the environment
// env (the parent's when nil), and kills it after limit. The error is nil
// for an exit with status 0.
func runSelf(limit time.Duration, env []string, kind string) (stdout, stderr string, err error) {
	ctx, cancel := context.WithTimeout(context.Background(), limit)
	defer cancel()
	cmd := exec.CommandContext(ctx, os.Args[0], "child", kind)
	cmd.Env = env
	var out, errs bytes.Buffer
	cmd.Stdout, cmd.Stderr = &out, &errs
	err = cmd.Run()
	if err != nil && ctx.Err() != nil {
		err = fmt.Errorf("killed after %v: %w", limit, err)
	}
	return out.String(), errs.String(), err
}

// exitCode is the status a child ended with: 0 for no error, -1 when the
// child did not end by exiting.
func exitCode(err error) int {
	if err == nil {
		return 0
	}
	var exit *exec.ExitError
	if errors.As(err, &exit) {
		return exit.ExitCode()
	}
	return -1
}

// excerpt is the start of text, short enough for a line of a report.
func excerpt(text string) string {
	const most = 300
	text = strings.Join(strings.Fields(text), " ")
	if len(text) > most {
		return text[:most] + "..."
	}
	return text
}

// nothing is a pointer that is always nil, so that the compiler has no
// dereference of it to reject or remove.
var nothing *int

// panicChild panics from a function the traceback can name.
//
//go:noinline
func panicChild() {
	panic("go-runtime deliberate panic")
}

// nilChild dereferences a nil pointer: the kernel's fault must reach Go's
// signal handler and come out as a panic.
//
//go:noinline
func nilChild() {
	fmt.Println(*nothing)
}

// sigintChild signals itself with SIGINT, which nothing in this process
// catches, and waits to be ended by it.
func sigintChild() {
	if err := syscall.Kill(syscall.Getpid(), syscall.SIGINT); err != nil {
		fmt.Fprintf(os.Stderr, "kill: %v\n", err)
		os.Exit(2)
	}
	time.Sleep(10 * time.Second)
	fmt.Fprintln(os.Stderr, "SIGINT did not end the process")
	os.Exit(3)
}

// spinning is read by the loop of preemptChild, which has no call that the
// runtime could stop it at.
var (
	spinning atomic.Bool
	spun     int
)

// preemptChild runs with one P: a goroutine loops without calling anything,
// and main must still get to sleep and return.
func preemptChild() {
	runtime.GOMAXPROCS(1)
	spinning.Store(true)
	go func() {
		for spinning.Load() {
			spun++
		}
	}()
	// The spinner takes the P as main goes to sleep, and the timer that
	// wakes main can only run once the runtime has taken the P away.
	start := time.Now()
	time.Sleep(10 * time.Millisecond)
	spinning.Store(false)
	if took := time.Since(start); took >= 2*time.Second {
		fmt.Fprintf(os.Stderr, "a sleep of 10 ms took %v\n", took)
		os.Exit(1)
	}
}

// zoneChild prints the abbreviation and offset of a July day in time.Local.
func zoneChild() {
	name, offset := time.Date(2026, time.July, 15, 12, 0, 0, 0, time.UTC).In(time.Local).Zone()
	fmt.Println(name, offset)
}

// panicTrace proves that a panic ends the process with status 2 and a
// traceback that names the goroutine and the function that panicked.
func panicTrace() error {
	_, stderr, err := runSelf(childLimit, nil, "panic")
	if code := exitCode(err); code != 2 {
		return fmt.Errorf("the child that panicked ended with %v, not status 2: %s", err, excerpt(stderr))
	}
	for _, want := range []string{
		"panic: go-runtime deliberate panic",
		"goroutine 1 [running]:",
		"main.panicChild",
	} {
		if !strings.Contains(stderr, want) {
			return fmt.Errorf("the panic's report has no %q: %s", want, excerpt(stderr))
		}
	}
	return nil
}

// nilDereference proves that a fault on address 0 reaches Go's signal
// handler and ends the process as a panic.
func nilDereference() error {
	_, stderr, err := runSelf(childLimit, nil, "nil")
	if code := exitCode(err); code != 2 {
		return fmt.Errorf("the child that dereferenced nil ended with %v, not status 2: %s", err, excerpt(stderr))
	}
	for _, want := range []string{
		"invalid memory address or nil pointer dereference",
		"[signal SIGSEGV: segmentation violation",
	} {
		if !strings.Contains(stderr, want) {
			return fmt.Errorf("the nil dereference's report has no %q: %s", want, excerpt(stderr))
		}
	}
	return nil
}

// signals proves that signals sent to the process reach os/signal's
// channel, and that SIGINT ends a process that does not catch it.
func signals() error {
	received := make(chan os.Signal, 2)
	signal.Notify(received, syscall.SIGINT, syscall.SIGUSR1)
	for _, sent := range []syscall.Signal{syscall.SIGINT, syscall.SIGUSR1} {
		if err := syscall.Kill(syscall.Getpid(), sent); err != nil {
			return fmt.Errorf("kill of this process with %v: %v", sent, err)
		}
		select {
		case got := <-received:
			if got != sent {
				return fmt.Errorf("sent %v to the process, and %v arrived", sent, got)
			}
		case <-time.After(2 * time.Second):
			return fmt.Errorf("%v sent to the process did not arrive within 2 s", sent)
		}
	}
	signal.Reset(syscall.SIGINT, syscall.SIGUSR1)

	_, stderr, err := runSelf(childLimit, nil, "sigint")
	var exit *exec.ExitError
	if !errors.As(err, &exit) {
		return fmt.Errorf("the child that sent itself SIGINT was not ended by it: %v: %s", err, excerpt(stderr))
	}
	status, ok := exit.Sys().(syscall.WaitStatus)
	if !ok || !status.Signaled() || status.Signal() != syscall.SIGINT {
		return fmt.Errorf("the child that sent itself SIGINT ended with %v: %s", exit, excerpt(stderr))
	}
	return nil
}

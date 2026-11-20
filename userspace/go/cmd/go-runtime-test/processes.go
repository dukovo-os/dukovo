// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

package main

import (
	"bytes"
	"context"
	cryptorand "crypto/rand"
	"errors"
	"fmt"
	"math/rand/v2"
	"os"
	"os/exec"
	"os/user"
	"strconv"
	"strings"
	"time"
)

// The commands of Dukovo's system that the checks run.
const (
	echoCommand  = "/system/commands/echo"
	catCommand   = "/system/commands/cat"
	idCommand    = "/system/commands/id"
	pwdCommand   = "/system/commands/pwd"
	shellCommand = "/system/commands/sh"
)

// commandLimit is how long a command that should end at once may take.
const commandLimit = 30 * time.Second

// pipesAndExec proves that programs can be started with arguments, an
// environment, and a pipe between them, and that their exit status comes
// back.
func pipesAndExec() error {
	ctx, cancel := context.WithTimeout(context.Background(), commandLimit)
	defer cancel()

	said, err := exec.CommandContext(ctx, echoCommand, "hi").Output()
	if err != nil || string(said) != "hi\n" {
		return fmt.Errorf("echo hi printed %q (%v), not \"hi\\n\"", said, err)
	}

	echo := exec.CommandContext(ctx, echoCommand, "through a pipe")
	cat := exec.CommandContext(ctx, catCommand)
	pipe, err := echo.StdoutPipe()
	if err != nil {
		return fmt.Errorf("StdoutPipe: %v", err)
	}
	var piped bytes.Buffer
	cat.Stdin, cat.Stdout = pipe, &piped
	if err := cat.Start(); err != nil {
		return fmt.Errorf("starting cat: %v", err)
	}
	if err := echo.Start(); err != nil {
		return fmt.Errorf("starting echo: %v", err)
	}
	if err := echo.Wait(); err != nil {
		return fmt.Errorf("echo into a pipe: %v", err)
	}
	if err := cat.Wait(); err != nil || piped.String() != "through a pipe\n" {
		return fmt.Errorf("echo | cat printed %q (%v), not \"through a pipe\\n\"", piped.String(), err)
	}

	err = exec.CommandContext(ctx, shellCommand, "-c", "exit 7").Run()
	if code := exitCode(err); code != 7 {
		return fmt.Errorf("sh -c 'exit 7' ended with %v, not status 7", err)
	}

	reached := exec.CommandContext(ctx, shellCommand, "-c", "echo $GO_RUNTIME_TEST")
	reached.Env = append(os.Environ(), "GO_RUNTIME_TEST=reached")
	said, err = reached.Output()
	if err != nil || string(said) != "reached\n" {
		return fmt.Errorf("a variable in Cmd.Env reached the child as %q (%v), not \"reached\\n\"", said, err)
	}

	out, _, err := runSelf(commandLimit, nil, "pid")
	if err != nil {
		return fmt.Errorf("the child that prints its process number: %v", err)
	}
	childPid, err := strconv.Atoi(strings.TrimSpace(out))
	if err != nil || childPid <= 0 || childPid == os.Getpid() {
		return fmt.Errorf("the child printed %q as its process number, and this process's is %d", strings.TrimSpace(out), os.Getpid())
	}
	return nil
}

// output runs a command and gives what it printed, without the newline at
// the end.
func output(program string, arguments ...string) (string, error) {
	ctx, cancel := context.WithTimeout(context.Background(), commandLimit)
	defer cancel()
	printed, err := exec.CommandContext(ctx, program, arguments...).Output()
	return strings.TrimSuffix(string(printed), "\n"), err
}

// environment proves that the process knows where it is and who runs it,
// as the commands of the system say.
func environment() error {
	home := os.Getenv("HOME")
	if home == "" {
		return errors.New("HOME is not set")
	}
	if name, err := os.Hostname(); err != nil || name == "" {
		return fmt.Errorf("Hostname gave %q and %v", name, err)
	}

	cwd, err := os.Getwd()
	if err != nil {
		return fmt.Errorf("Getwd: %v", err)
	}
	if pwd, err := output(pwdCommand); err != nil || pwd != cwd {
		return fmt.Errorf("Getwd says %q and pwd says %q (%v)", cwd, pwd, err)
	}

	current, err := user.Current()
	if err != nil {
		return fmt.Errorf("user.Current: %v", err)
	}
	if name, err := output(idCommand, "-un"); err != nil || name != current.Username {
		return fmt.Errorf("user.Current says the account is %q, and id -un says %q (%v)", current.Username, name, err)
	}
	if uid := strconv.Itoa(os.Getuid()); current.Uid != uid {
		return fmt.Errorf("user.Current says the account's number is %s, and Getuid says %s", current.Uid, uid)
	}
	if current.HomeDir != home {
		return fmt.Errorf("user.Current says the home is %q, and HOME is %q", current.HomeDir, home)
	}
	return nil
}

// timeZones proves that zone files are found and read, and that TZ chooses
// the local zone.
func timeZones() error {
	denver, err := time.LoadLocation("America/Denver")
	if err != nil {
		return fmt.Errorf("LoadLocation of America/Denver: %v", err)
	}
	for _, day := range []struct {
		month  time.Month
		name   string
		offset int
	}{
		{time.January, "MST", -7 * 3600},
		{time.July, "MDT", -6 * 3600},
	} {
		noon := time.Date(2026, day.month, 15, 12, 0, 0, 0, time.UTC).In(denver)
		if name, offset := noon.Zone(); name != day.name || offset != day.offset {
			return fmt.Errorf("Denver in %v is %s at %d seconds, not %s at %d", day.month, name, offset, day.name, day.offset)
		}
	}

	// time.Local is read from TZ once, at first use, so each zone is a
	// child's.
	for _, zone := range []struct{ tz, says string }{
		{"Europe/Paris", "CEST 7200"},
		{"America/Denver", "MDT -21600"},
	} {
		out, stderr, err := runSelf(commandLimit, append(os.Environ(), "TZ="+zone.tz), "zone")
		if got := strings.TrimSpace(out); err != nil || got != zone.says {
			return fmt.Errorf("with TZ=%s a July day in time.Local is %q, not %q (%v): %s", zone.tz, got, zone.says, err, excerpt(stderr))
		}
	}
	return nil
}

// random proves that the system's random bytes and Go's generator work and
// do not repeat.
func random() error {
	var first, second [32]byte
	if _, err := cryptorand.Read(first[:]); err != nil {
		return fmt.Errorf("crypto/rand.Read: %v", err)
	}
	if _, err := cryptorand.Read(second[:]); err != nil {
		return fmt.Errorf("crypto/rand.Read: %v", err)
	}
	if first == second {
		return errors.New("two reads of 32 bytes from crypto/rand were equal")
	}
	if first == [32]byte{} || second == [32]byte{} {
		return errors.New("crypto/rand gave 32 zero bytes")
	}

	if n := rand.IntN(1000); n < 0 || n >= 1000 {
		return fmt.Errorf("math/rand/v2's IntN(1000) gave %d", n)
	}
	if a, b := rand.Uint64(), rand.Uint64(); a == b {
		return fmt.Errorf("math/rand/v2 gave %d twice", a)
	}
	return nil
}

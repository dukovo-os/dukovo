// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

package main

import (
	"fmt"
	"math/rand/v2"
	"runtime"
	"sync"
	"sync/atomic"
	"syscall"
	"time"
)

// spinFor keeps the calling goroutine busy for d.
func spinFor(d time.Duration) {
	for end := time.Now().Add(d); time.Now().Before(end); {
	}
}

// waitWithin waits for wg for up to limit and reports whether it finished.
func waitWithin(wg *sync.WaitGroup, limit time.Duration) bool {
	done := make(chan struct{})
	go func() {
		wg.Wait()
		close(done)
	}()
	select {
	case <-done:
		return true
	case <-time.After(limit):
		return false
	}
}

// threads proves that Go runs on as many Ps as the machine has CPUs, and
// that goroutines locked to their threads get threads of their own.
func threads(cpus int) error {
	if got := runtime.GOMAXPROCS(0); got != cpus {
		return fmt.Errorf("GOMAXPROCS is %d on a machine with %d CPUs", got, cpus)
	}

	const workers = 64
	ids := make(chan int, workers)
	var wg sync.WaitGroup
	for range workers {
		wg.Add(1)
		go func() {
			defer wg.Done()
			// A locked goroutine keeps its thread while it spins, so
			// goroutines that overlap are on different threads.
			runtime.LockOSThread()
			defer runtime.UnlockOSThread()
			ids <- syscall.Gettid()
			spinFor(20 * time.Millisecond)
		}()
	}
	if !waitWithin(&wg, 30*time.Second) {
		return fmt.Errorf("%d goroutines locked to threads did not all finish within 30 s", workers)
	}
	close(ids)
	seen := make(map[int]bool)
	for id := range ids {
		if id <= 0 {
			return fmt.Errorf("Gettid gave %d", id)
		}
		seen[id] = true
	}
	if len(seen) < 2 {
		return fmt.Errorf("%d goroutines ran on %d thread", workers, len(seen))
	}
	return nil
}

// channelsAndMutexes proves that goroutines switch and wake one another
// through channels, a mutex, a wait group and a condition variable.
func channelsAndMutexes() error {
	if err := tokenRing(); err != nil {
		return err
	}
	if err := mutexCounter(); err != nil {
		return err
	}
	return conditionBroadcast()
}

// tokenRing passes a token around a ring of goroutines joined by unbuffered
// channels, so that every hop is a switch from one goroutine to the next.
func tokenRing() error {
	const (
		members = 1000
		laps    = 100
	)
	ring := make([]chan int, members)
	for i := range ring {
		ring[i] = make(chan int)
	}
	visits := make([]int, members)
	last := make(chan int)
	var wg sync.WaitGroup
	for i := range members {
		wg.Add(1)
		go func() {
			defer wg.Done()
			for token := range ring[i] {
				visits[i]++
				token++
				if token == members*laps {
					last <- token
					continue
				}
				ring[(i+1)%members] <- token
			}
		}()
	}
	ring[0] <- 0
	select {
	case token := <-last:
		if token != members*laps {
			return fmt.Errorf("the token came back as %d, not %d", token, members*laps)
		}
	case <-time.After(30 * time.Second):
		return fmt.Errorf("the token did not go round %d goroutines %d times within 30 s", members, laps)
	}
	for _, channel := range ring {
		close(channel)
	}
	if !waitWithin(&wg, 10*time.Second) {
		return fmt.Errorf("the ring's goroutines did not end when their channels closed")
	}
	for i, count := range visits {
		if count != laps {
			return fmt.Errorf("goroutine %d of the ring saw the token %d times, not %d", i, count, laps)
		}
	}
	return nil
}

// mutexCounter has goroutines add to one counter under a mutex: a lost
// wake-up or a broken lock shows as a total that is short, or as a hang.
func mutexCounter() error {
	const (
		workers = 100
		rounds  = 1000
	)
	var (
		lock  sync.Mutex
		total int
		wg    sync.WaitGroup
	)
	for range workers {
		wg.Add(1)
		go func() {
			defer wg.Done()
			for range rounds {
				lock.Lock()
				total++
				lock.Unlock()
			}
		}()
	}
	if !waitWithin(&wg, 30*time.Second) {
		return fmt.Errorf("%d goroutines counting under a mutex did not finish within 30 s", workers)
	}
	if total != workers*rounds {
		return fmt.Errorf("the counter reached %d, not %d", total, workers*rounds)
	}
	return nil
}

// conditionBroadcast parks goroutines on a condition variable and wakes
// them all with one Broadcast.
func conditionBroadcast() error {
	const waiters = 100
	var (
		lock     sync.Mutex
		cond     = sync.NewCond(&lock)
		arrived  int
		released bool
		woken    atomic.Int32
		wg       sync.WaitGroup
	)
	for range waiters {
		wg.Add(1)
		go func() {
			defer wg.Done()
			lock.Lock()
			arrived++
			for !released {
				cond.Wait()
			}
			lock.Unlock()
			woken.Add(1)
		}()
	}
	// Broadcast only when every waiter is parked, or a late one would see
	// the flag and never wait.
	for deadline := time.Now().Add(10 * time.Second); ; time.Sleep(time.Millisecond) {
		lock.Lock()
		parked := arrived
		lock.Unlock()
		if parked == waiters {
			break
		}
		if time.Now().After(deadline) {
			return fmt.Errorf("%d of %d waiters reached the condition variable within 10 s", parked, waiters)
		}
	}
	lock.Lock()
	released = true
	lock.Unlock()
	cond.Broadcast()
	if !waitWithin(&wg, 10*time.Second) {
		return fmt.Errorf("Broadcast woke %d of %d waiters within 10 s", woken.Load(), waiters)
	}
	return nil
}

// timers proves that sleeping, timer functions, tickers and Stop keep the
// time they are given.
func timers() error {
	start := time.Now()
	time.Sleep(50 * time.Millisecond)
	if took := time.Since(start); took < 50*time.Millisecond || took >= 2*time.Second {
		return fmt.Errorf("Sleep(50ms) took %v", took)
	}
	if err := timerFunctionsFire(); err != nil {
		return err
	}
	if err := tickerTicks(); err != nil {
		return err
	}
	return stoppedTimersStayQuiet()
}

// timerFunctionsFire starts a thousand timer functions with random delays
// and checks that every one runs, none before its delay is over.
func timerFunctionsFire() error {
	const (
		count    = 1000
		longest  = 200 * time.Millisecond
		deadline = 10 * time.Second
	)
	var (
		fired atomic.Int32
		early atomic.Int32
		wg    sync.WaitGroup
	)
	wg.Add(count)
	for range count {
		delay := rand.N(longest)
		start := time.Now()
		time.AfterFunc(delay, func() {
			if time.Since(start) < delay {
				early.Add(1)
			}
			fired.Add(1)
			wg.Done()
		})
	}
	if !waitWithin(&wg, deadline) {
		return fmt.Errorf("%d of %d timer functions ran within %v", fired.Load(), count, deadline)
	}
	if n := early.Load(); n > 0 {
		return fmt.Errorf("%d of %d timer functions ran before their delay was over", n, count)
	}
	return nil
}

// tickerTicks takes a ticker's ticks for 300 ms: at 10 ms a tick there are
// about 30, and 20 allow for a slow machine.
func tickerTicks() error {
	ticker := time.NewTicker(10 * time.Millisecond)
	defer ticker.Stop()
	end := time.After(300 * time.Millisecond)
	ticks := 0
counting:
	for {
		select {
		case <-ticker.C:
			ticks++
		case <-end:
			break counting
		}
	}
	if ticks < 20 {
		return fmt.Errorf("a ticker of 10 ms ticked %d times in 300 ms, fewer than 20", ticks)
	}
	return nil
}

// stoppedTimersStayQuiet stops a timer and a timer function before their
// time and waits past it.
func stoppedTimersStayQuiet() error {
	var ran atomic.Int32
	function := time.AfterFunc(30*time.Millisecond, func() { ran.Add(1) })
	timer := time.NewTimer(30 * time.Millisecond)
	if !function.Stop() || !timer.Stop() {
		return fmt.Errorf("Stop said a timer of 30 ms had already run")
	}
	time.Sleep(150 * time.Millisecond)
	select {
	case <-timer.C:
		return fmt.Errorf("a stopped timer sent on its channel")
	default:
	}
	if ran.Load() != 0 {
		return fmt.Errorf("a stopped timer function ran")
	}
	return nil
}

// clockFloor is a date before which the clock cannot be right: a machine
// whose clock was never read gives the epoch.
var clockFloor = time.Date(2026, time.January, 1, 0, 0, 0, 0, time.UTC)

// clocks proves that the wall clock is set and the monotonic one runs
// forward at the speed of sleeping.
func clocks() error {
	if now := time.Now(); now.Before(clockFloor) {
		return fmt.Errorf("the clock says %v, before %v", now, clockFloor)
	}
	previous := time.Now()
	for i := range 100000 {
		now := time.Now()
		if back := previous.Sub(now); back > 0 {
			return fmt.Errorf("the monotonic clock went back by %v at read %d", back, i)
		}
		previous = now
	}
	start := time.Now()
	time.Sleep(10 * time.Millisecond)
	if took := time.Since(start); took < 10*time.Millisecond || took > 110*time.Millisecond {
		return fmt.Errorf("Sleep(10ms) took %v by the clock, not within 100 ms of 10 ms", took)
	}
	return nil
}

// preemption proves that a goroutine in a loop with no function calls does
// not keep the only P from another goroutine, which only a signal to its
// thread (SIGURG) can interrupt. The proof runs in a child: where the
// preemption fails, the spinning goroutine never gives up the P, and nothing
// in the process could end the check.
func preemption() error {
	_, stderr, err := runSelf(20*time.Second, nil, "preempt")
	if err != nil {
		return fmt.Errorf("a loop with no calls kept the only P from a sleeping goroutine (%v): %s", err, excerpt(stderr))
	}
	return nil
}

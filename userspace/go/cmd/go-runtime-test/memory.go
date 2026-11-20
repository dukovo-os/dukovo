// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

package main

import (
	"fmt"
	"runtime"
	"runtime/debug"
)

const (
	// pageSize is the size of the pages Go's heap is made of, as far as
	// touching them goes.
	pageSize = 4096
	mebibyte = 1 << 20
)

// sink holds the block allocated last, so that the allocation is real.
var sink []byte

// touch writes to each page of block, so that every page of it is mapped.
func touch(block []byte, value byte) {
	for offset := 0; offset < len(block); offset += pageSize {
		block[offset] = value
	}
	block[len(block)-1] = value
}

// memory proves that the heap stays small while a gibibyte is allocated and
// dropped, that the runtime gives memory back to the system, and that a
// single large allocation works.
func memory() error {
	var before runtime.MemStats
	runtime.ReadMemStats(&before)

	const slices = 1024
	for i := range slices {
		block := make([]byte, mebibyte)
		touch(block, byte(i))
		sink = block
		if i%64 == 63 {
			runtime.GC()
			debug.FreeOSMemory()
		}
	}
	sink = nil
	runtime.GC()
	debug.FreeOSMemory()

	var after runtime.MemStats
	runtime.ReadMemStats(&after)
	if limit := uint64(256 * mebibyte); after.HeapSys >= limit {
		return fmt.Errorf("the heap holds %d MiB from the system after %d MiB were allocated and dropped, not under %d MiB",
			after.HeapSys/mebibyte, slices, limit/mebibyte)
	}
	if after.HeapReleased <= before.HeapReleased {
		return fmt.Errorf("the runtime gave no memory back: %d bytes released before, %d after",
			before.HeapReleased, after.HeapReleased)
	}

	big := make([]byte, 64*mebibyte)
	touch(big, 0x5a)
	for offset := 0; offset < len(big); offset += pageSize {
		if big[offset] != 0x5a {
			return fmt.Errorf("a page of a 64 MiB allocation read %#x at offset %d, not 0x5a", big[offset], offset)
		}
	}
	return nil
}

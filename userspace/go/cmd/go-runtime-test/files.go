// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

package main

import (
	"bytes"
	"errors"
	"fmt"
	"io/fs"
	"math/rand/v2"
	"os"
	"path/filepath"
)

// files proves that a file can be written, synced, read back, inspected,
// renamed, listed, changed and removed, and that a missing one is reported
// as one.
func files() error {
	dir, err := os.MkdirTemp(os.TempDir(), "go-runtime-")
	if err != nil {
		return fmt.Errorf("MkdirTemp in %s: %v", os.TempDir(), err)
	}
	defer os.RemoveAll(dir)

	// Pseudo-random bytes from a fixed seed: no block repeats another, so
	// a block read from the wrong place differs.
	content := make([]byte, 1<<20)
	generator := rand.New(rand.NewPCG(1, 2))
	for i := range content {
		content[i] = byte(generator.Uint32())
	}
	path := filepath.Join(dir, "data")
	if err := writeSynced(path, content); err != nil {
		return err
	}
	read, err := os.ReadFile(path)
	if err != nil {
		return fmt.Errorf("reading %s back: %v", path, err)
	}
	if !bytes.Equal(read, content) {
		return fmt.Errorf("%s read back with %d bytes, and they differ from the %d written", path, len(read), len(content))
	}
	info, err := os.Stat(path)
	if err != nil {
		return fmt.Errorf("Stat of %s: %v", path, err)
	}
	if info.Size() != int64(len(content)) || !info.Mode().IsRegular() {
		return fmt.Errorf("Stat says %s is %d bytes with mode %v, not a regular file of %d", path, info.Size(), info.Mode(), len(content))
	}

	moved := filepath.Join(dir, "moved")
	if err := os.Rename(path, moved); err != nil {
		return fmt.Errorf("Rename of %s: %v", path, err)
	}
	if _, err := os.Stat(path); !errors.Is(err, fs.ErrNotExist) {
		return fmt.Errorf("Stat of the old name after Rename gave %v, not ErrNotExist", err)
	}
	entries, err := os.ReadDir(dir)
	if err != nil {
		return fmt.Errorf("ReadDir of %s: %v", dir, err)
	}
	if len(entries) != 1 || entries[0].Name() != "moved" {
		return fmt.Errorf("ReadDir lists %d entries after Rename, not just \"moved\"", len(entries))
	}

	if err := os.Chmod(moved, 0o600); err != nil {
		return fmt.Errorf("Chmod of %s: %v", moved, err)
	}
	if info, err := os.Stat(moved); err != nil || info.Mode().Perm() != 0o600 {
		return fmt.Errorf("after Chmod to 0600, Stat gave %v and %v", info, err)
	}
	if err := os.Remove(moved); err != nil {
		return fmt.Errorf("Remove of %s: %v", moved, err)
	}
	if _, err := os.Stat(moved); !errors.Is(err, fs.ErrNotExist) {
		return fmt.Errorf("Stat after Remove gave %v, not ErrNotExist", err)
	}
	if _, err := os.Open(filepath.Join(dir, "missing")); !errors.Is(err, fs.ErrNotExist) {
		return fmt.Errorf("Open of a missing file gave %v, not ErrNotExist", err)
	}
	return nil
}

// writeSynced creates path holding content and makes sure it reached the
// disk before returning.
func writeSynced(path string, content []byte) error {
	file, err := os.OpenFile(path, os.O_WRONLY|os.O_CREATE|os.O_EXCL, 0o644)
	if err != nil {
		return fmt.Errorf("creating %s: %v", path, err)
	}
	if _, err := file.Write(content); err != nil {
		file.Close()
		return fmt.Errorf("writing %d bytes to %s: %v", len(content), path, err)
	}
	if err := file.Sync(); err != nil {
		file.Close()
		return fmt.Errorf("Sync of %s: %v", path, err)
	}
	if err := file.Close(); err != nil {
		return fmt.Errorf("closing %s: %v", path, err)
	}
	return nil
}

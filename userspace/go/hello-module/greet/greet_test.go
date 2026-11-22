// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

package greet

import (
	"encoding/json"
	"testing"
)

func TestLine(t *testing.T) {
	for name, want := range map[string]string{
		"alice": "Hello, Alice!",
		" bob ": "Hello, Bob!",
		"":      "Hello, nobody!",
	} {
		if got := Line(name); got != want {
			t.Errorf("Line(%q) = %q, want %q", name, got, want)
		}
	}
}

func TestAllIsSortedByName(t *testing.T) {
	got := All([]string{"carol", "alice", "bob", "dave", "erin"})
	want := []string{"alice", "bob", "carol", "dave", "erin"}
	if len(got) != len(want) {
		t.Fatalf("All gave %d greetings, want %d", len(got), len(want))
	}
	for i, name := range want {
		if got[i].Name != name {
			t.Errorf("greeting %d is for %q, want %q", i, got[i].Name, name)
		}
	}
}

func TestGreetingsAreJSON(t *testing.T) {
	text, err := json.Marshal(All([]string{"bob"}))
	if err != nil {
		t.Fatal(err)
	}
	if want := `[{"name":"bob","line":"Hello, Bob!"}]`; string(text) != want {
		t.Errorf("JSON is %s, want %s", text, want)
	}
}

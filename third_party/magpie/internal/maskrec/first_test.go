package maskrec

import (
	"testing"
	"time"
)

func placeholders(vs []Value) []string {
	var out []string
	for _, v := range vs {
		out = append(out, v.Placeholder)
	}
	return out
}

// A value is fresh only in the record that masked it first. The next turn
// of the same conversation, which sends it again with the history, is
// not; nor is it after a restart, when only the days on disk know it; a
// value sent as written was never masked.
func TestFreshOnlyTheFirstTime(t *testing.T) {
	now := time.Date(2026, 10, 29, 10, 0, 0, 0, time.Local)
	sandbox(t, now)
	key := Value{Placeholder: "{{API_KEY_aaaabbbb}}", Kind: "API_KEY", By: "rule", Head: "sk-", Len: 40}
	mail := Value{Placeholder: "{{EMAIL_ccccdddd}}", Kind: "EMAIL", By: "rule", Head: "a", Len: 12}
	warned := Value{Placeholder: "{{PHONE_eeeeffff}}", Kind: "PHONE", By: "rule", Head: "1", Len: 11, Action: "warn"}

	turn1 := Add(Record{Time: now, What: "chat", Agent: "claude", N: 1, Values: []Value{key}})
	if got := placeholders(Fresh(turn1)); len(got) != 1 || got[0] != key.Placeholder {
		t.Fatalf("turn 1 fresh = %v, want the key", got)
	}
	// turn 2: the key again with the history, a new email, a phone sent as written
	turn2 := Add(Record{Time: now.Add(time.Minute), What: "chat", Agent: "claude", N: 3,
		Values: []Value{{Placeholder: key.Placeholder, Kind: "API_KEY", By: "known", Len: 40}, mail, warned}})
	if got := placeholders(Fresh(turn2)); len(got) != 1 || got[0] != mail.Placeholder {
		t.Fatalf("turn 2 fresh = %v, want the email alone", got)
	}
	// asked again, turn 1 still says its key: the answer is by key, not by
	// when it was asked (the notifier asks after the turns went on)
	if got := placeholders(Fresh(turn1)); len(got) != 1 {
		t.Fatalf("turn 1 asked late = %v", got)
	}

	// a restart: memory is gone, the days on disk are what is left
	store.mu.Lock()
	store.snips, store.first, store.firstRead = nil, nil, false
	store.mu.Unlock()
	turn3 := Add(Record{Time: now.Add(2 * time.Minute), What: "chat", Agent: "claude", N: 2, Values: []Value{key, mail}})
	if got := Fresh(turn3); len(got) != 0 {
		t.Fatalf("after a restart the history is new again: %v", placeholders(got))
	}
}

// A record held while the days are read (Hold on a request's way, Fresh
// in the notifier) is in the index whichever came first.
func TestFreshHeldWhileReading(t *testing.T) {
	now := time.Date(2026, 10, 29, 10, 0, 0, 0, time.Local)
	sandbox(t, now)
	v := Value{Placeholder: "{{SECRET_gggghhhh}}", Kind: "SECRET", By: "rule", Len: 20}
	a := Add(Record{Time: now, What: "chat", N: 1, Values: []Value{v}})
	_ = Fresh(a) // the index is read
	b := Add(Record{Time: now.Add(time.Second), What: "chat", N: 1, Values: []Value{v}})
	if got := Fresh(b); len(got) != 0 {
		t.Fatalf("a value held after the index was read is fresh again: %v", placeholders(got))
	}
}

package provider

import (
	"encoding/json"
	"os"
	"strings"
	"testing"
	"time"
)

// Each quota is told when its account, plan or key last answered (#570):
// an account by its user, a key by the name or mask its card has, a card
// with no user by any of its provider's; the latest is marked last.
func TestQuotasLastServed(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)
	t.Setenv("USERPROFILE", home)
	t.Setenv("XDG_CONFIG_HOME", t.TempDir())
	if err := Save(Provider{ID: "dk", Name: "DK", Key: "sk-first-key-0001", KeyName: "work", Chat: "http://127.0.0.1:1/v1",
		Keys: []KeyAccount{{Key: "sk-other-key-0002"}}}); err != nil {
		t.Fatal(err)
	}
	now := time.Now().Truncate(time.Second)
	NoteServed(Provider{ID: "codex", Account: &Account{Agent: "codex", User: "A@x.com"}}, now.Add(-time.Hour))
	NoteServed(Provider{ID: "dk", Key: "sk-other-key-0002"}, now.Add(-time.Minute))
	NoteServed(Provider{ID: "solo", Key: "sk-solo"}, now.Add(-2*time.Hour))
	// the CLI is a process of its own: what it is told is read from the file
	servedAt.Lock()
	servedAt.m = map[string]time.Time{}
	servedAt.Unlock()
	qs := withServed([]Quota{
		{Provider: "codex", Kind: "subscription", User: "a@x.com"},
		{Provider: "codex", Kind: "subscription", User: "b@x.com"},
		{Provider: "dk", Kind: "plan", User: "work"},
		{Provider: "dk", Kind: "plan", User: Mask("sk-other-key-0002")},
		{Provider: "dk", Kind: "balance"},
		{Provider: "solo", Kind: "balance"},
		{Provider: "none", Kind: "balance"},
	}, LastServed())
	want := []*time.Time{ptr(now.Add(-time.Hour)), nil, nil, ptr(now.Add(-time.Minute)), ptr(now.Add(-time.Minute)), ptr(now.Add(-2 * time.Hour)), nil}
	for i, q := range qs {
		if (q.LastServedAt == nil) != (want[i] == nil) || q.LastServedAt != nil && !q.LastServedAt.Equal(*want[i]) {
			t.Errorf("%d %s %s %q: lastServedAt %v, want %v", i, q.Provider, q.Kind, q.User, q.LastServedAt, want[i])
		}
		if last := i == 3 || i == 4; q.Last != last {
			t.Errorf("%d %s %q: last %v", i, q.Provider, q.User, q.Last)
		}
	}
	b, _ := json.Marshal(qs[0])
	if !strings.Contains(string(b), `"lastServedAt":"`+now.Add(-time.Hour).UTC().Format(time.RFC3339)+`"`) || strings.Contains(string(b), `"last"`) {
		t.Errorf("json %s", b)
	}
	if b, _ := json.Marshal(qs[1]); strings.Contains(string(b), "lastServedAt") || strings.Contains(string(b), `"last"`) {
		t.Errorf("never served: %s", b)
	}
	if _, err := os.Stat(servedPath()); err != nil {
		t.Fatal("not kept on disk:", err)
	}
}

func ptr(t time.Time) *time.Time { return &t }

// The Usage page's cards are told it too (okingkee on X): the page and the
// tray panel show the account that answered last in full, and the menu
// bar's "account in use" follows it. A remote magpie's card is that one's,
// and the cache the cards are read from keeps none of it.
func TestCardsLastServed(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)
	t.Setenv("USERPROFILE", home)
	t.Setenv("XDG_CONFIG_HOME", t.TempDir())
	now := time.Now().Truncate(time.Second)
	NoteServed(Provider{ID: "codex", Account: &Account{Agent: "codex", User: "B@x.com"}}, now.Add(-time.Minute))
	NoteServed(Provider{ID: "solo", Key: "sk-solo"}, now.Add(-2*time.Hour))
	subs := []SubscriptionQuota{
		{Provider: "codex", User: "a@x.com"},
		{Provider: "codex", User: "b@x.com"},
		{Provider: "codex", User: "b@x.com", From: "studio"},
	}
	balances := []SubscriptionQuota{{Provider: "solo"}}
	got := cardsServed(subs, nil, balances, LastServed())
	if len(got) != 4 {
		t.Fatalf("cards: %+v", got)
	}
	want := []*time.Time{nil, ptr(now.Add(-time.Minute)), nil, ptr(now.Add(-2 * time.Hour))}
	for i, q := range got {
		if (q.LastServedAt == nil) != (want[i] == nil) || q.LastServedAt != nil && !q.LastServedAt.Equal(*want[i]) {
			t.Errorf("%d %s %q from %q: lastServedAt %v, want %v", i, q.Provider, q.User, q.From, q.LastServedAt, want[i])
		}
	}
	if subs[1].LastServedAt != nil || balances[0].LastServedAt != nil {
		t.Error("the cards read from were written to")
	}
	if b, _ := json.Marshal(got[1]); !strings.Contains(string(b), `"lastServedAt":"`) {
		t.Errorf("json %s", b)
	}
}

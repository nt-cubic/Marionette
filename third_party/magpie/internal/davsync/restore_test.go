package davsync

import (
	"context"
	"net/http/httptest"
	"os"
	"slices"
	"strings"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/backup"
	"github.com/yetone/magpie/internal/provider"
)

// Sync can be set to run by itself less often, or never — only when asked
// — and the gateway's loop keeps to it, a change taking effect without a
// restart (Sun1090, #847: free 坚果云 accounts are limited in requests).
func TestAutoSync(t *testing.T) {
	fake := &fakeDAV{files: map[string][]byte{}, etags: map[string]string{}, dirs: map[string]bool{"/dav": true}}
	srv := httptest.NewServer(fake)
	defer srv.Close()
	newComputer(t).use(t)
	cfg := Config{URL: srv.URL + "/dav/", User: "me", Password: "pw", Passphrase: "correct horse", Keys: true, Agents: true}
	if err := SetAuto(Manual); err != ErrOff {
		t.Fatalf("set with sync off: %v", err)
	}
	if err := Configure(cfg); err != nil {
		t.Fatal(err)
	}
	if v := Status(); v.Auto != 3 {
		t.Fatalf("by default it syncs every %d minutes", v.Auto)
	}
	if err := SetAuto(7); err == nil {
		t.Fatal("7 minutes was taken")
	}
	if err := SetAuto(Manual); err != nil {
		t.Fatal(err)
	}
	// saving the form again keeps it
	if err := Configure(cfg); err != nil {
		t.Fatal(err)
	}
	if v := Status(); v.Auto != 0 {
		t.Fatalf("manual, after saving again: every %d", v.Auto)
	}

	old := poll
	poll = 5 * time.Millisecond
	defer func() { poll = old }()
	ctx, cancel := context.WithCancel(context.Background())
	done := make(chan struct{})
	go func() { run(ctx, time.Millisecond); close(done) }()
	defer func() { cancel(); <-done }()
	time.Sleep(150 * time.Millisecond)
	fake.mu.Lock()
	n := fake.gets + fake.puts
	fake.mu.Unlock()
	if n != 0 {
		t.Fatalf("set to sync only when asked, it made %d requests by itself", n)
	}
	// set back to every 15 minutes: the loop syncs at once, then waits
	if err := SetAuto(15); err != nil {
		t.Fatal(err)
	}
	deadline := time.Now().Add(5 * time.Second)
	for Status().Last.IsZero() && time.Now().Before(deadline) {
		time.Sleep(10 * time.Millisecond)
	}
	if v := Status(); v.Last.IsZero() || v.Auto != 15 {
		t.Fatalf("every 15: %+v", v)
	}
	time.Sleep(100 * time.Millisecond)
	fake.mu.Lock()
	gets := fake.gets
	fake.mu.Unlock()
	time.Sleep(150 * time.Millisecond)
	fake.mu.Lock()
	defer fake.mu.Unlock()
	if fake.gets != gets {
		t.Fatalf("synced again within 15 minutes: %d reads, then %d", gets, fake.gets)
	}
}

// Restore makes this computer's setup the server's, keeping what was here
// to undo; the next sync pushes nothing back. Undo puts it back, and the
// next sync takes that to the others.
func TestRestore(t *testing.T) {
	fake := &fakeDAV{files: map[string][]byte{}, etags: map[string]string{}, dirs: map[string]bool{"/dav": true}}
	srv := httptest.NewServer(fake)
	defer srv.Close()
	ctx := context.Background()
	cfg := Config{URL: srv.URL + "/dav/", User: "me", Password: "pw", Passphrase: "correct horse", Keys: true, Agents: true}

	a, b := newComputer(t), newComputer(t)
	a.use(t)
	if _, err := Restore(ctx); err != ErrOff {
		t.Fatalf("restore with sync off: %v", err)
	}
	if err := Configure(cfg); err != nil {
		t.Fatal(err)
	}
	if _, err := Restore(ctx); err == nil || !strings.Contains(err.Error(), "nothing to restore") {
		t.Fatalf("restore from an empty server: %v", err)
	}
	provider.Save(provider.Provider{ID: "deepseek", Name: "DeepSeek", Chat: "https://api.deepseek.com/v1", Key: "k1"})
	if err := Now(ctx); err != nil {
		t.Fatal(err)
	}
	b.use(t)
	if err := Configure(cfg); err != nil {
		t.Fatal(err)
	}
	if err := Now(ctx); err != nil {
		t.Fatal(err)
	}
	Dismiss()
	// b's setup goes astray: a provider gone, another added
	provider.Delete("deepseek")
	provider.Save(provider.Provider{ID: "mine", Name: "Mine", Chat: "https://x/v1", Key: "kb"})
	puts := fake.puts
	got, err := Restore(ctx)
	if err != nil || !slices.Equal(got, []string{"providers"}) {
		t.Fatalf("restored %v: %v", got, err)
	}
	if ids := ids(); !slices.Equal(ids, []string{"deepseek=k1"}) {
		t.Fatalf("after restoring: %v", ids)
	}
	v := Status()
	if v.Notice == nil || !v.Notice.Restored || !v.Undo || v.Error != "" || fake.puts != puts {
		t.Fatalf("restored: %+v, %d puts", v, fake.puts-puts)
	}
	kept := loadState().Undo
	if old, err := backup.Open(must(os.ReadFile(kept)), "correct horse"); err != nil || len(old.Providers) != 1 || old.Providers[0].ID != "mine" || old.Providers[0].Key != "kb" {
		t.Fatalf("kept before restoring: %v %+v", err, old.Providers)
	}
	if err := Now(ctx); err != nil || fake.puts != puts {
		t.Fatalf("the sync after a restore pushed (%d puts): %v", fake.puts-puts, err)
	}

	back, err := Undo()
	if err != nil || !slices.Equal(back, []string{"providers"}) {
		t.Fatalf("undone %v: %v", back, err)
	}
	if ids := ids(); !slices.Equal(ids, []string{"mine=kb"}) {
		t.Fatalf("after undoing: %v", ids)
	}
	if v := Status(); v.Undo || v.Notice != nil {
		t.Fatalf("after undoing: %+v", v)
	}
	if _, err := Undo(); err == nil {
		t.Fatal("undone twice")
	}
	if err := Now(ctx); err != nil {
		t.Fatal(err)
	}
	a.use(t)
	if err := Now(ctx); err != nil {
		t.Fatal(err)
	}
	if ids := ids(); !slices.Equal(ids, []string{"mine=kb"}) {
		t.Fatalf("a after b undid: %v", ids)
	}
	// nothing differs: nothing brought in, nothing kept
	if got, err := Restore(ctx); err != nil || len(got) != 0 {
		t.Fatalf("restoring what is here: %v %v", got, err)
	}
}

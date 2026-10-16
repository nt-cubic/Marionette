package davsync

import (
	"context"
	"fmt"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/provider"
)

// Two computers sharing usage share what they read of their accounts'
// windows too (#651): each puts its own sealed beside its days, and each
// ends with the points of both, merged by account and window, nothing
// twice however often it syncs.
func TestQuotaHistorySharedBetweenComputers(t *testing.T) {
	old := usageEvery
	usageEvery = 0
	t.Cleanup(func() { usageEvery = old })
	fake := &usageDAV{fakeDAV: &fakeDAV{files: map[string][]byte{}, etags: map[string]string{}, dirs: map[string]bool{"/dav": true}}}
	srv := httptest.NewServer(fake)
	defer srv.Close()
	cfg := Config{URL: srv.URL + "/dav/", User: "me", Password: "pw", Passphrase: "correct horse", Keys: true, Usage: true}
	sync := func(t *testing.T) {
		t.Helper()
		if err := Now(context.Background()); err != nil {
			t.Fatal(err)
		}
		if v := Status(); v.UsageError != "" {
			t.Fatalf("usage not shared: %+v", v)
		}
	}
	now := time.Now().UTC().Truncate(time.Second)
	seed := func(t *testing.T, mins ...int) {
		t.Helper()
		var pts []string
		for _, m := range mins {
			pts = append(pts, fmt.Sprintf(`{"at":%q,"left":%d}`, now.Add(-time.Duration(m)*time.Minute).Format(time.RFC3339), 100-m/10))
		}
		if err := provider.MergeQuotaHistory([]byte(`{"codex|a@x.com":{"5 hours":[`+strings.Join(pts, ",")+`]}}`), now); err != nil {
			t.Fatal(err)
		}
	}
	points := func(t *testing.T) int {
		t.Helper()
		hs := provider.QuotaHistories(time.Time{}, "codex", "a@x.com")
		if len(hs) != 1 {
			t.Fatalf("histories: %+v", hs)
		}
		return len(hs[0].Lines[0].Points)
	}

	a, b := newComputer(t), newComputer(t)
	a.use(t)
	if err := Configure(cfg); err != nil {
		t.Fatal(err)
	}
	seed(t, 100, 60)
	sync(t)
	var sealed []byte
	for _, f := range fake.usageFiles() {
		if strings.HasSuffix(f, quotasExt) {
			sealed = fake.files["/dav/magpie/usage/"+f]
		}
	}
	if sealed == nil || strings.Contains(string(sealed), "a@x.com") {
		t.Fatalf("a's history on the server, sealed: %v", fake.usageFiles())
	}

	b.use(t)
	if err := Configure(cfg); err != nil {
		t.Fatal(err)
	}
	seed(t, 80, 20)
	sync(t)
	if n := points(t); n != 4 {
		t.Fatalf("b has %d points, want its 2 and a's 2", n)
	}
	sync(t)
	sync(t)
	if n := points(t); n != 4 {
		t.Fatalf("b, synced again: %d points", n)
	}
	a.use(t)
	sync(t)
	if n := points(t); n != 4 {
		t.Fatalf("a has %d points, want its 2 and b's 2", n)
	}
}

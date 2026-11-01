package davsync

import (
	"os"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/edit"
	"github.com/yetone/magpie/internal/filememo"
)

// TestLoadReadOnceARequest: every Providers page poll asked Load, and
// each read sync.json; a request reads it once, and sync turned off in
// it is off at once.
func TestLoadReadOnceARequest(t *testing.T) {
	newComputer(t).use(t)
	if _, ok := Load(); ok {
		t.Fatal("on with no sync.json")
	}
	os.MkdirAll(path(""), 0o755)
	if err := edit.WriteAtomic(path("sync.json"), []byte(`{"url":"https://dav.example.com/dav/","user":"me"}`)); err != nil {
		t.Fatal(err)
	}
	// written a while ago, so a request keeps what it read without a look
	ago := time.Now().Add(-time.Hour)
	if err := os.Chtimes(path("sync.json"), ago, ago); err != nil {
		t.Fatal(err)
	}
	defer filememo.Hold()()
	reads := loadReads.Load()
	for range 20 {
		c, ok := Load()
		if !ok || c.User != "me" {
			t.Fatalf("loaded %+v, %v", c, ok)
		}
		c.User = "changed"
	}
	if n := loadReads.Load() - reads; n != 1 {
		t.Fatalf("20 loads in one request read %d times, want 1", n)
	}
	if err := Off(); err != nil {
		t.Fatal(err)
	}
	if c, ok := Load(); ok {
		t.Fatalf("still on after Off: %+v", c)
	}
}

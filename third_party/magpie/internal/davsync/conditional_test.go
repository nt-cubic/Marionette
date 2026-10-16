package davsync

import (
	"context"
	"errors"
	"net/http"
	"net/http/httptest"
	"os"
	"runtime"
	"slices"
	"strings"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/backup"
	"github.com/yetone/magpie/internal/provider"
)

// A file unchanged on the server costs a request, not a download (Crispin
// on Discord, 坚果云 limiting what is read each month): the read is sent
// with the version last seen and answered 304. A file changed on the
// server is read in full and merged, as before; one changed only here is
// merged with the copy kept of the server's, not read again. A server
// with ETags, one that says them on a write too, and one with only
// Last-Modified.
func TestSyncUnchangedIsNotRead(t *testing.T) {
	for _, c := range []struct {
		name                  string
		putETag, lastModified bool
	}{{"etag", true, false}, {"etag-not-on-put", false, false}, {"last-modified", false, true}} {
		t.Run(c.name, func(t *testing.T) {
			fake := &fakeDAV{files: map[string][]byte{}, etags: map[string]string{}, dirs: map[string]bool{"/dav": true},
				cond: true, putETag: c.putETag, lastModified: c.lastModified}
			srv := httptest.NewServer(fake)
			defer srv.Close()
			ctx := context.Background()
			cfg := Config{URL: srv.URL + "/dav/", User: "me", Password: "pw", Passphrase: "correct horse", Keys: true, Agents: true}
			now := func(t *testing.T) {
				t.Helper()
				if err := Now(ctx); err != nil {
					t.Fatal(err)
				}
			}
			// afterPush is the full reads the sync after this computer's
			// write makes: none when the write said its ETag
			afterPush := 1
			if c.putETag {
				afterPush = 0
			}

			a, b := newComputer(t), newComputer(t)
			a.use(t)
			provider.Save(provider.Provider{ID: "deepseek", Name: "DeepSeek", Chat: "https://api.deepseek.com/v1", Key: "k1"})
			if err := Configure(cfg); err != nil {
				t.Fatal(err)
			}
			now(t)
			now(t)
			if fake.full != afterPush {
				t.Fatalf("after a's first write: %d full reads, want %d", fake.full, afterPush)
			}
			// nothing changed anywhere: 304s, nothing read, nothing written
			full, bytes, puts, gets, nm := fake.full, fake.bytes, fake.puts, fake.gets, fake.notModified
			for range 3 {
				now(t)
			}
			if fake.gets != gets+3 || fake.full != full || fake.bytes != bytes || fake.notModified != nm+3 || fake.puts != puts {
				t.Fatalf("unchanged: %d reads, %d full (%d bytes), %d 304s, %d writes", fake.gets-gets, fake.full-full, fake.bytes-bytes, fake.notModified-nm, fake.puts-puts)
			}
			if fi, err := os.Stat(path(cacheName)); err != nil || runtime.GOOS != "windows" && fi.Mode().Perm() != 0o600 {
				t.Fatalf("the server's copy: %v %v", fi, err)
			}

			// b joins and adds one: a reads the file in full, once, and has it
			b.use(t)
			if err := Configure(cfg); err != nil {
				t.Fatal(err)
			}
			now(t)
			provider.Save(provider.Provider{ID: "kimi", Name: "Kimi", Chat: "https://api.moonshot.cn/v1", Key: "k2"})
			now(t)
			a.use(t)
			full = fake.full
			now(t)
			if fake.full != full+1 {
				t.Fatalf("changed on the server: %d full reads", fake.full-full)
			}
			if got := ids(); !slices.Equal(got, []string{"deepseek=k1", "kimi=k2"}) {
				t.Fatalf("a after b added: %v", got)
			}
			full, notModified := fake.full, fake.notModified
			now(t)
			if fake.full != full || fake.notModified != notModified+1 {
				t.Fatalf("unchanged after a read: %d full reads", fake.full-full)
			}

			// changed only here: merged with the copy kept, not read again
			provider.Save(provider.Provider{ID: "three", Name: "Three", Chat: "https://z/v1", Key: "k3"})
			full, puts = fake.full, fake.puts
			now(t)
			if fake.full != full || fake.puts != puts+1 {
				t.Fatalf("changed here: %d full reads, %d writes", fake.full-full, fake.puts-puts)
			}
			remote, err := backup.Open(fake.files["/dav/magpie/magpie.magpie-backup"], "correct horse")
			if err != nil || len(remote.Providers) != 3 {
				t.Fatalf("on the server: %v %+v", err, remote.Providers)
			}
			b.use(t)
			now(t)
			if got := ids(); !slices.Equal(got, []string{"deepseek=k1", "kimi=k2", "three=k3"}) {
				t.Fatalf("b after a added: %v", got)
			}

			// the copy gone: the file read again, and merged as before
			a.use(t)
			now(t) // b's merge of a's write, if any, read
			os.Remove(path(cacheName))
			provider.Delete("deepseek")
			full = fake.full
			now(t)
			if fake.full != full+1 {
				t.Fatalf("no copy: %d full reads", fake.full-full)
			}
			if remote, _ := backup.Open(fake.files["/dav/magpie/magpie.magpie-backup"], "correct horse"); len(remote.Providers) != 2 {
				t.Fatalf("on the server: %+v", remote.Providers)
			}
		})
	}
}

// A server that doesn't do conditional reads sends the file every time, as
// it always did, and the sync is the same: nothing written when nothing
// changed.
func TestSyncIgnoringConditions(t *testing.T) {
	fake := &fakeDAV{files: map[string][]byte{}, etags: map[string]string{}, dirs: map[string]bool{"/dav": true}, putETag: true}
	srv := httptest.NewServer(fake)
	defer srv.Close()
	newComputer(t).use(t)
	provider.Save(provider.Provider{ID: "deepseek", Name: "DeepSeek", Chat: "https://api.deepseek.com/v1", Key: "k1"})
	if err := Configure(Config{URL: srv.URL + "/dav/", User: "me", Password: "pw", Passphrase: "correct horse", Keys: true}); err != nil {
		t.Fatal(err)
	}
	for range 4 {
		if err := Now(context.Background()); err != nil {
			t.Fatal(err)
		}
	}
	if fake.full != 3 || fake.notModified != 0 || fake.puts != 1 || Status().Error != "" {
		t.Fatalf("%d full reads, %d writes: %+v", fake.full, fake.puts, Status())
	}
}

// A server answering 429 is said to be limiting requests, and is tried
// less often: as long as its Retry-After says, up to 6 hours, or else
// twice as long each time, up to 30 minutes; back to every 3 after a sync
// that goes through.
func TestSyncRateLimited(t *testing.T) {
	fake := &fakeDAV{files: map[string][]byte{}, etags: map[string]string{}, dirs: map[string]bool{"/dav": true}}
	srv := httptest.NewServer(fake)
	defer srv.Close()
	newComputer(t).use(t)
	if err := Configure(Config{URL: srv.URL + "/dav/", User: "me", Password: "pw", Passphrase: "correct horse", Keys: true}); err != nil {
		t.Fatal(err)
	}
	fake.tooMany = true
	err := Now(context.Background())
	var rl *rateLimited
	if !errors.As(err, &rl) || !strings.Contains(err.Error(), "limiting how often") || !strings.Contains(err.Error(), "429") {
		t.Fatalf("429: %v", err)
	}
	next := Every
	var waits []time.Duration
	for range 6 {
		next = backoff(err, next)
		waits = append(waits, next)
	}
	want := []time.Duration{6 * time.Minute, 12 * time.Minute, 24 * time.Minute, 30 * time.Minute, 30 * time.Minute, 30 * time.Minute}
	if !slices.Equal(waits, want) {
		t.Fatalf("waits %v, want %v", waits, want)
	}
	for _, c := range []struct {
		after string
		want  time.Duration
	}{{"900", 15 * time.Minute}, {"10", Every}, {"86400", 6 * time.Hour}} {
		fake.retryAfter = c.after
		err := Now(context.Background())
		if got := backoff(err, Every); got != c.want {
			t.Errorf("Retry-After %s: %v, want %v (%v)", c.after, got, c.want, err)
		}
	}
	fake.tooMany = false
	err = Now(context.Background())
	if err != nil || backoff(err, 30*time.Minute) != Every {
		t.Fatalf("through again: %v", err)
	}
	if backoff(errLogin, 30*time.Minute) != Every {
		t.Fatal("another error waits longer")
	}
}

// Through S3 as well: the read is sent with If-None-Match, signed with the
// rest, and an object unchanged isn't sent again; a 503 SlowDown is the
// server limiting requests.
func TestS3Unchanged(t *testing.T) {
	f, srv := newFakeS3(t)
	newComputer(t).use(t)
	provider.Save(provider.Provider{ID: "deepseek", Name: "DeepSeek", Chat: "https://api.deepseek.com/v1", Key: "k1"})
	if err := Configure(f.config(srv)); err != nil {
		t.Fatal(err)
	}
	ctx := context.Background()
	for range 3 {
		if err := Now(ctx); err != nil {
			t.Fatal(err)
		}
	}
	etag := f.etags["team x+y/magpie/magpie.magpie-backup"]
	if want := []string{"GET", "PUT none:*", "GET none:" + etag, "GET none:" + etag}; !slices.Equal(f.log, want) || f.sent != 0 {
		t.Fatalf("requests %q (%d bytes read), want %q", f.log, f.sent, want)
	}

	slow := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Retry-After", "600")
		w.WriteHeader(http.StatusServiceUnavailable)
		w.Write([]byte(`<?xml version="1.0" encoding="UTF-8"?><Error><Code>SlowDown</Code><Message>Please reduce your request rate.</Message></Error>`))
	}))
	defer slow.Close()
	s, err := newS3(f.config(slow))
	if err != nil {
		t.Fatal(err)
	}
	_, _, err = s.get(ctx, version{})
	var rl *rateLimited
	if !errors.As(err, &rl) || backoff(err, Every) != 10*time.Minute || !strings.Contains(err.Error(), "S3 server is busy, or limiting") {
		t.Fatalf("SlowDown: %v", err)
	}
}

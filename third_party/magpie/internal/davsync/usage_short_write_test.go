package davsync

import (
	"bytes"
	"context"
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"reflect"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/usage"
)

// shortUsageDAV keeps the first cut usage writes short (-1 cuts them all),
// but answers 201 just as a server behind a broken tunnel does. The backup
// and folder operations use the ordinary test server unchanged.
type shortUsageDAV struct {
	*usageDAV
	cut, writes, sent int // guarded by fakeDAV.mu
	headStatus        int
	unknownSize       bool
}

func (d *shortUsageDAV) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	if !strings.HasPrefix(r.URL.Path, "/dav/magpie/usage/") {
		d.usageDAV.ServeHTTP(w, r)
		return
	}
	if u, p, ok := r.BasicAuth(); !ok || u != "me" || p != "pw" {
		w.WriteHeader(http.StatusUnauthorized)
		return
	}
	switch r.Method {
	case http.MethodPut:
		body, _ := io.ReadAll(r.Body)
		r.Body.Close()
		d.mu.Lock()
		short := false
		if d.dirs[urlDir(r.URL.Path)] {
			d.writes++
			d.sent = len(body)
			short = d.cut < 0 || d.writes <= d.cut
		}
		d.mu.Unlock()
		if short {
			body = body[:len(body)/2]
		}
		r.Body = io.NopCloser(bytes.NewReader(body))
		r.ContentLength = int64(len(body))
		d.usageDAV.ServeHTTP(w, r)
	case http.MethodHead:
		d.mu.Lock()
		defer d.mu.Unlock()
		if d.headStatus != 0 {
			w.WriteHeader(d.headStatus)
			return
		}
		body, ok := d.files[r.URL.Path]
		if !ok {
			w.WriteHeader(http.StatusNotFound)
			return
		}
		if !d.unknownSize {
			w.Header().Set("Content-Length", strconv.Itoa(len(body)))
		}
		w.Header().Set("ETag", d.etags[r.URL.Path])
		w.WriteHeader(http.StatusOK)
		w.(http.Flusher).Flush() // an unknown size stays unknown
	default:
		d.usageDAV.ServeHTTP(w, r)
	}
}

func usageShortServer(t *testing.T, cut int) (*shortUsageDAV, Config) {
	t.Helper()
	f := &shortUsageDAV{usageDAV: &usageDAV{fakeDAV: &fakeDAV{
		files: map[string][]byte{}, etags: map[string]string{},
		dirs: map[string]bool{"/dav": true, "/dav/magpie": true, "/dav/magpie/usage": true},
	}}, cut: cut}
	srv := httptest.NewServer(f)
	t.Cleanup(srv.Close)
	return f, Config{URL: srv.URL + "/dav", User: "me", Password: "pw", Passphrase: "correct horse", Keys: true, Agents: true, Usage: true}
}

// A day is marked sent only after a complete write. A persistently short
// write stops after three attempts; a later sync repairs the unchanged day.
func TestUsageSharedShortWrite(t *testing.T) {
	for _, c := range []struct {
		name        string
		cut, writes int
		fresh       bool
		headStatus  int
		unknownSize bool
	}{
		{name: "complete", writes: 1},
		{name: "first short", cut: 1, writes: 2},
		{name: "always short", cut: -1, writes: 3},
		{name: "folder first", cut: 1, writes: 2, fresh: true},
		{name: "HEAD unsupported", writes: 1, headStatus: http.StatusMethodNotAllowed},
		{name: "HEAD unavailable", writes: 1, headStatus: http.StatusServiceUnavailable},
		{name: "size unknown", writes: 1, unknownSize: true},
	} {
		t.Run(c.name, func(t *testing.T) {
			newComputer(t).use(t)
			f, cfg := usageShortServer(t, c.cut)
			f.mu.Lock()
			f.headStatus, f.unknownSize = c.headStatus, c.unknownSize
			if c.fresh {
				f.dirs = map[string]bool{"/dav": true}
			}
			f.mu.Unlock()
			d, err := newDAV(cfg)
			if err != nil {
				t.Fatal(err)
			}
			record := call(time.Now().UTC(), "deepseek-chat", 100)
			usage.Append(record)
			day := record.Time.Local().Format(time.DateOnly)
			id, _ := usage.Computer()
			name := id + "-" + day + usageExt
			st := &usageState{}
			ctx := context.Background()
			err = shareWith(ctx, cfg, st, d)
			f.mu.Lock()
			writes := f.writes
			kept, sent := len(f.files["/dav/magpie/usage/"+name]), f.sent
			f.mu.Unlock()
			if writes != c.writes {
				t.Errorf("writes=%d, want %d", writes, c.writes)
			}
			if c.cut < 0 {
				if err == nil || !strings.Contains(err.Error(), "cut short") || !strings.Contains(err.Error(), name) || !strings.Contains(err.Error(), fmt.Sprintf("%d of %d bytes", kept, sent)) {
					t.Errorf("short write error: %v, want the filename and kept size", err)
				}
				if len(st.Sent) != 0 || st.Whole {
					t.Errorf("short day marked sent: %+v", st)
				}
				if _, err := readDay(ctx, d, name, cfg.Passphrase); err == nil {
					t.Fatal("the server's cut file was unexpectedly readable")
				}
				f.mu.Lock()
				f.cut = 0
				f.mu.Unlock()
				if err := shareWith(ctx, cfg, st, d); err != nil {
					t.Fatalf("repairing the unchanged day: %v", err)
				}
				f.mu.Lock()
				writes = f.writes
				f.mu.Unlock()
				if writes != c.writes+1 {
					t.Errorf("writes after repair=%d, want %d", writes, c.writes+1)
				}
			} else if err != nil {
				t.Fatalf("sharing a whole day: %v", err)
			}
			shared, err := readDay(ctx, d, name, cfg.Passphrase)
			if err != nil {
				t.Fatalf("reading the shared day: %v", err)
			}
			if shared.Computer != id || shared.Day != day || len(shared.Calls) != 1 || !reflect.DeepEqual(shared.Calls[0].Record, record) {
				t.Errorf("shared day: %+v, want the original call", shared)
			}
			if len(st.Sent) != 1 || st.Sent[day] == "" || !st.Whole {
				t.Errorf("whole day not marked sent: %+v", st)
			}
			if err := shareWith(ctx, cfg, st, d); err != nil {
				t.Fatal(err)
			}
			f.mu.Lock()
			defer f.mu.Unlock()
			if f.writes != writes {
				t.Errorf("unchanged whole day uploaded again: %d writes, was %d", f.writes, writes)
			}
		})
	}
}

// A usage failure is visible separately from configuration sync and is
// cleared once the same day can be uploaded whole on the next sync.
func TestUsageSyncShortWrite(t *testing.T) {
	newComputer(t).use(t)
	f, cfg := usageShortServer(t, -1)
	if err := Configure(cfg); err != nil {
		t.Fatal(err)
	}
	usage.Append(call(time.Now().UTC(), "deepseek-chat", 100))
	ctx := context.Background()
	if err := SyncNow(ctx); err != nil {
		t.Fatalf("configuration sync: %v", err)
	}
	if s := Status(); s.Error != "" || s.Last.IsZero() || !strings.Contains(s.UsageError, "cut short") {
		t.Errorf("sync status: %+v", s)
	}
	if s := loadState().Usage; s == nil || !s.At.IsZero() || len(s.Sent) != 0 {
		t.Errorf("usage state after short write: %+v", s)
	}
	f.mu.Lock()
	f.cut = 0
	f.mu.Unlock()
	if err := SyncNow(ctx); err != nil {
		t.Fatalf("configuration sync after repair: %v", err)
	}
	if s := Status(); s.UsageError != "" {
		t.Errorf("usage error after repair: %q", s.UsageError)
	}
	if s := loadState().Usage; s == nil || s.At.IsZero() || len(s.Sent) != 1 {
		t.Errorf("usage state after repair: %+v", s)
	}
	f.mu.Lock()
	defer f.mu.Unlock()
	if f.writes != 4 {
		t.Errorf("usage writes=%d, want three short and one whole", f.writes)
	}
}

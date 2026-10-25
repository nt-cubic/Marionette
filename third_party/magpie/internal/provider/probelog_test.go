package provider

import (
	"context"
	"net/http"
	"net/http/httptest"
	"path/filepath"
	"sort"
	"strings"
	"sync"
	"sync/atomic"
	"testing"

	"github.com/yetone/magpie/internal/probelog"
)

// probeRows catches what probelog writes down while the test runs, through
// the HTTP client's transport as netproxy.Install sets it.
func probeRows(t *testing.T) func() []probelog.Row {
	t.Helper()
	var mu sync.Mutex
	var rows []probelog.Row
	sink, tr := probelog.Sink, http.DefaultClient.Transport
	probelog.Sink = func(r probelog.Row) { mu.Lock(); rows = append(rows, r); mu.Unlock() }
	next := tr
	if next == nil {
		next = http.DefaultTransport
	}
	http.DefaultClient.Transport = probelog.Transport(next)
	t.Cleanup(func() { probelog.Sink, http.DefaultClient.Transport = sink, tr })
	return func() []probelog.Row {
		mu.Lock()
		defer mu.Unlock()
		return append([]probelog.Row(nil), rows...)
	}
}

func probeHome(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)
	t.Setenv("USERPROFILE", home)
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(home, ".config"))
	t.Setenv("XDG_CACHE_HOME", filepath.Join(home, ".cache"))
}

// The editor's Test writes each request it sends down once: the model list
// as a model list, each endpoint's test as a test, with its model and the
// status the vendor answered (neiko, gys619 on Discord: a test left no
// trace in the request log).
func TestTestWritesItsRequestsDown(t *testing.T) {
	probeHome(t)
	rows := probeRows(t)
	var hits atomic.Int32
	srv := httptest.NewServer(http.HandlerFunc(func(rw http.ResponseWriter, r *http.Request) {
		hits.Add(1)
		if r.Method == http.MethodGet {
			_, _ = rw.Write([]byte(`{"data":[{"id":"gpt-6-sol"}]}`))
			return
		}
		if r.URL.Path == "/v1/messages" {
			rw.WriteHeader(http.StatusUnauthorized)
			_, _ = rw.Write([]byte(`{"error":{"message":"bad key"}}`))
			return
		}
		_, _ = rw.Write([]byte(`{}`))
	}))
	defer srv.Close()
	p := Provider{ID: "relay", Name: "Relay", Chat: srv.URL + "/v1", Anthropic: srv.URL,
		Key: "sk-chat", KeyProtocol: Chat, Models: []string{"gpt-6-sol"},
		Keys: []KeyAccount{{Key: "sk-anthropic", Protocol: Anthropic}}}
	p.Test(context.Background())
	got := rows()
	if int(hits.Load()) != len(got) {
		t.Fatalf("%d requests, %d rows: %+v", hits.Load(), len(got), got)
	}
	var lines []string
	for _, r := range got {
		if r.Provider != "relay" || r.Host != strings.TrimPrefix(srv.URL, "http://") {
			t.Errorf("row %+v", r)
		}
		lines = append(lines, r.Kind+" "+r.Endpoint+" "+r.Model+" "+http.StatusText(r.Status))
	}
	sort.Strings(lines)
	s := strings.Join(lines, "\n")
	for _, want := range []string{"model_list GET /v1/models  OK", "provider_test /v1/chat/completions gpt-6-sol OK", "provider_test /v1/messages "} {
		if !strings.Contains(s, want) {
			t.Errorf("no %q in rows:\n%s", want, s)
		}
	}
	if !strings.Contains(s, "Unauthorized") {
		t.Errorf("the refused test's status is lost:\n%s", s)
	}
}

// Detect APIs writes its requests down as a detection; a request sent
// with no mark (an agent's turn) is written nowhere.
func TestDetectWritesItsRequestsDown(t *testing.T) {
	probeHome(t)
	rows := probeRows(t)
	var hits atomic.Int32
	srv := httptest.NewServer(http.HandlerFunc(func(rw http.ResponseWriter, r *http.Request) {
		hits.Add(1)
		if r.Method == http.MethodGet {
			_, _ = rw.Write([]byte(`{"data":[{"id":"m1"}]}`))
			return
		}
		_, _ = rw.Write([]byte(`{}`))
	}))
	defer srv.Close()
	if _, err := (Provider{ID: "relay", Key: "k"}).Detect(context.Background(), srv.URL, "m1"); err != nil {
		t.Fatal(err)
	}
	got := rows()
	if len(got) == 0 || int(hits.Load()) != len(got) {
		t.Fatalf("%d requests, %d rows: %+v", hits.Load(), len(got), got)
	}
	for _, r := range got {
		if r.Kind != probelog.KindDetect && r.Kind != probelog.KindList || r.Provider != "relay" {
			t.Errorf("row %+v", r)
		}
		if r.Kind == probelog.KindDetect && !strings.HasPrefix(r.Endpoint, "GET ") && r.Model != "m1" {
			t.Errorf("a detection's row has no model: %+v", r)
		}
	}
	before := len(got)
	req, _ := http.NewRequest(http.MethodPost, srv.URL+"/v1/chat/completions", strings.NewReader(`{}`))
	res, err := (Provider{ID: "relay"}).Do(http.DefaultClient, req)
	if err != nil {
		t.Fatal(err)
	}
	res.Body.Close()
	if len(rows()) != before {
		t.Fatal("an agent's request was written down as magpie's own")
	}
}

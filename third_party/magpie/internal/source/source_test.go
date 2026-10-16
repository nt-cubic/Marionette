package source

import (
	"context"
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"net/url"
	"strings"
	"sync/atomic"
	"testing"
	"time"
)

func TestURLs(t *testing.T) {
	t.Setenv("MAGPIE_MIRRORS", "on")
	t.Setenv("MAGPIE_NPM_REGISTRY", "")
	t.Setenv("MAGPIE_GITHUB_MIRROR", "")

	got := URLs("https://registry.npmjs.org/@scope/pkg/latest?x=1")
	want := []string{
		"https://registry.npmjs.org/@scope/pkg/latest?x=1",
		"https://registry.npmmirror.com/@scope/pkg/latest?x=1",
	}
	if strings.Join(got, "\n") != strings.Join(want, "\n") {
		t.Fatalf("npm URLs = %q, want %q", got, want)
	}

	got = URLs("https://github.com/owner/repo/releases/download/v1/file")
	want = []string{
		"https://github.com/owner/repo/releases/download/v1/file",
		"https://gh-proxy.com/https://github.com/owner/repo/releases/download/v1/file",
	}
	if strings.Join(got, "\n") != strings.Join(want, "\n") {
		t.Fatalf("github URLs = %q, want %q", got, want)
	}

	t.Setenv("MAGPIE_NPM_REGISTRY", "https://npm.example/")
	t.Setenv("MAGPIE_GITHUB_MIRROR", "https://gh.example")
	got = URLs("https://registry.npmjs.org/pkg")
	if got[1] != "https://npm.example/pkg" {
		t.Fatalf("npm mirror = %q", got[1])
	}
	got = URLs("https://api.github.com/repos/o/r")
	if got[1] != "https://gh.example/https://api.github.com/repos/o/r" {
		t.Fatalf("github mirror = %q", got[1])
	}
}

func TestURLsDefaultOff(t *testing.T) {
	t.Setenv("MAGPIE_MIRRORS", "")
	got := URLs("https://github.com/o/r")
	if len(got) != 1 || got[0] != "https://github.com/o/r" {
		t.Fatalf("URLs by default = %q", got)
	}
}

func TestURLsOff(t *testing.T) {
	t.Setenv("MAGPIE_MIRRORS", "off")
	got := URLs("https://github.com/o/r")
	if len(got) != 1 || got[0] != "https://github.com/o/r" {
		t.Fatalf("URLs with mirrors off = %q", got)
	}
}

func TestDoFallsBack(t *testing.T) {
	var mirrorHits atomic.Int32
	official := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		http.Error(w, "down", http.StatusBadGateway)
	}))
	defer official.Close()
	mirror := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		mirrorHits.Add(1)
		io.WriteString(w, "from mirror")
	}))
	defer mirror.Close()

	req, _ := http.NewRequest("GET", official.URL+"/thing", nil)
	resp, err := do(http.DefaultClient, req, []string{official.URL + "/thing", mirror.URL + "/thing"})
	if err != nil {
		t.Fatal(err)
	}
	defer resp.Body.Close()
	body, _ := io.ReadAll(resp.Body)
	if string(body) != "from mirror" || mirrorHits.Load() != 1 {
		t.Fatalf("body %q, mirror hits %d", body, mirrorHits.Load())
	}
}

func TestDoFallsBackOnRateLimit(t *testing.T) {
	var mirrorHits atomic.Int32
	official := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("X-RateLimit-Remaining", "0")
		http.Error(w, "limited", http.StatusForbidden)
	}))
	defer official.Close()
	mirror := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		mirrorHits.Add(1)
		io.WriteString(w, "from mirror")
	}))
	defer mirror.Close()

	req, _ := http.NewRequest("GET", official.URL+"/thing", nil)
	resp, err := do(http.DefaultClient, req, []string{official.URL + "/thing", mirror.URL + "/thing"})
	if err != nil {
		t.Fatal(err)
	}
	defer resp.Body.Close()
	if mirrorHits.Load() != 1 {
		t.Fatalf("mirror hits %d", mirrorHits.Load())
	}
}

func TestCredentialsStayOfficial(t *testing.T) {
	req, _ := http.NewRequest("GET", "https://api.github.com/repos/o/r", nil)
	if !mirrorable(req) {
		t.Fatal("a request without credentials can't use a mirror")
	}
	req.Header.Set("Authorization", "Bearer secret")
	if mirrorable(req) {
		t.Fatal("a request with Authorization can use a mirror")
	}
	req.Header.Del("Authorization")
	req.Header.Set("Cookie", "session=secret")
	if mirrorable(req) {
		t.Fatal("a request with a cookie can use a mirror")
	}
	req.Header.Del("Cookie")
	req.URL.User = url.UserPassword("user", "secret")
	if mirrorable(req) {
		t.Fatal("a request with URL userinfo can use a mirror")
	}
	req.URL.User = nil
	q := req.URL.Query()
	q.Set("access_token", "secret")
	req.URL.RawQuery = q.Encode()
	if mirrorable(req) {
		t.Fatal("a request with an access_token can use a mirror")
	}
}

func TestDoOfficialSkipsMirror(t *testing.T) {
	var mirrorHits atomic.Int32
	official := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		io.WriteString(w, "official")
	}))
	defer official.Close()
	mirror := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		mirrorHits.Add(1)
		io.WriteString(w, "mirror")
	}))
	defer mirror.Close()
	t.Setenv("MAGPIE_GITHUB_MIRROR", mirror.URL)

	req, _ := http.NewRequest("GET", official.URL+"/thing", nil)
	resp, err := DoOfficial(http.DefaultClient, req)
	if err != nil {
		t.Fatal(err)
	}
	defer resp.Body.Close()
	if mirrorHits.Load() != 0 {
		t.Fatalf("mirror hits %d", mirrorHits.Load())
	}
}

func TestDoJoinsErrors(t *testing.T) {
	official := "http://127.0.0.1:1/thing"
	mirror := "http://127.0.0.1:2/thing"
	req, _ := http.NewRequest("GET", official, nil)
	_, err := do(http.DefaultClient, req, []string{official, mirror})
	if err == nil {
		t.Fatal("both sources failed but no error")
	}
	if !strings.Contains(err.Error(), official) || !strings.Contains(err.Error(), mirror) {
		t.Fatalf("error %q doesn't name both sources", err)
	}
}

func TestDoKeepsAnAnswer(t *testing.T) {
	var mirrorHits atomic.Int32
	official := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		http.NotFound(w, r)
	}))
	defer official.Close()
	mirror := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		mirrorHits.Add(1)
	}))
	defer mirror.Close()

	req, _ := http.NewRequest("GET", official.URL+"/thing", nil)
	resp, err := do(http.DefaultClient, req, []string{official.URL + "/thing", mirror.URL + "/thing"})
	if err != nil {
		t.Fatal(err)
	}
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusNotFound || mirrorHits.Load() != 0 {
		t.Fatalf("status %d, mirror hits %d", resp.StatusCode, mirrorHits.Load())
	}
}

func TestDoBodyOutlivesCandidateTimeout(t *testing.T) {
	official := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		http.Error(w, "down", http.StatusBadGateway)
	}))
	defer official.Close()
	mirror := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusOK)
		if f, ok := w.(http.Flusher); ok {
			f.Flush()
		}
		time.Sleep(1200 * time.Millisecond)
		io.WriteString(w, "slow mirror")
	}))
	defer mirror.Close()

	req, _ := http.NewRequest("GET", official.URL+"/thing", nil)
	client := &http.Client{Timeout: 2 * time.Second}
	resp, err := do(client, req, []string{official.URL + "/thing", mirror.URL + "/thing"})
	if err != nil {
		t.Fatal(err)
	}
	defer resp.Body.Close()
	body, err := io.ReadAll(resp.Body)
	if err != nil {
		t.Fatal(err)
	}
	if string(body) != "slow mirror" {
		t.Fatalf("body = %q", body)
	}
}

type recordedTransport struct {
	requests []string
}

func (rt *recordedTransport) RoundTrip(req *http.Request) (*http.Response, error) {
	rt.requests = append(rt.requests, req.URL.String())
	status := http.StatusBadGateway
	if strings.HasPrefix(req.URL.String(), "https://gh.example/") {
		status = http.StatusOK
	}
	return &http.Response{
		StatusCode: status,
		Status:     fmt.Sprintf("%d %s", status, http.StatusText(status)),
		Header:     make(http.Header),
		Body:       io.NopCloser(strings.NewReader("")),
		Request:    req,
	}, nil
}

func TestDoKeepsCredentialsOfficial(t *testing.T) {
	t.Setenv("MAGPIE_MIRRORS", "on")
	t.Setenv("MAGPIE_GITHUB_MIRROR", "https://gh.example")
	rt := &recordedTransport{}
	req, _ := http.NewRequest("GET", "https://api.github.com/repos/o/r", nil)
	req.Header.Set("Authorization", "Bearer secret")
	resp, err := Do(&http.Client{Transport: rt}, req)
	if err != nil {
		t.Fatal(err)
	}
	resp.Body.Close()
	if len(rt.requests) != 1 || rt.requests[0] != "https://api.github.com/repos/o/r" {
		t.Fatalf("requests = %q", rt.requests)
	}
}

func TestDoLastSourceUsesRemainingBudget(t *testing.T) {
	official := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		time.Sleep(1100 * time.Millisecond)
		http.Error(w, "slow official", http.StatusBadGateway)
	}))
	defer official.Close()
	mirror := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		time.Sleep(700 * time.Millisecond)
		io.WriteString(w, "slow mirror")
	}))
	defer mirror.Close()

	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()
	req, _ := http.NewRequestWithContext(ctx, "GET", official.URL+"/thing", nil)
	resp, err := do(http.DefaultClient, req, []string{official.URL + "/thing", mirror.URL + "/thing"})
	if err != nil {
		t.Fatal(err)
	}
	defer resp.Body.Close()
	body, err := io.ReadAll(resp.Body)
	if err != nil {
		t.Fatal(err)
	}
	if string(body) != "slow mirror" {
		t.Fatalf("body = %q", body)
	}
}

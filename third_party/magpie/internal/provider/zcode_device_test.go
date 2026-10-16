package provider

import (
	"context"
	"net/http"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

// stripDeviceMid sends requests as magpie did before #282: with no
// X-Device-Mid.
type stripDeviceMid struct{}

func (stripDeviceMid) RoundTrip(r *http.Request) (*http.Response, error) {
	r = r.Clone(r.Context())
	r.Header.Del("X-Device-Mid")
	return http.DefaultTransport.RoundTrip(r)
}

// #282: zcode.z.ai's Start Plan balance answers 400, code 3001 "parameter
// error", to a request with no X-Device-Mid, so an account with only the
// Start Plan could not be added: the refusal was taken for the account
// having no Start Plan. magpie now names the machine as ZCode does, and a
// balance it can't read is said as it is.
func TestZCodeSignInStartPlanDeviceMid(t *testing.T) {
	home := signIn(t)
	jwt := zcodeTestJWT(time.Now().Add(24 * time.Hour))
	u := newZCodeStartUpstream(t, jwt)
	u.balance = zcodeActiveStart(time.Now(), "active")
	ctx := context.Background()

	k, plan, err := zcodeSignedIn(ctx, "zai", "zai-tok", jwt)
	if err != nil || plan != "Start Plan" || k.JWT != jwt {
		t.Fatalf("start plan: %v %q %+v", err, plan, k)
	}
	if q := zcodeStartQuota(ctx, Login{User: "a@example.com"}, jwt); q.Error != "" || len(q.Windows) != 1 {
		t.Fatalf("quota: %+v", q)
	}

	// the id is a UUID, kept in magpie's own directory, never ZCode's
	id := zcodeDeviceMid()
	if !zcodeUUIDRe.MatchString(id) || id[14] != '4' {
		t.Fatalf("not a v4 UUID: %q", id)
	}
	if b, err := os.ReadFile(filepath.Join(home, ".config", "magpie", "zcode-device-mid")); err != nil || strings.TrimSpace(string(b)) != id {
		t.Fatalf("kept: %v %q", err, b)
	}
	if _, err := os.Stat(filepath.Join(home, ".zcode")); !os.IsNotExist(err) {
		t.Fatalf("touched ~/.zcode: %v", err)
	}
	// the same next call, and in the next run of magpie
	if zcodeDeviceMid() != id {
		t.Fatal("changed between calls")
	}
	zcodeDevices.Lock()
	zcodeDevices.m = map[string]string{}
	zcodeDevices.Unlock()
	if got := zcodeDeviceMid(); got != id {
		t.Fatalf("changed between runs: %q %q", got, id)
	}
	// but not on the Start Plan's model requests, which ZCode sends without it
	req, _ := http.NewRequest("POST", u.srv.URL+"/api/v1/zcode-plan/anthropic/v1/messages", nil)
	zcodeSourceHeaders(req)
	if req.Header.Get("X-Device-Mid") != "" {
		t.Fatalf("model request: %v", req.Header)
	}
	// not to Z.ai's business API
	req, _ = http.NewRequest("GET", "https://api.z.ai/api/biz/subscription/list", nil)
	zcodeDeviceHeader(req)
	if req.Header.Get("X-Device-Mid") != "" {
		t.Fatalf("business API: %v", req.Header)
	}

	// without it, as before: refused, and the refusal is what the sign-in says
	old := http.DefaultClient
	http.DefaultClient = &http.Client{Transport: stripDeviceMid{}}
	t.Cleanup(func() { http.DefaultClient = old })
	_, _, err = zcodeSignedIn(ctx, "zai", "zai-tok", jwt)
	if err == nil || !strings.Contains(err.Error(), "parameter error (400, code 3001)") || strings.Contains(err.Error(), "has ended") {
		t.Fatalf("without X-Device-Mid: %v", err)
	}
	http.DefaultClient = old

	// no Start Plan at all still says so
	u.balance = zcodeActiveStart(time.Now(), "expired")
	if _, _, err := zcodeSignedIn(ctx, "zai", "zai-tok", jwt); err == nil || !strings.Contains(err.Error(), "has ended or was never started") {
		t.Fatalf("no start plan: %v", err)
	}
}

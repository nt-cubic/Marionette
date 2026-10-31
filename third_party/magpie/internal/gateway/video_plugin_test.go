package gateway

import (
	"context"
	"io"
	"net/http"
	"slices"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/access"
	"github.com/yetone/magpie/internal/plugin"
	"github.com/yetone/magpie/internal/provider"
)

// A plugin's model that says it gives video (Veo, on XYenon's Google AI
// Studio plugin) makes videos: started, polled and fetched at the
// OpenAI-shaped videos API under the plugin's base, through its fetch, on
// the account that started it, whichever is first now. A gateway key held
// to another account may not ask after it, nor one held to other models
// start or ask after one; the start is in the usage ledger. The old
// gateway listed no plugin's video models and turned the start away 400.
func TestPluginMakesVideos(t *testing.T) {
	t.Setenv("FAKE_GOOGLE", "1")
	t.Setenv("FAKE_VIDEO", "1")
	var mu sync.Mutex
	var asked []string
	const vid = "models/veo-3.1-generate-preview/operations/abc.1"
	pid := besideFake(t, "google-ai-studio", http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		b, _ := io.ReadAll(r.Body)
		key := strings.TrimPrefix(r.Header.Get("Authorization"), "Bearer ")
		mu.Lock()
		asked = append(asked, r.Method+" "+r.URL.EscapedPath()+" "+key)
		mu.Unlock()
		w.Header().Set("Content-Type", "application/json")
		switch {
		case r.Method == "POST" && r.URL.Path == "/v1/videos":
			if !strings.Contains(string(b), `"model":"veo-3.1-generate-preview"`) || !strings.Contains(string(b), `"prompt":"a magpie"`) {
				w.WriteHeader(400)
				io.WriteString(w, `{"error":{"message":"sent `+strings.ReplaceAll(string(b), `"`, `'`)+`"}}`)
				return
			}
			io.WriteString(w, `{"id":"`+vid+`","object":"video","status":"queued","progress":0,"model":"veo-3.1-generate-preview"}`)
		case r.Method == "GET" && r.URL.Path == "/v1/videos/"+vid:
			io.WriteString(w, `{"id":"`+vid+`","object":"video","status":"completed","progress":100,"model":"veo-3.1-generate-preview"}`)
		case r.Method == "GET" && r.URL.Path == "/v1/videos/"+vid+"/content":
			w.Header().Set("Content-Type", "video/mp4")
			io.WriteString(w, "\x00\x00\x00\x18ftypmp42fake-veo")
		default:
			w.WriteHeader(404)
		}
	}))
	ctx, cancel := context.WithTimeout(context.Background(), time.Minute)
	defer cancel()
	if _, err := plugin.APIKey(ctx, "google-ai-studio", 0, nil, "k2", plugin.NewAccount); err != nil {
		t.Fatal(err)
	}
	p, err := provider.Find(pid)
	if err != nil {
		t.Fatal(err)
	}
	if ms := Videomakers(*p); len(ms) != 1 || ms[0].ID != "veo-3.1-generate-preview" || ms[0].Provider != pid {
		t.Fatalf("videomakers %v, want veo-3.1-generate-preview alone", ms)
	}
	accts := append([]provider.Provider{*p}, p.AlsoOn()...)
	if len(accts) != 2 {
		t.Fatalf("%d accounts, want 2", len(accts))
	}
	onSecond := "" // the second account's id, as a key's hold names it
	for _, q := range accts[1:] {
		onSecond = pid + "/" + q.AccountID()
	}
	keys, secrets := newCaller(t, "OnSecond", "OnFirst", "OtherModels")
	for i, c := range []access.Change{
		{Key: keys[0].ID, Accounts: []string{onSecond}},
		{Key: keys[1].ID, Accounts: []string{pid + "/" + accts[0].AccountID()}},
		{Key: keys[2].ID, Models: []string{"elsewhere/*"}},
	} {
		if _, err := access.Update([]string{"accounts-key", "accounts-key", "models-key"}[i], c); err != nil {
			t.Fatal(err)
		}
	}
	h := New().Handler()
	code, obj, raw := asKey(t, h, secrets[0], "POST", "/v1/videos", `{"model":"`+pid+`/veo-3.1-generate-preview","prompt":"a magpie"}`)
	id, _ := obj["id"].(string)
	if code != 200 || id == "" {
		t.Fatalf("start: %d %s", code, raw)
	}
	// a caller with no hold asks after it on the account that started it
	if code, obj, raw = asKey(t, h, "", "GET", "/v1/videos/"+id, ""); code != 200 || obj["status"] != "completed" || obj["model"] != pid+"/veo-3.1-generate-preview" {
		t.Fatalf("poll: %d %s", code, raw)
	}
	if code, _, raw = asKey(t, h, secrets[0], "GET", "/v1/videos/"+id+"/content", ""); code != 200 || !strings.HasSuffix(string(raw), "fake-veo") {
		t.Fatalf("content: %d %s", code, raw)
	}
	mu.Lock()
	got := slices.Clone(asked)
	mu.Unlock()
	// the content is asked for after the model making it (videoModelHeld)
	if len(got) != 4 || !strings.HasPrefix(got[0], "POST /v1/videos ") || !strings.HasSuffix(got[3], "/content k2") {
		t.Fatalf("the vendor was asked %v", got)
	}
	for _, a := range got {
		if !strings.HasSuffix(a, " k2") {
			t.Fatalf("asked on another account than the second: %v", got)
		}
	}
	if code, _, raw = asKey(t, h, secrets[1], "GET", "/v1/videos/"+id, ""); code != 403 {
		t.Fatalf("a key held to the first account asked after the second's video: %d %s", code, raw)
	}
	if code, _, raw = asKey(t, h, secrets[2], "GET", "/v1/videos/"+id, ""); code != 403 {
		t.Fatalf("a key held to other models asked after it: %d %s", code, raw)
	}
	if code, _, raw = asKey(t, h, secrets[2], "POST", "/v1/videos", `{"model":"`+pid+`/veo-3.1-generate-preview","prompt":"a magpie"}`); code != 403 {
		t.Fatalf("a key held to other models started one: %d %s", code, raw)
	}
	if rows := usageRows(t, "generate_content", 1); len(rows) != 1 || rows[0].Provider != pid || rows[0].Model != "veo-3.1-generate-preview" || rows[0].Status != 200 {
		t.Fatalf("usage %+v", rows)
	}
}

package gateway

import (
	"fmt"
	"io"
	"net/http"
	"strings"
	"sync"
	"testing"

	"github.com/yetone/magpie/internal/access"
	"github.com/yetone/magpie/internal/provider"
)

// A plugin whose models are all on Gemini's API (XYenon's Google AI Studio
// plugin) still takes embeddings and speech: they go OpenAI-shaped to its
// fetch, at <its base>/embeddings and /audio/speech, for the plugin to
// put in its vendor's shape, with the model's own id. The old gateway
// turned them away 400, the provider having no chat base. A gateway key
// held to other models is refused, and each call is in the usage ledger.
func TestGooglePluginTakesEmbeddingsAndSpeech(t *testing.T) {
	t.Setenv("FAKE_GOOGLE", "1")
	t.Setenv("FAKE_KINDS", "1")
	var mu sync.Mutex
	sent := map[string][]string{}
	pid := besideFake(t, "google-ai-studio", http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		b, _ := io.ReadAll(r.Body)
		mu.Lock()
		sent[r.URL.Path] = append(sent[r.URL.Path], string(b))
		mu.Unlock()
		switch r.URL.Path {
		case "/v1/embeddings":
			w.Header().Set("Content-Type", "application/json")
			fmt.Fprint(w, `{"object":"list","data":[{"object":"embedding","index":0,"embedding":[0.1,-0.2]}],"model":"gemini-embedding-001","usage":{"prompt_tokens":3,"total_tokens":3}}`)
		case "/v1/audio/speech":
			w.Header().Set("Content-Type", "audio/wav")
			io.WriteString(w, "RIFF-fake-wav")
		default:
			w.WriteHeader(404)
		}
	}))
	p, err := provider.Find(pid)
	if err != nil {
		t.Fatal(err)
	}
	if p.Base(provider.Chat) != "" {
		t.Fatalf("the provider has a chat base %q: every model of it should be on Gemini's API", p.Base(provider.Chat))
	}
	s := New()
	code, raw := postRetrieval(t, s, "/v1/embeddings", `{"model":"`+pid+`/gemini-embedding-001","input":"hello"}`)
	if code != 200 || !strings.Contains(raw, `"embedding":[0.1,-0.2]`) {
		t.Fatalf("embeddings: %d %s", code, raw)
	}
	code, _, audio := postSpeech(t, s, "/v1/audio/speech", `{"model":"`+pid+`/gemini-2.5-flash-preview-tts","input":"Hello from the magpie.","voice":"Kore"}`)
	if code != 200 || string(audio) != "RIFF-fake-wav" {
		t.Fatalf("speech: %d %s", code, audio)
	}
	mu.Lock()
	if e := sent["/v1/embeddings"]; len(e) != 1 || !strings.Contains(e[0], `"model":"gemini-embedding-001"`) || !strings.Contains(e[0], `"input":"hello"`) {
		t.Fatalf("embeddings were sent %v", e)
	}
	if a := sent["/v1/audio/speech"]; len(a) != 1 || !strings.Contains(a[0], `"model":"gemini-2.5-flash-preview-tts"`) || !strings.Contains(a[0], `"voice":"Kore"`) {
		t.Fatalf("speech was sent %v", a)
	}
	mu.Unlock()
	if rows := usageRows(t, "embeddings", 1); len(rows) != 1 || rows[0].Provider != pid || rows[0].Model != "gemini-embedding-001" || rows[0].Input != 3 || rows[0].Status != 200 {
		t.Fatalf("embeddings usage %+v", rows)
	}
	if rows := usageRows(t, "speech", 1); len(rows) != 1 || rows[0].Provider != pid || rows[0].Status != 200 {
		t.Fatalf("speech usage %+v", rows)
	}

	keys, secrets := newCaller(t, "Elsewhere")
	if _, err := access.Update("models-key", access.Change{Key: keys[0].ID, Models: []string{"elsewhere/*"}}); err != nil {
		t.Fatal(err)
	}
	h := s.Handler()
	if code, _, raw := asKey(t, h, secrets[0], "POST", "/v1/embeddings", `{"model":"`+pid+`/gemini-embedding-001","input":"hello"}`); code != 403 {
		t.Fatalf("a key held to other models: %d %s", code, raw)
	}
	if code, _, raw := asKey(t, h, secrets[0], "POST", "/v1/audio/speech", `{"model":"`+pid+`/gemini-2.5-flash-preview-tts","input":"hi"}`); code != 403 {
		t.Fatalf("a key held to other models, speech: %d %s", code, raw)
	}
	mu.Lock()
	defer mu.Unlock()
	if len(sent["/v1/embeddings"]) != 1 || len(sent["/v1/audio/speech"]) != 1 {
		t.Fatalf("a refused key's request reached the plugin: %v", sent)
	}
}

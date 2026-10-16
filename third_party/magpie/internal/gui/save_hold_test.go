package gui

import (
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/provider"
)

// A save answers with the whole state it wrote — every group's members,
// every model's facts — and used to rebuild the catalog for each of those
// look-ups: tens of milliseconds each, seconds over a slow disk. A write
// shares one build now, as a read does.
//
// What makes that safe is the write's own catalog.Touched dropping what is
// held as it happens, so this checks both halves: the save is cheap, and
// the answer is the state after the write, never the one before it.
func TestSaveSharesOneCatalogBuild(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)
	t.Setenv("USERPROFILE", home)
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(home, ".config"))
	t.Setenv("XDG_CACHE_HOME", filepath.Join(home, ".cache"))
	for _, id := range []string{"a", "b", "c"} {
		if err := provider.Save(provider.Provider{ID: id, Name: id, Key: "k" + id,
			Chat: "http://127.0.0.1:1/v1", Models: []string{"m1", "m2"}}); err != nil {
			t.Fatal(err)
		}
	}
	h := Handler(nil, nil)

	post := func(path, body string) (int, string) {
		t.Helper()
		rec := httptest.NewRecorder()
		h.ServeHTTP(rec, httptest.NewRequest("POST", path, strings.NewReader(body)))
		return rec.Code, rec.Body.String()
	}
	// one save builds the providers once, as a read does: the state it
	// answers with asks for each group's members and each model's facts, and
	// without the hold each of those look-ups built them anew — 20 of them
	// here, a second and more over a slow disk.
	warm := func() { post("/api/groups/save", `{"id":"g0","from":"g0","name":"G0","members":["a/m1"]}`) }
	warm()
	before := provider.AllBuilt.Load()
	code, body := post("/api/groups/save",
		`{"id":"g1","from":"g1","name":"G1","members":["a/m1","b/m1"]}`)
	if code != http.StatusOK {
		t.Fatalf("save: %d %s", code, body)
	}
	if built := provider.AllBuilt.Load() - before; built > 4 {
		t.Fatalf("a save built the providers %d times, want a shared build", built)
	}
	var st groupsJSON
	if err := json.Unmarshal([]byte(body), &st); err != nil {
		t.Fatalf("the answer is not the state: %v", err)
	}
	// saved again under a new name and with another member: the answer is
	// the state after that write, not the one before it
	code, body = post("/api/groups/save",
		`{"id":"g1","from":"g1","name":"G1 renamed","members":["a/m1","b/m2","c/m1"]}`)
	if code != http.StatusOK {
		t.Fatalf("save again: %d %s", code, body)
	}
	st = groupsJSON{}
	if err := json.Unmarshal([]byte(body), &st); err != nil {
		t.Fatalf("the answer is not the state: %v", err)
	}
	if !hasGroup(st, "g1", "G1 renamed") {
		t.Fatalf("the answer is stale after a rename: %s", clip(body))
	}
	for _, g := range st.Groups {
		if g.ID == "g1" && len(g.Info) != 3 {
			t.Fatalf("the answer has %d members, want 3: %s", len(g.Info), clip(body))
		}
	}
	// deleted: out of the answer at once
	code, body = post("/api/groups/delete", `{"id":"g1"}`)
	if code != http.StatusOK {
		t.Fatalf("delete: %d %s", code, body)
	}
	st = groupsJSON{}
	if err := json.Unmarshal([]byte(body), &st); err != nil {
		t.Fatalf("the answer is not the state: %v", err)
	}
	if hasGroup(st, "g1", "") {
		t.Fatalf("the answer still holds the deleted group: %s", clip(body))
	}
	// and a provider saved is in the answer that save returns
	code, body = post("/api/provider/save",
		`{"id":"d","name":"D","key":"kd","chat":"http://127.0.0.1:1/v1","models":["m1"],"new":true}`)
	if code != http.StatusOK {
		t.Fatalf("provider save: %d %s", code, body)
	}
	if !strings.Contains(body, `"d"`) {
		t.Fatalf("the answer does not hold the provider just saved: %s", clip(body))
	}
}

// hasGroup says the state carries the group under that name ("" skips it).
func hasGroup(st groupsJSON, id, name string) bool {
	for _, g := range st.Groups {
		if g.ID == id && (name == "" || g.Name == name) {
			return true
		}
	}
	return false
}

func clip(s string) string {
	if len(s) > 400 {
		return s[:400]
	}
	return s
}

// An agent's own field written by a POST — which does not touch the catalog
// (agent.Apply) — is in the answer that POST returns as well: what the
// middleware holds is dropped by the write, and what the agent's files are
// read through (filememo) is looked at again.
func TestAgentWriteAnsweredAsWritten(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)
	t.Setenv("USERPROFILE", home)
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(home, ".config"))
	t.Setenv("XDG_CACHE_HOME", filepath.Join(home, ".cache"))
	t.Setenv("CODEX_HOME", filepath.Join(home, ".codex"))
	if err := os.MkdirAll(filepath.Join(home, ".codex"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(home, ".codex", "config.toml"), []byte("model = \"gpt-5.4\"\n"), 0o600); err != nil {
		t.Fatal(err)
	}
	if err := provider.Save(provider.Provider{ID: "relay", Name: "Relay", Key: "k",
		Chat: "http://127.0.0.1:1/v1", Models: []string{"m1"}}); err != nil {
		t.Fatal(err)
	}
	h := Handler(nil, nil)
	call := func(path, body string) stateJSON {
		t.Helper()
		method := http.MethodPost
		if body == "" {
			method = http.MethodGet
		}
		rec := httptest.NewRecorder()
		h.ServeHTTP(rec, httptest.NewRequest(method, path, strings.NewReader(body)))
		if rec.Code != http.StatusOK {
			t.Fatalf("%s: %d %s", path, rec.Code, rec.Body)
		}
		var s stateJSON
		if err := json.Unmarshal(rec.Body.Bytes(), &s); err != nil {
			t.Fatalf("%s: not the state: %v", path, err)
		}
		return s
	}
	codex := func(s stateJSON) agentJSON {
		t.Helper()
		for _, a := range s.Agents {
			if a.ID == "codex" {
				return a
			}
		}
		t.Fatalf("no codex in %+v", s.Agents)
		return agentJSON{}
	}
	// a read first, as the page makes before it writes
	call("/api/state", "")
	if codex(call("/api/state", "")).Wired {
		t.Fatal("wired before magpie set anything")
	}
	if !codex(call("/api/set", `{"agent":"codex","field":"model","value":"relay/m1"}`)).Wired {
		t.Fatal("the answer to the write is stale: not wired")
	}
	if !codex(call("/api/state", "")).Wired {
		t.Fatal("not wired on the read after")
	}
	if codex(call("/api/set", `{"agent":"codex","field":"model","value":"gpt-5.4"}`)).Wired {
		t.Fatal("the answer to the unwiring write is stale: still wired")
	}
}

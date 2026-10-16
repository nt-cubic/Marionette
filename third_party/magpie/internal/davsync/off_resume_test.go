package davsync

import (
	"context"
	"net/http/httptest"
	"os"
	"path/filepath"
	"testing"

	"github.com/yetone/magpie/internal/backup"
	"github.com/yetone/magpie/internal/profile"
	"github.com/yetone/magpie/internal/provider"
)

// Sync turned off and on again to the same server is not a new computer
// (#939): what was changed here while it was off goes up, rather than the
// server's older setup coming down over it — the agents reset to how they
// were when it was turned off.
func TestOffAndOnAgainKeepsChangesHere(t *testing.T) {
	fake := &fakeDAV{files: map[string][]byte{}, etags: map[string]string{}, dirs: map[string]bool{"/dav": true}}
	srv := httptest.NewServer(fake)
	defer srv.Close()
	ctx := context.Background()
	newComputer(t).use(t)
	codex := func(model string) {
		dir := filepath.Join(os.Getenv("CODEX_HOME"))
		os.MkdirAll(dir, 0o755)
		if err := os.WriteFile(filepath.Join(dir, "config.toml"), []byte("model = \""+model+"\"\n"), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	codex("gpt-old")
	if profile.Fields()["codex.model"] != "gpt-old" {
		t.Skipf("Codex isn't seen here: %v", profile.Fields())
	}
	provider.Save(provider.Provider{ID: "deepseek", Name: "DeepSeek", Chat: "https://api.deepseek.com/v1", Key: "k1"})
	cfg := Config{URL: srv.URL + "/dav/", User: "me", Password: "pw", Passphrase: "correct horse", Keys: true, Agents: true}
	if err := Configure(cfg); err != nil {
		t.Fatal(err)
	}
	if err := Now(ctx); err != nil {
		t.Fatal(err)
	}

	if err := Off(); err != nil {
		t.Fatal(err)
	}
	// changed while off
	codex("gpt-new")
	provider.Save(provider.Provider{ID: "deepseek", Name: "DeepSeek 2", Chat: "https://api.deepseek.com/v1", Key: "k1"})

	if err := Configure(cfg); err != nil {
		t.Fatal(err)
	}
	if err := Now(ctx); err != nil {
		t.Fatal(err)
	}
	if m := profile.Fields()["codex.model"]; m != "gpt-new" {
		t.Fatalf("Codex's model went back to %q", m)
	}
	if ps, _ := provider.Stored(); len(ps) != 1 || ps[0].Name != "DeepSeek 2" {
		t.Fatalf("the provider went back: %+v", ps)
	}
	remote, err := backup.Open(fake.files["/dav/magpie/magpie.magpie-backup"], "correct horse")
	if err != nil {
		t.Fatal(err)
	}
	if remote.Agents["codex.model"] != "gpt-new" || remote.Providers[0].Name != "DeepSeek 2" {
		t.Fatalf("not pushed: %v %+v", remote.Agents["codex.model"], remote.Providers)
	}
	if st := Status(); st.Notice != nil {
		t.Fatalf("a notice from before it was turned off: %+v", st.Notice)
	}
}

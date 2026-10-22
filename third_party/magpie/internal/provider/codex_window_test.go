package provider

import (
	"os"
	"path/filepath"
	"testing"

	"github.com/yetone/magpie/internal/catalog"
)

// netube on Discord: a Codex Plus account's gpt-5.5 said 922K, models.dev's
// input limit for the id (OpenAI's API's), where Codex holds it to the 272K
// its ChatGPT list gives. Before the subscription's own list is fetched,
// its models are Codex CLI's models_cache.json's, and their window is that
// list's: on the Providers and Routing pages (WindowOf), in the catalog
// agents are told (Entry.Context) and in CodexWindow. A provider on
// OpenAI's API serving the same id keeps models.dev's.
func TestCodexModelsTakeCodexWindow(t *testing.T) {
	home := signIn(t)
	cache := catalog.CachePath()
	os.MkdirAll(filepath.Dir(cache), 0o755)
	if err := os.WriteFile(cache, []byte(`{"openai":{"id":"openai","models":{
	  "gpt-5.5":{"id":"gpt-5.5","limit":{"context":1050000,"input":922000,"output":128000}}}}}`), 0o644); err != nil {
		t.Fatal(err)
	}
	catalog.Reset()
	t.Cleanup(catalog.Reset)
	if err := os.WriteFile(filepath.Join(home, ".codex", "models_cache.json"), []byte(`{"etag":"\"e1\"","client_version":"0.159.0","models":[
	  {"slug":"gpt-5.5","display_name":"GPT-5.5","priority":1,"visibility":"list","context_window":272000,"max_context_window":272000}]}`), 0o644); err != nil {
		t.Fatal(err)
	}
	ForgetAccounts()
	t.Cleanup(ForgetAccounts)

	p, ok := codexAccount(home)
	if !ok {
		t.Fatal("no Codex sign-in")
	}
	var got int
	for _, m := range p.Available() {
		if m.ID == "gpt-5.5" {
			got = p.WindowOf(m)
		}
	}
	if got != 272000 {
		t.Errorf("Codex subscription's gpt-5.5 window %d, want Codex's 272000, not models.dev's 922000", got)
	}
	if n := CodexWindow("", "gpt-5.5"); n != 272000 {
		t.Errorf("CodexWindow(gpt-5.5) = %d, want 272000", n)
	}
	if n := CodexWindow("", "gpt-unlisted"); n != 0 {
		t.Errorf("CodexWindow(gpt-unlisted) = %d, want 0: no list says", n)
	}
	for _, e := range Catalog() {
		if e.Provider.ID == "codex" && e.Model == "gpt-5.5" && e.Context != 272000 {
			t.Errorf("catalog entry %s told agents %d, want 272000", e.ID, e.Context)
		}
	}

	// the same id on OpenAI's API is models.dev's, as before
	oai := Provider{ID: "oai", Name: "OAI", Key: "k", Catalog: "openai", Responses: "https://api.example.invalid/v1"}
	got = 0
	for _, m := range oai.Available() {
		if m.ID == "gpt-5.5" {
			got = oai.WindowOf(m)
		}
	}
	if got != 922000 {
		t.Errorf("OpenAI API gpt-5.5 window %d, want models.dev's 922000", got)
	}
}

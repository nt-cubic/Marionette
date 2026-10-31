package plugin

import (
	"context"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"sync"
	"testing"
)

// A repository of gateway hooks alone, package.json's magpie.hooks with
// no main, exports or index.js, installs and runs, but the GitHub list
// took it for one whose index.js Bun loads, found none, and left it out
// without a word (MengXiangxi, #3821). It is listed now, a request hook,
// when its hooks file is in the repository; one whose hooks file isn't
// (a dist built only to publish) couldn't be installed from it, and is
// left out, as a middleware's is. The first package.json is
// IvanXxxxyyuffff/magpie-system-prompt-file's, one the report names, as
// it is; neither package is on npm.
func TestTaggedHooksAlone(t *testing.T) {
	var mu sync.Mutex
	var asked []string
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, q *http.Request) {
		mu.Lock()
		asked = append(asked, q.URL.Path)
		mu.Unlock()
		name, ok := strings.CutPrefix(q.URL.Path, "/raw/")
		f, err := os.ReadFile(filepath.Join("testdata/github/raw", filepath.FromSlash(name)))
		if !ok || err != nil {
			http.Error(w, `"Not Found"`, 404)
			return
		}
		w.Write(f)
	}))
	t.Cleanup(srv.Close)
	api, raw, reg := githubAPI, githubRaw, npmRegistry
	githubAPI, githubRaw, npmRegistry = srv.URL, srv.URL+"/raw", srv.URL+"/npm"
	t.Cleanup(func() { githubAPI, githubRaw, npmRegistry = api, raw, reg })

	read := func(repo string) (Tagged, bool, bool, error) {
		mu.Lock()
		asked = nil
		mu.Unlock()
		return readTagged(context.Background(), Tagged{Spec: "github:" + repo, Repo: repo}, "main")
	}
	got, keep, known, err := read("IvanXxxxyyuffff/magpie-system-prompt-file")
	if !keep || !known || err != nil || got.Kind != "hooks" || got.Spec != "github:IvanXxxxyyuffff/magpie-system-prompt-file" ||
		got.Package != "magpie-system-prompt-file" || got.Version != "0.3.0" {
		t.Fatalf("hooks alone: %+v, kept %v, known %v, %v", got, keep, known, err)
	}
	// its hooks file is the one looked for, not an index.js it needn't have
	mu.Lock()
	a := slices.Clone(asked)
	mu.Unlock()
	if !slices.Contains(a, "/raw/IvanXxxxyyuffff/magpie-system-prompt-file/main/system-prompt.hooks.mjs") || slices.ContainsFunc(a, func(p string) bool { return strings.HasSuffix(p, "/index.js") }) {
		t.Fatalf("asked %v", a)
	}

	got, keep, known, err = read("someone/hooks-gone")
	if keep || !known || err != nil {
		t.Fatalf("hooks whose file isn't committed: %+v, kept %v, known %v, %v", got, keep, known, err)
	}
}

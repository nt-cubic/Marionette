package plugin

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"net/url"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"
)

// fakeGitHub answers as GitHub's search, raw.githubusercontent.com and npm
// do, from testdata/github (npm's real answers for the packages it has): search.json is a real answer for a topic (five
// repositories), with three more items made from its first — a fork, an
// archived one and magpie-community's — and someone/word-guard, whose
// package.json is word-guard's own, a middleware alone; someone/agent-aider,
// magpie-community's agent-aider package.json and module, an agent alone;
// someone/page-glance, a page alone (internal/gui/testdata/page-plugin's
// package.json), and someone/page-gone, whose page file isn't there; alfaoz's
// opencode-see-image (real), whose main is a dist the repository hasn't;
// and someone/flaky-entry, whose main GitHub doesn't answer for.
// reads counts what fakeGitHub answered other than searches: package.json,
// npm and entry files.
var reads atomic.Int32

// held is a path fakeGitHub doesn't answer until its channel closes: a
// read through a slow proxy.
type held struct {
	path string
	go_  chan struct{}
}

var holding atomic.Pointer[held]

func fakeGitHub(t *testing.T, up *atomic.Bool, asked *atomic.Int32) {
	t.Helper()
	t.Setenv("XDG_CACHE_HOME", t.TempDir())
	t.Setenv("MAGPIE_PLUGIN_MARKET", "")
	b, err := os.ReadFile("testdata/github/search.json")
	if err != nil {
		t.Fatal(err)
	}
	var r map[string]any
	if err := json.Unmarshal(b, &r); err != nil {
		t.Fatal(err)
	}
	items := r["items"].([]any)
	like := func(full string, set map[string]any) map[string]any {
		var it map[string]any
		raw, _ := json.Marshal(items[0])
		_ = json.Unmarshal(raw, &it)
		it["full_name"], it["html_url"] = full, "https://github.com/"+full
		it["owner"].(map[string]any)["login"] = strings.Split(full, "/")[0]
		for k, v := range set {
			it[k] = v
		}
		return it
	}
	items = append(items,
		like("someone/word-guard", map[string]any{"description": "word guard", "stargazers_count": 3, "license": nil}),
		like("someone/agent-aider", nil),
		like("someone/page-glance", nil),
		like("someone/page-gone", nil),
		like("alfaoz/opencode-see-image", nil),
		like("someone/flaky-entry", map[string]any{"license": map[string]any{"key": "other", "spdx_id": "NOASSERTION"}}),
		like("cyberElar/magpie-x-search", nil),
		like("forker/opencode-claude-auth", map[string]any{"fork": true}),
		like("old/opencode-old", map[string]any{"archived": true}),
		like("magpie-community/plugins", nil))
	// one page of the search, as GitHub cuts it
	search := func(q url.Values) []byte {
		per, _ := strconv.Atoi(q.Get("per_page"))
		page, _ := strconv.Atoi(q.Get("page"))
		from, to := min(max(page-1, 0)*per, len(items)), len(items)
		if per > 0 {
			to = min(from+per, len(items))
		}
		b, _ := json.Marshal(map[string]any{"total_count": len(items), "incomplete_results": false, "items": items[from:to]})
		return b
	}
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, q *http.Request) {
		if q.URL.Path != "/search/repositories" {
			reads.Add(1)
		}
		if h := holding.Load(); h != nil && q.URL.Path == h.path {
			<-h.go_
		}
		if !up.Load() || q.URL.Path == "/raw/someone/flaky-entry/main/index.js" {
			http.Error(w, "down", 503)
			return
		}
		if q.URL.Path == "/search/repositories" {
			asked.Add(1)
			if got := q.URL.Query().Get("q"); got != "topic:magpie-plugin archived:false fork:false" {
				t.Errorf("searched %q", got)
			}
			w.Write(search(q.URL.Query()))
			return
		}
		if name, ok := strings.CutPrefix(q.URL.Path, "/npm/"); ok {
			f, err := os.ReadFile(filepath.Join("testdata/github/npm", strings.ReplaceAll(strings.TrimSuffix(name, "/latest"), "/", "_")+".json"))
			if err != nil {
				http.Error(w, `"Not Found"`, 404)
				return
			}
			w.Write(f)
			return
		}
		f, err := os.ReadFile(filepath.Join("testdata/github/raw", filepath.FromSlash(strings.TrimPrefix(q.URL.Path, "/raw/"))))
		if err != nil {
			http.NotFound(w, q)
			return
		}
		w.Write(f)
	}))
	t.Cleanup(srv.Close)
	api, raw, reg := githubAPI, githubRaw, npmRegistry
	githubAPI, githubRaw, npmRegistry = srv.URL, srv.URL+"/raw", srv.URL+"/npm"
	forget := func() {
		taggedMu.Lock()
		taggedL = nil
		taggedMu.Unlock()
		readMu.Lock()
		clear(readRepo)
		readMu.Unlock()
	}
	forget()
	reads.Store(0)
	t.Cleanup(func() { githubAPI, githubRaw, npmRegistry = api, raw, reg; forget() })
}

// Repositories tagged magpie-plugin: each with what its package.json says,
// in GitHub's order; forks, archived ones and magpie-community's left out.
// Asked once in six hours; with GitHub down, what it said last.
// Every page of the search is read: with a page short of what is tagged,
// the list is the same as from one page, in GitHub's order (HoGee_xxl on X:
// 59 tagged, and the 9 past GitHub's first page of 50 never showed).
func TestTaggedReposEveryPage(t *testing.T) {
	var up atomic.Bool
	var asked atomic.Int32
	up.Store(true)
	fakeGitHub(t, &up, &asked)
	was := taggedPage
	taggedPage = 4
	t.Cleanup(func() { taggedPage = was })

	var repos []string
	for _, x := range TaggedRepos(context.Background()) {
		repos = append(repos, x.Repo)
	}
	want := "rynfar/meridian griffinmartin/opencode-claude-auth slkiser/opencode-quota someone/word-guard someone/agent-aider someone/page-glance someone/flaky-entry"
	if strings.Join(repos, " ") != want {
		t.Fatalf("repos %v\nwant %s", repos, want)
	}
	// 15 items, four to a page
	if n := asked.Load(); n != 4 {
		t.Fatalf("asked %d pages, want 4", n)
	}
}

func TestTaggedRepos(t *testing.T) {
	var up atomic.Bool
	var asked atomic.Int32
	up.Store(true)
	fakeGitHub(t, &up, &asked)
	ctx := context.Background()

	l := TaggedRepos(ctx)
	var repos []string
	for _, x := range l {
		repos = append(repos, x.Repo)
	}
	// iPolloWork and learn-opencode (a workspace and a course) have no
	// index.js to load, nor see-image its dist: installed from GitHub,
	// none would load. magpie-x-search (real, #1327) is on npm from its
	// repository, but a command alone, an MCP server: npm's copy names no
	// file to load, and it has no index.js
	want := "rynfar/meridian griffinmartin/opencode-claude-auth slkiser/opencode-quota someone/word-guard someone/agent-aider someone/page-glance someone/flaky-entry"
	if strings.Join(repos, " ") != want {
		t.Fatalf("repos %v\nwant %s", repos, want)
	}
	byRepo := map[string]Tagged{}
	for _, x := range l {
		byRepo[x.Repo] = x
	}
	c := byRepo["griffinmartin/opencode-claude-auth"]
	if c.Spec != "opencode-claude-auth" || c.Package != "opencode-claude-auth" || c.Version != "2.2.1" ||
		c.Owner != "griffinmartin" || c.License != "MIT" || c.Stars != 1301 || c.Kind != "" || c.Pushed.IsZero() || c.URL != "https://github.com/griffinmartin/opencode-claude-auth" {
		t.Fatalf("claude auth: %+v", c)
	}
	if x := byRepo["someone/flaky-entry"]; x.License != "" {
		t.Fatalf("NOASSERTION is no license: %+v", x)
	}
	// its own picture, package.json's magpie.icon, as an installed
	// plugin's provider has it; none when it gives none
	if x := byRepo["someone/flaky-entry"]; !strings.HasPrefix(x.Icon, "data:image/svg+xml;base64,") {
		t.Fatalf("magpie.icon: %q", x.Icon)
	}
	if c.Icon != "" {
		t.Fatalf("no magpie.icon, yet %q", c.Icon)
	}
	if x := byRepo["someone/agent-aider"]; x.Kind != "agent" || x.Spec != "github:someone/agent-aider" {
		t.Fatalf("an agent alone: %+v", x)
	}
	// a page alone: no module, magpie.page's file in the repository; one
	// whose file isn't there would show nothing, and isn't listed
	if x := byRepo["someone/page-glance"]; x.Kind != "page" || x.Spec != "github:someone/page-glance" || x.Package != "magpie-usage-glance" {
		t.Fatalf("a page alone: %+v", x)
	}
	if x := byRepo["someone/word-guard"]; x.Kind != "middleware" || x.Package != "@magpie-community/middleware-word-guard" || x.License != "" {
		t.Fatalf("a middleware alone: %+v", x)
	}
	// npm's copy when npm has it from this very repository (its build is
	// on npm only); else the repository: not on npm, or npm's is another's
	for repo, spec := range map[string]string{
		"rynfar/meridian": "@rynfar/meridian", "slkiser/opencode-quota": "@slkiser/opencode-quota",
		"someone/word-guard": "github:someone/word-guard", "someone/flaky-entry": "github:someone/flaky-entry",
	} {
		if x := byRepo[repo]; x.Spec != spec {
			t.Errorf("%s installs as %q, want %q", repo, x.Spec, spec)
		}
	}
	if x := byRepo["someone/word-guard"]; !IsGit(x.Spec) {
		t.Fatalf("%s isn't a spec Add installs from git", x.Spec)
	}

	TaggedRepos(ctx)
	if n := asked.Load(); n != 1 {
		t.Fatalf("asked %d times in ten minutes", n)
	}

	// ten minutes on: GitHub's search asked again, so a repository tagged
	// since shows; the repositories it read already aren't read again,
	// but for flaky-entry, whose entry GitHub didn't answer for
	taggedMu.Lock()
	taggedAt = time.Now().Add(-taggedTTL - time.Second)
	taggedMu.Unlock()
	before := reads.Load()
	if l := TaggedRepos(ctx); len(l) != 7 || asked.Load() != 2 {
		t.Fatalf("ten minutes on: asked %d times, %d repos", asked.Load(), len(l))
	}
	if n := reads.Load() - before; n != 3 {
		t.Fatalf("ten minutes on: %d reads, want flaky-entry's package.json, npm and entry again", n)
	}
	asked.Store(1)

	// started again within the six hours: the copy on disk, GitHub not
	// asked
	forget := func() { taggedMu.Lock(); taggedL = nil; taggedMu.Unlock() }
	forget()
	if err := os.Chtimes(taggedCache(), time.Now(), time.Now()); err != nil {
		t.Fatal(err)
	}
	if l := TaggedRepos(ctx); len(l) != 7 || asked.Load() != 1 {
		t.Fatalf("restarted: asked %d times, %d repos", asked.Load(), len(l))
	}

	// started again later, with GitHub down: the copy on disk all the same
	old := time.Now().Add(-7 * time.Hour)
	if err := os.Chtimes(taggedCache(), old, old); err != nil {
		t.Fatal(err)
	}
	up.Store(false)
	forget()
	if l := TaggedRepos(ctx); len(l) != 7 || l[1].Package != "opencode-claude-auth" || asked.Load() != 1 {
		t.Fatalf("down: %+v", l)
	}

	// nothing ever kept, GitHub down: none, not an error
	t.Setenv("XDG_CACHE_HOME", t.TempDir())
	taggedMu.Lock()
	taggedL = nil
	taggedMu.Unlock()
	if l := TaggedRepos(ctx); l == nil || len(l) != 0 {
		t.Fatalf("down, nothing kept: %#v", l)
	}

	// off: GitHub never asked
	up.Store(true)
	t.Setenv("MAGPIE_PLUGIN_MARKET", "off")
	taggedMu.Lock()
	taggedL = nil
	taggedMu.Unlock()
	before = asked.Load()
	if l := TaggedRepos(ctx); len(l) != 0 || asked.Load() != before {
		t.Fatalf("off: %+v", l)
	}
}

// A repository whose package.json comes after the page stops waiting (a
// slow proxy: larchsis on X) is as the list had it last, not left out; the
// list is asked again soon, not in ten minutes; and the read, finished
// meanwhile, is what the next ask has.
func TestTaggedReposSlowRead(t *testing.T) {
	var up atomic.Bool
	var asked atomic.Int32
	up.Store(true)
	fakeGitHub(t, &up, &asked)
	ctx := context.Background()
	if l := TaggedRepos(ctx); len(l) != 7 {
		t.Fatalf("%d repos", len(l))
	}

	// magpie started again eleven minutes on, word-guard's package.json
	// slow to come
	wait, retry := taggedWait, taggedRetry
	taggedWait, taggedRetry = 300*time.Millisecond, time.Second
	t.Cleanup(func() { taggedWait, taggedRetry = wait, retry })
	h := &held{path: "/raw/someone/word-guard/main/package.json", go_: make(chan struct{})}
	holding.Store(h)
	let := sync.OnceFunc(func() { close(h.go_) })
	t.Cleanup(func() { let(); holding.Store(nil) })
	taggedMu.Lock()
	taggedL = nil
	taggedMu.Unlock()
	readMu.Lock()
	clear(readRepo)
	readMu.Unlock()
	old := time.Now().Add(-taggedTTL - time.Minute)
	if err := os.Chtimes(taggedCache(), old, old); err != nil {
		t.Fatal(err)
	}
	has := func(l []Tagged, repo string) *Tagged {
		for i := range l {
			if l[i].Repo == repo {
				return &l[i]
			}
		}
		return nil
	}
	cutAsked := time.Now()
	l := TaggedRepos(ctx)
	cutDone := time.Now()
	if x := has(l, "someone/word-guard"); len(l) != 7 || x == nil || x.Kind != "middleware" {
		t.Fatalf("slow package.json: %d repos, word-guard %+v", len(l), x)
	}
	if n := asked.Load(); n != 2 {
		t.Fatalf("asked %d times", n)
	}
	// the list cut short is held for taggedRetry from when it was asked,
	// no more and no less: bounds from the clock around the ask, so no
	// slack a loaded runner could outgrow (#1889)
	taggedMu.Lock()
	until := taggedAt.Add(taggedTTL)
	taggedMu.Unlock()
	if until.Before(cutAsked.Add(taggedRetry)) || until.After(cutDone.Add(taggedRetry)) {
		t.Fatalf("a list cut short held till %v, asked between %v and %v", until, cutAsked, cutDone)
	}
	// nor is the list cut short kept ten minutes by a magpie started again
	if fi, err := os.Stat(taggedCache()); err != nil || time.Since(fi.ModTime()) < taggedTTL-taggedRetry-time.Second {
		t.Fatalf("the copy on disk stands till %v", fi.ModTime().Add(taggedTTL))
	}

	// the read comes in after the page stopped waiting, and is kept
	let()
	key := ""
	for deadline := time.Now().Add(10 * time.Second); ; {
		readMu.Lock()
		for k, v := range readRepo {
			if strings.HasPrefix(k, "someone/word-guard@") && v != nil {
				key = k
			}
		}
		readMu.Unlock()
		if key != "" {
			break
		}
		if time.Now().After(deadline) {
			t.Fatal("word-guard's late read was never kept")
		}
		time.Sleep(10 * time.Millisecond)
	}

	// within the ten minutes, but not yet past taggedRetry: not asked
	// again, and word-guard's package.json not read again. The poll for
	// the late read can take longer than taggedRetry on a loaded runner
	// (once it did, at load ~28, #1889), and a list cut short is then
	// asked again rightly; that it is held for taggedRetry is checked
	// above, against the clock around the ask. Here it is marked as cut
	// short now, as TaggedRepos marks one, and must not be asked again.
	taggedMu.Lock()
	taggedAt = time.Now().Add(-taggedTTL + taggedRetry)
	taggedMu.Unlock()
	TaggedRepos(ctx)
	if n := asked.Load(); n != 2 {
		t.Fatalf("asked again at once: %d", n)
	}
	time.Sleep(taggedRetry + 100*time.Millisecond)
	before := reads.Load()
	l = TaggedRepos(ctx)
	if n := asked.Load(); n != 3 {
		t.Fatalf("a list cut short kept: asked %d times", n)
	}
	if x := has(l, "someone/word-guard"); len(l) != 7 || x == nil || x.Package != "@magpie-community/middleware-word-guard" {
		t.Fatalf("asked again: %d repos, word-guard %+v", len(l), x)
	}
	// flaky-entry's package.json, npm and entry, as ten minutes on
	if n := reads.Load() - before; n != 3 {
		t.Fatalf("asked again: %d reads", n)
	}
	taggedMu.Lock()
	at := taggedAt
	taggedMu.Unlock()
	if time.Since(at) > time.Second {
		t.Fatalf("a whole list kept only till %v", at.Add(taggedTTL))
	}
}

// A tagged repository's page, before it is installed, is its README on
// GitHub; installed, the folder's.
func TestReadmeOfTaggedRepo(t *testing.T) {
	var up atomic.Bool
	var asked atomic.Int32
	up.Store(true)
	fakeGitHub(t, &up, &asked)
	t.Setenv("XDG_CONFIG_HOME", t.TempDir())
	p, err := Readme(context.Background(), "github:griffinmartin/opencode-claude-auth")
	if err != nil || !strings.Contains(p.Readme, "opencode-claude-auth") {
		t.Fatalf("readme %q, %v", p.Readme[:min(80, len(p.Readme))], err)
	}
	if _, err := Readme(context.Background(), "github:rynfar/meridian"); err == nil {
		t.Fatal("a repository with no README read as one")
	}
	for spec, want := range map[string]string{
		"github:a/b": "a/b", "github:a/b#v1": "a/b", "https://github.com/a/b": "a/b", "https://github.com/a/b.git": "a/b",
		"https://github.com/a/b/tree/main": "a/b", "gitlab:a/b": "", "github:a": "",
	} {
		if got := githubRepo(spec); got != want {
			t.Errorf("githubRepo(%q) = %q, want %q", spec, got, want)
		}
	}
}

// The file importing a package loads, as Bun resolves it.
func TestPkgEntry(t *testing.T) {
	for _, c := range []struct{ exports, main, want string }{
		{"", "", "index.js"},
		{"", "./dist/index.js", "./dist/index.js"},
		{`"./x.js"`, "./dist/index.js", "./x.js"},
		{`{".": "./a.js", "./b": "./b.js"}`, "", "./a.js"},
		{`{".": {"types": "./a.d.ts", "import": "./a.mjs", "require": "./a.cjs"}}`, "", "./a.mjs"},
		{`{"import": "./a.mjs", "default": "./a.js"}`, "", "./a.mjs"},
		{`{"./server": "./s.js"}`, "./m.js", "./m.js"},
	} {
		if got := pkgEntry(json.RawMessage(c.exports), c.main); got != c.want {
			t.Errorf("pkgEntry(%s, %q) = %q, want %q", c.exports, c.main, got, c.want)
		}
	}
}

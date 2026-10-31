package plugin

import (
	"context"
	"net/url"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/settings"
)

// jiakun_zhao on 𝕏 found page_<hash>.html and magpie_*.md files after
// using the chat page plugin, which starts the user's MCP servers from
// the plugin host and runs their tools for a model. A program a plugin
// starts with no folder of its own ran in the host's, magpie's config
// folder, so a tool's file saved by a relative name went beside
// settings.json, and one named providers.json would write over magpie's.
// It runs in plugin-work under that folder, with PWD to match; a folder
// the plugin gives is kept.
func TestPluginProgramsRunInTheirOwnFolder(t *testing.T) {
	sandbox(t)
	ctx, cancel := context.WithTimeout(context.Background(), time.Minute)
	defer cancel()
	abs, _ := filepath.Abs("testdata/clicwd/index.js")
	if _, err := Add(ctx, abs); err != nil {
		t.Fatal(err)
	}
	a, err := Authorize(ctx, "clicwd", 0, nil, NewAccount)
	if err != nil {
		t.Fatal(err)
	}
	u, err := url.Parse(a.URL)
	if err != nil {
		t.Fatal(err)
	}
	q := u.Query()
	want := filepath.Join(settings.Dir(), "plugin-work")
	if st, err := os.Stat(want); err != nil || !st.IsDir() {
		t.Fatalf("%s is not a folder: %v", want, err)
	}
	same := func(a, b string) bool {
		ea, _ := filepath.EvalSymlinks(a)
		eb, _ := filepath.EvalSymlinks(b)
		return ea != "" && strings.EqualFold(ea, eb)
	}
	for _, k := range []string{"spawned", "withEnv", "sync", "bun"} {
		cwd, pwd, _ := strings.Cut(q.Get(k), "\n")
		if !same(cwd, want) {
			t.Errorf("%s: a program the plugin started with no folder ran in %q, want %q", k, cwd, want)
		}
		if runtime.GOOS != "windows" && !same(pwd, want) {
			t.Errorf("%s: its PWD is %q, want %q", k, pwd, want)
		}
	}
	if cwd, _, _ := strings.Cut(q.Get("own"), "\n"); !same(cwd, q.Get("ownTarget")) {
		t.Errorf("a program started in %q ran in %q", q.Get("ownTarget"), cwd)
	}
}
